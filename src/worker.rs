use crate::errors::OjsError;
use crate::job::{
    AckRequest, FetchRequest, FetchResponse, HeartbeatRequest, HeartbeatResponse, Job, NackError,
    NackRequest,
};
use crate::middleware::{BoxFuture, HandlerFn, HandlerResult, Middleware, MiddlewareChain};
use crate::transport::{self, DynTransport, HttpTransport};
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::task::JoinSet;

// ---------------------------------------------------------------------------
// Worker state
// ---------------------------------------------------------------------------

/// The lifecycle state of a worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum WorkerState {
    /// Normal operation, actively fetching and processing jobs.
    Running = 0,
    /// No longer fetching new jobs, finishing active ones.
    Quiet = 1,
    /// Shutting down.
    Terminate = 2,
}

impl WorkerState {
    fn from_u8(v: u8) -> Self {
        match v {
            0 => WorkerState::Running,
            1 => WorkerState::Quiet,
            _ => WorkerState::Terminate,
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            WorkerState::Running => "running",
            WorkerState::Quiet => "quiet",
            WorkerState::Terminate => "terminate",
        }
    }
}

impl std::fmt::Display for WorkerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Job context
// ---------------------------------------------------------------------------

/// Context passed to job handler functions.
///
/// Contains the job being processed, metadata about the execution attempt,
/// and methods for interacting with the worker (e.g., heartbeat, set result).
#[derive(Clone)]
pub struct JobContext {
    /// The job being processed.
    pub job: Job,
    /// Current attempt number (1-indexed).
    pub attempt: u32,
    /// Queue the job was fetched from.
    pub queue: String,
    /// Workflow ID if this job is part of a workflow.
    pub workflow_id: Option<String>,
    /// Results from upstream workflow steps.
    pub parent_results: Option<HashMap<String, serde_json::Value>>,

    // Internal
    transport: DynTransport,
    worker_id: String,
}

impl JobContext {
    /// Send a heartbeat to extend the visibility timeout for this job.
    ///
    /// Call this periodically in long-running handlers to prevent the job
    /// from being considered stale and re-dispatched.
    pub async fn heartbeat(&self) -> crate::Result<()> {
        let req = HeartbeatRequest {
            worker_id: self.worker_id.clone(),
            active_jobs: Some(vec![self.job.id.clone()]),
            visibility_timeout_ms: None,
        };
        let _: HeartbeatResponse =
            transport::transport_post(&self.transport, "/workers/heartbeat", &req).await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Worker builder
// ---------------------------------------------------------------------------

/// Builder for constructing an OJS [`Worker`].
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
            #[cfg(feature = "reqwest-transport")]
            http_client: None,
        }
    }

    /// Set the OJS server URL.
    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
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
    pub fn http_client(mut self, client: reqwest::Client) -> Self {
        self.http_client = Some(client);
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
            active_jobs: Arc::new(RwLock::new(Vec::new())),
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
/// ```rust,ignore
/// use ojs::{Worker, JobContext};
/// use serde_json::json;
///
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
    active_jobs: Arc<RwLock<Vec<String>>>,
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

    /// Add middleware to the worker.
    ///
    /// Middleware wraps all job handlers and executes in registration order
    /// (first registered = outermost wrapper).
    pub async fn use_middleware(&self, name: impl Into<String>, mw: impl Middleware) {
        self.middleware.write().await.add(name, mw);
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
            let _active_count = self.active_count.clone();
            let mut shutdown_rx = shutdown_rx.clone();

            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(interval);
                ticker.tick().await; // skip first immediate tick

                loop {
                    tokio::select! {
                        _ = ticker.tick() => {
                            let jobs = active_jobs.read().await.clone();
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
                        let permit = semaphore.clone().acquire_owned().await.unwrap();
                        let job_id = job.id.clone();

                        // Track active job
                        self.active_count.fetch_add(1, Ordering::SeqCst);
                        self.active_jobs.write().await.push(job_id.clone());

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
                                jobs.retain(|id| id != &job_id);
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
// Job processing
// ---------------------------------------------------------------------------

async fn process_job(
    transport: &DynTransport,
    worker_id: &str,
    handlers: &Arc<RwLock<HashMap<String, HandlerFn>>>,
    middleware: &Arc<RwLock<MiddlewareChain>>,
    job: Job,
) -> crate::Result<()> {
    let job_type = job.job_type.clone();
    let job_id = job.id.clone();

    tracing::debug!(
        job_id = %job_id,
        job_type = %job_type,
        attempt = job.attempt,
        "processing job"
    );

    // Look up handler
    let handler: Option<HandlerFn> = {
        let handlers = handlers.read().await;
        handlers.get(&job_type).cloned()
    };

    let handler = match handler {
        Some(h) => h,
        None => {
            tracing::error!(job_type = %job_type, "no handler registered");
            nack_job(
                transport,
                &job_id,
                "handler_error",
                &format!("no handler registered for job type: {}", job_type),
                false,
            )
            .await?;
            return Ok(());
        }
    };

    // Wrap handler with middleware
    let wrapped = {
        let mw = middleware.read().await;
        mw.wrap(handler)
    };

    // Build job context
    let ctx = JobContext {
        attempt: job.attempt.max(1),
        queue: job.queue.clone(),
        workflow_id: job
            .meta
            .as_ref()
            .and_then(|m| m.get("workflow_id"))
            .and_then(|v| v.as_str())
            .map(String::from),
        parent_results: None,
        transport: transport.clone(),
        worker_id: worker_id.to_string(),
        job,
    };

    // Execute
    match wrapped(ctx).await {
        Ok(result) => {
            tracing::debug!(job_id = %job_id, "job completed successfully");
            ack_job(transport, &job_id, result).await?;
        }
        Err(e) => {
            tracing::warn!(job_id = %job_id, error = %e, "job failed");
            let (code, message, retryable) = match &e {
                OjsError::NonRetryable(msg) => ("handler_error".to_string(), msg.clone(), false),
                OjsError::Handler(msg) => ("handler_error".to_string(), msg.clone(), true),
                other => ("handler_error".to_string(), other.to_string(), true),
            };
            nack_job(transport, &job_id, &code, &message, retryable).await?;
        }
    }

    Ok(())
}

async fn ack_job(
    transport: &DynTransport,
    job_id: &str,
    result: serde_json::Value,
) -> crate::Result<()> {
    let req = AckRequest {
        job_id: job_id.to_string(),
        result: if result.is_null() { None } else { Some(result) },
    };
    transport::transport_post_no_response(transport, "/workers/ack", &req).await
}

async fn nack_job(
    transport: &DynTransport,
    job_id: &str,
    code: &str,
    message: &str,
    retryable: bool,
) -> crate::Result<()> {
    let req = NackRequest {
        job_id: job_id.to_string(),
        error: NackError {
            code: code.to_string(),
            message: message.to_string(),
            retryable: Some(retryable),
            details: None,
        },
    };
    transport::transport_post_no_response(transport, "/workers/nack", &req).await
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn generate_worker_id() -> String {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("worker_{}_{}", pid, nanos)
}
