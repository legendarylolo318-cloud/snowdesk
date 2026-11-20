//! Scripted play-mode demo used to record the README GIF (debug builds
//! only): `SNOWDESK_DEMO=<monitor name>` drives the real engine with
//! synthetic pointer events and draws a cursor.

use super::{Button, Engine, Ptr, toolbar};
use crate::sim::balls::DecorKind;
use crate::sim::grid::PACKED;
use crate::sim::{Instance, SHAPE_TRI, premul};
use crate::tools::Tool;
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
enum Target {
    /// Fractions of the monitor size.
    Frac(f32, f32),
    /// Relative to ball `n` counted from the newest (0 = newest): offsets in
    /// units of its radius.
    Ball(usize, f32, f32),
    /// Above ball `n` from the newest by `k` radii of the newest ball.
    Above(usize, f32),
    Toolbar(toolbar::Action),
    Penguin,
}

#[derive(Clone, Copy)]
enum Step {
    Move(Target, f32),
    Down(Button),
    Up(Button),
    Tool(Tool),
    Wait(f32),
}

use Button::{Left as L, Right as R};
use Step::*;
use Target::*;

fn script() -> Vec<Step> {
    vec![
        Wait(0.6),
        Move(Toolbar(toolbar::Action::Tool(Tool::Snowball)), 0.6),
        Down(L),
        Up(L),
        Tool(Tool::Snowball),
        // Base ball: roll it along the snow.
        Move(Frac(0.26, 0.94), 0.5),
        Down(L),
        Move(Frac(0.53, 0.94), 2.6),
        Up(L),
        // Middle ball.
        Move(Frac(0.68, 0.94), 0.5),
        Down(L),
        Move(Frac(0.73, 0.94), 1.0),
        // Lift it and drop it on the base.
        Move(Above(1, 1.6), 0.9),
        Up(L),
        Wait(0.5),
        // Head.
        Move(Frac(0.75, 0.94), 0.5),
        Down(L),
        Move(Frac(0.775, 0.94), 0.6),
        Move(Above(1, 1.6), 0.9),
        Up(L),
        Wait(0.9),
        // Decorate: hat, eyes, nose, scarf, arms.
        Move(Toolbar(toolbar::Action::Decor(DecorKind::Hat, 0)), 0.6),
        Down(L),
        Move(Ball(0, 0.0, -0.9), 0.7),
        Up(L),
        Move(Toolbar(toolbar::Action::Decor(DecorKind::Eye, 0)), 0.6),
        Down(L),
        Move(Ball(0, -0.32, -0.25), 0.6),
        Up(L),
        Move(Toolbar(toolbar::Action::Decor(DecorKind::Eye, 0)), 0.5),
        Down(L),
        Move(Ball(0, 0.32, -0.25), 0.6),
        Up(L),
        Move(Toolbar(toolbar::Action::Decor(DecorKind::Nose, 0)), 0.5),
        Down(L),
        Move(Ball(0, 0.2, 0.1), 0.6),
        Up(L),
        Move(Toolbar(toolbar::Action::Decor(DecorKind::Scarf, 0)), 0.5),
        Down(L),
        Move(Ball(1, 0.0, -0.8), 0.6),
        Up(L),
        Move(Toolbar(toolbar::Action::Decor(DecorKind::Arm, 0)), 0.5),
        Down(L),
        Move(Ball(1, 0.9, -0.2), 0.6),
        Up(L),
        Move(Toolbar(toolbar::Action::Decor(DecorKind::Arm, 0)), 0.5),
        Down(L),
        Move(Ball(1, -0.9, -0.2), 0.6),
        Up(L),
        Wait(0.6),
        // Poke the penguin, then carry it.
        Move(Penguin, 0.7),
        Down(L),
        Up(L),
        Wait(1.4),
        Move(Penguin, 0.6),
        Down(L),
        Move(Frac(0.35, 0.40), 0.9),
        Move(Frac(0.30, 0.45), 0.25),
        Up(L),
        Wait(1.2),
        // Snowball at the penguin.
        Move(Frac(0.75, 0.45), 0.6),
        Down(R),
        Move(Penguin, 0.12),
        Up(R),
        Wait(2.2),
    ]
}

pub struct Demo {
    mon: String,
    steps: Vec<Step>,
    i: usize,
    started: Instant,
    step_start: Instant,
    from: (f32, f32),
    to: Option<(f32, f32)>,
    pub cursor: (f32, f32),
}

impl Demo {
    pub fn from_env() -> Option<Self> {
        let mon = std::env::var("SNOWDESK_DEMO").ok()?;
        let now = Instant::now();
        Some(Self {
            mon,
            steps: script(),
            i: 0,
            started: now,
            step_start: now,
            from: (0.0, 0.0),
            to: None,
            cursor: (0.0, 0.0),
        })
    }

    pub fn monitor(&self) -> &str {
        &self.mon
    }
}

impl Engine {
    /// Advance the demo script; called once per drive.
    pub(super) fn demo_tick(&mut self) {
        let Some(mut d) = self.demo.take() else {
            return;
        };
        let Some(mi) = self.mons.iter().position(|m| m.spec.name == d.mon) else {
            self.demo = Some(d);
            return;
        };
        let now = Instant::now();
        if !self.play {
            if now - d.started < Duration::from_secs(2) {
                self.demo = Some(d);
                return;
            }
            // Fresh scene: a bed of packed snow to roll in, one penguin.
            let w = &mut self.mons[mi].world;
            w.clear_all();
            let gh = w.grid.h as i32;
            let depth = (gh as f32 * 0.05) as i32;
            for x in 0..w.grid.w as i32 {
                for y in gh - depth..gh {
                    w.grid.set(x, y, PACKED);
                }
            }
            // Put the penguin on the ground near where the snowman goes.
            let gx = w.w * 0.60;
            let gy = w.surface_px(gx);
            if let Some(c) = w.critters.first_mut() {
                c.x = gx;
                c.y = gy;
                c.dir = -1.0;
            }
            self.set_play(true);
            let (mw, mh) = (self.mons[mi].spec.w as f32, self.mons[mi].spec.h as f32);
            d.cursor = (mw * 0.5, mh * 0.5);
            d.from = d.cursor;
            d.step_start = now;
        }
        while let Some(&step) = d.steps.get(d.i) {
            let t = (now - d.step_start).as_secs_f32();
            let name = d.mon.clone();
            let done = match step {
                Move(target, dur) => {
                    let to = *d
                        .to
                        .get_or_insert_with(|| self.demo_resolve(mi, target, d.cursor));
                    let k = (t / dur).clamp(0.0, 1.0);
                    let e = k * k * (3.0 - 2.0 * k);
                    d.cursor = (
                        d.from.0 + (to.0 - d.from.0) * e,
                        d.from.1 + (to.1 - d.from.1) * e,
                    );
                    self.pointer(&name, Ptr::Move(d.cursor.0, d.cursor.1));
                    k >= 1.0
                }
                Down(b) => {
                    self.pointer(&name, Ptr::Down(b, d.cursor.0, d.cursor.1));
                    true
                }
                Up(b) => {
                    self.pointer(&name, Ptr::Up(b, d.cursor.0, d.cursor.1));
                    true
                }
                Tool(t) => {
                    self.tool = t;
                    true
                }
                Wait(s) => t >= s,
            };
            if !done {
                break;
            }
            d.i += 1;
            d.step_start = now;
            d.from = d.cursor;
            d.to = None;
        }
        if d.i >= d.steps.len() {
            tracing::info!("demo finished");
            self.set_play(false);
            return;
        }
        self.demo = Some(d);
    }

    fn demo_resolve(&self, mi: usize, t: Target, cur: (f32, f32)) -> (f32, f32) {
        let m = &self.mons[mi];
        let (w, h) = (m.spec.w as f32, m.spec.h as f32);
        let ball = |n: usize| m.world.balls.iter().rev().filter(|b| !b.thrown).nth(n);
        match t {
            Frac(x, y) => (x * w, y * h),
            Ball(n, dx, dy) => ball(n)
                .map(|b| (b.x + dx * b.r, b.y + dy * b.r))
                .unwrap_or(cur),
            Above(n, k) => match (ball(0), ball(n)) {
                (Some(top), Some(b)) => (b.x, b.y - b.r - top.r * k),
                _ => cur,
            },
            Toolbar(a) => m
                .toolbar
                .items
                .iter()
                .find(|i| i.action == a)
                .map(|i| (i.x + i.size / 2.0, i.y + i.size / 2.0))
                .unwrap_or(cur),
            Penguin => m
                .world
                .critters
                .first()
                .map(|c| {
                    let k = m.world.penguin_scale(c);
                    (c.x, c.y - 11.0 * k)
                })
                .unwrap_or(cur),
        }
    }

    /// A visible mouse cursor for the recording.
    pub(super) fn demo_cursor(out: &mut Vec<Instance>, x: f32, y: f32, s: f32) {
        let rot = std::f32::consts::PI * 1.25;
        let (dx, dy) = (rot.cos(), rot.sin());
        for (half, col, a) in [(11.0, [0.05, 0.05, 0.08], 1.0), (8.5, [1.0, 1.0, 1.0], 1.0)] {
            out.push(Instance {
                pos: [x - dx * half * s * 0.9, y - dy * half * s * 0.9],
                half: [half * s, half * 0.62 * s],
                rot,
                shape: SHAPE_TRI,
                color: premul(col, a),
            });
        }
    }
}
