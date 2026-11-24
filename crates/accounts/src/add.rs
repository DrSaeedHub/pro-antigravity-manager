//! The guided add flow: keep the current sign-in aside, let the person sign in fresh, save the
//! new sign-in as an account, and put the previous sign-in back if anything goes wrong.

use std::path::PathBuf;

use desktop::DesktopError;
use vault::read_email;

use crate::error::AccountsError;
use crate::name::is_valid_name;
use crate::service::Accounts;

/// What happened when an account was saved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddOutcome {
    /// The saved account's name.
    pub name: String,
    /// Its email, when readable.
    pub email: Option<String>,
    /// The desktop shortcut, or why it could not be created (the account is saved either way).
    pub shortcut: Result<PathBuf, DesktopError>,
}

/// An account being added. Dropping it unfinished puts the previous sign-in back.
#[derive(Debug)]
pub struct AddSession {
    accounts: Accounts,
    name: String,
    had_previous: bool,
    finished: bool,
}

impl AddSession {
    pub(crate) fn start(accounts: Accounts, name: &str) -> Result<Self, AccountsError> {
        if !is_valid_name(name) {
            return Err(AccountsError::InvalidName);
        }
        let settings = accounts.settings.clone();
        let had_previous = accounts.store.exists(&settings.canonical)?;
        if had_previous {
            accounts.store.copy(&settings.canonical, &settings.rollback())?;
        }
        let mut session = Self { accounts, name: name.to_owned(), had_previous, finished: false };
        let prepared = session
            .accounts
            .store
            .delete(&settings.canonical)
            .map_err(AccountsError::from)
            .and_then(|()| session.accounts.antigravity.launch().map_err(AccountsError::from));
        if let Err(error) = prepared {
            session.finish_with_restore();
            return Err(error);
        }
        tracing::info!(account = name, "adding an account: waiting for the sign-in");
        Ok(session)
    }

    /// The name being added.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether Antigravity has saved a new sign-in yet, and its email.
    pub fn detect(&self) -> Result<Option<Option<String>>, AccountsError> {
        let canonical = &self.accounts.settings.canonical;
        if self.accounts.store.exists(canonical)? {
            Ok(Some(read_email(self.accounts.store.as_ref(), canonical)))
        } else {
            Ok(None)
        }
    }

    /// Saves the new sign-in as the account. When Antigravity has not signed in yet this fails
    /// with [`AccountsError::NotSignedInYet`] and the session stays open; any other failure puts
    /// the previous sign-in back and ends the session.
    pub fn save(&mut self) -> Result<AddOutcome, AccountsError> {
        let Some(email) = self.detect()? else { return Err(AccountsError::NotSignedInYet) };
        let settings = self.accounts.settings.clone();
        let slot = settings.slot(&self.name);
        let saved = self
            .accounts
            .store
            .copy(&settings.canonical, &slot)
            .map_err(AccountsError::from)
            .and_then(|()| match self.accounts.store.exists(&slot) {
                Ok(true) => Ok(()),
                Ok(false) => Err(AccountsError::SaveNotVerified),
                Err(error) => Err(error.into()),
            });
        if let Err(error) = saved {
            self.finish_with_restore();
            return Err(error);
        }
        self.finished = true;
        if let Err(error) = self.accounts.store.delete(&settings.rollback()) {
            tracing::warn!(code = error.code(), "the kept-aside sign-in could not be removed");
        }
        tracing::info!(account = %self.name, "account saved");
        let shortcut = self.accounts.desktop.create_account_shortcut(&self.name);
        Ok(AddOutcome { name: self.name.clone(), email, shortcut })
    }

    /// Gives up: closes Antigravity and puts the previous sign-in back.
    pub fn cancel(mut self) -> Result<(), AccountsError> {
        if let Err(error) = self.accounts.antigravity.close() {
            tracing::warn!(code = error.code(), "Antigravity did not close while cancelling");
        }
        self.finished = true;
        self.restore()
    }

    fn finish_with_restore(&mut self) {
        self.finished = true;
        if let Err(error) = self.restore() {
            tracing::error!(code = error.code(), "the previous sign-in could not be restored");
        }
    }

    /// Puts the sign-in from before the session back (or none, if there was none).
    fn restore(&self) -> Result<(), AccountsError> {
        let settings = &self.accounts.settings;
        let store = &self.accounts.store;
        let rollback = settings.rollback();
        if self.had_previous && store.exists(&rollback)? {
            store.copy(&rollback, &settings.canonical)?;
        } else if !self.had_previous {
            store.delete(&settings.canonical)?;
        }
        store.delete(&rollback)?;
        tracing::info!(account = %self.name, "previous sign-in restored");
        Ok(())
    }
}

impl Drop for AddSession {
    fn drop(&mut self) {
        if !self.finished {
            self.finish_with_restore();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use vault::{CredentialStore, Operation};

    use super::*;
    use crate::service::tests::{CANONICAL, World, sign_in};

    const ROLLBACK: &str = "antigravity-switcher:setup-rollback";

    #[test]
    fn the_whole_flow_saves_the_new_sign_in() {
        let world = World::new();
        world.store.insert(CANONICAL, &sign_in("old@example.com"));

        let mut session = world.accounts.begin_add("work").unwrap();
        assert_eq!(session.name(), "work");
        assert_eq!(world.signed_in(), None, "signed out so a new sign-in can happen");
        assert_eq!(world.host.started().len(), 1, "Antigravity started for the sign-in");
        assert!(world.store.exists(ROLLBACK).unwrap());
        assert_eq!(session.detect().unwrap(), None);
        assert_eq!(session.save().unwrap_err(), AccountsError::NotSignedInYet);

        world.store.insert(CANONICAL, &sign_in("new@example.com"));
        assert_eq!(session.detect().unwrap(), Some(Some("new@example.com".into())));
        let outcome = session.save().unwrap();
        assert_eq!(outcome.name, "work");
        assert_eq!(outcome.email.as_deref(), Some("new@example.com"));
        assert!(outcome.shortcut.unwrap().exists());
        drop(session);

        assert_eq!(world.accounts.list().unwrap()[0].email.as_deref(), Some("new@example.com"));
        assert!(!world.store.exists(ROLLBACK).unwrap());
        assert_eq!(world.signed_in(), Some(sign_in("new@example.com")), "stays signed in as new");
    }

    #[test]
    fn a_sign_in_without_email_is_still_saved() {
        let world = World::new();
        let mut session = world.accounts.begin_add("work").unwrap();
        world.store.insert(CANONICAL, b"{}");
        assert_eq!(session.detect().unwrap(), Some(None));
        assert_eq!(session.save().unwrap().email, None);
    }

    #[test]
    fn cancelling_restores_the_previous_sign_in() {
        let world = World::new();
        world.store.insert(CANONICAL, &sign_in("old@example.com"));
        let session = world.accounts.begin_add("work").unwrap();
        world.store.insert(CANONICAL, &sign_in("half-done@example.com"));
        session.cancel().unwrap();
        assert_eq!(world.signed_in(), Some(sign_in("old@example.com")));
        assert!(!world.store.exists(ROLLBACK).unwrap());
        assert!(!world.accounts.antigravity().is_running(), "Antigravity closed");
        assert!(!world.accounts.is_saved("work").unwrap());
    }

    #[test]
    fn cancelling_without_a_previous_sign_in_signs_out() {
        let world = World::new();
        let session = world.accounts.begin_add("work").unwrap();
        world.store.insert(CANONICAL, &sign_in("half-done@example.com"));
        session.cancel().unwrap();
        assert_eq!(world.signed_in(), None);
    }

    #[test]
    fn cancelling_restores_even_if_antigravity_will_not_close() {
        let world = World::new();
        world.store.insert(CANONICAL, &sign_in("old@example.com"));
        let session = world.accounts.begin_add("work").unwrap();
        world.host.ignore_close_requests();
        world.host.survive_kills();
        session.cancel().unwrap();
        assert_eq!(world.signed_in(), Some(sign_in("old@example.com")));
    }

    #[test]
    fn cancelling_reports_a_failed_restore() {
        let world = World::new();
        world.store.insert(CANONICAL, &sign_in("old@example.com"));
        let session = world.accounts.begin_add("work").unwrap();
        world.store.fail(Operation::Copy, CANONICAL);
        assert_eq!(session.cancel().unwrap_err().code(), "vault.system");
    }

    #[test]
    fn dropping_an_unfinished_session_restores() {
        let world = World::new();
        world.store.insert(CANONICAL, &sign_in("old@example.com"));
        let session = world.accounts.begin_add("work").unwrap();
        world.store.insert(CANONICAL, &sign_in("half-done@example.com"));
        drop(session);
        assert_eq!(world.signed_in(), Some(sign_in("old@example.com")));
    }

    #[test]
    fn a_failed_save_restores_and_ends_the_session() {
        let world = World::new();
        world.store.insert(CANONICAL, &sign_in("old@example.com"));
        let mut session = world.accounts.begin_add("work").unwrap();
        world.store.insert(CANONICAL, &sign_in("new@example.com"));
        world.store.fail(Operation::Copy, &World::slot("work"));
        assert_eq!(session.save().unwrap_err().code(), "vault.system");
        assert_eq!(world.signed_in(), Some(sign_in("old@example.com")));
        assert!(!world.store.exists(ROLLBACK).unwrap());
    }

    #[test]
    fn a_failed_restore_is_logged_not_raised() {
        let world = World::new();
        world.store.insert(CANONICAL, &sign_in("old@example.com"));
        let mut session = world.accounts.begin_add("work").unwrap();
        world.store.insert(CANONICAL, &sign_in("new@example.com"));
        world.store.fail(Operation::Copy, &World::slot("work"));
        world.store.fail(Operation::Copy, CANONICAL);
        assert_eq!(session.save().unwrap_err().code(), "vault.system");
    }

    #[test]
    fn a_shortcut_failure_does_not_undo_the_save() {
        let world = World::new();
        let mut session = world.accounts.begin_add("work").unwrap();
        world.store.insert(CANONICAL, &sign_in("new@example.com"));
        world.shell.fail();
        let outcome = session.save().unwrap();
        assert_eq!(outcome.shortcut.unwrap_err().code(), "desktop.shortcut");
        assert!(world.accounts.is_saved("work").unwrap());
    }

    #[test]
    fn a_leftover_kept_aside_sign_in_is_only_logged() {
        let world = World::new();
        world.store.insert(CANONICAL, &sign_in("old@example.com"));
        let mut session = world.accounts.begin_add("work").unwrap();
        world.store.insert(CANONICAL, &sign_in("new@example.com"));
        world.store.fail(Operation::Delete, ROLLBACK);
        assert!(session.save().is_ok());
    }

    #[test]
    fn starting_checks_preconditions() {
        let world = World::new();
        assert_eq!(world.accounts.begin_add("bad name").unwrap_err(), AccountsError::InvalidName);
        world.save("work", "w@example.com");
        assert_eq!(world.accounts.begin_add("WORK").unwrap_err(), AccountsError::AlreadySaved);
        world.accounts.antigravity().launch().unwrap();
        assert_eq!(world.accounts.begin_add("new").unwrap_err(), AccountsError::AntigravityOpen);
    }

    #[test]
    fn a_missing_antigravity_restores_before_failing() {
        let world = World::new();
        world.store.insert(CANONICAL, &sign_in("old@example.com"));
        fs::remove_dir_all(world.root.path().join("LocalAppData\\Programs")).unwrap();
        let error = world.accounts.begin_add("work").unwrap_err();
        assert_eq!(error.code(), "antigravity.not_installed");
        assert_eq!(world.signed_in(), Some(sign_in("old@example.com")));
        assert!(!world.store.exists(ROLLBACK).unwrap());
    }

    #[test]
    fn starting_reports_store_failures() {
        let world = World::new();
        world.store.insert(CANONICAL, &sign_in("old@example.com"));
        world.store.fail(Operation::Copy, ROLLBACK);
        assert_eq!(world.accounts.begin_add("work").unwrap_err().code(), "vault.system");
        assert_eq!(world.signed_in(), Some(sign_in("old@example.com")));

        world.store.heal();
        world.store.fail(Operation::Delete, CANONICAL);
        assert_eq!(world.accounts.begin_add("work").unwrap_err().code(), "vault.system");
    }

    #[test]
    fn detection_and_saving_report_read_failures() {
        let world = World::new();
        let mut session = world.accounts.begin_add("work").unwrap();
        world.store.fail(Operation::Read, CANONICAL);
        assert_eq!(session.detect().unwrap_err().code(), "vault.system");
        assert_eq!(session.save().unwrap_err().code(), "vault.system");
        world.store.heal();
        session.cancel().unwrap();
    }

    #[test]
    fn an_unverifiable_save_restores() {
        let world = World::new();
        world.store.insert(CANONICAL, &sign_in("old@example.com"));
        let mut session = world.accounts.begin_add("work").unwrap();
        world.store.insert(CANONICAL, &sign_in("new@example.com"));
        // The copy "works" but the slot cannot be read back.
        world.store.fail(Operation::Read, &World::slot("work"));
        assert_eq!(session.save().unwrap_err().code(), "vault.system");
        world.store.heal();
        assert_eq!(world.signed_in(), Some(sign_in("old@example.com")));
    }
}
