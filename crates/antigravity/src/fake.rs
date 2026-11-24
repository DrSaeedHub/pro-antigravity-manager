//! A pretend process host for tests and previews.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::host::{AntigravityError, AppHost};

#[derive(Debug, Default)]
struct State {
    processes: usize,
    ignore_close: bool,
    survive_kill: bool,
    fail_start: bool,
    close_requests: usize,
    kills: usize,
    started: Vec<PathBuf>,
}

/// Simulated Antigravity processes.
#[derive(Debug, Default)]
pub struct FakeHost {
    state: Mutex<State>,
}

impl FakeHost {
    /// Nothing running.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// `count` processes running.
    #[must_use]
    pub fn with_processes(count: usize) -> Self {
        let host = Self::new();
        host.state().processes = count;
        host
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Processes stay open when asked to close.
    pub fn ignore_close_requests(&self) {
        self.state().ignore_close = true;
    }

    /// Processes survive being ended.
    pub fn survive_kills(&self) {
        self.state().survive_kill = true;
    }

    /// Starting fails.
    pub fn fail_starts(&self) {
        self.state().fail_start = true;
    }

    /// How many times closing was requested.
    #[must_use]
    pub fn close_requests(&self) -> usize {
        self.state().close_requests
    }

    /// How many times processes were ended.
    #[must_use]
    pub fn kills(&self) -> usize {
        self.state().kills
    }

    /// Every executable started, in order.
    #[must_use]
    pub fn started(&self) -> Vec<PathBuf> {
        self.state().started.clone()
    }
}

impl AppHost for FakeHost {
    fn running(&self) -> usize {
        self.state().processes
    }

    fn request_close(&self) {
        let mut state = self.state();
        state.close_requests += 1;
        if !state.ignore_close {
            state.processes = 0;
        }
    }

    fn kill(&self) {
        let mut state = self.state();
        state.kills += 1;
        if !state.survive_kill {
            state.processes = 0;
        }
    }

    fn start(&self, exe: &Path) -> Result<(), AntigravityError> {
        let mut state = self.state();
        if state.fail_start {
            return Err(AntigravityError::StartFailed("refused by the test".into()));
        }
        state.started.push(exe.to_path_buf());
        state.processes += 1;
        Ok(())
    }
}
