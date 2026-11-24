//! Windows adapter: DPAPI encryption for the current user.
#![allow(unsafe_code, reason = "Win32 calls")]

use windows::Win32::Foundation::{HLOCAL, LocalFree};
use windows::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
};
use windows::core::PCWSTR;

use crate::secret_box::{BoxError, SecretBox};

/// DPAPI for the current Windows user (`CryptProtectData`), compatible with .NET's
/// `ProtectedData` with `CurrentUser` scope.
#[derive(Debug, Default, Clone, Copy)]
pub struct Dpapi;

fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(data.len()).unwrap_or(u32::MAX),
        pbData: data.as_ptr().cast_mut(),
    }
}

fn run_dpapi(data: &[u8], entropy: &[u8], seal: bool) -> Result<Vec<u8>, BoxError> {
    let error = BoxError { action: if seal { "sealed" } else { "opened" } };
    let input = blob(data);
    let salt = blob(entropy);
    let salt = (!entropy.is_empty()).then_some(&raw const salt);
    let mut output = CRYPT_INTEGER_BLOB::default();
    // SAFETY: the input blobs point to live slices that DPAPI only reads; on success the output
    // buffer is allocated by Windows, copied, and freed once with LocalFree.
    unsafe {
        let result = if seal {
            CryptProtectData(
                &raw const input,
                PCWSTR::null(),
                salt,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &raw mut output,
            )
        } else {
            CryptUnprotectData(
                &raw const input,
                None,
                salt,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &raw mut output,
            )
        };
        result.map_err(|_| error.clone())?;
        let bytes = if output.pbData.is_null() {
            Vec::new()
        } else {
            std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec()
        };
        let _ = LocalFree(Some(HLOCAL(output.pbData.cast())));
        Ok(bytes)
    }
}

impl SecretBox for Dpapi {
    fn seal(&self, data: &[u8], entropy: &[u8]) -> Result<Vec<u8>, BoxError> {
        run_dpapi(data, entropy, true)
    }

    fn open(&self, sealed: &[u8], entropy: &[u8]) -> Result<Vec<u8>, BoxError> {
        run_dpapi(sealed, entropy, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpapi_round_trips_and_needs_the_same_entropy() {
        let sealed = Dpapi.seal(b"id\nsecret", b"antigravity-switcher-oauth-client").unwrap();
        assert_ne!(sealed, b"id\nsecret");
        let opened = Dpapi.open(&sealed, b"antigravity-switcher-oauth-client").unwrap();
        assert_eq!(opened, b"id\nsecret");
        assert_eq!(Dpapi.open(&sealed, b"other").unwrap_err().action, "opened");
        assert_eq!(Dpapi.open(b"not dpapi", b"").unwrap_err().action, "opened");
        let no_salt = Dpapi.seal(b"x", b"").unwrap();
        assert_eq!(Dpapi.open(&no_salt, b"").unwrap(), b"x");
    }
}
