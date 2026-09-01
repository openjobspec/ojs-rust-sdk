//! Worker lifecycle state: the [`WorkerState`] enum.

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
