struct U {
    res: vec2<f32>,
    grid: vec2<f32>,
    cell: f32,
    time: f32,
    aurora: f32,
    sparkle: f32,
};
@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var gridtex: texture_2d<u32>;

struct FsIn { @builtin(position) pos: vec4<f32>, };

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> FsIn {
    var p = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    var o: FsIn;
    o.pos = vec4(p[i], 0.0, 1.0);
    return o;
}

fn hash(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453);
}

fn cell_at(c: vec2<i32>) -> u32 {
    let g = vec2<i32>(u.grid);
    if (c.x < 0 || c.y < 0 || c.x >= g.x || c.y >= g.y) { return 0u; }
    return textureLoad(gridtex, c, 0).r;
}

@fragment
fn fs_grid(in: FsIn) -> @location(0) vec4<f32> {
    let c = vec2<i32>(floor(in.pos.xy / u.cell));
    let t = cell_at(c);
    if (t == 0u || t >= 4u) { return vec4(0.0); }
    let n = hash(vec2<f32>(c));
    let above = cell_at(c + vec2(0, -1));
    let top = select(0.0, 1.0, above == 0u || above >= 4u);
    var col = vec3(0.93, 0.96, 1.0) - n * 0.04;
    if (t == 2u) { col = vec3(0.80, 0.87, 0.97) - n * 0.04; }
    if (t == 3u) {
        col = vec3(0.55, 0.80, 0.98) + n * 0.05;
        let sh = sin((in.pos.x + in.pos.y) * 0.05 + u.time * 0.8);
        col += vec3(0.12) * max(sh, 0.0);
        if (u.sparkle > 0.5) {
            let s = hash(vec2<f32>(c) + floor(u.time * 2.0));
            if (s > 0.993) { col = vec3(1.0); }
        }
    }
    col = mix(col, vec3(1.0), top * 0.35);
    // Slightly darker toward cell bottoms for depth.
    let fy = fract(in.pos.y / u.cell);
    col *= 1.0 - 0.06 * fy;
    return vec4(col, 1.0);
}

@fragment
fn fs_aurora(in: FsIn) -> @location(0) vec4<f32> {
    let uv = in.pos.xy / u.res;
    let band = smoothstep(0.45, 0.0, uv.y);
    let w = sin(uv.x * 7.0 + u.time * 0.25) * 0.5 + sin(uv.x * 13.0 - u.time * 0.4) * 0.25;
    let curtain = smoothstep(0.0, 1.0, 0.5 + 0.5 * sin(uv.x * 30.0 + w * 3.0 + u.time * 0.3));
    let h = clamp(band * (0.5 + 0.5 * w + 0.4 * curtain), 0.0, 1.0);
    let col = mix(vec3(0.1, 0.9, 0.5), vec3(0.5, 0.3, 0.95), uv.y * 2.2 + w * 0.3);
    let a = h * 0.32 * u.aurora;
    return vec4(col * a, a);
}

struct VsIn {
    @builtin(vertex_index) vi: u32,
    @location(0) pos: vec2<f32>,
    @location(1) half: vec2<f32>,
    @location(2) rot: f32,
    @location(3) shape: u32,
    @location(4) color: vec4<f32>,
};
struct SpOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) half: vec2<f32>,
    @location(2) @interpolate(flat) shape: u32,
    @location(3) color: vec4<f32>,
};

@vertex
fn vs_sprite(i: VsIn) -> SpOut {
    var q = array<vec2<f32>, 6>(
        vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(-1.0, 1.0),
        vec2(-1.0, 1.0), vec2(1.0, -1.0), vec2(1.0, 1.0));
    let k = q[i.vi];
    let ext = i.half + vec2(1.5);
    let local = k * ext;
    let cs = cos(i.rot);
    let sn = sin(i.rot);
    let w = vec2(local.x * cs - local.y * sn, local.x * sn + local.y * cs) + i.pos;
    var o: SpOut;
    o.clip = vec4(w.x / u.res.x * 2.0 - 1.0, 1.0 - w.y / u.res.y * 2.0, 0.0, 1.0);
    o.local = local;
    o.half = i.half;
    o.shape = i.shape;
    o.color = i.color;
    return o;
}

@fragment
fn fs_sprite(i: SpOut) -> @location(0) vec4<f32> {
    let h = max(i.half, vec2(0.5));
    let q = i.local / h;
    var a = 0.0;
    let m = min(h.x, h.y);
    switch (i.shape) {
        case 0u: { a = smoothstep(1.0, 0.15, length(q)); }
        case 1u: { a = clamp((1.0 - length(q)) * m, 0.0, 1.0); }
        case 2u: {
            let d = abs(i.local) - h;
            a = clamp(0.5 - max(d.x, d.y), 0.0, 1.0);
        }
        case 3u: {
            let w = (1.0 - q.x) * 0.5;
            a = clamp((w - abs(q.y)) * m, 0.0, 1.0) * step(abs(q.x), 1.0);
        }
        case 4u: {
            let s = sqrt(abs(q.x)) + sqrt(abs(q.y));
            a = smoothstep(1.0, 0.7, s);
        }
        case 5u: { let r = max(1.0 - length(q), 0.0); a = r * r; }
        case 6u: {
            let d = abs(length(q) - 1.0 + 0.08) * m;
            a = clamp(1.0 - d, 0.0, 1.0);
        }
        default: { a = 1.0; }
    }
    return i.color * a;
}
