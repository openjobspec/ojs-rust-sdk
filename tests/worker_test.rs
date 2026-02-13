use ojs::Worker;
use std::time::Duration;

#[test]
fn test_worker_builder_requires_url() {
    let result = Worker::builder().build();
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("url is required"));
}

#[test]
fn test_worker_builder_with_defaults() {
    let worker = Worker::builder()
        .url("http://localhost:8080")
        .build()
        .unwrap();

    assert_eq!(worker.state(), ojs::WorkerState::Running);
    assert!(!worker.id().is_empty());
}

#[test]
fn test_worker_builder_with_all_options() {
    let worker = Worker::builder()
        .url("http://localhost:8080")
        .queues(vec!["email", "reports", "default"])
        .concurrency(20)
        .grace_period(Duration::from_secs(60))
        .heartbeat_interval(Duration::from_secs(10))
        .poll_interval(Duration::from_secs(2))
        .labels(vec!["region:us-east", "env:production"])
        .auth_token("worker-token")
        .build();

    assert!(worker.is_ok());
}

#[tokio::test]
async fn test_worker_register_handler() {
    let worker = Worker::builder()
        .url("http://localhost:8080")
        .build()
        .unwrap();

    worker
        .register("test.job", |_ctx: ojs::JobContext| async move {
            Ok(serde_json::json!({"result": "ok"}))
        })
        .await;

    // Handler registration should succeed without error
}
