//! Penguins: walking, sledding, footprints, and play-mode interaction.

use super::balls::{Ball, GRAVITY};
use super::grid::{EMPTY, ICE, SNOW};
use super::world::{STEP, World};
use super::{Instance, SHAPE_ELLIPSE, SHAPE_RECT, SHAPE_TRI, premul};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CritterState {
    Walk,
    Idle,
    Sled,
    /// In the air: hopped after a poke, dropped or thrown by the user.
    Air,
    /// Picked up and carried by the pointer (play mode).
    Held,
    /// Knocked over by a snowball or a hard landing; gets up again.
    Fallen,
}

/// A penguin. Positions are the feet, in monitor pixels.
#[derive(Clone, Debug)]
pub struct Critter {
    pub x: f32,
    pub y: f32,
    pub dir: f32,
    pub state: CritterState,
    pub t: f32,
    pub step_dist: f32,
    pub vx: f32,
    pub vy: f32,
    /// Per-penguin size variation (multiplied by the configured size).
    pub size: f32,
    /// Seconds of running away left (after being poked).
    pub flee: f32,
    /// Lying direction while fallen (-1 or 1).
    pub fall_dir: f32,
    /// Falls over when it lands (hit by a snowball).
    pub tumble: bool,
    /// Seconds spent in the air so far.
    pub air_time: f32,
    /// Seconds of snowball fight left (after being hit by the user).
    pub anger: f32,
    /// Seconds until it can throw again.
    pub throw_cd: f32,
    /// Throwing animation time left.
    pub throw_anim: f32,
    /// Pirouette time left (skating on ice).
    pub spin: f32,
}

const SPIN_TIME: f32 = 0.7;

/// Most penguins per monitor.
pub const MAX_PENGUINS: u32 = 12;
const PENGUIN_GRAVITY: f32 = 1300.0;

impl World {
    /// Pixel scale of penguin `c` (DPI scale x configured size x own size).
    pub fn penguin_scale(&self, c: &Critter) -> f32 {
        self.scale * self.cfg.penguin_size.clamp(0.5, 3.0) * c.size
    }

    pub(crate) fn spawn_critter(&self) -> Critter {
        let x = 30.0 + fastrand::f32() * (self.w - 60.0).max(1.0);
        Critter {
            x,
            y: self.surface_px(x),
            dir: if fastrand::bool() { 1.0 } else { -1.0 },
            state: CritterState::Walk,
            t: 1.0 + fastrand::f32() * 3.0,
            step_dist: 0.0,
            vx: 0.0,
            vy: 0.0,
            size: 0.85 + fastrand::f32() * 0.3,
            flee: 0.0,
            fall_dir: 1.0,
            tumble: false,
            air_time: 0.0,
            anger: 0.0,
            throw_cd: 0.0,
            throw_anim: 0.0,
            spin: 0.0,
        }
    }

    /// Ground under a penguin at x whose feet are at `feet` (px), if it can
    /// get there: snow or bricks taller than `climb` in its way block it.
    /// Feet-relative, so a penguin walks under window ledges and into an
    /// igloo instead of jumping onto whatever is highest.
    pub(crate) fn penguin_ground(&self, x: f32, feet: f32, climb: f32) -> Option<f32> {
        let cx = (x / self.cell) as i32;
        let from = ((feet - climb) / self.cell).floor() as i32;
        if from >= 0 && self.grid.get(cx, from) != EMPTY {
            return None;
        }
        Some(self.grid.surface_from(cx, from) as f32 * self.cell)
    }

    /// Ground below a falling penguin.
    fn fall_ground(&self, x: f32, feet: f32) -> f32 {
        let cx = (x / self.cell) as i32;
        let row = ((feet - 2.0) / self.cell).floor() as i32;
        self.grid.surface_from(cx, row) as f32 * self.cell
    }

    pub(crate) fn step_critters(&mut self) {
        if !self.cfg.critter {
            self.critters.clear();
            return;
        }
        // With several monitors the engine balances penguins between them
        // (they walk across); alone, each world keeps its own count.
        if !self.shared_penguins {
            let want = self.cfg.penguins.clamp(1, MAX_PENGUINS) as usize;
            while self.critters.len() < want {
                let c = self.spawn_critter();
                self.critters.push(c);
            }
            self.critters.truncate(want);
        }
        let mut list = std::mem::take(&mut self.critters);
        let mut throws = Vec::new();
        list.retain_mut(|c| {
            let keep = self.step_one_critter(c);
            if let Some(t) = self.penguin_throw(c) {
                throws.push(t);
            }
            keep
        });
        self.critters = list;
        for b in throws {
            if self.balls.len() < super::world::MAX_BALLS {
                self.balls.push(b);
            }
        }
    }

    /// Advance one penguin. Returns false if it walked off an open screen
    /// side (it is then in `leaving`, for the engine to hand to the next
    /// monitor).
    fn step_one_critter(&mut self, c: &mut Critter) -> bool {
        let (w, cell) = (self.w, self.cell);
        let k = self.penguin_scale(c);
        let sc = self.scale;
        let climb = (6.0 * k).max(2.5 * cell);
        c.t -= STEP;
        c.flee = (c.flee - STEP).max(0.0);
        c.anger = (c.anger - STEP).max(0.0);
        c.throw_cd = (c.throw_cd - STEP).max(0.0);
        c.throw_anim = (c.throw_anim - STEP).max(0.0);
        c.spin = (c.spin - STEP).max(0.0);
        // The cell the feet stand on (feet sit on its top edge, give or take).
        let feet_row = ((c.y + cell * 0.5) / cell) as i32;
        let cx = (c.x / cell) as i32;
        let on_ice = self.grid.get(cx, feet_row) == ICE;
        let reach = ((4.0 * k / cell).round() as i32).max(2);
        let from = ((c.y - climb) / cell).floor() as i32;
        let last = self.grid.w as i32 - 1;
        let slope = self.grid.surface_from((cx + reach).clamp(0, last), from)
            - self.grid.surface_from((cx - reach).clamp(0, last), from);
        match c.state {
            CritterState::Held => return true,
            CritterState::Air => {
                c.air_time += STEP;
                c.vy += PENGUIN_GRAVITY * sc * STEP;
                c.x += c.vx * STEP;
                c.y += c.vy * STEP;
                if let Some(right) = self.off_open_side(c.x, 0.0) {
                    self.leaving.push((c.clone(), right));
                    return false;
                }
                if c.x < 10.0 || c.x > w - 10.0 {
                    c.x = c.x.clamp(10.0, w - 10.0);
                    c.vx = -c.vx * 0.5;
                }
                let ground = self.fall_ground(c.x, c.y - c.vy * STEP);
                if c.vy > 0.0 && c.y >= ground {
                    c.y = ground;
                    let hard = c.vy > 900.0 * sc || c.vx.abs() > 600.0 * sc;
                    if c.tumble || hard {
                        c.state = CritterState::Fallen;
                        if !c.tumble {
                            c.fall_dir = if c.vx >= 0.0 { 1.0 } else { -1.0 };
                        }
                        c.t = 1.5;
                    } else if c.flee > 0.0 {
                        c.state = CritterState::Walk;
                        c.t = c.flee;
                    } else if c.air_time < 0.35 {
                        // Stepped off a small ledge: keep walking.
                        c.state = CritterState::Walk;
                        c.t = 1.0 + fastrand::f32() * 3.0;
                    } else {
                        c.state = CritterState::Idle;
                        c.t = 0.6;
                    }
                    c.vx = 0.0;
                    c.vy = 0.0;
                    c.tumble = false;
                    c.air_time = 0.0;
                }
                return true;
            }
            CritterState::Fallen => {
                c.vx *= 0.9;
                if c.t <= 0.0 {
                    c.state = CritterState::Idle;
                    c.t = 0.5;
                }
            }
            CritterState::Walk => {
                let speed = if c.flee > 0.0 { 75.0 } else { 28.0 };
                let want = c.dir * speed * k;
                if on_ice {
                    // Skating: speed builds and fades slowly, the odd twirl.
                    c.vx += (want * 1.6 - c.vx) * (0.9 * STEP);
                    if c.spin <= 0.0 && c.vx.abs() > 15.0 * k && fastrand::u16(..) < 220 {
                        c.spin = SPIN_TIME;
                    }
                } else {
                    c.vx = want;
                }
                if c.t <= 0.0 {
                    c.state = CritterState::Idle;
                    c.t = 1.0 + fastrand::f32() * 2.0;
                    if !on_ice {
                        c.vx = 0.0;
                    }
                } else if c.flee <= 0.0
                    && !on_ice
                    && slope.abs() >= 2 * reach
                    && fastrand::u8(..) < 4
                {
                    c.dir = slope.signum() as f32;
                    c.state = CritterState::Sled;
                    c.t = 4.0;
                }
            }
            CritterState::Idle => {
                if on_ice {
                    c.vx *= 1.0 - 0.5 * STEP; // glides to a stop
                } else {
                    c.vx = 0.0;
                }
                if c.t <= 0.0 {
                    c.state = CritterState::Walk;
                    c.t = 3.0 + fastrand::f32() * 5.0;
                    if fastrand::bool() {
                        c.dir = -c.dir;
                    }
                }
            }
            CritterState::Sled => {
                let a = (slope as f32 / (2.0 * reach as f32)).clamp(-1.0, 1.0);
                c.vx = (c.vx + a * 220.0 * sc * STEP).clamp(-160.0 * sc, 160.0 * sc);
                if c.vx.abs() < 12.0 * sc && c.t <= 0.0 || slope.abs() < 2 && c.t <= 0.0 {
                    c.state = CritterState::Idle;
                    c.t = 1.0;
                }
            }
        }
        let nx = c.x + c.vx * STEP;
        if let Some(right) = self.off_open_side(nx, -15.0) {
            c.x = nx;
            self.leaving.push((c.clone(), right));
            return false;
        }
        // Past the screen edge toward another monitor: keep walking level.
        let beyond = (nx < 0.0 && self.open_sides.0) || (nx >= w && self.open_sides.1);
        let ground = if beyond {
            Some(c.y)
        } else {
            self.penguin_ground(nx, c.y, climb)
        };
        match ground {
            // A wall (igloo, brick fort, steep pile): turn around.
            None => {
                c.dir = -c.dir;
                c.vx = -c.vx * 0.3;
            }
            Some(target) => {
                c.x = nx;
                if target > c.y + 10.0 * k {
                    // Walked off an edge: fall.
                    c.state = CritterState::Air;
                    c.air_time = 0.0;
                    c.vx = c.vx.clamp(-80.0 * sc, 80.0 * sc);
                    c.vy = 0.0;
                } else {
                    c.y += (target - c.y) * 0.25;
                }
            }
        }
        let closed_left = c.x < 20.0 && !self.open_sides.0;
        let closed_right = c.x > w - 20.0 && !self.open_sides.1;
        if closed_left || closed_right {
            c.x = c.x.clamp(20.0, w - 20.0);
            c.dir = -c.dir;
            c.vx = 0.0;
        }
        // Footprints.
        if c.state == CritterState::Walk {
            c.step_dist += (c.vx * STEP).abs();
            if c.step_dist > 7.0 * k {
                c.step_dist = 0.0;
                let fx = (c.x / cell) as i32;
                let fy = self.grid.surface_from(fx, ((c.y - climb) / cell) as i32);
                if self.grid.get(fx, fy) == SNOW {
                    self.grid.set(fx, fy, EMPTY);
                }
            }
        }
        true
    }

    /// Some(true/false) if x is past an open right/left side by `margin` px.
    fn off_open_side(&self, x: f32, margin: f32) -> Option<bool> {
        if self.open_sides.0 && x < margin {
            Some(false)
        } else if self.open_sides.1 && x > self.w - margin {
            Some(true)
        } else {
            None
        }
    }

    /// An angry penguin throws a snowball at the pointer (or somewhere near
    /// it) every couple of seconds.
    fn penguin_throw(&self, c: &mut Critter) -> Option<Ball> {
        if c.anger <= 0.0
            || c.throw_cd > 0.0
            || !matches!(c.state, CritterState::Idle | CritterState::Walk)
        {
            return None;
        }
        let k = self.penguin_scale(c);
        let (tx, ty) = self.target.unwrap_or_else(|| {
            (
                c.x + (fastrand::f32() - 0.5) * self.w * 0.5,
                c.y - fastrand::f32() * self.h * 0.3,
            )
        });
        c.dir = if tx >= c.x { 1.0 } else { -1.0 };
        let (sx, sy) = (c.x + c.dir * 13.0 * k, c.y - 22.0 * k);
        let (dx, dy) = (tx - sx, ty - sy);
        let t = (dx.hypot(dy) / (700.0 * self.scale)).clamp(0.45, 1.1);
        let mut b = Ball::new(sx, sy, 5.0 * self.scale);
        b.vx = dx / t;
        b.vy = dy / t - 0.5 * GRAVITY * t;
        b.thrown = true;
        b.by_penguin = true;
        c.throw_cd = 1.2 + fastrand::f32() * 1.6;
        c.throw_anim = 0.3;
        c.state = CritterState::Idle;
        c.t = 0.5;
        c.vx = 0.0;
        Some(b)
    }

    // ---- penguin interaction (play mode) --------------------------------

    /// Index of the penguin under (x, y), topmost first.
    pub fn penguin_at(&self, x: f32, y: f32) -> Option<usize> {
        self.critters.iter().enumerate().rev().find_map(|(i, c)| {
            let k = self.penguin_scale(c);
            let (cx, cy) = (c.x, c.y - 11.0 * k);
            let (rx, ry) = (11.0 * k, 15.0 * k);
            let (dx, dy) = ((x - cx) / rx, (y - cy) / ry);
            (dx * dx + dy * dy <= 1.0).then_some(i)
        })
    }

    /// A click: the penguin hops and runs away from the pointer.
    pub fn poke_penguin(&mut self, i: usize, from_x: f32) {
        let sc = self.scale;
        let Some(c) = self.critters.get_mut(i) else {
            return;
        };
        if matches!(c.state, CritterState::Held | CritterState::Air) {
            return;
        }
        let away = if c.x >= from_x { 1.0 } else { -1.0 };
        c.dir = away;
        c.state = CritterState::Air;
        c.air_time = 0.0;
        c.vx = away * 90.0 * sc;
        c.vy = -380.0 * sc;
        c.flee = 2.0;
    }

    /// The carried penguin: `i`, unless the list changed under it (another
    /// penguin walked off to a different monitor).
    fn held_index(&self, i: usize) -> usize {
        if self
            .critters
            .get(i)
            .is_some_and(|c| c.state == CritterState::Held)
        {
            i
        } else {
            self.critters
                .iter()
                .position(|c| c.state == CritterState::Held)
                .unwrap_or(usize::MAX)
        }
    }

    pub fn grab_penguin(&mut self, i: usize) {
        if let Some(c) = self.critters.get_mut(i) {
            c.state = CritterState::Held;
            c.vx = 0.0;
            c.vy = 0.0;
        }
    }

    /// Move a held penguin so its body hangs from the pointer.
    pub fn drag_penguin(&mut self, i: usize, x: f32, y: f32) {
        let i = self.held_index(i);
        let k = self.critters.get(i).map(|c| self.penguin_scale(c));
        if let (Some(c), Some(k)) = (self.critters.get_mut(i), k)
            && c.state == CritterState::Held
        {
            if (x - c.x).abs() > 0.5 {
                c.dir = (x - c.x).signum();
            }
            c.x = x.clamp(10.0, self.w - 10.0);
            c.y = y + 20.0 * k;
        }
    }

    /// Let go: it falls (or flies, if flicked) with the pointer's velocity.
    pub fn release_penguin(&mut self, i: usize, vx: f32, vy: f32) {
        let i = self.held_index(i);
        let max = 1800.0 * self.scale;
        if let Some(c) = self.critters.get_mut(i)
            && c.state == CritterState::Held
        {
            c.state = CritterState::Air;
            c.air_time = 0.0;
            c.vx = vx.clamp(-max, max);
            c.vy = vy.clamp(-max, max);
        }
    }

    /// A thrown snowball hit penguin `i`: it falls over. A hit from the
    /// user starts a snowball fight: it and the penguins nearby throw back.
    pub(crate) fn knock_penguin(&mut self, i: usize, ball_vx: f32, by_user: bool) {
        let sc = self.scale;
        let Some(c) = self.critters.get_mut(i) else {
            return;
        };
        if c.state == CritterState::Held {
            return;
        }
        c.fall_dir = if ball_vx >= 0.0 { 1.0 } else { -1.0 };
        c.state = CritterState::Air;
        c.vx = ball_vx.clamp(-200.0 * sc, 200.0 * sc) * 0.5;
        c.vy = -200.0 * sc;
        c.flee = 0.0;
        c.tumble = true;
        c.air_time = 0.0;
        if by_user {
            let x = c.x;
            c.anger = 9.0;
            c.throw_cd = 1.6;
            for o in self.critters.iter_mut() {
                if (o.x - x).abs() < 600.0 * sc && o.anger < 6.0 {
                    o.anger = 7.0;
                    o.throw_cd = o.throw_cd.max(0.4 + fastrand::f32() * 1.4);
                }
            }
        }
    }

    /// Thrown balls that hit a penguin this step (ball index, penguin index).
    pub(crate) fn penguin_hits(&self) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for (bi, b) in self.balls.iter().enumerate().filter(|(_, b)| b.thrown) {
            if let Some(pi) = self.penguin_at(b.x, b.y) {
                out.push((bi, pi));
            }
        }
        out
    }

    pub(crate) fn push_critters(&self, out: &mut Vec<Instance>) {
        for (i, c) in self.critters.iter().enumerate() {
            self.push_critter(out, c, i as f32);
        }
    }

    fn push_critter(&self, out: &mut Vec<Instance>, c: &Critter, seed: f32) {
        let k = self.penguin_scale(c);
        let d = if c.dir >= 0.0 { 1.0 } else { -1.0 };
        let t = self.time + seed * 1.37;
        let sled = c.state == CritterState::Sled;
        let spin = if c.spin > 0.0 {
            (1.0 - c.spin / SPIN_TIME) * std::f32::consts::TAU * d
        } else {
            0.0
        };
        let rot = spin
            + match c.state {
                CritterState::Walk => (t * if c.flee > 0.0 { 16.0 } else { 9.0 }).sin() * 0.12,
                CritterState::Sled => d * 1.35,
                CritterState::Held => (t * 6.0).sin() * 0.25,
                CritterState::Fallen => c.fall_dir * 1.45,
                CritterState::Air => -d * (c.vx / (400.0 * self.scale)).clamp(-0.5, 0.5),
                CritterState::Idle => 0.0,
            };
        // Body centre; when lying down it sits lower.
        let lying = c.state == CritterState::Fallen;
        let (bx, by) = if lying {
            (c.x, c.y - 7.0 * k)
        } else {
            (c.x, c.y - 11.0 * k)
        };
        let (sn, cs) = rot.sin_cos();
        let dark = premul([0.08, 0.09, 0.14], 1.0);
        let orange = premul([1.0, 0.6, 0.1], 1.0);
        // Offsets are in penguin units, rotated with the body.
        let push = |out: &mut Vec<Instance>, ox: f32, oy: f32, hx: f32, hy: f32, shape, col| {
            let (ox, oy) = (ox * d * k, oy * k);
            out.push(Instance {
                pos: [bx + ox * cs - oy * sn, by + ox * sn + oy * cs],
                half: [hx * k, hy * k],
                rot: if shape == SHAPE_TRI { rot * 0.5 } else { rot },
                shape,
                color: col,
            })
        };
        if sled {
            out.push(Instance {
                pos: [c.x, c.y + 1.0 * k],
                half: [16.0 * k, 1.5 * k],
                rot: 0.0,
                shape: SHAPE_RECT,
                color: premul([0.55, 0.35, 0.2], 1.0),
            });
        }
        // Flippers flap while airborne or carried.
        let flap = if matches!(c.state, CritterState::Air | CritterState::Held) {
            (t * 22.0).sin() * 3.0
        } else if c.throw_anim > 0.0 {
            9.0 // throwing arm up
        } else {
            0.0
        };
        push(out, -6.5, 1.0 - flap, 2.0, 5.5, SHAPE_ELLIPSE, dark);
        push(out, 0.0, 0.0, 7.5, 11.0, SHAPE_ELLIPSE, dark);
        push(
            out,
            1.5,
            2.0,
            4.5,
            8.0,
            SHAPE_ELLIPSE,
            premul([0.95, 0.95, 0.98], 1.0),
        );
        // Eye: an X while knocked over.
        let eye = if lying {
            premul([0.6, 0.6, 0.7], 1.0)
        } else {
            premul([1.0, 1.0, 1.0], 1.0)
        };
        push(out, 2.0, -8.0, 1.0, 1.0, SHAPE_ELLIPSE, eye);
        push(out, 6.0, -7.0, 3.0, 1.6, SHAPE_TRI, orange);
        if !lying {
            push(out, -2.0, 11.0, 3.0, 1.4, SHAPE_ELLIPSE, orange);
            push(out, 4.0, 11.0, 3.0, 1.4, SHAPE_ELLIPSE, orange);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::sim::world::World;

    fn world(n: u32, size: f32) -> World {
        let cfg = Config {
            intensity: 0.0,
            critter: true,
            penguins: n,
            penguin_size: size,
            ..Config::default()
        };
        let mut w = World::new(800, 400, 1.0, &cfg);
        w.step();
        w
    }

    /// Regression: snow sliding off window ledges piled up without limit
    /// under the window edges.
    #[test]
    fn ledge_overflow_respects_max_pile() {
        let cfg = Config {
            intensity: 1.0,
            gusts: false,
            wind: 0.0,
            max_pile: 0.1,
            ..Config::default()
        };
        let mut w = World::new(400, 400, 1.0, &cfg);
        w.set_windows(&[crate::sim::ledges::Ledge::whole(
            1,
            crate::sim::Rect {
                x: 120.0,
                y: 120.0,
                w: 160.0,
                h: 80.0,
            },
        )]);
        for _ in 0..60 * 120 {
            w.step();
        }
        let limit = (0.1 * w.grid.h as f32) as i32;
        let gh = w.grid.h as i32;
        for x in 0..w.grid.w as i32 {
            // Height of the bottom pile in this column.
            let mut n = 0;
            while n < gh && w.grid.get(x, gh - 1 - n) != crate::sim::grid::EMPTY {
                n += 1;
            }
            assert!(n <= limit + 3, "column {x}: pile {n} > limit {limit}");
        }
    }

    #[test]
    fn penguin_count_follows_config_and_is_capped() {
        let mut w = world(5, 1.0);
        assert_eq!(w.critters.len(), 5);
        w.cfg.penguins = 2;
        w.step();
        assert_eq!(w.critters.len(), 2);
        w.cfg.penguins = 1000;
        w.step();
        assert_eq!(w.critters.len(), MAX_PENGUINS as usize);
        w.cfg.critter = false;
        w.step();
        assert!(w.critters.is_empty());
    }

    #[test]
    fn bigger_penguins_are_bigger_targets() {
        let mut w = world(1, 3.0);
        let c = w.critters[0].clone();
        // 25 px above the feet is inside a 3x penguin, outside a 1x one.
        assert_eq!(w.penguin_at(c.x, c.y - 40.0), Some(0));
        w.cfg.penguin_size = 1.0;
        assert_eq!(w.penguin_at(c.x, c.y - 40.0), None);
    }

    #[test]
    fn poke_makes_it_hop_and_flee_then_land() {
        let mut w = world(1, 1.0);
        w.critters[0].x = 300.0;
        let x0 = w.critters[0].x;
        w.poke_penguin(0, x0 - 5.0);
        assert_eq!(w.critters[0].state, CritterState::Air);
        assert!(w.critters[0].vy < 0.0);
        for _ in 0..120 {
            w.step();
        }
        let c = &w.critters[0];
        assert_ne!(c.state, CritterState::Air, "landed");
        assert!(c.x > x0, "ran away from the click: {} -> {}", x0, c.x);
    }

    #[test]
    fn carry_and_fling() {
        let mut w = world(1, 1.0);
        w.grab_penguin(0);
        w.drag_penguin(0, 300.0, 100.0);
        w.step();
        assert_eq!(w.critters[0].state, CritterState::Held);
        assert_eq!(w.critters[0].x, 300.0);
        w.release_penguin(0, 600.0, -500.0);
        let mut max_x = 0.0f32;
        for _ in 0..240 {
            w.step();
            max_x = max_x.max(w.critters[0].x);
        }
        let c = &w.critters[0];
        assert!(max_x > 450.0, "flew right: {max_x}");
        assert!((c.y - w.surface_px(c.x)).abs() < 2.0, "back on the ground");
    }

    #[test]
    fn snowball_knocks_penguin_over() {
        let mut w = world(1, 1.0);
        for _ in 0..30 {
            w.step();
        }
        let c = w.critters[0].clone();
        w.critters[0].state = CritterState::Idle;
        w.critters[0].t = 10.0;
        w.throw_ball(c.x - 30.0, c.y - 11.0, 900.0, 0.0);
        let mut fell = false;
        for _ in 0..90 {
            w.step();
            fell |= w.critters[0].state == CritterState::Fallen;
        }
        assert!(fell, "penguin should fall over");
        assert!(w.balls.iter().all(|b| !b.thrown), "ball splatted");
    }

    #[test]
    fn penguins_glide_on_ice() {
        let mut w = world(1, 1.0);
        let gh = w.grid.h as i32;
        for x in 0..w.grid.w as i32 {
            w.grid.set(x, gh - 1, crate::sim::grid::ICE);
        }
        let c = &mut w.critters[0];
        c.x = 300.0;
        c.y = (gh - 1) as f32 * w.cell;
        c.state = CritterState::Walk;
        c.dir = 1.0;
        c.t = 3.0;
        // Keep the rink clear of the few flakes that still fall.
        let sweep = |w: &mut World| {
            for x in 0..w.grid.w as i32 {
                w.grid.set(x, gh - 2, crate::sim::grid::EMPTY);
            }
        };
        for _ in 0..180 {
            w.step();
            sweep(&mut w);
        }
        // Stopped walking but still sliding.
        w.critters[0].state = CritterState::Idle;
        w.critters[0].t = 10.0;
        let v0 = w.critters[0].vx;
        w.step();
        assert!(v0.abs() > 10.0, "built up speed on ice: {v0}");
        let c = &w.critters[0];
        assert!(
            c.vx.abs() > 10.0,
            "glides while idle: {:?}",
            (c.x, c.y, c.vx, c.state, c.dir, v0)
        );
    }

    #[test]
    fn hit_penguins_throw_back_at_the_pointer() {
        let mut w = world(3, 1.0);
        for (i, c) in w.critters.iter_mut().enumerate() {
            c.x = 200.0 + i as f32 * 60.0;
        }
        w.target = Some((600.0, 100.0));
        w.knock_penguin(0, 300.0, true);
        let mut thrown = 0;
        for _ in 0..60 * 6 {
            w.step();
            thrown = thrown.max(w.balls.iter().filter(|b| b.by_penguin).count());
        }
        assert!(thrown >= 2, "several penguins joined in: {thrown}");
        // Penguin snowballs do not start new fights by themselves.
        assert!(w.critters.iter().all(|c| c.anger < 9.0));
    }

    #[test]
    fn penguin_walks_off_an_open_side() {
        let mut w = world(1, 1.0);
        w.open_sides = (false, true);
        let c = &mut w.critters[0];
        c.x = w.w - 25.0;
        c.dir = 1.0;
        c.state = CritterState::Walk;
        c.t = 10.0;
        for _ in 0..240 {
            w.step();
            if !w.leaving.is_empty() {
                break;
            }
        }
        let c = w.critters.first();
        assert_eq!(
            w.leaving.len(),
            1,
            "{:?}",
            c.map(|c| (c.x, c.y, c.state, c.dir, c.vx))
        );
        assert!(w.leaving[0].1, "right side");
    }

    #[test]
    fn walls_turn_penguins_around() {
        let mut w = world(1, 1.0);
        let gh = w.grid.h as i32;
        let wall_x = (400.0 / w.cell) as i32;
        for y in gh - 30..gh {
            for x in wall_x..wall_x + 3 {
                w.grid.set(x, y, crate::sim::grid::BRICK);
            }
        }
        let c = &mut w.critters[0];
        c.x = 300.0;
        c.dir = 1.0;
        c.state = CritterState::Walk;
        c.t = 100.0;
        for _ in 0..60 * 8 {
            w.step();
            w.critters[0].t = 100.0;
            assert!(w.critters[0].x < 400.0, "went through/over the wall");
        }
    }
}
