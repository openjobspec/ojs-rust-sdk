//! [`JobContext`]: the per-job handler context, its heartbeat method, and
//! its durable-execution checkpoint save/get/delete methods.

use crate::errors::OjsError;
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

    // -----------------------------------------------------------------------
    // Durable execution: checkpoint support
    // -----------------------------------------------------------------------
    //
    // Implements the checkpoint API from `ojs-durable-execution.md` §4:
    // long-running jobs can persist intermediate state so that a crash or
    // visibility-timeout re-fetch can resume from the last checkpoint
    // instead of restarting from scratch. Per §9.1, OJS v0.1 intentionally
    // does NOT define a deterministic-replay model (no recorded `now()`/
    // `random()`/side-effect log) -- only this explicit, worker-controlled
    // checkpoint state. All three methods go through the same `Transport`
    // used for every other operation, so they inherit standard headers,
    // authentication, and retry behavior automatically.

    /// Save a checkpoint with the given state, overwriting any previous
    /// checkpoint for this job (`POST /jobs/:id/checkpoint`).
    pub async fn checkpoint<T: serde::Serialize>(&self, state: &T) -> crate::Result<()> {
        let body = serde_json::json!({ "state": state });
        transport::transport_post_no_response(
            &self.transport,
            &format!("/jobs/{}/checkpoint", self.job.id),
            &body,
        )
        .await
    }

    /// Retrieve the most recently saved checkpoint for this job
    /// (`GET /jobs/:id/checkpoint`).
    ///
    /// Returns `Ok(None)` if no checkpoint exists yet (server responds
    /// `404 Not Found`), matching the resume pattern described in
    /// `ojs-durable-execution.md` §6.2: absent checkpoint means start from
    /// `args`; present checkpoint means resume from its state.
    ///
    /// Note: on the first attempt after a crash/re-fetch, the backend also
    /// includes the checkpoint directly on the job envelope (see
    /// [`Job::checkpoint`]); this method additionally supports explicitly
    /// (re-)fetching the latest checkpoint at any point during execution.
    pub async fn get_checkpoint<T: serde::de::DeserializeOwned>(&self) -> crate::Result<Option<T>> {
        let path = format!("/jobs/{}/checkpoint", self.job.id);
        let resp: CheckpointEnvelope = match transport::transport_get(&self.transport, &path).await
        {
            Ok(resp) => resp,
            Err(OjsError::Server(ref err)) if err.http_status == 404 => return Ok(None),
            Err(e) => return Err(e),
        };
        let value: T = serde_json::from_value(resp.checkpoint.state).map_err(|e| {
            OjsError::Serialization(format!("failed to parse checkpoint state: {e}"))
        })?;
        Ok(Some(value))
    }

    /// Delete the checkpoint for this job (`DELETE /jobs/:id/checkpoint`).
    ///
    /// Treats "no checkpoint exists" (`404 Not Found`) as success, matching
    /// ordinary DELETE idempotency: deleting something already absent is
    /// not an error.
    pub async fn delete_checkpoint(&self) -> crate::Result<()> {
        let path = format!("/jobs/{}/checkpoint", self.job.id);
        match transport::transport_delete_no_response(&self.transport, &path).await {
            Ok(()) => Ok(()),
            Err(OjsError::Server(ref err)) if err.http_status == 404 => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// Wire envelope for `GET /jobs/:id/checkpoint`: `{"checkpoint": {"state": ...}}`
/// per `ojs-durable-execution.md` §4.3.
#[derive(Debug, serde::Deserialize)]
struct CheckpointEnvelope {
    checkpoint: CheckpointWire,
}

#[derive(Debug, serde::Deserialize)]
struct CheckpointWire {
    #[serde(default)]
    state: serde_json::Value,
}
