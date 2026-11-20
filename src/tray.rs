//! System tray: ksni (StatusNotifierItem) on Linux, tray-icon on Windows.

use crossbeam_channel::Sender;

#[derive(Debug, Clone, Copy)]
pub enum UiMsg {
    Show,
    TogglePlay,
    TogglePause,
    Quit,
    /// config.toml was modified on disk.
    ConfigFileChanged,
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
    }

    impl ksni::Tray for SnowTray {
        fn id(&self) -> String {
            "snowdesk".into()
        }
        fn title(&self) -> String {
            "snowdesk".into()
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
                        let _ = t.tx.send(msg);
                    }),
                    ..Default::default()
                }
                .into()
            };
            vec![
                item("Show panel", UiMsg::Show),
                item("Toggle play mode", UiMsg::TogglePlay),
                item("Pause snow", UiMsg::TogglePause),
                MenuItem::Separator,
                item("Quit", UiMsg::Quit),
            ]
        }
    }

    /// Keep the returned guard alive for the lifetime of the app.
    pub fn spawn(tx: Sender<UiMsg>) -> Option<Box<dyn std::any::Any>> {
        match (SnowTray { tx }).spawn() {
            Ok(h) => Some(Box::new(h)),
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
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, TrayIconBuilder};

    /// Must be called on the thread that runs the Win32 message loop (Slint's).
    pub fn spawn(tx: Sender<UiMsg>) -> Option<Box<dyn std::any::Any>> {
        let menu = Menu::new();
        let show = MenuItem::new("Show panel", true, None);
        let play = MenuItem::new("Toggle play mode", true, None);
        let pause = MenuItem::new("Pause snow", true, None);
        let quit = MenuItem::new("Quit", true, None);
        menu.append_items(&[
            &show,
            &play,
            &pause,
            &PredefinedMenuItem::separator(),
            &quit,
        ])
        .ok()?;
        let (rgba, n) = icon_rgba();
        let icon = Icon::from_rgba(rgba, n, n).ok()?;
        let (s, p, pa, q) = (
            show.id().clone(),
            play.id().clone(),
            pause.id().clone(),
            quit.id().clone(),
        );
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let m = if e.id == s {
                UiMsg::Show
            } else if e.id == p {
                UiMsg::TogglePlay
            } else if e.id == pa {
                UiMsg::TogglePause
            } else if e.id == q {
                UiMsg::Quit
            } else {
                return;
            };
            let _ = tx.send(m);
        }));
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("snowdesk")
            .with_icon(icon)
            .build()
            .ok()?;
        Some(Box::new(tray))
    }
}

pub use imp::spawn;
