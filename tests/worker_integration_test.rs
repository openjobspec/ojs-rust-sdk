use ojs::{JobContext, Worker};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Respond, ResponseTemplate};

/// Custom responder that returns jobs on the first call, then empty.
struct FetchResponder {
    call_count: AtomicUsize,
    jobs_response: serde_json::Value,
}

impl FetchResponder {
    fn new(jobs_response: serde_json::Value) -> Self {
        Self {
            call_count: AtomicUsize::new(0),
            jobs_response,
        }
    }
}

impl Respond for FetchResponder {
    fn respond(&self, _request: &wiremock::Request) -> ResponseTemplate {
        let count = self.call_count.fetch_add(1, Ordering::SeqCst);
        if count == 0 {
            ResponseTemplate::new(200).set_body_json(&self.jobs_response)
        } else {
            ResponseTemplate::new(200).set_body_json(json!({"jobs": []}))
        }
    }
}

#[tokio::test]
async fn test_worker_processes_and_acks_job() {
    let server = MockServer::start().await;

    // Mock: fetch returns one job on first call, then empty
    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/fetch"))
        .respond_with(FetchResponder::new(json!({
            "jobs": [{
                "specversion": "1.0",
                "id": "job-ack-001",
                "type": "test.success",
                "queue": "default",
                "args": [{"greeting": "hello"}],
                "state": "active",
                "attempt": 1,
                "priority": 0,
                "tags": []
            }]
        })))
        .mount(&server)
        .await;

    // Mock: ack endpoint — expect exactly 1 call
    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/ack"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    // Mock: heartbeat endpoint (worker sends heartbeats)
    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/heartbeat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"state": "running"})))
        .mount(&server)
        .await;

    let worker = Worker::builder()
        .url(server.uri())
        .queues(vec!["default"])
        .concurrency(1)
        .poll_interval(std::time::Duration::from_millis(50))
        .heartbeat_interval(std::time::Duration::from_secs(60))
        .build()
        .unwrap();

    // Register a handler that succeeds
    worker
        .register("test.success", |_ctx: JobContext| async move {
            Ok(json!({"status": "processed"}))
        })
        .await;

    // Run the worker in a background task and stop after a short delay
    let worker_handle = tokio::spawn(async move { worker.start().await });

    // Give the worker time to fetch, process, and ack the job
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Abort the worker (simulates shutdown)
    worker_handle.abort();
    let _ = worker_handle.await;

    // wiremock will verify the ack mock was called exactly once on drop
}

#[tokio::test]
async fn test_worker_nacks_on_handler_error() {
    let server = MockServer::start().await;

    // Mock: fetch returns one job that will fail
    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/fetch"))
        .respond_with(FetchResponder::new(json!({
            "jobs": [{
                "specversion": "1.0",
                "id": "job-nack-001",
                "type": "test.failure",
                "queue": "default",
                "args": [{}],
                "state": "active",
                "attempt": 1,
                "priority": 0,
                "tags": []
            }]
        })))
        .mount(&server)
        .await;

    // Mock: nack endpoint — expect exactly 1 call
    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/nack"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    // Mock: heartbeat
    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/heartbeat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"state": "running"})))
        .mount(&server)
        .await;

    let worker = Worker::builder()
        .url(server.uri())
        .queues(vec!["default"])
        .concurrency(1)
        .poll_interval(std::time::Duration::from_millis(50))
        .heartbeat_interval(std::time::Duration::from_secs(60))
        .build()
        .unwrap();

    // Register a handler that fails
    worker
        .register("test.failure", |_ctx: JobContext| async move {
            Err(ojs::OjsError::Handler("something went wrong".into()))
        })
        .await;

    let worker_handle = tokio::spawn(async move { worker.start().await });

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    worker_handle.abort();
    let _ = worker_handle.await;
}

#[tokio::test]
async fn test_worker_nacks_unregistered_job_type() {
    let server = MockServer::start().await;

    // Mock: fetch returns a job type with no registered handler
    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/fetch"))
        .respond_with(FetchResponder::new(json!({
            "jobs": [{
                "specversion": "1.0",
                "id": "job-unreg-001",
                "type": "unknown.job.type",
                "queue": "default",
                "args": [{}],
                "state": "active",
                "attempt": 1,
                "priority": 0,
                "tags": []
            }]
        })))
        .mount(&server)
        .await;

    // Mock: nack endpoint — expect 1 call (non-retryable for unknown handler)
    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/nack"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    // Mock: heartbeat
    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/heartbeat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"state": "running"})))
        .mount(&server)
        .await;

    let worker = Worker::builder()
        .url(server.uri())
        .queues(vec!["default"])
        .concurrency(1)
        .poll_interval(std::time::Duration::from_millis(50))
        .heartbeat_interval(std::time::Duration::from_secs(60))
        .build()
        .unwrap();

    // No handler registered — worker should nack with "no handler registered"

    let worker_handle = tokio::spawn(async move { worker.start().await });

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    worker_handle.abort();
    let _ = worker_handle.await;
}

#[tokio::test]
async fn test_worker_middleware_executes() {
    let server = MockServer::start().await;
    let middleware_called = Arc::new(AtomicUsize::new(0));
    let middleware_called_clone = middleware_called.clone();

    // Mock: fetch
    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/fetch"))
        .respond_with(FetchResponder::new(json!({
            "jobs": [{
                "specversion": "1.0",
                "id": "job-mw-001",
                "type": "test.middleware",
                "queue": "default",
                "args": [{}],
                "state": "active",
                "attempt": 1,
                "priority": 0,
                "tags": []
            }]
        })))
        .mount(&server)
        .await;

    // Mock: ack
    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/ack"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    // Mock: heartbeat
    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/heartbeat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"state": "running"})))
        .mount(&server)
        .await;

    let worker = Worker::builder()
        .url(server.uri())
        .queues(vec!["default"])
        .concurrency(1)
        .poll_interval(std::time::Duration::from_millis(50))
        .heartbeat_interval(std::time::Duration::from_secs(60))
        .build()
        .unwrap();

    // Add middleware that tracks calls
    struct CountingMiddleware {
        counter: Arc<AtomicUsize>,
    }

    impl ojs::Middleware for CountingMiddleware {
        fn handle(
            &self,
            ctx: JobContext,
            next: ojs::Next,
        ) -> ojs::BoxFuture<'static, ojs::HandlerResult> {
            self.counter.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move { next.run(ctx).await })
        }
    }

    worker
        .use_middleware(
            "counting",
            CountingMiddleware {
                counter: middleware_called_clone,
            },
        )
        .await;

    worker
        .register("test.middleware", |_ctx: JobContext| async move {
            Ok(json!({"ok": true}))
        })
        .await;

    let worker_handle = tokio::spawn(async move { worker.start().await });

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    worker_handle.abort();
    let _ = worker_handle.await;

    assert_eq!(
        middleware_called.load(Ordering::SeqCst),
        1,
        "middleware should have been called exactly once"
    );
}
