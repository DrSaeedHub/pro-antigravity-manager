//! The real process host: Toolhelp snapshots, `WM_CLOSE` to main windows, `TerminateProcess`;
//! and the version an executable carries.
#![allow(unsafe_code, reason = "Win32 calls")]

use std::collections::HashSet;
use std::path::Path;
use std::process::{Command, Stdio};

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, WPARAM};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GetWindow, GetWindowThreadProcessId, IsWindowVisible, PostMessageW,
    WM_CLOSE,
};
use windows::core::BOOL;

use crate::host::{AntigravityError, AppHost};

/// The product version an executable or library carries in its version resource, as
/// `major.minor.patch` (Antigravity 2.35.1 says "2.35.1"). `None` when it has none.
#[must_use]
pub fn file_version(path: &Path) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;

    use windows::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VS_FIXEDFILEINFO, VerQueryValueW,
    };
    use windows::core::{PCWSTR, w};

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let name = PCWSTR(wide.as_ptr());
    // SAFETY: `name` is a NUL-terminated path that lives through the calls; `data` is as large as
    // Windows asked for, and the pointer VerQueryValueW gives back points into `data`, which is
    // still alive while it is read.
    unsafe {
        let size = GetFileVersionInfoSizeW(name, None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0_u8; usize::try_from(size).ok()?];
        GetFileVersionInfoW(name, None, size, data.as_mut_ptr().cast()).ok()?;
        let mut fixed: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut length = 0_u32;
        let found = VerQueryValueW(data.as_ptr().cast(), w!("\\"), &raw mut fixed, &raw mut length);
        if !found.as_bool()
            || fixed.is_null()
            || usize::try_from(length).ok()? < size_of::<VS_FIXEDFILEINFO>()
        {
            return None;
        }
        let info = fixed.cast::<VS_FIXEDFILEINFO>().read_unaligned();
        let (high, low) = (info.dwProductVersionMS, info.dwProductVersionLS);
        Some(format!("{}.{}.{}", high >> 16, high & 0xFFFF, low >> 16))
    }
}

/// Processes whose executable is `<name>.exe` (case-insensitive), like `Antigravity.exe`.
#[derive(Debug, Clone)]
pub struct WindowsHost {
    executable: String,
}

impl WindowsHost {
    /// The host for Antigravity itself.
    #[must_use]
    pub fn antigravity() -> Self {
        Self::named("Antigravity")
    }

    /// The host for processes of `<name>.exe`.
    #[must_use]
    pub fn named(name: &str) -> Self {
        Self { executable: format!("{name}.exe").to_lowercase() }
    }

    fn process_ids(&self) -> Vec<u32> {
        let mut ids = Vec::new();
        // SAFETY: the snapshot handle is closed below; the entry struct is initialized with its
        // size as Toolhelp requires, and its name buffer is NUL-terminated by Windows.
        unsafe {
            let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
                return ids;
            };
            let mut entry = PROCESSENTRY32W {
                dwSize: u32::try_from(size_of::<PROCESSENTRY32W>()).unwrap_or(u32::MAX),
                ..PROCESSENTRY32W::default()
            };
            let mut more = Process32FirstW(snapshot, &raw mut entry).is_ok();
            while more {
                let length =
                    entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
                let name = String::from_utf16_lossy(&entry.szExeFile[..length]);
                if name.to_lowercase() == self.executable {
                    ids.push(entry.th32ProcessID);
                }
                more = Process32NextW(snapshot, &raw mut entry).is_ok();
            }
            let _ = CloseHandle(snapshot);
        }
        ids
    }
}

struct CloseTargets {
    ids: HashSet<u32>,
}

unsafe extern "system" fn close_main_window(window: HWND, data: LPARAM) -> BOOL {
    // SAFETY: `data` is the address of the `CloseTargets` that `request_close` keeps alive for
    // the whole `EnumWindows` call.
    let targets = unsafe { &*(data.0 as *const CloseTargets) };
    let mut id = 0u32;
    // SAFETY: `window` comes from EnumWindows; the calls only read window properties or post a
    // message to it.
    unsafe {
        GetWindowThreadProcessId(window, Some(&raw mut id));
        let unowned = GetWindow(window, GW_OWNER).is_err();
        if targets.ids.contains(&id) && IsWindowVisible(window).as_bool() && unowned {
            let _ = PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
    BOOL(1)
}

impl AppHost for WindowsHost {
    fn running(&self) -> usize {
        self.process_ids().len()
    }

    fn request_close(&self) {
        let targets = CloseTargets { ids: self.process_ids().into_iter().collect() };
        if targets.ids.is_empty() {
            return;
        }
        // SAFETY: the callback only reads `targets`, which outlives this synchronous call.
        unsafe {
            let _ = EnumWindows(Some(close_main_window), LPARAM(&raw const targets as isize));
        }
    }

    fn kill(&self) {
        for id in self.process_ids() {
            // SAFETY: the handle is used for TerminateProcess only and closed right after.
            unsafe {
                if let Ok(handle) = OpenProcess(PROCESS_TERMINATE, false, id) {
                    let _ = TerminateProcess(handle, 1);
                    let _ = CloseHandle(handle);
                }
            }
        }
    }

    fn start(&self, exe: &Path) -> Result<(), AntigravityError> {
        let mut command = Command::new(exe);
        // Antigravity gets its own console streams, never ours.
        command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        if let Some(folder) = exe.parent() {
            command.current_dir(folder);
        }
        command.spawn().map(drop).map_err(|error| AntigravityError::StartFailed(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_version_windows_files_carry() {
        // Windows itself always carries a version (and GitHub's runners keep it in C:\Windows too).
        let version = file_version(Path::new(r"C:\Windows\System32\kernel32.dll")).unwrap();
        let parts: Vec<u32> = version.split('.').map(|part| part.parse().unwrap()).collect();
        assert_eq!(parts.len(), 3, "{version}");
        assert!(parts[0] >= 6, "{version}");
    }

    #[test]
    fn a_file_without_a_version_has_none() {
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("Antigravity.exe");
        std::fs::write(&plain, b"not a program").unwrap();
        assert_eq!(file_version(&plain), None);
        assert_eq!(file_version(&dir.path().join("missing.exe")), None);
    }
}
