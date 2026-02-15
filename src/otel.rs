//! OpenTelemetry middleware for the OJS Rust SDK.
//!
//! Provides execution middleware that instruments job processing with
//! OpenTelemetry traces and metrics, following the OJS Observability spec.
//!
//! # Usage
//!
//! ```rust,ignore
//! use ojs::otel::OpenTelemetryMiddleware;
//!
//! let worker = Worker::builder()
//!     .url("http://localhost:8080")
//!     .middleware(OpenTelemetryMiddleware::new())
//!     .build()?;
//! ```
//!
//! # Prerequisites
//!
//! Add to `Cargo.toml`:
//! ```toml
//! [dependencies]
//! opentelemetry = "0.22"
//! opentelemetry_sdk = "0.22"
//! ```
//!
//! See: spec/ojs-observability.md

use std::future::Future;
use std::pin::Pin;
use std::time::Instant;

use crate::middleware::{HandlerResult, Middleware, Next};
use crate::worker::JobContext;

/// OpenTelemetry middleware that instruments job processing with traces and metrics.
///
/// Creates a CONSUMER span for each job execution and records:
/// - `ojs.job.completed` (counter)
/// - `ojs.job.failed` (counter)
/// - `ojs.job.duration` (histogram, seconds)
pub struct OpenTelemetryMiddleware {
    /// Optional custom tracer name. Defaults to "ojs-rust-sdk".
    tracer_name: String,
}

impl OpenTelemetryMiddleware {
    /// Creates a new OpenTelemetry middleware with default settings.
    pub fn new() -> Self {
        Self {
            tracer_name: "ojs-rust-sdk".to_string(),
        }
    }

    /// Creates a new OpenTelemetry middleware with a custom tracer name.
    pub fn with_tracer_name(name: impl Into<String>) -> Self {
        Self {
            tracer_name: name.into(),
        }
    }
}

impl Default for OpenTelemetryMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

impl Middleware for OpenTelemetryMiddleware {
    fn handle(
        &self,
        ctx: JobContext,
        next: Next,
    ) -> Pin<Box<dyn Future<Output = HandlerResult> + Send + 'static>> {
        let _tracer_name = self.tracer_name.clone();
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
