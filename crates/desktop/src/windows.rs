//! The real shell: `.lnk` files through the Windows Shell Link COM object.
#![allow(unsafe_code, reason = "COM calls")]

use std::path::Path;

use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoUninitialize, IPersistFile,
};
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
use windows::core::{HSTRING, Interface};

use crate::shell::{DesktopError, Shell, Shortcut};

/// Writes shortcuts with the Windows Shell.
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsShell;

/// Keeps COM initialized on this thread for the guard's lifetime.
struct ComScope {
    owned: bool,
}

impl ComScope {
    fn enter() -> Result<Self, String> {
        // SAFETY: plain COM initialization; balanced by CoUninitialize in Drop when we own it.
        let result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if result == RPC_E_CHANGED_MODE {
            // COM already runs in another mode on this thread; the Shell link works there too.
            return Ok(Self { owned: false });
        }
        result.ok().map(|()| Self { owned: true }).map_err(|error| error.message())
    }
}

impl Drop for ComScope {
    fn drop(&mut self) {
        if self.owned {
            // SAFETY: balances the successful CoInitializeEx of `enter`.
            unsafe { CoUninitialize() };
        }
    }
}

fn text(path: &Path) -> HSTRING {
    HSTRING::from(path.as_os_str())
}

fn write_link(shortcut: &Shortcut) -> windows::core::Result<()> {
    // SAFETY: COM calls on an object we created; every string argument outlives its call.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(&text(&shortcut.target))?;
        link.SetArguments(&HSTRING::from(shortcut.arguments.as_str()))?;
        link.SetWorkingDirectory(&text(&shortcut.working_dir))?;
        link.SetIconLocation(&text(&shortcut.icon), shortcut.icon_index)?;
        link.cast::<IPersistFile>()?.Save(&text(&shortcut.path), true)
    }
}

impl Shell for WindowsShell {
    fn create_shortcut(&self, shortcut: &Shortcut) -> Result<(), DesktopError> {
        let fail = |reason: String| DesktopError::Shortcut { path: shortcut.path.clone(), reason };
        let _com = ComScope::enter().map_err(fail)?;
        write_link(shortcut).map_err(|error| fail(error.message()))
    }
}

/// The primary screen's area outside the taskbar, in physical pixels: (left, top, right, bottom).
#[must_use]
pub fn work_area() -> Option<(i32, i32, i32, i32)> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{
        SPI_GETWORKAREA, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
    };
    let mut area = RECT::default();
    // SAFETY: Windows writes one RECT into `area`, which lives for the call.
    unsafe {
        SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some((&raw mut area).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }
    .ok()
    .map(|()| (area.left, area.top, area.right, area.bottom))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use windows::Win32::Storage::FileSystem::WIN32_FIND_DATAW;
    use windows::Win32::System::Com::STGM_READ;

    use super::*;

    fn wide_text(buffer: &[u16]) -> String {
        let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
        String::from_utf16_lossy(&buffer[..end])
    }

    /// Reads a shortcut back through the Shell.
    fn read_link(path: &Path) -> Shortcut {
        let _com = ComScope::enter().unwrap();
        // SAFETY: COM calls on an object we created; buffers are local and sized for the calls.
        unsafe {
            let link: IShellLinkW =
                CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).unwrap();
            link.cast::<IPersistFile>().unwrap().Load(&text(path), STGM_READ).unwrap();
            let mut target = [0u16; 1024];
            let mut find = WIN32_FIND_DATAW::default();
            link.GetPath(&mut target, &raw mut find, 0).unwrap();
            let mut arguments = [0u16; 1024];
            link.GetArguments(&mut arguments).unwrap();
            let mut working = [0u16; 1024];
            link.GetWorkingDirectory(&mut working).unwrap();
            let mut icon = [0u16; 1024];
            let mut index = 0;
            link.GetIconLocation(&mut icon, &raw mut index).unwrap();
            Shortcut {
                path: path.to_path_buf(),
                target: PathBuf::from(wide_text(&target)),
                arguments: wide_text(&arguments),
                working_dir: PathBuf::from(wide_text(&working)),
                icon: PathBuf::from(wide_text(&icon)),
                icon_index: index,
            }
        }
    }

    #[test]
    fn writes_a_shortcut_windows_can_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("Some App.exe");
        std::fs::write(&target, b"x").unwrap();
        let shortcut = Shortcut {
            path: dir.path().join("Antigravity - work.lnk"),
            target: target.clone(),
            arguments: "--launch \"work\"".into(),
            working_dir: dir.path().to_path_buf(),
            icon: target.clone(),
            icon_index: 0,
        };
        WindowsShell.create_shortcut(&shortcut).unwrap();
        assert_eq!(read_link(&shortcut.path), shortcut);

        // Replacing works too.
        let replaced = Shortcut { arguments: "--launch \"other\"".into(), ..shortcut.clone() };
        WindowsShell.create_shortcut(&replaced).unwrap();
        assert_eq!(read_link(&shortcut.path).arguments, "--launch \"other\"");
    }

    #[test]
    fn reports_a_folder_that_does_not_exist() {
        let dir = tempfile::tempdir().unwrap();
        let shortcut = Shortcut {
            path: dir.path().join("missing").join("x.lnk"),
            target: dir.path().join("x.exe"),
            arguments: String::new(),
            working_dir: dir.path().to_path_buf(),
            icon: dir.path().join("x.exe"),
            icon_index: 0,
        };
        let error = WindowsShell.create_shortcut(&shortcut).unwrap_err();
        assert_eq!(error.code(), "desktop.shortcut");
    }

    #[test]
    fn the_work_area_is_a_real_rectangle() {
        let (left, top, right, bottom) = work_area().unwrap();
        assert!(right > left && bottom > top, "{left},{top},{right},{bottom}");
    }

    #[test]
    fn com_can_be_entered_twice_on_one_thread() {
        let outer = ComScope::enter().unwrap();
        let inner = ComScope::enter().unwrap();
        drop(inner);
        drop(outer);
    }
}
