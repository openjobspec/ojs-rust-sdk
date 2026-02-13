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

    /// The handler returned an error while processing a job.
    #[error("handler error: {0}")]
    Handler(String),

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
}

impl fmt::Display for ServerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for ServerError {}

impl ServerError {
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
        }
    }
}

// ---------------------------------------------------------------------------
// Job-level error (attached to failed jobs)
// ---------------------------------------------------------------------------

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
