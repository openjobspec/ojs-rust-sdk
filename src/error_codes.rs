//! Standardized OJS error codes as defined in the OJS SDK Error Catalog
//! (`spec/ojs-error-catalog.md`). Each code maps to a canonical wire-format
//! string code from the OJS Error Specification.

use std::fmt;

/// A single entry in the OJS error catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ErrorCodeEntry {
    /// The OJS-XXXX numeric identifier (e.g., "OJS-1000").
    pub code: &'static str,
    /// Human-readable error name (e.g., "InvalidPayload").
    pub name: &'static str,
    /// SCREAMING_SNAKE_CASE wire-format code, or empty for client-side errors.
    pub canonical_code: &'static str,
    /// Default HTTP status code, or 0 for client-side errors.
    pub http_status: u16,
    /// Default human-readable description.
    pub message: &'static str,
    /// Default retryability.
    pub retryable: bool,
}

impl fmt::Display for ErrorCodeEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}: {}", self.code, self.name, self.message)
    }
}

// ---------------------------------------------------------------------------
// OJS-1xxx: Client Errors
// ---------------------------------------------------------------------------

pub const OJS_1000_INVALID_PAYLOAD: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-1000", name: "InvalidPayload", canonical_code: "INVALID_PAYLOAD", http_status: 400, message: "Job envelope fails structural validation", retryable: false };
pub const OJS_1001_INVALID_JOB_TYPE: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-1001", name: "InvalidJobType", canonical_code: "INVALID_JOB_TYPE", http_status: 400, message: "Job type is not registered or does not match the allowlist", retryable: false };
pub const OJS_1002_INVALID_QUEUE: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-1002", name: "InvalidQueue", canonical_code: "INVALID_QUEUE", http_status: 400, message: "Queue name is invalid or does not match naming rules", retryable: false };
pub const OJS_1003_INVALID_ARGS: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-1003", name: "InvalidArgs", canonical_code: "INVALID_ARGS", http_status: 400, message: "Job args fail type checking or schema validation", retryable: false };
pub const OJS_1004_INVALID_METADATA: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-1004", name: "InvalidMetadata", canonical_code: "INVALID_METADATA", http_status: 400, message: "Metadata field is malformed or exceeds the 64 KB size limit", retryable: false };
pub const OJS_1005_INVALID_STATE_TRANSITION: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-1005", name: "InvalidStateTransition", canonical_code: "INVALID_STATE_TRANSITION", http_status: 409, message: "Attempted an invalid lifecycle state change", retryable: false };
pub const OJS_1006_INVALID_RETRY_POLICY: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-1006", name: "InvalidRetryPolicy", canonical_code: "INVALID_RETRY_POLICY", http_status: 400, message: "Retry policy configuration is invalid", retryable: false };
pub const OJS_1007_INVALID_CRON_EXPRESSION: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-1007", name: "InvalidCronExpression", canonical_code: "INVALID_CRON_EXPRESSION", http_status: 400, message: "Cron expression syntax cannot be parsed", retryable: false };
pub const OJS_1008_SCHEMA_VALIDATION_FAILED: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-1008", name: "SchemaValidationFailed", canonical_code: "SCHEMA_VALIDATION_FAILED", http_status: 422, message: "Job args do not conform to the registered schema", retryable: false };
pub const OJS_1009_PAYLOAD_TOO_LARGE: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-1009", name: "PayloadTooLarge", canonical_code: "PAYLOAD_TOO_LARGE", http_status: 413, message: "Job envelope exceeds the server's maximum payload size", retryable: false };
pub const OJS_1010_METADATA_TOO_LARGE: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-1010", name: "MetadataTooLarge", canonical_code: "METADATA_TOO_LARGE", http_status: 413, message: "Metadata field exceeds the 64 KB limit", retryable: false };
pub const OJS_1011_CONNECTION_ERROR: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-1011", name: "ConnectionError", canonical_code: "", http_status: 0, message: "Could not establish a connection to the OJS server", retryable: true };
pub const OJS_1012_REQUEST_TIMEOUT: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-1012", name: "RequestTimeout", canonical_code: "", http_status: 0, message: "HTTP request to the OJS server timed out", retryable: true };
pub const OJS_1013_SERIALIZATION_ERROR: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-1013", name: "SerializationError", canonical_code: "", http_status: 0, message: "Failed to serialize the request or deserialize the response", retryable: false };
pub const OJS_1014_QUEUE_NAME_TOO_LONG: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-1014", name: "QueueNameTooLong", canonical_code: "QUEUE_NAME_TOO_LONG", http_status: 400, message: "Queue name exceeds the 255-byte maximum length", retryable: false };
pub const OJS_1015_JOB_TYPE_TOO_LONG: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-1015", name: "JobTypeTooLong", canonical_code: "JOB_TYPE_TOO_LONG", http_status: 400, message: "Job type exceeds the 255-byte maximum length", retryable: false };
pub const OJS_1016_CHECKSUM_MISMATCH: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-1016", name: "ChecksumMismatch", canonical_code: "CHECKSUM_MISMATCH", http_status: 400, message: "External payload reference checksum verification failed", retryable: false };
pub const OJS_1017_UNSUPPORTED_COMPRESSION: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-1017", name: "UnsupportedCompression", canonical_code: "UNSUPPORTED_COMPRESSION", http_status: 400, message: "The specified compression codec is not supported", retryable: false };

// ---------------------------------------------------------------------------
// OJS-2xxx: Server Errors
// ---------------------------------------------------------------------------

pub const OJS_2000_BACKEND_ERROR: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-2000", name: "BackendError", canonical_code: "BACKEND_ERROR", http_status: 500, message: "Internal backend storage or transport failure", retryable: true };
pub const OJS_2001_BACKEND_UNAVAILABLE: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-2001", name: "BackendUnavailable", canonical_code: "BACKEND_UNAVAILABLE", http_status: 503, message: "Backend storage system is unreachable", retryable: true };
pub const OJS_2002_BACKEND_TIMEOUT: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-2002", name: "BackendTimeout", canonical_code: "BACKEND_TIMEOUT", http_status: 504, message: "Backend operation timed out", retryable: true };
pub const OJS_2003_REPLICATION_LAG: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-2003", name: "ReplicationLag", canonical_code: "REPLICATION_LAG", http_status: 500, message: "Operation failed due to replication consistency issue", retryable: true };
pub const OJS_2004_INTERNAL_SERVER_ERROR: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-2004", name: "InternalServerError", canonical_code: "", http_status: 500, message: "Unclassified internal server error", retryable: true };

// ---------------------------------------------------------------------------
// OJS-3xxx: Job Lifecycle Errors
// ---------------------------------------------------------------------------

pub const OJS_3000_JOB_NOT_FOUND: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-3000", name: "JobNotFound", canonical_code: "NOT_FOUND", http_status: 404, message: "The requested job, queue, or resource does not exist", retryable: false };
pub const OJS_3001_DUPLICATE_JOB: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-3001", name: "DuplicateJob", canonical_code: "DUPLICATE_JOB", http_status: 409, message: "Unique job constraint was violated", retryable: false };
pub const OJS_3002_JOB_ALREADY_COMPLETED: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-3002", name: "JobAlreadyCompleted", canonical_code: "JOB_ALREADY_COMPLETED", http_status: 409, message: "Operation attempted on a job that has already completed", retryable: false };
pub const OJS_3003_JOB_ALREADY_CANCELLED: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-3003", name: "JobAlreadyCancelled", canonical_code: "JOB_ALREADY_CANCELLED", http_status: 409, message: "Operation attempted on a job that has already been cancelled", retryable: false };
pub const OJS_3004_QUEUE_PAUSED: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-3004", name: "QueuePaused", canonical_code: "QUEUE_PAUSED", http_status: 422, message: "The target queue is paused and not accepting new jobs", retryable: true };
pub const OJS_3005_HANDLER_ERROR: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-3005", name: "HandlerError", canonical_code: "HANDLER_ERROR", http_status: 0, message: "Job handler threw an exception during execution", retryable: true };
pub const OJS_3006_HANDLER_TIMEOUT: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-3006", name: "HandlerTimeout", canonical_code: "HANDLER_TIMEOUT", http_status: 0, message: "Job handler exceeded the configured execution timeout", retryable: true };
pub const OJS_3007_HANDLER_PANIC: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-3007", name: "HandlerPanic", canonical_code: "HANDLER_PANIC", http_status: 0, message: "Job handler caused an unrecoverable error", retryable: true };
pub const OJS_3008_NON_RETRYABLE_ERROR: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-3008", name: "NonRetryableError", canonical_code: "NON_RETRYABLE_ERROR", http_status: 0, message: "Error type matched non_retryable_errors in the retry policy", retryable: false };
pub const OJS_3009_JOB_CANCELLED: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-3009", name: "JobCancelled", canonical_code: "JOB_CANCELLED", http_status: 0, message: "Job was cancelled while it was executing", retryable: false };
pub const OJS_3010_NO_HANDLER_REGISTERED: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-3010", name: "NoHandlerRegistered", canonical_code: "", http_status: 0, message: "No handler is registered for the received job type", retryable: false };

// ---------------------------------------------------------------------------
// OJS-4xxx: Workflow Errors
// ---------------------------------------------------------------------------

pub const OJS_4000_WORKFLOW_NOT_FOUND: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-4000", name: "WorkflowNotFound", canonical_code: "", http_status: 404, message: "The specified workflow does not exist", retryable: false };
pub const OJS_4001_CHAIN_STEP_FAILED: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-4001", name: "ChainStepFailed", canonical_code: "", http_status: 422, message: "A step in a chain workflow failed, halting subsequent steps", retryable: false };
pub const OJS_4002_GROUP_TIMEOUT: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-4002", name: "GroupTimeout", canonical_code: "", http_status: 504, message: "A group workflow did not complete within the allowed timeout", retryable: true };
pub const OJS_4003_DEPENDENCY_FAILED: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-4003", name: "DependencyFailed", canonical_code: "", http_status: 422, message: "A required dependency job failed, preventing execution", retryable: false };
pub const OJS_4004_CYCLIC_DEPENDENCY: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-4004", name: "CyclicDependency", canonical_code: "", http_status: 400, message: "The workflow definition contains circular dependencies", retryable: false };
pub const OJS_4005_BATCH_CALLBACK_FAILED: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-4005", name: "BatchCallbackFailed", canonical_code: "", http_status: 422, message: "The batch completion callback job failed", retryable: true };
pub const OJS_4006_WORKFLOW_CANCELLED: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-4006", name: "WorkflowCancelled", canonical_code: "", http_status: 409, message: "The entire workflow was cancelled", retryable: false };

// ---------------------------------------------------------------------------
// OJS-5xxx: Authentication & Authorization Errors
// ---------------------------------------------------------------------------

pub const OJS_5000_UNAUTHENTICATED: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-5000", name: "Unauthenticated", canonical_code: "UNAUTHENTICATED", http_status: 401, message: "No authentication credentials provided or credentials are invalid", retryable: false };
pub const OJS_5001_PERMISSION_DENIED: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-5001", name: "PermissionDenied", canonical_code: "PERMISSION_DENIED", http_status: 403, message: "Authenticated but lacks the required permission", retryable: false };
pub const OJS_5002_TOKEN_EXPIRED: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-5002", name: "TokenExpired", canonical_code: "TOKEN_EXPIRED", http_status: 401, message: "The authentication token has expired", retryable: false };
pub const OJS_5003_TENANT_ACCESS_DENIED: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-5003", name: "TenantAccessDenied", canonical_code: "TENANT_ACCESS_DENIED", http_status: 403, message: "Operation on a tenant the caller does not have access to", retryable: false };

// ---------------------------------------------------------------------------
// OJS-6xxx: Rate Limiting & Backpressure Errors
// ---------------------------------------------------------------------------

pub const OJS_6000_RATE_LIMITED: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-6000", name: "RateLimited", canonical_code: "RATE_LIMITED", http_status: 429, message: "Rate limit exceeded", retryable: true };
pub const OJS_6001_QUEUE_FULL: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-6001", name: "QueueFull", canonical_code: "QUEUE_FULL", http_status: 429, message: "The queue has reached its configured maximum depth", retryable: true };
pub const OJS_6002_CONCURRENCY_LIMITED: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-6002", name: "ConcurrencyLimited", canonical_code: "", http_status: 429, message: "The concurrency limit has been reached", retryable: true };
pub const OJS_6003_BACKPRESSURE_APPLIED: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-6003", name: "BackpressureApplied", canonical_code: "", http_status: 429, message: "The server is applying backpressure", retryable: true };

// ---------------------------------------------------------------------------
// OJS-7xxx: Extension Errors
// ---------------------------------------------------------------------------

pub const OJS_7000_UNSUPPORTED_FEATURE: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-7000", name: "UnsupportedFeature", canonical_code: "UNSUPPORTED_FEATURE", http_status: 422, message: "Feature requires a conformance level the backend does not support", retryable: false };
pub const OJS_7001_CRON_SCHEDULE_CONFLICT: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-7001", name: "CronScheduleConflict", canonical_code: "", http_status: 409, message: "The cron schedule conflicts with an existing schedule", retryable: false };
pub const OJS_7002_UNIQUE_KEY_INVALID: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-7002", name: "UniqueKeyInvalid", canonical_code: "", http_status: 400, message: "The unique key specification is invalid or malformed", retryable: false };
pub const OJS_7003_MIDDLEWARE_ERROR: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-7003", name: "MiddlewareError", canonical_code: "", http_status: 500, message: "An error occurred in the middleware chain", retryable: true };
pub const OJS_7004_MIDDLEWARE_TIMEOUT: ErrorCodeEntry = ErrorCodeEntry { code: "OJS-7004", name: "MiddlewareTimeout", canonical_code: "", http_status: 504, message: "A middleware handler exceeded its allowed execution time", retryable: true };

// ---------------------------------------------------------------------------
// Catalog utilities
// ---------------------------------------------------------------------------

/// All defined OJS error catalog entries.
pub const ALL_ERROR_CODES: &[ErrorCodeEntry] = &[
    // OJS-1xxx
    OJS_1000_INVALID_PAYLOAD, OJS_1001_INVALID_JOB_TYPE, OJS_1002_INVALID_QUEUE,
    OJS_1003_INVALID_ARGS, OJS_1004_INVALID_METADATA, OJS_1005_INVALID_STATE_TRANSITION,
    OJS_1006_INVALID_RETRY_POLICY, OJS_1007_INVALID_CRON_EXPRESSION,
    OJS_1008_SCHEMA_VALIDATION_FAILED, OJS_1009_PAYLOAD_TOO_LARGE,
    OJS_1010_METADATA_TOO_LARGE, OJS_1011_CONNECTION_ERROR, OJS_1012_REQUEST_TIMEOUT,
    OJS_1013_SERIALIZATION_ERROR, OJS_1014_QUEUE_NAME_TOO_LONG, OJS_1015_JOB_TYPE_TOO_LONG,
    OJS_1016_CHECKSUM_MISMATCH, OJS_1017_UNSUPPORTED_COMPRESSION,
    // OJS-2xxx
    OJS_2000_BACKEND_ERROR, OJS_2001_BACKEND_UNAVAILABLE, OJS_2002_BACKEND_TIMEOUT,
    OJS_2003_REPLICATION_LAG, OJS_2004_INTERNAL_SERVER_ERROR,
    // OJS-3xxx
    OJS_3000_JOB_NOT_FOUND, OJS_3001_DUPLICATE_JOB, OJS_3002_JOB_ALREADY_COMPLETED,
    OJS_3003_JOB_ALREADY_CANCELLED, OJS_3004_QUEUE_PAUSED, OJS_3005_HANDLER_ERROR,
    OJS_3006_HANDLER_TIMEOUT, OJS_3007_HANDLER_PANIC, OJS_3008_NON_RETRYABLE_ERROR,
    OJS_3009_JOB_CANCELLED, OJS_3010_NO_HANDLER_REGISTERED,
    // OJS-4xxx
    OJS_4000_WORKFLOW_NOT_FOUND, OJS_4001_CHAIN_STEP_FAILED, OJS_4002_GROUP_TIMEOUT,
    OJS_4003_DEPENDENCY_FAILED, OJS_4004_CYCLIC_DEPENDENCY, OJS_4005_BATCH_CALLBACK_FAILED,
    OJS_4006_WORKFLOW_CANCELLED,
    // OJS-5xxx
    OJS_5000_UNAUTHENTICATED, OJS_5001_PERMISSION_DENIED, OJS_5002_TOKEN_EXPIRED,
    OJS_5003_TENANT_ACCESS_DENIED,
    // OJS-6xxx
    OJS_6000_RATE_LIMITED, OJS_6001_QUEUE_FULL, OJS_6002_CONCURRENCY_LIMITED,
    OJS_6003_BACKPRESSURE_APPLIED,
    // OJS-7xxx
    OJS_7000_UNSUPPORTED_FEATURE, OJS_7001_CRON_SCHEDULE_CONFLICT,
    OJS_7002_UNIQUE_KEY_INVALID, OJS_7003_MIDDLEWARE_ERROR, OJS_7004_MIDDLEWARE_TIMEOUT,
];

/// Look up an entry by its canonical wire-format code (e.g., `"INVALID_PAYLOAD"`).
pub fn lookup_by_canonical_code(canonical: &str) -> Option<&'static ErrorCodeEntry> {
    ALL_ERROR_CODES.iter().find(|e| e.canonical_code == canonical)
}

/// Look up an entry by its OJS-XXXX numeric code (e.g., `"OJS-1000"`).
pub fn lookup_by_code(code: &str) -> Option<&'static ErrorCodeEntry> {
    ALL_ERROR_CODES.iter().find(|e| e.code == code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lookup_by_canonical_code() {
        let entry = lookup_by_canonical_code("INVALID_PAYLOAD").unwrap();
        assert_eq!(entry.code, "OJS-1000");
        assert_eq!(entry.http_status, 400);
        assert!(!entry.retryable);
    }

    #[test]
    fn test_lookup_by_code() {
        let entry = lookup_by_code("OJS-3001").unwrap();
        assert_eq!(entry.canonical_code, "DUPLICATE_JOB");
        assert_eq!(entry.http_status, 409);
    }

    #[test]
    fn test_all_codes_count() {
        assert!(ALL_ERROR_CODES.len() >= 50, "Should have at least 50 error codes");
    }

    #[test]
    fn test_unknown_code_returns_none() {
        assert!(lookup_by_code("OJS-9999").is_none());
        assert!(lookup_by_canonical_code("DOES_NOT_EXIST").is_none());
    }
}
