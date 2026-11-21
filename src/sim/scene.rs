//! Scene save/load: small hand-rolled binary format (RLE grid + balls).

use super::balls::{Ball, Decor, DecorKind};
use super::grid::{Grid, WALL};
use super::world::World;

const MAGIC: &[u8; 5] = b"SNOW1";
/// Largest grid side accepted from a file (a 16K screen at 2 px cells).
const MAX_GRID: usize = 8192;

fn put_u32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}
fn put_f32(v: &mut Vec<u8>, x: f32) {
    v.extend_from_slice(&x.to_le_bytes());
}

struct Rd<'a>(&'a [u8]);
impl Rd<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        if self.0.len() < n {
            return None;
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Some(a)
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn f32(&mut self) -> Option<f32> {
        Some(f32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
}

impl World {
    pub fn encode_scene(&self, out: &mut Vec<u8>) {
        put_u32(out, self.grid.w as u32);
        put_u32(out, self.grid.h as u32);
        // RLE of cells with walls stored as empty.
        let mut runs: Vec<(u8, u32)> = Vec::new();
        for &c in &self.grid.cells {
            let c = if c == WALL { 0 } else { c };
            match runs.last_mut() {
                Some((v, n)) if *v == c => *n += 1,
                _ => runs.push((c, 1)),
            }
        }
        put_u32(out, runs.len() as u32);
        for (v, n) in runs {
            out.push(v);
            put_u32(out, n);
        }
        let balls: Vec<&Ball> = self.balls.iter().filter(|b| !b.thrown).collect();
        put_u32(out, balls.len() as u32);
        for b in balls {
            put_f32(out, b.x / self.w);
            put_f32(out, b.y / self.h);
            put_f32(out, b.r / self.scale);
            put_u32(out, b.decor.len() as u32);
            for d in &b.decor {
                out.push(d.kind as u8);
                out.push(d.color);
                put_f32(out, d.dx);
                put_f32(out, d.dy);
            }
        }
    }

    fn decode_scene(&mut self, r: &mut Rd) -> Option<()> {
        let (w, h) = (r.u32()? as usize, r.u32()? as usize);
        // A damaged or hostile file must not trigger a huge allocation or a
        // zero-sized grid (division / indexing below).
        if w == 0 || h == 0 || w > MAX_GRID || h > MAX_GRID {
            return None;
        }
        let nruns = r.u32()? as usize;
        let mut cells = Vec::with_capacity(w * h);
        for _ in 0..nruns {
            let v = r.u8()?;
            let n = r.u32()? as usize;
            if cells.len() + n > w * h || v > crate::sim::grid::MAX_CELL || v == WALL {
                return None;
            }
            cells.resize(cells.len() + n, v);
        }
        if cells.len() != w * h {
            return None;
        }
        let nb = r.u32()? as usize;
        let mut balls = Vec::new();
        for _ in 0..nb {
            let (x, y, rad) = (r.f32()?, r.f32()?, r.f32()?);
            let nd = r.u32()? as usize;
            let mut decor = Vec::new();
            for _ in 0..nd {
                let kind = DecorKind::from_u8(r.u8()?);
                let color = r.u8()?;
                let (dx, dy) = (r.f32()?, r.f32()?);
                if dx.is_finite() && dy.is_finite() && decor.len() < 64 {
                    decor.push(Decor {
                        kind,
                        color,
                        dx: dx.clamp(-2.0, 2.0),
                        dy: dy.clamp(-2.0, 2.0),
                    });
                }
            }
            // Skip balls with NaN/inf or absurd values (they would poison
            // the physics), and never exceed the live cap.
            let ok = [x, y, rad].iter().all(|v| v.is_finite())
                && (-0.5..=1.5).contains(&x)
                && (-0.5..=1.5).contains(&y)
                && (1.0..=2000.0).contains(&rad);
            if ok && balls.len() < crate::sim::world::MAX_BALLS {
                let mut b = Ball::new(x * self.w, y * self.h, rad * self.scale);
                b.decor = decor;
                balls.push(b);
            }
        }
        // Nearest-neighbour resample if the monitor size changed.
        let (gw, gh) = (self.grid.w, self.grid.h);
        let mut g = Grid::new(gw, gh);
        for y in 0..gh {
            let sy = y * h / gh;
            for x in 0..gw {
                let sx = x * w / gw;
                g.cells[y * gw + x] = cells[sy * w + sx];
            }
        }
        g.rebuild_meta();
        self.grid = g;
        self.balls = balls;
        self.rolling = None;
        self.windows_reapply();
        Some(())
    }
}

/// Encode named worlds into one buffer.
pub fn encode(worlds: &[(&str, &World)]) -> Vec<u8> {
    let mut v = MAGIC.to_vec();
    put_u32(&mut v, worlds.len() as u32);
    for (name, w) in worlds {
        put_u32(&mut v, name.len() as u32);
        v.extend_from_slice(name.as_bytes());
        w.encode_scene(&mut v);
    }
    v
}

/// Load a scene from someone else's computer: by monitor name when they
/// match, otherwise the first saved monitor onto the first monitor, and so
/// on. Returns how many loaded.
pub fn import(bytes: &[u8], worlds: &mut [(&str, &mut World)]) -> usize {
    let n = decode(bytes, worlds);
    if n > 0 {
        return n;
    }
    let mut r = Rd(bytes);
    if r.take(5) != Some(MAGIC) {
        return 0;
    }
    let Some(count) = r.u32() else { return 0 };
    let mut loaded = 0;
    for i in 0..count as usize {
        let Some(len) = r.u32() else { break };
        if r.take(len as usize).is_none() {
            break;
        }
        let ok = match worlds.get_mut(i) {
            Some((_, w)) => w.decode_scene(&mut r).is_some(),
            None => {
                let mut dummy = World::new(8, 8, 1.0, &crate::config::Config::default());
                dummy.decode_scene(&mut r).is_some()
            }
        };
        if !ok {
            break;
        }
        if i < worlds.len() {
            loaded += 1;
        }
    }
    loaded
}

/// Decode into the worlds with matching names; returns how many loaded.
pub fn decode(bytes: &[u8], worlds: &mut [(&str, &mut World)]) -> usize {
    let mut r = Rd(bytes);
    if r.take(5) != Some(MAGIC) {
        return 0;
    }
    let Some(n) = r.u32() else { return 0 };
    let mut loaded = 0;
    for _ in 0..n {
        let Some(len) = r.u32() else { break };
        let Some(name) = r
            .take(len as usize)
            .map(|b| String::from_utf8_lossy(b).to_string())
        else {
            break;
        };
        // Decode into a scratch world when the name does not match.
        match worlds.iter_mut().find(|(n, _)| *n == name) {
            Some((_, w)) => {
                if w.decode_scene(&mut r).is_some() {
                    loaded += 1;
                } else {
                    break;
                }
            }
            None => {
                // Skip: parse the structure without applying.
                let mut dummy = World::new(8, 8, 1.0, &crate::config::Config::default());
                if dummy.decode_scene(&mut r).is_none() {
                    break;
                }
            }
        }
    }
    loaded
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::sim::grid::{ICE, SNOW};

    #[test]
    fn scene_round_trip() {
        let cfg = Config::default();
        let mut a = World::new(200, 200, 1.0, &cfg);
        a.grid.set(5, 40, SNOW);
        a.grid.set(6, 40, ICE);
        a.balls.push(Ball::new(100.0, 100.0, 20.0));
        a.balls[0].decor.push(Decor {
            kind: DecorKind::Hat,
            color: 0,
            dx: 0.0,
            dy: -0.9,
        });
        let bytes = encode(&[("A", &a)]);
        let mut b = World::new(200, 200, 1.0, &cfg);
        let n = decode(&bytes, &mut [("A", &mut b)]);
        assert_eq!(n, 1);
        assert_eq!(b.grid.cells, a.grid.cells);
        assert_eq!(b.balls.len(), 1);
        assert_eq!(b.balls[0].decor[0].kind, DecorKind::Hat);
    }

    fn header(w: u32, h: u32) -> Vec<u8> {
        let mut v = MAGIC.to_vec();
        put_u32(&mut v, 1);
        put_u32(&mut v, 1);
        v.push(b'A');
        put_u32(&mut v, w);
        put_u32(&mut v, h);
        v
    }

    /// Regression: these used to allocate gigabytes or panic.
    #[test]
    fn hostile_headers_are_rejected() {
        let cfg = Config::default();
        for (w, h) in [(0, 0), (0, 5), (u32::MAX, u32::MAX), (70_000, 70_000)] {
            let mut b = World::new(50, 50, 1.0, &cfg);
            let mut bytes = header(w, h);
            put_u32(&mut bytes, 0);
            put_u32(&mut bytes, 0);
            assert_eq!(decode(&bytes, &mut [("A", &mut b)]), 0, "{w}x{h}");
        }
    }

    #[test]
    fn invalid_cells_and_nan_balls_are_dropped() {
        let cfg = Config::default();
        // Cell value 200 is not a valid material.
        let mut bytes = header(2, 2);
        put_u32(&mut bytes, 1);
        bytes.push(200);
        put_u32(&mut bytes, 4);
        put_u32(&mut bytes, 0);
        let mut b = World::new(50, 50, 1.0, &cfg);
        assert_eq!(decode(&bytes, &mut [("A", &mut b)]), 0);

        let mut bytes = header(2, 2);
        put_u32(&mut bytes, 1);
        bytes.push(SNOW);
        put_u32(&mut bytes, 4);
        put_u32(&mut bytes, 2);
        for v in [f32::NAN, 0.5, 10.0] {
            put_f32(&mut bytes, v);
        }
        put_u32(&mut bytes, 0);
        for v in [0.5, 0.5, 10.0] {
            put_f32(&mut bytes, v);
        }
        put_u32(&mut bytes, 0);
        let mut b = World::new(50, 50, 1.0, &cfg);
        assert_eq!(decode(&bytes, &mut [("A", &mut b)]), 1);
        assert_eq!(b.balls.len(), 1);
        assert!(b.balls[0].x.is_finite());
    }

    #[test]
    fn import_falls_back_to_monitor_order() {
        let cfg = Config::default();
        let mut a = World::new(200, 200, 1.0, &cfg);
        a.grid.set(5, 40, SNOW);
        a.balls.push(Ball::new(100.0, 100.0, 20.0));
        let bytes = encode(&[("THEIR-MONITOR", &a)]);
        let mut b = World::new(300, 300, 1.0, &cfg);
        assert_eq!(decode(&bytes, &mut [("MY-MONITOR", &mut b)]), 0);
        assert_eq!(import(&bytes, &mut [("MY-MONITOR", &mut b)]), 1);
        assert_eq!(b.balls.len(), 1);
        assert!(b.grid.count(SNOW) > 0);
    }

    #[test]
    fn garbage_is_rejected() {
        let mut b = World::new(50, 50, 1.0, &Config::default());
        assert_eq!(decode(b"nonsense", &mut [("A", &mut b)]), 0);
    }
}

/// Writes a heavy benchmark scene (mountains, snowmen, many balls) for the
/// monitors named in SNOWDESK_BENCH_MONS ("NAME:WxH,...") to the path in
/// SNOWDESK_BENCH_SCENE. Run with `cargo test --release -- --ignored`.
#[cfg(test)]
#[test]
#[ignore]
fn write_bench_scene() {
    let (Ok(path), Ok(mons)) = (
        std::env::var("SNOWDESK_BENCH_SCENE"),
        std::env::var("SNOWDESK_BENCH_MONS"),
    ) else {
        return;
    };
    let cfg = crate::config::Config::default();
    let mut worlds = Vec::new();
    for m in mons.split(',') {
        let (name, size) = m.split_once(':').unwrap();
        let (w, h) = size.split_once('x').unwrap();
        let mut world = World::new(w.parse().unwrap(), h.parse().unwrap(), 1.0, &cfg);
        world.generate_mountains();
        for i in 0..40 {
            world.stamp_snowman(world.w * (0.05 + 0.9 * i as f32 / 40.0));
        }
        while world.balls.len() < crate::sim::world::MAX_BALLS {
            let x = fastrand::f32() * world.w;
            world
                .balls
                .push(Ball::new(x, world.h * 0.3, 8.0 + fastrand::f32() * 20.0));
        }
        worlds.push((name.to_string(), world));
    }
    let list: Vec<(&str, &World)> = worlds.iter().map(|(n, w)| (n.as_str(), w)).collect();
    std::fs::write(path, encode(&list)).unwrap();
}

/// Writes a scene with an igloo and a brick wall for the monitors in
/// SNOWDESK_BENCH_MONS to SNOWDESK_BENCH_SCENE (visual checks).
#[cfg(test)]
#[test]
#[ignore]
fn write_igloo_scene() {
    let (Ok(path), Ok(mons)) = (
        std::env::var("SNOWDESK_BENCH_SCENE"),
        std::env::var("SNOWDESK_BENCH_MONS"),
    ) else {
        return;
    };
    let cfg = crate::config::Config {
        penguin_size: 1.6,
        ..crate::config::Config::default()
    };
    let mut worlds = Vec::new();
    for m in mons.split(',') {
        let (name, size) = m.split_once(':').unwrap();
        let (w, h) = size.split_once('x').unwrap();
        let mut world = World::new(w.parse().unwrap(), h.parse().unwrap(), 1.0, &cfg);
        let gh = world.grid.h as i32;
        for x in 0..world.grid.w as i32 {
            for y in gh - 8..gh {
                world.grid.set(x, y, crate::sim::grid::PACKED);
            }
        }
        world.stamp_igloo(world.w * 0.3);
        world.stamp_snowman(world.w * 0.55);
        let gh2 = world.grid.h as i32;
        for x in (world.grid.w as i32 * 3 / 4)..(world.grid.w as i32 * 3 / 4 + 40) {
            world.grid.set(x, gh2 - 9, crate::sim::grid::ICE);
        }
        for row in 0..4 {
            for i in 0..6 {
                world.place_brick(
                    world.w * 0.7 + i as f32 * 14.0,
                    world.h - 40.0 - row as f32 * 7.0,
                );
            }
        }
        worlds.push((name.to_string(), world));
    }
    let list: Vec<(&str, &World)> = worlds.iter().map(|(n, w)| (n.as_str(), w)).collect();
    std::fs::write(path, encode(&list)).unwrap();
}
