//! Builder challenge: tallest snowman and longest throw, kept across runs.
//! Sizes use "snow metres": 100 px at 1x scale.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const PX_PER_METRE: f32 = 100.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Records {
    pub tallest_m: f32,
    pub longest_m: f32,
}

impl Records {
    pub fn path() -> PathBuf {
        crate::config::Config::dir().join("records.toml")
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let p = Self::path();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(s) = toml::to_string(self) {
            let _ = std::fs::write(p, s);
        }
    }

    /// Offer a snowman height (m); true if it is a new record.
    pub fn snowman(&mut self, m: f32) -> bool {
        // Ignore tiny improvements from settling (5 cm).
        if m > self.tallest_m + 0.05 {
            self.tallest_m = m;
            true
        } else {
            false
        }
    }

    pub fn throw(&mut self, m: f32) -> bool {
        if m > self.longest_m + 0.05 {
            self.longest_m = m;
            true
        } else {
            false
        }
    }

    pub fn describe(&self) -> String {
        format!(
            "Tallest snowman {:.1} m · Longest throw {:.1} m",
            self.tallest_m, self.longest_m
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_only_go_up() {
        let mut r = Records::default();
        assert!(r.snowman(1.2));
        assert!(!r.snowman(1.0));
        assert!(!r.snowman(1.22), "settling noise");
        assert!(r.throw(18.0));
        assert_eq!(r.describe(), "Tallest snowman 1.2 m · Longest throw 18.0 m");
        let back: Records = toml::from_str(&toml::to_string(&r).unwrap()).unwrap();
        assert_eq!(back, r);
    }
}
