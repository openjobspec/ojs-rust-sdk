//! Worker: fetches and processes jobs, plus its builder.
//!
//! Split into cohesive actors, each in its own submodule:
//!
//! - `state` — the [`WorkerState`] lifecycle enum and the absorbing-`Terminate`
//!   state machine shared between the main loop and the heartbeat task.
//! - `context` — [`JobContext`], its heartbeat method, and its
//!   durable-execution checkpoint save/get/delete methods.
//! - `protocol` — worker protocol request/report helpers: per-job dispatch
//!   and the `ack`/`nack` calls it makes.
//! - `report` — the per-job terminal-reporting phase machine
//!   (unclaimed / reporting-in-flight / completed) that makes ACK/NACK
//!   exactly-once even when shutdown races an in-flight report.
//! - `shutdown` — shutdown signal waiting, bounded awaiting of terminal
//!   reports that were still in flight at grace expiry, and the single
//!   forced NACK per job that remains unreported afterwards.
//!
//! [`WorkerBuilder`] and [`Worker`] itself (registration, middleware, and
//! the fetch/dispatch main loop in [`Worker::start`]) remain here, as the
//! cohesive "worker" actor that owns and drives all of the above.

use crate::errors::OjsError;
use crate::job::{FetchRequest, FetchResponse, HeartbeatRequest, HeartbeatResponse, Job};
use crate::middleware::{BoxFuture, HandlerFn, HandlerResult, Middleware, MiddlewareChain};
#[cfg(feature = "reqwest-transport")]
use crate::transport::HttpTransport;
use crate::transport::{self, DynTransport};
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::task::JoinSet;

mod context;
mod protocol;
mod report;
mod shutdown;
mod state;

pub use context::JobContext;
pub use state::WorkerState;

pub(crate) use report::ActiveJobState;

type ActiveJobs = std::sync::Mutex<HashMap<String, Arc<ActiveJobState>>>;

#[derive(Debug)]
struct ActiveJobGuard {
    job_id: String,
    active_count: Arc<AtomicI64>,
    active_jobs: Arc<ActiveJobs>,
}

impl ActiveJobGuard {
    fn new(job_id: String, active_count: Arc<AtomicI64>, active_jobs: Arc<ActiveJobs>) -> Self {
        Self {
            job_id,
            active_count,
            active_jobs,
        }
    }
}

impl Drop for ActiveJobGuard {
    fn drop(&mut self) {
        self.active_count.fetch_sub(1, Ordering::SeqCst);
        let mut jobs = self
            .active_jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        jobs.remove(&self.job_id);
    }
}

// ---------------------------------------------------------------------------
// Worker builder
// ---------------------------------------------------------------------------

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
    transport: Option<DynTransport>,
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
            transport: None,
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
    ///
    /// After this period expires, handler execution is aborted, but any
    /// ACK/NACK that was already in flight keeps running and is awaited
    /// (bounded). Jobs that never started reporting -- and reports that
    /// never settle, which are cancelled first -- receive exactly one forced
    /// NACK. All forced-shutdown work shares one additional absolute
    /// five-second deadline.
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

    /// Use a custom [`Transport`](crate::transport::Transport) implementation
    /// instead of the built-in reqwest-based HTTP transport.
    ///
    /// When set, `url()`, `auth_token()`, `header()`, `timeout()`, and
    /// `http_client()` are ignored: a custom transport is responsible for
    /// its own request construction, authentication, and headers. This is
    /// also the only way to build a [`Worker`] when the `reqwest-transport`
    /// feature is disabled.
    pub fn transport(mut self, transport: DynTransport) -> Self {
        self.transport = Some(transport);
        self
    }

    /// Set the retry configuration for rate-limited responses.
    ///
    /// By default, the worker retries up to 3 times on `429 Too Many Requests`
    /// responses with exponential backoff. Use [`RetryConfig::disabled()`](crate::rate_limiter::RetryConfig::disabled) to
    /// turn off automatic retries.
    pub fn retry_config(mut self, config: crate::rate_limiter::RetryConfig) -> Self {
        self.retry_config = Some(config);
        self
    }

    /// Build the worker.
    pub fn build(self) -> crate::Result<Worker> {
        let transport: DynTransport = match self.transport {
            Some(t) => t,
            None => {
                let url = self
                    .url
                    .ok_or_else(|| OjsError::Builder("url is required".into()))?;

                #[cfg(not(feature = "reqwest-transport"))]
                {
                    return Err(OjsError::Builder(format!(
                        "Worker::builder().build() requires either a custom transport \
                         via `.transport(...)` or the `reqwest-transport` feature \
                         (attempted to connect to `{url}`)"
                    )));
                }

                #[cfg(feature = "reqwest-transport")]
                {
                    Arc::new(HttpTransport::new(
                        &url,
                        crate::transport::http::TransportConfig {
                            auth_token: self.auth_token,
                            headers: self.headers,
                            timeout: self.timeout,
                            retry_config: self.retry_config,
                            http_client: self.http_client,
                        },
                    ))
                }
            }
        };

        let worker_id = generate_worker_id();

        let (shutdown_tx, _) = tokio::sync::watch::channel(false);

        Ok(Worker {
            transport,
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
            active_jobs: Arc::new(std::sync::Mutex::new(HashMap::new())),
            shutdown_tx,
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
    active_jobs: Arc<ActiveJobs>,
    /// Owned by the `Worker` (rather than a local variable inside `start()`)
    /// so that `shutdown()` can be called from any task holding a reference
    /// to this worker, independent of whichever call is running `start()`.
    shutdown_tx: tokio::sync::watch::Sender<bool>,
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

    /// Attempt to transition the worker's lifecycle state.
    ///
    /// `Terminate` is an absorbing state: once set (locally, via
    /// [`Worker::shutdown`], or by a server-directed heartbeat response),
    /// no later transition -- local or server-directed -- can move the
    /// worker back to `Quiet` or `Running`. Without this, a heartbeat
    /// response that was already in flight when a local shutdown set
    /// `Terminate` could land afterward and silently revert the state.
    ///
    /// Delegates to the `state` submodule's `transition_shared`, which is
    /// also used directly (via a cloned `Arc<AtomicU8>`, no `&self`
    /// required) by the detached heartbeat task below.
    fn set_state(&self, new: WorkerState) {
        state::transition_shared(&self.state, new);
    }

    /// Request a graceful shutdown from any task holding a reference to
    /// this worker.
    ///
    /// Equivalent to the worker receiving a local Ctrl-C/SIGTERM: the fetch
    /// loop stops taking new jobs, active jobs get up to `grace_period` to
    /// finish. At grace expiry, terminal reporting is finalized before tasks
    /// are aborted: in-flight ACK/NACKs are awaited (bounded), everything
    /// still unreported afterwards gets exactly one forced NACK, and forced
    /// reports plus task joins share one absolute five-second deadline.
    /// Safe to call multiple times or
    /// before `start()` has been called; safe to call from a different task
    /// than the one running `start()` (e.g. a custom health check or admin
    /// endpoint), typically via `Arc<Worker>`. The request is latched even if
    /// no `start()` receiver exists yet, so a pre-start `shutdown()` still
    /// prevents the worker from ever fetching jobs.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send_replace(true);
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
    /// If grace expires, handler execution is aborted while terminal reports
    /// that were already in flight are allowed to finish (bounded);
    /// later-resuming handlers cannot ACK/NACK after the forced
    /// terminal-report claim, and blocking tasks cannot extend shutdown
    /// beyond the shared forced-shutdown deadline.
    pub async fn start(&self) -> crate::Result<()> {
        tracing::info!(
            worker_id = %self.worker_id,
            queues = ?self.queues,
            concurrency = self.concurrency,
            "worker starting"
        );

        let mut shutdown_rx = self.shutdown_tx.subscribe();

        // Spawn signal handler: Ctrl-C (SIGINT) everywhere, plus SIGTERM on
        // Unix (the standard shutdown signal from containers/Kubernetes).
        let shutdown_tx_signal = self.shutdown_tx.clone();
        let mut signal_handle = Some(tokio::spawn(async move {
            shutdown::wait_for_shutdown_signal().await;
            let _ = shutdown_tx_signal.send_replace(true);
        }));

        // Semaphore for concurrency control
        let semaphore = Arc::new(tokio::sync::Semaphore::new(self.concurrency));

        // Spawn heartbeat loop
        let mut heartbeat_handle = Some({
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
                            // Sort for deterministic wire ordering: a HashSet's
                            // iteration order is unspecified and would
                            // otherwise vary between heartbeats for the same
                            // job set.
                            let mut jobs: Vec<String> = active_jobs
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .keys()
                                .cloned()
                                .collect();
                            jobs.sort_unstable();
                            let req = HeartbeatRequest {
                                worker_id: worker_id.clone(),
                                active_jobs: Some(jobs),
                                visibility_timeout_ms: None,
                            };

                            match transport::transport_post::<_, HeartbeatResponse>(&transport, "/workers/heartbeat", &req).await {
                                Ok(resp) => {
                                    // Server can direct state changes. `set_state`
                                    // makes `Terminate` absorbing so a heartbeat
                                    // response that was already in flight when a
                                    // local shutdown set `Terminate` cannot revert
                                    // it back to `Quiet`/`Running`.
                                    match resp.state.as_str() {
                                        "quiet" => {
                                            state::transition_shared(&state, WorkerState::Quiet);
                                            tracing::info!("server directed worker to quiet mode");
                                        }
                                        "terminate" => {
                                            state::transition_shared(&state, WorkerState::Terminate);
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
        });

        // Main fetch loop
        let mut join_set = JoinSet::new();

        loop {
            // Check for shutdown
            if *shutdown_rx.borrow() {
                self.set_state(WorkerState::Terminate);
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
                        let job_state = Arc::new(ActiveJobState::new());

                        // Track active job
                        self.active_count.fetch_add(1, Ordering::SeqCst);
                        self.active_jobs
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .insert(job_id.clone(), job_state.clone());

                        let transport = self.transport.clone();
                        let worker_id = self.worker_id.clone();
                        let handlers = self.handlers.clone();
                        let middleware = self.middleware.clone();
                        let active_count = self.active_count.clone();
                        let active_jobs = self.active_jobs.clone();

                        join_set.spawn(async move {
                            let _permit = permit;
                            let _active_job =
                                ActiveJobGuard::new(job_id, active_count, active_jobs);

                            protocol::process_job(
                                &transport,
                                &worker_id,
                                &handlers,
                                &middleware,
                                job_state,
                                job,
                            )
                            .await
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
                log_job_task_result(result, "job task failed");
            }
        }

        // Graceful shutdown: wait for active jobs to complete
        tracing::info!("worker shutting down, waiting for active jobs...");

        let grace_deadline = tokio::time::Instant::now() + self.grace_period;
        let mut grace_expired = false;

        loop {
            if self.active_count.load(Ordering::SeqCst) == 0 {
                break;
            }

            if tokio::time::Instant::now() >= grace_deadline {
                grace_expired = true;
                let remaining_jobs: Vec<(String, Arc<ActiveJobState>)> = self
                    .active_jobs
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .iter()
                    .map(|(job_id, state)| (job_id.clone(), state.clone()))
                    .collect();
                // One absolute budget shared by every piece of forced
                // shutdown work, with an earlier sub-deadline reserved for
                // terminal reports that were already in flight.
                let shutdown_started = tokio::time::Instant::now();
                let forced_deadline = shutdown_started + shutdown::FORCED_SHUTDOWN_TIMEOUT;
                let report_deadline = shutdown_started + shutdown::IN_FLIGHT_REPORT_TIMEOUT;

                // Decide every job's fate before aborting or awaiting any
                // task: jobs that never started reporting are claimed here,
                // so a handler that resumes after blocking/CPU work sees the
                // claim and skips ACK/NACK, while jobs whose ACK/NACK is
                // already in flight are left running and awaited below.
                let plan = shutdown::plan_terminal_reports(remaining_jobs);
                tracing::warn!(
                    forced_jobs = plan.forced.len(),
                    in_flight_reports = plan.in_flight.len(),
                    already_reported = plan.already_reported,
                    report_deadline_ms = shutdown::IN_FLIGHT_REPORT_TIMEOUT.as_millis() as u64,
                    deadline_ms = shutdown::FORCED_SHUTDOWN_TIMEOUT.as_millis() as u64,
                    "grace period expired, finalizing terminal reports within the forced-shutdown deadline"
                );

                // Start forced releases and the bounded wait for in-flight
                // reports immediately, before any task termination is awaited.
                let transport = self.transport.clone();
                let forced_nack_handle = tokio::spawn(async move {
                    shutdown::finish_terminal_reports(
                        transport,
                        plan,
                        report_deadline,
                        forced_deadline,
                    )
                    .await;
                });

                if let Some(handle) = heartbeat_handle.as_ref() {
                    handle.abort();
                }
                if let Some(handle) = signal_handle.as_ref() {
                    handle.abort();
                }
                join_set.abort_all();

                // Poll all cleanup work concurrently, but never beyond the
                // same absolute deadline used by every forced NACK request.
                // In particular, an aborted async task stuck in CPU-bound or
                // blocking code is not awaited indefinitely.
                let cleanup = async {
                    let drain_jobs = async {
                        while let Some(result) = join_set.join_next().await {
                            log_job_task_result(result, "job task cancelled after grace expiry");
                        }
                    };
                    let wait_heartbeat = async {
                        if let Some(handle) = heartbeat_handle.take() {
                            let _ = handle.await;
                        }
                    };
                    let wait_signal = async {
                        if let Some(handle) = signal_handle.take() {
                            let _ = handle.await;
                        }
                    };
                    let wait_forced_nacks = async {
                        if let Err(error) = forced_nack_handle.await {
                            tracing::warn!(%error, "forced nack task failed");
                        }
                    };

                    tokio::join!(drain_jobs, wait_heartbeat, wait_signal, wait_forced_nacks);
                };

                if tokio::time::timeout_at(forced_deadline, cleanup)
                    .await
                    .is_err()
                {
                    tracing::warn!(
                        "forced-shutdown deadline elapsed before all aborted tasks terminated"
                    );
                }
                break;
            }

            // Reap completed tasks
            tokio::select! {
                result = join_set.join_next() => {
                    if let Some(result) = result {
                        log_job_task_result(result, "job task failed during shutdown");
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
        }

        if grace_expired {
            let _ = self.shutdown_tx.send_replace(true);
            tracing::info!(worker_id = %self.worker_id, "worker stopped");
            return Ok(());
        }

        if !grace_expired {
            if let Some(handle) = heartbeat_handle.as_ref() {
                handle.abort();
            }
            if let Some(handle) = signal_handle.as_ref() {
                handle.abort();
            }
        }
        while let Some(result) = join_set.join_next().await {
            log_job_task_result(result, "job task completed during final shutdown drain");
        }
        if let Some(handle) = heartbeat_handle.take() {
            let _ = handle.await;
        }
        if let Some(handle) = signal_handle.take() {
            let _ = handle.await;
        }
        let _ = self.shutdown_tx.send_replace(true);

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

fn log_job_task_result(result: Result<crate::Result<()>, tokio::task::JoinError>, message: &str) {
    match result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            tracing::error!(error = %e, "{message}");
        }
        Err(e) if e.is_cancelled() => {}
        Err(e) => {
            tracing::error!(error = %e, "{message}");
        }
    }
}
