//! The HUD's walks of its definitions in Rust (docs/graphics/native-hud-overlay.md,
//! "The definitions in Rust"): at `HUD_DEFINITIONS` (`0x001394b9`) the
//! definitions the HUD record names are walked in the guest's order
//! (`hud_kit_core::walk::definitions`), each by Rust's own loop, and the
//! guest's body is left out.
//!
//! The order is the guest's until [`walk::PROOF_DRAWS`] of its calls
//! walked the definitions Rust lists, datum and slot, in order. One call
//! in [`walk::RECHECK`] after that is the guest's and is checked again,
//! with every walk of it checked too ([`rechecking`]). A call that
//! differs keeps the order on the guest for the map.
//!
//! A definition whose loop is not Rust's (`walk::plan_loop`) or that gives
//! its loop back hands the rest to the guest: the guest resumes at that
//! definition's walk ([`RESUME`]), and its walk keeps what Rust has done
//! of it ([`take_hand_off`]).

use std::cell::RefCell;

use scene_extract::hud::walk::DefinitionWalk;

use super::walk::{self, Arm, Proof};

/// Where the guest resumes to walk the definition of each step of
/// `walk::DEFINITION_ORDER`, and the EBX the code there expects: the
/// return of the step before's walk. Step 0 has none: the body runs
/// from its entry. Step 5's code clears EBX itself.
pub const RESUME: [Option<(u32, Option<u32>)>; 8] = [
    None,
    Some((0x0013_94d8, Some(u32::MAX))),
    Some((0x0013_94e8, Some(u32::MAX))),
    Some((0x0013_94f8, Some(u32::MAX))),
    Some((0x0013_9508, Some(u32::MAX))),
    Some((0x0013_9518, None)),
    Some((0x0013_952a, Some(0))),
    Some((0x0013_9539, Some(0))),
];

/// The UI globals and the HUD record's place in them, which the body
/// writes before its first walk (`0x001394bf`).
pub const UI: u32 = 0x0051_0c4c;
pub const UI_RECORD: u32 = 0x1bc;

/// One call of `HUD_DEFINITIONS` in progress.
#[derive(Default)]
struct Call {
    /// The guest walks it as the order's check.
    shadow: bool,
    user: u32,
    record: u32,
    /// Rust's order, as (datum, slot).
    expected: Vec<(u32, u32)>,
    /// The guest's walks of `record`, as (datum, slot).
    walked: Vec<(u32, u32)>,
    /// The walk Rust handed to the guest: its datum and slot.
    hand_off: Option<(u32, u32)>,
}

thread_local! {
    static CALL: RefCell<Option<Call>> = const { RefCell::new(None) };
}

/// What the call's entry does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    /// The guest walks them, unchecked: a call differed on this map.
    Guest,
    /// The guest walks them and the order is checked at the return.
    Shadow,
    /// Rust walks them.
    Own(Vec<DefinitionWalk>),
}

fn pairs(walks: &[DefinitionWalk]) -> Vec<(u32, u32)> {
    walks.iter().map(|w| (w.datum, w.slot)).collect()
}

/// `HUD_DEFINITIONS`' entry: whose the call is. `walks` is Rust's order
/// for local user `user`'s record at `record`, `None` when the record did
/// not read.
pub fn enter(user: u32, record: u32, walks: Option<Vec<DefinitionWalk>>) -> Plan {
    CALL.with(|c| *c.borrow_mut() = None);
    let Some(walks) = walks else {
        walk::count("definitions_read");
        return Plan::Guest;
    };
    match walk::definitions_proof(user, Proof::arm) {
        Arm::Guest => {
            walk::count("definitions_guest");
            Plan::Guest
        }
        Arm::Shadow => {
            CALL.with(|c| *c.borrow_mut() = Some(Call { shadow: true, user, record, expected: pairs(&walks), ..Call::default() }));
            Plan::Shadow
        }
        Arm::Own => {
            CALL.with(|c| *c.borrow_mut() = Some(Call { user, record, ..Call::default() }));
            Plan::Own(walks)
        }
    }
}

/// Whether the call in progress is the order's check: every walk of it
/// is the guest's and is checked as well.
pub fn rechecking() -> bool {
    CALL.with(|c| c.borrow().as_ref().is_some_and(|c| c.shadow))
}

/// `HUD_WALK`'s entry: the guest walks `datum` of `record` with `slot`.
pub fn walked(record: u32, datum: u32, slot: u32) {
    CALL.with(|c| {
        if let Some(call) = c.borrow_mut().as_mut().filter(|c| c.shadow && c.record == record) {
            call.walked.push((datum, slot));
        }
    });
}

/// Rust gave the walk of `datum` with `slot` to the guest, after
/// `walk::enter` and `walk::plan_loop` ran for it.
pub fn hand_off(datum: u32, slot: u32) {
    walk::count("definitions_gave_back");
    CALL.with(|c| {
        if let Some(call) = c.borrow_mut().as_mut() {
            call.hand_off = Some((datum, slot));
        }
    });
}

/// `HUD_WALK`'s entry: whether this walk is the one Rust handed over.
/// Its entry has run (`walk::enter`, `walk::plan_loop`): the walk keeps
/// what Rust left and the guest walks it.
pub fn take_hand_off(record: u32, datum: u32, slot: u32) -> bool {
    CALL.with(|c| {
        let mut c = c.borrow_mut();
        let Some(call) = c.as_mut().filter(|c| c.record == record) else { return false };
        if call.hand_off == Some((datum, slot)) {
            call.hand_off = None;
            return true;
        }
        false
    })
}

/// Rust walked every definition: the body is left out.
pub fn owned() {
    walk::count("definitions_owned");
    CALL.with(|c| *c.borrow_mut() = None);
}

/// `HUD_DEFINITIONS`' return: the guest's walks against Rust's order.
pub fn leave() {
    let Some(call) = CALL.with(|c| c.borrow_mut().take()) else { return };
    if !call.shadow {
        return;
    }
    if call.walked != call.expected {
        walk::count("definitions_differed");
        walk::definitions_proof(call.user, |p| p.checked(false));
    } else {
        walk::count("definitions_matched");
        if walk::definitions_proof(call.user, |p| p.checked(true)) {
            walk::count("definitions_proven");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scene_extract::hud::walk::{DEFINITION_ORDER, definitions};

    const RECORD: u32 = 0x7000;
    /// Rust's order for [`record`]'s words.
    const ORDER: [(u32, u32); 6] = [(6, 3), (3, 4), (5, 2), (6, 3), (1, 0), (2, 0)];

    fn walks() -> Option<Vec<DefinitionWalk>> {
        let mut r = vec![0u8; scene_extract::hud::walk::RECORD_SIZE];
        for (i, w) in [1u32, 2, 3, u32::MAX, 5, 6, u32::MAX].iter().enumerate() {
            r[4 * i..4 * i + 4].copy_from_slice(&w.to_le_bytes());
        }
        definitions(&r)
    }

    /// One call for `user` whose guest walks, if it is the guest's, are
    /// `walked`: its plan, and whether it was the check of every walk.
    fn call(user: u32, guest: &[(u32, u32)]) -> (Plan, bool) {
        let plan = enter(user, RECORD, walks());
        let checking = rechecking();
        if plan != Plan::Own(walks().unwrap()) {
            for &(datum, slot) in guest {
                walked(RECORD, datum, slot);
            }
            leave();
        } else {
            owned();
        }
        (plan, checking)
    }

    #[test]
    fn the_definitions_order_is_the_guests_until_proven_then_rusts_with_one_call_in_64_checked() {
        let user = 0xd0;
        for n in 0..walk::PROOF_DRAWS {
            assert_eq!(call(user, &ORDER), (Plan::Shadow, true), "call {n}");
        }
        let plans: Vec<(Plan, bool)> = (0..walk::RECHECK).map(|_| call(user, &ORDER)).collect();
        let checks = plans.iter().filter(|p| p.0 == Plan::Shadow).count();
        assert_eq!(checks, 1);
        // The check rechecks every walk of it; an owned call checks none.
        assert!(plans.iter().all(|p| p.1 == (p.0 == Plan::Shadow)));
        assert!(plans.iter().all(|p| p.0 != Plan::Guest));
        assert!(matches!(&plans[0].0, Plan::Own(w) if pairs(w) == ORDER));
    }

    #[test]
    fn a_call_whose_walks_differ_keeps_the_definitions_on_the_guest_for_the_map() {
        let user = 0xd1;
        assert_eq!(call(user, &ORDER).0, Plan::Shadow);
        // The guest walked word 5 once: Rust's order is not the guest's.
        assert_eq!(call(user, &ORDER[1..]).0, Plan::Shadow);
        for _ in 0..2 * walk::RECHECK {
            assert_eq!(call(user, &ORDER), (Plan::Guest, false));
        }
        // Another user's order is its own.
        assert_eq!(call(0xd2, &ORDER).0, Plan::Shadow);
    }

    #[test]
    fn a_walk_of_another_record_is_not_the_calls() {
        let user = 0xd3;
        enter(user, RECORD, walks());
        for &(datum, slot) in &ORDER {
            walked(RECORD, datum, slot);
            walked(RECORD + 0x270, datum, slot);
        }
        leave();
        assert_eq!(call(user, &ORDER).0, Plan::Shadow);
        assert!(walk::definitions_proof(user, |p| !p.proven()));
    }

    #[test]
    fn a_handed_off_walk_is_taken_once_and_only_for_its_record() {
        enter(0xd4, RECORD, Some(Vec::new()));
        hand_off(6, 3);
        assert!(!take_hand_off(RECORD + 0x270, 6, 3), "another record");
        assert!(!take_hand_off(RECORD, 6, 0), "another slot");
        assert!(take_hand_off(RECORD, 6, 3));
        assert!(!take_hand_off(RECORD, 6, 3), "taken");
        leave();
        assert!(!take_hand_off(RECORD, 6, 3), "the call is over");
    }

    #[test]
    fn every_step_after_the_first_resumes_at_the_return_of_the_walk_before() {
        assert_eq!(RESUME.len(), DEFINITION_ORDER.len());
        assert!(RESUME[0].is_none());
        // Each `call 0x0013a532` is five bytes; the step's check follows it.
        let calls = [0x0013_94d3u32, 0x0013_94e3, 0x0013_94f3, 0x0013_9503, 0x0013_9513, 0x0013_9525, 0x0013_9534];
        for (step, call) in calls.iter().enumerate() {
            assert_eq!(RESUME[step + 1].map(|r| r.0), Some(call + 5), "step {}", step + 1);
        }
    }
}
