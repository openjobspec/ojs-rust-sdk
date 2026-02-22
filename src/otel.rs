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
use opentelemetry::trace::{SpanKind, Status, TraceContextExt, Tracer};
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
            let result = next.run(ctx).await;

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
