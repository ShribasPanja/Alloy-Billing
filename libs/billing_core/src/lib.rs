use clickhouse::Row;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize, Clone, Row)]
pub struct UsageEvent {
    #[serde(with = "clickhouse::serde::uuid")]
    pub event_id: Uuid,
    pub customer_id: String,
    pub event_type: String,
    pub amount: u64,
    pub idempotency_key: String,
    pub timestamp: i64,
}

// #[derive(Debug, Clone, Serialize, Deserialize)]
// pub enum EventType {
//     ApiCall,
//     DataStorage
// }

impl UsageEvent {
    pub fn new(
        customer_id: String,
        event_type: String,
        amount: u64,
        idempotency_key: String,
    ) -> Self {
        UsageEvent {
            event_id: Uuid::new_v4(),
            customer_id,
            event_type,
            amount,
            idempotency_key,
            timestamp: chrono::Utc::now().timestamp_millis(),
        }
    }
}
