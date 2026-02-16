use ojs::events::*;
use ojs::Event;
use serde_json::json;

// ---------------------------------------------------------------------------
// Event type constant tests
// ---------------------------------------------------------------------------

#[test]
fn test_core_job_event_constants() {
    assert_eq!(EVENT_JOB_ENQUEUED, "job.enqueued");
    assert_eq!(EVENT_JOB_STARTED, "job.started");
    assert_eq!(EVENT_JOB_COMPLETED, "job.completed");
    assert_eq!(EVENT_JOB_FAILED, "job.failed");
    assert_eq!(EVENT_JOB_DISCARDED, "job.discarded");
}

#[test]
fn test_extended_job_event_constants() {
    assert_eq!(EVENT_JOB_RETRYING, "job.retrying");
    assert_eq!(EVENT_JOB_CANCELLED, "job.cancelled");
    assert_eq!(EVENT_JOB_HEARTBEAT, "job.heartbeat");
    assert_eq!(EVENT_JOB_SCHEDULED, "job.scheduled");
    assert_eq!(EVENT_JOB_EXPIRED, "job.expired");
    assert_eq!(EVENT_JOB_PROGRESS, "job.progress");
}

#[test]
fn test_queue_event_constants() {
    assert_eq!(EVENT_QUEUE_PAUSED, "queue.paused");
    assert_eq!(EVENT_QUEUE_RESUMED, "queue.resumed");
}

#[test]
fn test_worker_event_constants() {
    assert_eq!(EVENT_WORKER_STARTED, "worker.started");
    assert_eq!(EVENT_WORKER_STOPPED, "worker.stopped");
    assert_eq!(EVENT_WORKER_QUIET, "worker.quiet");
    assert_eq!(EVENT_WORKER_HEARTBEAT, "worker.heartbeat");
}

#[test]
fn test_workflow_event_constants() {
    assert_eq!(EVENT_WORKFLOW_STARTED, "workflow.started");
    assert_eq!(EVENT_WORKFLOW_STEP_COMPLETED, "workflow.step_completed");
    assert_eq!(EVENT_WORKFLOW_COMPLETED, "workflow.completed");
    assert_eq!(EVENT_WORKFLOW_FAILED, "workflow.failed");
}

#[test]
fn test_cron_event_constants() {
    assert_eq!(EVENT_CRON_TRIGGERED, "cron.triggered");
    assert_eq!(EVENT_CRON_SKIPPED, "cron.skipped");
}

// ---------------------------------------------------------------------------
// Event envelope deserialization tests
// ---------------------------------------------------------------------------

#[test]
fn test_event_deserialization_complete() {
    let json = json!({
        "specversion": "1.0",
        "id": "evt_001",
        "type": "job.completed",
        "source": "ojs://my-service/workers/w1",
        "time": "2025-01-15T10:30:00Z",
        "subject": "job_abc123",
        "datacontenttype": "application/json",
        "data": {
            "result": {"status": "ok"},
            "duration_ms": 150
        }
    });

    let event: Event = serde_json::from_value(json).unwrap();
    assert_eq!(event.specversion, "1.0");
    assert_eq!(event.id, "evt_001");
    assert_eq!(event.event_type, "job.completed");
    assert_eq!(event.source, "ojs://my-service/workers/w1");
    assert_eq!(event.subject, Some("job_abc123".into()));
    assert_eq!(event.datacontenttype, Some("application/json".into()));
    assert!(event.data.is_some());
    let data = event.data.unwrap();
    assert!(data.contains_key("result"));
    assert!(data.contains_key("duration_ms"));
}

#[test]
fn test_event_deserialization_minimal() {
    let json = json!({
        "id": "evt_002",
        "type": "job.started",
        "source": "ojs://test/workers/w1",
        "time": "2025-01-01T00:00:00Z"
    });

    let event: Event = serde_json::from_value(json).unwrap();
    assert_eq!(event.id, "evt_002");
    assert_eq!(event.event_type, "job.started");
    assert_eq!(event.specversion, "1.0"); // default value
    assert!(event.subject.is_none());
    assert!(event.data.is_none());
    assert!(event.datacontenttype.is_none());
}

#[test]
fn test_event_with_job_payload() {
    let json = json!({
        "specversion": "1.0",
        "id": "evt_003",
        "type": "job.enqueued",
        "source": "ojs://my-service/api",
        "time": "2025-06-01T12:00:00Z",
        "subject": "job_xyz789",
        "data": {
            "job_type": "email.send",
            "queue": "email",
            "priority": 5,
            "args": [{"to": "user@example.com"}]
        }
    });

    let event: Event = serde_json::from_value(json).unwrap();
    assert_eq!(event.event_type, "job.enqueued");
    let data = event.data.unwrap();
    assert_eq!(data["job_type"], "email.send");
    assert_eq!(data["queue"], "email");
    assert_eq!(data["priority"], 5);
}

#[test]
fn test_event_with_workflow_payload() {
    let json = json!({
        "specversion": "1.0",
        "id": "evt_004",
        "type": "workflow.completed",
        "source": "ojs://my-service/workflows",
        "time": "2025-06-01T13:00:00Z",
        "subject": "wf_001",
        "data": {
            "workflow_id": "wf_001",
            "name": "ETL Pipeline",
            "steps_completed": 3,
            "duration_ms": 45000
        }
    });

    let event: Event = serde_json::from_value(json).unwrap();
    assert_eq!(event.event_type, "workflow.completed");
    assert_eq!(event.subject, Some("wf_001".into()));
    let data = event.data.unwrap();
    assert_eq!(data["workflow_id"], "wf_001");
    assert_eq!(data["name"], "ETL Pipeline");
    assert_eq!(data["steps_completed"], 3);
}

#[test]
fn test_event_timestamp_parsing() {
    let json = json!({
        "id": "evt_005",
        "type": "job.failed",
        "source": "ojs://test",
        "time": "2025-07-04T15:30:45Z"
    });

    let event: Event = serde_json::from_value(json).unwrap();
    assert_eq!(event.time.year(), 2025);
    assert_eq!(event.time.month(), 7);
    assert_eq!(event.time.day(), 4);
    assert_eq!(event.time.hour(), 15);
    assert_eq!(event.time.minute(), 30);
    assert_eq!(event.time.second(), 45);
}

#[test]
fn test_event_serialization_roundtrip() {
    let json = json!({
        "specversion": "1.0",
        "id": "evt_round",
        "type": "queue.paused",
        "source": "ojs://test/admin",
        "time": "2025-01-01T00:00:00Z",
        "subject": "queue:email",
        "data": {"reason": "maintenance"}
    });

    let event: Event = serde_json::from_value(json).unwrap();
    let serialized = serde_json::to_string(&event).unwrap();
    let roundtrip: Event = serde_json::from_str(&serialized).unwrap();

    assert_eq!(roundtrip.id, "evt_round");
    assert_eq!(roundtrip.event_type, "queue.paused");
    assert_eq!(roundtrip.subject, Some("queue:email".into()));
}

// Use the chrono traits for date assertions
use chrono::Datelike;
use chrono::Timelike;
