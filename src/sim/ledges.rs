//! Window ledges: snow on window tops, carried along when a window moves
//! gently, thrown off when it is shaken, closed or minimized; and icicles
//! that grow under snowy ledges.

use super::grid::{BRICK, EMPTY, ICE, PACKED, SNOW, WALL};
use super::world::{Flake, FlakeKind, MAX_FLAKES, World};
use super::{Instance, Rect, SHAPE_TRI, premul};

pub const LEDGE_ROWS: i32 = 3;
/// A window moving faster than this (px/s at 1x scale) is being shaken:
/// its snow flies off. Slower moves carry the snow along.
const SHAKE_SPEED: f32 = 1400.0;

/// A visible stretch of a window's top edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ledge {
    /// Stable window identity from the desktop feed.
    pub id: u64,
    /// Visible part of the top edge (monitor-local px; y is the edge).
    pub rect: Rect,
    /// The whole window.
    pub win: Rect,
}

impl Ledge {
    /// A ledge covering a whole window.
    #[cfg(test)]
    pub fn whole(id: u64, r: Rect) -> Self {
        Self {
            id,
            rect: r,
            win: r,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Icicle {
    pub id: u64,
    /// Offset from the window's left edge, px.
    pub off: f32,
    pub len: f32,
    pub max: f32,
    pub width: f32,
    /// Seconds until the next drip.
    pub drip: f32,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Change {
    Same,
    Gone,
    /// Moved slowly by (dx, dy) cells: carry the snow.
    Carry(i32, i32),
    /// Shaken: throw the snow off with this velocity (px/s).
    Shake(f32, f32),
}

fn is_snowlike(c: u8) -> bool {
    matches!(c, SNOW | PACKED | ICE | BRICK)
}

impl World {
    pub(crate) fn ledge(&self, r: &Rect) -> Option<(i32, i32, i32)> {
        if r.y < 2.0 * self.cell || r.w < 4.0 * self.cell || r.y > self.h {
            return None;
        }
        if r.w >= self.w * 0.98 && r.h >= self.h * 0.9 {
            return None;
        }
        let x0 = (r.x / self.cell).ceil().max(0.0) as i32;
        let x1 = (((r.x + r.w) / self.cell).floor() as i32).min(self.grid.w as i32);
        let y0 = (r.y / self.cell).round() as i32;
        (x1 > x0).then_some((x0, x1, y0))
    }

    /// Replace the visible window ledges (monitor-local px).
    pub fn set_windows(&mut self, new: &[Ledge]) {
        if !self.cfg.windows_collide {
            if !self.windows.is_empty() {
                self.set_windows_inner(&[]);
            }
            return;
        }
        self.set_windows_inner(new);
    }

    fn classify(&mut self, id: u64, old_win: Rect, new: &[Ledge]) -> Change {
        let Some(nw) = new.iter().find(|l| l.id == id).map(|l| l.win) else {
            return Change::Gone;
        };
        if nw == old_win {
            return Change::Same;
        }
        // Speed from the last time this window was seen to change.
        let now = self.time;
        let last = self
            .win_seen
            .iter()
            .find(|(i, _, _)| *i == id)
            .map(|(_, t, _)| *t)
            .unwrap_or(now - 1.0);
        let dt = (now - last).clamp(1.0 / 30.0, 1.0);
        self.win_seen.retain(|(i, _, _)| *i != id);
        self.win_seen.push((id, now, nw));
        let (dx, dy) = (nw.x - old_win.x, nw.y - old_win.y);
        let (vx, vy) = (dx / dt, dy / dt);
        let resized = (nw.w - old_win.w).abs() > 2.0 || (nw.h - old_win.h).abs() > 2.0;
        if vx.hypot(vy) > SHAKE_SPEED * self.scale {
            Change::Shake(vx, vy)
        } else if resized && dx.abs() < 1.0 && dy.abs() < 1.0 {
            // Resized in place: keep the snow where it still fits.
            Change::Carry(0, 0)
        } else {
            Change::Carry(
                (dx / self.cell).round() as i32,
                (dy / self.cell).round() as i32,
            )
        }
    }

    fn set_windows_inner(&mut self, new: &[Ledge]) {
        if self.windows == new {
            return;
        }
        let old = std::mem::take(&mut self.windows);
        // What happened to each window that had a ledge.
        let mut changes: Vec<(u64, Change)> = Vec::new();
        for l in &old {
            if !changes.iter().any(|(id, _)| *id == l.id) {
                let c = self.classify(l.id, l.win, new);
                changes.push((l.id, c));
            }
        }
        let change = |id: u64| {
            changes
                .iter()
                .find(|(i, _)| *i == id)
                .map(|(_, c)| *c)
                .unwrap_or(Change::Same)
        };
        // Lift carried snow and release the rest.
        let mut carried: Vec<(u64, i32, i32, i32, Vec<u8>)> = Vec::new();
        for l in old.iter().filter(|l| !new.contains(l)) {
            match change(l.id) {
                Change::Carry(dx, dy) => {
                    for (x, col) in self.lift_ledge(&l.rect) {
                        carried.push((l.id, x + dx, dx, dy, col));
                    }
                }
                Change::Shake(vx, vy) => self.release_ledge(&l.rect, vx * 0.6, vy * 0.6 - 150.0),
                Change::Same | Change::Gone => self.release_ledge(&l.rect, 0.0, 0.0),
            }
        }
        // Stamp every current ledge: releasing an old ledge may have cleared
        // wall cells it shared with a remaining one.
        for l in new {
            if let Some((x0, x1, y0)) = self.ledge(&l.rect) {
                for x in x0..x1 {
                    for y in y0..y0 + LEDGE_ROWS {
                        self.grid.set(x, y, WALL);
                    }
                }
            }
        }
        // Put carried snow back on the moved window, where it still has a
        // visible ledge; the rest slides off.
        for (id, x, _, _, col) in carried {
            let target = new
                .iter()
                .filter(|l| l.id == id)
                .find_map(|l| self.ledge(&l.rect).filter(|&(x0, x1, _)| x >= x0 && x < x1));
            match target {
                Some((_, _, y0)) => {
                    for (k, &c) in col.iter().enumerate() {
                        let y = y0 - 1 - k as i32;
                        if self.grid.get(x, y) == EMPTY {
                            self.grid.set(x, y, c);
                        }
                    }
                }
                None => {
                    let y0 = new
                        .iter()
                        .find(|l| l.id == id)
                        .map(|l| l.win.y)
                        .unwrap_or(0.0);
                    for k in 0..col.len() {
                        let fx = (x as f32 + 0.5) * self.cell;
                        let fy = y0 - (k as f32 + 1.0) * self.cell;
                        self.spawn_chunk(fx, fy, 0.0, 0.0);
                    }
                }
            }
        }
        // Icicles break off windows that closed or were shaken.
        let mut falling = Vec::new();
        self.icicles.retain(|ic| match change(ic.id) {
            Change::Gone | Change::Shake(..) => {
                falling.push(ic.clone());
                false
            }
            _ => true,
        });
        for ic in falling {
            if let Some(l) = old.iter().find(|l| l.id == ic.id) {
                let x = l.win.x + ic.off;
                let n = (ic.len / (6.0 * self.scale)).ceil().max(1.0) as usize;
                for k in 0..n {
                    self.spawn_shard(x, l.win.y + k as f32 * 6.0 * self.scale);
                }
            }
        }
        self.win_seen
            .retain(|(id, _, _)| new.iter().any(|l| l.id == *id));
        for l in new {
            if !self.win_seen.iter().any(|(i, _, _)| *i == l.id) {
                self.win_seen.push((l.id, self.time, l.win));
            }
        }
        self.windows = new.to_vec();
    }

    /// Remove the snow resting on a ledge, column by column (bottom up).
    fn lift_ledge(&mut self, r: &Rect) -> Vec<(i32, Vec<u8>)> {
        let Some((x0, x1, y0)) = self.ledge(r) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for x in x0..x1 {
            let mut col = Vec::new();
            let mut y = y0 - 1;
            while y >= 0 && is_snowlike(self.grid.get(x, y)) {
                col.push(self.grid.get(x, y));
                self.grid.set(x, y, EMPTY);
                y -= 1;
            }
            for y in y0..y0 + LEDGE_ROWS {
                self.grid.set(x, y, EMPTY);
            }
            if !col.is_empty() {
                out.push((x, col));
            }
        }
        out
    }

    fn spawn_chunk(&mut self, x: f32, y: f32, vx: f32, vy: f32) {
        if self.flakes.len() >= MAX_FLAKES {
            return;
        }
        self.flakes.push(Flake {
            x,
            y,
            vx: vx + (fastrand::f32() - 0.5) * 60.0 * self.scale,
            vy: vy * (0.6 + fastrand::f32() * 0.6),
            depth: 0.9,
            phase: 0.0,
            spin: (fastrand::f32() - 0.5) * 6.0,
            rot: 0.0,
            kind: FlakeKind::Chunk,
            temp: true,
        });
    }

    fn spawn_shard(&mut self, x: f32, y: f32) {
        if self.flakes.len() >= MAX_FLAKES {
            return;
        }
        self.flakes.push(Flake {
            x: x + (fastrand::f32() - 0.5) * 2.0 * self.scale,
            y,
            vx: (fastrand::f32() - 0.5) * 30.0 * self.scale,
            vy: 20.0 * self.scale,
            depth: 1.0,
            phase: 0.0,
            spin: (fastrand::f32() - 0.5) * 8.0,
            rot: std::f32::consts::FRAC_PI_2,
            kind: FlakeKind::Shard,
            temp: true,
        });
    }

    /// Snow on a ledge falls off as particles, with an optional push.
    fn release_ledge(&mut self, r: &Rect, vx: f32, vy: f32) {
        let Some((x0, x1, y0)) = self.ledge(r) else {
            return;
        };
        for x in x0..x1 {
            let mut y = y0 - 1;
            while y >= 0 && is_snowlike(self.grid.get(x, y)) {
                self.grid.set(x, y, EMPTY);
                let (fx, fy) = (x as f32 * self.cell + self.cell * 0.5, y as f32 * self.cell);
                self.spawn_chunk(fx, fy, vx, vy);
                y -= 1;
            }
            for y in y0..y0 + LEDGE_ROWS {
                self.grid.set(x, y, EMPTY);
            }
            self.grid.wake(x as usize, (y0 - 1).max(0) as usize);
        }
    }

    pub(crate) fn windows_reapply(&mut self) {
        self.reapply_walls();
    }

    pub(crate) fn reapply_walls(&mut self) {
        for c in self.grid.cells.iter_mut() {
            if *c == WALL {
                *c = EMPTY;
            }
        }
        let wins = self.windows.clone();
        for l in &wins {
            if let Some((x0, x1, y0)) = self.ledge(&l.rect) {
                for x in x0..x1 {
                    for y in y0..y0 + LEDGE_ROWS {
                        self.grid.set(x, y, WALL);
                    }
                }
            }
        }
        self.grid.mark_all_dirty();
    }

    // ---- icicles ---------------------------------------------------------

    /// Grow, spawn and drip icicles; called once per `dt` seconds.
    pub(crate) fn step_icicles(&mut self, dt: f32) {
        if !self.cfg.icicles {
            self.icicles.clear();
            return;
        }
        let sc = self.scale;
        let wins = self.windows.clone();
        let mut drips = Vec::new();
        for l in &wins {
            let Some((x0, x1, y0)) = self.ledge(&l.rect) else {
                continue;
            };
            let snowy = |w: &World, x: f32| {
                let cx = (x / w.cell) as i32;
                is_snowlike(w.grid.get(cx, y0 - 1))
            };
            // Grow (only under snow), about 3 minutes to full length.
            for ic in self.icicles.iter_mut().filter(|ic| ic.id == l.id) {
                let x = l.win.x + ic.off;
                if x >= l.rect.x && x < l.rect.x + l.rect.w {
                    let cx = (x / self.cell) as i32;
                    if is_snowlike(self.grid.get(cx, y0 - 1)) {
                        ic.len = (ic.len + ic.max * dt / 180.0).min(ic.max);
                    }
                    if ic.len > ic.max * 0.4 {
                        ic.drip -= dt;
                        if ic.drip <= 0.0 {
                            ic.drip = 6.0 + fastrand::f32() * 10.0;
                            drips.push((x, l.win.y + ic.len));
                        }
                    }
                }
            }
            // New icicles on snowy stretches, a few per 100 px.
            let (xa, xb) = (x0 as f32 * self.cell, x1 as f32 * self.cell);
            let width = xb - xa;
            let have = self
                .icicles
                .iter()
                .filter(|ic| {
                    ic.id == l.id && {
                        let x = l.win.x + ic.off;
                        x >= xa && x < xb
                    }
                })
                .count();
            let room = (width / (32.0 * sc)) as usize;
            if have < room && fastrand::f32() < (width / (600.0 * sc)).min(0.8) * dt {
                let x = xa + 4.0 * sc + fastrand::f32() * (width - 8.0 * sc).max(1.0);
                let crowded = self
                    .icicles
                    .iter()
                    .any(|ic| ic.id == l.id && (l.win.x + ic.off - x).abs() < 10.0 * sc);
                if !crowded && snowy(self, x) {
                    self.icicles.push(Icicle {
                        id: l.id,
                        off: x - l.win.x,
                        len: 1.5 * sc,
                        max: (8.0 + fastrand::f32() * 34.0) * sc,
                        width: (2.0 + fastrand::f32() * 2.5) * sc,
                        drip: 3.0 + fastrand::f32() * 8.0,
                    });
                }
            }
        }
        for (x, y) in drips {
            if self.flakes.len() < MAX_FLAKES {
                self.flakes.push(Flake {
                    x,
                    y,
                    vx: 0.0,
                    vy: 20.0 * sc,
                    depth: 1.0,
                    phase: 0.0,
                    spin: 0.0,
                    rot: 0.0,
                    kind: FlakeKind::Water,
                    temp: true,
                });
            }
        }
    }

    pub(crate) fn push_icicles(&self, out: &mut Vec<Instance>) {
        for ic in &self.icicles {
            let Some(l) = self.windows.iter().find(|l| {
                l.id == ic.id && {
                    let x = l.win.x + ic.off;
                    x >= l.rect.x && x < l.rect.x + l.rect.w
                }
            }) else {
                continue;
            };
            let x = l.win.x + ic.off;
            // A shimmer down the icicle as the light catches it.
            let tw = 0.8 + 0.2 * (self.time * 1.7 + ic.off * 0.13).sin();
            out.push(Instance {
                pos: [x, l.win.y + ic.len * 0.5],
                half: [ic.len * 0.5, ic.width],
                rot: std::f32::consts::FRAC_PI_2,
                shape: SHAPE_TRI,
                color: premul([0.72 * tw, 0.88 * tw, 1.0], 0.82),
            });
            out.push(Instance {
                pos: [x - ic.width * 0.25, l.win.y + ic.len * 0.35],
                half: [ic.len * 0.35, ic.width * 0.3],
                rot: std::f32::consts::FRAC_PI_2,
                shape: SHAPE_TRI,
                color: premul([1.0, 1.0, 1.0], 0.55),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn world() -> World {
        World::new(
            400,
            400,
            1.0,
            &Config {
                intensity: 0.0,
                ..Config::default()
            },
        )
    }

    fn rect(x: f32, y: f32) -> Rect {
        Rect {
            x,
            y,
            w: 160.0,
            h: 100.0,
        }
    }

    fn snow_on(w: &mut World, r: Rect) -> usize {
        let (x0, x1, y0) = w.ledge(&r).unwrap();
        let mut n = 0;
        for x in x0..x1 {
            for k in 1..=3 {
                if w.grid.get(x, y0 - k) == SNOW {
                    n += 1;
                }
            }
        }
        n
    }

    fn pile(w: &mut World, r: Rect) {
        let (x0, x1, y0) = w.ledge(&r).unwrap();
        for x in x0..x1 {
            for k in 1..=3 {
                w.grid.set(x, y0 - k, SNOW);
            }
        }
    }

    #[test]
    fn gentle_move_carries_snow() {
        let mut w = world();
        let a = rect(40.0, 200.0);
        w.set_windows(&[Ledge::whole(7, a)]);
        pile(&mut w, a);
        let before = snow_on(&mut w, a);
        w.time += 0.5;
        let b = rect(80.0, 180.0); // 45 px in 0.5 s
        w.set_windows(&[Ledge::whole(7, b)]);
        assert!(
            snow_on(&mut w, b) >= before * 9 / 10,
            "{} of {before}",
            snow_on(&mut w, b)
        );
        assert!(w.flakes.len() < 20, "little or nothing fell");
    }

    #[test]
    fn shake_throws_snow_off() {
        let mut w = world();
        let a = rect(40.0, 200.0);
        w.set_windows(&[Ledge::whole(7, a)]);
        pile(&mut w, a);
        w.time += 0.05;
        let b = rect(200.0, 200.0); // 160 px in 50 ms
        w.set_windows(&[Ledge::whole(7, b)]);
        assert_eq!(snow_on(&mut w, b), 0);
        assert!(
            w.flakes
                .iter()
                .filter(|f| f.kind == FlakeKind::Chunk)
                .count()
                > 50
        );
        assert!(w.flakes.iter().any(|f| f.vx > 400.0), "flung sideways");
    }

    #[test]
    fn icicles_grow_under_snow_and_fall_when_window_closes() {
        let mut w = world();
        w.cfg.icicles = true;
        let a = rect(40.0, 200.0);
        w.set_windows(&[Ledge::whole(7, a)]);
        pile(&mut w, a);
        for _ in 0..240 {
            w.step_icicles(1.0);
        }
        assert!(!w.icicles.is_empty());
        assert!(w.icicles.iter().all(|ic| ic.len > 1.5));
        let n = w.flakes.len();
        w.set_windows(&[]);
        assert!(w.icicles.is_empty());
        assert!(w.flakes.len() > n, "shards fall");
    }
}
