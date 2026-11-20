//! Falling-sand snow grid with chunk sleeping and dirty-chunk tracking.

use serde::{Deserialize, Serialize};

pub const EMPTY: u8 = 0;
pub const SNOW: u8 = 1;
pub const PACKED: u8 = 2;
pub const ICE: u8 = 3;
/// Window ledge / solid obstacle. Never drawn, never moves.
pub const WALL: u8 = 4;

pub const CHUNK: usize = 32;

#[derive(Clone, Serialize, Deserialize)]
pub struct Grid {
    pub w: usize,
    pub h: usize,
    pub cells: Vec<u8>,
    #[serde(skip)]
    cw: usize,
    #[serde(skip)]
    ch: usize,
    #[serde(skip)]
    awake: Vec<bool>,
    #[serde(skip)]
    awake_next: Vec<bool>,
    #[serde(skip)]
    dirty: Vec<bool>,
    /// Chunk holds visible snow (walls excluded); refreshed on upload.
    #[serde(skip)]
    occupied: Vec<bool>,
    #[serde(skip)]
    flip: bool,
    #[serde(skip)]
    pub moved_last: usize,
    /// When set, cells that start falling freely (e.g. sliding off a window
    /// ledge) are removed and reported in `fell` so the world can turn them
    /// into flakes, which respect the pile height limit.
    #[serde(skip)]
    pub free_fall_to_flakes: bool,
    #[serde(skip)]
    pub fell: Vec<(u16, u16)>,
}

impl Grid {
    pub fn new(w: usize, h: usize) -> Self {
        let mut g = Self {
            w,
            h,
            cells: vec![EMPTY; w * h],
            cw: 0,
            ch: 0,
            awake: Vec::new(),
            awake_next: Vec::new(),
            dirty: Vec::new(),
            occupied: Vec::new(),
            flip: false,
            moved_last: 0,
            free_fall_to_flakes: false,
            fell: Vec::new(),
        };
        g.rebuild_meta();
        g
    }

    /// Call after deserializing.
    pub fn rebuild_meta(&mut self) {
        self.cw = self.w.div_ceil(CHUNK);
        self.ch = self.h.div_ceil(CHUNK);
        let n = self.cw * self.ch;
        self.awake = vec![true; n];
        self.awake_next = vec![false; n];
        self.dirty = vec![true; n];
        self.occupied = vec![true; n];
    }

    #[inline]
    pub fn get(&self, x: i32, y: i32) -> u8 {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 {
            // Left/right/bottom edges are solid, top is open.
            return if y < 0 { EMPTY } else { WALL };
        }
        self.cells[y as usize * self.w + x as usize]
    }

    #[inline]
    fn mark(&mut self, x: usize, y: usize) {
        let (cx, cy) = (x / CHUNK, y / CHUNK);
        let i = cy * self.cw + cx;
        self.dirty[i] = true;
        self.awake_next[i] = true;
        // Wake neighbours when touching a chunk border.
        let (lx, ly) = (x % CHUNK, y % CHUNK);
        if lx == 0 && cx > 0 {
            self.awake_next[i - 1] = true;
        }
        if lx == CHUNK - 1 && cx + 1 < self.cw {
            self.awake_next[i + 1] = true;
        }
        if ly == 0 && cy > 0 {
            self.awake_next[i - self.cw] = true;
        }
        if ly == CHUNK - 1 && cy + 1 < self.ch {
            self.awake_next[i + self.cw] = true;
        }
    }

    /// Wake chunks around a cell without changing it (e.g. support removed).
    pub fn wake(&mut self, x: usize, y: usize) {
        if x < self.w && y < self.h {
            self.mark(x, y);
            // Support removal affects the cell above, which may be in the chunk above.
            if y > 0 {
                self.mark(x, y - 1);
            }
        }
    }

    pub fn set(&mut self, x: i32, y: i32, v: u8) {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 {
            return;
        }
        let i = y as usize * self.w + x as usize;
        if self.cells[i] != v {
            self.cells[i] = v;
            self.mark(x as usize, y as usize);
            if y > 0 {
                self.mark(x as usize, y as usize - 1);
            }
        }
    }

    /// Add a snow cell only if the target is empty.
    pub fn add(&mut self, x: i32, y: i32, v: u8) -> bool {
        if self.get(x, y) == EMPTY {
            self.set(x, y, v);
            true
        } else {
            false
        }
    }

    #[cfg(test)]
    pub fn count(&self, kind: u8) -> usize {
        self.cells.iter().filter(|&&c| c == kind).count()
    }

    pub fn clear_snow(&mut self) {
        for c in self.cells.iter_mut() {
            if *c != WALL {
                *c = EMPTY;
            }
        }
        self.dirty.iter_mut().for_each(|d| *d = true);
        self.awake_next.iter_mut().for_each(|d| *d = false);
    }

    #[cfg(test)]
    pub fn awake_chunks(&self) -> usize {
        self.awake.iter().filter(|&&a| a).count()
    }

    #[inline]
    fn swap(&mut self, a: (i32, i32), b: (i32, i32)) {
        let ia = a.1 as usize * self.w + a.0 as usize;
        let ib = b.1 as usize * self.w + b.0 as usize;
        self.cells.swap(ia, ib);
        self.mark(a.0 as usize, a.1 as usize);
        self.mark(b.0 as usize, b.1 as usize);
        self.moved_last += 1;
    }

    /// One fixed simulation step. Only awake chunks are processed.
    pub fn step(&mut self) {
        self.moved_last = 0;
        std::mem::swap(&mut self.awake, &mut self.awake_next);
        self.awake_next.iter_mut().for_each(|a| *a = false);
        self.flip = !self.flip;
        let flip = self.flip;
        for cy in (0..self.ch).rev() {
            for cx in 0..self.cw {
                if !self.awake[cy * self.cw + cx] {
                    continue;
                }
                let x0 = cx * CHUNK;
                let x1 = (x0 + CHUNK).min(self.w);
                let y0 = cy * CHUNK;
                let y1 = (y0 + CHUNK).min(self.h);
                for y in (y0..y1).rev() {
                    if flip {
                        for x in x0..x1 {
                            self.update(x as i32, y as i32);
                        }
                    } else {
                        for x in (x0..x1).rev() {
                            self.update(x as i32, y as i32);
                        }
                    }
                }
            }
        }
    }

    #[inline]
    fn update(&mut self, x: i32, y: i32) {
        let c = self.cells[y as usize * self.w + x as usize];
        if c == EMPTY || c == WALL {
            return;
        }
        let below = self.get(x, y + 1);
        if below == EMPTY {
            if self.free_fall_to_flakes && c == SNOW && self.get(x, y + 2) == EMPTY {
                let i = y as usize * self.w + x as usize;
                self.cells[i] = EMPTY;
                self.mark(x as usize, y as usize);
                if y > 0 {
                    self.mark(x as usize, y as usize - 1);
                }
                self.fell.push((x as u16, y as u16));
                self.moved_last += 1;
                return;
            }
            self.swap((x, y), (x, y + 1));
            return;
        }
        if c == ICE {
            return; // ice never slumps
        }
        let (a, b) = if fastrand::bool() { (-1, 1) } else { (1, -1) };
        for dx in [a, b] {
            if self.get(x + dx, y) != EMPTY || self.get(x + dx, y + 1) != EMPTY {
                continue;
            }
            // Packed snow holds slopes up to 2 cells steep: only slides if the
            // diagonal landing spot also has nothing underneath at depth 2.
            if c == PACKED && self.get(x + dx, y + 2) != EMPTY {
                continue;
            }
            self.swap((x, y), (x + dx, y + 1));
            return;
        }
    }

    /// Iterate dirty chunks, calling `f(x, y, w, h)` and clearing the flags.
    /// Horizontally adjacent dirty chunks are merged into one rect so a busy
    /// frame needs few texture uploads.
    pub fn drain_dirty(&mut self, mut f: impl FnMut(usize, usize, usize, usize)) {
        for cy in 0..self.ch {
            let mut cx = 0;
            while cx < self.cw {
                if !self.dirty[cy * self.cw + cx] {
                    cx += 1;
                    continue;
                }
                let start = cx;
                while cx < self.cw && self.dirty[cy * self.cw + cx] {
                    self.dirty[cy * self.cw + cx] = false;
                    self.occupied[cy * self.cw + cx] = self.chunk_has_snow(cx, cy);
                    cx += 1;
                }
                let x = start * CHUNK;
                let y = cy * CHUNK;
                let x1 = (cx * CHUNK).min(self.w);
                f(x, y, x1 - x, CHUNK.min(self.h - y));
            }
        }
    }

    fn chunk_has_snow(&self, cx: usize, cy: usize) -> bool {
        let (x0, y0) = (cx * CHUNK, cy * CHUNK);
        let x1 = (x0 + CHUNK).min(self.w);
        (y0..(y0 + CHUNK).min(self.h)).any(|y| {
            self.cells[y * self.w + x0..y * self.w + x1]
                .iter()
                .any(|&c| c != EMPTY && c != WALL)
        })
    }

    /// First cell row that may contain visible snow (as of the last
    /// `drain_dirty`), or None if the grid is empty. Lets the renderer skip
    /// the empty sky.
    pub fn first_snow_row(&self) -> Option<usize> {
        let cy = (0..self.ch).find(|&cy| {
            self.occupied[cy * self.cw..(cy + 1) * self.cw]
                .iter()
                .any(|&o| o)
        })?;
        Some(cy * CHUNK)
    }

    pub fn mark_all_dirty(&mut self) {
        self.dirty.iter_mut().for_each(|d| *d = true);
        self.awake_next.iter_mut().for_each(|a| *a = true);
    }

    /// The surface in column x nearest below row `from`: the first solid
    /// cell at or below it, or, if `from` is inside solid cells, the top of
    /// that solid run. Returns h if there is nothing below.
    pub fn surface_from(&self, x: i32, from: i32) -> i32 {
        if x < 0 || x >= self.w as i32 {
            return 0;
        }
        let mut y = from.clamp(0, self.h as i32);
        if y < self.h as i32 && self.get(x, y) != EMPTY {
            while y > 0 && self.get(x, y - 1) != EMPTY {
                y -= 1;
            }
            return y;
        }
        while y < self.h as i32 && self.get(x, y) == EMPTY {
            y += 1;
        }
        y
    }

    /// Y of the highest non-empty cell in column x (h if none).
    pub fn surface_y(&self, x: i32) -> i32 {
        if x < 0 || x >= self.w as i32 {
            return 0;
        }
        for y in 0..self.h as i32 {
            if self.cells[y as usize * self.w + x as usize] != EMPTY {
                return y;
            }
        }
        self.h as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settle(g: &mut Grid) {
        for _ in 0..600 {
            g.step();
        }
    }

    #[test]
    fn column_slumps_into_pile() {
        let mut g = Grid::new(64, 64);
        for y in 40..64 {
            g.set(32, y, SNOW);
        }
        settle(&mut g);
        // Slope: no cell may be more than 1 higher than a neighbour column.
        let hs: Vec<i32> = (0..64).map(|x| 64 - g.surface_y(x)).collect();
        for w in hs.windows(2) {
            assert!((w[0] - w[1]).abs() <= 1, "heights {:?}", hs);
        }
        assert_eq!(g.count(SNOW), 24, "mass conserved");
    }

    #[test]
    fn packed_snow_holds_steeper_slope() {
        let mut snow = Grid::new(64, 64);
        let mut packed = Grid::new(64, 64);
        for y in 44..64 {
            snow.set(32, y, SNOW);
            packed.set(32, y, PACKED);
        }
        settle(&mut snow);
        settle(&mut packed);
        let spread = |g: &Grid| (0..64).filter(|&x| g.surface_y(x) < 64).count();
        assert!(spread(&packed) < spread(&snow));
    }

    #[test]
    fn ice_does_not_slump() {
        let mut g = Grid::new(32, 32);
        for y in 20..32 {
            g.set(16, y, ICE);
        }
        settle(&mut g);
        assert_eq!(g.get(15, 31), EMPTY);
        assert_eq!(g.get(16, 20), ICE);
    }

    #[test]
    fn sleeps_when_settled() {
        let mut g = Grid::new(128, 128);
        for x in 0..128 {
            g.set(x, 127, SNOW);
        }
        settle(&mut g);
        g.step();
        assert_eq!(g.moved_last, 0);
        assert_eq!(g.awake_chunks(), 0);
    }

    #[test]
    fn dirty_chunks_merge_into_row_runs() {
        let mut g = Grid::new(100, 70);
        g.drain_dirty(|_, _, _, _| {});
        g.set(1, 1, SNOW); // chunk (0,0)
        g.set(40, 1, SNOW); // chunk (1,0)
        g.set(99, 69, SNOW); // chunk (3,2), clipped
        let mut rects = Vec::new();
        g.drain_dirty(|x, y, w, h| rects.push((x, y, w, h)));
        assert_eq!(rects, vec![(0, 0, 64, 32), (96, 64, 4, 6)]);
        let mut n = 0;
        g.drain_dirty(|_, _, _, _| n += 1);
        assert_eq!(n, 0, "flags cleared");
    }

    #[test]
    fn first_snow_row_tracks_content() {
        let mut g = Grid::new(64, 128);
        g.drain_dirty(|_, _, _, _| {});
        assert_eq!(g.first_snow_row(), None);
        g.set(3, 100, WALL);
        g.drain_dirty(|_, _, _, _| {});
        assert_eq!(g.first_snow_row(), None, "walls are not drawn");
        g.set(3, 70, SNOW);
        g.drain_dirty(|_, _, _, _| {});
        assert_eq!(g.first_snow_row(), Some(64));
        g.set(3, 70, EMPTY);
        g.drain_dirty(|_, _, _, _| {});
        assert_eq!(g.first_snow_row(), None);
    }

    #[test]
    fn free_falling_snow_is_reported_not_moved() {
        let mut g = Grid::new(16, 32);
        g.free_fall_to_flakes = true;
        g.set(5, 3, SNOW);
        g.set(9, 3, PACKED); // user-made packed snow keeps falling as cells
        g.step();
        assert_eq!(g.fell, vec![(5, 3)]);
        assert_eq!(g.count(SNOW), 0);
        assert_eq!(g.count(PACKED), 1);
        // Resting on something: stays a cell.
        g.fell.clear();
        g.set(2, 31, SNOW);
        g.step();
        assert!(g.fell.is_empty());
    }

    #[test]
    fn wall_supports_snow() {
        let mut g = Grid::new(32, 32);
        for x in 10..20 {
            g.set(x, 15, WALL);
        }
        g.set(15, 5, SNOW);
        settle(&mut g);
        assert_eq!(g.get(15, 14), SNOW);
    }
}
