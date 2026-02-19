use ojs::{Client, JobRequest, OjsError, RetryPolicy};
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const OJS_CONTENT_TYPE: &str = "application/openjobspec+json";

fn job_response(id: &str, job_type: &str) -> serde_json::Value {
    json!({
        "job": {
            "specversion": "1.0",
            "id": id,
            "type": job_type,
            "queue": "default",
            "args": [{"key": "value"}],
            "state": "available",
            "attempt": 0,
            "priority": 0,
            "created_at": "2025-01-01T00:00:00Z",
            "enqueued_at": "2025-01-01T00:00:00Z",
            "tags": []
        }
    })
}

// ---------------------------------------------------------------------------
// Enqueue tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_enqueue_simple() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/ojs/v1/jobs"))
        .and(header("Content-Type", OJS_CONTENT_TYPE))
        .and(header("Accept", OJS_CONTENT_TYPE))
        .and(header("OJS-Version", "1.0"))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(job_response("job-001", "email.send")),
        )
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::builder().url(server.uri()).build().unwrap();

    let job = client
        .enqueue("email.send", json!({"to": "user@example.com"}))
        .await
        .unwrap();

    assert_eq!(job.id, "job-001");
    assert_eq!(job.job_type, "email.send");
}

#[tokio::test]
async fn test_enqueue_with_options() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/ojs/v1/jobs"))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(job_response("job-002", "report.generate")),
        )
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::builder().url(server.uri()).build().unwrap();

    let job = client
        .enqueue("report.generate", json!({"id": 42}))
        .queue("reports")
        .priority(10)
        .retry(RetryPolicy::new().max_attempts(5))
        .tags(vec!["urgent".into()])
        .send()
        .await
        .unwrap();

    assert_eq!(job.id, "job-002");
}

#[tokio::test]
async fn test_enqueue_with_auth_token() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/ojs/v1/jobs"))
        .and(header("Authorization", "Bearer my-secret-token"))
        .respond_with(ResponseTemplate::new(201).set_body_json(job_response("job-003", "test")))
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::builder()
        .url(server.uri())
        .auth_token("my-secret-token")
        .build()
        .unwrap();

    let job = client.enqueue("test", json!({})).await.unwrap();

    assert_eq!(job.id, "job-003");
}

// ---------------------------------------------------------------------------
// Batch enqueue tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_batch_enqueue() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/ojs/v1/jobs/batch"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "jobs": [
                {
                    "specversion": "1.0",
                    "id": "batch-1",
                    "type": "notify",
                    "queue": "default",
                    "args": [{"user_id": 1}],
                    "state": "available",
                    "attempt": 0,
                    "priority": 0,
                    "tags": []
                },
                {
                    "specversion": "1.0",
                    "id": "batch-2",
                    "type": "notify",
                    "queue": "default",
                    "args": [{"user_id": 2}],
                    "state": "available",
                    "attempt": 0,
                    "priority": 0,
                    "tags": []
                }
            ],
            "count": 2
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::builder().url(server.uri()).build().unwrap();

    let jobs = client
        .enqueue_batch(vec![
            JobRequest::new("notify", json!({"user_id": 1})),
            JobRequest::new("notify", json!({"user_id": 2})),
        ])
        .await
        .unwrap();

    assert_eq!(jobs.len(), 2);
    assert_eq!(jobs[0].id, "batch-1");
    assert_eq!(jobs[1].id, "batch-2");
}

// ---------------------------------------------------------------------------
// Get/Cancel job tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_get_job() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/ojs/v1/jobs/job-123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "specversion": "1.0",
            "id": "job-123",
            "type": "email.send",
            "queue": "default",
            "args": [{"to": "user@example.com"}],
            "state": "active",
            "attempt": 1,
            "priority": 0,
            "tags": []
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::builder().url(server.uri()).build().unwrap();
    let job = client.get_job("job-123").await.unwrap();

    assert_eq!(job.id, "job-123");
    assert_eq!(job.state, Some(ojs::JobState::Active));
    assert_eq!(job.attempt, 1);
}

#[tokio::test]
async fn test_cancel_job() {
    let server = MockServer::start().await;

    Mock::given(method("DELETE"))
        .and(path("/ojs/v1/jobs/job-456"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "specversion": "1.0.0-rc.1",
            "id": "job-456",
            "type": "test",
            "queue": "default",
            "args": [],
            "state": "cancelled",
            "attempt": 0,
            "priority": 0,
            "tags": []
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::builder().url(server.uri()).build().unwrap();
    let job = client.cancel_job("job-456").await.unwrap();

    assert_eq!(job.state, Some(ojs::JobState::Cancelled));
}

// ---------------------------------------------------------------------------
// Error response tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_server_error_structured() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/ojs/v1/jobs/nonexistent"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "error": {
                "code": "not_found",
                "message": "job not found",
                "retryable": false
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::builder().url(server.uri()).build().unwrap();
    let err = client.get_job("nonexistent").await.unwrap_err();

    match err {
        OjsError::Server(server_err) => {
            assert!(server_err.is_not_found());
            assert_eq!(server_err.code(), "not_found");
            assert!(!server_err.is_retryable());
            assert_eq!(server_err.http_status, 404);
        }
        other => panic!("expected Server error, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_rate_limit_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/ojs/v1/jobs"))
        .respond_with(ResponseTemplate::new(429).set_body_json(json!({
            "error": {
                "code": "rate_limited",
                "message": "too many requests",
                "retryable": true
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::builder().url(server.uri()).build().unwrap();
    let err = client.enqueue("test", json!({})).await.unwrap_err();

    match err {
        OjsError::Server(server_err) => {
            assert!(server_err.is_rate_limited());
            assert!(server_err.is_retryable());
            assert_eq!(server_err.http_status, 429);
            assert!(server_err.retry_after().is_none());
        }
        other => panic!("expected Server error, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_rate_limit_error_with_retry_after() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/ojs/v1/jobs"))
        .respond_with(
            ResponseTemplate::new(429)
                .append_header("Retry-After", "30")
                .set_body_json(json!({
                    "error": {
                        "code": "rate_limited",
                        "message": "too many requests",
                        "retryable": true
                    }
                })),
        )
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::builder().url(server.uri()).build().unwrap();
    let err = client.enqueue("test", json!({})).await.unwrap_err();

    match err {
        OjsError::Server(server_err) => {
            assert!(server_err.is_rate_limited());
            assert_eq!(
                server_err.retry_after(),
                Some(std::time::Duration::from_secs(30))
            );
        }
        other => panic!("expected Server error, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_rate_limit_error_with_full_headers() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/ojs/v1/jobs"))
        .respond_with(
            ResponseTemplate::new(429)
                .append_header("Retry-After", "60")
                .append_header("X-RateLimit-Limit", "1000")
                .append_header("X-RateLimit-Remaining", "0")
                .append_header("X-RateLimit-Reset", "1700000000")
                .set_body_json(json!({
                    "error": {
                        "code": "rate_limited",
                        "message": "too many requests",
                        "retryable": true
                    }
                })),
        )
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::builder().url(server.uri()).build().unwrap();
    let err = client.enqueue("test", json!({})).await.unwrap_err();

    match err {
        OjsError::Server(server_err) => {
            assert!(server_err.is_rate_limited());
            assert_eq!(
                server_err.retry_after(),
                Some(std::time::Duration::from_secs(60))
            );
            let rl = server_err.rate_limit().expect("rate_limit should be set");
            assert_eq!(rl.limit, Some(1000));
            assert_eq!(rl.remaining, Some(0));
            assert_eq!(rl.reset, Some(1700000000));
            assert_eq!(rl.retry_after, Some(std::time::Duration::from_secs(60)));
        }
        other => panic!("expected Server error, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_duplicate_error() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/ojs/v1/jobs"))
        .respond_with(ResponseTemplate::new(409).set_body_json(json!({
            "error": {
                "code": "duplicate",
                "message": "job already exists",
                "retryable": false
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::builder().url(server.uri()).build().unwrap();
    let err = client.enqueue("test", json!({})).await.unwrap_err();

    match err {
        OjsError::Server(server_err) => {
            assert!(server_err.is_duplicate());
            assert!(!server_err.is_retryable());
        }
        other => panic!("expected Server error, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_unstructured_error_response() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/ojs/v1/health"))
        .respond_with(ResponseTemplate::new(502).set_body_string("Bad Gateway"))
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::builder().url(server.uri()).build().unwrap();
    let err = client.health().await.unwrap_err();

    match err {
        OjsError::Server(server_err) => {
            assert_eq!(server_err.code(), "http_502");
            assert!(server_err.is_retryable());
        }
        other => panic!("expected Server error, got: {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Health and manifest tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_health_check() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/ojs/v1/health"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "healthy",
            "version": "1.0.0",
            "uptime_seconds": 3600
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::builder().url(server.uri()).build().unwrap();
    let health = client.health().await.unwrap();

    assert_eq!(health.status, "healthy");
    assert_eq!(health.version, Some("1.0.0".into()));
    assert_eq!(health.uptime_seconds, Some(3600));
}

// ---------------------------------------------------------------------------
// Queue operations tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_list_queues() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/ojs/v1/queues"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "queues": [
                {"name": "default", "status": "active"},
                {"name": "email", "status": "active"}
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::builder().url(server.uri()).build().unwrap();
    let queues = client.list_queues().await.unwrap();

    assert_eq!(queues.len(), 2);
    assert_eq!(queues[0].name, "default");
    assert_eq!(queues[1].name, "email");
}

#[tokio::test]
async fn test_pause_resume_queue() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/ojs/v1/queues/default/pause"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/ojs/v1/queues/default/resume"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::builder().url(server.uri()).build().unwrap();

    client.pause_queue("default").await.unwrap();
    client.resume_queue("default").await.unwrap();
}

// ---------------------------------------------------------------------------
// Workflow tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_create_workflow() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/ojs/v1/workflows"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": "wf-001",
            "name": "ETL Pipeline",
            "state": "pending",
            "steps": [
                {"id": "step-0", "type": "fetch", "state": "pending", "depends_on": []},
                {"id": "step-1", "type": "transform", "state": "pending", "depends_on": ["step-0"]}
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::builder().url(server.uri()).build().unwrap();

    let workflow = client
        .create_workflow(
            ojs::chain(vec![
                ojs::Step::new("fetch", json!({})),
                ojs::Step::new("transform", json!({})),
            ])
            .name("ETL Pipeline"),
        )
        .await
        .unwrap();

    assert_eq!(workflow.id, "wf-001");
    assert_eq!(workflow.state, ojs::WorkflowState::Pending);
    assert_eq!(workflow.steps.len(), 2);
}

// ---------------------------------------------------------------------------
// Custom header tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_custom_headers() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/ojs/v1/health"))
        .and(header("X-Tenant-Id", "tenant-42"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "healthy"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::builder()
        .url(server.uri())
        .header("X-Tenant-Id", "tenant-42")
        .build()
        .unwrap();

    let health = client.health().await.unwrap();
    assert_eq!(health.status, "healthy");
}

// ---------------------------------------------------------------------------
// Schema operations tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_list_schemas() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/ojs/v1/schemas"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "schemas": [
                {
                    "uri": "urn:ojs:schema:email.send",
                    "type": "json-schema",
                    "version": "1.0.0",
                    "created_at": "2025-01-01T00:00:00Z"
                },
                {
                    "uri": "urn:ojs:schema:report.generate",
                    "type": "json-schema",
                    "version": "2.0.0",
                    "created_at": "2025-01-02T00:00:00Z"
                }
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::builder().url(server.uri()).build().unwrap();
    let schemas = client.list_schemas().await.unwrap();

    assert_eq!(schemas.len(), 2);
    assert_eq!(schemas[0].uri, "urn:ojs:schema:email.send");
    assert_eq!(schemas[0].schema_type, "json-schema");
    assert_eq!(schemas[1].uri, "urn:ojs:schema:report.generate");
    assert_eq!(schemas[1].version, "2.0.0");
}

#[tokio::test]
async fn test_register_schema() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/ojs/v1/schemas"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "uri": "urn:ojs:schema:email.send",
            "type": "json-schema",
            "version": "1.0.0",
            "created_at": "2025-01-01T00:00:00Z",
            "schema": {
                "type": "object",
                "properties": {
                    "to": {"type": "string"}
                },
                "required": ["to"]
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::builder().url(server.uri()).build().unwrap();
    let detail = client
        .register_schema(ojs::RegisterSchemaRequest {
            uri: "urn:ojs:schema:email.send".into(),
            schema_type: "json-schema".into(),
            version: "1.0.0".into(),
            schema: json!({
                "type": "object",
                "properties": {
                    "to": {"type": "string"}
                },
                "required": ["to"]
            }),
        })
        .await
        .unwrap();

    assert_eq!(detail.uri, "urn:ojs:schema:email.send");
    assert_eq!(detail.schema_type, "json-schema");
    assert_eq!(detail.version, "1.0.0");
    assert!(detail.schema.is_object());
}

#[tokio::test]
async fn test_get_schema() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/ojs/v1/schemas/urn%3Aojs%3Aschema%3Aemail%2Esend"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "uri": "urn:ojs:schema:email.send",
            "type": "json-schema",
            "version": "1.0.0",
            "created_at": "2025-01-01T00:00:00Z",
            "schema": {
                "type": "object",
                "properties": {
                    "to": {"type": "string"}
                }
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::builder().url(server.uri()).build().unwrap();
    let detail = client
        .get_schema("urn:ojs:schema:email.send")
        .await
        .unwrap();

    assert_eq!(detail.uri, "urn:ojs:schema:email.send");
    assert_eq!(detail.schema_type, "json-schema");
    assert!(detail.schema.is_object());
}

#[tokio::test]
async fn test_delete_schema() {
    let server = MockServer::start().await;

    Mock::given(method("DELETE"))
        .and(path("/ojs/v1/schemas/urn%3Aojs%3Aschema%3Aemail%2Esend"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::builder().url(server.uri()).build().unwrap();
    client
        .delete_schema("urn:ojs:schema:email.send")
        .await
        .unwrap();
}
