//! The multiplayer gate of `ai_initialize_for_new_map` (`0x1c79e0`,
//! docs/game/firefight.md, "The AI switch").
//!
//! The routine first compares the options mode (`[[0x4e6948]+8]`) with 2,
//! multiplayer (`0x1c79e5`), and when it is, clears the AI globals'
//! initialised byte (`[0x4f55d0]+1`, `0x1c79f1`) and returns. So retail
//! never initialises AI on a multiplayer map: `AI_SQUAD_PLACE` returns at
//! once while that byte is clear (`0x20152e`) and `ai_update` skips its
//! tick. The gate is the `jne` (`75 0b` at `0x1c79e9`) past that early
//! return; its target, [`AI_NEW_MAP_INITIALISE`], is the campaign's path,
//! which initialises the AI's pools for the map and sets the byte
//! (`0x1c7b0e`). A Firefight game answers the gate by jumping there.
//! Both addresses must be entries of the generated image, as the render
//! sites are ([`super::scene_effects`]), so both image tiers expose them.
use super::{Point, Sig};

const DOC: &str = "docs/game/firefight.md";

/// The `jne` past the multiplayer early return, after the mode compare.
pub const AI_NEW_MAP_MULTIPLAYER: Point =
    Point::new("AI_NEW_MAP_MULTIPLAYER", 0x001c_79e9, Sig::Site, DOC);
/// The `jne`'s target: the campaign's initialisation (`push ebx` at
/// `0x1c79f6`), which ends by setting the initialised byte and `ret`.
pub const AI_NEW_MAP_INITIALISE: u32 = 0x001c_79f6;

/// The site and its target, each once.
pub fn instruction_entries() -> impl Iterator<Item = u32> {
    [AI_NEW_MAP_MULTIPLAYER.base, AI_NEW_MAP_INITIALISE].into_iter()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gate_jumps_eleven_bytes_to_the_initialisation() {
        // A short `jne` is two bytes; `75 0b` lands 11 past its end.
        assert_eq!(
            AI_NEW_MAP_MULTIPLAYER.base + 2 + 0x0b,
            AI_NEW_MAP_INITIALISE
        );
        let all: Vec<u32> = instruction_entries().collect();
        assert_eq!(all, [AI_NEW_MAP_MULTIPLAYER.base, AI_NEW_MAP_INITIALISE]);
        // The composition root chains the lists: no address is in two.
        let others: Vec<u32> = super::super::render_aspect::instruction_entries()
            .chain(super::super::scene_effects::instruction_entries())
            .chain(super::super::game_types::instruction_entries())
            .chain(super::super::skull_gates::instruction_entries())
            .collect();
        assert!(all.iter().all(|e| !others.contains(e)));
    }
}
