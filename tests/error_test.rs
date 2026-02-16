use ojs::errors::JobError;
use ojs::{OjsError, ServerError};
use serde_json::json;

// ---------------------------------------------------------------------------
// OjsError variant tests
// ---------------------------------------------------------------------------

#[test]
fn test_ojs_error_server_display() {
    let server_err = ServerError::new("not_found", "job not found", 404);
    let err = OjsError::from(server_err);
    let display = err.to_string();
    assert!(display.contains("not_found"));
    assert!(display.contains("job not found"));
}

#[test]
fn test_ojs_error_transport_display() {
    let err = OjsError::Transport("connection refused".into());
    assert_eq!(err.to_string(), "transport error: connection refused");
}

#[test]
fn test_ojs_error_serialization_display() {
    let err = OjsError::Serialization("invalid JSON".into());
    assert_eq!(err.to_string(), "serialization error: invalid JSON");
}

#[test]
fn test_ojs_error_handler_display() {
    let err = OjsError::Handler("something went wrong".into());
    assert_eq!(err.to_string(), "handler error: something went wrong");
}

#[test]
fn test_ojs_error_non_retryable_display() {
    let err = OjsError::NonRetryable("permanent failure".into());
    assert_eq!(err.to_string(), "non-retryable error: permanent failure");
}

#[test]
fn test_ojs_error_builder_display() {
    let err = OjsError::Builder("url is required".into());
    assert_eq!(err.to_string(), "builder error: url is required");
}

#[test]
fn test_ojs_error_no_handler_display() {
    let err = OjsError::NoHandler("unknown.job".into());
    assert_eq!(
        err.to_string(),
        "no handler registered for job type: unknown.job"
    );
}

#[test]
fn test_ojs_error_worker_shutdown_display() {
    let err = OjsError::WorkerShutdown;
    assert_eq!(err.to_string(), "worker shut down");
}

// ---------------------------------------------------------------------------
// ServerError tests
// ---------------------------------------------------------------------------

#[test]
fn test_server_error_creation() {
    let err = ServerError::new("backend_error", "database timeout", 500);
    assert_eq!(err.code(), "backend_error");
    assert_eq!(err.message, "database timeout");
    assert_eq!(err.http_status, 500);
    assert!(!err.is_retryable());
}

#[test]
fn test_server_error_retryable() {
    let err = ServerError::new("rate_limited", "too many requests", 429).retryable(true);
    assert!(err.is_retryable());
    assert!(err.is_rate_limited());
}

#[test]
fn test_server_error_not_found() {
    let err = ServerError::new("not_found", "job not found", 404);
    assert!(err.is_not_found());
    assert!(!err.is_duplicate());
    assert!(!err.is_rate_limited());
    assert!(!err.is_queue_paused());
}

#[test]
fn test_server_error_duplicate() {
    let err = ServerError::new("duplicate", "job already exists", 409);
    assert!(err.is_duplicate());
    assert!(!err.is_not_found());
}

#[test]
fn test_server_error_queue_paused() {
    let err = ServerError::new("queue_paused", "queue is paused", 409);
    assert!(err.is_queue_paused());
}

#[test]
fn test_server_error_display() {
    let err = ServerError::new("not_found", "resource missing", 404);
    let display = err.to_string();
    assert_eq!(display, "[not_found] resource missing");
}

#[test]
fn test_server_error_fields() {
    // Deserialize to construct non-exhaustive struct
    let json = json!({
        "code": "invalid_request",
        "message": "validation failed",
        "retryable": false,
        "details": {"field": "email"},
        "request_id": "req_123"
    });
    let mut err: ServerError = serde_json::from_value(json).unwrap();
    err.http_status = 400;

    assert_eq!(err.code(), "invalid_request");
    assert_eq!(err.message, "validation failed");
    assert!(!err.is_retryable());
    assert!(err.details.is_some());
    assert_eq!(err.details.as_ref().unwrap()["field"], "email");
    assert_eq!(err.request_id, Some("req_123".into()));
    assert_eq!(err.http_status, 400);
}

// ---------------------------------------------------------------------------
// ServerError serde tests
// ---------------------------------------------------------------------------

#[test]
fn test_server_error_serialization_roundtrip() {
    let err = ServerError::new("backend_error", "connection lost", 503).retryable(true);

    let json_str = serde_json::to_string(&err).unwrap();
    let deserialized: ServerError = serde_json::from_str(&json_str).unwrap();

    assert_eq!(deserialized.code, "backend_error");
    assert_eq!(deserialized.message, "connection lost");
    assert!(deserialized.retryable);
    // http_status is #[serde(skip)] so it defaults to 0
    assert_eq!(deserialized.http_status, 0);
}

// ---------------------------------------------------------------------------
// From conversions
// ---------------------------------------------------------------------------

#[test]
fn test_server_error_into_ojs_error() {
    let server_err = ServerError::new("not_found", "missing", 404);
    let ojs_err: OjsError = server_err.into();

    match ojs_err {
        OjsError::Server(boxed) => {
            assert_eq!(boxed.code(), "not_found");
            assert_eq!(boxed.http_status, 404);
        }
        other => panic!("expected Server variant, got: {:?}", other),
    }
}

#[test]
fn test_serde_json_error_into_ojs_error() {
    let bad_json = "not valid json{{{";
    let json_err = serde_json::from_str::<serde_json::Value>(bad_json).unwrap_err();
    let ojs_err: OjsError = json_err.into();

    match ojs_err {
        OjsError::Serialization(msg) => {
            assert!(!msg.is_empty());
        }
        other => panic!("expected Serialization variant, got: {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// JobError tests
// ---------------------------------------------------------------------------

#[test]
fn test_job_error_serialization_roundtrip() {
    let json = json!({
        "type": "RuntimeError",
        "message": "null pointer dereference",
        "backtrace": ["main.rs:42", "handler.rs:15", "worker.rs:100"]
    });

    let err: JobError = serde_json::from_value(json).unwrap();
    let json_str = serde_json::to_string(&err).unwrap();
    let deserialized: JobError = serde_json::from_str(&json_str).unwrap();

    assert_eq!(deserialized.error_type, "RuntimeError");
    assert_eq!(deserialized.message, "null pointer dereference");
    assert_eq!(deserialized.backtrace.as_ref().unwrap().len(), 3);
}

#[test]
fn test_job_error_without_backtrace() {
    let json = json!({
        "type": "ValidationError",
        "message": "invalid email format"
    });

    let err: JobError = serde_json::from_value(json).unwrap();
    assert_eq!(err.error_type, "ValidationError");
    assert_eq!(err.message, "invalid email format");
    assert!(err.backtrace.is_none());
}

#[test]
fn test_job_error_fields() {
    let json = json!({
        "type": "TimeoutError",
        "message": "execution exceeded 30s"
    });

    let err: JobError = serde_json::from_value(json).unwrap();
    assert_eq!(err.error_type, "TimeoutError");
    assert_eq!(err.message, "execution exceeded 30s");
    assert!(err.backtrace.is_none());
}
