//! X11 helpers for the winit overlay. Override-redirect windows never get
//! keyboard focus, so in play mode Esc is grabbed on the root window instead.

use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{ConnectionExt, GrabMode, ModMask};
use x11rb::rust_connection::RustConnection;

const XK_ESCAPE: u32 = 0xff1b;

pub struct EscapeGrab {
    conn: Option<(RustConnection, u32, u8)>,
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
            let i = map.keysyms.iter().position(|&k| k == XK_ESCAPE)?;
            Some((c, root, min + (i / per) as u8))
        })();
        Self { conn, on: false }
    }

    pub fn set(&mut self, on: bool) {
        let Some((c, root, key)) = &self.conn else {
            return;
        };
        if on == self.on {
            return;
        }
        self.on = on;
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
        let _ = c.flush();
    }

    /// True if Esc was pressed since the last call (only while grabbed).
    pub fn pressed(&mut self) -> bool {
        let Some((c, _, key)) = &self.conn else {
            return false;
        };
        if !self.on {
            return false;
        }
        let mut hit = false;
        while let Ok(Some(ev)) = c.poll_for_event() {
            if let Event::KeyPress(k) = ev
                && k.detail == *key
            {
                hit = true;
            }
        }
        hit
    }
}
