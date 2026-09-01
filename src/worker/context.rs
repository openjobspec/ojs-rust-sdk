//! [`JobContext`]: the per-job handler context and its heartbeat method.

use crate::job::{HeartbeatRequest, HeartbeatResponse, Job};
use crate::transport::{self, DynTransport};
use std::collections::HashMap;

/// Context passed to job handler functions.
///
/// Contains the job being processed, metadata about the execution attempt,
/// and methods for interacting with the worker (e.g., heartbeat, set result).
#[derive(Clone)]
pub struct JobContext {
    /// The job being processed.
    pub job: Job,
    /// Current attempt number (1-indexed).
    pub attempt: u32,
    /// Queue the job was fetched from.
    pub queue: String,
    /// Workflow ID if this job is part of a workflow.
    pub workflow_id: Option<String>,
    /// Results from upstream workflow steps.
    pub parent_results: Option<HashMap<String, serde_json::Value>>,

    // Internal. `pub(crate)` (rather than module-private) so the dispatch
    // loop in the sibling `protocol` submodule can construct a `JobContext`
    // directly; still entirely invisible outside this crate.
    pub(crate) transport: DynTransport,
    pub(crate) worker_id: String,
}

impl JobContext {
    /// Send a heartbeat to extend the visibility timeout for this job.
    ///
    /// Call this periodically in long-running handlers to prevent the job
    /// from being considered stale and re-dispatched.
    pub async fn heartbeat(&self) -> crate::Result<()> {
        let req = HeartbeatRequest {
            worker_id: self.worker_id.clone(),
            active_jobs: Some(vec![self.job.id.clone()]),
            visibility_timeout_ms: None,
        };
        let _: HeartbeatResponse =
            transport::transport_post(&self.transport, "/workers/heartbeat", &req).await?;
        Ok(())
    }
}
