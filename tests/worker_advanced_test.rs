// This file exercises Client/Worker against a real (mocked) HTTP
// transport and therefore requires the `reqwest-transport` feature
// (enabled by default). Under `--no-default-features` this file
// compiles to an empty test binary instead of reporting spurious
// failures for a feature that was deliberately disabled.
#![cfg(feature = "reqwest-transport")]

use ojs::transport::{Method, Transport};
use ojs::{BoxFuture, HandlerResult, JobContext, Middleware, Next, OjsError, Worker};
use serde_json::json;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use tokio::sync::{Barrier, Notify};
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, Respond, ResponseTemplate};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Returns jobs on the first N calls, then empty.
struct MultiCallFetchResponder {
    call_count: AtomicUsize,
    responses: Vec<serde_json::Value>,
}

impl MultiCallFetchResponder {
    fn once(response: serde_json::Value) -> Self {
        Self {
            call_count: AtomicUsize::new(0),
            responses: vec![response],
        }
    }
}

impl Respond for MultiCallFetchResponder {
    fn respond(&self, _request: &wiremock::Request) -> ResponseTemplate {
        let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
        if idx < self.responses.len() {
            ResponseTemplate::new(200).set_body_json(&self.responses[idx])
        } else {
            ResponseTemplate::new(200).set_body_json(json!({"jobs": []}))
        }
    }
}

fn make_job(id: &str, job_type: &str) -> serde_json::Value {
    json!({
        "specversion": "1.0",
        "id": id,
        "type": job_type,
        "queue": "default",
        "args": [{"key": "value"}],
        "state": "active",
        "attempt": 1,
        "priority": 0,
        "tags": []
    })
}

fn mount_heartbeat(_server: &MockServer) -> Mock {
    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/heartbeat"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"state": "running"})))
}

#[derive(Debug, Default)]
struct CountingTransport {
    calls: AtomicUsize,
}

impl Transport for CountingTransport {
    fn request(
        &self,
        _method: Method,
        path: &str,
        _body: Option<serde_json::Value>,
        _raw_path: bool,
    ) -> Pin<
        Box<dyn std::future::Future<Output = ojs::Result<Option<serde_json::Value>>> + Send + '_>,
    > {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let response = match path {
            "/workers/fetch" => Some(json!({ "jobs": [] })),
            "/workers/heartbeat" => Some(json!({ "state": "running" })),
            "/workers/ack" | "/workers/nack" => None,
            other => panic!("unexpected worker path: {other}"),
        };
        Box::pin(async move { Ok(response) })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockedTerminalReport {
    Ack,
    HandlerNack,
}

#[derive(Debug)]
struct BlockingReportTransport {
    blocked_report: BlockedTerminalReport,
    fetch_job: serde_json::Value,
    fetch_served: AtomicBool,
    report_barrier: Arc<Barrier>,
    ack_attempts: AtomicUsize,
    handler_nack_attempts: AtomicUsize,
    shutdown_nack_attempts: AtomicUsize,
}

impl BlockingReportTransport {
    fn new(blocked_report: BlockedTerminalReport, fetch_job: serde_json::Value) -> Self {
        Self {
            blocked_report,
            fetch_job,
            fetch_served: AtomicBool::new(false),
            report_barrier: Arc::new(Barrier::new(2)),
            ack_attempts: AtomicUsize::new(0),
            handler_nack_attempts: AtomicUsize::new(0),
            shutdown_nack_attempts: AtomicUsize::new(0),
        }
    }

    async fn wait_for_blocked_report(&self) {
        self.report_barrier.clone().wait().await;
    }
}

impl Transport for BlockingReportTransport {
    fn request(
        &self,
        _method: Method,
        path: &str,
        body: Option<serde_json::Value>,
        _raw_path: bool,
    ) -> Pin<
        Box<dyn std::future::Future<Output = ojs::Result<Option<serde_json::Value>>> + Send + '_>,
    > {
        match path {
            "/workers/fetch" => {
                let jobs = if self.fetch_served.swap(true, Ordering::SeqCst) {
                    vec![]
                } else {
                    vec![self.fetch_job.clone()]
                };
                Box::pin(async move { Ok(Some(json!({ "jobs": jobs }))) })
            }
            "/workers/heartbeat" => {
                Box::pin(async move { Ok(Some(json!({ "state": "running" }))) })
            }
            "/workers/ack" => {
                self.ack_attempts.fetch_add(1, Ordering::SeqCst);
                if self.blocked_report == BlockedTerminalReport::Ack {
                    let barrier = self.report_barrier.clone();
                    Box::pin(async move {
                        barrier.wait().await;
                        std::future::pending::<ojs::Result<Option<serde_json::Value>>>().await
                    })
                } else {
                    Box::pin(async move { Ok(None) })
                }
            }
            "/workers/nack" => {
                let code = body
                    .as_ref()
                    .and_then(|value| value.get("error"))
                    .and_then(|value| value.get("code"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if code == "shutdown" {
                    self.shutdown_nack_attempts.fetch_add(1, Ordering::SeqCst);
                    Box::pin(async move { Ok(None) })
                } else {
                    self.handler_nack_attempts.fetch_add(1, Ordering::SeqCst);
                    if self.blocked_report == BlockedTerminalReport::HandlerNack {
                        let barrier = self.report_barrier.clone();
                        Box::pin(async move {
                            barrier.wait().await;
                            std::future::pending::<ojs::Result<Option<serde_json::Value>>>().await
                        })
                    } else {
                        Box::pin(async move { Ok(None) })
                    }
                }
            }
            other => panic!("unexpected worker path: {other}"),
        }
    }
}

#[derive(Debug)]
struct HeartbeatOrderingTransport {
    fetch_job: serde_json::Value,
    fetch_served: AtomicBool,
    heartbeat_started: AtomicBool,
    heartbeat_started_notify: Notify,
    heartbeat_terminated: AtomicBool,
    shutdown_nack_attempts: AtomicUsize,
    shutdown_nack_saw_terminated_heartbeat: AtomicBool,
    force_nack_started: (Mutex<bool>, Condvar),
}

impl HeartbeatOrderingTransport {
    fn new(fetch_job: serde_json::Value) -> Self {
        Self {
            fetch_job,
            fetch_served: AtomicBool::new(false),
            heartbeat_started: AtomicBool::new(false),
            heartbeat_started_notify: Notify::new(),
            heartbeat_terminated: AtomicBool::new(false),
            shutdown_nack_attempts: AtomicUsize::new(0),
            shutdown_nack_saw_terminated_heartbeat: AtomicBool::new(false),
            force_nack_started: (Mutex::new(false), Condvar::new()),
        }
    }

    async fn wait_for_heartbeat(&self) {
        loop {
            let notified = self.heartbeat_started_notify.notified();
            if self.heartbeat_started.load(Ordering::SeqCst) {
                return;
            }
            notified.await;
        }
    }
}

struct HeartbeatTerminationGuard<'a> {
    terminated: &'a AtomicBool,
    force_nack_started: &'a (Mutex<bool>, Condvar),
}

impl Drop for HeartbeatTerminationGuard<'_> {
    fn drop(&mut self) {
        let (lock, condvar) = self.force_nack_started;
        let started = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = condvar
            .wait_timeout_while(started, Duration::from_millis(500), |started| !*started)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.terminated.store(true, Ordering::SeqCst);
    }
}

impl Transport for HeartbeatOrderingTransport {
    fn request(
        &self,
        _method: Method,
        path: &str,
        body: Option<serde_json::Value>,
        _raw_path: bool,
    ) -> Pin<
        Box<dyn std::future::Future<Output = ojs::Result<Option<serde_json::Value>>> + Send + '_>,
    > {
        match path {
            "/workers/fetch" => {
                let jobs = if self.fetch_served.swap(true, Ordering::SeqCst) {
                    vec![]
                } else {
                    vec![self.fetch_job.clone()]
                };
                Box::pin(async move { Ok(Some(json!({ "jobs": jobs }))) })
            }
            "/workers/heartbeat" => Box::pin(async move {
                let _termination_guard = HeartbeatTerminationGuard {
                    terminated: &self.heartbeat_terminated,
                    force_nack_started: &self.force_nack_started,
                };
                self.heartbeat_started.store(true, Ordering::SeqCst);
                self.heartbeat_started_notify.notify_waiters();
                std::future::pending::<ojs::Result<Option<serde_json::Value>>>().await
            }),
            "/workers/nack" => {
                let code = body
                    .as_ref()
                    .and_then(|value| value.get("error"))
                    .and_then(|value| value.get("code"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                assert_eq!(code, "shutdown");
                self.shutdown_nack_attempts.fetch_add(1, Ordering::SeqCst);
                self.shutdown_nack_saw_terminated_heartbeat.store(
                    self.heartbeat_terminated.load(Ordering::SeqCst),
                    Ordering::SeqCst,
                );
                let (lock, condvar) = &self.force_nack_started;
                *lock
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
                condvar.notify_all();
                Box::pin(async move { Ok(None) })
            }
            "/workers/ack" => panic!("abandoned handler must not ACK"),
            other => panic!("unexpected worker path: {other}"),
        }
    }
}

#[derive(Debug)]
struct CpuBlockingHandlerTransport {
    fetch_job: serde_json::Value,
    fetch_served: AtomicBool,
    ack_attempts: AtomicUsize,
    shutdown_nack_attempts: AtomicUsize,
    shutdown_nack_started: Notify,
}

impl CpuBlockingHandlerTransport {
    fn new(fetch_job: serde_json::Value) -> Self {
        Self {
            fetch_job,
            fetch_served: AtomicBool::new(false),
            ack_attempts: AtomicUsize::new(0),
            shutdown_nack_attempts: AtomicUsize::new(0),
            shutdown_nack_started: Notify::new(),
        }
    }

    async fn wait_for_shutdown_nack(&self) {
        loop {
            let notified = self.shutdown_nack_started.notified();
            if self.shutdown_nack_attempts.load(Ordering::SeqCst) > 0 {
                return;
            }
            notified.await;
        }
    }
}

impl Transport for CpuBlockingHandlerTransport {
    fn request(
        &self,
        _method: Method,
        path: &str,
        body: Option<serde_json::Value>,
        _raw_path: bool,
    ) -> Pin<
        Box<dyn std::future::Future<Output = ojs::Result<Option<serde_json::Value>>> + Send + '_>,
    > {
        match path {
            "/workers/fetch" => {
                let jobs = if self.fetch_served.swap(true, Ordering::SeqCst) {
                    vec![]
                } else {
                    vec![self.fetch_job.clone()]
                };
                Box::pin(async move { Ok(Some(json!({ "jobs": jobs }))) })
            }
            "/workers/heartbeat" => {
                Box::pin(async move { Ok(Some(json!({ "state": "running" }))) })
            }
            "/workers/ack" => {
                self.ack_attempts.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move { Ok(None) })
            }
            "/workers/nack" => {
                let code = body
                    .as_ref()
                    .and_then(|value| value.get("error"))
                    .and_then(|value| value.get("code"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                assert_eq!(code, "shutdown");
                self.shutdown_nack_attempts.fetch_add(1, Ordering::SeqCst);
                self.shutdown_nack_started.notify_waiters();
                Box::pin(async move { Ok(None) })
            }
            other => panic!("unexpected worker path: {other}"),
        }
    }
}

/// Serves one job and answers its ACK only after a delay, so the ACK is
/// still in flight when a short grace period expires.
#[derive(Debug)]
struct SlowReportTransport {
    fetch_job: serde_json::Value,
    fetch_served: AtomicBool,
    ack_delay: Duration,
    ack_started: AtomicUsize,
    ack_started_notify: Notify,
    ack_completed: AtomicUsize,
    shutdown_nack_attempts: AtomicUsize,
}

impl SlowReportTransport {
    fn new(fetch_job: serde_json::Value, ack_delay: Duration) -> Self {
        Self {
            fetch_job,
            fetch_served: AtomicBool::new(false),
            ack_delay,
            ack_started: AtomicUsize::new(0),
            ack_started_notify: Notify::new(),
            ack_completed: AtomicUsize::new(0),
            shutdown_nack_attempts: AtomicUsize::new(0),
        }
    }

    async fn wait_for_ack_start(&self) {
        loop {
            let notified = self.ack_started_notify.notified();
            if self.ack_started.load(Ordering::SeqCst) > 0 {
                return;
            }
            notified.await;
        }
    }
}

impl Transport for SlowReportTransport {
    fn request(
        &self,
        _method: Method,
        path: &str,
        _body: Option<serde_json::Value>,
        _raw_path: bool,
    ) -> Pin<
        Box<dyn std::future::Future<Output = ojs::Result<Option<serde_json::Value>>> + Send + '_>,
    > {
        match path {
            "/workers/fetch" => {
                let jobs = if self.fetch_served.swap(true, Ordering::SeqCst) {
                    vec![]
                } else {
                    vec![self.fetch_job.clone()]
                };
                Box::pin(async move { Ok(Some(json!({ "jobs": jobs }))) })
            }
            "/workers/heartbeat" => {
                Box::pin(async move { Ok(Some(json!({ "state": "running" }))) })
            }
            "/workers/ack" => {
                self.ack_started.fetch_add(1, Ordering::SeqCst);
                self.ack_started_notify.notify_waiters();
                Box::pin(async move {
                    tokio::time::sleep(self.ack_delay).await;
                    self.ack_completed.fetch_add(1, Ordering::SeqCst);
                    Ok(None)
                })
            }
            "/workers/nack" => {
                self.shutdown_nack_attempts.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move { Ok(None) })
            }
            other => panic!("unexpected worker path: {other}"),
        }
    }
}

/// Serves one job and records every terminal report it receives, so a test
/// can prove a job is reported exactly once even when the handler's own fast
/// report races forced shutdown reporting.
#[derive(Debug)]
struct RecordingReportTransport {
    fetch_job: serde_json::Value,
    fetch_served: AtomicBool,
    fetch_notify: Notify,
    reports: Mutex<Vec<(String, &'static str)>>,
}

impl RecordingReportTransport {
    fn new(fetch_job: serde_json::Value) -> Self {
        Self {
            fetch_job,
            fetch_served: AtomicBool::new(false),
            fetch_notify: Notify::new(),
            reports: Mutex::new(Vec::new()),
        }
    }

    async fn wait_for_fetch(&self) {
        loop {
            let notified = self.fetch_notify.notified();
            if self.fetch_served.load(Ordering::SeqCst) {
                return;
            }
            notified.await;
        }
    }

    fn reports(&self) -> Vec<(String, &'static str)> {
        self.reports
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn record(&self, body: Option<serde_json::Value>, kind: &'static str) {
        let job_id = body
            .as_ref()
            .and_then(|b| b.get("job_id"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        self.reports
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((job_id, kind));
    }
}

impl Transport for RecordingReportTransport {
    fn request(
        &self,
        _method: Method,
        path: &str,
        body: Option<serde_json::Value>,
        _raw_path: bool,
    ) -> Pin<
        Box<dyn std::future::Future<Output = ojs::Result<Option<serde_json::Value>>> + Send + '_>,
    > {
        match path {
            "/workers/fetch" => {
                let jobs = if self.fetch_served.swap(true, Ordering::SeqCst) {
                    vec![]
                } else {
                    vec![self.fetch_job.clone()]
                };
                self.fetch_notify.notify_waiters();
                Box::pin(async move { Ok(Some(json!({ "jobs": jobs }))) })
            }
            "/workers/heartbeat" => {
                Box::pin(async move { Ok(Some(json!({ "state": "running" }))) })
            }
            "/workers/ack" => {
                self.record(body, "ack");
                Box::pin(async move { Ok(None) })
            }
            "/workers/nack" => {
                self.record(body, "nack");
                Box::pin(async move { Ok(None) })
            }
            other => panic!("unexpected worker path: {other}"),
        }
    }
}

/// Serves a large fixed set of jobs, alternating a type whose handler
/// completes (slow ACK) with one whose handler never finishes (forced NACK),
/// and records every terminal report for exactly-once assertions.
#[derive(Debug)]
struct MassShutdownTransport {
    job_count: usize,
    served: AtomicUsize,
    ack_delay: Duration,
    /// Jobs that either started an ACK or are hanging forever, i.e. jobs
    /// whose shutdown fate is now determined.
    reporting_or_hanging: AtomicUsize,
    all_started_notify: Notify,
    reports: Mutex<Vec<(String, &'static str)>>,
}

impl MassShutdownTransport {
    fn new(job_count: usize, ack_delay: Duration) -> Self {
        Self {
            job_count,
            served: AtomicUsize::new(0),
            ack_delay,
            reporting_or_hanging: AtomicUsize::new(0),
            all_started_notify: Notify::new(),
            reports: Mutex::new(Vec::new()),
        }
    }

    /// Every hanging job counts as soon as it is served; every completing
    /// job counts once its ACK request has actually begun.
    fn note_started(&self) {
        if self.reporting_or_hanging.fetch_add(1, Ordering::SeqCst) + 1 >= self.job_count {
            self.all_started_notify.notify_waiters();
        }
    }

    async fn wait_for_all_reports_started(&self) {
        loop {
            let notified = self.all_started_notify.notified();
            if self.reporting_or_hanging.load(Ordering::SeqCst) >= self.job_count {
                return;
            }
            notified.await;
        }
    }

    fn reports(&self) -> Vec<(String, &'static str)> {
        self.reports
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn record(&self, job_id: String, kind: &'static str) {
        self.reports
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((job_id, kind));
    }
}

impl Transport for MassShutdownTransport {
    fn request(
        &self,
        _method: Method,
        path: &str,
        body: Option<serde_json::Value>,
        _raw_path: bool,
    ) -> Pin<
        Box<dyn std::future::Future<Output = ojs::Result<Option<serde_json::Value>>> + Send + '_>,
    > {
        match path {
            "/workers/fetch" => {
                let count = body
                    .as_ref()
                    .and_then(|b| b.get("count"))
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(1) as usize;
                let mut jobs = Vec::new();
                for _ in 0..count {
                    let index = self.served.fetch_add(1, Ordering::SeqCst);
                    if index >= self.job_count {
                        self.served.store(self.job_count, Ordering::SeqCst);
                        break;
                    }
                    let job_type = if index % 2 == 0 {
                        "test.mass_ack"
                    } else {
                        "test.mass_hang"
                    };
                    if index % 2 == 1 {
                        // A hanging job's fate is sealed the moment it is
                        // dispatched: it can only ever be force-nacked.
                        self.note_started();
                    }
                    jobs.push(make_job(&format!("job-mass-{index:04}"), job_type));
                }
                Box::pin(async move { Ok(Some(json!({ "jobs": jobs }))) })
            }
            "/workers/heartbeat" => {
                Box::pin(async move { Ok(Some(json!({ "state": "running" }))) })
            }
            "/workers/ack" => {
                let job_id = body
                    .as_ref()
                    .and_then(|b| b.get("job_id"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                self.note_started();
                Box::pin(async move {
                    tokio::time::sleep(self.ack_delay).await;
                    self.record(job_id, "ack");
                    Ok(None)
                })
            }
            "/workers/nack" => {
                let job_id = body
                    .as_ref()
                    .and_then(|b| b.get("job_id"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let code = body
                    .as_ref()
                    .and_then(|b| b.get("error"))
                    .and_then(|e| e.get("code"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                let kind = if code == "shutdown" {
                    "shutdown_nack"
                } else {
                    "handler_nack"
                };
                self.record(job_id, kind);
                Box::pin(async move { Ok(None) })
            }
            other => panic!("unexpected worker path: {other}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Test: Handler body is actually invoked
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_handler_body_executes() {
    let server = MockServer::start().await;
    let handler_called = Arc::new(AtomicBool::new(false));
    let handler_called_clone = handler_called.clone();

    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/fetch"))
        .respond_with(MultiCallFetchResponder::once(json!({
            "jobs": [make_job("job-exec-001", "test.exec")]
        })))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/ack"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    mount_heartbeat(&server).mount(&server).await;

    let worker = Worker::builder()
        .url(server.uri())
        .queues(vec!["default"])
        .concurrency(1)
        .poll_interval(Duration::from_millis(50))
        .heartbeat_interval(Duration::from_secs(60))
        .build()
        .unwrap();

    worker
        .register("test.exec", move |_ctx: JobContext| {
            let flag = handler_called_clone.clone();
            async move {
                flag.store(true, Ordering::SeqCst);
                Ok(json!({"executed": true}))
            }
        })
        .await;

    let handle = tokio::spawn(async move { worker.start().await });
    tokio::time::sleep(Duration::from_millis(500)).await;
    handle.abort();
    let _ = handle.await;

    assert!(
        handler_called.load(Ordering::SeqCst),
        "handler body must be invoked"
    );
}

// ---------------------------------------------------------------------------
// Test: Handler receives correct job context
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_handler_receives_job_context() {
    let server = MockServer::start().await;
    let received_type = Arc::new(std::sync::Mutex::new(String::new()));
    let received_type_clone = received_type.clone();

    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/fetch"))
        .respond_with(MultiCallFetchResponder::once(json!({
            "jobs": [make_job("job-ctx-001", "email.send")]
        })))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/ack"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    mount_heartbeat(&server).mount(&server).await;

    let worker = Worker::builder()
        .url(server.uri())
        .queues(vec!["default"])
        .concurrency(1)
        .poll_interval(Duration::from_millis(50))
        .heartbeat_interval(Duration::from_secs(60))
        .build()
        .unwrap();

    worker
        .register("email.send", move |ctx: JobContext| {
            let t = received_type_clone.clone();
            async move {
                *t.lock().unwrap() = ctx.job.job_type.clone();
                Ok(json!({"ok": true}))
            }
        })
        .await;

    let handle = tokio::spawn(async move { worker.start().await });
    tokio::time::sleep(Duration::from_millis(500)).await;
    handle.abort();
    let _ = handle.await;

    assert_eq!(
        *received_type.lock().unwrap(),
        "email.send",
        "handler must receive correct job type in context"
    );
}

// ---------------------------------------------------------------------------
// Test: NonRetryable error results in retryable=false in NACK
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_non_retryable_error_nacks_with_retryable_false() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/fetch"))
        .respond_with(MultiCallFetchResponder::once(json!({
            "jobs": [make_job("job-nonretry-001", "test.non_retryable")]
        })))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/nack"))
        .and(body_partial_json(json!({
            "job_id": "job-nonretry-001",
            "error": {
                "code": "handler_error",
                "retryable": false
            }
        })))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    mount_heartbeat(&server).mount(&server).await;

    let worker = Worker::builder()
        .url(server.uri())
        .queues(vec!["default"])
        .concurrency(1)
        .poll_interval(Duration::from_millis(50))
        .heartbeat_interval(Duration::from_secs(60))
        .build()
        .unwrap();

    worker
        .register("test.non_retryable", |_ctx: JobContext| async move {
            Err(OjsError::NonRetryable("permanent failure".into()))
        })
        .await;

    let handle = tokio::spawn(async move { worker.start().await });
    tokio::time::sleep(Duration::from_millis(500)).await;
    handle.abort();
    let _ = handle.await;
}

// ---------------------------------------------------------------------------
// Test: Multiple middleware execute in registration order (onion model)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_middleware_execution_order() {
    let server = MockServer::start().await;
    let order = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));

    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/fetch"))
        .respond_with(MultiCallFetchResponder::once(json!({
            "jobs": [make_job("job-mw-order-001", "test.mw_order")]
        })))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/ack"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    mount_heartbeat(&server).mount(&server).await;

    let worker = Worker::builder()
        .url(server.uri())
        .queues(vec!["default"])
        .concurrency(1)
        .poll_interval(Duration::from_millis(50))
        .heartbeat_interval(Duration::from_secs(60))
        .build()
        .unwrap();

    struct OrderMiddleware {
        label: String,
        order: Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl Middleware for OrderMiddleware {
        fn handle(&self, ctx: JobContext, next: Next) -> BoxFuture<'static, HandlerResult> {
            let label = self.label.clone();
            let order = self.order.clone();
            Box::pin(async move {
                order.lock().unwrap().push(format!("{}-before", label));
                let result = next.run(ctx).await;
                order.lock().unwrap().push(format!("{}-after", label));
                result
            })
        }
    }

    let order_ref = order.clone();
    worker
        .use_middleware(
            "outer",
            OrderMiddleware {
                label: "outer".into(),
                order: order.clone(),
            },
        )
        .await;

    worker
        .use_middleware(
            "inner",
            OrderMiddleware {
                label: "inner".into(),
                order: order.clone(),
            },
        )
        .await;

    let handler_order = order.clone();
    worker
        .register("test.mw_order", move |_ctx: JobContext| {
            let o = handler_order.clone();
            async move {
                o.lock().unwrap().push("handler".to_string());
                Ok(json!({"ok": true}))
            }
        })
        .await;

    let handle = tokio::spawn(async move { worker.start().await });
    tokio::time::sleep(Duration::from_millis(500)).await;
    handle.abort();
    let _ = handle.await;

    let recorded = order_ref.lock().unwrap().clone();
    assert_eq!(
        recorded,
        vec![
            "outer-before",
            "inner-before",
            "handler",
            "inner-after",
            "outer-after",
        ],
        "middleware must execute in onion order: outer→inner→handler→inner→outer"
    );
}

// ---------------------------------------------------------------------------
// Test: Concurrent job processing with concurrency > 1
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_concurrent_job_processing() {
    let server = MockServer::start().await;
    let max_concurrent = Arc::new(AtomicUsize::new(0));
    let current_concurrent = Arc::new(AtomicUsize::new(0));

    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/fetch"))
        .respond_with(MultiCallFetchResponder::once(json!({
            "jobs": [
                make_job("job-conc-001", "test.slow"),
                make_job("job-conc-002", "test.slow"),
                make_job("job-conc-003", "test.slow")
            ]
        })))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/ack"))
        .respond_with(ResponseTemplate::new(204))
        .expect(3)
        .mount(&server)
        .await;

    mount_heartbeat(&server).mount(&server).await;

    let worker = Worker::builder()
        .url(server.uri())
        .queues(vec!["default"])
        .concurrency(3)
        .poll_interval(Duration::from_millis(50))
        .heartbeat_interval(Duration::from_secs(60))
        .build()
        .unwrap();

    let max_c = max_concurrent.clone();
    let cur_c = current_concurrent.clone();
    worker
        .register("test.slow", move |_ctx: JobContext| {
            let max_c = max_c.clone();
            let cur_c = cur_c.clone();
            async move {
                let current = cur_c.fetch_add(1, Ordering::SeqCst) + 1;
                // Track peak concurrency
                max_c.fetch_max(current, Ordering::SeqCst);
                // Simulate work
                tokio::time::sleep(Duration::from_millis(100)).await;
                cur_c.fetch_sub(1, Ordering::SeqCst);
                Ok(json!({"ok": true}))
            }
        })
        .await;

    let handle = tokio::spawn(async move { worker.start().await });
    tokio::time::sleep(Duration::from_millis(800)).await;
    handle.abort();
    let _ = handle.await;

    let peak = max_concurrent.load(Ordering::SeqCst);
    assert!(
        peak > 1,
        "with concurrency=3 and 3 slow jobs, peak concurrent should be >1, got {}",
        peak
    );
}

// ---------------------------------------------------------------------------
// Test: Graceful shutdown waits for in-flight jobs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_graceful_shutdown_completes_active_jobs() {
    let server = MockServer::start().await;
    let handler_completed = Arc::new(AtomicBool::new(false));
    let handler_completed_clone = handler_completed.clone();

    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/fetch"))
        .respond_with(MultiCallFetchResponder::once(json!({
            "jobs": [make_job("job-grace-001", "test.slow_grace")]
        })))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/ack"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    mount_heartbeat(&server).mount(&server).await;

    let worker = Arc::new(
        Worker::builder()
            .url(server.uri())
            .queues(vec!["default"])
            .concurrency(1)
            .poll_interval(Duration::from_millis(50))
            .heartbeat_interval(Duration::from_secs(60))
            .grace_period(Duration::from_secs(5))
            .build()
            .unwrap(),
    );

    worker
        .register("test.slow_grace", move |_ctx: JobContext| {
            let completed = handler_completed_clone.clone();
            async move {
                // Simulate slow job
                tokio::time::sleep(Duration::from_millis(300)).await;
                completed.store(true, Ordering::SeqCst);
                Ok(json!({"ok": true}))
            }
        })
        .await;

    let worker_for_start = worker.clone();
    let handle = tokio::spawn(async move { worker_for_start.start().await });

    // Wait for the job to start processing, then request a *graceful*
    // shutdown via the programmatic API (rather than aborting the task,
    // which cannot reliably exercise the grace-period wait at all).
    tokio::time::sleep(Duration::from_millis(100)).await;
    worker.shutdown();

    // `start()` should return on its own once the slow handler finishes and
    // is acked, well within the 5s grace period and this test's timeout.
    tokio::time::timeout(Duration::from_secs(3), handle)
        .await
        .expect("worker.start() should return once the active job finishes")
        .expect("worker task should not panic")
        .expect("start() should return Ok");

    // The handler ran to completion (not aborted mid-flight), and its ack
    // was observed by the mock (`.expect(1)` above) rather than a forced
    // NACK, proving the grace period actually waited for it.
    assert!(handler_completed.load(Ordering::SeqCst));
}

// ---------------------------------------------------------------------------
// Test: shutdown requested before `start()` is observed immediately
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_shutdown_before_start_returns_without_fetching() {
    let transport = Arc::new(CountingTransport::default());
    let worker = Worker::builder()
        .transport(transport.clone())
        .queues(vec!["default"])
        .build()
        .unwrap();

    worker.shutdown();

    tokio::time::timeout(Duration::from_secs(1), worker.start())
        .await
        .expect("worker.start() should observe the pre-start shutdown request")
        .expect("start() should return Ok");

    assert_eq!(
        transport.calls.load(Ordering::SeqCst),
        0,
        "pre-start shutdown must prevent fetches, heartbeats, and terminal reporting"
    );
}

// ---------------------------------------------------------------------------
// Test: a terminal report that can never complete is cancelled at the report
// deadline and then force-nacked exactly once (ACK variant)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_grace_expiry_cancels_a_permanently_pending_ack_then_force_nacks_once() {
    let transport = Arc::new(BlockingReportTransport::new(
        BlockedTerminalReport::Ack,
        make_job("job-report-ack-001", "test.blocked_ack"),
    ));
    let worker = Arc::new(
        Worker::builder()
            .transport(transport.clone())
            .queues(vec!["default"])
            .concurrency(1)
            .poll_interval(Duration::from_millis(10))
            .heartbeat_interval(Duration::from_secs(60))
            .grace_period(Duration::from_millis(50))
            .build()
            .unwrap(),
    );

    worker
        .register("test.blocked_ack", |_ctx: JobContext| async move {
            Ok(json!({ "ok": true }))
        })
        .await;

    let worker_for_start = worker.clone();
    let handle = tokio::spawn(async move { worker_for_start.start().await });

    transport.wait_for_blocked_report().await;
    let shutdown_started = tokio::time::Instant::now();
    worker.shutdown();

    // The SDK gives an in-flight report three seconds of the shared
    // five-second forced-shutdown budget before cancelling it.
    tokio::time::timeout(Duration::from_millis(5_500), handle)
        .await
        .expect("worker.start() should return after cancelling the blocked ACK report")
        .expect("worker task should not panic")
        .expect("start() should return Ok");

    assert_eq!(transport.ack_attempts.load(Ordering::SeqCst), 1);
    assert_eq!(
        transport.shutdown_nack_attempts.load(Ordering::SeqCst),
        1,
        "a permanently pending ACK must be cancelled and replaced by exactly one forced NACK"
    );
    assert!(
        shutdown_started.elapsed() >= Duration::from_millis(2_500),
        "the in-flight ACK must be awaited before it is cancelled"
    );
    assert!(
        shutdown_started.elapsed() < Duration::from_millis(5_500),
        "cancel-then-force-nack must still fit inside the shared shutdown deadline"
    );
}

// ---------------------------------------------------------------------------
// Test: same guarantee for a handler NACK that can never complete
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_grace_expiry_cancels_a_permanently_pending_handler_nack_then_force_nacks_once() {
    let transport = Arc::new(BlockingReportTransport::new(
        BlockedTerminalReport::HandlerNack,
        make_job("job-report-nack-001", "test.blocked_nack"),
    ));
    let worker = Arc::new(
        Worker::builder()
            .transport(transport.clone())
            .queues(vec!["default"])
            .concurrency(1)
            .poll_interval(Duration::from_millis(10))
            .heartbeat_interval(Duration::from_secs(60))
            .grace_period(Duration::from_millis(50))
            .build()
            .unwrap(),
    );

    worker
        .register("test.blocked_nack", |_ctx: JobContext| async move {
            Err(OjsError::Handler("expected failure".into()))
        })
        .await;

    let worker_for_start = worker.clone();
    let handle = tokio::spawn(async move { worker_for_start.start().await });

    transport.wait_for_blocked_report().await;
    worker.shutdown();

    tokio::time::timeout(Duration::from_millis(5_500), handle)
        .await
        .expect("worker.start() should return after cancelling the blocked handler NACK")
        .expect("worker task should not panic")
        .expect("start() should return Ok");

    assert_eq!(transport.handler_nack_attempts.load(Ordering::SeqCst), 1);
    assert_eq!(
        transport.shutdown_nack_attempts.load(Ordering::SeqCst),
        1,
        "a permanently pending handler NACK must be cancelled and replaced by exactly one forced NACK"
    );
    assert_eq!(
        transport.ack_attempts.load(Ordering::SeqCst),
        0,
        "the failed job must never be ACKed"
    );
}

// ---------------------------------------------------------------------------
// Test: an ACK still in flight at grace expiry is awaited to completion and
// never duplicated by a forced NACK
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_grace_expiry_awaits_in_flight_ack_that_completes_before_the_report_deadline() {
    let transport = Arc::new(SlowReportTransport::new(
        make_job("job-slow-ack-001", "test.slow_ack"),
        Duration::from_millis(400),
    ));
    let worker = Arc::new(
        Worker::builder()
            .transport(transport.clone())
            .queues(vec!["default"])
            .concurrency(1)
            .poll_interval(Duration::from_millis(10))
            .heartbeat_interval(Duration::from_secs(60))
            .grace_period(Duration::from_millis(20))
            .build()
            .unwrap(),
    );

    worker
        .register("test.slow_ack", |_ctx: JobContext| async move {
            Ok(json!({ "ok": true }))
        })
        .await;

    let worker_for_start = worker.clone();
    let handle = tokio::spawn(async move { worker_for_start.start().await });

    // The handler finishes immediately; its ACK is still in flight when the
    // 20ms grace period expires.
    transport.wait_for_ack_start().await;
    worker.shutdown();

    tokio::time::timeout(Duration::from_secs(3), handle)
        .await
        .expect("worker.start() should return once the in-flight ACK completes")
        .expect("worker task should not panic")
        .expect("start() should return Ok");

    assert_eq!(transport.ack_started.load(Ordering::SeqCst), 1);
    assert_eq!(
        transport.ack_completed.load(Ordering::SeqCst),
        1,
        "an ACK in flight at grace expiry must be allowed to finish"
    );
    assert_eq!(
        transport.shutdown_nack_attempts.load(Ordering::SeqCst),
        0,
        "a completed in-flight ACK must never be followed by a forced NACK"
    );
}

// ---------------------------------------------------------------------------
// Test: a fast ACK racing grace expiry is reported exactly once
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_fast_terminal_report_racing_grace_expiry_is_never_duplicated() {
    // A zero grace period makes forced shutdown reporting race the handler's
    // own fast ACK/NACK on every iteration.
    for attempt in 0..50 {
        let job_id = format!("job-race-{attempt:03}");
        let transport = Arc::new(RecordingReportTransport::new(make_job(
            &job_id,
            "test.race_report",
        )));
        let worker = Arc::new(
            Worker::builder()
                .transport(transport.clone())
                .queues(vec!["default"])
                .concurrency(1)
                .poll_interval(Duration::from_millis(1))
                .heartbeat_interval(Duration::from_secs(60))
                .grace_period(Duration::ZERO)
                .build()
                .unwrap(),
        );

        worker
            .register("test.race_report", |_ctx: JobContext| async move {
                Ok(json!({ "ok": true }))
            })
            .await;

        let worker_for_start = worker.clone();
        let handle = tokio::spawn(async move { worker_for_start.start().await });

        transport.wait_for_fetch().await;
        worker.shutdown();

        tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("worker.start() should return promptly")
            .expect("worker task should not panic")
            .expect("start() should return Ok");

        let reports = transport.reports();
        assert_eq!(
            reports.len(),
            1,
            "attempt {attempt}: expected exactly one terminal report, got {reports:?}"
        );
        assert_eq!(reports[0].0, job_id);
    }
}

// ---------------------------------------------------------------------------
// Test: 1000 concurrent jobs are each reported exactly once at shutdown
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_grace_expiry_reports_one_thousand_jobs_exactly_once() {
    const JOB_COUNT: usize = 1_000;

    let transport = Arc::new(MassShutdownTransport::new(
        JOB_COUNT,
        Duration::from_millis(250),
    ));
    let worker = Arc::new(
        Worker::builder()
            .transport(transport.clone())
            .queues(vec!["default"])
            .concurrency(JOB_COUNT)
            .poll_interval(Duration::from_millis(1))
            .heartbeat_interval(Duration::from_secs(60))
            .grace_period(Duration::from_millis(50))
            .build()
            .unwrap(),
    );

    // Half of the jobs complete immediately (their ACK is slow enough to
    // still be in flight at grace expiry); the other half never finish and
    // must be force-nacked.
    worker
        .register("test.mass_ack", |_ctx: JobContext| async move {
            Ok(json!({ "ok": true }))
        })
        .await;
    worker
        .register("test.mass_hang", |_ctx: JobContext| async move {
            std::future::pending::<HandlerResult>().await
        })
        .await;

    let worker_for_start = worker.clone();
    let handle = tokio::spawn(async move { worker_for_start.start().await });

    tokio::time::timeout(
        Duration::from_secs(20),
        transport.wait_for_all_reports_started(),
    )
    .await
    .expect("every job should be fetched and have started reporting or be hanging");
    worker.shutdown();

    tokio::time::timeout(Duration::from_secs(20), handle)
        .await
        .expect("worker.start() should finish all terminal reporting within its deadlines")
        .expect("worker task should not panic")
        .expect("start() should return Ok");

    let reports = transport.reports();
    let mut by_job: std::collections::HashMap<String, Vec<&'static str>> =
        std::collections::HashMap::new();
    for (job_id, kind) in &reports {
        by_job.entry(job_id.clone()).or_default().push(kind);
    }

    assert_eq!(
        by_job.len(),
        JOB_COUNT,
        "every one of the {JOB_COUNT} jobs must be reported"
    );
    for index in 0..JOB_COUNT {
        let job_id = format!("job-mass-{index:04}");
        let kinds = by_job
            .get(&job_id)
            .unwrap_or_else(|| panic!("{job_id} was never reported"));
        assert_eq!(
            kinds.len(),
            1,
            "{job_id} must be reported exactly once, got {kinds:?}"
        );
        let expected = if index % 2 == 0 {
            "ack"
        } else {
            "shutdown_nack"
        };
        assert_eq!(
            kinds[0], expected,
            "unexpected terminal report for {job_id}"
        );
    }
    assert_eq!(reports.len(), JOB_COUNT);
}

// ---------------------------------------------------------------------------
// Test: grace expiry begins forced reporting before waiting for heartbeat
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_grace_expiry_force_nacks_before_waiting_for_heartbeat_termination() {
    let transport = Arc::new(HeartbeatOrderingTransport::new(make_job(
        "job-heartbeat-order-001",
        "test.pending_for_heartbeat_order",
    )));
    let worker = Arc::new(
        Worker::builder()
            .transport(transport.clone())
            .queues(vec!["default"])
            .concurrency(1)
            .poll_interval(Duration::from_millis(10))
            .heartbeat_interval(Duration::from_millis(10))
            .grace_period(Duration::from_millis(50))
            .build()
            .unwrap(),
    );

    worker
        .register(
            "test.pending_for_heartbeat_order",
            |_ctx: JobContext| async move { std::future::pending::<HandlerResult>().await },
        )
        .await;

    let worker_for_start = worker.clone();
    let handle = tokio::spawn(async move { worker_for_start.start().await });

    tokio::time::timeout(Duration::from_secs(1), transport.wait_for_heartbeat())
        .await
        .expect("heartbeat should begin while the job is active");
    worker.shutdown();

    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("worker should finish after the grace period and forced NACK")
        .expect("worker task should not panic")
        .expect("start() should return Ok");

    assert_eq!(transport.shutdown_nack_attempts.load(Ordering::SeqCst), 1);
    assert!(
        !transport
            .shutdown_nack_saw_terminated_heartbeat
            .load(Ordering::SeqCst),
        "forced NACK must start before waiting for the aborted heartbeat task to terminate"
    );
}

// ---------------------------------------------------------------------------
// Test: CPU-blocking handlers cannot suppress forced NACK or deadline return
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_grace_expiry_cpu_blocking_handler_is_force_nacked_once_and_cannot_delay_return() {
    let transport = Arc::new(CpuBlockingHandlerTransport::new(make_job(
        "job-cpu-blocking-001",
        "test.cpu_blocking",
    )));
    let worker = Arc::new(
        Worker::builder()
            .transport(transport.clone())
            .queues(vec!["default"])
            .concurrency(1)
            .poll_interval(Duration::from_millis(10))
            .heartbeat_interval(Duration::from_secs(60))
            .grace_period(Duration::from_millis(25))
            .build()
            .unwrap(),
    );

    let handler_started = Arc::new(Notify::new());
    let handler_finished = Arc::new(AtomicBool::new(false));
    worker
        .register("test.cpu_blocking", {
            let handler_started = handler_started.clone();
            let handler_finished = handler_finished.clone();
            move |_ctx: JobContext| {
                let handler_started = handler_started.clone();
                let handler_finished = handler_finished.clone();
                async move {
                    handler_started.notify_one();
                    // Deliberately block a Tokio worker thread beyond the
                    // SDK's five-second forced-shutdown budget.
                    std::thread::sleep(Duration::from_secs(6));
                    handler_finished.store(true, Ordering::SeqCst);
                    Ok(json!({ "ok": true }))
                }
            }
        })
        .await;

    let worker_for_start = worker.clone();
    let handle = tokio::spawn(async move { worker_for_start.start().await });

    handler_started.notified().await;
    let shutdown_started = tokio::time::Instant::now();
    worker.shutdown();

    tokio::time::timeout(Duration::from_secs(1), transport.wait_for_shutdown_nack())
        .await
        .expect("forced NACK should begin while the handler task is still blocked");
    assert!(!handler_finished.load(Ordering::SeqCst));

    tokio::time::timeout(Duration::from_millis(5_750), handle)
        .await
        .expect("one forced-shutdown deadline must bound task joining")
        .expect("worker task should not panic")
        .expect("start() should return Ok");
    assert!(
        shutdown_started.elapsed() < Duration::from_millis(5_750),
        "shutdown exceeded its single five-second forced deadline"
    );
    assert_eq!(transport.shutdown_nack_attempts.load(Ordering::SeqCst), 1);

    tokio::time::timeout(Duration::from_secs(2), async {
        while !handler_finished.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("blocked handler should eventually resume");
    assert_eq!(
        transport.ack_attempts.load(Ordering::SeqCst),
        0,
        "the later-resuming handler must not ACK after forced reporting claimed the job"
    );
    assert_eq!(
        transport.shutdown_nack_attempts.load(Ordering::SeqCst),
        1,
        "terminal reporting must remain exactly once"
    );
}

// ---------------------------------------------------------------------------
// Test: Grace period expiry force-nacks jobs still active at the deadline
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_grace_period_expiry_force_nacks_abandoned_job() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/fetch"))
        .respond_with(MultiCallFetchResponder::once(json!({
            "jobs": [make_job("job-grace-timeout-001", "test.never_finishes")]
        })))
        .mount(&server)
        .await;

    // The handler never acks (it outlives the grace period), so the ack
    // endpoint must never be called...
    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/ack"))
        .respond_with(ResponseTemplate::new(204))
        .expect(0)
        .mount(&server)
        .await;

    // ...but the worker must force-nack it once the grace period expires,
    // releasing the server-side claim instead of silently abandoning it.
    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/nack"))
        .and(body_partial_json(json!({
            "job_id": "job-grace-timeout-001",
            "error": {
                "code": "shutdown",
                "retryable": true
            }
        })))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    mount_heartbeat(&server).mount(&server).await;

    let worker = Arc::new(
        Worker::builder()
            .url(server.uri())
            .queues(vec!["default"])
            .concurrency(1)
            .poll_interval(Duration::from_millis(50))
            .heartbeat_interval(Duration::from_secs(60))
            .grace_period(Duration::from_millis(200))
            .build()
            .unwrap(),
    );

    worker
        .register("test.never_finishes", |_ctx: JobContext| async move {
            // Sleeps far longer than the grace period; the worker must not
            // wait for it and must not silently abandon it either.
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok(json!({"ok": true}))
        })
        .await;

    let worker_for_start = worker.clone();
    let handle = tokio::spawn(async move { worker_for_start.start().await });

    tokio::time::sleep(Duration::from_millis(50)).await;
    worker.shutdown();

    tokio::time::timeout(Duration::from_secs(3), handle)
        .await
        .expect("worker.start() should return once the grace period expires and the job is force-nacked")
        .expect("worker task should not panic")
        .expect("start() should return Ok");
}

// ---------------------------------------------------------------------------
// Test: Worker handles fetch errors with backoff (no crash)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_worker_survives_fetch_errors() {
    let server = MockServer::start().await;
    let handler_called = Arc::new(AtomicBool::new(false));
    let handler_called_clone = handler_called.clone();

    // First call returns 500, second returns a job, third returns empty
    struct ErrorThenSuccessResponder {
        call_count: AtomicUsize,
    }
    impl Respond for ErrorThenSuccessResponder {
        fn respond(&self, _request: &wiremock::Request) -> ResponseTemplate {
            let count = self.call_count.fetch_add(1, Ordering::SeqCst);
            match count {
                0 => ResponseTemplate::new(500).set_body_json(json!({"error": "temporary"})),
                1 => ResponseTemplate::new(200).set_body_json(json!({
                    "jobs": [make_job("job-retry-001", "test.after_error")]
                })),
                _ => ResponseTemplate::new(200).set_body_json(json!({"jobs": []})),
            }
        }
    }

    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/fetch"))
        .respond_with(ErrorThenSuccessResponder {
            call_count: AtomicUsize::new(0),
        })
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/ack"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    mount_heartbeat(&server).mount(&server).await;

    let worker = Worker::builder()
        .url(server.uri())
        .queues(vec!["default"])
        .concurrency(1)
        .poll_interval(Duration::from_millis(50))
        .heartbeat_interval(Duration::from_secs(60))
        .build()
        .unwrap();

    worker
        .register("test.after_error", move |_ctx: JobContext| {
            let flag = handler_called_clone.clone();
            async move {
                flag.store(true, Ordering::SeqCst);
                Ok(json!({"recovered": true}))
            }
        })
        .await;

    let handle = tokio::spawn(async move { worker.start().await });
    tokio::time::sleep(Duration::from_millis(800)).await;
    handle.abort();
    let _ = handle.await;

    assert!(
        handler_called.load(Ordering::SeqCst),
        "worker must recover from fetch errors and process subsequent jobs"
    );
}

// ---------------------------------------------------------------------------
// Test: Heartbeat reports active_job_ids in a deterministic (sorted) order
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_heartbeat_active_job_ids_are_sorted() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/ojs/v1/workers/fetch"))
        .respond_with(MultiCallFetchResponder::once(json!({
            "jobs": [
                make_job("job-z", "test.slow_heartbeat"),
                make_job("job-a", "test.slow_heartbeat"),
                make_job("job-m", "test.slow_heartbeat")
            ]
        })))
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
        .concurrency(3)
        .poll_interval(Duration::from_millis(50))
        .heartbeat_interval(Duration::from_millis(100))
        .build()
        .unwrap();

    // Handlers stay active long enough to be included in at least one
    // heartbeat tick together (job ids "z", "a", "m" are intentionally out
    // of alphabetical order to catch a non-deterministic HashSet iteration).
    worker
        .register("test.slow_heartbeat", |_ctx: JobContext| async move {
            tokio::time::sleep(Duration::from_millis(400)).await;
            Ok(json!({"ok": true}))
        })
        .await;

    let handle = tokio::spawn(async move { worker.start().await });
    tokio::time::sleep(Duration::from_millis(600)).await;
    handle.abort();
    let _ = handle.await;

    let requests = server
        .received_requests()
        .await
        .expect("request recording should be enabled by default");

    let heartbeats_with_jobs: Vec<Vec<String>> = requests
        .iter()
        .filter(|r| r.url.path() == "/ojs/v1/workers/heartbeat")
        .filter_map(|r| {
            let body: serde_json::Value = serde_json::from_slice(&r.body).ok()?;
            let jobs = body.get("active_jobs")?.as_array()?;
            if jobs.is_empty() {
                return None;
            }
            Some(
                jobs.iter()
                    .map(|v| v.as_str().unwrap().to_string())
                    .collect(),
            )
        })
        .collect();

    assert!(
        !heartbeats_with_jobs.is_empty(),
        "expected at least one heartbeat while jobs were active"
    );
    for job_ids in heartbeats_with_jobs {
        let mut sorted = job_ids.clone();
        sorted.sort_unstable();
        assert_eq!(
            job_ids, sorted,
            "active_job_ids must be sent in sorted (deterministic) order"
        );
    }
}
