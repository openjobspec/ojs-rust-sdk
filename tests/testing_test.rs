//! Tests for the testing utilities module.
//!
//! These tests require the `testing` feature to be enabled.

#[cfg(feature = "testing")]
mod tests {
    use ojs::testing::{FakeStore, JobBuilder, MatchCriteria};
    use ojs::JobState;
    use serde_json::json;
    use std::collections::HashMap;

    // ---------------------------------------------------------------------------
    // FakeStore basic tests
    // ---------------------------------------------------------------------------

    #[test]
    fn test_fake_store_records_enqueued_jobs() {
        let store = FakeStore::new();

        let job = store.record_enqueue("email.send", vec![json!({"to": "a@b.com"})], None, None);
        assert_eq!(job.job_type, "email.send");
        assert_eq!(job.queue, "default");
        assert!(!job.id.is_empty());

        let all = store.all_enqueued();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].job_type, "email.send");
    }

    #[test]
    fn test_fake_store_records_multiple_types() {
        let store = FakeStore::new();

        store.record_enqueue("email.send", vec![], None, None);
        store.record_enqueue("report.generate", vec![], None, None);
        store.record_enqueue("email.send", vec![], None, None);

        let all = store.all_enqueued();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn test_fake_store_custom_queue() {
        let store = FakeStore::new();

        let job = store.record_enqueue("email.send", vec![], Some("email"), None);
        assert_eq!(job.queue, "email");
    }

    #[test]
    fn test_fake_store_with_meta() {
        let store = FakeStore::new();

        let mut meta = HashMap::new();
        meta.insert("tenant".to_string(), json!("acme"));

        let job = store.record_enqueue("email.send", vec![], None, Some(meta));
        assert_eq!(job.meta["tenant"], "acme");
    }

    // ---------------------------------------------------------------------------
    // FakeStore assertion tests
    // ---------------------------------------------------------------------------

    #[test]
    fn test_fake_store_assert_enqueued_basic() {
        let store = FakeStore::new();

        store.record_enqueue("email.send", vec![], None, None);
        store.assert_enqueued("email.send", None);
    }

    #[test]
    fn test_fake_store_assert_enqueued_with_count() {
        let store = FakeStore::new();

        store.record_enqueue("email.send", vec![], None, None);
        store.record_enqueue("email.send", vec![], None, None);
        store.record_enqueue("report.generate", vec![], None, None);

        let criteria = MatchCriteria {
            count: Some(2),
            ..Default::default()
        };
        store.assert_enqueued("email.send", Some(&criteria));
    }

    #[test]
    fn test_fake_store_assert_enqueued_with_queue_filter() {
        let store = FakeStore::new();

        store.record_enqueue("email.send", vec![], Some("email"), None);
        store.record_enqueue("email.send", vec![], Some("default"), None);

        let criteria = MatchCriteria {
            queue: Some("email".into()),
            count: Some(1),
            ..Default::default()
        };
        store.assert_enqueued("email.send", Some(&criteria));
    }

    #[test]
    fn test_fake_store_refute_enqueued() {
        let store = FakeStore::new();

        store.record_enqueue("email.send", vec![], None, None);
        store.refute_enqueued("payment.process");
    }

    #[test]
    #[should_panic(expected = "Expected at least one enqueued job")]
    fn test_fake_store_assert_enqueued_panics_when_missing() {
        let store = FakeStore::new();
        store.assert_enqueued("nonexistent.job", None);
    }

    // ---------------------------------------------------------------------------
    // FakeStore drain tests
    // ---------------------------------------------------------------------------

    #[test]
    fn test_fake_store_drain_returns_count() {
        let store = FakeStore::new();

        store.record_enqueue("email.send", vec![], None, None);
        store.record_enqueue("email.send", vec![], None, None);
        store.record_enqueue("report.generate", vec![], None, None);

        let processed = store.drain();
        assert_eq!(processed, 3);
    }

    #[test]
    fn test_fake_store_drain_marks_completed() {
        let store = FakeStore::new();

        store.record_enqueue("email.send", vec![], None, None);
        store.drain();

        store.assert_performed("email.send");
        store.assert_completed("email.send");
    }

    #[test]
    fn test_fake_store_drain_with_handler() {
        let store = FakeStore::new();

        store.register_handler("test.fail", |_job| Err("forced failure".into()));

        store.record_enqueue("test.fail", vec![], None, None);
        let processed = store.drain();
        assert_eq!(processed, 1);

        // Job should be discarded, not completed
        let all = store.all_enqueued();
        assert_eq!(all[0].state, "discarded");
    }

    #[test]
    fn test_fake_store_drain_empty() {
        let store = FakeStore::new();
        let processed = store.drain();
        assert_eq!(processed, 0);
    }

    #[test]
    fn test_fake_store_drain_does_not_reprocess() {
        let store = FakeStore::new();

        store.record_enqueue("email.send", vec![], None, None);
        let first = store.drain();
        assert_eq!(first, 1);

        let second = store.drain();
        assert_eq!(second, 0);
    }

    // ---------------------------------------------------------------------------
    // FakeStore clear tests
    // ---------------------------------------------------------------------------

    #[test]
    fn test_fake_store_clear_all() {
        let store = FakeStore::new();

        store.record_enqueue("email.send", vec![], None, None);
        store.record_enqueue("report.generate", vec![], None, None);
        store.drain();

        store.clear_all();
        assert!(store.all_enqueued().is_empty());
    }

    // ---------------------------------------------------------------------------
    // FakeStore ID generation
    // ---------------------------------------------------------------------------

    #[test]
    fn test_fake_store_generates_unique_ids() {
        let store = FakeStore::new();

        let job1 = store.record_enqueue("email.send", vec![], None, None);
        let job2 = store.record_enqueue("email.send", vec![], None, None);
        let job3 = store.record_enqueue("email.send", vec![], None, None);

        assert_ne!(job1.id, job2.id);
        assert_ne!(job2.id, job3.id);
        assert!(job1.id.starts_with("fake-"));
    }

    // ---------------------------------------------------------------------------
    // JobBuilder tests
    // ---------------------------------------------------------------------------

    #[test]
    fn test_job_builder_creates_valid_job() {
        let job = JobBuilder::new("email.send").build();

        assert_eq!(job.job_type, "email.send");
        assert_eq!(job.queue, "default");
        assert_eq!(job.priority, 0);
        assert_eq!(job.attempt, 0);
        assert_eq!(job.state, Some(JobState::Available));
        assert!(job.id.starts_with("test_"));
        assert!(!job.id.is_empty());
    }

    #[test]
    fn test_job_builder_custom_fields() {
        let job = JobBuilder::new("report.generate")
            .id("custom-id-123")
            .queue("reports")
            .priority(10)
            .attempt(3)
            .state(JobState::Active)
            .args(json!({"format": "pdf"}))
            .tags(vec!["urgent".into(), "vip".into()])
            .build();

        assert_eq!(job.id, "custom-id-123");
        assert_eq!(job.job_type, "report.generate");
        assert_eq!(job.queue, "reports");
        assert_eq!(job.priority, 10);
        assert_eq!(job.attempt, 3);
        assert_eq!(job.state, Some(JobState::Active));
        assert_eq!(job.tags, vec!["urgent", "vip"]);
    }

    #[test]
    fn test_job_builder_with_meta() {
        let mut meta = HashMap::new();
        meta.insert("tenant".to_string(), json!("acme"));
        meta.insert("region".to_string(), json!("us-east"));

        let job = JobBuilder::new("email.send").meta(meta).build();

        assert!(job.meta.is_some());
        let m = job.meta.unwrap();
        assert_eq!(m["tenant"], "acme");
        assert_eq!(m["region"], "us-east");
    }

    #[test]
    fn test_job_builder_with_retry_policy() {
        use ojs::RetryPolicy;

        let job = JobBuilder::new("test.retry")
            .retry(RetryPolicy::new().max_attempts(5))
            .build();

        assert!(job.retry.is_some());
        assert_eq!(job.retry.unwrap().max_attempts, 5);
    }

    #[test]
    fn test_job_builder_arg_extraction() {
        let job = JobBuilder::new("test.args")
            .args(json!({"name": "Alice", "age": 30}))
            .build();

        let name: String = job.arg("name").unwrap();
        assert_eq!(name, "Alice");

        let age: u32 = job.arg("age").unwrap();
        assert_eq!(age, 30);
    }

    // ---------------------------------------------------------------------------
    // MatchCriteria tests
    // ---------------------------------------------------------------------------

    #[test]
    fn test_match_criteria_default() {
        let criteria = MatchCriteria::default();
        assert!(criteria.args.is_none());
        assert!(criteria.queue.is_none());
        assert!(criteria.meta.is_none());
        assert!(criteria.count.is_none());
    }

    #[test]
    fn test_match_criteria_with_all_fields() {
        let criteria = MatchCriteria {
            args: Some(vec![json!({"to": "user@example.com"})]),
            queue: Some("email".into()),
            meta: Some(HashMap::new()),
            count: Some(1),
        };

        assert!(criteria.args.is_some());
        assert_eq!(criteria.queue, Some("email".into()));
        assert_eq!(criteria.count, Some(1));
    }
}
