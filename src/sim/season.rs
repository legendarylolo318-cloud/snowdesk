//! Seasons: the same engine with different weather. Winter snow, autumn
//! leaves, spring cherry blossoms, or rain that collects in puddles.

use super::grid::{LEAF, PETAL, SNOW, WATER};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Season {
    #[default]
    Winter,
    Autumn,
    Spring,
    Rain,
    /// Follow the calendar (northern hemisphere).
    Auto,
}

impl Season {
    pub const ALL: [Season; 5] = [
        Season::Winter,
        Season::Autumn,
        Season::Spring,
        Season::Rain,
        Season::Auto,
    ];

    /// The concrete season for `month` (1-12) when set to `Auto`.
    pub fn resolve(self, month: u32) -> Season {
        match self {
            Season::Auto => match month {
                12 | 1 | 2 => Season::Winter,
                3..=5 => Season::Spring,
                6..=8 => Season::Rain,
                _ => Season::Autumn,
            },
            s => s,
        }
    }

    /// What falling particles turn into when they settle.
    pub fn material(self) -> u8 {
        match self {
            Season::Autumn => LEAF,
            Season::Spring => PETAL,
            Season::Rain => WATER,
            _ => SNOW,
        }
    }

    /// Particle count relative to snow (leaves are big, so fewer).
    pub fn density(self) -> f32 {
        match self {
            Season::Autumn => 0.12,
            Season::Spring => 0.35,
            Season::Rain => 0.55,
            _ => 1.0,
        }
    }

    pub fn index(self) -> i32 {
        Self::ALL.iter().position(|&s| s == self).unwrap_or(0) as i32
    }

    pub fn from_index(i: i32) -> Self {
        Self::ALL
            .get(i.max(0) as usize)
            .copied()
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_follows_the_calendar() {
        assert_eq!(Season::Auto.resolve(1), Season::Winter);
        assert_eq!(Season::Auto.resolve(4), Season::Spring);
        assert_eq!(Season::Auto.resolve(7), Season::Rain);
        assert_eq!(Season::Auto.resolve(10), Season::Autumn);
        assert_eq!(Season::Autumn.resolve(1), Season::Autumn);
        for s in Season::ALL {
            assert_eq!(Season::from_index(s.index()), s);
        }
    }

    #[test]
    fn config_round_trips_season() {
        let c = crate::config::Config {
            season: Season::Spring,
            ..Default::default()
        };
        let back = crate::config::Config::from_toml(&c.to_toml()).unwrap();
        assert_eq!(back.season, Season::Spring);
        let c = crate::config::Config::from_toml("season = \"rain\"\n").unwrap();
        assert_eq!(c.season, Season::Rain);
    }

    #[test]
    fn each_season_settles_its_own_material() {
        use crate::sim::grid::{LEAF, PETAL, SNOW, WATER};
        for (season, mat) in [
            (Season::Winter, SNOW),
            (Season::Autumn, LEAF),
            (Season::Spring, PETAL),
            (Season::Rain, WATER),
        ] {
            let cfg = crate::config::Config {
                intensity: 1.0,
                season,
                ..Default::default()
            };
            let mut w = crate::sim::world::World::new(400, 300, 1.0, &cfg);
            w.season = season;
            for _ in 0..60 * 30 {
                w.step();
            }
            assert!(w.grid.count(mat) > 0, "{season:?} settled nothing");
        }
    }
}
