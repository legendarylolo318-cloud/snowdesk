//! Overlay thread: owns the GPU, per-monitor worlds and input handling.
//! Backends (Wayland layer-shell, winit for X11/Windows) feed it surfaces and
//! pointer events and call `Engine::drive` in their event loop.

#[cfg(debug_assertions)]
mod demo;
pub mod toolbar;
#[cfg(target_os = "linux")]
pub mod wayland;
#[cfg(windows)]
pub mod windows;
#[cfg(any(windows, target_os = "linux"))]
pub mod winit_common;
#[cfg(target_os = "linux")]
pub mod x11;

#[cfg(any(windows, target_os = "linux"))]
pub use winit_common::{Host, HostMsg};

use crate::audio::tap::Levels;
use crate::config::Config;
use crate::platform::{self, Shared, Snapshot};
use crate::render::{Gpu, MonitorRenderer};
use crate::sim::balls::DecorKind;
use crate::sim::world::World;
use crate::sim::{
    Instance, Rect, SHAPE_ELLIPSE, SHAPE_RECT, SHAPE_RING, SHAPE_STAR, SHAPE_TRI, premul,
};
use crate::tools::Tool;
use crossbeam_channel::{Receiver, Sender};
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub enum OverlayCmd {
    Config(Box<Config>),
    TogglePlay,
    TogglePause,
    /// Hide the snow on every monitor (the scene is kept) or show it again.
    Hide(bool),
    Clear,
    Tool(i32),
    Radius(f32),
    Undo,
    Redo,
    StampSnowman,
    Igloo,
    Mountains,
    SaveScene,
    LoadScene,
    /// Save a PNG of the current scene (overlay only) to Pictures/snowdesk.
    ExportPicture,
    /// Save a shareable scene file to Pictures/snowdesk.
    ExportScene,
    /// Load a scene file from someone else.
    ImportScene(std::path::PathBuf),
    Quit,
}

#[derive(Debug, Clone)]
pub enum OverlayEvent {
    Play(bool),
    Paused(bool),
    /// The snow is hidden on every monitor ("Hide snow for 1 hour").
    Hidden(bool),
    AutoPaused(bool),
    Monitors(Vec<String>),
    /// Weather forecast text for the panel ("" when weather is off).
    Forecast(String),
    /// Something worth telling the user (a file was saved, ...).
    Notice(String),
    /// Result of loading a scene file (the panel shows the Play tab).
    Imported(String),
    /// Builder challenge records, and which one was just beaten.
    Records(String, Option<&'static str>),
    /// Tool and radius changed from the overlay (tool bar, keys, wheel).
    ToolState(i32, f32),
    Error(String),
    Fatal(String),
}

#[derive(Clone)]
pub struct OverlayCtx {
    pub cmd_rx: Receiver<OverlayCmd>,
    pub ev_tx: Sender<OverlayEvent>,
    pub levels: Arc<Levels>,
    pub cfg: Config,
    pub health: Arc<Health>,
}

/// Liveness info read by the watchdog on the main thread.
pub struct Health {
    start: Instant,
    beat_ms: AtomicU64,
    /// True while the overlay takes input (play mode).
    pub play: AtomicBool,
}

impl Default for Health {
    fn default() -> Self {
        Self {
            start: Instant::now(),
            beat_ms: AtomicU64::new(0),
            play: AtomicBool::new(false),
        }
    }
}

impl Health {
    fn beat(&self) {
        self.beat_ms
            .store(self.start.elapsed().as_millis() as u64, Ordering::Relaxed);
    }
    /// Time since the overlay loop last ran.
    pub fn stalled_for(&self) -> Duration {
        let now = self.start.elapsed().as_millis() as u64;
        Duration::from_millis(now.saturating_sub(self.beat_ms.load(Ordering::Relaxed)))
    }
}

/// Error message used to request an overlay rebuild after GPU loss.
pub const DEVICE_LOST: &str = "GPU device lost";

#[derive(Clone, Debug)]
pub struct MonSpec {
    pub name: String,
    /// Origin and size in the feed's coordinate space.
    pub gx: f32,
    pub gy: f32,
    pub lw: f32,
    pub lh: f32,
    /// Surface pixels per feed unit.
    pub feed_scale: f32,
    pub w: u32,
    pub h: u32,
    /// DPI scale used for cell size and sprite sizes.
    pub scale: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug)]
pub enum Ptr {
    Move(f32, f32),
    Down(Button, f32, f32),
    Up(Button, f32, f32),
    Leave,
    Escape,
    /// Number key 1-9: pick that tool.
    Digit(u8),
    /// Scroll wheel, in notches (positive = up = bigger brush).
    Wheel(f32),
}

struct Mon {
    spec: MonSpec,
    world: World,
    renderer: MonitorRenderer,
    instances: Vec<Instance>,
    /// Cached tool bar layout (depends only on width and scale).
    toolbar: toolbar::Layout,
    blanked: bool,
    last_gen: u64,
    force_windows: bool,
}

pub struct Drive {
    pub sleep: Duration,
    pub play_changed: Option<bool>,
    pub quit: bool,
    /// The GPU device is gone; the backend must rebuild everything.
    pub lost: bool,
}

impl Drive {
    /// Nothing to draw yet (no monitor configured).
    pub fn idle() -> Self {
        Self {
            sleep: Duration::from_millis(100),
            play_changed: None,
            quit: false,
            lost: false,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct PenguinGrab {
    mon: usize,
    idx: usize,
    start: (f32, f32),
    /// Moved far enough to count as a drag (else it is a poke on release).
    dragging: bool,
}

/// Pointer velocity over the recent history (px/s), capped.
fn flick_velocity(
    hist: &VecDeque<(Instant, f32, f32)>,
    now: Instant,
    x: f32,
    y: f32,
    max: f32,
) -> (f32, f32) {
    let Some(&(t0, x0, y0)) = hist.front() else {
        return (0.0, 0.0);
    };
    let dt = (now - t0).as_secs_f32().max(0.03);
    let (mut vx, mut vy) = ((x - x0) / dt, (y - y0) / dt);
    let sp = vx.hypot(vy);
    if sp > max {
        vx *= max / sp;
        vy *= max / sp;
    }
    (vx, vy)
}

pub struct Engine {
    gpu: Arc<Gpu>,
    ctx: OverlayCtx,
    feed: Arc<Shared>,
    cfg: Config,
    mons: Vec<Mon>,
    play: bool,
    paused: bool,
    /// "Hide snow for 1 hour": every monitor is blanked.
    hidden: bool,
    auto_paused: bool,
    tool: Tool,
    radius: f32,
    last_frame: Instant,
    next_frame: Instant,
    last_beats: u32,
    aurora: f32,
    aurora_checked: Instant,
    weather: crate::sim::weather::Weather,
    records: crate::sim::records::Records,
    /// Capture each monitor after the next frame is drawn.
    export_picture: bool,
    forecast_at: Instant,
    /// Local month and hour, refreshed once a minute.
    month: u32,
    hour: u32,
    // pointer state
    ptr_mon: Option<usize>,
    ptr: (f32, f32),
    left_down: bool,
    stroke: bool,
    decor_drag: Option<(DecorKind, u8)>,
    right_down: bool,
    /// Penguin pressed with the left button (play mode).
    penguin: Option<PenguinGrab>,
    /// Sled being dragged (monitor, index).
    sled: Option<(usize, usize)>,
    #[cfg(debug_assertions)]
    demo: Option<demo::Demo>,
    ptr_hist: VecDeque<(Instant, f32, f32)>,
    pending_play: Option<bool>,
    quit: bool,
    /// Blanked monitors release their swapchain (Wayland only).
    pub shrink_when_blank: bool,
    /// Next time to return freed heap memory to the OS.
    next_trim: Instant,
}

impl Engine {
    pub fn new(gpu: Arc<Gpu>, ctx: OverlayCtx) -> Self {
        let cfg = ctx.cfg.clone();
        let now = Instant::now();
        let feed = platform::start();
        Self {
            gpu,
            ctx,
            feed,
            paused: cfg.paused,
            hidden: false,
            cfg,
            mons: Vec::new(),
            play: false,
            auto_paused: false,
            tool: Tool::Brush,
            radius: 28.0,
            last_frame: now,
            next_frame: now,
            last_beats: 0,
            aurora: 0.0,
            aurora_checked: now - Duration::from_secs(120),
            weather: crate::sim::weather::Weather::new(),
            records: crate::sim::records::Records::load(),
            export_picture: false,
            forecast_at: now,
            month: platform::local_month(),
            hour: platform::local_hour(),
            ptr_mon: None,
            ptr: (0.0, 0.0),
            left_down: false,
            stroke: false,
            decor_drag: None,
            right_down: false,
            ptr_hist: VecDeque::new(),
            pending_play: None,
            shrink_when_blank: false,
            next_trim: now + Duration::from_secs(5),
            penguin: None,
            sled: None,
            #[cfg(debug_assertions)]
            demo: demo::Demo::from_env(),
            quit: false,
        }
    }

    pub fn gpu(&self) -> Arc<Gpu> {
        self.gpu.clone()
    }

    pub fn monitor_names(&self) -> Vec<String> {
        self.mons.iter().map(|m| m.spec.name.clone()).collect()
    }

    pub fn add_monitor(
        &mut self,
        spec: MonSpec,
        surface: wgpu::Surface<'static>,
    ) -> anyhow::Result<()> {
        self.remove_monitor(&spec.name);
        let mut world = World::new(spec.w, spec.h, spec.scale, &self.cfg);
        let renderer =
            MonitorRenderer::new(self.gpu.clone(), surface, spec.w, spec.h, &world.grid)?;
        if self.cfg.restore_scene
            && let Ok(bytes) = std::fs::read(Config::scene_path())
        {
            crate::sim::scene::decode(&bytes, &mut [(spec.name.as_str(), &mut world)]);
        }
        tracing::info!(
            "monitor {} {}x{} scale {}",
            spec.name,
            spec.w,
            spec.h,
            spec.scale
        );
        self.mons.push(Mon {
            toolbar: toolbar::layout(spec.w as f32, spec.scale),
            spec,
            world,
            renderer,
            instances: Vec::with_capacity(4096),
            blanked: false,
            last_gen: u64::MAX,
            force_windows: true,
        });
        let _ = self
            .ctx
            .ev_tx
            .send(OverlayEvent::Monitors(self.monitor_names()));
        self.send_records(None);
        Ok(())
    }

    pub fn remove_monitor(&mut self, name: &str) {
        if let Some(i) = self.mons.iter().position(|m| m.spec.name == name) {
            self.mons.remove(i);
            self.ptr_mon = None;
            self.penguin = None;
            self.sled = None;
            let _ = self
                .ctx
                .ev_tx
                .send(OverlayEvent::Monitors(self.monitor_names()));
        }
    }

    pub fn resize_monitor(&mut self, spec: MonSpec) {
        if let Some(m) = self.mons.iter_mut().find(|m| m.spec.name == spec.name) {
            if (m.spec.w, m.spec.h) != (spec.w, spec.h) || m.spec.scale != spec.scale {
                let mut world = World::new(spec.w, spec.h, spec.scale, &self.cfg);
                m.renderer.resize(spec.w, spec.h, &mut world.grid);
                m.world = world;
                m.force_windows = true;
                m.toolbar = toolbar::layout(spec.w as f32, spec.scale);
                self.ptr_mon = None;
                self.penguin = None;
            }
            m.spec = spec;
        }
    }

    // ---- commands -------------------------------------------------------

    fn set_hidden(&mut self, h: bool) {
        if self.hidden != h {
            self.hidden = h;
            if h {
                self.set_play(false);
            }
            let _ = self.ctx.ev_tx.send(OverlayEvent::Hidden(h));
        }
    }

    fn set_play(&mut self, on: bool) {
        if self.play != on {
            self.play = on;
            self.left_down = false;
            self.right_down = false;
            self.stroke = false;
            self.decor_drag = None;
            self.drop_penguin();
            if let Some((sm, si)) = self.sled.take()
                && let Some(m) = self.mons.get_mut(sm)
            {
                m.world.push_sled(si, 0.0);
            }
            for m in &mut self.mons {
                m.world.roll_end();
            }
            self.pending_play = Some(on);
            self.ctx.health.play.store(on, Ordering::Relaxed);
            let _ = self.ctx.ev_tx.send(OverlayEvent::Play(on));
        }
    }

    fn save_scene(&self) {
        let list: Vec<(&str, &World)> = self
            .mons
            .iter()
            .map(|m| (m.spec.name.as_str(), &m.world))
            .collect();
        let bytes = crate::sim::scene::encode(&list);
        let p = Config::scene_path();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Err(e) = std::fs::write(&p, bytes) {
            let _ = self
                .ctx
                .ev_tx
                .send(OverlayEvent::Error(format!("Cannot save scene: {e}")));
        }
    }

    fn load_scene(&mut self) {
        match std::fs::read(Config::scene_path()) {
            Ok(bytes) => {
                let mut list: Vec<(&str, &mut World)> = self
                    .mons
                    .iter_mut()
                    .map(|m| (m.spec.name.as_str(), &mut m.world))
                    .collect();
                crate::sim::scene::decode(&bytes, &mut list);
                for m in &mut self.mons {
                    m.world.grid.mark_all_dirty();
                    m.force_windows = true;
                }
            }
            Err(_) => {
                let _ = self
                    .ctx
                    .ev_tx
                    .send(OverlayEvent::Error("No saved scene yet.".into()));
            }
        }
    }

    /// Queue-free command entry for backends that already received it.
    pub fn push_cmd(&mut self, c: OverlayCmd) {
        self.handle(c);
    }

    fn handle(&mut self, c: OverlayCmd) {
        match c {
            OverlayCmd::Config(cfg) => {
                self.cfg = *cfg;
                if self.paused != self.cfg.paused {
                    self.paused = self.cfg.paused;
                    let _ = self.ctx.ev_tx.send(OverlayEvent::Paused(self.paused));
                }
                for m in &mut self.mons {
                    m.world.cfg = self.cfg.clone();
                    m.force_windows = true;
                }
                self.next_frame = Instant::now();
            }
            OverlayCmd::TogglePlay => {
                let p = !self.play;
                if p {
                    // Playing with hidden snow makes no sense: show it.
                    self.set_hidden(false);
                }
                self.set_play(p)
            }
            OverlayCmd::TogglePause => {
                self.paused = !self.paused;
                self.cfg.paused = self.paused;
                if self.paused {
                    self.set_play(false);
                }
                let _ = self.ctx.ev_tx.send(OverlayEvent::Paused(self.paused));
            }
            OverlayCmd::Hide(h) => self.set_hidden(h),
            OverlayCmd::Clear => {
                for m in &mut self.mons {
                    m.world.clear_all();
                }
            }
            OverlayCmd::Tool(i) => self.tool = Tool::from_index(i),
            OverlayCmd::Radius(r) => self.radius = r.clamp(6.0, 200.0),
            OverlayCmd::Undo => self.each_world(|w| w.undo()),
            OverlayCmd::Redo => self.each_world(|w| w.redo()),
            OverlayCmd::StampSnowman => {
                let i = self.ptr_mon.unwrap_or(0);
                if let Some(m) = self.mons.get_mut(i) {
                    let x = m.world.w * (0.25 + 0.5 * fastrand::f32());
                    m.world.stamp_snowman(x);
                }
            }
            OverlayCmd::Igloo => {
                let i = self.ptr_mon.unwrap_or(0);
                if let Some(m) = self.mons.get_mut(i) {
                    let x = m.world.w * (0.25 + 0.5 * fastrand::f32());
                    m.world.stamp_igloo(x);
                }
            }
            OverlayCmd::Mountains => {
                let i = self.ptr_mon.unwrap_or(0);
                if let Some(m) = self.mons.get_mut(i) {
                    m.world.generate_mountains();
                }
            }
            OverlayCmd::SaveScene => self.save_scene(),
            OverlayCmd::ExportPicture => {
                self.export_picture = true;
                self.next_frame = Instant::now();
            }
            OverlayCmd::ExportScene => {
                let list: Vec<(&str, &World)> = self
                    .mons
                    .iter()
                    .map(|m| (m.spec.name.as_str(), &m.world))
                    .collect();
                let bytes = crate::sim::scene::encode(&list);
                let dir = platform::export_dir();
                let path = dir.join(format!("snowdesk-{}.snowscene", platform::local_stamp()));
                let msg = std::fs::create_dir_all(&dir)
                    .and_then(|_| std::fs::write(&path, bytes))
                    .map(|_| format!("Scene saved to {}", path.display()))
                    .unwrap_or_else(|e| format!("Cannot save the scene: {e}"));
                let _ = self.ctx.ev_tx.send(OverlayEvent::Notice(msg));
            }
            OverlayCmd::ImportScene(path) => {
                let msg = match std::fs::read(&path) {
                    Ok(bytes) => {
                        let mut list: Vec<(&str, &mut World)> = self
                            .mons
                            .iter_mut()
                            .map(|m| (m.spec.name.as_str(), &mut m.world))
                            .collect();
                        let n = crate::sim::scene::import(&bytes, &mut list);
                        for m in &mut self.mons {
                            m.world.grid.mark_all_dirty();
                            m.force_windows = true;
                        }
                        if n > 0 {
                            format!("Loaded a scene onto {n} monitor(s)")
                        } else {
                            "That file is not a snowdesk scene".to_string()
                        }
                    }
                    Err(e) => format!("Cannot open {}: {e}", path.display()),
                };
                tracing::info!("scene import: {msg}");
                let _ = self.ctx.ev_tx.send(OverlayEvent::Imported(msg));
            }
            OverlayCmd::LoadScene => self.load_scene(),
            OverlayCmd::Quit => {
                if self.cfg.restore_scene {
                    self.save_scene();
                }
                self.quit = true;
            }
        }
    }

    /// Let go of a carried penguin (it falls where it is).
    fn drop_penguin(&mut self) {
        if let Some(g) = self.penguin.take()
            && let Some(m) = self.mons.get_mut(g.mon)
        {
            m.world.release_penguin(g.idx, 0.0, 0.0);
        }
    }

    fn each_world(&mut self, f: impl Fn(&mut World)) {
        for m in &mut self.mons {
            f(&mut m.world);
        }
    }

    // ---- pointer ----------------------------------------------------------

    pub fn pointer(&mut self, mon: &str, ev: Ptr) {
        if !self.play {
            return;
        }
        let Some(mi) = self.mons.iter().position(|m| m.spec.name == mon) else {
            return;
        };
        match ev {
            Ptr::Escape => {
                self.set_play(false);
                return;
            }
            Ptr::Digit(d) => {
                if (1..=9).contains(&d) {
                    self.tool = Tool::from_index(d as i32 - 1);
                    self.send_tool_state();
                }
                return;
            }
            Ptr::Wheel(n) => {
                if n.is_finite() && n != 0.0 {
                    self.radius =
                        (self.radius * 1.12f32.powf(n.clamp(-5.0, 5.0))).clamp(6.0, 200.0);
                    self.send_tool_state();
                }
                return;
            }
            _ => {}
        }
        if let Ptr::Leave = ev {
            if self.ptr_mon == Some(mi) {
                self.ptr_mon = None;
            }
            if self.penguin.is_some_and(|g| g.mon == mi) {
                self.drop_penguin();
                self.left_down = false;
            }
            return;
        }
        self.ptr_mon = Some(mi);
        let sc = self.mons[mi].spec.scale;
        let radius_px = self.radius * sc;
        let now = Instant::now();
        match ev {
            Ptr::Move(x, y) => {
                self.ptr = (x, y);
                self.ptr_hist.push_back((now, x, y));
                while self
                    .ptr_hist
                    .front()
                    .is_some_and(|h| now - h.0 > Duration::from_millis(150))
                {
                    self.ptr_hist.pop_front();
                }
                if let Some((sm, si)) = self.sled
                    && sm == mi
                {
                    self.mons[mi].world.drag_sled(si, x, y);
                    return;
                }
                if let Some(g) = self.penguin.as_mut()
                    && g.mon == mi
                {
                    let w = &mut self.mons[mi].world;
                    if !g.dragging && (x - g.start.0).hypot(y - g.start.1) > 6.0 * sc {
                        g.dragging = true;
                        w.grab_penguin(g.idx);
                    }
                    if g.dragging {
                        w.drag_penguin(g.idx, x, y);
                    }
                    return;
                }
                if !self.left_down && !self.right_down {
                    self.mons[mi].world.cursor_footprint(x, y);
                }
                if self.mons[mi].world.frost_pointer(x, y) {
                    for m in &mut self.mons {
                        m.world.frost.touch();
                    }
                }
                if self.left_down && self.stroke {
                    let w = &mut self.mons[mi].world;
                    if w.rolling.is_some() {
                        // Rolling a new ball or carrying an old one.
                        w.roll_move(x, y);
                    } else if self.tool != Tool::Snowball {
                        w.apply_tool(self.tool, x, y, radius_px);
                    }
                }
            }
            Ptr::Down(Button::Left, x, y) => {
                self.ptr = (x, y);
                let lay = &self.mons[mi].toolbar;
                if lay.contains(x, y) {
                    match lay.hit(x, y) {
                        Some(toolbar::Action::Decor(k, c)) => {
                            self.decor_drag = Some((k, c));
                            self.left_down = true;
                        }
                        Some(a) => self.toolbar_action(a, mi),
                        None => {}
                    }
                    return;
                }
                self.left_down = true;
                // Penguins take clicks before any tool: poke, carry, throw.
                if let Some(idx) = self.mons[mi].world.penguin_at(x, y) {
                    self.penguin = Some(PenguinGrab {
                        mon: mi,
                        idx,
                        start: (x, y),
                        dragging: false,
                    });
                    self.ptr_hist.clear();
                    self.ptr_hist.push_back((now, x, y));
                    return;
                }
                // Sleds: drag and flick to push.
                if let Some(i) = self.mons[mi].world.sled_at(x, y) {
                    self.sled = Some((mi, i));
                    self.ptr_hist.clear();
                    self.ptr_hist.push_back((now, x, y));
                    return;
                }
                if self.tool == Tool::Sled {
                    self.mons[mi].world.place_sled(x, y);
                    return;
                }
                // Existing snowballs: pick them up (the snowball tool keeps
                // rolling them bigger).
                if let Some(i) = self.mons[mi].world.ball_at(x, y) {
                    let w = &mut self.mons[mi].world;
                    w.push_undo();
                    w.grab_ball(i, x, y, self.tool == Tool::Snowball);
                    self.stroke = true;
                    self.ptr_hist.clear();
                    self.ptr_hist.push_back((now, x, y));
                    return;
                }
                let tool = self.tool;
                let w = &mut self.mons[mi].world;
                w.push_undo();
                self.stroke = true;
                match tool {
                    Tool::Snowball => {
                        w.roll_begin(x, y);
                        w.roll_move(x, y);
                    }
                    Tool::Angel => w.stamp_angel(x),
                    t => w.apply_tool(t, x, y, radius_px),
                }
            }
            Ptr::Up(Button::Left, x, y) => {
                if let Some((sm, si)) = self.sled.take() {
                    let (vx, _) = flick_velocity(&self.ptr_hist, now, x, y, 2600.0 * sc);
                    if let Some(m) = self.mons.get_mut(sm) {
                        m.world.push_sled(si, vx);
                    }
                    self.left_down = false;
                    return;
                }
                if let Some(g) = self.penguin.take() {
                    if let Some(m) = self.mons.get_mut(g.mon) {
                        if g.dragging {
                            let (vx, vy) = flick_velocity(&self.ptr_hist, now, x, y, 1800.0 * sc);
                            m.world.release_penguin(g.idx, vx, vy);
                        } else {
                            m.world.poke_penguin(g.idx, x);
                        }
                    }
                    self.left_down = false;
                    return;
                }
                if let Some((k, c)) = self.decor_drag.take() {
                    self.mons[mi].world.decorate_at(x, y, k, c);
                }
                if self.stroke {
                    let (vx, vy) = flick_velocity(&self.ptr_hist, now, x, y, 2600.0 * sc);
                    self.mons[mi].world.roll_end_with(vx, vy);
                }
                self.left_down = false;
                self.stroke = false;
            }
            Ptr::Down(Button::Right, x, y) => {
                self.right_down = true;
                self.ptr = (x, y);
                self.ptr_hist.clear();
                self.ptr_hist.push_back((now, x, y));
            }
            Ptr::Up(Button::Right, x, y) if self.right_down => {
                self.right_down = false;
                let (vx, vy) = flick_velocity(&self.ptr_hist, now, x, y, 2600.0 * sc);
                if vx.hypot(vy) > 150.0 * sc {
                    self.mons[mi].world.throw_ball(x, y, vx, vy);
                }
            }
            _ => {}
        }
    }

    fn send_tool_state(&self) {
        let _ = self
            .ctx
            .ev_tx
            .send(OverlayEvent::ToolState(self.tool.index(), self.radius));
    }

    fn toolbar_action(&mut self, a: toolbar::Action, mi: usize) {
        use toolbar::Action as A;
        // Keep the panel's tool buttons and radius slider in step.
        let sync = matches!(a, A::Tool(_) | A::Radius(_));
        self.toolbar_action_inner(a, mi);
        if sync {
            self.send_tool_state();
        }
    }

    fn toolbar_action_inner(&mut self, a: toolbar::Action, mi: usize) {
        use toolbar::Action as A;
        match a {
            A::Tool(t) => self.tool = t,
            A::Undo => self.mons[mi].world.undo(),
            A::Redo => self.mons[mi].world.redo(),
            A::Clear => self.mons[mi].world.clear_all(),
            A::Snowman => {
                let x = self.mons[mi].world.w * (0.2 + 0.6 * fastrand::f32());
                self.mons[mi].world.stamp_snowman(x);
            }
            A::Igloo => {
                let x = self.mons[mi].world.w * (0.2 + 0.6 * fastrand::f32());
                self.mons[mi].world.stamp_igloo(x);
            }
            A::Mountains => self.mons[mi].world.generate_mountains(),
            A::Radius(d) => self.radius = (self.radius * (1.0 + d)).clamp(6.0, 200.0),
            A::Exit => self.set_play(false),
            A::Decor(..) => {}
        }
    }

    // ---- main step --------------------------------------------------------

    fn apply_feed(&mut self, snap: &Snapshot) {
        let cursor_on = !self.play && snap.supports_cursor;
        let mut active = false;
        for m in &mut self.mons {
            let s = &m.spec;
            if snap.generation != m.last_gen || m.force_windows {
                m.last_gen = snap.generation;
                m.force_windows = false;
                let f = s.feed_scale;
                let mut local = Vec::new();
                for w in &snap.windows {
                    let r = w.rect;
                    let on_mon = match &w.monitor {
                        Some(n) => *n == s.name,
                        // Unknown monitor: any overlap (a window spanning two
                        // screens gets a ledge on both).
                        None => {
                            r.x < s.gx + s.lw
                                && r.x + r.w > s.gx
                                && r.y < s.gy + s.lh
                                && r.y + r.h > s.gy
                        }
                    };
                    if on_mon {
                        let rect = Rect {
                            x: (r.x - s.gx) * f,
                            y: (r.y - s.gy) * f,
                            w: r.w * f,
                            h: r.h * f,
                        };
                        local.push((rect, w.ledge, w.id));
                    }
                }
                let margin = 3.0 * m.world.cell;
                m.world
                    .set_windows(&platform::visible_ledges(&local, margin));
            }
            let cur = match (cursor_on, snap.cursor) {
                (true, Some((cx, cy)))
                    if cx >= s.gx && cx < s.gx + s.lw && cy >= s.gy && cy < s.gy + s.lh =>
                {
                    Some(((cx - s.gx) * s.feed_scale, (cy - s.gy) * s.feed_scale))
                }
                _ => None,
            };
            m.world
                .set_cursor(if self.cfg.cursor_wind { cur } else { None });
            // Frost needs a pointer to wipe it: only where we can see one.
            m.world.frost_ok = snap.supports_cursor || self.play;
            if let Some((x, y)) = cur {
                active |= m.world.frost_pointer(x, y);
            }
        }
        if active {
            for m in &mut self.mons {
                m.world.frost.touch();
            }
        }
    }

    pub fn drive(&mut self) -> Drive {
        #[cfg(debug_assertions)]
        self.debug_fault();
        self.ctx.health.beat();
        while let Ok(c) = self.ctx.cmd_rx.try_recv() {
            self.handle(c);
        }
        #[cfg(debug_assertions)]
        self.demo_tick();
        if self.gpu.is_lost() {
            return Drive {
                sleep: Duration::ZERO,
                play_changed: None,
                quit: true,
                lost: true,
            };
        }
        let now = Instant::now();
        let snap = self.feed.snapshot();
        let fs_all = snap.fullscreen_any;
        let any_fs = fs_all
            || self
                .mons
                .iter()
                .any(|m| snap.fullscreen_monitors.contains(&m.spec.name));
        if any_fs != self.auto_paused {
            self.auto_paused = any_fs;
            let _ = self.ctx.ev_tx.send(OverlayEvent::AutoPaused(any_fs));
        }
        // Every monitor is either disabled or covered by a fullscreen app.
        let all_off = self.mons.iter().all(|m| {
            self.hidden
                || self.cfg.disabled_outputs.contains(&m.spec.name)
                || (self.auto_paused && (fs_all || snap.fullscreen_monitors.contains(&m.spec.name)))
        });
        let all_blanked = all_off && self.mons.iter().all(|m| m.blanked);
        let sleeping = self.paused || self.mons.is_empty() || (all_blanked && !self.play);
        self.feed.active.store(!sleeping, Ordering::Relaxed);
        self.feed.cursor_wanted.store(
            (self.cfg.cursor_wind || self.cfg.frost) && !self.play,
            Ordering::Relaxed,
        );

        let result = |s: &Self, sleep: Duration| Drive {
            sleep,
            play_changed: None,
            quit: s.quit,
            lost: false,
        };
        if self.quit {
            return result(self, Duration::ZERO);
        }
        let mut d = if sleeping {
            // Nothing is simulated or rendered; commands and the feed are
            // still checked at 10 Hz.
            self.last_frame = now;
            result(self, Duration::from_millis(100))
        } else if now < self.next_frame {
            result(self, self.next_frame - now)
        } else {
            self.frame(now, &snap);
            let iv = self.interval();
            result(self, iv.saturating_sub(now.elapsed()))
        };
        d.play_changed = self.pending_play.take();
        d
    }

    fn interval(&self) -> Duration {
        if self.cfg.fps_cap == 0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f32(1.0 / self.cfg.fps_cap.clamp(5, 360) as f32)
        }
    }

    fn frame(&mut self, now: Instant, snap: &Snapshot) {
        let dt = (now - self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;
        let iv = self.interval();
        self.next_frame = if iv.is_zero() {
            now
        } else {
            (self.next_frame + iv).max(now)
        };

        if now >= self.next_trim {
            // 5 s after start (GPU setup and shader compilation are done),
            // then every 10 minutes.
            self.next_trim = now + Duration::from_secs(600);
            platform::trim_heap();
        }
        if self.aurora_checked.elapsed() > Duration::from_secs(60) {
            self.aurora_checked = now;
            let h = platform::local_hour();
            self.hour = h;
            self.month = platform::local_month();
            self.aurora = if self.cfg.aurora && !(6..18).contains(&h) {
                1.0
            } else {
                0.0
            };
        }
        if !self.cfg.aurora {
            self.aurora = 0.0;
        }
        self.apply_feed(snap);
        let changed = self.weather.step(dt, self.cfg.weather, self.cfg.wind);
        if changed || now >= self.forecast_at {
            self.forecast_at = now + Duration::from_secs(20);
            let text = if self.cfg.weather {
                self.weather.forecast(self.cfg.season.resolve(self.month))
            } else {
                String::new()
            };
            let _ = self.ctx.ev_tx.send(OverlayEvent::Forecast(text));
        }

        let beats = self.ctx.levels.beats();
        let new_beats = beats.wrapping_sub(self.last_beats);
        self.last_beats = beats;
        let bass = self.ctx.levels.bass();

        let (play, tool, radius) = (self.play, self.tool, self.radius);
        let ptr = (self.ptr_mon, self.ptr);
        let drag = self.decor_drag;
        let carrying = self.penguin.is_some();
        let export = std::mem::take(&mut self.export_picture);
        let mut pictures: Vec<Picture> = Vec::new();
        let held = self.left_down && self.stroke;
        let aurora = self.aurora;
        let sparkle = self.cfg.sparkles;
        let (auto, fs_all) = (self.auto_paused, snap.fullscreen_any);
        let hidden = self.hidden;

        // Which monitors are showing snow this frame, and which screen
        // sides lead to one of them (penguins and snowballs walk/fly across).
        let off: Vec<bool> = self
            .mons
            .iter()
            .map(|m| {
                hidden
                    || self.cfg.disabled_outputs.contains(&m.spec.name)
                    || (auto && (fs_all || snap.fullscreen_monitors.contains(&m.spec.name)))
            })
            .collect();
        let sides: Vec<(Option<usize>, Option<usize>)> = (0..self.mons.len())
            .map(|i| {
                if off[i] {
                    (None, None)
                } else {
                    (
                        neighbor(&self.mons, &off, i, false),
                        neighbor(&self.mons, &off, i, true),
                    )
                }
            })
            .collect();
        let shared = self.mons.len() > 1;
        for (i, m) in self.mons.iter_mut().enumerate() {
            m.world.open_sides = (sides[i].0.is_some(), sides[i].1.is_some());
            m.world.shared_penguins = shared;
            m.world.target = (play && ptr.0 == Some(i)).then_some(ptr.1);
        }
        if shared {
            self.balance_penguins(&off);
        }

        for (i, m) in self.mons.iter_mut().enumerate() {
            let off = self.cfg.disabled_outputs.contains(&m.spec.name)
                || (auto && (fs_all || snap.fullscreen_monitors.contains(&m.spec.name)));
            if off {
                if !m.blanked {
                    m.blanked = true;
                    // A fullscreen game or a disabled monitor: hand the
                    // swapchain memory back where the platform allows it.
                    if self.shrink_when_blank {
                        m.renderer.blank_and_sleep();
                    } else {
                        m.renderer.blank();
                    }
                }
                continue;
            }
            m.blanked = false;
            m.world.play = play;
            m.world.season = self.cfg.season.resolve(self.month);
            m.world.weather = self.weather.fx;
            m.world.night = self.cfg.penguin_village && (self.hour >= 21 || self.hour < 7);
            let sc = m.spec.scale;
            if new_beats > 0 {
                m.world.beat((bass * 4.0).clamp(0.3, 1.2));
            }
            if held
                && ptr.0 == Some(i)
                && m.world.rolling.is_none()
                && let Tool::Brush | Tool::Shovel | Tool::Pack | Tool::Freeze | Tool::Melt = tool
            {
                m.world.apply_tool(tool, ptr.1.0, ptr.1.1, radius * sc);
            }
            m.world.advance(dt);
            m.world.build_instances(&mut m.instances);
            if play {
                toolbar::draw(
                    &mut m.instances,
                    &m.toolbar,
                    tool,
                    (radius - 6.0) / 194.0,
                    sc,
                    (ptr.0 == Some(i) && drag.is_none() && !carrying).then_some(ptr.1),
                );
                if ptr.0 == Some(i) && !carrying {
                    let r = radius * sc;
                    if matches!(
                        tool,
                        Tool::Brush | Tool::Shovel | Tool::Pack | Tool::Freeze | Tool::Melt
                    ) {
                        m.instances.push(Instance {
                            pos: [ptr.1.0, ptr.1.1],
                            half: [r, r],
                            rot: 0.0,
                            shape: SHAPE_RING,
                            color: premul([0.48, 0.64, 0.97], 0.9),
                        });
                    }
                    if let Some((k, c)) = drag {
                        toolbar::icon(
                            &mut m.instances,
                            toolbar::Action::Decor(k, c),
                            ptr.1.0,
                            ptr.1.1,
                            40.0 * sc,
                        );
                    }
                }
            }
            #[cfg(debug_assertions)]
            if let Some(d) = self.demo.as_ref()
                && d.monitor() == m.spec.name
                && play
            {
                Self::demo_cursor(&mut m.instances, d.cursor.0, d.cursor.1, sc);
            }
            let (cell, time) = (m.world.cell, m.world.time);
            m.renderer.render(
                &mut m.world.grid,
                &mut m.world.frost,
                cell,
                &m.instances,
                time,
                aurora,
                sparkle,
            );
            if export {
                let frosty = m.world.frost.any;
                if let Some(img) =
                    m.renderer
                        .capture(&m.world.grid, cell, m.instances.len(), aurora, frosty)
                {
                    pictures.push((m.spec.name.clone(), img));
                }
            }
        }
        if export {
            save_pictures(pictures, self.ctx.ev_tx.clone());
        }
        self.hand_over(&sides);
        self.check_records();
        let _ = (SHAPE_ELLIPSE, SHAPE_RECT, SHAPE_TRI, SHAPE_STAR);
    }
}

#[cfg(debug_assertions)]
impl Engine {
    /// Fault injection for testing overlay recovery (debug builds only):
    /// SNOWDESK_FAULT=panic|lost|hang fires 3 s after the overlay starts,
    /// once per process.
    fn debug_fault(&mut self) {
        static FIRED: AtomicBool = AtomicBool::new(false);
        let Some(kind) = std::env::var_os("SNOWDESK_FAULT") else {
            return;
        };
        if self.ctx.health.start.elapsed() < Duration::from_secs(3)
            || FIRED.swap(true, Ordering::Relaxed)
        {
            return;
        }
        match kind.to_str() {
            Some("panic") => panic!("injected fault"),
            Some("lost") => self.gpu.lost.store(true, Ordering::Relaxed),
            Some("hang") => {
                self.set_play(true);
                self.ctx.health.beat();
                loop {
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
            _ => {}
        }
    }
}

impl Engine {
    /// Compare snowmen and landed throws with the records; celebrate and
    /// save new ones.
    fn check_records(&mut self) {
        let mut beat = None;
        for m in &mut self.mons {
            if let Some((h, x, y)) = m.world.tallest
                && self.records.snowman(h)
            {
                m.world.cheer(x, y);
                beat = Some("New record: tallest snowman!");
            }
            for (d, x, y) in std::mem::take(&mut m.world.throws_landed) {
                if self.records.throw(d) {
                    m.world.cheer(x, y);
                    beat = Some("New record: longest throw!");
                }
            }
        }
        if beat.is_some() {
            self.records.save();
            self.send_records(beat);
        }
    }

    fn send_records(&self, beat: Option<&'static str>) {
        let _ = self
            .ctx
            .ev_tx
            .send(OverlayEvent::Records(self.records.describe(), beat));
    }
}

/// Encode and save captured monitors as PNGs off the overlay thread.
/// A captured monitor: name and (width, height, RGBA pixels).
type Picture = (String, (u32, u32, Vec<u8>));

fn save_pictures(pics: Vec<Picture>, ev: Sender<OverlayEvent>) {
    std::thread::spawn(move || {
        let dir = platform::export_dir();
        let stamp = platform::local_stamp();
        let mut saved = Vec::new();
        for (name, (w, h, rgba)) in pics {
            let safe: String = name
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '-' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            let path = dir.join(format!("snowdesk-{stamp}-{safe}.png"));
            let ok = std::fs::create_dir_all(&dir).is_ok()
                && image::RgbaImage::from_raw(w, h, rgba)
                    .is_some_and(|img| img.save(&path).is_ok());
            if ok {
                saved.push(path);
            }
        }
        let msg = match saved.as_slice() {
            [] => "Could not save the picture".to_string(),
            [one] => format!("Picture saved to {}", one.display()),
            many => format!("{} pictures saved to {}", many.len(), dir.display()),
        };
        let _ = ev.send(OverlayEvent::Notice(msg));
    });
}

/// The active monitor touching side `right` of monitor `i` (layout units).
fn neighbor(mons: &[Mon], off: &[bool], i: usize, right: bool) -> Option<usize> {
    let a = &mons[i].spec;
    mons.iter().enumerate().find_map(|(j, m)| {
        let b = &m.spec;
        let touching = if right {
            (b.gx - (a.gx + a.lw)).abs() < 2.0
        } else {
            (a.gx - (b.gx + b.lw)).abs() < 2.0
        };
        let overlap = b.gy < a.gy + a.lh && b.gy + b.lh > a.gy;
        (j != i && !off[j] && touching && overlap).then_some(j)
    })
}

impl Engine {
    /// Penguins that walked or were thrown off one monitor appear on the
    /// next one, at the same height in the desktop layout.
    fn hand_over(&mut self, sides: &[(Option<usize>, Option<usize>)]) {
        for i in 0..self.mons.len() {
            let critters = std::mem::take(&mut self.mons[i].world.leaving);
            let balls = std::mem::take(&mut self.mons[i].world.leaving_balls);
            for (c, right) in critters {
                let to = if right { sides[i].1 } else { sides[i].0 };
                if let Some(j) = to {
                    let (x, y, ratio) = self.map_point(i, j, right, c.y);
                    let mut c = c;
                    c.x = x;
                    c.y = y;
                    c.vx *= ratio;
                    c.vy *= ratio;
                    let w = &mut self.mons[j].world;
                    if c.state != crate::sim::world::CritterState::Air {
                        // Land on the new monitor's ground at that height.
                        c.state = crate::sim::world::CritterState::Air;
                        c.air_time = 0.3;
                        c.vy = 0.0;
                    }
                    w.critters.push(c);
                }
            }
            for (b, right) in balls {
                let to = if right { sides[i].1 } else { sides[i].0 };
                if let Some(j) = to {
                    let (x, y, ratio) = self.map_point(i, j, right, b.y);
                    let mut b = b;
                    b.x = x;
                    b.y = y;
                    b.vx *= ratio;
                    b.vy *= ratio;
                    b.r *= ratio;
                    let w = &mut self.mons[j].world;
                    if w.balls.len() < crate::sim::world::MAX_BALLS {
                        w.balls.push(b);
                    }
                }
            }
        }
    }

    /// Entry point on monitor `j` for something leaving monitor `i` at
    /// height `y`; also the pixel ratio between the two.
    fn map_point(&self, i: usize, j: usize, right: bool, y: f32) -> (f32, f32, f32) {
        let (a, b) = (&self.mons[i].spec, &self.mons[j].spec);
        let gy = a.gy + y / a.feed_scale;
        let ly = ((gy - b.gy) * b.feed_scale).clamp(0.0, b.h as f32 - 1.0);
        let x = if right { 2.0 } else { b.w as f32 - 2.0 };
        (x, ly, b.scale / a.scale)
    }

    /// Keep `penguins` per showing monitor in total; they wander between
    /// monitors, so add new ones where there are fewest.
    fn balance_penguins(&mut self, off: &[bool]) {
        if !self.cfg.critter {
            return;
        }
        let active: Vec<usize> = (0..self.mons.len()).filter(|&i| !off[i]).collect();
        if active.is_empty() {
            return;
        }
        let want =
            self.cfg.penguins.clamp(1, crate::sim::world::MAX_PENGUINS) as usize * active.len();
        let count = |e: &Self| {
            active
                .iter()
                .map(|&i| e.mons[i].world.critters.len())
                .sum::<usize>()
        };
        while count(self) < want {
            let &i = active
                .iter()
                .min_by_key(|&&i| self.mons[i].world.critters.len())
                .expect("non-empty");
            let c = self.mons[i].world.spawn_critter();
            self.mons[i].world.critters.push(c);
        }
        while count(self) > want {
            let &i = active
                .iter()
                .max_by_key(|&&i| self.mons[i].world.critters.len())
                .expect("non-empty");
            let w = &mut self.mons[i].world;
            match w
                .critters
                .iter()
                .rposition(|c| c.state != crate::sim::world::CritterState::Held)
            {
                Some(k) => {
                    w.critters.remove(k);
                }
                None => break,
            }
        }
        // A carried penguin's index must stay valid: drop the grab if the
        // list it points into shrank.
        if let Some(g) = self.penguin
            && self
                .mons
                .get(g.mon)
                .is_none_or(|m| g.idx >= m.world.critters.len())
        {
            self.penguin = None;
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        // Stop the desktop feed thread started in `Engine::new`.
        self.feed.stop.store(true, Ordering::Relaxed);
        self.ctx.health.play.store(false, Ordering::Relaxed);
    }
}

/// Which overlay implementation this session uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    /// wlr-layer-shell surfaces driven by the overlay thread itself.
    #[cfg(target_os = "linux")]
    Wayland,
    /// winit windows hosted on the main thread (X11, Windows).
    Winit,
}

/// Decide the backend before any thread or window exists. On a Wayland
/// session without layer-shell (GNOME) with XWayland available, the whole
/// app, panel included, is switched to X11 so both share one winit loop.
pub fn choose_backend() -> Backend {
    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            match wayland::probe() {
                Ok(()) => return Backend::Wayland,
                Err(e) if std::env::var_os("DISPLAY").is_some() => {
                    tracing::warn!("{e:#}; using X11/XWayland for the overlay and panel");
                    // SAFETY: called at the very start of `main`, before any
                    // other thread exists.
                    unsafe { std::env::remove_var("WAYLAND_DISPLAY") };
                }
                Err(e) => {
                    tracing::error!("{e:#}");
                    return Backend::Wayland;
                }
            }
        }
    }
    Backend::Winit
}

/// Overlay thread entry. A panic or GPU loss tears the overlay down
/// (surfaces and windows are dropped while unwinding, so no frozen,
/// input-blocking overlay can stay behind) and it is rebuilt a few times
/// before giving up.
pub fn run(
    ctx: OverlayCtx,
    backend: Backend,
    #[cfg(any(windows, target_os = "linux"))] host_rx: Option<Receiver<HostMsg>>,
) {
    let ev = ctx.ev_tx.clone();
    #[cfg(any(windows, target_os = "linux"))]
    let mut wins = winit_common::Windows::default();
    let mut restarts: VecDeque<Instant> = VecDeque::new();
    loop {
        let attempt = ctx.clone();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match backend {
            #[cfg(target_os = "linux")]
            Backend::Wayland => wayland::run(attempt),
            Backend::Winit => match &host_rx {
                Some(rx) => winit_common::run(attempt, rx, &mut wins),
                None => Err(anyhow::anyhow!("no window host")),
            },
        }));
        ctx.health.play.store(false, Ordering::Relaxed);
        let msg = match r {
            Ok(Ok(())) => return,
            Ok(Err(e)) if e.to_string() == DEVICE_LOST => e.to_string(),
            Ok(Err(e)) => {
                tracing::error!("overlay failed: {e:#}");
                let _ = ev.send(OverlayEvent::Fatal(format!("{e:#}")));
                return;
            }
            Err(p) => {
                let msg = p
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| p.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "unknown panic".into());
                tracing::error!("overlay panicked: {msg}");
                format!("crash: {msg}")
            }
        };
        let now = Instant::now();
        restarts.retain(|t| now - *t < Duration::from_secs(120));
        if restarts.len() >= 3 {
            let _ = ev.send(OverlayEvent::Fatal(format!(
                "Overlay stopped after repeated failures ({msg})"
            )));
            return;
        }
        restarts.push_back(now);
        let _ = ev.send(OverlayEvent::Error(format!(
            "Overlay restarted after {msg}"
        )));
        let _ = ev.send(OverlayEvent::Play(false));
        std::thread::sleep(Duration::from_secs(1));
    }
}
