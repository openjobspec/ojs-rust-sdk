//! Worker job processing example.
//!
//! Demonstrates how to create a worker, register handlers, add middleware,
//! and process jobs with graceful shutdown.

use ojs::{BoxFuture, HandlerResult, JobContext, Middleware, Next, Worker};
use serde_json::json;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Custom middleware: logging
// ---------------------------------------------------------------------------

struct LoggingMiddleware;

impl Middleware for LoggingMiddleware {
    fn handle(&self, ctx: JobContext, next: Next) -> BoxFuture<'static, HandlerResult> {
        Box::pin(async move {
            let job_id = ctx.job.id.clone();
            let job_type = ctx.job.job_type.clone();
            let start = std::time::Instant::now();

            println!("[LOG] Starting job {} (type: {})", job_id, job_type);

            let result = next.run(ctx).await;
            let elapsed = start.elapsed();

            match &result {
                Ok(_) => println!("[LOG] Job {} completed in {:?}", job_id, elapsed),
                Err(e) => println!("[LOG] Job {} failed in {:?}: {}", job_id, elapsed, e),
            }

            result
        })
    }
}

// ---------------------------------------------------------------------------
// Job handlers
// ---------------------------------------------------------------------------

async fn handle_email_send(ctx: JobContext) -> ojs::HandlerResult {
    let to: String = ctx.job.arg("to")?;
    let subject: String = ctx
        .job
        .arg("subject")
        .unwrap_or_else(|_| "No Subject".into());

    println!("  Sending email to {} with subject: {}", to, subject);

    // Simulate work
    tokio::time::sleep(Duration::from_millis(100)).await;

    Ok(json!({
        "message_id": format!("msg_{}", ctx.job.id),
        "delivered": true,
    }))
}

async fn handle_report_generate(ctx: JobContext) -> ojs::HandlerResult {
    let report_id: u64 = ctx.job.arg("report_id")?;

    println!("  Generating report #{}", report_id);

    // Simulate long-running work with heartbeat
    for i in 0..5 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        ctx.heartbeat().await?;
        println!("  Report #{} progress: {}%", report_id, (i + 1) * 20);
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
    // Create the worker
    let worker = Worker::builder()
        .url("http://localhost:8080")
        .queues(vec!["default", "email", "reports"])
        .concurrency(10)
        .grace_period(Duration::from_secs(30))
        .poll_interval(Duration::from_secs(1))
        .build()?;

    // Register middleware
    worker.use_middleware("logging", LoggingMiddleware).await;

    // Register handlers
    worker.register("email.send", handle_email_send).await;
    worker
        .register("report.generate", handle_report_generate)
        .await;

    // Inline handler with closure
    worker
        .register("notification.push", |ctx: JobContext| async move {
            let user_id: u64 = ctx.job.arg("user_id")?;
            let message: String = ctx.job.arg("message")?;
            println!("  Pushing notification to user {}: {}", user_id, message);
            Ok(json!({"delivered": true}))
        })
        .await;

    println!("Worker {} starting...", worker.id());
    println!("Press Ctrl+C to stop");

    // Start processing (blocks until shutdown)
    worker.start().await?;

    println!("Worker stopped gracefully");
    Ok(())
}

