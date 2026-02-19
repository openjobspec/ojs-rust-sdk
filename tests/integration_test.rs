use ojs::job::{ConflictStrategy, JobState, UniqueDimension, UniquePolicy};
use ojs::retry::{OnExhaustion, RetryPolicy};
use ojs::workflow::{batch, chain, group, BatchCallbacks, Step};
use serde_json::json;

// ---------------------------------------------------------------------------
// Job type tests
// ---------------------------------------------------------------------------

#[test]
fn test_job_state_is_terminal() {
    assert!(!JobState::Pending.is_terminal());
    assert!(!JobState::Scheduled.is_terminal());
    assert!(!JobState::Available.is_terminal());
    assert!(!JobState::Active.is_terminal());
    assert!(!JobState::Retryable.is_terminal());
    assert!(JobState::Completed.is_terminal());
    assert!(JobState::Cancelled.is_terminal());
    assert!(JobState::Discarded.is_terminal());
}

#[test]
fn test_job_state_display() {
    assert_eq!(JobState::Available.to_string(), "available");
    assert_eq!(JobState::Active.to_string(), "active");
    assert_eq!(JobState::Completed.to_string(), "completed");
    assert_eq!(JobState::Retryable.to_string(), "retryable");
}

#[test]
fn test_job_state_serde_roundtrip() {
    for state in &[
        JobState::Pending,
        JobState::Scheduled,
        JobState::Available,
        JobState::Active,
        JobState::Completed,
        JobState::Retryable,
        JobState::Cancelled,
        JobState::Discarded,
    ] {
        let json = serde_json::to_string(state).unwrap();
        let deserialized: JobState = serde_json::from_str(&json).unwrap();
        assert_eq!(*state, deserialized);
    }
}

// ---------------------------------------------------------------------------
// Retry policy tests
// ---------------------------------------------------------------------------

#[test]
fn test_retry_policy_defaults() {
    let policy = RetryPolicy::default();
    assert_eq!(policy.max_attempts, 3);
    assert_eq!(policy.initial_interval, "PT1S");
    assert_eq!(policy.backoff_coefficient, 2.0);
    assert_eq!(policy.max_interval, "PT5M");
    assert!(policy.jitter);
    assert!(policy.non_retryable_errors.is_empty());
    assert_eq!(policy.on_exhaustion, OnExhaustion::Discard);
}

#[test]
fn test_retry_policy_serde_roundtrip() {
    let policy = RetryPolicy::new()
        .max_attempts(5)
        .initial_interval("PT2S")
        .backoff_coefficient(3.0)
        .max_interval("PT10M")
        .jitter(false)
        .non_retryable_errors(vec!["validation.*".into(), "auth.forbidden".into()])
        .on_exhaustion(OnExhaustion::DeadLetter);

    let json = serde_json::to_string(&policy).unwrap();
    let deserialized: RetryPolicy = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.max_attempts, 5);
    assert_eq!(deserialized.initial_interval, "PT2S");
    assert_eq!(deserialized.backoff_coefficient, 3.0);
    assert!(!deserialized.jitter);
    assert_eq!(deserialized.non_retryable_errors.len(), 2);
    assert_eq!(deserialized.on_exhaustion, OnExhaustion::DeadLetter);
}

// ---------------------------------------------------------------------------
// Unique policy tests
// ---------------------------------------------------------------------------

#[test]
fn test_unique_policy_defaults() {
    let policy = UniquePolicy::default();
    assert_eq!(policy.keys, vec![UniqueDimension::Type]);
    assert_eq!(policy.on_conflict, ConflictStrategy::Reject);
}

#[test]
fn test_unique_policy_builder() {
    let policy = UniquePolicy::new()
        .keys(vec![
            UniqueDimension::Type,
            UniqueDimension::Queue,
            UniqueDimension::Args,
        ])
        .args_keys(vec!["user_id".into()])
        .period("PT1H")
        .on_conflict(ConflictStrategy::Replace);

    assert_eq!(policy.keys.len(), 3);
    assert_eq!(policy.args_keys.unwrap(), vec!["user_id"]);
    assert_eq!(policy.period.unwrap(), "PT1H");
    assert_eq!(policy.on_conflict, ConflictStrategy::Replace);
}

#[test]
fn test_unique_policy_serde_roundtrip() {
    let policy = UniquePolicy::new()
        .keys(vec![UniqueDimension::Type, UniqueDimension::Args])
        .period("PT30M")
        .on_conflict(ConflictStrategy::Ignore);

    let json = serde_json::to_string(&policy).unwrap();
    let deserialized: UniquePolicy = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.on_conflict, ConflictStrategy::Ignore);
    assert_eq!(deserialized.period.unwrap(), "PT30M");
}

// ---------------------------------------------------------------------------
// Workflow tests
// ---------------------------------------------------------------------------

#[test]
fn test_chain_workflow_creation() {
    let def = chain(vec![
        Step::new("step.a", json!({})),
        Step::new("step.b", json!({})),
        Step::new("step.c", json!({})),
    ])
    .name("test chain");

    assert_eq!(def.name.as_deref(), Some("test chain"));
    assert_eq!(def.steps.len(), 3);
}

#[test]
fn test_group_workflow_creation() {
    let def = group(vec![Step::new("a", json!({})), Step::new("b", json!({}))]);

    assert_eq!(def.steps.len(), 2);
}

#[test]
fn test_batch_workflow_creation() {
    let def = batch(
        BatchCallbacks::new()
            .on_complete(Step::new("report", json!({})))
            .on_failure(Step::new("alert", json!({}))),
        vec![Step::new("job.a", json!({})), Step::new("job.b", json!({}))],
    );

    assert_eq!(def.steps.len(), 2);
    assert!(def.callbacks.is_some());
    let cb = def.callbacks.unwrap();
    assert!(cb.on_complete.is_some());
    assert!(cb.on_failure.is_some());
    assert!(cb.on_success.is_none());
}

#[test]
fn test_step_with_options() {
    let step = Step::new("email.send", json!({"to": "a@b.com"}))
        .queue("email")
        .priority(10);

    assert_eq!(step.job_type, "email.send");
    assert_eq!(step.options.len(), 2);
}

// ---------------------------------------------------------------------------
// Job deserialization tests
// ---------------------------------------------------------------------------

#[test]
fn test_job_deserialize_from_server() {
    let json = json!({
        "specversion": "1.0",
        "id": "019461a8-1a2b-7c3d-8e4f-5a6b7c8d9e0f",
        "type": "email.send",
        "queue": "default",
        "args": [{"to": "user@example.com"}],
        "state": "available",
        "attempt": 0,
        "priority": 0,
        "created_at": "2025-01-01T00:00:00Z",
        "enqueued_at": "2025-01-01T00:00:00Z",
        "tags": ["urgent"]
    });

    let job: ojs::Job = serde_json::from_value(json).unwrap();
    assert_eq!(job.id, "019461a8-1a2b-7c3d-8e4f-5a6b7c8d9e0f");
    assert_eq!(job.job_type, "email.send");
    assert_eq!(job.queue, "default");
    assert_eq!(job.state, Some(JobState::Available));
    assert_eq!(job.attempt, 0);
    assert_eq!(job.tags, vec!["urgent"]);
}

#[test]
fn test_job_arg_extraction() {
    let json = json!({
        "specversion": "1.0",
        "id": "test-id",
        "type": "test",
        "queue": "default",
        "args": [{"name": "Alice", "age": 30, "active": true}]
    });

    let job: ojs::Job = serde_json::from_value(json).unwrap();

    let name: String = job.arg("name").unwrap();
    assert_eq!(name, "Alice");

    let age: u32 = job.arg("age").unwrap();
    assert_eq!(age, 30);

    let active: bool = job.arg("active").unwrap();
    assert!(active);

    // Missing arg should error
    let result: Result<String, _> = job.arg("missing");
    assert!(result.is_err());
}

#[test]
fn test_job_args_as_struct() {
    #[derive(serde::Deserialize, PartialEq, Debug)]
    struct EmailArgs {
        to: String,
        subject: String,
    }

    let json = json!({
        "specversion": "1.0",
        "id": "test-id",
        "type": "email.send",
        "queue": "default",
        "args": [{"to": "user@example.com", "subject": "Hello"}]
    });

    let job: ojs::Job = serde_json::from_value(json).unwrap();
    let args: EmailArgs = job.args_as().unwrap();

    assert_eq!(args.to, "user@example.com");
    assert_eq!(args.subject, "Hello");
}

// ---------------------------------------------------------------------------
// Error type tests
// ---------------------------------------------------------------------------

#[test]
fn test_server_error_classification() {
    let err = ojs::ServerError::new("not_found", "job not found", 404);

    assert!(err.is_not_found());
    assert!(!err.is_duplicate());
    assert!(!err.is_retryable());
}

#[test]
fn test_ojs_error_display() {
    let err = ojs::OjsError::Handler("something went wrong".into());
    assert_eq!(err.to_string(), "handler error: something went wrong");

    let err = ojs::OjsError::Builder("url is required".into());
    assert_eq!(err.to_string(), "builder error: url is required");
}

// ---------------------------------------------------------------------------
// Event tests
// ---------------------------------------------------------------------------

#[test]
fn test_event_deserialization() {
    let json = json!({
        "specversion": "1.0",
        "id": "evt_test",
        "type": "job.completed",
        "source": "ojs://test/workers/w1",
        "time": "2025-01-01T00:00:00Z",
        "subject": "job_123",
        "data": {"result": {"status": "ok"}}
    });

    let event: ojs::Event = serde_json::from_value(json).unwrap();
    assert_eq!(event.event_type, "job.completed");
    assert_eq!(event.subject, Some("job_123".into()));
    assert!(event.data.is_some());
}

// ---------------------------------------------------------------------------
// Queue type tests
// ---------------------------------------------------------------------------

#[test]
fn test_queue_stats_deserialization() {
    let json = json!({
        "queue": "default",
        "status": "active",
        "stats": {
            "available": 100,
            "active": 5,
            "scheduled": 10,
            "retryable": 2,
            "discarded": 0,
            "completed_last_hour": 500,
            "failed_last_hour": 3,
            "avg_duration_ms": 150.5,
            "avg_wait_ms": 25.0,
            "throughput_per_second": 8.3
        },
        "computed_at": "2025-01-01T00:00:00Z"
    });

    let stats: ojs::QueueStats = serde_json::from_value(json).unwrap();
    assert_eq!(stats.queue, "default");
    assert_eq!(stats.status, "active");
    assert_eq!(stats.stats.available, 100);
    assert_eq!(stats.stats.active, 5);
    assert_eq!(stats.stats.throughput_per_second, 8.3);
}
