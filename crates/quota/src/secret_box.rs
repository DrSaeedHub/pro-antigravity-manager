//! Encrypting small secrets for the current Windows user (DPAPI) behind a port.

use std::fmt::Debug;

/// Sealing or opening failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the protected data could not be {action}")]
pub struct BoxError {
    /// `sealed` or `opened`.
    pub action: &'static str,
}

/// Encrypts data so only the same user (and the same `entropy`) can decrypt it.
pub trait SecretBox: Send + Sync + Debug {
    /// Encrypts `data`.
    fn seal(&self, data: &[u8], entropy: &[u8]) -> Result<Vec<u8>, BoxError>;
    /// Decrypts what [`SecretBox::seal`] produced.
    fn open(&self, sealed: &[u8], entropy: &[u8]) -> Result<Vec<u8>, BoxError>;
}

/// A reversible stand-in for tests: a marker, then the data XOR-ed with the entropy.
#[derive(Debug, Default, Clone, Copy)]
pub struct FakeBox;

const MARK: &[u8] = b"FAKEBOX:";

fn mix(data: &[u8], entropy: &[u8]) -> Vec<u8> {
    if entropy.is_empty() {
        return data.to_vec();
    }
    data.iter().zip(entropy.iter().cycle()).map(|(byte, key)| byte ^ key).collect()
}

impl SecretBox for FakeBox {
    fn seal(&self, data: &[u8], entropy: &[u8]) -> Result<Vec<u8>, BoxError> {
        Ok([MARK, &mix(data, entropy)].concat())
    }

    fn open(&self, sealed: &[u8], entropy: &[u8]) -> Result<Vec<u8>, BoxError> {
        sealed
            .strip_prefix(MARK)
            .map(|body| mix(body, entropy))
            .ok_or(BoxError { action: "opened" })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fake_round_trips_and_rejects_foreign_data() {
        let sealed = FakeBox.seal(b"id\nsecret", b"salt").unwrap();
        assert_ne!(&sealed[MARK.len()..], b"id\nsecret");
        assert_eq!(FakeBox.open(&sealed, b"salt").unwrap(), b"id\nsecret");
        assert_eq!(FakeBox.open(b"garbage", b"salt").unwrap_err().action, "opened");
        assert_eq!(FakeBox.open(&FakeBox.seal(b"x", b"").unwrap(), b"").unwrap(), b"x");
        assert_eq!(
            BoxError { action: "sealed" }.to_string(),
            "the protected data could not be sealed"
        );
    }
}
