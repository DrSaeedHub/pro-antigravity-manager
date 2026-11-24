//! What can go wrong with saved accounts.

use antigravity::AntigravityError;
use vault::VaultError;

/// An account operation failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AccountsError {
    /// The name breaks the naming rule.
    #[error("the account name is not valid")]
    InvalidName,
    /// A saved account already has this name.
    #[error("an account with this name is already saved")]
    AlreadySaved,
    /// No saved account has this name.
    #[error("this account is not saved on this PC")]
    NotSaved,
    /// Antigravity must be closed first.
    #[error("Antigravity is still open")]
    AntigravityOpen,
    /// Saving was asked for before Antigravity finished signing in.
    #[error("Antigravity has not finished signing in")]
    NotSignedInYet,
    /// The copy could not be found after saving it.
    #[error("the account could not be verified after saving")]
    SaveNotVerified,
    /// Credential Manager failed.
    #[error(transparent)]
    Vault(#[from] VaultError),
    /// Closing or starting Antigravity failed.
    #[error(transparent)]
    Antigravity(#[from] AntigravityError),
}

impl AccountsError {
    /// A stable code for logs and the UI.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidName => "accounts.invalid_name",
            Self::AlreadySaved => "accounts.already_saved",
            Self::NotSaved => "accounts.not_saved",
            Self::AntigravityOpen => "accounts.antigravity_open",
            Self::NotSignedInYet => "accounts.not_signed_in_yet",
            Self::SaveNotVerified => "accounts.save_not_verified",
            Self::Vault(error) => error.code(),
            Self::Antigravity(error) => error.code(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_error_has_a_distinct_code() {
        let errors = [
            AccountsError::InvalidName,
            AccountsError::AlreadySaved,
            AccountsError::NotSaved,
            AccountsError::AntigravityOpen,
            AccountsError::NotSignedInYet,
            AccountsError::SaveNotVerified,
            AccountsError::Vault(VaultError::NotFound { target: "x".into() }),
            AccountsError::Antigravity(AntigravityError::NotInstalled),
        ];
        let mut codes: Vec<_> = errors.iter().map(AccountsError::code).collect();
        assert_eq!(codes[6], "vault.not_found");
        assert_eq!(codes[7], "antigravity.not_installed");
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), errors.len());
        for error in &errors {
            assert!(!error.to_string().is_empty());
        }
    }
}
