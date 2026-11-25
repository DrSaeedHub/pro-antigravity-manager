//! `--uninstall-dialog`: the window the uninstaller opens before it removes anything. The answer
//! is the process exit code, which the installer script reads (see `installer/*.iss`).

use std::cell::Cell;
use std::rc::Rc;

use slint::ComponentHandle;

use crate::bootstrap::Services;
use crate::ui::UninstallWindow;

/// The exit code of "keep the app".
pub const CANCELED: u8 = 0;
/// The exit code of "uninstall"; 1 is added to also delete the saved accounts, 2 to also delete
/// settings and logs.
pub const REMOVE: u8 = 20;

/// What the person chose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Choice {
    /// Delete every saved account too.
    pub delete_accounts: bool,
    /// Delete settings, logs and caches too.
    pub delete_settings: bool,
}

/// The exit code for `choice` (`None`: canceled).
#[must_use]
pub fn exit_code(choice: Option<Choice>) -> u8 {
    choice.map_or(CANCELED, |choice| {
        REMOVE + u8::from(choice.delete_accounts) + 2 * u8::from(choice.delete_settings)
    })
}

/// The window, knowing how many accounts are saved (not shown yet).
pub fn window(services: &Services) -> Result<UninstallWindow, slint::PlatformError> {
    let window = UninstallWindow::new()?;
    let accounts = services.accounts.list().map_or(0, |list| list.len());
    window.set_accounts(i32::try_from(accounts).unwrap_or(i32::MAX));
    Ok(window)
}

/// Shows the window and waits for the answer.
pub fn ask(services: &Services) -> Result<Option<Choice>, slint::PlatformError> {
    let window = window(services)?;
    let answer: Rc<Cell<Option<Choice>>> = Rc::new(Cell::new(None));
    {
        let weak = window.as_weak();
        let answer = answer.clone();
        window.on_remove(move || {
            if let Some(window) = weak.upgrade() {
                answer.set(Some(Choice {
                    delete_accounts: window.get_delete_accounts() && window.get_accounts() > 0,
                    delete_settings: window.get_delete_settings(),
                }));
            }
            let _ = slint::quit_event_loop();
        });
    }
    window.on_cancel(|| {
        let _ = slint::quit_event_loop();
    });
    window.window().on_close_requested(|| {
        let _ = slint::quit_event_loop();
        slint::CloseRequestResponse::HideWindow
    });
    window.show()?;
    crate::window::place_center(window.window());
    crate::window::bring_to_front(window.window());
    slint::run_event_loop_until_quit()?;
    let _ = window.hide();
    tracing::info!(choice = ?answer.get(), "uninstall dialog answered");
    Ok(answer.get())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_choice_has_its_own_exit_code() {
        let code = |delete_accounts, delete_settings| {
            exit_code(Some(Choice { delete_accounts, delete_settings }))
        };
        assert_eq!(exit_code(None), 0);
        assert_eq!(code(false, false), 20);
        assert_eq!(code(true, false), 21);
        assert_eq!(code(false, true), 22);
        assert_eq!(code(true, true), 23);
    }

    #[test]
    fn the_window_offers_the_accounts_only_when_there_are_some() {
        i_slint_backend_testing::init_no_event_loop();
        let window = UninstallWindow::new().unwrap();
        window.set_accounts(5);
        assert!(
            !window.get_delete_accounts() && !window.get_delete_settings(),
            "nothing by default"
        );
        window.set_delete_accounts(true);
        assert!(window.get_delete_accounts());
    }
}
