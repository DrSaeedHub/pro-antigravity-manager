//! The probe and the replacement with real executables: a stand-in program is copied under
//! different names (its name picks how it behaves) and run.
#![allow(clippy::unwrap_used, reason = "test helpers fail loudly")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use update::probe::{RunProbe, VersionProbe};
use update::replace::{cleanup, new_path, replace};

fn stand_in(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::copy(env!("CARGO_BIN_EXE_aas-version-probe"), &path).unwrap();
    path
}

#[test]
fn the_probe_reads_the_version_a_program_prints() {
    let dir = tempfile::tempdir().unwrap();
    let probe = RunProbe::default();
    assert_eq!(probe.version(&stand_in(dir.path(), "App.new.exe")).unwrap(), "4.5.6");
}

#[test]
fn the_probe_reports_programs_that_fail_hang_or_say_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let probe = RunProbe { timeout: Duration::from_secs(2) };
    let failing = probe.version(&stand_in(dir.path(), "fail.exe")).unwrap_err();
    assert!(failing.to_string().contains("exited"), "{failing}");
    let quiet = probe.version(&stand_in(dir.path(), "quiet.exe")).unwrap_err();
    assert!(quiet.to_string().contains("no version"), "{quiet}");
    let hanging = probe.version(&stand_in(dir.path(), "hang.exe")).unwrap_err();
    assert!(hanging.to_string().contains("no answer"), "{hanging}");
}

#[cfg(windows)]
#[test]
fn a_running_exe_can_be_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let exe = stand_in(dir.path(), "App.exe");
    let mut running = Command::new(&exe).spawn().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let new = new_path(&exe);
    std::fs::write(&new, b"new build").unwrap();

    let old = replace(&exe, &new, 1).unwrap();
    assert_eq!(std::fs::read(&exe).unwrap(), b"new build");
    assert!(running.try_wait().unwrap().is_none(), "the old program keeps running");
    assert_eq!(cleanup(&exe), 1, "the running old file cannot go yet");
    assert!(old.exists());

    running.kill().unwrap();
    running.wait().unwrap();
    let mut busy = cleanup(&exe);
    for _ in 0..20 {
        if busy == 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
        busy = cleanup(&exe);
    }
    assert_eq!(busy, 0);
    assert!(!old.exists(), "removed once the old program ended");
}
