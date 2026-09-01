//! Workflow orchestration: chain, group, and batch primitives.
//!
//! Split into three cohesive actors, each in its own submodule:
//!
//! - `definition` — the user-facing workflow definition/builders
//!   ([`WorkflowDefinition`], [`Step`], [`EnqueueOption`], [`BatchCallbacks`],
//!   [`WorkflowType`], and the [`chain`]/[`group`]/[`batch`] constructors).
//! - `wire` — the request wire encoder (`WorkflowDefinition::to_wire`) and
//!   its wire-format types. Crate-private: nothing outside this module tree
//!   names them.
//! - `response` — the response models returned by the server ([`Workflow`],
//!   [`WorkflowState`], [`WorkflowStepStatus`]).

mod definition;
mod response;
mod wire;

pub use definition::{
    batch, chain, group, BatchCallbacks, EnqueueOption, Step, WorkflowDefinition, WorkflowType,
};
pub(crate) use definition::{extract_meta, normalize_args, resolve_options};
pub use response::{Workflow, WorkflowState, WorkflowStepStatus};
