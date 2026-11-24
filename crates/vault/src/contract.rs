//! The behavior every [`CredentialStore`] must have. Each implementation runs this suite.

use crate::store::{CredentialStore, VaultError};

/// Runs the contract with names under `prefix`; `seed` creates a credential directly.
/// Leaves nothing behind under `prefix`.
pub fn run(store: &dyn CredentialStore, prefix: &str, seed: &dyn Fn(&str, &[u8], &str)) {
    let a = format!("{prefix}alpha");
    let b = format!("{prefix}beta");
    let missing = format!("{prefix}missing");

    // Nothing there yet.
    assert!(!store.exists(&a).unwrap());
    assert_eq!(store.read_blob(&a).unwrap(), None);
    store.delete(&missing).unwrap();
    assert!(store.list(prefix).unwrap().is_empty());
    assert_eq!(
        store.copy(&missing, &b).unwrap_err(),
        VaultError::NotFound { target: missing.clone() }
    );

    // A seeded credential can be found, read and listed; names are case-insensitive.
    seed(&a, br#"{"id_token_email":"a@example.com"}"#, "user-a");
    assert!(store.exists(&a).unwrap());
    assert!(store.exists(&a.to_uppercase()).unwrap());
    assert_eq!(store.read_blob(&a).unwrap().unwrap(), br#"{"id_token_email":"a@example.com"}"#);
    assert_eq!(store.list(prefix).unwrap(), vec![a.clone()]);
    assert_eq!(store.list(&prefix.to_uppercase()).unwrap(), vec![a.clone()]);

    // Copy creates, then replaces.
    store.copy(&a, &b).unwrap();
    assert_eq!(store.read_blob(&b).unwrap(), store.read_blob(&a).unwrap());
    seed(&a, b"second", "user-a");
    store.copy(&a, &b).unwrap();
    assert_eq!(store.read_blob(&b).unwrap().unwrap(), b"second");
    let mut listed = store.list(prefix).unwrap();
    listed.sort();
    assert_eq!(listed, vec![a.clone(), b.clone()]);

    // An empty blob is still a credential.
    seed(&a, b"", "user-a");
    assert_eq!(store.read_blob(&a).unwrap(), Some(Vec::new()));

    // Delete removes one name only.
    store.delete(&a).unwrap();
    assert!(!store.exists(&a).unwrap());
    assert_eq!(store.list(prefix).unwrap(), vec![b.clone()]);
    store.delete(&b.to_uppercase()).unwrap();
    assert!(store.list(prefix).unwrap().is_empty());
}
