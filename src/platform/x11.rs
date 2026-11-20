//! X11 window list (_NET_CLIENT_LIST), fullscreen state and pointer.

use super::{Shared, Snapshot, WinInfo};
use crate::sim::Rect;
use std::sync::Arc;
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{self, AtomEnum, ConnectionExt};
use x11rb::rust_connection::RustConnection;

struct Atoms {
    /// _NET_CLIENT_LIST_STACKING (bottom to top), or _NET_CLIENT_LIST.
    client_list: u32,
    wm_state: u32,
    fullscreen: u32,
    hidden: u32,
    frame_extents: u32,
    wm_class: u32,
}

fn atom(c: &RustConnection, name: &str) -> Option<u32> {
    Some(
        c.intern_atom(false, name.as_bytes())
            .ok()?
            .reply()
            .ok()?
            .atom,
    )
}

fn prop_u32(c: &RustConnection, w: u32, p: u32, ty: impl Into<u32>, len: u32) -> Vec<u32> {
    c.get_property(false, w, p, ty, 0, len)
        .ok()
        .and_then(|r| r.reply().ok())
        .and_then(|r| r.value32().map(|v| v.collect()))
        .unwrap_or_default()
}

fn refresh(c: &RustConnection, root: u32, a: &Atoms, own_pid_class: &str) -> (Vec<WinInfo>, bool) {
    let list = prop_u32(c, root, a.client_list, AtomEnum::WINDOW, 1024);
    let mut wins = Vec::new();
    let mut any_fs = false;
    // Topmost first.
    for w in list.into_iter().rev() {
        // Windows on other desktops and minimized ones are unmapped.
        let viewable = c
            .get_window_attributes(w)
            .ok()
            .and_then(|r| r.reply().ok())
            .is_some_and(|a| a.map_state == xproto::MapState::VIEWABLE);
        if !viewable {
            continue;
        }
        let state = prop_u32(c, w, a.wm_state, AtomEnum::ATOM, 32);
        if state.contains(&a.hidden) {
            continue;
        }
        let class = c
            .get_property(false, w, a.wm_class, AtomEnum::STRING, 0, 64)
            .ok()
            .and_then(|r| r.reply().ok())
            .map(|r| String::from_utf8_lossy(&r.value).to_string())
            .unwrap_or_default();
        let own = class.split('\0').any(|c| c == own_pid_class);
        let Some(g) = c.get_geometry(w).ok().and_then(|r| r.reply().ok()) else {
            continue;
        };
        let Some(t) = c
            .translate_coordinates(w, root, 0, 0)
            .ok()
            .and_then(|r| r.reply().ok())
        else {
            continue;
        };
        let ext = prop_u32(c, w, a.frame_extents, AtomEnum::CARDINAL, 4); // l r t b
        let top = ext.get(2).copied().unwrap_or(0) as f32;
        let fs = state.contains(&a.fullscreen);
        any_fs |= fs;
        wins.push(WinInfo {
            rect: Rect {
                x: t.dst_x as f32,
                y: t.dst_y as f32 - top,
                w: g.width as f32,
                h: g.height as f32 + top,
            },
            monitor: None,
            ledge: !own,
        });
    }
    (wins, any_fs)
}

pub fn run(shared: Arc<Shared>) {
    let Ok((c, screen)) = RustConnection::connect(None) else {
        return;
    };
    let root = c.setup().roots[screen].root;
    let stacking = atom(&c, "_NET_CLIENT_LIST_STACKING")
        .filter(|&a| !prop_u32(&c, root, a, AtomEnum::WINDOW, 1).is_empty());
    let (Some(client_list), Some(wm_state), Some(fullscreen), Some(hidden), Some(frame_extents)) = (
        stacking.or_else(|| atom(&c, "_NET_CLIENT_LIST")),
        atom(&c, "_NET_WM_STATE"),
        atom(&c, "_NET_WM_STATE_FULLSCREEN"),
        atom(&c, "_NET_WM_STATE_HIDDEN"),
        atom(&c, "_NET_FRAME_EXTENTS"),
    ) else {
        return;
    };
    let atoms = Atoms {
        client_list,
        wm_state,
        fullscreen,
        hidden,
        frame_extents,
        wm_class: AtomEnum::WM_CLASS.into(),
    };
    let mut snap = Snapshot {
        supports_cursor: true,
        ..Default::default()
    };
    let mut last = Instant::now() - Duration::from_secs(10);
    let mut pacer = super::CursorPacer::new();
    while !shared.stopped() {
        let active = shared.is_active();
        let want_cursor = active && shared.want_cursor();
        let mut publish = false;
        if last.elapsed() > Duration::from_millis(if active { 400 } else { 1500 }) {
            last = Instant::now();
            let (w, fs) = refresh(&c, root, &atoms, "snowdesk");
            if w != snap.windows {
                snap.generation += 1;
            }
            snap.windows = w;
            snap.fullscreen_any = fs;
            publish = true;
        }
        if want_cursor {
            let p = xproto::query_pointer(&c, root)
                .ok()
                .and_then(|r| r.reply().ok())
                .map(|r| (r.root_x as f32, r.root_y as f32));
            pacer.sample(p);
            if p != snap.cursor {
                snap.cursor = p;
                publish = true;
            }
        } else if snap.cursor.take().is_some() {
            publish = true;
        }
        if publish {
            shared.publish(snap.clone());
        }
        std::thread::sleep(if want_cursor {
            pacer.interval()
        } else if active {
            Duration::from_millis(100)
        } else {
            Duration::from_millis(500)
        });
    }
}
