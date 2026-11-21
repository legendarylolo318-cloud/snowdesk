//! Per-monitor snow world: flakes, settled grid, balls, windows, critter.

use super::balls::{self, Ball, DecorKind};
use super::grid::{BRICK, EMPTY, Grid, ICE, LEAF, PACKED, PETAL, SNOW, WALL, WATER};
pub use super::penguin::{Critter, CritterState, MAX_PENGUINS};
use super::season::Season;
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FlakeKind {
    Snow,
    Water,
    Chunk,
    /// A broken-off piece of icicle.
    Shard,
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
    pub(crate) windows: Vec<super::ledges::Ledge>,
    pub(crate) icicles: Vec<super::ledges::Icicle>,
    /// Last time (sim seconds) and rect each window was seen to change.
    pub(crate) win_seen: Vec<(u64, f32, Rect)>,
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
    pub sleds: Vec<super::sled::Sled>,
    /// Play mode: the user is building, so falling cells stay cells.
    pub play: bool,
    /// Screen sides (left, right) that lead to another monitor.
    pub open_sides: (bool, bool),
    /// The engine balances penguins across monitors (they walk across).
    pub shared_penguins: bool,
    /// Penguins and thrown balls that left through an open side
    /// (true = right side), collected by the engine.
    pub leaving: Vec<(Critter, bool)>,
    pub leaving_balls: Vec<(Ball, bool)>,
    /// Where angry penguins aim (the pointer in play mode).
    pub target: Option<(f32, f32)>,
    pub frost: super::frost::Frost,
    /// The season in effect (resolved from the config by the engine).
    pub season: Season,
    /// Current weather mood's effect (set by the engine).
    pub weather: super::weather::WeatherFx,
    /// Water cells (rain), recounted every second to cap puddles.
    water_cells: usize,
    /// Frost may grow (a pointer position is available to wipe it).
    pub frost_ok: bool,
    frost_ptr: Option<(f32, f32)>,
    pub(crate) undo: Vec<Snapshot>,
    pub(crate) redo: Vec<Snapshot>,
    pub(crate) rolling: Option<usize>,
    /// The ball in `rolling` is being carried (picked up), not rolled:
    /// it follows the pointer at this offset and does not scoop snow.
    pub(crate) carry: Option<(f32, f32)>,
    /// Where the last brick of the current drag went (brick tool).
    pub(crate) last_brick: Option<(f32, f32)>,
    /// Last cursor footprint (play mode) and which foot made it.
    pub(crate) last_print: Option<(f32, f32)>,
    pub(crate) print_left: bool,
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
            icicles: Vec::new(),
            win_seen: Vec::new(),
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
            sleds: Vec::new(),
            play: false,
            open_sides: (false, false),
            shared_penguins: false,
            leaving: Vec::new(),
            leaving_balls: Vec::new(),
            target: None,
            frost: super::frost::Frost::new(w as f32, h as f32, scale),
            season: cfg.season.resolve(1),
            weather: super::weather::WeatherFx::default(),
            water_cells: 0,
            frost_ok: true,
            frost_ptr: None,
            undo: Vec::new(),
            redo: Vec::new(),
            rolling: None,
            carry: None,
            last_brick: None,
            last_print: None,
            print_left: false,
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
        let i = (self.cfg.intensity * self.weather.intensity_mul).clamp(0.0, 1.0);
        let n = (50.0 + 2900.0 * i * i) * self.area_factor() * self.season.density();
        (n as usize).min(MAX_FLAKES - 2000)
    }

    /// Terminal fall speed of a season particle at this depth.
    fn fall_speed(&self, depth: f32) -> f32 {
        let s = self.scale;
        match self.season {
            Season::Autumn => (22.0 + 40.0 * depth) * s,
            Season::Spring => (28.0 + 50.0 * depth) * s,
            Season::Rain => (520.0 + 420.0 * depth) * s,
            _ => (40.0 + 80.0 * depth) * s,
        }
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
            vy: self.fall_speed(depth),
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
        self.evaporate();

        let mut hits = balls::step_balls_open(
            &mut self.balls,
            &self.grid,
            self.cell,
            STEP,
            self.open_sides,
        );
        self.balls_leaving();
        if !self.critters.is_empty() {
            for (bi, pi) in self.penguin_hits() {
                let (vx, user) = (self.balls[bi].vx, !self.balls[bi].by_penguin);
                self.knock_penguin(pi, vx, user);
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
        self.step_sleds();
        self.step_critters();
        if self.ticks.is_multiple_of(60) {
            self.step_icicles(1.0);
        }
        if self.ticks.is_multiple_of(15) {
            let on = self.cfg.frost && self.frost_ok && self.season == Season::Winter;
            self.frost.step(0.25, on);
        }
        self.cursor_prev = self.cursor;
    }

    /// The pointer is at (x, y): if it moved, wipe frost there. Returns
    /// true if it moved (the user is active).
    pub fn frost_pointer(&mut self, x: f32, y: f32) -> bool {
        let moved = self
            .frost_ptr
            .is_none_or(|(px, py)| (px - x).abs() + (py - y).abs() > 1.0);
        if moved {
            self.frost_ptr = Some((x, y));
            if self.cfg.frost {
                self.frost.wipe(x, y, 22.0 * self.scale);
            }
        }
        moved
    }

    /// Thrown balls that flew out through an open side go to the engine.
    fn balls_leaving(&mut self) {
        if self.open_sides == (false, false) {
            return;
        }
        let mut i = 0;
        while i < self.balls.len() {
            let b = &self.balls[i];
            let side = if b.thrown && self.open_sides.0 && b.x < -b.r {
                Some(false)
            } else if b.thrown && self.open_sides.1 && b.x > self.w + b.r {
                Some(true)
            } else {
                None
            };
            match side {
                Some(right) => {
                    let b = self.balls.remove(i);
                    if let Some(r) = self.rolling.as_mut()
                        && *r > i
                    {
                        *r -= 1;
                    }
                    self.leaving_balls.push((b, right));
                }
                None => i += 1,
            }
        }
    }

    fn pile_height_ok(&self, cx: i32, cy: i32) -> bool {
        let limit = ((self.cfg.max_pile * self.grid.h as f32) as i32).max(2);
        let mut n = 0;
        let mut y = cy + 1;
        while n < limit {
            match self.grid.get(cx, y) {
                SNOW | PACKED | ICE | BRICK | LEAF | PETAL => n += 1,
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
        let wind = (self.cfg.wind + self.weather.wind_add).clamp(-1.2, 1.2) * 0.8
            + if self.cfg.gusts || self.kick > 0.05 {
                self.gust * self.weather.gust_mul
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
                    // Leaves flutter widely, rain hardly sways.
                    let sway = match self.season {
                        Season::Autumn => 46.0,
                        Season::Spring => 26.0,
                        Season::Rain => 0.0,
                        _ => 14.0,
                    };
                    let tvx =
                        wind * 90.0 * sc * (0.4 + f.depth) + f.phase.sin() * sway * sc * f.depth;
                    let tvy = self.fall_speed(f.depth);
                    let k = if self.season == Season::Rain {
                        1.0
                    } else {
                        (STEP * 1.5).min(1.0)
                    };
                    f.vx += (tvx - f.vx) * k;
                    f.vy += (tvy - f.vy) * k;
                    f.rot += f.spin * STEP;
                }
                FlakeKind::Water => {
                    f.vy += 900.0 * sc * STEP;
                    f.vx *= 0.98;
                }
                FlakeKind::Chunk | FlakeKind::Shard => {
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
                    FlakeKind::Shard => {
                        if fastrand::u8(..) < 90 {
                            ICE
                        } else {
                            EMPTY
                        }
                    }
                    _ => self.season.material(),
                };
                let ok = if v == WATER {
                    // Cap puddles at ~3% of the screen; the rest splashes.
                    self.water_cells < self.grid.cells.len() / 32
                } else {
                    self.pile_height_ok(cx, cy)
                };
                if v != EMPTY && self.grid.get(cx, cy) == EMPTY && ok {
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

    /// Puddles dry up: a few water surface cells evaporate every step, and
    /// the water count used for the puddle cap is refreshed once a second.
    fn evaporate(&mut self) {
        if self.ticks.is_multiple_of(60) {
            self.water_cells = self.grid.cells.iter().filter(|&&c| c == WATER).count();
        }
        if self.water_cells == 0 {
            return;
        }
        let tries = if self.season == Season::Rain { 2 } else { 12 };
        for _ in 0..tries {
            let x = fastrand::i32(0..self.grid.w as i32);
            let y = self.grid.surface_y(x);
            if self.grid.get(x, y) == WATER {
                self.grid.set(x, y, EMPTY);
                self.water_cells = self.water_cells.saturating_sub(1);
            }
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
            if matches!(c, SNOW | PACKED | LEAF | PETAL) && y < self.grid.h as i32 {
                self.grid.set(x, y, EMPTY);
                self.grid.wake(x as usize, y as usize);
            }
        }
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
            // Hit anywhere: the whole snowman frowns.
            let frown = chain
                .iter()
                .map(|&i| self.balls[i].frown)
                .fold(0.0, f32::max);
            for &i in chain {
                self.balls[i].frown = self.balls[i].frown.max(frown);
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
        self.sleds.clear();
        for c in &mut self.critters {
            c.ride = None;
        }
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
                FlakeKind::Snow => self.push_particle(out, f, fade, base),
                FlakeKind::Shard => out.push(Instance {
                    pos: [fx, fy],
                    half: [4.0 * self.scale, 1.6 * self.scale],
                    rot: f.rot,
                    shape: SHAPE_TRI,
                    color: premul([0.78, 0.92, 1.0], 0.9 * fade),
                }),
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
        self.push_icicles(out);
        self.push_sleds(out);
        self.push_critters(out);
        if self.cfg.string_lights {
            self.push_lights(out);
        }
    }

    /// A falling particle of the current season.
    fn push_particle(&self, out: &mut Vec<Instance>, f: &Flake, fade: f32, base: f32) {
        let sc = self.scale;
        let pos = [f.x, f.y];
        // Stable per-particle variety from its phase seed.
        let pick = ((f.spin * 977.0).abs() as usize) % 4;
        match self.season {
            Season::Autumn => {
                const LEAVES: [[f32; 3]; 4] = [
                    [0.93, 0.45, 0.12],
                    [0.80, 0.22, 0.12],
                    [0.95, 0.72, 0.18],
                    [0.62, 0.36, 0.16],
                ];
                let s = (3.0 + 3.0 * f.depth) * sc * self.cfg.flake_size;
                // Tumbling: the leaf's apparent width flips as it turns.
                let flip = (f.phase * 2.3).sin().abs().max(0.25);
                out.push(Instance {
                    pos,
                    half: [s, s * 0.45 * flip + 0.6],
                    rot: f.rot,
                    shape: SHAPE_ELLIPSE,
                    color: premul(LEAVES[pick], (0.75 + 0.25 * f.depth) * fade),
                });
            }
            Season::Spring => {
                const PETALS: [[f32; 3]; 4] = [
                    [1.0, 0.75, 0.84],
                    [0.98, 0.66, 0.78],
                    [1.0, 0.86, 0.91],
                    [0.95, 0.58, 0.72],
                ];
                let s = (1.6 + 1.8 * f.depth) * sc * self.cfg.flake_size;
                let flip = (f.phase * 1.7).sin().abs().max(0.35);
                out.push(Instance {
                    pos,
                    half: [s, s * 0.7 * flip],
                    rot: f.rot,
                    shape: SHAPE_ELLIPSE,
                    color: premul(PETALS[pick], (0.6 + 0.35 * f.depth) * fade),
                });
            }
            Season::Rain => {
                // A streak along the direction of travel.
                let len = (3.0 + 6.0 * f.depth) * sc;
                out.push(Instance {
                    pos,
                    half: [0.55 * sc * (0.6 + f.depth), len],
                    rot: -f.vx.atan2(f.vy),
                    shape: SHAPE_RECT,
                    color: premul([0.62, 0.74, 0.92], (0.25 + 0.35 * f.depth) * fade),
                });
            }
            _ => {
                let r = base * (0.8 + 2.4 * f.depth);
                out.push(Instance {
                    pos,
                    half: [r, r],
                    rot: f.rot,
                    shape: SHAPE_SOFT,
                    color: premul([0.95, 0.97, 1.0], (0.45 + 0.5 * f.depth) * fade),
                });
            }
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
        for l in &self.windows {
            let Some((x0, x1, y0)) = self.ledge(&l.rect) else {
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

    /// Coal eyes that follow the pointer and blink, and a mouth that
    /// smiles, or frowns after a snowball hit.
    fn push_face(&self, out: &mut Vec<Instance>, b: &Ball, x: f32) {
        let eyes: Vec<(f32, f32)> = b
            .decor
            .iter()
            .filter(|d| d.kind == DecorKind::Eye)
            .map(|d| (d.dx, d.dy))
            .collect();
        if eyes.is_empty() {
            return;
        }
        let ink = premul([0.05, 0.05, 0.08], 1.0);
        let r = b.r;
        // Look toward the pointer (play mode) or the desktop cursor.
        let (lx, ly) = match self.target.or(self.cursor) {
            Some((px, py)) => {
                let (dx, dy) = (px - x, py - b.y);
                let d = dx.hypot(dy).max(1.0);
                (dx / d * r * 0.035, dy / d * r * 0.035)
            }
            None => (0.0, 0.0),
        };
        // Blink for 0.13 s every 3.5-6 s.
        let period = 3.5 + b.seed * 2.5;
        let blink = (self.time + b.seed * 7.0) % period < 0.13;
        let open = if blink { 0.15 } else { 1.0 };
        for &(dx, dy) in &eyes {
            out.push(Instance {
                pos: [x + dx * r + lx, b.y + dy * r + ly],
                half: [r * 0.1, r * 0.1 * open],
                rot: 0.0,
                shape: SHAPE_ELLIPSE,
                color: ink,
            });
        }
        // Mouth: five coal dots under the eyes.
        let n = eyes.len() as f32;
        let (cx, cy) = (
            eyes.iter().map(|e| e.0).sum::<f32>() / n,
            eyes.iter().map(|e| e.1).sum::<f32>() / n + 0.45,
        );
        let curve = if b.frown > 0.0 { -0.12 } else { 0.12 };
        for k in 0..5 {
            let t = (k as f32 - 2.0) / 2.0; // -1..1
            out.push(Instance {
                pos: [
                    x + (cx + t * 0.28) * r,
                    b.y + (cy + curve * (1.0 - t * t)) * r,
                ],
                half: [r * 0.045; 2],
                rot: 0.0,
                shape: SHAPE_ELLIPSE,
                color: ink,
            });
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
            self.push_face(out, b, x);
            for d in &b.decor {
                let (px, py) = (x + d.dx * b.r, b.y + d.dy * b.r);
                let side = if d.dx >= 0.0 { 1.0 } else { -1.0 };
                match d.kind {
                    DecorKind::Eye => {} // drawn by push_face
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
mod face_tests {
    use super::*;
    use crate::sim::balls::{Ball, Decor, DecorKind};

    fn face_world() -> World {
        let mut w = World::new(400, 400, 1.0, &Config::default());
        let mut b = Ball::new(200.0, 200.0, 40.0);
        for dx in [-0.3, 0.3] {
            b.decor.push(Decor {
                kind: DecorKind::Eye,
                color: 0,
                dx,
                dy: -0.2,
            });
        }
        b.seed = 0.5;
        w.balls.push(b);
        w
    }

    fn eye_and_mouth(w: &World) -> (Vec<Instance>, Vec<Instance>) {
        let mut out = Vec::new();
        w.push_face(&mut out, &w.balls[0], w.balls[0].x);
        let (eyes, mouth): (Vec<_>, Vec<_>) = out.into_iter().partition(|i| i.half[0] > 3.0);
        (eyes, mouth)
    }

    #[test]
    fn eyes_follow_the_pointer() {
        let mut w = face_world();
        w.target = Some((400.0, 200.0)); // to the right
        let (eyes, _) = eye_and_mouth(&w);
        assert!(eyes[0].pos[0] > 200.0 - 0.3 * 40.0, "looks right");
        w.target = Some((0.0, 200.0));
        let (eyes2, _) = eye_and_mouth(&w);
        assert!(eyes2[0].pos[0] < eyes[0].pos[0]);
    }

    #[test]
    fn smiles_then_frowns_when_hit() {
        let mut w = face_world();
        let (_, smile) = eye_and_mouth(&w);
        // Smile: middle dot lower (larger y) than the ends.
        assert!(smile[2].pos[1] > smile[0].pos[1]);
        w.balls[0].frown = 2.0;
        let (_, frown) = eye_and_mouth(&w);
        assert!(frown[2].pos[1] < frown[0].pos[1]);
    }

    #[test]
    fn blinks_now_and_then() {
        let mut w = face_world();
        let mut shut = 0;
        for i in 0..600 {
            w.time = i as f32 * 0.02;
            let (eyes, _) = eye_and_mouth(&w);
            if eyes[0].half[1] < eyes[0].half[0] * 0.5 {
                shut += 1;
            }
        }
        assert!(shut > 0 && shut < 60, "blinked briefly: {shut} of 600");
    }
}
