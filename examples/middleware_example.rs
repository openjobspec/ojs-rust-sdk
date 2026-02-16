//! Middleware example: logging, metrics, and trace-context middleware.
//!
//! Demonstrates the composable middleware pattern with `next.run(ctx)`.
//! Middleware wraps job handlers in an onion model — first-registered middleware
//! executes outermost.
//!
//! Execution order for [logging, metrics, tracing]:
//!
//! ```text
//! logging.before → metrics.before → tracing.before → handler
//! → tracing.after → metrics.after → logging.after
//! ```
//!
//! Prerequisites: An OJS-compatible server running at http://localhost:8080.

use ojs::{BoxFuture, Client, HandlerResult, JobContext, Middleware, Next, Worker};
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Logging middleware
// ---------------------------------------------------------------------------

struct LoggingMiddleware;

impl Middleware for LoggingMiddleware {
    fn handle(&self, ctx: JobContext, next: Next) -> BoxFuture<'static, HandlerResult> {
        Box::pin(async move {
            let job_id = ctx.job.id.clone();
            let job_type = ctx.job.job_type.clone();
            let attempt = ctx.attempt;
            let start = Instant::now();

            println!(
                "[logging] Starting {} (id={}, attempt={})",
                job_type, job_id, attempt
            );

            let result = next.run(ctx).await;
            let elapsed = start.elapsed();

            match &result {
                Ok(_) => println!(
                    "[logging] Completed {} in {:.1}ms",
                    job_type,
                    elapsed.as_secs_f64() * 1000.0
                ),
                Err(e) => println!(
                    "[logging] Failed {} after {:.1}ms: {}",
                    job_type,
                    elapsed.as_secs_f64() * 1000.0,
                    e
                ),
            }

            result
        })
    }
}

// ---------------------------------------------------------------------------
// Metrics middleware
// ---------------------------------------------------------------------------

struct MetricsMiddleware {
    completed: Arc<AtomicU64>,
    failed: Arc<AtomicU64>,
}

impl MetricsMiddleware {
    fn new() -> Self {
        Self {
            completed: Arc::new(AtomicU64::new(0)),
            failed: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl Middleware for MetricsMiddleware {
    fn handle(&self, ctx: JobContext, next: Next) -> BoxFuture<'static, HandlerResult> {
        let completed = Arc::clone(&self.completed);
        let failed = Arc::clone(&self.failed);

        Box::pin(async move {
            let job_type = ctx.job.job_type.clone();
            let queue = ctx.job.queue.clone();
            let start = Instant::now();

            let result = next.run(ctx).await;
            let duration_ms = start.elapsed().as_secs_f64() * 1000.0;

            match &result {
                Ok(_) => {
                    let total = completed.fetch_add(1, Ordering::Relaxed) + 1;
                    println!(
                        "[metrics] ojs.jobs.completed type={} queue={} duration={:.1}ms total={}",
                        job_type, queue, duration_ms, total
                    );
                }
                Err(_) => {
                    let total = failed.fetch_add(1, Ordering::Relaxed) + 1;
                    println!(
                        "[metrics] ojs.jobs.failed type={} queue={} duration={:.1}ms total={}",
                        job_type, queue, duration_ms, total
                    );
                }
            }

            result
        })
    }
}

// ---------------------------------------------------------------------------
// Trace-context middleware
// ---------------------------------------------------------------------------

struct TraceContextMiddleware;

impl Middleware for TraceContextMiddleware {
    fn handle(&self, ctx: JobContext, next: Next) -> BoxFuture<'static, HandlerResult> {
        Box::pin(async move {
            if let Some(meta) = &ctx.job.meta {
                if let Some(trace_id) = meta.get("trace_id").and_then(|v| v.as_str()) {
                    // In production, restore OpenTelemetry span context here.
                    println!("[trace] Restoring trace context: {}", trace_id);
                }
            }
            next.run(ctx).await
        })
    }
}

// ---------------------------------------------------------------------------
// Job handlers
// ---------------------------------------------------------------------------

async fn handle_email_send(ctx: JobContext) -> HandlerResult {
    let to: String = ctx.job.arg("to")?;
    let subject: String = ctx
        .job
        .arg("subject")
        .unwrap_or_else(|_| "No Subject".into());

    println!("  Sending email to {} with subject: {}", to, subject);
    tokio::time::sleep(Duration::from_millis(100)).await;

    Ok(json!({
        "message_id": format!("msg_{}", ctx.job.id),
        "delivered": true,
    }))
}

async fn handle_report_generate(ctx: JobContext) -> HandlerResult {
    let report_id: u64 = ctx.job.arg("report_id")?;
    println!("  Generating report #{}", report_id);

    for i in 0..3 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        ctx.heartbeat().await?;
        println!("  Report #{} progress: {}%", report_id, (i + 1) * 33);
    }

    Ok(json!({
        "report_id": report_id,
        "url": format!("https://example.com/reports/{}.pdf", report_id),
    }))
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> ojs::Result<()> {
    // ------------------------------------------------------------------
    // Client-side: inject trace context when enqueuing
    // ------------------------------------------------------------------
    // Note: The Rust SDK does not have a client-side middleware chain.
    // Use a helper function to inject metadata before enqueuing.

    let client = Client::builder().url("http://localhost:8080").build()?;

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let trace_id = format!("trace_{:016x}", nanos);

    let mut meta = HashMap::new();
    meta.insert("trace_id".to_string(), json!(trace_id));
    meta.insert("enqueued_by".to_string(), json!("my-service"));

    let job = client
        .enqueue(
            "email.send",
            json!({"to": "user@example.com", "subject": "Hello"}),
        )
        .queue("email")
        .meta(meta)
        .send()
        .await?;

    println!(
        "[enqueue] Created job {} with trace_id={}",
        job.id, trace_id
    );

    // ------------------------------------------------------------------
    // Worker-side: composable middleware chain
    // ------------------------------------------------------------------

    let worker = Worker::builder()
        .url("http://localhost:8080")
        .queues(vec!["email", "default"])
        .concurrency(10)
        .grace_period(Duration::from_secs(30))
        .poll_interval(Duration::from_secs(1))
        .build()?;

    // Register middleware (outermost to innermost)
    worker.use_middleware("logging", LoggingMiddleware).await;
    worker
        .use_middleware("metrics", MetricsMiddleware::new())
        .await;
    worker
        .use_middleware("trace-context", TraceContextMiddleware)
        .await;

    // Register handlers
    worker.register("email.send", handle_email_send).await;
    worker
        .register("report.generate", handle_report_generate)
        .await;

    println!("Worker {} starting...", worker.id());
    println!("Middleware chain: logging → metrics → trace-context");
    println!("Press Ctrl+C to stop");

    worker.start().await?;

    println!("Worker stopped gracefully");
    Ok(())
}
