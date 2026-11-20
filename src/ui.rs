//! Slint control panel glue (runs on the main thread).

use crate::audio::{AudioCmd, AudioEvent, library};
use crate::config::Config;
use crate::overlay::{OverlayCmd, OverlayEvent};
use crate::tray::UiMsg;
use crossbeam_channel::{Receiver, Sender};
use slint::{
    CloseRequestResponse, ComponentHandle, Model, ModelRc, SharedString, Timer, TimerMode, VecModel,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

slint::include_modules!();

const FPS: [u32; 4] = [30, 60, 120, 0];

pub struct Channels {
    pub overlay_tx: Sender<OverlayCmd>,
    pub overlay_rx: Receiver<OverlayEvent>,
    pub audio_tx: Sender<AudioCmd>,
    pub audio_rx: Receiver<AudioEvent>,
    pub ui_tx: Sender<UiMsg>,
    pub ui_rx: Receiver<UiMsg>,
}

thread_local! {
    /// Drains all channels; installed by `run`, called on the UI thread.
    static PUMP: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
}

/// Wake the UI thread only when a message is waiting (no polling timer).
/// `Select::ready` does not consume; the pump drains on the UI thread, and
/// the forwarder waits for that before looking again.
fn spawn_forwarder(
    rx: (
        Receiver<OverlayEvent>,
        Receiver<AudioEvent>,
        Receiver<UiMsg>,
    ),
) {
    std::thread::Builder::new()
        .name("ui-wake".into())
        .spawn(move || {
            loop {
                let mut sel = crossbeam_channel::Select::new();
                sel.recv(&rx.0);
                sel.recv(&rx.1);
                sel.recv(&rx.2);
                sel.ready();
                let (ack_tx, ack_rx) = crossbeam_channel::bounded(1);
                let posted = slint::invoke_from_event_loop(move || {
                    PUMP.with(|p| {
                        if let Some(f) = p.borrow().as_ref() {
                            f();
                        }
                    });
                    let _ = ack_tx.send(());
                });
                if posted.is_err() || ack_rx.recv().is_err() {
                    break; // event loop gone
                }
                // Coalesce bursts (e.g. a library rescan plus position ticks).
                std::thread::sleep(Duration::from_millis(15));
            }
        })
        .expect("spawn ui-wake thread");
}

/// Hooks the overlay windows into Slint's winit event loop (X11, Windows).
#[cfg(any(windows, target_os = "linux"))]
struct HostHandler(crate::overlay::Host);

#[cfg(any(windows, target_os = "linux"))]
impl slint::winit_030::CustomApplicationHandler for HostHandler {
    fn resumed(
        &mut self,
        el: &slint::winit_030::winit::event_loop::ActiveEventLoop,
    ) -> slint::winit_030::EventResult {
        self.0.maybe_scan(el);
        slint::winit_030::EventResult::Propagate
    }
    fn window_event(
        &mut self,
        _el: &slint::winit_030::winit::event_loop::ActiveEventLoop,
        id: slint::winit_030::winit::window::WindowId,
        _w: Option<&slint::winit_030::winit::window::Window>,
        _s: Option<&slint::Window>,
        ev: &slint::winit_030::winit::event::WindowEvent,
    ) -> slint::winit_030::EventResult {
        if self.0.window_event(id, ev) {
            slint::winit_030::EventResult::PreventDefault
        } else {
            slint::winit_030::EventResult::Propagate
        }
    }
    fn about_to_wait(
        &mut self,
        el: &slint::winit_030::winit::event_loop::ActiveEventLoop,
    ) -> slint::winit_030::EventResult {
        self.0.maybe_scan(el);
        slint::winit_030::EventResult::Propagate
    }
}

struct Ctl {
    cfg: RefCell<Config>,
    tx: Sender<OverlayCmd>,
    tracks: Rc<VecModel<TrackRow>>,
    monitors: Rc<VecModel<MonitorRow>>,
    now: Cell<Option<usize>>,
    save_timer: Timer,
    /// Edits not yet written to disk; they win over a concurrent file change.
    unsaved: Cell<bool>,
}

/// Save the config 500 ms after the last edit.
fn schedule_save(ctl: &Rc<Ctl>) {
    ctl.unsaved.set(true);
    let c2 = ctl.clone();
    ctl.save_timer.start(
        TimerMode::SingleShot,
        Duration::from_millis(500),
        move || {
            c2.cfg.borrow().save();
            c2.unsaved.set(false);
        },
    );
}

fn fmt_time(d: Duration) -> String {
    let s = d.as_secs();
    format!("{}:{:02}", s / 60, s % 60)
}

fn read_cfg(ui: &Panel, base: &Config) -> Config {
    Config {
        intensity: ui.get_intensity(),
        flake_size: ui.get_flake_size(),
        wind: ui.get_wind(),
        max_pile: ui.get_max_pile(),
        melt_rate: ui.get_melt_rate(),
        fps_cap: FPS[ui.get_fps_index().clamp(0, 3) as usize],
        gusts: ui.get_gusts(),
        cursor_wind: ui.get_cursor_wind(),
        windows_collide: ui.get_windows_collide(),
        music_reactive: ui.get_music_reactive(),
        critter: ui.get_critter(),
        string_lights: ui.get_lights(),
        sparkles: ui.get_sparkles(),
        aurora: ui.get_aurora(),
        restore_scene: ui.get_restore_scene(),
        autostart: ui.get_autostart(),
        paused: ui.get_paused(),
        volume: ui.get_volume(),
        shuffle: ui.get_shuffle(),
        repeat: ui.get_repeat(),
        ..base.clone()
    }
}

fn apply_cfg(ui: &Panel, c: &Config) {
    ui.set_intensity(c.intensity);
    ui.set_flake_size(c.flake_size);
    ui.set_wind(c.wind);
    ui.set_max_pile(c.max_pile);
    ui.set_melt_rate(c.melt_rate);
    ui.set_fps_index(FPS.iter().position(|&f| f == c.fps_cap).unwrap_or(1) as i32);
    ui.set_gusts(c.gusts);
    ui.set_cursor_wind(c.cursor_wind);
    ui.set_windows_collide(c.windows_collide);
    ui.set_music_reactive(c.music_reactive);
    ui.set_critter(c.critter);
    ui.set_lights(c.string_lights);
    ui.set_sparkles(c.sparkles);
    ui.set_aurora(c.aurora);
    ui.set_restore_scene(c.restore_scene);
    ui.set_autostart(c.autostart);
    ui.set_paused(c.paused);
    ui.set_volume(c.volume);
    ui.set_shuffle(c.shuffle);
    ui.set_repeat(c.repeat);
}

fn open_folder(p: &std::path::Path) {
    #[cfg(windows)]
    let cmd = "explorer";
    #[cfg(not(windows))]
    let cmd = "xdg-open";
    if let Err(e) = std::process::Command::new(cmd).arg(p).spawn() {
        tracing::warn!("cannot open folder: {e}");
    }
}

/// Build the panel, wire all callbacks and run the event loop until quit.
pub fn run(
    cfg: Config,
    ch: Channels,
    start_hidden: bool,
    #[cfg(any(windows, target_os = "linux"))] host: Option<crate::overlay::Host>,
) -> Result<(), slint::PlatformError> {
    let mut sel = slint::BackendSelector::new();
    // The software renderer keeps the panel out of GPU memory; it is plenty
    // fast for this UI. Override with SLINT_RENDERER=femtovg.
    if std::env::var_os("SLINT_RENDERER").is_none() {
        sel = sel.renderer_name("software".into());
    }
    #[cfg(any(windows, target_os = "linux"))]
    if let Some(h) = host {
        sel = sel
            .backend_name("winit".into())
            .with_winit_custom_application_handler(HostHandler(h));
    }
    sel.select()?;
    slint::set_xdg_app_id("snowdesk")?;
    let ui = Panel::new()?;
    let ctl = Rc::new(Ctl {
        cfg: RefCell::new(cfg.clone()),
        tx: ch.overlay_tx.clone(),
        tracks: Rc::new(VecModel::default()),
        monitors: Rc::new(VecModel::default()),
        now: Cell::new(None),
        save_timer: Timer::default(),
        unsaved: Cell::new(false),
    });
    apply_cfg(&ui, &cfg);
    ui.set_tracks(ModelRc::from(ctl.tracks.clone()));
    ui.set_monitors(ModelRc::from(ctl.monitors.clone()));
    ui.set_version(env!("CARGO_PKG_VERSION").into());
    ui.set_config_dir(Config::dir().display().to_string().into());
    ui.set_music_dir(SharedString::from(
        library::primary_dir().display().to_string(),
    ));

    ui.window()
        .on_close_requested(|| CloseRequestResponse::HideWindow);

    // --- settings ---
    {
        let (w, ctl) = (ui.as_weak(), ctl.clone());
        ui.on_config_edited(move || {
            let Some(ui) = w.upgrade() else { return };
            let new = read_cfg(&ui, &ctl.cfg.borrow());
            let autostart_changed = new.autostart != ctl.cfg.borrow().autostart;
            *ctl.cfg.borrow_mut() = new.clone();
            let _ = ctl.tx.send(OverlayCmd::Config(Box::new(new.clone())));
            if autostart_changed {
                crate::autostart::set(new.autostart);
            }
            schedule_save(&ctl);
        });
    }
    {
        let (w, ctl) = (ui.as_weak(), ctl.clone());
        ui.on_monitor_toggled(move |i, on| {
            let Some(ui) = w.upgrade() else { return };
            if let Some(mut row) = ctl.monitors.row_data(i as usize) {
                row.enabled = on;
                let name = row.name.to_string();
                ctl.monitors.set_row_data(i as usize, row);
                let mut c = ctl.cfg.borrow_mut();
                c.disabled_outputs.retain(|n| *n != name);
                if !on {
                    c.disabled_outputs.push(name);
                }
            }
            drop(ui);
            let c = ctl.cfg.borrow().clone();
            let _ = ctl.tx.send(OverlayCmd::Config(Box::new(c.clone())));
            c.save();
        });
    }
    // --- overlay controls ---
    macro_rules! fwd {
        ($on:ident, $cmd:expr) => {{
            let tx = ch.overlay_tx.clone();
            ui.$on(move || {
                let _ = tx.send($cmd);
            });
        }};
    }
    fwd!(on_toggle_play, OverlayCmd::TogglePlay);
    fwd!(on_clear, OverlayCmd::Clear);
    fwd!(on_undo, OverlayCmd::Undo);
    fwd!(on_redo, OverlayCmd::Redo);
    fwd!(on_stamp_snowman, OverlayCmd::StampSnowman);
    fwd!(on_mountains, OverlayCmd::Mountains);
    fwd!(on_save_scene, OverlayCmd::SaveScene);
    fwd!(on_load_scene, OverlayCmd::LoadScene);
    {
        let tx = ch.overlay_tx.clone();
        ui.on_toggle_pause(move || {
            let _ = tx.send(OverlayCmd::TogglePause);
        });
        let tx = ch.overlay_tx.clone();
        ui.on_tool_selected(move |i| {
            let _ = tx.send(OverlayCmd::Tool(i));
        });
        let tx = ch.overlay_tx.clone();
        ui.on_radius_edited(move |r| {
            let _ = tx.send(OverlayCmd::Radius(r));
        });
        let w = ui.as_weak();
        ui.on_dismiss_error(move || {
            if let Some(u) = w.upgrade() {
                u.set_error_text("".into());
            }
        });
    }
    ui.on_open_music_folder(|| {
        library::ensure_primary_dir();
        open_folder(&library::primary_dir());
    });
    // --- music controls ---
    {
        let a = ch.audio_tx.clone();
        ui.on_play_pause({
            let a = a.clone();
            move || {
                let _ = a.send(AudioCmd::PlayPause);
            }
        });
        ui.on_next({
            let a = a.clone();
            move || {
                let _ = a.send(AudioCmd::Next);
            }
        });
        ui.on_prev({
            let a = a.clone();
            move || {
                let _ = a.send(AudioCmd::Prev);
            }
        });
        ui.on_play_index({
            let a = a.clone();
            move |i| {
                let _ = a.send(AudioCmd::PlayIndex(i.max(0) as usize));
            }
        });
        ui.on_seek({
            let a = a.clone();
            move |f| {
                let _ = a.send(AudioCmd::Seek(f));
            }
        });
        let ctl1 = ctl.clone();
        ui.on_volume_edited({
            let a = a.clone();
            move |v| {
                let _ = a.send(AudioCmd::Volume(v));
                ctl1.cfg.borrow_mut().volume = v;
                schedule_save(&ctl1);
            }
        });
        let ctl2 = ctl.clone();
        ui.on_shuffle_edited({
            let a = a.clone();
            move |b| {
                let _ = a.send(AudioCmd::Shuffle(b));
                ctl2.cfg.borrow_mut().shuffle = b;
                schedule_save(&ctl2);
            }
        });
        let ctl3 = ctl.clone();
        ui.on_repeat_edited({
            let a = a.clone();
            move |b| {
                let _ = a.send(AudioCmd::Repeat(b));
                ctl3.cfg.borrow_mut().repeat = b;
                schedule_save(&ctl3);
            }
        });
    }

    // --- event pump: overlay, audio, tray/ipc ---
    {
        let w = ui.as_weak();
        let ctl = ctl.clone();
        let (orx, arx, urx) = (ch.overlay_rx.clone(), ch.audio_rx.clone(), ch.ui_rx.clone());
        let atx = ch.audio_tx.clone();
        let drain = move || {
            let Some(ui) = w.upgrade() else { return };
            while let Ok(e) = orx.try_recv() {
                on_overlay_event(&ui, &ctl, e);
            }
            while let Ok(e) = arx.try_recv() {
                on_audio_event(&ui, &ctl, e);
            }
            while let Ok(m) = urx.try_recv() {
                match m {
                    UiMsg::Show => {
                        let _ = ui.show();
                    }
                    UiMsg::TogglePlay => {
                        let _ = ctl.tx.send(OverlayCmd::TogglePlay);
                    }
                    UiMsg::TogglePause => {
                        let _ = ctl.tx.send(OverlayCmd::TogglePause);
                    }
                    UiMsg::Quit => {
                        let _ = slint::quit_event_loop();
                    }
                    UiMsg::ConfigFileChanged => reload_config(&ui, &ctl, &atx),
                }
            }
        };
        PUMP.with(|p| *p.borrow_mut() = Some(Box::new(drain)));
    }
    spawn_forwarder((ch.overlay_rx.clone(), ch.audio_rx.clone(), ch.ui_rx.clone()));
    let _cfg_watch = {
        let tx = ch.ui_tx.clone();
        Config::watch(move || {
            let _ = tx.send(UiMsg::ConfigFileChanged);
        })
    };
    // Wakes the winit loop so hotplugged monitors are picked up even while
    // the panel is idle (the host rescans in `about_to_wait`).
    let rescan = Timer::default();
    rescan.start(TimerMode::Repeated, Duration::from_secs(2), || {});

    if !start_hidden {
        ui.show()?;
    }
    slint::run_event_loop_until_quit()?;
    PUMP.with(|p| *p.borrow_mut() = None);
    ctl.cfg.borrow().save();
    Ok(())
}

fn on_overlay_event(ui: &Panel, ctl: &Ctl, e: OverlayEvent) {
    match e {
        OverlayEvent::Play(p) => ui.set_play_mode(p),
        OverlayEvent::Paused(p) => {
            ui.set_paused(p);
            ctl.cfg.borrow_mut().paused = p;
        }
        OverlayEvent::AutoPaused(p) => ui.set_auto_paused(p),
        OverlayEvent::Monitors(names) => {
            let off = ctl.cfg.borrow().disabled_outputs.clone();
            ctl.monitors.set_vec(
                names
                    .into_iter()
                    .map(|n| MonitorRow {
                        enabled: !off.contains(&n),
                        name: n.into(),
                    })
                    .collect::<Vec<_>>(),
            );
        }
        OverlayEvent::Error(m) => ui.set_error_text(m.into()),
        OverlayEvent::Fatal(m) => {
            ui.set_play_mode(false);
            ui.set_error_text(format!("Overlay stopped: {m}").into());
        }
    }
}

fn set_now_row(ctl: &Ctl, now: Option<usize>) {
    if let Some(old) = ctl.now.get()
        && let Some(mut r) = ctl.tracks.row_data(old)
    {
        r.playing = false;
        ctl.tracks.set_row_data(old, r);
    }
    if let Some(i) = now
        && let Some(mut r) = ctl.tracks.row_data(i)
    {
        r.playing = true;
        ctl.tracks.set_row_data(i, r);
    }
    ctl.now.set(now);
}

fn on_audio_event(ui: &Panel, ctl: &Ctl, e: AudioEvent) {
    match e {
        AudioEvent::Library { tracks, current } => {
            ctl.now.set(current);
            ctl.tracks.set_vec(
                tracks
                    .iter()
                    .enumerate()
                    .map(|(i, t)| TrackRow {
                        title: t.title.as_str().into(),
                        artist: t.artist.as_str().into(),
                        length: fmt_time(t.duration).into(),
                        playing: current == Some(i),
                    })
                    .collect::<Vec<_>>(),
            );
        }
        AudioEvent::Now { index, track, art } => {
            set_now_row(ctl, Some(index));
            ui.set_now_title(track.title.into());
            ui.set_now_artist(track.artist.into());
            ui.set_now_album(track.album.into());
            ui.set_duration(track.duration.as_secs_f32());
            ui.set_duration_text(fmt_time(track.duration).into());
            ui.set_position(0.0);
            ui.set_position_text(fmt_time(Duration::ZERO).into());
            match art {
                Some(a) => {
                    let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                        &a.rgba, a.width, a.height,
                    );
                    ui.set_now_art(slint::Image::from_rgba8(buf));
                    ui.set_has_art(true);
                }
                None => {
                    ui.set_now_art(slint::Image::default());
                    ui.set_has_art(false);
                }
            }
        }
        AudioEvent::Playing(p) => ui.set_playing(p),
        AudioEvent::Position { pos, dur } => {
            ui.set_position(pos.as_secs_f32());
            ui.set_position_text(fmt_time(pos).into());
            if dur > Duration::ZERO {
                ui.set_duration(dur.as_secs_f32());
            }
        }
        AudioEvent::Error(m) => ui.set_error_text(m.into()),
    }
}

/// Apply an edit of config.toml made outside the app (hot reload). Our own
/// saves produce the same config and are ignored.
fn reload_config(ui: &Panel, ctl: &Ctl, audio: &Sender<AudioCmd>) {
    if ctl.unsaved.get() {
        return;
    }
    let Ok(text) = std::fs::read_to_string(Config::path()) else {
        return;
    };
    let new = match Config::from_toml(&text) {
        Ok(c) => c,
        Err(e) => {
            ui.set_error_text(format!("config.toml not applied: {e}").into());
            return;
        }
    };
    let old = ctl.cfg.borrow().clone();
    if new == old {
        return;
    }
    tracing::info!("config.toml changed on disk; reloading");
    apply_cfg(ui, &new);
    let _ = ctl.tx.send(OverlayCmd::Config(Box::new(new.clone())));
    let _ = audio.send(AudioCmd::Volume(new.volume));
    let _ = audio.send(AudioCmd::Shuffle(new.shuffle));
    let _ = audio.send(AudioCmd::Repeat(new.repeat));
    if new.autostart != old.autostart {
        crate::autostart::set(new.autostart);
    }
    let off = new.disabled_outputs.clone();
    for i in 0..ctl.monitors.row_count() {
        if let Some(mut r) = ctl.monitors.row_data(i) {
            r.enabled = !off.contains(&r.name.to_string());
            ctl.monitors.set_row_data(i, r);
        }
    }
    *ctl.cfg.borrow_mut() = new;
}
