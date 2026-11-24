//! An in-memory credential store for tests and previews, with optional injected failures.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::store::{CredentialStore, VaultError};

/// An operation a test can make fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    /// [`CredentialStore::exists`] and [`CredentialStore::read_blob`].
    Read,
    /// [`CredentialStore::copy`], matched on the destination name.
    Copy,
    /// [`CredentialStore::delete`].
    Delete,
    /// [`CredentialStore::list`], matched on the prefix.
    List,
}

#[derive(Debug, Clone)]
struct Entry {
    target: String,
    blob: Vec<u8>,
    user_name: String,
}

#[derive(Debug, Default)]
struct State {
    entries: BTreeMap<String, Entry>,
    failures: Vec<(Operation, String)>,
}

/// Credentials kept in memory. Names compare case-insensitively.
#[derive(Debug, Default)]
pub struct MemoryStore {
    state: Mutex<State>,
}

/// The Windows error a failure injection reports (`E_ACCESSDENIED`).
pub const INJECTED_FAILURE_CODE: u32 = 0x8007_0005;

impl MemoryStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Stores `blob` under `target`, replacing an existing credential.
    pub fn insert(&self, target: &str, blob: &[u8]) {
        self.insert_with_user(target, blob, "");
    }

    /// Stores `blob` under `target` with a user name, replacing an existing credential.
    pub fn insert_with_user(&self, target: &str, blob: &[u8], user_name: &str) {
        self.state().entries.insert(
            target.to_lowercase(),
            Entry {
                target: target.to_owned(),
                blob: blob.to_vec(),
                user_name: user_name.to_owned(),
            },
        );
    }

    /// The user name stored with `target`.
    #[must_use]
    pub fn user_name(&self, target: &str) -> Option<String> {
        self.state().entries.get(&target.to_lowercase()).map(|entry| entry.user_name.clone())
    }

    /// Makes `operation` fail for names (or prefixes) equal to `target`, case-insensitively.
    pub fn fail(&self, operation: Operation, target: &str) {
        self.state().failures.push((operation, target.to_lowercase()));
    }

    /// Removes every injected failure.
    pub fn heal(&self) {
        self.state().failures.clear();
    }

    /// Every stored name, as stored, in case-insensitive order.
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        self.state().entries.values().map(|entry| entry.target.clone()).collect()
    }

    fn check(state: &State, operation: Operation, target: &str) -> Result<(), VaultError> {
        let key = target.to_lowercase();
        if state.failures.iter().any(|(op, name)| *op == operation && *name == key) {
            Err(VaultError::System { target: target.to_owned(), code: INJECTED_FAILURE_CODE })
        } else {
            Ok(())
        }
    }
}

impl CredentialStore for MemoryStore {
    fn exists(&self, target: &str) -> Result<bool, VaultError> {
        let state = self.state();
        Self::check(&state, Operation::Read, target)?;
        Ok(state.entries.contains_key(&target.to_lowercase()))
    }

    fn read_blob(&self, target: &str) -> Result<Option<Vec<u8>>, VaultError> {
        let state = self.state();
        Self::check(&state, Operation::Read, target)?;
        Ok(state.entries.get(&target.to_lowercase()).map(|entry| entry.blob.clone()))
    }

    fn copy(&self, source: &str, destination: &str) -> Result<(), VaultError> {
        let mut state = self.state();
        Self::check(&state, Operation::Read, source)?;
        Self::check(&state, Operation::Copy, destination)?;
        let entry = state
            .entries
            .get(&source.to_lowercase())
            .cloned()
            .ok_or_else(|| VaultError::NotFound { target: source.to_owned() })?;
        state
            .entries
            .insert(destination.to_lowercase(), Entry { target: destination.to_owned(), ..entry });
        Ok(())
    }

    fn delete(&self, target: &str) -> Result<(), VaultError> {
        let mut state = self.state();
        Self::check(&state, Operation::Delete, target)?;
        state.entries.remove(&target.to_lowercase());
        Ok(())
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>, VaultError> {
        let state = self.state();
        Self::check(&state, Operation::List, prefix)?;
        let prefix = prefix.to_lowercase();
        Ok(state
            .entries
            .iter()
            .filter(|(key, _)| key.starts_with(&prefix))
            .map(|(_, entry)| entry.target.clone())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract;

    #[test]
    fn passes_the_store_contract() {
        let store = MemoryStore::new();
        contract::run(&store, "memory-contract:", &|target, blob, user| {
            store.insert_with_user(target, blob, user);
        });
        assert!(store.names().is_empty());
    }

    #[test]
    fn copies_keep_the_user_name() {
        let store = MemoryStore::new();
        store.insert_with_user("a", b"x", "someone");
        store.copy("a", "b").unwrap();
        assert_eq!(store.user_name("B").as_deref(), Some("someone"));
        assert_eq!(store.user_name("missing"), None);
    }

    #[test]
    fn injected_failures_hit_only_their_operation_and_name() {
        let store = MemoryStore::new();
        store.insert("a", b"x");
        store.fail(Operation::Copy, "B");
        let error = store.copy("a", "b").unwrap_err();
        assert_eq!(error, VaultError::System { target: "b".into(), code: INJECTED_FAILURE_CODE });
        store.copy("a", "c").unwrap();

        store.fail(Operation::Read, "a");
        assert!(store.exists("a").is_err());
        assert!(store.read_blob("a").is_err());
        assert!(store.copy("a", "d").is_err());

        store.fail(Operation::Delete, "c");
        assert!(store.delete("c").is_err());
        store.fail(Operation::List, "x");
        assert!(store.list("x").is_err());
        assert!(store.list("y").is_ok());

        store.heal();
        store.copy("a", "b").unwrap();
        store.delete("c").unwrap();
        assert!(store.exists("a").unwrap());
    }
}
