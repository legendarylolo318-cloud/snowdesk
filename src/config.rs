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
    pub sparkles: bool,
    pub aurora: bool,
    pub restore_scene: bool,
    pub autostart: bool,
    pub cursor_wind: bool,
    pub windows_collide: bool,
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
            sparkles: false,
            aurora: false,
            restore_scene: false,
            autostart: false,
            cursor_wind: true,
            windows_collide: true,
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

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
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
        let tmp = p.with_extension("toml.tmp");
        if let Err(e) = std::fs::write(&tmp, self.to_toml()).and_then(|_| std::fs::rename(&tmp, &p))
        {
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
        let s = c.to_toml();
        assert_eq!(Config::from_toml(&s).unwrap(), c);
    }

    #[test]
    fn partial_file_uses_defaults() {
        let c = Config::from_toml("intensity = 0.8\n").unwrap();
        assert_eq!(c.intensity, 0.8);
        assert_eq!(c.fps_cap, 60);
    }
}
