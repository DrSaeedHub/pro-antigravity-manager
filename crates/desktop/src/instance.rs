//! One running copy per Windows user. A named mutex marks the running copy; named events let a
//! second start ask it to show its window (and wait for its answer), and let the installer ask it
//! to quit.
//!
//! Names, for `<name>`: `Local\<name>.instance` (the lock), `Local\<name>.show`,
//! `Local\<name>.shown` (the answer) and `Local\<name>.quit`. The installer script uses the last
//! two names too.
#![allow(unsafe_code, reason = "Win32 calls")]

use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, WAIT_OBJECT_0,
};
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, OpenEventW, SYNCHRONIZATION_SYNCHRONIZE,
    SetEvent, WaitForMultipleObjects, WaitForSingleObject,
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

/// What another process asked the running copy for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    /// Show the window (a second start).
    Show,
    /// End (the installer is about to replace the app).
    Quit,
}

/// Proof that this process is the running copy. Keep it alive for the app's lifetime.
#[derive(Debug)]
pub struct RunningCopy {
    lock: Mutex<Option<Owned>>,
    show: Owned,
    answer: Owned,
    quit: Owned,
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
    show: HSTRING,
    answer: HSTRING,
    quit: HSTRING,
}

fn names(name: &str) -> Names {
    Names {
        lock: HSTRING::from(format!("Local\\{name}.instance")),
        show: HSTRING::from(format!("Local\\{name}.show")),
        answer: HSTRING::from(format!("Local\\{name}.shown")),
        quit: HSTRING::from(format!("Local\\{name}.quit")),
    }
}

fn failed(action: &'static str, error: &windows::core::Error) -> DesktopError {
    DesktopError::File { action, path: "single instance".into(), reason: error.message() }
}

fn event(name: &HSTRING, action: &'static str) -> Result<Owned, DesktopError> {
    // SAFETY: plain object creation with a name that outlives the call; the handle is owned.
    unsafe { CreateEventW(None, false, false, name) }
        .map(Owned)
        .map_err(|error| failed(action, &error))
}

/// Becomes the running copy named `name`, unless one already runs.
pub fn claim(name: &str) -> Result<Claim, DesktopError> {
    let names = names(name);
    // SAFETY: plain object creation with a name that outlives the call; the handle is owned.
    let (mutex, already) = unsafe {
        let mutex = CreateMutexW(None, false, &names.lock)
            .map_err(|e| failed("creating the instance lock", &e))?;
        (Owned(mutex), GetLastError() == ERROR_ALREADY_EXISTS)
    };
    if already {
        return Ok(Claim::AlreadyRunning);
    }
    Ok(Claim::First(RunningCopy {
        lock: Mutex::new(Some(mutex)),
        show: event(&names.show, "creating the show event")?,
        answer: event(&names.answer, "creating the answer event")?,
        quit: event(&names.quit, "creating the quit event")?,
    }))
}

impl RunningCopy {
    /// Waits up to `limit` for another process to ask something, and answers a request to show
    /// the window. `None` when nothing was asked.
    #[must_use]
    pub fn wait(&self, limit: Duration) -> Option<Request> {
        let millis = u32::try_from(limit.as_millis()).unwrap_or(u32::MAX);
        let handles = [self.show.0, self.quit.0];
        // SAFETY: the event handles stay open while `self` lives.
        unsafe {
            let result = WaitForMultipleObjects(&handles, false, millis);
            if result == WAIT_OBJECT_0 {
                let _ = SetEvent(self.answer.0);
                Some(Request::Show)
            } else if result.0 == WAIT_OBJECT_0.0 + 1 {
                Some(Request::Quit)
            } else {
                None
            }
        }
    }

    /// Gives up the lock, so a new copy (the updated app) can become the running copy while this
    /// process ends.
    pub fn release(&self) {
        self.lock.lock().unwrap_or_else(PoisonError::into_inner).take();
    }
}

fn signal(name: &HSTRING) -> Option<Owned> {
    // SAFETY: opening a named event; `Owned` closes it.
    let event = unsafe { OpenEventW(EVENT_MODIFY_STATE, false, name) }.ok().map(Owned)?;
    // SAFETY: the handle is open.
    unsafe { SetEvent(event.0) }.ok().map(|()| event)
}

/// Asks the running copy named `name` to show its window and waits up to `answer_within` for it
/// to answer. False when no copy runs or it does not answer (it hangs or is ending).
#[must_use]
pub fn wake_running_copy(name: &str, answer_within: Duration) -> bool {
    let names = names(name);
    let millis = u32::try_from(answer_within.as_millis()).unwrap_or(u32::MAX);
    // SAFETY: opening a named event; `Owned` closes it.
    let Ok(answer) = (unsafe { OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, false, &names.answer) })
    else {
        return false;
    };
    let answer = Owned(answer);
    if signal(&names.show).is_none() {
        return false;
    }
    // SAFETY: the handle is open.
    unsafe { WaitForSingleObject(answer.0, millis) == WAIT_OBJECT_0 }
}

/// Asks the running copy named `name` to quit. False when no copy listens.
#[must_use]
pub fn ask_to_quit(name: &str) -> bool {
    signal(&names(name).quit).is_some()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
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
    fn a_released_lock_lets_the_next_copy_start() {
        let name = unique();
        let Claim::First(running) = claim(&name).unwrap() else { panic!("first claim must win") };
        running.release();
        running.release();
        let Claim::First(next) = claim(&name).unwrap() else { panic!("the lock was released") };
        drop(next);
        drop(running);
    }

    #[test]
    fn a_second_start_wakes_the_running_copy_and_gets_an_answer() {
        let name = unique();
        let Claim::First(running) = claim(&name).unwrap() else { panic!("first claim must win") };
        assert_eq!(running.wait(Duration::from_millis(10)), None, "nobody asked yet");
        let listener = std::thread::spawn(move || running.wait(Duration::from_secs(5)));
        assert!(wake_running_copy(&name, Duration::from_secs(5)));
        assert_eq!(listener.join().unwrap(), Some(Request::Show));
    }

    #[test]
    fn the_installer_can_ask_the_running_copy_to_quit() {
        let name = unique();
        let Claim::First(running) = claim(&name).unwrap() else { panic!("first claim must win") };
        let running = Arc::new(running);
        let listening = running.clone();
        let listener = std::thread::spawn(move || listening.wait(Duration::from_secs(5)));
        assert!(ask_to_quit(&name));
        assert_eq!(listener.join().unwrap(), Some(Request::Quit));
    }

    #[test]
    fn a_copy_that_does_not_answer_is_reported() {
        let name = unique();
        let Claim::First(_running) = claim(&name).unwrap() else { panic!("first claim must win") };
        assert!(!wake_running_copy(&name, Duration::from_millis(100)), "nobody is listening");
    }

    #[test]
    fn asking_nothing_reports_false() {
        assert!(!wake_running_copy(&unique(), Duration::from_millis(10)));
        assert!(!ask_to_quit(&unique()));
    }

    #[test]
    fn failures_are_described() {
        let error = windows::core::Error::from_hresult(windows::core::HRESULT(-2_147_024_891));
        assert_eq!(failed("x", &error).code(), "desktop.file");
        assert_eq!(
            event(&HSTRING::from("Local\\bad\\name\\x"), "y").unwrap_err().code(),
            "desktop.file"
        );
    }
}
