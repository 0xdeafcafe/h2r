//! Gestures on a PlayStation touchpad (DualSense, DualShock 4), for the
//! debug menu (docs/runtime/debug-menu.md).
//!
//! GameController reports the touchpad as a click and up to two finger
//! positions ([`Touchpad`], -1..=1 with +y up). [`Gestures`] turns a run of
//! those into discrete [`Gesture`]s:
//!
//! * a **swipe** is one finger travelling [`SWIPE`] along its main axis
//!   from where it landed. The anchor then moves to the finger, so a long
//!   drag repeats (scrolling through a list). Diagonals go to the larger
//!   axis.
//! * a **click** is the touchpad button going down.
//! * a **two-finger tap** is a second finger landing while the first is
//!   down. It fires once, at once; until every finger lifts nothing else
//!   fires (no swipes, and a click made with two fingers is not a click).
//!
//! Nothing here reaches the guest: the game never had a touchpad.

use crate::host::Touchpad;

/// What a finger did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gesture {
    /// Swiped towards the top of the pad.
    Up,
    Down,
    Left,
    Right,
    /// The touchpad button.
    Click,
    /// A second finger landed.
    TwoFingerTap,
}

/// Travel for one swipe, in the touchpad's units (the pad is 2.0 wide), so
/// a fifth of its width.
pub const SWIPE: f32 = 0.4;

/// One pad's gesture state.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Gestures {
    /// Where the current swipe is measured from.
    anchor: Option<(f32, f32)>,
    /// A second finger has been down since the last time none were.
    two: bool,
    click: bool,
}

impl Gestures {
    /// Feeds one report (`None`: a pad with no touchpad, or none reported);
    /// returns the gestures it completes, in order.
    pub fn update(&mut self, pad: Option<&Touchpad>) -> Vec<Gesture> {
        let mut out = Vec::new();
        let Some(t) = pad else {
            *self = Self::default();
            return out;
        };
        if t.secondary.is_some() && !self.two {
            self.two = true;
            self.anchor = None;
            out.push(Gesture::TwoFingerTap);
        }
        if t.click && !self.click && !self.two {
            out.push(Gesture::Click);
        }
        self.click = t.click;
        match t.primary {
            None if t.secondary.is_none() => {
                // Every finger is up: the next touch starts over.
                self.anchor = None;
                self.two = false;
            }
            None => {}
            Some(_) if self.two => {}
            Some(p) => match self.anchor {
                None => self.anchor = Some(p),
                Some(a) => {
                    if let Some(g) = swipe(a, p) {
                        out.push(g);
                        self.anchor = Some(p);
                    }
                }
            },
        }
        out
    }
}

/// The swipe from `a` to `b`, if it went far enough.
fn swipe(a: (f32, f32), b: (f32, f32)) -> Option<Gesture> {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    if !(dx.is_finite() && dy.is_finite()) {
        return None;
    }
    if dy.abs() >= dx.abs() {
        if dy >= SWIPE {
            return Some(Gesture::Up);
        }
        if dy <= -SWIPE {
            return Some(Gesture::Down);
        }
    } else {
        if dx >= SWIPE {
            return Some(Gesture::Right);
        }
        if dx <= -SWIPE {
            return Some(Gesture::Left);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(x: f32, y: f32) -> Touchpad {
        Touchpad {
            primary: Some((x, y)),
            ..Default::default()
        }
    }

    fn feed(g: &mut Gestures, frames: &[Touchpad]) -> Vec<Gesture> {
        frames.iter().flat_map(|t| g.update(Some(t))).collect()
    }

    #[test]
    fn a_swipe_fires_once_its_travel_is_reached() {
        let mut g = Gestures::default();
        let up = [
            one(0.0, -0.5),
            one(0.0, -0.25),
            one(0.0, 0.0),
            one(0.0, 0.25),
        ];
        assert_eq!(feed(&mut g, &up[..2]), vec![], "0.25 is not a swipe");
        assert_eq!(feed(&mut g, &up[2..3]), vec![Gesture::Up]);
        assert_eq!(feed(&mut g, &up[3..]), vec![], "re-anchored at the finger");
        let mut g = Gestures::default();
        let down = [one(0.3, 0.6), one(0.35, 0.1)];
        assert_eq!(feed(&mut g, &down), vec![Gesture::Down]);
        let mut g = Gestures::default();
        let sideways = [one(0.5, 0.0), one(0.0, 0.1), one(-0.6, 0.2)];
        assert_eq!(feed(&mut g, &sideways), vec![Gesture::Left, Gesture::Left]);
        let mut g = Gestures::default();
        assert_eq!(
            feed(&mut g, &[one(-0.9, 0.0), one(-0.3, 0.0)]),
            vec![Gesture::Right]
        );
    }

    #[test]
    fn a_long_drag_repeats_and_a_lift_starts_over() {
        let mut g = Gestures::default();
        let drag: Vec<Touchpad> = (0..=8).map(|i| one(0.0, 1.0 - 0.25 * i as f32)).collect();
        assert_eq!(feed(&mut g, &drag), vec![Gesture::Down; 4]);
        // Lift, land elsewhere: the new touch is the anchor, not a swipe.
        assert_eq!(g.update(Some(&Touchpad::default())), vec![]);
        assert_eq!(feed(&mut g, &[one(0.0, 0.9)]), vec![]);
    }

    #[test]
    fn diagonals_go_to_the_larger_axis() {
        let mut g = Gestures::default();
        assert_eq!(
            feed(&mut g, &[one(0.0, 0.0), one(0.45, 0.5)]),
            vec![Gesture::Up]
        );
        let mut g = Gestures::default();
        assert_eq!(
            feed(&mut g, &[one(0.0, 0.0), one(0.5, -0.45)]),
            vec![Gesture::Right]
        );
    }

    #[test]
    fn the_click_fires_on_its_press_only() {
        let mut g = Gestures::default();
        let press = Touchpad {
            click: true,
            ..one(0.0, 0.0)
        };
        assert_eq!(g.update(Some(&press)), vec![Gesture::Click]);
        assert_eq!(g.update(Some(&press)), vec![], "held");
        assert_eq!(g.update(Some(&one(0.0, 0.0))), vec![]);
        assert_eq!(g.update(Some(&press)), vec![Gesture::Click]);
    }

    #[test]
    fn a_second_finger_is_one_tap_and_mutes_the_rest_of_the_touch() {
        let mut g = Gestures::default();
        let two = |y: f32, click: bool| Touchpad {
            click,
            primary: Some((0.0, y)),
            secondary: Some((0.5, 0.0)),
        };
        assert_eq!(g.update(Some(&one(0.0, 0.0))), vec![]);
        assert_eq!(
            g.update(Some(&two(0.0, false))),
            vec![Gesture::TwoFingerTap]
        );
        assert_eq!(
            g.update(Some(&two(0.9, false))),
            vec![],
            "no swipe with two fingers"
        );
        assert_eq!(
            g.update(Some(&two(0.9, true))),
            vec![],
            "no click with two fingers"
        );
        // The second finger lifts first: still the same touch.
        assert_eq!(g.update(Some(&one(-0.5, 0.0))), vec![]);
        assert_eq!(g.update(Some(&Touchpad::default())), vec![]);
        assert_eq!(
            feed(&mut g, &[one(0.0, 0.0), one(0.0, 0.5)]),
            vec![Gesture::Up]
        );
    }

    #[test]
    fn a_pad_without_a_touchpad_resets() {
        let mut g = Gestures::default();
        g.update(Some(&one(0.0, 0.0)));
        assert_eq!(g.update(None), vec![]);
        assert_eq!(g, Gestures::default());
        let nan = one(f32::NAN, 0.0);
        assert_eq!(feed(&mut g, &[one(0.0, 0.0), nan]), vec![]);
    }
}
