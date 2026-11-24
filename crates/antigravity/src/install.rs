//! Where Antigravity is installed, and the files its sign-in client can be read from.

use std::path::{Path, PathBuf};

use kernel::{Folder, KnownFolders};

/// The places Antigravity installs itself, in the order they are tried.
#[must_use]
pub fn candidates(folders: &dyn KnownFolders) -> Vec<PathBuf> {
    let mut paths = Vec::with_capacity(3);
    if let Ok(local) = folders.path(Folder::LocalAppData) {
        paths.push(local.join("Programs").join("antigravity").join("Antigravity.exe"));
    }
    for folder in [Folder::ProgramFiles, Folder::ProgramFilesX86] {
        if let Ok(root) = folders.path(folder) {
            paths.push(root.join("Antigravity").join("Antigravity.exe"));
        }
    }
    paths
}

/// The first installed Antigravity executable, if any.
#[must_use]
pub fn locate(folders: &dyn KnownFolders) -> Option<PathBuf> {
    candidates(folders).into_iter().find(|path| path.is_file())
}

/// `language_server*.exe` files shipped with the Antigravity at `exe`, sorted by path.
#[must_use]
pub fn language_server_files(exe: &Path) -> Vec<PathBuf> {
    let Some(root) = exe.parent() else { return Vec::new() };
    let resources = root.join("resources");
    let folders = [
        resources.join("bin"),
        resources.join("app").join("extensions").join("antigravity").join("bin"),
        resources.join("app.asar.unpacked").join("bin"),
    ];
    let mut files = Vec::new();
    for folder in folders {
        let Ok(entries) = std::fs::read_dir(&folder) else { continue };
        let mut found: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_file() && is_language_server(path))
            .collect();
        found.sort();
        files.extend(found);
    }
    files
}

fn is_language_server(path: &Path) -> bool {
    let is_exe = path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("exe"));
    let named = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().starts_with("language_server"));
    is_exe && named
}

#[cfg(test)]
mod tests {
    use std::fs;

    use kernel::{FixedFolders, FolderError};

    use super::*;

    fn touch(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"x").unwrap();
    }

    #[test]
    fn candidates_follow_the_install_order() {
        let folders = FixedFolders::new(Path::new("C:\\r"));
        assert_eq!(
            candidates(&folders),
            vec![
                PathBuf::from("C:\\r\\LocalAppData\\Programs\\antigravity\\Antigravity.exe"),
                PathBuf::from("C:\\r\\ProgramFiles\\Antigravity\\Antigravity.exe"),
                PathBuf::from("C:\\r\\ProgramFilesX86\\Antigravity\\Antigravity.exe"),
            ]
        );
    }

    #[test]
    fn locate_returns_the_first_existing_install() {
        let root = tempfile::tempdir().unwrap();
        let folders = FixedFolders::new(root.path());
        assert_eq!(locate(&folders), None);

        let x86 = root.path().join("ProgramFilesX86\\Antigravity\\Antigravity.exe");
        touch(&x86);
        assert_eq!(locate(&folders), Some(x86));

        let user = root.path().join("LocalAppData\\Programs\\antigravity\\Antigravity.exe");
        touch(&user);
        assert_eq!(locate(&folders), Some(user));
    }

    #[test]
    fn a_folder_that_cannot_be_resolved_is_skipped() {
        #[derive(Debug)]
        struct OnlyProgramFiles(PathBuf);
        impl KnownFolders for OnlyProgramFiles {
            fn path(&self, folder: Folder) -> Result<PathBuf, FolderError> {
                match folder {
                    Folder::ProgramFiles => Ok(self.0.clone()),
                    other => Err(FolderError { folder: other.name() }),
                }
            }
        }
        let folders = OnlyProgramFiles(PathBuf::from("C:\\pf"));
        assert_eq!(
            candidates(&folders),
            vec![PathBuf::from("C:\\pf\\Antigravity\\Antigravity.exe")]
        );
    }

    #[test]
    fn finds_language_servers_in_every_known_folder() {
        let root = tempfile::tempdir().unwrap();
        let exe = root.path().join("Antigravity.exe");
        touch(&exe);
        let resources = root.path().join("resources");
        touch(&resources.join("bin\\language_server_windows_x64.exe"));
        touch(&resources.join("bin\\other.exe"));
        touch(&resources.join("bin\\language_server.txt"));
        touch(&resources.join("app\\extensions\\antigravity\\bin\\Language_Server_B.EXE"));
        touch(&resources.join("app.asar.unpacked\\bin\\language_server_a.exe"));
        fs::create_dir_all(resources.join("bin\\language_server_dir.exe")).unwrap();

        let files = language_server_files(&exe);
        let names: Vec<_> =
            files.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(
            names,
            vec![
                "language_server_windows_x64.exe",
                "Language_Server_B.EXE",
                "language_server_a.exe"
            ]
        );
    }

    #[test]
    fn no_install_means_no_language_servers() {
        let root = tempfile::tempdir().unwrap();
        assert!(language_server_files(&root.path().join("Antigravity.exe")).is_empty());
        assert!(language_server_files(Path::new("")).is_empty());
    }
}
