//! Sleds the user drops and pushes; they slide down slopes, glide on ice,
//! and parked sleds get borrowed by penguins for a ride.

use super::grid::{EMPTY, ICE};
use super::penguin::CritterState;
use super::world::{STEP, World};
use super::{Instance, SHAPE_RECT, premul};

pub const MAX_SLEDS: usize = 4;

#[derive(Clone, Debug)]
pub struct Sled {
    pub x: f32,
    /// Runner height (bottom), px.
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    /// Being dragged by the pointer.
    pub held: bool,
    /// A penguin is riding it.
    pub rider: bool,
    /// Seconds it has been standing still.
    pub still: f32,
}

impl World {
    /// Drop a new sled at (x, y); the oldest goes when there are too many.
    pub fn place_sled(&mut self, x: f32, y: f32) {
        if self.sleds.len() >= MAX_SLEDS {
            self.remove_sled(0);
        }
        self.sleds.push(Sled {
            x: x.clamp(20.0, self.w - 20.0),
            y,
            vx: 0.0,
            vy: 0.0,
            held: false,
            rider: false,
            still: 0.0,
        });
    }

    fn remove_sled(&mut self, i: usize) {
        self.sleds.remove(i);
        for c in &mut self.critters {
            match c.ride {
                Some(r) if r == i => {
                    c.ride = None;
                    c.state = CritterState::Air;
                    c.air_time = 0.0;
                }
                Some(r) if r > i => c.ride = Some(r - 1),
                _ => {}
            }
        }
    }

    pub fn sled_at(&self, x: f32, y: f32) -> Option<usize> {
        let s = self.scale;
        self.sleds
            .iter()
            .position(|d| (x - d.x).abs() < 20.0 * s && y > d.y - 14.0 * s && y < d.y + 4.0 * s)
    }

    pub fn drag_sled(&mut self, i: usize, x: f32, y: f32) {
        let w = self.w;
        if let Some(d) = self.sleds.get_mut(i) {
            d.held = true;
            d.x = x.clamp(20.0, w - 20.0);
            d.y = y;
            d.vx = 0.0;
            d.vy = 0.0;
        }
    }

    /// Let go with the pointer's flick: that is the push.
    pub fn push_sled(&mut self, i: usize, vx: f32) {
        let max = 900.0 * self.scale;
        if let Some(d) = self.sleds.get_mut(i) {
            d.held = false;
            d.vx = vx.clamp(-max, max);
            d.still = 0.0;
        }
    }

    pub(crate) fn step_sleds(&mut self) {
        let (cell, sc) = (self.cell, self.scale);
        for i in 0..self.sleds.len() {
            let d = self.sleds[i].clone();
            if d.held {
                continue;
            }
            let mut d = d;
            let col = |x: f32| (x / cell) as i32;
            let from = ((d.y - 8.0 * sc) / cell).floor() as i32;
            let ground = |x: f32| self.grid.surface_from(col(x), from) as f32 * cell;
            let here = ground(d.x);
            if d.y < here - 1.0 {
                // In the air: fall.
                d.vy += 1300.0 * sc * STEP;
                d.y = (d.y + d.vy * STEP).min(here);
            } else {
                d.vy = 0.0;
                d.y = here;
                // Slide downhill; ice is slippery.
                let (l, r) = (ground(d.x - 12.0 * sc), ground(d.x + 12.0 * sc));
                let slope = ((r - l) / (24.0 * sc)).clamp(-1.5, 1.5);
                let icy = self.grid.get(col(d.x), (here / cell) as i32) == ICE;
                d.vx += slope * 900.0 * sc * STEP;
                let friction = if icy { 0.12 } else { 2.0 };
                d.vx *= 1.0 - friction * STEP;
                if d.vx.abs() < 3.0 * sc {
                    d.vx = 0.0;
                }
            }
            let nx = d.x + d.vx * STEP;
            // A wall ahead (taller than the sled can ride over) stops it.
            let probe = nx + d.vx.signum() * 16.0 * sc;
            let wall_row = ((d.y - 10.0 * sc) / cell).floor() as i32;
            if d.vx != 0.0 && self.grid.get(col(probe), wall_row) != EMPTY {
                d.vx = -d.vx * 0.25;
            } else {
                d.x = nx;
            }
            if d.x < 20.0 || d.x > self.w - 20.0 {
                d.x = d.x.clamp(20.0, self.w - 20.0);
                d.vx = -d.vx * 0.3;
            }
            d.still = if d.vx.abs() < 4.0 * sc {
                d.still + STEP
            } else {
                0.0
            };
            self.sleds[i] = d;
        }
    }

    pub(crate) fn push_sleds(&self, out: &mut Vec<Instance>) {
        let s = self.scale;
        let wood = premul([0.62, 0.38, 0.2], 1.0);
        let dark = premul([0.35, 0.2, 0.1], 1.0);
        for d in &self.sleds {
            // Deck, two struts and a curled runner.
            out.push(Instance {
                pos: [d.x, d.y - 6.0 * s],
                half: [16.0 * s, 2.2 * s],
                rot: 0.0,
                shape: SHAPE_RECT,
                color: wood,
            });
            for ox in [-9.0, 9.0] {
                out.push(Instance {
                    pos: [d.x + ox * s, d.y - 3.0 * s],
                    half: [1.0 * s, 2.5 * s],
                    rot: 0.0,
                    shape: SHAPE_RECT,
                    color: dark,
                });
            }
            out.push(Instance {
                pos: [d.x, d.y - 0.8 * s],
                half: [18.0 * s, 0.9 * s],
                rot: 0.0,
                shape: SHAPE_RECT,
                color: dark,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::config::Config;
    use crate::sim::grid::{ICE, PACKED};
    use crate::sim::world::World;

    fn world() -> World {
        World::new(
            800,
            400,
            1.0,
            &Config {
                intensity: 0.0,
                ..Config::default()
            },
        )
    }

    #[test]
    fn sled_slides_downhill_and_further_on_ice() {
        let mut slope = world();
        let gh = slope.grid.h as i32;
        // Ramp falling to the right.
        for x in 0..slope.grid.w as i32 {
            for y in (40 + x / 3).min(gh - 1)..gh {
                slope.grid.set(x, y, PACKED);
            }
        }
        slope.place_sled(100.0, 0.0);
        for _ in 0..180 {
            slope.step();
        }
        assert!(
            slope.sleds[0].x > 160.0,
            "slid downhill: {}",
            slope.sleds[0].x
        );

        // Same push on snow vs ice: ice goes further.
        let dist = |mat: u8| {
            let mut w = world();
            let gh = w.grid.h as i32;
            for x in 0..w.grid.w as i32 {
                w.grid.set(x, gh - 1, mat);
            }
            w.place_sled(200.0, 0.0);
            for _ in 0..30 {
                w.step();
            }
            w.push_sled(0, 400.0);
            for _ in 0..120 {
                w.step();
            }
            w.sleds[0].x - 200.0
        };
        assert!(
            dist(ICE) > dist(PACKED) * 1.5,
            "ice {} snow {}",
            dist(ICE),
            dist(PACKED)
        );
    }

    #[test]
    fn penguins_borrow_a_parked_sled() {
        let mut w = World::new(
            800,
            400,
            1.0,
            &Config {
                intensity: 0.0,
                critter: true,
                penguins: 1,
                ..Config::default()
            },
        );
        w.step();
        w.place_sled(400.0, 0.0);
        w.critters[0].x = 380.0;
        let mut rode = false;
        for _ in 0..60 * 40 {
            w.step();
            rode |= w.critters[0].ride.is_some();
            if rode {
                break;
            }
            // Keep it nearby so the test is about hopping on.
            if (w.critters[0].x - 400.0).abs() > 60.0 {
                w.critters[0].x = 380.0;
            }
        }
        assert!(rode, "penguin never hopped on");
    }
}
