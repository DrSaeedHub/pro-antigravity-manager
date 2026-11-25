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

/// Shows why the app did not start (`code`, with `detail` when it helps), in the small launch
/// window, and waits until it is closed.
pub fn show_problem(code: &str, detail: &str) {
    use slint::ComponentHandle;

    let Ok(window) = crate::ui::LaunchWindow::new() else { return };
    window.set_account("Antigravity".into());
    window.set_problem_code(code.into());
    window.set_problem_detail(detail.into());
    window.set_stage(crate::ui::LaunchStage::Failed);
    window.on_cancel(|| {
        let _ = slint::quit_event_loop();
    });
    let _ = window.run();
}

/// Minimizes `window` to the taskbar, however often it was minimized and restored before.
pub fn minimize(window: &slint::Window) {
    use slint::winit_030::WinitWindowAccessor;
    window.with_winit_window(|winit_window| winit_window.set_minimized(true));
}

/// Brings `window` to the front, restored if it was minimized.
pub fn bring_to_front(window: &slint::Window) {
    use slint::winit_030::WinitWindowAccessor;
    window.with_winit_window(|winit_window| {
        winit_window.set_minimized(false);
        winit_window.focus_window();
    });
}

/// Puts `window` in the middle of the primary screen's work area.
pub fn place_center(window: &slint::Window) {
    #[cfg(windows)]
    if let Some((left, top, right, bottom)) = desktop::work_area() {
        let size = window.size();
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_possible_wrap,
            reason = "screen pixels"
        )]
        let (width, height) = (size.width as i32, size.height as i32);
        window.set_position(slint::PhysicalPosition::new(
            left + (right - left - width) / 2,
            top + (bottom - top - height) / 2,
        ));
    }
    #[cfg(not(windows))]
    let _ = window;
}

/// A rectangle on screen in physical pixels: (left, top, width, height).
pub type Frame = (i32, i32, i32, i32);
/// A screen's area outside the taskbar in physical pixels: (left, top, right, bottom).
pub type Area = (i32, i32, i32, i32);

/// `frame` moved (and if it must, shrunk) so it lies wholly inside `area`.
#[must_use]
pub fn fit(frame: Frame, area: Area) -> Frame {
    let (left, top, right, bottom) = area;
    let width = frame.2.min(right - left).max(1);
    let height = frame.3.min(bottom - top).max(1);
    let x = frame.0.clamp(left, right - width);
    let y = frame.1.clamp(top, bottom - height);
    (x, y, width, height)
}

/// Where a corner notice of `width` × `height` goes in `area`: bottom right, `gap` from the edges,
/// and never outside the area.
#[must_use]
pub fn bottom_right(width: i32, height: i32, gap: i32, area: Area) -> Frame {
    let (_, _, right, bottom) = area;
    fit((right - width - gap, bottom - height - gap, width, height), area)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap, reason = "screen pixels")]
fn frame_of(window: &slint::Window) -> Frame {
    let (position, size) = (window.position(), window.size());
    (position.x, position.y, size.width as i32, size.height as i32)
}

#[allow(clippy::cast_sign_loss, reason = "fit keeps sizes positive")]
fn apply(window: &slint::Window, before: Frame, after: Frame) {
    if (after.2, after.3) != (before.2, before.3) {
        window.set_size(slint::PhysicalSize::new(after.2 as u32, after.3 as u32));
    }
    if (after.0, after.1) != (before.0, before.1) {
        window.set_position(slint::PhysicalPosition::new(after.0, after.1));
    }
}

/// Moves `window` back inside the screen it is on (shrinking it if it is larger), when any part
/// of it lies outside the area above the taskbar.
pub fn keep_on_screen(window: &slint::Window) {
    #[cfg(windows)]
    {
        let frame = frame_of(window);
        if let Some(area) = desktop::work_area_near(frame.0 + frame.2 / 2, frame.1 + frame.3 / 2) {
            apply(window, frame, fit(frame, area));
        }
    }
    #[cfg(not(windows))]
    let _ = window;
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
    if let Some(area) = desktop::work_area() {
        let frame = frame_of(window);
        #[allow(clippy::cast_possible_truncation, reason = "a few pixels")]
        let gap = (16.0 * window.scale_factor()).round() as i32;
        apply(window, frame, bottom_right(frame.2, frame.3, gap, area));
    }
}

/// Keeps a corner notice in the bottom-right corner whenever its size or scale changes: the size
/// it is placed with first can still change once Windows applies the screen's scale, and a notice
/// placed with the old size would hang off the bottom of the screen.
pub fn pin_bottom_right(window: &slint::Window) {
    use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
    window.on_winit_window_event(|window, event| {
        if matches!(
            event,
            winit::event::WindowEvent::Resized(_)
                | winit::event::WindowEvent::ScaleFactorChanged { .. }
        ) {
            place_bottom_right(window);
        }
        EventResult::Propagate
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Area = (0, 0, 2560, 1392);

    #[test]
    fn a_frame_inside_stays_where_it_is() {
        assert_eq!(fit((100, 100, 800, 600), SCREEN), (100, 100, 800, 600));
    }

    #[test]
    fn a_frame_hanging_off_an_edge_comes_back() {
        assert_eq!(fit((2000, 1200, 860, 392), SCREEN), (1700, 1000, 860, 392));
        assert_eq!(fit((-50, -20, 800, 600), SCREEN), (0, 0, 800, 600));
        assert_eq!(fit((3000, 100, 800, 600), (2560, 0, 4480, 1040)), (3000, 100, 800, 600));
        assert_eq!(fit((5000, 100, 800, 600), (2560, 0, 4480, 1040)), (3680, 100, 800, 600));
    }

    #[test]
    fn a_frame_larger_than_the_screen_shrinks_to_it() {
        assert_eq!(fit((0, 100, 1770, 1170), (0, 0, 1920, 1032)), (0, 0, 1770, 1032));
    }

    #[test]
    fn a_notice_sits_above_the_taskbar_in_the_corner() {
        assert_eq!(bottom_right(645, 294, 24, SCREEN), (1891, 1074, 645, 294));
        // A taskbar on the left, a screen to the right of another.
        assert_eq!(bottom_right(430, 196, 16, (2620, 0, 4480, 1080)), (4034, 868, 430, 196));
        assert_eq!(bottom_right(3000, 196, 16, SCREEN), (0, 1180, 2560, 196), "never wider");
    }
}
