//! OJS lifecycle event types.
//!
//! This module defines the event envelope and type constants from the
//! [OJS Events specification](https://openjobspec.org). Events follow the
//! CloudEvents format and represent lifecycle transitions of jobs, workflows,
//! queues, and workers.
//!
//! These types are provided for deserializing events received from an OJS
//! server (e.g., via SSE or webhook). The SDK does not currently include a
//! built-in event subscription client — events should be consumed through
//! server-specific streaming endpoints.
//!
//! # Example
//!
//! ```rust
//! use ojs::Event;
//!
//! let raw = r#"{
//!     "specversion": "1.0",
//!     "id": "evt_001",
//!     "type": "job.completed",
//!     "source": "ojs://my-service/workers/w1",
//!     "time": "2025-01-01T00:00:00Z"
//! }"#;
//!
//! let event: Event = serde_json::from_str(raw).unwrap();
//! assert_eq!(event.event_type, "job.completed");
//! ```

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Event type constants
// ---------------------------------------------------------------------------

// Core job events (MUST emit)
pub const EVENT_JOB_ENQUEUED: &str = "job.enqueued";
pub const EVENT_JOB_STARTED: &str = "job.started";
pub const EVENT_JOB_COMPLETED: &str = "job.completed";
pub const EVENT_JOB_FAILED: &str = "job.failed";
pub const EVENT_JOB_DISCARDED: &str = "job.discarded";

// Extended job events (SHOULD emit)
pub const EVENT_JOB_RETRYING: &str = "job.retrying";
pub const EVENT_JOB_CANCELLED: &str = "job.cancelled";
pub const EVENT_JOB_HEARTBEAT: &str = "job.heartbeat";
pub const EVENT_JOB_SCHEDULED: &str = "job.scheduled";
pub const EVENT_JOB_EXPIRED: &str = "job.expired";
pub const EVENT_JOB_PROGRESS: &str = "job.progress";

// Queue events
pub const EVENT_QUEUE_PAUSED: &str = "queue.paused";
pub const EVENT_QUEUE_RESUMED: &str = "queue.resumed";

// Worker events
pub const EVENT_WORKER_STARTED: &str = "worker.started";
pub const EVENT_WORKER_STOPPED: &str = "worker.stopped";
pub const EVENT_WORKER_QUIET: &str = "worker.quiet";
pub const EVENT_WORKER_HEARTBEAT: &str = "worker.heartbeat";

// Workflow events
pub const EVENT_WORKFLOW_STARTED: &str = "workflow.started";
pub const EVENT_WORKFLOW_STEP_COMPLETED: &str = "workflow.step_completed";
pub const EVENT_WORKFLOW_COMPLETED: &str = "workflow.completed";
pub const EVENT_WORKFLOW_FAILED: &str = "workflow.failed";

// Cron events
pub const EVENT_CRON_TRIGGERED: &str = "cron.triggered";
pub const EVENT_CRON_SKIPPED: &str = "cron.skipped";

// ---------------------------------------------------------------------------
// Event envelope (CloudEvents-inspired)
// ---------------------------------------------------------------------------

/// An OJS lifecycle event following the CloudEvents envelope format.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    /// Event specification version (always "1.0").
    #[serde(default = "default_event_specversion")]
    pub specversion: String,

    /// Unique event identifier.
    pub id: String,

    /// Event type (e.g., "job.completed").
    #[serde(rename = "type")]
    pub event_type: String,

    /// Event source URI (e.g., "ojs://my-service/workers/worker-1").
    pub source: String,

    /// When the event occurred.
    pub time: DateTime<Utc>,

    /// Event subject (typically a job ID, queue name, etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,

    /// Content type of the data payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datacontenttype: Option<String>,

    /// Event-specific payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<HashMap<String, serde_json::Value>>,
}

fn default_event_specversion() -> String {
    "1.0".to_string()
}
