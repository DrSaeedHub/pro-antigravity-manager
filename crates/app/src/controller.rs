//! Connects the main window to the services. Slow work (Credential Manager, closing Antigravity,
//! Google) runs off the UI thread; results come back through the Slint event loop.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use accounts::{AccountsError, AddSession, SavedAccount, is_valid_name};
use desktop::Preferences;
use quota::format::{percent, reset_time};
use quota::{AccountQuota, Decision, Fuel, decide};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::bootstrap::Services;
use crate::ui::{
    AccountView, AddStep, AppWindow, NoticeKind, NoticeWindow, SheetKind, Toast, ToastKind, Tray,
    TrayAccount,
};
use crate::view;

/// How often every account's quota is read again (window open or in the tray).
pub const REFRESH: Duration = Duration::from_secs(60);
/// How often the countdown notice ticks.
const COUNTDOWN_STEP: Duration = Duration::from_secs(1);
/// How often the add flow checks whether Antigravity finished signing in.
pub const SIGN_IN_POLL: Duration = Duration::from_secs(2);

#[derive(Debug, Default)]
struct State {
    accounts: Vec<SavedAccount>,
    quotas: HashMap<String, AccountQuota>,
    active_email: Option<String>,
    generation: u64,
    pending: usize,
    details_for: Option<String>,
    polling: bool,
    prefs: Preferences,
    /// The account whose automatic switch the person declined, until it refills.
    snoozed: Option<String>,
    /// Decisions wait for a reading that started after this generation (the last switch).
    switch_marker: u64,
    /// A switch or its countdown is under way.
    switching: bool,
    all_empty_noticed: bool,
}

thread_local! {
    // UI-thread objects that are not `Send`: the tray icon, the notice window and its timer.
    static TRAY: RefCell<Option<Tray>> = const { RefCell::new(None) };
    static NOTICE: RefCell<Option<NoticeWindow>> = const { RefCell::new(None) };
    static COUNTDOWN: RefCell<Option<slint::Timer>> = const { RefCell::new(None) };
}

/// The code and detail of an account problem, for the UI.
#[must_use]
pub fn account_problem(error: &AccountsError) -> (SharedString, SharedString) {
    let detail = match error {
        AccountsError::Vault(inner) => inner.to_string(),
        AccountsError::Antigravity(inner) => inner.to_string(),
        _ => String::new(),
    };
    (error.code().into(), detail.into())
}

struct Inner {
    services: Services,
    window: slint::Weak<AppWindow>,
    state: Mutex<State>,
    add: Mutex<Option<AddSession>>,
    toast_serial: AtomicI32,
}

impl std::fmt::Debug for Inner {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Inner").field("services", &self.services).finish_non_exhaustive()
    }
}

/// The main window's controller. Keep it alive while the window runs.
pub struct Controller {
    inner: Arc<Inner>,
    _tick: slint::Timer,
    _poll: slint::Timer,
}

impl std::fmt::Debug for Controller {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Controller").field("inner", &self.inner).finish_non_exhaustive()
    }
}

type Done<T> = Box<dyn FnOnce(&AppWindow, &Arc<Inner>, T) + Send>;

impl Inner {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn add_session(&self) -> MutexGuard<'_, Option<AddSession>> {
        self.add.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Runs `job` on a worker thread, then `done` on the UI thread.
    fn work<T: Send + 'static>(
        self: &Arc<Self>,
        job: impl FnOnce(&Arc<Self>) -> T + Send + 'static,
        done: impl FnOnce(&AppWindow, &Arc<Self>, T) + Send + 'static,
    ) {
        let inner = self.clone();
        let done: Done<T> = Box::new(done);
        self.services.runtime.spawn_blocking(move || {
            let result = job(&inner);
            let back = inner.clone();
            let _ = inner.window.upgrade_in_event_loop(move |window| done(&window, &back, result));
        });
    }

    fn toast(&self, window: &AppWindow, kind: ToastKind, code: &str, name: &str, detail: &str) {
        let serial = self.toast_serial.fetch_add(1, Ordering::Relaxed) + 1;
        window.set_toast(Toast {
            serial,
            kind,
            code: code.into(),
            name: name.into(),
            detail: detail.into(),
        });
    }

    fn toast_problem(&self, window: &AppWindow, error: &AccountsError) {
        let (code, detail) = account_problem(error);
        self.toast(window, ToastKind::Problem, &code, "", &detail);
    }

    fn sheet_problem(window: &AppWindow, error: Option<&AccountsError>) {
        let (code, detail) = error.map(account_problem).unwrap_or_default();
        window.set_sheet_problem_code(code);
        window.set_sheet_problem_detail(detail);
    }

    /// Redraws every card (and the open details) from the current state.
    fn render(&self, window: &AppWindow) {
        let state = self.state();
        let now = self.services.clock.now();
        let views: Vec<AccountView> = state
            .accounts
            .iter()
            .map(|saved| {
                let quota = state.quotas.get(&saved.name.to_lowercase());
                view::account_view(saved, quota, state.active_email.as_deref(), now)
            })
            .collect();
        let model = window.get_accounts();
        match model.as_any().downcast_ref::<VecModel<AccountView>>() {
            Some(rows) if rows.row_count() == views.len() => {
                for (index, row) in views.into_iter().enumerate() {
                    if rows.row_data(index).as_ref() != Some(&row) {
                        rows.set_row_data(index, row);
                    }
                }
            }
            Some(rows) => rows.set_vec(views),
            None => window.set_accounts(ModelRc::new(VecModel::from(views))),
        }
        if let Some(name) = &state.details_for
            && let Some(saved) = state.accounts.iter().find(|a| a.name.eq_ignore_ascii_case(name))
        {
            let quota = state.quotas.get(&saved.name.to_lowercase());
            window.set_details(view::details(saved, quota, now, &self.services.clock.time_zone()));
        }
        Self::update_tray(&state, window.get_start_with_windows());
    }

    fn update_tray(state: &State, start_with_windows: bool) {
        let threshold = state.prefs.threshold();
        let rows: Vec<TrayAccount> = state
            .accounts
            .iter()
            .map(|saved| {
                let quota = state.quotas.get(&saved.name.to_lowercase());
                view::tray_account(saved, quota, state.active_email.as_deref(), threshold)
            })
            .collect();
        let active = rows.iter().find(|row| row.active).cloned();
        TRAY.with_borrow(|tray| {
            let Some(tray) = tray else { return };
            tray.set_accounts(ModelRc::new(VecModel::from(rows)));
            let active = active.unwrap_or_default();
            tray.set_active_name(active.name);
            tray.set_active_five_hour(active.five_hour);
            tray.set_active_weekly(active.weekly);
            tray.set_auto_switch(state.prefs.auto_switch);
            tray.set_start_with_windows(start_with_windows);
        });
    }

    fn reload_accounts(self: &Arc<Self>) {
        self.work(
            |inner| {
                let accounts = &inner.services.accounts;
                (
                    accounts.list(),
                    accounts.active_email(),
                    accounts.desktop().is_installed(),
                    accounts.desktop().is_running_installed(),
                )
            },
            |window, inner, (listed, active, installed, running_installed)| {
                window.set_installed(installed);
                window.set_running_installed(running_installed);
                window.set_loaded(true);
                match listed {
                    Ok(list) => {
                        window.set_list_problem_code(SharedString::new());
                        let mut state = inner.state();
                        state
                            .quotas
                            .retain(|key, _| list.iter().any(|a| a.name.to_lowercase() == *key));
                        state.accounts = list;
                        state.active_email = active;
                    }
                    Err(error) => {
                        let (code, detail) = account_problem(&error);
                        window.set_list_problem_code(code);
                        window.set_list_problem_detail(detail);
                    }
                }
                inner.render(window);
                inner.refresh_quota(window);
            },
        );
    }

    fn refresh_quota(self: &Arc<Self>, window: &AppWindow) {
        let (generation, names) = {
            let mut state = self.state();
            state.generation += 1;
            state.pending = state.accounts.len();
            (state.generation, state.accounts.iter().map(|a| a.name.clone()).collect::<Vec<_>>())
        };
        window.set_refreshing(!names.is_empty());
        for name in names {
            let inner = self.clone();
            let target = self.services.accounts.settings().slot(&name);
            self.services.runtime.spawn(async move {
                let quota = inner.services.quota.fetch(&name, &target).await;
                let back = inner.clone();
                let _ = inner.window.upgrade_in_event_loop(move |window| {
                    back.quota_arrived(&window, generation, quota);
                });
            });
        }
    }

    fn quota_arrived(self: &Arc<Self>, window: &AppWindow, generation: u64, quota: AccountQuota) {
        let mut complete = false;
        {
            let mut state = self.state();
            let key = quota.account.to_lowercase();
            let newer =
                state.quotas.get(&key).is_none_or(|known| known.fetched_at <= quota.fetched_at);
            let still_saved = state.accounts.iter().any(|a| a.name.to_lowercase() == key);
            if newer && still_saved {
                state.quotas.insert(key, quota);
            }
            if generation == state.generation {
                state.pending = state.pending.saturating_sub(1);
                if state.pending == 0 {
                    let failed = state.quotas.values().filter(|q| q.error.is_some()).count();
                    let now = self.services.clock.now();
                    let local = self.services.clock.time_zone().to_datetime(now);
                    window.set_refreshing(false);
                    window.set_updated_at(local.strftime("%H:%M").to_string().into());
                    window.set_failed_count(i32::try_from(failed).unwrap_or(i32::MAX));
                    complete = true;
                }
            }
        }
        self.render(window);
        if complete {
            self.consider_switch(window);
        }
    }

    /// Every minute: read the accounts and the active sign-in again, then every quota.
    fn tick(self: &Arc<Self>) {
        self.reload_accounts();
    }

    fn launch(self: &Arc<Self>, window: &AppWindow, name: &str) {
        let accounts = &self.services.accounts;
        let active = {
            let state = self.state();
            let email =
                state.accounts.iter().find(|a| a.name == name).and_then(|a| a.email.clone());
            let quota_email = state.quotas.get(&name.to_lowercase()).and_then(|q| q.email.clone());
            let email = quota_email.or(email);
            matches!((&state.active_email, &email), (Some(a), Some(e)) if a.eq_ignore_ascii_case(e))
        };
        if accounts.antigravity().is_running() {
            if active {
                self.toast(window, ToastKind::Info, "already-open", name, "");
                return;
            }
            window.set_target_name(name.into());
            Self::sheet_problem(window, None);
            window.set_sheet(SheetKind::Switch);
            return;
        }
        self.switch(name.to_owned(), false, active);
    }

    fn switch(self: &Arc<Self>, name: String, close_first: bool, active: bool) {
        self.work(
            move |inner| {
                let accounts = &inner.services.accounts;
                if close_first {
                    accounts.antigravity().close()?;
                }
                if active {
                    // Already signed in as this account: keep Antigravity's own, newer tokens.
                    accounts.antigravity().launch()?;
                    return Ok(name);
                }
                accounts.switch_to(&name).map(|()| name)
            },
            move |window, inner, result: Result<String, AccountsError>| {
                window.set_busy(false);
                match result {
                    Ok(name) => {
                        window.set_sheet(SheetKind::None);
                        {
                            let mut state = inner.state();
                            state.active_email = inner.services.accounts.active_email();
                            state.switch_marker = state.generation;
                            state.snoozed = None;
                        }
                        inner.render(window);
                        let code = if active { "opened" } else { "switched" };
                        inner.toast(window, ToastKind::Success, code, &name, "");
                    }
                    Err(error) if window.get_sheet() == SheetKind::Switch => {
                        Self::sheet_problem(window, Some(&error));
                    }
                    Err(error) => inner.toast_problem(window, &error),
                }
            },
        );
    }

    fn confirm_switch(self: &Arc<Self>, window: &AppWindow) {
        window.set_busy(true);
        Self::sheet_problem(window, None);
        self.switch(window.get_target_name().to_string(), true, false);
    }

    fn show_details(&self, window: &AppWindow, name: &str) {
        self.state().details_for = Some(name.to_owned());
        self.render(window);
        window.set_drawer_open(true);
    }

    fn create_shortcut(self: &Arc<Self>, name: String) {
        self.work(
            move |inner| (inner.services.accounts.create_shortcut(&name), name),
            |window, inner, (result, name)| match result {
                Ok(Ok(_)) => inner.toast(window, ToastKind::Success, "shortcut-created", &name, ""),
                Ok(Err(error)) => {
                    inner.toast(
                        window,
                        ToastKind::Problem,
                        error.code(),
                        &name,
                        &error.to_string(),
                    );
                }
                Err(error) => inner.toast_problem(window, &error),
            },
        );
    }

    fn confirm_remove(self: &Arc<Self>, window: &AppWindow) {
        let name = window.get_target_name().to_string();
        window.set_busy(true);
        self.work(
            move |inner| (inner.services.accounts.remove(&name), name),
            |window, inner, (result, name)| {
                window.set_busy(false);
                match result {
                    Ok(outcome) => {
                        window.set_sheet(SheetKind::None);
                        let code = if outcome.shortcut_error.is_some() {
                            "removed-shortcut-kept"
                        } else {
                            "removed"
                        };
                        inner.toast(window, ToastKind::Success, code, &name, "");
                        inner.reload_accounts();
                    }
                    Err(error) => Self::sheet_problem(window, Some(&error)),
                }
            },
        );
    }

    fn install(self: &Arc<Self>) {
        self.work(
            |inner| inner.services.accounts.desktop().install(),
            |window, inner, result| match result {
                Ok(_) => {
                    window.set_installed(true);
                    inner.toast(window, ToastKind::Success, "installed", "", "");
                }
                Err(error) => {
                    inner.toast(window, ToastKind::Problem, error.code(), "", &error.to_string());
                }
            },
        );
    }

    fn add_submit(self: &Arc<Self>, window: &AppWindow, name: &str) {
        let name = name.trim().to_owned();
        window.set_add_name(name.as_str().into());
        if !is_valid_name(&name) {
            Self::sheet_problem(window, Some(&AccountsError::InvalidName));
            return;
        }
        Self::sheet_problem(window, None);
        window.set_busy(true);
        self.work(
            move |inner| {
                let accounts = &inner.services.accounts;
                match accounts.is_saved(&name) {
                    Ok(true) => Err(AccountsError::AlreadySaved),
                    Ok(false) => Ok((name, accounts.antigravity().is_running())),
                    Err(error) => Err(error),
                }
            },
            |window, inner, result| match result {
                Ok((_, true)) => {
                    window.set_busy(false);
                    window.set_add_step(AddStep::Close);
                }
                Ok((name, false)) => inner.begin_add(name, false),
                Err(error) => {
                    window.set_busy(false);
                    Self::sheet_problem(window, Some(&error));
                }
            },
        );
    }

    fn begin_add(self: &Arc<Self>, name: String, close_first: bool) {
        self.work(
            move |inner| {
                let accounts = &inner.services.accounts;
                if close_first {
                    accounts.antigravity().close()?;
                }
                let session = accounts.begin_add(&name)?;
                *inner.add_session() = Some(session);
                Ok(())
            },
            |window, inner, result: Result<(), AccountsError>| {
                window.set_busy(false);
                match result {
                    Ok(()) => {
                        window.set_add_detected(false);
                        window.set_add_email(SharedString::new());
                        window.set_add_step(AddStep::SignIn);
                        inner.state().polling = true;
                    }
                    Err(error) => Self::sheet_problem(window, Some(&error)),
                }
            },
        );
    }

    fn add_close_and_continue(self: &Arc<Self>, window: &AppWindow) {
        window.set_busy(true);
        Self::sheet_problem(window, None);
        self.begin_add(window.get_add_name().to_string(), true);
    }

    fn poll_sign_in(self: &Arc<Self>) {
        if !self.state().polling {
            return;
        }
        self.work(
            |inner| inner.add_session().as_ref().map(AddSession::detect),
            |window, _inner, result| {
                if let Some(Ok(found)) = result {
                    window.set_add_detected(found.is_some());
                    window.set_add_email(found.flatten().unwrap_or_default().into());
                }
            },
        );
    }

    fn add_save(self: &Arc<Self>, window: &AppWindow) {
        window.set_busy(true);
        Self::sheet_problem(window, None);
        self.work(
            |inner| {
                let mut slot = inner.add_session();
                let Some(session) = slot.as_mut() else {
                    return Err(AccountsError::NotSignedInYet);
                };
                let result = session.save();
                if !matches!(result, Err(AccountsError::NotSignedInYet)) {
                    *slot = None;
                }
                result
            },
            |window, inner, result| {
                window.set_busy(false);
                match result {
                    Ok(outcome) => {
                        inner.state().polling = false;
                        window.set_sheet(SheetKind::None);
                        let code =
                            if outcome.shortcut.is_ok() { "saved" } else { "saved-no-shortcut" };
                        inner.toast(window, ToastKind::Success, code, &outcome.name, "");
                        inner.reload_accounts();
                    }
                    Err(AccountsError::NotSignedInYet) => {
                        Self::sheet_problem(window, Some(&AccountsError::NotSignedInYet));
                    }
                    Err(error) => {
                        inner.state().polling = false;
                        window.set_add_step(AddStep::Name);
                        Self::sheet_problem(window, Some(&error));
                    }
                }
            },
        );
    }

    fn add_cancel(self: &Arc<Self>, window: &AppWindow) {
        self.state().polling = false;
        let session = self.add_session().take();
        let Some(session) = session else {
            window.set_sheet(SheetKind::None);
            Self::sheet_problem(window, None);
            return;
        };
        window.set_busy(true);
        self.work(
            move |_| session.cancel(),
            |window, inner, result| {
                window.set_busy(false);
                window.set_sheet(SheetKind::None);
                Self::sheet_problem(window, None);
                match result {
                    Ok(()) => inner.toast(window, ToastKind::Info, "add-canceled", "", ""),
                    Err(error) => inner.toast_problem(window, &error),
                }
            },
        );
    }

    /// Closing the window keeps the app running in the tray.
    fn close_window(self: &Arc<Self>, window: &AppWindow) {
        let _ = window.hide();
        let first_time = {
            let mut state = self.state();
            let first = !state.prefs.tray_hint_shown;
            state.prefs.tray_hint_shown = true;
            first
        };
        if first_time {
            self.save_prefs();
            self.show_notice(NoticeKind::InTray, "", "", "", "");
        }
    }

    /// Ends the app: an unfinished add puts the previous sign-in back first.
    fn quit(self: &Arc<Self>, window: &AppWindow) {
        let _ = window.hide();
        Self::close_notice();
        TRAY.with_borrow(|tray| {
            if let Some(tray) = tray {
                let _ = tray.hide();
            }
        });
        let session = self.add_session().take();
        match session {
            // Put the previous sign-in back before leaving.
            Some(session) => self.work(
                move |_| session.cancel(),
                |_, _, _| {
                    let _ = slint::quit_event_loop();
                },
            ),
            None => {
                let _ = slint::quit_event_loop();
            }
        }
    }

    /// Brings the window back (from the tray, the taskbar or behind other windows).
    fn show_window(window: &AppWindow) {
        let _ = window.show();
        crate::window::bring_to_front(window.window());
    }

    fn save_prefs(self: &Arc<Self>) {
        let prefs = self.state().prefs;
        self.work(
            move |inner| {
                inner.services.accounts.desktop().preferences().and_then(|store| store.save(&prefs))
            },
            |_, _, result| {
                if let Err(error) = result {
                    tracing::warn!(code = error.code(), %error, "settings could not be saved");
                }
            },
        );
    }

    /// The window's settings sheet (or the tray) changed a value.
    fn settings_changed(self: &Arc<Self>, window: &AppWindow) {
        {
            let mut state = self.state();
            let chosen = Preferences {
                auto_switch: window.get_auto_switch(),
                empty_at_percent: u8::try_from(window.get_empty_at()).unwrap_or(5),
                warning_seconds: u16::try_from(window.get_warning_seconds()).unwrap_or(60),
                ..state.prefs
            }
            .normalized();
            state.prefs = chosen;
            state.all_empty_noticed = false;
        }
        self.save_prefs();
        self.render(window);
    }

    fn set_auto_switch(self: &Arc<Self>, window: &AppWindow, on: bool) {
        window.set_auto_switch(on);
        self.settings_changed(window);
    }

    fn set_start_with_windows(self: &Arc<Self>, window: &AppWindow, on: bool) {
        window.set_start_with_windows(on);
        self.render(window);
        self.work(
            move |inner| inner.services.accounts.desktop().set_start_with_windows(on),
            move |window, inner, result| {
                if let Err(error) = result {
                    let actual = inner.services.accounts.desktop().starts_with_windows();
                    window.set_start_with_windows(actual);
                    inner.render(window);
                    inner.toast(window, ToastKind::Problem, error.code(), "", &error.to_string());
                }
            },
        );
    }

    /// An account picked in the tray menu: the window opens only when a question is needed.
    fn pick_from_tray(self: &Arc<Self>, window: &AppWindow, name: &str) {
        if self.services.accounts.antigravity().is_running() {
            Self::show_window(window);
        }
        self.launch(window, name);
    }

    /// The account Antigravity is signed in with, by name.
    fn active_name(state: &State) -> Option<String> {
        state
            .accounts
            .iter()
            .find(|saved| {
                let quota = state.quotas.get(&saved.name.to_lowercase());
                view::is_active(saved, quota, state.active_email.as_deref())
            })
            .map(|saved| saved.name.clone())
    }

    /// After a complete reading: leave an account that ran out, if the person wants that.
    fn consider_switch(self: &Arc<Self>, window: &AppWindow) {
        if self.add_session().is_some() {
            return;
        }
        let mut state = self.state();
        if !state.prefs.auto_switch || state.switching || state.generation <= state.switch_marker {
            return;
        }
        let threshold = state.prefs.threshold();
        let Some(active) = Self::active_name(&state) else { return };
        let fuels: Vec<Fuel> = state
            .accounts
            .iter()
            .filter_map(|saved| state.quotas.get(&saved.name.to_lowercase()))
            .filter_map(|quota| Fuel::from_quota(quota, threshold))
            .collect();
        if let Some(snoozed) = state.snoozed.clone() {
            let refilled = fuels.iter().any(|fuel| {
                fuel.account.eq_ignore_ascii_case(&snoozed) && !fuel.is_empty(threshold)
            });
            if refilled || !snoozed.eq_ignore_ascii_case(&active) {
                state.snoozed = None;
            } else {
                return;
            }
        }
        match decide(&active, &fuels, threshold) {
            Decision::Stay => state.all_empty_noticed = false,
            Decision::SwitchTo(target) => {
                state.all_empty_noticed = false;
                state.switching = true;
                let warning = state.prefs.warning_seconds;
                let left = fuels
                    .iter()
                    .find(|fuel| fuel.account == target)
                    .map(|fuel| percent(fuel.left()))
                    .unwrap_or_default();
                drop(state);
                tracing::info!(from = %active, to = %target, "out of quota: switching automatically");
                if warning > 0 && self.services.accounts.antigravity().is_running() {
                    self.start_countdown(&active, &target, &left, warning);
                } else {
                    self.perform_auto_switch(active, target);
                }
            }
            Decision::AllEmpty(first) => {
                if state.all_empty_noticed {
                    return;
                }
                state.all_empty_noticed = true;
                drop(state);
                tracing::info!("every account is out of quota");
                let (name, when) = first
                    .map(|(name, at)| {
                        let now = self.services.clock.now();
                        (name, reset_time(at, now, &self.services.clock.time_zone()))
                    })
                    .unwrap_or_default();
                self.show_notice(NoticeKind::AllEmpty, "", &name, "", &when);
                let _ = window;
            }
        }
    }

    fn start_countdown(self: &Arc<Self>, from: &str, to: &str, left: &str, seconds: u16) {
        self.show_notice(NoticeKind::Countdown, from, to, left, "");
        let total = i32::from(seconds);
        NOTICE.with_borrow(|notice| {
            if let Some(notice) = notice {
                notice.set_seconds_total(total);
                notice.set_seconds_left(total);
            }
        });
        let timer = slint::Timer::default();
        let inner = self.clone();
        let (from, to) = (from.to_owned(), to.to_owned());
        timer.start(slint::TimerMode::Repeated, COUNTDOWN_STEP, move || {
            let left = NOTICE.with_borrow(|notice| {
                notice.as_ref().map_or(0, |notice| {
                    let left = notice.get_seconds_left() - 1;
                    notice.set_seconds_left(left);
                    left
                })
            });
            if left <= 0 {
                inner.finish_countdown(&from, &to, true);
            }
        });
        COUNTDOWN.with_borrow_mut(|slot| *slot = Some(timer));
    }

    /// Ends the countdown: switch now, or leave this account alone until it refills.
    fn finish_countdown(self: &Arc<Self>, from: &str, to: &str, switch: bool) {
        Self::close_notice();
        if switch {
            self.perform_auto_switch(from.to_owned(), to.to_owned());
        } else {
            let mut state = self.state();
            state.switching = false;
            state.snoozed = Some(from.to_owned());
            tracing::info!(account = from, "automatic switch declined until it refills");
        }
    }

    /// Switches: closes and restarts Antigravity when it runs, else only changes the sign-in.
    fn perform_auto_switch(self: &Arc<Self>, from: String, to: String) {
        self.work(
            move |inner| {
                let accounts = &inner.services.accounts;
                let antigravity = accounts.antigravity();
                let result = if antigravity.is_running() {
                    antigravity
                        .close()
                        .map_err(AccountsError::from)
                        .and_then(|()| accounts.switch_to(&to))
                        .map(|()| true)
                } else {
                    accounts.sign_in_as(&to).map(|()| false)
                };
                (result, from, to)
            },
            |window, inner, (result, from, to)| {
                {
                    let mut state = inner.state();
                    state.switching = false;
                    state.switch_marker = state.generation;
                    state.active_email = inner.services.accounts.active_email();
                }
                inner.render(window);
                match result {
                    Ok(true) => inner.toast(window, ToastKind::Success, "switched", &to, ""),
                    Ok(false) => inner.show_notice(NoticeKind::SignedIn, &from, &to, "", ""),
                    Err(error) => {
                        tracing::error!(code = error.code(), %error, "automatic switch failed");
                        inner.toast_problem(window, &error);
                    }
                }
            },
        );
    }

    /// Shows the small notice in the bottom-right corner (replacing one already shown).
    fn show_notice(
        self: &Arc<Self>,
        kind: NoticeKind,
        from: &str,
        to: &str,
        left: &str,
        refill: &str,
    ) {
        let existing = NOTICE.with_borrow(Option::is_some);
        if !existing {
            let Ok(notice) = NoticeWindow::new() else { return };
            let inner = self.clone();
            let weak = notice.as_weak();
            notice.on_act(move || {
                if let Some(notice) = weak.upgrade() {
                    inner.finish_countdown(&notice.get_from(), &notice.get_to(), true);
                }
            });
            let inner = self.clone();
            let weak = notice.as_weak();
            notice.on_dismiss(move || {
                let Some(notice) = weak.upgrade() else { return };
                if notice.get_kind() == NoticeKind::Countdown {
                    inner.finish_countdown(&notice.get_from(), &notice.get_to(), false);
                } else {
                    Self::close_notice();
                }
            });
            NOTICE.with_borrow_mut(|slot| *slot = Some(notice));
        }
        NOTICE.with_borrow(|notice| {
            let Some(notice) = notice else { return };
            notice.set_kind(kind);
            notice.set_from(from.into());
            notice.set_to(to.into());
            notice.set_left(left.into());
            notice.set_refill(refill.into());
            let _ = notice.show();
            crate::window::place_bottom_right(notice.window());
        });
    }

    fn close_notice() {
        COUNTDOWN.with_borrow_mut(|slot| *slot = None);
        NOTICE.with_borrow_mut(|slot| {
            if let Some(notice) = slot.take() {
                let _ = notice.hide();
            }
        });
    }
}

impl Controller {
    /// Wires `window` to `services` and starts loading.
    #[must_use]
    pub fn new(window: &AppWindow, services: Services) -> Self {
        let inner = Arc::new(Inner {
            services,
            window: window.as_weak(),
            state: Mutex::new(State::default()),
            add: Mutex::new(None),
            toast_serial: AtomicI32::new(0),
        });
        window.set_version(env!("CARGO_PKG_VERSION").into());
        window.set_accounts(ModelRc::from(Rc::new(VecModel::<AccountView>::default())));
        {
            let desktop = inner.services.accounts.desktop();
            let prefs = desktop.preferences().map(|store| store.load()).unwrap_or_default();
            inner.state().prefs = prefs;
            window.set_auto_switch(prefs.auto_switch);
            window.set_empty_at(i32::from(prefs.empty_at_percent));
            window.set_warning_seconds(i32::from(prefs.warning_seconds));
            window.set_start_with_windows(desktop.starts_with_windows());
        }

        macro_rules! on {
            ($setter:ident, |$window:ident, $inner:ident $(, $arg:ident)*| $body:expr) => {{
                let weak = window.as_weak();
                let inner = inner.clone();
                window.$setter(move |$($arg),*| {
                    if let Some($window) = weak.upgrade() {
                        let $inner = &inner;
                        $body;
                    }
                });
            }};
        }
        on!(on_refresh, |w, i| i.refresh_quota(&w));
        on!(on_launch, |w, i, name| i.launch(&w, &name));
        on!(on_confirm_switch, |w, i| i.confirm_switch(&w));
        on!(on_show_details, |w, i, name| i.show_details(&w, &name));
        on!(on_create_shortcut, |w, i, name| {
            let _ = &w;
            i.create_shortcut(name.to_string());
        });
        on!(on_confirm_remove, |w, i| i.confirm_remove(&w));
        on!(on_install, |w, i| {
            let _ = &w;
            i.install();
        });
        on!(on_add_submit, |w, i, name| i.add_submit(&w, &name));
        on!(on_add_close_and_continue, |w, i| i.add_close_and_continue(&w));
        on!(on_add_save, |w, i| i.add_save(&w));
        on!(on_add_cancel, |w, i| i.add_cancel(&w));
        on!(on_sheet_closed, |w, i| {
            let _ = i;
            Inner::sheet_problem(&w, None);
        });
        on!(on_close_window, |w, i| i.close_window(&w));
        on!(on_quit, |w, i| i.quit(&w));
        on!(on_settings_changed, |w, i| i.settings_changed(&w));
        on!(on_start_with_windows_toggled, |w, i, on| i.set_start_with_windows(&w, on));
        Self::create_tray(window, &inner);
        {
            // Alt+F4 and the taskbar's "Close window" keep the app in the tray, like our close button.
            let weak = window.as_weak();
            let inner = inner.clone();
            window.window().on_close_requested(move || {
                if let Some(window) = weak.upgrade() {
                    inner.close_window(&window);
                }
                slint::CloseRequestResponse::HideWindow
            });
        }

        let tick = slint::Timer::default();
        {
            let weak = window.as_weak();
            let inner = inner.clone();
            tick.start(slint::TimerMode::Repeated, REFRESH, move || {
                if weak.upgrade().is_some() {
                    inner.tick();
                }
            });
        }
        let poll = slint::Timer::default();
        {
            let inner = inner.clone();
            poll.start(slint::TimerMode::Repeated, SIGN_IN_POLL, move || inner.poll_sign_in());
        }

        inner.render(window);
        inner.reload_accounts();
        Self { inner, _tick: tick, _poll: poll }
    }

    /// The tray icon and its menu; the app works without one if the system refuses it.
    fn create_tray(window: &AppWindow, inner: &Arc<Inner>) {
        let Ok(tray) = Tray::new() else {
            tracing::warn!("no tray icon");
            return;
        };
        macro_rules! on_tray {
            ($setter:ident, |$window:ident, $inner:ident $(, $arg:ident)*| $body:expr) => {{
                let weak = window.as_weak();
                let inner = inner.clone();
                tray.$setter(move |$($arg),*| {
                    if let Some($window) = weak.upgrade() {
                        let $inner = &inner;
                        $body;
                    }
                });
            }};
        }
        on_tray!(on_open, |w, i| {
            let _ = i;
            Inner::show_window(&w);
        });
        on_tray!(on_pick, |w, i, name| i.pick_from_tray(&w, &name));
        on_tray!(on_auto_switch_toggled, |w, i, on| i.set_auto_switch(&w, on));
        on_tray!(on_start_with_windows_toggled, |w, i, on| i.set_start_with_windows(&w, on));
        on_tray!(on_open_settings, |w, i| {
            let _ = i;
            Inner::show_window(&w);
            w.set_sheet(SheetKind::Settings);
        });
        on_tray!(on_quit, |w, i| i.quit(&w));
        TRAY.with_borrow_mut(|slot| *slot = Some(tray));
    }

    /// Whether an account is being added right now.
    #[must_use]
    pub fn is_adding(&self) -> bool {
        self.inner.add_session().is_some()
    }

    /// Shows the window (another start asked for it, or the app was started normally).
    pub fn show(&self) {
        if let Some(window) = self.inner.window.upgrade() {
            Inner::show_window(&window);
        }
    }
}

/// How the app was started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Start {
    /// Show the window.
    Window,
    /// Stay in the tray (started with Windows).
    Tray,
}

/// Runs the app until Quit. `wake` returns true whenever another start asks for the window; it is
/// polled on its own thread.
pub fn run(
    services: Services,
    start: Start,
    wake: Option<Box<dyn Fn() -> bool + Send>>,
) -> Result<(), slint::PlatformError> {
    let window = AppWindow::new()?;
    let controller = Controller::new(&window, services);
    if start == Start::Window {
        controller.show();
    }
    if let Some(wake) = wake {
        let weak = window.as_weak();
        std::thread::spawn(move || {
            loop {
                if wake() {
                    let _ = weak.upgrade_in_event_loop(|window| Inner::show_window(&window));
                }
            }
        });
    }
    slint::run_event_loop_until_quit()?;
    release();
    drop(controller);
    drop(window);
    Ok(())
}

/// Releases the tray icon and the notice. Call it after the event loop ends, while the window
/// system still runs: tearing them down during process exit hangs on Windows.
pub fn release() {
    Inner::close_notice();
    TRAY.with_borrow_mut(|slot| *slot = None);
}

/// The notice in the bottom-right corner, while one is shown.
#[must_use]
pub fn shown_notice() -> Option<NoticeWindow> {
    NOTICE.with_borrow(|notice| notice.as_ref().map(ComponentHandle::clone_strong))
}

#[cfg(test)]
mod tests {
    use vault::VaultError;

    use super::*;

    fn setup() -> (tempfile::TempDir, AppWindow, Controller) {
        i_slint_backend_testing::init_no_event_loop();
        let dir = tempfile::tempdir().unwrap();
        let services =
            crate::bootstrap::build(crate::bootstrap::preview_parts(dir.path())).unwrap();
        let window = AppWindow::new().unwrap();
        let controller = Controller::new(&window, services);
        (dir, window, controller)
    }

    #[test]
    fn starts_empty_with_the_version() {
        let (_dir, window, controller) = setup();
        assert_eq!(window.get_version(), env!("CARGO_PKG_VERSION"));
        assert_eq!(window.get_accounts().row_count(), 0);
        assert_eq!(window.get_sheet(), SheetKind::None);
        assert!(!controller.is_adding());
        assert!(format!("{controller:?}").starts_with("Controller"));
    }

    #[test]
    fn an_invalid_name_is_refused_before_any_work() {
        let (_dir, window, _controller) = setup();
        window.invoke_open_add();
        assert_eq!(window.get_sheet(), SheetKind::Add);
        assert_eq!(window.get_add_step(), AddStep::Name);
        window.invoke_add_submit("  bad name ".into());
        assert_eq!(window.get_add_name(), "bad name");
        assert_eq!(window.get_sheet_problem_code(), "accounts.invalid_name");
        assert!(!window.get_busy());
        window.invoke_sheet_closed();
        assert_eq!(window.get_sheet_problem_code(), "");
    }

    #[test]
    fn a_valid_name_starts_background_work() {
        let (_dir, window, _controller) = setup();
        window.invoke_open_add();
        window.invoke_add_submit("work".into());
        assert_eq!(window.get_sheet_problem_code(), "");
        assert!(window.get_busy(), "waiting for the name check");
    }

    #[test]
    fn cancelling_without_a_session_just_closes_the_sheet() {
        let (_dir, window, controller) = setup();
        window.invoke_open_add();
        window.invoke_add_cancel();
        assert_eq!(window.get_sheet(), SheetKind::None);
        assert!(!controller.is_adding());
    }

    #[test]
    fn closing_from_windows_hides_the_window() {
        let (_dir, window, controller) = setup();
        window.show().unwrap();
        assert!(window.window().is_visible());
        window.window().dispatch_event(slint::platform::WindowEvent::CloseRequested);
        assert!(!window.window().is_visible(), "Alt+F4 or the taskbar hide it to the tray");
        assert!(NOTICE.with_borrow(Option::is_some), "the first time, a note says it still runs");
        assert!(controller.inner.state().prefs.tray_hint_shown);
    }

    // --- automatic switching -------------------------------------------------------------

    struct Rig {
        _dir: tempfile::TempDir,
        store: Arc<vault::MemoryStore>,
        window: AppWindow,
        controller: Controller,
    }

    fn blob(email: &str) -> Vec<u8> {
        format!(r#"{{"id_token_email":"{email}","token":{{"refresh_token":"r"}}}}"#).into_bytes()
    }

    fn reading(account: &str, email: &str, five_hour: f64, weekly: f64) -> AccountQuota {
        let mut quota = AccountQuota::new(account, jiff::Timestamp::now());
        quota.email = Some(email.into());
        quota.groups.push(quota::QuotaGroup {
            name: String::new(),
            description: String::new(),
            buckets: [("5h", five_hour), ("weekly", weekly)]
                .into_iter()
                .map(|(window, fraction)| quota::QuotaBucket {
                    id: window.into(),
                    name: String::new(),
                    window: window.into(),
                    fraction,
                    reset: None,
                })
                .collect(),
        });
        quota
    }

    /// Accounts a (active, 2% left) and b (90% left), Antigravity running or not.
    fn rig(antigravity_running: bool) -> Rig {
        i_slint_backend_testing::init_no_event_loop();
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(vault::MemoryStore::new());
        store.insert("antigravity-switcher:a", &blob("a@example.com"));
        store.insert("antigravity-switcher:b", &blob("b@example.com"));
        store.insert("gemini:antigravity", &blob("a@example.com"));
        let host =
            Arc::new(antigravity::FakeHost::with_processes(usize::from(antigravity_running)));
        let parts = crate::bootstrap::Parts {
            store: store.clone(),
            host,
            ..crate::bootstrap::preview_parts(dir.path())
        };
        let exe = dir.path().join(r"LocalAppData\Programs\antigravity\Antigravity.exe");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"x").unwrap();
        let services = crate::bootstrap::build(parts).unwrap();
        let window = AppWindow::new().unwrap();
        let controller = Controller::new(&window, services);
        {
            let mut state = controller.inner.state();
            state.accounts = vec![
                SavedAccount { name: "a".into(), email: Some("a@example.com".into()) },
                SavedAccount { name: "b".into(), email: Some("b@example.com".into()) },
            ];
            state.active_email = Some("a@example.com".into());
            state.quotas.insert("a".into(), reading("a", "a@example.com", 0.02, 0.6));
            state.quotas.insert("b".into(), reading("b", "b@example.com", 0.9, 0.9));
            state.generation = 3;
            state.switch_marker = 0;
        }
        Rig { _dir: dir, store, window, controller }
    }

    fn signed_in(rig: &Rig) -> Vec<u8> {
        vault::CredentialStore::read_blob(rig.store.as_ref(), "gemini:antigravity")
            .unwrap()
            .unwrap()
    }

    fn wait_for(rig: &Rig, email: &str) -> bool {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if signed_in(rig) == blob(email) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn an_empty_account_is_left_quietly_when_antigravity_is_closed() {
        let rig = rig(false);
        rig.controller.inner.consider_switch(&rig.window);
        assert!(rig.controller.inner.state().switching);
        assert!(wait_for(&rig, "b@example.com"), "signed in as b without starting Antigravity");
    }

    #[test]
    fn an_open_antigravity_gets_a_countdown_first() {
        let rig = rig(true);
        rig.controller.inner.consider_switch(&rig.window);
        let kind = NOTICE.with_borrow(|notice| {
            notice.as_ref().map(|n| (n.get_kind(), n.get_to(), n.get_seconds_left()))
        });
        assert_eq!(kind, Some((NoticeKind::Countdown, "b".into(), 60)));
        assert!(COUNTDOWN.with_borrow(Option::is_some));
        assert_eq!(signed_in(&rig), blob("a@example.com"), "nothing changes during the countdown");

        rig.controller.inner.finish_countdown("a", "b", false);
        let state = rig.controller.inner.state();
        assert_eq!(state.snoozed.as_deref(), Some("a"));
        assert!(!state.switching);
        assert!(NOTICE.with_borrow(Option::is_none));
    }

    #[test]
    fn switch_now_switches() {
        let rig = rig(true);
        rig.controller.inner.consider_switch(&rig.window);
        rig.controller.inner.finish_countdown("a", "b", true);
        assert!(wait_for(&rig, "b@example.com"));
    }

    #[test]
    fn no_warning_switches_at_once() {
        let rig = rig(true);
        rig.controller.inner.state().prefs.warning_seconds = 0;
        rig.controller.inner.consider_switch(&rig.window);
        assert!(NOTICE.with_borrow(Option::is_none));
        assert!(wait_for(&rig, "b@example.com"));
    }

    #[test]
    fn snoozed_account_is_left_alone_until_it_refills() {
        let rig = rig(false);
        rig.controller.inner.state().snoozed = Some("a".into());
        rig.controller.inner.consider_switch(&rig.window);
        assert!(!rig.controller.inner.state().switching);

        let refilled = reading("a", "a@example.com", 1.0, 1.0);
        rig.controller.inner.state().quotas.insert("a".into(), refilled);
        rig.controller.inner.consider_switch(&rig.window);
        assert_eq!(rig.controller.inner.state().snoozed, None, "refilled: the snooze ends");
    }

    #[test]
    fn auto_switch_off_decides_nothing() {
        let rig = rig(false);
        rig.controller.inner.state().prefs.auto_switch = false;
        rig.controller.inner.consider_switch(&rig.window);
        assert!(!rig.controller.inner.state().switching);
    }

    #[test]
    fn decisions_wait_for_fresh_quota() {
        let rig = rig(false);
        rig.controller.inner.state().switch_marker = 3;
        rig.controller.inner.consider_switch(&rig.window);
        assert!(!rig.controller.inner.state().switching);
    }

    #[test]
    fn every_account_empty_shows_one_notice() {
        let rig = rig(false);
        let empty = reading("b", "b@example.com", 0.01, 0.9);
        rig.controller.inner.state().quotas.insert("b".into(), empty);
        rig.controller.inner.consider_switch(&rig.window);
        let kind = NOTICE.with_borrow(|notice| notice.as_ref().map(NoticeWindow::get_kind));
        assert_eq!(kind, Some(NoticeKind::AllEmpty));
        assert!(rig.controller.inner.state().all_empty_noticed);
        assert!(!rig.controller.inner.state().switching);
    }

    #[test]
    fn settings_are_saved_and_applied() {
        let rig = rig(false);
        rig.window.set_empty_at(12);
        rig.window.set_warning_seconds(30);
        rig.window.invoke_settings_changed();
        let prefs = rig.controller.inner.state().prefs;
        assert_eq!((prefs.empty_at_percent, prefs.warning_seconds), (12, 30));
        let store = rig.controller.inner.services.accounts.desktop().preferences().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while store.load() != prefs && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(store.load(), prefs);
    }

    #[test]
    fn quitting_hides_everything() {
        let rig = rig(false);
        rig.window.show().unwrap();
        rig.window.invoke_quit();
        assert!(!rig.window.window().is_visible());
    }

    #[test]
    fn details_open_the_drawer_while_quota_loads() {
        let (_dir, window, _controller) = setup();
        window.invoke_show_details("work".into());
        assert!(window.get_drawer_open());
    }

    #[test]
    fn account_problems_carry_useful_details_only() {
        let vault = AccountsError::Vault(VaultError::System { target: "x".into(), code: 5 });
        let (code, detail) = account_problem(&vault);
        assert_eq!(code, "vault.system");
        assert!(detail.contains("Credential Manager"), "{detail}");
        assert_eq!(
            account_problem(&AccountsError::InvalidName),
            ("accounts.invalid_name".into(), "".into())
        );
        let start =
            AccountsError::Antigravity(antigravity::AntigravityError::StartFailed("denied".into()));
        assert!(account_problem(&start).1.contains("denied"));
    }
}
