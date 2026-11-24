//! Windows adapters: DPAPI encryption and the system proxy (static, PAC file or auto-detect).
#![allow(unsafe_code, reason = "Win32 calls")]

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{GlobalFree, HGLOBAL, HLOCAL, LocalFree};
use windows::Win32::Networking::WinHttp::{
    WINHTTP_ACCESS_TYPE_NAMED_PROXY, WINHTTP_ACCESS_TYPE_NO_PROXY, WINHTTP_AUTO_DETECT_TYPE_DHCP,
    WINHTTP_AUTO_DETECT_TYPE_DNS_A, WINHTTP_AUTOPROXY_AUTO_DETECT, WINHTTP_AUTOPROXY_CONFIG_URL,
    WINHTTP_AUTOPROXY_OPTIONS, WINHTTP_CURRENT_USER_IE_PROXY_CONFIG, WINHTTP_PROXY_INFO,
    WinHttpCloseHandle, WinHttpGetIEProxyConfigForCurrentUser, WinHttpGetProxyForUrl, WinHttpOpen,
};
use windows::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
};
use windows::core::{BOOL, HSTRING, PCWSTR, PWSTR};

use crate::proxy::{ProxyResolver, bypassed, host_of, pick};
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

/// The proxy settings of the signed-in user.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProxyConfig {
    /// "Automatically detect settings".
    pub auto_detect: bool,
    /// "Use setup script" address.
    pub pac_url: Option<String>,
    /// The manual proxy list.
    pub proxy: Option<String>,
    /// The manual bypass list.
    pub bypass: Option<String>,
}

/// Takes ownership of a string WinHTTP allocated with `GlobalAlloc`.
fn take(text: PWSTR) -> Option<String> {
    if text.is_null() {
        return None;
    }
    // SAFETY: WinHTTP returns NUL-terminated strings it allocated with GlobalAlloc; we copy and
    // free each exactly once.
    unsafe {
        let value = text.to_string().ok();
        let _ = GlobalFree(Some(HGLOBAL(text.0.cast())));
        value.filter(|v| !v.is_empty())
    }
}

/// Reads the current user's proxy settings.
#[must_use]
pub fn current_config() -> ProxyConfig {
    let mut raw = WINHTTP_CURRENT_USER_IE_PROXY_CONFIG::default();
    // SAFETY: Windows fills the struct; `take` frees every string it allocated.
    if unsafe { WinHttpGetIEProxyConfigForCurrentUser(&raw mut raw) }.is_err() {
        return ProxyConfig::default();
    }
    ProxyConfig {
        auto_detect: raw.fAutoDetect.as_bool(),
        pac_url: take(raw.lpszAutoConfigUrl),
        proxy: take(raw.lpszProxy),
        bypass: take(raw.lpszProxyBypass),
    }
}

/// What automatic proxy discovery (PAC script or WPAD) decided.
enum AutoProxy {
    /// Not configured, or it failed: use the manual settings.
    Unavailable,
    /// Connect directly.
    Direct,
    /// Use this proxy list, with this bypass list.
    Use(String, Option<String>),
}

fn auto_proxy(config: &ProxyConfig, url: &str) -> AutoProxy {
    if !config.auto_detect && config.pac_url.is_none() {
        return AutoProxy::Unavailable;
    }
    let pac = config.pac_url.as_deref().map(HSTRING::from);
    let target = HSTRING::from(url);
    let mut options = WINHTTP_AUTOPROXY_OPTIONS {
        dwFlags: if pac.is_some() { WINHTTP_AUTOPROXY_CONFIG_URL } else { 0 }
            | if config.auto_detect { WINHTTP_AUTOPROXY_AUTO_DETECT } else { 0 },
        dwAutoDetectFlags: if config.auto_detect {
            WINHTTP_AUTO_DETECT_TYPE_DHCP | WINHTTP_AUTO_DETECT_TYPE_DNS_A
        } else {
            0
        },
        lpszAutoConfigUrl: pac.as_ref().map_or(PCWSTR::null(), |p| PCWSTR(p.as_ptr())),
        fAutoLogonIfChallenged: BOOL(1),
        ..WINHTTP_AUTOPROXY_OPTIONS::default()
    };
    let mut info = WINHTTP_PROXY_INFO::default();
    // SAFETY: the session handle is closed below; every string passed lives until the end of
    // this function; the returned strings are freed by `take`.
    unsafe {
        let session = WinHttpOpen(
            &HSTRING::from("antigravity-account-switcher"),
            WINHTTP_ACCESS_TYPE_NO_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        );
        if session.is_null() {
            return AutoProxy::Unavailable;
        }
        let result = WinHttpGetProxyForUrl(session, &target, &raw mut options, &raw mut info);
        let _ = WinHttpCloseHandle(session);
        if result.is_err() {
            return AutoProxy::Unavailable;
        }
        let list = take(info.lpszProxy);
        let bypass = take(info.lpszProxyBypass);
        match list {
            Some(list) if info.dwAccessType == WINHTTP_ACCESS_TYPE_NAMED_PROXY => {
                AutoProxy::Use(list, bypass)
            }
            _ => AutoProxy::Direct,
        }
    }
}

/// The proxy for `url` under `config`: automatic discovery first, then the manual list.
#[must_use]
pub fn resolve(config: &ProxyConfig, url: &str) -> Option<String> {
    let host = host_of(url)?;
    let scheme = if url.starts_with("http://") { "http" } else { "https" };
    let choose = |list: &str, bypass: Option<&str>| {
        if bypass.is_some_and(|b| bypassed(b, host)) { None } else { pick(list, scheme) }
    };
    match auto_proxy(config, url) {
        AutoProxy::Use(list, bypass) => choose(&list, bypass.as_deref()),
        AutoProxy::Direct => None,
        AutoProxy::Unavailable => {
            config.proxy.as_deref().and_then(|list| choose(list, config.bypass.as_deref()))
        }
    }
}

/// The current user's proxy, remembered per host for a few minutes.
#[derive(Debug, Default)]
pub struct SystemProxy {
    remembered: Mutex<HashMap<String, (Instant, Option<String>)>>,
}

/// How long a resolved proxy is reused.
const REMEMBER_FOR: Duration = Duration::from_secs(300);

impl SystemProxy {
    /// A resolver with an empty memory.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl ProxyResolver for SystemProxy {
    fn proxy_for(&self, url: &str) -> Option<String> {
        let host = host_of(url)?.to_ascii_lowercase();
        let mut remembered = self.remembered.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((when, proxy)) = remembered.get(&host)
            && when.elapsed() < REMEMBER_FOR
        {
            return proxy.clone();
        }
        let proxy = resolve(&current_config(), url);
        tracing::info!(%host, proxy = proxy.as_deref().unwrap_or("direct"), "proxy chosen");
        remembered.insert(host, (Instant::now(), proxy.clone()));
        proxy
    }
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

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

    #[test]
    fn manual_settings_apply_with_their_bypass_list() {
        let config = ProxyConfig {
            proxy: Some("http=web:80;https=secure:443".into()),
            bypass: Some("<local>;*.internal".into()),
            ..ProxyConfig::default()
        };
        assert_eq!(
            resolve(&config, "https://oauth2.googleapis.com/token").as_deref(),
            Some("http://secure:443")
        );
        assert_eq!(resolve(&config, "http://example.com/").as_deref(), Some("http://web:80"));
        assert_eq!(resolve(&config, "https://api.internal/x"), None);
        assert_eq!(resolve(&ProxyConfig::default(), "https://example.com"), None);
        assert_eq!(resolve(&config, "not a url"), None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_pac_script_is_evaluated_by_windows() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/proxy.pac"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(
                        "function FindProxyForURL(url, host) {\n\
                   if (shExpMatch(host, \"*.example.org\")) return \"DIRECT\";\n\
                   return \"PROXY 10.20.30.40:3128; DIRECT\";\n\
                 }",
                    )
                    .insert_header("content-type", "application/x-ns-proxy-autoconfig"),
            )
            .mount(&server)
            .await;
        let config = ProxyConfig {
            pac_url: Some(format!("{}/proxy.pac", server.uri())),
            proxy: Some("fallback:1".into()),
            ..ProxyConfig::default()
        };
        let (proxied, direct) = tokio::task::spawn_blocking(move || {
            (
                resolve(&config, "https://oauth2.googleapis.com/token"),
                resolve(&config, "https://a.example.org/"),
            )
        })
        .await
        .unwrap();
        assert_eq!(proxied.as_deref(), Some("http://10.20.30.40:3128"));
        assert_eq!(direct, None);
    }

    #[test]
    fn a_broken_pac_address_falls_back_to_the_manual_proxy() {
        let config = ProxyConfig {
            pac_url: Some("http://127.0.0.1:9/missing.pac".into()),
            proxy: Some("fallback:8080".into()),
            ..ProxyConfig::default()
        };
        assert_eq!(
            resolve(&config, "https://example.com/").as_deref(),
            Some("http://fallback:8080")
        );
    }

    #[test]
    fn the_system_resolver_reads_settings_and_remembers_hosts() {
        // Reads the real settings without changing them; the answer depends on this PC.
        let _ = current_config();
        let resolver = SystemProxy::new();
        let first = resolver.proxy_for("https://oauth2.googleapis.com/token");
        let second = resolver.proxy_for("https://OAUTH2.googleapis.com/other");
        assert_eq!(first, second);
        assert_eq!(resolver.proxy_for("garbage"), None);
    }
}
