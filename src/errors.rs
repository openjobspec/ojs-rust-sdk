use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

// ---------------------------------------------------------------------------
// Error codes (from OJS HTTP spec)
// ---------------------------------------------------------------------------

pub const ERR_HANDLER_ERROR: &str = "handler_error";
pub const ERR_TIMEOUT: &str = "timeout";
pub const ERR_CANCELLED: &str = "cancelled";
pub const ERR_INVALID_PAYLOAD: &str = "invalid_payload";
pub const ERR_INVALID_REQUEST: &str = "invalid_request";
pub const ERR_NOT_FOUND: &str = "not_found";
pub const ERR_BACKEND_ERROR: &str = "backend_error";
pub const ERR_RATE_LIMITED: &str = "rate_limited";
pub const ERR_DUPLICATE: &str = "duplicate";
pub const ERR_QUEUE_PAUSED: &str = "queue_paused";
pub const ERR_SCHEMA_VALIDATION: &str = "schema_validation";
pub const ERR_UNSUPPORTED: &str = "unsupported";
pub const ERR_ENVELOPE_TOO_LARGE: &str = "envelope_too_large";

// ---------------------------------------------------------------------------
// Main SDK error type
// ---------------------------------------------------------------------------

#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum OjsError {
    /// An error returned by the OJS server.
    #[error("{0}")]
    Server(Box<ServerError>),

    /// HTTP transport error.
    #[error("transport error: {0}")]
    Transport(String),

    /// Serialization / deserialization failure.
    #[error("serialization error: {0}")]
    Serialization(String),

    /// The handler returned a retryable error while processing a job.
    #[error("handler error: {0}")]
    Handler(String),

    /// The handler returned a non-retryable error. The job will not be retried.
    #[error("non-retryable error: {0}")]
    NonRetryable(String),

    /// Builder misconfiguration.
    #[error("builder error: {0}")]
    Builder(String),

    /// No handler registered for the given job type.
    #[error("no handler registered for job type: {0}")]
    NoHandler(String),

    /// Worker has been shut down.
    #[error("worker shut down")]
    WorkerShutdown,
}

impl From<ServerError> for OjsError {
    fn from(err: ServerError) -> Self {
        OjsError::Server(Box::new(err))
    }
}

#[cfg(feature = "reqwest-transport")]
impl From<reqwest::Error> for OjsError {
    fn from(err: reqwest::Error) -> Self {
        OjsError::Transport(err.to_string())
    }
}

impl From<serde_json::Error> for OjsError {
    fn from(err: serde_json::Error) -> Self {
        OjsError::Serialization(err.to_string())
    }
}

// ---------------------------------------------------------------------------
// Server error (structured error from OJS backend)
// ---------------------------------------------------------------------------

#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerError {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<HashMap<String, serde_json::Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(skip)]
    pub http_status: u16,
    /// Duration to wait before retrying, extracted from the `Retry-After`
    /// response header. `None` if the header was absent or invalid.
    #[serde(skip)]
    pub retry_after: Option<std::time::Duration>,
    /// Rate limit metadata from response headers, if present.
    #[serde(skip)]
    pub rate_limit: Option<RateLimitInfo>,
}

impl fmt::Display for ServerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for ServerError {}

impl ServerError {
    /// Create a new server error.
    pub fn new(code: impl Into<String>, message: impl Into<String>, http_status: u16) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable: false,
            details: None,
            request_id: None,
            http_status,
            retry_after: None,
            rate_limit: None,
        }
    }

    /// Set whether this error is retryable.
    pub fn retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }

    /// Returns `true` if this error indicates the operation can be retried.
    pub fn is_retryable(&self) -> bool {
        self.retryable
    }

    /// Returns the machine-readable error code.
    pub fn code(&self) -> &str {
        &self.code
    }

    /// Returns `true` if this is a "not found" error.
    pub fn is_not_found(&self) -> bool {
        self.code == ERR_NOT_FOUND
    }

    /// Returns `true` if this is a "duplicate" error.
    pub fn is_duplicate(&self) -> bool {
        self.code == ERR_DUPLICATE
    }

    /// Returns `true` if this is a "rate limited" error.
    pub fn is_rate_limited(&self) -> bool {
        self.code == ERR_RATE_LIMITED
    }

    /// Returns `true` if this is a "queue paused" error.
    pub fn is_queue_paused(&self) -> bool {
        self.code == ERR_QUEUE_PAUSED
    }

    /// Returns the retry-after duration, if the server provided one.
    pub fn retry_after(&self) -> Option<std::time::Duration> {
        self.retry_after
    }

    /// Returns the rate limit info, if the server provided rate limit headers.
    pub fn rate_limit(&self) -> Option<&RateLimitInfo> {
        self.rate_limit.as_ref()
    }
}

// ---------------------------------------------------------------------------
// Rate limit info
// ---------------------------------------------------------------------------

/// Rate limit metadata extracted from response headers.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateLimitInfo {
    /// Maximum requests allowed per window (`X-RateLimit-Limit`).
    pub limit: Option<i64>,
    /// Remaining requests in current window (`X-RateLimit-Remaining`).
    pub remaining: Option<i64>,
    /// Unix timestamp when window resets (`X-RateLimit-Reset`).
    pub reset: Option<i64>,
    /// Duration to wait before retrying (`Retry-After`).
    pub retry_after: Option<std::time::Duration>,
}

// ---------------------------------------------------------------------------
// Wire format for parsing server error responses
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct ErrorResponse {
    pub error: ServerErrorPayload,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ServerErrorPayload {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub retryable: bool,
    #[serde(default)]
    pub details: Option<HashMap<String, serde_json::Value>>,
    #[serde(default)]
    pub request_id: Option<String>,
}

impl ServerErrorPayload {
    pub fn into_server_error(self, http_status: u16) -> ServerError {
        ServerError {
            code: self.code,
            message: self.message,
            retryable: self.retryable,
            details: self.details,
            request_id: self.request_id,
            http_status,
            retry_after: None,
            rate_limit: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Job-level error (attached to failed jobs)
// ---------------------------------------------------------------------------

#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobError {
    /// Error type / class name.
    #[serde(rename = "type")]
    pub error_type: String,
    /// Human-readable error description.
    pub message: String,
    /// Optional stack trace frames (max 50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backtrace: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------
// Result type alias
// ---------------------------------------------------------------------------

pub type Result<T> = std::result::Result<T, OjsError>;
