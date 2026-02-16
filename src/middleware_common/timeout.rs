//! Timeout middleware for OJS job processing.
//!
//! Enforces a maximum execution time for job handlers using
//! [`tokio::time::timeout`].
//!
//! # Example
//!
//! ```rust,no_run
//! use ojs::middleware_common::timeout::TimeoutMiddleware;
//! use std::time::Duration;
//!
//! let mw = TimeoutMiddleware::new(Duration::from_secs(30));
//! // worker.use_middleware("timeout", mw);
//! ```

use std::time::Duration;

use crate::errors::OjsError;
use crate::middleware::{BoxFuture, HandlerResult, Middleware, Next};
use crate::worker::JobContext;

/// Middleware that aborts job execution after a configurable timeout.
///
/// If the handler does not complete within the specified duration,
/// the job fails with an [`OjsError::Timeout`] error.
pub struct TimeoutMiddleware {
    duration: Duration,
}

impl TimeoutMiddleware {
    /// Creates a new timeout middleware with the given duration.
    pub fn new(duration: Duration) -> Self {
        Self { duration }
    }
}

impl Middleware for TimeoutMiddleware {
    fn handle(&self, ctx: JobContext, next: Next) -> BoxFuture<'static, HandlerResult> {
        let duration = self.duration;
        let job_id = ctx.job.id.clone();
        let job_type = ctx.job.job_type.clone();

        Box::pin(async move {
            match tokio::time::timeout(duration, next.run(ctx)).await {
                Ok(result) => result,
                Err(_elapsed) => Err(OjsError::Handler(format!(
                    "Job {} (id={}) timed out after {}ms",
                    job_type,
                    job_id,
                    duration.as_millis()
                ))),
            }
        })
    }
}
