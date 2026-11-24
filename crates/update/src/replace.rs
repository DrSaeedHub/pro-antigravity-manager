//! Putting a new executable in place of the running one, and cleaning up afterwards.
//!
//! Windows does not let a running executable be overwritten or deleted, but it may be renamed. So
//! the running file is renamed to `<name>.<stamp>.old`, and the new one takes its name. The next
//! start removes the old file (it may still be running for a moment, so this is retried).

use std::path::{Path, PathBuf};

use crate::error::UpdateError;

fn sibling(exe: &Path, name: &str) -> PathBuf {
    exe.with_file_name(name)
}

fn stem(exe: &Path) -> String {
    exe.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default()
}

fn file_name(exe: &Path) -> String {
    exe.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Where the downloaded package waits next to `exe`.
#[must_use]
pub fn download_path(exe: &Path) -> PathBuf {
    sibling(exe, &format!("{}.download.zip", stem(exe)))
}

/// Where the new executable waits next to `exe` until it replaces it.
#[must_use]
pub fn new_path(exe: &Path) -> PathBuf {
    sibling(exe, &format!("{}.new.exe", stem(exe)))
}

/// Where the running `exe` goes while it is replaced.
#[must_use]
pub fn old_path(exe: &Path, stamp: u128) -> PathBuf {
    sibling(exe, &format!("{}.{stamp}.old", file_name(exe)))
}

/// Whether `name` is something an update leaves behind next to `exe`.
fn is_leftover(exe: &Path, name: &str) -> bool {
    let file = file_name(exe).to_lowercase();
    let name = name.to_lowercase();
    let old = name.strip_prefix(&format!("{file}.")).is_some_and(|rest| {
        rest.strip_suffix(".old").is_some_and(|stamp| stamp.bytes().all(|b| b.is_ascii_digit()))
    });
    old || [download_path(exe), new_path(exe)]
        .iter()
        .any(|path| file_name(path).to_lowercase() == name)
}

/// Renames the running `exe` aside and moves `new` into its name. When the second step fails, the
/// first is undone. Returns where the old executable went.
pub fn replace(exe: &Path, new: &Path, stamp: u128) -> Result<PathBuf, UpdateError> {
    if !new.is_file() {
        return Err(UpdateError::Package(format!("{} is missing", new.display())));
    }
    let old = old_path(exe, stamp);
    std::fs::rename(exe, &old).map_err(|error| UpdateError::file("moving aside", exe, &error))?;
    if let Err(error) = std::fs::rename(new, exe) {
        let _ = std::fs::rename(&old, exe);
        return Err(UpdateError::file("putting in place", exe, &error));
    }
    tracing::info!(exe = %exe.display(), "new version in place");
    Ok(old)
}

/// Removes what earlier updates left next to `exe` (old executables, unfinished downloads).
/// Returns how many files are still there because they are in use.
pub fn cleanup(exe: &Path) -> usize {
    let Some(dir) = exe.parent() else { return 0 };
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let mut busy = 0;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_leftover(exe, &name) && entry.path().is_file() {
            match std::fs::remove_file(entry.path()) {
                Ok(()) => tracing::info!(file = %name, "removed what an update left"),
                Err(_) => busy += 1,
            }
        }
    }
    busy
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_sit_next_to_the_exe() {
        let exe = Path::new("C:/Apps/App.exe");
        assert_eq!(download_path(exe), Path::new("C:/Apps/App.download.zip"));
        assert_eq!(new_path(exe), Path::new("C:/Apps/App.new.exe"));
        assert_eq!(old_path(exe, 42), Path::new("C:/Apps/App.exe.42.old"));
    }

    #[test]
    fn leftovers_are_recognised_and_nothing_else() {
        let exe = Path::new("C:/Apps/App.exe");
        for name in ["App.exe.17.old", "app.EXE.1.OLD", "App.new.exe", "App.download.zip"] {
            assert!(is_leftover(exe, name), "{name}");
        }
        for name in ["App.exe", "App.exe.old", "App.exe.x1.old", "Other.exe.1.old", "settings.json"]
        {
            assert!(!is_leftover(exe, name), "{name}");
        }
    }

    #[test]
    fn the_new_exe_takes_the_old_ones_name() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("App.exe");
        std::fs::write(&exe, b"old build").unwrap();
        let new = new_path(&exe);
        std::fs::write(&new, b"new build").unwrap();
        let old = replace(&exe, &new, 7).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"new build");
        assert_eq!(std::fs::read(&old).unwrap(), b"old build");
        assert!(!new.exists());
    }

    #[test]
    fn a_missing_new_exe_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("App.exe");
        std::fs::write(&exe, b"old build").unwrap();
        let error = replace(&exe, &new_path(&exe), 7).unwrap_err();
        assert_eq!(error.code(), "update.package");
        assert_eq!(std::fs::read(&exe).unwrap(), b"old build");
    }

    #[test]
    fn a_folder_in_place_of_the_new_exe_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("App.exe");
        std::fs::write(&exe, b"old build").unwrap();
        let new = new_path(&exe);
        std::fs::create_dir(&new).unwrap();
        assert_eq!(replace(&exe, &new, 1).unwrap_err().code(), "update.package");
        assert_eq!(std::fs::read(&exe).unwrap(), b"old build");
    }

    #[cfg(windows)]
    #[test]
    fn a_failed_move_restores_the_old_exe() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("App.exe");
        std::fs::write(&exe, b"old build").unwrap();
        let new = new_path(&exe);
        std::fs::write(&new, b"new build").unwrap();
        // Held open without sharing, the new file cannot be moved: the old one must come back.
        let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(&new).unwrap();
        let error = replace(&exe, &new, 1).unwrap_err();
        drop(held);
        assert_eq!(error.code(), "update.file", "{error}");
        assert_eq!(std::fs::read(&exe).unwrap(), b"old build");
        assert!(!old_path(&exe, 1).exists());
    }

    #[test]
    fn a_missing_exe_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("App.exe");
        let new = new_path(&exe);
        std::fs::write(&new, b"new build").unwrap();
        assert_eq!(replace(&exe, &new, 1).unwrap_err().code(), "update.file");
        assert!(new.exists(), "the new file waits for another try");
    }

    #[test]
    fn leftovers_are_cleaned() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("App.exe");
        for name in [
            "App.exe",
            "App.exe.1.old",
            "App.exe.2.old",
            "App.new.exe",
            "App.download.zip",
            "keep.txt",
        ] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        assert_eq!(cleanup(&exe), 0);
        let mut left: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["App.exe", "keep.txt"]);
        assert_eq!(cleanup(Path::new("")), 0);
        assert_eq!(cleanup(&dir.path().join("missing").join("App.exe")), 0);
    }
}
