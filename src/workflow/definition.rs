//! User-facing workflow definition and builders: [`Step`], [`EnqueueOption`],
//! [`BatchCallbacks`], [`WorkflowDefinition`]/[`WorkflowType`], the
//! `chain`/`group`/`batch` constructors, and pre-transport validation.
//!
//! This is the "authoring" side of a workflow: the types and builders a
//! caller uses to describe what should run. Encoding that description onto
//! the wire is a separate concern (see the sibling `wire` module).

use crate::job::EnqueueOptionsWire;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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

    /// Validate the definition before it is sent to the server.
    pub(crate) fn validate(&self) -> crate::Result<()> {
        if self.steps.is_empty() {
            return Err(crate::OjsError::Builder(
                "workflow must contain at least one step or job".to_string(),
            ));
        }

        // Defaults are materialized into every step on the wire, so validate
        // them first even if an individual step overrides the same option.
        // Otherwise an invalid workflow-level default could escape local
        // validation depending on which steps happen to override it.
        crate::client::validate_enqueue_options(&self.options)?;

        let validate_step = |step: &Step| -> crate::Result<()> {
            crate::client::validate_job_type(&step.job_type)?;
            crate::client::validate_enqueue_options(&step.options)
        };

        for step in &self.steps {
            validate_step(step)?;
        }

        if self.workflow_type == WorkflowType::Batch {
            let callbacks = self.callbacks.as_ref().ok_or_else(|| {
                crate::OjsError::Builder(
                    "batch workflow must define at least one callback".to_string(),
                )
            })?;
            let callback_steps = [
                callbacks.on_complete.as_ref(),
                callbacks.on_success.as_ref(),
                callbacks.on_failure.as_ref(),
            ];
            if callback_steps.iter().all(Option::is_none) {
                return Err(crate::OjsError::Builder(
                    "batch workflow must define at least one callback".to_string(),
                ));
            }
            for callback in callback_steps.into_iter().flatten() {
                validate_step(callback)?;
            }
        }

        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_workflow_validation_rejects_empty_steps() {
        assert!(chain(Vec::new()).validate().is_err());
        assert!(group(Vec::new()).validate().is_err());
    }

    #[test]
    fn test_workflow_validation_rejects_empty_batch_callbacks() {
        let def = batch(BatchCallbacks::new(), vec![Step::new("job.run", json!({}))]);
        assert!(def.validate().is_err());
    }

    #[test]
    fn test_workflow_validation_checks_callback_job_types() {
        let def = batch(
            BatchCallbacks::new().on_complete(Step::new("Invalid.Type", json!({}))),
            vec![Step::new("job.run", json!({}))],
        );
        assert!(def.validate().is_err());
    }

    #[test]
    fn test_workflow_validation_rejects_invalid_default_before_valid_override() {
        let def = chain(vec![Step::new("job.run", json!({})).queue("valid-override")])
            .with_option(EnqueueOption::Queue("Invalid.Default".into()));

        let err = def.validate().unwrap_err();
        assert!(err.to_string().contains("Invalid.Default"));
    }

    #[test]
    fn test_workflow_validation_rejects_invalid_step_override() {
        let def = chain(vec![
            Step::new("job.run", json!({})).queue("Invalid.Override")
        ])
        .with_option(EnqueueOption::Queue("valid-default".into()));

        let err = def.validate().unwrap_err();
        assert!(err.to_string().contains("Invalid.Override"));
    }

    #[test]
    fn test_workflow_validation_accepts_valid_default_and_override() {
        let def = chain(vec![Step::new("job.run", json!({})).queue("valid-override")])
            .with_option(EnqueueOption::Queue("valid-default".into()));

        assert!(def.validate().is_ok());
    }

    #[test]
    fn test_workflow_step_queue_uses_255_byte_boundary() {
        let exact = "a".repeat(255);
        assert!(chain(vec![Step::new("job.run", json!({})).queue(exact)])
            .validate()
            .is_ok());

        let over = "a".repeat(256);
        let err = chain(vec![Step::new("job.run", json!({})).queue(over)])
            .validate()
            .unwrap_err()
            .to_string();
        assert!(err.contains("255 bytes"), "got: {err}");
    }

    #[test]
    fn test_workflow_default_queue_uses_utf8_byte_boundary() {
        let exact = chain(vec![Step::new("job.run", json!({}))])
            .with_option(EnqueueOption::Queue("a".repeat(255)));
        assert!(exact.validate().is_ok());

        let over = chain(vec![Step::new("job.run", json!({})).queue("valid-override")])
            .with_option(EnqueueOption::Queue("猫".repeat(86)));
        let err = over.validate().unwrap_err().to_string();
        assert!(err.contains("255 bytes"), "got: {err}");
    }
}
