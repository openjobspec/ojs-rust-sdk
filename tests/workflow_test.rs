use ojs::workflow::*;
use serde_json::json;

// ---------------------------------------------------------------------------
// WorkflowState tests
// ---------------------------------------------------------------------------

#[test]
fn test_workflow_state_display() {
    assert_eq!(WorkflowState::Pending.to_string(), "pending");
    assert_eq!(WorkflowState::Running.to_string(), "running");
    assert_eq!(WorkflowState::Completed.to_string(), "completed");
    assert_eq!(WorkflowState::Failed.to_string(), "failed");
    assert_eq!(WorkflowState::Cancelled.to_string(), "cancelled");
}

#[test]
fn test_workflow_state_serde_roundtrip() {
    let states = [
        WorkflowState::Pending,
        WorkflowState::Running,
        WorkflowState::Completed,
        WorkflowState::Failed,
        WorkflowState::Cancelled,
    ];

    for state in &states {
        let json_val = serde_json::to_string(state).unwrap();
        let deser: WorkflowState = serde_json::from_str(&json_val).unwrap();
        assert_eq!(*state, deser, "roundtrip failed for {:?}", state);
    }
}

#[test]
fn test_workflow_state_deserialize_snake_case() {
    let state: WorkflowState = serde_json::from_str("\"pending\"").unwrap();
    assert_eq!(state, WorkflowState::Pending);

    let state: WorkflowState = serde_json::from_str("\"running\"").unwrap();
    assert_eq!(state, WorkflowState::Running);
}

// ---------------------------------------------------------------------------
// Step builder tests
// ---------------------------------------------------------------------------

#[test]
fn test_step_new() {
    let step = Step::new("email.send", json!(["user@example.com"]));
    assert_eq!(step.job_type, "email.send");
    assert_eq!(step.args, json!(["user@example.com"]));
    assert!(step.options.is_empty());
}

#[test]
fn test_step_with_queue() {
    let step = Step::new("email.send", json!(["user@example.com"]))
        .queue("email-queue");
    assert_eq!(step.options.len(), 1);
}

#[test]
fn test_step_chained_options() {
    let step = Step::new("video.transcode", json!({"url": "s3://bucket/video.mp4"}))
        .queue("transcode")
        .priority(10)
        .timeout(std::time::Duration::from_secs(300));
    assert_eq!(step.options.len(), 3);
}

// ---------------------------------------------------------------------------
// Workflow constructor tests
// ---------------------------------------------------------------------------

#[test]
fn test_chain_workflow() {
    let wf = chain(vec![
        Step::new("step1", json!({"order": 1})),
        Step::new("step2", json!({"order": 2})),
        Step::new("step3", json!({"order": 3})),
    ]);

    assert_eq!(wf.workflow_type, WorkflowType::Chain);
    assert_eq!(wf.steps.len(), 3);
}

#[test]
fn test_group_workflow() {
    let wf = group(vec![
        Step::new("parallel_a", json!({})),
        Step::new("parallel_b", json!({})),
    ]);

    assert_eq!(wf.workflow_type, WorkflowType::Group);
    assert_eq!(wf.steps.len(), 2);
}

#[test]
fn test_batch_workflow() {
    let callbacks = BatchCallbacks::new()
        .on_complete(Step::new("on_done", json!({})));

    let wf = batch(callbacks, vec![
        Step::new("batch_item", json!({"id": 1})),
        Step::new("batch_item", json!({"id": 2})),
        Step::new("batch_item", json!({"id": 3})),
    ]);

    assert_eq!(wf.workflow_type, WorkflowType::Batch);
    assert_eq!(wf.steps.len(), 3);
}

#[test]
fn test_workflow_with_name() {
    let wf = chain(vec![
        Step::new("step1", json!({})),
    ]).name("my-workflow");

    assert!(wf.name.is_some());
    assert_eq!(wf.name.unwrap(), "my-workflow");
}

#[test]
fn test_empty_chain() {
    let wf = chain(vec![]);
    assert_eq!(wf.steps.len(), 0);
}

// ---------------------------------------------------------------------------
// WorkflowDefinition serde tests
// ---------------------------------------------------------------------------

#[test]
fn test_workflow_definition_has_steps() {
    let wf = chain(vec![
        Step::new("step1", json!(["arg1"])).queue("q1"),
        Step::new("step2", json!(["arg2"])).queue("q2"),
    ]);

    assert_eq!(wf.steps.len(), 2);
    assert_eq!(wf.steps[0].job_type, "step1");
    assert_eq!(wf.steps[1].job_type, "step2");
}

// ---------------------------------------------------------------------------
// BatchCallbacks tests
// ---------------------------------------------------------------------------

#[test]
fn test_batch_callbacks_builder() {
    let callbacks = BatchCallbacks::new()
        .on_success(Step::new("on_success_handler", json!({})))
        .on_failure(Step::new("on_failure_handler", json!({})))
        .on_complete(Step::new("on_complete_handler", json!({})));

    assert!(callbacks.on_success.is_some());
    assert!(callbacks.on_failure.is_some());
    assert!(callbacks.on_complete.is_some());
}

#[test]
fn test_batch_callbacks_empty() {
    let callbacks = BatchCallbacks::new();
    assert!(callbacks.on_success.is_none());
    assert!(callbacks.on_failure.is_none());
    assert!(callbacks.on_complete.is_none());
}

// ---------------------------------------------------------------------------
// Workflow response deserialization tests
// ---------------------------------------------------------------------------

#[test]
fn test_workflow_deserialize() {
    let json_val = json!({
        "id": "wf_01234",
        "state": "running",
        "steps": [
            {"id": "step-0", "type": "step1", "job_id": "job_111", "state": "completed"},
            {"id": "step-1", "type": "step2", "job_id": "job_222", "state": "active"}
        ],
        "created_at": "2026-03-09T20:00:00Z"
    });

    let wf: Workflow = serde_json::from_value(json_val).unwrap();
    assert_eq!(wf.id, "wf_01234");
    assert_eq!(wf.state, WorkflowState::Running);
    assert_eq!(wf.steps.len(), 2);
    assert_eq!(wf.steps[0].job_type, "step1");
    assert_eq!(wf.steps[1].state, "active");
}

#[test]
fn test_workflow_completed() {
    let json_val = json!({
        "id": "wf_done",
        "state": "completed",
        "steps": [],
        "created_at": "2026-03-09T20:00:00Z"
    });

    let wf: Workflow = serde_json::from_value(json_val).unwrap();
    assert_eq!(wf.state, WorkflowState::Completed);
}
