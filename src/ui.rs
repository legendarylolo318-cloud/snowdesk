//! Slint control panel glue (runs on the main thread).

use crate::audio::{AudioCmd, AudioEvent, library};
use crate::config::Config;
use crate::overlay::{OverlayCmd, OverlayEvent};
use crate::tray::{Pick, Tray, TrayState, UiMsg};
use crossbeam_channel::{Receiver, Sender};
use slint::{CloseRequestResponse, ComponentHandle, Model, ModelRc, Timer, TimerMode, VecModel};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

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
struct HostHandler(crate::overlay::Host, Sender<UiMsg>);

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
        s: Option<&slint::Window>,
        ev: &slint::winit_030::winit::event::WindowEvent,
    ) -> slint::winit_030::EventResult {
        // Files dropped onto the panel (X11 and Windows; winit has no
        // drag and drop on Wayland).
        if s.is_some()
            && let slint::winit_030::winit::event::WindowEvent::DroppedFile(p) = ev
        {
            let _ = self.1.send(UiMsg::Picked(Some(dropped(p.clone()))));
            return slint::winit_030::EventResult::PreventDefault;
        }
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

/// A dropped file: scenes are loaded, anything else goes to the music.
fn dropped(p: PathBuf) -> Pick {
    if p.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("snowscene"))
    {
        Pick::Scene(p)
    } else {
        Pick::MusicFiles(vec![p])
    }
}

/// How a settings change enters the undo history.
enum Hist {
    /// Panel edits: one step per burst (a slider drag is one step).
    Edit,
    /// Presets, resets, tray picks: always their own step.
    Step(&'static str),
    /// App settings (music folder): not undoable.
    No,
}

const UNDO_DEPTH: usize = 30;

struct Ctl {
    cfg: RefCell<Config>,
    /// Earlier settings, newest last, with what changed them.
    history: RefCell<Vec<(Config, &'static str)>>,
    last_edit: Cell<Option<Instant>>,
    tray: Option<Tray>,
    tray_state: RefCell<TrayState>,
    /// Now-playing info for the tray.
    song: RefCell<String>,
    music_playing: Cell<bool>,
    has_music: Cell<bool>,
    play_mode: Cell<bool>,
    hidden: Cell<bool>,
    /// "Hide snow for 1 hour": resumes the snow when it fires.
    hide_timer: Timer,
    /// A file dialog is open (only one at a time).
    picking: Rc<Cell<bool>>,
    ui_tx: Sender<UiMsg>,
    tx: Sender<OverlayCmd>,
    audio: Sender<AudioCmd>,
    /// A fullscreen app covers every monitor (no point listening).
    auto_paused: Cell<bool>,
    tracks: Rc<VecModel<TrackRow>>,
    monitors: Rc<VecModel<MonitorRow>>,
    now: Cell<Option<usize>>,
    save_timer: Timer,
    /// Edits not yet written to disk; they win over a concurrent file change.
    unsaved: Cell<bool>,
}

/// Save the config 500 ms after the last edit.
/// Run system audio capture only while it can matter.
fn sync_capture(ctl: &Ctl) {
    let c = ctl.cfg.borrow();
    let on = c.system_audio
        && c.music_reactive
        && !c.paused
        && !ctl.auto_paused.get()
        && !ctl.hidden.get();
    let _ = ctl.audio.send(AudioCmd::SystemAudio(on));
}

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

/// Apply a new config from the panel, tray or a preset: tell the overlay,
/// save it, and remember the old one for "Undo change".
fn commit(ui: &Panel, ctl: &Rc<Ctl>, new: Config, hist: Hist) {
    let old = ctl.cfg.borrow().clone();
    if new == old {
        return;
    }
    let now = Instant::now();
    let push = match hist {
        Hist::Edit => {
            let burst = ctl
                .last_edit
                .get()
                .is_some_and(|t| now - t < Duration::from_millis(1200));
            ctl.last_edit.set(Some(now));
            (!burst).then_some("your last settings change")
        }
        Hist::Step(l) => {
            ctl.last_edit.set(None);
            Some(l)
        }
        Hist::No => None,
    };
    if let Some(label) = push {
        let mut h = ctl.history.borrow_mut();
        h.push((old.clone(), label));
        if h.len() > UNDO_DEPTH {
            h.remove(0);
        }
    }
    *ctl.cfg.borrow_mut() = new.clone();
    let _ = ctl.tx.send(OverlayCmd::Config(Box::new(new.clone())));
    if new.autostart != old.autostart {
        crate::autostart::set(new.autostart);
    }
    sync_capture(ctl);
    schedule_save(ctl);
    sync_undo(ui, ctl);
    sync_tray(ctl);
}

fn sync_undo(ui: &Panel, ctl: &Ctl) {
    let h = ctl.history.borrow();
    ui.set_can_undo_settings(!h.is_empty());
    ui.set_undo_settings_label(h.last().map(|x| x.1).unwrap_or("").into());
}

/// Push the tray menu state if it changed.
fn sync_tray(ctl: &Ctl) {
    let Some(tray) = &ctl.tray else { return };
    let c = ctl.cfg.borrow();
    let st = TrayState {
        song: ctl.song.borrow().clone(),
        music_playing: ctl.music_playing.get(),
        has_music: ctl.has_music.get(),
        paused: c.paused,
        hidden: ctl.hidden.get(),
        play_mode: ctl.play_mode.get(),
        amount: crate::tray::amount_index(c.intensity),
        season: c.season.index().clamp(0, 4) as usize,
    };
    drop(c);
    if *ctl.tray_state.borrow() != st {
        tray.set(&st);
        *ctl.tray_state.borrow_mut() = st;
    }
}

fn show_music_dir(ui: &Panel) {
    ui.set_music_dir(library::active_dir().display().to_string().into());
}

/// Open a file dialog on its own thread (it blocks until closed) and post
/// the result back as `UiMsg::Picked`.
fn pick(ctl: &Ctl, what: &'static str) {
    if ctl.picking.replace(true) {
        return;
    }
    let tx = ctl.ui_tx.clone();
    let start = library::active_dir();
    let r = std::thread::Builder::new()
        .name("file-dialog".into())
        .spawn(move || {
            // Always answer, even if the dialog panics, or no dialog could
            // ever open again.
            let res = std::panic::catch_unwind(|| dialog(what, &start)).unwrap_or(None);
            let _ = tx.send(UiMsg::Picked(res));
        });
    if r.is_err() {
        ctl.picking.set(false);
    }
}

fn dialog(what: &str, start: &std::path::Path) -> Option<Pick> {
    let d = rfd::FileDialog::new();
    match what {
        "music" => d
            .set_title("Add music to snowdesk")
            .set_directory(dirs_home())
            .add_filter("Music", library::EXTENSIONS)
            .pick_files()
            .map(Pick::MusicFiles),
        "folder" => d
            .set_title("Choose your music folder")
            .set_directory(start)
            .pick_folder()
            .map(Pick::MusicFolder),
        _ => d
            .set_title("Load a snowdesk scene")
            .set_directory(crate::platform::export_dir())
            .add_filter("snowdesk scene", &["snowscene"])
            .pick_file()
            .map(Pick::Scene),
    }
}

fn dirs_home() -> PathBuf {
    directories::UserDirs::new()
        .and_then(|u| u.audio_dir().map(|p| p.to_path_buf()))
        .or_else(|| directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn on_picked(ui: &Panel, ctl: &Rc<Ctl>, p: Option<Pick>) {
    ctl.picking.set(false);
    match p {
        None => {}
        Some(Pick::Scene(p)) => {
            let _ = ctl.tx.send(OverlayCmd::ImportScene(p));
        }
        Some(Pick::MusicFiles(files)) => {
            ui.set_music_notice("Adding music…".into());
            let tx = ctl.ui_tx.clone();
            let dest = library::active_dir();
            let _ = std::thread::Builder::new()
                .name("add-music".into())
                .spawn(move || {
                    let r = library::import_files(&files, &dest).map_err(|e| e.to_string());
                    let _ = tx.send(UiMsg::MusicAdded(r));
                });
        }
        // The config is TOML: a folder name that is not valid UTF-8 cannot
        // be stored in it.
        Some(Pick::MusicFolder(d)) if d.to_str().is_none() => ui.set_music_notice(
            "That folder's name has characters snowdesk cannot save. Please rename it or pick another folder."
                .into(),
        ),
        Some(Pick::MusicFolder(d)) => set_music_dir(ui, ctl, Some(d)),
    }
}

fn set_music_dir(ui: &Panel, ctl: &Rc<Ctl>, d: Option<PathBuf>) {
    let mut c = ctl.cfg.borrow().clone();
    c.music_dir = d.clone();
    library::set_custom_dir(d.clone());
    let _ = ctl.audio.send(AudioCmd::MusicDir(d));
    commit(ui, ctl, c, Hist::No);
    ui.set_custom_music_dir(ctl.cfg.borrow().music_dir.is_some());
    show_music_dir(ui);
    ui.set_music_notice(format!("Playing music from {}", library::active_dir().display()).into());
}

/// Undo the newest settings change, keeping app state (volume, monitors,
/// music folder, pause) as it is now.
fn undo_settings(ui: &Panel, ctl: &Rc<Ctl>) {
    let Some((mut old, _)) = ctl.history.borrow_mut().pop() else {
        return;
    };
    {
        let cur = ctl.cfg.borrow();
        old.volume = cur.volume;
        old.shuffle = cur.shuffle;
        old.repeat = cur.repeat;
        old.paused = cur.paused;
        old.disabled_outputs = cur.disabled_outputs.clone();
        old.music_dir = cur.music_dir.clone();
    }
    apply_cfg(ui, &old);
    commit(ui, ctl, old, Hist::No);
    ctl.last_edit.set(None);
    sync_undo(ui, ctl);
}

/// Change the config from code (tray, presets): update the panel too.
fn change(ui: &Panel, ctl: &Rc<Ctl>, label: &'static str, f: impl FnOnce(&mut Config)) {
    let mut c = ctl.cfg.borrow().clone();
    f(&mut c);
    apply_cfg(ui, &c);
    commit(ui, ctl, c, Hist::Step(label));
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
        system_audio: ui.get_system_audio(),
        season: crate::sim::season::Season::from_index(ui.get_season_index()),
        weather: ui.get_weather(),
        penguin_village: ui.get_village(),
        icicles: ui.get_icicles(),
        frost: ui.get_frost(),
        critter: ui.get_critter(),
        penguins: ui
            .get_penguins()
            .round()
            .clamp(1.0, crate::sim::world::MAX_PENGUINS as f32) as u32,
        penguin_size: ui.get_penguin_size().clamp(0.5, 3.0),
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
    ui.set_system_audio(c.system_audio);
    ui.set_season_index(c.season.index());
    ui.set_weather(c.weather);
    ui.set_village(c.penguin_village);
    ui.set_icicles(c.icicles);
    ui.set_frost(c.frost);
    ui.set_penguins(c.penguins.clamp(1, crate::sim::world::MAX_PENGUINS) as f32);
    ui.set_penguin_size(c.penguin_size.clamp(0.5, 3.0));
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
    tray: Option<Tray>,
    #[cfg(any(windows, target_os = "linux"))] host: Option<crate::overlay::Host>,
) -> Result<(), slint::PlatformError> {
    let mut sel = slint::BackendSelector::new();
    // The software renderer keeps the panel out of GPU memory; it is plenty
    // fast for this UI. Override with SLINT_RENDERER=femtovg.
    if std::env::var_os("SLINT_RENDERER").is_none() {
        sel = sel.renderer_name("software".into());
    }
    #[cfg(any(windows, target_os = "linux"))]
    let can_drop = host.is_some();
    #[cfg(not(any(windows, target_os = "linux")))]
    let can_drop = false;
    #[cfg(any(windows, target_os = "linux"))]
    if let Some(h) = host {
        sel = sel
            .backend_name("winit".into())
            .with_winit_custom_application_handler(HostHandler(h, ch.ui_tx.clone()));
    }
    sel.select()?;
    slint::set_xdg_app_id("snowdesk")?;
    let ui = Panel::new()?;
    let ctl = Rc::new(Ctl {
        cfg: RefCell::new(cfg.clone()),
        audio: ch.audio_tx.clone(),
        auto_paused: Cell::new(false),
        tx: ch.overlay_tx.clone(),
        tracks: Rc::new(VecModel::default()),
        monitors: Rc::new(VecModel::default()),
        now: Cell::new(None),
        save_timer: Timer::default(),
        unsaved: Cell::new(false),
        history: RefCell::new(Vec::new()),
        last_edit: Cell::new(None),
        tray,
        tray_state: RefCell::new(TrayState::default()),
        song: RefCell::new(String::new()),
        music_playing: Cell::new(false),
        has_music: Cell::new(false),
        play_mode: Cell::new(false),
        hidden: Cell::new(false),
        hide_timer: Timer::default(),
        picking: Rc::new(Cell::new(false)),
        ui_tx: ch.ui_tx.clone(),
    });
    sync_tray(&ctl);
    sync_capture(&ctl);
    apply_cfg(&ui, &cfg);
    ui.set_tracks(ModelRc::from(ctl.tracks.clone()));
    ui.set_monitors(ModelRc::from(ctl.monitors.clone()));
    ui.set_version(env!("CARGO_PKG_VERSION").into());
    ui.set_config_dir(Config::dir().display().to_string().into());
    show_music_dir(&ui);
    ui.set_custom_music_dir(cfg.music_dir.is_some());
    ui.set_can_drop(can_drop);

    ui.window()
        .on_close_requested(|| CloseRequestResponse::HideWindow);

    // --- settings ---
    {
        let (w, ctl) = (ui.as_weak(), ctl.clone());
        ui.on_config_edited(move || {
            let Some(ui) = w.upgrade() else { return };
            let new = read_cfg(&ui, &ctl.cfg.borrow());
            commit(&ui, &ctl, new, Hist::Edit);
        });
    }
    {
        let (w, ctl1) = (ui.as_weak(), ctl.clone());
        ui.on_preset(move |i| {
            let (Some(ui), Some(p)) = (w.upgrade(), crate::config::Preset::from_index(i)) else {
                return;
            };
            change(&ui, &ctl1, "the preset", |c| p.apply(c));
        });
        let (w, ctl2) = (ui.as_weak(), ctl.clone());
        ui.on_reset_section(move |i| {
            let (Some(ui), Some(s)) = (w.upgrade(), crate::config::Section::from_index(i)) else {
                return;
            };
            change(&ui, &ctl2, "the reset", |c| s.reset(c));
        });
        let (w, ctl3) = (ui.as_weak(), ctl.clone());
        ui.on_undo_settings(move || {
            if let Some(ui) = w.upgrade() {
                undo_settings(&ui, &ctl3);
            }
        });
        let ctl4 = ctl.clone();
        ui.on_add_music(move || pick(&ctl4, "music"));
        let ctl5 = ctl.clone();
        ui.on_choose_music_folder(move || pick(&ctl5, "folder"));
        let ctl6 = ctl.clone();
        ui.on_browse_scene(move || pick(&ctl6, "scene"));
        let (w, ctl7) = (ui.as_weak(), ctl.clone());
        ui.on_default_music_folder(move || {
            if let Some(ui) = w.upgrade() {
                set_music_dir(&ui, &ctl7, None);
            }
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
    fwd!(on_igloo, OverlayCmd::Igloo);
    fwd!(on_export_picture, OverlayCmd::ExportPicture);
    fwd!(on_export_scene, OverlayCmd::ExportScene);
    {
        let tx = ch.overlay_tx.clone();
        ui.on_import_scene(move |p| {
            let p = p.trim().trim_matches('"').trim_matches('\'');
            if !p.is_empty() {
                let _ = tx.send(OverlayCmd::ImportScene(p.into()));
            }
        });
    }
    ui.on_open_exports(|| {
        let d = crate::platform::export_dir();
        let _ = std::fs::create_dir_all(&d);
        open_folder(&d);
    });
    fwd!(on_save_scene, OverlayCmd::SaveScene);
    fwd!(on_load_scene, OverlayCmd::LoadScene);
    {
        let tx = ch.overlay_tx.clone();
        ui.on_toggle_pause(move || {
            let _ = tx.send(OverlayCmd::TogglePause);
        });
        let ui_tx = ch.ui_tx.clone();
        ui.on_hide_for(move |m| {
            let _ = ui_tx.send(UiMsg::HideFor(m.max(0) as u32));
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
        open_folder(&library::active_dir());
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
                    UiMsg::PlayPauseMusic => {
                        let _ = atx.send(AudioCmd::PlayPause);
                    }
                    UiMsg::NextTrack => {
                        let _ = atx.send(AudioCmd::Next);
                    }
                    UiMsg::Amount(i) => {
                        if let Some(&(_, v)) = crate::tray::AMOUNTS.get(i) {
                            change(&ui, &ctl, "the snow amount", |c| c.intensity = v);
                        }
                    }
                    UiMsg::Season(i) => change(&ui, &ctl, "the season", |c| {
                        c.season = crate::sim::season::Season::from_index(i)
                    }),
                    UiMsg::Preset(i) => {
                        if let Some(p) = crate::config::Preset::from_index(i) {
                            change(&ui, &ctl, "the preset", |c| p.apply(c));
                        }
                    }
                    UiMsg::HideFor(0) => {
                        ctl.hide_timer.stop();
                        let _ = ctl.tx.send(OverlayCmd::Hide(false));
                    }
                    UiMsg::HideFor(min) => {
                        let _ = ctl.tx.send(OverlayCmd::Hide(true));
                        let tx = ctl.tx.clone();
                        ctl.hide_timer.start(
                            TimerMode::SingleShot,
                            Duration::from_secs(u64::from(min) * 60),
                            move || {
                                let _ = tx.send(OverlayCmd::Hide(false));
                            },
                        );
                    }
                    UiMsg::Picked(p) => on_picked(&ui, &ctl, p),
                    UiMsg::MusicAdded(r) => ui.set_music_notice(
                        match r {
                            Ok(0) => "No new music files were found there.".to_string(),
                            Ok(1) => "Added 1 track.".to_string(),
                            Ok(n) => format!("Added {n} tracks."),
                            Err(e) => format!("Could not add the music: {e}"),
                        }
                        .into(),
                    ),
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
        OverlayEvent::Play(p) => {
            ui.set_play_mode(p);
            ctl.play_mode.set(p);
            sync_tray(ctl);
        }
        OverlayEvent::Paused(p) => {
            ui.set_paused(p);
            ctl.cfg.borrow_mut().paused = p;
            sync_capture(ctl);
            sync_tray(ctl);
        }
        OverlayEvent::Hidden(h) => {
            ui.set_hidden(h);
            ctl.hidden.set(h);
            if !h {
                ctl.hide_timer.stop();
            }
            sync_capture(ctl);
            sync_tray(ctl);
        }
        OverlayEvent::ToolState(t, r) => {
            ui.set_tool_index(t);
            ui.set_radius(r);
        }
        OverlayEvent::AutoPaused(p) => {
            ui.set_auto_paused(p);
            ctl.auto_paused.set(p);
            sync_capture(ctl);
        }
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
        OverlayEvent::Forecast(t) => ui.set_forecast(t.into()),
        OverlayEvent::Notice(t) => ui.set_notice(t.into()),
        OverlayEvent::Imported(t) => {
            ui.set_notice(t.into());
            ui.set_tab(1);
            let _ = ui.show();
        }
        OverlayEvent::Records(text, beat) => {
            ui.set_records(text.into());
            if let Some(b) = beat {
                ui.set_record_news(b.into());
            }
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
            ctl.has_music.set(!tracks.is_empty());
            if tracks.is_empty() {
                ctl.song.borrow_mut().clear();
            }
            sync_tray(ctl);
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
            *ctl.song.borrow_mut() = if track.artist.is_empty() {
                track.title.clone()
            } else {
                format!("{} — {}", track.title, track.artist)
            };
            sync_tray(ctl);
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
        AudioEvent::Playing(p) => {
            ui.set_playing(p);
            ctl.music_playing.set(p);
            sync_tray(ctl);
        }
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
    let _ = audio.send(AudioCmd::SystemAudio(
        new.system_audio && new.music_reactive && !new.paused,
    ));
    if new.autostart != old.autostart {
        crate::autostart::set(new.autostart);
    }
    if new.music_dir != old.music_dir {
        library::set_custom_dir(new.music_dir.clone());
        let _ = audio.send(AudioCmd::MusicDir(new.music_dir.clone()));
        ui.set_custom_music_dir(new.music_dir.is_some());
        show_music_dir(ui);
    }
    let off = new.disabled_outputs.clone();
    for i in 0..ctl.monitors.row_count() {
        if let Some(mut r) = ctl.monitors.row_data(i) {
            r.enabled = !off.contains(&r.name.to_string());
            ctl.monitors.set_row_data(i, r);
        }
    }
    *ctl.cfg.borrow_mut() = new;
    sync_tray(ctl);
}
