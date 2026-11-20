//! Windows specifics for the winit overlay windows. The overlay is a
//! DirectComposition swapchain (wgpu DX12 `DxgiFromVisual`) in a topmost tool
//! window; click-through comes from winit's `set_cursor_hittest(false)`,
//! which sets WS_EX_TRANSPARENT | WS_EX_LAYERED.
//!
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawWindowHandle, WindowHandle,
};
use std::sync::Arc;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetWindowLongPtrW, HWND_TOPMOST, LWA_ALPHA, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos, WS_EX_LAYERED,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
};
use winit::window::Window;

/// A [`Window`] whose handle can be read from any thread. winit refuses
/// `window_handle()` off the main thread on Windows, but the overlay thread
/// owns the wgpu surfaces and changes window styles. Both are fine to do
/// from another thread: DXGI/DirectComposition and the Win32 style calls
/// below do not need the window's own thread.
pub struct AnyThread(pub Arc<Window>);

impl HasWindowHandle for AnyThread {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        use winit::platform::windows::WindowExtWindows;
        // SAFETY: see the type docs; the handle is only used for surface
        // creation and thread-agnostic Win32 calls while the window lives
        // (the overlay drops its surfaces before its window reference).
        unsafe { self.0.window_handle_any_thread() }
    }
}

impl HasDisplayHandle for AnyThread {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        Ok(DisplayHandle::windows())
    }
}

fn hwnd(w: &Window) -> Option<HWND> {
    use winit::platform::windows::WindowExtWindows;
    // SAFETY: only used for the thread-agnostic style calls in this file.
    match unsafe { w.window_handle_any_thread() }.ok()?.as_raw() {
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
