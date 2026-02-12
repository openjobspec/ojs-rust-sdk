use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A named job queue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Queue {
    /// Queue name.
    pub name: String,
    /// Queue status (e.g., "active", "paused").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// When the queue was created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
}

/// Statistical metrics for a queue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueStats {
    /// Queue name.
    pub queue: String,
    /// Queue operational status.
    pub status: String,
    /// Statistical metrics.
    pub stats: QueueStatsMetrics,
    /// When these stats were computed.
    pub computed_at: DateTime<Utc>,
}

/// Detailed queue metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueStatsMetrics {
    /// Jobs in "available" state.
    #[serde(default)]
    pub available: u64,
    /// Jobs currently being executed.
    #[serde(default)]
    pub active: u64,
    /// Jobs scheduled for future execution.
    #[serde(default)]
    pub scheduled: u64,
    /// Jobs eligible for retry.
    #[serde(default)]
    pub retryable: u64,
    /// Permanently failed jobs.
    #[serde(default)]
    pub discarded: u64,
    /// Jobs completed in the last hour.
    #[serde(default)]
    pub completed_last_hour: u64,
    /// Jobs failed in the last hour.
    #[serde(default)]
    pub failed_last_hour: u64,
    /// Average execution duration in milliseconds.
    #[serde(default)]
    pub avg_duration_ms: f64,
    /// Average queue wait time in milliseconds.
    #[serde(default)]
    pub avg_wait_ms: f64,
    /// Jobs completed per second.
    #[serde(default)]
    pub throughput_per_second: f64,
}

/// Pagination metadata for list operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pagination {
    pub total: u64,
    pub limit: u64,
    pub offset: u64,
    pub has_more: bool,
}

/// Response wrapper for queue list endpoints.
#[derive(Debug, Deserialize)]
pub(crate) struct QueuesResponse {
    pub queues: Vec<Queue>,
}

/// Response wrapper for dead letter list endpoints.
#[derive(Debug, Deserialize)]
pub(crate) struct DeadLetterResponse {
    pub jobs: Vec<crate::Job>,
    #[serde(default)]
    pub pagination: Option<Pagination>,
}

/// Cron job definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    /// Unique cron job name.
    pub name: String,
    /// Cron expression (5-6 field or @special).
    pub cron: String,
    /// IANA timezone.
    #[serde(default = "default_timezone")]
    pub timezone: String,
    /// Job type to trigger.
    #[serde(rename = "type")]
    pub job_type: String,
    /// Job arguments.
    #[serde(default)]
    pub args: serde_json::Value,
    /// Metadata for triggered jobs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<std::collections::HashMap<String, serde_json::Value>>,
    /// Job options.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<serde_json::Value>,
    /// Overlap handling policy.
    #[serde(default = "default_overlap_policy")]
    pub overlap_policy: OverlapPolicy,
    /// Whether the cron job is active.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Human-readable description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    // System-managed fields
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_run_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
}

/// How to handle overlapping cron runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlapPolicy {
    /// Don't enqueue if previous run is still active.
    Skip,
    /// Allow concurrent executions.
    Allow,
    /// Cancel active run, start new one.
    CancelPrevious,
    /// Queue new run to start after previous completes.
    Enqueue,
}

impl Default for OverlapPolicy {
    fn default() -> Self {
        Self::Skip
    }
}

/// Request body for registering a cron job.
#[derive(Debug, Serialize)]
pub struct CronJobRequest {
    pub name: String,
    pub cron: String,
    #[serde(default = "default_timezone")]
    pub timezone: String,
    #[serde(rename = "type")]
    pub job_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<std::collections::HashMap<String, serde_json::Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<serde_json::Value>,
    #[serde(default)]
    pub overlap_policy: OverlapPolicy,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Response for cron list endpoints.
#[derive(Debug, Deserialize)]
pub(crate) struct CronJobsResponse {
    pub cron_jobs: Vec<CronJob>,
}

/// Server health status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthStatus {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uptime_seconds: Option<u64>,
}

/// Server conformance manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub ojs_version: String,
    pub implementation: ManifestImplementation,
    pub conformance_level: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conformance_tier: Option<String>,
    pub protocols: Vec<String>,
    pub backend: String,
    #[serde(default)]
    pub capabilities: ManifestCapabilities,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoints: Option<ManifestEndpoints>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestImplementation {
    pub name: String,
    pub version: String,
    pub language: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ManifestCapabilities {
    #[serde(default)]
    pub batch_enqueue: bool,
    #[serde(default)]
    pub cron_jobs: bool,
    #[serde(default)]
    pub dead_letter: bool,
    #[serde(default)]
    pub delayed_jobs: bool,
    #[serde(default)]
    pub job_ttl: bool,
    #[serde(default)]
    pub priority_queues: bool,
    #[serde(default)]
    pub rate_limiting: bool,
    #[serde(default)]
    pub schema_validation: bool,
    #[serde(default)]
    pub unique_jobs: bool,
    #[serde(default)]
    pub workflows: bool,
    #[serde(default)]
    pub pause_resume: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEndpoints {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<String>,
}

fn default_timezone() -> String {
    "UTC".to_string()
}

fn default_overlap_policy() -> OverlapPolicy {
    OverlapPolicy::Skip
}

fn default_enabled() -> bool {
    true
}
