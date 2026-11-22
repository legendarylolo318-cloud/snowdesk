//! winit backend for X11 and Windows: one borderless, transparent,
//! always-on-top window per monitor.
//!
//! winit allows a single event loop per process and the Slint panel already
//! runs one, so the overlay windows live on the main thread inside that loop
//! ([`Host`], hooked in through Slint's custom application handler, or a
//! small standalone loop in `--no-panel` mode). Windows are `Send + Sync`, so
//! they are handed to the overlay thread, which owns the GPU surfaces, renders
//! and toggles click-through. Input events are forwarded over a channel.

use super::{Button, Engine, MonSpec, OverlayCtx, OverlayEvent, Ptr};
use crate::render::Gpu;
use anyhow::{Result, anyhow};
use crossbeam_channel::{Receiver, Sender, select};
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::monitor::MonitorHandle;
use winit::window::{Window, WindowAttributes, WindowId, WindowLevel};

/// Main thread → overlay thread.
pub enum HostMsg {
    Add { spec: MonSpec, window: Arc<Window> },
    Resize(MonSpec),
    Remove(String),
    Ptr(String, Ptr),
}

fn attrs(pos: PhysicalPosition<i32>, size: PhysicalSize<u32>) -> WindowAttributes {
    let a = Window::default_attributes()
        .with_title("snowdesk-overlay")
        .with_decorations(false)
        .with_transparent(true)
        .with_resizable(false)
        .with_active(false)
        .with_window_level(WindowLevel::AlwaysOnTop)
        .with_position(pos)
        .with_inner_size(size);
    #[cfg(target_os = "linux")]
    let a = {
        use winit::platform::x11::{WindowAttributesExtX11, WindowType};
        a.with_override_redirect(true)
            .with_x11_window_type(vec![WindowType::Dock])
    };
    #[cfg(windows)]
    let a = {
        use winit::platform::windows::WindowAttributesExtWindows;
        // DirectComposition draws the swapchain; no GDI redirection surface.
        a.with_skip_taskbar(true).with_no_redirection_bitmap(true)
    };
    a
}

fn mon_name(i: usize, m: &MonitorHandle) -> String {
    m.name().unwrap_or_else(|| format!("monitor{i}"))
}

fn spec_for(name: String, m: &MonitorHandle) -> MonSpec {
    let (pos, size) = (m.position(), m.size());
    MonSpec {
        name,
        gx: pos.x as f32,
        gy: pos.y as f32,
        lw: size.width as f32,
        lh: size.height as f32,
        feed_scale: 1.0,
        w: size.width,
        h: size.height,
        scale: m.scale_factor() as f32,
    }
}

struct HostWin {
    id: WindowId,
    window: Arc<Window>,
    spec: MonSpec,
    last: (f32, f32),
}

/// Owns the overlay windows on the main thread.
pub struct Host {
    tx: Sender<HostMsg>,
    ev_tx: Sender<OverlayEvent>,
    wins: Vec<HostWin>,
    last_scan: Option<Instant>,
}

impl Host {
    pub fn new(tx: Sender<HostMsg>, ev_tx: Sender<OverlayEvent>) -> Self {
        Self {
            tx,
            ev_tx,
            wins: Vec::new(),
            last_scan: None,
        }
    }

    /// Create/remove/resize windows to match the monitors, at most every 2 s.
    pub fn maybe_scan(&mut self, el: &ActiveEventLoop) {
        if self
            .last_scan
            .is_some_and(|t| t.elapsed() < Duration::from_secs(2))
        {
            return;
        }
        self.last_scan = Some(Instant::now());
        self.scan(el);
    }

    fn scan(&mut self, el: &ActiveEventLoop) {
        let mons: Vec<_> = el.available_monitors().collect();
        let names: Vec<String> = mons
            .iter()
            .enumerate()
            .map(|(i, m)| mon_name(i, m))
            .collect();
        self.wins.retain(|w| {
            let keep = names.contains(&w.spec.name);
            if !keep {
                let _ = self.tx.send(HostMsg::Remove(w.spec.name.clone()));
            }
            keep
        });
        for (m, name) in mons.iter().zip(names) {
            let spec = spec_for(name, m);
            if let Some(w) = self.wins.iter_mut().find(|w| w.spec.name == spec.name) {
                let same = (w.spec.gx, w.spec.gy, w.spec.w, w.spec.h, w.spec.scale)
                    == (spec.gx, spec.gy, spec.w, spec.h, spec.scale);
                if !same {
                    w.window.set_outer_position(m.position());
                    let _ = w.window.request_inner_size(m.size());
                    w.spec = spec.clone();
                    let _ = self.tx.send(HostMsg::Resize(spec));
                }
                continue;
            }
            match el.create_window(attrs(m.position(), m.size())) {
                Ok(window) => {
                    let window = Arc::new(window);
                    tracing::debug!(
                        "overlay window {} requested {:?} {:?}, got {:?} {:?}",
                        spec.name,
                        m.position(),
                        m.size(),
                        window.outer_position(),
                        window.inner_size()
                    );
                    let _ = window.set_cursor_hittest(false);
                    #[cfg(windows)]
                    super::windows::harden(&window);
                    self.wins.push(HostWin {
                        id: window.id(),
                        window: window.clone(),
                        spec: spec.clone(),
                        last: (0.0, 0.0),
                    });
                    let _ = self.tx.send(HostMsg::Add { spec, window });
                }
                Err(e) => {
                    let _ = self.ev_tx.send(OverlayEvent::Error(format!(
                        "Cannot create overlay window on {}: {e}",
                        spec.name
                    )));
                }
            }
        }
    }

    /// Handle an event for one of our windows. Returns false for other windows.
    pub fn window_event(&mut self, id: WindowId, ev: &WindowEvent) -> bool {
        let Some(w) = self.wins.iter_mut().find(|w| w.id == id) else {
            return false;
        };
        let name = || w.spec.name.clone();
        let msg = match ev {
            WindowEvent::CursorMoved { position, .. } => {
                w.last = (position.x as f32, position.y as f32);
                Some(Ptr::Move(w.last.0, w.last.1))
            }
            WindowEvent::CursorLeft { .. } => Some(Ptr::Leave),
            WindowEvent::MouseInput { state, button, .. } => {
                let b = match button {
                    MouseButton::Left => Some(Button::Left),
                    MouseButton::Right => Some(Button::Right),
                    _ => None,
                };
                let (x, y) = w.last;
                b.map(|b| match state {
                    ElementState::Pressed => Ptr::Down(b, x, y),
                    ElementState::Released => Ptr::Up(b, x, y),
                })
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                match &event.logical_key {
                    Key::Named(NamedKey::Escape) => Some(Ptr::Escape),
                    Key::Character(c) => match c.as_str().as_bytes() {
                        [d @ b'1'..=b'9'] => Some(Ptr::Digit(d - b'0')),
                        _ => None,
                    },
                    _ => None,
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let n = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => *y,
                    winit::event::MouseScrollDelta::PixelDelta(p) => p.y as f32 / 40.0,
                };
                (n != 0.0).then_some(Ptr::Wheel(n))
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                // Keep covering the whole monitor; the next scan updates the spec.
                let _ = w
                    .window
                    .request_inner_size(PhysicalSize::new(w.spec.w, w.spec.h));
                self.last_scan = None;
                None
            }
            WindowEvent::Resized(size) => {
                if (size.width, size.height) != (w.spec.w, w.spec.h)
                    && size.width > 0
                    && size.height > 0
                {
                    w.spec.w = size.width;
                    w.spec.h = size.height;
                    let _ = self.tx.send(HostMsg::Resize(w.spec.clone()));
                }
                None
            }
            _ => None,
        };
        if let Some(p) = msg {
            let _ = self.tx.send(HostMsg::Ptr(name(), p));
        }
        true
    }
}

/// `--no-panel` mode: a plain winit loop on the main thread hosting the
/// overlay windows. Returns when `quit` reports true.
pub fn run_standalone(host: Host, quit: impl FnMut() -> bool + 'static) -> Result<()> {
    use winit::application::ApplicationHandler;
    use winit::event_loop::{ControlFlow, EventLoop};
    struct App<Q> {
        host: Host,
        quit: Q,
    }
    impl<Q: FnMut() -> bool> ApplicationHandler for App<Q> {
        fn resumed(&mut self, el: &ActiveEventLoop) {
            self.host.maybe_scan(el);
        }
        fn window_event(&mut self, _: &ActiveEventLoop, id: WindowId, ev: WindowEvent) {
            self.host.window_event(id, &ev);
        }
        fn about_to_wait(&mut self, el: &ActiveEventLoop) {
            if (self.quit)() {
                el.exit();
                return;
            }
            self.host.maybe_scan(el);
            el.set_control_flow(ControlFlow::WaitUntil(
                Instant::now() + Duration::from_millis(250),
            ));
        }
    }
    let el = EventLoop::new().map_err(|e| anyhow!("event loop: {e}"))?;
    el.run_app(&mut App { host, quit })
        .map_err(|e| anyhow!("event loop: {e}"))
}

// ---- overlay thread -------------------------------------------------------

/// Windows received from the host. Kept across overlay restarts.
#[derive(Default)]
pub struct Windows {
    list: Vec<(MonSpec, Arc<Window>)>,
}

/// What wgpu gets to build a surface from. This runs on the overlay thread,
/// so on Windows the window must be wrapped to hand out its handle there.
#[cfg(windows)]
fn surface_target(w: &Arc<Window>) -> super::windows::AnyThread {
    super::windows::AnyThread(w.clone())
}
#[cfg(not(windows))]
fn surface_target(w: &Arc<Window>) -> Arc<Window> {
    w.clone()
}

fn attach(
    engine: &mut Option<Engine>,
    ctx: &OverlayCtx,
    spec: MonSpec,
    window: &Arc<Window>,
) -> Result<()> {
    match engine {
        None => {
            let (gpu, surf) = Gpu::new(|inst| Ok(inst.create_surface(surface_target(window))?))?;
            let mut e = Engine::new(Arc::new(gpu), ctx.clone());
            e.add_monitor(spec, surf)?;
            *engine = Some(e);
        }
        Some(e) => {
            let surf = e.gpu().instance.create_surface(surface_target(window))?;
            e.add_monitor(spec, surf)?;
        }
    }
    Ok(())
}

fn set_input(wins: &Windows, play: bool) {
    for (_, w) in &wins.list {
        let _ = w.set_cursor_hittest(play);
        #[cfg(windows)]
        super::windows::after_hittest(w);
        if play {
            w.focus_window();
        }
    }
}

/// Overlay-thread loop for the winit backends.
pub fn run(ctx: OverlayCtx, host_rx: &Receiver<HostMsg>, wins: &mut Windows) -> Result<()> {
    let mut engine: Option<Engine> = None;
    for (spec, w) in wins.list.clone() {
        attach(&mut engine, &ctx, spec, &w)?;
    }
    set_input(wins, false);
    #[cfg(target_os = "linux")]
    let mut esc = super::x11::EscapeGrab::new();
    let cmd_rx = ctx.cmd_rx.clone();
    let handle = |m: HostMsg, engine: &mut Option<Engine>, wins: &mut Windows| -> Result<()> {
        match m {
            HostMsg::Add { spec, window } => {
                wins.list.retain(|(s, _)| s.name != spec.name);
                wins.list.push((spec.clone(), window.clone()));
                attach(engine, &ctx, spec, &window)?;
            }
            HostMsg::Resize(spec) => {
                if let Some(x) = wins.list.iter_mut().find(|(s, _)| s.name == spec.name) {
                    x.0 = spec.clone();
                }
                if let Some(e) = engine.as_mut() {
                    e.resize_monitor(spec);
                }
            }
            HostMsg::Remove(name) => {
                // Surface first, then our window reference.
                if let Some(e) = engine.as_mut() {
                    e.remove_monitor(&name);
                }
                wins.list.retain(|(s, _)| s.name != name);
            }
            HostMsg::Ptr(name, p) => {
                if let Some(e) = engine.as_mut() {
                    e.pointer(&name, p);
                }
            }
        }
        Ok(())
    };
    loop {
        while let Ok(m) = host_rx.try_recv() {
            handle(m, &mut engine, wins)?;
        }
        #[cfg(target_os = "linux")]
        for p in esc.pressed() {
            if let (Some(e), Some((s, _))) = (engine.as_mut(), wins.list.first()) {
                e.pointer(&s.name, p);
            }
        }
        let d = match engine.as_mut() {
            Some(e) => e.drive(),
            None => super::Drive::idle(),
        };
        if d.lost {
            return Err(anyhow!(super::DEVICE_LOST));
        }
        if d.quit {
            return Ok(());
        }
        if let Some(p) = d.play_changed {
            set_input(wins, p);
            #[cfg(target_os = "linux")]
            esc.set(p);
        }
        select! {
            recv(host_rx) -> m => match m {
                Ok(m) => handle(m, &mut engine, wins)?,
                Err(_) => return Ok(()), // host gone: the app is exiting
            },
            recv(cmd_rx) -> c => match (c, engine.as_mut()) {
                (Ok(c), Some(e)) => e.push_cmd(c),
                (Ok(super::OverlayCmd::Quit), None) | (Err(_), _) => return Ok(()),
                _ => {}
            },
            default(d.sleep) => {}
        }
    }
}
