//! X11 helpers for the winit overlay. Override-redirect windows never get
//! keyboard focus, so in play mode Esc and the tool keys 1-9 are grabbed on
//! the root window instead.

use super::Ptr;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{ConnectionExt, GrabMode, ModMask};
use x11rb::rust_connection::RustConnection;

const XK_ESCAPE: u32 = 0xff1b;
/// XK_1 ..= XK_9.
const XK_DIGITS: std::ops::RangeInclusive<u32> = 0x31..=0x39;

/// Grabbed keycodes and what each means.
type Keys = Vec<(u8, Ptr)>;

pub struct EscapeGrab {
    /// Connection, root window, keys.
    conn: Option<(RustConnection, u32, Keys)>,
    on: bool,
}

impl EscapeGrab {
    pub fn new() -> Self {
        let conn = (|| {
            let (c, screen) = RustConnection::connect(None).ok()?;
            let root = c.setup().roots[screen].root;
            let (min, max) = (c.setup().min_keycode, c.setup().max_keycode);
            let map = c
                .get_keyboard_mapping(min, max - min + 1)
                .ok()?
                .reply()
                .ok()?;
            let per = map.keysyms_per_keycode.max(1) as usize;
            // First keycode whose unshifted keysym is `sym`.
            let code = |sym: u32| {
                map.keysyms
                    .chunks(per)
                    .position(|k| k.first() == Some(&sym))
                    .map(|i| min + i as u8)
            };
            let mut keys = vec![(code(XK_ESCAPE)?, Ptr::Escape)];
            for s in XK_DIGITS {
                if let Some(k) = code(s) {
                    keys.push((k, Ptr::Digit((s - 0x30) as u8)));
                }
            }
            Some((c, root, keys))
        })();
        Self { conn, on: false }
    }

    pub fn set(&mut self, on: bool) {
        let Some((c, root, keys)) = &self.conn else {
            return;
        };
        if on == self.on {
            return;
        }
        self.on = on;
        for (key, _) in keys {
            let _ = if on {
                c.grab_key(
                    false,
                    *root,
                    ModMask::ANY,
                    *key,
                    GrabMode::ASYNC,
                    GrabMode::ASYNC,
                )
                .map(|_| ())
            } else {
                c.ungrab_key(*key, *root, ModMask::ANY).map(|_| ())
            };
        }
        let _ = c.flush();
    }

    /// Grabbed keys pressed since the last call (only while grabbed).
    pub fn pressed(&mut self) -> Vec<Ptr> {
        let Some((c, _, keys)) = &self.conn else {
            return Vec::new();
        };
        let mut out = Vec::new();
        while let Ok(Some(ev)) = c.poll_for_event() {
            if self.on
                && let Event::KeyPress(k) = ev
                && let Some((_, p)) = keys.iter().find(|(code, _)| *code == k.detail)
            {
                out.push(*p);
            }
        }
        out
    }
}
