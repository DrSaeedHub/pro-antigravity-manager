//! The shell port: writing `.lnk` shortcuts.

use std::fmt::Debug;
use std::path::PathBuf;

/// A desktop-integration step failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DesktopError {
    /// A known folder (desktop, Start menu, Local AppData) could not be found.
    #[error("Windows could not find the {0} folder")]
    Folder(&'static str),
    /// Windows could not write the shortcut.
    #[error("the shortcut {path} could not be created: {reason}")]
    Shortcut {
        /// The `.lnk` file.
        path: PathBuf,
        /// What Windows said.
        reason: String,
    },
    /// A file operation failed.
    #[error("{action} failed for {path}: {reason}")]
    File {
        /// What was being done.
        action: &'static str,
        /// The file or folder.
        path: PathBuf,
        /// What Windows said.
        reason: String,
    },
}

impl DesktopError {
    /// A stable code for logs and the UI.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Folder(_) => "desktop.folder",
            Self::Shortcut { .. } => "desktop.shortcut",
            Self::File { .. } => "desktop.file",
        }
    }

    pub(crate) fn file(
        action: &'static str,
        path: &std::path::Path,
        error: &std::io::Error,
    ) -> Self {
        Self::File { action, path: path.to_path_buf(), reason: error.to_string() }
    }
}

/// A Windows shortcut.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shortcut {
    /// Where the `.lnk` file goes.
    pub path: PathBuf,
    /// The program it starts.
    pub target: PathBuf,
    /// The command-line arguments.
    pub arguments: String,
    /// The folder the program starts in.
    pub working_dir: PathBuf,
    /// The file holding its icon.
    pub icon: PathBuf,
    /// The icon's index in that file.
    pub icon_index: i32,
}

/// Writes shortcuts.
pub trait Shell: Send + Sync + Debug {
    /// Creates or replaces the shortcut file `shortcut.path`.
    fn create_shortcut(&self, shortcut: &Shortcut) -> Result<(), DesktopError>;
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn errors_have_codes_and_readable_messages() {
        let folder = DesktopError::Folder("Desktop");
        assert_eq!(folder.code(), "desktop.folder");
        assert_eq!(folder.to_string(), "Windows could not find the Desktop folder");

        let shortcut = DesktopError::Shortcut { path: "a.lnk".into(), reason: "denied".into() };
        assert_eq!(shortcut.code(), "desktop.shortcut");
        assert_eq!(shortcut.to_string(), "the shortcut a.lnk could not be created: denied");

        let io = std::io::Error::other("disk full");
        let file = DesktopError::file("copying the app", Path::new("x.exe"), &io);
        assert_eq!(file.code(), "desktop.file");
        assert_eq!(file.to_string(), "copying the app failed for x.exe: disk full");
    }
}
