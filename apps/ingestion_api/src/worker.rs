use billing_core::UsageEvent;
use clickhouse::{Client, insert};
use metrics::{counter, gauge, histogram};
use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, Semaphore, mpsc};
use tokio::time::{Duration, interval};
use tokio_util::sync::CancellationToken;

pub async fn start_event_worker(
    rx: Arc<Mutex<mpsc::Receiver<UsageEvent>>>,
    client: Client,
    mut redis: ConnectionManager,
    worker_token: CancellationToken,
    semaphore: Arc<Semaphore>,
    worker_id: usize,
) {
    let mut buffer = Vec::with_capacity(5000);
    let mut flush_interval = interval(Duration::from_millis(500));

    loop {
        tokio::select! {
            _ = worker_token.cancelled() => {
                println!("Worker {}: Final flush...", worker_id);
                let mut rx_lock = rx.lock().await;
                while let Ok(event) = rx_lock.try_recv() {
                    buffer.push(event);
                }
                process_and_flush(client.clone(), &mut redis, &mut buffer, semaphore.clone()).await;
                return;
            }

            maybe_event = async {
                let mut rx_lock = rx.lock().await;
                rx_lock.recv().await
            } => {
                if let Some(event) = maybe_event {
                    buffer.push(event);
                    
                    let current_occupancy = 100_000 - rx.lock().await.capacity();
                    gauge!("mpsc_buffer_usage").set(current_occupancy as f64);

                    if buffer.len() >= 5000 {
                        process_and_flush(client.clone(), &mut redis, &mut buffer, semaphore.clone()).await;
                    }
                }
            }

            _ = flush_interval.tick() => {
                if !buffer.is_empty() {
                    process_and_flush(client.clone(), &mut redis, &mut buffer, semaphore.clone()).await;
                }
            }
        }
    }
}

async fn process_and_flush(
    client: Client,
    redis: &mut ConnectionManager,
    buffer: &mut Vec<UsageEvent>,
    semaphore: Arc<Semaphore>,
) {
    if buffer.is_empty() { return; }

    histogram!("worker_batch_size").record(buffer.len() as f64);

    let keys: Vec<String> = buffer.iter().map(|e| e.idempotency_key.clone()).collect();
    let results: Vec<Option<String>> = match redis.mget(&keys).await {
        Ok(res) => res,
        Err(e) => {
            eprintln!("Redis MGET error: {:?}", e);
            vec![None; keys.len()]
        }
    };

    let mut unique_events = Vec::with_capacity(buffer.len());
    let mut pipe = redis::pipe();
    let mut has_new_keys = false;

    for (i, result) in results.iter().enumerate() {
        if result.is_none() {
            let event = &buffer[i];
            unique_events.push(event.clone());
            pipe.set_ex(&event.idempotency_key, 1, 86400);
            has_new_keys = true;
        } else {
            counter!("api_duplicate_request_total", "source" => "redis").increment(1);
        }
    }

    if has_new_keys {
        let _: redis::RedisResult<()> = pipe.query_async(redis).await;
    }

    if !unique_events.is_empty() {
        let permit = semaphore.acquire_owned().await.expect("Semaphore closed");
        
        tokio::spawn(async move {
            let _permit = permit;
            if let Err(e) = flush_with_retry(&client, &mut unique_events, 5).await {
                eprintln!("Clickhouse Critical Error: {:?}", e);
            }
        });
    }

    buffer.clear();
}

async fn flush_with_retry(
    client: &Client,
    events: &mut Vec<UsageEvent>,
    max_retries: u8,
) -> Result<(), clickhouse::error::Error> {
    let mut attempts = 0;
    let start = Instant::now();

    while attempts < max_retries {
        let mut insert: insert::Insert<UsageEvent> = client.insert("usage_events").await?;
        for event in events.iter() {
            insert.write(event).await?;
        }

        match insert.end().await {
            Ok(_) => {
                histogram!("clickhouse_flush_duration_seconds")
                    .record(start.elapsed().as_secs_f64());
                counter!("events_flushed_to_clickhouse").increment(events.len() as u64);
                events.clear();
                return Ok(());
            }
            Err(e) => {
                attempts += 1;
                counter!("clickhouse_flush_failures").increment(1);
                if attempts >= max_retries {
                    return Err(e);
                }
                tokio::time::sleep(Duration::from_millis(100 * attempts as u64)).await;
            }
        }
    }
    Ok(())
}
