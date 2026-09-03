//! Verifies `TimeoutMiddleware` reports a timed-out handler using the
//! canonical `timeout` NACK code (`ojs::error_codes::ERR_TIMEOUT`), not the
//! generic `handler_error` code, end-to-end through a real `Worker`.
#![cfg(all(feature = "reqwest-transport", feature = "common-middleware"))]

use ojs::middleware_common::timeout::TimeoutMiddleware;
use ojs::{JobContext, Worker};
use serde_json::json;
use std::time::Duration;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn test_timeout_reports_canonical_timeout_nack_code() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/fetch"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "jobs": [{
                "specversion": "1.0",
                "id": "job-timeout-1",
                "type": "test.slow",
                "queue": "default",
                "args": [{}],
                "state": "active",
                "attempt": 1,
                "priority": 0,
                "tags": []
            }]
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/fetch"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"jobs": []})))
        .mount(&server)
        .await;

    // Assert the NACK carries the canonical "timeout" code and is
    // retryable, not the generic "handler_error" code the middleware used
    // to (mis)report via the doc-mismatched `OjsError::Handler` variant.
    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/nack"))
        .and(body_partial_json(json!({
            "job_id": "job-timeout-1",
            "error": {"code": "timeout", "retryable": true}
        })))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/heartbeat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"state": "running"})))
        .mount(&server)
        .await;

    let worker = Worker::builder()
        .url(server.uri())
        .queues(vec!["default"])
        .concurrency(1)
        .poll_interval(Duration::from_millis(50))
        .heartbeat_interval(Duration::from_secs(60))
        .build()
        .unwrap();

    worker
        .use_middleware("timeout", TimeoutMiddleware::new(Duration::from_millis(50)))
        .await;

    worker
        .register("test.slow", |_ctx: JobContext| async move {
            // Sleeps far longer than the middleware's configured timeout.
            tokio::time::sleep(Duration::from_secs(5)).await;
            Ok(json!({"ok": true}))
        })
        .await;

    let handle = tokio::spawn(async move { worker.start().await });
    tokio::time::sleep(Duration::from_millis(500)).await;
    handle.abort();
}
