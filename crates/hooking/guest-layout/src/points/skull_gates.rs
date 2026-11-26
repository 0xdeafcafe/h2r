//! The campaign gates of the AI skulls' readers (docs/game/skulls.md,
//! "Where each effect lives" and "In Firefight").
//!
//! Every reader of an AI skull's byte in the skull array (`SKULLS`,
//! `0x4f55dc + n`) first compares the active options mode with 1, the
//! campaign (`[[0x4e6948]+8]`, or a register already loaded from it), and
//! jumps past its byte with `jne` when it is not. A gate is that `jne`:
//! the site is the jump, the resume the instruction it falls through to,
//! which loads the skull's byte (`mov al, [imm]` or `mov cl, [imm]`). A
//! `before` hook at the site that resumes at the fall-through answers the
//! gate "on" and leaves every instruction around it to retail: the
//! compare and whatever the compiler scheduled between it and the jump
//! (x87 and SSE stores) have already run, and the byte load comes next.
//!
//! The list is the whole absolute-reference scan of the XBE for the seven
//! AI skulls' bytes, less Famine's multiplayer reader (`0xfda54` in
//! `0xfda00`, a placed weapon's rounds, which the multiplayer path
//! answers at the function's entry). Each site and resume must be an entry
//! of the generated image, as the render sites are ([`super::scene_effects`]),
//! so both image tiers expose them. No site is a call's continuation: each
//! follows a compare or a store.
use super::{Point, Sig};

const DOC: &str = "docs/game/skulls.md";

/// One reader's gate: the skull it reads, the `jne` past its byte, and the
/// byte load the `jne` falls through to.
#[derive(Debug)]
pub struct Gate {
    /// The skull's index in the array.
    pub skull: u8,
    /// The `jne` (two bytes, `75 xx`).
    pub site: Point,
    /// The instruction after it: the load of the skull's byte.
    pub resume: u32,
}

const fn gate(skull: u8, name: &'static str, site: u32) -> Gate {
    Gate {
        skull,
        site: Point::new(name, site, Sig::Site, DOC),
        // A short `jne` is two bytes.
        resume: site + 2,
    }
}

/// Assassins.
pub const ASSASSINS: u8 = 2;
/// Thunderstorm.
pub const THUNDERSTORM: u8 = 3;
/// Famine.
pub const FAMINE: u8 = 4;
/// Ghost.
pub const GHOST: u8 = 7;
/// Catch.
pub const CATCH: u8 = 9;
/// Mythic.
pub const MYTHIC: u8 = 12;
/// Angry.
pub const ANGRY: u8 = 13;

/// Every gate, by skull, then address. A static, so each site is one
/// point by identity for the plan.
pub static GATES: [Gate; 13] = [
    // The active camouflage `c6810` (`UNIT_CAMO_UPDATE`): an AI unit not
    // friendly to the local player gets body `+134` bit 3.
    gate(ASSASSINS, "SKULL_GATE_ASSASSINS", 0x000c_683f),
    // `0x2036c0`: a character takes the next rank's variant.
    gate(THUNDERSTORM, "SKULL_GATE_THUNDERSTORM", 0x0020_3757),
    // The actor's deletion `0x1e2a90`: its dropped weapon's rounds (the
    // float at `[esp+10]` scaled by `xmm3`), then the dropped count.
    gate(FAMINE, "SKULL_GATE_FAMINE_DROP", 0x001e_2e4e),
    gate(FAMINE, "SKULL_GATE_FAMINE_DROP_COUNT", 0x001e_2ed4),
    // `0xc9770`: the flinch cap players have applies to AI too.
    gate(GHOST, "SKULL_GATE_GHOST", 0x000c_9869),
    // `0x1aded0`, the grenade decision: four reads.
    gate(CATCH, "SKULL_GATE_CATCH_THROW", 0x001a_e51d),
    gate(CATCH, "SKULL_GATE_CATCH_CHANCE", 0x001a_e568),
    gate(CATCH, "SKULL_GATE_CATCH_RETHROW", 0x001a_e76a),
    gate(CATCH, "SKULL_GATE_CATCH_RETHROW_CHANCE", 0x001a_e7af),
    // The actor's deletion `0x1e2a90`: its grenades are dropped.
    gate(CATCH, "SKULL_GATE_CATCH_DROP", 0x001e_2d59),
    // `0x1e04a0` and `0x1e0dc0`: vitality doubled (the mode in `edx`).
    gate(MYTHIC, "SKULL_GATE_MYTHIC", 0x001e_0576),
    gate(MYTHIC, "SKULL_GATE_MYTHIC_RESTORE", 0x001e_0eab),
    // `0x1fc7e0`: the actor's fire rate.
    gate(ANGRY, "SKULL_GATE_ANGRY", 0x001f_da13),
];

/// The gates of `skull`.
pub fn of(skull: u8) -> impl Iterator<Item = &'static Gate> {
    GATES.iter().filter(move |g| g.skull == skull)
}

/// Every site and its resume, each once.
pub fn instruction_entries() -> impl Iterator<Item = u32> {
    GATES.iter().flat_map(|g| [g.site.base, g.resume])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_ai_skull_has_a_gate_and_each_falls_through_to_its_byte() {
        // The AI skulls of a Firefight host (skulls_mp::FIREFIGHT_AI_SKULLS).
        for skull in [2, 3, 4, 7, 9, 12, 13] {
            assert!(of(skull).next().is_some(), "skull {skull}");
        }
        for g in &GATES {
            assert_eq!(g.resume, g.site.base + 2, "{}", g.site.name);
            assert!(matches!(g.site.sig, Sig::Site), "{}", g.site.name);
        }
    }

    #[test]
    fn the_entries_are_unique_and_none_is_another_lists() {
        let all: Vec<u32> = instruction_entries().collect();
        let mut sorted = all.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), all.len());
        assert_eq!(all.len(), 2 * GATES.len());
        let mut names: Vec<&str> = GATES.iter().map(|g| g.site.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), GATES.len());
        // The composition root chains the lists: no address is in two.
        let others: Vec<u32> = super::super::render_aspect::instruction_entries()
            .chain(super::super::scene_effects::instruction_entries())
            .chain(super::super::game_types::instruction_entries())
            .collect();
        assert!(all.iter().all(|e| !others.contains(e)));
    }
}
