//! Listing, switching to and removing saved accounts.

use std::path::PathBuf;
use std::sync::Arc;

use antigravity::Antigravity;
use desktop::{Desktop, DesktopError};
use vault::{CredentialStore, read_email};

use crate::add::AddSession;
use crate::error::AccountsError;
use crate::name::is_valid_name;

/// Credential names (kept from the original app so its saved accounts keep working).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountSettings {
    /// Antigravity's own sign-in.
    pub canonical: String,
    /// Saved accounts are `<prefix><name>`.
    pub slot_prefix: String,
    /// The account name reserved for the sign-in kept aside while adding an account.
    pub rollback_name: String,
}

impl Default for AccountSettings {
    fn default() -> Self {
        Self {
            canonical: "gemini:antigravity".into(),
            slot_prefix: "antigravity-switcher:".into(),
            rollback_name: "setup-rollback".into(),
        }
    }
}

impl AccountSettings {
    /// The credential name of the saved account `name`.
    #[must_use]
    pub fn slot(&self, name: &str) -> String {
        format!("{}{name}", self.slot_prefix)
    }

    /// The credential that holds the previous sign-in while an account is being added.
    #[must_use]
    pub fn rollback(&self) -> String {
        self.slot(&self.rollback_name)
    }
}

/// A saved account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedAccount {
    /// Its name.
    pub name: String,
    /// The email of its sign-in, when readable.
    pub email: Option<String>,
}

/// What happened when an account was removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveOutcome {
    /// The desktop shortcut could not be removed (the account itself was).
    pub shortcut_error: Option<DesktopError>,
}

/// Saved accounts on this PC.
#[derive(Debug, Clone)]
pub struct Accounts {
    pub(crate) store: Arc<dyn CredentialStore>,
    pub(crate) antigravity: Antigravity,
    pub(crate) desktop: Desktop,
    pub(crate) settings: AccountSettings,
}

impl Accounts {
    /// The service over its parts.
    #[must_use]
    pub fn new(
        store: Arc<dyn CredentialStore>,
        antigravity: Antigravity,
        desktop: Desktop,
        settings: AccountSettings,
    ) -> Self {
        Self { store, antigravity, desktop, settings }
    }

    /// Antigravity on this PC.
    #[must_use]
    pub fn antigravity(&self) -> &Antigravity {
        &self.antigravity
    }

    /// Desktop integration.
    #[must_use]
    pub fn desktop(&self) -> &Desktop {
        &self.desktop
    }

    /// The credential store.
    #[must_use]
    pub fn store(&self) -> &Arc<dyn CredentialStore> {
        &self.store
    }

    /// The credential names in use.
    #[must_use]
    pub fn settings(&self) -> &AccountSettings {
        &self.settings
    }

    /// Every saved account, sorted by name (case-insensitive).
    pub fn list(&self) -> Result<Vec<SavedAccount>, AccountsError> {
        let prefix_length = self.settings.slot_prefix.len();
        let mut accounts: Vec<SavedAccount> = self
            .store
            .list(&self.settings.slot_prefix)?
            .into_iter()
            .filter_map(|target| target.get(prefix_length..).map(str::to_owned))
            .filter(|name| {
                is_valid_name(name) && !name.eq_ignore_ascii_case(&self.settings.rollback_name)
            })
            .map(|name| {
                let email = read_email(self.store.as_ref(), &self.settings.slot(&name));
                SavedAccount { name, email }
            })
            .collect();
        accounts.sort_by_key(|account| account.name.to_lowercase());
        Ok(accounts)
    }

    /// The email Antigravity is signed in with now.
    #[must_use]
    pub fn active_email(&self) -> Option<String> {
        read_email(self.store.as_ref(), &self.settings.canonical)
    }

    /// Whether Antigravity is signed in as the saved account `name` right now (same email).
    #[must_use]
    pub fn is_active(&self, name: &str) -> bool {
        let saved = read_email(self.store.as_ref(), &self.settings.slot(name));
        match (saved, self.active_email()) {
            (Some(saved), Some(active)) => saved.eq_ignore_ascii_case(&active),
            _ => false,
        }
    }

    /// Whether `name` is saved.
    pub fn is_saved(&self, name: &str) -> Result<bool, AccountsError> {
        if !is_valid_name(name) {
            return Err(AccountsError::InvalidName);
        }
        Ok(self.store.exists(&self.settings.slot(name))?)
    }

    fn require_saved(&self, name: &str) -> Result<(), AccountsError> {
        if self.is_saved(name)? { Ok(()) } else { Err(AccountsError::NotSaved) }
    }

    /// Signs Antigravity in as `name` without starting it; it uses this account next time it
    /// starts. Antigravity must be closed.
    pub fn sign_in_as(&self, name: &str) -> Result<(), AccountsError> {
        self.require_saved(name)?;
        if self.antigravity.is_running() {
            return Err(AccountsError::AntigravityOpen);
        }
        self.store.copy(&self.settings.slot(name), &self.settings.canonical)?;
        tracing::info!(account = name, "signed in");
        Ok(())
    }

    /// Signs Antigravity in as `name` and starts it. Antigravity must be closed first.
    pub fn switch_to(&self, name: &str) -> Result<(), AccountsError> {
        self.sign_in_as(name)?;
        self.antigravity.launch()?;
        Ok(())
    }

    /// Removes the saved account `name` and its desktop shortcut. Antigravity's current sign-in,
    /// projects and conversations are not touched.
    pub fn remove(&self, name: &str) -> Result<RemoveOutcome, AccountsError> {
        if !is_valid_name(name) {
            return Err(AccountsError::InvalidName);
        }
        self.store.delete(&self.settings.slot(name))?;
        tracing::info!(account = name, "removed");
        Ok(RemoveOutcome { shortcut_error: self.desktop.delete_account_shortcut(name).err() })
    }

    /// Creates (or replaces) the desktop shortcut that switches to `name`.
    pub fn create_shortcut(
        &self,
        name: &str,
    ) -> Result<Result<PathBuf, DesktopError>, AccountsError> {
        self.require_saved(name)?;
        Ok(self.desktop.create_account_shortcut(name))
    }

    /// Starts adding the account `name`: keeps the current sign-in aside, clears it, and starts
    /// Antigravity so the person can sign in. Antigravity must be closed first.
    pub fn begin_add(&self, name: &str) -> Result<AddSession, AccountsError> {
        if self.is_saved(name)? {
            return Err(AccountsError::AlreadySaved);
        }
        if self.antigravity.is_running() {
            return Err(AccountsError::AntigravityOpen);
        }
        AddSession::start(self.clone(), name)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::fs;
    use std::path::Path;
    use std::time::Duration;

    use antigravity::{FakeHost, HostSettings};
    use desktop::{DesktopSettings, FakeShell};
    use kernel::{FixedFolders, KnownFolders};
    use vault::{MemoryStore, Operation};

    use super::*;

    pub(crate) const CANONICAL: &str = "gemini:antigravity";

    pub(crate) fn sign_in(email: &str) -> Vec<u8> {
        format!(r#"{{"id_token_email":"{email}","token":{{"refresh_token":"r-{email}"}}}}"#)
            .into_bytes()
    }

    pub(crate) struct World {
        pub root: tempfile::TempDir,
        pub store: Arc<MemoryStore>,
        pub host: Arc<FakeHost>,
        pub shell: Arc<FakeShell>,
        pub accounts: Accounts,
    }

    impl World {
        pub(crate) fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let exe = root.path().join("LocalAppData\\Programs\\antigravity\\Antigravity.exe");
            fs::create_dir_all(exe.parent().unwrap()).unwrap();
            fs::write(&exe, b"x").unwrap();
            let folders: Arc<dyn KnownFolders> = Arc::new(FixedFolders::new(root.path()));
            let store = Arc::new(MemoryStore::new());
            let host = Arc::new(FakeHost::new());
            let shell = Arc::new(FakeShell::new());
            let quick = HostSettings {
                close_wait: Duration::from_millis(20),
                kill_wait: Duration::from_millis(20),
                poll: Duration::from_millis(1),
            };
            let antigravity = Antigravity::new(host.clone(), folders.clone(), quick);
            let desktop = Desktop::new(
                shell.clone(),
                folders,
                antigravity.clone(),
                root.path().join("app.exe"),
                DesktopSettings::default(),
            );
            let accounts =
                Accounts::new(store.clone(), antigravity, desktop, AccountSettings::default());
            Self { root, store, host, shell, accounts }
        }

        pub(crate) fn slot(name: &str) -> String {
            format!("antigravity-switcher:{name}")
        }

        pub(crate) fn save(&self, name: &str, email: &str) {
            self.store.insert(&Self::slot(name), &sign_in(email));
        }

        pub(crate) fn signed_in(&self) -> Option<Vec<u8>> {
            self.store.read_blob(CANONICAL).unwrap()
        }

        pub(crate) fn desktop_dir(&self) -> &Path {
            self.root.path()
        }
    }

    #[test]
    fn settings_build_credential_names() {
        let settings = AccountSettings::default();
        assert_eq!(settings.slot("work"), "antigravity-switcher:work");
        assert_eq!(settings.rollback(), "antigravity-switcher:setup-rollback");
        assert_eq!(settings.canonical, CANONICAL);
    }

    #[test]
    fn lists_saved_accounts_sorted_with_emails() {
        let world = World::new();
        world.save("zeta", "z@example.com");
        world.save("Alpha", "a@example.com");
        world.save("beta", "b@example.com");
        world.store.insert(&World::slot("broken"), b"not json");
        world.store.insert(&World::slot("setup-rollback"), &sign_in("old@example.com"));
        world.store.insert(&World::slot("has space"), &sign_in("x@example.com"));
        world.store.insert("other:thing", b"{}");

        let listed = world.accounts.list().unwrap();
        let names: Vec<_> = listed.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["Alpha", "beta", "broken", "zeta"]);
        assert_eq!(listed[0].email.as_deref(), Some("a@example.com"));
        assert_eq!(listed[2].email, None);
    }

    #[test]
    fn listing_reports_store_failures() {
        let world = World::new();
        world.store.fail(Operation::List, "antigravity-switcher:");
        assert_eq!(world.accounts.list().unwrap_err().code(), "vault.system");
    }

    #[test]
    fn the_active_email_comes_from_antigravitys_sign_in() {
        let world = World::new();
        assert_eq!(world.accounts.active_email(), None);
        world.store.insert(CANONICAL, &sign_in("me@example.com"));
        assert_eq!(world.accounts.active_email().as_deref(), Some("me@example.com"));
    }

    #[test]
    fn the_active_account_is_found_by_email() {
        let world = World::new();
        world.save("work", "Work@example.com");
        world.save("home", "home@example.com");
        assert!(!world.accounts.is_active("work"), "nobody signed in");
        world.store.insert(CANONICAL, &sign_in("work@example.com"));
        assert!(world.accounts.is_active("work"));
        assert!(!world.accounts.is_active("home"));
        assert!(!world.accounts.is_active("ghost"));
    }

    #[test]
    fn switching_copies_the_sign_in_and_starts_antigravity() {
        let world = World::new();
        world.save("work", "work@example.com");
        world.store.insert(CANONICAL, &sign_in("me@example.com"));

        world.accounts.switch_to("work").unwrap();
        assert_eq!(world.signed_in(), Some(sign_in("work@example.com")));
        assert_eq!(world.host.started().len(), 1);
        assert_eq!(world.accounts.active_email().as_deref(), Some("work@example.com"));
    }

    #[test]
    fn sign_in_without_starting_antigravity() {
        let world = World::new();
        world.save("work", "work@example.com");
        world.accounts.sign_in_as("work").unwrap();
        assert_eq!(world.signed_in(), Some(sign_in("work@example.com")));
        assert!(world.host.started().is_empty(), "Antigravity stays closed");
        assert_eq!(world.accounts.sign_in_as("ghost").unwrap_err(), AccountsError::NotSaved);
        world.accounts.antigravity().launch().unwrap();
        assert_eq!(world.accounts.sign_in_as("work").unwrap_err(), AccountsError::AntigravityOpen);
    }

    #[test]
    fn switching_checks_its_preconditions() {
        let world = World::new();
        assert_eq!(world.accounts.switch_to("bad name").unwrap_err(), AccountsError::InvalidName);
        assert_eq!(world.accounts.switch_to("ghost").unwrap_err(), AccountsError::NotSaved);

        world.save("work", "w@example.com");
        world.accounts.antigravity().launch().unwrap();
        assert_eq!(world.accounts.switch_to("work").unwrap_err(), AccountsError::AntigravityOpen);
        assert_eq!(world.signed_in(), None, "nothing changes while Antigravity is open");
    }

    #[test]
    fn switching_reports_a_missing_antigravity_after_signing_in() {
        let world = World::new();
        world.save("work", "w@example.com");
        fs::remove_dir_all(world.root.path().join("LocalAppData\\Programs")).unwrap();
        let error = world.accounts.switch_to("work").unwrap_err();
        assert_eq!(error.code(), "antigravity.not_installed");
        assert_eq!(world.signed_in(), Some(sign_in("w@example.com")));
    }

    #[test]
    fn switching_reports_copy_failures() {
        let world = World::new();
        world.save("work", "w@example.com");
        world.store.fail(Operation::Copy, CANONICAL);
        assert_eq!(world.accounts.switch_to("work").unwrap_err().code(), "vault.system");
        assert!(world.host.started().is_empty());
    }

    #[test]
    fn removing_deletes_the_slot_and_the_shortcut_only() {
        let world = World::new();
        world.save("work", "w@example.com");
        world.store.insert(CANONICAL, &sign_in("w@example.com"));
        let shortcut = world.accounts.create_shortcut("work").unwrap().unwrap();
        assert!(shortcut.starts_with(world.desktop_dir()));

        let outcome = world.accounts.remove("work").unwrap();
        assert_eq!(outcome, RemoveOutcome { shortcut_error: None });
        assert!(!world.accounts.is_saved("work").unwrap());
        assert!(!shortcut.exists());
        assert_eq!(
            world.signed_in(),
            Some(sign_in("w@example.com")),
            "Antigravity stays signed in"
        );
    }

    #[test]
    fn removing_reports_problems() {
        let world = World::new();
        assert_eq!(world.accounts.remove("").unwrap_err(), AccountsError::InvalidName);
        world.save("work", "w@example.com");
        world.store.fail(Operation::Delete, &World::slot("work"));
        assert_eq!(world.accounts.remove("work").unwrap_err().code(), "vault.system");

        world.store.heal();
        let shortcut = world.accounts.desktop().account_shortcut_path("work").unwrap();
        fs::create_dir_all(&shortcut).unwrap();
        let outcome = world.accounts.remove("work").unwrap();
        assert_eq!(outcome.shortcut_error.unwrap().code(), "desktop.file");
        assert!(!world.accounts.is_saved("work").unwrap());
    }

    #[test]
    fn shortcuts_are_only_for_saved_accounts() {
        let world = World::new();
        assert_eq!(world.accounts.create_shortcut("ghost").unwrap_err(), AccountsError::NotSaved);
        world.save("work", "w@example.com");
        world.shell.fail();
        let failed = world.accounts.create_shortcut("work").unwrap();
        assert_eq!(failed.unwrap_err().code(), "desktop.shortcut");
    }

    #[test]
    fn accessors_expose_the_parts() {
        let world = World::new();
        assert!(!world.accounts.antigravity().is_running());
        assert_eq!(world.accounts.settings(), &AccountSettings::default());
        assert!(!world.accounts.store().exists(CANONICAL).unwrap());
        assert!(!world.accounts.desktop().is_installed());
    }
}
