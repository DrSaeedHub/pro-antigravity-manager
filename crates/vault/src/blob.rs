//! Reading the sign-in that Antigravity stores inside its credential (a JSON document).

use jiff::Timestamp;
use kernel::{Secret, json};
use serde_json::Value;

use crate::store::{CredentialStore, VaultError};

/// The text of a credential blob: UTF-16 LE when its second byte is 0, otherwise UTF-8.
/// Invalid sequences become U+FFFD. An empty blob has no text.
#[must_use]
pub fn decode_text(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }
    if bytes.len() >= 2 && bytes[1] == 0 {
        let units: Vec<u16> =
            bytes.as_chunks::<2>().0.iter().map(|pair| u16::from_le_bytes(*pair)).collect();
        Some(String::from_utf16_lossy(&units))
    } else {
        Some(String::from_utf8_lossy(bytes).into_owned())
    }
}

/// The saved sign-in could not be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SignInError {
    /// Reading the credential failed.
    #[error(transparent)]
    Vault(#[from] VaultError),
    /// The credential is empty or is not JSON.
    #[error("the saved sign-in could not be read")]
    Unreadable,
}

impl SignInError {
    /// A stable code for logs and the UI.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Vault(error) => error.code(),
            Self::Unreadable => "vault.unreadable",
        }
    }
}

/// What the app needs from a sign-in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SignIn {
    /// `id_token_email`.
    pub email: Option<String>,
    /// `token.refresh_token`.
    pub refresh_token: Option<Secret>,
    /// `token.access_token`.
    pub access_token: Option<Secret>,
    /// `token.expiry`.
    pub access_expiry: Option<Timestamp>,
}

impl SignIn {
    /// Parses the JSON text of a sign-in; `None` when it is not a JSON object.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let root: Value = serde_json::from_str(text).ok()?;
        if !root.is_object() {
            return None;
        }
        let token = json::get(&root, "token").cloned().unwrap_or(Value::Null);
        let secret = |key: &str| json::text(&token, key).filter(|v| !v.is_empty()).map(Secret::new);
        Some(Self {
            email: json::text(&root, "id_token_email").filter(|v| !v.is_empty()),
            refresh_token: secret("refresh_token"),
            access_token: secret("access_token"),
            access_expiry: json::time(&token, "expiry"),
        })
    }

    /// The sign-in stored under `target`; `None` when the credential does not exist.
    pub fn read(store: &dyn CredentialStore, target: &str) -> Result<Option<Self>, SignInError> {
        let Some(bytes) = store.read_blob(target)? else { return Ok(None) };
        let text = decode_text(&bytes).ok_or(SignInError::Unreadable)?;
        Self::parse(&text).map(Some).ok_or(SignInError::Unreadable)
    }
}

/// The email of the sign-in under `target`, or `None` for any problem.
#[must_use]
pub fn read_email(store: &dyn CredentialStore, target: &str) -> Option<String> {
    SignIn::read(store, target).ok().flatten().and_then(|sign_in| sign_in.email)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::memory::{MemoryStore, Operation};

    const SAMPLE: &str = r#"{
        "id_token_email": "ada@example.com",
        "token": {
            "access_token": "ya29.access",
            "refresh_token": "1//refresh",
            "expiry": "2026-11-23T10:00:00.123456789+01:00"
        }
    }"#;

    fn utf16(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    #[test]
    fn decodes_utf8_and_utf16() {
        assert_eq!(decode_text(b"{\"a\":1}").as_deref(), Some("{\"a\":1}"));
        assert_eq!(decode_text(&utf16("{\"a\":1}")).as_deref(), Some("{\"a\":1}"));
        assert_eq!(decode_text(b""), None);
        assert_eq!(decode_text(b"x").as_deref(), Some("x"));
        assert_eq!(decode_text(&[0xff, 0x41]).as_deref(), Some("\u{fffd}A"));
    }

    #[test]
    fn parses_a_full_sign_in() {
        let sign_in = SignIn::parse(SAMPLE).unwrap();
        assert_eq!(sign_in.email.as_deref(), Some("ada@example.com"));
        assert_eq!(sign_in.access_token.unwrap().expose(), "ya29.access");
        assert_eq!(sign_in.refresh_token.unwrap().expose(), "1//refresh");
        assert_eq!(sign_in.access_expiry, Some("2026-11-23T09:00:00.123456789Z".parse().unwrap()));
    }

    #[test]
    fn missing_or_empty_fields_are_none() {
        let sign_in =
            SignIn::parse(r#"{"id_token_email":"","token":{"refresh_token":""}}"#).unwrap();
        assert_eq!(sign_in, SignIn::default());
        assert_eq!(SignIn::parse("{}").unwrap(), SignIn::default());
        assert_eq!(SignIn::parse(r#"{"token":"text"}"#).unwrap(), SignIn::default());
    }

    #[test]
    fn non_objects_are_not_sign_ins() {
        assert_eq!(SignIn::parse("not json"), None);
        assert_eq!(SignIn::parse("[1,2]"), None);
        assert_eq!(SignIn::parse("\"text\""), None);
    }

    #[test]
    fn reads_from_a_store() {
        let store = MemoryStore::new();
        store.insert("good", SAMPLE.as_bytes());
        store.insert("wide", &utf16(SAMPLE));
        store.insert("empty", b"");
        store.insert("junk", b"<xml/>");

        assert_eq!(SignIn::read(&store, "missing").unwrap(), None);
        assert_eq!(read_email(&store, "good").as_deref(), Some("ada@example.com"));
        assert_eq!(read_email(&store, "wide").as_deref(), Some("ada@example.com"));
        assert_eq!(SignIn::read(&store, "empty").unwrap_err(), SignInError::Unreadable);
        assert_eq!(SignIn::read(&store, "junk").unwrap_err().code(), "vault.unreadable");
        assert_eq!(read_email(&store, "junk"), None);
        assert_eq!(read_email(&store, "missing"), None);

        store.fail(Operation::Read, "good");
        let error = SignIn::read(&store, "good").unwrap_err();
        assert_eq!(error.code(), "vault.system");
        assert_eq!(read_email(&store, "good"), None);
    }

    #[test]
    fn secrets_do_not_leak_through_debug() {
        let text = format!("{:?}", SignIn::parse(SAMPLE).unwrap());
        assert!(!text.contains("ya29") && !text.contains("1//refresh"), "{text}");
    }

    proptest! {
        #[test]
        fn decoding_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
            let _ = decode_text(&bytes);
        }

        #[test]
        fn utf16_round_trips(text in "[a-zA-Z0-9{}\":,@. ]{1,40}") {
            prop_assert_eq!(decode_text(&utf16(&text)), Some(text));
        }
    }
}
