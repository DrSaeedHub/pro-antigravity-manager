//! Antigravity's "installed app" sign-in client: found by scanning its language server, cached
//! encrypted for the Windows user (`oauth-client.bin`, the same format the first app used).

use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use kernel::Secret;
use regex::bytes::Regex;

use crate::secret_box::SecretBox;

/// A sign-in client: its id and secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientCredentials {
    /// `….apps.googleusercontent.com`.
    pub id: String,
    /// `GOCSPX-…`.
    pub secret: Secret,
}

/// Every client id and secret found, in the order found, without repeats.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Discovered {
    /// Client ids.
    pub ids: Vec<String>,
    /// Client secrets.
    pub secrets: Vec<String>,
}

impl Discovered {
    /// Whether a usable pair can be formed.
    #[must_use]
    pub fn is_usable(&self) -> bool {
        !self.ids.is_empty() && !self.secrets.is_empty()
    }
}

static CLIENT_ID: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"[0-9]{6,15}-[a-z0-9]{32}\.apps\.googleusercontent\.com").ok());
static CLIENT_SECRET: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"GOCSPX-[A-Za-z0-9_\-]{28}").ok());

/// Bytes read at a time, and kept from the previous read so matches across reads are found.
const CHUNK: usize = 8 * 1024 * 1024;
const OVERLAP: usize = 256;

fn collect(pattern: &LazyLock<Option<Regex>>, text: &[u8], into: &mut Vec<String>) {
    let Some(pattern) = pattern.as_ref() else { return };
    for found in pattern.find_iter(text) {
        let value = String::from_utf8_lossy(found.as_bytes()).into_owned();
        if !into.contains(&value) {
            into.push(value);
        }
    }
}

/// Adds the client ids and secrets inside `reader` to `found`.
pub fn scan_reader(mut reader: impl Read, found: &mut Discovered) -> io::Result<()> {
    let mut buffer = vec![0u8; CHUNK + OVERLAP];
    let mut carried = 0;
    loop {
        let read = reader.read(&mut buffer[carried..])?;
        if read == 0 {
            return Ok(());
        }
        let total = carried + read;
        collect(&CLIENT_ID, &buffer[..total], &mut found.ids);
        collect(&CLIENT_SECRET, &buffer[..total], &mut found.secrets);
        carried = OVERLAP.min(total);
        buffer.copy_within(total - carried..total, 0);
    }
}

/// Scans every file; unreadable files are skipped.
#[must_use]
pub fn scan(files: &[PathBuf]) -> Discovered {
    let mut found = Discovered::default();
    for file in files {
        match File::open(file).and_then(|handle| scan_reader(handle, &mut found)) {
            Ok(()) => {}
            Err(error) => tracing::warn!(file = %file.display(), %error, "could not scan"),
        }
    }
    tracing::info!(ids = found.ids.len(), secrets = found.secrets.len(), "sign-in client scan");
    found
}

/// The encrypted cache file of the sign-in client.
#[derive(Debug, Clone)]
pub struct ClientCache {
    path: PathBuf,
    secret_box: Arc<dyn SecretBox>,
    entropy: Vec<u8>,
}

impl ClientCache {
    /// The cache at `path`, sealed with `secret_box` and `entropy`.
    #[must_use]
    pub fn new(path: PathBuf, secret_box: Arc<dyn SecretBox>, entropy: &[u8]) -> Self {
        Self { path, secret_box, entropy: entropy.to_vec() }
    }

    /// Where the cache lives.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The cached client, if the file exists and opens.
    #[must_use]
    pub fn load(&self) -> Option<ClientCredentials> {
        let sealed = std::fs::read(&self.path).ok()?;
        let plain = self.secret_box.open(&sealed, &self.entropy).ok()?;
        let text = String::from_utf8(plain).ok()?;
        let (id, secret) = text.split_once('\n')?;
        (!id.is_empty() && !secret.is_empty() && !secret.contains('\n'))
            .then(|| ClientCredentials { id: id.to_owned(), secret: Secret::new(secret) })
    }

    /// Stores `client`. Failures are logged, not raised: the cache only saves a scan.
    pub fn save(&self, client: &ClientCredentials) {
        let plain = format!("{}\n{}", client.id, client.secret.expose());
        let result = self
            .secret_box
            .seal(plain.as_bytes(), &self.entropy)
            .map_err(|error| error.to_string())
            .and_then(|sealed| {
                if let Some(folder) = self.path.parent() {
                    std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
                }
                std::fs::write(&self.path, sealed).map_err(|error| error.to_string())
            });
        if let Err(reason) = result {
            tracing::warn!(%reason, "the sign-in client could not be cached");
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::io::Cursor;

    use super::*;
    use crate::secret_box::FakeBox;

    pub(crate) const ID_A: &str =
        concat!("123456789-abcdefghijklmnopqrstuvwxyz012345", ".apps.googleusercontent.com");
    pub(crate) const ID_B: &str =
        concat!("987654321-zyxwvutsrqponmlkjihgfedcba543210", ".apps.googleusercontent.com");
    pub(crate) const SECRET_A: &str = concat!("GOCSPX", "-AAAAAAAAAAAAAAAAAAAAAAAAAAAA");
    pub(crate) const SECRET_B: &str = concat!("GOCSPX", "-bbbbbbbbbbbbbbbbbbbbbbbb_-12");

    #[test]
    fn the_patterns_compile() {
        assert!(CLIENT_ID.is_some());
        assert!(CLIENT_SECRET.is_some());
    }

    #[test]
    fn finds_ids_and_secrets_in_binary_noise_without_repeats() {
        let mut data = vec![0u8, 159, 255, 1];
        data.extend_from_slice(ID_A.as_bytes());
        data.extend_from_slice(b"\x00\x01junk");
        data.extend_from_slice(SECRET_B.as_bytes());
        data.push(0);
        data.extend_from_slice(ID_A.as_bytes());
        data.extend_from_slice(
            concat!("12345-short", ".apps.googleusercontent.com GOCSPX", "-tooshort ").as_bytes(),
        );
        data.extend_from_slice(SECRET_A.as_bytes());
        let mut found = Discovered::default();
        scan_reader(Cursor::new(data), &mut found).unwrap();
        assert_eq!(found.ids, [ID_A]);
        assert_eq!(found.secrets, [SECRET_B, SECRET_A]);
        assert!(found.is_usable());
    }

    #[test]
    fn finds_a_match_split_across_two_reads() {
        // A reader that hands out the data in two pieces, the second starting mid-id.
        struct Split(Vec<Vec<u8>>);
        impl Read for Split {
            fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
                if self.0.is_empty() {
                    return Ok(0);
                }
                let piece = self.0.remove(0);
                out[..piece.len()].copy_from_slice(&piece);
                Ok(piece.len())
            }
        }
        let (head, tail) = ID_B.split_at(20);
        let reader =
            Split(vec![[b"xx".as_slice(), head.as_bytes()].concat(), tail.as_bytes().to_vec()]);
        let mut found = Discovered::default();
        scan_reader(reader, &mut found).unwrap();
        assert_eq!(found.ids, [ID_B]);
        assert!(!found.is_usable());
    }

    #[test]
    fn scanning_files_skips_unreadable_ones() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("language_server.exe");
        std::fs::write(&good, format!("..{ID_A}..{SECRET_A}..")).unwrap();
        let found = scan(&[dir.path().join("missing.exe"), good]);
        assert_eq!(found, Discovered { ids: vec![ID_A.into()], secrets: vec![SECRET_A.into()] });
    }

    #[test]
    fn the_cache_round_trips_and_ignores_bad_files() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ClientCache::new(
            dir.path().join("sub").join("oauth-client.bin"),
            Arc::new(FakeBox),
            b"e",
        );
        assert_eq!(cache.load(), None);
        let client = ClientCredentials { id: ID_A.into(), secret: Secret::new(SECRET_A) };
        cache.save(&client);
        assert_eq!(cache.load(), Some(client));
        assert!(cache.path().ends_with("oauth-client.bin"));

        for bad in [
            b"garbage".to_vec(),
            FakeBox.seal(b"no-newline", b"e").unwrap(),
            FakeBox.seal(b"\nsecret", b"e").unwrap(),
            FakeBox.seal(&[0xff, b'\n', 0xfe], b"e").unwrap(),
        ] {
            std::fs::write(cache.path(), bad).unwrap();
            assert_eq!(cache.load(), None);
        }
    }

    #[test]
    fn a_cache_that_cannot_be_written_is_only_logged() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("file");
        std::fs::write(&blocker, b"x").unwrap();
        let cache = ClientCache::new(blocker.join("oauth-client.bin"), Arc::new(FakeBox), b"e");
        cache.save(&ClientCredentials { id: ID_A.into(), secret: Secret::new(SECRET_A) });
        assert_eq!(cache.load(), None);
    }
}
