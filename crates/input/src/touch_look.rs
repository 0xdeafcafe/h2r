//! Direct touch look (docs/runtime/ios.md, "Look"): a drag on the right side
//! of the screen turns the camera by an angle, not by a stick deflection.
//!
//! Halo 2 reads the right stick as a turn *rate*, through its own deadzone,
//! response curve, maximum rate and acceleration. A drag fed through that
//! (round 1) could not be both precise and fast: slow drags fell in the
//! deadzone and fast flicks hit the maximum rate. Here a drag's travel in
//! points becomes yaw and pitch in radians ([`drag_turn`]), which the boot's
//! touch-look hook adds to player one's facing in the player-control record
//! once a tick (`boot::touch_glue`). The game's stick path is not involved.
//!
//! * **Gain**: [`BASE_DEG_PER_POINT`] times the Touch Look Speed multiplier,
//!   times an acceleration factor ([`accel_gain`]): drags slower than
//!   [`ACCEL_FROM`] points a second turn at the base gain (precise), faster
//!   ones up to `1 + ACCEL_MAX * acceleration` times it (a flick turns far).
//!   No deadzone: every point moves the camera.
//! * **Zoom**: the turn is divided by the zoom magnification, so a drag
//!   moves the scoped view as far on screen as an unzoomed one.
//! * **On target**: while the reticle is red the turn is scaled by
//!   [`ON_TARGET_SCALE`] (aim slowdown), [`applied`].
//! * **Gyro**: the device's rotation rate times the interval is a turn too.
//!
//! The pending turn is a process-wide accumulator ([`PENDING`]): the touch
//! host adds to it, the hook takes it. Turn not taken within
//! [`STALE_TURN`] (a pause, a menu) is dropped rather than applied late.
//! [`set_consumer`] says whether the hook is installed; without it the touch
//! pad keeps the stick path (`touch::TouchPad`).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// Degrees of turn per point dragged, at Touch Look Speed 5 and before
/// acceleration: a slow drag across the right half of a phone (about 400
/// points) turns about 80 degrees.
pub const BASE_DEG_PER_POINT: f64 = 0.2;
/// Drags slower than this (points a second) get no acceleration.
pub const ACCEL_FROM: f64 = 150.0;
/// At this speed and above the acceleration is all there.
pub const ACCEL_FULL: f64 = 1500.0;
/// Extra gain at full acceleration and Look Acceleration 5 (1.0): a fast
/// flick turns three times as far per point as a slow drag.
pub const ACCEL_MAX: f64 = 2.0;
/// Turn scale while the reticle is red over an enemy.
pub const ON_TARGET_SCALE: f64 = 0.55;
/// Pending turn older than this, never taken, is dropped.
pub const STALE_TURN: Duration = Duration::from_millis(500);
/// Gyro: radians of camera turn per radian the device turns, at Gyro Aim 5.
pub const GYRO_GAIN: f64 = 1.0;
/// The shortest and longest interval counted between two touch reports
/// (for drag speed and the gyro's turn).
pub const MIN_INTERVAL: Duration = Duration::from_millis(4);
pub const MAX_INTERVAL: Duration = Duration::from_millis(50);

/// The acceleration factor for a drag at `speed` points a second with
/// Look Acceleration `acceleration` (1.0 is the default, 0 is off): 1 below
/// [`ACCEL_FROM`], rising smoothly (smoothstep) to `1 + ACCEL_MAX *
/// acceleration` at [`ACCEL_FULL`].
pub fn accel_gain(speed: f64, acceleration: f64) -> f64 {
    let a = if acceleration.is_finite() {
        acceleration.max(0.0)
    } else {
        0.0
    };
    let s = if speed.is_finite() {
        speed.max(0.0)
    } else {
        0.0
    };
    let t = ((s - ACCEL_FROM) / (ACCEL_FULL - ACCEL_FROM)).clamp(0.0, 1.0);
    1.0 + a * ACCEL_MAX * t * t * (3.0 - 2.0 * t)
}

/// A drag of `delta` points (screen space, +y down) over `dt` seconds as a
/// turn in radians: `(right, up)`, +x turns right, +y looks up (down with
/// `invert_y`). `sensitivity` multiplies the base gain (Touch Look Speed / 5).
pub fn drag_turn(
    delta: (f64, f64),
    dt: f64,
    sensitivity: f64,
    acceleration: f64,
    invert_y: bool,
) -> (f64, f64) {
    let (dx, dy) = delta;
    if !(dx.is_finite() && dy.is_finite()) || !(sensitivity.is_finite() && sensitivity > 0.0) {
        return (0.0, 0.0);
    }
    let dt = if dt.is_finite() && dt > 0.0 {
        dt
    } else {
        MAX_INTERVAL.as_secs_f64()
    };
    let speed = dx.hypot(dy) / dt;
    let k = (BASE_DEG_PER_POINT * sensitivity * accel_gain(speed, acceleration)).to_radians();
    let up = if invert_y { dy } else { -dy };
    (dx * k, up * k)
}

/// The gyro's rotation rate (radians a second, +x turn right, +y look up)
/// over `dt` seconds as a turn, at Gyro Aim `sensitivity` / 5.
pub fn gyro_turn(rate: (f32, f32), dt: f64, sensitivity: f64) -> (f64, f64) {
    if !(sensitivity.is_finite() && sensitivity > 0.0 && dt.is_finite() && dt > 0.0) {
        return (0.0, 0.0);
    }
    let axis = |v: f32| {
        let v = f64::from(v);
        if v.is_finite() {
            v * dt * sensitivity * GYRO_GAIN
        } else {
            0.0
        }
    };
    (axis(rate.0), axis(rate.1))
}

/// A pending turn as the hook applies it: divided by the zoom
/// `magnification` (1 when unzoomed; nonsense counts as 1), and scaled by
/// [`ON_TARGET_SCALE`] while `on_target`.
pub fn applied(turn: (f64, f64), magnification: f32, on_target: bool) -> (f64, f64) {
    let m = f64::from(magnification);
    let m = if m.is_finite() && m >= 1.0 { m } else { 1.0 };
    let s = if on_target { ON_TARGET_SCALE } else { 1.0 } / m;
    (turn.0 * s, turn.1 * s)
}

/// Turn the hook has not taken yet.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PendingTurn {
    turn: (f64, f64),
    since: Option<Instant>,
}

impl PendingTurn {
    /// Adds a turn at `now`. A pending turn already stale is dropped first.
    pub fn add(&mut self, now: Instant, turn: (f64, f64)) {
        if !(turn.0.is_finite() && turn.1.is_finite()) || turn == (0.0, 0.0) {
            return;
        }
        if self.stale(now) {
            *self = Self::default();
        }
        self.since.get_or_insert(now);
        self.turn.0 += turn.0;
        self.turn.1 += turn.1;
    }

    fn stale(&self, now: Instant) -> bool {
        self.since
            .is_some_and(|t| now.saturating_duration_since(t) > STALE_TURN)
    }

    /// Takes everything pending at `now`: nothing when it is stale.
    pub fn take(&mut self, now: Instant) -> (f64, f64) {
        let stale = self.stale(now);
        let turn = std::mem::take(self).turn;
        if stale { (0.0, 0.0) } else { turn }
    }

    pub fn pending(&self) -> (f64, f64) {
        self.turn
    }
}

/// The one accumulator between the touch host and the hook.
pub static PENDING: Mutex<PendingTurn> = Mutex::new(PendingTurn {
    turn: (0.0, 0.0),
    since: None,
});
static CONSUMER: AtomicBool = AtomicBool::new(false);
/// The zoom magnification (f32 bits) of player one's view; 1.0 unzoomed.
static ZOOM: AtomicU32 = AtomicU32::new(0x3f80_0000);

/// The boot's hook is installed and takes the pending turn each tick.
pub fn set_consumer(on: bool) {
    CONSUMER.store(on, Ordering::Relaxed);
}

/// Whether touch look goes direct (a hook takes the turn).
pub fn consumer() -> bool {
    CONSUMER.load(Ordering::Relaxed)
}

/// Adds a turn to the shared accumulator.
pub fn push(now: Instant, turn: (f64, f64)) {
    PENDING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .add(now, turn);
}

/// Takes the shared accumulator's turn.
pub fn take(now: Instant) -> (f64, f64) {
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).take(now)
}

/// Drops whatever is pending (the controls hid).
pub fn clear() {
    *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = PendingTurn::default();
}

/// Player one's zoom magnification, published by the camera hook.
pub fn set_zoom(magnification: f32) {
    ZOOM.store(magnification.to_bits(), Ordering::Relaxed);
}

pub fn zoom() -> f32 {
    f32::from_bits(ZOOM.load(Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deg((x, y): (f64, f64)) -> (f64, f64) {
        (x.to_degrees(), y.to_degrees())
    }

    #[test]
    fn a_slow_drag_turns_by_the_base_gain_with_no_deadzone() {
        // 10 points over 0.1 s: 100 points a second, under ACCEL_FROM.
        let (x, y) = deg(drag_turn((10.0, 0.0), 0.1, 1.0, 1.0, false));
        assert!((x - 2.0).abs() < 1e-9, "{x}");
        assert_eq!(y, 0.0);
        // A single point still turns: no deadzone.
        let (x, _) = deg(drag_turn((0.5, 0.0), 0.05, 1.0, 1.0, false));
        assert!((x - 0.1).abs() < 1e-9, "{x}");
    }

    #[test]
    fn up_looks_up_unless_inverted_and_right_turns_right() {
        let (x, y) = drag_turn((3.0, -4.0), 0.1, 1.0, 0.0, false);
        assert!(x > 0.0 && y > 0.0);
        assert!((y / x - 4.0 / 3.0).abs() < 1e-12, "direction kept");
        let (_, y) = drag_turn((0.0, -4.0), 0.1, 1.0, 0.0, true);
        assert!(y < 0.0);
    }

    #[test]
    fn acceleration_is_smooth_and_capped() {
        assert_eq!(accel_gain(0.0, 1.0), 1.0);
        assert_eq!(accel_gain(ACCEL_FROM, 1.0), 1.0);
        assert!((accel_gain(ACCEL_FULL, 1.0) - (1.0 + ACCEL_MAX)).abs() < 1e-12);
        assert_eq!(
            accel_gain(ACCEL_FULL * 10.0, 1.0),
            accel_gain(ACCEL_FULL, 1.0)
        );
        let mid = accel_gain((ACCEL_FROM + ACCEL_FULL) / 2.0, 1.0);
        assert!((mid - (1.0 + ACCEL_MAX / 2.0)).abs() < 1e-12, "{mid}");
        let mut last = 1.0;
        for s in (0..3000).step_by(10) {
            let g = accel_gain(f64::from(s), 1.0);
            assert!(g >= last, "monotonic at {s}");
            assert!(g - last < 0.05, "no jump at {s}");
            last = g;
        }
        assert_eq!(accel_gain(5000.0, 0.0), 1.0, "Look Acceleration 0 is off");
        assert_eq!(accel_gain(f64::NAN, 1.0), 1.0);
    }

    #[test]
    fn a_fast_flick_turns_far() {
        // 300 points in 0.1 s (3000 points a second): three times the base.
        let (x, _) = deg(drag_turn((300.0, 0.0), 0.1, 1.0, 1.0, false));
        assert!((x - 300.0 * 0.2 * 3.0).abs() < 1e-9, "{x}");
        // The same travel, slowly: the base gain.
        let (x, _) = deg(drag_turn((300.0, 0.0), 3.0, 1.0, 1.0, false));
        assert!((x - 60.0).abs() < 1e-9, "{x}");
    }

    #[test]
    fn sensitivity_multiplies_and_nonsense_is_zero() {
        let a = drag_turn((10.0, 0.0), 0.1, 1.0, 1.0, false).0;
        let b = drag_turn((10.0, 0.0), 0.1, 2.0, 1.0, false).0;
        assert!((b - 2.0 * a).abs() < 1e-12);
        assert_eq!(drag_turn((f64::NAN, 0.0), 0.1, 1.0, 1.0, false), (0.0, 0.0));
        assert_eq!(drag_turn((1.0, 0.0), 0.1, 0.0, 1.0, false), (0.0, 0.0));
        // A zero interval is taken as the longest one, not a division by 0.
        assert!(drag_turn((1.0, 0.0), 0.0, 1.0, 1.0, false).0.is_finite());
    }

    #[test]
    fn zoom_divides_and_on_target_slows() {
        let t = (0.2, -0.1);
        assert_eq!(applied(t, 1.0, false), t);
        let z = applied(t, 2.0, false);
        assert!((z.0 - 0.1).abs() < 1e-12 && (z.1 + 0.05).abs() < 1e-12);
        let o = applied(t, 1.0, true);
        assert!((o.0 - 0.2 * ON_TARGET_SCALE).abs() < 1e-12);
        assert_eq!(applied(t, f32::NAN, false), t);
        assert_eq!(applied(t, 0.5, false), t, "below 1 is not a zoom");
    }

    #[test]
    fn gyro_turns_by_rate_times_interval() {
        let (x, y) = gyro_turn((1.0, -0.5), 0.02, 1.0);
        assert!((x - 0.02).abs() < 1e-12 && (y + 0.01).abs() < 1e-12);
        assert_eq!(gyro_turn((1.0, 1.0), 0.02, 0.0), (0.0, 0.0));
    }

    #[test]
    fn pending_turn_adds_takes_and_drops_stale() {
        let t0 = Instant::now();
        let mut p = PendingTurn::default();
        p.add(t0, (0.1, 0.0));
        p.add(t0 + Duration::from_millis(10), (0.1, 0.2));
        assert_eq!(p.take(t0 + Duration::from_millis(20)), (0.2, 0.2));
        assert_eq!(p.take(t0 + Duration::from_millis(20)), (0.0, 0.0), "spent");
        p.add(t0, (1.0, 1.0));
        assert_eq!(p.take(t0 + STALE_TURN * 2), (0.0, 0.0), "stale is dropped");
        p.add(t0, (1.0, 1.0));
        p.add(t0 + STALE_TURN * 2, (0.5, 0.0));
        assert_eq!(
            p.pending(),
            (0.5, 0.0),
            "a stale turn is dropped when a new one comes"
        );
        p.add(t0, (f64::NAN, 0.0));
        assert_eq!(p.pending(), (0.5, 0.0));
    }
}
