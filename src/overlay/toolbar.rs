//! In-overlay play-mode tool bar (drawn with the sprite pipeline; no text
//! rendering needed, so it works identically on every backend).

use crate::sim::balls::DecorKind;
use crate::sim::{
    Instance, SHAPE_ELLIPSE, SHAPE_GLOW, SHAPE_RECT, SHAPE_RING, SHAPE_STAR, SHAPE_TRI, premul,
};
use crate::tools::Tool;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    Tool(Tool),
    Undo,
    Redo,
    Clear,
    Snowman,
    Mountains,
    Radius(f32),
    Decor(DecorKind, u8),
    Exit,
}

pub struct Item {
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub action: Action,
}

const SCARF: [[f32; 3]; 4] = [
    [0.9, 0.2, 0.25],
    [0.3, 0.55, 0.95],
    [0.3, 0.8, 0.4],
    [0.95, 0.75, 0.2],
];

fn groups() -> Vec<Vec<Action>> {
    vec![
        vec![
            Action::Tool(Tool::Brush),
            Action::Tool(Tool::Shovel),
            Action::Tool(Tool::Pack),
            Action::Tool(Tool::Freeze),
            Action::Tool(Tool::Melt),
            Action::Tool(Tool::Snowball),
            Action::Tool(Tool::Angel),
        ],
        vec![Action::Radius(-0.2), Action::Radius(0.2)],
        vec![
            Action::Undo,
            Action::Redo,
            Action::Clear,
            Action::Snowman,
            Action::Mountains,
        ],
        vec![
            Action::Decor(DecorKind::Eye, 0),
            Action::Decor(DecorKind::Nose, 0),
            Action::Decor(DecorKind::Arm, 0),
            Action::Decor(DecorKind::Hat, 0),
            Action::Decor(DecorKind::Button, 0),
            Action::Decor(DecorKind::Scarf, 0),
            Action::Decor(DecorKind::Scarf, 1),
            Action::Decor(DecorKind::Scarf, 2),
            Action::Decor(DecorKind::Scarf, 3),
        ],
        vec![Action::Exit],
    ]
}

pub struct Layout {
    pub items: Vec<Item>,
    pub bar: (f32, f32, f32, f32), // x, y, w, h
}

pub fn layout(screen_w: f32, scale: f32) -> Layout {
    let g = groups();
    let n: usize = g.iter().map(|x| x.len()).sum();
    // Shrink the bar to fit narrow (e.g. portrait or high-scale) screens.
    let natural = (n as f32 * 40.0 - 6.0 + (g.len() - 1) as f32 * 12.0 + 16.0 + 40.0) * scale;
    let scale = scale * (screen_w / natural).min(1.0);
    let s = 34.0 * scale;
    let gap = 6.0 * scale;
    let sep = 12.0 * scale;
    let pad = 8.0 * scale;
    let total = n as f32 * (s + gap) - gap + (g.len() - 1) as f32 * sep;
    let mut x = ((screen_w - total) / 2.0).max(pad);
    #[allow(unused_mut)]
    let mut y = 14.0 * scale + pad;
    // Recording aid (debug builds): move the bar clear of a desktop panel.
    #[cfg(debug_assertions)]
    if let Some(dy) = std::env::var("SNOWDESK_TOOLBAR_DY")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
    {
        y += dy;
    }
    let mut items = Vec::new();
    for (gi, grp) in g.iter().enumerate() {
        if gi > 0 {
            x += sep - gap;
        }
        for a in grp {
            items.push(Item {
                x,
                y,
                size: s,
                action: *a,
            });
            x += s + gap;
        }
    }
    Layout {
        items,
        bar: (
            ((screen_w - total) / 2.0).max(pad) - pad,
            y - pad,
            total + 2.0 * pad,
            s + 2.0 * pad,
        ),
    }
}

impl Layout {
    pub fn contains(&self, px: f32, py: f32) -> bool {
        let (x, y, w, h) = self.bar;
        px >= x && px <= x + w && py >= y && py <= y + h
    }
    pub fn hit(&self, px: f32, py: f32) -> Option<Action> {
        self.items
            .iter()
            .find(|i| px >= i.x && px <= i.x + i.size && py >= i.y && py <= i.y + i.size)
            .map(|i| i.action)
    }
}

#[allow(clippy::too_many_arguments)]
fn q(
    out: &mut Vec<Instance>,
    cx: f32,
    cy: f32,
    hx: f32,
    hy: f32,
    rot: f32,
    shape: u32,
    rgb: [f32; 3],
    a: f32,
) {
    out.push(Instance {
        pos: [cx, cy],
        half: [hx, hy],
        rot,
        shape,
        color: premul(rgb, a),
    });
}

/// Draw an icon for `action` centred at (cx, cy) with item size `s`.
pub fn icon(out: &mut Vec<Instance>, action: Action, cx: f32, cy: f32, s: f32) {
    let w = [0.95, 0.97, 1.0];
    let u = s / 34.0;
    match action {
        Action::Tool(Tool::Brush) => {
            q(
                out,
                cx,
                cy - 4.0 * u,
                8.0 * u,
                8.0 * u,
                0.0,
                SHAPE_ELLIPSE,
                w,
                1.0,
            );
            for dx in [-6.0, 0.0, 6.0] {
                q(
                    out,
                    cx + dx * u,
                    cy + 10.0 * u,
                    1.6 * u,
                    1.6 * u,
                    0.0,
                    SHAPE_ELLIPSE,
                    w,
                    0.9,
                );
            }
        }
        Action::Tool(Tool::Shovel) => {
            q(
                out,
                cx + 3.0 * u,
                cy - 3.0 * u,
                1.6 * u,
                12.0 * u,
                0.6,
                SHAPE_RECT,
                [0.6, 0.45, 0.3],
                1.0,
            );
            q(
                out,
                cx - 5.0 * u,
                cy + 7.0 * u,
                6.0 * u,
                4.0 * u,
                0.6,
                SHAPE_RECT,
                [0.7, 0.75, 0.85],
                1.0,
            );
        }
        Action::Tool(Tool::Pack) => {
            for (dx, dy) in [(-5.0, -5.0), (5.0, -5.0), (-5.0, 5.0), (5.0, 5.0)] {
                q(
                    out,
                    cx + dx * u,
                    cy + dy * u,
                    4.2 * u,
                    4.2 * u,
                    0.0,
                    SHAPE_RECT,
                    [0.75, 0.85, 0.98],
                    1.0,
                );
            }
        }
        Action::Tool(Tool::Freeze) => q(
            out,
            cx,
            cy,
            11.0 * u,
            11.0 * u,
            0.0,
            SHAPE_STAR,
            [0.5, 0.85, 1.0],
            1.0,
        ),
        Action::Tool(Tool::Melt) => {
            q(
                out,
                cx,
                cy - 5.0 * u,
                10.0 * u,
                10.0 * u,
                0.0,
                SHAPE_GLOW,
                [1.0, 0.7, 0.2],
                1.0,
            );
            q(
                out,
                cx,
                cy - 5.0 * u,
                4.0 * u,
                4.0 * u,
                0.0,
                SHAPE_ELLIPSE,
                [1.0, 0.85, 0.3],
                1.0,
            );
            q(
                out,
                cx,
                cy + 8.0 * u,
                2.4 * u,
                4.0 * u,
                0.0,
                SHAPE_ELLIPSE,
                [0.5, 0.75, 1.0],
                1.0,
            );
        }
        Action::Tool(Tool::Snowball) => {
            q(
                out,
                cx,
                cy,
                10.0 * u,
                10.0 * u,
                0.0,
                SHAPE_ELLIPSE,
                [0.62, 0.7, 0.85],
                1.0,
            );
            q(
                out,
                cx - 1.0 * u,
                cy - 1.0 * u,
                9.0 * u,
                9.0 * u,
                0.0,
                SHAPE_ELLIPSE,
                w,
                1.0,
            );
        }
        Action::Tool(Tool::Angel) => {
            q(
                out,
                cx,
                cy + 2.0 * u,
                3.0 * u,
                7.0 * u,
                0.0,
                SHAPE_ELLIPSE,
                w,
                1.0,
            );
            q(
                out,
                cx,
                cy - 8.0 * u,
                3.2 * u,
                3.2 * u,
                0.0,
                SHAPE_ELLIPSE,
                w,
                1.0,
            );
            q(
                out,
                cx - 8.0 * u,
                cy - 1.0 * u,
                6.0 * u,
                1.8 * u,
                0.5,
                SHAPE_RECT,
                w,
                1.0,
            );
            q(
                out,
                cx + 8.0 * u,
                cy - 1.0 * u,
                6.0 * u,
                1.8 * u,
                -0.5,
                SHAPE_RECT,
                w,
                1.0,
            );
        }
        Action::Undo => q(
            out,
            cx,
            cy,
            9.0 * u,
            7.0 * u,
            std::f32::consts::PI,
            SHAPE_TRI,
            [0.48, 0.64, 0.97],
            1.0,
        ),
        Action::Redo => q(
            out,
            cx,
            cy,
            9.0 * u,
            7.0 * u,
            0.0,
            SHAPE_TRI,
            [0.48, 0.64, 0.97],
            1.0,
        ),
        Action::Clear => {
            let r = [0.97, 0.46, 0.56];
            q(out, cx, cy, 11.0 * u, 1.8 * u, 0.785, SHAPE_RECT, r, 1.0);
            q(out, cx, cy, 11.0 * u, 1.8 * u, -0.785, SHAPE_RECT, r, 1.0);
        }
        Action::Snowman => {
            q(
                out,
                cx,
                cy + 7.0 * u,
                7.5 * u,
                7.5 * u,
                0.0,
                SHAPE_ELLIPSE,
                w,
                1.0,
            );
            q(
                out,
                cx,
                cy - 3.0 * u,
                5.5 * u,
                5.5 * u,
                0.0,
                SHAPE_ELLIPSE,
                w,
                1.0,
            );
            q(
                out,
                cx,
                cy - 11.0 * u,
                3.8 * u,
                3.8 * u,
                0.0,
                SHAPE_ELLIPSE,
                w,
                1.0,
            );
        }
        Action::Mountains => {
            q(
                out,
                cx - 4.0 * u,
                cy,
                9.0 * u,
                9.0 * u,
                -std::f32::consts::FRAC_PI_2,
                SHAPE_TRI,
                [0.55, 0.62, 0.8],
                1.0,
            );
            q(
                out,
                cx + 6.0 * u,
                cy + 3.0 * u,
                6.0 * u,
                6.0 * u,
                -std::f32::consts::FRAC_PI_2,
                SHAPE_TRI,
                w,
                1.0,
            );
        }
        Action::Radius(d) => {
            let r = if d < 0.0 { 5.0 } else { 11.0 };
            q(out, cx, cy, r * u, r * u, 0.0, SHAPE_RING, w, 1.0);
        }
        Action::Decor(k, c) => match k {
            DecorKind::Eye => q(
                out,
                cx,
                cy,
                5.0 * u,
                5.0 * u,
                0.0,
                SHAPE_ELLIPSE,
                [0.02, 0.02, 0.04],
                1.0,
            ),
            DecorKind::Button => q(
                out,
                cx,
                cy,
                4.0 * u,
                4.0 * u,
                0.0,
                SHAPE_ELLIPSE,
                [0.3, 0.3, 0.4],
                1.0,
            ),
            DecorKind::Nose => q(
                out,
                cx,
                cy,
                10.0 * u,
                3.5 * u,
                0.0,
                SHAPE_TRI,
                [1.0, 0.5, 0.1],
                1.0,
            ),
            DecorKind::Arm => q(
                out,
                cx,
                cy,
                11.0 * u,
                1.4 * u,
                -0.5,
                SHAPE_RECT,
                [0.55, 0.38, 0.22],
                1.0,
            ),
            DecorKind::Hat => {
                q(
                    out,
                    cx,
                    cy + 5.0 * u,
                    10.0 * u,
                    1.8 * u,
                    0.0,
                    SHAPE_RECT,
                    [0.02, 0.02, 0.04],
                    1.0,
                );
                q(
                    out,
                    cx,
                    cy - 2.0 * u,
                    6.0 * u,
                    6.5 * u,
                    0.0,
                    SHAPE_RECT,
                    [0.02, 0.02, 0.04],
                    1.0,
                );
            }
            DecorKind::Scarf => q(
                out,
                cx,
                cy,
                10.0 * u,
                3.5 * u,
                0.0,
                SHAPE_RECT,
                SCARF[c as usize % 4],
                1.0,
            ),
        },
        Action::Exit => {
            q(
                out,
                cx,
                cy,
                6.0 * u,
                9.0 * u,
                0.0,
                SHAPE_RECT,
                [0.97, 0.46, 0.56],
                1.0,
            );
            q(
                out,
                cx + 2.0 * u,
                cy,
                1.4 * u,
                1.4 * u,
                0.0,
                SHAPE_ELLIPSE,
                [1.0, 1.0, 1.0],
                1.0,
            );
        }
    }
}

pub fn draw(out: &mut Vec<Instance>, l: &Layout, selected: Tool, radius_scale: f32, scale: f32) {
    let (x, y, w, h) = l.bar;
    // Panel (#24283b) with accent edge.
    q(
        out,
        x + w / 2.0,
        y + h / 2.0,
        w / 2.0 + 1.5 * scale,
        h / 2.0 + 1.5 * scale,
        0.0,
        SHAPE_RECT,
        [0.2, 0.25, 0.4],
        0.9,
    );
    q(
        out,
        x + w / 2.0,
        y + h / 2.0,
        w / 2.0,
        h / 2.0,
        0.0,
        SHAPE_RECT,
        [0.141, 0.157, 0.231],
        0.94,
    );
    for it in &l.items {
        let (cx, cy) = (it.x + it.size / 2.0, it.y + it.size / 2.0);
        let sel = matches!(it.action, Action::Tool(t) if t == selected);
        let bg = if sel {
            [0.478, 0.635, 0.969]
        } else {
            [0.102, 0.106, 0.149]
        };
        q(
            out,
            cx,
            cy,
            it.size / 2.0,
            it.size / 2.0,
            0.0,
            SHAPE_RECT,
            bg,
            if sel { 0.55 } else { 1.0 },
        );
        icon(out, it.action, cx, cy, it.size);
    }
    // radius readout: ring sized by current radius
    let ring = (6.0 + 10.0 * radius_scale.clamp(0.0, 1.0)) * scale;
    let last = l
        .items
        .iter()
        .find(|i| matches!(i.action, Action::Radius(d) if d > 0.0));
    if let Some(i) = last {
        q(
            out,
            i.x + i.size + 10.0 * scale,
            i.y + i.size / 2.0,
            ring,
            ring,
            0.0,
            SHAPE_RING,
            [0.48, 0.64, 0.97],
            1.0,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_hits_items() {
        let l = layout(1920.0, 1.0);
        let first = &l.items[0];
        assert_eq!(
            l.hit(first.x + 2.0, first.y + 2.0),
            Some(Action::Tool(Tool::Brush))
        );
        assert!(l.contains(first.x, first.y));
        assert_eq!(l.hit(0.0, 900.0), None);
    }

    #[test]
    fn layout_fits_narrow_screens() {
        for (w, sc) in [(1080.0, 1.0), (1920.0, 2.0), (800.0, 1.5)] {
            let l = layout(w, sc);
            let (x, _, bw, _) = l.bar;
            assert!(x >= 0.0 && x + bw <= w + 1.0, "{w} {sc}: {:?}", l.bar);
        }
    }
}
