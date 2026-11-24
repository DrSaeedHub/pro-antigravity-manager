//! A string that must never appear in logs or debug output (tokens, client secrets).

use std::fmt;

/// A secret text value. `Debug` and `Display` never show the value.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Secret(String);

impl Secret {
    /// Wraps a secret value.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The value itself. Call only where the value is sent to its owner (for example an HTTP body).
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Secret(***)")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("***")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_never_print_their_value() {
        let secret = Secret::new("ya29.very-secret");
        assert_eq!(format!("{secret:?}"), "Secret(***)");
        assert_eq!(format!("{secret}"), "***");
        assert!(!format!("{secret:#?}").contains("ya29"));
    }

    #[test]
    fn expose_returns_the_value() {
        assert_eq!(Secret::new(String::from("abc")).expose(), "abc");
        assert_eq!(Secret::new("x"), Secret::new("x"));
    }
}
