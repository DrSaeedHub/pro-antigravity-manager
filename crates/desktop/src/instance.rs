//! One running copy per Windows user: a named mutex marks the running copy, and a named event lets
//! a second start ask it to show its window.
#![allow(unsafe_code, reason = "Win32 calls")]

use std::time::Duration;

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, WAIT_OBJECT_0,
};
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, OpenEventW, SYNCHRONIZATION_SYNCHRONIZE,
    SetEvent, WaitForSingleObject,
};
use windows::core::HSTRING;

use crate::shell::DesktopError;

/// A handle owned by this process, closed on drop.
#[derive(Debug)]
struct Owned(HANDLE);

// SAFETY: kernel object handles may be used and closed from any thread.
unsafe impl Send for Owned {}
// SAFETY: as above; waiting on and closing a handle are thread-safe kernel calls.
unsafe impl Sync for Owned {}

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by this process and is closed exactly once.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Proof that this process is the running copy. Keep it alive for the app's lifetime.
#[derive(Debug)]
pub struct RunningCopy {
    _mutex: Owned,
    wake: Owned,
    answer: Owned,
}

/// The result of trying to become the running copy.
#[derive(Debug)]
pub enum Claim {
    /// This process is the running copy.
    First(RunningCopy),
    /// Another copy already runs.
    AlreadyRunning,
}

struct Names {
    lock: HSTRING,
    wake: HSTRING,
    answer: HSTRING,
}

fn names(name: &str) -> Names {
    Names {
        lock: HSTRING::from(format!("Local\\{name}.instance")),
        wake: HSTRING::from(format!("Local\\{name}.show")),
        answer: HSTRING::from(format!("Local\\{name}.shown")),
    }
}

fn failed(action: &'static str, error: &windows::core::Error) -> DesktopError {
    DesktopError::File { action, path: "single instance".into(), reason: error.message() }
}

/// Becomes the running copy named `name`, unless one already runs.
pub fn claim(name: &str) -> Result<Claim, DesktopError> {
    let names = names(name);
    // SAFETY: plain object creation with names that outlive the calls; handles are owned below.
    unsafe {
        let mutex = CreateMutexW(None, false, &names.lock)
            .map_err(|e| failed("creating the instance lock", &e))?;
        let already = GetLastError() == ERROR_ALREADY_EXISTS;
        let mutex = Owned(mutex);
        if already {
            return Ok(Claim::AlreadyRunning);
        }
        let wake = Owned(
            CreateEventW(None, false, false, &names.wake)
                .map_err(|e| failed("creating the wake event", &e))?,
        );
        let answer = Owned(
            CreateEventW(None, false, false, &names.answer)
                .map_err(|e| failed("creating the answer event", &e))?,
        );
        Ok(Claim::First(RunningCopy { _mutex: mutex, wake, answer }))
    }
}

impl RunningCopy {
    /// Waits up to `limit` for another start to ask for the window, and answers it. True when
    /// asked.
    #[must_use]
    pub fn wait_for_wake(&self, limit: Duration) -> bool {
        let millis = u32::try_from(limit.as_millis()).unwrap_or(u32::MAX);
        // SAFETY: both event handles stay open while `self` lives.
        unsafe {
            if WaitForSingleObject(self.wake.0, millis) != WAIT_OBJECT_0 {
                return false;
            }
            let _ = SetEvent(self.answer.0);
            true
        }
    }
}

/// Asks the running copy named `name` to show its window and waits up to `answer_within` for it
/// to answer. False when no copy runs or it does not answer (it hangs or is ending).
#[must_use]
pub fn wake_running_copy(name: &str, answer_within: Duration) -> bool {
    let names = names(name);
    let millis = u32::try_from(answer_within.as_millis()).unwrap_or(u32::MAX);
    // SAFETY: opening named events, signalling one and waiting on the other; `Owned` closes them.
    unsafe {
        let (Ok(wake), Ok(answer)) = (
            OpenEventW(EVENT_MODIFY_STATE, false, &names.wake),
            OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, false, &names.answer),
        ) else {
            return false;
        };
        let (wake, answer) = (Owned(wake), Owned(answer));
        SetEvent(wake.0).is_ok() && WaitForSingleObject(answer.0, millis) == WAIT_OBJECT_0
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn unique() -> String {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        format!("aas-test-instance-{}-{nanos}", std::process::id())
    }

    #[test]
    fn the_first_claim_wins_and_a_second_one_sees_it() {
        let name = unique();
        let Claim::First(running) = claim(&name).unwrap() else { panic!("first claim must win") };
        assert!(matches!(claim(&name).unwrap(), Claim::AlreadyRunning));
        drop(running);
        assert!(matches!(claim(&name).unwrap(), Claim::First(_)), "free again once dropped");
    }

    #[test]
    fn a_second_start_wakes_the_running_copy_and_gets_an_answer() {
        let name = unique();
        let Claim::First(running) = claim(&name).unwrap() else { panic!("first claim must win") };
        assert!(!running.wait_for_wake(Duration::from_millis(10)), "nobody asked yet");
        let listener = std::thread::spawn(move || running.wait_for_wake(Duration::from_secs(5)));
        assert!(wake_running_copy(&name, Duration::from_secs(5)));
        assert!(listener.join().unwrap());
    }

    #[test]
    fn a_copy_that_does_not_answer_is_reported() {
        let name = unique();
        let Claim::First(_running) = claim(&name).unwrap() else { panic!("first claim must win") };
        assert!(!wake_running_copy(&name, Duration::from_millis(100)), "nobody is listening");
    }

    #[test]
    fn waking_nothing_reports_false() {
        assert!(!wake_running_copy(&unique(), Duration::from_millis(10)));
    }

    #[test]
    fn failures_are_described() {
        let error = windows::core::Error::from_hresult(windows::core::HRESULT(-2_147_024_891));
        assert_eq!(failed("x", &error).code(), "desktop.file");
    }
}
