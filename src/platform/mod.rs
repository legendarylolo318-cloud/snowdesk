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
    /// Stable identity (Hyprland address, X11 window id, HWND), so snow can
    /// travel with a window that moves.
    pub id: u64,
}

/// After a window moves, poll window geometry this often for a moment so a
/// drag or a shake is tracked smoothly (feeds otherwise poll every 400 ms).
pub const MOVING_POLL: std::time::Duration = std::time::Duration::from_millis(50);
/// How long to keep polling fast after the last change.
pub const MOVING_HOLD: std::time::Duration = std::time::Duration::from_millis(700);

/// Visible parts of each window's top edge, given rects ordered front to
/// back. An edge is hidden where a window in front covers the band from
/// `margin` above the edge (where snow piles up) down to the edge.
pub fn visible_ledges(wins: &[(Rect, bool, u64)], margin: f32) -> Vec<crate::sim::ledges::Ledge> {
    let mut out = Vec::new();
    let mut spans: Vec<(f32, f32)> = Vec::new();
    for (i, &(r, ledge, id)) in wins.iter().enumerate() {
        if !ledge || r.w <= 0.0 {
            continue;
        }
        spans.clear();
        spans.push((r.x, r.x + r.w));
        for &(o, _, _) in &wins[..i] {
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
        out.extend(spans.iter().map(|&(s0, s1)| crate::sim::ledges::Ledge {
            id,
            rect: Rect {
                x: s0,
                y: r.y,
                w: s1 - s0,
                h: r.h,
            },
            win: r,
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

/// Cursor polling rate: fast while the pointer moves, slow once it has been
/// still for a moment (movement is picked up within 100 ms, which is
/// invisible for cursor wind, and an idle desktop stops polling at 60 Hz).
pub struct CursorPacer {
    last_move: std::time::Instant,
    last: Option<(f32, f32)>,
}

impl CursorPacer {
    pub fn new() -> Self {
        Self {
            last_move: std::time::Instant::now(),
            last: None,
        }
    }

    /// Record a sample; returns true if the cursor moved.
    pub fn sample(&mut self, c: Option<(f32, f32)>) -> bool {
        let moved = c != self.last;
        if moved {
            self.last = c;
            self.last_move = std::time::Instant::now();
        }
        moved
    }

    pub fn interval(&self) -> std::time::Duration {
        if self.last_move.elapsed() < std::time::Duration::from_millis(500) {
            std::time::Duration::from_millis(15)
        } else {
            std::time::Duration::from_millis(100)
        }
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

/// Give freed heap memory back to the OS. glibc keeps memory freed after
/// big one-off allocations (GPU setup, shader compilation, font loading,
/// library scans) mapped; this returns it. A no-op elsewhere.
pub fn trim_heap() {
    #[cfg(target_os = "linux")]
    unsafe {
        libc::malloc_trim(0);
    }
}

/// Limit glibc to a couple of malloc arenas. By default each thread that
/// allocates can get its own arena; snowdesk has ~15 mostly idle threads,
/// so that only fragments memory. Call first thing in `main`.
pub fn limit_malloc_arenas() {
    #[cfg(target_os = "linux")]
    unsafe {
        libc::mallopt(libc::M_ARENA_MAX, 2);
    }
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

    #[test]
    fn cursor_pacer_slows_down_when_still() {
        let mut p = CursorPacer::new();
        assert!(p.sample(Some((1.0, 2.0))));
        assert_eq!(p.interval(), std::time::Duration::from_millis(15));
        p.last_move -= std::time::Duration::from_secs(1);
        assert!(!p.sample(Some((1.0, 2.0))));
        assert_eq!(p.interval(), std::time::Duration::from_millis(100));
        assert!(p.sample(Some((5.0, 2.0))));
        assert_eq!(p.interval(), std::time::Duration::from_millis(15));
    }

    fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    #[test]
    fn front_window_hides_part_of_edge_behind() {
        // Front window covers the middle of the back window's top edge.
        let wins = [
            (r(300.0, 50.0, 200.0, 400.0), true, 1),
            (r(100.0, 100.0, 600.0, 300.0), true, 2),
        ];
        let v: Vec<Rect> = visible_ledges(&wins, 10.0).iter().map(|l| l.rect).collect();
        assert_eq!(v[0], r(300.0, 50.0, 200.0, 400.0));
        assert_eq!(
            &v[1..],
            &[r(100.0, 100.0, 200.0, 300.0), r(500.0, 100.0, 200.0, 300.0)]
        );
    }

    #[test]
    fn window_below_edge_does_not_hide_it() {
        let wins = [
            (r(0.0, 500.0, 900.0, 100.0), true, 1),
            (r(100.0, 100.0, 300.0, 300.0), true, 2),
        ];
        assert_eq!(visible_ledges(&wins, 10.0).len(), 2);
    }

    #[test]
    fn panel_occludes_but_holds_no_snow() {
        let wins = [
            (r(0.0, 0.0, 400.0, 400.0), false, 1),
            (r(100.0, 100.0, 600.0, 300.0), true, 2),
        ];
        let v = visible_ledges(&wins, 10.0);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].rect, r(400.0, 100.0, 300.0, 300.0));
        assert_eq!(v[0].id, 2);
        assert_eq!(v[0].win, r(100.0, 100.0, 600.0, 300.0));
    }
}
