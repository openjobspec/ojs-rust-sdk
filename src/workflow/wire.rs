//! Request wire encoder and wire-format types for workflow creation.

use super::definition::{normalize_args, resolve_options, WorkflowDefinition, WorkflowType};
use crate::job::EnqueueOptionsWire;
use serde::Serialize;

impl WorkflowDefinition {
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

#[cfg(test)]
mod tests {
    use crate::workflow::{batch, chain, group, BatchCallbacks, Step};
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
}
