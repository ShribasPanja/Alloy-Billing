#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    use std::sync::Arc;
    use tokio::sync::mpsc;
    use dashmap::DashMap;

    #[tokio::test]
    async fn test_ingestion_and_local_deduplication() {
        let (tx, mut rx) = mpsc::channel(100);
        let state = AppState {
            tx,
            local_cache: Arc::new(DashMap::new()),
        };

        let app = Router::new()
            .route("/ingest", post(ingest_handler))
            .with_state(state);

        let payload = r#"{"id": "evt_1", "idempotency_key": "key_1", "amount": 100, "timestamp": 0}"#;
        let request = Request::builder()
            .method("POST")
            .uri("/ingest")
            .header("content-type", "application/json")
            .body(Body::from(payload))
            .unwrap();

        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);

        let received = rx.try_recv().expect("Event should be in buffer");
        assert_eq!(received.idempotency_key, "key_1");

        let request_dup = Request::builder()
            .method("POST")
            .uri("/ingest")
            .header("content-type", "application/json")
            .body(Body::from(payload))
            .unwrap();

        let response_dup = app.oneshot(request_dup).await.unwrap();
        assert_eq!(response_dup.status(), StatusCode::ACCEPTED);
        assert!(rx.try_recv().is_err(), "Duplicate should not reach buffer");
    }
}