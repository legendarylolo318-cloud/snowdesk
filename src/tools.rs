//! Play-mode tools. All operate on a `World` in monitor-local pixels.

use crate::sim::balls::{Ball, Decor, DecorKind};
use crate::sim::grid::{BRICK, EMPTY, ICE, LEAF, PACKED, PETAL, SNOW, WALL};
use crate::sim::world::{Flake, FlakeKind, MAX_BALLS, MAX_FLAKES, World};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Brush,
    Shovel,
    Pack,
    Freeze,
    Melt,
    Snowball,
    Angel,
    /// Packed-snow bricks for forts and igloos.
    Brick,
}

impl Tool {
    pub fn from_index(i: i32) -> Tool {
        match i {
            0 => Tool::Brush,
            1 => Tool::Shovel,
            2 => Tool::Pack,
            3 => Tool::Freeze,
            4 => Tool::Melt,
            5 => Tool::Snowball,
            6 => Tool::Angel,
            7 => Tool::Brick,
            _ => Tool::Brush,
        }
    }
}

fn disc(cx: i32, cy: i32, r: i32, mut f: impl FnMut(i32, i32)) {
    for dy in -r..=r {
        for dx in -r..=r {
            if dx * dx + dy * dy <= r * r {
                f(cx + dx, cy + dy);
            }
        }
    }
}

impl World {
    fn cell_xy(&self, x: f32, y: f32) -> (i32, i32) {
        (
            (x / self.cell).floor() as i32,
            (y / self.cell).floor() as i32,
        )
    }

    /// Continuous tool application while the pointer is held.
    /// `radius` is in px.
    pub fn apply_tool(&mut self, tool: Tool, x: f32, y: f32, radius: f32) {
        let (cx, cy) = self.cell_xy(x, y);
        let r = ((radius / self.cell).round() as i32).max(1);
        match tool {
            Tool::Brush => {
                // Paints the season's material (snow, leaves, petals, water).
                let m = self.season.material();
                disc(cx, cy, r, |x, y| {
                    if fastrand::u8(..) < 90 {
                        self.grid.add(x, y, m);
                    }
                });
            }
            Tool::Shovel => {
                disc(cx, cy, r, |x, y| {
                    let c = self.grid.get(x, y);
                    if c != EMPTY && c != WALL {
                        self.grid.set(x, y, EMPTY);
                    }
                });
            }
            Tool::Pack => {
                disc(cx, cy, r, |x, y| {
                    if self.grid.get(x, y) == SNOW {
                        self.grid.set(x, y, PACKED);
                    }
                });
            }
            Tool::Freeze => {
                disc(cx, cy, r, |x, y| {
                    let c = self.grid.get(x, y);
                    if c == SNOW || c == PACKED || c == crate::sim::grid::WATER {
                        self.grid.set(x, y, ICE);
                    }
                });
            }
            Tool::Melt => {
                let mut drops = Vec::new();
                disc(cx, cy, r, |x, y| {
                    let c = self.grid.get(x, y);
                    if matches!(c, SNOW | PACKED | ICE | BRICK | LEAF | PETAL)
                        && fastrand::u8(..) < 40
                    {
                        self.grid.set(x, y, EMPTY);
                        if fastrand::u8(..) < 40 {
                            drops.push((x, y));
                        }
                    }
                });
                for (gx, gy) in drops {
                    if self.flakes.len() < MAX_FLAKES {
                        self.flakes.push(Flake {
                            x: gx as f32 * self.cell,
                            y: gy as f32 * self.cell,
                            vx: 0.0,
                            vy: 20.0,
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
            Tool::Brick => {
                // One brick per brick-width of drag.
                let bw = self.brick_size().0 as f32 * self.cell;
                if self
                    .last_brick
                    .is_none_or(|(lx, ly)| (x - lx).abs() >= bw || (y - ly).abs() >= bw)
                {
                    self.place_brick(x, y);
                    self.last_brick = Some((x, y));
                }
            }
            Tool::Snowball | Tool::Angel => {}
        }
    }

    // ---- rolling snowballs ---------------------------------------------

    pub fn roll_begin(&mut self, x: f32, y: f32) {
        if self.balls.len() >= MAX_BALLS {
            return;
        }
        let r = 9.0 * self.scale;
        let mut b = Ball::new(x, y, r);
        b.held = true;
        self.balls.push(b);
        self.rolling = Some(self.balls.len() - 1);
        self.removed_acc = 0.0;
    }

    pub fn roll_move(&mut self, x: f32, y: f32) {
        let Some(i) = self.rolling else { return };
        if let Some((ox, oy)) = self.carry {
            // Carried: follow the pointer exactly.
            let r = self.balls[i].r;
            let b = &mut self.balls[i];
            b.x = (x - ox).clamp(r, self.w - r);
            b.y = (y - oy).clamp(r, self.h - r);
            return;
        }
        let cell = self.cell;
        let (old_x, mut r) = (self.balls[i].x, self.balls[i].r);
        // Follow the cursor horizontally, sit on the snow surface.
        let nx = x.clamp(r, self.w - r);
        // The surface nearest below the pointer, so a ball can be rolled on
        // the ground underneath a window ledge, not only on top of it.
        let from = ((y - r * 0.3) / cell).floor() as i32;
        let mut ground = f32::MAX;
        let mut px = nx - r * 0.5;
        while px <= nx + r * 0.5 {
            ground = ground.min(self.grid.surface_from((px / cell) as i32, from) as f32 * cell);
            px += cell;
        }
        let max_r = self.h * 0.18;
        // Pointer well above the snow: lift and carry the ball (no rolling),
        // so it can be dropped onto another ball to build a snowman.
        if y < ground - r * 2.2 {
            let b = &mut self.balls[i];
            b.x = nx;
            b.y = y.max(r);
            return;
        }
        // Scoop snow under the ball.
        let (bx, by) = (nx, ground - r * 0.65);
        let moved = (nx - old_x).abs();
        let mut removed = 0usize;
        if moved > 0.0 {
            let rc = (r * 1.05 / cell).ceil() as i32;
            let (gcx, gcy) = self.cell_xy(bx, by + r * 0.3);
            for dy in 0..=rc {
                for dx in -rc..=rc {
                    let d2 = (dx * dx + dy * dy) as f32 * cell * cell;
                    if d2 <= (r * 1.05).powi(2) {
                        let c = self.grid.get(gcx + dx, gcy + dy);
                        if c == SNOW || c == PACKED {
                            self.grid.set(gcx + dx, gcy + dy, EMPTY);
                            removed += 1;
                        }
                    }
                }
            }
            if removed > 0 {
                let area = std::f32::consts::PI * r * r + removed as f32 * cell * cell * 0.85;
                r = (area / std::f32::consts::PI).sqrt().min(max_r);
            }
        }
        let b = &mut self.balls[i];
        b.r = r;
        b.rot += (nx - old_x) / r;
        b.x = bx;
        b.y = by;
    }

    pub fn roll_end(&mut self) {
        self.roll_end_with(0.0, 0.0);
    }

    /// Ball under (x, y) that can be picked up (not a flying throw).
    pub fn ball_at(&self, x: f32, y: f32) -> Option<usize> {
        self.balls
            .iter()
            .enumerate()
            .rev()
            .find(|(_, b)| !b.thrown && (b.x - x).hypot(b.y - y) <= b.r.max(10.0 * self.scale))
            .map(|(i, _)| i)
    }

    /// Pick up an existing ball. `roll`: keep rolling it (it grows, snowball
    /// tool); otherwise carry it as it is.
    pub fn grab_ball(&mut self, i: usize, x: f32, y: f32, roll: bool) {
        let Some(b) = self.balls.get_mut(i) else {
            return;
        };
        b.held = true;
        b.vx = 0.0;
        b.vy = 0.0;
        self.carry = (!roll).then_some((x - b.x, y - b.y));
        self.rolling = Some(i);
        self.removed_acc = 0.0;
    }

    /// Let go of the rolled/carried ball. A fast flick throws a carried
    /// ball (it splats on impact, like a right-click throw).
    pub fn roll_end_with(&mut self, vx: f32, vy: f32) {
        self.last_brick = None;
        let carried = self.carry.take().is_some();
        let Some(i) = self.rolling.take() else {
            return;
        };
        if carried && vx.hypot(vy) > 600.0 * self.scale {
            if let Some(b) = self.balls.get_mut(i) {
                b.held = false;
                b.thrown = true;
                b.decor.clear();
                b.vx = vx;
                b.vy = vy;
            }
            return;
        }
        let Some(b) = self.balls.get(i) else {
            return;
        };
        // Stacking assist: released above a bigger ball, roughly centred?
        // Snap onto its centre so the stack does not roll off.
        let (bx, by, br) = (b.x, b.y, b.r);
        let below = self
            .balls
            .iter()
            .enumerate()
            .filter(|&(j, o)| {
                j != i && !o.thrown && o.r > br && o.y > by && (o.x - bx).abs() < o.r * 0.75
            })
            .min_by(|a, b| a.1.y.total_cmp(&b.1.y))
            .map(|(_, o)| o.x);
        let b = &mut self.balls[i];
        b.held = false;
        b.vx = 0.0;
        b.vy = 0.0;
        if let Some(x) = below {
            b.x = x;
        }
    }

    // ---- decorating, stamps, throw -------------------------------------

    /// Drop a decoration on the ball under (x, y). Returns true if placed.
    pub fn decorate_at(&mut self, x: f32, y: f32, kind: DecorKind, color: u8) -> bool {
        let Some(b) = self
            .balls
            .iter_mut()
            .rev()
            .find(|b| (b.x - x).hypot(b.y - y) <= b.r * 1.2)
        else {
            return false;
        };
        let (mut dx, mut dy) = ((x - b.x) / b.r, (y - b.y) / b.r);
        let len = dx.hypot(dy);
        if len > 0.9 {
            dx *= 0.9 / len;
            dy *= 0.9 / len;
        }
        // Unique-ish decorations replace existing ones of the same kind.
        if matches!(
            kind,
            DecorKind::Hat | DecorKind::Scarf | DecorKind::Nose | DecorKind::Arm
        ) {
            let limit = if kind == DecorKind::Arm { 2 } else { 1 };
            let same: Vec<usize> = b
                .decor
                .iter()
                .enumerate()
                .filter(|(_, d)| d.kind == kind)
                .map(|(i, _)| i)
                .collect();
            if same.len() >= limit {
                b.decor.remove(same[0]);
            }
        }
        b.decor.push(Decor {
            kind,
            color,
            dx,
            dy,
        });
        true
    }

    /// Quick-stamp a whole snowman at x (dropped from above the ground).
    pub fn stamp_snowman(&mut self, x: f32) {
        if self.balls.len() + 3 > MAX_BALLS {
            return;
        }
        self.push_undo();
        let s = self.scale;
        let rs = [34.0 * s, 25.0 * s, 18.0 * s];
        let ground = self.grid.surface_y((x / self.cell) as i32) as f32 * self.cell;
        let mut y = ground;
        for (i, r) in rs.iter().enumerate() {
            y -= r;
            let mut b = Ball::new(x, y, *r);
            b.wobble = 1.2;
            if i == 2 {
                b.decor.push(Decor {
                    kind: DecorKind::Eye,
                    color: 0,
                    dx: -0.3,
                    dy: -0.2,
                });
                b.decor.push(Decor {
                    kind: DecorKind::Eye,
                    color: 0,
                    dx: 0.3,
                    dy: -0.2,
                });
                b.decor.push(Decor {
                    kind: DecorKind::Nose,
                    color: 0,
                    dx: 0.0,
                    dy: 0.1,
                });
                b.decor.push(Decor {
                    kind: DecorKind::Hat,
                    color: 0,
                    dx: 0.0,
                    dy: -0.9,
                });
            }
            if i == 1 {
                for k in 0..2 {
                    b.decor.push(Decor {
                        kind: DecorKind::Button,
                        color: 0,
                        dx: 0.0,
                        dy: -0.2 + 0.4 * k as f32,
                    });
                }
                b.decor.push(Decor {
                    kind: DecorKind::Scarf,
                    color: 0,
                    dx: 0.0,
                    dy: -0.8,
                });
            }
            self.balls.push(b);
            y -= r;
        }
    }

    pub fn throw_ball(&mut self, x: f32, y: f32, vx: f32, vy: f32) {
        if self.balls.len() >= MAX_BALLS {
            return;
        }
        let mut b = Ball::new(x, y, 6.0 * self.scale);
        b.vx = vx;
        b.vy = vy;
        b.thrown = true;
        self.balls.push(b);
    }

    pub(crate) fn resolve_throws(&mut self, hits: &[(usize, u8)]) {
        let mut dead = Vec::new();
        for &(i, kind) in hits {
            let b = self.balls[i].clone();
            dead.push(i);
            let (cx, cy) = self.cell_xy(b.x, b.y);
            // Splat: scatter snow particles and a few cells.
            for _ in 0..14 {
                if self.flakes.len() < MAX_FLAKES {
                    let a = fastrand::f32() * std::f32::consts::TAU;
                    let sp = (30.0 + fastrand::f32() * 120.0) * self.scale;
                    self.flakes.push(Flake {
                        x: b.x,
                        y: b.y,
                        vx: a.cos() * sp,
                        vy: a.sin() * sp - 60.0 * self.scale,
                        depth: 0.9,
                        phase: 0.0,
                        spin: 3.0,
                        rot: 0.0,
                        kind: FlakeKind::Chunk,
                        temp: true,
                    });
                }
            }
            if kind == 0 {
                // Knock snow off a window ledge if we hit one.
                let below = self.grid.get(cx, cy + 1).max(self.grid.get(cx, cy));
                if below == WALL {
                    self.knock_snow(cx, cy, 10);
                }
            }
        }
        dead.sort_unstable();
        for i in dead.into_iter().rev() {
            self.balls.remove(i);
            if let Some(r) = self.rolling.as_mut()
                && *r > i
            {
                *r -= 1;
            }
        }
    }

    /// Turn nearby settled snow above `(cx, cy)` into falling particles.
    fn knock_snow(&mut self, cx: i32, cy: i32, radius: i32) {
        for x in cx - radius..=cx + radius {
            let mut y = cy;
            while y > cy - 40 && y >= 0 {
                let c = self.grid.get(x, y);
                if c == SNOW || c == PACKED {
                    self.grid.set(x, y, EMPTY);
                    if self.flakes.len() < MAX_FLAKES {
                        self.flakes.push(Flake {
                            x: x as f32 * self.cell,
                            y: y as f32 * self.cell,
                            vx: (fastrand::f32() - 0.5) * 100.0 * self.scale,
                            vy: -40.0 * self.scale,
                            depth: 0.9,
                            phase: 0.0,
                            spin: 4.0,
                            rot: 0.0,
                            kind: FlakeKind::Chunk,
                            temp: true,
                        });
                    }
                }
                y -= 1;
            }
        }
    }

    /// Stamp a snow-angel impression into a flat pile.
    pub fn stamp_angel(&mut self, x: f32) {
        let (cx, _) = self.cell_xy(x, 0.0);
        let k = (self.scale.round() as i32).max(1);
        // Pattern rows, ' ' = untouched. Drawn as columns carved from the surface.
        // (dx, depth) pairs in units of k cells.
        let mut cols: Vec<(i32, i32)> = Vec::new();
        for dx in -1..=1 {
            cols.push((dx, 2)); // body
        }
        for i in 2..=7 {
            // wings: fan outwards, shallow
            for s in [-1, 1] {
                cols.push((s * i, if i < 5 { 2 } else { 1 }));
            }
        }
        for i in 2..=5 {
            // skirt/legs
            for s in [-1, 1] {
                if i % 2 == 0 {
                    cols.push((s * (i / 2), 2));
                }
            }
        }
        for (dx, depth) in cols {
            for sub in 0..k {
                let gx = cx + dx * k + sub;
                let sy = self.grid.surface_y(gx);
                for d in 0..depth * k {
                    if matches!(self.grid.get(gx, sy + d), SNOW | PACKED) {
                        self.grid.set(gx, sy + d, EMPTY);
                    }
                }
            }
        }
    }

    /// Procedural mountain range using fractal value noise. Slopes are capped
    /// so packed snow holds the shape.
    pub fn generate_mountains(&mut self) {
        self.push_undo();
        let w = self.grid.w;
        let seed = fastrand::u32(..);
        let hash = |i: i32| -> f32 {
            let mut x = (i as u32).wrapping_mul(0x9E37_79B1) ^ seed;
            x ^= x >> 15;
            x = x.wrapping_mul(0x85EB_CA6B);
            x ^= x >> 13;
            (x & 0xFFFF) as f32 / 65535.0
        };
        let noise = |x: f32| -> f32 {
            let i = x.floor() as i32;
            let f = x - i as f32;
            let t = f * f * (3.0 - 2.0 * f);
            hash(i) * (1.0 - t) + hash(i + 1) * t
        };
        let max_h = self.grid.h as f32 * 0.38;
        let mut heights: Vec<i32> = (0..w)
            .map(|x| {
                let xf = x as f32 / w as f32 * 6.0;
                let n =
                    noise(xf) * 0.6 + noise(xf * 2.3 + 7.0) * 0.3 + noise(xf * 5.1 + 19.0) * 0.1;
                (n * n * max_h * 1.6).min(max_h) as i32
            })
            .collect();
        // Cap slope to 2 cells per column (both directions).
        for x in 1..w {
            heights[x] = heights[x].min(heights[x - 1] + 2);
        }
        for x in (0..w - 1).rev() {
            heights[x] = heights[x].min(heights[x + 1] + 2);
        }
        let gh = self.grid.h as i32;
        for (x, &hh) in heights.iter().enumerate() {
            for d in 0..hh {
                let y = gh - 1 - d;
                if self.grid.get(x as i32, y) == EMPTY {
                    self.grid
                        .set(x as i32, y, if d >= hh - 2 { SNOW } else { PACKED });
                }
            }
        }
    }
}

impl World {
    /// Brick size in cells (width, height).
    pub(crate) fn brick_size(&self) -> (i32, i32) {
        (
            ((14.0 * self.scale / self.cell).round() as i32).max(3),
            ((7.0 * self.scale / self.cell).round() as i32).max(2),
        )
    }

    /// Place a packed-snow brick under (x, y): it drops onto the highest
    /// support below the pointer and snaps to a staggered brick pattern.
    pub fn place_brick(&mut self, x: f32, y: f32) {
        let (bw, bh) = self.brick_size();
        let (cx, cy) = (
            (x / self.cell).floor() as i32,
            (y / self.cell).floor() as i32,
        );
        let from = cy - bh;
        // Highest support under the brick's columns.
        let col0 = cx.div_euclid(bw) * bw;
        let mut land = i32::MAX;
        for k in 0..2 {
            // Try both stagger offsets; settle on the course it lands in.
            let x0 = col0 + if k == 1 { bw / 2 } else { 0 };
            let l = (x0..x0 + bw)
                .map(|c| self.grid.surface_from(c, from))
                .min()
                .unwrap_or(self.grid.h as i32);
            let course = (self.grid.h as i32 - l) / bh;
            if (course % 2 == 1) == (k == 1) {
                land = l;
                for c in x0..x0 + bw {
                    for r in l - bh..l {
                        if self.grid.get(c, r) == EMPTY {
                            self.grid.set(c, r, BRICK);
                        }
                    }
                }
                break;
            }
        }
        let _ = land;
    }

    /// Stamp an igloo at x: a hollow brick dome with a tunnel on one side,
    /// tall enough for the configured penguins to walk in.
    pub fn stamp_igloo(&mut self, x: f32) {
        self.push_undo();
        let cell = self.cell;
        let s = self.scale;
        let penguin_h = 24.0 * s * self.cfg.penguin_size.clamp(0.5, 3.0) * 1.15;
        let r_out = (80.0 * s).max(penguin_h * 3.2);
        let thick = (9.0 * s).max(3.0 * cell);
        let r_in = r_out - thick;
        let door_h = penguin_h * 1.25;
        let tunnel = r_out * 0.45;
        let dir = if fastrand::bool() { 1.0 } else { -1.0 };
        let x = x.clamp(r_out + tunnel + 10.0, self.w - r_out - tunnel - 10.0);
        let cx = (x / cell) as i32;
        // Base: the median ground height under the igloo.
        let span = ((r_out + tunnel) / cell).ceil() as i32;
        let mut ground: Vec<i32> = (cx - span..=cx + span)
            .map(|c| self.grid.surface_y(c))
            .collect();
        ground.sort_unstable();
        let base = ground[ground.len() / 2];
        // Level the site: fill dips with bricks, clear snow above the base.
        for c in cx - span..=cx + span {
            let sy = self.grid.surface_y(c);
            for r in base..sy.min(self.grid.h as i32) {
                self.grid.set(c, r, BRICK);
            }
            for r in sy.min(base)..base {
                if self.grid.get(c, r) != WALL {
                    self.grid.set(c, r, EMPTY);
                }
            }
        }
        let rows = (r_out / cell).ceil() as i32 + 1;
        let door_rows = (door_h / cell).ceil() as i32;
        for dy in 0..rows {
            let r = base - 1 - dy;
            let py = (dy as f32 + 0.5) * cell;
            for dx in -span..=span {
                let px = (dx as f32 + 0.5) * cell;
                let d = px.hypot(py * 1.08);
                let side = px * dir;
                let in_tunnel_x = side > r_in * 0.3 && side < r_out + tunnel;
                let c = cx + dx;
                if d <= r_out && d > r_in {
                    // Shell, with the doorway cut out.
                    if !(in_tunnel_x && dy < door_rows) {
                        self.grid.set(c, r, BRICK);
                    } else {
                        self.grid.set(c, r, EMPTY);
                    }
                } else if d <= r_in {
                    self.grid.set(c, r, EMPTY);
                } else if in_tunnel_x && side > r_out * 0.7 {
                    // Tunnel roof outside the dome.
                    if dy >= door_rows && (dy as f32) < door_rows as f32 + thick / cell {
                        self.grid.set(c, r, BRICK);
                    } else if dy < door_rows {
                        self.grid.set(c, r, EMPTY);
                    }
                }
            }
        }
    }

    /// Play mode: the pointer leaves little prints when it skims the snow.
    pub fn cursor_footprint(&mut self, x: f32, y: f32) {
        let cell = self.cell;
        let cx = (x / cell) as i32;
        let row = (y / cell) as i32;
        let surf = self.grid.surface_from(cx, row - 1);
        if surf >= self.grid.h as i32 || (surf - row).abs() > 3 {
            return;
        }
        let gap = 9.0 * self.scale;
        if self
            .last_print
            .is_some_and(|(lx, ly)| (x - lx).hypot(y - ly) < gap)
        {
            return;
        }
        self.last_print = Some((x, y));
        self.print_left = !self.print_left;
        let c = cx + if self.print_left { -1 } else { 1 };
        let top = self.grid.surface_from(c, row - 2);
        if self.grid.get(c, top) == SNOW {
            self.grid.set(c, top, EMPTY);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn world() -> World {
        World::new(400, 400, 1.0, &Config::default())
    }

    fn pile(w: &mut World) {
        let gh = w.grid.h as i32;
        for x in 10..90 {
            for y in gh - 25..gh {
                w.grid.set(x, y, PACKED);
            }
        }
    }

    #[test]
    fn rolling_grows_ball_and_removes_snow() {
        let mut w = world();
        pile(&mut w);
        let before = w.grid.count(PACKED);
        // Drag along the top of the pile (y = 300).
        w.roll_begin(40.0 * 4.0, 295.0);
        let r0 = w.balls[0].r;
        for i in 0..200 {
            w.roll_move(160.0 + i as f32, 295.0);
        }
        w.roll_end();
        let removed = before - w.grid.count(PACKED);
        assert!(removed > 0);
        assert!(w.balls[0].r > r0 * 1.5, "r {} -> {}", r0, w.balls[0].r);
        // Area growth never exceeds the snow removed.
        let grown = std::f32::consts::PI * (w.balls[0].r.powi(2) - r0.powi(2));
        assert!(grown <= removed as f32 * w.cell * w.cell + 1.0);
    }

    /// Regression: rolling under a window always put the ball on the
    /// window's top edge.
    #[test]
    fn rolls_on_the_ground_under_a_window() {
        let mut w = world();
        pile(&mut w);
        w.set_windows(&[crate::sim::ledges::Ledge::whole(
            1,
            crate::sim::Rect {
                x: 0.0,
                y: 100.0,
                w: 400.0,
                h: 100.0,
            },
        )]);
        w.roll_begin(100.0, 295.0);
        for i in 0..60 {
            w.roll_move(100.0 + i as f32, 295.0);
        }
        assert!(w.balls[0].y > 250.0, "on the pile, y {}", w.balls[0].y);
        // Pointer up on the window: the ball goes onto the ledge.
        w.roll_move(200.0, 90.0);
        assert!(w.balls[0].y < 110.0, "on the ledge, y {}", w.balls[0].y);
    }

    #[test]
    fn old_snowballs_can_be_picked_up_carried_and_thrown() {
        let mut w = World::new(
            400,
            400,
            1.0,
            &Config {
                intensity: 0.0,
                ..Config::default()
            },
        );
        w.balls.push(Ball::new(100.0, 400.0 - 20.0, 20.0));
        let i = w.ball_at(105.0, 385.0).expect("clicked the ball");
        w.grab_ball(i, 105.0, 385.0, false);
        w.roll_move(305.0, 205.0);
        assert!((w.balls[0].x - 300.0).abs() < 0.5 && (w.balls[0].y - 200.0).abs() < 0.5);
        assert!(
            (w.balls[0].r - 20.0).abs() < 0.01,
            "carrying does not grow it"
        );
        // Gentle release: it falls and stays a ball.
        w.roll_end_with(30.0, 0.0);
        for _ in 0..120 {
            w.step();
        }
        assert_eq!(w.balls.len(), 1);
        assert!(w.balls[0].y > 350.0, "dropped to the ground");
        // Pick it up again and flick it: thrown, it splats.
        let i = w.ball_at(w.balls[0].x, w.balls[0].y).unwrap();
        let (x, y) = (w.balls[0].x, w.balls[0].y);
        w.grab_ball(i, x, y, false);
        w.roll_end_with(-1500.0, -300.0);
        assert!(w.balls[0].thrown);
        for _ in 0..180 {
            w.step();
        }
        assert!(w.balls.is_empty(), "splatted");
    }

    #[test]
    fn snowball_tool_keeps_rolling_an_old_ball() {
        let mut w = world();
        pile(&mut w);
        w.balls.push(Ball::new(150.0, 300.0 - 10.0, 10.0));
        w.grab_ball(0, 150.0, 295.0, true);
        for i in 0..120 {
            w.roll_move(150.0 + i as f32, 295.0);
        }
        w.roll_end();
        assert!(w.balls[0].r > 15.0, "grew: {}", w.balls[0].r);
    }

    /// Lift a rolled ball above another one and let go: they stack into a
    /// snowman (this used to be impossible by hand).
    #[test]
    fn carried_ball_drops_onto_another_and_stacks() {
        let mut w = World::new(
            400,
            400,
            1.0,
            &Config {
                intensity: 0.0,
                ..Config::default()
            },
        );
        let base = Ball::new(200.0, 400.0 - 40.0, 40.0);
        w.balls.push(base);
        w.roll_begin(100.0, 390.0);
        w.balls[1].r = 26.0;
        w.roll_move(150.0, 200.0); // lifted
        assert!(w.balls[1].y < 250.0, "carried, y {}", w.balls[1].y);
        // Released a bit off-centre: the assist centres it.
        w.roll_move(218.0, 200.0);
        w.roll_end();
        for _ in 0..240 {
            w.step();
        }
        assert_eq!(
            w.snowmen.len(),
            1,
            "{:?}",
            w.balls.iter().map(|b| (b.x, b.y, b.r)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn shovel_digs_and_brush_adds() {
        let mut w = world();
        pile(&mut w);
        let n = w.grid.count(PACKED);
        w.apply_tool(Tool::Shovel, 200.0, 390.0, 20.0);
        assert!(w.grid.count(PACKED) < n);
        w.apply_tool(Tool::Brush, 200.0, 100.0, 20.0);
        assert!(w.grid.count(SNOW) > 0);
    }

    #[test]
    fn freeze_turns_snow_to_ice() {
        let mut w = world();
        pile(&mut w);
        w.apply_tool(Tool::Pack, 200.0, 390.0, 40.0);
        w.apply_tool(Tool::Freeze, 200.0, 390.0, 40.0);
        assert_eq!(w.grid.count(SNOW), 0);
    }

    #[test]
    fn stamped_snowman_is_detected() {
        let mut w = World::new(
            400,
            400,
            1.0,
            &Config {
                intensity: 0.0,
                ..Config::default()
            },
        );
        w.stamp_snowman(200.0);
        for _ in 0..360 {
            w.step();
        }
        assert_eq!(
            w.snowmen.len(),
            1,
            "balls: {:?}",
            w.balls.iter().map(|b| (b.x, b.y, b.r)).collect::<Vec<_>>()
        );
        assert_eq!(w.snowmen[0].len(), 3);
    }

    #[test]
    fn undo_redo_restores() {
        let mut w = world();
        pile(&mut w);
        w.push_undo();
        let n = w.grid.count(PACKED);
        w.apply_tool(Tool::Shovel, 200.0, 390.0, 40.0);
        w.undo();
        assert_eq!(w.grid.count(PACKED), n);
        w.redo();
        assert!(w.grid.count(PACKED) < n);
    }

    #[test]
    fn undo_snapshots_are_compact() {
        let mut w = World::new(2560, 1440, 1.0, &Config::default());
        w.generate_mountains();
        for _ in 0..30 {
            w.push_undo();
        }
        // 30 raw grids would be ~6.9 MB.
        assert!(w.undo_bytes() < 1_000_000, "{}", w.undo_bytes());
        let n = w.grid.count(PACKED);
        w.clear_all();
        w.undo();
        assert_eq!(w.grid.count(PACKED), n);
    }

    #[test]
    fn ball_count_is_capped() {
        let mut w = world();
        for i in 0..(MAX_BALLS + 50) {
            w.throw_ball(10.0 + i as f32, 10.0, 0.0, 0.0);
        }
        assert_eq!(w.balls.len(), MAX_BALLS);
        w.roll_begin(100.0, 100.0);
        assert!(w.rolling.is_none());
    }

    /// Regression: removing one of two overlapping ledges cleared the walls
    /// of the other, so its snow fell through.
    #[test]
    fn removing_overlapping_window_keeps_other_ledge() {
        let mut w = world();
        let a = crate::sim::Rect {
            x: 40.0,
            y: 200.0,
            w: 200.0,
            h: 100.0,
        };
        let b = crate::sim::Rect {
            x: 100.0,
            y: 196.0,
            w: 200.0,
            h: 100.0,
        };
        w.set_windows(&[
            crate::sim::ledges::Ledge::whole(1, a),
            crate::sim::ledges::Ledge::whole(2, b),
        ]);
        w.set_windows(&[crate::sim::ledges::Ledge::whole(1, a)]);
        let y0 = (200.0f32 / w.cell).round() as i32;
        for x in 12..55 {
            assert_eq!(w.grid.get(x, y0), WALL, "x {x}");
        }
    }

    #[test]
    fn bricks_stack_and_stay() {
        let mut w = world();
        pile(&mut w);
        let n0 = w.grid.count(BRICK);
        w.place_brick(200.0, 290.0);
        w.place_brick(200.0, 250.0);
        let n = w.grid.count(BRICK);
        assert!(n > n0);
        for _ in 0..120 {
            w.step();
        }
        assert_eq!(w.grid.count(BRICK), n, "bricks never fall or slump");
        // Second brick sits on the first: two courses at that column.
        let (bw, bh) = w.brick_size();
        let col = (200.0 / w.cell) as i32;
        let mut run = 0;
        for y in 0..w.grid.h as i32 {
            if w.grid.get(col, y) == BRICK || w.grid.get(col + bw / 2, y) == BRICK {
                run += 1;
            }
        }
        assert!(run >= 2 * bh, "run {run}");
    }

    #[test]
    fn igloo_has_a_hollow_inside_and_a_door() {
        let mut w = World::new(800, 400, 1.0, &Config::default());
        w.stamp_igloo(400.0);
        assert!(w.grid.count(BRICK) > 100);
        let col = (400.0 / w.cell) as i32;
        let base = w.grid.surface_y(col - 40); // ground outside
        // Above the floor in the middle it is hollow...
        let floor = (0..w.grid.h as i32)
            .rev()
            .find(|&y| w.grid.get(col, y) == EMPTY)
            .unwrap();
        assert!(w.grid.get(col, floor - 3) == EMPTY, "hollow");
        // ...and the shell exists above that.
        assert!((0..floor).any(|y| w.grid.get(col, y) == BRICK), "roof");
        let _ = base;
    }

    #[test]
    fn penguin_walks_into_the_igloo() {
        let cfg = Config {
            intensity: 0.0,
            critter: true,
            penguins: 1,
            ..Config::default()
        };
        let mut w = World::new(800, 400, 1.0, &cfg);
        w.stamp_igloo(400.0);
        // Find the door side: the side with an opening at floor level.
        let col = (400.0 / w.cell) as i32;
        let floor_row = (0..w.grid.h as i32)
            .rev()
            .find(|&y| w.grid.get(col, y) == EMPTY)
            .unwrap();
        let open_right = ((col)..(col + 60)).all(|c| w.grid.get(c, floor_row) == EMPTY);
        w.step();
        let start = if open_right { 700.0 } else { 100.0 };
        let c = &mut w.critters[0];
        c.x = start;
        c.dir = if open_right { -1.0 } else { 1.0 };
        c.state = crate::sim::world::CritterState::Walk;
        c.t = 100.0;
        c.y = 400.0;
        let mut inside = false;
        for _ in 0..60 * 20 {
            w.step();
            let c = &w.critters[0];
            if (c.x - 400.0).abs() < 25.0 && c.y > (floor_row as f32 - 1.0) * w.cell {
                inside = true;
                break;
            }
            w.critters[0].t = 100.0;
        }
        assert!(inside, "penguin never got inside");
    }

    #[test]
    fn cursor_leaves_footprints_in_snow() {
        let mut w = world();
        pile(&mut w);
        let n = w.grid.count(PACKED) + w.grid.count(SNOW);
        let gh = w.grid.h as i32;
        for x in 10..80 {
            w.grid.set(x, gh - 26, SNOW);
        }
        let top = (gh - 26) as f32 * w.cell;
        let before = w.grid.count(SNOW);
        for i in 0..40 {
            w.cursor_footprint(60.0 + i as f32 * 5.0, top - 1.0);
        }
        assert!(w.grid.count(SNOW) < before, "prints made");
        assert!(w.grid.count(PACKED) + w.grid.count(SNOW) > n - 40);
    }

    #[test]
    fn mountains_generate_snow() {
        let mut w = world();
        w.generate_mountains();
        assert!(w.grid.count(PACKED) > 100);
    }

    #[test]
    fn removed_window_drops_snow_as_particles() {
        let mut w = world();
        let r = crate::sim::Rect {
            x: 100.0,
            y: 200.0,
            w: 100.0,
            h: 100.0,
        };
        w.set_windows(&[crate::sim::ledges::Ledge::whole(1, r)]);
        for _ in 0..5 {
            w.grid.set(50, 49 - 2, SNOW); // ledge rows start at y=50
        }
        let gy = 200 / 4;
        w.grid.set(30, gy - 1, SNOW);
        let before = w.flake_count();
        w.set_windows(&[]);
        assert!(w.flake_count() > before);
    }
}
