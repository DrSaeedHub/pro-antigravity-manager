//! The process port and the `Antigravity` service built on it.

use std::fmt::Debug;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use kernel::KnownFolders;

use crate::install;

/// Something went wrong with the Antigravity app.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AntigravityError {
    /// Antigravity is not installed in any known place.
    #[error("Antigravity was not found on this PC")]
    NotInstalled,
    /// Some Antigravity processes were still running after the polite close and the forced one.
    #[error("Antigravity could not be closed completely")]
    StillRunning,
    /// Windows could not start the executable.
    #[error("Antigravity could not be started: {0}")]
    StartFailed(String),
}

impl AntigravityError {
    /// A stable code for logs and the UI.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NotInstalled => "antigravity.not_installed",
            Self::StillRunning => "antigravity.still_running",
            Self::StartFailed(_) => "antigravity.start_failed",
        }
    }
}

/// The operating-system side: processes of one executable name.
pub trait AppHost: Send + Sync + Debug {
    /// How many processes are running.
    fn running(&self) -> usize;
    /// Asks every main window of the processes to close (like clicking its close button).
    fn request_close(&self);
    /// Ends every remaining process.
    fn kill(&self);
    /// Starts `exe` with no arguments, from its own folder.
    fn start(&self, exe: &Path) -> Result<(), AntigravityError>;
}

/// How long to wait while closing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostSettings {
    /// After asking the windows to close.
    pub close_wait: Duration,
    /// After ending the processes.
    pub kill_wait: Duration,
    /// Between checks.
    pub poll: Duration,
}

impl Default for HostSettings {
    fn default() -> Self {
        Self {
            close_wait: Duration::from_secs(8),
            kill_wait: Duration::from_secs(8),
            poll: Duration::from_millis(250),
        }
    }
}

/// Antigravity on this PC.
#[derive(Debug, Clone)]
pub struct Antigravity {
    host: Arc<dyn AppHost>,
    folders: Arc<dyn KnownFolders>,
    settings: HostSettings,
}

impl Antigravity {
    /// The service over a process host and the known folders.
    #[must_use]
    pub fn new(
        host: Arc<dyn AppHost>,
        folders: Arc<dyn KnownFolders>,
        settings: HostSettings,
    ) -> Self {
        Self { host, folders, settings }
    }

    /// The installed executable, if any.
    #[must_use]
    pub fn executable(&self) -> Option<PathBuf> {
        install::locate(self.folders.as_ref())
    }

    /// Whether any Antigravity process runs.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.host.running() > 0
    }

    /// Closes every Antigravity process: asks the windows to close, waits, ends what is left,
    /// waits again. Errors when something still runs.
    pub fn close(&self) -> Result<(), AntigravityError> {
        if !self.is_running() {
            return Ok(());
        }
        tracing::info!("closing Antigravity");
        self.host.request_close();
        if self.wait_until_closed(self.settings.close_wait) {
            return Ok(());
        }
        tracing::warn!("Antigravity did not close in time; ending its processes");
        self.host.kill();
        if self.wait_until_closed(self.settings.kill_wait) {
            Ok(())
        } else {
            Err(AntigravityError::StillRunning)
        }
    }

    fn wait_until_closed(&self, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        loop {
            if !self.is_running() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(self.settings.poll);
        }
    }

    /// Starts the installed Antigravity.
    pub fn launch(&self) -> Result<(), AntigravityError> {
        let exe = self.executable().ok_or(AntigravityError::NotInstalled)?;
        tracing::info!(path = %exe.display(), "starting Antigravity");
        self.host.start(&exe)
    }

    /// The language-server files of the installed Antigravity.
    #[must_use]
    pub fn language_server_files(&self) -> Vec<PathBuf> {
        self.executable().map(|exe| install::language_server_files(&exe)).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use kernel::FixedFolders;

    use super::*;
    use crate::fake::FakeHost;

    fn quick() -> HostSettings {
        HostSettings {
            close_wait: Duration::from_millis(30),
            kill_wait: Duration::from_millis(30),
            poll: Duration::from_millis(1),
        }
    }

    fn service(host: &Arc<FakeHost>, root: &Path) -> Antigravity {
        Antigravity::new(host.clone(), Arc::new(FixedFolders::new(root)), quick())
    }

    fn install(root: &Path) -> PathBuf {
        let exe = root.join("LocalAppData\\Programs\\antigravity\\Antigravity.exe");
        fs::create_dir_all(exe.parent().unwrap()).unwrap();
        fs::write(&exe, b"x").unwrap();
        exe
    }

    #[test]
    fn closing_nothing_does_nothing() {
        let root = tempfile::tempdir().unwrap();
        let host = Arc::new(FakeHost::new());
        service(&host, root.path()).close().unwrap();
        assert_eq!(host.close_requests(), 0);
        assert_eq!(host.kills(), 0);
    }

    #[test]
    fn a_polite_close_is_enough_when_windows_close() {
        let root = tempfile::tempdir().unwrap();
        let host = Arc::new(FakeHost::with_processes(3));
        let antigravity = service(&host, root.path());
        assert!(antigravity.is_running());
        antigravity.close().unwrap();
        assert!(!antigravity.is_running());
        assert_eq!(host.close_requests(), 1);
        assert_eq!(host.kills(), 0);
    }

    #[test]
    fn stubborn_processes_are_ended() {
        let root = tempfile::tempdir().unwrap();
        let host = Arc::new(FakeHost::with_processes(2));
        host.ignore_close_requests();
        service(&host, root.path()).close().unwrap();
        assert_eq!(host.close_requests(), 1);
        assert_eq!(host.kills(), 1);
    }

    #[test]
    fn unkillable_processes_are_reported() {
        let root = tempfile::tempdir().unwrap();
        let host = Arc::new(FakeHost::with_processes(1));
        host.ignore_close_requests();
        host.survive_kills();
        let error = service(&host, root.path()).close().unwrap_err();
        assert_eq!(error, AntigravityError::StillRunning);
        assert_eq!(error.code(), "antigravity.still_running");
    }

    #[test]
    fn launch_starts_the_installed_executable() {
        let root = tempfile::tempdir().unwrap();
        let host = Arc::new(FakeHost::new());
        let antigravity = service(&host, root.path());
        assert_eq!(antigravity.launch().unwrap_err(), AntigravityError::NotInstalled);

        let exe = install(root.path());
        assert_eq!(antigravity.executable(), Some(exe.clone()));
        antigravity.launch().unwrap();
        assert_eq!(host.started(), vec![exe]);
        assert!(antigravity.is_running());
    }

    #[test]
    fn launch_reports_start_failures() {
        let root = tempfile::tempdir().unwrap();
        install(root.path());
        let host = Arc::new(FakeHost::new());
        host.fail_starts();
        let error = service(&host, root.path()).launch().unwrap_err();
        assert_eq!(error.code(), "antigravity.start_failed");
        assert_eq!(error.to_string(), "Antigravity could not be started: refused by the test");
    }

    #[test]
    fn language_servers_come_from_the_installed_app() {
        let root = tempfile::tempdir().unwrap();
        let host = Arc::new(FakeHost::new());
        let antigravity = service(&host, root.path());
        assert!(antigravity.language_server_files().is_empty());
        let exe = install(root.path());
        let server = exe.parent().unwrap().join("resources\\bin\\language_server.exe");
        fs::create_dir_all(server.parent().unwrap()).unwrap();
        fs::write(&server, b"x").unwrap();
        assert_eq!(antigravity.language_server_files(), vec![server]);
    }

    #[test]
    fn errors_have_codes_and_default_settings_match_the_reference() {
        assert_eq!(AntigravityError::NotInstalled.code(), "antigravity.not_installed");
        assert_eq!(
            AntigravityError::NotInstalled.to_string(),
            "Antigravity was not found on this PC"
        );
        let settings = HostSettings::default();
        assert_eq!(settings.close_wait, Duration::from_secs(8));
        assert_eq!(settings.kill_wait, Duration::from_secs(8));
        assert_eq!(settings.poll, Duration::from_millis(250));
    }
}
