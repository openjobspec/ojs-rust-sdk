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

#[cfg(test)]
mod tests {
    use super::super::definition::{batch, chain, group, BatchCallbacks, EnqueueOption, Step};
    use serde_json::json;

    #[test]
    fn test_chain_wire_format() {
        let def = chain(vec![
            Step::new("fetch", json!({"url": "https://example.com"})),
            Step::new("transform", json!({"format": "csv"})),
            Step::new("notify", json!({"channel": "slack"})),
        ]);

        let wire = def.to_wire();
        let value = serde_json::to_value(&wire).unwrap();

        // Discriminated union per workflow.schema.json / the shared Go
        // backend: root `type` + `steps`, no `jobs`/`callbacks`, no
        // synthetic per-step `id`/`depends_on`, and no root `options`.
        assert_eq!(value["type"], "chain");
        assert!(value.get("jobs").is_none());
        assert!(value.get("callbacks").is_none());
        let steps = value["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 3);
        for step in steps {
            assert!(step.get("id").is_none());
            assert!(step.get("depends_on").is_none());
        }
        assert_eq!(steps[0]["type"], "fetch");
        assert_eq!(steps[1]["type"], "transform");
        assert_eq!(steps[2]["type"], "notify");
    }

    #[test]
    fn test_group_wire_format() {
        let def = group(vec![
            Step::new("export.csv", json!({})),
            Step::new("export.pdf", json!({})),
        ]);

        let wire = def.to_wire();
        let value = serde_json::to_value(&wire).unwrap();

        assert_eq!(value["type"], "group");
        assert!(value.get("steps").is_none());
        assert!(value.get("callbacks").is_none());
        let jobs = value["jobs"].as_array().unwrap();
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0]["type"], "export.csv");
        assert_eq!(jobs[1]["type"], "export.pdf");
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
        let value = serde_json::to_value(&wire).unwrap();

        assert_eq!(value["type"], "batch");
        assert!(value.get("steps").is_none());
        let jobs = value["jobs"].as_array().unwrap();
        assert_eq!(jobs.len(), 2); // callbacks are NOT folded into `jobs`
        assert_eq!(value["callbacks"]["on_complete"]["type"], "report");
        assert!(value["callbacks"].get("on_success").is_none());
        assert!(value["callbacks"].get("on_failure").is_none());
    }

    #[test]
    fn test_batch_requires_at_least_one_callback_field_present() {
        // `callbacks` itself must be present with only the configured
        // hooks; unset hooks must be omitted rather than emitted as `null`.
        let def = batch(
            BatchCallbacks::new()
                .on_success(Step::new("celebrate", json!({})))
                .on_failure(Step::new("alert", json!({}))),
            vec![Step::new("job.run", json!({}))],
        );
        let value = serde_json::to_value(def.to_wire()).unwrap();
        assert!(value["callbacks"].get("on_complete").is_none());
        assert_eq!(value["callbacks"]["on_success"]["type"], "celebrate");
        assert_eq!(value["callbacks"]["on_failure"]["type"], "alert");
    }

    #[test]
    fn test_workflow_level_default_options_materialize_into_each_job() {
        let def = chain(vec![
            Step::new("fetch", json!({})),
            Step::new("transform", json!({})).queue("fast-lane"),
        ])
        .with_option(EnqueueOption::Queue("default-queue".into()))
        .with_option(EnqueueOption::Priority(5));

        let value = serde_json::to_value(def.to_wire()).unwrap();
        let steps = value["steps"].as_array().unwrap();

        // Step 0 has no per-step override: inherits both workflow defaults.
        assert_eq!(steps[0]["options"]["queue"], "default-queue");
        assert_eq!(steps[0]["options"]["priority"], 5);

        // Step 1 overrides `queue` but still inherits `priority`.
        assert_eq!(steps[1]["options"]["queue"], "fast-lane");
        assert_eq!(steps[1]["options"]["priority"], 5);
    }
}
