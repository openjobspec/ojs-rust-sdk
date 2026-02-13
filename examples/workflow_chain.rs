//! Workflow example demonstrating chains, groups, and batches.
//!
//! Shows how to create and manage multi-step job workflows.

use ojs::{batch, chain, group, BatchCallbacks, Client, Step};
use serde_json::json;

#[tokio::main]
async fn main() -> ojs::Result<()> {
    let client = Client::builder().url("http://localhost:8080").build()?;

    // -----------------------------------------------------------------------
    // 1. Chain workflow (sequential execution)
    // -----------------------------------------------------------------------
    // fetch -> transform -> notify

    let chain_def = chain(vec![
        Step::new("data.fetch", json!({"url": "https://api.example.com/data"})),
        Step::new("data.transform", json!({"format": "csv"})),
        Step::new(
            "notification.send",
            json!({"channel": "slack", "message": "Data ready!"}),
        ),
    ])
    .name("ETL Pipeline");

    let workflow = client.create_workflow(chain_def).await?;
    println!(
        "Chain workflow created: {} (state: {})",
        workflow.id, workflow.state
    );

    // -----------------------------------------------------------------------
    // 2. Group workflow (parallel execution)
    // -----------------------------------------------------------------------
    // All exports run simultaneously

    let group_def = group(vec![
        Step::new("export.csv", json!({"report_id": "rpt_001"})),
        Step::new("export.pdf", json!({"report_id": "rpt_001"})),
        Step::new("export.xlsx", json!({"report_id": "rpt_001"})),
    ])
    .name("Multi-format Export");

    let workflow = client.create_workflow(group_def).await?;
    println!(
        "Group workflow created: {} (state: {})",
        workflow.id, workflow.state
    );

    // -----------------------------------------------------------------------
    // 3. Batch workflow (parallel with callbacks)
    // -----------------------------------------------------------------------
    // Send emails to many users, then report results

    let batch_def = batch(
        BatchCallbacks::new()
            .on_complete(Step::new(
                "batch.report",
                json!({"type": "email_campaign", "campaign_id": "camp_001"}),
            ))
            .on_failure(Step::new(
                "batch.alert",
                json!({"channel": "pagerduty", "severity": "warning"}),
            )),
        vec![
            Step::new(
                "email.send",
                json!({"to": "user1@example.com", "template": "promo"}),
            ),
            Step::new(
                "email.send",
                json!({"to": "user2@example.com", "template": "promo"}),
            ),
            Step::new(
                "email.send",
                json!({"to": "user3@example.com", "template": "promo"}),
            ),
        ],
    )
    .name("Email Campaign");

    let workflow = client.create_workflow(batch_def).await?;
    println!(
        "Batch workflow created: {} (state: {})",
        workflow.id, workflow.state
    );

    // -----------------------------------------------------------------------
    // 4. Check workflow status
    // -----------------------------------------------------------------------

    let status = client.get_workflow(&workflow.id).await?;
    println!("\nWorkflow {} status:", status.id);
    println!("  State: {}", status.state);
    for step in &status.steps {
        println!(
            "  Step {}: {} (state: {}, job: {:?})",
            step.id, step.job_type, step.state, step.job_id
        );
    }

    // -----------------------------------------------------------------------
    // 5. Cancel a workflow
    // -----------------------------------------------------------------------

    let cancelled = client.cancel_workflow(&workflow.id).await?;
    println!(
        "\nWorkflow {} cancelled (steps cancelled: {:?})",
        cancelled.id, cancelled.steps_cancelled
    );

    Ok(())
}
