//! Why quota could not be read.

/// Reading an account's quota failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QuotaError {
    /// The account is no longer saved.
    #[error("the account is not saved on this PC")]
    NotSaved,
    /// The saved sign-in is not readable JSON.
    #[error("the saved sign-in could not be read")]
    Unreadable,
    /// The saved sign-in has no refresh token.
    #[error("the saved sign-in has no refresh token")]
    NoRefreshToken,
    /// Google no longer accepts the sign-in (`invalid_grant`).
    #[error("the sign-in has expired")]
    SignInExpired,
    /// Google refused the token request for another reason.
    #[error("Google refused the sign-in: {description} ({code})")]
    OAuth {
        /// The OAuth error code.
        code: String,
        /// Google's description.
        description: String,
    },
    /// The sign-in client could not be found in the Antigravity install.
    #[error("Antigravity's sign-in client was not found in its install")]
    ClientNotFound,
    /// Google could not be reached.
    #[error("Google could not be reached: {0}")]
    Network(String),
    /// The quota API answered with an error.
    #[error("{method}: HTTP {status}{}", message.as_deref().map(|m| format!(" - {m}")).unwrap_or_default())]
    Api {
        /// The API method.
        method: String,
        /// The HTTP status.
        status: u16,
        /// Google's error message, when there is one.
        message: Option<String>,
    },
}

impl QuotaError {
    /// A stable code for logs and the UI.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NotSaved => "quota.not_saved",
            Self::Unreadable => "quota.unreadable",
            Self::NoRefreshToken => "quota.no_refresh_token",
            Self::SignInExpired => "quota.sign_in_expired",
            Self::OAuth { .. } => "quota.oauth",
            Self::ClientNotFound => "quota.client_not_found",
            Self::Network(_) => "quota.network",
            Self::Api { .. } => "quota.api",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_distinct_and_messages_carry_details() {
        let errors = [
            QuotaError::NotSaved,
            QuotaError::Unreadable,
            QuotaError::NoRefreshToken,
            QuotaError::SignInExpired,
            QuotaError::OAuth { code: "invalid_client".into(), description: "Unauthorized".into() },
            QuotaError::ClientNotFound,
            QuotaError::Network("timed out".into()),
            QuotaError::Api { method: "loadCodeAssist".into(), status: 500, message: None },
        ];
        let mut codes: Vec<_> = errors.iter().map(QuotaError::code).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), errors.len());
        assert_eq!(
            errors[4].to_string(),
            "Google refused the sign-in: Unauthorized (invalid_client)"
        );
        assert_eq!(errors[6].to_string(), "Google could not be reached: timed out");
        assert_eq!(errors[7].to_string(), "loadCodeAssist: HTTP 500");
        let with_message = QuotaError::Api {
            method: "fetchAvailableModels".into(),
            status: 403,
            message: Some("denied".into()),
        };
        assert_eq!(with_message.to_string(), "fetchAvailableModels: HTTP 403 - denied");
    }
}
