pub mod balls;
pub mod frost;
pub mod grid;
pub mod ledges;
pub mod penguin;
pub mod records;
pub mod scene;
pub mod season;
pub mod sled;
pub mod village;
pub mod weather;
pub mod world;

/// One instanced quad for the sprite pipeline (40 bytes).
#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Instance {
    pub pos: [f32; 2],
    pub half: [f32; 2],
    pub rot: f32,
    pub shape: u32,
    /// Premultiplied RGBA.
    pub color: [f32; 4],
}

pub const SHAPE_SOFT: u32 = 0;
pub const SHAPE_ELLIPSE: u32 = 1;
pub const SHAPE_RECT: u32 = 2;
pub const SHAPE_TRI: u32 = 3;
pub const SHAPE_STAR: u32 = 4;
pub const SHAPE_GLOW: u32 = 5;
pub const SHAPE_RING: u32 = 6;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

pub fn premul(rgb: [f32; 3], a: f32) -> [f32; 4] {
    [rgb[0] * a, rgb[1] * a, rgb[2] * a, a]
}
