use crate::errors::OjsError;
use crate::job::{BatchEnqueueRequest, BatchEnqueueResponse, EnqueueRequest, EnqueueResponse, Job};
use crate::queue::{
    CronJob, CronJobRequest, CronJobsResponse, DeadLetterResponse, HealthStatus, Manifest,
    Pagination, Queue, QueueStats, QueuesResponse,
};
use crate::schema::{RegisterSchemaRequest, Schema, SchemaDetail, SchemasResponse};
use crate::transport::{self, DynTransport, HttpTransport};
use crate::workflow::{EnqueueOption, Workflow, WorkflowDefinition};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// Percent-encode a string for use in URL path segments or query values.
fn url_encode(s: &str) -> String {
    utf8_percent_encode(s, NON_ALPHANUMERIC).to_string()
}

// ---------------------------------------------------------------------------
// Client builder
// ---------------------------------------------------------------------------

/// Builder for constructing an OJS [`Client`].
#[must_use = "builders do nothing until `.build()` is called"]
pub struct ClientBuilder {
    url: Option<String>,
    auth_token: Option<String>,
    headers: HashMap<String, String>,
    timeout: Option<Duration>,
    retry_config: Option<crate::rate_limiter::RetryConfig>,
    #[cfg(feature = "reqwest-transport")]
    http_client: Option<reqwest::Client>,
}

impl ClientBuilder {
    fn new() -> Self {
        Self {
            url: None,
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
    /// By default, the client retries up to 3 times on `429 Too Many Requests`
    /// responses with exponential backoff. Use [`RetryConfig::disabled()`] to
    /// turn off automatic retries.
    pub fn retry_config(mut self, config: crate::rate_limiter::RetryConfig) -> Self {
        self.retry_config = Some(config);
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
                timeout: self.timeout,
                retry_config: self.retry_config,
                #[cfg(feature = "reqwest-transport")]
                http_client: self.http_client,
            },
        );

        Ok(Client {
            transport: Arc::new(transport),
        })
    }
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// An OJS client for enqueuing jobs and managing resources.
///
/// # Example
///
/// ```rust,no_run
/// use ojs::Client;
/// use serde_json::json;
///
/// # #[tokio::main]
/// # async fn main() -> ojs::Result<()> {
/// let client = Client::builder()
///     .url("http://localhost:8080")
///     .build()?;
///
/// let job = client
///     .enqueue("email.send", json!({"to": "user@example.com"}))
///     .await?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Debug)]
pub struct Client {
    transport: DynTransport,
}

impl Client {
    /// Create a new client builder.
    pub fn builder() -> ClientBuilder {
        ClientBuilder::new()
    }

    /// Create a client with a custom transport (for testing or alternative backends).
    pub fn with_transport(transport: DynTransport) -> Self {
        Self { transport }
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
    /// ```rust,no_run
    /// use ojs::{Client, RetryPolicy};
    /// use serde_json::json;
    /// use std::time::Duration;
    ///
    /// # #[tokio::main]
    /// # async fn main() -> ojs::Result<()> {
    /// # let client = Client::builder().url("http://localhost:8080").build()?;
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
    /// # Ok(())
    /// # }
    /// ```
    pub fn enqueue(&self, job_type: impl Into<String>, args: serde_json::Value) -> EnqueueBuilder {
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
                let args = crate::workflow::normalize_args(&r.args);
                EnqueueRequest {
                    job_type: r.job_type,
                    args,
                    meta: r.meta,
                    schema: None,
                    options: crate::workflow::resolve_options(&r.options),
                }
            })
            .collect();

        let resp: BatchEnqueueResponse = transport::transport_post(
            &self.transport,
            "/jobs/batch",
            &BatchEnqueueRequest {
                jobs: wire_requests,
            },
        )
        .await?;

        Ok(resp.jobs)
    }

    /// Get job details by ID.
    pub async fn get_job(&self, id: &str) -> crate::Result<Job> {
        transport::transport_get(&self.transport, &format!("/jobs/{}", id)).await
    }

    /// Cancel a job by ID.
    pub async fn cancel_job(&self, id: &str) -> crate::Result<Job> {
        transport::transport_delete(&self.transport, &format!("/jobs/{}", id)).await
    }

    // -----------------------------------------------------------------------
    // Workflow operations
    // -----------------------------------------------------------------------

    /// Create a workflow.
    pub async fn create_workflow(&self, def: WorkflowDefinition) -> crate::Result<Workflow> {
        let wire = def.to_wire();
        transport::transport_post(&self.transport, "/workflows", &wire).await
    }

    /// Get workflow status by ID.
    pub async fn get_workflow(&self, id: &str) -> crate::Result<Workflow> {
        transport::transport_get(&self.transport, &format!("/workflows/{}", id)).await
    }

    /// Cancel a workflow by ID.
    pub async fn cancel_workflow(&self, id: &str) -> crate::Result<Workflow> {
        transport::transport_delete(&self.transport, &format!("/workflows/{}", id)).await
    }

    // -----------------------------------------------------------------------
    // Queue operations
    // -----------------------------------------------------------------------

    /// List all queues.
    pub async fn list_queues(&self) -> crate::Result<Vec<Queue>> {
        let resp: QueuesResponse = transport::transport_get(&self.transport, "/queues").await?;
        Ok(resp.queues)
    }

    /// Get statistics for a specific queue.
    pub async fn get_queue_stats(&self, name: &str) -> crate::Result<QueueStats> {
        let name = url_encode(name);
        transport::transport_get(&self.transport, &format!("/queues/{}/stats", name)).await
    }

    /// Pause a queue (stop dispatching jobs from it).
    pub async fn pause_queue(&self, name: &str) -> crate::Result<()> {
        let name = url_encode(name);
        transport::transport_post_empty_no_response(
            &self.transport,
            &format!("/queues/{}/pause", name),
        )
        .await
    }

    /// Resume a paused queue.
    pub async fn resume_queue(&self, name: &str) -> crate::Result<()> {
        let name = url_encode(name);
        transport::transport_post_empty_no_response(
            &self.transport,
            &format!("/queues/{}/resume", name),
        )
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
        let resp: DeadLetterResponse = transport::transport_get(
            &self.transport,
            &format!(
                "/dead-letter?queue={}&limit={}&offset={}",
                url_encode(queue),
                limit,
                offset
            ),
        )
        .await?;
        Ok((resp.jobs, resp.pagination))
    }

    /// Retry a dead letter job.
    pub async fn retry_dead_letter_job(&self, id: &str) -> crate::Result<Job> {
        transport::transport_post_empty(&self.transport, &format!("/dead-letter/{}/retry", id))
            .await
    }

    /// Discard a dead letter job permanently.
    pub async fn discard_dead_letter_job(&self, id: &str) -> crate::Result<()> {
        transport::transport_delete_no_response(&self.transport, &format!("/dead-letter/{}", id))
            .await
    }

    // -----------------------------------------------------------------------
    // Cron operations
    // -----------------------------------------------------------------------

    /// List all registered cron jobs.
    pub async fn list_cron_jobs(&self) -> crate::Result<Vec<CronJob>> {
        let resp: CronJobsResponse = transport::transport_get(&self.transport, "/cron").await?;
        Ok(resp.cron_jobs)
    }

    /// Register a new cron job.
    pub async fn register_cron_job(&self, req: CronJobRequest) -> crate::Result<CronJob> {
        transport::transport_post(&self.transport, "/cron", &req).await
    }

    /// Unregister a cron job by name.
    pub async fn unregister_cron_job(&self, name: &str) -> crate::Result<()> {
        let name = url_encode(name);
        transport::transport_delete_no_response(&self.transport, &format!("/cron/{}", name)).await
    }

    // -----------------------------------------------------------------------
    // Schema operations
    // -----------------------------------------------------------------------

    /// List all registered schemas.
    pub async fn list_schemas(&self) -> crate::Result<Vec<Schema>> {
        let resp: SchemasResponse = transport::transport_get(&self.transport, "/schemas").await?;
        Ok(resp.schemas)
    }

    /// Register a new schema.
    pub async fn register_schema(&self, req: RegisterSchemaRequest) -> crate::Result<SchemaDetail> {
        transport::transport_post(&self.transport, "/schemas", &req).await
    }

    /// Get a schema by URI.
    pub async fn get_schema(&self, uri: &str) -> crate::Result<SchemaDetail> {
        let uri = url_encode(uri);
        transport::transport_get(&self.transport, &format!("/schemas/{}", uri)).await
    }

    /// Delete a schema by URI.
    pub async fn delete_schema(&self, uri: &str) -> crate::Result<()> {
        let uri = url_encode(uri);
        transport::transport_delete_no_response(&self.transport, &format!("/schemas/{}", uri)).await
    }

    // -----------------------------------------------------------------------
    // Server operations
    // -----------------------------------------------------------------------

    /// Check server health.
    pub async fn health(&self) -> crate::Result<HealthStatus> {
        transport::transport_get(&self.transport, "/health").await
    }

    /// Get the server's conformance manifest.
    pub async fn manifest(&self) -> crate::Result<Manifest> {
        transport::transport_get_raw(&self.transport, "/ojs/manifest").await
    }

    // -----------------------------------------------------------------------
    // Transport access
    // -----------------------------------------------------------------------

    /// Get a reference to the underlying transport handle.
    pub fn transport(&self) -> &DynTransport {
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
#[must_use = "enqueue builders do nothing until `.send()` is called or `.await`ed"]
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
        let args = crate::workflow::normalize_args(&self.args);
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

        let resp: EnqueueResponse =
            transport::transport_post(&self.client.transport, "/jobs", &req).await?;
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
