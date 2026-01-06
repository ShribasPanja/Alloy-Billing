use std::env;

use clickhouse::Client;
use redis::{AsyncCommands, SetOptions, aio::ConnectionManager};

pub fn create_client() -> Client {
    Client::default()
        .with_url(env::var("click_house_url").unwrap_or("http://localhost:8123".to_string()))
        .with_user(env::var("click_house_user").unwrap())
        .with_password(env::var("click_house_password").unwrap())
        .with_database("alloy_billing")
        .with_option("async_insert", "1")
        .with_option("wait_for_async_insert", "0")
}

pub async fn init_db(client: &Client) -> Result<(), clickhouse::error::Error> {
    let create_table_sql = r#"
        CREATE TABLE IF NOT EXISTS usage_events (
            event_id UUID,
            customer_id String,
            event_type String,
            amount UInt64,
            idempotency_key String,
            timestamp Int64
        ) 
        ENGINE = MergeTree()
        PRIMARY KEY (customer_id, timestamp)
        ORDER BY (customer_id, timestamp);
    "#;

    client.query(create_table_sql).execute().await?;
    println!("ClickHouse: usage_events table is ready.");

    Ok(())
}

pub async fn is_duplicate(mut redis_conn: ConnectionManager, key: &str) -> bool {
    let result: Option<String> = redis_conn
        .set_options(
            key,
            "1",
            SetOptions::default()
                .conditional_set(redis::ExistenceCheck::NX)
                .with_expiration(redis::SetExpiry::EX(8600)),
        )
        .await
        .ok();
    result.is_none()
}
