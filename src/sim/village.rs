//! Penguin village life: at night penguins go home to an igloo and sleep
//! (warm light in the door), in the morning they come out. In the day, holes
//! cut into an ice sheet become fishing holes; penguins queue up and fish.

use super::grid::{EMPTY, ICE, WATER};
use super::penguin::{Critter, CritterState};
use super::world::{STEP, World};
use super::{Instance, SHAPE_ELLIPSE, SHAPE_GLOW, SHAPE_RECT, SHAPE_TRI, premul};

#[derive(Clone, Debug)]
pub struct Igloo {
    /// Centre x and floor y (px).
    pub x: f32,
    pub floor: f32,
    /// Side of the door (-1 or 1).
    pub dir: f32,
    pub r_in: f32,
    pub r_out: f32,
    pub door_h: f32,
    pub tunnel: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Hole {
    /// Centre x and ice surface y (px), opening width.
    pub x: f32,
    pub y: f32,
    pub w: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Goal {
    #[default]
    None,
    Home(usize),
    Fish(usize),
}

impl World {
    /// Openings in a sheet of ice (both sides ice, open sky above).
    pub(crate) fn scan_holes(&self) -> Vec<Hole> {
        let g = &self.grid;
        let cell = self.cell;
        let mut out: Vec<Hole> = Vec::new();
        let surface_ice = |x: i32, y: i32| g.get(x, y) == ICE && g.get(x, y - 1) == EMPTY;
        for y in 1..g.h as i32 {
            let mut last: Option<i32> = None;
            for x in 0..g.w as i32 {
                if !surface_ice(x, y) {
                    continue;
                }
                if let Some(l) = last {
                    let gap = x - l - 1;
                    let open = (l + 1..x).all(|c| matches!(g.get(c, y), EMPTY | WATER));
                    if (1..=24).contains(&gap) && open {
                        let hx = (l + 1 + x) as f32 * 0.5 * cell;
                        if !out.iter().any(|h| (h.x - hx).abs() < 30.0 * self.scale) {
                            out.push(Hole {
                                x: hx,
                                y: y as f32 * cell,
                                w: gap as f32 * cell,
                            });
                        }
                    }
                }
                last = Some(x);
            }
        }
        out
    }

    /// Village behaviour for one penguin. Some(keep) if it handled the step.
    pub(crate) fn village_step(&mut self, c: &mut Critter, k: f32) -> Option<bool> {
        match c.state {
            CritterState::Sleep => {
                if !self.night || !matches!(c.goal, Goal::Home(i) if i < self.igloos.len()) {
                    // Morning (or the igloo is gone): come out of the door.
                    if let Goal::Home(i) = c.goal
                        && let Some(ig) = self.igloos.get(i)
                    {
                        c.x = ig.x + ig.dir * ig.r_in * 0.4;
                        c.y = ig.floor;
                        c.dir = ig.dir;
                    }
                    c.state = CritterState::Walk;
                    c.t = 4.0;
                    c.goal = Goal::None;
                }
                return Some(true);
            }
            CritterState::Fish => {
                c.fish_t -= STEP;
                c.catch_t = (c.catch_t - STEP).max(0.0);
                let gone = !matches!(c.goal, Goal::Fish(i) if i < self.holes.len());
                if gone || c.fish_t <= 0.0 || self.night {
                    c.state = CritterState::Walk;
                    c.t = 3.0;
                    c.dir = -c.dir;
                    c.goal = Goal::None;
                } else if c.catch_t <= 0.0 && fastrand::u16(..) < 110 {
                    c.catch_t = 1.0; // a fish!
                }
                return Some(true);
            }
            CritterState::Idle | CritterState::Walk => {}
            _ => return None,
        }
        if c.anger > 0.0 {
            return None;
        }
        // Choose a goal.
        if self.night && !self.igloos.is_empty() {
            if !matches!(c.goal, Goal::Home(i) if i < self.igloos.len()) {
                let i = (0..self.igloos.len())
                    .min_by(|&a, &b| {
                        (self.igloos[a].x - c.x)
                            .abs()
                            .total_cmp(&(self.igloos[b].x - c.x).abs())
                    })
                    .unwrap_or(0);
                c.goal = Goal::Home(i);
            }
        } else if matches!(c.goal, Goal::Home(_)) {
            c.goal = Goal::None;
        }
        if c.goal == Goal::None
            && !self.night
            && !self.holes.is_empty()
            && fastrand::u16(..) < 40
            && let Some(i) = (0..self.holes.len())
                .filter(|&i| (self.holes[i].x - c.x).abs() < 700.0 * self.scale)
                .min_by(|&a, &b| {
                    (self.holes[a].x - c.x)
                        .abs()
                        .total_cmp(&(self.holes[b].x - c.x).abs())
                })
        {
            c.goal = Goal::Fish(i);
        }
        if matches!(c.goal, Goal::Fish(i) if i >= self.holes.len()) {
            c.goal = Goal::None;
        }
        // Steer toward it.
        let target = match c.goal {
            Goal::None => return None,
            Goal::Home(i) => {
                let ig = &self.igloos[i];
                let inside = (c.x - ig.x).abs() < ig.r_in * 0.5 && (c.y - ig.floor).abs() < 8.0 * k;
                if inside {
                    c.state = CritterState::Sleep;
                    c.vx = 0.0;
                    return Some(true);
                }
                // At floor level on the door side (outside the door or in the
                // tunnel): walk in. Anywhere else, including up on the dome or
                // the tunnel roof: go round to the door first.
                let at_floor = (c.y - ig.floor).abs() < 8.0 * k;
                let door_side = (c.x - ig.x) * ig.dir > ig.r_in * 0.3;
                if at_floor && door_side {
                    ig.x
                } else {
                    ig.x + ig.dir * (ig.r_out + ig.tunnel + 12.0 * k)
                }
            }
            Goal::Fish(i) => {
                let h = &self.holes[i];
                let side = if c.x >= h.x { 1.0 } else { -1.0 };
                // Queue up: one spot per penguin already fishing this side.
                let slot = self
                    .critters
                    .iter()
                    .filter(|o| {
                        o.state == CritterState::Fish
                            && o.goal == Goal::Fish(i)
                            && (o.x >= h.x) == (side > 0.0)
                    })
                    .count() as f32;
                let stand = h.x + side * (h.w * 0.5 + 9.0 * k + slot * 22.0 * k);
                if (c.x - stand).abs() < 4.0 * k {
                    c.state = CritterState::Fish;
                    c.fish_t = 8.0 + fastrand::f32() * 14.0;
                    c.catch_t = 0.0;
                    c.dir = -side;
                    c.vx = 0.0;
                    c.x = stand;
                    return Some(true);
                }
                stand
            }
        };
        c.dir = if target >= c.x { 1.0 } else { -1.0 };
        if c.state == CritterState::Idle {
            c.state = CritterState::Walk;
        }
        c.t = c.t.max(1.0);
        None
    }

    /// Re-check igloos (undo or shovels may have removed one) and holes.
    pub(crate) fn refresh_village(&mut self) {
        let cell = self.cell;
        let before = self.igloos.len();
        self.igloos.retain(|ig| {
            let col = (ig.x / cell) as i32;
            let roof = ((ig.floor - ig.r_out * 0.95) / cell) as i32;
            (roof - 2..roof + 3).any(|r| self.grid.get(col, r) != EMPTY)
        });
        if self.igloos.len() != before {
            for c in &mut self.critters {
                if matches!(c.goal, Goal::Home(_)) {
                    c.goal = Goal::None;
                }
            }
        }
        let holes = self.scan_holes();
        if holes != self.holes {
            for c in &mut self.critters {
                if matches!(c.goal, Goal::Fish(_)) && c.state != CritterState::Fish {
                    c.goal = Goal::None;
                }
            }
            self.holes = holes;
        }
    }

    /// Warm light in igloo doors with someone asleep inside; fishing lines.
    pub(crate) fn push_village(&self, out: &mut Vec<Instance>) {
        for (i, ig) in self.igloos.iter().enumerate() {
            let home = self
                .critters
                .iter()
                .any(|c| c.state == CritterState::Sleep && c.goal == Goal::Home(i));
            if home {
                let flicker = 0.85 + 0.15 * (self.time * 7.0 + i as f32).sin();
                out.push(Instance {
                    pos: [
                        ig.x + ig.dir * (ig.r_out - 4.0 * self.scale),
                        ig.floor - ig.door_h * 0.45,
                    ],
                    half: [ig.door_h * 0.8; 2],
                    rot: 0.0,
                    shape: SHAPE_GLOW,
                    color: premul([1.0, 0.68, 0.3], 0.35 * flicker),
                });
            }
        }
        for c in self
            .critters
            .iter()
            .filter(|c| c.state == CritterState::Fish)
        {
            let Goal::Fish(i) = c.goal else { continue };
            let Some(h) = self.holes.get(i) else { continue };
            let k = self.penguin_scale(c);
            let (tx, ty) = (c.x + c.dir * 8.0 * k, c.y - 18.0 * k);
            let bob = (self.time * 3.0 + c.x).sin() * 1.2 * self.scale;
            let (hx, hy) = (h.x, h.y + 3.0 * self.scale + bob);
            // Rod and line.
            let (dx, dy) = (hx - tx, hy - ty);
            out.push(Instance {
                pos: [(tx + hx) * 0.5, (ty + hy) * 0.5],
                half: [dx.hypot(dy) * 0.5, 0.5 * self.scale],
                rot: dy.atan2(dx),
                shape: SHAPE_RECT,
                color: premul([0.85, 0.85, 0.9], 0.8),
            });
            out.push(Instance {
                pos: [hx, hy],
                half: [2.0 * self.scale; 2],
                rot: 0.0,
                shape: SHAPE_ELLIPSE,
                color: premul([0.95, 0.25, 0.2], 1.0),
            });
            if c.catch_t > 0.0 {
                // A fish arcs out of the hole into the penguin's beak.
                let t = 1.0 - c.catch_t;
                let fx = hx + (c.x - hx) * t;
                let fy =
                    hy - (t * (1.0 - t) * 4.0) * 40.0 * self.scale - (hy - (c.y - 20.0 * k)) * t;
                let s = self.scale;
                out.push(Instance {
                    pos: [fx, fy],
                    half: [5.0 * s, 2.2 * s],
                    rot: t * 6.0,
                    shape: SHAPE_ELLIPSE,
                    color: premul([0.95, 0.55, 0.2], 1.0),
                });
                out.push(Instance {
                    pos: [
                        fx - 5.0 * s * (t * 6.0).cos(),
                        fy - 5.0 * s * (t * 6.0).sin(),
                    ],
                    half: [2.5 * s, 2.5 * s],
                    rot: t * 6.0 + std::f32::consts::PI,
                    shape: SHAPE_TRI,
                    color: premul([0.95, 0.55, 0.2], 1.0),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::sim::grid::{ICE, PACKED};

    fn world() -> World {
        let cfg = Config {
            intensity: 0.0,
            critter: true,
            penguins: 1,
            ..Config::default()
        };
        let mut w = World::new(900, 400, 1.0, &cfg);
        w.step();
        w
    }

    #[test]
    fn penguins_sleep_in_the_igloo_at_night_and_come_out_in_the_morning() {
        let mut w = world();
        w.stamp_igloo(450.0);
        assert_eq!(w.igloos.len(), 1);
        w.night = true;
        w.critters[0].x = 100.0;
        let mut slept = false;
        for _ in 0..60 * 90 {
            w.step();
            if w.critters[0].state == CritterState::Sleep {
                slept = true;
                break;
            }
        }
        assert!(
            slept,
            "never got home: {:?}",
            (w.critters[0].x, w.critters[0].y, w.critters[0].state)
        );
        let mut out = Vec::new();
        w.push_village(&mut out);
        assert!(!out.is_empty(), "door glows");
        w.night = false;
        w.step();
        assert_eq!(w.critters[0].state, CritterState::Walk, "woke up");
    }

    #[test]
    fn a_hole_in_the_ice_becomes_a_fishing_spot() {
        let mut w = world();
        let gh = w.grid.h as i32;
        for x in 0..w.grid.w as i32 {
            for y in gh - 6..gh {
                w.grid.set(x, y, PACKED);
            }
            w.grid.set(x, gh - 7, ICE);
        }
        // Cut a hole.
        for x in 110..116 {
            for y in gh - 7..gh - 2 {
                w.grid.set(x, y, crate::sim::grid::EMPTY);
            }
        }
        w.refresh_village();
        assert_eq!(w.holes.len(), 1, "{:?}", w.holes);
        w.critters[0].x = 300.0;
        let mut fished = false;
        for _ in 0..60 * 120 {
            w.step();
            if w.critters[0].state == CritterState::Fish {
                fished = true;
                break;
            }
        }
        assert!(fished, "never went fishing");
        let h = &w.holes[0];
        assert!((w.critters[0].x - h.x).abs() < 60.0, "fishing at the hole");
    }
}
