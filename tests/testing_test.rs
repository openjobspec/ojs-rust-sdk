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
    fn test_fake_store_handler_need_not_be_sync() {
        let store = FakeStore::new();
        let calls = std::cell::Cell::new(0);

        store.register_handler("test.cell", move |_job| {
            calls.set(calls.get() + 1);
            assert_eq!(calls.get(), 1);
            Ok(())
        });

        store.record_enqueue("test.cell", vec![], None, None);
        assert_eq!(store.drain(), 1);
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

    // ---------------------------------------------------------------------------
    // MatchCriteria.args / .meta must actually be honored (ojs-testing.md §6.1
    // "Expected args (deep equality)" is a MUST-level requirement)
    // ---------------------------------------------------------------------------

    #[test]
    fn test_assert_enqueued_matches_on_exact_args() {
        let store = FakeStore::new();
        store.record_enqueue(
            "email.send",
            vec![json!({"to": "user@example.com"})],
            None,
            None,
        );

        let matching = MatchCriteria {
            args: Some(vec![json!({"to": "user@example.com"})]),
            ..Default::default()
        };
        // Must not panic: the enqueued args match exactly.
        store.assert_enqueued("email.send", Some(&matching));
    }

    #[test]
    #[should_panic(expected = "Expected 1 enqueued job(s)")]
    fn test_assert_enqueued_rejects_wrong_args() {
        let store = FakeStore::new();
        store.record_enqueue(
            "email.send",
            vec![json!({"to": "user@example.com"})],
            None,
            None,
        );

        // Before the fix, `args` was accepted but never checked, so this
        // assertion for the *wrong* recipient would incorrectly pass.
        let wrong = MatchCriteria {
            args: Some(vec![json!({"to": "someone-else@example.com"})]),
            count: Some(1),
            ..Default::default()
        };
        store.assert_enqueued("email.send", Some(&wrong));
    }

    #[test]
    fn test_assert_enqueued_matches_on_meta_subset() {
        let store = FakeStore::new();
        let mut meta = HashMap::new();
        meta.insert("tenant".to_string(), json!("acme"));
        meta.insert("trace_id".to_string(), json!("trace-123"));
        store.record_enqueue("report.generate", vec![], None, Some(meta));

        // Only asserting on a subset of the recorded meta must still match.
        let mut expected_meta = HashMap::new();
        expected_meta.insert("tenant".to_string(), json!("acme"));
        let criteria = MatchCriteria {
            meta: Some(expected_meta),
            ..Default::default()
        };
        store.assert_enqueued("report.generate", Some(&criteria));
    }

    #[test]
    #[should_panic(expected = "Expected at least one enqueued job")]
    fn test_assert_enqueued_rejects_wrong_meta_value() {
        let store = FakeStore::new();
        let mut meta = HashMap::new();
        meta.insert("tenant".to_string(), json!("acme"));
        store.record_enqueue("report.generate", vec![], None, Some(meta));

        let mut wrong_meta = HashMap::new();
        wrong_meta.insert("tenant".to_string(), json!("other-tenant"));
        let criteria = MatchCriteria {
            meta: Some(wrong_meta),
            ..Default::default()
        };
        store.assert_enqueued("report.generate", Some(&criteria));
    }

    // ---------------------------------------------------------------------------
    // FakeStore::drain() must not deadlock when a handler calls back into
    // the same store (e.g. to simulate a chained follow-up job)
    // ---------------------------------------------------------------------------

    #[test]
    fn test_drain_handler_can_enqueue_into_same_store_without_deadlock() {
        let store = FakeStore::new();
        let follow_up_store = store.clone();

        store.register_handler("job.first", move |_job| {
            // Reentrant call into the same store from within a handler:
            // this must not deadlock against drain()'s own lock.
            follow_up_store.record_enqueue("job.second", vec![], None, None);
            Ok(())
        });

        store.record_enqueue("job.first", vec![], None, None);

        // Run the reentrant drain on a background thread with a bounded
        // join timeout: if the fix regresses, this fails fast with a clear
        // "deadlocked" panic instead of hanging the whole test run forever.
        let store_for_thread = store.clone();
        let handle = std::thread::spawn(move || store_for_thread.drain());
        let start = std::time::Instant::now();
        loop {
            if handle.is_finished() {
                break;
            }
            assert!(
                start.elapsed() < std::time::Duration::from_secs(5),
                "drain() deadlocked when a handler re-entered the same FakeStore"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let processed = handle.join().unwrap();

        assert_eq!(
            processed, 1,
            "only job.first was available when drain() started"
        );
        store.assert_enqueued("job.second", None);
    }

    #[test]
    fn test_recursive_drain_defers_same_handler_without_deadlock() {
        let store = FakeStore::new();
        let recursive_store = store.clone();
        let calls = std::cell::Cell::new(0);

        store.register_handler("job.same", move |_job| {
            let call = calls.get();
            calls.set(call + 1);
            if call == 0 {
                recursive_store.record_enqueue("job.same", vec![], None, None);
                assert_eq!(recursive_store.drain(), 0);
            }
            Ok(())
        });

        store.record_enqueue("job.same", vec![], None, None);
        assert_eq!(store.drain(), 1);
        assert_eq!(store.drain(), 1);
        assert_eq!(store.drain(), 0);
    }

    // ---------------------------------------------------------------------------
    // ojs-testing.md section 6.1/6.2 RECOMMENDED helper completeness
    // ---------------------------------------------------------------------------

    #[test]
    fn test_refute_enqueued_matching_honors_criteria() {
        let store = FakeStore::new();
        store.record_enqueue("email.send", vec![json!({"to": "a@b.com"})], None, None);

        // No job of this type was enqueued to "reports" queue.
        let criteria = MatchCriteria {
            queue: Some("reports".to_string()),
            ..Default::default()
        };
        store.refute_enqueued_matching("email.send", Some(&criteria));
    }

    #[test]
    #[should_panic(expected = "Expected no enqueued jobs")]
    fn test_refute_enqueued_matching_fails_when_criteria_matches() {
        let store = FakeStore::new();
        store.record_enqueue("email.send", vec![], Some("default"), None);

        let criteria = MatchCriteria {
            queue: Some("default".to_string()),
            ..Default::default()
        };
        store.refute_enqueued_matching("email.send", Some(&criteria));
    }

    #[test]
    fn test_refute_performed() {
        let store = FakeStore::new();
        store.record_enqueue("email.send", vec![], None, None);
        // Not drained: nothing has been performed yet.
        store.refute_performed("email.send");
    }

    #[test]
    #[should_panic(expected = "Expected no performed jobs")]
    fn test_refute_performed_fails_after_drain() {
        let store = FakeStore::new();
        store.record_enqueue("email.send", vec![], None, None);
        store.drain();
        store.refute_performed("email.send");
    }

    #[test]
    fn test_assert_failed_after_handler_error() {
        let store = FakeStore::new();
        store.register_handler("payment.charge", |_job| Err("card declined".to_string()));
        store.record_enqueue("payment.charge", vec![], None, None);
        store.drain();
        store.assert_failed("payment.charge");
    }

    #[test]
    #[should_panic(expected = "Expected a failed")]
    fn test_assert_failed_panics_when_job_succeeded() {
        let store = FakeStore::new();
        store.record_enqueue("email.send", vec![], None, None);
        store.drain(); // no handler registered -> defaults to "completed"
        store.assert_failed("email.send");
    }

    #[test]
    fn test_all_enqueued_matching_filters_by_type_and_criteria() {
        let store = FakeStore::new();
        store.record_enqueue("email.send", vec![], Some("email"), None);
        store.record_enqueue("email.send", vec![], Some("reports"), None);
        store.record_enqueue("report.generate", vec![], Some("reports"), None);

        // No filter at all: everything.
        assert_eq!(store.all_enqueued_matching(None, None).len(), 3);

        // Filter by type only.
        assert_eq!(
            store.all_enqueued_matching(Some("email.send"), None).len(),
            2
        );

        // Filter by type + queue criteria.
        let criteria = MatchCriteria {
            queue: Some("reports".to_string()),
            ..Default::default()
        };
        let matches = store.all_enqueued_matching(Some("email.send"), Some(&criteria));
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].queue, "reports");
    }

    // ---------------------------------------------------------------------------
    // Match criteria are an independent predicate from the job type: they must
    // be applied whether or not a type is supplied (previously, a `None` type
    // returned every recorded job and silently ignored the criteria).
    // ---------------------------------------------------------------------------

    fn store_with_mixed_jobs() -> FakeStore {
        let store = FakeStore::new();
        let mut acme = HashMap::new();
        acme.insert("tenant".to_string(), json!("acme"));
        let mut globex = HashMap::new();
        globex.insert("tenant".to_string(), json!("globex"));

        store.record_enqueue(
            "email.send",
            vec![json!({"to": "a@example.com"})],
            Some("email"),
            Some(acme.clone()),
        );
        store.record_enqueue(
            "email.send",
            vec![json!({"to": "b@example.com"})],
            Some("reports"),
            Some(globex),
        );
        store.record_enqueue(
            "report.generate",
            vec![json!({"range": "monthly"})],
            Some("reports"),
            Some(acme),
        );
        store
    }

    #[test]
    fn test_all_enqueued_matching_applies_queue_criteria_without_a_type() {
        let store = store_with_mixed_jobs();
        let criteria = MatchCriteria {
            queue: Some("reports".to_string()),
            ..Default::default()
        };

        let matches = store.all_enqueued_matching(None, Some(&criteria));
        assert_eq!(matches.len(), 2);
        assert!(matches.iter().all(|j| j.queue == "reports"));
    }

    #[test]
    fn test_all_enqueued_matching_applies_args_criteria_without_a_type() {
        let store = store_with_mixed_jobs();
        let criteria = MatchCriteria {
            args: Some(vec![json!({"range": "monthly"})]),
            ..Default::default()
        };

        let matches = store.all_enqueued_matching(None, Some(&criteria));
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].job_type, "report.generate");
    }

    #[test]
    fn test_all_enqueued_matching_applies_meta_criteria_without_a_type() {
        let store = store_with_mixed_jobs();
        let mut expected_meta = HashMap::new();
        expected_meta.insert("tenant".to_string(), json!("globex"));
        let criteria = MatchCriteria {
            meta: Some(expected_meta),
            ..Default::default()
        };

        let matches = store.all_enqueued_matching(None, Some(&criteria));
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].queue, "reports");
        assert_eq!(matches[0].job_type, "email.send");
    }

    #[test]
    fn test_all_enqueued_matching_type_only_ignores_other_types() {
        let store = store_with_mixed_jobs();
        assert_eq!(
            store
                .all_enqueued_matching(Some("report.generate"), None)
                .len(),
            1
        );
        assert!(store
            .all_enqueued_matching(Some("unknown.type"), None)
            .is_empty());
    }

    #[test]
    fn test_all_enqueued_matching_combines_type_and_criteria() {
        let store = store_with_mixed_jobs();
        let mut expected_meta = HashMap::new();
        expected_meta.insert("tenant".to_string(), json!("acme"));
        let criteria = MatchCriteria {
            queue: Some("reports".to_string()),
            meta: Some(expected_meta),
            ..Default::default()
        };

        // The type predicate excludes the acme/reports report job.
        assert!(store
            .all_enqueued_matching(Some("email.send"), Some(&criteria))
            .is_empty());
        assert_eq!(
            store
                .all_enqueued_matching(Some("report.generate"), Some(&criteria))
                .len(),
            1
        );
    }

    #[test]
    fn test_all_enqueued_matching_with_empty_criteria_returns_everything() {
        let store = store_with_mixed_jobs();
        assert_eq!(store.all_enqueued_matching(None, None).len(), 3);
        assert_eq!(
            store
                .all_enqueued_matching(None, Some(&MatchCriteria::default()))
                .len(),
            3
        );
    }

    #[test]
    fn test_all_enqueued_matching_args_use_deep_equality() {
        let store = FakeStore::new();
        store.record_enqueue(
            "payment.charge",
            vec![json!({"amount": {"cents": 100, "currency": "USD"}, "tags": ["a", "b"]})],
            None,
            None,
        );

        // Deep equality: key order is irrelevant, nested values are compared.
        let equivalent = MatchCriteria {
            args: Some(vec![
                json!({"tags": ["a", "b"], "amount": {"currency": "USD", "cents": 100}}),
            ]),
            ..Default::default()
        };
        assert_eq!(
            store.all_enqueued_matching(None, Some(&equivalent)).len(),
            1
        );

        // ...but array order, nested values, and arity all matter.
        for wrong in [
            json!({"amount": {"cents": 100, "currency": "USD"}, "tags": ["b", "a"]}),
            json!({"amount": {"cents": 101, "currency": "USD"}, "tags": ["a", "b"]}),
            json!({"amount": {"cents": 100}, "tags": ["a", "b"]}),
        ] {
            let criteria = MatchCriteria {
                args: Some(vec![wrong.clone()]),
                ..Default::default()
            };
            assert!(
                store
                    .all_enqueued_matching(None, Some(&criteria))
                    .is_empty(),
                "args {wrong} must not deep-equal the recorded args"
            );
        }

        let wrong_arity = MatchCriteria {
            args: Some(vec![json!({}), json!({})]),
            ..Default::default()
        };
        assert!(store
            .all_enqueued_matching(None, Some(&wrong_arity))
            .is_empty());
    }
}
