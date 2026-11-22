//! System tray: ksni (StatusNotifierItem) on Linux, tray-icon on Windows.

use crossbeam_channel::Sender;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub enum UiMsg {
    Show,
    TogglePlay,
    TogglePause,
    Quit,
    /// config.toml was modified on disk.
    ConfigFileChanged,
    PlayPauseMusic,
    NextTrack,
    /// Index into [`AMOUNTS`].
    Amount(usize),
    /// Season index (see `Season::from_index`).
    Season(i32),
    /// Index into `Preset::ALL`.
    Preset(i32),
    /// Hide the snow, showing it again after this many minutes (0 = show
    /// it now).
    HideFor(u32),
    /// A file dialog (run on its own thread) finished; None = cancelled.
    Picked(Option<Pick>),
    /// Music files were copied into the music folder (count or error).
    MusicAdded(Result<usize, String>),
}

#[derive(Debug, Clone)]
pub enum Pick {
    MusicFiles(Vec<PathBuf>),
    MusicFolder(PathBuf),
    Scene(PathBuf),
}

/// Snow amounts offered in the tray (label, intensity).
pub const AMOUNTS: [(&str, f32); 4] = [
    ("Light", 0.15),
    ("Medium", 0.35),
    ("Heavy", 0.65),
    ("Blizzard", 1.0),
];

pub const SEASONS: [&str; 5] = ["Winter", "Autumn", "Spring", "Rain", "Auto"];

/// The amount closest to an intensity.
pub fn amount_index(intensity: f32) -> usize {
    (0..AMOUNTS.len())
        .min_by(|&a, &b| {
            (AMOUNTS[a].1 - intensity)
                .abs()
                .total_cmp(&(AMOUNTS[b].1 - intensity).abs())
        })
        .unwrap_or(1)
}

/// What the tray menu shows; the panel pushes this when it changes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrayState {
    /// "Title — Artist", empty when nothing is loaded.
    pub song: String,
    pub music_playing: bool,
    pub has_music: bool,
    pub paused: bool,
    pub hidden: bool,
    pub play_mode: bool,
    pub amount: usize,
    pub season: usize,
}

/// 32×32 RGBA snowflake on a dark disc.
pub fn icon_rgba() -> (Vec<u8>, u32) {
    const N: u32 = 32;
    let mut v = Vec::with_capacity((N * N * 4) as usize);
    for y in 0..N {
        for x in 0..N {
            let (dx, dy) = (x as f32 - 15.5, y as f32 - 15.5);
            let d = dx.hypot(dy);
            let mut px = [0u8; 4];
            if d < 15.0 {
                px = [36, 40, 59, 255];
                for k in 0..6 {
                    let t = k as f32 * std::f32::consts::PI / 3.0;
                    let perp = (-t.sin() * dx + t.cos() * dy).abs();
                    let along = t.cos() * dx + t.sin() * dy;
                    if perp < 1.2 && along > 0.0 && along < 12.0 {
                        px = [122, 162, 247, 255];
                    }
                }
                if d < 2.2 {
                    px = [192, 202, 245, 255];
                }
            }
            v.extend_from_slice(&px);
        }
    }
    (v, N)
}

#[cfg(target_os = "linux")]
mod imp {
    use super::*;
    use ksni::blocking::TrayMethods;

    struct SnowTray {
        tx: Sender<UiMsg>,
        st: TrayState,
    }

    impl ksni::Tray for SnowTray {
        fn id(&self) -> String {
            "snowdesk".into()
        }
        fn title(&self) -> String {
            "snowdesk".into()
        }
        fn tool_tip(&self) -> ksni::ToolTip {
            ksni::ToolTip {
                title: "snowdesk".into(),
                description: if self.st.song.is_empty() {
                    String::new()
                } else {
                    format!("♪ {}", self.st.song)
                },
                ..Default::default()
            }
        }
        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            let (rgba, n) = icon_rgba();
            let mut argb = Vec::with_capacity(rgba.len());
            for p in rgba.as_chunks::<4>().0 {
                argb.extend_from_slice(&[p[3], p[0], p[1], p[2]]);
            }
            vec![ksni::Icon {
                width: n as i32,
                height: n as i32,
                data: argb,
            }]
        }
        fn activate(&mut self, _x: i32, _y: i32) {
            let _ = self.tx.send(UiMsg::Show);
        }
        fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
            use ksni::menu::*;
            let item = |label: &str, msg: UiMsg| -> ksni::MenuItem<Self> {
                StandardItem {
                    label: label.into(),
                    activate: Box::new(move |t: &mut Self| {
                        let _ = t.tx.send(msg.clone());
                    }),
                    ..Default::default()
                }
                .into()
            };
            let radio = |label: &str,
                         names: Vec<String>,
                         selected: usize,
                         f: fn(usize) -> UiMsg|
             -> ksni::MenuItem<Self> {
                SubMenu {
                    label: label.into(),
                    submenu: vec![
                        RadioGroup {
                            selected,
                            select: Box::new(move |t: &mut Self, i| {
                                let _ = t.tx.send(f(i));
                            }),
                            options: names
                                .into_iter()
                                .map(|n| RadioItem {
                                    label: n,
                                    ..Default::default()
                                })
                                .collect(),
                        }
                        .into(),
                    ],
                    ..Default::default()
                }
                .into()
            };
            let st = &self.st;
            let mut v = vec![
                item("Show panel", UiMsg::Show),
                item(
                    if st.play_mode {
                        "Exit play mode"
                    } else {
                        "Play mode"
                    },
                    UiMsg::TogglePlay,
                ),
                MenuItem::Separator,
            ];
            if !st.song.is_empty() {
                v.push(
                    StandardItem {
                        label: format!("♪ {}", st.song.replace('_', "__")),
                        enabled: false,
                        ..Default::default()
                    }
                    .into(),
                );
            }
            if st.has_music {
                v.push(item(
                    if st.music_playing {
                        "Pause music"
                    } else {
                        "Play music"
                    },
                    UiMsg::PlayPauseMusic,
                ));
                v.push(item("Next track", UiMsg::NextTrack));
                v.push(MenuItem::Separator);
            }
            v.push(radio(
                "Snow amount",
                AMOUNTS.iter().map(|a| a.0.to_string()).collect(),
                st.amount,
                UiMsg::Amount,
            ));
            v.push(radio(
                "Season",
                SEASONS.iter().map(|s| s.to_string()).collect(),
                st.season,
                |i| UiMsg::Season(i as i32),
            ));
            v.push(
                SubMenu {
                    label: "Preset".into(),
                    submenu: crate::config::Preset::ALL
                        .iter()
                        .enumerate()
                        .map(|(i, p)| item(p.label(), UiMsg::Preset(i as i32)))
                        .collect(),
                    ..Default::default()
                }
                .into(),
            );
            v.push(item(
                if st.paused {
                    "Resume snow"
                } else {
                    "Pause snow"
                },
                UiMsg::TogglePause,
            ));
            if st.hidden {
                v.push(item("Show snow again", UiMsg::HideFor(0)));
            } else {
                v.push(item("Hide snow for 1 hour", UiMsg::HideFor(60)));
            }
            v.push(MenuItem::Separator);
            v.push(item("Quit", UiMsg::Quit));
            v
        }
    }

    pub struct Tray(ksni::blocking::Handle<SnowTray>);

    impl Tray {
        pub fn set(&self, st: &TrayState) {
            let st = st.clone();
            self.0.update(move |t| t.st = st);
        }
    }

    pub fn spawn(tx: Sender<UiMsg>) -> Option<Tray> {
        match (SnowTray {
            tx,
            st: TrayState::default(),
        })
        .spawn()
        {
            Ok(h) => Some(Tray(h)),
            Err(e) => {
                tracing::warn!("no system tray available: {e}");
                None
            }
        }
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use tray_icon::menu::{
        CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu,
    };
    use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

    pub struct Tray {
        _icon: TrayIcon,
        song: MenuItem,
        music: MenuItem,
        next: MenuItem,
        play: MenuItem,
        pause: MenuItem,
        hide: MenuItem,
        unhide: MenuItem,
        amounts: Vec<CheckMenuItem>,
        seasons: Vec<CheckMenuItem>,
    }

    impl Tray {
        pub fn set(&self, st: &TrayState) {
            let song = if st.song.is_empty() {
                "♪ Nothing playing".to_string()
            } else {
                format!("♪ {}", st.song.replace('&', "&&"))
            };
            self.song.set_text(song);
            self.music.set_text(if st.music_playing {
                "Pause music"
            } else {
                "Play music"
            });
            self.music.set_enabled(st.has_music);
            self.next.set_enabled(st.has_music);
            self.play.set_text(if st.play_mode {
                "Exit play mode"
            } else {
                "Play mode"
            });
            self.pause.set_text(if st.paused {
                "Resume snow"
            } else {
                "Pause snow"
            });
            self.hide.set_enabled(!st.hidden);
            self.unhide.set_enabled(st.hidden);
            for (i, a) in self.amounts.iter().enumerate() {
                a.set_checked(i == st.amount);
            }
            for (i, s) in self.seasons.iter().enumerate() {
                s.set_checked(i == st.season);
            }
            let _ = self._icon.set_tooltip(Some(if st.song.is_empty() {
                "snowdesk".to_string()
            } else {
                format!("snowdesk\n♪ {}", st.song)
            }));
        }
    }

    /// Must be called on the thread that runs the Win32 message loop (Slint's).
    pub fn spawn(tx: Sender<UiMsg>) -> Option<Tray> {
        let mut ids: Vec<(MenuId, UiMsg)> = Vec::new();
        let mut mk = |label: &str, msg: UiMsg| {
            let m = MenuItem::new(label, true, None);
            ids.push((m.id().clone(), msg));
            m
        };
        let show = mk("Show panel", UiMsg::Show);
        let play = mk("Play mode", UiMsg::TogglePlay);
        let music = mk("Play music", UiMsg::PlayPauseMusic);
        let next = mk("Next track", UiMsg::NextTrack);
        let pause = mk("Pause snow", UiMsg::TogglePause);
        let hide = mk("Hide snow for 1 hour", UiMsg::HideFor(60));
        let unhide = mk("Show snow again", UiMsg::HideFor(0));
        let quit = mk("Quit", UiMsg::Quit);
        let presets: Vec<MenuItem> = crate::config::Preset::ALL
            .iter()
            .enumerate()
            .map(|(i, p)| mk(p.label(), UiMsg::Preset(i as i32)))
            .collect();
        let mut check = |label: &str, msg: UiMsg| {
            let m = CheckMenuItem::new(label, true, false, None);
            ids.push((m.id().clone(), msg));
            m
        };
        let amounts: Vec<CheckMenuItem> = AMOUNTS
            .iter()
            .enumerate()
            .map(|(i, a)| check(a.0, UiMsg::Amount(i)))
            .collect();
        let seasons: Vec<CheckMenuItem> = SEASONS
            .iter()
            .enumerate()
            .map(|(i, s)| check(s, UiMsg::Season(i as i32)))
            .collect();
        let song = MenuItem::new("♪ Nothing playing", false, None);
        let sub = |label: &str, items: &[&dyn tray_icon::menu::IsMenuItem]| {
            Submenu::with_items(label, true, items).ok()
        };
        let amount_menu = sub(
            "Snow amount",
            &amounts
                .iter()
                .map(|m| m as &dyn tray_icon::menu::IsMenuItem)
                .collect::<Vec<_>>(),
        )?;
        let season_menu = sub(
            "Season",
            &seasons
                .iter()
                .map(|m| m as &dyn tray_icon::menu::IsMenuItem)
                .collect::<Vec<_>>(),
        )?;
        let preset_menu = sub(
            "Preset",
            &presets
                .iter()
                .map(|m| m as &dyn tray_icon::menu::IsMenuItem)
                .collect::<Vec<_>>(),
        )?;
        let menu = Menu::new();
        menu.append_items(&[
            &show,
            &play,
            &PredefinedMenuItem::separator(),
            &song,
            &music,
            &next,
            &PredefinedMenuItem::separator(),
            &amount_menu,
            &season_menu,
            &preset_menu,
            &pause,
            &hide,
            &unhide,
            &PredefinedMenuItem::separator(),
            &quit,
        ])
        .ok()?;
        let (rgba, n) = icon_rgba();
        let icon = Icon::from_rgba(rgba, n, n).ok()?;
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            if let Some((_, m)) = ids.iter().find(|(id, _)| *id == e.id) {
                let _ = tx.send(m.clone());
            }
        }));
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("snowdesk")
            .with_icon(icon)
            .build()
            .ok()?;
        let t = Tray {
            _icon: tray,
            song,
            music,
            next,
            play,
            pause,
            hide,
            unhide,
            amounts,
            seasons,
        };
        t.set(&TrayState::default());
        Some(t)
    }
}

pub use imp::{Tray, spawn};
