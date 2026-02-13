use crate::errors::OjsError;
use crate::job::{
    BatchEnqueueRequest, BatchEnqueueResponse, EnqueueRequest, EnqueueResponse, Job,
};
use crate::queue::{
    CronJob, CronJobRequest, CronJobsResponse, DeadLetterResponse, HealthStatus, Manifest,
    Pagination, Queue, QueueStats, QueuesResponse,
};
use crate::transport::HttpTransport;
use crate::workflow::{
    EnqueueOption, Workflow, WorkflowDefinition,
};
use std::collections::HashMap;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Client builder
// ---------------------------------------------------------------------------

/// Builder for constructing an OJS [`Client`].
pub struct ClientBuilder {
    url: Option<String>,
    auth_token: Option<String>,
    headers: HashMap<String, String>,
    #[cfg(feature = "reqwest-transport")]
    http_client: Option<reqwest::Client>,
}

impl ClientBuilder {
    fn new() -> Self {
        Self {
            url: None,
            auth_token: None,
            headers: HashMap::new(),
            #[cfg(feature = "reqwest-transport")]
            http_client: None,
        }
    }

    /// Set the OJS server URL.
    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }

    /// Set the authentication bearer token.
    pub fn auth_token(mut self, token: impl Into<String>) -> Self {
        self.auth_token = Some(token.into());
        self
    }

    /// Add a custom HTTP header.
    pub fn header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }

    /// Provide a custom reqwest HTTP client.
    #[cfg(feature = "reqwest-transport")]
    pub fn http_client(mut self, client: reqwest::Client) -> Self {
        self.http_client = Some(client);
        self
    }

    /// Build the client.
    pub fn build(self) -> crate::Result<Client> {
        let url = self
            .url
            .ok_or_else(|| OjsError::Builder("url is required".into()))?;

        let transport = HttpTransport::new(
            &url,
            crate::transport::http::TransportConfig {
                auth_token: self.auth_token,
                headers: self.headers,
                #[cfg(feature = "reqwest-transport")]
                http_client: self.http_client,
            },
        );

        Ok(Client { transport })
    }
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// An OJS client for enqueuing jobs and managing resources.
///
/// # Example
///
/// ```rust,ignore
/// use ojs::Client;
/// use serde_json::json;
///
/// let client = Client::builder()
///     .url("http://localhost:8080")
///     .build()?;
///
/// let job = client
///     .enqueue("email.send", json!({"to": "user@example.com"}))
///     .await?;
/// ```
#[derive(Clone, Debug)]
pub struct Client {
    transport: HttpTransport,
}

impl Client {
    /// Create a new client builder.
    pub fn builder() -> ClientBuilder {
        ClientBuilder::new()
    }

    // -----------------------------------------------------------------------
    // Job operations
    // -----------------------------------------------------------------------

    /// Enqueue a job with the given type and arguments.
    ///
    /// Returns an [`EnqueueBuilder`] for configuring optional settings before
    /// sending.
    ///
    /// If no options are needed, the builder can be `.await`ed directly since
    /// it implements `IntoFuture`.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Simple enqueue (no options)
    /// let job = client.enqueue("email.send", json!({"to": "user@example.com"})).await?;
    ///
    /// // Enqueue with options
    /// let job = client.enqueue("report.generate", json!({"id": 42}))
    ///     .queue("reports")
    ///     .delay(Duration::from_secs(300))
    ///     .retry(RetryPolicy::new().max_attempts(5))
    ///     .send()
    ///     .await?;
    /// ```
    pub fn enqueue(
        &self,
        job_type: impl Into<String>,
        args: serde_json::Value,
    ) -> EnqueueBuilder {
        EnqueueBuilder {
            client: self.clone(),
            job_type: job_type.into(),
            args,
            options: Vec::new(),
            meta: None,
            schema: None,
        }
    }

    /// Enqueue multiple jobs atomically in a single request.
    pub async fn enqueue_batch(&self, requests: Vec<JobRequest>) -> crate::Result<Vec<Job>> {
        let wire_requests: Vec<EnqueueRequest> = requests
            .into_iter()
            .map(|r| {
                let args = normalize_args(r.args);
                EnqueueRequest {
                    job_type: r.job_type,
                    args,
                    meta: r.meta,
                    schema: None,
                    options: crate::workflow::resolve_options(&r.options),
                }
            })
            .collect();

        let resp: BatchEnqueueResponse = self
            .transport
            .post("/jobs/batch", &BatchEnqueueRequest { jobs: wire_requests })
            .await?;

        Ok(resp.jobs)
    }

    /// Get job details by ID.
    pub async fn get_job(&self, id: &str) -> crate::Result<Job> {
        self.transport.get(&format!("/jobs/{}", id)).await
    }

    /// Cancel a job by ID.
    pub async fn cancel_job(&self, id: &str) -> crate::Result<Job> {
        self.transport.delete(&format!("/jobs/{}", id)).await
    }

    // -----------------------------------------------------------------------
    // Workflow operations
    // -----------------------------------------------------------------------

    /// Create a workflow.
    pub async fn create_workflow(
        &self,
        def: WorkflowDefinition,
    ) -> crate::Result<Workflow> {
        let wire = def.to_wire();
        self.transport.post("/workflows", &wire).await
    }

    /// Get workflow status by ID.
    pub async fn get_workflow(&self, id: &str) -> crate::Result<Workflow> {
        self.transport.get(&format!("/workflows/{}", id)).await
    }

    /// Cancel a workflow by ID.
    pub async fn cancel_workflow(&self, id: &str) -> crate::Result<Workflow> {
        self.transport.delete(&format!("/workflows/{}", id)).await
    }

    // -----------------------------------------------------------------------
    // Queue operations
    // -----------------------------------------------------------------------

    /// List all queues.
    pub async fn list_queues(&self) -> crate::Result<Vec<Queue>> {
        let resp: QueuesResponse = self.transport.get("/queues").await?;
        Ok(resp.queues)
    }

    /// Get statistics for a specific queue.
    pub async fn get_queue_stats(&self, name: &str) -> crate::Result<QueueStats> {
        self.transport
            .get(&format!("/queues/{}/stats", name))
            .await
    }

    /// Pause a queue (stop dispatching jobs from it).
    pub async fn pause_queue(&self, name: &str) -> crate::Result<()> {
        self.transport
            .post_empty_no_response(&format!("/queues/{}/pause", name))
            .await
    }

    /// Resume a paused queue.
    pub async fn resume_queue(&self, name: &str) -> crate::Result<()> {
        self.transport
            .post_empty_no_response(&format!("/queues/{}/resume", name))
            .await
    }

    // -----------------------------------------------------------------------
    // Dead letter operations
    // -----------------------------------------------------------------------

    /// List dead letter jobs for a queue.
    pub async fn list_dead_letter_jobs(
        &self,
        queue: &str,
        limit: u64,
        offset: u64,
    ) -> crate::Result<(Vec<Job>, Option<Pagination>)> {
        let resp: DeadLetterResponse = self
            .transport
            .get(&format!(
                "/dead-letter?queue={}&limit={}&offset={}",
                queue, limit, offset
            ))
            .await?;
        Ok((resp.jobs, resp.pagination))
    }

    /// Retry a dead letter job.
    pub async fn retry_dead_letter_job(&self, id: &str) -> crate::Result<Job> {
        self.transport
            .post_empty(&format!("/dead-letter/{}/retry", id))
            .await
    }

    /// Discard a dead letter job permanently.
    pub async fn discard_dead_letter_job(&self, id: &str) -> crate::Result<()> {
        self.transport
            .delete_no_response(&format!("/dead-letter/{}", id))
            .await
    }

    // -----------------------------------------------------------------------
    // Cron operations
    // -----------------------------------------------------------------------

    /// List all registered cron jobs.
    pub async fn list_cron_jobs(&self) -> crate::Result<Vec<CronJob>> {
        let resp: CronJobsResponse = self.transport.get("/cron").await?;
        Ok(resp.cron_jobs)
    }

    /// Register a new cron job.
    pub async fn register_cron_job(&self, req: CronJobRequest) -> crate::Result<CronJob> {
        self.transport.post("/cron", &req).await
    }

    /// Unregister a cron job by name.
    pub async fn unregister_cron_job(&self, name: &str) -> crate::Result<()> {
        self.transport
            .delete_no_response(&format!("/cron/{}", name))
            .await
    }

    // -----------------------------------------------------------------------
    // Server operations
    // -----------------------------------------------------------------------

    /// Check server health.
    pub async fn health(&self) -> crate::Result<HealthStatus> {
        self.transport.get("/health").await
    }

    /// Get the server's conformance manifest.
    pub async fn manifest(&self) -> crate::Result<Manifest> {
        self.transport.get_raw("/ojs/manifest").await
    }

    // -----------------------------------------------------------------------
    // Internal: used by worker for ack/nack operations
    // -----------------------------------------------------------------------

    #[allow(dead_code)]
    pub(crate) fn transport(&self) -> &HttpTransport {
        &self.transport
    }
}

// ---------------------------------------------------------------------------
// Enqueue builder
// ---------------------------------------------------------------------------

/// A builder for configuring and sending a job enqueue request.
///
/// Created via [`Client::enqueue`]. Can be `.await`ed directly for simple
/// enqueue, or configured with chained methods and finished with `.send()`.
pub struct EnqueueBuilder {
    client: Client,
    job_type: String,
    args: serde_json::Value,
    options: Vec<EnqueueOption>,
    meta: Option<HashMap<String, serde_json::Value>>,
    schema: Option<String>,
}

impl EnqueueBuilder {
    /// Set the target queue.
    pub fn queue(mut self, queue: impl Into<String>) -> Self {
        self.options.push(EnqueueOption::Queue(queue.into()));
        self
    }

    /// Set the job priority.
    pub fn priority(mut self, priority: i32) -> Self {
        self.options.push(EnqueueOption::Priority(priority));
        self
    }

    /// Set the execution timeout.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.options.push(EnqueueOption::Timeout(timeout));
        self
    }

    /// Delay execution by the given duration.
    pub fn delay(mut self, delay: Duration) -> Self {
        self.options.push(EnqueueOption::Delay(delay));
        self
    }

    /// Schedule execution at a specific time.
    pub fn scheduled_at(mut self, at: chrono::DateTime<chrono::Utc>) -> Self {
        self.options.push(EnqueueOption::ScheduledAt(at));
        self
    }

    /// Set the job expiration time.
    pub fn expires_at(mut self, at: chrono::DateTime<chrono::Utc>) -> Self {
        self.options.push(EnqueueOption::ExpiresAt(at));
        self
    }

    /// Set the retry policy.
    pub fn retry(mut self, policy: crate::RetryPolicy) -> Self {
        self.options.push(EnqueueOption::Retry(policy));
        self
    }

    /// Set the unique/deduplication policy.
    pub fn unique(mut self, policy: crate::job::UniquePolicy) -> Self {
        self.options.push(EnqueueOption::Unique(policy));
        self
    }

    /// Add tags to the job.
    pub fn tags(mut self, tags: Vec<String>) -> Self {
        self.options.push(EnqueueOption::Tags(tags));
        self
    }

    /// Set metadata on the job.
    pub fn meta(mut self, meta: HashMap<String, serde_json::Value>) -> Self {
        self.meta = Some(meta);
        self
    }

    /// Set a schema URI for argument validation.
    pub fn schema(mut self, schema: impl Into<String>) -> Self {
        self.schema = Some(schema.into());
        self
    }

    /// Send the enqueue request.
    pub async fn send(self) -> crate::Result<Job> {
        let args = normalize_args(self.args);
        let options_wire = crate::workflow::resolve_options(&self.options);
        let meta = self
            .meta
            .or_else(|| crate::workflow::extract_meta(&self.options));

        let req = EnqueueRequest {
            job_type: self.job_type,
            args,
            meta,
            schema: self.schema,
            options: options_wire,
        };

        let resp: EnqueueResponse = self.client.transport.post("/jobs", &req).await?;
        Ok(resp.job)
    }
}

/// `IntoFuture` implementation allows `client.enqueue(type, args).await?`
/// without explicitly calling `.send()`.
impl std::future::IntoFuture for EnqueueBuilder {
    type Output = crate::Result<Job>;
    type IntoFuture = std::pin::Pin<Box<dyn std::future::Future<Output = Self::Output> + Send>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.send())
    }
}

// ---------------------------------------------------------------------------
// Job request (for batch enqueue)
// ---------------------------------------------------------------------------

/// A job request for use with [`Client::enqueue_batch`].
pub struct JobRequest {
    pub job_type: String,
    pub args: serde_json::Value,
    pub meta: Option<HashMap<String, serde_json::Value>>,
    pub options: Vec<EnqueueOption>,
}

impl JobRequest {
    /// Create a new job request.
    pub fn new(job_type: impl Into<String>, args: serde_json::Value) -> Self {
        Self {
            job_type: job_type.into(),
            args,
            meta: None,
            options: Vec::new(),
        }
    }

    /// Set metadata.
    pub fn meta(mut self, meta: HashMap<String, serde_json::Value>) -> Self {
        self.meta = Some(meta);
        self
    }

    /// Add an enqueue option.
    pub fn with_option(mut self, opt: EnqueueOption) -> Self {
        self.options.push(opt);
        self
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Normalize args into the wire format (JSON array).
fn normalize_args(args: serde_json::Value) -> serde_json::Value {
    match args {
        serde_json::Value::Array(_) => args,
        obj @ serde_json::Value::Object(_) => serde_json::Value::Array(vec![obj]),
        other => serde_json::Value::Array(vec![other]),
    }
}
