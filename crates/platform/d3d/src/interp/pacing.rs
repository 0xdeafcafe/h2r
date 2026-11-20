//! When in-between frames are shown and what fraction of a tick each one
//! is at. Times are nanoseconds on one host clock; frames are stamped on
//! the game's thread when `Swap` is called (their arrival), so the render
//! thread's lag does not move them.
//!
//! - The guest frame arriving at `T(N)` is shown blended from frame `N-1`
//!   (alpha 0 at `T(N)`) to frame `N` (alpha 1 one frame interval later,
//!   when `N+1` is due). That is the one tick of delay the design takes:
//!   nothing is extrapolated, and a late frame holds at alpha 1.
//! - The interval is the measured one between the two arrivals, so a
//!   frame that ran two ticks (catch-up) blends over its longer gap.
//! - A gap longer than [`HOLD_GAP_NS`] (the guest below 30 fps, a load, a
//!   pause in host time) ends the blend: the next frame is shown as it is
//!   and starts a new pair.
//! - In-between frames are due on a fixed schedule at the display rate
//!   ([`Schedule`]); a late one skips slots, never bursts.

/// One guest frame at 30 fps.
pub const GUEST_FRAME_NS: u64 = 1_000_000_000 / 30;

/// Frames further apart than this are not blended (below about 22 fps).
pub const HOLD_GAP_NS: u64 = 45_000_000;

/// The replay rate this slice supports (docs/game/high-framerate.md,
/// "Phase 2"): 60 and above all replay at 60 until 120 has measured
/// headroom.
pub const MAX_REPLAY_HZ: u32 = 60;

/// What the frame-rate setting (`graphics.fps`, `H2_FPS`) asks of the
/// replay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rate {
    /// 30 (or anything at or below): no replay, frames as today.
    Off,
    /// In-between frames at this many a second.
    Hz(u32),
}

impl Rate {
    /// From the setting's value: 30 is off, 0 is uncapped; everything
    /// above 30 replays at [`MAX_REPLAY_HZ`] at most.
    pub fn from_setting(fps: u32) -> Rate {
        match fps {
            0 => Rate::Hz(MAX_REPLAY_HZ),
            f if f <= 30 => Rate::Off,
            f => Rate::Hz(f.min(MAX_REPLAY_HZ)),
        }
    }

    /// Nanoseconds between in-between frames.
    pub fn period_ns(self) -> Option<u64> {
        match self {
            Rate::Off => None,
            Rate::Hz(h) => Some(1_000_000_000 / h.max(1) as u64),
        }
    }
}

/// The blend fraction at `now` between the frames that arrived at `prev`
/// and `cur`: 0 at `cur`'s arrival, 1 one interval later and after.
/// Equal or reversed stamps give 1 (nothing to blend).
pub fn alpha(now: u64, prev: u64, cur: u64) -> f32 {
    if cur <= prev {
        return 1.0;
    }
    let interval = (cur - prev) as f64;
    let since = now.saturating_sub(cur) as f64;
    (since / interval).clamp(0.0, 1.0) as f32
}

/// Whether the slot at `now` should wait for the next guest frame: when
/// it falls within a quarter period of the frame expected one interval
/// after `cur`, drawing it would show alpha ~1 just before that frame
/// shows the same poses at alpha 0 (a duplicate, which judders). Returns
/// when to look again: a quarter period after the expected arrival, when a
/// frame that is still missing is late and the blend shows alpha 1.
pub fn defer(now: u64, prev: u64, cur: u64, period: u64) -> Option<u64> {
    if cur <= prev {
        return None;
    }
    let expected = cur + (cur - prev);
    let margin = period / 4;
    (now + margin >= expected && now < expected + margin).then_some(expected + margin)
}

/// Whether frames that arrived at `prev` and `cur` may be blended.
pub fn blendable(prev: u64, cur: u64) -> bool {
    cur > prev && cur - prev <= HOLD_GAP_NS
}

/// Display slots at a fixed period.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Schedule {
    next: Option<u64>,
}

impl Schedule {
    /// Whether a slot is due at `now`; if so, the next slot is the first
    /// one after `now` on the grid (late calls skip slots).
    pub fn due(&mut self, now: u64, period: u64) -> bool {
        let period = period.max(1);
        let next = *self.next.get_or_insert(now);
        if now < next {
            return false;
        }
        let behind = (now - next) / period;
        self.next = Some(next + (behind + 1) * period);
        true
    }

    /// Whether a slot is due at `now`, without taking it.
    pub fn is_due(&self, now: u64) -> bool {
        self.next.is_none_or(|n| now >= n)
    }

    /// When the next slot is due, once one has been taken.
    pub fn deadline(&self) -> Option<u64> {
        self.next
    }

    /// Puts the grid's next slot one period after `now` (a frame was just
    /// shown).
    pub fn shown_at(&mut self, now: u64, period: u64) {
        self.next = Some(now + period.max(1));
    }
}

/// A display frame in [`simulate`]: when, and at what alpha (`None`: the
/// guest's own frame shown as it is).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shown {
    pub at: u64,
    pub alpha: Option<f32>,
    /// Index of the guest frame it is based on.
    pub frame: usize,
}

/// What the replay shows for guest frames arriving at `arrivals` (sorted),
/// with the display at `rate`, until `end`: at each arrival a frame (blended
/// at its alpha when the pair is blendable), then in-between frames on the
/// schedule until alpha reaches 1. The same rules as the backend, without a
/// GPU, for tests and for reasoning about pacing.
pub fn simulate(arrivals: &[u64], rate: Rate, end: u64) -> Vec<Shown> {
    let mut out = Vec::new();
    let Some(period) = rate.period_ns() else {
        return arrivals
            .iter()
            .enumerate()
            .map(|(i, &at)| Shown {
                at,
                alpha: None,
                frame: i,
            })
            .collect();
    };
    let mut sched = Schedule::default();
    for (i, &at) in arrivals.iter().enumerate() {
        let blend = i > 0 && blendable(arrivals[i - 1], at);
        if !blend {
            out.push(Shown {
                at,
                alpha: None,
                frame: i,
            });
            continue;
        }
        let prev = arrivals[i - 1];
        let a = alpha(at, prev, at);
        out.push(Shown {
            at,
            alpha: Some(a),
            frame: i,
        });
        sched.shown_at(at, period);
        let stop = arrivals.get(i + 1).copied().unwrap_or(end);
        let mut last = a;
        while last < 1.0 {
            let mut t = sched.deadline().expect("a slot after a shown frame");
            if t >= stop {
                break;
            }
            if let Some(w) = defer(t, prev, at, period) {
                if w >= stop {
                    break;
                }
                t = w;
            }
            assert!(sched.due(t, period));
            last = alpha(t, prev, at);
            out.push(Shown {
                at: t,
                alpha: Some(last),
                frame: i,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    #[test]
    fn settings_map_to_rates() {
        assert_eq!(Rate::from_setting(30), Rate::Off);
        assert_eq!(Rate::from_setting(60), Rate::Hz(60));
        assert_eq!(
            Rate::from_setting(120),
            Rate::Hz(60),
            "120 waits on measured headroom"
        );
        assert_eq!(Rate::from_setting(0), Rate::Hz(60), "uncapped");
    }

    #[test]
    fn alpha_is_zero_at_arrival_and_one_an_interval_later() {
        let (p, c) = (100 * MS, 133 * MS);
        assert_eq!(alpha(c, p, c), 0.0);
        assert!((alpha(c + 33 * MS / 2, p, c) - 0.5).abs() < 1e-6);
        assert_eq!(alpha(c + 33 * MS, p, c), 1.0);
        assert_eq!(
            alpha(c + 500 * MS, p, c),
            1.0,
            "a late frame holds, never extrapolates"
        );
        assert_eq!(alpha(c - 5 * MS, p, c), 0.0);
        assert_eq!(alpha(c, c, c), 1.0, "no interval: nothing to blend");
    }

    #[test]
    fn the_schedule_skips_late_slots() {
        let mut s = Schedule::default();
        assert!(s.due(0, 16 * MS));
        assert!(!s.due(10 * MS, 16 * MS));
        assert!(s.due(16 * MS, 16 * MS));
        // 50 ms late: one frame, then back on the grid.
        assert!(s.due(82 * MS, 16 * MS));
        assert_eq!(s.deadline(), Some(96 * MS));
    }

    fn ticks(n: usize) -> Vec<u64> {
        (0..n).map(|i| i as u64 * GUEST_FRAME_NS).collect()
    }

    #[test]
    fn sixty_shows_two_frames_per_tick() {
        let shown = simulate(&ticks(5), Rate::Hz(60), 5 * GUEST_FRAME_NS);
        assert_eq!(
            shown[0].alpha, None,
            "the first frame has nothing to blend from"
        );
        for f in 1..4 {
            let per: Vec<f32> = shown
                .iter()
                .filter(|s| s.frame == f)
                .map(|s| s.alpha.unwrap())
                .collect();
            assert_eq!(per.len(), 2, "frame {f}: {per:?}");
            assert_eq!(per[0], 0.0);
            assert!((per[1] - 0.5).abs() < 1e-3, "{per:?}");
        }
    }

    #[test]
    fn off_shows_each_frame_once() {
        let shown = simulate(&ticks(4), Rate::Off, 4 * GUEST_FRAME_NS);
        assert_eq!(shown.len(), 4);
        assert!(shown.iter().all(|s| s.alpha.is_none()));
    }

    #[test]
    fn a_catch_up_frame_blends_over_its_longer_gap() {
        // Frame 2 comes 40 ms after frame 1 (it ran two ticks' worth of
        // budget late): blended over 40 ms, at 60 Hz three frames.
        let arrivals = [0, 33 * MS, 73 * MS, 113 * MS];
        let shown = simulate(&arrivals, Rate::Hz(60), 140 * MS);
        let f2: Vec<f32> = shown
            .iter()
            .filter(|s| s.frame == 2)
            .map(|s| s.alpha.unwrap())
            .collect();
        assert_eq!(f2.len(), 3, "{f2:?}");
        assert!((f2[1] - 16.666 / 40.0).abs() < 1e-3, "{f2:?}");
        assert!((f2[2] - 33.333 / 40.0).abs() < 1e-3, "{f2:?}");
    }

    #[test]
    fn a_pause_or_a_slow_guest_holds_the_newest_frame() {
        // 200 ms with no frame (a load, a pacer rebase), then 30 Hz again.
        let arrivals = [0, 33 * MS, 233 * MS, 266 * MS];
        let shown = simulate(&arrivals, Rate::Hz(60), 290 * MS);
        let f2: Vec<_> = shown.iter().filter(|s| s.frame == 2).collect();
        assert_eq!(f2.len(), 1);
        assert_eq!(f2[0].alpha, None, "shown as it is, no blend across the gap");
        // Frame 1's blend stops at alpha 1 and nothing is shown in the gap.
        let f1_last = shown.iter().rfind(|s| s.frame == 1).unwrap();
        assert_eq!(f1_last.alpha, Some(1.0));
        assert!(f1_last.at <= 75 * MS);
        // Frame 3 blends again.
        assert_eq!(shown.iter().filter(|s| s.frame == 3).count(), 2);
    }

    #[test]
    fn a_late_frame_holds_at_one() {
        // Frame 2 is 10 ms late (still blendable): frame 1 reaches alpha 1
        // and holds; nothing past 1 is drawn.
        let f = GUEST_FRAME_NS;
        let arrivals = [0, f, f + 43 * MS];
        let shown = simulate(&arrivals, Rate::Hz(60), 100 * MS);
        let f1: Vec<f32> = shown
            .iter()
            .filter(|s| s.frame == 1)
            .map(|s| s.alpha.unwrap())
            .collect();
        assert_eq!(f1.len(), 3, "{f1:?}");
        assert_eq!(f1[0], 0.0);
        assert!((f1[1] - 0.5).abs() < 1e-3 && f1[2] == 1.0, "{f1:?}");
    }
}
