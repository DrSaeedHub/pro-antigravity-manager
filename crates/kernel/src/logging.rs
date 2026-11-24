//! A small diagnostic log file. It never receives tokens: secret values are [`crate::Secret`]s.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tracing::Level;

/// The log file name inside the log folder.
pub const LOG_FILE: &str = "app.log";
/// The previous log, kept after a rotation.
pub const OLD_LOG_FILE: &str = "app.1.log";
/// A log larger than this is rotated when the app starts.
pub const MAX_LOG_BYTES: u64 = 1024 * 1024;

/// The log could not be set up.
#[derive(Debug, thiserror::Error)]
pub enum LoggingError {
    /// The folder or file could not be created.
    #[error("could not open the log file: {0}")]
    Io(#[from] io::Error),
    /// Logging was already set up in this process.
    #[error("logging is already set up")]
    AlreadySet,
}

/// Renames `dir/app.log` to `dir/app.1.log` when it is larger than `limit` bytes.
/// Returns whether it rotated.
pub fn rotate_if_large(dir: &Path, limit: u64) -> io::Result<bool> {
    let current = dir.join(LOG_FILE);
    match fs::metadata(&current) {
        Ok(meta) if meta.len() > limit => {
            fs::rename(&current, dir.join(OLD_LOG_FILE))?;
            Ok(true)
        }
        Ok(_) => Ok(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// Opens (creating as needed) the log file in `dir`, after rotating an oversized one.
pub fn open_log(dir: &Path) -> Result<(File, PathBuf), LoggingError> {
    fs::create_dir_all(dir)?;
    rotate_if_large(dir, MAX_LOG_BYTES)?;
    let path = dir.join(LOG_FILE);
    let file = OpenOptions::new().create(true).append(true).open(&path)?;
    Ok((file, path))
}

/// Sends `info` and more severe events of this process to `dir/app.log`.
pub fn init(dir: &Path) -> Result<PathBuf, LoggingError> {
    let (file, path) = open_log(dir)?;
    tracing_subscriber::fmt()
        .with_writer(Mutex::new(file))
        .with_ansi(false)
        .with_max_level(Level::INFO)
        .with_target(true)
        .try_init()
        .map_err(|_| LoggingError::AlreadySet)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_small_log_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(LOG_FILE), b"short").unwrap();
        assert!(!rotate_if_large(dir.path(), 10).unwrap());
        assert!(dir.path().join(LOG_FILE).exists());
    }

    #[test]
    fn a_large_log_is_rotated_over_the_previous_one() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(OLD_LOG_FILE), b"older").unwrap();
        fs::write(dir.path().join(LOG_FILE), b"0123456789ABC").unwrap();
        assert!(rotate_if_large(dir.path(), 10).unwrap());
        assert!(!dir.path().join(LOG_FILE).exists());
        assert_eq!(fs::read(dir.path().join(OLD_LOG_FILE)).unwrap(), b"0123456789ABC");
    }

    #[test]
    fn a_missing_log_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!rotate_if_large(dir.path(), 10).unwrap());
    }

    #[test]
    fn open_log_creates_the_folder_and_appends() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("logs");
        let (file, path) = open_log(&logs).unwrap();
        drop(file);
        assert_eq!(path, logs.join(LOG_FILE));
        assert!(path.exists());
    }

    #[test]
    fn init_writes_events_and_refuses_a_second_setup() {
        let dir = tempfile::tempdir().unwrap();
        let path = init(dir.path()).unwrap();
        tracing::info!(target: "kernel_test", "hello from the test");
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("hello from the test"), "{text}");
        assert!(matches!(init(dir.path()), Err(LoggingError::AlreadySet)));
    }

    #[test]
    fn open_log_reports_io_errors() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("not-a-folder");
        fs::write(&blocker, b"file").unwrap();
        let error = open_log(&blocker.join("logs")).unwrap_err();
        assert!(matches!(error, LoggingError::Io(_)));
        assert!(error.to_string().starts_with("could not open the log file"));
    }
}
