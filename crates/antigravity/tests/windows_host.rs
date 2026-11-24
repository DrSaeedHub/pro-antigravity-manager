//! The real Windows host against real processes: copies of a tiny test window under a unique
//! executable name, so no other program (and never Antigravity) is touched.
#![cfg(windows)]
#![allow(clippy::unwrap_used, reason = "test helpers fail loudly")]

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::Arc;
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use antigravity::{Antigravity, AntigravityError, AppHost, HostSettings, WindowsHost};
use kernel::FixedFolders;

struct Dummy {
    _dir: tempfile::TempDir,
    exe: PathBuf,
    name: String,
    children: Vec<Child>,
}

impl Dummy {
    fn new(tag: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let name = format!("aas-dummy-{tag}-{}-{nanos}", std::process::id());
        let exe = dir.path().join(format!("{name}.exe"));
        std::fs::copy(env!("CARGO_BIN_EXE_aas-test-window"), &exe).unwrap();
        Self { _dir: dir, exe, name, children: Vec::new() }
    }

    fn spawn(&mut self, stubborn: bool) {
        let mut command = Command::new(&self.exe);
        if stubborn {
            command.arg("stubborn");
        }
        self.children.push(command.spawn().unwrap());
    }
}

impl Drop for Dummy {
    fn drop(&mut self) {
        for child in &mut self.children {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn wait_for(host: &WindowsHost, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while host.running() != count {
        assert!(Instant::now() < deadline, "expected {count} processes, saw {}", host.running());
        sleep(Duration::from_millis(50));
    }
    // Give the windows a moment to appear before closing them.
    sleep(Duration::from_millis(300));
}

fn quick() -> HostSettings {
    HostSettings {
        close_wait: Duration::from_secs(5),
        kill_wait: Duration::from_secs(5),
        poll: Duration::from_millis(50),
    }
}

fn service(host: WindowsHost) -> Antigravity {
    Antigravity::new(Arc::new(host), Arc::new(FixedFolders::new(Path::new("C:\\none"))), quick())
}

#[test]
fn counts_and_closes_windows_politely() {
    let mut dummy = Dummy::new("polite");
    let host = WindowsHost::named(&dummy.name);
    assert_eq!(host.running(), 0);
    dummy.spawn(false);
    dummy.spawn(false);
    wait_for(&host, 2);

    service(host.clone()).close().unwrap();
    assert_eq!(host.running(), 0);
    for child in &mut dummy.children {
        assert_eq!(child.wait().unwrap().code(), Some(0), "closed by WM_CLOSE, not killed");
    }
}

#[test]
fn ends_processes_that_ignore_the_close_request() {
    let mut dummy = Dummy::new("stubborn");
    let host = WindowsHost::named(&dummy.name);
    dummy.spawn(true);
    wait_for(&host, 1);

    let settings = HostSettings { close_wait: Duration::from_millis(400), ..quick() };
    let antigravity = Antigravity::new(
        Arc::new(host.clone()),
        Arc::new(FixedFolders::new(Path::new("C:\\none"))),
        settings,
    );
    antigravity.close().unwrap();
    assert_eq!(host.running(), 0);
    assert_eq!(dummy.children[0].wait().unwrap().code(), Some(1), "ended by TerminateProcess");
}

#[test]
fn starts_an_executable_from_its_folder() {
    let dummy = Dummy::new("start");
    let host = WindowsHost::named(&dummy.name);
    host.start(&dummy.exe).unwrap();
    wait_for(&host, 1);
    host.kill();
    let deadline = Instant::now() + Duration::from_secs(10);
    while host.running() > 0 {
        assert!(Instant::now() < deadline);
        sleep(Duration::from_millis(50));
    }
}

#[test]
fn reports_a_missing_executable() {
    let host = WindowsHost::named("aas-never-exists");
    let error = host.start(Path::new("C:\\definitely\\missing\\nothing.exe")).unwrap_err();
    assert!(matches!(error, AntigravityError::StartFailed(_)), "{error:?}");
    host.request_close();
    host.kill();
}

#[test]
fn the_antigravity_host_looks_for_antigravity_exe() {
    // Reads only: whether Antigravity runs on this PC is not changed.
    let host = WindowsHost::antigravity();
    let _ = host.running();
    assert_eq!(format!("{host:?}"), "WindowsHost { executable: \"antigravity.exe\" }");
}
