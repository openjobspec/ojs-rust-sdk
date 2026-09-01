//! Worker: fetches and processes jobs, plus its builder.
//!
//! Split into cohesive actors, each in its own submodule:
//!
//! - `state` — the [`WorkerState`] lifecycle enum.
//! - `context` — [`JobContext`], the per-job handler context, and its
//!   heartbeat method.
//! - `protocol` — worker protocol request/report helpers: per-job dispatch
//!   and the `ack`/`nack` calls it makes.
//!
//! [`WorkerBuilder`] and [`Worker`] itself (registration, middleware, and
//! the fetch/dispatch main loop in [`Worker::start`]) remain here, as the
//! cohesive "worker" actor that owns and drives all of the above.

use crate::errors::OjsError;
use crate::job::{FetchRequest, FetchResponse, HeartbeatRequest, HeartbeatResponse, Job};
use crate::middleware::{BoxFuture, HandlerFn, HandlerResult, Middleware, MiddlewareChain};
use crate::transport::{self, DynTransport, HttpTransport};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicI64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::task::JoinSet;

mod context;
mod protocol;
mod state;

pub use context::JobContext;
pub use state::WorkerState;

use protocol::process_job;

// ---------------------------------------------------------------------------
// Worker builder
// ---------------------------------------------------------------------------

/// Builder for constructing an OJS [`Worker`].
#[must_use = "builders do nothing until `.build()` is called"]
pub struct WorkerBuilder {
    url: Option<String>,
    queues: Vec<String>,
    concurrency: usize,
    grace_period: Duration,
    heartbeat_interval: Duration,
    poll_interval: Duration,
    labels: Vec<String>,
    auth_token: Option<String>,
    headers: HashMap<String, String>,
    timeout: Option<Duration>,
    retry_config: Option<crate::rate_limiter::RetryConfig>,
    #[cfg(feature = "reqwest-transport")]
    http_client: Option<reqwest::Client>,
}

impl WorkerBuilder {
    fn new() -> Self {
        Self {
            url: None,
            queues: vec!["default".to_string()],
            concurrency: 10,
            grace_period: Duration::from_secs(25),
            heartbeat_interval: Duration::from_secs(5),
            poll_interval: Duration::from_secs(1),
            labels: Vec::new(),
            auth_token: None,
            headers: HashMap::new(),
            timeout: None,
            retry_config: None,
            #[cfg(feature = "reqwest-transport")]
            http_client: None,
        }
    }

    /// Set the OJS server URL.
    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }

    /// Apply shared connection configuration.
    ///
    /// This sets the URL, auth token, headers, and timeout from a
    /// [`ConnectionConfig`](crate::ConnectionConfig). Individual builder
    /// methods called after this will override the config values.
    pub fn connection(mut self, config: crate::ConnectionConfig) -> Self {
        self.url = Some(config.url);
        self.auth_token = config.auth_token;
        self.headers = config.headers;
        self.timeout = config.timeout;
        self
    }

    /// Set the queues to fetch jobs from (priority order: left to right).
    pub fn queues(mut self, queues: Vec<impl Into<String>>) -> Self {
        self.queues = queues.into_iter().map(Into::into).collect();
        self
    }

    /// Set the maximum number of concurrent jobs.
    pub fn concurrency(mut self, n: usize) -> Self {
        self.concurrency = n;
        self
    }

    /// Set the grace period for shutdown (time to wait for active jobs).
    pub fn grace_period(mut self, d: Duration) -> Self {
        self.grace_period = d;
        self
    }

    /// Set the heartbeat interval.
    pub fn heartbeat_interval(mut self, d: Duration) -> Self {
        self.heartbeat_interval = d;
        self
    }

    /// Set the poll interval for fetching new jobs.
    pub fn poll_interval(mut self, d: Duration) -> Self {
        self.poll_interval = d;
        self
    }

    /// Add labels to identify this worker.
    pub fn labels(mut self, labels: Vec<impl Into<String>>) -> Self {
        self.labels = labels.into_iter().map(Into::into).collect();
        self
    }

    /// Set the authentication bearer token.
    pub fn auth_token(mut self, token: impl Into<String>) -> Self {
        self.auth_token = Some(token.into());
        self
    }

    /// Set the request timeout. Defaults to 30 seconds.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Provide a custom reqwest HTTP client.
    #[cfg(feature = "reqwest-transport")]
    #[cfg_attr(docsrs, doc(cfg(feature = "reqwest-transport")))]
    pub fn http_client(mut self, client: reqwest::Client) -> Self {
        self.http_client = Some(client);
        self
    }

    /// Set the retry configuration for rate-limited responses.
    ///
    /// By default, the worker retries up to 3 times on `429 Too Many Requests`
    /// responses with exponential backoff. Use [`RetryConfig::disabled()`] to
    /// turn off automatic retries.
    pub fn retry_config(mut self, config: crate::rate_limiter::RetryConfig) -> Self {
        self.retry_config = Some(config);
        self
    }

    /// Build the worker.
    pub fn build(self) -> crate::Result<Worker> {
        let url = self
            .url
            .ok_or_else(|| OjsError::Builder("url is required".into()))?;

        let transport = HttpTransport::new(
            &url,
            crate::transport::http::TransportConfig {
                auth_token: self.auth_token,
                headers: self.headers,
                timeout: self.timeout,
                retry_config: self.retry_config,
                #[cfg(feature = "reqwest-transport")]
                http_client: self.http_client,
            },
        );

        let worker_id = generate_worker_id();

        Ok(Worker {
            transport: Arc::new(transport),
            worker_id,
            queues: self.queues,
            concurrency: self.concurrency,
            grace_period: self.grace_period,
            heartbeat_interval: self.heartbeat_interval,
            poll_interval: self.poll_interval,
            labels: self.labels,
            handlers: Arc::new(RwLock::new(HashMap::new())),
            middleware: Arc::new(RwLock::new(MiddlewareChain::new())),
            state: Arc::new(AtomicU8::new(WorkerState::Running as u8)),
            active_count: Arc::new(AtomicI64::new(0)),
            active_jobs: Arc::new(RwLock::new(HashSet::new())),
        })
    }
}

// ---------------------------------------------------------------------------
// Worker
// ---------------------------------------------------------------------------

/// An OJS worker that fetches and processes jobs.
///
/// # Example
///
/// ```rust,no_run
/// use ojs::{Worker, JobContext};
/// use serde_json::json;
///
/// # #[tokio::main]
/// # async fn main() -> ojs::Result<()> {
/// let worker = Worker::builder()
///     .url("http://localhost:8080")
///     .queues(vec!["default", "email"])
///     .concurrency(10)
///     .build()?;
///
/// worker.register("email.send", |ctx: JobContext| async move {
///     let to: String = ctx.job.arg("to")?;
///     // process the job...
///     Ok(json!({"status": "sent"}))
/// }).await;
///
/// worker.start().await?;
/// # Ok(())
/// # }
/// ```
pub struct Worker {
    transport: DynTransport,
    worker_id: String,
    queues: Vec<String>,
    concurrency: usize,
    grace_period: Duration,
    heartbeat_interval: Duration,
    poll_interval: Duration,
    #[allow(dead_code)]
    labels: Vec<String>,
    handlers: Arc<RwLock<HashMap<String, HandlerFn>>>,
    middleware: Arc<RwLock<MiddlewareChain>>,
    state: Arc<AtomicU8>,
    active_count: Arc<AtomicI64>,
    active_jobs: Arc<RwLock<HashSet<String>>>,
}

impl std::fmt::Debug for Worker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Worker")
            .field("worker_id", &self.worker_id)
            .field("queues", &self.queues)
            .field("concurrency", &self.concurrency)
            .finish()
    }
}

impl Worker {
    /// Create a new worker builder.
    pub fn builder() -> WorkerBuilder {
        WorkerBuilder::new()
    }

    /// Register a handler for a job type.
    ///
    /// The handler receives a [`JobContext`] and should return a JSON value
    /// on success. Returning an `Err` will cause the job to be nack'd.
    pub async fn register<F, Fut>(&self, job_type: impl Into<String>, handler: F)
    where
        F: Fn(JobContext) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = HandlerResult> + Send + 'static,
    {
        let handler: HandlerFn =
            Arc::new(move |ctx| Box::pin(handler(ctx)) as BoxFuture<'static, HandlerResult>);
        let mut handlers = self.handlers.write().await;
        handlers.insert(job_type.into(), handler);
    }

    /// Register a typed handler that auto-deserializes job args via serde.
    ///
    /// The handler receives a [`JobContext`] and the deserialized args `T`.
    /// This provides compile-time type safety for job arguments.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use ojs::{Worker, JobContext};
    /// use serde::Deserialize;
    /// use serde_json::json;
    ///
    /// #[derive(Deserialize)]
    /// struct EmailArgs {
    ///     to: String,
    ///     subject: String,
    /// }
    ///
    /// # #[tokio::main]
    /// # async fn main() -> ojs::Result<()> {
    /// let worker = Worker::builder()
    ///     .url("http://localhost:8080")
    ///     .build()?;
    ///
    /// worker.register_typed("email.send", |ctx: JobContext, args: EmailArgs| async move {
    ///     println!("Sending to {}: {}", args.to, args.subject);
    ///     Ok(json!({"status": "sent"}))
    /// }).await;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn register_typed<T, F, Fut>(&self, job_type: impl Into<String>, handler: F)
    where
        T: serde::de::DeserializeOwned + Send + 'static,
        F: Fn(JobContext, T) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = HandlerResult> + Send + 'static,
    {
        let handler = Arc::new(handler);
        let typed_handler: HandlerFn = Arc::new(move |ctx: JobContext| {
            let handler = handler.clone();
            Box::pin(async move {
                let args: T = serde_json::from_value(ctx.job.args.clone()).map_err(|e| {
                    OjsError::Handler(format!("failed to deserialize job args: {}", e))
                })?;
                handler(ctx, args).await
            }) as BoxFuture<'static, HandlerResult>
        });
        let mut handlers = self.handlers.write().await;
        handlers.insert(job_type.into(), typed_handler);
    }

    /// Add middleware to the worker.
    ///
    /// Middleware wraps all job handlers and executes in registration order
    /// (first registered = outermost wrapper).
    pub async fn use_middleware(&self, name: impl Into<String>, mw: impl Middleware) {
        self.middleware.write().await.add(name, mw);
    }

    /// Remove a named middleware from the worker.
    pub async fn remove_middleware(&self, name: &str) {
        self.middleware.write().await.remove(name);
    }

    /// Insert middleware before an existing named middleware.
    ///
    /// If the named middleware doesn't exist, the new middleware is prepended.
    pub async fn insert_middleware_before(
        &self,
        existing: &str,
        name: impl Into<String>,
        mw: impl Middleware,
    ) {
        self.middleware
            .write()
            .await
            .insert_before(existing, name, mw);
    }

    /// Insert middleware after an existing named middleware.
    ///
    /// If the named middleware doesn't exist, the new middleware is appended.
    pub async fn insert_middleware_after(
        &self,
        existing: &str,
        name: impl Into<String>,
        mw: impl Middleware,
    ) {
        self.middleware
            .write()
            .await
            .insert_after(existing, name, mw);
    }

    /// Get the current worker state.
    pub fn state(&self) -> WorkerState {
        WorkerState::from_u8(self.state.load(Ordering::SeqCst))
    }

    /// Get the worker ID.
    pub fn id(&self) -> &str {
        &self.worker_id
    }

    /// Start the worker and begin processing jobs.
    ///
    /// This method blocks until the worker is shut down (via signal or
    /// context cancellation). It performs graceful shutdown: stops fetching
    /// new jobs and waits for active jobs to complete within the grace period.
    pub async fn start(&self) -> crate::Result<()> {
        tracing::info!(
            worker_id = %self.worker_id,
            queues = ?self.queues,
            concurrency = self.concurrency,
            "worker starting"
        );

        let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);

        // Spawn signal handler
        let shutdown_tx_signal = shutdown_tx.clone();
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            let _ = shutdown_tx_signal.send(true);
        });

        // Semaphore for concurrency control
        let semaphore = Arc::new(tokio::sync::Semaphore::new(self.concurrency));

        // Spawn heartbeat loop
        let heartbeat_handle = {
            let transport = self.transport.clone();
            let worker_id = self.worker_id.clone();
            let interval = self.heartbeat_interval;
            let state = self.state.clone();
            let active_jobs = self.active_jobs.clone();
            let mut shutdown_rx = shutdown_rx.clone();

            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(interval);
                ticker.tick().await; // skip first immediate tick

                loop {
                    tokio::select! {
                        _ = ticker.tick() => {
                            let jobs: Vec<String> = active_jobs.read().await.iter().cloned().collect();
                            let req = HeartbeatRequest {
                                worker_id: worker_id.clone(),
                                active_jobs: Some(jobs),
                                visibility_timeout_ms: None,
                            };

                            match transport::transport_post::<_, HeartbeatResponse>(&transport, "/workers/heartbeat", &req).await {
                                Ok(resp) => {
                                    // Server can direct state changes
                                    match resp.state.as_str() {
                                        "quiet" => {
                                            state.store(WorkerState::Quiet as u8, Ordering::SeqCst);
                                            tracing::info!("server directed worker to quiet mode");
                                        }
                                        "terminate" => {
                                            state.store(WorkerState::Terminate as u8, Ordering::SeqCst);
                                            tracing::info!("server directed worker to terminate");
                                        }
                                        _ => {}
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(error = %e, "heartbeat failed");
                                }
                            }
                        }
                        _ = shutdown_rx.changed() => {
                            break;
                        }
                    }
                }
            })
        };

        // Main fetch loop
        let mut join_set = JoinSet::new();

        loop {
            // Check for shutdown
            if *shutdown_rx.borrow() {
                self.state
                    .store(WorkerState::Terminate as u8, Ordering::SeqCst);
                break;
            }

            let current_state = self.state();
            if current_state == WorkerState::Terminate {
                break;
            }

            if current_state == WorkerState::Quiet {
                // In quiet mode, don't fetch new jobs, just wait
                tokio::select! {
                    _ = tokio::time::sleep(self.poll_interval) => {}
                    _ = shutdown_rx.changed() => { break; }
                }
                continue;
            }

            // Calculate how many jobs to fetch
            let active = self.active_count.load(Ordering::SeqCst) as usize;
            let capacity = self.concurrency.saturating_sub(active);

            if capacity == 0 {
                tokio::select! {
                    _ = tokio::time::sleep(self.poll_interval) => {}
                    _ = shutdown_rx.changed() => { break; }
                }
                continue;
            }

            // Fetch jobs
            let fetch_count = capacity.min(10) as u32; // Batch size cap
            match self.fetch_jobs(fetch_count).await {
                Ok(jobs) => {
                    if jobs.is_empty() {
                        // No jobs available, wait before polling again
                        tokio::select! {
                            _ = tokio::time::sleep(self.poll_interval) => {}
                            _ = shutdown_rx.changed() => { break; }
                        }
                        continue;
                    }

                    for job in jobs {
                        let permit = match semaphore.clone().acquire_owned().await {
                            Ok(p) => p,
                            Err(_) => break, // Semaphore closed — shutting down
                        };
                        let job_id = job.id.clone();

                        // Track active job
                        self.active_count.fetch_add(1, Ordering::SeqCst);
                        self.active_jobs.write().await.insert(job_id.clone());

                        let transport = self.transport.clone();
                        let worker_id = self.worker_id.clone();
                        let handlers = self.handlers.clone();
                        let middleware = self.middleware.clone();
                        let active_count = self.active_count.clone();
                        let active_jobs = self.active_jobs.clone();

                        join_set.spawn(async move {
                            let result =
                                process_job(&transport, &worker_id, &handlers, &middleware, job)
                                    .await;

                            // Cleanup
                            active_count.fetch_sub(1, Ordering::SeqCst);
                            {
                                let mut jobs = active_jobs.write().await;
                                jobs.remove(&job_id);
                            }

                            drop(permit);
                            result
                        });
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "failed to fetch jobs");
                    tokio::select! {
                        _ = tokio::time::sleep(self.poll_interval) => {}
                        _ = shutdown_rx.changed() => { break; }
                    }
                }
            }

            // Reap completed tasks
            while let Some(result) = join_set.try_join_next() {
                if let Err(e) = result {
                    tracing::error!(error = %e, "job task panicked");
                }
            }
        }

        // Graceful shutdown: wait for active jobs to complete
        tracing::info!("worker shutting down, waiting for active jobs...");

        let grace_deadline = tokio::time::Instant::now() + self.grace_period;

        loop {
            if self.active_count.load(Ordering::SeqCst) == 0 {
                break;
            }

            if tokio::time::Instant::now() >= grace_deadline {
                let remaining = self.active_count.load(Ordering::SeqCst);
                tracing::warn!(
                    remaining_jobs = remaining,
                    "grace period expired, abandoning remaining jobs"
                );
                break;
            }

            // Reap completed tasks
            tokio::select! {
                result = join_set.join_next() => {
                    if let Some(Err(e)) = result {
                        tracing::error!(error = %e, "job task panicked during shutdown");
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
        }

        heartbeat_handle.abort();
        let _ = shutdown_tx.send(true);

        tracing::info!(worker_id = %self.worker_id, "worker stopped");
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Internal worker protocol methods
    // -----------------------------------------------------------------------

    async fn fetch_jobs(&self, count: u32) -> crate::Result<Vec<Job>> {
        let req = FetchRequest {
            queues: self.queues.clone(),
            count: Some(count),
            worker_id: Some(self.worker_id.clone()),
            visibility_timeout_ms: None,
        };

        let resp: FetchResponse =
            transport::transport_post(&self.transport, "/workers/fetch", &req).await?;
        Ok(resp.jobs)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn generate_worker_id() -> String {
    format!("worker_{}", uuid::Uuid::now_v7())
}
