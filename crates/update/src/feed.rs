//! The release manifest (`latest.json`) and the rules for trusting what it points to.
//!
//! ```json
//! {
//!   "schema": 1,
//!   "version": "1.2.0",
//!   "published": "2026-11-25T10:00:00Z",
//!   "notes": "https://github.com/<owner>/<repo>/releases/tag/v1.2.0",
//!   "windows-x64": {
//!     "url": "https://github.com/<owner>/<repo>/releases/download/v1.2.0/<name>-1.2.0-portable.zip",
//!     "sha256": "<64 hex digits>",
//!     "size": 9111831,
//!     "file": "AntigravityAccountSwitcher.exe"
//!   }
//! }
//! ```

use reqwest::Url;
use semver::Version;
use serde_json::Value;

use crate::error::UpdateError;

/// The manifest format this version reads.
pub const SCHEMA: u64 = 1;
/// The largest package accepted.
pub const MAX_PACKAGE_BYTES: u64 = 200 * 1024 * 1024;

/// A published release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// Its version.
    pub version: Version,
    /// When it was published (RFC 3339), or empty.
    pub published: String,
    /// Its release-notes page, or empty when the manifest names none the app may open.
    pub notes: String,
    /// The package for this platform.
    pub package: Package,
}

/// The download for one platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    /// Where to download it.
    pub url: String,
    /// Its SHA-256, 64 lowercase hex digits.
    pub sha256: String,
    /// Its size in bytes.
    pub size: u64,
    /// The executable's file name inside the zip.
    pub file: String,
}

fn text<'a>(object: &'a Value, key: &str) -> Option<&'a str> {
    object.get(key).and_then(Value::as_str).map(str::trim).filter(|value| !value.is_empty())
}

fn bad(reason: &str) -> UpdateError {
    UpdateError::BadFeed(reason.to_owned())
}

/// Reads a manifest. `platform` picks the package (`windows-x64`); `repository` is the project's
/// address, which every package must live under.
pub fn parse(manifest: &str, platform: &str, repository: &str) -> Result<Release, UpdateError> {
    let root: Value = serde_json::from_str(manifest).map_err(|error| bad(&error.to_string()))?;
    match root.get("schema").and_then(Value::as_u64) {
        Some(SCHEMA) => {}
        Some(other) => {
            return Err(bad(&format!("schema {other} is newer than this version reads")));
        }
        None => return Err(bad("no schema")),
    }
    let version = text(&root, "version").ok_or_else(|| bad("no version"))?;
    let version = Version::parse(version.trim_start_matches('v'))
        .map_err(|error| bad(&format!("version {version}: {error}")))?;
    let entry = root.get(platform).filter(|entry| entry.is_object()).ok_or(UpdateError::NoBuild)?;
    let url = text(entry, "url").ok_or_else(|| bad("no package address"))?;
    if !trusted(url, repository) {
        return Err(UpdateError::Untrusted(url.to_owned()));
    }
    let sha256 = text(entry, "sha256").ok_or_else(|| bad("no checksum"))?.to_ascii_lowercase();
    if sha256.len() != 64 || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(bad("the checksum is not SHA-256"));
    }
    let size = entry.get("size").and_then(Value::as_u64).ok_or_else(|| bad("no size"))?;
    if size == 0 || size > MAX_PACKAGE_BYTES {
        return Err(bad(&format!("a package of {size} bytes")));
    }
    let file = text(entry, "file").ok_or_else(|| bad("no executable name"))?;
    if file.contains(['/', '\\', ':']) || file.starts_with('.') {
        return Err(bad("the executable name is not a plain file name"));
    }
    let notes = text(&root, "notes").filter(|notes| trusted(notes, repository)).unwrap_or_default();
    Ok(Release {
        version,
        published: text(&root, "published").unwrap_or_default().to_owned(),
        notes: notes.to_owned(),
        package: Package { url: url.to_owned(), sha256, size, file: file.to_owned() },
    })
}

/// Whether `url` lives under `repository`: same scheme, host and port, a path inside the
/// repository's, and HTTPS unless the repository itself is plain HTTP (a local test server).
#[must_use]
pub fn trusted(url: &str, repository: &str) -> bool {
    let (Ok(url), Ok(repository)) = (Url::parse(url), Url::parse(repository)) else {
        return false;
    };
    let base = repository.path().trim_end_matches('/');
    let inside = url.path().strip_prefix(base).is_some_and(|rest| rest.starts_with('/'));
    url.scheme() == repository.scheme()
        && matches!(url.scheme(), "https" | "http")
        && url.host_str().is_some()
        && url.host_str() == repository.host_str()
        && url.port_or_known_default() == repository.port_or_known_default()
        && url.username().is_empty()
        && inside
}

/// Whether `offered` should replace `current`: newer, and not a prerelease unless `current` is one.
#[must_use]
pub fn is_newer(current: &Version, offered: &Version) -> bool {
    offered > current && (offered.pre.is_empty() || !current.pre.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPO: &str = "https://github.com/owner/repo";

    fn manifest(entry: &str) -> String {
        format!(
            r#"{{"schema":1,"version":"v1.2.0","published":"2026-11-25T10:00:00Z",
               "notes":"{REPO}/releases/tag/v1.2.0","windows-x64":{entry}}}"#
        )
    }

    fn entry(url: &str) -> String {
        format!(
            r#"{{"url":"{url}","sha256":"{}","size":9111831,"file":"App.exe"}}"#,
            "AB".repeat(32)
        )
    }

    #[test]
    fn a_good_manifest_is_read() {
        let package = format!("{REPO}/releases/download/v1.2.0/app-1.2.0-portable.zip");
        let release = parse(&manifest(&entry(&package)), "windows-x64", REPO).unwrap();
        assert_eq!(release.version, Version::new(1, 2, 0));
        assert_eq!(release.published, "2026-11-25T10:00:00Z");
        assert_eq!(release.notes, format!("{REPO}/releases/tag/v1.2.0"));
        assert_eq!(release.package.url, package);
        assert_eq!(release.package.sha256, "ab".repeat(32));
        assert_eq!(release.package.size, 9_111_831);
        assert_eq!(release.package.file, "App.exe");
    }

    #[test]
    fn bad_manifests_are_refused() {
        let good = entry(&format!("{REPO}/releases/download/v1/x.zip"));
        let cases = [
            "not json".to_owned(),
            r#"{"version":"1.0.0"}"#.to_owned(),
            r#"{"schema":2,"version":"1.0.0"}"#.to_owned(),
            r#"{"schema":1}"#.to_owned(),
            r#"{"schema":1,"version":"one"}"#.to_owned(),
            manifest(r#"{"sha256":"x","size":1,"file":"a.exe"}"#),
            manifest(&good.replace(&"AB".repeat(32), "abc")),
            manifest(&good.replace(&"AB".repeat(32), &"zz".repeat(32))),
            manifest(&good.replace("9111831", "0")),
            manifest(&good.replace("9111831", "999999999999")),
            manifest(&good.replace(r#","size":9111831"#, "")),
            manifest(&good.replace("App.exe", "../App.exe")),
            manifest(&good.replace("App.exe", "C:App.exe")),
            manifest(&good.replace(r#","file":"App.exe""#, "")),
        ];
        for case in &cases {
            let error = parse(case, "windows-x64", REPO).unwrap_err();
            assert_eq!(error.code(), "update.bad_feed", "{case}: {error}");
        }
    }

    #[test]
    fn a_release_without_this_platform_has_no_build() {
        let text = manifest(&entry(&format!("{REPO}/releases/download/v1/x.zip")));
        assert_eq!(parse(&text, "linux-x64", REPO).unwrap_err(), UpdateError::NoBuild);
        let text = manifest(r#""not an object""#);
        assert_eq!(parse(&text, "windows-x64", REPO).unwrap_err(), UpdateError::NoBuild);
    }

    #[test]
    fn foreign_hosts_are_refused() {
        for url in [
            "https://github.com/someone-else/repo/releases/download/v1/x.zip",
            "https://github.com/owner/repository-evil/x.zip",
            "http://github.com/owner/repo/releases/download/v1/x.zip",
            "https://evil.example/owner/repo/x.zip",
            "https://user@github.com/owner/repo/x.zip",
            "https://github.com:8443/owner/repo/x.zip",
            "file:///C:/owner/repo/x.zip",
            "not a url",
        ] {
            let error = parse(&manifest(&entry(url)), "windows-x64", REPO).unwrap_err();
            assert_eq!(error, UpdateError::Untrusted(url.to_owned()), "{url}");
        }
    }

    #[test]
    fn untrusted_notes_are_dropped() {
        let text = manifest(&entry(&format!("{REPO}/releases/download/v1/x.zip")))
            .replace(&format!("{REPO}/releases/tag/v1.2.0"), "https://evil.example/notes");
        assert_eq!(parse(&text, "windows-x64", REPO).unwrap().notes, "");
    }

    #[test]
    fn a_local_test_repository_may_be_plain_http() {
        let repo = "http://127.0.0.1:8765/t/r";
        assert!(trusted("http://127.0.0.1:8765/t/r/releases/download/v1/x.zip", repo));
        assert!(!trusted("http://127.0.0.1:8766/t/r/releases/download/v1/x.zip", repo));
        assert!(!trusted("https://127.0.0.1:8765/t/r/x.zip", repo));
        assert!(!trusted("http://127.0.0.1:8765/t/r", repo), "the repository itself is no file");
        assert!(!trusted("ftp://127.0.0.1/t/r/x", "ftp://127.0.0.1/t/r"));
        assert!(!trusted("https://github.com/owner/repo/x", "not a url"));
    }

    #[test]
    fn only_newer_stable_versions_are_offered() {
        let v = |text: &str| Version::parse(text).unwrap();
        assert!(is_newer(&v("1.1.0"), &v("1.2.0")));
        assert!(is_newer(&v("1.1.0"), &v("1.1.1")));
        assert!(!is_newer(&v("1.2.0"), &v("1.2.0")));
        assert!(!is_newer(&v("1.2.0"), &v("1.1.9")));
        assert!(!is_newer(&v("1.1.0"), &v("1.2.0-beta.1")));
        assert!(is_newer(&v("1.2.0-beta.1"), &v("1.2.0-beta.2")));
        assert!(is_newer(&v("1.2.0-beta.1"), &v("1.2.0")));
    }
}
