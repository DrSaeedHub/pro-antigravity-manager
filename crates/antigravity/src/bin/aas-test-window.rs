//! A tiny visible window used only by the Windows host tests.
//!
//! `aas-test-window` closes when asked; `aas-test-window stubborn` ignores close requests.
#![allow(unsafe_code, reason = "Win32 calls")]
#![windows_subsystem = "windows"]

#[cfg(windows)]
mod imp {
    use std::sync::atomic::{AtomicBool, Ordering};

    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
        GetMessageW, MSG, PostQuitMessage, RegisterClassW, TranslateMessage, WINDOW_EX_STYLE,
        WM_CLOSE, WM_DESTROY, WNDCLASSW, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
    };
    use windows::core::w;

    static STUBBORN: AtomicBool = AtomicBool::new(false);

    unsafe extern "system" fn procedure(
        window: HWND,
        message: u32,
        w: WPARAM,
        l: LPARAM,
    ) -> LRESULT {
        // SAFETY: standard window-procedure calls on the window Windows passed in.
        unsafe {
            match message {
                WM_CLOSE if STUBBORN.load(Ordering::Relaxed) => LRESULT(0),
                WM_CLOSE => {
                    let _ = DestroyWindow(window);
                    LRESULT(0)
                }
                WM_DESTROY => {
                    PostQuitMessage(0);
                    LRESULT(0)
                }
                _ => DefWindowProcW(window, message, w, l),
            }
        }
    }

    pub fn run() {
        STUBBORN.store(std::env::args().nth(1).as_deref() == Some("stubborn"), Ordering::Relaxed);
        // SAFETY: a minimal Win32 window and message loop; every pointer is local and valid.
        unsafe {
            let Ok(module) = GetModuleHandleW(None) else { return };
            let class = WNDCLASSW {
                lpfnWndProc: Some(procedure),
                hInstance: module.into(),
                lpszClassName: w!("AasTestWindow"),
                ..WNDCLASSW::default()
            };
            RegisterClassW(&raw const class);
            let created = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("AasTestWindow"),
                w!("aas test window"),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                200,
                120,
                None,
                None,
                Some(module.into()),
                None,
            );
            if created.is_err() {
                return;
            }
            let mut message = MSG::default();
            while GetMessageW(&raw mut message, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&raw const message);
                DispatchMessageW(&raw const message);
            }
        }
    }
}

fn main() {
    #[cfg(windows)]
    imp::run();
}
