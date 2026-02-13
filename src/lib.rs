//! # OJS - Open Job Spec SDK for Rust
//!
//! The official Rust SDK for the [Open Job Spec](https://openjobspec.org) protocol.
//! OJS is a language-agnostic specification for background job processing, providing
//! interoperability across languages and backends.
//!
//! ## Quick Start
//!
//! ### Enqueuing Jobs (Producer)
//!
//! ```rust,ignore
//! use ojs::Client;
//! use serde_json::json;
//!
//! let client = Client::builder()
//!     .url("http://localhost:8080")
//!     .build()?;
//!
//! // Simple enqueue
//! let job = client.enqueue("email.send", json!({"to": "user@example.com"})).await?;
//!
//! // Enqueue with options
//! let job = client.enqueue("report.generate", json!({"id": 42}))
//!     .queue("reports")
//!     .delay(std::time::Duration::from_secs(300))
//!     .retry(RetryPolicy::new().max_attempts(5))
//!     .send()
//!     .await?;
//! ```
//!
//! ### Processing Jobs (Consumer)
//!
//! ```rust,ignore
//! use ojs::{Worker, JobContext};
//! use serde_json::json;
//!
//! let worker = Worker::builder()
//!     .url("http://localhost:8080")
//!     .queues(vec!["default", "email"])
//!     .concurrency(10)
//!     .build()?;
//!
//! worker.register("email.send", |ctx: JobContext| async move {
//!     let to: String = ctx.job.arg("to")?;
//!     // send the email...
//!     Ok(json!({"status": "sent"}))
//! }).await;
//!
//! worker.start().await?;
//! ```
//!
//! ## Features
//!
//! - **Async-first**: Built on `tokio` for high-performance async I/O
//! - **Type-safe**: Strong typing with serde serialization
//! - **Middleware**: Tower-inspired middleware chain for cross-cutting concerns
//! - **Workflows**: Chain, group, and batch workflow primitives
//! - **Full OJS compliance**: Implements the complete OJS v1.0.0-rc.1 specification

pub mod client;
pub mod errors;
pub mod events;
pub mod job;
pub mod middleware;
pub mod queue;
pub mod retry;
mod transport;
pub mod worker;
pub mod workflow;

// ---------------------------------------------------------------------------
// Public re-exports
// ---------------------------------------------------------------------------

pub use client::{Client, ClientBuilder, EnqueueBuilder, JobRequest};
pub use errors::{JobError, OjsError, Result, ServerError};
pub use events::Event;
pub use job::{ConflictStrategy, Job, JobState, UniqueDimension, UniquePolicy};
pub use middleware::{BoxFuture, HandlerResult, Middleware, Next};
pub use queue::{
    CronJob, CronJobRequest, HealthStatus, Manifest, OverlapPolicy, Pagination, Queue, QueueStats,
};
pub use retry::{OnExhaustion, RetryPolicy};
pub use worker::{JobContext, Worker, WorkerBuilder, WorkerState};
pub use workflow::{
    batch, chain, group, BatchCallbacks, EnqueueOption, Step, Workflow, WorkflowDefinition,
    WorkflowState, WorkflowStepStatus, WorkflowType,
};

/// The OJS specification version implemented by this SDK.
pub const OJS_VERSION: &str = "1.0.0-rc.1";
