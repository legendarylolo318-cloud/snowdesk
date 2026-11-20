//! Per-monitor snow world: flakes, settled grid, balls, windows, critter.

use super::balls::{self, Ball, DecorKind};
use super::grid::{EMPTY, Grid, ICE, PACKED, SNOW, WALL};
use super::{
    Instance, Rect, SHAPE_ELLIPSE, SHAPE_GLOW, SHAPE_RECT, SHAPE_SOFT, SHAPE_STAR, SHAPE_TRI,
    premul,
};
use crate::config::Config;

pub const STEP: f32 = 1.0 / 60.0;
pub const MAX_FLAKES: usize = 20_000;
/// Ball-ball collisions are O(n²); keep large builds bounded.
pub const MAX_BALLS: usize = 200;
const UNDO_STEPS: usize = 30;
const LEDGE_ROWS: i32 = 3;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FlakeKind {
    Snow,
    Water,
    Chunk,
}

#[derive(Clone, Copy, Debug)]
pub struct Flake {
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    pub depth: f32,
    pub phase: f32,
    pub spin: f32,
    pub rot: f32,
    pub kind: FlakeKind,
    /// Temporary flakes vanish on landing instead of respawning.
    pub temp: bool,
}

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
}

/// Most penguins per monitor.
pub const MAX_PENGUINS: u32 = 12;
const PENGUIN_GRAVITY: f32 = 1300.0;

#[derive(Clone)]
pub(crate) struct Snapshot {
    runs: Vec<(u8, u32)>,
    balls: Vec<Ball>,
}

impl Snapshot {
    fn take(grid: &Grid, balls: &[Ball]) -> Self {
        let mut runs: Vec<(u8, u32)> = Vec::new();
        for &c in &grid.cells {
            match runs.last_mut() {
                Some((v, n)) if *v == c => *n += 1,
                _ => runs.push((c, 1)),
            }
        }
        runs.shrink_to_fit();
        Self {
            runs,
            balls: balls.to_vec(),
        }
    }

    fn cells(&self, len: usize) -> Vec<u8> {
        let mut v = Vec::with_capacity(len);
        for &(c, n) in &self.runs {
            v.extend(std::iter::repeat_n(c, n as usize));
        }
        v.resize(len, EMPTY);
        v
    }
}

pub struct World {
    pub grid: Grid,
    pub cell: f32,
    pub w: f32,
    pub h: f32,
    pub scale: f32,
    pub cfg: Config,
    pub(crate) flakes: Vec<Flake>,
    n_main: usize,
    pub balls: Vec<Ball>,
    pub snowmen: Vec<Vec<usize>>,
    pub(crate) windows: Vec<Rect>,
    cursor: Option<(f32, f32)>,
    cursor_prev: Option<(f32, f32)>,
    gust: f32,
    gust_target: f32,
    gust_timer: f32,
    kick: f32,
    accum: f32,
    pub time: f32,
    ticks: u32,
    pub critters: Vec<Critter>,
    /// Play mode: the user is building, so falling cells stay cells.
    pub play: bool,
    pub(crate) undo: Vec<Snapshot>,
    pub(crate) redo: Vec<Snapshot>,
    pub(crate) rolling: Option<usize>,
    pub(crate) removed_acc: f32,
}

#[inline]
fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl World {
    /// `w`,`h` in physical pixels.
    pub fn new(w: u32, h: u32, scale: f32, cfg: &Config) -> Self {
        let cell = (3.5 * scale).round().max(2.0);
        let gw = (w as f32 / cell).ceil() as usize;
        let gh = (h as f32 / cell).ceil() as usize;
        Self {
            grid: Grid::new(gw, gh),
            cell,
            w: w as f32,
            h: h as f32,
            scale,
            cfg: cfg.clone(),
            flakes: Vec::with_capacity(4096),
            n_main: 0,
            balls: Vec::new(),
            snowmen: Vec::new(),
            windows: Vec::new(),
            cursor: None,
            cursor_prev: None,
            gust: 0.0,
            gust_target: 0.0,
            gust_timer: 2.0,
            kick: 0.0,
            accum: 0.0,
            time: 0.0,
            ticks: 0,
            critters: Vec::new(),
            play: false,
            undo: Vec::new(),
            redo: Vec::new(),
            rolling: None,
            removed_acc: 0.0,
        }
    }

    #[cfg(test)]
    pub fn flake_count(&self) -> usize {
        self.flakes.len()
    }

    pub fn set_cursor(&mut self, p: Option<(f32, f32)>) {
        self.cursor = p;
    }

    /// A bass hit from the audio tap: gust plus a burst of flakes.
    pub fn beat(&mut self, strength: f32) {
        if !self.cfg.music_reactive {
            return;
        }
        self.kick = (self.kick + strength).min(1.5);
        self.gust_target = (if fastrand::bool() { 1.0 } else { -1.0 }) * (0.6 + self.kick * 0.4);
        self.gust_timer = 0.6;
        let n = (20.0 * self.area_factor()) as usize;
        for _ in 0..n {
            if self.flakes.len() < MAX_FLAKES {
                let mut f = self.new_flake(false);
                f.temp = true;
                f.vy += 60.0 * self.scale;
                self.flakes.push(f);
            }
        }
    }

    fn area_factor(&self) -> f32 {
        (self.w * self.h) / (1920.0 * 1080.0 * self.scale * self.scale)
    }

    fn target_flakes(&self) -> usize {
        let i = self.cfg.intensity.clamp(0.0, 1.0);
        let n = (50.0 + 2900.0 * i * i) * self.area_factor();
        (n as usize).min(MAX_FLAKES - 2000)
    }

    fn new_flake(&self, anywhere: bool) -> Flake {
        let depth = 0.35 + fastrand::f32() * 0.65;
        Flake {
            x: fastrand::f32() * self.w,
            y: if anywhere {
                fastrand::f32() * self.h
            } else {
                -10.0 * self.scale - fastrand::f32() * 40.0
            },
            vx: 0.0,
            vy: (40.0 + 80.0 * depth) * self.scale,
            depth,
            phase: fastrand::f32() * std::f32::consts::TAU,
            spin: (fastrand::f32() - 0.5) * 3.0,
            rot: 0.0,
            kind: FlakeKind::Snow,
            temp: false,
        }
    }

    /// Advance by real elapsed seconds using fixed steps. Returns steps run.
    pub fn advance(&mut self, dt: f32) -> u32 {
        self.accum = (self.accum + dt).min(STEP * 5.0);
        let mut n = 0;
        while self.accum >= STEP {
            self.accum -= STEP;
            self.step();
            n += 1;
        }
        n
    }

    pub fn step(&mut self) {
        self.time += STEP;
        // Gust random walk.
        self.gust_timer -= STEP;
        if self.gust_timer <= 0.0 {
            self.gust_target = if self.cfg.gusts && fastrand::u8(..) < 120 {
                (fastrand::f32() * 2.0 - 1.0) * 0.8
            } else {
                0.0
            };
            self.gust_timer = 2.0 + fastrand::f32() * 5.0;
        }
        self.gust += (self.gust_target - self.gust) * STEP * 1.2;
        self.kick *= 1.0 - STEP * 2.0;

        // Population control.
        let target = self.target_flakes();
        if self.n_main < target {
            for _ in 0..6.min(target - self.n_main) {
                let anywhere = self.flakes.is_empty() || self.time < 1.0;
                let f = self.new_flake(anywhere);
                self.flakes.push(f);
                self.n_main += 1;
            }
        } else if self.n_main > target + 20
            && let Some(f) = self
                .flakes
                .iter_mut()
                .find(|f| !f.temp && f.kind == FlakeKind::Snow)
        {
            f.temp = true;
            self.n_main -= 1;
        }

        self.step_flakes();
        self.grid.free_fall_to_flakes = !self.play;
        self.grid.step();
        self.cells_to_flakes();
        self.natural_melt();

        let mut hits = balls::step_balls(&mut self.balls, &self.grid, self.cell, STEP);
        if !self.critters.is_empty() {
            for (bi, pi) in self.penguin_hits() {
                let vx = self.balls[bi].vx;
                self.knock_penguin(pi, vx);
                if !hits.iter().any(|h| h.0 == bi) {
                    hits.push((bi, 2));
                }
            }
            hits.sort_unstable();
        }
        if !hits.is_empty() {
            self.resolve_throws(&hits);
        }
        self.ticks = self.ticks.wrapping_add(1);
        if self.ticks.is_multiple_of(20) {
            self.update_snowmen();
        }
        self.step_critters();
        self.cursor_prev = self.cursor;
    }

    fn pile_height_ok(&self, cx: i32, cy: i32) -> bool {
        let limit = ((self.cfg.max_pile * self.grid.h as f32) as i32).max(2);
        let mut n = 0;
        let mut y = cy + 1;
        while n < limit {
            match self.grid.get(cx, y) {
                SNOW | PACKED | ICE => n += 1,
                _ => break,
            }
            y += 1;
        }
        // Piles on window tops stay lower (about a third of the limit).
        if self.grid.get(cx, y) == WALL {
            return n < (limit / 3).max(3);
        }
        n < limit
    }

    fn step_flakes(&mut self) {
        let sc = self.scale;
        let wind = self.cfg.wind * 0.8
            + if self.cfg.gusts || self.kick > 0.05 {
                self.gust
            } else {
                0.0
            };
        let cur = if self.cfg.cursor_wind {
            self.cursor
        } else {
            None
        };
        let (cvx, cvy) = match (cur, self.cursor_prev) {
            (Some(a), Some(b)) => ((a.0 - b.0) / STEP, (a.1 - b.1) / STEP),
            _ => (0.0, 0.0),
        };
        let radius = 120.0 * sc;
        let mut i = 0;
        while i < self.flakes.len() {
            let mut f = self.flakes[i];
            let mut land = false;
            match f.kind {
                FlakeKind::Snow => {
                    f.phase += STEP * (0.8 + f.depth);
                    let tvx =
                        wind * 90.0 * sc * (0.4 + f.depth) + f.phase.sin() * 14.0 * sc * f.depth;
                    let tvy = (40.0 + 80.0 * f.depth) * sc;
                    let k = (STEP * 1.5).min(1.0);
                    f.vx += (tvx - f.vx) * k;
                    f.vy += (tvy - f.vy) * k;
                    f.rot += f.spin * STEP;
                }
                FlakeKind::Water => {
                    f.vy += 900.0 * sc * STEP;
                    f.vx *= 0.98;
                }
                FlakeKind::Chunk => {
                    f.vy = (f.vy + 500.0 * sc * STEP).min(260.0 * sc);
                    f.vx *= 0.97;
                    f.rot += f.spin * STEP;
                }
            }
            if let Some((cx, cy)) = cur {
                let (dx, dy) = (f.x - cx, f.y - cy);
                let d2 = dx * dx + dy * dy;
                if d2 < radius * radius && d2 > 1.0 {
                    let d = d2.sqrt();
                    let k = (1.0 - d / radius).powi(2) * f.depth.max(0.5);
                    f.vx += (dx / d * 500.0 * sc + cvx * 0.9) * k * STEP * 4.0;
                    f.vy += (dy / d * 300.0 * sc + cvy * 0.5) * k * STEP * 4.0;
                }
            }
            f.x += f.vx * STEP;
            f.y += f.vy * STEP;

            if f.x < -30.0 {
                f.x += self.w + 60.0;
            } else if f.x > self.w + 30.0 {
                f.x -= self.w + 60.0;
            }
            if f.y < -80.0 * sc {
                f.vy = f.vy.max(10.0);
            }
            if f.y >= 0.0 {
                let cx = (f.x / self.cell) as i32;
                let cy = (f.y / self.cell) as i32;
                if self.grid.get(cx, cy) != EMPTY {
                    land = true;
                }
            }
            if land {
                let cx = (f.x / self.cell) as i32;
                let mut cy = (f.y / self.cell) as i32;
                let mut tries = 0;
                while self.grid.get(cx, cy) != EMPTY && cy > 0 && tries < 8 {
                    cy -= 1;
                    tries += 1;
                }
                let v = match f.kind {
                    FlakeKind::Water => {
                        if fastrand::u8(..) < 150 {
                            ICE
                        } else {
                            EMPTY
                        }
                    }
                    _ => SNOW,
                };
                if v != EMPTY && self.grid.get(cx, cy) == EMPTY && self.pile_height_ok(cx, cy) {
                    self.grid.set(cx, cy, v);
                }
                self.recycle(i);
                continue;
            }
            if f.y > self.h + 20.0 {
                self.recycle(i);
                continue;
            }
            self.flakes[i] = f;
            i += 1;
        }
    }

    /// Snow cells that fell off a ledge drift down as flakes again, so they
    /// obey the pile height limit (they used to pile up without bound under
    /// window edges in a blizzard).
    fn cells_to_flakes(&mut self) {
        let fell = std::mem::take(&mut self.grid.fell);
        for &(x, y) in &fell {
            if self.flakes.len() >= MAX_FLAKES {
                break;
            }
            self.flakes.push(Flake {
                x: (x as f32 + 0.5) * self.cell,
                y: (y as f32 + 1.0) * self.cell,
                vx: 0.0,
                vy: 30.0 * self.scale,
                depth: 1.0,
                phase: fastrand::f32() * std::f32::consts::TAU,
                spin: (fastrand::f32() - 0.5) * 3.0,
                rot: 0.0,
                kind: FlakeKind::Snow,
                temp: true,
            });
        }
        self.grid.fell = fell;
        self.grid.fell.clear();
    }

    /// Respawn a persistent flake at the top, or drop it.
    fn recycle(&mut self, i: usize) {
        let f = self.flakes[i];
        if !f.temp && f.kind == FlakeKind::Snow {
            self.flakes[i] = self.new_flake(false);
        } else {
            self.flakes.swap_remove(i);
        }
    }

    fn natural_melt(&mut self) {
        let r = self.cfg.melt_rate;
        if r <= 0.0 {
            return;
        }
        let n = (r * 6.0).ceil() as usize;
        for _ in 0..n {
            if fastrand::f32() > r.min(1.0) {
                continue;
            }
            let x = fastrand::i32(0..self.grid.w as i32);
            let y = self.grid.surface_y(x);
            let c = self.grid.get(x, y);
            if (c == SNOW || c == PACKED) && y < self.grid.h as i32 {
                self.grid.set(x, y, EMPTY);
                self.grid.wake(x as usize, y as usize);
            }
        }
    }

    // ---- windows -------------------------------------------------------

    fn ledge(&self, r: &Rect) -> Option<(i32, i32, i32)> {
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

    /// Replace the set of window rects (monitor-local px). Snow on windows
    /// that moved or closed falls off as particles.
    pub fn set_windows(&mut self, new: &[Rect]) {
        if !self.cfg.windows_collide {
            if !self.windows.is_empty() {
                self.set_windows_inner(&[]);
            }
            return;
        }
        self.set_windows_inner(new);
    }

    fn set_windows_inner(&mut self, new: &[Rect]) {
        if self.windows == new {
            return;
        }
        let old = std::mem::take(&mut self.windows);
        for r in old.iter().filter(|r| !new.contains(r)) {
            self.release_ledge(r);
        }
        // Stamp every current ledge, not just new ones: releasing an old
        // ledge may have cleared wall cells it shared with a remaining one.
        // (`set` is a no-op for cells that are already walls.)
        for r in new {
            if let Some((x0, x1, y0)) = self.ledge(r) {
                for x in x0..x1 {
                    for y in y0..y0 + LEDGE_ROWS {
                        self.grid.set(x, y, WALL);
                    }
                }
            }
        }
        self.windows = new.to_vec();
    }

    fn release_ledge(&mut self, r: &Rect) {
        let Some((x0, x1, y0)) = self.ledge(r) else {
            return;
        };
        for x in x0..x1 {
            let mut y = y0 - 1;
            while y >= 0 {
                let c = self.grid.get(x, y);
                if !(c == SNOW || c == PACKED || c == ICE) {
                    break;
                }
                self.grid.set(x, y, EMPTY);
                if self.flakes.len() < MAX_FLAKES {
                    self.flakes.push(Flake {
                        x: x as f32 * self.cell + self.cell * 0.5,
                        y: y as f32 * self.cell,
                        vx: (fastrand::f32() - 0.5) * 60.0 * self.scale,
                        vy: 0.0,
                        depth: 0.9,
                        phase: 0.0,
                        spin: (fastrand::f32() - 0.5) * 6.0,
                        rot: 0.0,
                        kind: FlakeKind::Chunk,
                        temp: true,
                    });
                }
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

    fn reapply_walls(&mut self) {
        for c in self.grid.cells.iter_mut() {
            if *c == WALL {
                *c = EMPTY;
            }
        }
        let wins = self.windows.clone();
        for r in &wins {
            if let Some((x0, x1, y0)) = self.ledge(r) {
                for x in x0..x1 {
                    for y in y0..y0 + LEDGE_ROWS {
                        self.grid.set(x, y, WALL);
                    }
                }
            }
        }
        self.grid.mark_all_dirty();
    }

    // ---- snowmen & critter ---------------------------------------------

    fn update_snowmen(&mut self) {
        let found = balls::find_snowmen(&self.balls);
        for b in &mut self.balls {
            b.in_snowman = false;
        }
        for chain in &found {
            let fresh = chain
                .iter()
                .any(|&i| !self.snowmen.iter().any(|s| s.contains(&i)));
            for &i in chain {
                self.balls[i].in_snowman = true;
                if fresh {
                    self.balls[i].wobble = 1.2;
                }
            }
        }
        self.snowmen = found;
    }

    pub(crate) fn surface_px(&self, x: f32) -> f32 {
        self.grid.surface_y((x / self.cell) as i32) as f32 * self.cell
    }

    /// Pixel scale of penguin `c` (DPI scale x configured size x own size).
    pub fn penguin_scale(&self, c: &Critter) -> f32 {
        self.scale * self.cfg.penguin_size.clamp(0.5, 3.0) * c.size
    }

    fn spawn_critter(&self) -> Critter {
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
        }
    }

    fn step_critters(&mut self) {
        if !self.cfg.critter {
            self.critters.clear();
            return;
        }
        let want = self.cfg.penguins.clamp(1, MAX_PENGUINS) as usize;
        while self.critters.len() < want {
            let c = self.spawn_critter();
            self.critters.push(c);
        }
        self.critters.truncate(want);
        let mut list = std::mem::take(&mut self.critters);
        for c in &mut list {
            self.step_one_critter(c);
        }
        self.critters = list;
    }

    fn step_one_critter(&mut self, c: &mut Critter) {
        let (w, cell) = (self.w, self.cell);
        let k = self.penguin_scale(c);
        let sc = self.scale;
        c.t -= STEP;
        c.flee = (c.flee - STEP).max(0.0);
        let cx = (c.x / cell) as i32;
        let reach = ((4.0 * k / cell).round() as i32).max(2);
        let slope = self.grid.surface_y(cx + reach) - self.grid.surface_y(cx - reach);
        match c.state {
            CritterState::Held => return,
            CritterState::Air => {
                c.air_time += STEP;
                c.vy += PENGUIN_GRAVITY * sc * STEP;
                c.x += c.vx * STEP;
                c.y += c.vy * STEP;
                if c.x < 10.0 || c.x > w - 10.0 {
                    c.x = c.x.clamp(10.0, w - 10.0);
                    c.vx = -c.vx * 0.5;
                }
                let ground = self.surface_px(c.x);
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
                return;
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
                c.vx = c.dir * speed * k;
                if c.t <= 0.0 {
                    c.state = CritterState::Idle;
                    c.t = 1.0 + fastrand::f32() * 2.0;
                    c.vx = 0.0;
                } else if c.flee <= 0.0 && slope.abs() >= 2 * reach && fastrand::u8(..) < 4 {
                    c.dir = slope.signum() as f32;
                    c.state = CritterState::Sled;
                    c.t = 4.0;
                }
            }
            CritterState::Idle => {
                c.vx = 0.0;
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
        c.x += c.vx * STEP;
        if c.x < 20.0 || c.x > w - 20.0 {
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
                let fy = self.grid.surface_y(fx);
                if self.grid.get(fx, fy) == SNOW {
                    self.grid.set(fx, fy, EMPTY);
                }
            }
        }
        let target = self.surface_px(c.x);
        if target < c.y - 12.0 * k {
            // Ground fell away under it: drop instead of teleporting up/down.
            c.y += (target - c.y) * 0.25;
        } else if target > c.y + 10.0 * k {
            c.state = CritterState::Air;
            c.air_time = 0.0;
            c.vx = c.vx.clamp(-80.0 * sc, 80.0 * sc);
            c.vy = 0.0;
        } else {
            c.y += (target - c.y) * 0.25;
        }
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

    pub fn grab_penguin(&mut self, i: usize) {
        if let Some(c) = self.critters.get_mut(i) {
            c.state = CritterState::Held;
            c.vx = 0.0;
            c.vy = 0.0;
        }
    }

    /// Move a held penguin so its body hangs from the pointer.
    pub fn drag_penguin(&mut self, i: usize, x: f32, y: f32) {
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

    /// A thrown snowball hit penguin `i`: it falls over.
    pub(crate) fn knock_penguin(&mut self, i: usize, ball_vx: f32) {
        let sc = self.scale;
        if let Some(c) = self.critters.get_mut(i)
            && c.state != CritterState::Held
        {
            c.fall_dir = if ball_vx >= 0.0 { 1.0 } else { -1.0 };
            c.state = CritterState::Air;
            c.vx = ball_vx.clamp(-200.0 * sc, 200.0 * sc) * 0.5;
            c.vy = -200.0 * sc;
            c.flee = 0.0;
            c.tumble = true;
            c.air_time = 0.0;
        }
    }

    /// Thrown balls that hit a penguin this step (ball index, penguin index).
    fn penguin_hits(&self) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for (bi, b) in self.balls.iter().enumerate().filter(|(_, b)| b.thrown) {
            if let Some(pi) = self.penguin_at(b.x, b.y) {
                out.push((bi, pi));
            }
        }
        out
    }

    // ---- snapshots -------------------------------------------------------

    pub fn push_undo(&mut self) {
        self.undo.push(Snapshot::take(&self.grid, &self.balls));
        if self.undo.len() > UNDO_STEPS {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn restore(&mut self, s: Snapshot) {
        self.grid.cells = s.cells(self.grid.cells.len());
        self.balls = s.balls;
        self.rolling = None;
        self.reapply_walls();
    }

    pub fn undo(&mut self) {
        if let Some(s) = self.undo.pop() {
            self.redo.push(Snapshot::take(&self.grid, &self.balls));
            self.restore(s);
        }
    }

    pub fn redo(&mut self) {
        if let Some(s) = self.redo.pop() {
            self.undo.push(Snapshot::take(&self.grid, &self.balls));
            self.restore(s);
        }
    }

    #[cfg(test)]
    pub(crate) fn undo_bytes(&self) -> usize {
        self.undo
            .iter()
            .chain(&self.redo)
            .map(|s| s.runs.len() * 5)
            .sum()
    }

    pub fn clear_all(&mut self) {
        self.push_undo();
        self.grid.clear_snow();
        self.balls.clear();
        self.snowmen.clear();
        self.rolling = None;
        self.reapply_walls();
    }

    // ---- rendering ------------------------------------------------------

    pub fn build_instances(&self, out: &mut Vec<Instance>) {
        out.clear();
        let sc = self.scale;
        let edge = 48.0 * sc;
        let base = 1.3 * sc * self.cfg.flake_size;
        for f in &self.flakes {
            let (fx, fy) = (f.x, f.y);
            let fade = smooth(-10.0, edge, fx)
                * smooth(-10.0, edge, self.w - fx)
                * smooth(-30.0 * sc, edge, fy);
            if fade <= 0.0 {
                continue;
            }
            match f.kind {
                FlakeKind::Snow => {
                    let r = base * (0.8 + 2.4 * f.depth);
                    let a = (0.45 + 0.5 * f.depth) * fade;
                    out.push(Instance {
                        pos: [fx, fy],
                        half: [r, r],
                        rot: f.rot,
                        shape: SHAPE_SOFT,
                        color: premul([0.95, 0.97, 1.0], a),
                    });
                }
                FlakeKind::Chunk => out.push(Instance {
                    pos: [fx, fy],
                    half: [self.cell * 0.5; 2],
                    rot: f.rot,
                    shape: SHAPE_RECT,
                    color: premul([0.93, 0.96, 1.0], 0.95 * fade),
                }),
                FlakeKind::Water => out.push(Instance {
                    pos: [fx, fy],
                    half: [1.5 * sc, 2.5 * sc],
                    rot: 0.0,
                    shape: SHAPE_ELLIPSE,
                    color: premul([0.5, 0.75, 1.0], 0.85),
                }),
            }
        }
        self.push_ball_instances(out);
        self.push_critters(out);
        if self.cfg.string_lights {
            self.push_lights(out);
        }
    }

    fn push_lights(&self, out: &mut Vec<Instance>) {
        let cols = [
            [1.0, 0.25, 0.3],
            [1.0, 0.85, 0.3],
            [0.4, 0.95, 0.5],
            [0.45, 0.65, 1.0],
        ];
        let step = 18.0 * self.scale;
        for r in &self.windows {
            let Some((x0, x1, y0)) = self.ledge(r) else {
                continue;
            };
            let (xa, xb) = (x0 as f32 * self.cell, x1 as f32 * self.cell);
            let y = y0 as f32 * self.cell - 3.0 * self.scale;
            let mut x = xa + step * 0.5;
            let mut i = 0usize;
            while x < xb {
                let tw = 0.65 + 0.35 * (self.time * 3.0 + i as f32 * 1.7).sin();
                let c = cols[i % 4];
                out.push(Instance {
                    pos: [x, y],
                    half: [7.0 * self.scale; 2],
                    rot: 0.0,
                    shape: SHAPE_GLOW,
                    color: premul(c, 0.55 * tw),
                });
                out.push(Instance {
                    pos: [x, y],
                    half: [2.2 * self.scale; 2],
                    rot: 0.0,
                    shape: SHAPE_ELLIPSE,
                    color: premul(c, tw),
                });
                x += step;
                i += 1;
            }
        }
    }

    fn push_critters(&self, out: &mut Vec<Instance>) {
        for (i, c) in self.critters.iter().enumerate() {
            self.push_critter(out, c, i as f32);
        }
    }

    fn push_critter(&self, out: &mut Vec<Instance>, c: &Critter, seed: f32) {
        let k = self.penguin_scale(c);
        let d = if c.dir >= 0.0 { 1.0 } else { -1.0 };
        let t = self.time + seed * 1.37;
        let sled = c.state == CritterState::Sled;
        let rot = match c.state {
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

    fn push_ball_instances(&self, out: &mut Vec<Instance>) {
        const SCARF: [[f32; 3]; 4] = [
            [0.9, 0.2, 0.25],
            [0.3, 0.55, 0.95],
            [0.3, 0.8, 0.4],
            [0.95, 0.75, 0.2],
        ];
        let ink = premul([0.05, 0.05, 0.08], 1.0);
        for b in &self.balls {
            let wob = if b.wobble > 0.0 {
                (self.time * 16.0).sin() * b.wobble.min(1.0) * b.r * 0.06
            } else {
                0.0
            };
            let x = b.x + wob;
            // shadow-ish rim, body, highlight
            out.push(Instance {
                pos: [x, b.y],
                half: [b.r * 1.03; 2],
                rot: 0.0,
                shape: SHAPE_ELLIPSE,
                color: premul([0.62, 0.7, 0.85], 1.0),
            });
            out.push(Instance {
                pos: [x - b.r * 0.04, b.y - b.r * 0.05],
                half: [b.r * 0.96; 2],
                rot: 0.0,
                shape: SHAPE_ELLIPSE,
                color: premul([0.96, 0.98, 1.0], 1.0),
            });
            out.push(Instance {
                pos: [x - b.r * 0.35, b.y - b.r * 0.35],
                half: [b.r * 0.35; 2],
                rot: b.rot,
                shape: SHAPE_SOFT,
                color: premul([1.0, 1.0, 1.0], 0.9),
            });
            for d in &b.decor {
                let (px, py) = (x + d.dx * b.r, b.y + d.dy * b.r);
                let side = if d.dx >= 0.0 { 1.0 } else { -1.0 };
                match d.kind {
                    DecorKind::Eye => out.push(Instance {
                        pos: [px, py],
                        half: [b.r * 0.1; 2],
                        rot: 0.0,
                        shape: SHAPE_ELLIPSE,
                        color: ink,
                    }),
                    DecorKind::Button => out.push(Instance {
                        pos: [px, py],
                        half: [b.r * 0.09; 2],
                        rot: 0.0,
                        shape: SHAPE_ELLIPSE,
                        color: premul([0.15, 0.15, 0.2], 1.0),
                    }),
                    DecorKind::Nose => out.push(Instance {
                        pos: [px + side * b.r * 0.25, py],
                        half: [b.r * 0.28, b.r * 0.09],
                        rot: if side > 0.0 {
                            0.0
                        } else {
                            std::f32::consts::PI
                        },
                        shape: SHAPE_TRI,
                        color: premul([1.0, 0.5, 0.1], 1.0),
                    }),
                    DecorKind::Arm => out.push(Instance {
                        pos: [x + side * b.r * 1.35, b.y - b.r * 0.35],
                        half: [b.r * 0.62, b.r * 0.04],
                        rot: -side * 0.55,
                        shape: SHAPE_RECT,
                        color: premul([0.4, 0.26, 0.14], 1.0),
                    }),
                    DecorKind::Hat => {
                        out.push(Instance {
                            pos: [x, b.y - b.r * 0.92],
                            half: [b.r * 0.75, b.r * 0.07],
                            rot: 0.0,
                            shape: SHAPE_RECT,
                            color: ink,
                        });
                        out.push(Instance {
                            pos: [x, b.y - b.r * 1.35],
                            half: [b.r * 0.48, b.r * 0.4],
                            rot: 0.0,
                            shape: SHAPE_RECT,
                            color: ink,
                        });
                    }
                    DecorKind::Scarf => {
                        let col = premul(SCARF[d.color as usize % 4], 1.0);
                        out.push(Instance {
                            pos: [x, b.y + b.r * 0.62],
                            half: [b.r * 0.78, b.r * 0.14],
                            rot: 0.0,
                            shape: SHAPE_RECT,
                            color: col,
                        });
                        out.push(Instance {
                            pos: [x + b.r * 0.45, b.y + b.r * 0.95],
                            half: [b.r * 0.1, b.r * 0.3],
                            rot: 0.1,
                            shape: SHAPE_RECT,
                            color: col,
                        });
                    }
                }
            }
        }
        let _ = SHAPE_STAR;
    }
}

#[cfg(test)]
mod penguin_tests {
    use super::*;

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
        w.set_windows(&[crate::sim::Rect {
            x: 120.0,
            y: 120.0,
            w: 160.0,
            h: 80.0,
        }]);
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
}
