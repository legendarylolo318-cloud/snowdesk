//! Windows: EnumWindows + DWMWA_EXTENDED_FRAME_BOUNDS, GetCursorPos and
//! SHQueryUserNotificationState for fullscreen detection.
//! NOTE: written against windows-sys; not compiled/tested on a Windows host
//! in the Linux development environment.

use super::{Shared, Snapshot, WinInfo};
use crate::sim::Rect;
use std::sync::Arc;
use std::time::Duration;
use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use windows_sys::Win32::UI::Shell::SHQueryUserNotificationState;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GWL_EXSTYLE, GetCursorPos, GetForegroundWindow, GetWindowLongW,
    GetWindowTextLengthW, GetWindowThreadProcessId, IsIconic, IsWindowVisible, WS_EX_TOOLWINDOW,
};

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
        });
    }
    1
}

fn fullscreen_busy() -> bool {
    unsafe {
        let mut state = 0i32;
        if SHQueryUserNotificationState(&mut state) != 0 {
            return false;
        }
        // QUNS_BUSY = 2, RUNNING_D3D_FULL_SCREEN = 3, PRESENTATION_MODE = 4
        if !(2..=4).contains(&state) {
            return false;
        }
        // Make sure the foreground window isn't ours.
        let fg = GetForegroundWindow();
        let mut pid = 0u32;
        GetWindowThreadProcessId(fg, &mut pid);
        pid != std::process::id()
    }
}

pub fn run(shared: Arc<Shared>) {
    let mut snap = Snapshot {
        supports_cursor: true,
        ..Default::default()
    };
    let mut last_enum: Option<std::time::Instant> = None;
    while !shared.stopped() {
        let active = shared.is_active();
        let want_cursor = active && shared.want_cursor();
        let mut publish = false;
        if want_cursor {
            let mut p = POINT { x: 0, y: 0 };
            let c = unsafe { (GetCursorPos(&mut p) != 0).then_some((p.x as f32, p.y as f32)) };
            if c != snap.cursor {
                snap.cursor = c;
                publish = true;
            }
        } else if snap.cursor.take().is_some() {
            publish = true;
        }
        let period = Duration::from_millis(if active { 400 } else { 1600 });
        if last_enum.is_none_or(|t| t.elapsed() >= period) {
            last_enum = Some(std::time::Instant::now());
            let mut ctx = Ctx {
                wins: Vec::new(),
                pid: std::process::id(),
            };
            unsafe {
                EnumWindows(Some(enum_proc), &mut ctx as *mut _ as LPARAM);
            }
            // EnumWindows lists top-level windows top to bottom.
            if ctx.wins != snap.windows {
                snap.generation += 1;
            }
            snap.windows = ctx.wins;
            snap.fullscreen_any = fullscreen_busy();
            publish = true;
        }
        if publish {
            shared.publish(snap.clone());
        }
        // Poll fast only for cursor wind; otherwise mostly sleep.
        let ms = if want_cursor {
            16
        } else if active {
            100
        } else {
            500
        };
        std::thread::sleep(Duration::from_millis(ms));
    }
}
