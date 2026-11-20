//! Wayland backend: wlr-layer-shell OVERLAY surfaces, one per output.
//! Passive mode uses an empty input region (fully click-through); play mode
//! uses the full region and exclusive keyboard focus so Esc works.

use super::{Button, Drive, Engine, MonSpec, OverlayCtx, Ptr};
use crate::render::Gpu;
use anyhow::{Context, Result, anyhow};
use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
};
use smithay_client_toolkit::reexports::client::{
    Connection, Proxy, QueueHandle,
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_surface},
};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Region},
    delegate_registry,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers},
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
    },
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
    },
};
use std::os::fd::AsRawFd;
use std::ptr::NonNull;
use std::sync::Arc;
use std::time::Duration;
use wayland_backend::client::ObjectId;

struct Out {
    output: wl_output::WlOutput,
    layer: LayerSurface,
    name: String,
    size: (u32, u32),
    scale: i32,
    live: bool,
}

struct App {
    registry: RegistryState,
    outputs_state: OutputState,
    seats: SeatState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    conn: Connection,
    outs: Vec<Out>,
    engine: Option<Engine>,
    gpu: Option<Arc<Gpu>>,
    ctx: OverlayCtx,
    pointer: Option<wl_pointer::WlPointer>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    play: bool,
    error: Option<anyhow::Error>,
}

fn raw_handles(
    conn: &Connection,
    surf: &wl_surface::WlSurface,
) -> Result<(RawDisplayHandle, RawWindowHandle)> {
    let d = NonNull::new(conn.backend().display_ptr() as *mut _)
        .ok_or_else(|| anyhow!("null wl_display"))?;
    let id: ObjectId = surf.id();
    let s = NonNull::new(id.as_ptr() as *mut _).ok_or_else(|| anyhow!("null wl_surface"))?;
    Ok((
        RawDisplayHandle::Wayland(WaylandDisplayHandle::new(d)),
        RawWindowHandle::Wayland(WaylandWindowHandle::new(s)),
    ))
}

impl App {
    fn spawn_surface(&mut self, qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
        if self.outs.iter().any(|o| o.output == output) {
            return;
        }
        let Some(info) = self.outputs_state.info(&output) else {
            return;
        };
        let Some(name) = info.name.clone() else {
            return;
        };
        let surface = self.compositor.create_surface(qh);
        let layer = self.layer_shell.create_layer_surface(
            qh,
            surface,
            Layer::Overlay,
            Some("snowdesk"),
            Some(&output),
        );
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_size(0, 0);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        if let Ok(empty) = Region::new(&self.compositor) {
            layer.set_input_region(Some(empty.wl_region()));
        }
        layer.commit();
        self.outs.push(Out {
            output,
            layer,
            name,
            size: (0, 0),
            scale: info.scale_factor.max(1),
            live: false,
        });
    }

    fn set_input(&mut self, play: bool) {
        self.play = play;
        for o in &self.outs {
            if play {
                o.layer.set_input_region(None);
                o.layer
                    .set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
            } else if let Ok(empty) = Region::new(&self.compositor) {
                o.layer.set_input_region(Some(empty.wl_region()));
                o.layer
                    .set_keyboard_interactivity(KeyboardInteractivity::None);
            }
            o.layer.commit();
        }
    }

    fn spec_for(&self, i: usize) -> MonSpec {
        let o = &self.outs[i];
        let info = self.outputs_state.info(&o.output);
        let pos = info
            .as_ref()
            .and_then(|i| i.logical_position)
            .unwrap_or((0, 0));
        let scale = o.scale as f32;
        MonSpec {
            name: o.name.clone(),
            gx: pos.0 as f32,
            gy: pos.1 as f32,
            lw: o.size.0 as f32,
            lh: o.size.1 as f32,
            feed_scale: scale,
            w: o.size.0 * o.scale as u32,
            h: o.size.1 * o.scale as u32,
            scale,
        }
    }

    /// Remove an output: the GPU surface must go before the wl_surface.
    fn drop_out(&mut self, i: usize) {
        let name = self.outs[i].name.clone();
        if let Some(e) = self.engine.as_mut() {
            e.remove_monitor(&name);
        }
        self.outs.remove(i);
    }

    fn out_by_surface(&self, s: &wl_surface::WlSurface) -> Option<usize> {
        self.outs.iter().position(|o| o.layer.wl_surface() == s)
    }

    fn setup_monitor(&mut self, i: usize) -> Result<()> {
        let spec = self.spec_for(i);
        let (d, w) = raw_handles(&self.conn, self.outs[i].layer.wl_surface())?;
        let _ = self.outs[i]
            .layer
            .set_buffer_scale(self.outs[i].scale as u32);
        if let Some(e) = self.engine.as_mut() {
            let gpu = e.gpu();
            let surf = unsafe {
                gpu.instance
                    .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                        raw_display_handle: Some(d),
                        raw_window_handle: w,
                    })?
            };
            e.add_monitor(spec, surf)?;
        } else {
            let (gpu, surf) = Gpu::new(|inst| unsafe {
                Ok(
                    inst.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                        raw_display_handle: Some(d),
                        raw_window_handle: w,
                    })?,
                )
            })?;
            let gpu = Arc::new(gpu);
            self.gpu = Some(gpu.clone());
            let mut e = Engine::new(gpu, self.ctx.clone());
            e.shrink_when_blank = true;
            e.add_monitor(spec, surf)?;
            self.engine = Some(e);
        }
        self.outs[i].live = true;
        if self.play {
            self.set_input(true);
        }
        Ok(())
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        s: &wl_surface::WlSurface,
        f: i32,
    ) {
        if let Some(i) = self.out_by_surface(s) {
            self.outs[i].scale = f.max(1);
            let _ = self.outs[i].layer.set_buffer_scale(f.max(1) as u32);
            if self.outs[i].live {
                let spec = self.spec_for(i);
                if let Some(e) = self.engine.as_mut() {
                    e.resize_monitor(spec);
                }
            }
        }
    }
    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}
    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs_state
    }
    fn new_output(&mut self, _: &Connection, qh: &QueueHandle<Self>, o: wl_output::WlOutput) {
        self.spawn_surface(qh, o);
    }
    fn update_output(&mut self, _: &Connection, qh: &QueueHandle<Self>, o: wl_output::WlOutput) {
        self.spawn_surface(qh, o);
    }
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, o: wl_output::WlOutput) {
        if let Some(i) = self.outs.iter().position(|x| x.output == o) {
            self.drop_out(i);
        }
    }
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        if let Some(i) = self.outs.iter().position(|o| &o.layer == layer) {
            self.drop_out(i);
        }
    }
    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        layer: &LayerSurface,
        c: LayerSurfaceConfigure,
        _: u32,
    ) {
        let Some(i) = self.outs.iter().position(|o| &o.layer == layer) else {
            return;
        };
        let (w, h) = c.new_size;
        if w == 0 || h == 0 {
            return;
        }
        self.outs[i].size = (w, h);
        let res = if self.outs[i].live {
            let spec = self.spec_for(i);
            if let Some(e) = self.engine.as_mut() {
                e.resize_monitor(spec);
            }
            Ok(())
        } else {
            self.setup_monitor(i)
        };
        if let Err(e) = res {
            self.error = Some(e);
        }
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seats
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        cap: Capability,
    ) {
        if cap == Capability::Pointer && self.pointer.is_none() {
            self.pointer = self.seats.get_pointer(qh, &seat).ok();
        }
        if cap == Capability::Keyboard && self.keyboard.is_none() {
            self.keyboard = self.seats.get_keyboard(qh, &seat, None).ok();
        }
    }
    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        cap: Capability,
    ) {
        if cap == Capability::Pointer {
            self.pointer = None;
        }
        if cap == Capability::Keyboard {
            self.keyboard = None;
        }
    }
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl PointerHandler for App {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        let Some(e) = self.engine.as_mut() else {
            return;
        };
        for ev in events {
            let Some(i) = self
                .outs
                .iter()
                .position(|o| o.layer.wl_surface() == &ev.surface)
            else {
                continue;
            };
            let s = self.outs[i].scale as f32;
            let (x, y) = (ev.position.0 as f32 * s, ev.position.1 as f32 * s);
            let name = self.outs[i].name.clone();
            let p = match ev.kind {
                PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => Ptr::Move(x, y),
                PointerEventKind::Leave { .. } => Ptr::Leave,
                PointerEventKind::Press { button, .. } => match button {
                    272 => Ptr::Down(Button::Left, x, y),
                    273 => Ptr::Down(Button::Right, x, y),
                    _ => continue,
                },
                PointerEventKind::Release { button, .. } => match button {
                    272 => Ptr::Up(Button::Left, x, y),
                    273 => Ptr::Up(Button::Right, x, y),
                    _ => continue,
                },
                _ => continue,
            };
            e.pointer(&name, p);
        }
    }
}

impl KeyboardHandler for App {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
    }
    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
    ) {
    }
    fn press_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        ev: KeyEvent,
    ) {
        if ev.keysym == Keysym::Escape
            && let (Some(e), Some(o)) = (self.engine.as_mut(), self.outs.first())
        {
            e.pointer(&o.name, Ptr::Escape);
        }
    }
    fn repeat_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
    }
    fn release_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
    }
    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: Modifiers,
        _: RawModifiers,
        _: u32,
    ) {
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState, SeatState];
}

impl Drop for App {
    fn drop(&mut self) {
        // GPU surfaces reference the wl_surfaces; destroy them first (this
        // also runs while unwinding from a panic).
        self.engine = None;
        self.outs.clear();
    }
}

delegate_registry!(App);
smithay_client_toolkit::delegate_dispatch2!(App);

/// Check that the compositor offers wlr-layer-shell.
pub fn probe() -> Result<()> {
    let conn = Connection::connect_to_env().context("connect to Wayland")?;
    let (globals, _queue) = registry_queue_init::<App>(&conn)?;
    let ok = globals
        .contents()
        .with_list(|l| l.iter().any(|g| g.interface == "zwlr_layer_shell_v1"));
    if ok {
        Ok(())
    } else {
        Err(anyhow!(
            "this Wayland compositor has no wlr-layer-shell (GNOME?)"
        ))
    }
}

pub fn run(ctx: OverlayCtx) -> Result<()> {
    let conn = Connection::connect_to_env().context("connect to Wayland")?;
    let (globals, mut queue) = registry_queue_init::<App>(&conn)?;
    let qh = queue.handle();
    let compositor = CompositorState::bind(&globals, &qh).context("wl_compositor missing")?;
    let layer_shell = LayerShell::bind(&globals, &qh)
        .context("wlr-layer-shell not supported by this compositor (GNOME?)")?;
    let mut app = App {
        registry: RegistryState::new(&globals),
        outputs_state: OutputState::new(&globals, &qh),
        seats: SeatState::new(&globals, &qh),
        compositor,
        layer_shell,
        conn: conn.clone(),
        outs: Vec::new(),
        engine: None,
        gpu: None,
        ctx,
        pointer: None,
        keyboard: None,
        play: false,
        error: None,
    };
    queue.roundtrip(&mut app)?;
    queue.roundtrip(&mut app)?;

    loop {
        queue.dispatch_pending(&mut app)?;
        if let Some(e) = app.error.take() {
            return Err(e);
        }
        let d = match app.engine.as_mut() {
            Some(e) => e.drive(),
            None => {
                // Drain commands until the first output is configured.
                while let Ok(c) = app.ctx.cmd_rx.try_recv() {
                    if matches!(c, super::OverlayCmd::Quit) {
                        return Ok(());
                    }
                }
                Drive {
                    sleep: Duration::from_millis(50),
                    ..Drive::idle()
                }
            }
        };
        if d.lost {
            return Err(anyhow!(super::DEVICE_LOST));
        }
        if d.quit {
            return Ok(());
        }
        if let Some(p) = d.play_changed {
            app.set_input(p);
        }
        queue.flush()?;
        if let Some(guard) = queue.prepare_read() {
            // Round up: truncating to 0 ms would busy-spin for the last
            // millisecond of every frame.
            let ms = d
                .sleep
                .min(Duration::from_millis(100))
                .as_micros()
                .div_ceil(1000) as i32;
            let mut pfd = libc::pollfd {
                fd: guard.connection_fd().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let r = unsafe { libc::poll(&mut pfd, 1, ms) };
            if r > 0 {
                guard.read()?;
            } else {
                drop(guard);
            }
        }
    }
}
