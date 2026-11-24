//! A shell that records shortcuts and writes placeholder files, for tests and previews.

use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::shell::{DesktopError, Shell, Shortcut};

#[derive(Debug, Default)]
struct State {
    created: Vec<Shortcut>,
    fail: bool,
}

/// Records every shortcut and writes a small file at its path.
#[derive(Debug, Default)]
pub struct FakeShell {
    state: Mutex<State>,
}

impl FakeShell {
    /// A working shell.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Every later shortcut fails.
    pub fn fail(&self) {
        self.state().fail = true;
    }

    /// The shortcuts created so far.
    #[must_use]
    pub fn created(&self) -> Vec<Shortcut> {
        self.state().created.clone()
    }
}

impl Shell for FakeShell {
    fn create_shortcut(&self, shortcut: &Shortcut) -> Result<(), DesktopError> {
        let mut state = self.state();
        let fail = |reason: String| DesktopError::Shortcut { path: shortcut.path.clone(), reason };
        if state.fail {
            return Err(fail("refused by the test".into()));
        }
        if let Some(folder) = shortcut.path.parent() {
            std::fs::create_dir_all(folder).map_err(|error| fail(error.to_string()))?;
        }
        std::fs::write(&shortcut.path, shortcut.arguments.as_bytes())
            .map_err(|error| fail(error.to_string()))?;
        state.created.push(shortcut.clone());
        Ok(())
    }
}
