//! Response models returned by the server for workflow operations:
//! [`Workflow`], [`WorkflowState`], and [`WorkflowStepStatus`].

use super::definition::WorkflowType;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Workflow state
// ---------------------------------------------------------------------------

/// The lifecycle state of a workflow.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowState {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl std::fmt::Display for WorkflowState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            WorkflowState::Pending => "pending",
            WorkflowState::Running => "running",
            WorkflowState::Completed => "completed",
            WorkflowState::Failed => "failed",
            WorkflowState::Cancelled => "cancelled",
        };
        write!(f, "{}", s)
    }
}

// ---------------------------------------------------------------------------
// Workflow (response type)
// ---------------------------------------------------------------------------

/// Wire envelope wrapping a workflow response (`{"workflow": {...}}`), as
/// returned by `POST /workflows`, `GET /workflows/:id`, and
/// `DELETE /workflows/:id`.
#[derive(Debug, Deserialize)]
pub(crate) struct WorkflowResponseWire {
    pub workflow: Workflow,
}

/// A workflow instance returned from the server.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workflow {
    /// Workflow identifier.
    pub id: String,
    /// Workflow name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Workflow primitive type ("chain", "group", or "batch"), when the
    /// server includes it.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub workflow_type: Option<WorkflowType>,
    /// Current workflow state.
    pub state: WorkflowState,
    /// When the workflow was created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
    /// When the workflow completed (successfully or otherwise).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    /// When the workflow was cancelled, if it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancelled_at: Option<DateTime<Utc>>,
    /// Number of steps cancelled by a workflow cancellation request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps_cancelled: Option<u32>,
    /// Number of steps already complete when cancellation was requested.
    ///
    /// The public field retains its original name for source compatibility
    /// and accepts the specification's `steps_already_completed` spelling.
    #[serde(
        default,
        alias = "steps_already_completed",
        skip_serializing_if = "Option::is_none"
    )]
    pub steps_already_complete: Option<u32>,
    /// Total chain steps, for chain workflows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps_total: Option<u32>,
    /// Completed chain steps, for chain workflows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps_completed: Option<u32>,
    /// Total group/batch jobs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jobs_total: Option<u32>,
    /// Completed group/batch jobs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jobs_completed: Option<u32>,
    /// Batch callback definitions, echoed back for batch workflows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callbacks: Option<serde_json::Value>,
    /// Individual workflow steps with their status, when the server
    /// includes per-step detail.
    #[serde(default)]
    pub steps: Vec<WorkflowStepStatus>,
}

/// Status of a single step within a workflow.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowStepStatus {
    /// Step identifier, or an empty string when an older/minimal server
    /// response omits it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// Job type.
    #[serde(rename = "type")]
    pub job_type: String,
    /// Current state as a raw string from the server (e.g. "waiting",
    /// "pending", "active", "completed", "failed", "cancelled").
    pub state: String,
    /// Associated job ID, absent while the step is still `waiting`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    /// Step identifiers this step depends on.
    ///
    /// Current conforming responses generally omit this legacy field, but
    /// it remains public for backward compatibility.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    /// The step's persisted enqueue options, if it declared any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<serde_json::Value>,
    /// When execution started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    /// When execution completed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    /// Step result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
}

impl WorkflowStepStatus {
    /// Try to parse the state as a typed [`crate::JobState`].
    ///
    /// Returns `None` if the server returned a state string that doesn't
    /// map to a known `JobState` variant (e.g. the workflow-only `waiting`
    /// state, which has no direct `JobState` equivalent).
    pub fn job_state(&self) -> Option<crate::JobState> {
        serde_json::from_value(serde_json::Value::String(self.state.clone())).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_workflow_step_status_job_state() {
        let step = WorkflowStepStatus {
            id: "step-0".into(),
            job_type: "fetch".into(),
            state: "completed".into(),
            job_id: None,
            depends_on: Vec::new(),
            options: None,
            started_at: None,
            completed_at: None,
            result: None,
        };
        assert_eq!(step.job_state(), Some(crate::JobState::Completed));

        let step_unknown = WorkflowStepStatus {
            id: "step-1".into(),
            job_type: "fetch".into(),
            state: "unknown_state".into(),
            job_id: None,
            depends_on: Vec::new(),
            options: None,
            started_at: None,
            completed_at: None,
            result: None,
        };
        assert_eq!(step_unknown.job_state(), None);
    }
}
