//! The credential-store port.

use std::fmt::Debug;

/// A credential operation failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VaultError {
    /// The credential to read or copy does not exist.
    #[error("the credential {target} does not exist")]
    NotFound {
        /// The credential name.
        target: String,
    },
    /// Windows refused the operation.
    #[error("Windows Credential Manager refused the operation on {target} (code {code:#010x})")]
    System {
        /// The credential name.
        target: String,
        /// The Windows error code (HRESULT).
        code: u32,
    },
    /// A scoped store was asked to touch a name outside its scope (a test safety guard).
    #[error("{target} is outside this store's scope")]
    OutOfScope {
        /// The credential name.
        target: String,
    },
}

impl VaultError {
    /// A stable code for logs and the UI.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NotFound { .. } => "vault.not_found",
            Self::System { .. } => "vault.system",
            Self::OutOfScope { .. } => "vault.out_of_scope",
        }
    }
}

/// Generic credentials addressed by name. Names compare case-insensitively, like Windows does.
pub trait CredentialStore: Send + Sync + Debug {
    /// Whether `target` exists.
    fn exists(&self, target: &str) -> Result<bool, VaultError>;

    /// The secret bytes of `target`, or `None` when it does not exist.
    fn read_blob(&self, target: &str) -> Result<Option<Vec<u8>>, VaultError>;

    /// Writes an identical copy of `source` (every field) under the name `destination`,
    /// replacing an existing one.
    fn copy(&self, source: &str, destination: &str) -> Result<(), VaultError>;

    /// Deletes `target`. A missing credential is not an error.
    fn delete(&self, target: &str) -> Result<(), VaultError>;

    /// The names of every credential whose name starts with `prefix` (case-insensitive),
    /// as stored.
    fn list(&self, prefix: &str) -> Result<Vec<String>, VaultError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_have_stable_codes_and_messages() {
        let missing = VaultError::NotFound { target: "a".into() };
        let system = VaultError::System { target: "a".into(), code: 0x8007_0005 };
        let scope = VaultError::OutOfScope { target: "a".into() };
        assert_eq!(missing.code(), "vault.not_found");
        assert_eq!(system.code(), "vault.system");
        assert_eq!(scope.code(), "vault.out_of_scope");
        assert_eq!(missing.to_string(), "the credential a does not exist");
        assert_eq!(
            system.to_string(),
            "Windows Credential Manager refused the operation on a (code 0x80070005)"
        );
        assert_eq!(scope.to_string(), "a is outside this store's scope");
    }
}
