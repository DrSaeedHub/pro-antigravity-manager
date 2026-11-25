//! The command line. `--launch <name>` is what an account shortcut runs: a small window confirms
//! closing Antigravity when it is open, switches, and reports problems; the main window never
//! opens. `--version` prints the version (the updater asks a downloaded copy this), `--updated
//! <old version>` is how the previous version starts the new one, and `--uninstall` is run by the
//! uninstaller to remove the shortcuts the app made.

use std::time::Duration;

use accounts::AccountsError;
use slint::ComponentHandle;

use crate::bootstrap::Services;
use crate::controller::account_problem;
use crate::ui::{LaunchStage, LaunchWindow};

/// The command-line switch.
pub const LAUNCH: &str = "--launch";

/// The switch that starts the app in the tray (used when starting with Windows).
pub const TRAY: &str = "--tray";

/// The switch that prints the version and exits.
pub const VERSION: &str = update::probe::VERSION_SWITCH;

/// The switch the previous version passes, with its version, when it starts the new one.
pub const UPDATED: &str = "--updated";

/// The switch the uninstaller passes.
pub const UNINSTALL: &str = "--uninstall";

/// After `--uninstall`: also delete every saved account.
pub const DELETE_ACCOUNTS: &str = "--delete-accounts";

/// The switch the uninstaller passes to ask what to remove (the answer is the exit code).
pub const UNINSTALL_DIALOG: &str = "--uninstall-dialog";

/// Whether the arguments are exactly `--uninstall-dialog`.
#[must_use]
pub fn uninstall_dialog_request(args: &[String]) -> bool {
    only(args, UNINSTALL_DIALOG)
}

fn only(args: &[String], switch: &str) -> bool {
    matches!(args, [flag] if flag.eq_ignore_ascii_case(switch))
}

/// Whether the arguments are exactly `--version`.
#[must_use]
pub fn version_request(args: &[String]) -> bool {
    only(args, VERSION)
}

/// What the uninstaller asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Uninstall {
    /// Remove the shortcuts the app made; saved accounts stay.
    Shortcuts,
    /// Also delete every saved account.
    Everything,
}

/// The uninstaller's request, when the arguments are exactly `--uninstall`, optionally followed
/// by `--delete-accounts`.
#[must_use]
pub fn uninstall_request(args: &[String]) -> Option<Uninstall> {
    match args {
        [flag] if flag.eq_ignore_ascii_case(UNINSTALL) => Some(Uninstall::Shortcuts),
        [flag, delete]
            if flag.eq_ignore_ascii_case(UNINSTALL)
                && delete.eq_ignore_ascii_case(DELETE_ACCOUNTS) =>
        {
            Some(Uninstall::Everything)
        }
        _ => None,
    }
}

/// The version this copy was updated from, when the arguments are exactly `--updated <version>`.
#[must_use]
pub fn updated_from(args: &[String]) -> Option<&str> {
    match args {
        [flag, from] if flag.eq_ignore_ascii_case(UPDATED) => Some(from.as_str()),
        _ => None,
    }
}

/// Removes what the app put outside its folder: every saved account's desktop shortcut and the
/// Start-with-Windows shortcut, and with [`Uninstall::Everything`] the saved accounts themselves.
/// Antigravity's own sign-in and the settings stay. Returns what could not be removed.
#[must_use]
pub fn uninstall(services: &Services, scope: Uninstall) -> usize {
    let desktop = services.accounts.desktop();
    let mut failed = 0;
    let listed = match scope {
        Uninstall::Everything => services.accounts.remove_all().map(|left| {
            failed += left;
            Vec::new()
        }),
        Uninstall::Shortcuts => services.accounts.list(),
    };
    match listed {
        Ok(saved) => {
            for account in saved {
                if let Err(error) = desktop.delete_account_shortcut(&account.name) {
                    tracing::warn!(code = error.code(), %error, "shortcut left");
                    failed += 1;
                }
            }
        }
        Err(error) => {
            tracing::warn!(code = error.code(), %error, "saved accounts unreadable");
            failed += 1;
        }
    }
    if let Err(error) = desktop.set_start_with_windows(false) {
        tracing::warn!(code = error.code(), %error, "Start with Windows left on");
        failed += 1;
    }
    tracing::info!(failed, ?scope, "uninstall clean-up done");
    failed
}

/// Whether the arguments are exactly `--tray` (any letter case).
#[must_use]
pub fn tray_start(args: &[String]) -> bool {
    only(args, TRAY)
}

/// The account to switch to when the arguments are exactly `--launch <name>`
/// (the switch in any letter case).
#[must_use]
pub fn launch_target(args: &[String]) -> Option<&str> {
    match args {
        [flag, name] if flag.eq_ignore_ascii_case(LAUNCH) => Some(name.as_str()),
        _ => None,
    }
}

fn fail(window: &LaunchWindow, error: &AccountsError) {
    let (code, detail) = account_problem(error);
    window.set_problem_code(code);
    window.set_problem_detail(detail);
    window.set_stage(LaunchStage::Failed);
}

fn switch(window: &LaunchWindow, services: &Services, name: &str, close_first: bool, active: bool) {
    window.set_stage(LaunchStage::Working);
    let weak = window.as_weak();
    let accounts = services.accounts.clone();
    let name = name.to_owned();
    services.runtime.spawn_blocking(move || {
        let result = (|| {
            if close_first {
                accounts.antigravity().close()?;
            }
            if active {
                // Already signed in as this account: keep Antigravity's own, newer tokens.
                return accounts.antigravity().launch().map_err(AccountsError::from);
            }
            accounts.switch_to(&name)
        })();
        let _ = weak.upgrade_in_event_loop(move |window| match result {
            Ok(()) => {
                slint::Timer::single_shot(Duration::from_millis(700), || {
                    let _ = slint::quit_event_loop();
                });
            }
            Err(error) => fail(&window, &error),
        });
    });
}

/// Runs the switch for `name` and returns when its window closes.
pub fn run(services: &Services, name: &str) -> Result<(), slint::PlatformError> {
    let Some(window) = open(services, name)? else { return Ok(()) };
    slint::run_event_loop_until_quit()?;
    drop(window);
    Ok(())
}

/// Shows the window for `name`, already switching when no question is needed. `None` when
/// Antigravity already runs as this account: there is nothing to do.
pub fn open(services: &Services, name: &str) -> Result<Option<LaunchWindow>, slint::PlatformError> {
    let window = LaunchWindow::new()?;
    window.set_account(name.into());
    window.on_cancel(|| {
        let _ = slint::quit_event_loop();
    });
    window.window().on_close_requested(|| {
        let _ = slint::quit_event_loop();
        slint::CloseRequestResponse::HideWindow
    });
    {
        let weak = window.as_weak();
        let services = services.clone();
        let name = name.to_owned();
        window.on_confirm(move || {
            if let Some(window) = weak.upgrade() {
                switch(&window, &services, &name, true, false);
            }
        });
    }

    match services.accounts.is_saved(name) {
        Ok(false) => fail(&window, &AccountsError::NotSaved),
        Err(error) => fail(&window, &error),
        Ok(true) => {
            let active = services.accounts.is_active(name);
            let running = services.accounts.antigravity().is_running();
            match (active, running) {
                // Antigravity already runs as this account: nothing to do.
                (true, true) => return Ok(None),
                (false, true) => window.set_stage(LaunchStage::Confirm),
                (_, false) => switch(&window, services, name, false, active),
            }
        }
    }
    window.show()?;
    crate::window::keep_on_screen(window.window());
    Ok(Some(window))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn only_exactly_tray_starts_in_the_tray() {
        assert!(tray_start(&args(&["--tray"])));
        assert!(tray_start(&args(&["--TRAY"])));
        assert!(!tray_start(&args(&[])));
        assert!(!tray_start(&args(&["--tray", "x"])));
        assert!(!tray_start(&args(&["--launch", "x"])));
    }

    #[test]
    fn the_update_and_uninstall_switches_are_exact() {
        assert!(version_request(&args(&["--version"])));
        assert!(version_request(&args(&["--VERSION"])));
        assert!(!version_request(&args(&["--version", "x"])));
        assert_eq!(uninstall_request(&args(&["--uninstall"])), Some(Uninstall::Shortcuts));
        assert_eq!(
            uninstall_request(&args(&["--UNINSTALL", "--delete-accounts"])),
            Some(Uninstall::Everything)
        );
        assert_eq!(uninstall_request(&args(&["--uninstall", "--other"])), None);
        assert!(uninstall_dialog_request(&args(&["--uninstall-dialog"])));
        assert!(!uninstall_dialog_request(&args(&["--uninstall-dialog", "x"])));
        assert_eq!(uninstall_request(&args(&[])), None);
        assert_eq!(updated_from(&args(&["--updated", "1.1.0"])), Some("1.1.0"));
        assert_eq!(updated_from(&args(&["--updated"])), None);
        assert_eq!(updated_from(&args(&["--tray", "1.1.0"])), None);
    }

    /// Preview services with the accounts `work` and `home` saved, and their shortcuts.
    fn with_accounts(dir: &std::path::Path) -> (Services, Arc<vault::MemoryStore>) {
        let store = Arc::new(vault::MemoryStore::new());
        let blob = br#"{"id_token_email":"a@example.com","token":{"refresh_token":"r"}}"#;
        store.insert("antigravity-switcher:work", blob);
        store.insert("antigravity-switcher:home", blob);
        store.insert("gemini:antigravity", blob);
        let parts = crate::bootstrap::Parts {
            store: store.clone(),
            ..crate::bootstrap::preview_parts(dir)
        };
        let services = crate::bootstrap::build(parts).unwrap();
        for name in ["work", "home"] {
            services.accounts.create_shortcut(name).unwrap().unwrap();
        }
        services.accounts.desktop().set_start_with_windows(true).unwrap();
        (services, store)
    }

    #[test]
    fn uninstalling_removes_the_shortcuts_the_app_made() {
        let dir = tempfile::tempdir().unwrap();
        let (services, store) = with_accounts(dir.path());
        let desktop = services.accounts.desktop();
        let work = desktop.account_shortcut_path("work").unwrap();
        assert!(work.exists() && desktop.starts_with_windows());
        assert_eq!(uninstall(&services, Uninstall::Shortcuts), 0);
        assert!(!work.exists());
        assert!(!desktop.starts_with_windows());
        assert_eq!(services.accounts.list().unwrap().len(), 2, "saved accounts stay");
        assert!(
            vault::CredentialStore::read_blob(store.as_ref(), "gemini:antigravity")
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn uninstalling_everything_also_deletes_the_saved_accounts() {
        let dir = tempfile::tempdir().unwrap();
        let (services, store) = with_accounts(dir.path());
        assert_eq!(uninstall(&services, Uninstall::Everything), 0);
        assert!(services.accounts.list().unwrap().is_empty());
        assert!(!services.accounts.desktop().account_shortcut_path("home").unwrap().exists());
        assert!(
            vault::CredentialStore::read_blob(store.as_ref(), "gemini:antigravity")
                .unwrap()
                .is_some(),
            "Antigravity stays signed in"
        );
        store.fail(vault::Operation::List, "antigravity-switcher:");
        assert_eq!(uninstall(&services, Uninstall::Everything), 1, "unreadable accounts count");
    }

    #[test]
    fn only_exactly_two_arguments_launch() {
        assert_eq!(launch_target(&args(&["--launch", "work"])), Some("work"));
        assert_eq!(launch_target(&args(&["--LAUNCH", "work"])), Some("work"));
        assert_eq!(launch_target(&args(&[])), None);
        assert_eq!(launch_target(&args(&["--launch"])), None);
        assert_eq!(launch_target(&args(&["--launch", "a", "b"])), None);
        assert_eq!(launch_target(&args(&["--open", "work"])), None);
    }
}
