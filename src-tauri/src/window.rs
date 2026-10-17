//! Native window look: blur behind the glass and rounded corners (Windows only).

use tauri::WebviewWindow;

pub fn apply_effects(window: &WebviewWindow) {
    #[cfg(windows)]
    {
        // Fully transparent tint: the page itself paints the colour and opacity.
        let _ = window_vibrancy::apply_blur(window, Some((0, 0, 0, 0)));
        round_corners(window);
    }
    #[cfg(not(windows))]
    let _ = window;
}

#[cfg(windows)]
#[link(name = "dwmapi")]
extern "system" {
    fn DwmSetWindowAttribute(
        hwnd: isize,
        attribute: u32,
        value: *const core::ffi::c_void,
        size: u32,
    ) -> i32;
}

#[cfg(windows)]
fn round_corners(window: &WebviewWindow) {
    const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
    const DWMWCP_ROUND: i32 = 2;
    if let Ok(hwnd) = window.hwnd() {
        unsafe {
            DwmSetWindowAttribute(
                hwnd.0 as isize,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &DWMWCP_ROUND as *const i32 as *const core::ffi::c_void,
                std::mem::size_of::<i32>() as u32,
            );
        }
    }
}
