//! Known folders from the Windows Shell (`SHGetKnownFolderPath`).
#![allow(unsafe_code, reason = "Win32 calls")]

use std::path::PathBuf;

use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{
    FOLDERID_Desktop, FOLDERID_LocalAppData, FOLDERID_ProgramFiles, FOLDERID_ProgramFilesX86,
    FOLDERID_Programs, FOLDERID_Startup, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
};
use windows::core::GUID;

use super::{Folder, FolderError, KnownFolders};

/// The real known folders of the signed-in Windows user.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemFolders;

fn folder_id(folder: Folder) -> &'static GUID {
    match folder {
        Folder::LocalAppData => &FOLDERID_LocalAppData,
        Folder::ProgramFiles => &FOLDERID_ProgramFiles,
        Folder::ProgramFilesX86 => &FOLDERID_ProgramFilesX86,
        Folder::Desktop => &FOLDERID_Desktop,
        Folder::StartMenuPrograms => &FOLDERID_Programs,
        Folder::Startup => &FOLDERID_Startup,
    }
}

impl KnownFolders for SystemFolders {
    fn path(&self, folder: Folder) -> Result<PathBuf, FolderError> {
        let error = || FolderError { folder: folder.name() };
        // SAFETY: the GUID reference is valid for the call; on success Windows returns a
        // NUL-terminated string that we copy and then free exactly once with CoTaskMemFree.
        unsafe {
            let raw = SHGetKnownFolderPath(folder_id(folder), KF_FLAG_DEFAULT, None)
                .map_err(|_| error())?;
            let text = raw.to_string();
            CoTaskMemFree(Some(raw.0.cast_const().cast()));
            text.map(PathBuf::from).map_err(|_| error())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_known_folder_resolves_to_an_existing_absolute_path() {
        for folder in Folder::ALL {
            let path = SystemFolders.path(folder).unwrap();
            assert!(path.is_absolute(), "{folder:?} -> {path:?}");
            assert!(path.exists(), "{folder:?} -> {path:?}");
        }
    }

    #[test]
    fn local_app_data_is_the_users_local_folder() {
        let path = SystemFolders.path(Folder::LocalAppData).unwrap();
        assert!(path.ends_with("AppData\\Local") || path.ends_with("AppData/Local"), "{path:?}");
    }
}
