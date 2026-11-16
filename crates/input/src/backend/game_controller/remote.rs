//! The Apple TV remote (a `GCMicroGamepad`) as an Xbox pad, for the menus.
//!
//! The remote has a touch surface that also clicks, a select click, Back
//! (`buttonMenu`) and Play/Pause. [`RemoteNav`] turns them into what a menu
//! needs (docs/runtime/tvos.md, "The remote"):
//!
//! * a click near an edge of the clickpad is that d-pad direction, held as
//!   long as the click is; a click in the middle is A;
//! * a swipe is a d-pad press per [`STEP`] of travel, each held for
//!   [`PULSE`] then released for [`GAP`], so the game sees one step per
//!   press however fast the finger moves;
//! * Back is B and Play/Pause is Start.
//!
//! Gameplay on the remote is out of scope: there are no sticks or triggers.
//! Pure logic over [`RemoteSnapshot`]s and a caller's clock, so it is tested
//! without the framework.

use std::collections::VecDeque;
use std::time::Duration;

use crate::host::{HostButton, HostPadState};

/// A remote's inputs, read out of the framework. `touch` is the absolute
/// finger position on the clickpad, -1..=1 with +Y up, and (0, 0) when no
/// finger is on it (`reportsAbsoluteDpadValues`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RemoteSnapshot {
    pub touch: (f32, f32),
    /// The clickpad is clicked (`buttonA`).
    pub select: bool,
    /// Play/Pause (`buttonX`).
    pub play_pause: bool,
    /// Back, or Menu on older remotes (`buttonMenu`).
    pub back: bool,
}

/// The name a remote is reported under (`crate::active::pad_kind` reads it).
pub const NAME: &str = "Apple TV Remote";

/// Travel across the clickpad (of its 2-unit width) that makes one step.
pub const STEP: f32 = 0.5;
/// How far from the centre a click counts as a direction.
pub const EDGE: f32 = 0.5;
/// How long each swipe step holds its d-pad button.
pub const PULSE: Duration = Duration::from_millis(100);
/// The release between two steps, so the game sees each press.
pub const GAP: Duration = Duration::from_millis(70);
/// Steps a fast swipe may queue ahead.
pub const MAX_QUEUED: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
}

impl Dir {
    fn button(self) -> HostButton {
        match self {
            Dir::Up => HostButton::DpadUp,
            Dir::Down => HostButton::DpadDown,
            Dir::Left => HostButton::DpadLeft,
            Dir::Right => HostButton::DpadRight,
        }
    }

    /// The direction of the larger component of `v`, if it reaches `min`.
    fn of(v: (f32, f32), min: f32) -> Option<Dir> {
        let (x, y) = (
            if v.0.is_finite() { v.0 } else { 0.0 },
            if v.1.is_finite() { v.1 } else { 0.0 },
        );
        if x.abs().max(y.abs()) < min {
            return None;
        }
        Some(if x.abs() >= y.abs() {
            if x > 0.0 { Dir::Right } else { Dir::Left }
        } else if y > 0.0 {
            Dir::Up
        } else {
            Dir::Down
        })
    }
}

/// What a held click means, decided when it went down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Click {
    Centre,
    Edge(Dir),
}

/// One remote's navigation state. See the module docs.
#[derive(Clone, Debug, Default)]
pub struct RemoteNav {
    click: Option<Click>,
    /// Where the finger was at the last step (or touch-down).
    anchor: Option<(f32, f32)>,
    queued: VecDeque<Dir>,
    /// The step being shown and when it ends; then a gap until `next_at`.
    showing: Option<(Dir, Duration)>,
    next_at: Duration,
}

fn touching(t: (f32, f32)) -> bool {
    t.0 != 0.0 || t.1 != 0.0
}

impl RemoteNav {
    /// The pad state for `s` at `now` (any monotonic clock).
    pub fn update(&mut self, s: &RemoteSnapshot, now: Duration) -> HostPadState {
        // A click is decided on the way down and held until released; while
        // clicked, the finger's travel makes no steps.
        match (self.click, s.select) {
            (None, true) => {
                self.click = Some(match Dir::of(s.touch, EDGE) {
                    Some(d) => Click::Edge(d),
                    None => Click::Centre,
                });
                self.anchor = None;
                self.queued.clear();
            }
            (Some(_), false) => self.click = None,
            _ => {}
        }
        if self.click.is_none() {
            if touching(s.touch) {
                let anchor = *self.anchor.get_or_insert(s.touch);
                let travel = (s.touch.0 - anchor.0, s.touch.1 - anchor.1);
                if let Some(d) = Dir::of(travel, STEP) {
                    if self.queued.len() < MAX_QUEUED {
                        self.queued.push_back(d);
                    }
                    self.anchor = Some(s.touch);
                }
            } else {
                self.anchor = None;
            }
        }
        if let Some((_, until)) = self.showing
            && now >= until
        {
            self.showing = None;
        }
        if self.showing.is_none()
            && now >= self.next_at
            && let Some(d) = self.queued.pop_front()
        {
            self.showing = Some((d, now + PULSE));
            self.next_at = now + PULSE + GAP;
        }

        let mut st = HostPadState::default();
        match self.click {
            Some(Click::Centre) => st.buttons.set(HostButton::South, true),
            Some(Click::Edge(d)) => st.buttons.set(d.button(), true),
            None => {}
        }
        if let Some((d, _)) = self.showing {
            st.buttons.set(d.button(), true);
        }
        st.buttons.set(HostButton::East, s.back);
        st.buttons.set(HostButton::Start, s.play_pause);
        let press = |b| if st.buttons.contains(b) { 1.0 } else { 0.0 };
        st.analog_buttons = Some([
            press(HostButton::South),
            press(HostButton::East),
            0.0,
            0.0,
            0.0,
            0.0,
        ]);
        st.prefiltered = true;
        st
    }
}

/// Adds `remote`'s presses to `pad` (a gamepad's state), so either device
/// works: buttons are or-ed and pressures take the larger.
pub fn merge_remote(pad: &mut HostPadState, remote: &HostPadState) {
    use HostButton::*;
    for b in [
        South,
        East,
        West,
        North,
        LeftBumper,
        RightBumper,
        Back,
        Start,
        LeftStick,
        RightStick,
        DpadUp,
        DpadDown,
        DpadLeft,
        DpadRight,
    ] {
        if remote.buttons.contains(b) {
            pad.buttons.set(b, true);
        }
    }
    if let Some(r) = remote.analog_buttons {
        let p = pad.analog_buttons.get_or_insert([0.0; 6]);
        for (p, r) in p.iter_mut().zip(r) {
            *p = p.max(r);
        }
    }
}

/// Whether the remote's state has anything pressed.
pub fn remote_active(st: &HostPadState) -> bool {
    *st != HostPadState {
        analog_buttons: st.analog_buttons,
        prefiltered: st.prefiltered,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    fn held(st: &HostPadState) -> Vec<HostButton> {
        use HostButton::*;
        [South, East, Start, DpadUp, DpadDown, DpadLeft, DpadRight]
            .into_iter()
            .filter(|b| st.buttons.contains(*b))
            .collect()
    }

    fn touch(x: f32, y: f32) -> RemoteSnapshot {
        RemoteSnapshot {
            touch: (x, y),
            ..Default::default()
        }
    }

    #[test]
    fn a_centre_click_is_a_and_an_edge_click_is_that_direction() {
        let mut n = RemoteNav::default();
        let st = n.update(
            &RemoteSnapshot {
                touch: (0.1, -0.2),
                select: true,
                ..Default::default()
            },
            MS,
        );
        assert_eq!(held(&st), [HostButton::South]);
        assert_eq!(
            st.analog_buttons.unwrap()[0],
            1.0,
            "A carries full pressure"
        );
        // Held while clicked, even if the finger slides to an edge.
        let st = n.update(
            &RemoteSnapshot {
                touch: (0.9, 0.0),
                select: true,
                ..Default::default()
            },
            2 * MS,
        );
        assert_eq!(held(&st), [HostButton::South]);
        assert!(
            held(&n.update(&touch(0.9, 0.0), 3 * MS)).is_empty(),
            "released"
        );
        let mut n = RemoteNav::default();
        for (pos, want) in [
            ((0.0, 0.9), HostButton::DpadUp),
            ((0.0, -0.8), HostButton::DpadDown),
            ((-0.7, 0.1), HostButton::DpadLeft),
            ((0.95, -0.3), HostButton::DpadRight),
        ] {
            let st = n.update(
                &RemoteSnapshot {
                    touch: pos,
                    select: true,
                    ..Default::default()
                },
                MS,
            );
            assert_eq!(held(&st), [want], "{pos:?}");
            assert!(held(&n.update(&RemoteSnapshot::default(), 2 * MS)).is_empty());
        }
    }

    #[test]
    fn back_is_b_and_play_pause_is_start() {
        let mut n = RemoteNav::default();
        let st = n.update(
            &RemoteSnapshot {
                back: true,
                ..Default::default()
            },
            MS,
        );
        assert_eq!(held(&st), [HostButton::East]);
        assert_eq!(st.analog_buttons.unwrap()[1], 1.0);
        let st = n.update(
            &RemoteSnapshot {
                play_pause: true,
                ..Default::default()
            },
            2 * MS,
        );
        assert_eq!(held(&st), [HostButton::Start]);
    }

    #[test]
    fn a_swipe_makes_one_pulsed_step_per_step_of_travel() {
        let mut n = RemoteNav::default();
        let mut t = Duration::ZERO;
        let mut at = |n: &mut RemoteNav, s: RemoteSnapshot, dt: u32| {
            t += dt * MS;
            held(&n.update(&s, t))
        };
        assert!(
            at(&mut n, touch(0.0, 0.6), 10).is_empty(),
            "touch-down sets the anchor"
        );
        assert!(
            at(&mut n, touch(0.0, 0.3), 10).is_empty(),
            "less than a step"
        );
        // A step down: pressed for PULSE, then released.
        assert_eq!(at(&mut n, touch(0.0, 0.05), 10), [HostButton::DpadDown]);
        assert_eq!(at(&mut n, touch(0.0, 0.05), 50), [HostButton::DpadDown]);
        assert!(at(&mut n, touch(0.0, 0.05), 60).is_empty(), "PULSE is over");
        // A fast swipe on: two more steps queue, shown one after the other
        // with a gap between them.
        assert!(
            at(&mut n, touch(0.0, -0.5), 10).is_empty(),
            "still in the gap"
        );
        assert_eq!(at(&mut n, touch(0.0, -1.0), 60), [HostButton::DpadDown]);
        assert!(
            at(&mut n, RemoteSnapshot::default(), 110).is_empty(),
            "lifted; first done"
        );
        assert_eq!(
            at(&mut n, RemoteSnapshot::default(), 70),
            [HostButton::DpadDown],
            "the queued one"
        );
        assert!(at(&mut n, RemoteSnapshot::default(), 110).is_empty());
        assert!(
            at(&mut n, RemoteSnapshot::default(), 500).is_empty(),
            "nothing more queued"
        );
    }

    #[test]
    fn sideways_swipes_and_the_queue_limit() {
        let mut n = RemoteNav::default();
        n.update(&touch(-0.9, 0.0), MS);
        assert_eq!(
            held(&n.update(&touch(-0.3, 0.1), 2 * MS)),
            [HostButton::DpadRight]
        );
        // Wild travel back and forth queues at most MAX_QUEUED more.
        let mut t = 3;
        for i in 0..10 {
            let x = if i % 2 == 0 { 0.9 } else { -0.9 };
            n.update(&touch(x, 0.0), t * MS);
            t += 1;
        }
        let mut steps = 0;
        let mut was = true;
        for _ in 0..200 {
            t += 10;
            let now = !held(&n.update(&RemoteSnapshot::default(), t * MS)).is_empty();
            if now && !was {
                steps += 1;
            }
            was = now;
        }
        assert_eq!(steps, MAX_QUEUED);
    }

    #[test]
    fn a_click_drops_queued_steps() {
        let mut n = RemoteNav::default();
        n.update(&touch(0.0, 0.9), MS);
        n.update(&touch(0.0, 0.3), 2 * MS);
        n.update(&touch(0.0, -0.3), 3 * MS);
        let st = n.update(
            &RemoteSnapshot {
                touch: (0.0, 0.0),
                select: true,
                ..Default::default()
            },
            4 * MS,
        );
        assert_eq!(
            held(&st),
            [HostButton::South, HostButton::DpadDown],
            "the step already showing finishes"
        );
        let st = n.update(
            &RemoteSnapshot {
                select: true,
                ..Default::default()
            },
            400 * MS,
        );
        assert_eq!(held(&st), [HostButton::South], "the queued one is gone");
    }

    #[test]
    fn merging_ors_buttons_and_keeps_the_larger_pressure() {
        let mut pad = HostPadState::default();
        pad.buttons.set(HostButton::West, true);
        pad.analog_buttons = Some([0.25, 0.0, 1.0, 0.0, 0.0, 0.0]);
        pad.left_stick = (0.5, 0.0);
        let mut n = RemoteNav::default();
        let remote = n.update(
            &RemoteSnapshot {
                select: true,
                ..Default::default()
            },
            MS,
        );
        assert!(remote_active(&remote));
        merge_remote(&mut pad, &remote);
        assert!(pad.buttons.contains(HostButton::South) && pad.buttons.contains(HostButton::West));
        assert_eq!(pad.analog_buttons.unwrap(), [1.0, 0.0, 1.0, 0.0, 0.0, 0.0]);
        assert_eq!(pad.left_stick, (0.5, 0.0), "the gamepad's sticks stay");
        assert!(!remote_active(
            &n.update(&RemoteSnapshot::default(), 2 * MS)
        ));
    }
}
