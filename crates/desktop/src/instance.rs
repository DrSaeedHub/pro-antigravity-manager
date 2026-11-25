//! One running copy per Windows user, and a second start that brings it back instead of opening
//! another.
//!
//! A named mutex marks the running copy, and a small named shared-memory block says which process
//! it is (its id and start time). A second start asks it, through a named event, to show its
//! window, lets it come to the front, and waits for its answer. A copy that is alive but does not
//! answer is waited for and then left alone: a second copy never opens. Only a holder that is gone
//! (it ended, or hangs while ending, which Windows can leave behind holding the lock) is replaced:
//! the new start takes the lock over and answers from then on. The installer asks the running copy
//! to quit through another event.
//!
//! Names, for `<name>`: `Local\<name>.instance` (the lock), `Local\<name>.owner` (who holds it),
//! `Local\<name>.show`, `Local\<name>.shown` (the answer) and `Local\<name>.quit`. The installer
//! script uses the lock and quit names too.
#![allow(unsafe_code, reason = "Win32 calls")]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, FILETIME, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
    WAIT_OBJECT_0,
};
use windows::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_READ, FILE_MAP_WRITE, MapViewOfFile, OpenFileMappingW,
    PAGE_READWRITE, UnmapViewOfFile,
};
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, GetCurrentProcess, GetCurrentProcessId,
    GetExitCodeProcess, GetProcessTimes, OpenEventW, OpenProcess,
    PROCESS_QUERY_LIMITED_INFORMATION, SYNCHRONIZATION_SYNCHRONIZE, SetEvent,
    WaitForMultipleObjects, WaitForSingleObject,
};
use windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;
use windows::core::HSTRING;

use crate::shell::DesktopError;

/// What `GetExitCodeProcess` reports for a process that has not begun to end.
const STILL_ACTIVE: u32 = 259;
/// The owner block: process id, then its start time.
const OWNER_BYTES: usize = 16;

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

/// A process, told apart from a later one that reuses its id by its start time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Process {
    /// The process id.
    pub id: u32,
    /// When it started (a `FILETIME` as one number).
    pub started: u64,
}

impl Process {
    /// This process.
    #[must_use]
    pub fn current() -> Self {
        // SAFETY: the pseudo-handle of the current process needs no closing.
        let started = unsafe { started_at(GetCurrentProcess()) };
        // SAFETY: a plain query.
        let id = unsafe { GetCurrentProcessId() };
        Self { id, started: started.unwrap_or_default() }
    }

    /// Whether this process still runs and has not begun to end.
    #[must_use]
    pub fn is_alive(&self) -> bool {
        // SAFETY: the handle is owned and closed by `Owned`.
        let Ok(handle) =
            (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, self.id) })
        else {
            return false;
        };
        let handle = Owned(handle);
        let mut code = 0u32;
        // SAFETY: the handle is open; `code` lives for the call.
        let running =
            unsafe { GetExitCodeProcess(handle.0, &raw mut code) }.is_ok() && code == STILL_ACTIVE;
        // SAFETY: as above.
        running && unsafe { started_at(handle.0) } == Some(self.started)
    }
}

/// When the process behind `handle` started.
///
/// # Safety
/// `handle` must be an open process handle with query rights.
unsafe fn started_at(handle: HANDLE) -> Option<u64> {
    let (mut created, mut ended, mut kernel, mut user) =
        (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    // SAFETY: the caller passes an open handle; the four structs live for the call.
    unsafe {
        GetProcessTimes(handle, &raw mut created, &raw mut ended, &raw mut kernel, &raw mut user)
    }
    .ok()?;
    Some((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}

/// Proof that this process is the running copy. Keep it alive for the app's lifetime.
#[derive(Debug)]
pub struct RunningCopy {
    lock: Mutex<Option<Owned>>,
    owner: Owned,
    show: Owned,
    answer: Owned,
    quit: Owned,
    retired: AtomicBool,
}

/// The result of trying to become the running copy.
#[derive(Debug)]
pub enum Claim {
    /// This process is the running copy.
    First(RunningCopy),
    /// Another copy already holds the lock.
    AlreadyRunning,
}

/// What a second start found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reached {
    /// The running copy answered (it shows its window), or, for a quiet start, it is alive.
    Running,
    /// A running copy is alive but did not answer in time (it hangs). Start nothing.
    Unresponsive,
    /// The lock's holder is gone or ending: take the lock over.
    Gone,
}

struct Names {
    lock: HSTRING,
    owner: HSTRING,
    show: HSTRING,
    answer: HSTRING,
    quit: HSTRING,
}

fn names(name: &str) -> Names {
    Names {
        lock: HSTRING::from(format!("Local\\{name}.instance")),
        owner: HSTRING::from(format!("Local\\{name}.owner")),
        show: HSTRING::from(format!("Local\\{name}.show")),
        answer: HSTRING::from(format!("Local\\{name}.shown")),
        quit: HSTRING::from(format!("Local\\{name}.quit")),
    }
}

fn failed(action: &'static str, error: &windows::core::Error) -> DesktopError {
    DesktopError::File { action, path: "single instance".into(), reason: error.message() }
}

fn event(name: &HSTRING, action: &'static str) -> Result<Owned, DesktopError> {
    // SAFETY: creates or opens a named event; the handle is owned.
    unsafe { CreateEventW(None, false, false, name) }
        .map(Owned)
        .map_err(|error| failed(action, &error))
}

/// Writes `process` into the owner block named `name` (created if missing). Zero means nobody.
fn write_owner(name: &HSTRING, process: Option<Process>) -> Result<Owned, DesktopError> {
    let size = u32::try_from(OWNER_BYTES).unwrap_or(16);
    // SAFETY: creates or opens a page-file-backed section of OWNER_BYTES; the handle is owned.
    let section =
        unsafe { CreateFileMappingW(INVALID_HANDLE_VALUE, None, PAGE_READWRITE, 0, size, name) }
            .map(Owned)
            .map_err(|error| failed("creating the owner record", &error))?;
    write_owner_on(&section, process)?;
    Ok(section)
}

/// The process recorded as the running copy named `name`, if any.
#[must_use]
pub fn owner(name: &str) -> Option<Process> {
    let names = names(name);
    // SAFETY: opens the section read-only, copies OWNER_BYTES out of the view and unmaps it; the
    // section handle is owned.
    unsafe {
        let section = Owned(OpenFileMappingW(FILE_MAP_READ.0, false, &names.owner).ok()?);
        let view = MapViewOfFile(section.0, FILE_MAP_READ, 0, 0, OWNER_BYTES);
        if view.Value.is_null() {
            return None;
        }
        let mut bytes = [0u8; OWNER_BYTES];
        std::ptr::copy_nonoverlapping(view.Value.cast::<u8>(), bytes.as_mut_ptr(), OWNER_BYTES);
        let _ = UnmapViewOfFile(view);
        let id = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let mut started = [0u8; 8];
        started.copy_from_slice(&bytes[8..]);
        (id != 0).then(|| Process { id, started: u64::from_le_bytes(started) })
    }
}

/// Opens (or creates) every named object and records this process as the owner.
fn become_owner(names: &Names, lock: Owned) -> Result<RunningCopy, DesktopError> {
    Ok(RunningCopy {
        lock: Mutex::new(Some(lock)),
        owner: write_owner(&names.owner, Some(Process::current()))?,
        show: event(&names.show, "creating the show event")?,
        answer: event(&names.answer, "creating the answer event")?,
        quit: event(&names.quit, "creating the quit event")?,
        retired: AtomicBool::new(false),
    })
}

fn open_lock(names: &Names) -> Result<(Owned, bool), DesktopError> {
    // SAFETY: creates or opens a named mutex; the handle is owned.
    unsafe {
        let mutex = CreateMutexW(None, false, &names.lock)
            .map_err(|e| failed("creating the instance lock", &e))?;
        Ok((Owned(mutex), GetLastError() == ERROR_ALREADY_EXISTS))
    }
}

/// Becomes the running copy named `name`, unless one already holds the lock.
pub fn claim(name: &str) -> Result<Claim, DesktopError> {
    let names = names(name);
    let (lock, already) = open_lock(&names)?;
    if already {
        return Ok(Claim::AlreadyRunning);
    }
    become_owner(&names, lock).map(Claim::First)
}

/// Becomes the running copy named `name` although another process holds the lock: use it only
/// when [`reach`] said the holder is gone. From now on this process answers second starts.
pub fn take_over(name: &str) -> Result<RunningCopy, DesktopError> {
    let names = names(name);
    let (lock, _) = open_lock(&names)?;
    become_owner(&names, lock)
}

impl RunningCopy {
    /// Waits up to `limit` for another process to ask something, and answers a request to show
    /// the window. `None` when nothing was asked, or once this copy has been released.
    #[must_use]
    pub fn wait(&self, limit: Duration) -> Option<Request> {
        let millis = u32::try_from(limit.as_millis()).unwrap_or(u32::MAX);
        let handles = [self.show.0, self.quit.0];
        // SAFETY: the event handles stay open while `self` lives.
        let result = unsafe { WaitForMultipleObjects(&handles, false, millis) };
        if self.retired.load(Ordering::Acquire) {
            return None;
        }
        if result == WAIT_OBJECT_0 {
            // SAFETY: as above.
            let _ = unsafe { SetEvent(self.answer.0) };
            Some(Request::Show)
        } else if result.0 == WAIT_OBJECT_0.0 + 1 {
            Some(Request::Quit)
        } else {
            None
        }
    }

    /// Gives up being the running copy (the updated app is about to start): the lock is let go,
    /// the owner record cleared, and requests are no longer answered.
    pub fn release(&self) {
        self.retired.store(true, Ordering::Release);
        let _ = write_owner_on(&self.owner, None);
        self.lock.lock().unwrap_or_else(PoisonError::into_inner).take();
    }
}

/// Clears or rewrites the owner block through an open section handle.
fn write_owner_on(section: &Owned, process: Option<Process>) -> Result<(), DesktopError> {
    // SAFETY: maps the open section for writing, writes OWNER_BYTES and unmaps it.
    unsafe {
        let view = MapViewOfFile(section.0, FILE_MAP_WRITE, 0, 0, OWNER_BYTES);
        if view.Value.is_null() {
            return Err(failed("writing the owner record", &windows::core::Error::from_thread()));
        }
        let process = process.unwrap_or(Process { id: 0, started: 0 });
        let mut bytes = [0u8; OWNER_BYTES];
        bytes[..4].copy_from_slice(&process.id.to_le_bytes());
        bytes[8..].copy_from_slice(&process.started.to_le_bytes());
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), view.Value.cast::<u8>(), OWNER_BYTES);
        let _ = UnmapViewOfFile(view);
    }
    Ok(())
}

fn signal(name: &HSTRING) -> Option<Owned> {
    // SAFETY: opening a named event; `Owned` closes it.
    let event = unsafe { OpenEventW(EVENT_MODIFY_STATE, false, name) }.ok().map(Owned)?;
    // SAFETY: the handle is open.
    unsafe { SetEvent(event.0) }.ok().map(|()| event)
}

/// Asks the running copy named `name` to show its window, lets it come to the front, and waits up
/// to `answer_within` for its answer. False when nobody answers.
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
    // This start was launched by the person, so it may hand the foreground on: without that,
    // Windows would only flash the running copy's taskbar button.
    let allowed = owner(name).map_or(u32::MAX, |process| process.id);
    // SAFETY: a plain call.
    let _ = unsafe { AllowSetForegroundWindow(allowed) };
    if signal(&names.show).is_none() {
        return false;
    }
    // SAFETY: the handle is open.
    unsafe { WaitForSingleObject(answer.0, millis) == WAIT_OBJECT_0 }
}

/// After [`claim`] found the lock taken: brings the running copy's window back (a quiet start
/// only checks that it is alive), and tells whether to start nothing or to take the lock over.
/// A live copy that does not answer is asked again until `patience` runs out.
#[must_use]
pub fn reach(name: &str, quiet: bool, answer_within: Duration, patience: Duration) -> Reached {
    let started = Instant::now();
    loop {
        match owner(name) {
            Some(process) if !process.is_alive() => return Reached::Gone,
            Some(_) if quiet => return Reached::Running,
            Some(_) => {
                if wake_running_copy(name, answer_within) {
                    return Reached::Running;
                }
                if started.elapsed() >= patience {
                    return Reached::Unresponsive;
                }
            }
            // No owner record: a copy older than these records, or one that let go of the lock.
            None => {
                return if wake_running_copy(name, answer_within) {
                    Reached::Running
                } else {
                    Reached::Gone
                };
            }
        }
    }
}

/// Asks the running copy named `name` to quit. False when no copy listens.
#[must_use]
pub fn ask_to_quit(name: &str) -> bool {
    signal(&names(name).quit).is_some()
}

#[cfg(test)]
mod tests {
    use std::os::windows::io::AsRawHandle;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn unique() -> String {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        format!("aas-test-instance-{}-{nanos}", std::process::id())
    }

    const SOON: Duration = Duration::from_millis(150);

    /// A process that has ended, as an owner record would name it.
    fn ended_process() -> Process {
        let mut child = std::process::Command::new("cmd").args(["/c", "exit"]).spawn().unwrap();
        child.wait().unwrap();
        // SAFETY: the child's handle stays open until `child` drops.
        let started = unsafe { started_at(HANDLE(child.as_raw_handle())) }.unwrap();
        Process { id: child.id(), started }
    }

    #[test]
    fn the_first_claim_wins_records_itself_and_a_second_one_sees_it() {
        let name = unique();
        let Claim::First(running) = claim(&name).unwrap() else { panic!("first claim must win") };
        assert_eq!(owner(&name), Some(Process::current()));
        assert!(Process::current().is_alive());
        assert!(matches!(claim(&name).unwrap(), Claim::AlreadyRunning));
        drop(running);
        assert_eq!(owner(&name), None, "the record goes with the last handle");
        assert!(matches!(claim(&name).unwrap(), Claim::First(_)), "free again once dropped");
    }

    #[test]
    fn a_second_start_wakes_the_running_copy_and_gets_an_answer() {
        let name = unique();
        let Claim::First(running) = claim(&name).unwrap() else { panic!("first claim must win") };
        assert_eq!(running.wait(Duration::from_millis(10)), None, "nobody asked yet");
        let listener = std::thread::spawn(move || running.wait(Duration::from_secs(5)));
        assert_eq!(reach(&name, false, Duration::from_secs(5), SOON), Reached::Running);
        assert_eq!(listener.join().unwrap(), Some(Request::Show));
    }

    #[test]
    fn a_quiet_start_only_checks_the_running_copy_is_alive() {
        let name = unique();
        let Claim::First(_running) = claim(&name).unwrap() else { panic!("first claim must win") };
        assert_eq!(reach(&name, true, SOON, SOON), Reached::Running);
    }

    #[test]
    fn a_live_copy_that_does_not_answer_is_left_alone() {
        let name = unique();
        let Claim::First(_running) = claim(&name).unwrap() else { panic!("first claim must win") };
        assert!(!wake_running_copy(&name, Duration::from_millis(100)), "nobody is listening");
        let started = Instant::now();
        assert_eq!(reach(&name, false, SOON, Duration::from_millis(400)), Reached::Unresponsive);
        assert!(started.elapsed() >= Duration::from_millis(400), "it kept asking");
    }

    #[test]
    fn a_holder_that_is_gone_is_replaced_and_the_new_copy_answers() {
        let name = unique();
        // A copy that ended (or hangs while ending) but still holds the lock and its record.
        let (stale_lock, _) = open_lock(&names(&name)).unwrap();
        let stale_record = write_owner(&names(&name).owner, Some(ended_process())).unwrap();
        assert!(matches!(claim(&name).unwrap(), Claim::AlreadyRunning));
        assert_eq!(reach(&name, false, SOON, Duration::from_secs(5)), Reached::Gone);
        assert_eq!(reach(&name, true, SOON, SOON), Reached::Gone, "a quiet start too");

        let running = Arc::new(take_over(&name).unwrap());
        assert_eq!(owner(&name), Some(Process::current()));
        let listening = running.clone();
        let listener = std::thread::spawn(move || listening.wait(Duration::from_secs(5)));
        assert_eq!(reach(&name, false, Duration::from_secs(5), SOON), Reached::Running);
        assert_eq!(listener.join().unwrap(), Some(Request::Show));
        drop((stale_lock, stale_record));
    }

    #[test]
    fn a_holder_without_a_record_is_replaced_only_when_it_does_not_answer() {
        let name = unique();
        let (old_lock, _) = open_lock(&names(&name)).unwrap();
        assert_eq!(owner(&name), None);
        assert_eq!(reach(&name, false, SOON, SOON), Reached::Gone, "nobody answers");
        let show = event(&names(&name).show, "x").unwrap();
        let answer = event(&names(&name).answer, "x").unwrap();
        let pair = (show, answer);
        let replier = std::thread::spawn(move || {
            let (show, answer) = pair;
            // SAFETY: both handles are open for the thread's life.
            unsafe {
                if WaitForSingleObject(show.0, 5000) == WAIT_OBJECT_0 {
                    let _ = SetEvent(answer.0);
                }
            }
        });
        assert_eq!(reach(&name, false, Duration::from_secs(5), SOON), Reached::Running);
        replier.join().unwrap();
        drop(old_lock);
    }

    #[test]
    fn a_released_copy_lets_the_next_one_start_and_stops_answering() {
        let name = unique();
        let Claim::First(running) = claim(&name).unwrap() else { panic!("first claim must win") };
        running.release();
        running.release();
        assert_eq!(owner(&name), None, "the record is cleared");
        let Claim::First(next) = claim(&name).unwrap() else { panic!("the lock was released") };
        drop(next);
        let running = Arc::new(running);
        let listening = running.clone();
        let listener = std::thread::spawn(move || listening.wait(Duration::from_secs(5)));
        assert!(!wake_running_copy(&name, Duration::from_millis(300)), "it no longer answers");
        assert_eq!(listener.join().unwrap(), None);
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
    fn processes_are_alive_only_until_they_end() {
        assert!(!ended_process().is_alive());
        let now = Process::current();
        assert!(!Process { started: now.started + 1, ..now }.is_alive(), "another start time");
        assert!(!Process { id: 0, started: 0 }.is_alive());
    }

    #[test]
    fn asking_nothing_reports_false() {
        assert!(!wake_running_copy(&unique(), Duration::from_millis(10)));
        assert!(!ask_to_quit(&unique()));
        assert_eq!(owner(&unique()), None);
    }

    #[test]
    fn failures_are_described() {
        let error = windows::core::Error::from_hresult(windows::core::HRESULT(-2_147_024_891));
        assert_eq!(failed("x", &error).code(), "desktop.file");
        let bad = HSTRING::from("Local\\bad\\name\\x");
        assert_eq!(event(&bad, "y").unwrap_err().code(), "desktop.file");
        assert_eq!(write_owner(&bad, None).unwrap_err().code(), "desktop.file");
    }
}
