//! AWS Lambda adapter for OJS job processing.
//!
//! Provides a [`LambdaHandler`] that processes OJS jobs delivered via:
//!
//! - **SQS event source mapping** (recommended): Lambda receives batched SQS
//!   messages containing OJS job payloads. Returns partial batch failures so
//!   SQS only retries the failed messages.
//!
//! - **HTTP push delivery**: An OJS server POSTs job payloads to a Lambda
//!   Function URL. Returns OJS-compatible push delivery responses.
//!
//! - **Direct invocation**: A single OJS job event is passed directly to the
//!   Lambda function.
//!
//! # Example: SQS Event Source Mapping
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
//!         // Process the job
//!         println!("Sending email for job {}", job.id);
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
//! # Example: Direct Invocation
//!
//! ```rust,ignore
//! use ojs::serverless::{LambdaHandler, JobEvent, DirectResponse};
//! use lambda_runtime::{service_fn, LambdaEvent};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), lambda_runtime::Error> {
//!     let mut handler = LambdaHandler::new();
//!
//!     handler.register("report.generate", |_ctx, job: JobEvent| async move {
//!         println!("Generating report for job {}", job.id);
//!         Ok(())
//!     });
//!
//!     let shared = std::sync::Arc::new(handler);
//!     lambda_runtime::run(service_fn(move |event: LambdaEvent<JobEvent>| {
//!         let handler = shared.clone();
//!         async move {
//!             let resp = handler.handle_direct(event.payload).await;
//!             Ok::<DirectResponse, lambda_runtime::Error>(resp)
//!         }
//!     })).await
//! }
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::RwLock;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// An OJS job delivered to a serverless function.
///
/// This is a simplified job envelope containing only the fields relevant
/// for serverless processing. It is deserialized from SQS message bodies,
/// HTTP push delivery requests, or direct invocation events.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobEvent {
    /// Unique job identifier (UUIDv7).
    pub id: String,

    /// Dot-namespaced job type (e.g., `email.send`).
    #[serde(rename = "type")]
    pub job_type: String,

    /// Target queue name.
    #[serde(default = "default_queue")]
    pub queue: String,

    /// Positional job arguments.
    #[serde(default = "default_args")]
    pub args: serde_json::Value,

    /// Current attempt number.
    #[serde(default = "default_attempt")]
    pub attempt: u32,

    /// Extensible metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Value>,

    /// Job priority.
    #[serde(default)]
    pub priority: i32,
}

fn default_queue() -> String {
    "default".to_string()
}

fn default_args() -> serde_json::Value {
    serde_json::Value::Array(vec![])
}

fn default_attempt() -> u32 {
    1
}

/// SQS event containing one or more messages.
///
/// This mirrors the AWS SQS event structure. When using the
/// `aws_lambda_events` crate, you can use its `SqsEvent` type directly
/// and call [`LambdaHandler::handle_sqs_records`] with the records.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SqsEvent {
    /// The SQS message records.
    #[serde(rename = "Records", default)]
    pub records: Vec<SqsMessage>,
}

/// A single SQS message containing an OJS job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SqsMessage {
    /// Unique SQS message identifier.
    #[serde(rename = "messageId", default)]
    pub message_id: String,

    /// The message body (JSON-encoded OJS job).
    #[serde(default)]
    pub body: String,

    /// SQS message attributes.
    #[serde(default)]
    pub attributes: HashMap<String, String>,

    /// The receipt handle for deleting the message.
    #[serde(rename = "receiptHandle", default)]
    pub receipt_handle: String,
}

/// Response format for SQS batch item failures.
///
/// Returning failed message IDs tells SQS to retry only those messages.
/// See: <https://docs.aws.amazon.com/lambda/latest/dg/with-sqs.html>
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SqsBatchResponse {
    /// List of failed message identifiers.
    #[serde(rename = "batchItemFailures")]
    pub batch_item_failures: Vec<BatchItemFailure>,
}

/// Identifies a single failed message in an SQS batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchItemFailure {
    /// The SQS message ID of the failed item.
    #[serde(rename = "itemIdentifier")]
    pub item_identifier: String,
}

/// HTTP push delivery request body from an OJS server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushDeliveryRequest {
    /// The job to process.
    pub job: JobEvent,

    /// Identifier of the push worker registration.
    #[serde(default)]
    pub worker_id: String,

    /// Unique delivery identifier for idempotency.
    #[serde(default)]
    pub delivery_id: String,
}

/// HTTP push delivery response body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushDeliveryResponse {
    /// Processing result: `"completed"` or `"failed"`.
    pub status: String,

    /// Result data from successful processing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,

    /// Error information if processing failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<PushError>,
}

/// Describes a job processing failure in push delivery responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushError {
    /// Machine-readable error code.
    pub code: String,

    /// Human-readable error description.
    pub message: String,

    /// Whether the job should be retried.
    pub retryable: bool,
}

/// Response from direct Lambda invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectResponse {
    /// Processing result: `"completed"` or `"failed"`.
    pub status: String,

    /// The job ID that was processed.
    pub job_id: String,

    /// Error message if processing failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// Handler types
// ---------------------------------------------------------------------------

/// A boxed future returned by serverless job handlers.
type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A serverless handler function that processes an OJS job event.
type HandlerFn = Arc<
    dyn Fn(HandlerContext, JobEvent) -> BoxFuture<'static, Result<(), ServerlessError>>
        + Send
        + Sync,
>;

/// Context provided to serverless job handlers.
///
/// Contains metadata about the invocation environment that may be useful
/// for the handler.
#[derive(Debug, Clone)]
pub struct HandlerContext {
    /// The OJS server URL, if configured.
    pub ojs_url: Option<String>,
}

/// Error type for serverless handler failures.
#[derive(Debug, thiserror::Error)]
pub enum ServerlessError {
    /// A retryable handler error. The job will be returned to the queue.
    #[error("handler error: {0}")]
    Handler(String),

    /// A non-retryable error. The job will not be retried.
    #[error("non-retryable error: {0}")]
    NonRetryable(String),

    /// JSON deserialization failed.
    #[error("deserialization error: {0}")]
    Deserialization(String),

    /// No handler registered for the job type.
    #[error("no handler registered for job type: {0}")]
    NoHandler(String),
}

// ---------------------------------------------------------------------------
// LambdaHandler
// ---------------------------------------------------------------------------

/// Processes OJS jobs delivered to AWS Lambda via SQS, HTTP push, or direct invocation.
///
/// Register handlers for job types, then wire one of the `handle_*` methods
/// as the Lambda entry point.
///
/// # Thread Safety
///
/// `LambdaHandler` is `Send + Sync` and can be shared across async tasks.
/// Handler registration after construction uses interior mutability via `RwLock`.
///
/// # Example
///
/// ```rust,ignore
/// use ojs::serverless::{LambdaHandler, JobEvent};
///
/// let mut handler = LambdaHandler::new();
///
/// handler.register("email.send", |_ctx, job: JobEvent| async move {
///     println!("Processing {}: {:?}", job.id, job.args);
///     Ok(())
/// });
/// ```
pub struct LambdaHandler {
    handlers: Arc<RwLock<HashMap<String, HandlerFn>>>,
    ojs_url: Option<String>,
}

impl LambdaHandler {
    /// Create a new `LambdaHandler` with no registered handlers.
    pub fn new() -> Self {
        Self {
            handlers: Arc::new(RwLock::new(HashMap::new())),
            ojs_url: None,
        }
    }

    /// Create a new `LambdaHandler` with the given OJS server URL.
    ///
    /// The URL is made available to handlers via [`HandlerContext::ojs_url`]
    /// for optional callback operations.
    pub fn with_ojs_url(url: impl Into<String>) -> Self {
        Self {
            handlers: Arc::new(RwLock::new(HashMap::new())),
            ojs_url: Some(url.into()),
        }
    }

    /// Register a handler for a specific job type.
    ///
    /// The handler receives a [`HandlerContext`] and the deserialized
    /// [`JobEvent`], and must return `Ok(())` on success or a
    /// [`ServerlessError`] on failure.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// # use ojs::serverless::{LambdaHandler, JobEvent};
    /// let mut handler = LambdaHandler::new();
    ///
    /// handler.register("email.send", |ctx, job: JobEvent| async move {
    ///     println!("Processing job {} from queue {}", job.id, job.queue);
    ///     Ok(())
    /// });
    /// ```
    pub fn register<F, Fut>(&mut self, job_type: impl Into<String>, handler: F)
    where
        F: Fn(HandlerContext, JobEvent) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), ServerlessError>> + Send + 'static,
    {
        let handler: HandlerFn = Arc::new(move |ctx, event| Box::pin(handler(ctx, event)));
        // Use try_write for synchronous registration during setup.
        // This will not block because registration happens before the handler is shared.
        if let Ok(mut handlers) = self.handlers.try_write() {
            handlers.insert(job_type.into(), handler);
        } else {
            // Fallback: spawn a blocking task (should not happen in practice)
            let handlers = self.handlers.clone();
            let job_type = job_type.into();
            tokio::task::block_in_place(move || {
                let rt = tokio::runtime::Handle::current();
                rt.block_on(async {
                    handlers.write().await.insert(job_type, handler);
                });
            });
        }
    }

    /// Register a handler asynchronously.
    ///
    /// Use this when registering handlers after the handler has been shared
    /// (e.g., inside an async context).
    pub async fn register_async<F, Fut>(&self, job_type: impl Into<String>, handler: F)
    where
        F: Fn(HandlerContext, JobEvent) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), ServerlessError>> + Send + 'static,
    {
        let handler: HandlerFn = Arc::new(move |ctx, event| Box::pin(handler(ctx, event)));
        self.handlers.write().await.insert(job_type.into(), handler);
    }

    // ------------------------------------------------------------------
    // SQS Event Source Mapping
    // ------------------------------------------------------------------

    /// Process an SQS event containing OJS jobs.
    ///
    /// Iterates over all SQS records, deserializes each message body as an
    /// OJS job, and dispatches to the registered handler. Returns a
    /// [`SqsBatchResponse`] with partial batch failures so SQS only retries
    /// failed messages.
    ///
    /// This method never returns an `Err` -- individual job failures are
    /// captured in the batch response. Only use this with SQS event source
    /// mappings that have `ReportBatchItemFailures` enabled.
    pub async fn handle_sqs(
        &self,
        event: SqsEvent,
    ) -> Result<SqsBatchResponse, ServerlessError> {
        let mut failures = Vec::new();

        for record in &event.records {
            // Deserialize the job from the SQS message body
            let job: JobEvent = match serde_json::from_str(&record.body) {
                Ok(job) => job,
                Err(e) => {
                    tracing::error!(
                        message_id = %record.message_id,
                        error = %e,
                        "failed to deserialize SQS message body"
                    );
                    failures.push(BatchItemFailure {
                        item_identifier: record.message_id.clone(),
                    });
                    continue;
                }
            };

            // Process the job
            match self.process_job(job.clone()).await {
                Ok(()) => {
                    tracing::info!(
                        job_id = %job.id,
                        job_type = %job.job_type,
                        "job completed"
                    );
                }
                Err(e) => {
                    tracing::error!(
                        job_id = %job.id,
                        job_type = %job.job_type,
                        error = %e,
                        "job processing failed"
                    );
                    failures.push(BatchItemFailure {
                        item_identifier: record.message_id.clone(),
                    });
                }
            }
        }

        Ok(SqsBatchResponse {
            batch_item_failures: failures,
        })
    }

    // ------------------------------------------------------------------
    // HTTP Push Delivery
    // ------------------------------------------------------------------

    /// Process an HTTP push delivery request.
    ///
    /// Parses the push delivery envelope, dispatches the job to the
    /// registered handler, and returns an OJS-compatible
    /// [`PushDeliveryResponse`].
    pub async fn handle_http(&self, request: PushDeliveryRequest) -> PushDeliveryResponse {
        match self.process_job(request.job.clone()).await {
            Ok(()) => PushDeliveryResponse {
                status: "completed".to_string(),
                result: None,
                error: None,
            },
            Err(e) => {
                let retryable = !matches!(e, ServerlessError::NonRetryable(_));
                PushDeliveryResponse {
                    status: "failed".to_string(),
                    result: None,
                    error: Some(PushError {
                        code: "handler_error".to_string(),
                        message: e.to_string(),
                        retryable,
                    }),
                }
            }
        }
    }

    /// Process a raw HTTP push delivery request from a JSON body string.
    ///
    /// This is a convenience method for when you receive the raw request
    /// body as a string (e.g., from a Lambda Function URL event).
    pub async fn handle_http_raw(
        &self,
        body: &str,
    ) -> Result<PushDeliveryResponse, ServerlessError> {
        let request: PushDeliveryRequest =
            serde_json::from_str(body).map_err(|e| ServerlessError::Deserialization(e.to_string()))?;
        Ok(self.handle_http(request).await)
    }

    // ------------------------------------------------------------------
    // Direct Invocation
    // ------------------------------------------------------------------

    /// Process a single job from a direct Lambda invocation.
    ///
    /// The event is the OJS job payload itself. Returns a [`DirectResponse`]
    /// indicating success or failure.
    pub async fn handle_direct(&self, event: JobEvent) -> DirectResponse {
        let job_id = event.id.clone();

        match self.process_job(event).await {
            Ok(()) => {
                tracing::info!(job_id = %job_id, "job completed");
                DirectResponse {
                    status: "completed".to_string(),
                    job_id,
                    error: None,
                }
            }
            Err(e) => {
                tracing::error!(job_id = %job_id, error = %e, "job processing failed");
                DirectResponse {
                    status: "failed".to_string(),
                    job_id,
                    error: Some(e.to_string()),
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Internal
    // ------------------------------------------------------------------

    async fn process_job(&self, job: JobEvent) -> Result<(), ServerlessError> {
        let handlers = self.handlers.read().await;
        let handler = handlers
            .get(&job.job_type)
            .ok_or_else(|| ServerlessError::NoHandler(job.job_type.clone()))?
            .clone();

        // Drop the read lock before calling the handler
        drop(handlers);

        let ctx = HandlerContext {
            ojs_url: self.ojs_url.clone(),
        };

        handler(ctx, job).await
    }
}

impl Default for LambdaHandler {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_job_event(id: &str, job_type: &str) -> JobEvent {
        JobEvent {
            id: id.to_string(),
            job_type: job_type.to_string(),
            queue: "default".to_string(),
            args: json!([{"to": "user@example.com"}]),
            attempt: 1,
            meta: None,
            priority: 0,
        }
    }

    #[tokio::test]
    async fn test_handle_sqs_success() {
        let mut handler = LambdaHandler::new();
        handler.register("email.send", |_ctx, _job: JobEvent| async move { Ok(()) });

        let event = SqsEvent {
            records: vec![
                SqsMessage {
                    message_id: "msg-1".to_string(),
                    body: serde_json::to_string(&make_job_event("job-1", "email.send")).unwrap(),
                    attributes: HashMap::new(),
                    receipt_handle: String::new(),
                },
                SqsMessage {
                    message_id: "msg-2".to_string(),
                    body: serde_json::to_string(&make_job_event("job-2", "email.send")).unwrap(),
                    attributes: HashMap::new(),
                    receipt_handle: String::new(),
                },
            ],
        };

        let resp = handler.handle_sqs(event).await.unwrap();
        assert!(resp.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn test_handle_sqs_partial_failure() {
        let mut handler = LambdaHandler::new();
        handler.register("email.send", |_ctx, _job: JobEvent| async move { Ok(()) });
        // No handler for "unknown.type"

        let event = SqsEvent {
            records: vec![
                SqsMessage {
                    message_id: "msg-1".to_string(),
                    body: serde_json::to_string(&make_job_event("job-1", "email.send")).unwrap(),
                    attributes: HashMap::new(),
                    receipt_handle: String::new(),
                },
                SqsMessage {
                    message_id: "msg-2".to_string(),
                    body: serde_json::to_string(&make_job_event("job-2", "unknown.type")).unwrap(),
                    attributes: HashMap::new(),
                    receipt_handle: String::new(),
                },
            ],
        };

        let resp = handler.handle_sqs(event).await.unwrap();
        assert_eq!(resp.batch_item_failures.len(), 1);
        assert_eq!(resp.batch_item_failures[0].item_identifier, "msg-2");
    }

    #[tokio::test]
    async fn test_handle_sqs_invalid_json() {
        let handler = LambdaHandler::new();

        let event = SqsEvent {
            records: vec![SqsMessage {
                message_id: "msg-1".to_string(),
                body: "{invalid json".to_string(),
                attributes: HashMap::new(),
                receipt_handle: String::new(),
            }],
        };

        let resp = handler.handle_sqs(event).await.unwrap();
        assert_eq!(resp.batch_item_failures.len(), 1);
    }

    #[tokio::test]
    async fn test_handle_direct_success() {
        let mut handler = LambdaHandler::new();
        handler.register("email.send", |_ctx, _job: JobEvent| async move { Ok(()) });

        let job = make_job_event("job-1", "email.send");
        let resp = handler.handle_direct(job).await;

        assert_eq!(resp.status, "completed");
        assert_eq!(resp.job_id, "job-1");
        assert!(resp.error.is_none());
    }

    #[tokio::test]
    async fn test_handle_direct_no_handler() {
        let handler = LambdaHandler::new();

        let job = make_job_event("job-1", "unknown.type");
        let resp = handler.handle_direct(job).await;

        assert_eq!(resp.status, "failed");
        assert_eq!(resp.job_id, "job-1");
        assert!(resp.error.is_some());
    }

    #[tokio::test]
    async fn test_handle_http_success() {
        let mut handler = LambdaHandler::new();
        handler.register("email.send", |_ctx, _job: JobEvent| async move { Ok(()) });

        let req = PushDeliveryRequest {
            job: make_job_event("job-1", "email.send"),
            worker_id: "w1".to_string(),
            delivery_id: "d1".to_string(),
        };

        let resp = handler.handle_http(req).await;
        assert_eq!(resp.status, "completed");
        assert!(resp.error.is_none());
    }

    #[tokio::test]
    async fn test_handle_http_handler_error() {
        let mut handler = LambdaHandler::new();
        handler.register("email.send", |_ctx, _job: JobEvent| async move {
            Err(ServerlessError::Handler("SMTP failure".to_string()))
        });

        let req = PushDeliveryRequest {
            job: make_job_event("job-1", "email.send"),
            worker_id: "w1".to_string(),
            delivery_id: "d1".to_string(),
        };

        let resp = handler.handle_http(req).await;
        assert_eq!(resp.status, "failed");
        assert!(resp.error.is_some());
        let err = resp.error.unwrap();
        assert!(err.retryable);
        assert!(err.message.contains("SMTP failure"));
    }

    #[tokio::test]
    async fn test_handle_http_non_retryable() {
        let mut handler = LambdaHandler::new();
        handler.register("email.send", |_ctx, _job: JobEvent| async move {
            Err(ServerlessError::NonRetryable("invalid recipient".to_string()))
        });

        let req = PushDeliveryRequest {
            job: make_job_event("job-1", "email.send"),
            worker_id: "w1".to_string(),
            delivery_id: "d1".to_string(),
        };

        let resp = handler.handle_http(req).await;
        assert_eq!(resp.status, "failed");
        let err = resp.error.unwrap();
        assert!(!err.retryable);
    }

    #[tokio::test]
    async fn test_job_event_deserialization() {
        let raw = r#"{"id":"j1","type":"test","queue":"q","args":[{"key":"value"}],"attempt":1,"priority":5}"#;
        let job: JobEvent = serde_json::from_str(raw).unwrap();

        assert_eq!(job.id, "j1");
        assert_eq!(job.job_type, "test");
        assert_eq!(job.queue, "q");
        assert_eq!(job.priority, 5);
        assert_eq!(job.attempt, 1);
    }

    #[tokio::test]
    async fn test_job_event_defaults() {
        let raw = r#"{"id":"j1","type":"test"}"#;
        let job: JobEvent = serde_json::from_str(raw).unwrap();

        assert_eq!(job.queue, "default");
        assert_eq!(job.attempt, 1);
        assert_eq!(job.priority, 0);
        assert_eq!(job.args, json!([]));
    }

    #[tokio::test]
    async fn test_handle_http_raw() {
        let mut handler = LambdaHandler::new();
        handler.register("email.send", |_ctx, _job: JobEvent| async move { Ok(()) });

        let body = r#"{"job":{"id":"j1","type":"email.send","queue":"default","args":[],"attempt":1},"worker_id":"w1","delivery_id":"d1"}"#;
        let resp = handler.handle_http_raw(body).await.unwrap();
        assert_eq!(resp.status, "completed");
    }

    #[tokio::test]
    async fn test_with_ojs_url() {
        let mut handler = LambdaHandler::with_ojs_url("https://ojs.example.com");
        handler.register("test", |ctx, _job: JobEvent| async move {
            assert_eq!(ctx.ojs_url.as_deref(), Some("https://ojs.example.com"));
            Ok(())
        });

        let job = make_job_event("j1", "test");
        let resp = handler.handle_direct(job).await;
        assert_eq!(resp.status, "completed");
    }
}
