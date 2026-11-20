// No console window on Windows (CLI flags attach to the parent console).
#![cfg_attr(windows, windows_subsystem = "windows")]

mod audio;
mod autostart;
mod config;
mod ipc;
mod overlay;
mod platform;
mod render;
mod sim;
mod tools;
mod tray;
mod ui;

use crossbeam_channel::unbounded;
use ipc::IpcCmd;
use overlay::{OverlayCmd, OverlayCtx};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tray::UiMsg;

const HELP: &str = "snowdesk — snow over your desktop

USAGE: snowdesk [OPTIONS]

OPTIONS:
  --toggle-play   toggle play mode in the running instance
  --pause         pause / resume snow
  --clear         clear all snow and snowballs
  --next-track    skip to the next track
  --show          show the control panel
  --quit          quit the running instance
  --hidden        start without showing the control panel
  --no-panel      headless overlay only (no UI, no tray)
  -h, --help      show this help
";

fn cli_command(args: &[String]) -> Option<IpcCmd> {
    args.iter().find_map(|a| match a.as_str() {
        "--toggle-play" => Some(IpcCmd::TogglePlay),
        "--pause" => Some(IpcCmd::Pause),
        "--clear" => Some(IpcCmd::Clear),
        "--next-track" => Some(IpcCmd::NextTrack),
        "--show" => Some(IpcCmd::Show),
        "--quit" => Some(IpcCmd::Quit),
        _ => None,
    })
}

/// With the "windows" subsystem stdout goes nowhere; when started from a
/// terminal, write CLI output (help, errors) to that terminal.
#[cfg(windows)]
fn attach_console() {
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    #[cfg(windows)]
    if !args.is_empty() {
        attach_console();
    }
    if args.iter().any(|a| a == "-h" || a == "--help") {
        print!("{HELP}");
        return;
    }
    if let Some(cmd) = cli_command(&args) {
        if let Err(e) = ipc::send(cmd) {
            eprintln!("snowdesk is not running ({e})");
            std::process::exit(1);
        }
        return;
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "snowdesk=info,warn".into()),
        )
        .init();
    let hidden = args.iter().any(|a| a == "--hidden");
    let headless = args.iter().any(|a| a == "--no-panel");
    // Before any thread starts: may switch the process to X11.
    let backend = overlay::choose_backend();

    let cfg = config::Config::load();
    let (cmd_tx, cmd_rx) = unbounded();
    let (ev_tx, ev_rx) = unbounded();
    let (aev_tx, aev_rx) = unbounded();
    let (ui_tx, ui_rx) = unbounded();
    let (audio_tx, audio_rx) = unbounded();

    // Single instance: the listener doubles as the lock.
    {
        let (cmd_tx, ui_tx, audio_tx) = (cmd_tx.clone(), ui_tx.clone(), audio_tx.clone());
        let res = ipc::listen(move |c| match c {
            IpcCmd::TogglePlay => {
                let _ = cmd_tx.send(OverlayCmd::TogglePlay);
            }
            IpcCmd::Pause => {
                let _ = cmd_tx.send(OverlayCmd::TogglePause);
            }
            IpcCmd::Clear => {
                let _ = cmd_tx.send(OverlayCmd::Clear);
            }
            IpcCmd::NextTrack => {
                let _ = audio_tx.send(audio::AudioCmd::Next);
            }
            IpcCmd::Show => {
                let _ = ui_tx.send(UiMsg::Show);
            }
            IpcCmd::Quit => {
                let _ = ui_tx.send(UiMsg::Quit);
            }
        });
        if let Err(e) = res {
            if e.kind() == std::io::ErrorKind::AddrInUse {
                let _ = ipc::send(IpcCmd::Show);
                eprintln!("snowdesk is already running; asked it to show its panel.");
            } else {
                eprintln!("ipc error: {e}");
            }
            return;
        }
    }
    let audio = audio::spawn(
        audio_tx,
        audio_rx,
        aev_tx,
        cfg.volume,
        cfg.shuffle,
        cfg.repeat,
    );

    let health = Arc::new(overlay::Health::default());
    watchdog(health.clone());

    // Overlay thread. Panics are caught inside `overlay::run`.
    let ctx = OverlayCtx {
        cmd_rx,
        ev_tx: ev_tx.clone(),
        levels: audio.levels.clone(),
        cfg: cfg.clone(),
        health,
    };
    let (host_tx, host_rx) = unbounded();
    let host = (backend == overlay::Backend::Winit).then(|| overlay::Host::new(host_tx, ev_tx));
    let overlay_rx = host.is_some().then_some(host_rx);
    let overlay_done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let overlay_thread = {
        let done = overlay_done.clone();
        std::thread::Builder::new()
            .name("overlay".into())
            .spawn(move || {
                overlay::run(ctx, backend, overlay_rx);
                done.store(true, std::sync::atomic::Ordering::Relaxed);
            })
            .expect("spawn overlay thread")
    };

    if headless {
        // Nobody shows these without a panel, but they must be drained or
        // the unbounded channels grow forever (audio positions at 4 Hz).
        std::thread::spawn(move || {
            while let Ok(e) = ev_rx.recv() {
                tracing::info!("overlay event: {e:?}");
            }
        });
        std::thread::spawn(move || {
            while let Ok(e) = aev_rx.recv() {
                match e {
                    audio::AudioEvent::Position { .. } => {}
                    audio::AudioEvent::Library { tracks, .. } => {
                        tracing::info!("music library: {} tracks", tracks.len())
                    }
                    audio::AudioEvent::Now { track, .. } => {
                        tracing::info!("now playing: {}", track.title)
                    }
                    e => tracing::info!("audio event: {e:?}"),
                }
            }
        });
        // Hot reload without a panel: apply config.toml edits directly.
        let _cfg_watch = {
            let (otx, atx) = (cmd_tx.clone(), audio.tx.clone());
            let last = std::sync::Mutex::new(cfg.clone());
            config::Config::watch(move || {
                let Ok(text) = std::fs::read_to_string(config::Config::path()) else {
                    return;
                };
                let Ok(new) = config::Config::from_toml(&text) else {
                    tracing::warn!("config.toml has errors; not applied");
                    return;
                };
                let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
                if *last != new {
                    tracing::info!("config.toml changed on disk; reloading");
                    let _ = otx.send(OverlayCmd::Config(Box::new(new.clone())));
                    let _ = atx.send(audio::AudioCmd::Volume(new.volume));
                    let _ = atx.send(audio::AudioCmd::Shuffle(new.shuffle));
                    let _ = atx.send(audio::AudioCmd::Repeat(new.repeat));
                    *last = new;
                }
            })
        };
        // Headless: run until `snowdesk --quit` or the overlay dies.
        let done = overlay_done.clone();
        let quit = move || {
            matches!(ui_rx.try_recv(), Ok(UiMsg::Quit))
                || done.load(std::sync::atomic::Ordering::Relaxed)
        };
        match host {
            Some(host) => {
                if let Err(e) = overlay::winit_common::run_standalone(host, quit) {
                    eprintln!("{e:#}");
                }
            }
            None => {
                while !quit() {
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
        }
    } else {
        let _tray = tray::spawn(ui_tx.clone());
        let ch = ui::Channels {
            overlay_tx: cmd_tx.clone(),
            overlay_rx: ev_rx,
            audio_tx: audio.tx.clone(),
            audio_rx: aev_rx,
            ui_tx: ui_tx.clone(),
            ui_rx,
        };
        if let Err(e) = ui::run(cfg, ch, hidden, host) {
            eprintln!("UI error: {e}");
        }
    }
    let _ = cmd_tx.send(OverlayCmd::Quit);
    let _ = audio.tx.send(audio::AudioCmd::Quit);
    // Never hang on exit (e.g. a driver blocked in present): give the
    // overlay a moment to save the scene and tear down, then leave anyway.
    let deadline = Instant::now() + Duration::from_secs(3);
    while !overlay_thread.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    ipc::cleanup();
    if overlay_thread.is_finished() {
        let _ = overlay_thread.join();
    } else {
        eprintln!("overlay did not stop in time; exiting anyway");
        std::process::exit(0);
    }
}

/// If the overlay stops running while it takes input (play mode), the
/// screen would be blocked. Kill the process instead: the compositor / OS
/// then removes the overlay windows.
fn watchdog(h: Arc<overlay::Health>) {
    std::thread::Builder::new()
        .name("watchdog".into())
        .spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(1));
                if h.play.load(std::sync::atomic::Ordering::Relaxed)
                    && h.stalled_for() > Duration::from_secs(5)
                {
                    eprintln!("snowdesk: overlay stopped responding in play mode; exiting");
                    std::process::abort();
                }
            }
        })
        .expect("spawn watchdog");
}
