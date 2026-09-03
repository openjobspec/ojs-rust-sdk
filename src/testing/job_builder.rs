//! Builder for creating [`crate::Job`] instances directly in tests,
//! independent of the fake store/drain actor.

/// Builder for creating [`crate::Job`] instances in tests.
///
/// Provides sensible defaults for all fields so you only need to set
/// what matters for your test.
///
/// # Example
///
/// ```rust
/// # #[cfg(feature = "testing")]
/// # {
/// use ojs::testing::JobBuilder;
/// use serde_json::json;
///
/// let job = JobBuilder::new("email.send")
///     .args(json!({"to": "user@example.com"}))
///     .queue("email")
///     .build();
///
/// assert_eq!(job.job_type, "email.send");
/// assert_eq!(job.queue, "email");
/// # }
/// ```
pub struct JobBuilder {
    job: crate::Job,
}

impl JobBuilder {
    /// Create a new builder with the given job type and sensible defaults.
    pub fn new(job_type: impl Into<String>) -> Self {
        Self {
            job: crate::Job {
                specversion: crate::OJS_VERSION.to_string(),
                id: format!("test_{}", uuid::Uuid::now_v7()),
                job_type: job_type.into(),
                queue: "default".to_string(),
                args: serde_json::json!([{}]),
                meta: None,
                priority: 0,
                timeout: None,
                scheduled_at: None,
                expires_at: None,
                retry: None,
                unique: None,
                schema: None,
                state: Some(crate::JobState::Available),
                attempt: 0,
                max_attempts: None,
                created_at: Some(chrono::Utc::now()),
                enqueued_at: Some(chrono::Utc::now()),
                started_at: None,
                completed_at: None,
                error: None,
                result: None,
                tags: vec![],
                timeout_ms: None,
                checkpoint: None,
            },
        }
    }

    /// Set the job ID.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.job.id = id.into();
        self
    }

    /// Set the job arguments.
    pub fn args(mut self, args: serde_json::Value) -> Self {
        self.job.args = crate::workflow::normalize_args(&args);
        self
    }

    /// Set the target queue.
    pub fn queue(mut self, queue: impl Into<String>) -> Self {
        self.job.queue = queue.into();
        self
    }

    /// Set the job state.
    pub fn state(mut self, state: crate::JobState) -> Self {
        self.job.state = Some(state);
        self
    }

    /// Set the attempt number.
    pub fn attempt(mut self, attempt: u32) -> Self {
        self.job.attempt = attempt;
        self
    }

    /// Set the priority.
    pub fn priority(mut self, priority: i32) -> Self {
        self.job.priority = priority;
        self
    }

    /// Set metadata.
    pub fn meta(mut self, meta: std::collections::HashMap<String, serde_json::Value>) -> Self {
        self.job.meta = Some(meta);
        self
    }

    /// Set tags.
    pub fn tags(mut self, tags: Vec<String>) -> Self {
        self.job.tags = tags;
        self
    }

    /// Set the retry policy.
    pub fn retry(mut self, policy: crate::RetryPolicy) -> Self {
        self.job.retry = Some(policy);
        self
    }

    /// Build the [`crate::Job`].
    pub fn build(self) -> crate::Job {
        self.job
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_job_builder_defaults() {
        let job = JobBuilder::new("email.send").build();
        assert_eq!(job.job_type, "email.send");
        assert_eq!(job.queue, "default");
        assert_eq!(job.priority, 0);
        assert_eq!(job.attempt, 0);
        assert_eq!(job.state, Some(crate::JobState::Available));
        assert!(job.id.starts_with("test_"));
    }

    #[test]
    fn test_job_builder_custom_fields() {
        let job = JobBuilder::new("report.generate")
            .id("custom-id")
            .queue("reports")
            .priority(5)
            .attempt(2)
            .state(crate::JobState::Active)
            .args(serde_json::json!({"id": 42}))
            .tags(vec!["urgent".into()])
            .build();

        assert_eq!(job.id, "custom-id");
        assert_eq!(job.job_type, "report.generate");
        assert_eq!(job.queue, "reports");
        assert_eq!(job.priority, 5);
        assert_eq!(job.attempt, 2);
        assert_eq!(job.state, Some(crate::JobState::Active));
        assert_eq!(job.tags, vec!["urgent"]);
        let id: u32 = job.arg("id").unwrap();
        assert_eq!(id, 42);
    }
}
