use ojs::{Client, OjsError, RetryConfig};
use serde_json::json;
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn rate_limit_response() -> serde_json::Value {
    json!({
        "error": {
            "code": "rate_limited",
            "message": "too many requests",
            "retryable": true
        }
    })
}

fn job_response() -> serde_json::Value {
    json!({
        "job": {
            "specversion": "1.0",
            "id": "job-001",
            "type": "test",
            "queue": "default",
            "args": [],
            "state": "available",
            "attempt": 0,
            "priority": 0,
            "created_at": "2025-01-01T00:00:00Z",
            "enqueued_at": "2025-01-01T00:00:00Z",
            "tags": []
        }
    })
}

// ---------------------------------------------------------------------------
// 429 retry succeeds on second attempt
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_retry_on_429_succeeds() {
    let server = MockServer::start().await;

    // First request: 429, second request: 201 success.
    Mock::given(method("POST"))
        .and(path("/ojs/v1/jobs"))
        .respond_with(ResponseTemplate::new(429).set_body_json(rate_limit_response()))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/ojs/v1/jobs"))
        .respond_with(ResponseTemplate::new(201).set_body_json(job_response()))
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::builder()
        .url(server.uri())
        .retry_config(
            RetryConfig::new()
                .with_max_retries(3)
                .with_min_backoff(Duration::from_millis(10))
                .with_max_backoff(Duration::from_millis(50)),
        )
        .build()
        .unwrap();

    let job = client.enqueue("test", json!({})).await.unwrap();
    assert_eq!(job.id, "job-001");
}

// ---------------------------------------------------------------------------
// Retry-After header is respected
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_retry_respects_retry_after_header() {
    let server = MockServer::start().await;

    // First request: 429 with Retry-After: 1
    Mock::given(method("POST"))
        .and(path("/ojs/v1/jobs"))
        .respond_with(
            ResponseTemplate::new(429)
                .append_header("Retry-After", "1")
                .set_body_json(rate_limit_response()),
        )
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/ojs/v1/jobs"))
        .respond_with(ResponseTemplate::new(201).set_body_json(job_response()))
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::builder()
        .url(server.uri())
        .retry_config(
            RetryConfig::new()
                .with_max_retries(3)
                .with_min_backoff(Duration::from_millis(10)),
        )
        .build()
        .unwrap();

    let start = std::time::Instant::now();
    let job = client.enqueue("test", json!({})).await.unwrap();
    let elapsed = start.elapsed();

    assert_eq!(job.id, "job-001");
    // Should have waited ~1 second due to Retry-After header.
    assert!(
        elapsed >= Duration::from_millis(900),
        "expected at least ~1s delay from Retry-After, got {:?}",
        elapsed
    );
}

// ---------------------------------------------------------------------------
// Max retries exhausted returns error
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_max_retries_exhausted() {
    let server = MockServer::start().await;

    // All requests return 429.
    Mock::given(method("POST"))
        .and(path("/ojs/v1/jobs"))
        .respond_with(ResponseTemplate::new(429).set_body_json(rate_limit_response()))
        .expect(3) // 1 initial + 2 retries
        .mount(&server)
        .await;

    let client = Client::builder()
        .url(server.uri())
        .retry_config(
            RetryConfig::new()
                .with_max_retries(2)
                .with_min_backoff(Duration::from_millis(10))
                .with_max_backoff(Duration::from_millis(20)),
        )
        .build()
        .unwrap();

    let err = client.enqueue("test", json!({})).await.unwrap_err();
    match err {
        OjsError::Server(server_err) => {
            assert!(server_err.is_rate_limited());
            assert_eq!(server_err.http_status, 429);
        }
        other => panic!("expected Server error, got: {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Retries disabled returns error immediately
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_retry_disabled() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/ojs/v1/jobs"))
        .respond_with(ResponseTemplate::new(429).set_body_json(rate_limit_response()))
        .expect(1) // Only one attempt when retries disabled
        .mount(&server)
        .await;

    let client = Client::builder()
        .url(server.uri())
        .retry_config(RetryConfig::disabled())
        .build()
        .unwrap();

    let err = client.enqueue("test", json!({})).await.unwrap_err();
    match err {
        OjsError::Server(server_err) => {
            assert!(server_err.is_rate_limited());
        }
        other => panic!("expected Server error, got: {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Non-429 errors are not retried
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_non_429_errors_not_retried() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/ojs/v1/jobs/nonexistent"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "error": {
                "code": "not_found",
                "message": "job not found",
                "retryable": false
            }
        })))
        .expect(1) // Should only be called once
        .mount(&server)
        .await;

    let client = Client::builder()
        .url(server.uri())
        .retry_config(
            RetryConfig::new()
                .with_max_retries(3)
                .with_min_backoff(Duration::from_millis(10)),
        )
        .build()
        .unwrap();

    let err = client.get_job("nonexistent").await.unwrap_err();
    match err {
        OjsError::Server(server_err) => {
            assert!(server_err.is_not_found());
        }
        other => panic!("expected Server error, got: {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Default retry config retries on 429
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_default_retry_config() {
    let server = MockServer::start().await;

    // First request: 429, second request: success.
    Mock::given(method("GET"))
        .and(path("/ojs/v1/health"))
        .respond_with(ResponseTemplate::new(429).set_body_json(rate_limit_response()))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/ojs/v1/health"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "healthy"
        })))
        .expect(1)
        .mount(&server)
        .await;

    // Default config (no explicit retry_config set) enables retries.
    let client = Client::builder().url(server.uri()).build().unwrap();

    let health = client.health().await.unwrap();
    assert_eq!(health.status, "healthy");
}
