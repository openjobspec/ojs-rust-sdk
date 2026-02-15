//! OJS Testing Module — fake mode, assertions, and test utilities.
//!
//! Implements the OJS Testing Specification (ojs-testing.md).
//!
//! # Usage
//!
//! ```rust,ignore
//! use ojs::testing::FakeStore;
//!
//! #[test]
//! fn test_sends_welcome_email() {
//!     let store = FakeStore::new();
//!     // ... code that enqueues via store.record_enqueue(...)
//!     store.assert_enqueued("email.send", None);
//! }
//! ```

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// A job recorded in fake mode.
#[derive(Debug, Clone)]
pub struct FakeJob {
    pub id: String,
    pub job_type: String,
    pub queue: String,
    pub args: Vec<serde_json::Value>,
    pub meta: HashMap<String, serde_json::Value>,
    pub state: String,
    pub attempt: u32,
    pub created_at: String,
}

/// Match criteria for assertions.
#[derive(Default)]
pub struct MatchCriteria {
    pub args: Option<Vec<serde_json::Value>>,
    pub queue: Option<String>,
    pub meta: Option<HashMap<String, serde_json::Value>>,
    pub count: Option<usize>,
}

/// In-memory job store for fake mode.
#[derive(Clone)]
pub struct FakeStore {
    inner: Arc<Mutex<FakeStoreInner>>,
}

type HandlerMap = HashMap<String, Box<dyn Fn(&FakeJob) -> Result<(), String> + Send>>;

struct FakeStoreInner {
    enqueued: Vec<FakeJob>,
    performed: Vec<FakeJob>,
    handlers: HandlerMap,
    next_id: u64,
}

impl Default for FakeStore {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeStore {
    /// Create a new fake store.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(FakeStoreInner {
                enqueued: Vec::new(),
                performed: Vec::new(),
                handlers: HashMap::new(),
                next_id: 0,
            })),
        }
    }

    /// Record a job enqueue.
    pub fn record_enqueue(
        &self,
        job_type: &str,
        args: Vec<serde_json::Value>,
        queue: Option<&str>,
        meta: Option<HashMap<String, serde_json::Value>>,
    ) -> FakeJob {
        let mut inner = self.inner.lock().unwrap();
        inner.next_id += 1;
        let job = FakeJob {
            id: format!("fake-{:06}", inner.next_id),
            job_type: job_type.to_string(),
            queue: queue.unwrap_or("default").to_string(),
            args,
            meta: meta.unwrap_or_default(),
            state: "available".to_string(),
            attempt: 0,
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        inner.enqueued.push(job.clone());
        job
    }

    /// Register a handler for drain execution.
    pub fn register_handler<F>(&self, job_type: &str, handler: F)
    where
        F: Fn(&FakeJob) -> Result<(), String> + Send + 'static,
    {
        let mut inner = self.inner.lock().unwrap();
        inner.handlers.insert(job_type.to_string(), Box::new(handler));
    }

    /// Assert that at least one job of the given type was enqueued.
    pub fn assert_enqueued(&self, job_type: &str, criteria: Option<&MatchCriteria>) {
        let inner = self.inner.lock().unwrap();
        let matches = filter_jobs(&inner.enqueued, job_type, criteria);

        if let Some(c) = criteria {
            if let Some(expected) = c.count {
                assert_eq!(
                    matches.len(),
                    expected,
                    "Expected {} enqueued job(s) of type '{}', found {}",
                    expected,
                    job_type,
                    matches.len()
                );
                return;
            }
        }

        assert!(
            !matches.is_empty(),
            "Expected at least one enqueued job of type '{}', found none. Enqueued types: {:?}",
            job_type,
            inner.enqueued.iter().map(|j| &j.job_type).collect::<Vec<_>>()
        );
    }

    /// Assert that NO job of the given type was enqueued.
    pub fn refute_enqueued(&self, job_type: &str) {
        let inner = self.inner.lock().unwrap();
        let matches: Vec<_> = inner.enqueued.iter().filter(|j| j.job_type == job_type).collect();
        assert!(
            matches.is_empty(),
            "Expected no enqueued jobs of type '{}', but found {}",
            job_type,
            matches.len()
        );
    }

    /// Assert that at least one job of the given type was performed.
    pub fn assert_performed(&self, job_type: &str) {
        let inner = self.inner.lock().unwrap();
        assert!(
            inner.performed.iter().any(|j| j.job_type == job_type),
            "Expected at least one performed job of type '{}', found none",
            job_type
        );
    }

    /// Assert that at least one job of the given type completed.
    pub fn assert_completed(&self, job_type: &str) {
        let inner = self.inner.lock().unwrap();
        assert!(
            inner.performed.iter().any(|j| j.job_type == job_type && j.state == "completed"),
            "Expected a completed job of type '{}', found none",
            job_type
        );
    }

    /// Return all enqueued jobs.
    pub fn all_enqueued(&self) -> Vec<FakeJob> {
        self.inner.lock().unwrap().enqueued.clone()
    }

    /// Clear all enqueued and performed jobs.
    pub fn clear_all(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.enqueued.clear();
        inner.performed.clear();
    }

    /// Process all available jobs using registered handlers.
    pub fn drain(&self) -> usize {
        let mut inner = self.inner.lock().unwrap();
        let mut processed = 0;

        for i in 0..inner.enqueued.len() {
            if inner.enqueued[i].state != "available" {
                continue;
            }

            inner.enqueued[i].state = "active".to_string();
            inner.enqueued[i].attempt += 1;

            let job_type = inner.enqueued[i].job_type.clone();
            let job_clone = inner.enqueued[i].clone();

            if let Some(handler) = inner.handlers.get(&job_type) {
                match handler(&job_clone) {
                    Ok(()) => inner.enqueued[i].state = "completed".to_string(),
                    Err(_) => inner.enqueued[i].state = "discarded".to_string(),
                }
            } else {
                inner.enqueued[i].state = "completed".to_string();
            }

            let performed_job = inner.enqueued[i].clone();
            inner.performed.push(performed_job);
            processed += 1;
        }

        processed
    }
}

fn filter_jobs<'a>(
    jobs: &'a [FakeJob],
    job_type: &str,
    criteria: Option<&MatchCriteria>,
) -> Vec<&'a FakeJob> {
    jobs.iter()
        .filter(|j| {
            if j.job_type != job_type {
                return false;
            }
            if let Some(c) = criteria {
                if let Some(ref q) = c.queue {
                    if j.queue != *q {
                        return false;
                    }
                }
            }
            true
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fake_store_basics() {
        let store = FakeStore::new();

        store.record_enqueue("email.send", vec![], None, None);
        store.record_enqueue("email.send", vec![], None, None);
        store.record_enqueue("report.generate", vec![], None, None);

        store.assert_enqueued("email.send", None);
        store.assert_enqueued("report.generate", None);
        store.refute_enqueued("payment.process");

        let all = store.all_enqueued();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn test_drain_processes_jobs() {
        let store = FakeStore::new();

        store.record_enqueue("email.send", vec![], None, None);
        store.record_enqueue("email.send", vec![], None, None);

        let processed = store.drain();
        assert_eq!(processed, 2);

        store.assert_completed("email.send");
    }

    #[test]
    fn test_clear_all() {
        let store = FakeStore::new();
        store.record_enqueue("email.send", vec![], None, None);
        store.clear_all();
        assert!(store.all_enqueued().is_empty());
    }
}
