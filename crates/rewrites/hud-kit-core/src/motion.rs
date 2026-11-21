//! How a value moves between ticks (docs/game/hud-kit.md, "Motion"): a
//! [`Motion`] and the curves the draw hooks sample, pure functions of the
//! milliseconds since the value changed.
//!
//! The logic stays per tick: a HUD publishes a target fill and a motion at
//! 30 Hz. The runtime stamps a changed value with the wall time and the
//! fill it was showing ([`restamp`]); each drawn frame then asks
//! [`animate`] and [`scale_at`] at the milliseconds since that stamp, so a
//! curve is the same at 30, 60 or 120 frames a second, only sampled more
//! often. Every step is at least 60 ms, so at 30 a blink or the swell
//! still gets two frames.
//!
//! Guest-free, as the rest of the crate: no clock is read here.

use crate::model::Value;

/// How a value's fill, visibility and size move after it changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Motion {
    /// The value as it is.
    #[default]
    None,
    /// The fill eases from the fill shown to the new one in [`EASE_MS`].
    Ease,
    /// The arming bar's primed ending ([`ending`]): the fill sweeps to
    /// full, the bar swells, blinks once and closes to its centre.
    Ending,
    /// Left before the end: the fill runs out in [`RUN_OUT_MS`], the
    /// value is hidden from [`RUN_OUT_GONE_MS`].
    RunOut,
    /// Lost: the value blinks [`BLINKS`] times in [`BLINK_MS`], then is
    /// hidden.
    Blink,
}

impl Motion {
    pub fn name(self) -> &'static str {
        match self {
            Motion::None => "none",
            Motion::Ease => "ease",
            Motion::Ending => "ending",
            Motion::RunOut => "run out",
            Motion::Blink => "blink",
        }
    }
}

/// A fill step's ease.
pub const EASE_MS: f32 = 200.0;
/// [`Motion::RunOut`]: the fill is 0 at this, the value gone at the next.
pub const RUN_OUT_MS: f32 = 200.0;
pub const RUN_OUT_GONE_MS: f32 = 300.0;
/// [`Motion::Blink`]: shown, off, three times, each half a sixth of it.
pub const BLINKS: u32 = 3;
pub const BLINK_MS: f32 = 500.0;

/// [`Motion::Ending`]'s table (the arming design's "The ending").
pub mod ending {
    /// The fill sweeps from the fill shown to full.
    pub const SWEEP_END: f32 = 130.0;
    /// The bar swells and settles.
    pub const SWELL_START: f32 = 130.0;
    pub const SWELL_END: f32 = 290.0;
    /// The swell's peak, wide and tall: 12% and 30% over.
    pub const SWELL: [f32; 2] = [0.12, 0.30];
    /// One blink off.
    pub const BLINK_START: f32 = 290.0;
    pub const BLINK_END: f32 = 360.0;
    /// The bar closes to its centre, across, and is gone.
    pub const CLOSE_START: f32 = 430.0;
    pub const CLOSE_END: f32 = 600.0;
}

/// The ease of `t` (0 to 1, clamped): smoothstep, never past its ends and
/// never going back.
pub fn ease(t: f32) -> f32 {
    let t = if t.is_nan() { 1.0 } else { t.clamp(0.0, 1.0) };
    t * t * (3.0 - 2.0 * t)
}

fn lerp(from: f32, to: f32, t: f32) -> f32 {
    if t >= 1.0 { to } else { from + (to - from) * t }
}

/// The fill `ms` after a change from `from` to `to` under `motion`.
pub fn fill_at(motion: Motion, from: f32, to: f32, ms: f32) -> f32 {
    match motion {
        Motion::None | Motion::Blink => to,
        Motion::Ease => lerp(from, to, ease(ms / EASE_MS)),
        Motion::Ending => lerp(from, to, ease(ms / ending::SWEEP_END)),
        Motion::RunOut => lerp(from, to, ease(ms / RUN_OUT_MS)),
    }
}

/// Whether the value shows `ms` after the change. `scaled`: the runtime
/// can scale the widget ([`scale_at`] is applied); without it the ending
/// is the plain one, gone where the close would start.
pub fn shown_at(motion: Motion, ms: f32, scaled: bool) -> bool {
    match motion {
        Motion::None | Motion::Ease => true,
        Motion::Ending => {
            let gone = if scaled {
                ending::CLOSE_END
            } else {
                ending::CLOSE_START
            };
            !(ending::BLINK_START..ending::BLINK_END).contains(&ms) && ms < gone
        }
        Motion::RunOut => ms < RUN_OUT_GONE_MS,
        Motion::Blink => {
            let slot = BLINK_MS / (2 * BLINKS) as f32;
            ms < BLINK_MS && ((ms.max(0.0) / slot) as u32).is_multiple_of(2)
        }
    }
}

/// The widget's size `ms` after the change, wide and tall, 1 its own.
pub fn scale_at(motion: Motion, ms: f32) -> [f32; 2] {
    if motion != Motion::Ending {
        return [1.0, 1.0];
    }
    if (ending::SWELL_START..ending::SWELL_END).contains(&ms) {
        let u = (ms - ending::SWELL_START) / (ending::SWELL_END - ending::SWELL_START);
        let bump = (std::f32::consts::PI * u).sin().max(0.0);
        return ending::SWELL.map(|s| 1.0 + s * bump);
    }
    if ms >= ending::CLOSE_START {
        let u = (ms - ending::CLOSE_START) / (ending::CLOSE_END - ending::CLOSE_START);
        return [1.0 - ease(u), 1.0];
    }
    [1.0, 1.0]
}

/// `v` as it draws `ms` after its stamp: its fill on its curve, hidden
/// where its curve hides it.
pub fn animate(v: &Value, ms: f32, scaled: bool) -> Value {
    if v.motion == Motion::None {
        return v.clone();
    }
    Value {
        fill: fill_at(v.motion, v.from, v.fill, ms),
        shown: v.shown && shown_at(v.motion, ms, scaled),
        ..v.clone()
    }
}

/// Whether `new` is a change of `old` the motion starts again on: shown,
/// fill or motion.
pub fn changed(old: &Value, new: &Value) -> bool {
    old.shown != new.shown || old.fill != new.fill || old.motion != new.motion
}

/// A published value against the one before it, `old` and the
/// milliseconds since its stamp: `Some(stamped)` when it changed (its
/// `from` the fill `old` was showing then, 0 if it was not showing), else
/// `None` and the old stamp holds (with `old`'s `from`). The runtime asks
/// only for a value with a motion.
pub fn restamp(old: Option<(&Value, f32)>, new: &Value, scaled: bool) -> Option<Value> {
    let from = match old {
        Some((o, _)) if !changed(o, new) => return None,
        Some((o, ms)) => {
            let a = animate(o, ms, scaled);
            if a.shown { a.fill } else { 0.0 }
        }
        None => 0.0,
    };
    Some(Value {
        from,
        ..new.clone()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    /// Each curve at its key times.
    #[test]
    fn the_curves_hold_their_tables() {
        // The ease: from, half way, to.
        assert_eq!(fill_at(Motion::Ease, 0.0, 1.0, 0.0), 0.0);
        assert!(close(fill_at(Motion::Ease, 0.0, 1.0, EASE_MS / 2.0), 0.5));
        assert_eq!(
            fill_at(Motion::Ease, 1.0 / 3.0, 2.0 / 3.0, EASE_MS),
            2.0 / 3.0
        );
        assert_eq!(fill_at(Motion::None, 0.2, 0.7, 0.0), 0.7);
        // The ending: the sweep to full by 130, the swell's peak at 210,
        // the blink off 290 to 360, the close from 430, gone at 600.
        assert_eq!(fill_at(Motion::Ending, 2.0 / 3.0, 1.0, 0.0), 2.0 / 3.0);
        assert_eq!(fill_at(Motion::Ending, 2.0 / 3.0, 1.0, 130.0), 1.0);
        assert_eq!(scale_at(Motion::Ending, 100.0), [1.0, 1.0]);
        let peak = scale_at(Motion::Ending, 210.0);
        assert!(close(peak[0], 1.12) && close(peak[1], 1.30), "{peak:?}");
        assert_eq!(scale_at(Motion::Ending, 300.0), [1.0, 1.0]);
        assert_eq!(scale_at(Motion::Ending, 430.0), [1.0, 1.0]);
        let mid = scale_at(Motion::Ending, 515.0);
        assert!(close(mid[0], 0.5) && mid[1] == 1.0, "{mid:?}");
        assert!(close(scale_at(Motion::Ending, 600.0)[0], 0.0));
        for (ms, shown) in [
            (0.0, true),
            (289.0, true),
            (290.0, false),
            (359.0, false),
            (360.0, true),
            (599.0, true),
            (600.0, false),
        ] {
            assert_eq!(shown_at(Motion::Ending, ms, true), shown, "{ms}");
        }
        // The plain ending, with no scale: gone where the close starts.
        assert!(shown_at(Motion::Ending, 429.0, false));
        assert!(!shown_at(Motion::Ending, 430.0, false));
        // The run out: empty at 200, gone at 300.
        assert_eq!(fill_at(Motion::RunOut, 2.0 / 3.0, 0.0, 200.0), 0.0);
        assert!(fill_at(Motion::RunOut, 2.0 / 3.0, 0.0, 100.0) > 0.0);
        assert!(shown_at(Motion::RunOut, 299.0, true));
        assert!(!shown_at(Motion::RunOut, 300.0, true));
        // The blink: on, off, three times, gone at 500.
        let on: Vec<bool> = (0..6)
            .map(|k| shown_at(Motion::Blink, k as f32 * BLINK_MS / 6.0 + 1.0, true))
            .collect();
        assert_eq!(on, [true, false, true, false, true, false]);
        assert!(!shown_at(Motion::Blink, BLINK_MS, true));
        assert_eq!(fill_at(Motion::Blink, 0.5, 0.0, 10.0), 0.0);
        // Nothing but the ending scales.
        for m in [Motion::None, Motion::Ease, Motion::RunOut, Motion::Blink] {
            assert_eq!(scale_at(m, 200.0), [1.0, 1.0], "{m:?}");
        }
    }

    /// Any sample time gives a value between its neighbours: the fills
    /// never overshoot or go back, and each step of the table is at least
    /// two frames at 30 Hz.
    #[test]
    fn every_sample_lies_between_its_neighbours() {
        for (m, from, to) in [
            (Motion::Ease, 0.0, 2.0 / 3.0),
            (Motion::Ease, 2.0 / 3.0, 1.0 / 3.0),
            (Motion::Ending, 2.0 / 3.0, 1.0),
            (Motion::RunOut, 2.0 / 3.0, 0.0),
        ] {
            let (lo, hi) = (from.min(to), from.max(to));
            let mut last = from;
            for k in 0..=700 {
                let f = fill_at(m, from, to, k as f32);
                assert!((lo - 1e-6..=hi + 1e-6).contains(&f), "{m:?} {k}: {f}");
                assert!((f - last) * (to - from) >= -1e-9, "{m:?} {k}: back");
                last = f;
            }
            assert_eq!(last, to, "{m:?} ends at its fill");
        }
        // The scale: within its peak, never below 0, swelling then
        // settling, then closing without opening again.
        let mut last = [1.0f32, 1.0];
        for k in 0..=700 {
            let ms = k as f32;
            let s = scale_at(Motion::Ending, ms);
            for a in 0..2 {
                assert!(
                    (0.0..=1.0 + ending::SWELL[a] + 1e-6).contains(&s[a]),
                    "{k}: {s:?}"
                );
            }
            if ms >= ending::CLOSE_START {
                assert!(s[0] <= last[0] + 1e-6, "{k}: the close opens");
            }
            last = s;
        }
        let frame = 1000.0 / 30.0;
        for span in [
            ending::SWEEP_END,
            ending::SWELL_END - ending::SWELL_START,
            ending::BLINK_END - ending::BLINK_START,
            ending::CLOSE_END - ending::CLOSE_START,
            BLINK_MS / (2 * BLINKS) as f32,
        ] {
            assert!(span >= 2.0 * frame - 10.0, "{span}");
        }
    }

    /// A value with a motion moves on its curve; one without is as given.
    #[test]
    fn animate_follows_the_motion() {
        let v = Value {
            motion: Motion::Ending,
            from: 2.0 / 3.0,
            ..Value::fill(1.0)
        };
        assert_eq!(animate(&v, 0.0, true).fill, 2.0 / 3.0);
        assert!(!animate(&v, 300.0, true).shown);
        assert!(animate(&v, 500.0, true).shown);
        assert!(!animate(&v, 500.0, false).shown, "the plain ending");
        let still = Value::fill(0.4);
        assert_eq!(animate(&still, 9999.0, true), still);
        let hidden = Value {
            motion: Motion::Ease,
            ..Value::hidden()
        };
        assert!(!animate(&hidden, 0.0, true).shown);
    }

    /// A change is stamped from the fill shown; the same value again keeps
    /// its stamp; a value that had gone starts from empty.
    #[test]
    fn a_change_starts_from_the_fill_shown() {
        let eased = |fill| Value {
            motion: Motion::Ease,
            ..Value::fill(fill)
        };
        let first = restamp(None, &eased(1.0 / 3.0), true).unwrap();
        assert_eq!(first.from, 0.0);
        assert_eq!(restamp(Some((&first, 50.0)), &eased(1.0 / 3.0), true), None);
        // Half way through the ease to 1/3, on to 2/3: from about 1/6.
        let next = restamp(Some((&first, EASE_MS / 2.0)), &eased(2.0 / 3.0), true).unwrap();
        assert!(close(next.from, 1.0 / 6.0), "{}", next.from);
        assert_eq!(next.fill, 2.0 / 3.0);
        // An ending long over, then a new charge: from empty, not full.
        let ended = Value {
            motion: Motion::Ending,
            from: 2.0 / 3.0,
            ..Value::fill(1.0)
        };
        let again = restamp(Some((&ended, 5000.0)), &eased(0.0), true).unwrap();
        assert_eq!(again.from, 0.0);
        // Text or style alone is no change.
        let styled = Value {
            text: "x".into(),
            ..first.clone()
        };
        assert_eq!(restamp(Some((&first, 10.0)), &styled, true), None);
    }
}
