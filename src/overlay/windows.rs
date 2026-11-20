//! Windows specifics for the winit overlay windows. The overlay is a
//! DirectComposition swapchain (wgpu DX12 `DxgiFromVisual`) in a topmost tool
//! window; click-through comes from winit's `set_cursor_hittest(false)`,
//! which sets WS_EX_TRANSPARENT | WS_EX_LAYERED.
//!
//! Not tested on a Windows machine yet; it is compiled in CI.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetWindowLongPtrW, HWND_TOPMOST, LWA_ALPHA, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos, WS_EX_LAYERED,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
};
use winit::window::Window;

fn hwnd(w: &Window) -> Option<HWND> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match w.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get() as HWND),
        _ => None,
    }
}

/// Keep the overlay out of the taskbar / Alt-Tab and above other windows.
pub fn harden(w: &Window) {
    let Some(hwnd) = hwnd(w) else { return };
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(
            hwnd,
            GWL_EXSTYLE,
            ex | (WS_EX_TOOLWINDOW | WS_EX_TOPMOST) as isize,
        );
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
    after_hittest(w);
}

/// A layered window without layered attributes is never shown. winit adds
/// WS_EX_LAYERED for click-through but never sets them, so do it here after
/// every hit-test change.
pub fn after_hittest(w: &Window) {
    let Some(hwnd) = hwnd(w) else { return };
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        if ex & WS_EX_LAYERED != 0 {
            SetLayeredWindowAttributes(hwnd, 0, 255, LWA_ALPHA);
        }
    }
}
