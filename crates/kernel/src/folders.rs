//! Windows known folders (`LocalAppData`, Program Files, Desktop, Start menu) behind a port.

use std::fmt::Debug;
use std::path::{Path, PathBuf};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::SystemFolders;

/// A folder the app needs to find.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Folder {
    /// `%LOCALAPPDATA%`.
    LocalAppData,
    /// `%ProgramFiles%` (64-bit).
    ProgramFiles,
    /// `%ProgramFiles(x86)%`.
    ProgramFilesX86,
    /// The user's desktop folder.
    Desktop,
    /// The user's Start menu `Programs` folder.
    StartMenuPrograms,
    /// The user's Startup folder (programs started at sign-in).
    Startup,
}

impl Folder {
    /// Every folder, in a fixed order.
    pub const ALL: [Self; 6] = [
        Self::LocalAppData,
        Self::ProgramFiles,
        Self::ProgramFilesX86,
        Self::Desktop,
        Self::StartMenuPrograms,
        Self::Startup,
    ];

    /// A stable name used for logs and test folders.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::LocalAppData => "LocalAppData",
            Self::ProgramFiles => "ProgramFiles",
            Self::ProgramFilesX86 => "ProgramFilesX86",
            Self::Desktop => "Desktop",
            Self::StartMenuPrograms => "StartMenuPrograms",
            Self::Startup => "Startup",
        }
    }
}

/// A known folder could not be resolved.
#[derive(Debug, thiserror::Error)]
#[error("Windows could not find the {folder} folder")]
pub struct FolderError {
    /// The folder that failed.
    pub folder: &'static str,
}

/// Finds the paths of known folders.
pub trait KnownFolders: Send + Sync + Debug {
    /// The path of `folder`.
    fn path(&self, folder: Folder) -> Result<PathBuf, FolderError>;
}

/// Every known folder placed under one root, for tests and previews.
#[derive(Debug, Clone)]
pub struct FixedFolders {
    root: PathBuf,
}

impl FixedFolders {
    /// Folders under `root`, one sub-folder per [`Folder`] named by [`Folder::name`].
    #[must_use]
    pub fn new(root: &Path) -> Self {
        Self { root: root.to_path_buf() }
    }
}

impl KnownFolders for FixedFolders {
    fn path(&self, folder: Folder) -> Result<PathBuf, FolderError> {
        Ok(self.root.join(folder.name()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_folders_live_under_their_root() {
        let folders = FixedFolders::new(Path::new("C:\\root"));
        for folder in Folder::ALL {
            let path = folders.path(folder).unwrap();
            assert_eq!(path, Path::new("C:\\root").join(folder.name()));
        }
    }

    #[test]
    fn folder_names_are_unique() {
        let mut names: Vec<_> = Folder::ALL.iter().map(|folder| folder.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), Folder::ALL.len());
    }

    #[test]
    fn folder_error_names_the_folder() {
        let error = FolderError { folder: Folder::Desktop.name() };
        assert_eq!(error.to_string(), "Windows could not find the Desktop folder");
    }
}
