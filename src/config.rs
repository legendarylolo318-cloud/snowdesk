//! TOML configuration, stored in the per-OS config dir.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// 0.0 (flurry) .. 1.0 (blizzard)
    pub intensity: f32,
    pub flake_size: f32,
    /// -1.0 (left) .. 1.0 (right)
    pub wind: f32,
    pub gusts: bool,
    /// Max pile height as fraction of screen height.
    pub max_pile: f32,
    /// 0 = never melts.
    pub melt_rate: f32,
    /// 0 = follow monitor refresh ("uncapped").
    pub fps_cap: u32,
    /// Output names that are disabled.
    pub disabled_outputs: Vec<String>,
    pub paused: bool,
    pub volume: f32,
    pub shuffle: bool,
    pub repeat: bool,
    pub music_reactive: bool,
    pub critter: bool,
    /// Penguins per monitor (when `critter` is on).
    pub penguins: u32,
    /// Penguin size multiplier (0.5 ..= 3.0).
    pub penguin_size: f32,
    pub string_lights: bool,
    /// Icicles grow under snowy window ledges.
    pub icicles: bool,
    /// Frost creeps in from the screen corners while you are idle.
    pub frost: bool,
    /// React to any audio playing on the computer, not only our player.
    pub system_audio: bool,
    /// Winter snow, autumn leaves, spring blossoms, rain, or by the calendar.
    pub season: crate::sim::season::Season,
    /// Changing weather: calm spells, wind and the occasional storm.
    pub weather: bool,
    /// Penguins go home to igloos at night (9 pm - 7 am).
    pub penguin_village: bool,
    pub sparkles: bool,
    pub aurora: bool,
    pub restore_scene: bool,
    pub autostart: bool,
    pub cursor_wind: bool,
    pub windows_collide: bool,
    /// Your own music folder (scanned first; "Add music" copies here).
    pub music_dir: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            intensity: 0.35,
            flake_size: 1.0,
            wind: 0.1,
            gusts: true,
            max_pile: 0.12,
            melt_rate: 0.0,
            fps_cap: 60,
            disabled_outputs: Vec::new(),
            paused: false,
            volume: 0.6,
            shuffle: false,
            repeat: false,
            music_reactive: true,
            critter: false,
            penguins: 1,
            penguin_size: 1.0,
            string_lights: false,
            icicles: true,
            frost: false,
            system_audio: false,
            season: crate::sim::season::Season::Winter,
            weather: true,
            penguin_village: true,
            sparkles: false,
            aurora: false,
            restore_scene: false,
            autostart: false,
            cursor_wind: true,
            windows_collide: true,
            music_dir: None,
        }
    }
}

/// One-click looks for the Snow tab and the tray menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    Cozy,
    Blizzard,
    Minimal,
    Party,
}

impl Preset {
    pub const ALL: [Preset; 4] = [Self::Cozy, Self::Blizzard, Self::Minimal, Self::Party];

    pub fn from_index(i: i32) -> Option<Self> {
        Self::ALL.get(usize::try_from(i).ok()?).copied()
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Cozy => "Cozy",
            Self::Blizzard => "Blizzard",
            Self::Minimal => "Minimal",
            Self::Party => "Party",
        }
    }

    /// Change only the look; app settings (monitors, music, autostart,
    /// pause, season) stay as they are.
    pub fn apply(self, c: &mut Config) {
        let d = Config::default();
        c.flake_size = d.flake_size;
        c.melt_rate = d.melt_rate;
        c.cursor_wind = true;
        c.windows_collide = true;
        c.icicles = true;
        match self {
            Self::Cozy => {
                c.intensity = 0.3;
                c.wind = 0.05;
                c.gusts = false;
                c.max_pile = 0.12;
                c.weather = true;
                c.critter = true;
                c.penguins = 2;
                c.penguin_size = 1.0;
                c.penguin_village = true;
                c.string_lights = true;
                c.frost = false;
                c.sparkles = false;
                c.aurora = true;
                c.music_reactive = true;
                c.fps_cap = 60;
            }
            Self::Blizzard => {
                c.intensity = 1.0;
                c.flake_size = 1.2;
                c.wind = 0.6;
                c.gusts = true;
                c.max_pile = 0.3;
                c.weather = false;
                c.frost = true;
                c.string_lights = false;
                c.sparkles = true;
                c.aurora = false;
                c.music_reactive = true;
                c.fps_cap = 60;
            }
            Self::Minimal => {
                c.intensity = 0.15;
                c.wind = 0.05;
                c.gusts = false;
                c.max_pile = 0.06;
                c.weather = false;
                c.critter = false;
                c.string_lights = false;
                c.icicles = false;
                c.frost = false;
                c.sparkles = false;
                c.aurora = false;
                c.music_reactive = false;
                c.fps_cap = 30;
            }
            Self::Party => {
                c.intensity = 0.55;
                c.wind = 0.15;
                c.gusts = true;
                c.max_pile = 0.15;
                c.weather = true;
                c.critter = true;
                c.penguins = 4;
                c.penguin_village = true;
                c.string_lights = true;
                c.sparkles = true;
                c.aurora = true;
                c.music_reactive = true;
                c.fps_cap = 60;
            }
        }
    }
}

/// Groups of settings on the Snow tab, each with its own Reset button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Snowfall,
    Penguins,
    Extras,
}

impl Section {
    pub fn from_index(i: i32) -> Option<Self> {
        [Self::Snowfall, Self::Penguins, Self::Extras]
            .get(usize::try_from(i).ok()?)
            .copied()
    }

    pub fn reset(self, c: &mut Config) {
        let d = Config::default();
        match self {
            Self::Snowfall => {
                c.intensity = d.intensity;
                c.flake_size = d.flake_size;
                c.wind = d.wind;
                c.max_pile = d.max_pile;
                c.melt_rate = d.melt_rate;
                c.season = d.season;
                c.fps_cap = d.fps_cap;
                c.weather = d.weather;
                c.gusts = d.gusts;
                c.cursor_wind = d.cursor_wind;
                c.windows_collide = d.windows_collide;
                c.music_reactive = d.music_reactive;
                c.system_audio = d.system_audio;
            }
            Self::Penguins => {
                c.critter = d.critter;
                c.penguins = d.penguins;
                c.penguin_size = d.penguin_size;
                c.penguin_village = d.penguin_village;
            }
            Self::Extras => {
                c.string_lights = d.string_lights;
                c.icicles = d.icicles;
                c.frost = d.frost;
                c.sparkles = d.sparkles;
                c.aurora = d.aurora;
            }
        }
    }
}

impl Config {
    pub fn dir() -> PathBuf {
        directories::ProjectDirs::from("", "", "snowdesk")
            .map(|d| d.config_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
    }

    pub fn path() -> PathBuf {
        Self::dir().join("config.toml")
    }

    pub fn scene_path() -> PathBuf {
        Self::dir().join("scene.bin")
    }

    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }

    pub fn from_toml(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    pub fn load() -> Self {
        match std::fs::read_to_string(Self::path()) {
            Ok(s) => Self::from_toml(&s).unwrap_or_else(|e| {
                tracing::warn!("bad config: {e}; using defaults");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) {
        let p = Self::path();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        // Write a temp file and rename it so a crash never leaves a
        // truncated config behind.
        // Never replace a good file with an empty one.
        let text = match self.to_toml() {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("cannot save config: {e}");
                return;
            }
        };
        let tmp = p.with_extension("toml.tmp");
        if let Err(e) = std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, &p)) {
            tracing::warn!("cannot save config: {e}");
        }
    }
}

impl Config {
    /// Call `on_change` whenever config.toml is written by anyone (hot
    /// reload). Keep the returned watcher alive.
    pub fn watch(on_change: impl Fn() + Send + 'static) -> Option<notify::RecommendedWatcher> {
        use notify::{RecursiveMode, Watcher};
        let dir = Self::dir();
        let _ = std::fs::create_dir_all(&dir);
        let mut w = notify::recommended_watcher(move |r: notify::Result<notify::Event>| {
            if let Ok(ev) = r
                && !ev.kind.is_access()
                && ev
                    .paths
                    .iter()
                    .any(|p| p.file_name() == Some("config.toml".as_ref()))
            {
                on_change();
            }
        })
        .ok()?;
        w.watch(&dir, RecursiveMode::NonRecursive).ok()?;
        Some(w)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let c = Config {
            intensity: 0.9,
            wind: -0.5,
            disabled_outputs: vec!["DP-1".into()],
            repeat: true,
            ..Config::default()
        };
        let s = c.to_toml().unwrap();
        assert_eq!(Config::from_toml(&s).unwrap(), c);
    }

    #[test]
    fn music_dir_round_trips() {
        let c = Config {
            music_dir: Some(PathBuf::from("/home/me/Music")),
            ..Config::default()
        };
        assert_eq!(Config::from_toml(&c.to_toml().unwrap()).unwrap(), c);
        assert_eq!(Config::from_toml("").unwrap().music_dir, None);
    }

    #[test]
    fn presets_keep_app_settings() {
        let base = Config {
            disabled_outputs: vec!["HDMI-A-1".into()],
            volume: 0.2,
            autostart: true,
            music_dir: Some(PathBuf::from("/m")),
            ..Config::default()
        };
        for p in Preset::ALL {
            let mut c = base.clone();
            p.apply(&mut c);
            assert_eq!(c.disabled_outputs, base.disabled_outputs);
            assert_eq!(c.volume, base.volume);
            assert!(c.autostart);
            assert_eq!(c.music_dir, base.music_dir);
            assert_ne!(c, base, "{p:?} changed nothing");
        }
        assert_eq!(Preset::from_index(3), Some(Preset::Party));
        assert_eq!(Preset::from_index(-1), None);
    }

    #[test]
    fn section_reset_only_touches_its_section() {
        let mut c = Config::default();
        Preset::Party.apply(&mut c);
        c.intensity = 0.9;
        let before = c.clone();
        Section::Penguins.reset(&mut c);
        assert!(!c.critter && c.penguins == 1);
        assert_eq!(c.intensity, 0.9);
        assert_eq!(c.aurora, before.aurora);
        Section::Snowfall.reset(&mut c);
        Section::Extras.reset(&mut c);
        assert_eq!(c, Config::default());
    }

    #[test]
    fn partial_file_uses_defaults() {
        let c = Config::from_toml("intensity = 0.8\n").unwrap();
        assert_eq!(c.intensity, 0.8);
        assert_eq!(c.fps_cap, 60);
    }
}
