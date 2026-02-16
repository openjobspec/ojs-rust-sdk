use ojs::{CronJob, HealthStatus, Manifest, OverlapPolicy, Queue, QueueStats};
use serde_json::json;

// ---------------------------------------------------------------------------
// Queue tests
// ---------------------------------------------------------------------------

#[test]
fn test_queue_serialization_roundtrip() {
    let queue = Queue {
        name: "default".into(),
        status: Some("active".into()),
        created_at: None,
    };

    let json_str = serde_json::to_string(&queue).unwrap();
    let deserialized: Queue = serde_json::from_str(&json_str).unwrap();

    assert_eq!(deserialized.name, "default");
    assert_eq!(deserialized.status, Some("active".into()));
}

#[test]
fn test_queue_deserialization_minimal() {
    let json = json!({"name": "emails"});
    let queue: Queue = serde_json::from_value(json).unwrap();

    assert_eq!(queue.name, "emails");
    assert!(queue.status.is_none());
    assert!(queue.created_at.is_none());
}

#[test]
fn test_queue_deserialization_with_all_fields() {
    let json = json!({
        "name": "reports",
        "status": "paused",
        "created_at": "2025-01-01T00:00:00Z"
    });

    let queue: Queue = serde_json::from_value(json).unwrap();
    assert_eq!(queue.name, "reports");
    assert_eq!(queue.status, Some("paused".into()));
    assert!(queue.created_at.is_some());
}

// ---------------------------------------------------------------------------
// QueueStats tests
// ---------------------------------------------------------------------------

#[test]
fn test_queue_stats_deserialization() {
    let json = json!({
        "queue": "default",
        "status": "active",
        "stats": {
            "available": 50,
            "active": 3,
            "scheduled": 7,
            "retryable": 1,
            "discarded": 0,
            "completed_last_hour": 200,
            "failed_last_hour": 2,
            "avg_duration_ms": 120.5,
            "avg_wait_ms": 10.0,
            "throughput_per_second": 5.5
        },
        "computed_at": "2025-06-01T12:00:00Z"
    });

    let stats: QueueStats = serde_json::from_value(json).unwrap();
    assert_eq!(stats.queue, "default");
    assert_eq!(stats.status, "active");
    assert_eq!(stats.stats.available, 50);
    assert_eq!(stats.stats.active, 3);
    assert_eq!(stats.stats.scheduled, 7);
    assert_eq!(stats.stats.retryable, 1);
    assert_eq!(stats.stats.discarded, 0);
    assert_eq!(stats.stats.completed_last_hour, 200);
    assert_eq!(stats.stats.failed_last_hour, 2);
    assert_eq!(stats.stats.avg_duration_ms, 120.5);
    assert_eq!(stats.stats.avg_wait_ms, 10.0);
    assert_eq!(stats.stats.throughput_per_second, 5.5);
}

#[test]
fn test_queue_stats_default_metric_values() {
    let json = json!({
        "queue": "minimal",
        "status": "active",
        "stats": {},
        "computed_at": "2025-06-01T12:00:00Z"
    });

    let stats: QueueStats = serde_json::from_value(json).unwrap();
    assert_eq!(stats.stats.available, 0);
    assert_eq!(stats.stats.active, 0);
    assert_eq!(stats.stats.throughput_per_second, 0.0);
}

// ---------------------------------------------------------------------------
// CronJob tests
// ---------------------------------------------------------------------------

#[test]
fn test_cron_job_serialization_roundtrip() {
    let cron = json!({
        "name": "daily-report",
        "cron": "0 9 * * *",
        "timezone": "America/New_York",
        "type": "report.generate",
        "args": [{"format": "pdf"}],
        "overlap_policy": "skip",
        "enabled": true
    });

    let cron_job: CronJob = serde_json::from_value(cron).unwrap();
    assert_eq!(cron_job.name, "daily-report");
    assert_eq!(cron_job.cron, "0 9 * * *");
    assert_eq!(cron_job.timezone, "America/New_York");
    assert_eq!(cron_job.job_type, "report.generate");
    assert_eq!(cron_job.overlap_policy, OverlapPolicy::Skip);
    assert!(cron_job.enabled);

    let json_str = serde_json::to_string(&cron_job).unwrap();
    let roundtrip: CronJob = serde_json::from_str(&json_str).unwrap();
    assert_eq!(roundtrip.name, "daily-report");
    assert_eq!(roundtrip.cron, "0 9 * * *");
}

#[test]
fn test_cron_job_default_values() {
    let json = json!({
        "name": "nightly-cleanup",
        "cron": "0 0 * * *",
        "type": "cleanup.run",
        "args": []
    });

    let cron_job: CronJob = serde_json::from_value(json).unwrap();
    assert_eq!(cron_job.timezone, "UTC");
    assert_eq!(cron_job.overlap_policy, OverlapPolicy::Skip);
    assert!(cron_job.enabled);
}

#[test]
fn test_overlap_policy_variants() {
    let values = vec![
        ("\"skip\"", OverlapPolicy::Skip),
        ("\"allow\"", OverlapPolicy::Allow),
        ("\"cancel_previous\"", OverlapPolicy::CancelPrevious),
        ("\"enqueue\"", OverlapPolicy::Enqueue),
    ];

    for (json_str, expected) in values {
        let deserialized: OverlapPolicy = serde_json::from_str(json_str).unwrap();
        assert_eq!(deserialized, expected);

        let serialized = serde_json::to_string(&expected).unwrap();
        let roundtrip: OverlapPolicy = serde_json::from_str(&serialized).unwrap();
        assert_eq!(roundtrip, expected);
    }
}

// ---------------------------------------------------------------------------
// HealthStatus tests
// ---------------------------------------------------------------------------

#[test]
fn test_health_status_serialization() {
    let health = HealthStatus {
        status: "healthy".into(),
        version: Some("1.0.0".into()),
        uptime_seconds: Some(3600),
    };

    let json_str = serde_json::to_string(&health).unwrap();
    let deserialized: HealthStatus = serde_json::from_str(&json_str).unwrap();

    assert_eq!(deserialized.status, "healthy");
    assert_eq!(deserialized.version, Some("1.0.0".into()));
    assert_eq!(deserialized.uptime_seconds, Some(3600));
}

#[test]
fn test_health_status_minimal() {
    let json = json!({"status": "healthy"});
    let health: HealthStatus = serde_json::from_value(json).unwrap();

    assert_eq!(health.status, "healthy");
    assert!(health.version.is_none());
    assert!(health.uptime_seconds.is_none());
}

// ---------------------------------------------------------------------------
// Manifest tests
// ---------------------------------------------------------------------------

#[test]
fn test_manifest_serialization() {
    let json = json!({
        "ojs_version": "1.0.0-rc.1",
        "implementation": {
            "name": "ojs-backend-redis",
            "version": "0.1.0",
            "language": "Go",
            "homepage": "https://github.com/openjobspec/ojs-backend-redis"
        },
        "conformance_level": 4,
        "conformance_tier": "full",
        "protocols": ["http"],
        "backend": "redis",
        "capabilities": {
            "batch_enqueue": true,
            "cron_jobs": true,
            "dead_letter": true,
            "delayed_jobs": true,
            "job_ttl": true,
            "priority_queues": true,
            "rate_limiting": false,
            "schema_validation": true,
            "unique_jobs": true,
            "workflows": true,
            "pause_resume": true
        },
        "extensions": ["retry", "workflows"]
    });

    let manifest: Manifest = serde_json::from_value(json).unwrap();

    assert_eq!(manifest.ojs_version, "1.0.0-rc.1");
    assert_eq!(manifest.implementation.name, "ojs-backend-redis");
    assert_eq!(manifest.conformance_level, 4);
    assert!(manifest.capabilities.batch_enqueue);
    assert!(manifest.capabilities.workflows);
    assert!(!manifest.capabilities.rate_limiting);
    assert_eq!(manifest.extensions.as_ref().unwrap().len(), 2);

    // Roundtrip
    let json_str = serde_json::to_string(&manifest).unwrap();
    let roundtrip: Manifest = serde_json::from_str(&json_str).unwrap();
    assert_eq!(roundtrip.ojs_version, "1.0.0-rc.1");
    assert_eq!(roundtrip.implementation.name, "ojs-backend-redis");
}

#[test]
fn test_manifest_capabilities_default() {
    let json = json!({
        "ojs_version": "1.0.0-rc.1",
        "implementation": {"name": "test", "version": "0.1.0", "language": "Rust"},
        "conformance_level": 0,
        "protocols": ["http"],
        "backend": "memory",
        "capabilities": {}
    });

    let manifest: Manifest = serde_json::from_value(json).unwrap();
    let caps = &manifest.capabilities;

    assert!(!caps.batch_enqueue);
    assert!(!caps.cron_jobs);
    assert!(!caps.dead_letter);
    assert!(!caps.delayed_jobs);
    assert!(!caps.workflows);
    assert!(!caps.pause_resume);
}
