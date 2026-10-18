//! Native window look: blur behind the glass and rounded corners (Windows only).
//! Uses the same two DWM calls as the original Python version.

use tauri::WebviewWindow;

pub fn apply_effects(window: &WebviewWindow) {
    // Make sure the webview itself paints nothing behind the page.
    let _ = window.set_background_color(Some(tauri::window::Color(0, 0, 0, 0)));

    #[cfg(windows)]
    if let Ok(hwnd) = window.hwnd() {
        let hwnd = hwnd.0 as isize;
        native::round_corners(hwnd);
        native::enable_blur(hwnd);
    }
    #[cfg(not(windows))]
    let _ = window;
}

#[cfg(windows)]
mod native {
    use core::ffi::c_void;

    #[repr(C)]
    struct AccentPolicy {
        accent_state: i32,
        accent_flags: i32,
        gradient_color: u32,
        animation_id: i32,
    }

    #[repr(C)]
    struct WindowCompositionAttribData {
        attribute: i32,
        data: *mut c_void,
        size: usize,
    }

    type SetWindowCompositionAttribute =
        unsafe extern "system" fn(isize, *mut WindowCompositionAttribData) -> i32;

    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmSetWindowAttribute(hwnd: isize, attr: u32, value: *const c_void, size: u32) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryA(name: *const u8) -> isize;
        fn GetProcAddress(module: isize, name: *const u8) -> *const c_void;
    }

    /// DWMWA_WINDOW_CORNER_PREFERENCE = DWMWCP_ROUND
    pub fn round_corners(hwnd: isize) {
        let pref: i32 = 2;
        unsafe {
            DwmSetWindowAttribute(hwnd, 33, &pref as *const i32 as *const c_void, 4);
        }
    }

    /// ACCENT_ENABLE_BLURBEHIND with a fully transparent tint.
    pub fn enable_blur(hwnd: isize) {
        unsafe {
            let lib = LoadLibraryA(b"user32.dll\0".as_ptr());
            if lib == 0 {
                return;
            }
            let proc = GetProcAddress(lib, b"SetWindowCompositionAttribute\0".as_ptr());
            if proc.is_null() {
                return;
            }
            let set: SetWindowCompositionAttribute = std::mem::transmute(proc);
            let mut accent = AccentPolicy {
                accent_state: 3,
                accent_flags: 0,
                gradient_color: 0,
                animation_id: 0,
            };
            let mut data = WindowCompositionAttribData {
                attribute: 19, // WCA_ACCENT_POLICY
                data: &mut accent as *mut _ as *mut c_void,
                size: std::mem::size_of::<AccentPolicy>(),
            };
            set(hwnd, &mut data);
        }
    }
}
