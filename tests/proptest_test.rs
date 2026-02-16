//! Property-based tests for OJS data structures using proptest.
//!
//! Validates serialization roundtrips, state machine invariants,
//! and error handling edge cases via randomized inputs.

use ojs::job::{Job, JobState};
use ojs::retry::{OnExhaustion, RetryPolicy};
use proptest::prelude::*;

// ---------------------------------------------------------------------------
// Strategies
// ---------------------------------------------------------------------------

fn arb_job_state() -> impl Strategy<Value = JobState> {
    prop_oneof![
        Just(JobState::Pending),
        Just(JobState::Scheduled),
        Just(JobState::Available),
        Just(JobState::Active),
        Just(JobState::Completed),
        Just(JobState::Retryable),
        Just(JobState::Cancelled),
        Just(JobState::Discarded),
    ]
}

fn arb_retry_policy() -> impl Strategy<Value = RetryPolicy> {
    (1..100u32).prop_map(|max| RetryPolicy::new().max_attempts(max))
}

fn arb_json_value() -> impl Strategy<Value = serde_json::Value> {
    prop_oneof![
        Just(serde_json::Value::Null),
        any::<bool>().prop_map(serde_json::Value::Bool),
        any::<i64>().prop_map(|v| serde_json::json!(v)),
        "[a-zA-Z0-9_]{0,50}".prop_map(|s| serde_json::Value::String(s)),
        Just(serde_json::json!([])),
        Just(serde_json::json!({})),
    ]
}

fn arb_job() -> impl Strategy<Value = serde_json::Value> {
    (
        "[a-z][a-z0-9.]{1,30}",                        // job_type
        "[a-z][a-z0-9_]{0,20}",                        // queue
        prop::collection::vec(arb_json_value(), 0..5), // args
        0..100i32,                                     // priority
    )
        .prop_map(|(job_type, queue, args, priority)| {
            serde_json::json!({
                "specversion": "1.0.0-rc.1",
                "id": format!("01234567-89ab-7cde-8000-{:012x}", priority as u64),
                "type": job_type,
                "queue": queue,
                "args": args,
                "priority": priority,
                "state": "available",
                "attempt": 1,
                "created_at": "2025-01-01T00:00:00Z",
            })
        })
}

// ---------------------------------------------------------------------------
// Property tests
// ---------------------------------------------------------------------------

proptest! {
    /// Job serialization roundtrip: deserialize → serialize → deserialize must be stable.
    #[test]
    fn job_serde_roundtrip(job_json in arb_job()) {
        let job: Job = serde_json::from_value(job_json).unwrap();
        let serialized = serde_json::to_string(&job).unwrap();
        let deserialized: Job = serde_json::from_str(&serialized).unwrap();

        prop_assert_eq!(&job.id, &deserialized.id);
        prop_assert_eq!(&job.job_type, &deserialized.job_type);
        prop_assert_eq!(&job.queue, &deserialized.queue);
        prop_assert_eq!(job.priority, deserialized.priority);
    }

    /// RetryPolicy serialization roundtrip.
    #[test]
    fn retry_policy_roundtrip(policy in arb_retry_policy()) {
        let serialized = serde_json::to_string(&policy).unwrap();
        let deserialized: RetryPolicy = serde_json::from_str(&serialized).unwrap();

        prop_assert_eq!(policy.max_attempts, deserialized.max_attempts);
    }

    /// Terminal states are exactly: completed, cancelled, discarded.
    #[test]
    fn terminal_state_invariant(state in arb_job_state()) {
        let is_terminal = state.is_terminal();
        let expected = matches!(state, JobState::Completed | JobState::Cancelled | JobState::Discarded);
        prop_assert_eq!(is_terminal, expected);
    }

    /// JobState Display roundtrip via serde.
    #[test]
    fn job_state_display_serde_roundtrip(state in arb_job_state()) {
        let display = state.to_string();
        let json_str = format!("\"{}\"", display);
        let deserialized: JobState = serde_json::from_str(&json_str).unwrap();
        prop_assert_eq!(state, deserialized);
    }

    /// OnExhaustion serialization roundtrip.
    #[test]
    fn on_exhaustion_roundtrip(
        variant in prop_oneof![
            Just(OnExhaustion::Discard),
            Just(OnExhaustion::DeadLetter),
        ]
    ) {
        let serialized = serde_json::to_string(&variant).unwrap();
        let deserialized: OnExhaustion = serde_json::from_str(&serialized).unwrap();
        prop_assert_eq!(variant, deserialized);
    }
}
