//! The in-memory fake job store: recorded jobs, match criteria, assertions,
//! and the reentrant-safe drain loop.

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

/// A single registered fake-mode job handler.
type JobHandler = Arc<Mutex<Box<dyn Fn(&FakeJob) -> Result<(), String> + Send>>>;

type HandlerMap = HashMap<String, JobHandler>;

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
        inner.handlers.insert(
            job_type.to_string(),
            Arc::new(Mutex::new(Box::new(handler))),
        );
    }

    /// Assert that at least one job of the given type was enqueued.
    pub fn assert_enqueued(&self, job_type: &str, criteria: Option<&MatchCriteria>) {
        let inner = self.inner.lock().unwrap();
        let matches = filter_jobs(&inner.enqueued, Some(job_type), criteria);

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
            inner
                .enqueued
                .iter()
                .map(|j| &j.job_type)
                .collect::<Vec<_>>()
        );
    }

    /// Assert that NO job of the given type was enqueued.
    pub fn refute_enqueued(&self, job_type: &str) {
        self.refute_enqueued_matching(job_type, None);
    }

    /// Assert that NO job of the given type matching `criteria` was
    /// enqueued.
    ///
    /// Per `ojs-testing.md` §6.1, `refute_enqueued` takes the same optional
    /// criteria as [`assert_enqueued`](Self::assert_enqueued); this is a
    /// separate method (rather than changing `refute_enqueued`'s
    /// signature) so the existing no-criteria call sites are unaffected.
    pub fn refute_enqueued_matching(&self, job_type: &str, criteria: Option<&MatchCriteria>) {
        let inner = self.inner.lock().unwrap();
        let matches = filter_jobs(&inner.enqueued, Some(job_type), criteria);
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

    /// Assert that NO job of the given type was performed.
    ///
    /// `ojs-testing.md` §6.2 (RECOMMENDED).
    pub fn refute_performed(&self, job_type: &str) {
        let inner = self.inner.lock().unwrap();
        let matches: Vec<_> = inner
            .performed
            .iter()
            .filter(|j| j.job_type == job_type)
            .collect();
        assert!(
            matches.is_empty(),
            "Expected no performed jobs of type '{}', but found {}",
            job_type,
            matches.len()
        );
    }

    /// Assert that at least one job of the given type failed (reached the
    /// `discarded` state after its registered handler returned `Err`).
    ///
    /// `ojs-testing.md` §6.2 (RECOMMENDED): "Asserts that at least one job
    /// of the given type failed (reached `retryable` or `discarded`
    /// state)". `FakeStore::drain` only models the two terminal outcomes
    /// (`completed`/`discarded`), so this checks for `discarded`.
    pub fn assert_failed(&self, job_type: &str) {
        let inner = self.inner.lock().unwrap();
        assert!(
            inner
                .performed
                .iter()
                .any(|j| j.job_type == job_type && j.state == "discarded"),
            "Expected a failed (discarded) job of type '{}', found none",
            job_type
        );
    }

    /// Assert that at least one job of the given type completed.
    pub fn assert_completed(&self, job_type: &str) {
        let inner = self.inner.lock().unwrap();
        assert!(
            inner
                .performed
                .iter()
                .any(|j| j.job_type == job_type && j.state == "completed"),
            "Expected a completed job of type '{}', found none",
            job_type
        );
    }

    /// Return all enqueued jobs.
    pub fn all_enqueued(&self) -> Vec<FakeJob> {
        self.inner.lock().unwrap().enqueued.clone()
    }

    /// Return all enqueued jobs, optionally filtered by type and/or match
    /// criteria (queue/args/meta).
    ///
    /// `ojs-testing.md` §6.1: "Returns all jobs enqueued in the current
    /// test context, optionally filtered by type, queue, or args. This is
    /// not an assertion -- it returns data for custom assertions."
    ///
    /// Type and criteria are independent filters: passing `None` for
    /// `job_type` still applies the supplied queue/args/meta criteria across
    /// every recorded job type.
    pub fn all_enqueued_matching(
        &self,
        job_type: Option<&str>,
        criteria: Option<&MatchCriteria>,
    ) -> Vec<FakeJob> {
        let inner = self.inner.lock().unwrap();
        filter_jobs(&inner.enqueued, job_type, criteria)
            .into_iter()
            .cloned()
            .collect()
    }

    /// Clear all enqueued and performed jobs.
    pub fn clear_all(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.enqueued.clear();
        inner.performed.clear();
    }

    /// Process all available jobs using registered handlers.
    ///
    /// Handlers are invoked *without* holding the store's internal lock, so
    /// a handler that calls back into this same [`FakeStore`] (e.g.
    /// [`record_enqueue`](Self::record_enqueue), to simulate a chained
    /// follow-up job -- a natural pattern when testing workflow-like
    /// behavior) does not deadlock against the non-reentrant
    /// `std::sync::Mutex` guarding it.
    pub fn drain(&self) -> usize {
        // Phase 1: under the lock, mark every currently-available job
        // "active", snapshot it, and grab a cheap `Arc`-cloned handler
        // reference (if any) for it. This is the only phase that touches
        // the shared state before handlers run.
        struct Runnable {
            job: FakeJob,
            handler: Option<JobHandler>,
        }

        enum HandlerOutcome {
            Processed(Result<(), String>),
            Deferred,
        }

        let runnable: Vec<Runnable> = {
            let mut inner = self.inner.lock().unwrap();
            let mut runnable = Vec::new();
            for i in 0..inner.enqueued.len() {
                if inner.enqueued[i].state != "available" {
                    continue;
                }
                inner.enqueued[i].state = "active".to_string();
                inner.enqueued[i].attempt += 1;
                let job = inner.enqueued[i].clone();
                let handler = inner.handlers.get(&job.job_type).cloned();
                runnable.push(Runnable { job, handler });
            }
            runnable
        };

        // Phase 2: run each handler with no lock held at all. A handler
        // that calls `record_enqueue`/`assert_*`/`drain` itself acquires
        // the lock fresh, on its own, exactly like any other caller.
        let outcomes: Vec<HandlerOutcome> = runnable
            .iter()
            .map(|r| match &r.handler {
                Some(handler) => match handler.try_lock() {
                    Ok(handler) => HandlerOutcome::Processed(handler(&r.job)),
                    Err(std::sync::TryLockError::WouldBlock) => HandlerOutcome::Deferred,
                    Err(std::sync::TryLockError::Poisoned(_)) => HandlerOutcome::Processed(Err(
                        "registered fake job handler lock is poisoned".to_string(),
                    )),
                },
                None => HandlerOutcome::Processed(Ok(())),
            })
            .collect();

        // Phase 3: re-acquire the lock only to record each outcome.
        // Look jobs up by their stable ID rather than carrying raw indices
        // across the unlocked handler calls: a reentrant/concurrent
        // `clear_all()` may remove the snapshot while a handler is running.
        let mut inner = self.inner.lock().unwrap();
        let mut processed = 0;
        for (r, outcome) in runnable.into_iter().zip(outcomes) {
            match outcome {
                HandlerOutcome::Processed(result) => {
                    processed += 1;
                    if let Some(job) = inner.enqueued.iter_mut().find(|job| job.id == r.job.id) {
                        job.state = match result {
                            Ok(()) => "completed".to_string(),
                            Err(_) => "discarded".to_string(),
                        };
                        let performed_job = job.clone();
                        inner.performed.push(performed_job);
                    }
                }
                HandlerOutcome::Deferred => {
                    // A recursive/concurrent drain is already invoking this
                    // non-Sync handler. Leave the job available for a later
                    // drain instead of blocking on the per-handler mutex.
                    if let Some(job) = inner.enqueued.iter_mut().find(|job| job.id == r.job.id) {
                        job.state = "available".to_string();
                        job.attempt -= 1;
                    }
                }
            }
        }

        processed
    }
}

/// Filter recorded jobs by an optional job type and optional match
/// criteria.
///
/// The two are **independent** predicates: the job type (when supplied)
/// narrows by `job.job_type`, and the criteria (when supplied) narrow by
/// queue/args/meta. Criteria are therefore honored for a type-less query
/// too -- previously `all_enqueued_matching(None, Some(&criteria))` silently
/// returned every recorded job, ignoring the caller's queue/args/meta
/// filter entirely.
fn filter_jobs<'a>(
    jobs: &'a [FakeJob],
    job_type: Option<&str>,
    criteria: Option<&MatchCriteria>,
) -> Vec<&'a FakeJob> {
    jobs.iter()
        .filter(|j| matches_job_type(j, job_type) && matches_criteria(j, criteria))
        .collect()
}

fn matches_job_type(job: &FakeJob, job_type: Option<&str>) -> bool {
    match job_type {
        Some(job_type) => job.job_type == job_type,
        None => true,
    }
}

fn matches_criteria(job: &FakeJob, criteria: Option<&MatchCriteria>) -> bool {
    let Some(c) = criteria else {
        return true;
    };

    if let Some(ref q) = c.queue {
        if job.queue != *q {
            return false;
        }
    }
    // `ojs-testing.md` §6.1: `assert_enqueued`'s `options.args`
    // criterion is "Expected args (deep equality)". This was
    // previously accepted but never actually checked, so an
    // assertion for the wrong args could pass as long as the
    // job type/queue matched.
    if let Some(ref expected_args) = c.args {
        if &job.args != expected_args {
            return false;
        }
    }
    // Meta is matched as a subset: every key/value pair in the
    // criteria must be present in the job's meta (extra keys on
    // the job are ignored), matching how most fake-mode
    // assertion helpers treat metadata across the OJS SDKs.
    if let Some(ref expected_meta) = c.meta {
        for (key, value) in expected_meta {
            if job.meta.get(key) != Some(value) {
                return false;
            }
        }
    }

    true
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
