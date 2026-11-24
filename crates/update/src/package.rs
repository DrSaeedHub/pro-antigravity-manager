//! Taking the executable out of a downloaded package (a zip).

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use crate::error::UpdateError;

/// The largest executable accepted from a package.
pub const MAX_EXECUTABLE_BYTES: u64 = 500 * 1024 * 1024;

fn unusable(reason: impl std::fmt::Display) -> UpdateError {
    UpdateError::Package(reason.to_string())
}

/// Writes the top-level entry `file` (any letter case) of the zip `archive` to `to`. On failure
/// `to` is removed.
pub fn extract(archive: &Path, file: &str, to: &Path) -> Result<(), UpdateError> {
    let result = copy_out(archive, file, to);
    if result.is_err() {
        let _ = std::fs::remove_file(to);
    }
    result
}

fn copy_out(archive: &Path, file: &str, to: &Path) -> Result<(), UpdateError> {
    let reader =
        File::open(archive).map_err(|error| UpdateError::file("opening", archive, &error))?;
    let mut zip = zip::ZipArchive::new(reader).map_err(unusable)?;
    let index = (0..zip.len())
        .find(|&index| {
            zip.name_for_index(index)
                .is_some_and(|name| name.is_ok_and(|name| name.eq_ignore_ascii_case(file)))
        })
        .ok_or_else(|| unusable(format!("{file} is not in it")))?;
    let mut entry = zip.by_index(index).map_err(unusable)?;
    if !entry.is_file() || entry.size() > MAX_EXECUTABLE_BYTES {
        return Err(unusable(format!("{file} is not a usable file")));
    }
    let mut out = File::create(to).map_err(|error| UpdateError::file("creating", to, &error))?;
    let mut buffer = vec![0u8; 64 * 1024];
    let mut written = 0u64;
    loop {
        let read = entry.read(&mut buffer).map_err(unusable)?;
        if read == 0 {
            break;
        }
        written += read as u64;
        if written > MAX_EXECUTABLE_BYTES {
            return Err(unusable(format!("{file} is too large")));
        }
        out.write_all(&buffer[..read]).map_err(|error| UpdateError::file("writing", to, &error))?;
    }
    out.flush().map_err(|error| UpdateError::file("writing", to, &error))?;
    Ok(())
}

/// A zip with `entries` (name, bytes), for tests.
#[cfg(test)]
pub(crate) fn make_zip(path: &Path, entries: &[(&str, &[u8])]) {
    use zip::write::SimpleFileOptions;
    let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, bytes) in entries {
        zip.start_file(*name, options).unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_executable_comes_out_of_the_zip() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("p.zip");
        make_zip(
            &archive,
            &[("LICENSE", b"MIT"), ("App.exe", b"MZ new build"), ("README.md", b"#")],
        );
        let to = dir.path().join("App.new.exe");
        extract(&archive, "app.EXE", &to).unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), b"MZ new build");
    }

    #[test]
    fn a_zip_without_the_exe_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("p.zip");
        make_zip(&archive, &[("LICENSE", b"MIT"), ("sub/App.exe", b"MZ")]);
        let to = dir.path().join("App.new.exe");
        let error = extract(&archive, "App.exe", &to).unwrap_err();
        assert_eq!(error, UpdateError::Package("App.exe is not in it".into()));
        assert!(!to.exists());
    }

    #[test]
    fn something_that_is_not_a_zip_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("p.zip");
        std::fs::write(&archive, b"<html>not a zip</html>").unwrap();
        let error = extract(&archive, "App.exe", &dir.path().join("x")).unwrap_err();
        assert_eq!(error.code(), "update.package");
        let error = extract(&dir.path().join("missing.zip"), "App.exe", &dir.path().join("x"));
        assert_eq!(error.unwrap_err().code(), "update.file");
    }

    #[test]
    fn a_folder_entry_is_not_an_executable() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("p.zip");
        {
            let mut zip = zip::ZipWriter::new(File::create(&archive).unwrap());
            zip.add_directory("App.exe/", zip::write::SimpleFileOptions::default()).unwrap();
            zip.finish().unwrap();
        }
        let error = extract(&archive, "App.exe/", &dir.path().join("x")).unwrap_err();
        assert_eq!(error, UpdateError::Package("App.exe/ is not a usable file".into()));
    }
}
