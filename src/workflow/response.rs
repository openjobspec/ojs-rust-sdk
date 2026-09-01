//! Response models returned by the server for workflow operations:
//! [`Workflow`], [`WorkflowState`], and [`WorkflowStepStatus`].

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

/// A workflow instance returned from the server.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workflow {
    /// Workflow identifier.
    pub id: String,
    /// Workflow name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Current workflow state.
    pub state: WorkflowState,
    /// When the workflow was created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
    /// Individual workflow steps with their status.
    #[serde(default)]
    pub steps: Vec<WorkflowStepStatus>,

    // Cancel response fields
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancelled_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps_cancelled: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps_already_complete: Option<u32>,
}

/// Status of a single step within a workflow.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowStepStatus {
    /// Step identifier (e.g., "step-0", "job-1").
    pub id: String,
    /// Job type.
    #[serde(rename = "type")]
    pub job_type: String,
    /// Current state as a raw string from the server.
    pub state: String,
    /// Associated job ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    /// Step IDs this depends on.
    #[serde(default)]
    pub depends_on: Vec<String>,
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
    /// map to a known `JobState` variant.
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
            depends_on: vec![],
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
            depends_on: vec![],
            started_at: None,
            completed_at: None,
            result: None,
        };
        assert_eq!(step_unknown.job_state(), None);
    }
}
