//! Hyprland IPC: `.socket.sock` for requests, `.socket2.sock` for events.

use super::{Shared, Snapshot, WinInfo};
use crate::sim::Rect;
use serde_json::Value;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn base() -> Option<PathBuf> {
    let sig = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?;
    let rt = std::env::var_os("XDG_RUNTIME_DIR")?;
    Some(PathBuf::from(rt).join("hypr").join(sig))
}

pub fn available() -> bool {
    base().is_some_and(|b| b.join(".socket.sock").exists())
}

fn request(cmd: &str) -> Option<String> {
    let mut s = UnixStream::connect(base()?.join(".socket.sock")).ok()?;
    s.set_read_timeout(Some(Duration::from_millis(300))).ok()?;
    s.write_all(cmd.as_bytes()).ok()?;
    let mut out = String::new();
    s.read_to_string(&mut out).ok()?;
    Some(out)
}

pub fn cursor_pos() -> Option<(f32, f32)> {
    let r = request("cursorpos")?;
    let (x, y) = r.trim().split_once(',')?;
    Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
}

/// Bars and docks from `layers -j`: layer surfaces on the bottom/top
/// levels whose size looks like a bar (snow settles on them). Shells that
/// use one big transparent layer for their bar (some quickshell/ags setups)
/// report no usable bar rectangle and are skipped.
pub fn parse_layers(layers: &str, monitors: &str) -> Vec<WinInfo> {
    let (Ok(layers), Ok(monitors)) = (
        serde_json::from_str::<Value>(layers),
        serde_json::from_str::<Value>(monitors),
    ) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let Some(map) = layers.as_object() else {
        return out;
    };
    for (mon, v) in map {
        // Monitor size in layout (logical) units.
        let Some(m) = monitors
            .as_array()
            .and_then(|a| a.iter().find(|m| m["name"].as_str() == Some(mon)))
        else {
            continue;
        };
        let scale = m["scale"].as_f64().unwrap_or(1.0).max(0.1);
        let mw = m["width"].as_f64().unwrap_or(0.0) / scale;
        let mh = m["height"].as_f64().unwrap_or(0.0) / scale;
        for level in ["1", "2"] {
            let Some(list) = v["levels"][level].as_array() else {
                continue;
            };
            for l in list {
                if l["namespace"].as_str() == Some("snowdesk") {
                    continue;
                }
                let (Some(x), Some(y), Some(w), Some(h)) = (
                    l["x"].as_f64(),
                    l["y"].as_f64(),
                    l["w"].as_f64(),
                    l["h"].as_f64(),
                ) else {
                    continue;
                };
                let bar_like = h > 4.0 && h <= mh * 0.2 && w >= mw * 0.12;
                if !bar_like {
                    continue;
                }
                out.push(WinInfo {
                    rect: Rect {
                        x: x as f32,
                        y: y as f32,
                        w: w as f32,
                        h: h as f32,
                    },
                    monitor: Some(mon.clone()),
                    ledge: true,
                    id: l["address"]
                        .as_str()
                        .and_then(|a| u64::from_str_radix(a.trim_start_matches("0x"), 16).ok())
                        .unwrap_or(0)
                        ^ 0x1a7e_0000_0000_0000,
                });
            }
        }
    }
    out
}

/// Parse `clients -j` + `monitors -j` into the windows visible on screen,
/// front to back, plus the monitors showing a fullscreen window.
pub fn parse_state(clients: &str, monitors: &str) -> Option<(Vec<WinInfo>, Vec<String>)> {
    let clients: Value = serde_json::from_str(clients).ok()?;
    let monitors: Value = serde_json::from_str(monitors).ok()?;
    // Visible workspace id -> (monitor name, is the special workspace).
    let mut ws_mon: Vec<(i64, String, bool)> = Vec::new();
    for m in monitors.as_array()? {
        let name = m["name"].as_str()?.to_string();
        if let Some(id) = m["activeWorkspace"]["id"].as_i64() {
            ws_mon.push((id, name.clone(), false));
        }
        // A shown scratchpad (special workspace) sits above the normal one.
        if let Some(id) = m["specialWorkspace"]["id"].as_i64()
            && id != 0
        {
            ws_mon.push((id, name, true));
        }
    }
    // (stacking key, window); lower key = closer to the viewer.
    let mut wins: Vec<((u8, i64), WinInfo)> = Vec::new();
    let mut fullscreen = Vec::new();
    for c in clients.as_array()? {
        if !c["mapped"].as_bool().unwrap_or(true) || c["hidden"].as_bool().unwrap_or(false) {
            continue;
        }
        // Our own overlay windows (X11 backend under XWayland).
        if c["title"].as_str() == Some("snowdesk-overlay") {
            continue;
        }
        let ws = c["workspace"]["id"].as_i64().unwrap_or(0);
        let pinned = c["pinned"].as_bool().unwrap_or(false);
        let at = &c["at"];
        let size = &c["size"];
        let (Some(x), Some(y), Some(w), Some(h)) = (
            at[0].as_f64(),
            at[1].as_f64(),
            size[0].as_f64(),
            size[1].as_f64(),
        ) else {
            continue;
        };
        let rect = Rect {
            x: x as f32,
            y: y as f32,
            w: w as f32,
            h: h as f32,
        };
        let (mon, special) = match ws_mon.iter().find(|(id, _, _)| *id == ws) {
            Some((_, m, sp)) => (Some(m.clone()), *sp),
            // Pinned floating windows show on every workspace of their monitor.
            None if pinned => (None, false),
            None => continue,
        };
        let fs = match &c["fullscreen"] {
            Value::Bool(b) => *b,
            v => v.as_i64().unwrap_or(0) >= 2,
        };
        if fs
            && let Some(m) = &mon
            && !fullscreen.contains(m)
        {
            fullscreen.push(m.clone());
        }
        let floating = c["floating"].as_bool().unwrap_or(false);
        let layer = if fs {
            0
        } else if pinned {
            1
        } else if special {
            2 + u8::from(!floating)
        } else if floating {
            4
        } else {
            5
        };
        let focus = c["focusHistoryID"].as_i64().unwrap_or(i64::MAX);
        // Our own panel hides what is behind it but never collects snow.
        let own = c["class"].as_str() == Some("snowdesk");
        wins.push((
            (layer, focus),
            WinInfo {
                rect,
                monitor: mon,
                ledge: !own,
                id: c["address"]
                    .as_str()
                    .and_then(|a| u64::from_str_radix(a.trim_start_matches("0x"), 16).ok())
                    .unwrap_or(0),
            },
        ));
    }
    wins.sort_by_key(|(k, _)| *k);
    Some((wins.into_iter().map(|(_, w)| w).collect(), fullscreen))
}

/// Returns true if the window list changed.
fn refresh(snap: &mut Snapshot) -> bool {
    let (Some(c), Some(m)) = (request("j/clients"), request("j/monitors")) else {
        return false;
    };
    if let Some((mut w, f)) = parse_state(&c, &m) {
        // Bars and docks sit above windows: put them first (front-most).
        if let Some(layers) = request("j/layers") {
            let mut bars = parse_layers(&layers, &m);
            bars.append(&mut w);
            w = bars;
        }
        let changed = w != snap.windows;
        if changed {
            snap.generation += 1;
        }
        snap.windows = w;
        snap.fullscreen_monitors = f;
        return changed;
    }
    false
}

pub fn run(shared: Arc<Shared>) {
    let Some(b) = base() else { return };
    let mut ev = UnixStream::connect(b.join(".socket2.sock")).ok();
    let mut snap = Snapshot {
        supports_cursor: true,
        ..Default::default()
    };
    refresh(&mut snap);
    shared.publish(snap.clone());
    let mut buf = [0u8; 4096];
    let mut last_refresh = Instant::now();
    let mut last_cursor = Instant::now();
    let mut pacer = super::CursorPacer::new();
    let mut dirty = false;
    let mut moving_until = Instant::now();
    while !shared.stopped() {
        let moving = Instant::now() < moving_until;
        let active = shared.is_active();
        let want_cursor = active && shared.want_cursor();
        let tick = if want_cursor {
            pacer.interval()
        } else if active {
            Duration::from_millis(200)
        } else {
            Duration::from_millis(500)
        };
        let tick = if moving && active {
            tick.min(super::MOVING_POLL)
        } else {
            tick
        };
        match ev.as_mut() {
            Some(s) => {
                let _ = s.set_read_timeout(Some(tick));
                match s.read(&mut buf) {
                    Ok(0) => ev = None,
                    Ok(_) => dirty = true,
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) => {}
                    Err(_) => ev = None,
                }
            }
            None => {
                std::thread::sleep(tick.max(Duration::from_millis(250)));
                ev = UnixStream::connect(b.join(".socket2.sock")).ok();
                dirty = true;
            }
        }
        let period = if active && moving {
            super::MOVING_POLL
        } else if active {
            Duration::from_millis(400)
        } else {
            Duration::from_secs(2)
        };
        let mut publish = false;
        if (dirty && last_refresh.elapsed() > Duration::from_millis(40))
            || last_refresh.elapsed() > period
        {
            dirty = false;
            last_refresh = Instant::now();
            if refresh(&mut snap) {
                moving_until = Instant::now() + super::MOVING_HOLD;
            }
            publish = true;
        }
        if want_cursor && last_cursor.elapsed() >= pacer.interval() {
            last_cursor = Instant::now();
            let c = cursor_pos();
            pacer.sample(c);
            if c != snap.cursor {
                snap.cursor = c;
                publish = true;
            }
        } else if !want_cursor && snap.cursor.is_some() {
            snap.cursor = None;
            publish = true;
        }
        if publish {
            shared.publish(snap.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_clients() {
        let clients = r#"[
          {"mapped":true,"hidden":false,"at":[10,20],"size":[800,600],"workspace":{"id":1},"class":"firefox","fullscreen":0,"floating":false,"focusHistoryID":1},
          {"mapped":true,"hidden":false,"at":[0,0],"size":[1920,1080],"workspace":{"id":2},"class":"game","fullscreen":2,"focusHistoryID":3},
          {"mapped":true,"hidden":false,"at":[5,5],"size":[100,100],"workspace":{"id":1},"class":"snowdesk","fullscreen":0,"floating":true,"focusHistoryID":0},
          {"mapped":true,"hidden":false,"at":[5,5],"size":[100,100],"workspace":{"id":9},"class":"other","fullscreen":0},
          {"mapped":true,"hidden":false,"at":[50,60],"size":[300,200],"workspace":{"id":-98},"class":"scratch","fullscreen":0,"floating":true,"focusHistoryID":2}
        ]"#;
        let mons = r#"[{"name":"DP-1","activeWorkspace":{"id":1},"specialWorkspace":{"id":-98,"name":"special:term"}},{"name":"HDMI-A-1","activeWorkspace":{"id":2},"specialWorkspace":{"id":0,"name":""}}]"#;
        let (w, f) = parse_state(clients, mons).unwrap();
        // fullscreen game, scratchpad, floating panel, tiled firefox
        assert_eq!(w.len(), 4);
        assert_eq!(w[0].rect.w, 1920.0);
        assert_eq!(w[1].rect.w, 300.0);
        assert_eq!(w[1].monitor.as_deref(), Some("DP-1"));
        assert!(!w[2].ledge, "own panel only occludes");
        assert_eq!(w[3].rect.w, 800.0);
        assert_eq!(f, vec!["HDMI-A-1".to_string()]);
    }

    #[test]
    fn bars_and_docks_from_layers() {
        let mons = r#"[{"name":"DP-1","width":2560,"height":1440,"scale":1.0}]"#;
        let layers = r#"{"DP-1":{"levels":{
            "0":[{"address":"0x1","namespace":"wallpaper","x":0,"y":0,"w":2560,"h":1440}],
            "2":[{"address":"0x2","namespace":"waybar","x":0,"y":0,"w":2560,"h":32},
                 {"address":"0x3","namespace":"dock","x":900,"y":1360,"w":760,"h":72},
                 {"address":"0x4","namespace":"quickshell","x":0,"y":0,"w":2560,"h":740}],
            "3":[{"address":"0x5","namespace":"snowdesk","x":0,"y":0,"w":2560,"h":1440}]}}}"#;
        let bars = parse_layers(layers, mons);
        let names: Vec<f32> = bars.iter().map(|b| b.rect.h).collect();
        assert_eq!(
            names,
            vec![32.0, 72.0],
            "waybar and the dock; not the wallpaper, a full-screen layer or us"
        );
        assert!(
            bars.iter()
                .all(|b| b.ledge && b.monitor.as_deref() == Some("DP-1"))
        );
        assert!(parse_layers("nope", mons).is_empty());
    }

    #[test]
    fn bad_json_is_ignored() {
        assert!(parse_state("not json", "[]").is_none());
        let (w, _) = parse_state(r#"[{"at":"x"}]"#, "[]").unwrap();
        assert!(w.is_empty());
    }
}
