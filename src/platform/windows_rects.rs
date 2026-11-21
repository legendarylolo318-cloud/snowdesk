//! Windows: EnumWindows + DWMWA_EXTENDED_FRAME_BOUNDS, GetCursorPos and
//! foreground-window-covers-monitor fullscreen detection.

use super::{Shared, Snapshot, WinInfo};
use crate::sim::Rect;
use std::sync::Arc;
use std::time::Duration;
use windows_sys::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONULL, MONITORINFO, MONITORINFOEXW, MonitorFromWindow,
};
use windows_sys::Win32::UI::Shell::SHQueryUserNotificationState;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowW, GWL_EXSTYLE, GWL_STYLE, GetClassNameW, GetCursorPos,
    GetForegroundWindow, GetWindowLongW, GetWindowRect, GetWindowTextLengthW,
    GetWindowThreadProcessId, IsIconic, IsWindowVisible, IsZoomed, WS_CAPTION, WS_EX_TOOLWINDOW,
};
use windows_sys::core::BOOL;

struct Ctx {
    wins: Vec<WinInfo>,
    pid: u32,
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lp: LPARAM) -> BOOL {
    let ctx = unsafe { &mut *(lp as *mut Ctx) };
    unsafe {
        if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 {
            return 1;
        }
        if GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW != 0 {
            return 1;
        }
        if GetWindowTextLengthW(hwnd) == 0 {
            return 1;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        // Our control panel hides what is behind it but holds no snow.
        let own = pid == ctx.pid;
        let mut cloaked = 0u32;
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED as u32,
            &mut cloaked as *mut _ as *mut _,
            4,
        );
        if cloaked != 0 {
            return 1;
        }
        let mut r: RECT = std::mem::zeroed();
        let hr = DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS as u32,
            &mut r as *mut _ as *mut _,
            std::mem::size_of::<RECT>() as u32,
        );
        if hr != 0 {
            return 1;
        }
        ctx.wins.push(WinInfo {
            rect: Rect {
                x: r.left as f32,
                y: r.top as f32,
                w: (r.right - r.left) as f32,
                h: (r.bottom - r.top) as f32,
            },
            monitor: None,
            ledge: !own,
            id: hwnd as usize as u64,
        });
    }
    1
}

/// The monitor (by device name, e.g. `\\.\DISPLAY1`, the same name the
/// overlay uses) covered by the foreground window, if that window is a
/// fullscreen app.
///
/// `SHQueryUserNotificationState` is not used for this: Windows reports
/// QUNS_BUSY for any borderless, monitor-sized, topmost window, which is
/// exactly what our own overlay is, so snowdesk would pause itself.
fn fullscreen_monitor() -> Option<String> {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(fg, &mut pid);
        if pid == std::process::id() {
            return None;
        }
        // The desktop (also hosts wallpaper apps) and the taskbar.
        let mut cls = [0u16; 64];
        let n = GetClassNameW(fg, cls.as_mut_ptr(), cls.len() as i32).max(0) as usize;
        let cls = String::from_utf16_lossy(&cls[..n]);
        if matches!(
            cls.as_str(),
            "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
        ) {
            return None;
        }
        // A maximized window with a title bar is not fullscreen, even with
        // an auto-hidden taskbar.
        let style = GetWindowLongW(fg, GWL_STYLE) as u32;
        if IsZoomed(fg) != 0 && style & WS_CAPTION == WS_CAPTION {
            return None;
        }
        let mon = MonitorFromWindow(fg, MONITOR_DEFAULTTONULL);
        if mon.is_null() {
            return None;
        }
        let mut mi: MONITORINFOEXW = std::mem::zeroed();
        mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(mon, &mut mi as *mut _ as *mut MONITORINFO) == 0 {
            return None;
        }
        let mut r: RECT = std::mem::zeroed();
        if GetWindowRect(fg, &mut r) == 0 {
            return None;
        }
        let m = mi.monitorInfo.rcMonitor;
        let covers =
            r.left <= m.left && r.top <= m.top && r.right >= m.right && r.bottom >= m.bottom;
        if !covers {
            return None;
        }
        let len = mi
            .szDevice
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(mi.szDevice.len());
        Some(String::from_utf16_lossy(&mi.szDevice[..len]))
    }
}

/// The primary taskbar's rectangle, unless it is auto-hidden off screen.
fn taskbar_rect() -> Option<Rect> {
    let class: Vec<u16> = "Shell_TrayWnd\0".encode_utf16().collect();
    unsafe {
        let hwnd = FindWindowW(class.as_ptr(), std::ptr::null());
        if hwnd.is_null() || IsWindowVisible(hwnd) == 0 {
            return None;
        }
        let mut r: RECT = std::mem::zeroed();
        if GetWindowRect(hwnd, &mut r) == 0 || r.bottom - r.top < 8 {
            return None;
        }
        Some(Rect {
            x: r.left as f32,
            y: r.top as f32,
            w: (r.right - r.left) as f32,
            h: (r.bottom - r.top) as f32,
        })
    }
}

/// Presentation mode ("do not disturb" for slideshows) pauses everywhere.
fn presentation_mode() -> bool {
    let mut state = 0i32;
    // QUNS_PRESENTATION_MODE = 4
    unsafe { SHQueryUserNotificationState(&mut state) == 0 && state == 4 }
}

pub fn run(shared: Arc<Shared>) {
    let mut snap = Snapshot {
        supports_cursor: true,
        ..Default::default()
    };
    let mut last_enum: Option<std::time::Instant> = None;
    let mut pacer = super::CursorPacer::new();
    let mut moving_until = std::time::Instant::now();
    while !shared.stopped() {
        let active = shared.is_active();
        let want_cursor = active && shared.want_cursor();
        let mut publish = false;
        if want_cursor {
            let mut p = POINT { x: 0, y: 0 };
            let c = unsafe { (GetCursorPos(&mut p) != 0).then_some((p.x as f32, p.y as f32)) };
            pacer.sample(c);
            if c != snap.cursor {
                snap.cursor = c;
                publish = true;
            }
        } else if snap.cursor.take().is_some() {
            publish = true;
        }
        let moving = std::time::Instant::now() < moving_until;
        let period = if active && moving {
            super::MOVING_POLL
        } else {
            Duration::from_millis(if active { 400 } else { 1600 })
        };
        if last_enum.is_none_or(|t| t.elapsed() >= period) {
            last_enum = Some(std::time::Instant::now());
            let mut ctx = Ctx {
                wins: Vec::new(),
                pid: std::process::id(),
            };
            // The taskbar is a tool window (skipped below), but snow should
            // settle on it: add it first (it is on top).
            if let Some(r) = taskbar_rect() {
                ctx.wins.push(WinInfo {
                    rect: r,
                    monitor: None,
                    ledge: true,
                    id: 0x7a5c_ba72,
                });
            }
            unsafe {
                EnumWindows(Some(enum_proc), &mut ctx as *mut _ as LPARAM);
            }
            // EnumWindows lists top-level windows top to bottom.
            if ctx.wins != snap.windows {
                snap.generation += 1;
                moving_until = std::time::Instant::now() + super::MOVING_HOLD;
            }
            snap.windows = ctx.wins;
            snap.fullscreen_monitors = fullscreen_monitor().into_iter().collect();
            snap.fullscreen_any = presentation_mode();
            publish = true;
        }
        if publish {
            shared.publish(snap.clone());
        }
        // Poll fast only for cursor wind; otherwise mostly sleep.
        let ms = if want_cursor {
            pacer.interval().as_millis() as u64
        } else if active {
            100
        } else {
            500
        };
        let ms = if active && std::time::Instant::now() < moving_until {
            ms.min(super::MOVING_POLL.as_millis() as u64)
        } else {
            ms
        };
        std::thread::sleep(Duration::from_millis(ms));
    }
}
