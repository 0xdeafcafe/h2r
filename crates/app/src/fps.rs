//! The display frame-rate setting (`H2_FPS`) and the display clock
//! (docs/game/high-framerate.md).
//!
//! The guest always runs at 30 Hz: the same 60 Hz logical vblanks, the same
//! two-vblank frame interval, one tick per frame, one input poll per frame.
//! Nothing here reaches it. [`FpsTarget`] only decides how often the
//! *window* redraws:
//!
//! * `30` (the default) is exactly what the shell always did: a redraw when
//!   the guest presents (and for the overlay and watermark), under the
//!   `vsync` setting.
//! * `60` and `120` redraw on their own clock ([`DisplaySchedule`]) as well,
//!   at that rate, under the `vsync` setting.
//! * `uncapped` redraws as fast as the event loop turns, with vsync off.
//!
//! Until phase 2 (host replay with interpolated camera and object matrices)
//! a redraw between guest presents shows the newest guest frame again, so
//! above 30 the picture is no smoother: only present pacing changes. What
//! is here is the setting, the display clock and the interpolation
//! fraction ([`alpha`]) that phase 2 will use.
//!
//! The target can change while the window runs: [`FpsControl`] carries a
//! new one from another thread (the Video screen's Frame Rate row), and
//! the window picks it up at its next turn ([`follow`]): the schedule is
//! retargeted and, when vsync must change (uncapped and back), the window
//! surface is configured again.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

/// The environment variable [`FpsTarget::from_env`] reads.
pub const ENV: &str = "H2_FPS";

/// How often the window presents.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FpsTarget {
    /// One redraw per guest present: today's behaviour.
    #[default]
    Thirty,
    Sixty,
    OneTwenty,
    /// Every turn of the event loop, vsync off.
    Uncapped,
}

impl FpsTarget {
    pub const ALL: [FpsTarget; 4] = [
        FpsTarget::Thirty,
        FpsTarget::Sixty,
        FpsTarget::OneTwenty,
        FpsTarget::Uncapped,
    ];

    /// `30`, `60`, `120` or `uncapped` (case-insensitive; empty is `30`).
    pub fn parse(s: &str) -> Result<FpsTarget, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "30" => Ok(FpsTarget::Thirty),
            "60" => Ok(FpsTarget::Sixty),
            "120" => Ok(FpsTarget::OneTwenty),
            "uncapped" | "unlimited" | "max" => Ok(FpsTarget::Uncapped),
            other => Err(format!("{ENV}={other:?}: expected 30, 60, 120 or uncapped")),
        }
    }

    /// From `H2_FPS`; unset means `30`.
    pub fn from_env() -> Result<FpsTarget, String> {
        match std::env::var(ENV) {
            Ok(v) => Self::parse(&v),
            Err(std::env::VarError::NotPresent) => Ok(FpsTarget::Thirty),
            Err(e) => Err(format!("{ENV}: {e}")),
        }
    }

    /// The name [`parse`](Self::parse) takes.
    pub fn name(self) -> &'static str {
        match self {
            FpsTarget::Thirty => "30",
            FpsTarget::Sixty => "60",
            FpsTarget::OneTwenty => "120",
            FpsTarget::Uncapped => "uncapped",
        }
    }

    /// Time between redraws of the window's own clock. `None` at 30: the
    /// window redraws when the guest presents, as it always has. Zero for
    /// uncapped.
    pub fn display_interval(self) -> Option<Duration> {
        match self {
            FpsTarget::Thirty => None,
            FpsTarget::Sixty => Some(Duration::from_nanos(1_000_000_000 / 60)),
            FpsTarget::OneTwenty => Some(Duration::from_nanos(1_000_000_000 / 120)),
            FpsTarget::Uncapped => Some(Duration::ZERO),
        }
    }

    /// Whether the window should wait for the display's vblank, given the
    /// `vsync` setting. Uncapped never does.
    pub fn vsync(self, vsync: bool) -> bool {
        vsync && self != FpsTarget::Uncapped
    }
}

/// A frame-rate target that another thread sets and the window follows.
/// Cheap to clone; every clone is the same cell.
#[derive(Clone, Debug)]
pub struct FpsControl(Arc<AtomicU8>);

impl FpsControl {
    pub fn new(target: FpsTarget) -> Self {
        Self(Arc::new(AtomicU8::new(Self::index(target))))
    }

    fn index(target: FpsTarget) -> u8 {
        FpsTarget::ALL
            .iter()
            .position(|t| *t == target)
            .unwrap_or(0) as u8
    }

    pub fn set(&self, target: FpsTarget) {
        self.0.store(Self::index(target), Ordering::Release);
    }

    pub fn get(&self) -> FpsTarget {
        FpsTarget::ALL
            .get(self.0.load(Ordering::Acquire) as usize)
            .copied()
            .unwrap_or_default()
    }
}

/// What the window must do when `control` asks for another target than
/// `current`: the new target, and the surface's new vsync when the
/// `vsync` setting under it changes (uncapped turns it off). `None`: no
/// change.
pub fn follow(
    control: &FpsControl,
    current: FpsTarget,
    vsync: bool,
) -> Option<(FpsTarget, Option<bool>)> {
    let wanted = control.get();
    if wanted == current {
        return None;
    }
    let (was, now) = (current.vsync(vsync), wanted.vsync(vsync));
    Some((wanted, (was != now).then_some(now)))
}

impl std::fmt::Display for FpsTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// Where display time comes from. The window uses [`WallClock`]; tests and
/// the determinism harness use [`FakeDisplayClock`], so "the presenter ran
/// at 120" is reproducible.
pub trait DisplayClock {
    fn now(&mut self) -> Duration;
}

/// Wall time since the clock was made.
pub struct WallClock(Instant);

impl WallClock {
    pub fn new() -> Self {
        Self(Instant::now())
    }
}

impl Default for WallClock {
    fn default() -> Self {
        Self::new()
    }
}

impl DisplayClock for WallClock {
    fn now(&mut self) -> Duration {
        self.0.elapsed()
    }
}

/// A clock that moves `step` on every read, starting at zero.
#[derive(Clone, Debug)]
pub struct FakeDisplayClock {
    now: Duration,
    step: Duration,
}

impl FakeDisplayClock {
    pub fn new(step: Duration) -> Self {
        Self {
            now: Duration::ZERO,
            step,
        }
    }
}

impl DisplayClock for FakeDisplayClock {
    fn now(&mut self) -> Duration {
        let t = self.now;
        self.now += self.step;
        t
    }
}

/// When the window's own redraws are due.
#[derive(Clone, Debug)]
pub struct DisplaySchedule {
    interval: Option<Duration>,
    next: Option<Duration>,
    /// Redraws the schedule has asked for.
    pub due_count: u64,
    /// Due times skipped because the window fell more than an interval
    /// behind (a slow redraw); the schedule moves on instead of bursting.
    pub skipped: u64,
}

impl DisplaySchedule {
    pub fn new(target: FpsTarget) -> Self {
        Self {
            interval: target.display_interval(),
            next: None,
            due_count: 0,
            skipped: 0,
        }
    }

    /// Runs at `target` from the next call to [`due`](Self::due): the
    /// grid starts again there, so a lower rate does not wait out a slot
    /// booked at the higher one.
    pub fn retarget(&mut self, target: FpsTarget) {
        self.interval = target.display_interval();
        self.next = None;
    }

    /// Whether a redraw is due at `now`; if so, the next one is booked.
    /// Never due at 30 (the guest's presents drive redraws there).
    pub fn due(&mut self, now: Duration) -> bool {
        let Some(interval) = self.interval else {
            return false;
        };
        let next = *self.next.get_or_insert(now);
        if now < next {
            return false;
        }
        let mut following = next + interval;
        if !interval.is_zero() && now >= following {
            // More than one interval late: skip the missed slots.
            let late = (now - next).as_nanos() / interval.as_nanos();
            self.skipped += late as u64;
            following = next + interval * late as u32 + interval;
        }
        self.next = Some(following);
        self.due_count += 1;
        true
    }

    /// When the next redraw is due (`None` at 30, or before the first call
    /// to [`due`](Self::due)).
    pub fn next_due(&self) -> Option<Duration> {
        self.next
    }
}

/// Phase 2's interpolation fraction: where display time `display` (guest
/// ticks, already moved back by the one-tick interpolation delay) falls
/// between the guest frames stamped `previous` and `newest`. Clamped to
/// [0, 1]; never extrapolates. A frame that ran no tick (`previous ==
/// newest`: pause, load) shows the newest frame (1).
pub fn alpha(display: u64, previous: u64, newest: u64) -> f32 {
    if newest <= previous || display >= newest {
        return 1.0;
    }
    if display <= previous {
        return 0.0;
    }
    ((display - previous) as f64 / (newest - previous) as f64) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_four_settings_and_defaults_to_today() {
        assert_eq!(FpsTarget::default(), FpsTarget::Thirty);
        for t in FpsTarget::ALL {
            assert_eq!(FpsTarget::parse(t.name()), Ok(t));
        }
        assert_eq!(FpsTarget::parse(""), Ok(FpsTarget::Thirty));
        assert_eq!(FpsTarget::parse(" Uncapped "), Ok(FpsTarget::Uncapped));
        for bad in ["29", "144", "fast", "-1"] {
            assert!(
                FpsTarget::parse(bad).unwrap_err().contains("H2_FPS"),
                "{bad}"
            );
        }
    }

    #[test]
    fn thirty_is_the_shell_as_it_was() {
        let t = FpsTarget::Thirty;
        assert_eq!(t.display_interval(), None);
        assert!(t.vsync(true));
        assert!(!t.vsync(false));
        let mut s = DisplaySchedule::new(t);
        for ms in 0..1000 {
            assert!(!s.due(Duration::from_millis(ms)));
        }
        assert_eq!(s.next_due(), None);
        assert!(!FpsTarget::Uncapped.vsync(true));
        assert!(FpsTarget::OneTwenty.vsync(true));
    }

    /// Drives a schedule from a fake clock polled every `poll` for one second.
    fn redraws(target: FpsTarget, poll: Duration) -> DisplaySchedule {
        let mut clock = FakeDisplayClock::new(poll);
        let mut s = DisplaySchedule::new(target);
        loop {
            let now = clock.now();
            if now >= Duration::from_secs(1) {
                return s;
            }
            s.due(now);
        }
    }

    #[test]
    fn sixty_and_120_redraw_at_their_rate_on_a_fake_clock() {
        let poll = Duration::from_micros(250);
        assert_eq!(redraws(FpsTarget::Sixty, poll).due_count, 60);
        assert_eq!(redraws(FpsTarget::OneTwenty, poll).due_count, 120);
        assert_eq!(
            redraws(FpsTarget::Uncapped, poll).due_count,
            4000,
            "every poll"
        );
        assert_eq!(redraws(FpsTarget::OneTwenty, poll).skipped, 0);
    }

    #[test]
    fn a_late_window_skips_slots_instead_of_bursting() {
        let mut s = DisplaySchedule::new(FpsTarget::OneTwenty);
        assert!(s.due(Duration::ZERO));
        // 50 ms stall: one redraw, five missed slots, then back on the grid.
        assert!(s.due(Duration::from_millis(50)));
        assert!(!s.due(Duration::from_millis(50)));
        assert_eq!(s.skipped, 5);
        let next = s.next_due().unwrap();
        assert!(
            next > Duration::from_millis(50) && next <= Duration::from_millis(59),
            "{next:?}"
        );
    }

    #[test]
    fn a_control_carries_every_target_across_clones() {
        let c = FpsControl::new(FpsTarget::Thirty);
        let other = c.clone();
        for t in FpsTarget::ALL {
            other.set(t);
            assert_eq!(c.get(), t);
        }
    }

    #[test]
    fn follow_says_what_changes_and_when_vsync_does() {
        let c = FpsControl::new(FpsTarget::Thirty);
        assert_eq!(follow(&c, FpsTarget::Thirty, true), None);
        c.set(FpsTarget::Sixty);
        assert_eq!(
            follow(&c, FpsTarget::Thirty, true),
            Some((FpsTarget::Sixty, None))
        );
        c.set(FpsTarget::Uncapped);
        assert_eq!(
            follow(&c, FpsTarget::Sixty, true),
            Some((FpsTarget::Uncapped, Some(false))),
            "uncapped turns vsync off"
        );
        c.set(FpsTarget::OneTwenty);
        assert_eq!(
            follow(&c, FpsTarget::Uncapped, true),
            Some((FpsTarget::OneTwenty, Some(true))),
            "and back on"
        );
        assert_eq!(
            follow(&c, FpsTarget::Uncapped, false),
            Some((FpsTarget::OneTwenty, None)),
            "vsync off in the settings: it stays off"
        );
    }

    #[test]
    fn a_retargeted_schedule_runs_at_the_new_rate_at_once() {
        let poll = Duration::from_micros(250);
        let mut clock = FakeDisplayClock::new(poll);
        let mut s = DisplaySchedule::new(FpsTarget::Thirty);
        let count = |s: &mut DisplaySchedule, clock: &mut FakeDisplayClock, until: Duration| {
            let start = s.due_count;
            loop {
                let now = clock.now();
                if now >= until {
                    return s.due_count - start;
                }
                s.due(now);
            }
        };
        // 30: the guest's presents drive redraws; the schedule never does.
        assert_eq!(count(&mut s, &mut clock, Duration::from_secs(1)), 0);
        s.retarget(FpsTarget::OneTwenty);
        assert_eq!(count(&mut s, &mut clock, Duration::from_secs(2)), 120);
        s.retarget(FpsTarget::Sixty);
        assert_eq!(count(&mut s, &mut clock, Duration::from_secs(3)), 60);
        s.retarget(FpsTarget::Thirty);
        assert_eq!(count(&mut s, &mut clock, Duration::from_secs(4)), 0);
        assert_eq!(s.next_due(), None, "nothing booked at 30");
    }

    #[test]
    fn alpha_is_clamped_and_never_extrapolates() {
        assert_eq!(alpha(150, 100, 200), 0.5);
        assert_eq!(alpha(100, 100, 200), 0.0);
        assert_eq!(alpha(50, 100, 200), 0.0);
        assert_eq!(alpha(200, 100, 200), 1.0);
        assert_eq!(alpha(10_000, 100, 200), 1.0, "a late frame holds at 1");
        assert_eq!(alpha(150, 200, 200), 1.0, "no tick ran");
        assert_eq!(alpha(150, 300, 200), 1.0);
        // Two ticks in one frame (catch-up) interpolate over both.
        assert_eq!(alpha(100 + 333_333, 100, 100 + 2 * 333_333), 0.5);
    }
}
