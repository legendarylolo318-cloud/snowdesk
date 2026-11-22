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
    Igloo,
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
            Action::Tool(Tool::Brick),
            Action::Tool(Tool::Sled),
        ],
        vec![Action::Radius(-0.2), Action::Radius(0.2)],
        vec![
            Action::Undo,
            Action::Redo,
            Action::Clear,
            Action::Snowman,
            Action::Igloo,
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
    pub screen_w: f32,
    /// Scale after shrinking to fit (for the tooltip text).
    pub scale: f32,
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
        screen_w,
        scale,
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
        Action::Tool(Tool::Brick) => {
            let b = [0.84, 0.89, 0.98];
            for (ox, oy) in [(-5.0, 5.0), (5.0, 5.0), (0.0, -1.5)] {
                q(
                    out,
                    cx + ox * u,
                    cy + oy * u,
                    4.6 * u,
                    2.8 * u,
                    0.0,
                    SHAPE_RECT,
                    b,
                    1.0,
                );
            }
        }
        Action::Tool(Tool::Sled) => {
            let wood = [0.62, 0.38, 0.2];
            q(
                out,
                cx,
                cy + 2.0 * u,
                11.0 * u,
                2.2 * u,
                0.0,
                SHAPE_RECT,
                wood,
                1.0,
            );
            q(
                out,
                cx - 6.0 * u,
                cy + 5.5 * u,
                1.0 * u,
                2.4 * u,
                0.0,
                SHAPE_RECT,
                wood,
                1.0,
            );
            q(
                out,
                cx + 6.0 * u,
                cy + 5.5 * u,
                1.0 * u,
                2.4 * u,
                0.0,
                SHAPE_RECT,
                wood,
                1.0,
            );
            q(
                out,
                cx,
                cy + 8.5 * u,
                13.0 * u,
                1.0 * u,
                0.0,
                SHAPE_RECT,
                [0.35, 0.2, 0.1],
                1.0,
            );
        }
        Action::Igloo => {
            q(
                out,
                cx,
                cy + 3.0 * u,
                11.0 * u,
                9.0 * u,
                0.0,
                SHAPE_ELLIPSE,
                [0.84, 0.89, 0.98],
                1.0,
            );
            q(
                out,
                cx,
                cy + 11.0 * u,
                12.0 * u,
                3.0 * u,
                0.0,
                SHAPE_RECT,
                [0.141, 0.157, 0.231],
                1.0,
            );
            q(
                out,
                cx + 5.0 * u,
                cy + 5.5 * u,
                2.6 * u,
                3.0 * u,
                0.0,
                SHAPE_ELLIPSE,
                [0.1, 0.1, 0.16],
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

/// Tooltip text for a tool bar button (upper case: the pixel font has no
/// lower case). Tools show their number key.
pub fn label(a: Action) -> &'static str {
    match a {
        Action::Tool(Tool::Brush) => "BRUSH: ADD SNOW  (1)",
        Action::Tool(Tool::Shovel) => "SHOVEL: DIG SNOW AWAY  (2)",
        Action::Tool(Tool::Pack) => "PACK SNOW HARD  (3)",
        Action::Tool(Tool::Freeze) => "FREEZE INTO ICE  (4)",
        Action::Tool(Tool::Melt) => "MELT  (5)",
        Action::Tool(Tool::Snowball) => "SNOWBALL: DRAG ALONG SNOW TO ROLL  (6)",
        Action::Tool(Tool::Angel) => "SNOW ANGEL  (7)",
        Action::Tool(Tool::Brick) => "BRICKS: BUILD A FORT  (8)",
        Action::Tool(Tool::Sled) => "SLED: CLICK TO DROP, FLICK TO PUSH  (9)",
        Action::Radius(d) if d < 0.0 => "SMALLER BRUSH  (SCROLL DOWN)",
        Action::Radius(_) => "BIGGER BRUSH  (SCROLL UP)",
        Action::Undo => "UNDO",
        Action::Redo => "REDO",
        Action::Clear => "CLEAR ALL SNOW",
        Action::Snowman => "STAMP A SNOWMAN",
        Action::Igloo => "BUILD AN IGLOO",
        Action::Mountains => "MAKE MOUNTAINS",
        Action::Decor(DecorKind::Eye, _) => "EYES: DRAG ONTO A SNOWBALL",
        Action::Decor(DecorKind::Nose, _) => "CARROT NOSE: DRAG ONTO A SNOWBALL",
        Action::Decor(DecorKind::Arm, _) => "STICK ARM: DRAG ONTO A SNOWBALL",
        Action::Decor(DecorKind::Hat, _) => "HAT: DRAG ONTO A SNOWBALL",
        Action::Decor(DecorKind::Button, _) => "BUTTON: DRAG ONTO A SNOWBALL",
        Action::Decor(DecorKind::Scarf, _) => "SCARF: DRAG ONTO A SNOWBALL",
        Action::Exit => "BACK TO YOUR DESKTOP  (ESC)",
    }
}

/// 5x7 pixel font: one byte per row, bit 4 = leftmost column.
fn glyph(c: char) -> [u8; 7] {
    match c {
        'A' => [14, 17, 17, 31, 17, 17, 17],
        'B' => [30, 17, 17, 30, 17, 17, 30],
        'C' => [14, 17, 16, 16, 16, 17, 14],
        'D' => [30, 17, 17, 17, 17, 17, 30],
        'E' => [31, 16, 16, 30, 16, 16, 31],
        'F' => [31, 16, 16, 30, 16, 16, 16],
        'G' => [14, 17, 16, 23, 17, 17, 15],
        'H' => [17, 17, 17, 31, 17, 17, 17],
        'I' => [14, 4, 4, 4, 4, 4, 14],
        'J' => [7, 2, 2, 2, 2, 18, 12],
        'K' => [17, 18, 20, 24, 20, 18, 17],
        'L' => [16, 16, 16, 16, 16, 16, 31],
        'M' => [17, 27, 21, 21, 17, 17, 17],
        'N' => [17, 17, 25, 21, 19, 17, 17],
        'O' => [14, 17, 17, 17, 17, 17, 14],
        'P' => [30, 17, 17, 30, 16, 16, 16],
        'Q' => [14, 17, 17, 17, 21, 18, 13],
        'R' => [30, 17, 17, 30, 20, 18, 17],
        'S' => [15, 16, 16, 14, 1, 1, 30],
        'T' => [31, 4, 4, 4, 4, 4, 4],
        'U' => [17, 17, 17, 17, 17, 17, 14],
        'V' => [17, 17, 17, 17, 17, 10, 4],
        'W' => [17, 17, 17, 21, 21, 21, 10],
        'X' => [17, 17, 10, 4, 10, 17, 17],
        'Y' => [17, 17, 10, 4, 4, 4, 4],
        'Z' => [31, 1, 2, 4, 8, 16, 31],
        '0' => [14, 17, 19, 21, 25, 17, 14],
        '1' => [4, 12, 4, 4, 4, 4, 14],
        '2' => [14, 17, 1, 2, 4, 8, 31],
        '3' => [31, 2, 4, 2, 1, 17, 14],
        '4' => [2, 6, 10, 18, 31, 2, 2],
        '5' => [31, 16, 30, 1, 1, 17, 14],
        '6' => [6, 8, 16, 30, 17, 17, 14],
        '7' => [31, 1, 2, 4, 8, 8, 8],
        '8' => [14, 17, 17, 14, 17, 17, 14],
        '9' => [14, 17, 17, 15, 1, 2, 12],
        '-' => [0, 0, 0, 31, 0, 0, 0],
        '+' => [0, 4, 4, 31, 4, 4, 0],
        '(' => [2, 4, 8, 8, 8, 4, 2],
        ')' => [8, 4, 2, 2, 2, 4, 8],
        ':' => [0, 12, 12, 0, 12, 12, 0],
        '.' => [0, 0, 0, 0, 0, 12, 12],
        ',' => [0, 0, 0, 0, 12, 4, 8],
        _ => [0; 7],
    }
}

/// Width of `text` in pixel-font pixels (6 per character, minus the gap).
fn text_px(text: &str) -> f32 {
    (text.chars().count() * 6).saturating_sub(1) as f32
}

/// Draw `text` with its top-left at (x, y); `px` is one font pixel. Runs of
/// lit pixels become one rectangle each.
fn text(out: &mut Vec<Instance>, text: &str, x: f32, y: f32, px: f32, rgb: [f32; 3]) {
    for (i, c) in text.chars().enumerate() {
        let gx = x + (i * 6) as f32 * px;
        for (row, bits) in glyph(c).iter().enumerate() {
            let mut col = 0;
            while col < 5 {
                if bits & (16 >> col) == 0 {
                    col += 1;
                    continue;
                }
                let start = col;
                while col < 5 && bits & (16 >> col) != 0 {
                    col += 1;
                }
                let n = (col - start) as f32;
                q(
                    out,
                    gx + (start as f32 + n / 2.0) * px,
                    y + (row as f32 + 0.5) * px,
                    n * px / 2.0,
                    px / 2.0,
                    0.0,
                    SHAPE_RECT,
                    rgb,
                    1.0,
                );
            }
        }
    }
}

/// Tooltip under the hovered button.
fn tooltip(out: &mut Vec<Instance>, l: &Layout, it: &Item) {
    let s = label(it.action);
    let px = (2.0 * l.scale).max(1.0).round();
    let pad = 6.0 * px / 2.0;
    let (w, h) = (text_px(s) * px + 2.0 * pad, 7.0 * px + 2.0 * pad);
    let cx = (it.x + it.size / 2.0).clamp(w / 2.0 + 4.0, (l.screen_w - w / 2.0 - 4.0).max(w / 2.0));
    let top = l.bar.1 + l.bar.3 + 8.0 * l.scale;
    q(
        out,
        cx,
        top + h / 2.0,
        w / 2.0 + px / 2.0,
        h / 2.0 + px / 2.0,
        0.0,
        SHAPE_RECT,
        [0.48, 0.64, 0.97],
        0.9,
    );
    q(
        out,
        cx,
        top + h / 2.0,
        w / 2.0,
        h / 2.0,
        0.0,
        SHAPE_RECT,
        [0.102, 0.106, 0.149],
        0.96,
    );
    text(
        out,
        s,
        (cx - w / 2.0 + pad).round(),
        (top + pad).round(),
        px,
        [0.75, 0.79, 0.96],
    );
}

pub fn draw(
    out: &mut Vec<Instance>,
    l: &Layout,
    selected: Tool,
    radius_scale: f32,
    scale: f32,
    hover: Option<(f32, f32)>,
) {
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
    if let Some((hx, hy)) = hover
        && let Some(it) = l
            .items
            .iter()
            .find(|i| hx >= i.x && hx <= i.x + i.size && hy >= i.y && hy <= i.y + i.size)
    {
        tooltip(out, l, it);
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
    fn every_label_is_drawable() {
        let l = layout(1920.0, 1.0);
        for it in &l.items {
            let s = label(it.action);
            assert!(
                s.chars().all(|c| c == ' ' || glyph(c) != [0; 7]),
                "missing glyph in {s:?}"
            );
            let mut v = Vec::new();
            tooltip(&mut v, &l, it);
            // Stays on screen.
            let (x0, x1) = v.iter().fold((f32::MAX, f32::MIN), |(a, b), i| {
                (a.min(i.pos[0] - i.half[0]), b.max(i.pos[0] + i.half[0]))
            });
            assert!(x0 >= 0.0 && x1 <= 1920.0, "{s}: {x0}..{x1}");
        }
        // Tools name their number key.
        assert!(label(Action::Tool(Tool::Sled)).ends_with("(9)"));
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
