//! Downloading a package to a file while checking its size and SHA-256.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use sha2::{Digest, Sha256};

use crate::error::UpdateError;
use crate::feed::Package;

/// Lowercase hex of `bytes`.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
        out.push(char::from(b"0123456789abcdef"[usize::from(byte >> 4)]));
        out.push(char::from(b"0123456789abcdef"[usize::from(byte & 0x0f)]));
        out
    })
}

/// Downloads `package` into `to`. `progress(received, total)` follows along; setting `cancel`
/// stops it. On any failure `to` is removed, so a half download never stays behind.
pub async fn download(
    http: &reqwest::Client,
    package: &Package,
    to: &Path,
    progress: &(dyn Fn(u64, u64) + Sync),
    cancel: &AtomicBool,
) -> Result<(), UpdateError> {
    let result = fetch(http, package, to, progress, cancel).await;
    if result.is_err() {
        let _ = std::fs::remove_file(to);
    }
    result
}

async fn fetch(
    http: &reqwest::Client,
    package: &Package,
    to: &Path,
    progress: &(dyn Fn(u64, u64) + Sync),
    cancel: &AtomicBool,
) -> Result<(), UpdateError> {
    let file = File::create(to).map_err(|error| UpdateError::file("creating", to, &error))?;
    let mut file = BufWriter::new(file);
    let mut response = http
        .get(&package.url)
        .send()
        .await
        .map_err(|error| UpdateError::Network(net::describe(&error)))?;
    if !response.status().is_success() {
        return Err(UpdateError::Download(response.status().as_u16()));
    }
    let mut hasher = Sha256::new();
    let mut received = 0u64;
    progress(0, package.size);
    while let Some(chunk) =
        response.chunk().await.map_err(|error| UpdateError::Network(net::describe(&error)))?
    {
        if cancel.load(Ordering::Relaxed) {
            return Err(UpdateError::Canceled);
        }
        received += chunk.len() as u64;
        if received > package.size {
            return Err(UpdateError::Size { expected: package.size, got: received });
        }
        hasher.update(&chunk);
        file.write_all(&chunk).map_err(|error| UpdateError::file("writing", to, &error))?;
        progress(received, package.size);
    }
    file.flush().map_err(|error| UpdateError::file("writing", to, &error))?;
    drop(file);
    if received != package.size {
        return Err(UpdateError::Size { expected: package.size, got: received });
    }
    if hex(&hasher.finalize()) != package.sha256 {
        return Err(UpdateError::Checksum);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::time::Duration;

    use net::{HttpOptions, NoProxy};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    const BODY: &[u8] = b"pretend package bytes";

    fn sha(bytes: &[u8]) -> String {
        hex(&Sha256::digest(bytes))
    }

    async fn serve(body: &[u8]) -> (MockServer, Package) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/p.zip"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body.to_vec()))
            .mount(&server)
            .await;
        let package = Package {
            url: format!("{}/p.zip", server.uri()),
            sha256: sha(BODY),
            size: BODY.len() as u64,
            file: "App.exe".into(),
        };
        (server, package)
    }

    fn http() -> reqwest::Client {
        net::client(&HttpOptions::new("t", Duration::from_secs(10)), Arc::new(NoProxy)).unwrap()
    }

    #[test]
    fn hex_is_lowercase_and_padded() {
        assert_eq!(hex(&[0x00, 0x0f, 0xa0, 0xff]), "000fa0ff");
        assert_eq!(sha(b"abc").len(), 64);
    }

    #[tokio::test]
    async fn a_good_download_arrives_whole_with_progress() {
        let (_server, package) = serve(BODY).await;
        let dir = tempfile::tempdir().unwrap();
        let to = dir.path().join("p.zip");
        let seen = Mutex::new(Vec::new());
        let progress = |got, total| seen.lock().unwrap().push((got, total));
        download(&http(), &package, &to, &progress, &AtomicBool::new(false)).await.unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), BODY);
        let seen = seen.into_inner().unwrap();
        assert_eq!(seen.first(), Some(&(0, BODY.len() as u64)));
        assert_eq!(seen.last(), Some(&(BODY.len() as u64, BODY.len() as u64)));
    }

    #[tokio::test]
    async fn a_wrong_hash_or_size_keeps_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let to = dir.path().join("p.zip");
        let none = |_, _| {};
        let go = AtomicBool::new(false);

        let (_a, mut package) = serve(BODY).await;
        package.sha256 = sha(b"something else");
        let error = download(&http(), &package, &to, &none, &go).await.unwrap_err();
        assert_eq!(error, UpdateError::Checksum);
        assert!(!to.exists());

        let (_b, mut package) = serve(BODY).await;
        package.size += 5;
        let error = download(&http(), &package, &to, &none, &go).await.unwrap_err();
        assert_eq!(error, UpdateError::Size { expected: package.size, got: BODY.len() as u64 });
        assert!(!to.exists());

        let (_c, mut package) = serve(BODY).await;
        package.size -= 5;
        let error = download(&http(), &package, &to, &none, &go).await.unwrap_err();
        assert_eq!(error.code(), "update.size");
        assert!(!to.exists());
    }

    #[tokio::test]
    async fn http_and_network_failures_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let to = dir.path().join("p.zip");
        let none = |_, _| {};
        let go = AtomicBool::new(false);
        let (server, mut package) = serve(BODY).await;
        package.url = format!("{}/missing.zip", server.uri());
        let error = download(&http(), &package, &to, &none, &go).await.unwrap_err();
        assert_eq!(error, UpdateError::Download(404));
        package.url = "http://127.0.0.1:9/p.zip".into();
        let error = download(&http(), &package, &to, &none, &go).await.unwrap_err();
        assert_eq!(error.code(), "update.network");
        assert!(!to.exists());
    }

    #[tokio::test]
    async fn cancel_stops_the_download() {
        let (_server, package) = serve(BODY).await;
        let dir = tempfile::tempdir().unwrap();
        let to = dir.path().join("p.zip");
        let error =
            download(&http(), &package, &to, &|_, _| {}, &AtomicBool::new(true)).await.unwrap_err();
        assert_eq!(error, UpdateError::Canceled);
        assert!(!to.exists());
    }

    #[tokio::test]
    async fn a_missing_folder_is_reported_before_downloading() {
        let (_server, package) = serve(BODY).await;
        let dir = tempfile::tempdir().unwrap();
        let to = dir.path().join("no-such-folder").join("p.zip");
        let error = download(&http(), &package, &to, &|_, _| {}, &AtomicBool::new(false))
            .await
            .unwrap_err();
        assert_eq!(error.code(), "update.file");
    }
}
