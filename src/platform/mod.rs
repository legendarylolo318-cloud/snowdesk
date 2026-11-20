//! Desktop state feeds: window rectangles, cursor position, fullscreen state.
//! Each source runs on its own thread and publishes immutable snapshots.

#[cfg(target_os = "linux")]
pub mod hyprland;
#[cfg(windows)]
pub mod windows_rects;
#[cfg(target_os = "linux")]
pub mod x11;

use crate::sim::Rect;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// A visible window. Feeds list windows front to back (topmost first).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WinInfo {
    /// Global coordinates in the feed's own units (Hyprland: logical px).
    pub rect: Rect,
    /// Monitor the window is visible on, when the source knows.
    pub monitor: Option<String>,
    /// False for windows that only hide what is behind them but never hold
    /// snow themselves (our own control panel).
    pub ledge: bool,
}

/// Visible parts of each window's top edge, given rects ordered front to
/// back. An edge is hidden where a window in front covers the band from
/// `margin` above the edge (where snow piles up) down to the edge.
pub fn visible_ledges(wins: &[(Rect, bool)], margin: f32) -> Vec<Rect> {
    let mut out = Vec::new();
    let mut spans: Vec<(f32, f32)> = Vec::new();
    for (i, &(r, ledge)) in wins.iter().enumerate() {
        if !ledge || r.w <= 0.0 {
            continue;
        }
        spans.clear();
        spans.push((r.x, r.x + r.w));
        for &(o, _) in &wins[..i] {
            if !(o.y <= r.y + 1.0 && o.y + o.h > r.y - margin) {
                continue;
            }
            let (a, b) = (o.x, o.x + o.w);
            let mut next = Vec::with_capacity(spans.len() + 1);
            for &(s0, s1) in &spans {
                if b <= s0 || a >= s1 {
                    next.push((s0, s1));
                    continue;
                }
                if a > s0 {
                    next.push((s0, a));
                }
                if b < s1 {
                    next.push((b, s1));
                }
            }
            spans = next;
            if spans.is_empty() {
                break;
            }
        }
        out.extend(spans.iter().map(|&(s0, s1)| Rect {
            x: s0,
            y: r.y,
            w: s1 - s0,
            h: r.h,
        }));
    }
    out
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub windows: Vec<WinInfo>,
    /// Monitors (by name) that currently have a fullscreen app.
    pub fullscreen_monitors: Vec<String>,
    /// Fullscreen app on a source without per-monitor info.
    pub fullscreen_any: bool,
    pub cursor: Option<(f32, f32)>,
    pub supports_cursor: bool,
    /// Bumped whenever the window list changed.
    pub generation: u64,
}

pub struct Shared {
    snap: Mutex<Arc<Snapshot>>,
    pub cursor_wanted: AtomicBool,
    /// False while rendering is paused: sources slow right down.
    pub active: AtomicBool,
    pub stop: AtomicBool,
}

impl Shared {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            snap: Mutex::new(Arc::new(Snapshot::default())),
            cursor_wanted: AtomicBool::new(true),
            active: AtomicBool::new(true),
            stop: AtomicBool::new(false),
        })
    }
    pub fn publish(&self, s: Snapshot) {
        *self.snap.lock().unwrap() = Arc::new(s);
    }
    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.snap.lock().unwrap().clone()
    }
    pub fn want_cursor(&self) -> bool {
        self.cursor_wanted.load(Ordering::Relaxed)
    }
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }
    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
}

/// Start the best available source for this session.
pub fn start() -> Arc<Shared> {
    let shared = Shared::new();
    #[cfg(target_os = "linux")]
    {
        let s = shared.clone();
        if hyprland::available() {
            tracing::info!("desktop feed: Hyprland IPC");
            std::thread::Builder::new()
                .name("feed-hypr".into())
                .spawn(move || hyprland::run(s))
                .ok();
        } else if std::env::var_os("DISPLAY").is_some()
            && std::env::var_os("WAYLAND_DISPLAY").is_none()
        {
            tracing::info!("desktop feed: X11");
            std::thread::Builder::new()
                .name("feed-x11".into())
                .spawn(move || x11::run(s))
                .ok();
        } else {
            tracing::info!(
                "no window/cursor source on this compositor; wind and window ledges disabled"
            );
        }
    }
    #[cfg(windows)]
    {
        let s = shared.clone();
        std::thread::Builder::new()
            .name("feed-win".into())
            .spawn(move || windows_rects::run(s))
            .ok();
    }
    shared
}

/// Local wall-clock hour (0–23), for the night-time aurora.
pub fn local_hour() -> u32 {
    #[cfg(target_os = "linux")]
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        tm.tm_hour as u32
    }
    #[cfg(windows)]
    unsafe {
        let mut st: windows_sys::Win32::Foundation::SYSTEMTIME = std::mem::zeroed();
        windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut st);
        st.wHour as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    #[test]
    fn front_window_hides_part_of_edge_behind() {
        // Front window covers the middle of the back window's top edge.
        let wins = [
            (r(300.0, 50.0, 200.0, 400.0), true),
            (r(100.0, 100.0, 600.0, 300.0), true),
        ];
        let v = visible_ledges(&wins, 10.0);
        assert_eq!(v[0], r(300.0, 50.0, 200.0, 400.0));
        assert_eq!(
            &v[1..],
            &[r(100.0, 100.0, 200.0, 300.0), r(500.0, 100.0, 200.0, 300.0)]
        );
    }

    #[test]
    fn window_below_edge_does_not_hide_it() {
        let wins = [
            (r(0.0, 500.0, 900.0, 100.0), true),
            (r(100.0, 100.0, 300.0, 300.0), true),
        ];
        assert_eq!(visible_ledges(&wins, 10.0).len(), 2);
    }

    #[test]
    fn panel_occludes_but_holds_no_snow() {
        let wins = [
            (r(0.0, 0.0, 400.0, 400.0), false),
            (r(100.0, 100.0, 600.0, 300.0), true),
        ];
        assert_eq!(
            visible_ledges(&wins, 10.0),
            vec![r(400.0, 100.0, 300.0, 300.0)]
        );
    }
}
