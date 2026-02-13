use ojs::{Client, JobRequest, RetryPolicy};
use serde_json::json;

#[test]
fn test_client_builder_requires_url() {
    let result = Client::builder().build();
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("url is required"));
}

#[test]
fn test_client_builder_with_url() {
    let result = Client::builder()
        .url("http://localhost:8080")
        .build();
    assert!(result.is_ok());
}

#[test]
fn test_client_builder_with_all_options() {
    let result = Client::builder()
        .url("http://localhost:8080")
        .auth_token("my-token")
        .header("X-Custom", "value")
        .build();
    assert!(result.is_ok());
}

#[test]
fn test_job_request_creation() {
    let req = JobRequest::new("email.send", json!({"to": "user@example.com"}));
    assert_eq!(req.job_type, "email.send");
    assert!(req.meta.is_none());
    assert!(req.options.is_empty());
}

#[test]
fn test_retry_policy_builder() {
    let policy = RetryPolicy::new()
        .max_attempts(5)
        .initial_interval("PT2S")
        .backoff_coefficient(3.0)
        .jitter(false);

    assert_eq!(policy.max_attempts, 5);
    assert_eq!(policy.initial_interval, "PT2S");
    assert_eq!(policy.backoff_coefficient, 3.0);
    assert!(!policy.jitter);
}
