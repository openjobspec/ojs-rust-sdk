//! Tests for `JobContext` durable-execution checkpoint support
//! (`ojs-durable-execution.md` §4): save/get/delete against the standard
//! `/ojs/v1/jobs/:id/checkpoint` resource.
#![cfg(feature = "reqwest-transport")]

use ojs::{JobContext, Worker};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, Respond, ResponseTemplate};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct MigrationState {
    processed: usize,
    cursor: String,
}

/// Returns one job on the first fetch, then an empty list forever after.
struct OnceFetchResponder {
    call_count: AtomicUsize,
    job: serde_json::Value,
}

impl OnceFetchResponder {
    fn new(job: serde_json::Value) -> Self {
        Self {
            call_count: AtomicUsize::new(0),
            job,
        }
    }
}

impl Respond for OnceFetchResponder {
    fn respond(&self, _request: &wiremock::Request) -> ResponseTemplate {
        if self.call_count.fetch_add(1, Ordering::SeqCst) == 0 {
            ResponseTemplate::new(200).set_body_json(json!({"jobs": [self.job]}))
        } else {
            ResponseTemplate::new(200).set_body_json(json!({"jobs": []}))
        }
    }
}

fn job_fixture(id: &str, job_type: &str) -> serde_json::Value {
    json!({
        "specversion": "1.0",
        "id": id,
        "type": job_type,
        "queue": "default",
        "args": [{}],
        "state": "active",
        "attempt": 1,
        "priority": 0,
        "tags": []
    })
}

#[tokio::test]
async fn test_checkpoint_save_sends_post_with_state_body() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/fetch"))
        .respond_with(OnceFetchResponder::new(job_fixture(
            "job-cp-1",
            "test.checkpoint_save",
        )))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/ojs/v1/jobs/job-cp-1/checkpoint"))
        .and(body_json(json!({
            "state": {"processed": 5000, "cursor": "cursor_abc123"}
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "checkpoint": {"job_id": "job-cp-1", "sequence": 1, "created_at": "2026-01-01T00:00:00Z"}
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/ack"))
        .respond_with(ResponseTemplate::new(204))
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
        .register("test.checkpoint_save", |ctx: JobContext| async move {
            let state = MigrationState {
                processed: 5000,
                cursor: "cursor_abc123".to_string(),
            };
            ctx.checkpoint(&state)
                .await
                .expect("checkpoint save should succeed");
            Ok(json!({"ok": true}))
        })
        .await;

    let handle = tokio::spawn(async move { worker.start().await });
    tokio::time::sleep(Duration::from_millis(400)).await;
    handle.abort();
}

#[tokio::test]
async fn test_get_checkpoint_returns_some_when_present() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/fetch"))
        .respond_with(OnceFetchResponder::new(job_fixture(
            "job-cp-2",
            "test.checkpoint_resume",
        )))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/ojs/v1/jobs/job-cp-2/checkpoint"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "checkpoint": {
                "job_id": "job-cp-2",
                "state": {"processed": 1000, "cursor": "cursor_xyz"},
                "sequence": 3,
                "created_at": "2026-01-01T00:00:00Z"
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/ack"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/heartbeat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"state": "running"})))
        .mount(&server)
        .await;

    let seen = Arc::new(tokio::sync::Mutex::new(None));
    let seen_clone = seen.clone();

    let worker = Worker::builder()
        .url(server.uri())
        .queues(vec!["default"])
        .concurrency(1)
        .poll_interval(Duration::from_millis(50))
        .heartbeat_interval(Duration::from_secs(60))
        .build()
        .unwrap();

    worker
        .register("test.checkpoint_resume", move |ctx: JobContext| {
            let seen = seen_clone.clone();
            async move {
                let state: Option<MigrationState> = ctx.get_checkpoint().await.unwrap();
                *seen.lock().await = state;
                Ok(json!({"ok": true}))
            }
        })
        .await;

    let handle = tokio::spawn(async move { worker.start().await });
    tokio::time::sleep(Duration::from_millis(400)).await;
    handle.abort();

    let got = seen.lock().await.clone();
    assert_eq!(
        got,
        Some(MigrationState {
            processed: 1000,
            cursor: "cursor_xyz".to_string(),
        })
    );
}

#[tokio::test]
async fn test_get_checkpoint_returns_none_on_404() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/fetch"))
        .respond_with(OnceFetchResponder::new(job_fixture(
            "job-cp-3",
            "test.checkpoint_none",
        )))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/ojs/v1/jobs/job-cp-3/checkpoint"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "error": {"code": "not_found", "message": "no checkpoint", "retryable": false}
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/ack"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/heartbeat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"state": "running"})))
        .mount(&server)
        .await;

    let seen = Arc::new(tokio::sync::Mutex::new(Some(MigrationState {
        processed: 999,
        cursor: "sentinel".to_string(),
    })));
    let seen_clone = seen.clone();

    let worker = Worker::builder()
        .url(server.uri())
        .queues(vec!["default"])
        .concurrency(1)
        .poll_interval(Duration::from_millis(50))
        .heartbeat_interval(Duration::from_secs(60))
        .build()
        .unwrap();

    worker
        .register("test.checkpoint_none", move |ctx: JobContext| {
            let seen = seen_clone.clone();
            async move {
                let state: Option<MigrationState> = ctx.get_checkpoint().await.unwrap();
                *seen.lock().await = state;
                Ok(json!({"ok": true}))
            }
        })
        .await;

    let handle = tokio::spawn(async move { worker.start().await });
    tokio::time::sleep(Duration::from_millis(400)).await;
    handle.abort();

    assert_eq!(*seen.lock().await, None);
}

#[tokio::test]
async fn test_delete_checkpoint_treats_404_as_success() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/fetch"))
        .respond_with(OnceFetchResponder::new(job_fixture(
            "job-cp-4",
            "test.checkpoint_delete",
        )))
        .mount(&server)
        .await;

    Mock::given(method("DELETE"))
        .and(path("/ojs/v1/jobs/job-cp-4/checkpoint"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "error": {"code": "not_found", "message": "no checkpoint", "retryable": false}
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/ack"))
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
        .register("test.checkpoint_delete", |ctx: JobContext| async move {
            // A 404 (nothing to delete) must not fail the handler.
            ctx.delete_checkpoint()
                .await
                .expect("404 delete must be treated as success");
            Ok(json!({"ok": true}))
        })
        .await;

    let handle = tokio::spawn(async move { worker.start().await });
    tokio::time::sleep(Duration::from_millis(400)).await;
    handle.abort();
}
