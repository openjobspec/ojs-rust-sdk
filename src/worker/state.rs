//! Worker lifecycle state: the [`WorkerState`] enum and the small state
//! machine enforcing that `Terminate` is an absorbing transition.

use std::sync::atomic::{AtomicU8, Ordering};

/// The lifecycle state of a worker.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum WorkerState {
    /// Normal operation, actively fetching and processing jobs.
    Running = 0,
    /// No longer fetching new jobs, finishing active ones.
    Quiet = 1,
    /// Shutting down.
    Terminate = 2,
}

impl WorkerState {
    pub(crate) fn from_u8(v: u8) -> Self {
        match v {
            0 => WorkerState::Running,
            1 => WorkerState::Quiet,
            _ => WorkerState::Terminate,
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            WorkerState::Running => "running",
            WorkerState::Quiet => "quiet",
            WorkerState::Terminate => "terminate",
        }
    }
}

impl std::fmt::Display for WorkerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Attempt to transition the worker's shared lifecycle state to `new`.
///
/// `Terminate` is an absorbing state: once set (locally, via
/// [`crate::worker::Worker::shutdown`], or by a server-directed heartbeat
/// response), no later transition -- local or server-directed -- can move
/// the worker back to `Quiet` or `Running`. Without this, a heartbeat
/// response that was already in flight when a local shutdown set
/// `Terminate` could land afterward and silently revert the state.
///
/// Usable both from a method holding `&Worker` and from the detached
/// heartbeat task, which only holds a cloned `Arc<AtomicU8>` rather than
/// `&self`.
pub(crate) fn transition_shared(state: &AtomicU8, new: WorkerState) {
    let mut current = state.load(Ordering::SeqCst);
    loop {
        if current == WorkerState::Terminate as u8 {
            return;
        }
        match state.compare_exchange_weak(current, new as u8, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return,
            Err(actual) => current = actual,
        }
    }
}
