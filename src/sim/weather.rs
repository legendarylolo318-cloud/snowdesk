//! Weather moods: calm, normal, windy and the occasional short storm, on a
//! schedule shared by every monitor. The next mood is chosen in advance so
//! the panel can show a forecast.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mood {
    Calm,
    Normal,
    Windy,
    Storm,
}

impl Mood {
    pub fn name(self) -> &'static str {
        match self {
            Mood::Calm => "Calm",
            Mood::Normal => "Steady snow",
            Mood::Windy => "Windy",
            Mood::Storm => "Snowstorm",
        }
    }

    /// How long a mood lasts (seconds).
    fn duration(self) -> f32 {
        let r = fastrand::f32();
        match self {
            Mood::Calm => 180.0 + r * 180.0,
            Mood::Normal => 240.0 + r * 240.0,
            Mood::Windy => 120.0 + r * 180.0,
            Mood::Storm => 60.0 + r * 60.0,
        }
    }

    fn pick_after(self) -> Mood {
        loop {
            let r = fastrand::f32();
            let m = if r < 0.4 {
                Mood::Normal
            } else if r < 0.65 {
                Mood::Calm
            } else if r < 0.9 {
                Mood::Windy
            } else {
                Mood::Storm
            };
            if m != self {
                return m;
            }
        }
    }
}

/// What the weather does to the configured snow right now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeatherFx {
    pub intensity_mul: f32,
    /// Added to the configured wind (-1..1).
    pub wind_add: f32,
    pub gust_mul: f32,
}

impl Default for WeatherFx {
    fn default() -> Self {
        Self {
            intensity_mul: 1.0,
            wind_add: 0.0,
            gust_mul: 1.0,
        }
    }
}

pub struct Weather {
    pub mood: Mood,
    pub next: Mood,
    /// Seconds until `next` starts.
    pub left: f32,
    /// Wind direction for windy moods and storms.
    side: f32,
    pub fx: WeatherFx,
}

impl Default for Weather {
    fn default() -> Self {
        Self::new()
    }
}

impl Weather {
    pub fn new() -> Self {
        let mood = Mood::Normal;
        Self {
            mood,
            next: mood.pick_after(),
            left: mood.duration(),
            side: if fastrand::bool() { 1.0 } else { -1.0 },
            fx: WeatherFx::default(),
        }
    }

    fn target(&self, base_wind: f32) -> WeatherFx {
        match self.mood {
            Mood::Calm => WeatherFx {
                intensity_mul: 0.5,
                wind_add: -base_wind * 0.7,
                gust_mul: 0.3,
            },
            Mood::Normal => WeatherFx::default(),
            Mood::Windy => WeatherFx {
                intensity_mul: 1.1,
                wind_add: 0.6 * self.side,
                gust_mul: 1.8,
            },
            Mood::Storm => WeatherFx {
                intensity_mul: 2.2,
                wind_add: 0.8 * self.side,
                gust_mul: 2.5,
            },
        }
    }

    /// Advance by `dt` seconds. Returns true when the mood changed.
    pub fn step(&mut self, dt: f32, enabled: bool, base_wind: f32) -> bool {
        if !enabled {
            self.fx = WeatherFx::default();
            return false;
        }
        let mut changed = false;
        self.left -= dt;
        if self.left <= 0.0 {
            self.mood = self.next;
            self.next = self.mood.pick_after();
            self.left = self.mood.duration();
            self.side = if fastrand::bool() { 1.0 } else { -1.0 };
            changed = true;
        }
        // Ease into the new mood over ~15 s.
        let t = self.target(base_wind);
        let k = (dt / 15.0).min(1.0);
        self.fx.intensity_mul += (t.intensity_mul - self.fx.intensity_mul) * k;
        self.fx.wind_add += (t.wind_add - self.fx.wind_add) * k;
        self.fx.gust_mul += (t.gust_mul - self.fx.gust_mul) * k;
        changed
    }

    /// "Windy now · Calm in ~3 min".
    pub fn forecast(&self) -> String {
        let mins = (self.left / 60.0).ceil().max(1.0) as u32;
        format!(
            "{} now · {} in ~{} min",
            self.mood.name(),
            self.next.name(),
            mins
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moods_change_on_schedule_and_ease_in() {
        let mut w = Weather::new();
        w.next = Mood::Storm;
        w.left = 1.0;
        assert!(w.step(2.0, true, 0.1), "changed");
        assert_eq!(w.mood, Mood::Storm);
        assert_ne!(w.next, Mood::Storm, "never the same mood twice");
        // Eases in: not at full strength yet, and gets there.
        assert!(w.fx.intensity_mul < 2.0);
        for _ in 0..60 {
            w.step(1.0, true, 0.1);
            if w.mood != Mood::Storm {
                break;
            }
        }
        assert!(w.forecast().contains(" now · "));
        // Turned off: plain configured weather.
        w.step(1.0, false, 0.1);
        assert_eq!(w.fx, WeatherFx::default());
    }
}
