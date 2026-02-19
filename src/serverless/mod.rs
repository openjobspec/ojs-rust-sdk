//! Serverless adapters for running OJS job handlers in serverless environments.
//!
//! This module provides adapters for running OJS job handlers inside serverless
//! functions such as AWS Lambda. The adapters bridge serverless invocation events
//! into OJS job processing, handling deserialization, handler dispatch, and
//! response formatting.
//!
//! # Feature Flag
//!
//! This module is only available when the `serverless-lambda` feature is enabled:
//!
//! ```toml
//! [dependencies]
//! ojs = { version = "0.1", features = ["serverless-lambda"] }
//! ```
//!
//! # AWS Lambda with SQS
//!
//! The most common pattern is using SQS event source mapping to trigger Lambda
//! functions. The [`LambdaHandler`] wraps your job handlers and translates SQS
//! events into OJS job processing:
//!
//! ```rust,ignore
//! use ojs::serverless::{LambdaHandler, JobEvent};
//! use lambda_runtime::{service_fn, LambdaEvent};
//! use aws_lambda_events::event::sqs::SqsEvent;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), lambda_runtime::Error> {
//!     let mut handler = LambdaHandler::new();
//!
//!     handler.register("email.send", |_ctx, job: JobEvent| async move {
//!         println!("Processing job: {}", job.id);
//!         Ok(())
//!     });
//!
//!     let shared = std::sync::Arc::new(handler);
//!     lambda_runtime::run(service_fn(move |event: LambdaEvent<SqsEvent>| {
//!         let handler = shared.clone();
//!         async move { handler.handle_sqs(event.payload).await }
//!     })).await
//! }
//! ```
//!
//! # HTTP Push Delivery
//!
//! For HTTP push delivery (OJS server pushes jobs to a Lambda Function URL),
//! use [`LambdaHandler::handle_http`].
//!
//! # Direct Invocation
//!
//! For direct Lambda invocation with a single job payload, use
//! [`LambdaHandler::handle_direct`].

#[cfg(feature = "serverless-lambda")]
#[cfg_attr(docsrs, doc(cfg(feature = "serverless-lambda")))]
pub mod aws_lambda;

#[cfg(feature = "serverless-lambda")]
#[cfg_attr(docsrs, doc(cfg(feature = "serverless-lambda")))]
pub use aws_lambda::*;
