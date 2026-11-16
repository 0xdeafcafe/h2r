//! Relative look motion turned into a right-stick turn rate: the path mouse
//! look takes (`backend::keyboard`), shared with the Steam Controller's
//! right trackpad (`steam_pad`, `trackpad`).
//!
//! Halo reads aim as a stick, which it treats as a turn rate. Relative
//! motion (mouse counts, trackpad travel) is held until the guest has read a
//! state carrying it, and becomes a deflection proportional to its speed
//! over the guest's read interval: `delta * k * (1/60 s) / interval`, the
//! interval counted as at least [`LOOK_FRAME`]. Motion left unread for over
//! [`STALE_MOTION`] is dropped rather than delivered late as a flick. See
//! `docs/input/input.md`, "Keyboard and mouse".

use std::time::{Duration, Instant};

/// The read interval at which `delta * k` is the deflection.
pub const LOOK_FRAME: Duration = Duration::from_nanos(1_000_000_000 / 60);

/// Motion older than this, with no guest read in between, is dropped.
pub const STALE_MOTION: Duration = Duration::from_secs(1);

/// Motion the guest has not read yet.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LookMotion {
    /// Unread motion, screen space (+y down).
    delta: (f64, f64),
    /// The part of `delta` in the newest reported state.
    sampled: (f64, f64),
    /// When the current read interval began: the last guest read, or the
    /// first motion after none was pending.
    since: Option<Instant>,
}

impl LookMotion {
    /// Adds motion at `now` (screen space, +y down). Non-finite motion is
    /// ignored; motion after a stale interval starts a new one.
    pub fn add(&mut self, now: Instant, dx: f64, dy: f64) {
        if !(dx.is_finite() && dy.is_finite()) {
            return;
        }
        if self
            .since
            .is_none_or(|t| now.saturating_duration_since(t) > STALE_MOTION)
        {
            self.clear();
            self.since = Some(now);
        }
        self.delta.0 += dx;
        self.delta.1 += dy;
    }

    /// The deflection for the state about to be reported: unread motion
    /// times `k` (deflection per unit per [`LOOK_FRAME`]) over the read
    /// interval, +y up (motion up looks up) unless `invert_y`. Unclamped:
    /// the caller scales the stick back to the unit circle, keeping a fast
    /// flick's direction. Remembers what it reported for [`read`](Self::read).
    pub fn stick(&mut self, now: Instant, k: f64, invert_y: bool) -> (f64, f64) {
        self.sampled = self.delta;
        if self.delta == (0.0, 0.0) {
            return (0.0, 0.0);
        }
        let sign = if invert_y { 1.0 } else { -1.0 };
        let interval = self
            .since
            .map_or(LOOK_FRAME, |t| now.saturating_duration_since(t))
            .max(LOOK_FRAME);
        let k = k * LOOK_FRAME.as_secs_f64() / interval.as_secs_f64();
        (self.delta.0 * k, sign * self.delta.1 * k)
    }

    /// The guest read the newest state: spend the motion it carried and
    /// start the next read interval.
    pub fn read(&mut self, now: Instant) {
        self.delta.0 -= self.sampled.0;
        self.delta.1 -= self.sampled.1;
        self.sampled = (0.0, 0.0);
        self.since = Some(now);
    }

    /// Drops everything pending.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Unread motion.
    pub fn pending(&self) -> (f64, f64) {
        self.delta
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn held_until_read_then_spent() {
        let t0 = Instant::now();
        let mut m = LookMotion::default();
        m.add(t0, 5.0, -10.0);
        assert_eq!(m.stick(t0, 0.04, false), (5.0 * 0.04, 10.0 * 0.04));
        assert_eq!(m.stick(t0, 0.04, false), (5.0 * 0.04, 10.0 * 0.04));
        m.read(t0);
        assert_eq!(m.stick(t0, 0.04, false), (0.0, 0.0));
        assert_eq!(m.pending(), (0.0, 0.0));
    }

    #[test]
    fn a_longer_read_interval_lowers_the_rate() {
        let t0 = Instant::now();
        let mut m = LookMotion::default();
        m.read(t0);
        m.add(t0, 6.0, 0.0);
        let at = t0 + LOOK_FRAME * 2;
        let (x, _) = m.stick(at, 0.1, false);
        assert!((x - 0.3).abs() < 1e-9, "{x}");
        let (_, y) = {
            let mut i = LookMotion::default();
            i.add(t0, 0.0, 1.0);
            i.stick(t0, 1.0, true)
        };
        assert_eq!(y, 1.0, "inverted: down looks down");
    }

    #[test]
    fn stale_motion_is_dropped_and_bad_motion_ignored() {
        let t0 = Instant::now();
        let mut m = LookMotion::default();
        m.add(t0, 100.0, 0.0);
        m.add(t0 + STALE_MOTION * 2, 1.0, 0.0);
        assert_eq!(m.pending(), (1.0, 0.0));
        m.add(t0, f64::NAN, 3.0);
        assert_eq!(m.pending(), (1.0, 0.0));
    }
}
