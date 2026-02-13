use crate::errors::JobError;
use crate::retry::RetryPolicy;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Job State
// ---------------------------------------------------------------------------

/// The lifecycle state of a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    /// Staged for future activation (external trigger needed).
    Pending,
    /// Scheduled for future execution at `scheduled_at`.
    Scheduled,
    /// Ready to be fetched by a worker.
    Available,
    /// Currently being processed by a worker.
    Active,
    /// Successfully completed.
    Completed,
    /// Failed but eligible for retry.
    Retryable,
    /// Explicitly cancelled.
    Cancelled,
    /// All retries exhausted, permanently failed.
    Discarded,
}

impl JobState {
    /// Returns `true` if this is a terminal state (completed, cancelled, or discarded).
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            JobState::Completed | JobState::Cancelled | JobState::Discarded
        )
    }
}

impl std::fmt::Display for JobState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            JobState::Pending => "pending",
            JobState::Scheduled => "scheduled",
            JobState::Available => "available",
            JobState::Active => "active",
            JobState::Completed => "completed",
            JobState::Retryable => "retryable",
            JobState::Cancelled => "cancelled",
            JobState::Discarded => "discarded",
        };
        write!(f, "{}", s)
    }
}

// ---------------------------------------------------------------------------
// Unique Policy
// ---------------------------------------------------------------------------

/// Deduplication policy for unique jobs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UniquePolicy {
    /// Dimensions for uniqueness hash.
    #[serde(default = "default_unique_keys")]
    pub keys: Vec<UniqueDimension>,

    /// Specific arg keys to include when `args` is in `keys`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args_keys: Option<Vec<String>>,

    /// Specific meta keys to include when `meta` is in `keys`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta_keys: Option<Vec<String>>,

    /// ISO 8601 duration for the uniqueness window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub period: Option<String>,

    /// Job states to check for existing duplicates.
    #[serde(default = "default_unique_states")]
    pub states: Vec<JobState>,

    /// Strategy when a duplicate is detected.
    #[serde(default)]
    pub on_conflict: ConflictStrategy,
}

impl Default for UniquePolicy {
    fn default() -> Self {
        Self {
            keys: default_unique_keys(),
            args_keys: None,
            meta_keys: None,
            period: None,
            states: default_unique_states(),
            on_conflict: ConflictStrategy::default(),
        }
    }
}

impl UniquePolicy {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn keys(mut self, keys: Vec<UniqueDimension>) -> Self {
        self.keys = keys;
        self
    }

    pub fn args_keys(mut self, keys: Vec<String>) -> Self {
        self.args_keys = Some(keys);
        self
    }

    pub fn meta_keys(mut self, keys: Vec<String>) -> Self {
        self.meta_keys = Some(keys);
        self
    }

    pub fn period(mut self, period: impl Into<String>) -> Self {
        self.period = Some(period.into());
        self
    }

    pub fn states(mut self, states: Vec<JobState>) -> Self {
        self.states = states;
        self
    }

    pub fn on_conflict(mut self, strategy: ConflictStrategy) -> Self {
        self.on_conflict = strategy;
        self
    }
}

/// Dimensions used for computing the uniqueness hash.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UniqueDimension {
    Type,
    Queue,
    Args,
    Meta,
}

/// Strategy for handling duplicate job conflicts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictStrategy {
    /// Return 409 Conflict, don't enqueue.
    #[default]
    Reject,
    /// Cancel existing job, enqueue new one.
    Replace,
    /// Update args but preserve `scheduled_at`.
    ReplaceExceptSchedule,
    /// Silent no-op, return existing job ID.
    Ignore,
}

fn default_unique_keys() -> Vec<UniqueDimension> {
    vec![UniqueDimension::Type]
}

fn default_unique_states() -> Vec<JobState> {
    vec![
        JobState::Available,
        JobState::Active,
        JobState::Scheduled,
        JobState::Retryable,
        JobState::Pending,
    ]
}

// ---------------------------------------------------------------------------
// Job
// ---------------------------------------------------------------------------

/// A job envelope conforming to the OJS specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    /// OJS specification version.
    #[serde(default = "default_specversion")]
    pub specversion: String,

    /// Unique job identifier (UUIDv7).
    pub id: String,

    /// Dot-namespaced job type (e.g., `email.send`).
    #[serde(rename = "type")]
    pub job_type: String,

    /// Target queue name.
    #[serde(default = "default_queue")]
    pub queue: String,

    /// Positional job arguments (JSON array).
    #[serde(default = "default_args")]
    pub args: serde_json::Value,

    /// Extensible metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<HashMap<String, serde_json::Value>>,

    /// Job priority (higher = more urgent).
    #[serde(default)]
    pub priority: i32,

    /// Execution timeout in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,

    /// ISO 8601 time for delayed execution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled_at: Option<DateTime<Utc>>,

    /// ISO 8601 deadline; job is discarded if not started by this time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,

    /// Retry policy configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<RetryPolicy>,

    /// Deduplication policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unique: Option<UniquePolicy>,

    /// Schema URI for args validation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,

    // ---- System-managed fields (read-only) ----
    /// Current lifecycle state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<JobState>,

    /// Current attempt number (0 = never executed).
    #[serde(default)]
    pub attempt: u32,

    /// Maximum number of attempts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_attempts: Option<u32>,

    /// When the job was created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,

    /// When the job became available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enqueued_at: Option<DateTime<Utc>>,

    /// When execution began (most recent attempt).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,

    /// When the job reached a terminal state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,

    /// Last error information.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<JobError>,

    /// Return value from successful execution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,

    /// Tags for filtering.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,

    /// Timeout in milliseconds (wire format).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

impl Job {
    /// Extract a typed argument from the job args by key.
    ///
    /// If the args is an array containing a single object `[{...}]`, this
    /// looks up `key` inside that object. If the args is a plain object,
    /// it looks up `key` directly.
    pub fn arg<T: serde::de::DeserializeOwned>(&self, key: &str) -> crate::Result<T> {
        let obj = match &self.args {
            serde_json::Value::Array(arr) if arr.len() == 1 => {
                arr[0]
                    .as_object()
                    .ok_or_else(|| crate::OjsError::Handler("args[0] is not an object".to_string()))?
            }
            serde_json::Value::Object(map) => map,
            _ => {
                return Err(crate::OjsError::Handler(
                    "args is not an object or single-element array".into(),
                ))
            }
        };

        let value = obj.get(key).ok_or_else(|| {
            crate::OjsError::Handler(format!("missing argument: {}", key))
        })?;

        serde_json::from_value(value.clone()).map_err(|e| {
            crate::OjsError::Handler(format!("failed to deserialize arg '{}': {}", key, e))
        })
    }

    /// Extract all args as a typed struct.
    ///
    /// If the args is `[{...}]`, deserializes the inner object.
    /// If the args is `{...}`, deserializes directly.
    pub fn args_as<T: serde::de::DeserializeOwned>(&self) -> crate::Result<T> {
        let value = match &self.args {
            serde_json::Value::Array(arr) if arr.len() == 1 => &arr[0],
            other => other,
        };
        serde_json::from_value(value.clone()).map_err(|e| {
            crate::OjsError::Handler(format!("failed to deserialize args: {}", e))
        })
    }
}

fn default_specversion() -> String {
    "1.0.0-rc.1".to_string()
}

fn default_queue() -> String {
    "default".to_string()
}

fn default_args() -> serde_json::Value {
    serde_json::Value::Array(vec![])
}

// ---------------------------------------------------------------------------
// Wire format types for HTTP protocol
// ---------------------------------------------------------------------------

/// Request body for POST /ojs/v1/jobs
#[derive(Debug, Serialize)]
pub(crate) struct EnqueueRequest {
    #[serde(rename = "type")]
    pub job_type: String,
    pub args: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<HashMap<String, serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<EnqueueOptionsWire>,
}

/// Wire format for enqueue options.
#[derive(Debug, Default, Serialize)]
pub(crate) struct EnqueueOptionsWire {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queue: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delay_until: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry: Option<RetryPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unique: Option<UniquePolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility_timeout_ms: Option<u64>,
}

/// Response body for POST /ojs/v1/jobs
#[derive(Debug, Deserialize)]
pub(crate) struct EnqueueResponse {
    pub job: Job,
}

/// Request body for POST /ojs/v1/jobs/batch
#[derive(Debug, Serialize)]
pub(crate) struct BatchEnqueueRequest {
    pub jobs: Vec<EnqueueRequest>,
}

/// Response body for POST /ojs/v1/jobs/batch
#[derive(Debug, Deserialize)]
pub(crate) struct BatchEnqueueResponse {
    pub jobs: Vec<Job>,
    #[allow(dead_code)]
    pub count: usize,
}

/// Request body for POST /ojs/v1/workers/fetch
#[derive(Debug, Serialize)]
pub(crate) struct FetchRequest {
    pub queues: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility_timeout_ms: Option<u64>,
}

/// Response body for POST /ojs/v1/workers/fetch
#[derive(Debug, Deserialize)]
pub(crate) struct FetchResponse {
    pub jobs: Vec<Job>,
}

/// Request body for POST /ojs/v1/workers/ack
#[derive(Debug, Serialize)]
pub(crate) struct AckRequest {
    pub job_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
}

/// Request body for POST /ojs/v1/workers/nack
#[derive(Debug, Serialize)]
pub(crate) struct NackRequest {
    pub job_id: String,
    pub error: NackError,
}

#[derive(Debug, Serialize)]
pub(crate) struct NackError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<HashMap<String, serde_json::Value>>,
}

/// Request body for POST /ojs/v1/workers/heartbeat
#[derive(Debug, Serialize)]
pub(crate) struct HeartbeatRequest {
    pub worker_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_jobs: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility_timeout_ms: Option<u64>,
}

/// Response body for POST /ojs/v1/workers/heartbeat
#[derive(Debug, Deserialize)]
pub(crate) struct HeartbeatResponse {
    pub state: String,
    #[allow(dead_code)]
    #[serde(default)]
    pub jobs_extended: Option<Vec<String>>,
    #[allow(dead_code)]
    #[serde(default)]
    pub server_time: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_job_arg_extraction_from_array() {
        let job = Job {
            specversion: "1.0.0-rc.1".into(),
            id: "test".into(),
            job_type: "email.send".into(),
            queue: "default".into(),
            args: json!([{"to": "user@example.com", "subject": "Hello"}]),
            meta: None,
            priority: 0,
            timeout: None,
            scheduled_at: None,
            expires_at: None,
            retry: None,
            unique: None,
            schema: None,
            state: None,
            attempt: 0,
            max_attempts: None,
            created_at: None,
            enqueued_at: None,
            started_at: None,
            completed_at: None,
            error: None,
            result: None,
            tags: vec![],
            timeout_ms: None,
        };

        let to: String = job.arg("to").unwrap();
        assert_eq!(to, "user@example.com");
    }

    #[test]
    fn test_job_state_is_terminal() {
        assert!(!JobState::Available.is_terminal());
        assert!(!JobState::Active.is_terminal());
        assert!(JobState::Completed.is_terminal());
        assert!(JobState::Cancelled.is_terminal());
        assert!(JobState::Discarded.is_terminal());
    }

    #[test]
    fn test_job_state_serde() {
        let state = JobState::Available;
        let json = serde_json::to_string(&state).unwrap();
        assert_eq!(json, "\"available\"");
        let deserialized: JobState = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, state);
    }
}
