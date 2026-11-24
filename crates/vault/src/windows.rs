//! The real store: Windows Credential Manager generic credentials (`CredReadW` and friends).
#![allow(unsafe_code, reason = "Win32 calls")]

use std::ptr;

use windows::Win32::Foundation::ERROR_NOT_FOUND;
use windows::Win32::Security::Credentials::{
    CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredEnumerateW, CredFree, CredReadW, CredWriteW,
};
use windows::core::{PCWSTR, PWSTR};

use crate::store::{CredentialStore, VaultError};

/// Windows Credential Manager. A scoped store refuses every name outside its prefix, so tests can
/// never touch real credentials.
#[derive(Debug, Clone, Default)]
pub struct WindowsStore {
    scope: Option<String>,
}

impl WindowsStore {
    /// The store the app uses: every generic credential of the signed-in user.
    #[must_use]
    pub fn new() -> Self {
        Self { scope: None }
    }

    /// A store limited to names starting with `prefix` (case-insensitive).
    #[must_use]
    pub fn scoped(prefix: impl Into<String>) -> Self {
        Self { scope: Some(prefix.into().to_lowercase()) }
    }

    fn check(&self, target: &str) -> Result<(), VaultError> {
        match &self.scope {
            Some(scope) if !target.to_lowercase().starts_with(scope) => {
                Err(VaultError::OutOfScope { target: target.to_owned() })
            }
            _ => Ok(()),
        }
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

fn is_not_found(error: &windows::core::Error) -> bool {
    error.code() == ERROR_NOT_FOUND.to_hresult()
}

fn system(target: &str, error: &windows::core::Error) -> VaultError {
    #[allow(clippy::cast_sign_loss, reason = "an HRESULT is reported as its bit pattern")]
    let code = error.code().0 as u32;
    VaultError::System { target: target.to_owned(), code }
}

/// A credential returned by Windows, freed with `CredFree` when dropped.
struct Owned(*mut CREDENTIALW);

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: the pointer came from CredReadW and is freed exactly once, here.
        unsafe { CredFree(self.0.cast_const().cast()) };
    }
}

fn read_raw(target: &str) -> Result<Option<Owned>, VaultError> {
    let name = wide(target);
    let mut raw: *mut CREDENTIALW = ptr::null_mut();
    // SAFETY: `name` is NUL-terminated and outlives the call; Windows writes `raw` on success.
    match unsafe { CredReadW(PCWSTR(name.as_ptr()), CRED_TYPE_GENERIC, None, &raw mut raw) } {
        Ok(()) => Ok(Some(Owned(raw))),
        Err(error) if is_not_found(&error) => Ok(None),
        Err(error) => Err(system(target, &error)),
    }
}

impl CredentialStore for WindowsStore {
    fn exists(&self, target: &str) -> Result<bool, VaultError> {
        self.check(target)?;
        Ok(read_raw(target)?.is_some())
    }

    fn read_blob(&self, target: &str) -> Result<Option<Vec<u8>>, VaultError> {
        self.check(target)?;
        let Some(owned) = read_raw(target)? else { return Ok(None) };
        // SAFETY: `owned.0` points to a valid CREDENTIALW until `owned` drops; the blob pointer
        // and size describe memory owned by that same allocation.
        let blob = unsafe {
            let credential = &*owned.0;
            if credential.CredentialBlob.is_null() || credential.CredentialBlobSize == 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(
                    credential.CredentialBlob,
                    credential.CredentialBlobSize as usize,
                )
                .to_vec()
            }
        };
        Ok(Some(blob))
    }

    fn copy(&self, source: &str, destination: &str) -> Result<(), VaultError> {
        self.check(source)?;
        self.check(destination)?;
        let owned =
            read_raw(source)?.ok_or_else(|| VaultError::NotFound { target: source.to_owned() })?;
        let mut name = wide(destination);
        // SAFETY: the copied struct borrows pointers from `owned` (alive until the end of this
        // function) and from `name` (alive too); CredWriteW only reads them.
        unsafe {
            let mut credential = *owned.0;
            credential.TargetName = PWSTR(name.as_mut_ptr());
            CredWriteW(&raw const credential, 0).map_err(|error| system(destination, &error))
        }
    }

    fn delete(&self, target: &str) -> Result<(), VaultError> {
        self.check(target)?;
        let name = wide(target);
        // SAFETY: `name` is NUL-terminated and outlives the call.
        match unsafe { CredDeleteW(PCWSTR(name.as_ptr()), CRED_TYPE_GENERIC, None) } {
            Ok(()) => Ok(()),
            Err(error) if is_not_found(&error) => Ok(()),
            Err(error) => Err(system(target, &error)),
        }
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>, VaultError> {
        self.check(prefix)?;
        let filter = wide(&format!("{prefix}*"));
        let mut count = 0u32;
        let mut array: *mut *mut CREDENTIALW = ptr::null_mut();
        // SAFETY: `filter` is NUL-terminated; Windows fills `count` and `array` on success.
        let result = unsafe {
            CredEnumerateW(PCWSTR(filter.as_ptr()), None, &raw mut count, &raw mut array)
        };
        match result {
            Ok(()) => {}
            Err(error) if is_not_found(&error) => return Ok(Vec::new()),
            Err(error) => return Err(system(prefix, &error)),
        }
        let lower = prefix.to_lowercase();
        let mut names = Vec::with_capacity(count as usize);
        // SAFETY: `array` holds `count` valid credential pointers until CredFree below; each
        // TargetName is a NUL-terminated string inside that allocation.
        unsafe {
            for index in 0..count as usize {
                let credential = &**array.add(index);
                if let Ok(name) = credential.TargetName.to_string()
                    && name.to_lowercase().starts_with(&lower)
                {
                    names.push(name);
                }
            }
            CredFree(array.cast_const().cast());
        }
        Ok(names)
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use windows::Win32::Security::Credentials::CRED_PERSIST_LOCAL_MACHINE;

    use super::*;
    use crate::contract;

    /// Credential Manager can lose concurrent writes, so these tests take turns inside one
    /// process too (nextest runs them one at a time across processes, see `.config/nextest.toml`).
    static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn turn() -> std::sync::MutexGuard<'static, ()> {
        TURN.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A prefix no real credential can have.
    fn test_prefix() -> String {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        format!("aas-test-{}-{nanos}:", std::process::id())
    }

    /// Creates a generic credential directly (test seeding only).
    fn seed(target: &str, blob: &[u8], user_name: &str) {
        let mut name = wide(target);
        let mut user = wide(user_name);
        let mut comment = wide("Antigravity Account Switcher test");
        let mut bytes = blob.to_vec();
        let credential = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: PWSTR(name.as_mut_ptr()),
            Comment: PWSTR(comment.as_mut_ptr()),
            CredentialBlobSize: u32::try_from(bytes.len()).unwrap(),
            CredentialBlob: if bytes.is_empty() { ptr::null_mut() } else { bytes.as_mut_ptr() },
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            UserName: PWSTR(user.as_mut_ptr()),
            ..CREDENTIALW::default()
        };
        // SAFETY: every pointer refers to a local buffer that outlives the call.
        unsafe { CredWriteW(&raw const credential, 0) }.unwrap();
    }

    /// The user name and comment stored with `target`.
    fn fields(target: &str) -> (String, String) {
        let owned = read_raw(target).unwrap().unwrap();
        // SAFETY: valid until `owned` drops at the end of this function.
        unsafe {
            let credential = &*owned.0;
            (credential.UserName.to_string().unwrap(), credential.Comment.to_string().unwrap())
        }
    }

    #[test]
    fn passes_the_store_contract() {
        let _turn = turn();
        let prefix = test_prefix();
        let store = WindowsStore::scoped(&prefix);
        contract::run(&store, &prefix, &seed);
    }

    #[test]
    fn copies_keep_every_field() {
        let _turn = turn();
        let prefix = test_prefix();
        let store = WindowsStore::scoped(&prefix);
        let (a, b) = (format!("{prefix}a"), format!("{prefix}b"));
        seed(&a, b"{}", "person@example.com");
        store.copy(&a, &b).unwrap();
        assert_eq!(
            fields(&b),
            ("person@example.com".to_owned(), "Antigravity Account Switcher test".to_owned())
        );
        store.delete(&a).unwrap();
        store.delete(&b).unwrap();
    }

    #[test]
    fn a_scoped_store_refuses_real_names() {
        let store = WindowsStore::scoped(test_prefix());
        for target in ["gemini:antigravity", "antigravity-switcher:work"] {
            let refused = VaultError::OutOfScope { target: target.to_owned() };
            assert_eq!(store.exists(target).unwrap_err(), refused);
            assert_eq!(store.read_blob(target).unwrap_err(), refused);
            assert_eq!(store.delete(target).unwrap_err(), refused);
            assert_eq!(store.list(target).unwrap_err(), refused);
            assert_eq!(store.copy(target, target).unwrap_err(), refused);
        }
    }

    #[test]
    fn the_unscoped_store_accepts_any_name_and_reports_missing_ones() {
        let _turn = turn();
        // Reads a name nobody uses: proves the real store is wired without touching real data.
        let store = WindowsStore::new();
        let unused = format!("{}never-created", test_prefix());
        assert!(!store.exists(&unused).unwrap());
        assert!(store.list(&unused).unwrap().is_empty());
    }

    #[test]
    fn system_errors_carry_the_hresult() {
        let error = windows::core::Error::from_hresult(windows::core::HRESULT(-2_147_024_891));
        assert_eq!(
            system("x", &error),
            VaultError::System { target: "x".into(), code: 0x8007_0005 }
        );
        assert!(!is_not_found(&error));
    }
}
