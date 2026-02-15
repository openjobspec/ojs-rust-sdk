//! Tracing middleware for the OJS Rust SDK.
//!
//! Provides a [`TracingMiddleware`] that instruments job processing with
//! structured [`tracing`] spans and events. This is compatible with any
//! `tracing` subscriber, including `tracing-opentelemetry` for bridging
//! to OpenTelemetry collectors.
//!
//! Enable via the `tracing-middleware` feature:
//!
//! ```toml
//! [dependencies]
//! ojs = { version = "0.1", features = ["tracing-middleware"] }
//! ```
//!
//! # Usage
//!
//! ```rust,no_run
//! use ojs::tracing_mw::TracingMiddleware;
//! use ojs::Worker;
//!
//! # fn main() -> ojs::Result<()> {
//! let worker = Worker::builder()
//!     .url("http://localhost:8080")
//!     .build()?;
//!
//! // worker.use_middleware("tracing", TracingMiddleware::new()).await;
//! # Ok(())
//! # }
//! ```

use std::future::Future;
use std::pin::Pin;
use std::time::Instant;

use crate::middleware::{HandlerResult, Middleware, Next};
use crate::worker::JobContext;

/// Tracing middleware that instruments job processing with structured spans.
///
/// Creates an `info_span!("process")` for each job execution with semantic
/// attributes following OJS conventions. Logs completion/failure as events
/// with elapsed duration.
///
/// For OpenTelemetry integration, combine with the
/// [`tracing-opentelemetry`](https://docs.rs/tracing-opentelemetry) subscriber
/// layer.
pub struct TracingMiddleware {
    span_name: String,
}

impl TracingMiddleware {
    /// Creates a new tracing middleware with the default span name `"process"`.
    pub fn new() -> Self {
        Self {
            span_name: "process".to_string(),
        }
    }

    /// Creates a new tracing middleware with a custom span name.
    pub fn with_span_name(name: impl Into<String>) -> Self {
        Self {
            span_name: name.into(),
        }
    }
}

impl Default for TracingMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

impl Middleware for TracingMiddleware {
    fn handle(
        &self,
        ctx: JobContext,
        next: Next,
    ) -> Pin<Box<dyn Future<Output = HandlerResult> + Send + 'static>> {
        let _span_name = self.span_name.clone();
        let job_type = ctx.job.job_type.clone();
        let job_id = ctx.job.id.clone();
        let queue = ctx.job.queue.clone();
        let attempt = ctx.job.attempt;

        Box::pin(async move {
            // Log span start using tracing (bridges to OTel via tracing-opentelemetry)
            let span = tracing::info_span!(
                "process",
                otel.kind = "CONSUMER",
                messaging.system = "ojs",
                messaging.operation = "process",
                ojs.job.r#type = %job_type,
                ojs.job.id = %job_id,
                ojs.job.queue = %queue,
                ojs.job.attempt = attempt,
            );

            let _guard = span.enter();
            let start = Instant::now();

            let result = next.run(ctx).await;
            let duration_secs = start.elapsed().as_secs_f64();

            match &result {
                Ok(_) => {
                    tracing::info!(
                        ojs.job.r#type = %job_type,
                        ojs.job.queue = %queue,
                        duration_s = duration_secs,
                        "job completed"
                    );
                }
                Err(e) => {
                    tracing::error!(
                        ojs.job.r#type = %job_type,
                        ojs.job.queue = %queue,
                        duration_s = duration_secs,
                        error = %e,
                        "job failed"
                    );
                }
            }

            result
        })
    }
}
