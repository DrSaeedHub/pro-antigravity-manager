//! Account shortcuts, the local install, and the app's data folder.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use antigravity::Antigravity;
use kernel::{Folder, KnownFolders};

use crate::preferences::PreferenceStore;
use crate::shell::{DesktopError, Shell, Shortcut};

/// Names used on disk (kept from the original app, so its shortcuts and data keep working).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopSettings {
    /// Folder under Local AppData that holds the installed exe and the app's files.
    pub data_folder: String,
    /// File name of the installed executable.
    pub executable: String,
    /// The app shortcut on the desktop and in the Start menu.
    pub app_shortcut: String,
    /// The Start-menu folder.
    pub start_menu_folder: String,
    /// Account shortcuts are named `<prefix><account>.lnk`.
    pub account_shortcut_prefix: String,
    /// The command-line switch an account shortcut passes, before the account name.
    pub launch_switch: String,
    /// The command-line switch that starts the app in the tray (used when starting with Windows).
    pub tray_switch: String,
    /// The settings file in the data folder.
    pub preferences_file: String,
}

impl Default for DesktopSettings {
    fn default() -> Self {
        Self {
            data_folder: "AntigravityAccountSwitcher".into(),
            executable: "AntigravityAccountSwitcher.exe".into(),
            app_shortcut: "Antigravity Account Switcher.lnk".into(),
            start_menu_folder: "Antigravity Account Switcher".into(),
            account_shortcut_prefix: "Antigravity - ".into(),
            launch_switch: "--launch".into(),
            tray_switch: "--tray".into(),
            preferences_file: "settings.json".into(),
        }
    }
}

/// Desktop integration for one running copy of the app (`current_exe`).
#[derive(Debug, Clone)]
pub struct Desktop {
    shell: Arc<dyn Shell>,
    folders: Arc<dyn KnownFolders>,
    antigravity: Antigravity,
    current_exe: PathBuf,
    settings: DesktopSettings,
}

impl Desktop {
    /// The service. `current_exe` is the executable that is running now.
    #[must_use]
    pub fn new(
        shell: Arc<dyn Shell>,
        folders: Arc<dyn KnownFolders>,
        antigravity: Antigravity,
        current_exe: PathBuf,
        settings: DesktopSettings,
    ) -> Self {
        Self { shell, folders, antigravity, current_exe, settings }
    }

    /// The executable that is running now.
    #[must_use]
    pub fn current_exe(&self) -> &Path {
        &self.current_exe
    }

    /// Opens a web page in the default browser.
    pub fn open(&self, address: &str) -> Result<(), DesktopError> {
        self.shell.open(address)
    }

    fn folder(&self, folder: Folder) -> Result<PathBuf, DesktopError> {
        self.folders.path(folder).map_err(|error| DesktopError::Folder(error.folder))
    }

    /// The app's own folder: the installed exe, the log and the sign-in client cache live here.
    pub fn data_dir(&self) -> Result<PathBuf, DesktopError> {
        Ok(self.folder(Folder::LocalAppData)?.join(&self.settings.data_folder))
    }

    /// Where the installed copy of the app lives.
    pub fn installed_exe(&self) -> Result<PathBuf, DesktopError> {
        Ok(self.data_dir()?.join(&self.settings.executable))
    }

    /// Whether a local copy is installed.
    #[must_use]
    pub fn is_installed(&self) -> bool {
        self.installed_exe().is_ok_and(|path| path.is_file())
    }

    /// Whether this running copy is the installed one.
    #[must_use]
    pub fn is_running_installed(&self) -> bool {
        self.installed_exe().is_ok_and(|path| same_file(&path, &self.current_exe))
    }

    /// Account shortcuts start Antigravity, so they wear its icon (ours when it is missing).
    fn icon(&self) -> PathBuf {
        self.antigravity.executable().unwrap_or_else(|| self.current_exe.clone())
    }

    fn shortcut(path: PathBuf, target: PathBuf, arguments: String, icon: PathBuf) -> Shortcut {
        let working_dir = target.parent().map(Path::to_path_buf).unwrap_or_default();
        Shortcut { path, target, arguments, working_dir, icon, icon_index: 0 }
    }

    /// The desktop shortcut of `account`.
    pub fn account_shortcut_path(&self, account: &str) -> Result<PathBuf, DesktopError> {
        let name = format!("{}{account}.lnk", self.settings.account_shortcut_prefix);
        Ok(self.folder(Folder::Desktop)?.join(name))
    }

    /// Creates (or replaces) the desktop shortcut that switches straight to `account`. It starts
    /// the installed copy when there is one, else this copy.
    pub fn create_account_shortcut(&self, account: &str) -> Result<PathBuf, DesktopError> {
        let path = self.account_shortcut_path(account)?;
        let target = self.preferred_exe()?;
        let arguments = format!("{} \"{account}\"", self.settings.launch_switch);
        self.shell.create_shortcut(&Self::shortcut(
            path.clone(),
            target,
            arguments,
            self.icon(),
        ))?;
        tracing::info!(account, "account shortcut created");
        Ok(path)
    }

    /// Removes the desktop shortcut of `account`, if there is one.
    pub fn delete_account_shortcut(&self, account: &str) -> Result<(), DesktopError> {
        let path = self.account_shortcut_path(account)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(DesktopError::file("removing the shortcut", &path, &error)),
        }
    }

    /// Copies this exe into the data folder (unless it already runs from there) and creates the
    /// app shortcuts on the desktop and in the Start menu. Returns the installed exe.
    pub fn install(&self) -> Result<PathBuf, DesktopError> {
        let dir = self.data_dir()?;
        std::fs::create_dir_all(&dir)
            .map_err(|error| DesktopError::file("creating the app folder", &dir, &error))?;
        let installed = self.installed_exe()?;
        if !same_file(&installed, &self.current_exe) {
            std::fs::copy(&self.current_exe, &installed)
                .map_err(|error| DesktopError::file("copying the app", &installed, &error))?;
        }
        let desktop = self.folder(Folder::Desktop)?.join(&self.settings.app_shortcut);
        self.shell.create_shortcut(&Self::shortcut(
            desktop,
            installed.clone(),
            String::new(),
            installed.clone(),
        ))?;
        let menu = self.folder(Folder::StartMenuPrograms)?.join(&self.settings.start_menu_folder);
        std::fs::create_dir_all(&menu)
            .map_err(|error| DesktopError::file("creating the Start menu folder", &menu, &error))?;
        let entry = menu.join(&self.settings.app_shortcut);
        self.shell.create_shortcut(&Self::shortcut(
            entry,
            installed.clone(),
            String::new(),
            installed.clone(),
        ))?;
        tracing::info!(path = %installed.display(), "installed");
        Ok(installed)
    }

    /// The settings file.
    pub fn preferences(&self) -> Result<PreferenceStore, DesktopError> {
        Ok(PreferenceStore::new(self.data_dir()?.join(&self.settings.preferences_file)))
    }

    /// The copy to start for shortcuts and at sign-in: the installed one, else this one.
    fn preferred_exe(&self) -> Result<PathBuf, DesktopError> {
        if self.is_installed() { self.installed_exe() } else { Ok(self.current_exe.clone()) }
    }

    /// The shortcut in the Startup folder that starts the app in the tray at sign-in.
    pub fn startup_shortcut_path(&self) -> Result<PathBuf, DesktopError> {
        Ok(self.folder(Folder::Startup)?.join(&self.settings.app_shortcut))
    }

    /// Whether the app starts with Windows.
    #[must_use]
    pub fn starts_with_windows(&self) -> bool {
        self.startup_shortcut_path().is_ok_and(|path| path.is_file())
    }

    /// Makes the app start (in the tray) when the person signs in to Windows, or stops it.
    pub fn set_start_with_windows(&self, on: bool) -> Result<(), DesktopError> {
        let path = self.startup_shortcut_path()?;
        if on {
            let exe = self.preferred_exe()?;
            let shortcut =
                Self::shortcut(path, exe.clone(), self.settings.tray_switch.clone(), exe);
            self.shell.create_shortcut(&shortcut)?;
            tracing::info!("starts with Windows");
            return Ok(());
        }
        match std::fs::remove_file(&path) {
            Ok(()) => {
                tracing::info!("no longer starts with Windows");
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(DesktopError::file("removing the Startup shortcut", &path, &error)),
        }
    }
}

/// Whether two paths name the same file (compared as absolute paths, case-insensitively).
fn same_file(first: &Path, second: &Path) -> bool {
    let normal = |path: &Path| {
        std::path::absolute(path)
            .unwrap_or_else(|_| path.to_path_buf())
            .to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .to_lowercase()
    };
    normal(first) == normal(second)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::Duration;

    use antigravity::{FakeHost, HostSettings};
    use kernel::FixedFolders;

    use super::*;
    use crate::fake::FakeShell;

    struct Setup {
        root: tempfile::TempDir,
        shell: Arc<FakeShell>,
        desktop: Desktop,
    }

    fn setup() -> Setup {
        let root = tempfile::tempdir().unwrap();
        let folders: Arc<dyn KnownFolders> = Arc::new(FixedFolders::new(root.path()));
        let settings = HostSettings { poll: Duration::from_millis(1), ..HostSettings::default() };
        let antigravity = Antigravity::new(Arc::new(FakeHost::new()), folders.clone(), settings);
        let current = root.path().join("usb").join("AntigravityAccountSwitcher.exe");
        fs::create_dir_all(current.parent().unwrap()).unwrap();
        fs::write(&current, b"portable build").unwrap();
        let shell = Arc::new(FakeShell::new());
        let desktop =
            Desktop::new(shell.clone(), folders, antigravity, current, DesktopSettings::default());
        Setup { root, shell, desktop }
    }

    fn install_antigravity(root: &Path) -> PathBuf {
        let exe = root.join("LocalAppData\\Programs\\antigravity\\Antigravity.exe");
        fs::create_dir_all(exe.parent().unwrap()).unwrap();
        fs::write(&exe, b"x").unwrap();
        exe
    }

    #[test]
    fn data_folder_and_installed_exe_live_under_local_app_data() {
        let s = setup();
        let data = s.root.path().join("LocalAppData\\AntigravityAccountSwitcher");
        assert_eq!(s.desktop.data_dir().unwrap(), data);
        assert_eq!(s.desktop.installed_exe().unwrap(), data.join("AntigravityAccountSwitcher.exe"));
        assert!(!s.desktop.is_installed());
        assert!(!s.desktop.is_running_installed());
        assert_eq!(
            s.desktop.current_exe(),
            s.root.path().join("usb\\AntigravityAccountSwitcher.exe")
        );
    }

    #[test]
    fn web_pages_open_through_the_shell() {
        let s = setup();
        s.desktop.open("https://github.com/o/r").unwrap();
        assert_eq!(s.shell.opened(), ["https://github.com/o/r"]);
        assert_eq!(s.desktop.open("notepad.exe").unwrap_err().code(), "desktop.open");
    }

    #[test]
    fn account_shortcuts_start_this_copy_until_installed() {
        let s = setup();
        let path = s.desktop.create_account_shortcut("work").unwrap();
        assert_eq!(path, s.root.path().join("Desktop\\Antigravity - work.lnk"));
        assert!(path.exists());
        let shortcut = &s.shell.created()[0];
        assert_eq!(shortcut.target, s.root.path().join("usb\\AntigravityAccountSwitcher.exe"));
        assert_eq!(shortcut.arguments, "--launch \"work\"");
        assert_eq!(shortcut.working_dir, s.root.path().join("usb"));
        assert_eq!(shortcut.icon, shortcut.target, "no Antigravity: our own icon");
        assert_eq!(shortcut.icon_index, 0);

        let antigravity = install_antigravity(s.root.path());
        s.desktop.install().unwrap();
        s.desktop.create_account_shortcut("work").unwrap();
        let latest = s.shell.created().pop().unwrap();
        assert_eq!(latest.target, s.desktop.installed_exe().unwrap());
        assert_eq!(latest.icon, antigravity, "Antigravity's icon when it is installed");
    }

    #[test]
    fn deleting_a_shortcut_is_quiet_when_there_is_none() {
        let s = setup();
        s.desktop.delete_account_shortcut("ghost").unwrap();
        let path = s.desktop.create_account_shortcut("work").unwrap();
        s.desktop.delete_account_shortcut("work").unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn deleting_reports_other_failures() {
        let s = setup();
        let path = s.desktop.account_shortcut_path("busy").unwrap();
        fs::create_dir_all(&path).unwrap();
        let error = s.desktop.delete_account_shortcut("busy").unwrap_err();
        assert_eq!(error.code(), "desktop.file");
    }

    #[test]
    fn install_copies_the_exe_and_creates_both_app_shortcuts() {
        let s = setup();
        let installed = s.desktop.install().unwrap();
        assert_eq!(fs::read(&installed).unwrap(), b"portable build");
        assert!(s.desktop.is_installed());
        let created = s.shell.created();
        assert_eq!(created.len(), 2);
        assert_eq!(
            created[0].path,
            s.root.path().join("Desktop\\Antigravity Account Switcher.lnk")
        );
        assert_eq!(
            created[1].path,
            s.root.path().join(
                "StartMenuPrograms\\Antigravity Account Switcher\\Antigravity Account Switcher.lnk"
            )
        );
        assert!(created.iter().all(|c| c.target == installed && c.arguments.is_empty()));
        assert!(created.iter().all(|c| c.icon == installed), "the app's own icon");
    }

    #[test]
    fn install_from_the_installed_copy_does_not_copy_onto_itself() {
        let s = setup();
        let installed = s.desktop.install().unwrap();
        let again = Desktop::new(
            s.shell.clone(),
            Arc::new(FixedFolders::new(s.root.path())),
            s.desktop.antigravity.clone(),
            installed.clone(),
            DesktopSettings::default(),
        );
        assert!(again.is_running_installed());
        assert_eq!(again.install().unwrap(), installed);
        assert_eq!(fs::read(&installed).unwrap(), b"portable build");
    }

    #[test]
    fn install_reports_copy_and_shortcut_failures() {
        let s = setup();
        fs::create_dir_all(s.desktop.installed_exe().unwrap()).unwrap();
        assert_eq!(s.desktop.install().unwrap_err().code(), "desktop.file");

        let s = setup();
        s.shell.fail();
        assert_eq!(s.desktop.install().unwrap_err().code(), "desktop.shortcut");
        assert_eq!(s.desktop.create_account_shortcut("x").unwrap_err().code(), "desktop.shortcut");
    }

    #[test]
    fn missing_known_folders_are_reported() {
        #[derive(Debug)]
        struct NoFolders;
        impl KnownFolders for NoFolders {
            fn path(&self, folder: Folder) -> Result<PathBuf, kernel::FolderError> {
                Err(kernel::FolderError { folder: folder.name() })
            }
        }
        let folders: Arc<dyn KnownFolders> = Arc::new(NoFolders);
        let antigravity =
            Antigravity::new(Arc::new(FakeHost::new()), folders.clone(), HostSettings::default());
        let desktop = Desktop::new(
            Arc::new(FakeShell::new()),
            folders,
            antigravity,
            PathBuf::from("C:\\a.exe"),
            DesktopSettings::default(),
        );
        assert_eq!(desktop.data_dir().unwrap_err(), DesktopError::Folder("LocalAppData"));
        assert!(!desktop.is_installed());
        assert!(!desktop.is_running_installed());
        assert_eq!(desktop.create_account_shortcut("a").unwrap_err().code(), "desktop.folder");
        assert_eq!(desktop.delete_account_shortcut("a").unwrap_err().code(), "desktop.folder");
        assert_eq!(desktop.install().unwrap_err().code(), "desktop.folder");
    }

    #[test]
    fn starting_with_windows_is_a_startup_shortcut() {
        let s = setup();
        assert!(!s.desktop.starts_with_windows());
        s.desktop.set_start_with_windows(true).unwrap();
        assert!(s.desktop.starts_with_windows());
        let shortcut = s.shell.created().pop().unwrap();
        assert_eq!(shortcut.path, s.root.path().join(r"Startup\Antigravity Account Switcher.lnk"));
        assert_eq!(shortcut.arguments, "--tray");
        assert_eq!(shortcut.target, s.root.path().join(r"usb\AntigravityAccountSwitcher.exe"));

        let installed = s.desktop.install().unwrap();
        s.desktop.set_start_with_windows(true).unwrap();
        assert_eq!(
            s.shell.created().pop().unwrap().target,
            installed,
            "the installed copy once there is one"
        );

        s.desktop.set_start_with_windows(false).unwrap();
        assert!(!s.desktop.starts_with_windows());
        s.desktop.set_start_with_windows(false).unwrap();
    }

    #[test]
    fn start_with_windows_reports_problems() {
        let s = setup();
        fs::create_dir_all(s.desktop.startup_shortcut_path().unwrap()).unwrap();
        assert_eq!(s.desktop.set_start_with_windows(false).unwrap_err().code(), "desktop.file");
        let s = setup();
        s.shell.fail();
        assert_eq!(s.desktop.set_start_with_windows(true).unwrap_err().code(), "desktop.shortcut");
    }

    #[test]
    fn preferences_live_in_the_data_folder() {
        let s = setup();
        let store = s.desktop.preferences().unwrap();
        assert_eq!(
            store.path(),
            s.root.path().join(r"LocalAppData\AntigravityAccountSwitcher\settings.json")
        );
    }

    #[test]
    fn same_file_ignores_case_and_trailing_separators() {
        assert!(same_file(Path::new("C:\\A\\b.exe"), Path::new("c:\\a\\B.EXE")));
        assert!(same_file(Path::new("C:\\A\\"), Path::new("C:\\a")));
        assert!(!same_file(Path::new("C:\\a.exe"), Path::new("C:\\b.exe")));
    }
}
