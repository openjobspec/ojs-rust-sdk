//! Logging middleware for OJS job processing.
//!
//! Logs job start, completion, and failure events with timing information
//! using the [`tracing`] crate.
//!
//! # Example
//!
//! ```rust,no_run
//! use ojs::middleware_common::logging::LoggingMiddleware;
//!
//! let mw = LoggingMiddleware::new();
//! // worker.use_middleware("logging", mw);
//! ```

use std::time::Instant;

use crate::middleware::{BoxFuture, HandlerResult, Middleware, Next};
use crate::worker::JobContext;

/// Middleware that logs job lifecycle events using `tracing`.
///
/// Logs job start at `DEBUG` level, completion at `INFO` level,
/// and failure at `ERROR` level, all with elapsed duration.
pub struct LoggingMiddleware;

impl LoggingMiddleware {
    /// Creates a new logging middleware.
    pub fn new() -> Self {
        Self
    }
}

impl Default for LoggingMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

impl Middleware for LoggingMiddleware {
    fn handle(&self, ctx: JobContext, next: Next) -> BoxFuture<'static, HandlerResult> {
        let job_type = ctx.job.job_type.clone();
        let job_id = ctx.job.id.clone();
        let queue = ctx.job.queue.clone();
        let attempt = ctx.job.attempt;

        Box::pin(async move {
            tracing::debug!(
                ojs.job.r#type = %job_type,
                ojs.job.id = %job_id,
                ojs.job.queue = %queue,
                ojs.job.attempt = attempt,
                "job started"
            );

            let start = Instant::now();
            let result = next.run(ctx).await;
            let duration_ms = start.elapsed().as_millis();

            match &result {
                Ok(_) => {
                    tracing::info!(
                        ojs.job.r#type = %job_type,
                        ojs.job.id = %job_id,
                        duration_ms = duration_ms,
                        "job completed"
                    );
                }
                Err(e) => {
                    tracing::error!(
                        ojs.job.r#type = %job_type,
                        ojs.job.id = %job_id,
                        duration_ms = duration_ms,
                        error = %e,
                        "job failed"
                    );
                }
            }

            result
        })
    }
}

