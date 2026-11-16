//! When iOS shows its on-screen controls (`GCVirtualController`, docs/runtime/ios.md).
//!
//! The overlay is itself a controller: the GameController backend sees it
//! as one more extended gamepad, so it drives player one like any pad.
//! It shows only while nothing better is there, and gets out of the way of
//! the system keyboard.

/// One connected controller, as the host sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pad {
    /// The overlay's own controller (`GCVirtualController.controller`).
    pub is_overlay: bool,
    /// Has an extended gamepad profile: a real pad with sticks and
    /// triggers. Others (a Siri Remote paired to an iPad, a micro or
    /// motion-only controller) cannot play Halo, so they don't count.
    pub extended: bool,
}

/// Physical pads that can play the game.
pub fn physical(pads: &[Pad]) -> usize {
    pads.iter().filter(|p| !p.is_overlay && p.extended).count()
}

/// Whether the overlay should be connected: no physical pad that can play,
/// and the system keyboard not on screen (it would cover the controls, and
/// taps meant for keys would move the sticks).
pub fn wanted(physical_pads: usize, keyboard_shown: bool) -> bool {
    physical_pads == 0 && !keyboard_shown
}

#[cfg(test)]
mod tests {
    use super::*;

    const OVERLAY: Pad = Pad { is_overlay: true, extended: true };
    const PAD: Pad = Pad { is_overlay: false, extended: true };
    const REMOTE: Pad = Pad { is_overlay: false, extended: false };

    #[test]
    fn only_real_extended_pads_count() {
        assert_eq!(physical(&[]), 0);
        assert_eq!(physical(&[OVERLAY]), 0, "the overlay never hides itself");
        assert_eq!(physical(&[OVERLAY, REMOTE]), 0);
        assert_eq!(physical(&[OVERLAY, PAD]), 1);
        assert_eq!(physical(&[PAD, PAD, REMOTE]), 2);
    }

    #[test]
    fn the_overlay_shows_without_a_pad_and_hides_for_one_or_the_keyboard() {
        assert!(wanted(0, false));
        assert!(!wanted(1, false), "a pad connected");
        assert!(!wanted(0, true), "the system keyboard is up");
        assert!(!wanted(2, true));
        // Last pad gone, keyboard down: back it comes.
        assert!(wanted(physical(&[OVERLAY, REMOTE]), false));
    }
}
