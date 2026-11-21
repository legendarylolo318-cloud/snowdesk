//! Audio engine: runs on its own thread, owns the rodio player, the music
//! library and a folder watcher.

pub mod capture;
pub mod library;
pub mod tap;

use crossbeam_channel::{Receiver, Sender, select};
use library::Track;
use notify::{RecursiveMode, Watcher};
use rodio::{DeviceSinkBuilder, Player};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub enum AudioCmd {
    PlayPause,
    Next,
    Prev,
    PlayIndex(usize),
    /// Fraction 0..1 of the current track.
    Seek(f32),
    Volume(f32),
    Shuffle(bool),
    Repeat(bool),
    /// Analyse all audio on the computer instead of only our player.
    SystemAudio(bool),
    Quit,
}

#[derive(Debug, Clone)]
pub enum AudioEvent {
    /// The whole library, plus the index of the current track in it.
    Library {
        tracks: Vec<Track>,
        current: Option<usize>,
    },
    Now {
        index: usize,
        track: Track,
        art: Option<library::Art>,
    },
    Playing(bool),
    Position {
        pos: Duration,
        dur: Duration,
    },
    Error(String),
}

pub struct AudioHandle {
    pub tx: Sender<AudioCmd>,
    pub levels: Arc<tap::Levels>,
}

pub fn spawn(
    tx: Sender<AudioCmd>,
    rx: Receiver<AudioCmd>,
    events: Sender<AudioEvent>,
    volume: f32,
    shuffle: bool,
    repeat: bool,
) -> AudioHandle {
    let levels = Arc::new(tap::Levels::default());
    let l2 = levels.clone();
    std::thread::Builder::new()
        .name("audio".into())
        .spawn(move || {
            Engine::new(events, l2, volume, shuffle, repeat).run(rx);
        })
        .expect("spawn audio thread");
    AudioHandle { tx, levels }
}

/// Fallback device order: lower is better; `u8::MAX` = never use.
fn device_rank(desc: &str) -> u8 {
    let d = desc.to_ascii_lowercase();
    if d.contains("discard all samples") || d == "null" {
        u8::MAX
    } else if d.contains("pipewire") {
        0
    } else if d.contains("pulse") {
        1
    } else if d.starts_with("default") || d.contains("default") {
        2
    } else {
        3
    }
}

struct Engine {
    events: Sender<AudioEvent>,
    levels: Arc<tap::Levels>,
    library: library::Library,
    current: Option<usize>,
    sink: Option<rodio::stream::MixerDeviceSink>,
    player: Option<Player>,
    /// Set from the audio device's error callback (device unplugged etc.).
    device_error: Arc<AtomicBool>,
    volume: f32,
    shuffle: bool,
    repeat: bool,
    started: bool,
    /// Last position and when it last advanced (stall detection).
    progress: (Duration, Instant),
    /// System audio capture, while "react to any music" is on.
    capture: Option<capture::Capture>,
}

impl Engine {
    fn new(
        events: Sender<AudioEvent>,
        levels: Arc<tap::Levels>,
        v: f32,
        sh: bool,
        rp: bool,
    ) -> Self {
        Self {
            events,
            levels,
            library: library::Library::default(),
            current: None,
            sink: None,
            player: None,
            device_error: Arc::new(AtomicBool::new(false)),
            volume: v,
            shuffle: sh,
            repeat: rp,
            started: false,
            progress: (Duration::ZERO, Instant::now()),
            capture: None,
        }
    }

    fn tracks(&self) -> &[Track] {
        &self.library.tracks
    }

    fn rescan(&mut self) {
        let cur_path = self
            .current
            .and_then(|i| self.tracks().get(i))
            .map(|t| t.path.clone());
        self.library.rescan(&library::music_dirs());
        // Indices shift when files are added or removed: follow the path.
        self.current = cur_path.and_then(|p| self.tracks().iter().position(|t| t.path == p));
        let _ = self.events.send(AudioEvent::Library {
            tracks: self.tracks().to_vec(),
            current: self.current,
        });
        // Scanning reads every file's tags: give the scratch memory back.
        crate::platform::trim_heap();
    }

    fn open_output(&mut self) -> bool {
        use rodio::cpal::traits::HostTrait;
        let err = self.device_error.clone();
        err.store(false, Ordering::Relaxed);
        let on_error = move |e: rodio::cpal::Error| {
            tracing::warn!("audio device error: {e}");
            err.store(true, Ordering::Relaxed);
        };
        let open = |b: DeviceSinkBuilder| {
            b.with_error_callback(on_error.clone())
                .open_sink_or_fallback()
        };
        // Default device first, then any other output (same order as
        // rodio's `open_default_sink`, plus the error callback).
        let mut res = DeviceSinkBuilder::from_default_device().and_then(open);
        if let Err(e) = &res {
            tracing::debug!("default audio device: {e}");
        }
        if res.is_err()
            && let Ok(devs) = rodio::cpal::default_host().output_devices()
        {
            // Prefer sound-server devices; never pick ALSA's "null" sink,
            // which discards samples (tracks would "finish" instantly).
            let mut devs: Vec<_> = devs
                .map(|d| {
                    let name = rodio::DeviceTrait::description(&d)
                        .map(|n| n.to_string())
                        .unwrap_or_default();
                    (device_rank(&name), name, d)
                })
                .filter(|(rank, _, _)| *rank < u8::MAX)
                .collect();
            devs.sort_by_key(|(rank, _, _)| *rank);
            for (_, name, d) in devs {
                match DeviceSinkBuilder::from_device(d).and_then(open) {
                    Ok(sink) => {
                        tracing::info!("audio output: {name}");
                        res = Ok(sink);
                        break;
                    }
                    Err(e) => tracing::debug!("audio device {name}: {e}"),
                }
            }
        }
        match res {
            Ok(mut sink) => {
                sink.log_on_drop(false);
                let p = Player::connect_new(sink.mixer());
                p.set_volume(self.volume);
                self.player = Some(p);
                self.sink = Some(sink);
                true
            }
            Err(e) => {
                let _ = self
                    .events
                    .send(AudioEvent::Error(format!("No audio output: {e}")));
                false
            }
        }
    }

    fn ensure_player(&mut self) -> bool {
        self.player.is_some() || self.open_output()
    }

    fn play_index(&mut self, i: usize) -> bool {
        self.play_at(i, Duration::ZERO, false)
    }

    /// Start track `i` at `pos`. Returns false if it could not be played.
    fn play_at(&mut self, i: usize, pos: Duration, paused: bool) -> bool {
        let Some(track) = self.tracks().get(i).cloned() else {
            return false;
        };
        if !self.ensure_player() {
            return false;
        }
        let file = match std::fs::File::open(&track.path) {
            Ok(f) => f,
            Err(e) => {
                let _ = self
                    .events
                    .send(AudioEvent::Error(format!("{}: {e}", track.title)));
                return false;
            }
        };
        let dec = match rodio::Decoder::try_from(file) {
            Ok(d) => d,
            Err(e) => {
                let _ = self.events.send(AudioEvent::Error(format!(
                    "Cannot decode {}: {e}",
                    track.title
                )));
                return false;
            }
        };
        let Some(p) = self.player.as_ref() else {
            return false;
        };
        p.clear();
        p.append(tap::Tap::new(dec, self.levels.clone()));
        if pos > Duration::ZERO {
            let _ = p.try_seek(pos);
        }
        if paused {
            p.pause();
        } else {
            p.play();
        }
        self.current = Some(i);
        self.started = true;
        self.progress = (pos, Instant::now());
        let art = library::read_art(&track.path);
        let _ = self.events.send(AudioEvent::Now {
            index: i,
            track,
            art,
        });
        let _ = self.events.send(AudioEvent::Playing(!paused));
        true
    }

    /// Re-open the output after the device vanished or stopped consuming
    /// samples, resuming the current track where it was.
    fn recover_output(&mut self) {
        let (pos, paused) = self
            .player
            .as_ref()
            .map(|p| (p.get_pos(), p.is_paused()))
            .unwrap_or_default();
        self.player = None;
        self.sink = None;
        tracing::info!("re-opening audio output");
        if let Some(i) = self.current.filter(|_| self.started) {
            if !self.play_at(i, pos, paused) {
                self.started = false;
                let _ = self.events.send(AudioEvent::Playing(false));
            }
        } else {
            self.device_error.store(false, Ordering::Relaxed);
        }
    }

    fn step(&mut self, dir: i32) {
        let n = self.tracks().len();
        if n == 0 || !self.ensure_player() {
            return;
        }
        // Skip files that fail to open/decode, but never loop forever.
        let mut cur = self.current.unwrap_or(if dir > 0 { n - 1 } else { 0 });
        for _ in 0..n.min(32) {
            let next = if self.shuffle && n > 1 && dir > 0 {
                loop {
                    let r = fastrand::usize(..n);
                    if r != cur {
                        break r;
                    }
                }
            } else {
                (cur as i64 + dir as i64).rem_euclid(n as i64) as usize
            };
            if self.play_index(next) {
                return;
            }
            cur = next;
        }
    }

    fn handle(&mut self, c: AudioCmd) -> bool {
        match c {
            AudioCmd::Quit => return false,
            AudioCmd::PlayPause => match (&self.player, self.started) {
                (Some(p), true) => {
                    if p.is_paused() {
                        p.play();
                    } else {
                        p.pause();
                    }
                    self.progress = (p.get_pos(), Instant::now());
                    let _ = self.events.send(AudioEvent::Playing(!p.is_paused()));
                }
                _ => match self.current {
                    Some(i) => {
                        if !self.play_index(i) {
                            self.step(1);
                        }
                    }
                    None => self.step(1),
                },
            },
            AudioCmd::Next => self.step(1),
            AudioCmd::Prev => {
                let restart = self
                    .player
                    .as_ref()
                    .is_some_and(|p| p.get_pos() > Duration::from_secs(3));
                match (restart, self.current) {
                    (true, Some(i)) => {
                        self.play_index(i);
                    }
                    _ => self.step(-1),
                }
            }
            AudioCmd::PlayIndex(i) => {
                self.play_index(i);
            }
            AudioCmd::Seek(f) => {
                if let (Some(p), Some(t)) = (
                    &self.player,
                    self.current.and_then(|i| self.library.tracks.get(i)),
                ) {
                    let d = t.duration.mul_f32(f.clamp(0.0, 1.0));
                    if let Err(e) = p.try_seek(d) {
                        tracing::debug!("seek failed: {e}");
                    }
                    self.progress = (p.get_pos(), Instant::now());
                }
            }
            AudioCmd::Volume(v) => {
                self.volume = v.clamp(0.0, 1.5);
                if let Some(p) = &self.player {
                    p.set_volume(self.volume);
                }
            }
            AudioCmd::Shuffle(b) => self.shuffle = b,
            AudioCmd::Repeat(b) => self.repeat = b,
            AudioCmd::SystemAudio(on) => {
                if on && self.capture.is_none() {
                    match capture::start(self.levels.clone()) {
                        Ok(c) => self.capture = Some(c),
                        Err(e) => {
                            let _ = self.events.send(AudioEvent::Error(format!(
                                "Cannot listen to system audio: {e}"
                            )));
                        }
                    }
                } else if !on {
                    self.capture = None;
                }
            }
        }
        true
    }

    /// Called every 250 ms while a track is loaded.
    fn tick(&mut self) {
        if self.device_error.load(Ordering::Relaxed) {
            self.recover_output();
            return;
        }
        let (Some(p), Some(i)) = (&self.player, self.current) else {
            return;
        };
        if !self.started {
            return;
        }
        if p.empty() {
            tracing::debug!("track finished at {:?}", p.get_pos());
            self.started = false;
            let last = i + 1 >= self.tracks().len();
            if self.shuffle || !last || self.repeat {
                self.step(1);
            } else {
                let _ = self.events.send(AudioEvent::Playing(false));
            }
            return;
        }
        let pos = p.get_pos();
        let dur = self.tracks().get(i).map(|t| t.duration).unwrap_or_default();
        if p.is_paused() || pos != self.progress.0 {
            self.progress = (pos, Instant::now());
        } else if self.progress.1.elapsed() > Duration::from_secs(4) {
            // Playing but no samples consumed: the device went away silently.
            tracing::warn!("audio output stalled");
            self.recover_output();
            return;
        }
        let _ = self.events.send(AudioEvent::Position { pos, dur });
    }

    fn run(mut self, rx: Receiver<AudioCmd>) {
        library::ensure_primary_dir();
        self.rescan();

        // Watch folders; bursts of events are coalesced into one rescan.
        let (wtx, wrx) = crossbeam_channel::unbounded::<()>();
        let mut watcher = notify::recommended_watcher(move |r: notify::Result<notify::Event>| {
            if r.is_ok_and(|e| !e.kind.is_access()) {
                let _ = wtx.send(());
            }
        })
        .ok();
        if let Some(w) = watcher.as_mut() {
            for d in library::music_dirs() {
                if d.is_dir() {
                    let _ = w.watch(&d, RecursiveMode::Recursive);
                }
            }
        }
        let mut rescan_at: Option<Instant> = None;
        loop {
            // Only tick while something is loaded or a rescan is pending, so
            // an idle player never wakes up.
            let ticking = self.started || self.device_error.load(Ordering::Relaxed);
            let wait = match rescan_at {
                Some(t) => t.saturating_duration_since(Instant::now()),
                None if ticking => Duration::from_millis(250),
                None => Duration::from_secs(3600),
            }
            .min(Duration::from_millis(if ticking { 250 } else { 3_600_000 }));
            select! {
                recv(rx) -> c => match c {
                    Ok(c) => if !self.handle(c) { break },
                    Err(_) => break,
                },
                recv(wrx) -> _ => {
                    while wrx.try_recv().is_ok() {}
                    rescan_at = Some(Instant::now() + Duration::from_millis(700));
                }
                default(wait) => {}
            }
            if rescan_at.is_some_and(|t| Instant::now() >= t) {
                rescan_at = None;
                self.rescan();
            }
            if ticking {
                self.tick();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::device_rank;

    /// Regression: the fallback picked ALSA's null device, so every track
    /// "finished" immediately.
    #[test]
    fn null_sink_is_never_used_and_servers_come_first() {
        assert_eq!(
            device_rank("Discard all samples (playback) or generate zero samples (capture)"),
            u8::MAX
        );
        assert_eq!(device_rank("null"), u8::MAX);
        assert!(device_rank("PipeWire Sound Server") < device_rank("HDA Intel PCH, ALC897 Analog"));
        assert!(device_rank("PulseAudio Sound Server") < device_rank("Default ALSA Output"));
    }
}
