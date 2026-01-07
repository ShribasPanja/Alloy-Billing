use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use billing_core::UsageEvent;
use billing_db::{create_client, init_db};
use dotenvy::dotenv;
use ingestion_api::worker::start_event_worker;
use metrics::{counter, gauge};
use metrics_exporter_prometheus::PrometheusBuilder;
use moka::future::Cache;
use std::{sync::Arc, time::Duration};
use tokio::{main, signal::ctrl_c, sync::{Mutex, Semaphore, mpsc}};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct AppState {
    pub tx: mpsc::Sender<UsageEvent>,
    pub local_cache: Cache<String, i64>,
    pub flush_semaphore: Arc<Semaphore>
}

#[main]
async fn main() {
    dotenv().ok();

    let clickhouse_client = create_client();
    init_db(&clickhouse_client)
        .await
        .expect("Clickhouse init failed");

    let client = redis::Client::open(std::env::var("redis_url").unwrap()).expect("Redis URL wrong");
    let redis_mgr = redis::aio::ConnectionManager::new(client)
        .await
        .expect("Redis connection failed");

    let token = CancellationToken::new();
    let (tx, rx) = mpsc::channel::<UsageEvent>(100_000);
    let local_cache = Cache::builder()
        .max_capacity(100_000) // Limits memory usage
        .time_to_live(Duration::from_secs(600)) // Auto-expire after 10 mins
        .build();
    let flush_semaphore = Arc::new(Semaphore::new(50));
    let shared_rx = Arc::new(Mutex::new(rx));

    PrometheusBuilder::new()
        .with_http_listener(([0, 0, 0, 0], 8000))
        .install()
        .expect("Prometheus failed");

    let state = AppState {
        tx,
        local_cache,
        flush_semaphore,
    };

    let num_workers = num_cpus::get();
    println!("⚙️ Spawning {} worker threads...", num_workers);


    for i in 0..num_workers {
        let worker_rx = Arc::clone(&shared_rx);
        let ch_client = clickhouse_client.clone();
        let r_mgr = redis_mgr.clone();
        let worker_token = token.clone();
        let semaphore = Arc::clone(&state.flush_semaphore);

        tokio::spawn(async move {
            start_event_worker(worker_rx, ch_client, r_mgr, worker_token, semaphore, i).await;
        });
    }

    let app = Router::new()
        .route("/", get(|| async { "OK" }))
        .route("/ingest", post(ingest_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    println!("🚀 Ingestion API running on port 3000");

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            ctrl_c().await.expect("Shutdown failed");
            token.cancel();
        })
        .await
        .unwrap();
}

async fn ingest_handler(
    State(state): State<AppState>,
    Json(mut payload): Json<UsageEvent>,
) -> impl IntoResponse {
    counter!("api_request_total").increment(1);
    gauge!("mpsc_buffer_usage").set(state.tx.capacity() as f64);
    gauge!("local_cache_size").set(state.local_cache.entry_count() as f64);

    payload.timestamp = chrono::Utc::now().timestamp();

    if let Some(_) = state.local_cache.get(&payload.idempotency_key).await {
        counter!("api_duplicate_request_total", "source" => "local").increment(1);
        return (StatusCode::ACCEPTED, "Duplicate").into_response();
    }
    
    state.local_cache.insert(payload.idempotency_key.clone(), payload.timestamp).await;

    match state.tx.try_send(payload) {
        Ok(_) => {
            counter!("api_request_processed", "status" => "success").increment(1);
            (StatusCode::ACCEPTED, "transaction accepted").into_response()
        }
        Err(_) => {
            counter!("api_request_processed", "status" => "error").increment(1);
            (StatusCode::SERVICE_UNAVAILABLE, "Buffer Full").into_response()
        }
    }
}
