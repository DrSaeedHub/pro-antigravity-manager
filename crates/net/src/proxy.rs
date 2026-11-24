//! Choosing a proxy for a URL the way Windows would (port and the pure parsing rules).

use std::fmt::Debug;

/// Finds the proxy for a URL.
pub trait ProxyResolver: Send + Sync + Debug {
    /// `http://host:port` of the proxy for `url`, or `None` to connect directly.
    fn proxy_for(&self, url: &str) -> Option<String>;
}

/// Always connects directly (tests, and machines without a proxy).
#[derive(Debug, Default, Clone, Copy)]
pub struct NoProxy;

impl ProxyResolver for NoProxy {
    fn proxy_for(&self, _url: &str) -> Option<String> {
        None
    }
}

/// Picks the proxy for `scheme` from a Windows proxy list such as `host:8080`,
/// `http=a:80;https=b:443`, or the `PROXY a:80; DIRECT` form a PAC script returns.
/// SOCKS entries are skipped.
#[must_use]
pub fn pick(list: &str, scheme: &str) -> Option<String> {
    let entries: Vec<&str> =
        list.split([';', ' ', '\t', '\n', '\r']).map(str::trim).filter(|e| !e.is_empty()).collect();
    let mut generic = None;
    let mut pac_style = entries.iter().peekable();
    while let Some(entry) = pac_style.next() {
        if (entry.eq_ignore_ascii_case("PROXY") || entry.eq_ignore_ascii_case("HTTPS"))
            && let Some(address) = pac_style.peek()
        {
            return Some(normalize(address));
        }
        if ["SOCKS", "SOCKS4", "SOCKS5"].iter().any(|kind| entry.eq_ignore_ascii_case(kind)) {
            pac_style.next();
            continue;
        }
        if entry.eq_ignore_ascii_case("DIRECT") && generic.is_none() {
            return None;
        }
        if let Some((kind, address)) = entry.split_once('=') {
            if kind.eq_ignore_ascii_case(scheme) && !address.is_empty() {
                return Some(normalize(address));
            }
        } else if is_address(entry) && generic.is_none() {
            generic = Some(normalize(entry));
        }
    }
    generic
}

fn is_address(entry: &str) -> bool {
    !entry.eq_ignore_ascii_case("PROXY")
        && !entry.eq_ignore_ascii_case("SOCKS")
        && !entry.eq_ignore_ascii_case("SOCKS5")
        && entry.contains(':')
}

fn normalize(address: &str) -> String {
    if address.contains("://") { address.to_owned() } else { format!("http://{address}") }
}

/// Whether `host` is excluded by a Windows bypass list (`<local>;*.corp;10.*`).
#[must_use]
pub fn bypassed(list: &str, host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    list.split([';', ' ']).map(str::trim).filter(|e| !e.is_empty()).any(|entry| {
        let entry = entry.to_ascii_lowercase();
        if entry == "<local>" {
            return !host.contains('.');
        }
        wildcard(&entry, &host)
    })
}

/// `*` matches any run of characters.
fn wildcard(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    let [first, middle @ .., last] = parts.as_slice() else { return pattern == text };
    if parts.len() == 1 {
        return pattern == text;
    }
    let Some(mut rest) = text.strip_prefix(first) else { return false };
    for part in middle {
        let Some(found) = rest.find(part) else { return false };
        rest = &rest[found + part.len()..];
    }
    rest.ends_with(last)
}

/// The host part of an absolute URL.
#[must_use]
pub fn host_of(url: &str) -> Option<&str> {
    let after_scheme = url.split_once("://")?.1;
    let authority = after_scheme.split(['/', '?', '#']).next()?;
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if host.starts_with('[') {
        host.split_once(']').map_or(host, |(h, _)| &h[1..])
    } else {
        host.split(':').next().unwrap_or(host)
    };
    (!host.is_empty()).then_some(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_from_the_common_list_forms() {
        assert_eq!(pick("127.0.0.1:10809", "https").as_deref(), Some("http://127.0.0.1:10809"));
        assert_eq!(pick("http=a:80;https=b:443", "https").as_deref(), Some("http://b:443"));
        assert_eq!(pick("http=a:80;https=b:443", "http").as_deref(), Some("http://a:80"));
        assert_eq!(pick("http=a:80", "https"), None);
        assert_eq!(pick("socks=s:1080", "https"), None);
        assert_eq!(pick("HTTPS=b:443; c:8080", "https").as_deref(), Some("http://b:443"));
        assert_eq!(pick("c:8080;https=b:443", "https").as_deref(), Some("http://b:443"));
        assert_eq!(pick("", "https"), None);
        assert_eq!(pick("https=", "https"), None);
    }

    #[test]
    fn picks_from_pac_answers() {
        assert_eq!(pick("PROXY p:3128; DIRECT", "https").as_deref(), Some("http://p:3128"));
        assert_eq!(pick("HTTPS p:443", "https").as_deref(), Some("http://p:443"));
        assert_eq!(pick("DIRECT", "https"), None);
        assert_eq!(pick("SOCKS s:1080", "https"), None);
        assert_eq!(pick("PROXY", "https"), None);
        assert_eq!(pick("http://p:1", "https").as_deref(), Some("http://p:1"));
    }

    #[test]
    fn bypass_lists_match_local_and_wildcards() {
        assert!(bypassed("<local>", "intranet"));
        assert!(!bypassed("<local>", "oauth2.googleapis.com"));
        assert!(bypassed("*.googleapis.com", "oauth2.googleapis.com"));
        assert!(bypassed("10.*;*.corp", "10.0.0.1"));
        assert!(bypassed("OAUTH2.googleapis.com", "oauth2.googleapis.com"));
        assert!(!bypassed("*.corp", "google.com"));
        assert!(bypassed("a*b*c", "a-x-b-y-c"));
        assert!(!bypassed("a*b*c", "a-x-c"));
        assert!(!bypassed("", "anything"));
    }

    #[test]
    fn hosts_come_out_of_urls() {
        assert_eq!(host_of("https://oauth2.googleapis.com/token"), Some("oauth2.googleapis.com"));
        assert_eq!(host_of("http://127.0.0.1:8080/x?y"), Some("127.0.0.1"));
        assert_eq!(host_of("https://user:pw@h.example/x"), Some("h.example"));
        assert_eq!(host_of("http://[::1]:80/"), Some("::1"));
        assert_eq!(host_of("no-scheme"), None);
        assert_eq!(host_of("https:///path"), None);
    }

    #[test]
    fn no_proxy_is_always_direct() {
        assert_eq!(NoProxy.proxy_for("https://example.com"), None);
    }
}
