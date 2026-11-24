//! The composition root: the only place that knows every module and picks the real adapters.

use std::path::PathBuf;
use std::sync::Arc;

use accounts::{AccountSettings, Accounts};
use antigravity::{Antigravity, AppHost, HostSettings};
use desktop::{Desktop, DesktopSettings, Shell};
use kernel::{Clock, KnownFolders};
use net::ProxyResolver;
use quota::client::ClientCache;
use quota::{QuotaService, QuotaSettings, SecretBox};
use update::{UpdateSettings, Updater, VersionProbe};
use vault::CredentialStore;

/// Everything the UI needs.
#[derive(Debug, Clone)]
pub struct Services {
    /// Saved accounts.
    pub accounts: Accounts,
    /// Quota reading.
    pub quota: Arc<QuotaService>,
    /// New versions from GitHub.
    pub updater: Arc<Updater>,
    /// Time and the local zone.
    pub clock: Arc<dyn Clock>,
    /// Background work.
    pub runtime: Arc<tokio::runtime::Runtime>,
}

/// The app could not start.
#[derive(Debug, thiserror::Error)]
pub enum BootError {
    /// A folder the app needs could not be found.
    #[error(transparent)]
    Desktop(#[from] desktop::DesktopError),
    /// The HTTP client could not be built.
    #[error(transparent)]
    Http(#[from] quota::QuotaError),
    /// The update client could not be built.
    #[error(transparent)]
    Update(#[from] update::UpdateError),
    /// The background runtime could not start.
    #[error("the background runtime could not start: {0}")]
    Runtime(#[from] std::io::Error),
}

/// The adapters to build the services from.
#[derive(Debug, Clone)]
pub struct Parts {
    /// Credentials.
    pub store: Arc<dyn CredentialStore>,
    /// Antigravity's processes.
    pub host: Arc<dyn AppHost>,
    /// Known folders.
    pub folders: Arc<dyn KnownFolders>,
    /// Shortcuts.
    pub shell: Arc<dyn Shell>,
    /// Encryption for the sign-in client cache.
    pub secret_box: Arc<dyn SecretBox>,
    /// The proxy for Google.
    pub proxy: Arc<dyn ProxyResolver>,
    /// Time.
    pub clock: Arc<dyn Clock>,
    /// The running executable.
    pub current_exe: PathBuf,
    /// Quota endpoints (tests point them elsewhere).
    pub quota_settings: QuotaSettings,
    /// Where releases come from (tests point it elsewhere).
    pub update_settings: UpdateSettings,
    /// Asks a downloaded executable for its version.
    pub probe: Arc<dyn VersionProbe>,
}

/// Builds the services from `parts`.
pub fn build(parts: Parts) -> Result<Services, BootError> {
    let antigravity = Antigravity::new(parts.host, parts.folders.clone(), HostSettings::default());
    let desktop = Desktop::new(
        parts.shell,
        parts.folders,
        antigravity.clone(),
        parts.current_exe,
        DesktopSettings::default(),
    );
    let cache = ClientCache::new(
        desktop.data_dir()?.join("oauth-client.bin"),
        parts.secret_box,
        &parts.quota_settings.cache_entropy,
    );
    let http = parts.quota_settings.http_client(parts.proxy.clone())?;
    let updater = Updater::new(parts.update_settings, parts.proxy, parts.probe)?;
    let accounts = Accounts::new(
        parts.store.clone(),
        antigravity.clone(),
        desktop,
        AccountSettings::default(),
    );
    let quota = QuotaService::new(
        http,
        parts.store,
        antigravity,
        cache,
        parts.clock.clone(),
        parts.quota_settings,
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("switcher-worker")
        .enable_all()
        .build()?;
    Ok(Services {
        accounts,
        quota: Arc::new(quota),
        updater: Arc::new(updater),
        clock: parts.clock,
        runtime: Arc::new(runtime),
    })
}

/// The real Windows adapters.
#[cfg(windows)]
#[must_use]
pub fn windows_parts() -> Parts {
    Parts {
        store: Arc::new(vault::WindowsStore::new()),
        host: Arc::new(antigravity::WindowsHost::antigravity()),
        folders: Arc::new(kernel::SystemFolders),
        shell: Arc::new(desktop::WindowsShell),
        secret_box: Arc::new(quota::Dpapi),
        proxy: Arc::new(net::SystemProxy::new()),
        clock: Arc::new(kernel::SystemClock),
        current_exe: std::env::current_exe()
            .unwrap_or_else(|_| PathBuf::from("AntigravityAccountSwitcher.exe")),
        quota_settings: QuotaSettings::default(),
        update_settings: UpdateSettings::default(),
        probe: Arc::new(update::RunProbe::default()),
    }
}

/// Pretend adapters under `root`, for previews and tests: nothing real is touched.
#[must_use]
pub fn preview_parts(root: &std::path::Path) -> Parts {
    Parts {
        store: Arc::new(vault::MemoryStore::new()),
        host: Arc::new(antigravity::FakeHost::new()),
        folders: Arc::new(kernel::FixedFolders::new(root)),
        shell: Arc::new(desktop::FakeShell::new()),
        secret_box: Arc::new(quota::FakeBox),
        proxy: Arc::new(net::NoProxy),
        clock: Arc::new(kernel::SystemClock),
        current_exe: root.join("AntigravityAccountSwitcher.exe"),
        quota_settings: QuotaSettings {
            token_url: "http://127.0.0.1:9/token".into(),
            api_base: "http://127.0.0.1:9/v1internal:".into(),
            ..QuotaSettings::default()
        },
        update_settings: UpdateSettings {
            repository: "http://127.0.0.1:9/preview/repo".into(),
            ..UpdateSettings::default()
        },
        probe: Arc::new(update::FakeProbe::answering(env!("CARGO_PKG_VERSION"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_services_build_and_touch_nothing_real() {
        let dir = tempfile::tempdir().unwrap();
        let services = build(preview_parts(dir.path())).unwrap();
        assert!(services.accounts.list().unwrap().is_empty());
        assert!(!services.accounts.antigravity().is_running());
        assert_eq!(
            services.accounts.desktop().data_dir().unwrap(),
            dir.path().join("LocalAppData").join("AntigravityAccountSwitcher")
        );
    }

    #[test]
    fn a_missing_data_folder_stops_the_start() {
        #[derive(Debug)]
        struct NoFolders;
        impl KnownFolders for NoFolders {
            fn path(&self, folder: kernel::Folder) -> Result<PathBuf, kernel::FolderError> {
                Err(kernel::FolderError { folder: folder.name() })
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let parts = Parts { folders: Arc::new(NoFolders), ..preview_parts(dir.path()) };
        let error = build(parts).unwrap_err();
        assert!(error.to_string().contains("LocalAppData"), "{error}");
    }
}
