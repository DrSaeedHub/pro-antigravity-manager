//! The real shell: `.lnk` files through the Windows Shell Link COM object, web pages through
//! `ShellExecuteW`, and the version shown in Installed apps.
#![allow(unsafe_code, reason = "COM, Shell and registry calls")]

use std::path::Path;

use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoUninitialize, IPersistFile,
};
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
use windows::core::{HSTRING, Interface};

use crate::shell::{DesktopError, Shell, Shortcut, check_web_address};

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

    fn open(&self, address: &str) -> Result<(), DesktopError> {
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        check_web_address(address)?;
        let _com = ComScope::enter()
            .map_err(|reason| DesktopError::Open { target: address.to_owned(), reason })?;
        // SAFETY: every string argument outlives the call; ShellExecuteW returns a status code
        // in the shape of a handle, greater than 32 on success.
        let result = unsafe {
            ShellExecuteW(
                None,
                &HSTRING::from("open"),
                &HSTRING::from(address),
                None,
                None,
                SW_SHOWNORMAL,
            )
        };
        let status = result.0 as usize;
        if status > 32 {
            Ok(())
        } else {
            Err(DesktopError::Open {
                target: address.to_owned(),
                reason: format!("Windows answered {status}"),
            })
        }
    }
}

/// Where Windows lists installed apps for the current user.
const UNINSTALL_KEYS: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";

/// Sets the version Installed apps shows for the entry whose `InstallLocation` is `folder` (the
/// installer's entry), after the app updated itself there. False when there is no such entry.
#[must_use]
pub fn record_installed_version(folder: &Path, version: &str) -> bool {
    set_display_version(UNINSTALL_KEYS, folder, version)
}

fn same_folder(a: &str, b: &Path) -> bool {
    let trim = |text: &str| text.trim_end_matches(['\\', '/']).to_lowercase();
    trim(a) == trim(&b.to_string_lossy())
}

fn wide_value(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn set_display_version(root: &str, folder: &Path, version: &str) -> bool {
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_READ, REG_SZ, RRF_RT_REG_SZ, RegCloseKey, RegEnumKeyExW,
        RegGetValueW, RegOpenKeyExW, RegSetKeyValueW,
    };
    let mut keys = HKEY::default();
    // SAFETY: the key handle is closed below; every buffer is local and sized for its call.
    unsafe {
        if RegOpenKeyExW(HKEY_CURRENT_USER, &HSTRING::from(root), None, KEY_READ, &raw mut keys)
            .is_err()
        {
            return false;
        }
        let mut found = false;
        let mut index = 0;
        loop {
            let mut name = [0u16; 256];
            let mut length = 256u32;
            let listed = RegEnumKeyExW(
                keys,
                index,
                Some(windows::core::PWSTR(name.as_mut_ptr())),
                &raw mut length,
                None,
                None,
                None,
                None,
            );
            if listed.is_err() {
                break;
            }
            index += 1;
            let subkey = HSTRING::from_wide(&name[..length as usize]);
            let mut location = [0u16; 1024];
            let mut size = 2048u32;
            let read = RegGetValueW(
                keys,
                &subkey,
                &HSTRING::from("InstallLocation"),
                RRF_RT_REG_SZ,
                None,
                Some(location.as_mut_ptr().cast()),
                Some(&raw mut size),
            );
            if read.is_err() {
                continue;
            }
            let end = location.iter().position(|&c| c == 0).unwrap_or(location.len());
            if !same_folder(&String::from_utf16_lossy(&location[..end]), folder) {
                continue;
            }
            let wide = wide_value(version);
            let written = RegSetKeyValueW(
                keys,
                &subkey,
                &HSTRING::from("DisplayVersion"),
                REG_SZ.0,
                Some(wide.as_ptr().cast()),
                u32::try_from(wide.len() * 2).unwrap_or(0),
            );
            found |= written.is_ok();
        }
        let _ = RegCloseKey(keys);
        found
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
    fn something_that_is_not_a_web_address_is_never_opened() {
        let error = WindowsShell.open("C:/Windows/System32/calc.exe").unwrap_err();
        assert_eq!(error.code(), "desktop.open");
    }

    /// A throw-away key under the current user, deleted when dropped.
    struct TestKeys(String);

    impl TestKeys {
        fn new() -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos());
            Self(format!(r"Software\AntigravityAccountSwitcherTest-{nanos}\Uninstall"))
        }

        fn entry(&self, name: &str, location: &str, version: &str) {
            use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegSetKeyValueW};
            for (value, data) in [("InstallLocation", location), ("DisplayVersion", version)] {
                let wide = wide_value(data);
                // SAFETY: the strings and the buffer outlive the call.
                let result = unsafe {
                    RegSetKeyValueW(
                        HKEY_CURRENT_USER,
                        &HSTRING::from(format!(r"{}\{name}", self.0)),
                        &HSTRING::from(value),
                        REG_SZ.0,
                        Some(wide.as_ptr().cast()),
                        u32::try_from(wide.len() * 2).unwrap(),
                    )
                };
                assert!(result.is_ok());
            }
        }

        fn version(&self, name: &str) -> String {
            use windows::Win32::System::Registry::{
                HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW,
            };
            let mut data = [0u16; 64];
            let mut size = 128u32;
            // SAFETY: the buffer is local and its size is passed.
            let result = unsafe {
                RegGetValueW(
                    HKEY_CURRENT_USER,
                    &HSTRING::from(format!(r"{}\{name}", self.0)),
                    &HSTRING::from("DisplayVersion"),
                    RRF_RT_REG_SZ,
                    None,
                    Some(data.as_mut_ptr().cast()),
                    Some(&raw mut size),
                )
            };
            assert!(result.is_ok());
            wide_text(&data)
        }
    }

    impl Drop for TestKeys {
        fn drop(&mut self) {
            use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RegDeleteTreeW};
            let parent = self.0.trim_end_matches(r"\Uninstall");
            // SAFETY: deletes only the throw-away key this test created.
            unsafe {
                let _ = RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(parent));
            }
        }
    }

    #[test]
    fn the_installed_apps_entry_follows_an_update() {
        let keys = TestKeys::new();
        keys.entry("{other}_is1", r"C:\Other\", "3.0");
        keys.entry("{ours}_is1", r"C:\Users\x\AppData\Local\App\", "1.1.0");
        let folder = Path::new(r"C:\Users\X\AppData\Local\App");
        assert!(set_display_version(&keys.0, folder, "1.2.0"));
        assert_eq!(keys.version("{ours}_is1"), "1.2.0");
        assert_eq!(keys.version("{other}_is1"), "3.0", "other apps are left alone");
        assert!(!set_display_version(&keys.0, Path::new(r"C:\Nowhere"), "9"));
        assert!(!set_display_version(r"Software\AntigravityAccountSwitcherTest-none", folder, "9"));
        // The real list is read without changing anything: no entry is ours.
        assert!(!record_installed_version(Path::new(r"C:\Nowhere\At\All"), "9"));
    }

    #[test]
    fn folders_compare_without_case_or_trailing_slashes() {
        assert!(same_folder(r"C:\A\B\", Path::new(r"c:\a\b")));
        assert!(!same_folder(r"C:\A\B", Path::new(r"C:\A\BC")));
    }

    #[test]
    fn com_can_be_entered_twice_on_one_thread() {
        let outer = ComScope::enter().unwrap();
        let inner = ComScope::enter().unwrap();
        drop(inner);
        drop(outer);
    }
}
