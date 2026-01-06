use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use billing_core::UsageEvent;
use billing_db::{create_client, init_db};
use dashmap::DashMap;
use dotenvy::dotenv;
use ingestion_api::worker::start_event_worker;
use metrics::{counter, gauge};
use metrics_exporter_prometheus::PrometheusBuilder;
use std::sync::Arc;
use tokio::{main, signal::ctrl_c, sync::mpsc};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct AppState {
    pub tx: mpsc::Sender<UsageEvent>,
    pub local_cache: Arc<DashMap<String, i64>>,
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

    PrometheusBuilder::new()
        .with_http_listener(([0, 0, 0, 0], 8000))
        .install()
        .expect("Prometheus failed");

    let state = AppState {
        tx,
        local_cache: Arc::new(DashMap::with_capacity(100_000)),
    };

    let cache_for_janitor = state.local_cache.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            let now = chrono::Utc::now().timestamp();
            cache_for_janitor.retain(|_, ts| *ts > now - 600);
            gauge!("local_cache_size").set(cache_for_janitor.len() as f64);
        }
    });

    let worker_token = token.clone();
    tokio::spawn(async move {
        start_event_worker(rx, clickhouse_client, redis_mgr, worker_token).await;
    });

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
    payload.timestamp = chrono::Utc::now().timestamp();

    match state.local_cache.entry(payload.idempotency_key.clone()) {
        dashmap::mapref::entry::Entry::Occupied(_) => {
            counter!("api_duplicate_request_total", "source" => "local").increment(1);
            return (StatusCode::ACCEPTED, "Duplicate").into_response();
        }
        dashmap::mapref::entry::Entry::Vacant(entry) => {
            entry.insert(payload.timestamp);
        }
    }

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
