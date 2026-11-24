//! Why an update could not be checked for or installed.

use std::path::Path;

/// Checking for or installing an update failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UpdateError {
    /// GitHub could not be reached.
    #[error("GitHub could not be reached: {0}")]
    Network(String),
    /// The release feed answered with an unexpected HTTP status.
    #[error("the release feed answered HTTP {0}")]
    Feed(u16),
    /// The release feed is not a manifest this version understands.
    #[error("the release feed could not be read: {0}")]
    BadFeed(String),
    /// The newest release has no package for this platform.
    #[error("the newest release has no build for this PC")]
    NoBuild,
    /// The manifest points outside the project's repository.
    #[error("the release points outside the project: {0}")]
    Untrusted(String),
    /// The package download answered with an unexpected HTTP status.
    #[error("the download answered HTTP {0}")]
    Download(u16),
    /// The download is not the size the manifest says.
    #[error("the download has {got} bytes instead of {expected}")]
    Size {
        /// The size in the manifest.
        expected: u64,
        /// The bytes received.
        got: u64,
    },
    /// The download does not match the manifest's SHA-256.
    #[error("the download does not match its checksum")]
    Checksum,
    /// The package is not a zip with the expected executable.
    #[error("the downloaded package is unusable: {0}")]
    Package(String),
    /// The folder of the running copy cannot be written.
    #[error("this copy cannot change its own folder: {0}")]
    NotWritable(String),
    /// A file operation failed.
    #[error("{action} {path}: {reason}")]
    File {
        /// What was being done.
        action: &'static str,
        /// The file.
        path: String,
        /// The system's reason.
        reason: String,
    },
    /// The new executable did not start or reported another version.
    #[error("the new version did not start correctly: {0}")]
    Probe(String),
    /// The person canceled the update.
    #[error("the update was canceled")]
    Canceled,
}

impl UpdateError {
    /// A stable code for logs and the UI.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Network(_) => "update.network",
            Self::Feed(_) => "update.feed",
            Self::BadFeed(_) => "update.bad_feed",
            Self::NoBuild => "update.no_build",
            Self::Untrusted(_) => "update.untrusted",
            Self::Download(_) => "update.download",
            Self::Size { .. } => "update.size",
            Self::Checksum => "update.checksum",
            Self::Package(_) => "update.package",
            Self::NotWritable(_) => "update.not_writable",
            Self::File { .. } => "update.file",
            Self::Probe(_) => "update.probe",
            Self::Canceled => "update.canceled",
        }
    }

    /// A failed file operation on `path`. A refusal to write becomes [`Self::NotWritable`] with
    /// the folder, because that is what the person can act on.
    #[must_use]
    pub fn file(action: &'static str, path: &Path, error: &std::io::Error) -> Self {
        if error.kind() == std::io::ErrorKind::PermissionDenied {
            let folder = path.parent().unwrap_or(path);
            return Self::NotWritable(folder.display().to_string());
        }
        Self::File { action, path: path.display().to_string(), reason: error.to_string() }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Error, ErrorKind};

    use super::*;

    #[test]
    fn every_error_has_its_code() {
        let all = [
            UpdateError::Network(String::new()),
            UpdateError::Feed(500),
            UpdateError::BadFeed(String::new()),
            UpdateError::NoBuild,
            UpdateError::Untrusted(String::new()),
            UpdateError::Download(404),
            UpdateError::Size { expected: 2, got: 1 },
            UpdateError::Checksum,
            UpdateError::Package(String::new()),
            UpdateError::NotWritable(String::new()),
            UpdateError::File { action: "a", path: String::new(), reason: String::new() },
            UpdateError::Probe(String::new()),
            UpdateError::Canceled,
        ];
        let codes: Vec<&str> = all.iter().map(UpdateError::code).collect();
        assert!(codes.iter().all(|code| code.starts_with("update.")));
        let mut unique = codes.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), codes.len(), "codes are distinct");
        assert_eq!(all[6].to_string(), "the download has 1 bytes instead of 2");
    }

    #[test]
    fn a_refused_write_names_the_folder() {
        let path = Path::new("C:/Program Files/App/app.new.exe");
        let refused = UpdateError::file("writing", path, &Error::from(ErrorKind::PermissionDenied));
        assert_eq!(
            refused,
            UpdateError::NotWritable(Path::new("C:/Program Files/App").display().to_string())
        );
        let other = UpdateError::file("writing", path, &Error::from(ErrorKind::NotFound));
        assert_eq!(other.code(), "update.file");
        assert!(other.to_string().starts_with("writing "));
    }
}
