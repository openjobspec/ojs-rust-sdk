//! Metrics recording middleware for OJS job processing.
//!
//! Provides a pluggable [`MetricsRecorder`] trait for recording job
//! execution metrics to any backend (Prometheus, StatsD, etc.).
//!
//! # Example
//!
//! ```rust,no_run
//! use ojs::middleware_common::metrics::{MetricsMiddleware, MetricsRecorder};
//! use std::sync::Arc;
//!
//! struct MyRecorder;
//!
//! impl MetricsRecorder for MyRecorder {
//!     fn job_started(&self, job_type: &str, queue: &str) {}
//!     fn job_completed(&self, job_type: &str, queue: &str, duration_ms: u128) {}
//!     fn job_failed(&self, job_type: &str, queue: &str, duration_ms: u128, error: &str) {}
//! }
//!
//! let mw = MetricsMiddleware::new(Arc::new(MyRecorder));
//! // worker.use_middleware("metrics", mw);
//! ```

use std::sync::Arc;
use std::time::Instant;

use crate::middleware::{BoxFuture, HandlerResult, Middleware, Next};
use crate::worker::JobContext;

/// Trait for recording job execution metrics.
///
/// Implement this trait to forward metrics to Prometheus, StatsD,
/// Datadog, or any other metrics system.
pub trait MetricsRecorder: Send + Sync + 'static {
    /// Called when a job starts processing.
    fn job_started(&self, job_type: &str, queue: &str);

    /// Called when a job completes successfully.
    fn job_completed(&self, job_type: &str, queue: &str, duration_ms: u128);

    /// Called when a job fails with an error.
    fn job_failed(&self, job_type: &str, queue: &str, duration_ms: u128, error: &str);
}

/// Middleware that records job execution metrics via a [`MetricsRecorder`].
pub struct MetricsMiddleware {
    recorder: Arc<dyn MetricsRecorder>,
}

impl MetricsMiddleware {
    /// Creates a new metrics middleware with the given recorder.
    pub fn new(recorder: Arc<dyn MetricsRecorder>) -> Self {
        Self { recorder }
    }
}

impl Middleware for MetricsMiddleware {
    fn handle(&self, ctx: JobContext, next: Next) -> BoxFuture<'static, HandlerResult> {
        let recorder = self.recorder.clone();
        let job_type = ctx.job.job_type.clone();
        let queue = ctx.job.queue.clone();

        Box::pin(async move {
            recorder.job_started(&job_type, &queue);
            let start = Instant::now();

            let result = next.run(ctx).await;
            let duration_ms = start.elapsed().as_millis();

            match &result {
                Ok(_) => recorder.job_completed(&job_type, &queue, duration_ms),
                Err(e) => recorder.job_failed(&job_type, &queue, duration_ms, &e.to_string()),
            }

            result
        })
    }
}
