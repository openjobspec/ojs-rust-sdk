#![cfg_attr(docsrs, feature(doc_cfg))]
#![deny(warnings)]
#![deny(clippy::all)]
#![warn(clippy::pedantic)]
// Allowed pedantic lints for existing codebase compatibility
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::return_self_not_must_use)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_lossless)]
#![allow(clippy::struct_field_names)]
#![allow(clippy::struct_excessive_bools)]
#![allow(clippy::needless_pass_by_value)]
#![allow(clippy::trivially_copy_pass_by_ref)]
#![allow(clippy::single_match_else)]
#![allow(clippy::missing_fields_in_debug)]
#![allow(clippy::ref_option)]
#![allow(clippy::uninlined_format_args)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::manual_let_else)]
#![allow(clippy::ignored_unit_patterns)]
#![allow(clippy::float_cmp)]
#![allow(clippy::map_unwrap_or)]
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
//! ```rust,no_run
//! use ojs::{Client, RetryPolicy};
//! use serde_json::json;
//!
//! # #[tokio::main]
//! # async fn main() -> ojs::Result<()> {
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
//! # Ok(())
//! # }
//! ```
//!
//! ### Processing Jobs (Consumer)
//!
//! ```rust,no_run
//! use ojs::{Worker, JobContext};
//! use serde_json::json;
//!
//! # #[tokio::main]
//! # async fn main() -> ojs::Result<()> {
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
//! # Ok(())
//! # }
//! ```
//!
//! ## Features
//!
//! - **Async-first**: Built on `tokio` for high-performance async I/O
//! - **Type-safe**: Strong typing with serde serialization
//! - **Middleware**: Tower-inspired middleware chain for cross-cutting concerns
//! - **Workflows**: Chain, group, and batch workflow primitives
//! - **Full OJS compliance**: Implements the complete OJS v1.0 specification

pub mod client;
pub mod config;
pub mod error_codes;
pub mod errors;
pub mod events;
pub mod job;
pub mod middleware;
pub mod queue;
pub mod rate_limiter;
pub mod retry;
pub mod schema;
#[cfg(feature = "testing")]
#[cfg_attr(docsrs, doc(cfg(feature = "testing")))]
pub mod testing;
pub mod transport;
pub mod worker;
pub mod workflow;

/// Server-Sent Events (SSE) subscription for real-time job events.
pub mod subscribe;

/// Tracing middleware for structured job processing instrumentation.
#[cfg(feature = "tracing-middleware")]
#[cfg_attr(docsrs, doc(cfg(feature = "tracing-middleware")))]
pub mod tracing_mw;

/// Common middleware implementations (logging, timeout, metrics).
#[cfg(feature = "common-middleware")]
#[cfg_attr(docsrs, doc(cfg(feature = "common-middleware")))]
pub mod middleware_common;

/// Native OpenTelemetry tracing and metrics middleware.
#[cfg(feature = "otel-middleware")]
#[cfg_attr(docsrs, doc(cfg(feature = "otel-middleware")))]
pub mod otel;

/// Serverless adapters for AWS Lambda and other FaaS platforms.
#[cfg(feature = "serverless-lambda")]
#[cfg_attr(docsrs, doc(cfg(feature = "serverless-lambda")))]
pub mod serverless;

// ---------------------------------------------------------------------------
// Public re-exports
// ---------------------------------------------------------------------------

pub use client::{Client, ClientBuilder, EnqueueBuilder, JobRequest};
pub use config::ConnectionConfig;
pub use errors::{JobError, OjsError, RateLimitInfo, Result, ServerError};
pub use rate_limiter::RetryConfig;
pub use events::Event;
pub use job::{ConflictStrategy, Job, JobState, UniqueDimension, UniquePolicy};
pub use middleware::{BoxFuture, FnMiddleware, HandlerResult, Middleware, Next};
pub use queue::{
    CronJob, CronJobRequest, HealthStatus, Manifest, OverlapPolicy, Pagination, Queue, QueueStats,
};
pub use retry::{OnExhaustion, RetryPolicy};
pub use schema::{RegisterSchemaRequest, Schema, SchemaDetail};
pub use transport::{DynTransport, Method as TransportMethod, Transport};
pub use worker::{JobContext, Worker, WorkerBuilder, WorkerState};
pub use workflow::{
    batch, chain, group, BatchCallbacks, EnqueueOption, Step, Workflow, WorkflowDefinition,
    WorkflowState, WorkflowStepStatus, WorkflowType,
};

/// The OJS specification version implemented by this SDK.
pub const OJS_VERSION: &str = "1.0";
