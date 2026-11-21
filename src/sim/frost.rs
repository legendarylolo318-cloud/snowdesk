//! Frost that creeps in from the screen corners while you are idle. The
//! pointer wipes it away (or writes in it); wiped frost heals slowly the
//! next time you leave the computer alone.

/// Seconds without pointer movement before frost starts to grow.
const IDLE_BEFORE: f32 = 12.0;
/// Seconds of idling until frost reaches its full extent.
const FULL_AFTER: f32 = 240.0;
/// Full extent, as a fraction of the screen diagonal from each corner.
const MAX_REACH: f32 = 0.2;
/// Growth per update (0..255); about 16 s for a cell to fully frost.
const GROW: u8 = 4;

pub struct Frost {
    /// Frost map size in cells and cell size in px.
    pub w: u32,
    pub h: u32,
    pub cell: f32,
    pub data: Vec<u8>,
    /// Changed since the renderer last uploaded it.
    pub dirty: bool,
    /// Any frost at all (skip the pass otherwise).
    pub any: bool,
    level: f32,
    idle: f32,
    noise: Vec<u8>,
}

fn hash(i: u32) -> u8 {
    let mut x = i.wrapping_mul(0x9E37_79B1) ^ 0x5bd1_e995;
    x ^= x >> 15;
    x = x.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 13;
    (x & 0xff) as u8
}

/// Smooth value noise (one random value every 6 cells, blended), so the
/// frost edge forms soft organic lobes instead of speckles.
fn smooth_noise(w: u32, h: u32) -> Vec<u8> {
    const STEP: u32 = 6;
    let (gw, gh) = (w / STEP + 2, h / STEP + 2);
    let node = |x: u32, y: u32| hash(y * gw + x + 17) as f32;
    let mut out = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            let (gx, gy) = (x / STEP, y / STEP);
            let (fx, fy) = (
                (x % STEP) as f32 / STEP as f32,
                (y % STEP) as f32 / STEP as f32,
            );
            let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
            let top = node(gx, gy) * (1.0 - sx) + node(gx + 1, gy) * sx;
            let bot = node(gx, gy + 1) * (1.0 - sx) + node(gx + 1, gy + 1) * sx;
            out.push((top * (1.0 - sy) + bot * sy) as u8);
        }
    }
    let _ = gh;
    out
}

impl Frost {
    pub fn new(px_w: f32, px_h: f32, scale: f32) -> Self {
        let cell = (12.0 * scale).max(6.0);
        let w = (px_w / cell).ceil().max(1.0) as u32;
        let h = (px_h / cell).ceil().max(1.0) as u32;
        let n = (w * h) as usize;
        Self {
            w,
            h,
            cell,
            data: vec![0; n],
            dirty: true,
            any: false,
            level: 0.0,
            idle: 0.0,
            noise: smooth_noise(w, h),
        }
    }

    /// Advance by `dt` seconds.
    pub fn step(&mut self, dt: f32, enabled: bool) {
        if !enabled {
            if self.any {
                self.data.iter_mut().for_each(|v| *v = 0);
                self.any = false;
                self.dirty = true;
            }
            self.level = 0.0;
            self.idle = 0.0;
            return;
        }
        // Recording/visual-check aid (debug builds): grow frost 10x faster
        // regardless of pointer activity.
        #[cfg(debug_assertions)]
        let dt = if std::env::var_os("SNOWDESK_FROST_FAST").is_some() {
            self.idle = IDLE_BEFORE;
            dt * 10.0
        } else {
            dt
        };
        self.idle += dt;
        if self.idle < IDLE_BEFORE {
            return;
        }
        self.level = (self.level + dt / FULL_AFTER).min(1.0);
        let (w, h) = (self.w as f32, self.h as f32);
        let diag = w.hypot(h);
        let reach = self.level * MAX_REACH;
        let mut changed = false;
        for y in 0..self.h {
            for x in 0..self.w {
                let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                let d = fx.min(w - fx).hypot(fy.min(h - fy)) / diag;
                let i = (y * self.w + x) as usize;
                let n = self.noise[i] as f32 / 255.0 - 0.5;
                let t = ((reach + n * 0.07 - d) / 0.04).clamp(0.0, 1.0);
                let target = (t * 255.0) as u8;
                let v = &mut self.data[i];
                if *v < target {
                    *v = v.saturating_add(GROW).min(target);
                    changed = true;
                }
            }
        }
        if changed {
            self.any = true;
            self.dirty = true;
        }
    }

    /// Someone is using the computer: no frost growth for a while.
    pub fn touch(&mut self) {
        self.idle = 0.0;
    }

    /// The pointer moved at (x, y) px: wipe a stroke and reset the idle time.
    pub fn wipe(&mut self, x: f32, y: f32, radius: f32) {
        self.idle = 0.0;
        if !self.any {
            return;
        }
        let r = (radius / self.cell).ceil() as i32;
        let (cx, cy) = ((x / self.cell) as i32, (y / self.cell) as i32);
        for dy in -r..=r {
            for dx in -r..=r {
                let (gx, gy) = (cx + dx, cy + dy);
                if gx < 0 || gy < 0 || gx >= self.w as i32 || gy >= self.h as i32 {
                    continue;
                }
                let (px, py) = (
                    (gx as f32 + 0.5) * self.cell - x,
                    (gy as f32 + 0.5) * self.cell - y,
                );
                if px.hypot(py) <= radius {
                    let v = &mut self.data[(gy as u32 * self.w + gx as u32) as usize];
                    if *v > 0 {
                        *v = 0;
                        self.dirty = true;
                    }
                }
            }
        }
    }

    #[cfg(test)]
    pub fn at(&self, x: f32, y: f32) -> u8 {
        self.data[((y / self.cell) as u32 * self.w + (x / self.cell) as u32) as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grows_from_corners_only_when_idle_and_wipes() {
        let mut f = Frost::new(1920.0, 1080.0, 1.0);
        for _ in 0..40 {
            f.step(0.25, true); // 10 s: not idle long enough yet
        }
        assert!(!f.any);
        for _ in 0..4 * 300 {
            f.step(0.25, true);
        }
        assert!(f.any);
        assert!(f.at(5.0, 5.0) > 200, "corner frosted");
        assert_eq!(f.at(960.0, 540.0), 0, "centre stays clear");
        f.wipe(5.0, 5.0, 30.0);
        assert_eq!(f.at(5.0, 5.0), 0, "wiped");
        f.step(0.25, true);
        assert_eq!(f.at(5.0, 5.0), 0, "does not regrow while in use");
        f.step(0.25, false);
        assert!(!f.any, "turned off: cleared");
    }
}
