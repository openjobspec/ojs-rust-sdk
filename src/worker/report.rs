//! Per-job terminal-reporting state: the phase machine that guarantees a job
//! is ACK'd or NACK'd exactly once, even when a graceful shutdown races an
//! in-flight report.
//!
//! A single "claimed" flag cannot express the state that shutdown actually
//! needs to make a decision: a job whose report is *already in flight* must
//! be awaited (its ACK may still succeed), while a job that never started
//! reporting must be force-NACKed immediately. This module therefore tracks
//! three phases per active job:
//!
//! - [`ReportPhase::Unclaimed`] — no terminal report has been started.
//! - [`ReportPhase::Reporting`] — exactly one owner is reporting right now.
//! - [`ReportPhase::Completed`] — a terminal report succeeded (absorbing).
//!
//! Ownership is transferred only through the mutex-guarded transitions below,
//! so at most one party (the handler task or the shutdown path) can ever be
//! the reporter for a given job.

use std::sync::Mutex;
use tokio::task::JoinHandle;

/// The terminal-reporting phase of one active job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReportPhase {
    /// No ACK/NACK has been started for this job yet.
    Unclaimed,
    /// A terminal report is in flight; its owner holds exclusive reporting
    /// rights until it completes, fails, or is cancelled.
    Reporting,
    /// A terminal report was accepted by the server. Absorbing: no further
    /// report may be sent for this job.
    Completed,
}

/// What the shutdown path is allowed to do with a job that is still active
/// when the grace period expires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShutdownClaim {
    /// The job had not started reporting; shutdown now owns its terminal
    /// report exclusively and must emit exactly one forced NACK.
    Forced,
    /// A terminal report was already in flight. Shutdown must await it
    /// (bounded) instead of racing it with a second report.
    ReportInFlight,
    /// The job was already reported successfully; nothing left to do.
    AlreadyReported,
}

/// Shared per-job reporting state, held both by the job's handler task and
/// by the worker's active-job registry (and therefore by shutdown).
#[derive(Debug)]
pub(crate) struct ActiveJobState {
    inner: Mutex<Inner>,
}

#[derive(Debug)]
struct Inner {
    phase: ReportPhase,
    /// Join handle of the detached task performing the in-flight report.
    ///
    /// Present only while a handler-owned report is running: it lets the
    /// shutdown path await that report and, if it never settles, cancel it
    /// before taking ownership. `None` while shutdown itself owns the
    /// report (it awaits its own future directly).
    task: Option<JoinHandle<()>>,
    /// Last terminal-report failure, retained for shutdown diagnostics.
    last_error: Option<String>,
}

impl Default for ActiveJobState {
    fn default() -> Self {
        Self::new()
    }
}

impl ActiveJobState {
    pub(crate) fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                phase: ReportPhase::Unclaimed,
                task: None,
                last_error: None,
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The job's current reporting phase.
    pub(crate) fn phase(&self) -> ReportPhase {
        self.lock().phase
    }

    /// The last recorded terminal-report failure, if any.
    pub(crate) fn last_error(&self) -> Option<String> {
        self.lock().last_error.clone()
    }

    /// Atomically enter [`ReportPhase::Reporting`] and start the report.
    ///
    /// `spawn_report` is invoked **only** when this caller won the claim, and
    /// runs while the phase lock is held so the phase change and the
    /// resulting task handle become visible together: shutdown can never
    /// observe `Reporting` without also being able to await/cancel the task
    /// that owns it.
    ///
    /// Returns `false` when another owner is already reporting or the job
    /// has already been reported, in which case no report is started.
    pub(crate) fn begin_report<F>(&self, spawn_report: F) -> bool
    where
        F: FnOnce() -> JoinHandle<()>,
    {
        let mut inner = self.lock();
        if inner.phase != ReportPhase::Unclaimed {
            return false;
        }
        inner.phase = ReportPhase::Reporting;
        inner.task = Some(spawn_report());
        true
    }

    /// Record the outcome of a report this caller owned.
    ///
    /// Success is absorbing. Failure releases ownership back to
    /// [`ReportPhase::Unclaimed`] and records the error, so the worker's
    /// shutdown path may still attempt a forced release for the job.
    pub(crate) fn finish_report(&self, outcome: Result<(), String>) {
        let mut inner = self.lock();
        inner.task = None;
        match outcome {
            Ok(()) => {
                inner.phase = ReportPhase::Completed;
                inner.last_error = None;
            }
            Err(error) => {
                inner.phase = ReportPhase::Unclaimed;
                inner.last_error = Some(error);
            }
        }
    }

    /// Record the outcome of a shutdown-owned forced report.
    ///
    /// Unlike a handler-owned report, failure must not release ownership:
    /// the request may have reached the server before its local future
    /// failed or timed out. Keeping the phase at [`ReportPhase::Reporting`]
    /// prevents a later-resuming handler from sending a potentially
    /// duplicate ACK/NACK after shutdown returns.
    pub(crate) fn finish_shutdown_report(&self, outcome: Result<(), String>) {
        let mut inner = self.lock();
        inner.task = None;
        match outcome {
            Ok(()) => {
                inner.phase = ReportPhase::Completed;
                inner.last_error = None;
            }
            Err(error) => {
                inner.phase = ReportPhase::Reporting;
                inner.last_error = Some(error);
            }
        }
    }

    /// Decide -- atomically -- what the shutdown path may do with this job.
    ///
    /// A job that had not begun reporting is claimed here (before any
    /// handler task is aborted), so a handler that resumes later from
    /// blocking or CPU-bound work observes `Reporting` and skips its own
    /// ACK/NACK.
    pub(crate) fn claim_for_shutdown(&self) -> ShutdownClaim {
        let mut inner = self.lock();
        match inner.phase {
            ReportPhase::Unclaimed => {
                inner.phase = ReportPhase::Reporting;
                ShutdownClaim::Forced
            }
            ReportPhase::Reporting => ShutdownClaim::ReportInFlight,
            ReportPhase::Completed => ShutdownClaim::AlreadyReported,
        }
    }

    /// Take the in-flight report's join handle so the shutdown path can
    /// await it and, if necessary, cancel it.
    pub(crate) fn take_report_task(&self) -> Option<JoinHandle<()>> {
        self.lock().task.take()
    }

    /// Take ownership of terminal reporting after the previously in-flight
    /// report has definitively stopped running (it completed, failed, or was
    /// cancelled *and joined*).
    ///
    /// Returns `false` when that report already succeeded, which is the only
    /// case where a forced NACK would be a duplicate. Callers must not call
    /// this while the previous reporter could still be running.
    pub(crate) fn claim_after_report_settled(&self) -> bool {
        let mut inner = self.lock();
        if inner.phase == ReportPhase::Completed {
            return false;
        }
        inner.phase = ReportPhase::Reporting;
        inner.task = None;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn noop_task() -> JoinHandle<()> {
        tokio::spawn(std::future::ready(()))
    }

    #[tokio::test]
    async fn test_new_state_is_unclaimed() {
        let state = ActiveJobState::new();
        assert_eq!(state.phase(), ReportPhase::Unclaimed);
        assert!(state.last_error().is_none());
    }

    #[tokio::test]
    async fn test_begin_report_is_exclusive() {
        let state = ActiveJobState::new();
        assert!(state.begin_report(noop_task));
        assert_eq!(state.phase(), ReportPhase::Reporting);
        assert!(
            !state.begin_report(noop_task),
            "a second owner must not be able to start a terminal report"
        );
    }

    #[tokio::test]
    async fn test_successful_report_is_absorbing() {
        let state = ActiveJobState::new();
        assert!(state.begin_report(noop_task));
        state.finish_report(Ok(()));

        assert_eq!(state.phase(), ReportPhase::Completed);
        assert!(!state.begin_report(noop_task));
        assert_eq!(state.claim_for_shutdown(), ShutdownClaim::AlreadyReported);
        assert!(!state.claim_after_report_settled());
    }

    #[tokio::test]
    async fn test_failed_report_releases_ownership_and_records_error() {
        let state = ActiveJobState::new();
        assert!(state.begin_report(noop_task));
        state.finish_report(Err("connection reset".to_string()));

        assert_eq!(state.phase(), ReportPhase::Unclaimed);
        assert_eq!(state.last_error().as_deref(), Some("connection reset"));
        assert_eq!(
            state.claim_for_shutdown(),
            ShutdownClaim::Forced,
            "a released job must still be force-releasable at shutdown"
        );
    }

    #[tokio::test]
    async fn test_failed_shutdown_report_retains_ownership_and_records_error() {
        let state = ActiveJobState::new();
        assert_eq!(state.claim_for_shutdown(), ShutdownClaim::Forced);
        state.finish_shutdown_report(Err("deadline elapsed".to_string()));

        assert_eq!(state.phase(), ReportPhase::Reporting);
        assert_eq!(state.last_error().as_deref(), Some("deadline elapsed"));
        assert!(
            !state.begin_report(noop_task),
            "a late handler must not report after shutdown's attempt may have reached the server"
        );
    }

    #[tokio::test]
    async fn test_shutdown_claim_blocks_a_later_handler_report() {
        let state = ActiveJobState::new();
        assert_eq!(state.claim_for_shutdown(), ShutdownClaim::Forced);
        assert!(
            !state.begin_report(noop_task),
            "a handler resuming after the forced claim must not report"
        );
    }

    #[tokio::test]
    async fn test_shutdown_observes_in_flight_report_with_its_task() {
        let state = ActiveJobState::new();
        assert!(state.begin_report(|| tokio::spawn(std::future::pending::<()>())));

        assert_eq!(state.claim_for_shutdown(), ShutdownClaim::ReportInFlight);
        let handle = state
            .take_report_task()
            .expect("an in-flight report must expose its task to shutdown");
        handle.abort();
        let _ = handle.await;

        assert!(
            state.claim_after_report_settled(),
            "a cancelled report leaves the job unreported, so shutdown takes over"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_concurrent_owners_produce_exactly_one_report() {
        for _ in 0..256 {
            let state = Arc::new(ActiveJobState::new());
            let owners = Arc::new(AtomicUsize::new(0));

            let mut tasks = Vec::new();
            for _ in 0..8 {
                let state = Arc::clone(&state);
                let owners = Arc::clone(&owners);
                tasks.push(tokio::spawn(async move {
                    if state.begin_report(noop_task) {
                        owners.fetch_add(1, Ordering::SeqCst);
                    }
                }));
            }
            let shutdown_state = Arc::clone(&state);
            let shutdown_owners = Arc::clone(&owners);
            tasks.push(tokio::spawn(async move {
                if shutdown_state.claim_for_shutdown() == ShutdownClaim::Forced {
                    shutdown_owners.fetch_add(1, Ordering::SeqCst);
                }
            }));

            for task in tasks {
                task.await.unwrap();
            }

            assert_eq!(
                owners.load(Ordering::SeqCst),
                1,
                "exactly one party may own a job's terminal report"
            );
        }
    }
}
