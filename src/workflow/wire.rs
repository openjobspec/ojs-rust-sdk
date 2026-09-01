//! Request wire encoder and wire-format types for workflow creation.
//!
//! Matches the discriminated-union shape defined by `workflow.schema.json`
//! and implemented by the shared OJS Go backend
//! (`WorkflowRequest`/`WorkflowJobRequest` in `ojs-go-backend-common`): a
//! required root `type` plus `steps` (chain only), `jobs` (group and batch
//! only), and `callbacks` (batch only). There is no root `options` field
//! and no synthetic step `id`/`depends_on` -- chain order is simply array
//! position (see `ojs-workflows.md` §4.3), and group/batch jobs have no
//! ordering at all (§5.2).

use super::definition::{normalize_args, resolve_options, Step, WorkflowDefinition, WorkflowType};
use crate::job::EnqueueOptionsWire;
use serde::Serialize;

impl WorkflowDefinition {
    /// Convert to wire format for the HTTP request.
    pub(crate) fn to_wire(&self) -> WorkflowRequest {
        let build_job = |step: &Step| -> WorkflowJobWire {
            // Workflow-level defaults first, then this step's own options:
            // `resolve_options` folds a slice left-to-right, so later
            // (more specific) entries win when both set the same field.
            let mut combined = self.options.clone();
            combined.extend(step.options.iter().cloned());
            WorkflowJobWire {
                job_type: step.job_type.clone(),
                args: normalize_args(&step.args),
                options: resolve_options(&combined),
            }
        };

        let (steps, jobs, callbacks) = match self.workflow_type {
            WorkflowType::Chain => (Some(self.steps.iter().map(build_job).collect()), None, None),
            WorkflowType::Group => (None, Some(self.steps.iter().map(build_job).collect()), None),
            WorkflowType::Batch => (
                None,
                Some(self.steps.iter().map(build_job).collect()),
                self.callbacks.as_ref().map(|cb| WorkflowCallbacksWire {
                    on_complete: cb.on_complete.as_ref().map(build_job),
                    on_success: cb.on_success.as_ref().map(build_job),
                    on_failure: cb.on_failure.as_ref().map(build_job),
                }),
            ),
        };

        WorkflowRequest {
            workflow_type: self.workflow_type,
            name: self.name.clone(),
            steps,
            jobs,
            callbacks,
        }
    }
}

// ---------------------------------------------------------------------------
// Wire format types (request)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub(crate) struct WorkflowRequest {
    #[serde(rename = "type")]
    pub workflow_type: WorkflowType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Chain steps only. Mutually exclusive with `jobs`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steps: Option<Vec<WorkflowJobWire>>,
    /// Group/batch jobs only. Mutually exclusive with `steps`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jobs: Option<Vec<WorkflowJobWire>>,
    /// Batch callbacks only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub callbacks: Option<WorkflowCallbacksWire>,
}

/// A single job envelope within a workflow request (a chain step, a
/// group/batch job, or a batch callback). Matches `WorkflowJobRequest` in
/// the shared OJS Go backend: just `type`/`args`/`options`, with no client-
/// invented `id` or `depends_on` -- the backend assigns each step a
/// position-based identity (falling back to its array index) for status
/// reporting.
#[derive(Debug, Serialize)]
pub(crate) struct WorkflowJobWire {
    #[serde(rename = "type")]
    pub job_type: String,
    pub args: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<EnqueueOptionsWire>,
}

#[derive(Debug, Default, Serialize)]
pub(crate) struct WorkflowCallbacksWire {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_complete: Option<WorkflowJobWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_success: Option<WorkflowJobWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_failure: Option<WorkflowJobWire>,
}
