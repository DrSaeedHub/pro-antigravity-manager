//! The update part of the controller: a check soon after start and every minute, the header
//! button, the About panel, the tray item, the corner notice, and the update sheet that downloads,
//! checks, puts the new version in place and restarts on it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, MutexGuard, PoisonError};
use std::time::Duration;

use slint::ComponentHandle;
use update::{Check, Release, UpdateError};

use super::{Inner, NOTICE, TRAY};
use crate::ui::{AppWindow, NoticeKind, SheetKind, ToastKind, UpdateStage};

/// The project's page (its GitHub repository).
pub const PROJECT: &str = env!("CARGO_PKG_REPOSITORY");
/// How often GitHub is asked for a new version.
pub const CHECK_EVERY: Duration = Duration::from_secs(60);
/// The first check waits a little, so the window comes up first.
pub const FIRST_CHECK: Duration = Duration::from_secs(5);
/// How long the next start keeps trying to delete what the update left (the old copy may still
/// be ending).
const CLEAN_UP_FOR: Duration = Duration::from_secs(60);

/// What the controller knows about updates.
#[derive(Debug, Default)]
pub(super) struct UpdateState {
    /// The newer release, once a check found one.
    available: Option<Release>,
    /// A check is under way.
    checking: bool,
    /// An update is downloading or being put in place.
    busy: bool,
    /// Set to stop the download.
    cancel: Arc<AtomicBool>,
}

/// `https://github.com/o/r` as people read it: `github.com/o/r`.
#[must_use]
pub fn display_address(address: &str) -> String {
    let bare = address.split_once("://").map_or(address, |(_, rest)| rest);
    bare.trim_end_matches('/').to_owned()
}

/// Bytes as megabytes with one decimal: `9.1 MB`.
#[must_use]
pub fn megabytes(bytes: u64) -> String {
    let tenths = bytes.saturating_mul(10).saturating_add(524_288) / 1_048_576;
    format!("{}.{} MB", tenths / 10, tenths % 10)
}

/// Deletes what an update left next to `exe`, retrying while the previous copy ends.
pub fn clean_up_after_update(exe: &Path) {
    let started = std::time::Instant::now();
    while update::replace::cleanup(exe) > 0 && started.elapsed() < CLEAN_UP_FOR {
        std::thread::sleep(Duration::from_secs(1));
    }
}

impl Inner {
    fn updates(&self) -> MutexGuard<'_, UpdateState> {
        self.updates.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The tray's "Update to …" item follows what is known.
    pub(super) fn update_tray_version(&self) {
        let version = self.updates().available.as_ref().map(|release| release.version.to_string());
        TRAY.with_borrow(|tray| {
            if let Some(tray) = tray {
                tray.set_update_version(version.unwrap_or_default().into());
            }
        });
    }

    /// The timer's check, when automatic checks are on.
    pub(super) fn scheduled_check(self: &Arc<Self>) {
        if self.state().prefs.check_updates {
            self.check_for_updates(false);
        }
    }

    /// Asks GitHub for the newest release. A manual check shows its progress and its problems;
    /// an automatic one stays quiet when it fails.
    pub(super) fn check_for_updates(self: &Arc<Self>, manual: bool) {
        {
            let mut updates = self.updates();
            if updates.busy || updates.checking {
                return;
            }
            updates.checking = true;
        }
        if manual && let Some(window) = self.window.upgrade() {
            window.set_update_stage(UpdateStage::Checking);
        }
        let inner = self.clone();
        self.services.runtime.spawn(async move {
            let result = inner.services.updater.check().await;
            let back = inner.clone();
            let _ = inner.window.upgrade_in_event_loop(move |window| {
                back.check_done(&window, result, manual);
            });
        });
    }

    fn check_done(
        self: &Arc<Self>,
        window: &AppWindow,
        result: Result<Check, UpdateError>,
        manual: bool,
    ) {
        self.updates().checking = false;
        if self.updates().busy {
            return;
        }
        let now = self.services.clock.now();
        let at = self.services.clock.time_zone().to_datetime(now).strftime("%H:%M").to_string();
        match result {
            Ok(Check::UpToDate) => {
                self.updates().available = None;
                window.set_update_version("".into());
                window.set_update_stage(UpdateStage::UpToDate);
                window.set_update_checked_at(at.into());
            }
            Ok(Check::Available(release)) => {
                let version = release.version.to_string();
                let news = self.updates().available.as_ref().is_none_or(|known| known != &release);
                if news {
                    tracing::info!(%version, "a new version is out");
                }
                self.updates().available = Some(release);
                window.set_update_version(version.clone().into());
                window.set_update_stage(UpdateStage::Available);
                window.set_update_checked_at(at.into());
                self.announce(window, &version);
            }
            Err(error) => {
                tracing::warn!(code = error.code(), %error, manual, "update check failed");
                if manual {
                    Self::show_update_problem(window, &error, false);
                }
            }
        }
        self.update_tray_version();
    }

    fn show_update_problem(window: &AppWindow, error: &UpdateError, installing: bool) {
        window.set_install_failed(installing);
        window.set_update_problem_code(error.code().into());
        window.set_update_problem_detail(update_detail(error).into());
        window.set_update_stage(UpdateStage::Failed);
    }

    /// The corner notice, once per version, while the window is hidden and the corner is free.
    fn announce(self: &Arc<Self>, window: &AppWindow, version: &str) {
        if window.window().is_visible() || NOTICE.with_borrow(Option::is_some) {
            return;
        }
        {
            let mut state = self.state();
            if state.prefs.notified_version == version {
                return;
            }
            version.clone_into(&mut state.prefs.notified_version);
        }
        self.save_prefs();
        self.show_notice(NoticeKind::Update, "", version, "", "");
    }

    /// Downloads, checks and puts the new version in place, then restarts on it. Waits while an
    /// account is being added or a switch is under way.
    pub(super) fn start_update(self: &Arc<Self>, window: &AppWindow) {
        Self::show_window(window);
        let release = {
            let updates = self.updates();
            if updates.busy {
                window.set_sheet(SheetKind::Update);
                return;
            }
            updates.available.clone()
        };
        let Some(release) = release else {
            window.set_sheet(SheetKind::About);
            self.check_for_updates(true);
            return;
        };
        window.set_sheet(SheetKind::Update);
        if self.add_session().is_some() || self.state().switching {
            window.set_install_failed(true);
            window.set_update_problem_code("update.wait".into());
            window.set_update_problem_detail("".into());
            window.set_update_stage(UpdateStage::Failed);
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut updates = self.updates();
            updates.busy = true;
            updates.cancel = cancel.clone();
        }
        window.set_update_stage(UpdateStage::Downloading);
        window.set_update_progress(0.0);
        window.set_update_received(megabytes(0).into());
        window.set_update_total(megabytes(release.package.size).into());
        let exe = self.services.accounts.desktop().current_exe().to_path_buf();
        tracing::info!(version = %release.version, "updating");
        let inner = self.clone();
        self.services.runtime.spawn(async move {
            let shown = AtomicU64::new(u64::MAX);
            let weak = inner.window.clone();
            let progress = move |received: u64, total: u64| {
                let percent = received.saturating_mul(100) / total.max(1);
                if shown.swap(percent, Ordering::Relaxed) != percent {
                    let _ = weak.upgrade_in_event_loop(move |window| {
                        window.set_update_progress(fraction(received, total));
                        window.set_update_received(megabytes(received).into());
                    });
                }
            };
            let prepared = inner.services.updater.prepare(&release, &exe, &progress, &cancel).await;
            let back = inner.clone();
            let _ = inner.window.upgrade_in_event_loop(move |window| {
                back.prepared(&window, &release, exe, prepared);
            });
        });
    }

    fn prepared(
        self: &Arc<Self>,
        window: &AppWindow,
        release: &Release,
        exe: PathBuf,
        prepared: Result<PathBuf, UpdateError>,
    ) {
        let new = match prepared {
            Ok(new) => new,
            Err(UpdateError::Canceled) => {
                self.updates().busy = false;
                tracing::info!("update canceled");
                window.set_update_stage(UpdateStage::Available);
                window.set_sheet(SheetKind::None);
                return;
            }
            Err(error) => return self.update_failed(window, &error),
        };
        window.set_update_progress(1.0);
        window.set_update_stage(UpdateStage::Installing);
        let version = release.version.to_string();
        self.work(
            move |inner| inner.services.updater.install(&exe, &new).map(|_old| exe),
            move |window, inner, installed| match installed {
                Ok(exe) => {
                    inner.record_installed_version(&exe, &version);
                    *inner.restart.lock().unwrap_or_else(PoisonError::into_inner) = Some(exe);
                    tracing::info!(%version, "restarting on the new version");
                    inner.quit(window);
                }
                Err(error) => inner.update_failed(window, &error),
            },
        );
    }

    /// Keeps the version in Installed apps right when the installer's copy updated itself.
    fn record_installed_version(&self, exe: &Path, version: &str) {
        let desktop = self.services.accounts.desktop();
        if !desktop.is_running_installed() {
            return;
        }
        #[cfg(windows)]
        if let Some(folder) = exe.parent()
            && desktop::record_installed_version(folder, version)
        {
            tracing::info!(version, "Installed apps shows the new version");
        }
        #[cfg(not(windows))]
        let _ = (exe, version);
    }

    fn update_failed(&self, window: &AppWindow, error: &UpdateError) {
        self.updates().busy = false;
        tracing::error!(code = error.code(), %error, "update failed");
        Self::show_update_problem(window, error, true);
    }

    /// The sheet's Cancel: stops a download, or closes the sheet.
    pub(super) fn cancel_update(&self, window: &AppWindow) {
        let updates = self.updates();
        if updates.busy {
            updates.cancel.store(true, Ordering::Relaxed);
        } else {
            drop(updates);
            if window.get_update_stage() == UpdateStage::Failed {
                let known = self.updates().available.is_some();
                window.set_update_stage(if known {
                    UpdateStage::Available
                } else {
                    UpdateStage::Idle
                });
            }
            window.set_sheet(SheetKind::None);
        }
    }

    /// Opens a page: `project`, `notes` (the new release's notes) or `download` (the newest
    /// release's page).
    pub(super) fn open_link(&self, window: &AppWindow, kind: &str) {
        let settings = self.services.updater.settings();
        let notes = self
            .updates()
            .available
            .as_ref()
            .map(|release| release.notes.clone())
            .filter(|notes| !notes.is_empty());
        let address = match kind {
            "project" => PROJECT.to_owned(),
            "notes" => notes.unwrap_or_else(|| settings.releases_page()),
            _ => settings.releases_page(),
        };
        if let Err(error) = self.services.accounts.desktop().open(&address) {
            tracing::warn!(code = error.code(), %error, "page not opened");
            self.toast(window, ToastKind::Problem, error.code(), "", &error.to_string());
        }
    }
}

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "a progress bar needs no more precision"
)]
fn fraction(received: u64, total: u64) -> f32 {
    (received as f64 / total.max(1) as f64).clamp(0.0, 1.0) as f32
}

/// The part of a problem worth showing next to its sentence.
fn update_detail(error: &UpdateError) -> String {
    match error {
        UpdateError::Feed(status) | UpdateError::Download(status) => status.to_string(),
        UpdateError::BadFeed(detail)
        | UpdateError::Package(detail)
        | UpdateError::NotWritable(detail)
        | UpdateError::Probe(detail) => detail.clone(),
        UpdateError::File { reason, .. } => reason.clone(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_read_without_their_scheme() {
        assert_eq!(display_address("https://github.com/o/r/"), "github.com/o/r");
        assert_eq!(display_address("github.com/o/r"), "github.com/o/r");
    }

    #[test]
    fn sizes_and_progress_read_well() {
        assert_eq!(megabytes(0), "0.0 MB");
        assert_eq!(megabytes(9_111_831), "8.7 MB");
        assert_eq!(megabytes(1_048_576), "1.0 MB");
        assert!((fraction(5, 10) - 0.5).abs() < f32::EPSILON);
        assert!((fraction(20, 10) - 1.0).abs() < f32::EPSILON);
        assert!((fraction(0, 0)).abs() < f32::EPSILON);
    }

    #[test]
    fn problem_details_keep_what_helps() {
        assert_eq!(update_detail(&UpdateError::Feed(503)), "503");
        assert_eq!(update_detail(&UpdateError::NotWritable("C:/x".into())), "C:/x");
        assert_eq!(update_detail(&UpdateError::Checksum), "");
        let file = UpdateError::File { action: "a", path: "p".into(), reason: "denied".into() };
        assert_eq!(update_detail(&file), "denied");
    }

    // --- the controller's update flow ----------------------------------------------------

    use crate::controller::{Controller, Outcome};

    struct Rig {
        _dir: tempfile::TempDir,
        shell: Arc<desktop::FakeShell>,
        window: AppWindow,
        controller: Controller,
    }

    fn rig() -> Rig {
        i_slint_backend_testing::init_no_event_loop();
        Inner::close_notice();
        let dir = tempfile::tempdir().unwrap();
        let shell = Arc::new(desktop::FakeShell::new());
        let parts = crate::bootstrap::Parts {
            shell: shell.clone(),
            ..crate::bootstrap::preview_parts(dir.path())
        };
        let services = crate::bootstrap::build(parts).unwrap();
        let window = AppWindow::new().unwrap();
        let controller = Controller::new(&window, services);
        Rig { _dir: dir, shell, window, controller }
    }

    fn release(version: &str) -> Release {
        Release {
            version: semver::Version::parse(version).unwrap(),
            published: String::new(),
            notes: format!("https://github.com/o/r/releases/tag/v{version}"),
            package: update::Package {
                url: "http://127.0.0.1:9/preview/repo/releases/download/x.zip".into(),
                sha256: "0".repeat(64),
                size: 9_111_831,
                file: "AntigravityAccountSwitcher.exe".into(),
            },
        }
    }

    fn notice_kind() -> Option<(NoticeKind, slint::SharedString)> {
        NOTICE.with_borrow(|notice| notice.as_ref().map(|n| (n.get_kind(), n.get_to())))
    }

    #[test]
    fn a_new_version_shows_in_the_header_about_and_tray() {
        let rig = rig();
        let inner = &rig.controller.inner;
        rig.window.show().unwrap();
        inner.check_done(&rig.window, Ok(Check::Available(release("9.9.9"))), false);
        assert_eq!(rig.window.get_update_version(), "9.9.9");
        assert_eq!(rig.window.get_update_stage(), UpdateStage::Available);
        assert_ne!(rig.window.get_update_checked_at(), "");
        assert_eq!(notice_kind(), None, "the window is open: no corner notice");

        inner.check_done(&rig.window, Ok(Check::UpToDate), false);
        assert_eq!(rig.window.get_update_version(), "");
        assert_eq!(rig.window.get_update_stage(), UpdateStage::UpToDate);
    }

    #[test]
    fn a_hidden_window_gets_one_corner_notice_per_version() {
        let rig = rig();
        let inner = &rig.controller.inner;
        inner.check_done(&rig.window, Ok(Check::Available(release("9.9.9"))), false);
        assert_eq!(notice_kind(), Some((NoticeKind::Update, "9.9.9".into())));
        assert_eq!(inner.state().prefs.notified_version, "9.9.9");
        Inner::close_notice();
        inner.check_done(&rig.window, Ok(Check::Available(release("9.9.9"))), false);
        assert_eq!(notice_kind(), None, "announced once");
        inner.check_done(&rig.window, Ok(Check::Available(release("9.9.10"))), false);
        assert_eq!(notice_kind(), Some((NoticeKind::Update, "9.9.10".into())));
        Inner::close_notice();
    }

    #[test]
    fn the_corner_notice_never_covers_another() {
        let rig = rig();
        let inner = &rig.controller.inner;
        inner.show_notice(NoticeKind::Countdown, "a", "b", "90%", "");
        inner.check_done(&rig.window, Ok(Check::Available(release("9.9.9"))), false);
        assert_eq!(notice_kind().map(|(kind, _)| kind), Some(NoticeKind::Countdown));
        assert_eq!(inner.state().prefs.notified_version, "", "still to be announced");
        Inner::close_notice();
    }

    #[test]
    fn only_manual_checks_show_their_problems() {
        let rig = rig();
        let inner = &rig.controller.inner;
        inner.check_done(&rig.window, Err(UpdateError::Network("offline".into())), false);
        assert_eq!(rig.window.get_update_stage(), UpdateStage::Idle);
        inner.check_done(&rig.window, Err(UpdateError::Feed(503)), true);
        assert_eq!(rig.window.get_update_stage(), UpdateStage::Failed);
        assert_eq!(rig.window.get_update_problem_code(), "update.feed");
        assert_eq!(rig.window.get_update_problem_detail(), "503");
        assert!(!rig.window.get_install_failed());
    }

    #[test]
    fn checks_follow_the_setting_and_never_overlap() {
        let rig = rig();
        let inner = &rig.controller.inner;
        inner.state().prefs.check_updates = false;
        inner.scheduled_check();
        assert!(!inner.updates().checking, "automatic checks are off");
        inner.check_for_updates(true);
        assert!(inner.updates().checking, "a manual check still runs");
        assert_eq!(rig.window.get_update_stage(), UpdateStage::Checking);
        inner.check_for_updates(true);
        assert!(inner.updates().checking);

        rig.window.set_check_updates(true);
        inner.settings_changed(&rig.window);
        assert!(inner.state().prefs.check_updates);
    }

    #[test]
    fn update_without_a_known_release_checks_first() {
        let rig = rig();
        rig.controller.inner.start_update(&rig.window);
        assert_eq!(rig.window.get_sheet(), SheetKind::About);
        assert!(rig.controller.inner.updates().checking);
    }

    #[test]
    fn updates_wait_for_a_switch_to_finish() {
        let rig = rig();
        let inner = &rig.controller.inner;
        inner.updates().available = Some(release("9.9.9"));
        inner.state().switching = true;
        inner.start_update(&rig.window);
        assert_eq!(rig.window.get_sheet(), SheetKind::Update);
        assert_eq!(rig.window.get_update_stage(), UpdateStage::Failed);
        assert_eq!(rig.window.get_update_problem_code(), "update.wait");
        assert!(!inner.updates().busy);
    }

    #[test]
    fn an_update_downloads_once_and_can_be_canceled() {
        let rig = rig();
        let inner = &rig.controller.inner;
        inner.updates().available = Some(release("9.9.9"));
        inner.start_update(&rig.window);
        assert_eq!(rig.window.get_sheet(), SheetKind::Update);
        assert_eq!(rig.window.get_update_stage(), UpdateStage::Downloading);
        assert_eq!(rig.window.get_update_total(), "8.7 MB");
        assert!(inner.updates().busy);
        let cancel = inner.updates().cancel.clone();

        rig.window.set_sheet(SheetKind::None);
        inner.start_update(&rig.window);
        assert_eq!(rig.window.get_sheet(), SheetKind::Update, "a second click shows the progress");
        inner.check_for_updates(true);
        assert!(!inner.updates().checking, "no check while updating");

        inner.cancel_update(&rig.window);
        assert!(cancel.load(Ordering::Relaxed));
        let exe = PathBuf::from("x.exe");
        inner.prepared(&rig.window, &release("9.9.9"), exe, Err(UpdateError::Canceled));
        assert!(!inner.updates().busy);
        assert_eq!(rig.window.get_update_stage(), UpdateStage::Available);
        assert_eq!(rig.window.get_sheet(), SheetKind::None);
    }

    #[test]
    fn a_failed_update_says_so_and_can_be_closed() {
        let rig = rig();
        let inner = &rig.controller.inner;
        inner.updates().available = Some(release("9.9.9"));
        inner.updates().busy = true;
        rig.window.set_sheet(SheetKind::Update);
        let exe = PathBuf::from("x.exe");
        inner.prepared(&rig.window, &release("9.9.9"), exe, Err(UpdateError::Checksum));
        assert!(!inner.updates().busy);
        assert_eq!(rig.window.get_update_stage(), UpdateStage::Failed);
        assert_eq!(rig.window.get_update_problem_code(), "update.checksum");
        assert!(rig.window.get_install_failed());
        inner.cancel_update(&rig.window);
        assert_eq!(rig.window.get_update_stage(), UpdateStage::Available);
        assert_eq!(rig.window.get_sheet(), SheetKind::None);

        inner.updates().available = None;
        rig.window.set_update_stage(UpdateStage::Failed);
        inner.cancel_update(&rig.window);
        assert_eq!(rig.window.get_update_stage(), UpdateStage::Idle);
    }

    #[test]
    fn links_open_the_project_notes_and_downloads() {
        let rig = rig();
        let inner = &rig.controller.inner;
        inner.open_link(&rig.window, "project");
        inner.open_link(&rig.window, "notes");
        inner.updates().available = Some(release("9.9.9"));
        inner.open_link(&rig.window, "notes");
        inner.open_link(&rig.window, "download");
        assert_eq!(
            rig.shell.opened(),
            [
                PROJECT.to_owned(),
                "http://127.0.0.1:9/preview/repo/releases/latest".to_owned(),
                "https://github.com/o/r/releases/tag/v9.9.9".to_owned(),
                "http://127.0.0.1:9/preview/repo/releases/latest".to_owned(),
            ]
        );
        assert_eq!(rig.window.get_project(), display_address(PROJECT));
        rig.shell.fail();
        inner.open_link(&rig.window, "project");
        assert_eq!(rig.window.get_toast().code, "desktop.open");
    }

    #[test]
    fn a_finished_update_restarts_on_the_new_file() {
        let rig = rig();
        assert_eq!(rig.controller.outcome(), Outcome::Quit);
        *rig.controller.inner.restart.lock().unwrap() = Some(PathBuf::from("new.exe"));
        assert_eq!(rig.controller.outcome(), Outcome::Restart(PathBuf::from("new.exe")));
        assert_eq!(rig.controller.outcome(), Outcome::Quit, "taken once");
        rig.controller.updated("1.1.0");
        assert_eq!(rig.window.get_toast().code, "updated");
        assert_eq!(rig.window.get_toast().name, env!("CARGO_PKG_VERSION"));
        rig.controller.inner.record_installed_version(Path::new("x.exe"), "9.9.9");
    }

    #[test]
    fn leftovers_of_an_update_are_removed_at_start() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("App.exe");
        std::fs::write(&exe, b"x").unwrap();
        std::fs::write(dir.path().join("App.exe.5.old"), b"old").unwrap();
        clean_up_after_update(&exe);
        assert!(!dir.path().join("App.exe.5.old").exists());
        assert!(exe.exists());
    }
}
