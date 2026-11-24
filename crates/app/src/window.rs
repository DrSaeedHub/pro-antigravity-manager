//! Window-system setup: the winit backend with a frameless window that keeps the Windows
//! shadow and rounded corners.

use slint::winit_030::winit;

/// Selects the winit backend and prepares every window it creates.
pub fn select_backend() -> Result<(), slint::PlatformError> {
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .with_winit_window_attributes_hook(decorate)
        .select()
}

/// Frameless, but with the system drop shadow and Windows 11 rounded corners.
fn decorate(attributes: winit::window::WindowAttributes) -> winit::window::WindowAttributes {
    #[cfg(windows)]
    {
        use winit::platform::windows::{CornerPreference, WindowAttributesExtWindows};
        attributes.with_undecorated_shadow(true).with_corner_preference(CornerPreference::Round)
    }
    #[cfg(not(windows))]
    {
        attributes
    }
}

/// Shows why the app could not start, in the small launch window, and waits until it is closed.
pub fn show_startup_problem(detail: &str) {
    use slint::ComponentHandle;

    let Ok(window) = crate::ui::LaunchWindow::new() else { return };
    window.set_account("Antigravity".into());
    window.set_problem_code("app.startup".into());
    window.set_problem_detail(detail.into());
    window.set_stage(crate::ui::LaunchStage::Failed);
    window.on_cancel(|| {
        let _ = slint::quit_event_loop();
    });
    let _ = window.run();
}

/// Brings `window` to the front, restored if it was minimized.
pub fn bring_to_front(window: &slint::Window) {
    use slint::winit_030::WinitWindowAccessor;
    window.with_winit_window(|winit_window| {
        winit_window.set_minimized(false);
        winit_window.focus_window();
    });
}

/// Puts `window` in the bottom-right corner of the primary screen, above the taskbar, and keeps
/// it off the taskbar (it is a notice, not an app window).
pub fn place_bottom_right(window: &slint::Window) {
    #[cfg(windows)]
    {
        use slint::winit_030::WinitWindowAccessor;
        use slint::winit_030::winit::platform::windows::WindowExtWindows;
        window.with_winit_window(|winit_window| winit_window.set_skip_taskbar(true));
    }
    #[cfg(windows)]
    if let Some((_, _, right, bottom)) = desktop::work_area() {
        let size = window.size();
        let gap = (16.0 * window.scale_factor()).round();
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_possible_wrap,
            reason = "screen pixels"
        )]
        let (x, y) =
            (right - size.width as i32 - gap as i32, bottom - size.height as i32 - gap as i32);
        window.set_position(slint::PhysicalPosition::new(x, y));
    }
}
