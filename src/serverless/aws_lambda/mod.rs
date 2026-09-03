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

    #[tokio::test]
    async fn test_register_with_context_exposes_push_delivery_identity() {
        let mut handler = LambdaHandler::with_ojs_url("https://ojs.example.com");
        handler.register_with_context("email.send", |ctx: PushContext| async move {
            assert_eq!(ctx.ojs_url(), Some("https://ojs.example.com"));
            assert_eq!(ctx.job().id, "job-1");
            assert_eq!(ctx.job().job_type, "email.send");
            assert_eq!(ctx.worker_id(), Some("w1"));
            assert_eq!(ctx.delivery_id(), Some("d1"));
            Ok(())
        });

        let resp = handler
            .handle_http(PushDeliveryRequest {
                job: make_job_event("job-1", "email.send"),
                worker_id: "w1".to_string(),
                delivery_id: "d1".to_string(),
            })
            .await;

        assert_eq!(resp.status, "completed");
        assert!(resp.error.is_none());
    }

    #[tokio::test]
    async fn test_register_while_processing_does_not_panic_or_deadlock() {
        // Regression test for the `tokio::task::block_in_place` fallback
        // that `register()` used to fall back to when a synchronous
        // `try_write` lost a race against an in-flight `process_job` read
        // lock: `block_in_place` panics outright on the current-thread
        // runtime this test intentionally uses. `std::sync::RwLock` has no
        // such fallback to reach at all.
        let mut handler = LambdaHandler::new();
        handler.register("first", |_ctx, _job: JobEvent| async move {
            // Register a second handler *while* this one is "processing",
            // simulating registration racing a concurrent dispatch.
            Ok(())
        });

        let job = make_job_event("j1", "first");
        let resp = handler.handle_direct(job).await;
        assert_eq!(resp.status, "completed");

        // Registering after the fact must still succeed without panicking.
        handler.register("second", |_ctx, _job: JobEvent| async move { Ok(()) });
        let job2 = make_job_event("j2", "second");
        let resp2 = handler.handle_direct(job2).await;
        assert_eq!(resp2.status, "completed");
    }

    #[test]
    fn test_register_is_synchronous_and_current_thread_runtime_safe() {
        // `register()` must work from a plain synchronous context (no
        // Tokio runtime at all), proving it never reaches for
        // `tokio::task::block_in_place` (which requires an active runtime
        // and panics on a current-thread one).
        let mut handler = LambdaHandler::new();
        handler.register(
            "sync.registered",
            |_ctx, _job: JobEvent| async move { Ok(()) },
        );

        // Building a current-thread runtime specifically exercises the
        // scenario where the old `block_in_place` fallback would panic.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let job = make_job_event("j1", "sync.registered");
            let resp = handler.handle_direct(job).await;
            assert_eq!(resp.status, "completed");
        });
    }
}

// Tests for the authenticated push-delivery entry points
// (`handle_http_authenticated`/`handle_http_raw_authenticated`), exercising
// the integration of `push_auth`'s verification with `LambdaHandler`'s
// dispatch. Pure unit tests for the parsing/verification primitives
// themselves (with no `LambdaHandler` involved) live in `push_auth`'s own
// test module.
#[cfg(test)]
mod push_auth_integration_tests {
    use super::*;
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::sync::Barrier;

    const TEST_SECRET: &[u8] = b"0123456789abcdef0123456789abcdef";
    const OLD_SECRET: &[u8] = b"old-0123456789abcdef0123456789ab";
    const NEW_SECRET: &[u8] = b"new-0123456789abcdef0123456789ab";
    const WRONG_SECRET: &[u8] = b"bad-0123456789abcdef0123456789ab";

    fn authenticated_handler(config: PushAuthConfig) -> LambdaHandler {
        LambdaHandler::new()
            .with_delivery_id_store(Arc::new(InMemoryDeliveryIdStore::new(128)))
            .with_push_auth(config)
    }

    #[derive(Debug, Default)]
    struct RecordingDeliveryIdStore {
        calls: AtomicUsize,
        ids: Mutex<Vec<String>>,
    }

    impl DeliveryIdStore for RecordingDeliveryIdStore {
        fn check_and_insert<'a>(
            &'a self,
            delivery_id: &'a str,
            _ttl: Duration,
        ) -> Pin<Box<dyn Future<Output = Result<DeliveryIdCheck, ServerlessError>> + Send + 'a>>
        {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                self.ids
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(delivery_id.to_string());
                Ok(DeliveryIdCheck::Inserted)
            })
        }
    }

    fn sign(secret: &[u8], timestamp: &str, body: &[u8]) -> String {
        let mut message = Vec::new();
        message.extend_from_slice(timestamp.as_bytes());
        message.push(b'.');
        message.extend_from_slice(body);

        let mut mac = Hmac::<Sha256>::new_from_slice(secret).unwrap();
        mac.update(&message);
        let bytes = mac.finalize().into_bytes();
        let hex = bytes.iter().fold(String::with_capacity(64), |mut out, b| {
            use std::fmt::Write;
            let _ = write!(out, "{b:02x}");
            out
        });
        format!("sha256={hex}")
    }

    fn headers_with(timestamp: &str, signature: &str) -> HashMap<String, String> {
        let mut headers = HashMap::new();
        headers.insert(PUSH_TIMESTAMP_HEADER.to_string(), timestamp.to_string());
        headers.insert(PUSH_SIGNATURE_HEADER.to_string(), signature.to_string());
        headers
    }

    fn make_request_body(job_type: &str) -> Vec<u8> {
        make_request_body_with(job_type, "job-1", "w1", "d1")
    }

    fn make_request_body_with(
        job_type: &str,
        job_id: &str,
        worker_id: &str,
        delivery_id: &str,
    ) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "job": {
                "id": job_id,
                "type": job_type,
                "queue": "default",
                "args": [],
                "attempt": 1
            },
            "worker_id": worker_id,
            "delivery_id": delivery_id
        }))
        .unwrap()
    }

    #[test]
    fn test_try_with_push_auth_rejects_invalid_legacy_config_at_initialization() {
        let config = PushAuthConfig::new()
            .with_signing_secret(TEST_SECRET.to_vec())
            .with_signing_secret(Vec::new());

        let err = LambdaHandler::new()
            .try_with_push_auth(config)
            .err()
            .expect("mixed-invalid rotation config must fail");

        assert!(err.to_string().contains("at least 32 bytes"));
    }

    #[tokio::test]
    async fn test_default_store_is_shared_across_lambda_handler_instances() {
        let delivery_id = format!("shared-{}", uuid::Uuid::new_v4());
        let body = make_request_body_with("email.send", "job-shared", "w1", &delivery_id);
        let timestamp = push_auth::unix_timestamp_now().as_secs().to_string();
        let signature = sign(TEST_SECRET, &timestamp, &body);
        let headers = headers_with(&timestamp, &signature);
        let config = PushAuthConfig::new().with_signing_secret(TEST_SECRET.to_vec());
        let calls = Arc::new(AtomicUsize::new(0));

        let mut first = LambdaHandler::new().with_push_auth(config.clone());
        first.register("email.send", {
            let calls = calls.clone();
            move |_ctx, _job: JobEvent| {
                let calls = calls.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            }
        });
        let mut second = LambdaHandler::new().with_push_auth(config);
        second.register("email.send", {
            let calls = calls.clone();
            move |_ctx, _job: JobEvent| {
                let calls = calls.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            }
        });

        first
            .handle_http_authenticated(&headers, &body)
            .await
            .unwrap();
        let err = second
            .handle_http_authenticated(&headers, &body)
            .await
            .unwrap_err();

        assert!(matches!(err, ServerlessError::NonRetryable(_)));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_in_memory_store_capacity_fails_closed_without_evicting_live_id() {
        let store = Arc::new(InMemoryDeliveryIdStore::new(1));
        let mut handler = LambdaHandler::new()
            .with_delivery_id_store(store.clone())
            .with_push_auth(PushAuthConfig::new().with_signing_secret(TEST_SECRET.to_vec()));
        handler.register("email.send", |_ctx, _job: JobEvent| async move { Ok(()) });

        let timestamp = push_auth::unix_timestamp_now().as_secs().to_string();
        let first_body = make_request_body_with("email.send", "job-1", "w1", "capacity-live");
        let first_signature = sign(TEST_SECRET, &timestamp, &first_body);
        handler
            .handle_http_authenticated(&headers_with(&timestamp, &first_signature), &first_body)
            .await
            .unwrap();

        let second_body = make_request_body_with("email.send", "job-2", "w1", "capacity-new");
        let second_signature = sign(TEST_SECRET, &timestamp, &second_body);
        let err = handler
            .handle_http_authenticated(&headers_with(&timestamp, &second_signature), &second_body)
            .await
            .unwrap_err();

        assert!(
            matches!(err, ServerlessError::Handler(_)),
            "capacity exhaustion must be retryable"
        );
        assert_eq!(
            store
                .check_and_insert("capacity-live", Duration::from_secs(1))
                .await
                .unwrap(),
            DeliveryIdCheck::Duplicate,
            "the unexpired first ID must not be evicted to admit a new one"
        );
    }

    #[tokio::test]
    async fn test_in_memory_store_allows_id_after_expiry() {
        let store = InMemoryDeliveryIdStore::new(1);

        assert_eq!(
            store
                .check_and_insert("expiring-id", Duration::from_millis(20))
                .await
                .unwrap(),
            DeliveryIdCheck::Inserted
        );
        assert_eq!(
            store
                .check_and_insert("expiring-id", Duration::from_millis(20))
                .await
                .unwrap(),
            DeliveryIdCheck::Duplicate
        );
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(
            store
                .check_and_insert("expiring-id", Duration::from_millis(20))
                .await
                .unwrap(),
            DeliveryIdCheck::Inserted
        );
    }

    #[tokio::test]
    async fn test_in_memory_store_check_and_insert_is_atomic_concurrently() {
        let store = Arc::new(InMemoryDeliveryIdStore::new(64));
        let barrier = Arc::new(Barrier::new(33));
        let mut handles = Vec::new();

        for _ in 0..32 {
            let store = store.clone();
            let barrier = barrier.clone();
            handles.push(tokio::spawn(async move {
                barrier.wait().await;
                store
                    .check_and_insert("concurrent-id", Duration::from_secs(1))
                    .await
                    .unwrap()
            }));
        }
        barrier.wait().await;

        let mut inserted = 0;
        let mut duplicates = 0;
        for handle in handles {
            match handle.await.unwrap() {
                DeliveryIdCheck::Inserted => inserted += 1,
                DeliveryIdCheck::Duplicate => duplicates += 1,
            }
        }

        assert_eq!(inserted, 1);
        assert_eq!(duplicates, 31);
    }

    #[tokio::test]
    async fn test_custom_delivery_id_store_is_used() {
        let store = Arc::new(RecordingDeliveryIdStore::default());
        let mut handler = LambdaHandler::new()
            .with_delivery_id_store(store.clone())
            .with_push_auth(PushAuthConfig::new().with_signing_secret(TEST_SECRET.to_vec()));
        handler.register("email.send", |_ctx, _job: JobEvent| async move { Ok(()) });

        let body =
            make_request_body_with("email.send", "job-custom", "w1", "custom-store-delivery");
        let timestamp = push_auth::unix_timestamp_now().as_secs().to_string();
        let signature = sign(TEST_SECRET, &timestamp, &body);

        handler
            .handle_http_authenticated(&headers_with(&timestamp, &signature), &body)
            .await
            .unwrap();

        assert_eq!(store.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            store
                .ids
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_slice(),
            ["custom-store-delivery"]
        );
    }

    #[tokio::test]
    async fn test_valid_signature_dispatches_to_handler() {
        let secret = TEST_SECRET;
        let mut handler =
            authenticated_handler(PushAuthConfig::new().with_signing_secret(secret.to_vec()));
        handler.register("email.send", |_ctx, _job: JobEvent| async move { Ok(()) });

        let body = make_request_body("email.send");
        let now = push_auth::unix_timestamp_now().as_secs();
        let timestamp = now.to_string();
        let signature = sign(secret, &timestamp, &body);
        let headers = headers_with(&timestamp, &signature);

        let resp = handler
            .handle_http_authenticated(&headers, &body)
            .await
            .unwrap();
        assert_eq!(resp.status, "completed");
    }

    #[tokio::test]
    async fn test_missing_delivery_id_is_rejected_for_authenticated_push() {
        let secret = TEST_SECRET;
        let handler =
            authenticated_handler(PushAuthConfig::new().with_signing_secret(secret.to_vec()));

        let body = make_request_body_with("email.send", "job-1", "w1", "");
        let timestamp = push_auth::unix_timestamp_now().as_secs().to_string();
        let signature = sign(secret, &timestamp, &body);
        let headers = headers_with(&timestamp, &signature);

        let err = handler
            .handle_http_authenticated(&headers, &body)
            .await
            .unwrap_err();
        assert!(matches!(err, ServerlessError::NonRetryable(_)));
    }

    #[tokio::test]
    async fn test_delivery_and_job_headers_must_match_authenticated_body() {
        let secret = TEST_SECRET;
        let handler =
            authenticated_handler(PushAuthConfig::new().with_signing_secret(secret.to_vec()));

        let body = make_request_body("email.send");
        let timestamp = push_auth::unix_timestamp_now().as_secs().to_string();
        let signature = sign(secret, &timestamp, &body);

        let mut bad_delivery_headers = headers_with(&timestamp, &signature);
        bad_delivery_headers.insert(
            PUSH_DELIVERY_ID_HEADER.to_string(),
            "different-delivery".to_string(),
        );
        let err = handler
            .handle_http_authenticated(&bad_delivery_headers, &body)
            .await
            .unwrap_err();
        assert!(matches!(err, ServerlessError::NonRetryable(_)));

        let mut bad_job_headers = headers_with(&timestamp, &signature);
        bad_job_headers.insert(PUSH_JOB_ID_HEADER.to_string(), "different-job".to_string());
        let err = handler
            .handle_http_authenticated(&bad_job_headers, &body)
            .await
            .unwrap_err();
        assert!(matches!(err, ServerlessError::NonRetryable(_)));
    }

    #[tokio::test]
    async fn test_signed_replay_is_rejected_within_freshness_window() {
        let secret = TEST_SECRET;
        let calls = Arc::new(AtomicUsize::new(0));
        let mut handler =
            authenticated_handler(PushAuthConfig::new().with_signing_secret(secret.to_vec()));
        handler.register("email.send", {
            let calls = calls.clone();
            move |_ctx, _job: JobEvent| {
                let calls = calls.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            }
        });

        let body = make_request_body("email.send");
        let timestamp = push_auth::unix_timestamp_now().as_secs().to_string();
        let signature = sign(secret, &timestamp, &body);
        let headers = headers_with(&timestamp, &signature);

        let resp = handler
            .handle_http_authenticated(&headers, &body)
            .await
            .unwrap();
        assert_eq!(resp.status, "completed");

        let err = handler
            .handle_http_authenticated(&headers, &body)
            .await
            .unwrap_err();
        assert!(matches!(err, ServerlessError::NonRetryable(_)));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_missing_timestamp_header_rejected_without_dispatch() {
        let secret = TEST_SECRET;
        let handler =
            authenticated_handler(PushAuthConfig::new().with_signing_secret(secret.to_vec()));

        let body = make_request_body("email.send");
        let mut headers = HashMap::new();
        headers.insert(
            PUSH_SIGNATURE_HEADER.to_string(),
            "sha256=deadbeef".to_string(),
        );

        let err = handler
            .handle_http_authenticated(&headers, &body)
            .await
            .unwrap_err();
        assert!(matches!(err, ServerlessError::NonRetryable(_)));
    }

    #[tokio::test]
    async fn test_missing_signature_header_rejected() {
        let secret = TEST_SECRET;
        let handler =
            authenticated_handler(PushAuthConfig::new().with_signing_secret(secret.to_vec()));

        let body = make_request_body("email.send");
        let mut headers = HashMap::new();
        headers.insert(
            PUSH_TIMESTAMP_HEADER.to_string(),
            push_auth::unix_timestamp_now().as_secs().to_string(),
        );

        let err = handler
            .handle_http_authenticated(&headers, &body)
            .await
            .unwrap_err();
        assert!(matches!(err, ServerlessError::NonRetryable(_)));
    }

    #[tokio::test]
    async fn test_stale_timestamp_rejected() {
        let secret = TEST_SECRET;
        let handler = authenticated_handler(
            PushAuthConfig::new()
                .with_signing_secret(secret.to_vec())
                .with_freshness_window(Duration::from_secs(60)),
        );

        let body = make_request_body("email.send");
        // 1 hour old: well outside the 60s freshness window.
        let stale_timestamp = (push_auth::unix_timestamp_now().as_secs() - 3600).to_string();
        let signature = sign(secret, &stale_timestamp, &body);
        let headers = headers_with(&stale_timestamp, &signature);

        let err = handler
            .handle_http_authenticated(&headers, &body)
            .await
            .unwrap_err();
        assert!(matches!(err, ServerlessError::NonRetryable(_)));
    }

    #[tokio::test]
    async fn test_wrong_signature_rejected() {
        let secret = TEST_SECRET;
        let handler =
            authenticated_handler(PushAuthConfig::new().with_signing_secret(secret.to_vec()));

        let body = make_request_body("email.send");
        let timestamp = push_auth::unix_timestamp_now().as_secs().to_string();
        // Signed with a *different* secret than the one configured.
        let wrong_signature = sign(WRONG_SECRET, &timestamp, &body);
        let headers = headers_with(&timestamp, &wrong_signature);

        let err = handler
            .handle_http_authenticated(&headers, &body)
            .await
            .unwrap_err();
        assert!(matches!(err, ServerlessError::NonRetryable(_)));
    }

    #[tokio::test]
    async fn test_tampered_body_rejected() {
        let secret = TEST_SECRET;
        let mut handler =
            authenticated_handler(PushAuthConfig::new().with_signing_secret(secret.to_vec()));
        handler.register("email.send", |_ctx, _job: JobEvent| async move { Ok(()) });

        let original_body = make_request_body("email.send");
        let timestamp = push_auth::unix_timestamp_now().as_secs().to_string();
        // Sign the ORIGINAL body...
        let signature = sign(secret, &timestamp, &original_body);
        let headers = headers_with(&timestamp, &signature);

        // ...but present a *different* body with that stale signature,
        // simulating a tampered-in-transit or replayed-with-edits request.
        let tampered_body = make_request_body("payment.charge");

        let err = handler
            .handle_http_authenticated(&headers, &tampered_body)
            .await
            .unwrap_err();
        assert!(matches!(err, ServerlessError::NonRetryable(_)));
    }

    #[tokio::test]
    async fn test_secret_rotation_accepts_either_secret() {
        let old_secret = OLD_SECRET;
        let new_secret = NEW_SECRET;
        let mut handler = authenticated_handler(
            PushAuthConfig::new()
                .with_signing_secret(old_secret.to_vec())
                .with_signing_secret(new_secret.to_vec()),
        );
        handler.register("email.send", |_ctx, _job: JobEvent| async move { Ok(()) });

        let first_body = make_request_body_with("email.send", "job-1", "w1", "d1");
        let second_body = make_request_body_with("email.send", "job-1", "w1", "d2");
        let timestamp = push_auth::unix_timestamp_now().as_secs().to_string();

        // A request signed with the *old* secret still verifies.
        let old_signature = sign(old_secret, &timestamp, &first_body);
        let headers = headers_with(&timestamp, &old_signature);
        let resp = handler
            .handle_http_authenticated(&headers, &first_body)
            .await
            .unwrap();
        assert_eq!(resp.status, "completed");

        // A distinct delivery signed with the *new* secret also verifies.
        let new_signature = sign(new_secret, &timestamp, &second_body);
        let headers2 = headers_with(&timestamp, &new_signature);
        let resp2 = handler
            .handle_http_authenticated(&headers2, &second_body)
            .await
            .unwrap();
        assert_eq!(resp2.status, "completed");
    }

    #[tokio::test]
    async fn test_secret_rotation_does_not_bypass_delivery_replay_cache() {
        let old_secret = OLD_SECRET;
        let new_secret = NEW_SECRET;
        let calls = Arc::new(AtomicUsize::new(0));
        let mut handler = authenticated_handler(
            PushAuthConfig::new()
                .with_signing_secret(old_secret.to_vec())
                .with_signing_secret(new_secret.to_vec()),
        );
        handler.register("email.send", {
            let calls = calls.clone();
            move |_ctx, _job: JobEvent| {
                let calls = calls.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            }
        });

        let first_body = make_request_body_with("email.send", "job-1", "w1", "shared-delivery");
        let second_body = make_request_body_with("email.send", "job-1", "w1", "fresh-delivery");
        let timestamp = push_auth::unix_timestamp_now().as_secs().to_string();

        let old_signature = sign(old_secret, &timestamp, &first_body);
        let resp = handler
            .handle_http_authenticated(&headers_with(&timestamp, &old_signature), &first_body)
            .await
            .unwrap();
        assert_eq!(resp.status, "completed");

        let replay_signature = sign(new_secret, &timestamp, &first_body);
        let err = handler
            .handle_http_authenticated(&headers_with(&timestamp, &replay_signature), &first_body)
            .await
            .unwrap_err();
        assert!(matches!(err, ServerlessError::NonRetryable(_)));

        let fresh_signature = sign(new_secret, &timestamp, &second_body);
        let resp = handler
            .handle_http_authenticated(&headers_with(&timestamp, &fresh_signature), &second_body)
            .await
            .unwrap();
        assert_eq!(resp.status, "completed");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_insecure_unsigned_bypasses_verification() {
        let mut handler = authenticated_handler(
            PushAuthConfig::new().allow_insecure_unsigned_for_local_development(),
        );
        handler.register("email.send", |_ctx, _job: JobEvent| async move { Ok(()) });

        let body = make_request_body("email.send");
        // No timestamp/signature headers at all.
        let headers = HashMap::new();

        let resp = handler
            .handle_http_authenticated(&headers, &body)
            .await
            .unwrap();
        assert_eq!(resp.status, "completed");
    }

    #[tokio::test]
    async fn test_no_push_auth_configured_fails_closed() {
        // A handler with NO `.with_push_auth(...)` call at all must refuse
        // to use the authenticated entry point rather than silently
        // accepting unsigned requests.
        let handler = LambdaHandler::new();
        let body = make_request_body("email.send");
        let headers = HashMap::new();

        let err = handler
            .handle_http_authenticated(&headers, &body)
            .await
            .unwrap_err();
        assert!(matches!(err, ServerlessError::NonRetryable(_)));
    }

    #[tokio::test]
    async fn test_no_secrets_configured_fails_closed() {
        // `PushAuthConfig::new()` with no secrets and no explicit insecure
        // opt-in must also fail closed.
        let handler = authenticated_handler(PushAuthConfig::new());
        let body = make_request_body("email.send");
        let timestamp = push_auth::unix_timestamp_now().as_secs().to_string();
        let signature = sign(TEST_SECRET, &timestamp, &body);
        let headers = headers_with(&timestamp, &signature);

        let err = handler
            .handle_http_authenticated(&headers, &body)
            .await
            .unwrap_err();
        assert!(matches!(err, ServerlessError::NonRetryable(_)));
    }

    #[tokio::test]
    async fn test_header_lookup_is_case_insensitive() {
        let secret = TEST_SECRET;
        let mut handler =
            authenticated_handler(PushAuthConfig::new().with_signing_secret(secret.to_vec()));
        handler.register("email.send", |_ctx, _job: JobEvent| async move { Ok(()) });

        let body = make_request_body("email.send");
        let timestamp = push_auth::unix_timestamp_now().as_secs().to_string();
        let signature = sign(secret, &timestamp, &body);

        let mut headers = HashMap::new();
        headers.insert("x-ojs-timestamp".to_string(), timestamp);
        headers.insert("x-ojs-signature".to_string(), signature);

        let resp = handler
            .handle_http_authenticated(&headers, &body)
            .await
            .unwrap();
        assert_eq!(resp.status, "completed");
    }

    #[tokio::test]
    async fn test_raw_string_authenticated_wrapper_matches_bytes_version() {
        let secret = TEST_SECRET;
        let mut handler =
            authenticated_handler(PushAuthConfig::new().with_signing_secret(secret.to_vec()));
        handler.register("email.send", |_ctx, _job: JobEvent| async move { Ok(()) });

        let body = make_request_body("email.send");
        let body_str = String::from_utf8(body.clone()).unwrap();
        let timestamp = push_auth::unix_timestamp_now().as_secs().to_string();
        let signature = sign(secret, &timestamp, &body);
        let headers = headers_with(&timestamp, &signature);

        let resp = handler
            .handle_http_raw_authenticated(&headers, &body_str)
            .await
            .unwrap();
        assert_eq!(resp.status, "completed");
    }
}
