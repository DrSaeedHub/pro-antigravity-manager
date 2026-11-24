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

fn only(args: &[String], switch: &str) -> bool {
    matches!(args, [flag] if flag.eq_ignore_ascii_case(switch))
}

/// Whether the arguments are exactly `--version`.
#[must_use]
pub fn version_request(args: &[String]) -> bool {
    only(args, VERSION)
}

/// Whether the arguments are exactly `--uninstall`.
#[must_use]
pub fn uninstall_request(args: &[String]) -> bool {
    only(args, UNINSTALL)
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
/// Start-with-Windows shortcut. Saved accounts and settings stay. Returns the shortcuts that could
/// not be removed.
#[must_use]
pub fn uninstall(services: &Services) -> usize {
    let desktop = services.accounts.desktop();
    let mut failed = 0;
    match services.accounts.list() {
        Ok(saved) => {
            for account in saved {
                if let Err(error) = desktop.delete_account_shortcut(&account.name) {
                    tracing::warn!(code = error.code(), %error, "shortcut left");
                    failed += 1;
                }
            }
        }
        Err(error) => tracing::warn!(code = error.code(), %error, "saved accounts unreadable"),
    }
    if let Err(error) = desktop.set_start_with_windows(false) {
        tracing::warn!(code = error.code(), %error, "Start with Windows left on");
        failed += 1;
    }
    tracing::info!(failed, "uninstall clean-up done");
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
    Ok(Some(window))
}

#[cfg(test)]
mod tests {
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
        assert!(uninstall_request(&args(&["--uninstall"])));
        assert!(!uninstall_request(&args(&[])));
        assert_eq!(updated_from(&args(&["--updated", "1.1.0"])), Some("1.1.0"));
        assert_eq!(updated_from(&args(&["--updated"])), None);
        assert_eq!(updated_from(&args(&["--tray", "1.1.0"])), None);
    }

    #[test]
    fn uninstalling_removes_the_shortcuts_the_app_made() {
        let dir = tempfile::tempdir().unwrap();
        let services =
            crate::bootstrap::build(crate::bootstrap::preview_parts(dir.path())).unwrap();
        let desktop = services.accounts.desktop();
        desktop.set_start_with_windows(true).unwrap();
        assert!(desktop.starts_with_windows());
        assert_eq!(uninstall(&services), 0);
        assert!(!desktop.starts_with_windows());
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
