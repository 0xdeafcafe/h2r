//! Interior sites the `rules` feature hooks for the script index
//! (docs/game/game-types-without-carriers.md, "No retail engine object").
//! Each must be an entry of the generated image, as the render sites are
//! ([`super::scene_effects`]), so both image tiers expose them.
use super::{Point, Sig};

const DOC: &str = "docs/game/game-types-without-carriers.md";

/// In the starting-location gather `0x23b170` (called from
/// `0x23bc40`, which `PLAYER_SPAWN_LOCATION` calls at `0x14ef6c`): ESI
/// the engine globals (`E`), EDI the starting location, EAX its
/// per-type mask (`[edi+8]`, nonzero), ECX the engine object. The next
/// instructions jump on `E+0xc14` through `0x23b3c0` with no bounds
/// check; the entries for 5 and 6 are zero.
pub const SPAWN_TYPE_MASK: Point = Point::new("SPAWN_TYPE_MASK", 0x0023_b2dc, Sig::Site, DOC);
/// Slayer's case of that jump: the mask's bit 0 (`mov al,[edi+8]`), then
/// the common test at `0x23b305`.
pub const SPAWN_TYPE_MASK_SLAYER: u32 = 0x0023_b2ea;

/// Every site above and its resume, each once.
pub fn instruction_entries() -> impl Iterator<Item = u32> {
    [SPAWN_TYPE_MASK.base, SPAWN_TYPE_MASK_SLAYER].into_iter()
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_site_and_its_resume_are_entries() {
        let all: Vec<u32> = super::instruction_entries().collect();
        assert_eq!(
            all,
            [super::SPAWN_TYPE_MASK.base, super::SPAWN_TYPE_MASK_SLAYER]
        );
        // None is a render site too: the composition root chains the lists.
        let render: Vec<u32> = super::super::render_aspect::instruction_entries()
            .chain(super::super::scene_effects::instruction_entries())
            .collect();
        assert!(all.iter().all(|e| !render.contains(e)));
    }
}
