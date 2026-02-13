use serde::{Deserialize, Serialize};

/// Retry policy configuration for failed jobs.
///
/// Follows the OJS retry specification with support for multiple backoff
/// strategies, jitter, and configurable exhaustion behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// Total number of attempts including the initial execution.
    /// Default: 3
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,

    /// ISO 8601 duration before the first retry.
    /// Default: "PT1S" (1 second)
    #[serde(default = "default_initial_interval")]
    pub initial_interval: String,

    /// Multiplier for exponential backoff between retries.
    /// Default: 2.0
    #[serde(default = "default_backoff_coefficient")]
    pub backoff_coefficient: f64,

    /// Maximum delay cap (ISO 8601 duration).
    /// Default: "PT5M" (5 minutes)
    #[serde(default = "default_max_interval")]
    pub max_interval: String,

    /// Whether to add ±50% randomness to delay to prevent thundering herd.
    /// Default: true
    #[serde(default = "default_jitter")]
    pub jitter: bool,

    /// Error type prefixes that should not be retried.
    #[serde(default)]
    pub non_retryable_errors: Vec<String>,

    /// Action when all retry attempts are exhausted.
    /// Default: "discard"
    #[serde(default = "default_on_exhaustion")]
    pub on_exhaustion: OnExhaustion,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: default_max_attempts(),
            initial_interval: default_initial_interval(),
            backoff_coefficient: default_backoff_coefficient(),
            max_interval: default_max_interval(),
            jitter: default_jitter(),
            non_retryable_errors: Vec::new(),
            on_exhaustion: default_on_exhaustion(),
        }
    }
}

impl RetryPolicy {
    /// Create a new retry policy with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the maximum number of attempts.
    pub fn max_attempts(mut self, n: u32) -> Self {
        self.max_attempts = n;
        self
    }

    /// Set the initial retry interval as an ISO 8601 duration string.
    pub fn initial_interval(mut self, interval: impl Into<String>) -> Self {
        self.initial_interval = interval.into();
        self
    }

    /// Set the backoff coefficient.
    pub fn backoff_coefficient(mut self, coefficient: f64) -> Self {
        self.backoff_coefficient = coefficient;
        self
    }

    /// Set the maximum interval between retries as an ISO 8601 duration.
    pub fn max_interval(mut self, interval: impl Into<String>) -> Self {
        self.max_interval = interval.into();
        self
    }

    /// Enable or disable jitter.
    pub fn jitter(mut self, enabled: bool) -> Self {
        self.jitter = enabled;
        self
    }

    /// Add error type prefixes that should not be retried.
    pub fn non_retryable_errors(mut self, errors: Vec<String>) -> Self {
        self.non_retryable_errors = errors;
        self
    }

    /// Set the action to take when retries are exhausted.
    pub fn on_exhaustion(mut self, action: OnExhaustion) -> Self {
        self.on_exhaustion = action;
        self
    }
}

/// Action taken when all retry attempts are exhausted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnExhaustion {
    /// Discard the job (move to discarded state).
    #[default]
    Discard,
    /// Move the job to the dead letter queue.
    DeadLetter,
}

// ---------------------------------------------------------------------------
// Default value functions for serde
// ---------------------------------------------------------------------------

fn default_max_attempts() -> u32 {
    3
}

fn default_initial_interval() -> String {
    "PT1S".to_string()
}

fn default_backoff_coefficient() -> f64 {
    2.0
}

fn default_max_interval() -> String {
    "PT5M".to_string()
}

fn default_jitter() -> bool {
    true
}

fn default_on_exhaustion() -> OnExhaustion {
    OnExhaustion::Discard
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_retry_policy() {
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
    fn test_builder_pattern() {
        let policy = RetryPolicy::new()
            .max_attempts(5)
            .initial_interval("PT2S")
            .backoff_coefficient(3.0)
            .max_interval("PT10M")
            .jitter(false)
            .non_retryable_errors(vec!["validation.*".into()])
            .on_exhaustion(OnExhaustion::DeadLetter);

        assert_eq!(policy.max_attempts, 5);
        assert_eq!(policy.initial_interval, "PT2S");
        assert_eq!(policy.backoff_coefficient, 3.0);
        assert_eq!(policy.max_interval, "PT10M");
        assert!(!policy.jitter);
        assert_eq!(policy.non_retryable_errors, vec!["validation.*"]);
        assert_eq!(policy.on_exhaustion, OnExhaustion::DeadLetter);
    }

    #[test]
    fn test_serde_roundtrip() {
        let policy = RetryPolicy::new().max_attempts(5);
        let json = serde_json::to_string(&policy).unwrap();
        let deserialized: RetryPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.max_attempts, 5);
    }
}
