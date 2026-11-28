//! The grown tag arena (docs/formats/map-format.md, "The tag arena").
//!
//! Retail `0x0012b400` reserves the physical arena the tag cache loads
//! into with two immediates: the high cursor `0x3145000` (`mov eax` at
//! `0x0012b41d`) and the size `0x30e4000` (`push` at `0x0012b423`). Our
//! runtime reserves [`map::arena::ARENA`], [`map::arena::GROWTH`] past
//! retail's end, in a guest RAM grown by the same amount. Two patches move
//! the game onto it:
//!
//! - `arena size`: a `before` on `XPhysicalAlloc` (`0x002d156a`) turns
//!   the call with retail's size at retail's base into one for the whole
//!   grown arena, so the kernel maps and books every page of it.
//! - `arena cursors`: an `after` on `0x0012b400` moves the high bound
//!   (`0x004e6428`) and the depth-0 high cursor (`0x004e6440`) from
//!   retail's end to the grown end, so the game carves down from there and
//!   `0x12c1e0` hands the extra pages to the pool tag growth comes out of.
//!
//! Nothing else in the image names the arena's end or size: they occur
//! only as those two immediates, and every tag address is relative to the
//! fixed low end (`0x80061000`).

use guest_layout::points;
use hooks::{BuildError, Builder, Flow, Manifest};
use map::arena::{ARENA, RETAIL_ARENA, cursors};

pub const FEATURE: &str = "tag-arena";

/// Retail's arena size, as `0x0012b423` pushes it.
const RETAIL_LEN: u32 = RETAIL_ARENA.end - RETAIL_ARENA.start;
/// The grown arena's size.
const LEN: u32 = ARENA.end - ARENA.start;

pub struct TagArena;

impl hooks::Feature for TagArena {
    fn manifest(&self) -> Manifest {
        Manifest::new(FEATURE, "Tag arena grown past retail's")
            .always_on()
            .doc("docs/formats/map-format.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        b.raw().before(&points::XPHYSICAL_ALLOC, "arena size", |c| {
            let at = c.cpu.esp.wrapping_add(4);
            let rt = std::sync::Arc::clone(c.ctx.runtime());
            let mut mem = rt.memory();
            let size = mem.u32(at)?;
            let address = mem.u32(at + 4)?;
            if let Some(grown) = grown_size(size, address) {
                mem.set_u32(at, grown)?;
            }
            Ok(Flow::Continue)
        });
        b.raw().after(&points::PHYSICAL_ARENA_INITIALIZE, "arena cursors", |c| {
            let rt = std::sync::Arc::clone(c.ctx.runtime());
            let mut mem = rt.memory();
            for word in [cursors::HIGH_BOUND, cursors::HIGH] {
                let v = mem.u32(word)?;
                if let Some(end) = grown_cursor(v) {
                    mem.set_u32(word, end)?;
                }
            }
            Ok(())
        });
        Ok(())
    }
}

/// The size `XPhysicalAlloc(size, address)` should ask for: the grown
/// arena's for retail's arena request, else none (left alone).
pub fn grown_size(size: u32, address: u32) -> Option<u32> {
    (size == RETAIL_LEN && address == RETAIL_ARENA.start).then_some(LEN)
}

/// Where a cursor word retail `0x0012b400` left should point.
pub fn grown_cursor(value: u32) -> Option<u32> {
    (value == RETAIL_ARENA.end).then_some(ARENA.end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_retail_arena_request_and_cursors_are_patched() {
        assert_eq!(grown_size(0x030e_4000, 0x0006_1000), Some(0x030e_4000 + map::arena::GROWTH));
        assert_eq!(grown_size(0x030e_4000, u32::MAX), None);
        assert_eq!(grown_size(0x1000, 0x0006_1000), None);
        assert_eq!(grown_cursor(0x0314_5000), Some(0x0314_5000 + map::arena::GROWTH));
        assert_eq!(grown_cursor(0x0006_1000), None);
        // The grown request still ends inside the runtime's reservation.
        assert_eq!(RETAIL_ARENA.start + LEN, ARENA.end);
    }
}
