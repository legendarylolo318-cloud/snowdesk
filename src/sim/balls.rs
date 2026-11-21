//! Rigid snowballs: circle physics, stacking, snowman detection.

use super::grid::Grid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum DecorKind {
    Eye = 0,
    Nose = 1,
    Arm = 2,
    Hat = 3,
    Scarf = 4,
    Button = 5,
}

impl DecorKind {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Eye,
            1 => Self::Nose,
            2 => Self::Arm,
            3 => Self::Hat,
            4 => Self::Scarf,
            _ => Self::Button,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Decor {
    pub kind: DecorKind,
    /// Palette index (scarf colours).
    pub color: u8,
    /// Offset from ball centre in units of the ball radius.
    pub dx: f32,
    pub dy: f32,
}

#[derive(Clone, Debug, Default)]
pub struct Ball {
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    pub r: f32,
    pub rot: f32,
    pub decor: Vec<Decor>,
    /// Seconds of snowman wobble left.
    pub wobble: f32,
    /// Being rolled by the user: kinematic.
    pub held: bool,
    /// Thrown projectile: splats on impact.
    pub thrown: bool,
    pub in_snowman: bool,
    /// Thrown by a penguin (does not make penguins angry).
    pub by_penguin: bool,
}

impl Ball {
    pub fn new(x: f32, y: f32, r: f32) -> Self {
        Self {
            x,
            y,
            r,
            ..Default::default()
        }
    }
    fn mass(&self) -> f32 {
        self.r * self.r
    }
}

pub const GRAVITY: f32 = 1400.0;

/// Ground contact for a circle against the grid height field.
/// Returns (max penetration, contact centroid dx).
pub fn ground_contact(b: &Ball, grid: &Grid, cell: f32) -> (f32, f32) {
    let mut pen = 0.0f32;
    let mut sum_dx = 0.0f32;
    let mut sum_w = 0.0f32;
    let n = (b.r / cell).floor() as i32;
    let top_row = (((b.y - b.r * 0.5) / cell).floor() as i32).max(0);
    for k in -n..=n {
        let dx = k as f32 * cell;
        let cx = ((b.x + dx) / cell).floor() as i32;
        let mut row = top_row;
        while row < grid.h as i32 && grid.get(cx, row) == 0 {
            row += 1;
        }
        let gy = row as f32 * cell;
        let yb = b.y + (b.r * b.r - dx * dx).max(0.0).sqrt();
        let p = yb - gy;
        if p > 0.0 {
            pen = pen.max(p);
            sum_dx += dx * p;
            sum_w += p;
        }
    }
    (pen, if sum_w > 0.0 { sum_dx / sum_w } else { 0.0 })
}

/// Ground height (px) under column `x`, scanning down from `from_row`.
fn ground_y(grid: &Grid, cell: f32, x: f32, from_row: i32) -> f32 {
    let cx = (x / cell).floor() as i32;
    let mut row = from_row.max(0);
    while row < grid.h as i32 && grid.get(cx, row) == 0 {
        row += 1;
    }
    row as f32 * cell
}

/// Advance all balls one fixed step. Returns indices of thrown balls that
/// hit something (index, hit kind: 0 = ground/snow, 1 = ball).
#[cfg(test)]
pub fn step_balls(balls: &mut [Ball], grid: &Grid, cell: f32, dt: f32) -> Vec<(usize, u8)> {
    step_balls_open(balls, grid, cell, dt, (false, false))
}

/// Like [`step_balls`]; thrown balls pass through screen sides marked open
/// (left, right) so they can fly on to the next monitor.
pub fn step_balls_open(
    balls: &mut [Ball],
    grid: &Grid,
    cell: f32,
    dt: f32,
    open: (bool, bool),
) -> Vec<(usize, u8)> {
    let mut hits = Vec::new();
    let wpx = grid.w as f32 * cell;
    // A recognised snowman keeps its shape: its slow balls hold their x
    // (balls stacked on spheres are otherwise an unstable equilibrium and
    // roll off after a few seconds). They still fall if the ground goes,
    // and a snowball hit (velocity) knocks them loose.
    let lock_x: Vec<Option<f32>> = balls
        .iter()
        .map(|b| {
            (b.in_snowman && !b.held && !b.thrown && b.vx.abs() < 60.0 && b.vy.abs() < 200.0)
                .then_some(b.x)
        })
        .collect();
    for (i, b) in balls.iter_mut().enumerate() {
        b.wobble = (b.wobble - dt).max(0.0);
        if b.held {
            continue;
        }
        b.vy += GRAVITY * dt;
        b.x += b.vx * dt;
        b.y += b.vy * dt;
        if b.x < b.r && !(b.thrown && open.0) {
            b.x = b.r;
            b.vx = b.vx.abs() * 0.3;
        }
        if b.x > wpx - b.r && !(b.thrown && open.1) {
            b.x = wpx - b.r;
            b.vx = -b.vx.abs() * 0.3;
        }
        let (pen, _) = ground_contact(b, grid, cell);
        if pen > 0.0 {
            if b.thrown {
                hits.push((i, 0));
                continue;
            }
            b.y -= pen;
            if b.vy > 0.0 {
                b.vy = -b.vy * 0.15;
                if b.vy.abs() < 30.0 {
                    b.vy = 0.0;
                }
            }
            // Roll downhill. The slope is the terrain height difference
            // across the ball, so one snow cell landing at its edge does not
            // send a resting ball (or a whole snowman) rolling away. Static
            // friction: only real slopes make a ball roll.
            let top_row = ((b.y - b.r * 0.5) / cell).floor() as i32;
            let span = b.r * 0.7;
            let slope = ((ground_y(grid, cell, b.x + span, top_row)
                - ground_y(grid, cell, b.x - span, top_row))
                / (2.0 * span))
                .clamp(-1.5, 1.5);
            if slope.abs() > 0.3 {
                b.vx += slope * GRAVITY * 0.6 * dt;
            }
            b.vx *= 1.0 - 2.0 * dt;
            if b.vx.abs() < 2.0 {
                b.vx = 0.0;
            }
        }
        b.rot += b.vx / b.r * dt;
    }
    // Ball-ball collisions.
    for _ in 0..3 {
        for i in 0..balls.len() {
            for j in (i + 1)..balls.len() {
                let (a, b) = balls.split_at_mut(j);
                let (a, b) = (&mut a[i], &mut b[0]);
                let (dx, dy) = (b.x - a.x, b.y - a.y);
                let d2 = dx * dx + dy * dy;
                let rr = a.r + b.r;
                if d2 >= rr * rr || d2 < 1e-6 {
                    continue;
                }
                if a.thrown || b.thrown {
                    let (t, o) = if a.thrown { (i, j) } else { (j, i) };
                    hits.push((t, 1));
                    let (tx, ty, m) = if a.thrown {
                        (a.vx, a.vy, a.mass() / b.mass())
                    } else {
                        (b.vx, b.vy, b.mass() / a.mass())
                    };
                    let other = if o == j { &mut *b } else { &mut *a };
                    other.vx += tx * m.min(1.5) * 0.8;
                    other.vy += ty * m.min(1.5) * 0.4 - 80.0;
                    other.wobble = 1.5;
                    continue;
                }
                let d = d2.sqrt();
                let (nx, ny) = (dx / d, dy / d);
                let pen = rr - d;
                let (ma, mb) = (a.mass(), b.mass());
                let (wa, wb) = (mb / (ma + mb), ma / (ma + mb));
                // Held balls are immovable.
                let (wa, wb) = if a.held {
                    (0.0, 1.0)
                } else if b.held {
                    (1.0, 0.0)
                } else {
                    (wa, wb)
                };
                a.x -= nx * pen * wa;
                a.y -= ny * pen * wa;
                b.x += nx * pen * wb;
                b.y += ny * pen * wb;
                let rvx = b.vx - a.vx;
                let rvy = b.vy - a.vy;
                let vn = rvx * nx + rvy * ny;
                if vn < 0.0 {
                    let jn = -vn * 1.05;
                    a.vx -= nx * jn * wa;
                    a.vy -= ny * jn * wa;
                    b.vx += nx * jn * wb;
                    b.vy += ny * jn * wb;
                }
                // Friction so stacks stay put.
                let (tx, ty) = (-ny, nx);
                let vt = rvx * tx + rvy * ty;
                a.vx += tx * vt * 0.3 * wa;
                a.vy += ty * vt * 0.3 * wa;
                b.vx -= tx * vt * 0.3 * wb;
                b.vy -= ty * vt * 0.3 * wb;
            }
        }
    }
    for (b, lx) in balls.iter_mut().zip(&lock_x) {
        if let Some(x) = *lx
            && b.vx.abs() < 60.0
        {
            b.x = x;
            b.vx = 0.0;
        }
    }
    // A thrown ball can hit the ground and another ball in the same step:
    // report each ball once (ground wins) or it would be removed twice.
    hits.sort_unstable();
    hits.dedup_by_key(|h| h.0);
    hits
}

/// Find stacks of 2–3 balls: each upper ball is smaller than and resting on
/// the one below, roughly centred. Returns chains from bottom to top.
pub fn find_snowmen(balls: &[Ball]) -> Vec<Vec<usize>> {
    let mut order: Vec<usize> = (0..balls.len())
        .filter(|&i| !balls[i].thrown && !balls[i].held)
        .collect();
    order.sort_by(|&a, &b| balls[b].y.total_cmp(&balls[a].y)); // lowest first
    let mut used = vec![false; balls.len()];
    let mut out = Vec::new();
    for &bot in &order {
        if used[bot] {
            continue;
        }
        let mut chain = vec![bot];
        loop {
            let b = &balls[*chain.last().unwrap()];
            let up = order.iter().copied().find(|&u| {
                let o = &balls[u];
                !used[u]
                    && !chain.contains(&u)
                    && o.r < b.r * 0.98
                    && o.r > b.r * 0.35
                    && o.y < b.y
                    && (o.x - b.x).abs() < b.r * 0.5
                    && ((o.x - b.x).hypot(o.y - b.y)) < (o.r + b.r) * 1.12
                    && o.vx.abs() + o.vy.abs() < 40.0
            });
            match up {
                Some(u) if chain.len() < 3 => chain.push(u),
                _ => break,
            }
        }
        if chain.len() >= 2 {
            for &c in &chain {
                used[c] = true;
            }
            out.push(chain);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_two_and_three_stacks() {
        let b = |x, y, r| Ball::new(x, y, r);
        let balls = vec![
            b(100.0, 500.0, 40.0),
            b(101.0, 430.0, 30.0),
            b(100.0, 380.0, 20.0),
        ];
        let s = find_snowmen(&balls);
        assert_eq!(s, vec![vec![0, 1, 2]]);
    }

    #[test]
    fn rejects_side_by_side_and_bigger_on_top() {
        let b = |x, y, r| Ball::new(x, y, r);
        let side = vec![b(100.0, 500.0, 40.0), b(190.0, 510.0, 30.0)];
        assert!(find_snowmen(&side).is_empty());
        let inverted = vec![b(100.0, 500.0, 20.0), b(100.0, 450.0, 40.0)];
        // Bigger ball is the lowest in chain order only if lower; here it is above.
        assert!(find_snowmen(&inverted).is_empty());
    }

    /// Regression: a throw touching the ground and a ball in one step was
    /// reported twice and removed twice (wrong ball removed / panic).
    #[test]
    fn thrown_ball_reported_once() {
        let grid = Grid::new(100, 100);
        let cell = 4.0;
        let mut target = Ball::new(200.0, 400.0 - 30.0, 30.0);
        target.vy = 0.0;
        let mut t = Ball::new(200.0 - 15.0, 400.0 - 6.0, 6.0);
        t.thrown = true;
        t.vy = 400.0;
        let mut balls = vec![target, t];
        let hits = step_balls(&mut balls, &grid, cell, 1.0 / 60.0);
        let idx: Vec<usize> = hits.iter().map(|h| h.0).collect();
        let mut d = idx.clone();
        d.dedup();
        assert_eq!(idx, d);
        assert!(idx.contains(&1));
    }

    #[test]
    fn edge_bump_does_not_roll_resting_ball() {
        let mut grid = Grid::new(100, 100);
        let cell = 4.0;
        // One snow cell at the right edge under the ball.
        grid.set(59, 99, crate::sim::grid::SNOW);
        let mut balls = vec![Ball::new(210.0, 400.0 - 34.0, 34.0)];
        for _ in 0..240 {
            step_balls(&mut balls, &grid, cell, 1.0 / 60.0);
        }
        assert!((balls[0].x - 210.0).abs() < 4.0, "x {}", balls[0].x);
    }

    #[test]
    fn ball_rolls_down_a_real_slope() {
        let mut grid = Grid::new(100, 100);
        let cell = 4.0;
        // Ramp falling to the right: 1 cell per column.
        for x in 0..100 {
            for y in (50 + x / 2)..100 {
                grid.set(x, y, crate::sim::grid::PACKED);
            }
        }
        let gx = 30.0;
        let gy = (50 + 30 / 2) as f32 * cell;
        let mut balls = vec![Ball::new(gx * cell, gy - 20.0, 16.0)];
        for _ in 0..120 {
            step_balls(&mut balls, &grid, cell, 1.0 / 60.0);
        }
        assert!(balls[0].x > gx * cell + 20.0, "x {}", balls[0].x);
    }

    /// Regression: three-ball snowmen fell apart after a few seconds.
    #[test]
    fn three_ball_snowman_stays_up() {
        let grid = Grid::new(100, 100);
        let cell = 4.0;
        let floor = 400.0;
        let mut balls = vec![
            Ball::new(200.0, floor - 45.0, 45.0),
            Ball::new(201.5, floor - 90.0 - 32.0, 32.0),
            Ball::new(199.0, floor - 154.0 - 21.0, 21.0),
        ];
        for step in 0..1200 {
            step_balls(&mut balls, &grid, cell, 1.0 / 60.0);
            if step % 20 == 0 {
                let found = find_snowmen(&balls);
                for b in balls.iter_mut() {
                    b.in_snowman = false;
                }
                for c in &found {
                    for &i in c {
                        balls[i].in_snowman = true;
                    }
                }
            }
        }
        assert_eq!(find_snowmen(&balls), vec![vec![0, 1, 2]]);
    }

    #[test]
    fn stack_stays_stable_on_floor() {
        let grid = Grid::new(100, 100);
        let cell = 4.0;
        let floor = 400.0;
        let mut balls = vec![
            Ball::new(200.0, floor - 40.0, 40.0),
            Ball::new(200.0, floor - 80.0 - 28.0, 28.0),
        ];
        for _ in 0..300 {
            step_balls(&mut balls, &grid, cell, 1.0 / 60.0);
        }
        assert!((balls[0].y - (floor - 40.0)).abs() < 3.0);
        assert!((balls[1].x - balls[0].x).abs() < 0.5 * balls[0].r);
        assert_eq!(find_snowmen(&balls).len(), 1);
    }
}
