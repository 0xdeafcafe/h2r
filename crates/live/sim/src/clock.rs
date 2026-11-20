//! Pacing and speed-up (docs/runtime/sim.md §3).
//!
//! The kernel clock is logical: it advances by what guest code costs and
//! jumps idle waits to their deadline, so an unpaced run goes as fast as the
//! host runs the guest. A pace only ever slows it down, through the
//! kernel's `Pacer` (`ticks_per_second` of 100 ns kernel time per wall
//! second).

/// Kernel time units a second: 100 ns ticks.
pub const KERNEL_HZ: u64 = 10_000_000;

/// Game ticks a second (the tick budget at `0x00146660`).
pub const GAME_HZ: u64 = 30;

/// How fast a match may run against the wall clock.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Pace {
    /// As fast as the host goes (the default).
    #[default]
    Unpaced,
    /// No faster than real time.
    Realtime,
    /// No faster than this many times real time.
    Times(f64),
}

impl Pace {
    /// `unpaced`, `realtime` (or `1x`), or `x<factor>` / `<factor>x`.
    pub fn parse(s: &str) -> Result<Pace, String> {
        let k = s.trim().to_ascii_lowercase();
        match k.as_str() {
            "" | "unpaced" | "fast" | "max" => return Ok(Pace::Unpaced),
            "realtime" | "real" | "real-time" => return Ok(Pace::Realtime),
            _ => {}
        }
        let num = k
            .strip_prefix('x')
            .or_else(|| k.strip_suffix('x'))
            .ok_or_else(|| format!("unknown pace {s:?} (unpaced, realtime, x<factor>)"))?;
        let f: f64 = num
            .parse()
            .map_err(|_| format!("pace {s:?}: the factor is not a number"))?;
        if !(f.is_finite() && f > 0.0) {
            return Err(format!("pace {s:?}: the factor must be above 0"));
        }
        if f > 10_000.0 {
            return Err(format!("pace {s:?}: at most x10000 (leave it unpaced)"));
        }
        if f == 1.0 {
            return Ok(Pace::Realtime);
        }
        Ok(Pace::Times(f))
    }

    /// The pacer's kernel ticks per wall second, or `None` for no pacer.
    pub fn ticks_per_second(self) -> Option<u64> {
        match self {
            Pace::Unpaced => None,
            Pace::Realtime => Some(KERNEL_HZ),
            Pace::Times(f) => Some(((KERNEL_HZ as f64) * f).round().max(1.0) as u64),
        }
    }

    pub fn label(self) -> String {
        match self {
            Pace::Unpaced => "unpaced".into(),
            Pace::Realtime => "realtime".into(),
            Pace::Times(f) => format!("x{f}"),
        }
    }
}

/// Measures a match's speed-up: game time over wall time.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SpeedMeter {
    /// Wall seconds from process start to the first match tick (boot,
    /// load, lobby).
    pub setup_s: f64,
    /// Wall seconds from the first match tick to the last.
    pub match_s: f64,
    /// Game ticks in the match.
    pub ticks: u64,
}

impl SpeedMeter {
    /// The match's game time, in seconds.
    pub fn game_seconds(&self) -> f64 {
        self.ticks as f64 / GAME_HZ as f64
    }

    /// Game time over the match's wall time; 0 when there was none.
    pub fn speedup(&self) -> f64 {
        if self.match_s > 0.0 {
            self.game_seconds() / self.match_s
        } else {
            0.0
        }
    }

    /// Game time over the whole process's wall time (setup included).
    pub fn process_speedup(&self) -> f64 {
        let all = self.setup_s + self.match_s;
        if all > 0.0 {
            self.game_seconds() / all
        } else {
            0.0
        }
    }

    /// Wall milliseconds a game tick took.
    pub fn ms_per_tick(&self) -> f64 {
        if self.ticks > 0 {
            self.match_s * 1000.0 / self.ticks as f64
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paces_parse() {
        assert_eq!(Pace::parse("unpaced"), Ok(Pace::Unpaced));
        assert_eq!(Pace::parse(""), Ok(Pace::Unpaced));
        assert_eq!(Pace::parse("Realtime"), Ok(Pace::Realtime));
        assert_eq!(Pace::parse("x1"), Ok(Pace::Realtime));
        assert_eq!(Pace::parse("x4"), Ok(Pace::Times(4.0)));
        assert_eq!(Pace::parse("2.5x"), Ok(Pace::Times(2.5)));
        assert!(Pace::parse("x0").is_err());
        assert!(Pace::parse("x-3").is_err());
        assert!(Pace::parse("xfast").is_err());
        assert!(Pace::parse("warp").is_err());
        assert!(Pace::parse("x100000").is_err());
    }

    #[test]
    fn pacer_rates() {
        assert_eq!(Pace::Unpaced.ticks_per_second(), None);
        assert_eq!(Pace::Realtime.ticks_per_second(), Some(10_000_000));
        assert_eq!(Pace::Times(4.0).ticks_per_second(), Some(40_000_000));
        assert_eq!(Pace::Times(0.5).ticks_per_second(), Some(5_000_000));
    }

    #[test]
    fn labels_round_trip() {
        for p in [Pace::Unpaced, Pace::Realtime, Pace::Times(3.0)] {
            assert_eq!(Pace::parse(&p.label()), Ok(p));
        }
    }

    #[test]
    fn speedups() {
        let m = SpeedMeter {
            setup_s: 10.0,
            match_s: 30.0,
            ticks: 30 * 300,
        };
        assert_eq!(m.game_seconds(), 300.0);
        assert_eq!(m.speedup(), 10.0);
        assert_eq!(m.process_speedup(), 7.5);
        assert!((m.ms_per_tick() - 30_000.0 / 9000.0).abs() < 1e-9);
        assert_eq!(SpeedMeter::default().speedup(), 0.0);
        assert_eq!(SpeedMeter::default().process_speedup(), 0.0);
        assert_eq!(SpeedMeter::default().ms_per_tick(), 0.0);
    }
}
