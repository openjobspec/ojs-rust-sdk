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
