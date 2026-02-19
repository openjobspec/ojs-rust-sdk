use std::time::Duration;

/// Configuration for automatic HTTP request retries on rate-limited (429) responses.
///
/// When enabled, the transport layer will automatically retry requests that
/// receive a `429 Too Many Requests` response. The retry delay respects the
/// server's `Retry-After` header when present, falling back to exponential
/// backoff with jitter.
///
/// # Example
///
/// ```rust
/// use ojs::RetryConfig;
/// use std::time::Duration;
///
/// let config = RetryConfig::default()
///     .with_max_retries(5)
///     .with_min_backoff(Duration::from_millis(200))
///     .with_max_backoff(Duration::from_secs(60));
/// ```
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts after the initial request (default: 3).
    pub max_retries: u32,
    /// Minimum backoff duration used as the base for exponential backoff (default: 500ms).
    pub min_backoff: Duration,
    /// Maximum backoff duration; delays are clamped to this value (default: 30s).
    pub max_backoff: Duration,
    /// Whether automatic retries are enabled (default: true).
    pub enabled: bool,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            min_backoff: Duration::from_millis(500),
            max_backoff: Duration::from_secs(30),
            enabled: true,
        }
    }
}

impl RetryConfig {
    /// Create a new `RetryConfig` with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the maximum number of retry attempts.
    pub fn with_max_retries(mut self, n: u32) -> Self {
        self.max_retries = n;
        self
    }

    /// Set the minimum backoff duration.
    pub fn with_min_backoff(mut self, d: Duration) -> Self {
        self.min_backoff = d;
        self
    }

    /// Set the maximum backoff duration.
    pub fn with_max_backoff(mut self, d: Duration) -> Self {
        self.max_backoff = d;
        self
    }

    /// Enable or disable automatic retries.
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Create a `RetryConfig` with retries disabled.
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Self::default()
        }
    }

    /// Compute the backoff delay for a given attempt, optionally using the
    /// server-provided `Retry-After` duration.
    pub(crate) fn compute_backoff(
        &self,
        attempt: u32,
        retry_after: Option<Duration>,
    ) -> Duration {
        if let Some(ra) = retry_after {
            // Respect the server's Retry-After, clamped to max_backoff.
            return ra.min(self.max_backoff);
        }

        // Exponential backoff: min_backoff * 2^attempt, clamped to max_backoff.
        let base_ms = self.min_backoff.as_millis() as u64;
        let exp_ms = base_ms.saturating_mul(1u64.checked_shl(attempt).unwrap_or(u64::MAX));
        let max_ms = self.max_backoff.as_millis() as u64;
        let clamped_ms = exp_ms.min(max_ms);

        // Add jitter: uniform in [clamped/2, clamped] using system time nanos.
        let half = clamped_ms / 2;
        let jitter_range = clamped_ms - half;
        let jitter = if jitter_range > 0 {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos() as u64;
            nanos % jitter_range
        } else {
            0
        };

        Duration::from_millis(half + jitter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = RetryConfig::default();
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.min_backoff, Duration::from_millis(500));
        assert_eq!(config.max_backoff, Duration::from_secs(30));
        assert!(config.enabled);
    }

    #[test]
    fn test_disabled_config() {
        let config = RetryConfig::disabled();
        assert!(!config.enabled);
    }

    #[test]
    fn test_builder_methods() {
        let config = RetryConfig::new()
            .with_max_retries(5)
            .with_min_backoff(Duration::from_millis(200))
            .with_max_backoff(Duration::from_secs(60))
            .with_enabled(false);

        assert_eq!(config.max_retries, 5);
        assert_eq!(config.min_backoff, Duration::from_millis(200));
        assert_eq!(config.max_backoff, Duration::from_secs(60));
        assert!(!config.enabled);
    }

    #[test]
    fn test_backoff_respects_retry_after() {
        let config = RetryConfig::default();
        let delay = config.compute_backoff(0, Some(Duration::from_secs(5)));
        assert_eq!(delay, Duration::from_secs(5));
    }

    #[test]
    fn test_backoff_clamps_retry_after_to_max() {
        let config = RetryConfig::default().with_max_backoff(Duration::from_secs(10));
        let delay = config.compute_backoff(0, Some(Duration::from_secs(60)));
        assert_eq!(delay, Duration::from_secs(10));
    }

    #[test]
    fn test_exponential_backoff_range() {
        let config = RetryConfig::default()
            .with_min_backoff(Duration::from_millis(1000))
            .with_max_backoff(Duration::from_secs(30));

        // attempt 0: base=1000ms, range=[500, 1000]
        let delay = config.compute_backoff(0, None);
        assert!(delay >= Duration::from_millis(500));
        assert!(delay <= Duration::from_millis(1000));

        // attempt 1: base=2000ms, range=[1000, 2000]
        let delay = config.compute_backoff(1, None);
        assert!(delay >= Duration::from_millis(1000));
        assert!(delay <= Duration::from_millis(2000));
    }

    #[test]
    fn test_backoff_clamped_to_max() {
        let config = RetryConfig::default()
            .with_min_backoff(Duration::from_secs(1))
            .with_max_backoff(Duration::from_secs(5));

        // attempt 10: 1s * 2^10 = 1024s, should be clamped to 5s
        let delay = config.compute_backoff(10, None);
        assert!(delay <= Duration::from_secs(5));
    }
}
