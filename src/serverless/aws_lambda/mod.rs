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
//!
//! # Example: Identity-aware HTTP Push Delivery
//!
//! ```rust,ignore
//! use ojs::serverless::{LambdaHandler, PushAuthConfig, PushContext};
//!
//! let mut handler = LambdaHandler::new()
//!     .try_with_push_auth(
//!         PushAuthConfig::new()
//!             .try_with_signing_secret_from_env("OJS_PUSH_SIGNING_SECRET")?
//!     )?;
//!
//! handler.register_with_context("email.send", |ctx: PushContext| async move {
//!     println!(
//!         "job={} worker={:?} delivery={:?}",
//!         ctx.job().id,
//!         ctx.worker_id(),
//!         ctx.delivery_id(),
//!     );
//!     Ok(())
//! });
//! ```
//!
//! # Module organization
//!
//! This adapter is split into cohesive actors, each in its own submodule:
//!
//! - `events` — event/body wire types ([`JobEvent`] and the SQS/HTTP-push/
//!   direct-invocation envelope shapes). Pure data; no dispatch logic.
//! - `push_auth` — push-auth configuration and constant-time HMAC-SHA256
//!   verification ([`PushAuthConfig`] and a crate-private verification
//!   function used by the authenticated `handle_http_*` entry points).
//!
//! [`LambdaHandler`] itself (handler registration and SQS/HTTP-push/direct
//! dispatch) remains here, as the cohesive "registry and dispatch" actor
//! that owns and drives both of the above.

mod events;
mod push_auth;

pub use events::{
    BatchItemFailure, DirectResponse, JobEvent, PushDeliveryRequest, PushDeliveryResponse,
    PushError, SqsBatchResponse, SqsEvent, SqsMessage,
};
pub use push_auth::{
    PushAuthConfig, DEFAULT_PUSH_FRESHNESS_WINDOW, MIN_PUSH_SIGNING_SECRET_BYTES,
    PUSH_DELIVERY_ID_HEADER, PUSH_JOB_ID_HEADER, PUSH_SIGNATURE_HEADER, PUSH_TIMESTAMP_HEADER,
};

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

// ---------------------------------------------------------------------------
// Handler types
// ---------------------------------------------------------------------------

/// A boxed future returned by serverless job handlers.
type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A serverless handler function that processes an OJS job event.
type HandlerFn =
    Arc<dyn Fn(HandlerInvocation) -> BoxFuture<'static, Result<(), ServerlessError>> + Send + Sync>;

/// Context provided to serverless job handlers.
///
/// Contains metadata about the invocation environment that may be useful
/// for the handler.
#[derive(Debug, Clone)]
pub struct HandlerContext {
    /// The OJS server URL, if configured.
    pub ojs_url: Option<String>,
}

/// Extended per-delivery context exposed by
/// [`LambdaHandler::register_with_context`].
///
/// Unlike [`HandlerContext`], this context also carries the current job and
/// any delivery identity supplied by the transport. HTTP push delivery
/// populates `worker_id` and `delivery_id`; SQS and direct invocation leave
/// them absent.
#[derive(Debug, Clone)]
pub struct PushContext {
    ojs_url: Option<String>,
    job: JobEvent,
    worker_id: Option<String>,
    delivery_id: Option<String>,
}

impl PushContext {
    /// The OJS server URL, if configured.
    pub fn ojs_url(&self) -> Option<&str> {
        self.ojs_url.as_deref()
    }

    /// The job being processed.
    pub fn job(&self) -> &JobEvent {
        &self.job
    }

    /// The push worker identity, when the current invocation came from HTTP
    /// push delivery and the backend supplied one.
    pub fn worker_id(&self) -> Option<&str> {
        self.worker_id.as_deref()
    }

    /// The delivery-attempt identity, when the current invocation came from
    /// HTTP push delivery and the backend supplied one.
    pub fn delivery_id(&self) -> Option<&str> {
        self.delivery_id.as_deref()
    }
}

#[derive(Debug, Clone)]
struct HandlerInvocation {
    ojs_url: Option<String>,
    job: JobEvent,
    worker_id: Option<String>,
    delivery_id: Option<String>,
}

impl HandlerInvocation {
    fn direct(ojs_url: Option<String>, job: JobEvent) -> Self {
        Self {
            ojs_url,
            job,
            worker_id: None,
            delivery_id: None,
        }
    }

    fn push(ojs_url: Option<String>, request: &PushDeliveryRequest) -> Self {
        Self {
            ojs_url,
            job: request.job.clone(),
            worker_id: nonempty_field(&request.worker_id),
            delivery_id: nonempty_field(&request.delivery_id),
        }
    }

    fn handler_context(&self) -> HandlerContext {
        HandlerContext {
            ojs_url: self.ojs_url.clone(),
        }
    }

    fn push_context(&self) -> PushContext {
        PushContext {
            ojs_url: self.ojs_url.clone(),
            job: self.job.clone(),
            worker_id: self.worker_id.clone(),
            delivery_id: self.delivery_id.clone(),
        }
    }
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

const DEFAULT_DELIVERY_ID_STORE_CAPACITY: usize = 4_096;
const EXPIRED_ENTRY_CLEANUP_LIMIT: usize = 64;

#[derive(Debug)]
struct DeliveryIdEntry {
    delivery_id: String,
    expires_at: std::time::Instant,
}

#[derive(Debug)]
struct InMemoryDeliveryIdState {
    entries: HashMap<String, std::time::Instant>,
    order: VecDeque<DeliveryIdEntry>,
    max_entries: usize,
}

impl InMemoryDeliveryIdState {
    fn new(max_entries: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            max_entries,
        }
    }

    fn check_and_insert(
        &mut self,
        delivery_id: &str,
        now: std::time::Instant,
        ttl: Duration,
    ) -> Result<DeliveryIdCheck, ServerlessError> {
        if delivery_id.trim().is_empty() {
            return Err(ServerlessError::NonRetryable(
                "delivery ID store requires a non-empty delivery_id".into(),
            ));
        }
        if ttl.is_zero() {
            return Err(ServerlessError::NonRetryable(
                "delivery ID store requires a TTL greater than zero".into(),
            ));
        }

        self.purge_expired(now);

        if let Some(expires_at) = self.entries.get(delivery_id).copied() {
            if expires_at > now {
                return Ok(DeliveryIdCheck::Duplicate);
            }
            self.entries.remove(delivery_id);
        }

        if self.entries.len() >= self.max_entries {
            return Err(ServerlessError::Handler(
                "delivery replay protection capacity is exhausted; retry the delivery later".into(),
            ));
        }

        let expires_at = now.checked_add(ttl).ok_or_else(|| {
            ServerlessError::Handler(
                "delivery replay protection TTL exceeds the in-memory clock range; retry the delivery later"
                    .into(),
            )
        })?;
        let delivery_id = delivery_id.to_string();
        self.entries.insert(delivery_id.clone(), expires_at);
        self.order.push_back(DeliveryIdEntry {
            delivery_id,
            expires_at,
        });
        Ok(DeliveryIdCheck::Inserted)
    }

    fn purge_expired(&mut self, now: std::time::Instant) {
        let entries_to_scan = self.order.len().min(EXPIRED_ENTRY_CLEANUP_LIMIT);
        for _ in 0..entries_to_scan {
            let Some(entry) = self.order.pop_front() else {
                break;
            };
            if self.entries.get(&entry.delivery_id).copied() != Some(entry.expires_at) {
                continue;
            }
            if entry.expires_at <= now {
                self.entries.remove(&entry.delivery_id);
            } else {
                self.order.push_back(entry);
            }
        }
    }
}

/// Result of an atomic delivery-ID check-and-insert operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryIdCheck {
    /// The ID was absent (or expired) and has now been stored for the TTL.
    Inserted,
    /// The ID was already present and unexpired.
    Duplicate,
}

/// Async replay-protection storage for authenticated push delivery IDs.
///
/// Implementations must make `check_and_insert` atomic across all concurrent
/// callers that share the store: exactly one caller may receive
/// [`DeliveryIdCheck::Inserted`] for a delivery ID during its TTL; every
/// other caller must receive [`DeliveryIdCheck::Duplicate`].
///
/// Use an external implementation backed by DynamoDB, Redis, or another
/// shared conditional-write store in production when multiple Lambda
/// execution environments may receive the same delivery. The SDK's default
/// in-memory implementation is shared only within one process and cannot
/// guarantee cross-process deduplication.
pub trait DeliveryIdStore: Send + Sync {
    /// Atomically check for an unexpired delivery ID and insert it with `ttl`
    /// when absent.
    fn check_and_insert<'a>(
        &'a self,
        delivery_id: &'a str,
        ttl: Duration,
    ) -> Pin<Box<dyn Future<Output = Result<DeliveryIdCheck, ServerlessError>> + Send + 'a>>;
}

/// Bounded in-memory [`DeliveryIdStore`].
///
/// Unexpired entries are never evicted. Once capacity is full, a new ID
/// returns a retryable [`ServerlessError::Handler`] instead of being admitted
/// without replay protection. Expired-entry cleanup scans a bounded number of
/// entries per operation.
#[derive(Debug)]
pub struct InMemoryDeliveryIdStore {
    state: std::sync::Mutex<InMemoryDeliveryIdState>,
}

impl InMemoryDeliveryIdStore {
    /// Create an isolated in-memory store with `max_entries` capacity.
    pub fn new(max_entries: usize) -> Self {
        Self {
            state: std::sync::Mutex::new(InMemoryDeliveryIdState::new(max_entries)),
        }
    }
}

impl DeliveryIdStore for InMemoryDeliveryIdStore {
    fn check_and_insert<'a>(
        &'a self,
        delivery_id: &'a str,
        ttl: Duration,
    ) -> Pin<Box<dyn Future<Output = Result<DeliveryIdCheck, ServerlessError>> + Send + 'a>> {
        Box::pin(async move {
            self.state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .check_and_insert(delivery_id, std::time::Instant::now(), ttl)
        })
    }
}

fn default_delivery_id_store() -> Arc<dyn DeliveryIdStore> {
    static STORE: OnceLock<Arc<dyn DeliveryIdStore>> = OnceLock::new();
    STORE
        .get_or_init(|| {
            Arc::new(InMemoryDeliveryIdStore::new(
                DEFAULT_DELIVERY_ID_STORE_CAPACITY,
            ))
        })
        .clone()
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
    push_auth: Option<PushAuthConfig>,
    delivery_id_store: Arc<dyn DeliveryIdStore>,
}

impl LambdaHandler {
    /// Create a new `LambdaHandler` with no registered handlers.
    pub fn new() -> Self {
        Self {
            handlers: Arc::new(RwLock::new(HashMap::new())),
            ojs_url: None,
            push_auth: None,
            delivery_id_store: default_delivery_id_store(),
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
            push_auth: None,
            delivery_id_store: default_delivery_id_store(),
        }
    }

    /// Configure HTTP push delivery authentication, enabling
    /// [`handle_http_authenticated`](Self::handle_http_authenticated) and
    /// [`handle_http_raw_authenticated`](Self::handle_http_raw_authenticated).
    ///
    /// `handle_http`/`handle_http_raw` remain available and unauthenticated
    /// for callers who authenticate push delivery upstream (e.g. an API
    /// Gateway authorizer); this is required only for the new
    /// authenticated entry points.
    pub fn with_push_auth(mut self, config: PushAuthConfig) -> Self {
        self.push_auth = Some(config);
        self
    }

    /// Validate and configure HTTP push delivery authentication.
    ///
    /// Prefer this over [`with_push_auth`](Self::with_push_auth) for new code
    /// so missing, empty, short, or mixed-invalid rotation secrets fail
    /// during initialization rather than on the first delivery.
    pub fn try_with_push_auth(mut self, config: PushAuthConfig) -> Result<Self, ServerlessError> {
        config.validate()?;
        self.push_auth = Some(config);
        Ok(self)
    }

    /// Configure replay-protection storage for authenticated HTTP push
    /// delivery.
    ///
    /// The default is one process-shared [`InMemoryDeliveryIdStore`] used by
    /// all `LambdaHandler` instances in the current execution environment.
    /// For cross-environment deduplication, provide a DynamoDB/Redis-backed
    /// implementation with an atomic conditional insert and TTL.
    pub fn with_delivery_id_store(mut self, store: Arc<dyn DeliveryIdStore>) -> Self {
        self.delivery_id_store = store;
        self
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
    ///
    /// Fully synchronous: registration only ever needs a brief
    /// `std::sync::RwLock` write (never held across an `.await`), so there
    /// is no blocking-runtime fallback to reason about. An earlier version
    /// used a `tokio::sync::RwLock` here and fell back to
    /// `tokio::task::block_in_place` when a synchronous `try_write` lost a
    /// race -- `block_in_place` panics outright on a current-thread
    /// runtime, which a Lambda deployment may well use.
    pub fn register<F, Fut>(&mut self, job_type: impl Into<String>, handler: F)
    where
        F: Fn(HandlerContext, JobEvent) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), ServerlessError>> + Send + 'static,
    {
        let handler: HandlerFn = Arc::new(move |invocation| {
            Box::pin(handler(
                invocation.handler_context(),
                invocation.job.clone(),
            ))
        });
        self.insert_handler(job_type.into(), handler);
    }

    /// Register a handler asynchronously.
    ///
    /// Equivalent to [`register`](Self::register); kept as an `async fn`
    /// for API compatibility with call sites that `.await` it (e.g. after
    /// the handler has already been shared via `Arc` inside an async
    /// context). The lock itself is still synchronous and brief.
    #[allow(clippy::unused_async)]
    pub async fn register_async<F, Fut>(&self, job_type: impl Into<String>, handler: F)
    where
        F: Fn(HandlerContext, JobEvent) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), ServerlessError>> + Send + 'static,
    {
        let handler: HandlerFn = Arc::new(move |invocation| {
            Box::pin(handler(
                invocation.handler_context(),
                invocation.job.clone(),
            ))
        });
        self.insert_handler(job_type.into(), handler);
    }

    /// Register a handler that receives the full per-delivery context,
    /// including `job`, `worker_id`, and `delivery_id` when available.
    pub fn register_with_context<F, Fut>(&mut self, job_type: impl Into<String>, handler: F)
    where
        F: Fn(PushContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), ServerlessError>> + Send + 'static,
    {
        let handler: HandlerFn =
            Arc::new(move |invocation| Box::pin(handler(invocation.push_context())));
        self.insert_handler(job_type.into(), handler);
    }

    /// Async compatibility wrapper around
    /// [`register_with_context`](Self::register_with_context).
    #[allow(clippy::unused_async)]
    pub async fn register_with_context_async<F, Fut>(&self, job_type: impl Into<String>, handler: F)
    where
        F: Fn(PushContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), ServerlessError>> + Send + 'static,
    {
        let handler: HandlerFn =
            Arc::new(move |invocation| Box::pin(handler(invocation.push_context())));
        self.insert_handler(job_type.into(), handler);
    }

    fn insert_handler(&self, job_type: String, handler: HandlerFn) {
        let mut handlers = self
            .handlers
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        handlers.insert(job_type, handler);
    }

    fn validate_authenticated_push_request(
        headers: &HashMap<String, String>,
        request: &PushDeliveryRequest,
    ) -> Result<(), ServerlessError> {
        let delivery_id = request.delivery_id.trim();
        if delivery_id.is_empty() {
            return Err(ServerlessError::NonRetryable(
                "invalid push delivery: authenticated push requires a non-empty delivery_id".into(),
            ));
        }

        validate_optional_header_match(
            headers,
            PUSH_DELIVERY_ID_HEADER,
            delivery_id,
            "delivery_id",
        )?;
        validate_optional_header_match(headers, PUSH_JOB_ID_HEADER, request.job.id.trim(), "job.id")
    }

    async fn remember_authenticated_delivery(
        &self,
        delivery_id: &str,
        ttl: Duration,
    ) -> Result<(), ServerlessError> {
        match self
            .delivery_id_store
            .check_and_insert(delivery_id, ttl)
            .await?
        {
            DeliveryIdCheck::Inserted => Ok(()),
            DeliveryIdCheck::Duplicate => Err(ServerlessError::NonRetryable(
                "invalid push delivery: duplicate delivery_id replayed within the freshness window"
                    .into(),
            )),
        }
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
    pub async fn handle_sqs(&self, event: SqsEvent) -> Result<SqsBatchResponse, ServerlessError> {
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
            match self
                .process_job(HandlerInvocation::direct(self.ojs_url.clone(), job.clone()))
                .await
            {
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
        match self
            .process_job(HandlerInvocation::push(self.ojs_url.clone(), &request))
            .await
        {
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
        let request: PushDeliveryRequest = serde_json::from_str(body)
            .map_err(|e| ServerlessError::Deserialization(e.to_string()))?;
        Ok(self.handle_http(request).await)
    }

    /// Process an HTTP push delivery request, first authenticating it via
    /// [`PushAuthConfig`] (configured with [`with_push_auth`](Self::with_push_auth)).
    ///
    /// Verifies the `X-OJS-Timestamp`/`X-OJS-Signature` headers (constant-time
    /// HMAC-SHA256, bounded freshness window) against the raw request body
    /// **before** decoding or dispatching it to any handler. Use this (or
    /// [`handle_http_raw_authenticated`](Self::handle_http_raw_authenticated))
    /// instead of [`handle_http`](Self::handle_http)/[`handle_http_raw`](Self::handle_http_raw)
    /// for any endpoint reachable directly from the public internet (e.g. a
    /// Lambda Function URL) that isn't already authenticated upstream.
    ///
    /// `headers` should contain the request's HTTP headers (lookup is
    /// case-insensitive); if a header appears multiple times, callers
    /// should join the values with commas first (matching how
    /// `X-OJS-Signature` itself supports multiple comma-separated
    /// signatures for secret rotation). Forwarding `X-OJS-Delivery-ID` and
    /// `X-OJS-Job-ID` is recommended: when present, they are validated
    /// against the signed body before dispatch.
    ///
    /// # Errors
    ///
    /// Returns [`ServerlessError::NonRetryable`] if authentication was not
    /// configured, the timestamp is missing/malformed/stale, the body's
    /// `delivery_id` is empty, a forwarded delivery/job header mismatches
    /// the body, the same authenticated `delivery_id` is replayed within
    /// the freshness window, or no configured secret's signature matches --
    /// in every case, `raw_body` is never dispatched to a handler more than
    /// once.
    pub async fn handle_http_authenticated(
        &self,
        headers: &HashMap<String, String>,
        raw_body: &[u8],
    ) -> Result<PushDeliveryResponse, ServerlessError> {
        let config = self.push_auth.as_ref().ok_or_else(|| {
            ServerlessError::NonRetryable(
                "push authentication is not configured: call `.with_push_auth(...)`                  before using an authenticated entry point"
                    .into(),
            )
        })?;

        let timestamp_header = push_auth::find_header(headers, PUSH_TIMESTAMP_HEADER);
        let signature_headers: Vec<&str> = push_auth::find_header(headers, PUSH_SIGNATURE_HEADER)
            .into_iter()
            .collect();
        let now = push_auth::unix_timestamp_now();

        let replay_ttl = push_auth::authenticate_push(
            config,
            timestamp_header,
            &signature_headers,
            raw_body,
            now,
        )?;

        let request: PushDeliveryRequest = serde_json::from_slice(raw_body)
            .map_err(|e| ServerlessError::Deserialization(e.to_string()))?;
        Self::validate_authenticated_push_request(headers, &request)?;
        self.remember_authenticated_delivery(request.delivery_id.trim(), replay_ttl)
            .await?;
        Ok(self.handle_http(request).await)
    }

    /// String-body convenience wrapper around
    /// [`handle_http_authenticated`](Self::handle_http_authenticated) for
    /// callers that receive the raw body as a `String` (e.g. from a Lambda
    /// Function URL event).
    pub async fn handle_http_raw_authenticated(
        &self,
        headers: &HashMap<String, String>,
        raw_body: &str,
    ) -> Result<PushDeliveryResponse, ServerlessError> {
        self.handle_http_authenticated(headers, raw_body.as_bytes())
            .await
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

        match self
            .process_job(HandlerInvocation::direct(self.ojs_url.clone(), event))
            .await
        {
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

    async fn process_job(&self, invocation: HandlerInvocation) -> Result<(), ServerlessError> {
        // Scoped so the read guard is provably dropped before the
        // `.await` below, never held across it.
        let job_type = invocation.job.job_type.clone();
        let handler = {
            let handlers = self
                .handlers
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            handlers
                .get(&job_type)
                .ok_or_else(|| ServerlessError::NoHandler(job_type.clone()))?
                .clone()
        };

        handler(invocation).await
    }
}

fn nonempty_field(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn validate_optional_header_match(
    headers: &HashMap<String, String>,
    header_name: &str,
    expected: &str,
    field_name: &str,
) -> Result<(), ServerlessError> {
    let Some(value) = push_auth::find_header(headers, header_name) else {
        return Ok(());
    };
    if value.trim().is_empty() || value != expected {
        return Err(ServerlessError::NonRetryable(format!(
            "invalid push delivery: {header_name} does not match body {field_name}"
        )));
    }
    Ok(())
}

impl Default for LambdaHandler {
    fn default() -> Self {
        Self::new()
    }
}

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
            Err(ServerlessError::NonRetryable(
                "invalid recipient".to_string(),
            ))
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
