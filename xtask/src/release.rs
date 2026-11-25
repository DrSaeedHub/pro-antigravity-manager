//! The pure parts of `cargo xtask dist`: options, names, the checks, the release feed and notes.

use std::fmt::Write as _;

use serde_json::json;
use sha2::{Digest, Sha256};

/// The app's executable.
pub const EXECUTABLE: &str = "AntigravityAccountSwitcher.exe";
/// The platform key in `latest.json` (see `update::platform`).
pub const PLATFORM: &str = "windows-x64";

/// What `dist` was asked for.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// The tag being released (`v1.2.0`).
    pub tag: Option<String>,
    /// Skip the installer (no Inno Setup on this PC).
    pub portable_only: bool,
}

impl Options {
    /// Reads `--tag <tag>` and `--portable-only`.
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut options = Self::default();
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--tag" => {
                    let tag = args.next().ok_or("--tag needs a value, such as v1.2.0")?;
                    options.tag = Some(tag.clone());
                }
                "--portable-only" => options.portable_only = true,
                other => return Err(format!("unknown option {other}")),
            }
        }
        Ok(options)
    }
}

/// The release files' names.
#[derive(Debug, PartialEq, Eq)]
pub struct Names {
    /// The installer.
    pub setup: String,
    /// The portable zip (also what running copies update from).
    pub portable: String,
}

impl Names {
    /// The names for `version`.
    #[must_use]
    pub fn new(version: &str) -> Self {
        Self {
            setup: format!("AntigravityAccountSwitcher-{version}-setup.exe"),
            portable: format!("AntigravityAccountSwitcher-{version}-portable.zip"),
        }
    }
}

/// The tag for `version`: `tag` when it names exactly this version, else `v<version>`.
pub fn tag_for(version: &str, tag: Option<&str>) -> Result<String, String> {
    match tag {
        None => Ok(format!("v{version}")),
        Some(tag) if tag.strip_prefix('v') == Some(version) => Ok(tag.to_owned()),
        Some(tag) => Err(format!(
            "the tag {tag} does not match the version in Cargo.toml ({version}): tag v{version}, or change the version first"
        )),
    }
}

/// Fails when `repository` (from Cargo.toml) is not the GitHub repository `building`
/// (`owner/name`): running copies would look for updates in the wrong place.
pub fn check_repository(repository: &str, building: &str) -> Result<(), String> {
    let expected = format!("https://github.com/{building}");
    if repository.trim_end_matches('/').eq_ignore_ascii_case(&expected) {
        Ok(())
    } else {
        Err(format!(
            "Cargo.toml says repository = \"{repository}\" but this is {expected}: change `repository` in [workspace.package] so the app updates from here"
        ))
    }
}

/// `1.3.0` for `1.3.0-beta.1+build` (Windows version fields take numbers only).
#[must_use]
pub fn numeric(version: &str) -> &str {
    version.split(['-', '+']).next().unwrap_or(version)
}

/// Lowercase hex SHA-256.
#[must_use]
pub fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().fold(String::with_capacity(64), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// `latest.json`: the feed `crates/update` reads. `setup` is the installer, when there is one:
/// installed copies run it after updating themselves.
#[must_use]
pub fn manifest(
    version: &str,
    tag: &str,
    repository: &str,
    published: &str,
    names: &Names,
    zip: &[u8],
    setup: Option<&[u8]>,
) -> String {
    let base = repository.trim_end_matches('/');
    let mut value = json!({
        "schema": 1,
        "version": version,
        "published": published,
        "notes": format!("{base}/releases/tag/{tag}"),
        PLATFORM: {
            "url": format!("{base}/releases/download/{tag}/{}", names.portable),
            "sha256": sha256(zip),
            "size": zip.len(),
            "file": EXECUTABLE,
        }
    });
    if let Some(setup) = setup {
        value[PLATFORM]["installer"] = json!({
            "url": format!("{base}/releases/download/{tag}/{}", names.setup),
            "sha256": sha256(setup),
            "size": setup.len(),
            "file": names.setup,
        });
    }
    let mut text = serde_json::to_string_pretty(&value).unwrap_or_default();
    text.push('\n');
    text
}

/// The `## <version>` section of the CHANGELOG, without its heading.
#[must_use]
pub fn notes(changelog: &str, version: &str) -> Option<String> {
    let heading = format!("## {version}");
    let mut lines = changelog.lines().skip_while(|line| line.trim() != heading);
    lines.next()?;
    let body: Vec<&str> = lines.take_while(|line| !line.starts_with("## ")).collect();
    let text = body.join("\n").trim().to_owned();
    (!text.is_empty()).then_some(text)
}

/// The release page's text: what changed, then which file to take.
#[must_use]
pub fn notes_page(notes: &str, names: &Names, portable_only: bool) -> String {
    let mut page = format!("{notes}\n\n## Download\n\n");
    if !portable_only {
        let _ = writeln!(
            page,
            "- **Installer:** `{}`. Installs for your Windows user, no administrator rights; Start menu and desktop shortcuts, and an entry in Installed apps.",
            names.setup
        );
    }
    let _ = write!(
        page,
        "- **Portable:** `{}`. Unzip and run `{EXECUTABLE}` from anywhere.\n\n",
        names.portable
    );
    page.push_str(
        "A running copy finds this version within a minute and updates itself when you choose Update. Checksums are in `SHA256SUMS.txt`.\n",
    );
    page
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn options_are_read() {
        assert_eq!(Options::parse(&[]).unwrap(), Options::default());
        let options = Options::parse(&args(&["--tag", "v1.2.0", "--portable-only"])).unwrap();
        assert_eq!(options, Options { tag: Some("v1.2.0".into()), portable_only: true });
        assert!(Options::parse(&args(&["--tag"])).is_err());
        assert!(Options::parse(&args(&["--fast"])).is_err());
    }

    #[test]
    fn the_tag_must_be_the_version() {
        assert_eq!(tag_for("1.2.0", None).unwrap(), "v1.2.0");
        assert_eq!(tag_for("1.2.0", Some("v1.2.0")).unwrap(), "v1.2.0");
        assert_eq!(tag_for("1.3.0-beta.1", Some("v1.3.0-beta.1")).unwrap(), "v1.3.0-beta.1");
        assert!(tag_for("1.2.0", Some("v1.2.1")).unwrap_err().contains("does not match"));
        assert!(tag_for("1.2.0", Some("1.2.0")).is_err());
    }

    #[test]
    fn the_repository_must_be_the_one_being_built() {
        let ours = "https://github.com/DrSaeedHub/pro-antigravity-manager";
        assert!(check_repository(ours, "DrSaeedHub/pro-antigravity-manager").is_ok());
        assert!(
            check_repository(&format!("{ours}/"), "drsaeedhub/Pro-Antigravity-Manager").is_ok()
        );
        let error = check_repository(ours, "someone/fork").unwrap_err();
        assert!(error.contains("https://github.com/someone/fork"), "{error}");
    }

    #[test]
    fn names_and_numbers() {
        let names = Names::new("1.2.0");
        assert_eq!(names.setup, "AntigravityAccountSwitcher-1.2.0-setup.exe");
        assert_eq!(names.portable, "AntigravityAccountSwitcher-1.2.0-portable.zip");
        assert_eq!(numeric("1.3.0-beta.1+7"), "1.3.0");
        assert_eq!(numeric("1.2.0"), "1.2.0");
        assert_eq!(
            sha256(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn the_feed_points_at_the_tagged_zip() {
        let names = Names::new("1.2.0");
        let text = manifest(
            "1.2.0",
            "v1.2.0",
            "https://github.com/o/r/",
            "2026-11-25T10:00:00Z",
            &names,
            b"zip",
            Some(b"setup"),
        );
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["schema"], 1);
        assert_eq!(value["version"], "1.2.0");
        assert_eq!(value["notes"], "https://github.com/o/r/releases/tag/v1.2.0");
        let entry = &value[PLATFORM];
        assert_eq!(
            entry["url"],
            "https://github.com/o/r/releases/download/v1.2.0/AntigravityAccountSwitcher-1.2.0-portable.zip"
        );
        assert_eq!(entry["sha256"], sha256(b"zip"));
        let installer = &entry["installer"];
        assert_eq!(
            installer["url"],
            "https://github.com/o/r/releases/download/v1.2.0/AntigravityAccountSwitcher-1.2.0-setup.exe"
        );
        assert_eq!(installer["sha256"], sha256(b"setup"));
        assert_eq!(installer["size"], 5);
        assert_eq!(installer["file"], "AntigravityAccountSwitcher-1.2.0-setup.exe");
        let portable_only =
            manifest("1.2.0", "v1.2.0", "https://github.com/o/r", "x", &names, b"z", None);
        let value: serde_json::Value = serde_json::from_str(&portable_only).unwrap();
        assert!(value[PLATFORM].get("installer").is_none());
        assert_eq!(entry["size"], 3);
        assert_eq!(entry["file"], EXECUTABLE);
    }

    #[test]
    fn notes_come_from_the_changelog_section() {
        let changelog = "# Changelog\n\n## Unreleased\n\n- next\n\n## 1.2.0\n\n- one\n- two\n\n## 1.1.0\n\n- old\n";
        assert_eq!(notes(changelog, "1.2.0").unwrap(), "- one\n- two");
        assert_eq!(notes(changelog, "1.1.0").unwrap(), "- old");
        assert_eq!(notes(changelog, "9.9.9"), None);
        assert_eq!(notes("## 1.0.0\n\n## 0.9.0\n", "1.0.0"), None);
        let page = notes_page("- one", &Names::new("1.2.0"), false);
        assert!(page.starts_with("- one\n\n## Download"));
        assert!(page.contains("-setup.exe"));
        assert!(!notes_page("- one", &Names::new("1.2.0"), true).contains("-setup.exe"));
    }
}
