use crate::job::EnqueueOptionsWire;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
// Step (user-facing)
// ---------------------------------------------------------------------------

/// A single step within a workflow.
#[derive(Debug, Clone)]
pub struct Step {
    /// Job type to execute.
    pub job_type: String,
    /// Job arguments.
    pub args: serde_json::Value,
    /// Per-step enqueue options.
    pub options: Vec<EnqueueOption>,
}

impl Step {
    /// Create a new workflow step.
    pub fn new(job_type: impl Into<String>, args: serde_json::Value) -> Self {
        Self {
            job_type: job_type.into(),
            args,
            options: Vec::new(),
        }
    }

    /// Add an enqueue option to this step.
    pub fn with_option(mut self, opt: EnqueueOption) -> Self {
        self.options.push(opt);
        self
    }

    /// Set the queue for this step.
    pub fn queue(self, queue: impl Into<String>) -> Self {
        self.with_option(EnqueueOption::Queue(queue.into()))
    }

    /// Set the priority for this step.
    pub fn priority(self, priority: i32) -> Self {
        self.with_option(EnqueueOption::Priority(priority))
    }

    /// Set the timeout for this step.
    pub fn timeout(self, timeout: std::time::Duration) -> Self {
        self.with_option(EnqueueOption::Timeout(timeout))
    }
}

// ---------------------------------------------------------------------------
// Enqueue options (used by both client and workflow steps)
// ---------------------------------------------------------------------------

/// Options that can be applied when enqueuing a job.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum EnqueueOption {
    Queue(String),
    Priority(i32),
    Timeout(std::time::Duration),
    Delay(std::time::Duration),
    ScheduledAt(DateTime<Utc>),
    ExpiresAt(DateTime<Utc>),
    Retry(crate::RetryPolicy),
    Unique(crate::job::UniquePolicy),
    Tags(Vec<String>),
    Meta(HashMap<String, serde_json::Value>),
    VisibilityTimeout(std::time::Duration),
}

/// Resolve a list of enqueue options into the wire format.
pub(crate) fn resolve_options(opts: &[EnqueueOption]) -> Option<EnqueueOptionsWire> {
    if opts.is_empty() {
        return None;
    }

    let mut wire = EnqueueOptionsWire::default();

    for opt in opts {
        match opt {
            EnqueueOption::Queue(q) => wire.queue = Some(q.clone()),
            EnqueueOption::Priority(p) => wire.priority = Some(*p),
            EnqueueOption::Timeout(d) => wire.timeout_ms = Some(d.as_millis() as u64),
            EnqueueOption::Delay(d) => {
                wire.delay_until =
                    Some(Utc::now() + chrono::Duration::from_std(*d).unwrap_or_default());
            }
            EnqueueOption::ScheduledAt(t) => wire.delay_until = Some(*t),
            EnqueueOption::ExpiresAt(t) => wire.expires_at = Some(*t),
            EnqueueOption::Retry(r) => wire.retry = Some(r.clone()),
            EnqueueOption::Unique(u) => wire.unique = Some(u.clone()),
            EnqueueOption::Tags(t) => wire.tags = Some(t.clone()),
            EnqueueOption::VisibilityTimeout(d) => {
                wire.visibility_timeout_ms = Some(d.as_millis() as u64);
            }
            EnqueueOption::Meta(_) => { /* meta is handled separately on the request body */ }
        }
    }

    Some(wire)
}

/// Extract meta from enqueue options.
pub(crate) fn extract_meta(opts: &[EnqueueOption]) -> Option<HashMap<String, serde_json::Value>> {
    for opt in opts {
        if let EnqueueOption::Meta(m) = opt {
            return Some(m.clone());
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Batch callbacks
// ---------------------------------------------------------------------------

/// Callbacks for batch workflows.
#[derive(Debug, Clone)]
pub struct BatchCallbacks {
    /// Runs when ALL jobs finish (regardless of outcome).
    pub on_complete: Option<Step>,
    /// Runs only if ALL jobs succeeded.
    pub on_success: Option<Step>,
    /// Runs if ANY job failed.
    pub on_failure: Option<Step>,
}

impl BatchCallbacks {
    pub fn new() -> Self {
        Self {
            on_complete: None,
            on_success: None,
            on_failure: None,
        }
    }

    pub fn on_complete(mut self, step: Step) -> Self {
        self.on_complete = Some(step);
        self
    }

    pub fn on_success(mut self, step: Step) -> Self {
        self.on_success = Some(step);
        self
    }

    pub fn on_failure(mut self, step: Step) -> Self {
        self.on_failure = Some(step);
        self
    }
}

impl Default for BatchCallbacks {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Workflow definition (user-facing)
// ---------------------------------------------------------------------------

/// A workflow definition describing how jobs should be orchestrated.
#[derive(Debug, Clone)]
pub struct WorkflowDefinition {
    /// Workflow type: "chain", "group", or "batch".
    pub workflow_type: WorkflowType,
    /// Human-readable workflow name.
    pub name: Option<String>,
    /// Steps (for chain) or jobs (for group/batch).
    pub steps: Vec<Step>,
    /// Callbacks (for batch workflows).
    pub callbacks: Option<BatchCallbacks>,
    /// Default options applied to all steps.
    pub options: Vec<EnqueueOption>,
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowType {
    Chain,
    Group,
    Batch,
}

impl std::fmt::Display for WorkflowType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkflowType::Chain => write!(f, "chain"),
            WorkflowType::Group => write!(f, "group"),
            WorkflowType::Batch => write!(f, "batch"),
        }
    }
}

/// Create a chain workflow (sequential execution).
///
/// Each step depends on the previous one completing successfully.
pub fn chain(steps: Vec<Step>) -> WorkflowDefinition {
    WorkflowDefinition {
        workflow_type: WorkflowType::Chain,
        name: None,
        steps,
        callbacks: None,
        options: Vec::new(),
    }
}

/// Create a group workflow (parallel execution).
///
/// All jobs execute concurrently with no dependencies between them.
pub fn group(jobs: Vec<Step>) -> WorkflowDefinition {
    WorkflowDefinition {
        workflow_type: WorkflowType::Group,
        name: None,
        steps: jobs,
        callbacks: None,
        options: Vec::new(),
    }
}

/// Create a batch workflow (parallel execution with callbacks).
///
/// All jobs execute concurrently. When they complete, the appropriate
/// callback is triggered based on the outcome.
pub fn batch(callbacks: BatchCallbacks, jobs: Vec<Step>) -> WorkflowDefinition {
    WorkflowDefinition {
        workflow_type: WorkflowType::Batch,
        name: None,
        steps: jobs,
        callbacks: Some(callbacks),
        options: Vec::new(),
    }
}

impl WorkflowDefinition {
    /// Set a human-readable name for this workflow.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Add default options applied to all steps.
    pub fn with_option(mut self, opt: EnqueueOption) -> Self {
        self.options.push(opt);
        self
    }

    /// Convert to wire format for the HTTP request.
    pub(crate) fn to_wire(&self) -> WorkflowRequest {
        let mut wire_steps = Vec::new();

        match self.workflow_type {
            WorkflowType::Chain => {
                for (i, step) in self.steps.iter().enumerate() {
                    let depends_on = if i > 0 {
                        vec![format!("step-{}", i - 1)]
                    } else {
                        vec![]
                    };
                    wire_steps.push(WorkflowStepWire {
                        id: format!("step-{}", i),
                        job_type: step.job_type.clone(),
                        args: normalize_args(&step.args),
                        depends_on,
                        options: resolve_options(&step.options),
                    });
                }
            }
            WorkflowType::Group => {
                for (i, step) in self.steps.iter().enumerate() {
                    wire_steps.push(WorkflowStepWire {
                        id: format!("job-{}", i),
                        job_type: step.job_type.clone(),
                        args: normalize_args(&step.args),
                        depends_on: vec![],
                        options: resolve_options(&step.options),
                    });
                }
            }
            WorkflowType::Batch => {
                let job_ids: Vec<String> = (0..self.steps.len())
                    .map(|i| format!("job-{}", i))
                    .collect();

                for (i, step) in self.steps.iter().enumerate() {
                    wire_steps.push(WorkflowStepWire {
                        id: format!("job-{}", i),
                        job_type: step.job_type.clone(),
                        args: normalize_args(&step.args),
                        depends_on: vec![],
                        options: resolve_options(&step.options),
                    });
                }

                if let Some(ref callbacks) = self.callbacks {
                    if let Some(ref step) = callbacks.on_complete {
                        wire_steps.push(WorkflowStepWire {
                            id: "on-complete".into(),
                            job_type: step.job_type.clone(),
                            args: normalize_args(&step.args),
                            depends_on: job_ids.clone(),
                            options: resolve_options(&step.options),
                        });
                    }
                    if let Some(ref step) = callbacks.on_success {
                        wire_steps.push(WorkflowStepWire {
                            id: "on-success".into(),
                            job_type: step.job_type.clone(),
                            args: normalize_args(&step.args),
                            depends_on: job_ids.clone(),
                            options: resolve_options(&step.options),
                        });
                    }
                    if let Some(ref step) = callbacks.on_failure {
                        wire_steps.push(WorkflowStepWire {
                            id: "on-failure".into(),
                            job_type: step.job_type.clone(),
                            args: normalize_args(&step.args),
                            depends_on: job_ids,
                            options: resolve_options(&step.options),
                        });
                    }
                }
            }
        }

        WorkflowRequest {
            name: self.name.clone(),
            steps: wire_steps,
            options: resolve_options(&self.options),
        }
    }
}

/// Normalize args into wire format (JSON array).
pub(crate) fn normalize_args(args: &serde_json::Value) -> serde_json::Value {
    match args {
        serde_json::Value::Array(_) => args.clone(),
        obj @ serde_json::Value::Object(_) => serde_json::Value::Array(vec![obj.clone()]),
        other => serde_json::Value::Array(vec![other.clone()]),
    }
}

// ---------------------------------------------------------------------------
// Wire format types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub(crate) struct WorkflowRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub steps: Vec<WorkflowStepWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<EnqueueOptionsWire>,
}

#[derive(Debug, Serialize)]
pub(crate) struct WorkflowStepWire {
    pub id: String,
    #[serde(rename = "type")]
    pub job_type: String,
    pub args: serde_json::Value,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<EnqueueOptionsWire>,
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
    use serde_json::json;

    #[test]
    fn test_chain_wire_format() {
        let def = chain(vec![
            Step::new("fetch", json!({"url": "https://example.com"})),
            Step::new("transform", json!({"format": "csv"})),
            Step::new("notify", json!({"channel": "slack"})),
        ]);

        let wire = def.to_wire();
        assert_eq!(wire.steps.len(), 3);
        assert!(wire.steps[0].depends_on.is_empty());
        assert_eq!(wire.steps[1].depends_on, vec!["step-0"]);
        assert_eq!(wire.steps[2].depends_on, vec!["step-1"]);
    }

    #[test]
    fn test_group_wire_format() {
        let def = group(vec![
            Step::new("export.csv", json!({})),
            Step::new("export.pdf", json!({})),
        ]);

        let wire = def.to_wire();
        assert_eq!(wire.steps.len(), 2);
        assert!(wire.steps[0].depends_on.is_empty());
        assert!(wire.steps[1].depends_on.is_empty());
    }

    #[test]
    fn test_batch_wire_format() {
        let def = batch(
            BatchCallbacks::new().on_complete(Step::new("report", json!({}))),
            vec![
                Step::new("email.send", json!({"to": "a@b.com"})),
                Step::new("email.send", json!({"to": "c@d.com"})),
            ],
        );

        let wire = def.to_wire();
        assert_eq!(wire.steps.len(), 3); // 2 jobs + 1 callback
        assert_eq!(wire.steps[2].id, "on-complete");
        assert_eq!(wire.steps[2].depends_on, vec!["job-0", "job-1"]);
    }

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
