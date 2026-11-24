//! Checking for, preparing and installing an update.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use net::{HttpOptions, ProxyResolver};
use semver::Version;

use crate::download::download;
use crate::error::UpdateError;
use crate::feed::{self, Release, is_newer};
use crate::package::extract;
use crate::probe::VersionProbe;
use crate::replace::{download_path, new_path, replace};

/// The platform this build is for, as the manifest names it.
#[must_use]
pub const fn platform() -> &'static str {
    if cfg!(all(windows, target_arch = "x86_64")) {
        "windows-x64"
    } else if cfg!(all(windows, target_arch = "aarch64")) {
        "windows-arm64"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "macos-arm64"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "macos-x64"
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "linux-x64"
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        "linux-arm64"
    } else {
        "unknown"
    }
}

/// Where releases come from and how long to wait for them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateSettings {
    /// The project's address, `https://github.com/<owner>/<repo>`.
    pub repository: String,
    /// The running version.
    pub current: Version,
    /// The manifest key of this build.
    pub platform: String,
    /// The `User-Agent` header.
    pub user_agent: String,
    /// The limit for reading the manifest.
    pub check_timeout: Duration,
    /// The limit for connecting.
    pub connect_timeout: Duration,
    /// The longest pause inside a download.
    pub read_timeout: Duration,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            repository: env!("CARGO_PKG_REPOSITORY").to_owned(),
            current: Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Version::new(0, 0, 0)),
            platform: platform().to_owned(),
            user_agent: format!("antigravity-account-switcher/{}", env!("CARGO_PKG_VERSION")),
            check_timeout: Duration::from_secs(20),
            connect_timeout: Duration::from_secs(15),
            read_timeout: Duration::from_secs(30),
        }
    }
}

impl UpdateSettings {
    fn base(&self) -> &str {
        self.repository.trim_end_matches('/')
    }

    /// The newest stable release's manifest.
    #[must_use]
    pub fn feed_url(&self) -> String {
        format!("{}/releases/latest/download/latest.json", self.base())
    }

    /// The page of the newest release, where people download it by hand.
    #[must_use]
    pub fn releases_page(&self) -> String {
        format!("{}/releases/latest", self.base())
    }

    /// The project's page.
    #[must_use]
    pub fn project_page(&self) -> String {
        self.base().to_owned()
    }
}

/// What a check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Check {
    /// Nothing newer (or nothing published yet).
    UpToDate,
    /// A newer release for this platform.
    Available(Release),
}

/// Checks the release feed and installs releases.
#[derive(Debug, Clone)]
pub struct Updater {
    http: reqwest::Client,
    settings: UpdateSettings,
    probe: Arc<dyn VersionProbe>,
}

impl Updater {
    /// An updater sending through `proxy`. Plain HTTP is refused unless the repository itself is
    /// a plain-HTTP address (a local test server).
    pub fn new(
        settings: UpdateSettings,
        proxy: Arc<dyn ProxyResolver>,
        probe: Arc<dyn VersionProbe>,
    ) -> Result<Self, UpdateError> {
        let options = HttpOptions {
            user_agent: settings.user_agent.clone(),
            timeout: None,
            connect_timeout: Some(settings.connect_timeout),
            read_timeout: Some(settings.read_timeout),
            https_only: settings.repository.starts_with("https://"),
        };
        let http = net::client(&options, proxy)
            .map_err(|error| UpdateError::Network(net::describe(&error)))?;
        Ok(Self { http, settings, probe })
    }

    /// The settings in use.
    #[must_use]
    pub fn settings(&self) -> &UpdateSettings {
        &self.settings
    }

    /// Reads the newest release's manifest. Nothing published yet counts as up to date.
    pub async fn check(&self) -> Result<Check, UpdateError> {
        let response = self
            .http
            .get(self.settings.feed_url())
            .timeout(self.settings.check_timeout)
            .send()
            .await
            .map_err(|error| UpdateError::Network(net::describe(&error)))?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(Check::UpToDate);
        }
        if !status.is_success() {
            return Err(UpdateError::Feed(status.as_u16()));
        }
        let text =
            response.text().await.map_err(|error| UpdateError::Network(net::describe(&error)))?;
        let release = feed::parse(&text, &self.settings.platform, &self.settings.repository)?;
        Ok(if is_newer(&self.settings.current, &release.version) {
            Check::Available(release)
        } else {
            Check::UpToDate
        })
    }

    /// Downloads `release` next to `exe`, checks it, takes the new executable out and makes sure
    /// it reports the offered version. Returns the new executable, ready for [`Self::install`].
    /// Nothing of the running copy changes here.
    pub async fn prepare(
        &self,
        release: &Release,
        exe: &Path,
        progress: &(dyn Fn(u64, u64) + Sync),
        cancel: &AtomicBool,
    ) -> Result<PathBuf, UpdateError> {
        if !feed::trusted(&release.package.url, &self.settings.repository) {
            return Err(UpdateError::Untrusted(release.package.url.clone()));
        }
        let archive = download_path(exe);
        let new = new_path(exe);
        download(&self.http, &release.package, &archive, progress, cancel).await?;
        let extracted = extract(&archive, &release.package.file, &new);
        let _ = std::fs::remove_file(&archive);
        extracted?;
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            let _ = std::fs::remove_file(&new);
            return Err(UpdateError::Canceled);
        }
        let probe = self.probe.clone();
        let probed = new.clone();
        let answer = tokio::task::spawn_blocking(move || probe.version(&probed))
            .await
            .map_err(|error| UpdateError::Probe(error.to_string()))
            .flatten();
        let offered = release.version.to_string();
        match answer {
            Ok(version) if version == offered => Ok(new),
            Ok(version) => {
                let _ = std::fs::remove_file(&new);
                Err(UpdateError::Probe(format!("it reports {version} instead of {offered}")))
            }
            Err(error) => {
                let _ = std::fs::remove_file(&new);
                Err(error)
            }
        }
    }

    /// Moves the prepared `new` executable into the name of the running `exe`.
    pub fn install(&self, exe: &Path, new: &Path) -> Result<PathBuf, UpdateError> {
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
        replace(exe, new, stamp)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use net::NoProxy;
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::download::hex;
    use crate::package::make_zip;
    use crate::probe::FakeProbe;

    struct Rig {
        server: MockServer,
        dir: tempfile::TempDir,
        exe: PathBuf,
        zip: Vec<u8>,
    }

    impl Rig {
        async fn new() -> Self {
            let server = MockServer::start().await;
            let dir = tempfile::tempdir().unwrap();
            let exe = dir.path().join("App.exe");
            std::fs::write(&exe, b"running build").unwrap();
            let archive = dir.path().join("made.zip");
            make_zip(&archive, &[("App.exe", b"new build"), ("LICENSE", b"MIT")]);
            let zip = std::fs::read(&archive).unwrap();
            std::fs::remove_file(&archive).unwrap();
            Self { server, dir, exe, zip }
        }

        fn repository(&self) -> String {
            format!("{}/owner/repo", self.server.uri())
        }

        fn updater(&self, current: &str, probe: FakeProbe) -> Updater {
            let settings = UpdateSettings {
                repository: self.repository(),
                current: Version::parse(current).unwrap(),
                ..UpdateSettings::default()
            };
            Updater::new(settings, Arc::new(NoProxy), Arc::new(probe)).unwrap()
        }

        fn manifest(&self, version: &str) -> serde_json::Value {
            json!({
                "schema": 1,
                "version": version,
                "published": "2026-11-25T10:00:00Z",
                "notes": format!("{}/releases/tag/v{version}", self.repository()),
                platform(): {
                    "url": format!("{}/releases/download/v{version}/app-portable.zip", self.repository()),
                    "sha256": hex(&Sha256::digest(&self.zip)),
                    "size": self.zip.len(),
                    "file": "App.exe",
                }
            })
        }

        async fn publish(&self, manifest: serde_json::Value) {
            Mock::given(method("GET"))
                .and(path("/owner/repo/releases/latest/download/latest.json"))
                .respond_with(ResponseTemplate::new(200).set_body_json(manifest))
                .mount(&self.server)
                .await;
            Mock::given(method("GET"))
                .and(path("/owner/repo/releases/download/v9.9.9/app-portable.zip"))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(self.zip.clone()))
                .mount(&self.server)
                .await;
        }

        async fn available(&self, updater: &Updater) -> Release {
            match updater.check().await.unwrap() {
                Check::Available(release) => release,
                Check::UpToDate => panic!("an update was expected"),
            }
        }
    }

    #[test]
    fn the_addresses_come_from_the_repository() {
        let settings = UpdateSettings {
            repository: "https://github.com/o/r/".into(),
            ..UpdateSettings::default()
        };
        assert_eq!(
            settings.feed_url(),
            "https://github.com/o/r/releases/latest/download/latest.json"
        );
        assert_eq!(settings.releases_page(), "https://github.com/o/r/releases/latest");
        assert_eq!(settings.project_page(), "https://github.com/o/r");
        let real = UpdateSettings::default();
        assert!(real.repository.starts_with("https://github.com/"), "{}", real.repository);
        assert_eq!(real.current.to_string(), env!("CARGO_PKG_VERSION"));
        assert_ne!(real.platform, "unknown");
    }

    #[tokio::test]
    async fn a_newer_release_is_offered() {
        let rig = Rig::new().await;
        rig.publish(rig.manifest("9.9.9")).await;
        let release = rig.available(&rig.updater("1.1.0", FakeProbe::answering("9.9.9"))).await;
        assert_eq!(release.version, Version::new(9, 9, 9));
        assert_eq!(release.notes, format!("{}/releases/tag/v9.9.9", rig.repository()));
    }

    #[tokio::test]
    async fn same_or_older_is_up_to_date() {
        let rig = Rig::new().await;
        rig.publish(rig.manifest("9.9.9")).await;
        let updater = rig.updater("9.9.9", FakeProbe::answering("9.9.9"));
        assert_eq!(updater.check().await.unwrap(), Check::UpToDate);
        let updater = rig.updater("10.0.0", FakeProbe::answering("9.9.9"));
        assert_eq!(updater.check().await.unwrap(), Check::UpToDate);
    }

    #[tokio::test]
    async fn no_release_yet_is_quiet() {
        let rig = Rig::new().await;
        let updater = rig.updater("1.1.0", FakeProbe::answering("9.9.9"));
        assert_eq!(updater.check().await.unwrap(), Check::UpToDate);
    }

    #[tokio::test]
    async fn feed_problems_are_reported() {
        let rig = Rig::new().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&rig.server)
            .await;
        let updater = rig.updater("1.1.0", FakeProbe::answering("9.9.9"));
        assert_eq!(updater.check().await.unwrap_err(), UpdateError::Feed(503));

        let settings = UpdateSettings {
            repository: "http://127.0.0.1:9/o/r".into(),
            ..UpdateSettings::default()
        };
        let offline =
            Updater::new(settings, Arc::new(NoProxy), Arc::new(FakeProbe::answering("x")));
        assert_eq!(offline.unwrap().check().await.unwrap_err().code(), "update.network");
    }

    #[tokio::test]
    async fn a_broken_manifest_is_reported() {
        let rig = Rig::new().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<html>oops</html>"))
            .mount(&rig.server)
            .await;
        let updater = rig.updater("1.1.0", FakeProbe::answering("9.9.9"));
        assert_eq!(updater.check().await.unwrap_err().code(), "update.bad_feed");
    }

    #[tokio::test]
    async fn a_release_is_prepared_and_installed() {
        let rig = Rig::new().await;
        rig.publish(rig.manifest("9.9.9")).await;
        let updater = rig.updater("1.1.0", FakeProbe::answering("9.9.9"));
        let release = rig.available(&updater).await;
        let seen = Mutex::new(0u64);
        let progress = |got, _| *seen.lock().unwrap() = got;
        let new =
            updater.prepare(&release, &rig.exe, &progress, &AtomicBool::new(false)).await.unwrap();
        assert_eq!(*seen.lock().unwrap(), rig.zip.len() as u64);
        assert_eq!(new, new_path(&rig.exe));
        assert_eq!(std::fs::read(&new).unwrap(), b"new build");
        assert!(!download_path(&rig.exe).exists(), "the package is removed");
        assert_eq!(std::fs::read(&rig.exe).unwrap(), b"running build", "nothing replaced yet");

        let old = updater.install(&rig.exe, &new).unwrap();
        assert_eq!(std::fs::read(&rig.exe).unwrap(), b"new build");
        assert_eq!(std::fs::read(old).unwrap(), b"running build");
        assert!(rig.dir.path().exists());
    }

    #[tokio::test]
    async fn a_wrong_version_answer_replaces_nothing() {
        let rig = Rig::new().await;
        rig.publish(rig.manifest("9.9.9")).await;
        for probe in [FakeProbe::answering("9.9.8"), FakeProbe::failing("crashed")] {
            let updater = rig.updater("1.1.0", probe);
            let release = rig.available(&updater).await;
            let error = updater
                .prepare(&release, &rig.exe, &|_, _| {}, &AtomicBool::new(false))
                .await
                .unwrap_err();
            assert_eq!(error.code(), "update.probe", "{error}");
            assert!(!new_path(&rig.exe).exists());
            assert_eq!(std::fs::read(&rig.exe).unwrap(), b"running build");
        }
    }

    #[tokio::test]
    async fn a_package_without_the_exe_replaces_nothing() {
        let rig = Rig::new().await;
        let mut manifest = rig.manifest("9.9.9");
        manifest[platform()]["file"] = json!("Other.exe");
        rig.publish(manifest).await;
        let updater = rig.updater("1.1.0", FakeProbe::answering("9.9.9"));
        let release = rig.available(&updater).await;
        let error = updater
            .prepare(&release, &rig.exe, &|_, _| {}, &AtomicBool::new(false))
            .await
            .unwrap_err();
        assert_eq!(error.code(), "update.package");
        assert!(!new_path(&rig.exe).exists());
        assert!(!download_path(&rig.exe).exists());
    }

    #[tokio::test]
    async fn a_canceled_or_untrusted_release_replaces_nothing() {
        let rig = Rig::new().await;
        rig.publish(rig.manifest("9.9.9")).await;
        let updater = rig.updater("1.1.0", FakeProbe::answering("9.9.9"));
        let mut release = rig.available(&updater).await;
        let error = updater
            .prepare(&release, &rig.exe, &|_, _| {}, &AtomicBool::new(true))
            .await
            .unwrap_err();
        assert_eq!(error, UpdateError::Canceled);
        release.package.url = "https://evil.example/owner/repo/x.zip".into();
        let error = updater
            .prepare(&release, &rig.exe, &|_, _| {}, &AtomicBool::new(false))
            .await
            .unwrap_err();
        assert_eq!(error.code(), "update.untrusted");
        assert_eq!(std::fs::read(&rig.exe).unwrap(), b"running build");
    }

    #[tokio::test]
    async fn a_github_repository_refuses_plain_http() {
        let settings = UpdateSettings {
            repository: "https://github.com/o/r".into(),
            ..UpdateSettings::default()
        };
        let updater =
            Updater::new(settings, Arc::new(NoProxy), Arc::new(FakeProbe::answering("x"))).unwrap();
        let error = updater.http.get("http://127.0.0.1:9/x").send().await.unwrap_err();
        assert!(!net::describe(&error).is_empty());
        assert_eq!(updater.settings().repository, "https://github.com/o/r");
    }
}
