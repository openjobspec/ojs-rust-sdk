#![allow(clippy::approx_constant)]

use ojs::job::{ConflictStrategy, JobState, UniqueDimension, UniquePolicy};
use ojs::retry::{OnExhaustion, RetryPolicy};
use ojs::{Job, JobRequest};
use serde_json::json;

// ---------------------------------------------------------------------------
// Job deserialization tests
// ---------------------------------------------------------------------------

#[test]
fn test_job_deserialize_complete() {
    let json = json!({
        "specversion": "1.0",
        "id": "019461a8-1a2b-7c3d-8e4f-5a6b7c8d9e0f",
        "type": "email.send",
        "queue": "email",
        "args": [{"to": "user@example.com", "subject": "Hello"}],
        "meta": {"tenant": "acme"},
        "priority": 10,
        "timeout": 300,
        "state": "active",
        "attempt": 2,
        "max_attempts": 5,
        "created_at": "2025-01-01T00:00:00Z",
        "enqueued_at": "2025-01-01T00:00:01Z",
        "started_at": "2025-01-01T00:00:02Z",
        "tags": ["urgent", "vip"],
        "retry": {
            "max_attempts": 5,
            "initial_interval": "PT2S",
            "backoff_coefficient": 2.0,
            "max_interval": "PT5M",
            "jitter": true,
            "non_retryable_errors": [],
            "on_exhaustion": "discard"
        }
    });

    let job: Job = serde_json::from_value(json).unwrap();
    assert_eq!(job.id, "019461a8-1a2b-7c3d-8e4f-5a6b7c8d9e0f");
    assert_eq!(job.job_type, "email.send");
    assert_eq!(job.queue, "email");
    assert_eq!(job.state, Some(JobState::Active));
    assert_eq!(job.attempt, 2);
    assert_eq!(job.max_attempts, Some(5));
    assert_eq!(job.priority, 10);
    assert_eq!(job.timeout, Some(300));
    assert_eq!(job.tags, vec!["urgent", "vip"]);
    assert!(job.meta.is_some());
    assert!(job.created_at.is_some());
    assert!(job.enqueued_at.is_some());
    assert!(job.started_at.is_some());
    assert!(job.retry.is_some());
}

#[test]
fn test_job_deserialize_minimal() {
    let json = json!({
        "id": "minimal-job-001",
        "type": "test.minimal",
    });

    let job: Job = serde_json::from_value(json).unwrap();
    assert_eq!(job.id, "minimal-job-001");
    assert_eq!(job.job_type, "test.minimal");
    assert_eq!(job.queue, "default");
    assert_eq!(job.specversion, "1.0");
    assert_eq!(job.priority, 0);
    assert_eq!(job.attempt, 0);
    assert!(job.state.is_none());
    assert!(job.meta.is_none());
    assert!(job.timeout.is_none());
    assert!(job.retry.is_none());
    assert!(job.unique.is_none());
    assert!(job.tags.is_empty());
}

#[test]
fn test_job_with_error_field() {
    let json = json!({
        "id": "failed-job",
        "type": "test.fail",
        "queue": "default",
        "state": "discarded",
        "attempt": 3,
        "error": {
            "type": "RuntimeError",
            "message": "connection refused",
            "backtrace": ["main.rs:10", "handler.rs:20"]
        }
    });

    let job: Job = serde_json::from_value(json).unwrap();
    assert_eq!(job.state, Some(JobState::Discarded));
    assert!(job.error.is_some());
    let error = job.error.unwrap();
    assert_eq!(error.error_type, "RuntimeError");
    assert_eq!(error.message, "connection refused");
    assert_eq!(error.backtrace.unwrap().len(), 2);
}

#[test]
fn test_job_with_result_field() {
    let json = json!({
        "id": "completed-job",
        "type": "test.complete",
        "queue": "default",
        "state": "completed",
        "result": {"status": "ok", "processed_count": 42}
    });

    let job: Job = serde_json::from_value(json).unwrap();
    assert_eq!(job.state, Some(JobState::Completed));
    assert!(job.result.is_some());
    assert_eq!(job.result.as_ref().unwrap()["processed_count"], 42);
}

// ---------------------------------------------------------------------------
// JobRequest tests
// ---------------------------------------------------------------------------

#[test]
fn test_job_request_creation() {
    let req = JobRequest::new("email.send", json!({"to": "user@example.com"}));
    assert_eq!(req.job_type, "email.send");
    assert!(req.meta.is_none());
    assert!(req.options.is_empty());
}

#[test]
fn test_job_request_with_meta() {
    let mut meta = std::collections::HashMap::new();
    meta.insert("tenant".to_string(), json!("acme"));
    let req = JobRequest::new("report.generate", json!({"id": 42})).meta(meta);
    assert_eq!(req.job_type, "report.generate");
    assert!(req.meta.is_some());
}

#[test]
fn test_job_request_with_option() {
    use ojs::EnqueueOption;
    let req = JobRequest::new("urgent.task", json!({}))
        .with_option(EnqueueOption::Queue("critical".into()));
    assert_eq!(req.job_type, "urgent.task");
    assert_eq!(req.options.len(), 1);
}

// ---------------------------------------------------------------------------
// RetryPolicy tests
// ---------------------------------------------------------------------------

#[test]
fn test_retry_policy_serialization_roundtrip() {
    let policy = RetryPolicy::new()
        .max_attempts(10)
        .initial_interval("PT5S")
        .backoff_coefficient(1.5)
        .max_interval("PT1H")
        .jitter(false)
        .non_retryable_errors(vec!["auth.*".into()])
        .on_exhaustion(OnExhaustion::DeadLetter);

    let json_str = serde_json::to_string(&policy).unwrap();
    let deserialized: RetryPolicy = serde_json::from_str(&json_str).unwrap();

    assert_eq!(deserialized.max_attempts, 10);
    assert_eq!(deserialized.initial_interval, "PT5S");
    assert_eq!(deserialized.backoff_coefficient, 1.5);
    assert_eq!(deserialized.max_interval, "PT1H");
    assert!(!deserialized.jitter);
    assert_eq!(deserialized.non_retryable_errors, vec!["auth.*"]);
    assert_eq!(deserialized.on_exhaustion, OnExhaustion::DeadLetter);
}

#[test]
fn test_retry_policy_default_values() {
    let policy = RetryPolicy::default();
    assert_eq!(policy.max_attempts, 3);
    assert_eq!(policy.initial_interval, "PT1S");
    assert_eq!(policy.backoff_coefficient, 2.0);
    assert_eq!(policy.max_interval, "PT5M");
    assert!(policy.jitter);
    assert!(policy.non_retryable_errors.is_empty());
    assert_eq!(policy.on_exhaustion, OnExhaustion::Discard);
}

// ---------------------------------------------------------------------------
// UniquePolicy tests
// ---------------------------------------------------------------------------

#[test]
fn test_unique_policy_serialization_roundtrip() {
    let policy = UniquePolicy::new()
        .keys(vec![
            UniqueDimension::Type,
            UniqueDimension::Queue,
            UniqueDimension::Args,
        ])
        .args_keys(vec!["user_id".into()])
        .period("PT1H")
        .on_conflict(ConflictStrategy::Replace);

    let json_str = serde_json::to_string(&policy).unwrap();
    let deserialized: UniquePolicy = serde_json::from_str(&json_str).unwrap();

    assert_eq!(deserialized.keys.len(), 3);
    assert_eq!(deserialized.args_keys.unwrap(), vec!["user_id"]);
    assert_eq!(deserialized.period.unwrap(), "PT1H");
    assert_eq!(deserialized.on_conflict, ConflictStrategy::Replace);
}

#[test]
fn test_unique_policy_default_values() {
    let policy = UniquePolicy::default();
    assert_eq!(policy.keys, vec![UniqueDimension::Type]);
    assert_eq!(policy.on_conflict, ConflictStrategy::Reject);
    assert!(policy.args_keys.is_none());
    assert!(policy.period.is_none());
}

// ---------------------------------------------------------------------------
// JobState tests
// ---------------------------------------------------------------------------

#[test]
fn test_job_state_all_variants() {
    let states = vec![
        (JobState::Pending, "pending", false),
        (JobState::Scheduled, "scheduled", false),
        (JobState::Available, "available", false),
        (JobState::Active, "active", false),
        (JobState::Completed, "completed", true),
        (JobState::Retryable, "retryable", false),
        (JobState::Cancelled, "cancelled", true),
        (JobState::Discarded, "discarded", true),
    ];

    for (state, expected_str, expected_terminal) in states {
        assert_eq!(state.to_string(), expected_str);
        assert_eq!(state.is_terminal(), expected_terminal);
    }
}

#[test]
fn test_job_state_serde_all_variants() {
    let all_states = vec![
        JobState::Pending,
        JobState::Scheduled,
        JobState::Available,
        JobState::Active,
        JobState::Completed,
        JobState::Retryable,
        JobState::Cancelled,
        JobState::Discarded,
    ];

    for state in all_states {
        let json_str = serde_json::to_string(&state).unwrap();
        let deserialized: JobState = serde_json::from_str(&json_str).unwrap();
        assert_eq!(state, deserialized);
    }
}

// ---------------------------------------------------------------------------
// ConflictStrategy tests
// ---------------------------------------------------------------------------

#[test]
fn test_conflict_strategy_serde_roundtrip() {
    let strategies = vec![
        ConflictStrategy::Reject,
        ConflictStrategy::Replace,
        ConflictStrategy::ReplaceExceptSchedule,
        ConflictStrategy::Ignore,
    ];

    for strategy in strategies {
        let json_str = serde_json::to_string(&strategy).unwrap();
        let deserialized: ConflictStrategy = serde_json::from_str(&json_str).unwrap();
        assert_eq!(strategy, deserialized);
    }
}

// ---------------------------------------------------------------------------
// Job arg tests
// ---------------------------------------------------------------------------

#[test]
fn test_job_args_various_types() {
    let json = json!({
        "id": "args-test",
        "type": "test.args",
        "queue": "default",
        "args": [{"string": "hello", "number": 42, "float": 3.14, "bool": true, "null_val": null}]
    });

    let job: Job = serde_json::from_value(json).unwrap();

    let s: String = job.arg("string").unwrap();
    assert_eq!(s, "hello");

    let n: i64 = job.arg("number").unwrap();
    assert_eq!(n, 42);

    let f: f64 = job.arg("float").unwrap();
    assert!((f - 3.14).abs() < f64::EPSILON);

    let b: bool = job.arg("bool").unwrap();
    assert!(b);
}

#[test]
fn test_job_args_missing_key_returns_error() {
    let json = json!({
        "id": "missing-test",
        "type": "test.missing",
        "args": [{"existing": "value"}]
    });

    let job: Job = serde_json::from_value(json).unwrap();
    let result: Result<String, _> = job.arg("nonexistent");
    assert!(result.is_err());
}

#[test]
fn test_job_args_as_struct() {
    #[derive(serde::Deserialize, PartialEq, Debug)]
    struct TestArgs {
        name: String,
        count: u32,
    }

    let json = json!({
        "id": "struct-test",
        "type": "test.struct",
        "args": [{"name": "Alice", "count": 5}]
    });

    let job: Job = serde_json::from_value(json).unwrap();
    let args: TestArgs = job.args_as().unwrap();
    assert_eq!(args.name, "Alice");
    assert_eq!(args.count, 5);
}
