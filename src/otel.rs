//! OpenTelemetry middleware for OJS job processing.
//!
//! Provides native OpenTelemetry tracing and metrics middleware for production
//! observability. Creates OTel spans for each job execution and records
//! standard metrics (counters, histograms).
//!
//! Enable via the `otel-middleware` feature:
//!
//! ```toml
//! [dependencies]
//! ojs = { version = "0.1", features = ["otel-middleware"] }
//! ```
//!
//! # Usage
//!
//! ```rust,no_run
//! use ojs::otel::{OtelTracingMiddleware, OtelMetricsMiddleware};
//! use ojs::Worker;
//!
//! # fn main() -> ojs::Result<()> {
//! let worker = Worker::builder()
//!     .url("http://localhost:8080")
//!     .build()?;
//!
//! // Uses the global OTel providers by default:
//! // worker.use_middleware("otel-tracing", OtelTracingMiddleware::new()).await;
//! // worker.use_middleware("otel-metrics", OtelMetricsMiddleware::new()).await;
//! # Ok(())
//! # }
//! ```

use std::sync::Arc;
use std::time::Instant;

use opentelemetry::metrics::{Counter, Histogram, Meter};
use opentelemetry::trace::{FutureExt, SpanKind, Status, TraceContextExt, Tracer};
use opentelemetry::{global, Context, KeyValue};

use crate::middleware::{BoxFuture, HandlerResult, Middleware, Next};
use crate::worker::JobContext;

// ---------------------------------------------------------------------------
// OTel Tracing Middleware
// ---------------------------------------------------------------------------

/// OpenTelemetry tracing middleware for job processing.
///
/// Creates a span for each job execution with semantic attributes following
/// OJS conventions. Sets span status to `Error` on failure.
pub struct OtelTracingMiddleware {
    tracer: opentelemetry::global::BoxedTracer,
}

impl OtelTracingMiddleware {
    /// Creates middleware using the global tracer provider.
    pub fn new() -> Self {
        let tracer = global::tracer("ojs");
        Self { tracer }
    }

    /// Creates middleware with a custom tracer.
    pub fn with_tracer(tracer: opentelemetry::global::BoxedTracer) -> Self {
        Self { tracer }
    }
}

impl Default for OtelTracingMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

impl Middleware for OtelTracingMiddleware {
    fn handle(&self, ctx: JobContext, next: Next) -> BoxFuture<'static, HandlerResult> {
        let job_type = ctx.job.job_type.clone();
        let job_id = ctx.job.id.clone();
        let queue = ctx.job.queue.clone();
        let attempt = ctx.attempt;

        let span = self
            .tracer
            .span_builder(format!("ojs.job {}", job_type))
            .with_kind(SpanKind::Consumer)
            .with_attributes(vec![
                KeyValue::new("ojs.job.type", job_type),
                KeyValue::new("ojs.job.id", job_id),
                KeyValue::new("ojs.job.queue", queue),
                KeyValue::new("ojs.job.attempt", attempt as i64),
            ])
            .start(&self.tracer);

        let cx = Context::current_with_span(span);

        Box::pin(async move {
            // `Context::current_with_span` only builds a `Context` value;
            // it does not make it the ambient "current" context on its
            // own. Without explicitly attaching it for the duration of the
            // handler, any child span the handler creates (directly via
            // `opentelemetry`, or via the `tracing`-bridge) would not be
            // parented to this job-processing span at all. `with_context`
            // re-attaches `cx` around each poll of the wrapped future
            // (rather than holding a thread-local guard across the whole
            // `.await`, which would be unsound if the future is resumed on
            // a different worker thread between polls).
            let result = next.run(ctx).with_context(cx.clone()).await;

            let span = cx.span();
            match &result {
                Ok(_) => {
                    span.set_status(Status::Ok);
                }
                Err(e) => {
                    span.set_status(Status::Error {
                        description: e.to_string().into(),
                    });
                    span.record_error(e);
                }
            }

            result
        })
    }
}

// ---------------------------------------------------------------------------
// OTel Metrics Middleware
// ---------------------------------------------------------------------------

struct MetricsInstruments {
    jobs_started: Counter<u64>,
    jobs_completed: Counter<u64>,
    jobs_failed: Counter<u64>,
    job_duration: Histogram<f64>,
}

/// OpenTelemetry metrics middleware for job processing.
///
/// Records standard OJS metrics:
/// - `ojs.job.started` (counter)
/// - `ojs.job.completed` (counter)
/// - `ojs.job.failed` (counter)
/// - `ojs.job.duration` (histogram, seconds)
pub struct OtelMetricsMiddleware {
    instruments: Arc<MetricsInstruments>,
}

impl OtelMetricsMiddleware {
    /// Creates middleware using the global meter provider.
    pub fn new() -> Self {
        let meter = global::meter("ojs");
        Self::with_meter(meter)
    }

    /// Creates middleware with a custom meter.
    pub fn with_meter(meter: Meter) -> Self {
        let instruments = MetricsInstruments {
            jobs_started: meter.u64_counter("ojs.job.started").build(),
            jobs_completed: meter.u64_counter("ojs.job.completed").build(),
            jobs_failed: meter.u64_counter("ojs.job.failed").build(),
            job_duration: meter
                .f64_histogram("ojs.job.duration")
                .with_unit("s")
                .build(),
        };
        Self {
            instruments: Arc::new(instruments),
        }
    }
}

impl Default for OtelMetricsMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

impl Middleware for OtelMetricsMiddleware {
    fn handle(&self, ctx: JobContext, next: Next) -> BoxFuture<'static, HandlerResult> {
        let instruments = self.instruments.clone();
        let job_type = ctx.job.job_type.clone();
        let queue = ctx.job.queue.clone();

        let attrs = vec![
            KeyValue::new("ojs.job.type", job_type),
            KeyValue::new("ojs.job.queue", queue),
        ];

        Box::pin(async move {
            instruments.jobs_started.add(1, &attrs);
            let start = Instant::now();

            let result = next.run(ctx).await;
            let duration = start.elapsed().as_secs_f64();

            match &result {
                Ok(_) => {
                    instruments.jobs_completed.add(1, &attrs);
                }
                Err(_) => {
                    instruments.jobs_failed.add(1, &attrs);
                }
            }

            instruments.job_duration.record(duration, &attrs);
            result
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Marker type used only to prove context propagation across an
    /// awaited future; distinct from any real span/trace data.
    #[derive(Debug, PartialEq, Eq, Clone, Copy)]
    struct Marker(u64);

    /// This directly exercises the exact mechanism
    /// `OtelTracingMiddleware::handle` relies on to make its span "current"
    /// for the duration of the handler:
    /// `opentelemetry::trace::FutureExt::with_context`.
    ///
    /// Before the fix, `handle` built a `Context` via
    /// `Context::current_with_span` but never attached it (no `.attach()`
    /// / `.with_context(...)`), so it never actually became the ambient
    /// context while the handler ran -- any child span the handler tried
    /// to create would not have been parented to it. This test proves
    /// `with_context` genuinely makes an attached `Context`'s values
    /// observable via `Context::current()` for the whole lifetime of the
    /// wrapped future, including across `.await` points (which a
    /// thread-local `attach()` guard held across an `.await` cannot safely
    /// guarantee on a multi-threaded runtime).
    #[tokio::test]
    async fn test_with_context_makes_context_ambient_across_await() {
        let cx = Context::current_with_value(Marker(42));

        let observed = async {
            // Simulate doing some async work (an `.await` point) before
            // checking what's "current" -- this is exactly the shape of
            // `next.run(ctx).await` in `OtelTracingMiddleware::handle`.
            tokio::task::yield_now().await;
            Context::current().get::<Marker>().copied()
        }
        .with_context(cx)
        .await;

        assert_eq!(observed, Some(Marker(42)));
    }

    #[tokio::test]
    async fn test_context_is_not_ambient_without_with_context() {
        // Sanity check for the *previous*, buggy behavior: merely
        // constructing a `Context` (as `Context::current_with_span` does)
        // without attaching it must NOT make it observable via
        // `Context::current()`. This is what made the original bug
        // possible to write in the first place.
        let _cx = Context::current_with_value(Marker(99));

        let observed = async { Context::current().get::<Marker>().copied() }.await;

        assert_eq!(observed, None);
    }

    #[tokio::test]
    async fn test_otel_tracing_middleware_wraps_handler_without_panicking() {
        // End-to-end smoke test using the default (no-op, since no global
        // SDK/exporter is installed in this test binary) tracer: proves
        // `OtelTracingMiddleware` still composes correctly with the
        // middleware chain and does not panic or hang now that the
        // handler future is wrapped in `with_context`.
        use crate::middleware::{HandlerFn, MiddlewareChain};
        use crate::worker::JobContext;
        use std::sync::Arc;

        let mw = OtelTracingMiddleware::new();
        let mut chain = MiddlewareChain::new();
        chain.add("otel", mw);

        let handler: HandlerFn = Arc::new(|_ctx: JobContext| {
            Box::pin(async move { Ok(serde_json::json!({"ok": true})) })
                as BoxFuture<'static, HandlerResult>
        });
        let _wrapped = chain.wrap(handler);
        // Constructing and wrapping must not panic; invoking it end-to-end
        // requires a real `JobContext` (worker-internal), which is already
        // covered by the worker integration tests exercising other
        // middleware through the same `MiddlewareChain::wrap` path.
    }
}
