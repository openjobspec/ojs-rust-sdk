use ojs::retry::{OnExhaustion, RetryPolicy};

// ---------------------------------------------------------------------------
// Default values
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
fn test_retry_policy_new_equals_default() {
    let new_policy = RetryPolicy::new();
    let default_policy = RetryPolicy::default();
    assert_eq!(new_policy.max_attempts, default_policy.max_attempts);
    assert_eq!(new_policy.initial_interval, default_policy.initial_interval);
    assert_eq!(
        new_policy.backoff_coefficient,
        default_policy.backoff_coefficient
    );
}

// ---------------------------------------------------------------------------
// Builder pattern
// ---------------------------------------------------------------------------

#[test]
fn test_retry_policy_builder() {
    let policy = RetryPolicy::new()
        .max_attempts(5)
        .initial_interval("PT2S")
        .backoff_coefficient(3.0)
        .max_interval("PT10M");

    assert_eq!(policy.max_attempts, 5);
    assert_eq!(policy.initial_interval, "PT2S");
    assert_eq!(policy.backoff_coefficient, 3.0);
    assert_eq!(policy.max_interval, "PT10M");
}

#[test]
fn test_retry_policy_no_jitter() {
    let policy = RetryPolicy::new().jitter(false);
    assert!(!policy.jitter);
}

#[test]
fn test_retry_policy_non_retryable_errors() {
    let policy = RetryPolicy::new().non_retryable_errors(vec!["ValidationError".into()]);
    assert_eq!(policy.non_retryable_errors, vec!["ValidationError"]);
}

#[test]
fn test_retry_policy_multiple_non_retryable_errors() {
    let policy = RetryPolicy::new()
        .non_retryable_errors(vec!["ValidationError".into(), "AuthenticationError".into()]);
    assert_eq!(policy.non_retryable_errors.len(), 2);
}

#[test]
fn test_retry_policy_on_exhaustion_discard() {
    let policy = RetryPolicy::new().on_exhaustion(OnExhaustion::Discard);
    assert_eq!(policy.on_exhaustion, OnExhaustion::Discard);
}

#[test]
fn test_retry_policy_on_exhaustion_dead_letter() {
    let policy = RetryPolicy::new().on_exhaustion(OnExhaustion::DeadLetter);
    assert_eq!(policy.on_exhaustion, OnExhaustion::DeadLetter);
}

// ---------------------------------------------------------------------------
// Serialization / deserialization
// ---------------------------------------------------------------------------

#[test]
fn test_retry_policy_serde_roundtrip() {
    let policy = RetryPolicy::new()
        .max_attempts(10)
        .initial_interval("PT500MS")
        .backoff_coefficient(1.5)
        .max_interval("PT30M")
        .jitter(false)
        .non_retryable_errors(vec!["PermanentError".into()]);

    let json_str = serde_json::to_string(&policy).unwrap();
    let deserialized: RetryPolicy = serde_json::from_str(&json_str).unwrap();

    assert_eq!(deserialized.max_attempts, 10);
    assert_eq!(deserialized.initial_interval, "PT500MS");
    assert_eq!(deserialized.backoff_coefficient, 1.5);
    assert_eq!(deserialized.max_interval, "PT30M");
    assert!(!deserialized.jitter);
    assert_eq!(deserialized.non_retryable_errors, vec!["PermanentError"]);
}

#[test]
fn test_retry_policy_deserialize_from_server() {
    let json = serde_json::json!({
        "max_attempts": 7,
        "initial_interval": "PT3S",
        "backoff_coefficient": 2.5,
        "max_interval": "PT1H",
        "jitter": true,
        "non_retryable_errors": ["Fatal", "Auth"],
        "on_exhaustion": "discard"
    });

    let policy: RetryPolicy = serde_json::from_value(json).unwrap();
    assert_eq!(policy.max_attempts, 7);
    assert_eq!(policy.initial_interval, "PT3S");
    assert_eq!(policy.backoff_coefficient, 2.5);
    assert_eq!(policy.max_interval, "PT1H");
    assert!(policy.jitter);
    assert_eq!(policy.non_retryable_errors.len(), 2);
    assert_eq!(policy.on_exhaustion, OnExhaustion::Discard);
}

#[test]
fn test_retry_policy_deserialize_defaults_missing_fields() {
    let json = serde_json::json!({});
    let policy: RetryPolicy = serde_json::from_value(json).unwrap();

    assert_eq!(policy.max_attempts, 3);
    assert_eq!(policy.initial_interval, "PT1S");
    assert_eq!(policy.backoff_coefficient, 2.0);
    assert!(policy.jitter);
}

// ---------------------------------------------------------------------------
// OnExhaustion tests
// ---------------------------------------------------------------------------

#[test]
fn test_on_exhaustion_serde() {
    let discard: OnExhaustion = serde_json::from_str("\"discard\"").unwrap();
    assert_eq!(discard, OnExhaustion::Discard);

    let dl: OnExhaustion = serde_json::from_str("\"dead_letter\"").unwrap();
    assert_eq!(dl, OnExhaustion::DeadLetter);
}

#[test]
fn test_on_exhaustion_roundtrip() {
    let original = OnExhaustion::Discard;
    let json = serde_json::to_string(&original).unwrap();
    let deser: OnExhaustion = serde_json::from_str(&json).unwrap();
    assert_eq!(original, deser);
}

// ---------------------------------------------------------------------------
// Edge cases
// ---------------------------------------------------------------------------

#[test]
fn test_retry_policy_zero_attempts() {
    let policy = RetryPolicy::new().max_attempts(0);
    assert_eq!(policy.max_attempts, 0);
}

#[test]
fn test_retry_policy_very_high_attempts() {
    let policy = RetryPolicy::new().max_attempts(u32::MAX);
    assert_eq!(policy.max_attempts, u32::MAX);
}

#[test]
fn test_retry_policy_zero_coefficient() {
    let policy = RetryPolicy::new().backoff_coefficient(0.0);
    assert_eq!(policy.backoff_coefficient, 0.0);
}
