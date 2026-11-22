//! The bounce drill (docs/runtime/sim.md, "The bounce drill"): a sim
//! match's practising bots try one site only, each try from a start near
//! it with the word already at -3, so a run shows the final approach and
//! the jump alone.
//!
//! The host sets the drill ([`set`]) and writes the word: a bot standing
//! at the start and not primed asks for it ([`request_arm`], made by the
//! brain with its unit's datum), and the host takes the asks
//! ([`take_arm_requests`]) at the next tick and writes -3 over the unit's
//! level pair word, as the agent API's `havok.pair.set` does. The bots
//! crate has no Havok layout of its own (as with
//! [`crate::set_pair_reader`]).

use std::sync::Mutex;

use crate::brain::Vec3;

/// The drill a match runs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drill {
    /// The site to try: the known site nearest it (within [`SAME`]).
    pub site: Vec3,
    /// Where each try starts, primed. `None`: the site's approach (for a
    /// drop, its takeoff or launch floor), so the try is the jump alone.
    pub start: Option<Vec3>,
}

/// A known site this near the drill's `site` (XY and height, world
/// units) is the one tried.
pub const SAME: f32 = 0.5;

static DRILL: Mutex<Option<Drill>> = Mutex::new(None);
static ARM: Mutex<Vec<u32>> = Mutex::new(Vec::new());
/// The most asks kept between two takes.
const ARM_MAX: usize = 16;

/// Sets the drill (`None`: none, the plain practice).
pub fn set(d: Option<Drill>) {
    *DRILL.lock().unwrap_or_else(|e| e.into_inner()) = d;
}

/// The drill, when one is set.
pub fn current() -> Option<Drill> {
    *DRILL.lock().unwrap_or_else(|e| e.into_inner())
}

/// A bot at the drill's start asks for its word at -3: its unit's datum.
pub fn request_arm(unit: u32) {
    let mut v = ARM.lock().unwrap_or_else(|e| e.into_inner());
    if !v.contains(&unit) && v.len() < ARM_MAX {
        v.push(unit);
    }
}

/// The asks since the last take, each unit once.
pub fn take_arm_requests() -> Vec<u32> {
    std::mem::take(&mut *ARM.lock().unwrap_or_else(|e| e.into_inner()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asks_are_taken_once_each() {
        request_arm(0xe1a0_0001);
        request_arm(0xe1a0_0001);
        request_arm(0xe1a0_0002);
        let mut got = take_arm_requests();
        got.sort();
        assert_eq!(got, vec![0xe1a0_0001, 0xe1a0_0002]);
        assert!(take_arm_requests().is_empty());
    }
}
