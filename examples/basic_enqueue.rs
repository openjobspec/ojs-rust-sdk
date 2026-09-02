//! Basic job enqueue example.
//!
//! Demonstrates how to create a client and enqueue jobs with various options.

use ojs::{Client, JobRequest, RetryPolicy};
use serde_json::json;
use std::time::Duration;

#[tokio::main]
async fn main() -> ojs::Result<()> {
    // Create the client
    let client = Client::builder().url("http://localhost:8080").build()?;

    // 1. Simple enqueue (no options)
    let job = client
        .enqueue(
            "email.send",
            json!({"to": "user@example.com", "subject": "Welcome!"}),
        )
        .await?;
    println!("Enqueued job: {} (state: {:?})", job.id, job.state);

    // 2. Enqueue with options
    let job = client
        .enqueue("report.generate", json!({"report_id": 42}))
        .queue("reports")
        .priority(10)
        .delay(Duration::from_secs(60))
        .retry(RetryPolicy::new().max_attempts(5))
        .tags(vec!["urgent".into(), "weekly".into()])
        .send()
        .await?;
    println!("Enqueued delayed job: {} (state: {:?})", job.id, job.state);

    // 3. Batch enqueue
    let jobs = client
        .enqueue_batch(vec![
            JobRequest::new(
                "notification.push",
                json!({"user_id": 1, "message": "Hello"}),
            ),
            JobRequest::new(
                "notification.push",
                json!({"user_id": 2, "message": "Hello"}),
            ),
            JobRequest::new(
                "notification.push",
                json!({"user_id": 3, "message": "Hello"}),
            ),
        ])
        .await?;
    println!("Batch enqueued {} jobs", jobs.len());

    // 4. Get job status
    let status = client.get_job(&job.id).await?;
    println!("Job {} state: {:?}", status.id, status.state);

    // 5. Cancel a job
    let cancelled = client.cancel_job(&job.id).await?;
    println!(
        "Cancelled job: {} (state: {:?})",
        cancelled.id, cancelled.state
    );

    // 6. Health check
    let health = client.health().await?;
    println!("Server health: {}", health.status);

    Ok(())
}
