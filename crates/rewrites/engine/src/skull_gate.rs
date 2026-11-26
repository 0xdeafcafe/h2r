//! The AI skulls' campaign gate, answered outside the campaign
//! (docs/game/skulls.md, "In Firefight").
//!
//! Every reader of an AI skull's byte tests the options mode for the
//! campaign first (`[[0x4e6948]+8] == 1`). A Firefight host runs actors in
//! a multiplayer game, so for the skulls it has on, the gate is answered
//! "on": the lifted readers by a hook at each gate's `jne`
//! (`points::skull_gates`), and the native bodies that hold a reader (the
//! active camouflage `c6810`) by asking [`passes`]. The skull's byte in the
//! array still decides, as in the campaign.
//!
//! One set per process: the game being played. The game's start clears it.

use std::sync::atomic::{AtomicU32, Ordering};

/// Bit `n`: skull `n`'s readers pass the gate outside the campaign.
static ANSWERED: AtomicU32 = AtomicU32::new(0);

/// The options mode of the campaign.
pub const CAMPAIGN: u32 = 1;

/// Answer the gate "on" for the skulls whose bit is set in `bits`, and
/// "as retail" for the rest.
pub fn answer(bits: u32) {
    ANSWERED.store(bits, Ordering::SeqCst);
}

/// The skulls answered "on".
pub fn answered_bits() -> u32 {
    ANSWERED.load(Ordering::SeqCst)
}

/// Whether skull `skull`'s readers pass the gate outside the campaign.
pub fn answered(skull: u8) -> bool {
    skull < 32 && answered_bits() & (1 << skull) != 0
}

/// Whether a reader of skull `skull` in options mode `mode` goes on to
/// load the skull's byte: the campaign, or a gate answered "on".
pub fn passes(mode: u32, skull: u8) -> bool {
    mode == CAMPAIGN || answered(skull)
}

#[cfg(test)]
pub(crate) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_answered_skulls_pass_outside_the_campaign() {
        let _held = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        answer(0);
        assert!(passes(CAMPAIGN, 2));
        assert!(!passes(2, 2));
        answer(1 << 2 | 1 << 13);
        assert!(passes(2, 2) && passes(2, 13) && passes(3, 2));
        assert!(!passes(2, 12));
        assert!(!answered(40));
        answer(0);
        assert!(!passes(2, 13));
    }
}
