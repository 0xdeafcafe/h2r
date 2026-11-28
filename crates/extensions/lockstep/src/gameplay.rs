//! The simulation's own part of the game-state arena, and a game start
//! free of the process's history (docs/game/theater-design.md, "The start
//! hash").
//!
//! The arena holds two things a film's hash must not cover:
//!
//! - **Render-side arrays** the simulation never reads (particles,
//!   effects, lights, decals, cached render states, looping sounds,
//!   cloth, liquid). Their bytes are the renderer's and vary with what the
//!   process drew. [`mask_render`] zeroes their pools before hashing.
//! - **The salt counters of the datum arrays.** Every handle the game
//!   stores carries its array's salt, and an array not cleared with
//!   `data_delete_all` between two maps keeps handing out the salts it had
//!   reached. A game that follows another in the same process stores
//!   different handles everywhere. [`salt_seeds`] lists what each array's
//!   counter is seeded to (`data_delete_all`'s own rule); the game writes
//!   them before every game starts, so a game starts from what a fresh
//!   process has.

use crate::pools::{HEAD, find_pools};
use guest_layout::datum::layout;

/// The pools (data arrays, by their header names) the simulation does not
/// read. A pool runs from its header to the next, so the array's elements
/// and bitmap are covered.
pub const RENDER_ARRAYS: &[&str] = &[
    "cloth",
    "liquid",
    "lights",
    "particles",
    "particle_system",
    "particle_emitter",
    "particle_location",
    "effect",
    "effect location",
    "cached object render states",
    "decals",
    "object looping sounds",
];

/// Arena words the simulation does not read, found by comparing the arena
/// at the first update of a game that followed the main menu with a fresh
/// process's: `(offset, len)` from the arena's base. Both are in the bytes
/// before the first array header, the engine's time globals (`+0x28e0`:
/// a 64-bit stamp, the tick rate 30, seconds per tick 1/30, speed 1.0):
/// `+0x28d8` is a byte the menu's game left at 1 (fresh: 0), and `+0x28f8`
/// is the float after the speed (fresh: 0.5, after a menu: 0.0), which
/// reads as the sub-tick fraction the renderer interpolates by. Neither is
/// named in the engine's code yet.
pub const IDLE_WORDS: &[(usize, usize)] = &[
    (0x28d8, 1),
    (0x28f8, 4),
    // An unheaded buffer in the tail of the arena, in the pool of the last
    // array (`flocks`): 2872 records of 16 bytes, three floats of a world
    // position and a packed word (`0x7fc001ff` in the dump). A game writes
    // them as it plays and a fresh process has zeros at a game's first
    // update. Its owner is not identified (no array header, and none of the
    // tick code read for it); it is left out of the hash, not cleared.
    (0x3ea000, 0xb380),
];

/// Unowned bytes inside an array's pool that a game after another leaves
/// stale: `(pool name, offset from the pool's header, length)`. The first
/// is 12 bytes in the pool of the `noncollideable object cluster refs`
/// array, well past its elements and bitmap; the second is the word after
/// the `flocks` array's bitmap. A fresh process has zeros in both. Neither
/// is a structure the engine's code has been read for.
pub const SLACK: &[(&str, usize, usize)] = &[
    ("noncollideable object cluster r", 0x7ed4, 12),
    ("flocks", 0x1e4, 4),
];

/// Whether the pool `name` (as [`find_pools`] names it) is render-side.
pub fn is_render(name: &str) -> bool {
    RENDER_ARRAYS.contains(&name.split('#').next().unwrap_or(name))
}

/// Zero every render-side pool of `block`, the [`IDLE_WORDS`] and the
/// [`SLACK`]. Returns how many bytes of pools.
pub fn mask_render(block: &mut [u8]) -> usize {
    let mut zeroed = 0;
    for &(at, len) in IDLE_WORDS {
        if let Some(word) = block.get_mut(at..at + len) {
            word.fill(0);
        }
    }
    for pool in find_pools(block) {
        for &(name, offset, len) in SLACK {
            let at = pool.offset as usize + offset;
            if pool.name == name && offset + len <= pool.len as usize {
                block[at..at + len].fill(0);
            }
        }
        if is_render(&pool.name) {
            let start = (pool.offset as usize).min(block.len());
            let end = (start + pool.len as usize).min(block.len());
            block[start..end].fill(0);
            zeroed += end - start;
        }
    }
    zeroed
}

/// The name of the object memory pool's header (`loopobjects`), which sits
/// in the arena after the `object` array and its bitmap.
pub const OBJECT_POOL_NAME: &[u8] = b"loopobjects\0";
/// The pool header: where the pool's storage is (`+0x28`, a guest
/// address), how big it is (`+0x2c`) and the bytes used (`+0x30`). The
/// storage follows the header at `+0x40`, which holds the allocator's first
/// block header.
const POOL_STORAGE: usize = 0x28;
const POOL_SIZE: usize = 0x2c;
const POOL_STORAGE_AT: usize = 0x40;
/// Bytes at the start of the pool's storage that are left alone when it is
/// cleared: the allocator's first block header (its size, the links to the
/// neighbouring blocks, a check word). Every later block of an empty pool is
/// free, and a chain of blocks cannot be walked once games have been
/// through it: the walk of an earlier version stopped a quarter of the way.
pub const POOL_KEEP: usize = 0x40;
const GLOBALS_FROM: usize = 0x28;
const GLOBALS_TO: usize = 0x80;

/// The object memory pool of the arena at `base`, where the `object` array
/// stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectPool {
    /// Live elements of the `object` array.
    pub live: i32,
    /// The pool's storage, as offsets into the block.
    pub storage: std::ops::Range<usize>,
    /// The object globals just past the storage: the next object sequence
    /// number (`+0x28`) and the placement scratch after it (to `+0x80`).
    /// The process zeroes them once, at its start, and a map load does not
    /// reset them, so a game after another begins with the sequence
    /// number where the last game left it and its scratch stale.
    pub globals: std::ops::Range<usize>,
}

/// Find the object memory pool in `block` from its header, which lies inside
/// the `object` array's pool of [`find_pools`]. `None` when the arena has no
/// `object` array or no such header, or the header is not as expected.
pub fn object_pool(block: &[u8], base: u32) -> Option<ObjectPool> {
    let pools = find_pools(block);
    let objects = pools.iter().find(|p| p.name == "object")?;
    let word = |at: usize| -> Option<u32> {
        Some(u32::from_le_bytes(block.get(at..at + 4)?.try_into().ok()?))
    };
    let live = word(objects.offset as usize + layout::ACTIVE_COUNT as usize)? as i32;
    let end = (objects.offset + objects.len) as usize;
    let header = (objects.offset as usize..end.saturating_sub(POOL_STORAGE_AT))
        .step_by(4)
        .find(|&at| block[at..].starts_with(OBJECT_POOL_NAME))?;
    let start = (word(header + POOL_STORAGE)?.checked_sub(base)?) as usize;
    let size = word(header + POOL_SIZE)? as usize;
    (start == header + POOL_STORAGE_AT && size > POOL_KEEP && start + size <= block.len())
        .then(|| {
            let end = start + size;
            ObjectPool {
                live,
                storage: start..end,
                globals: (end + GLOBALS_FROM).min(block.len())..(end + GLOBALS_TO).min(block.len()),
            }
        })
}

/// Zero the elements of every data array of `block` that are free (a zero
/// salt word): `datum_delete` clears the salt and leaves the rest of the
/// element as it was, so a free slot holds what an earlier game put there.
/// `base` is the guest address of `block`, to turn the arrays' storage
/// pointers into offsets. Returns the elements zeroed.
pub fn mask_free(block: &mut [u8], base: u32) -> usize {
    let word = |b: &[u8], at: usize| -> Option<u32> {
        Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
    };
    let mut zeroed = 0;
    for pool in find_pools(block) {
        if pool.name == HEAD {
            continue;
        }
        let at = pool.offset as usize;
        let (Some(max), Some(size), Some(data)) = (
            word(block, at + layout::MAXIMUM_COUNT as usize),
            word(block, at + layout::ELEMENT_SIZE as usize),
            word(block, at + layout::DATA as usize),
        ) else {
            continue;
        };
        let (max, size) = (max as usize, size as usize);
        let Some(start) = data.checked_sub(base).map(|d| d as usize) else { continue };
        if max == 0 || size < 2 || max > 1 << 20 || start.saturating_add(max * size) > block.len() {
            continue;
        }
        for slot in 0..max {
            let element = start + slot * size;
            if block[element] == 0 && block[element + 1] == 0 {
                block[element..element + size].fill(0);
                zeroed += 1;
            }
        }
    }
    zeroed
}

/// What a film's hash covers of the arena at `base`: [`mask_render`] and
/// [`mask_free`].
pub fn mask_simulation(block: &mut [u8], base: u32) {
    mask_render(block);
    mask_free(block, base);
}

/// The salt counter each data array of `block` is seeded to by
/// `data_delete_all`: the first two bytes of its name, as a little-endian
/// word, with bit 15 set. `(offset of the counter in the block, value)`.
pub fn salt_seeds(block: &[u8]) -> Vec<(u32, u16)> {
    find_pools(block)
        .into_iter()
        .filter(|p| p.name != HEAD)
        .filter_map(|p| {
            let at = p.offset as usize;
            let name = block.get(at..at + 2)?;
            // `strncpy` of two bytes stops at the name's end.
            let second = if name[0] == 0 { 0 } else { name[1] };
            let seed = u16::from_le_bytes([name[0], second]) | 0x8000;
            Some((p.offset + layout::NEXT_SALT, seed))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash_arena;

    const SIG_AT: usize = layout::SIGNATURE as usize;
    const SALT_AT: usize = layout::NEXT_SALT as usize;

    /// An arena of data arrays, each a header and 0x100 bytes after it.
    fn arena(names: &[&str], salts: &[u16], fill: u8) -> Vec<u8> {
        let mut block = vec![0u8; 0x1000];
        for (i, name) in names.iter().enumerate() {
            let at = 0x20 + i * 0x200;
            block[at..at + name.len()].copy_from_slice(name.as_bytes());
            block[at + SIG_AT..at + SIG_AT + 4]
                .copy_from_slice(&guest_layout::datum::SIGNATURE.to_le_bytes());
            block[at + SALT_AT..at + SALT_AT + 2].copy_from_slice(&salts[i].to_le_bytes());
            block[at + 0x4c..at + 0x14c].fill(fill + i as u8);
        }
        block
    }

    fn apply(block: &mut [u8]) {
        for (at, seed) in salt_seeds(block) {
            block[at as usize..at as usize + 2].copy_from_slice(&seed.to_le_bytes());
        }
    }

    #[test]
    fn render_arrays_do_not_reach_the_hash_and_simulation_arrays_do() {
        let names = ["objects", "particles", "effect location", "actor"];
        let salts = [0x8000; 4];
        let hash = |block: &[u8]| {
            let mut masked = block.to_vec();
            mask_render(&mut masked);
            hash_arena(&masked, None)
        };
        let base = arena(&names, &salts, 1);
        // What the renderer drew differs; the simulation's state does not.
        let mut drew = base.clone();
        drew[0x20 + 0x200 + 0x60] ^= 0xff; // particles
        drew[0x20 + 2 * 0x200 + 0x60] ^= 0xff; // effect location
        assert_ne!(hash_arena(&base, None), hash_arena(&drew, None));
        assert_eq!(hash(&base), hash(&drew));
        // An object or an actor that differs does.
        for pool in [0, 3] {
            let mut moved = base.clone();
            moved[0x20 + pool * 0x200 + 0x60] ^= 1;
            assert_ne!(hash(&base), hash(&moved), "{}", names[pool]);
        }
        let mut masked = base.clone();
        assert_eq!(mask_render(&mut masked), 0x200 * 2);
        assert!(is_render("particles#2") && !is_render("objects") && !is_render(HEAD));
    }

    #[test]
    fn a_game_after_another_hashes_as_the_first_game_of_a_process_once_the_salts_are_seeded() {
        let names = ["objects", "actor", "hs thread"];
        // A fresh process: counters as the arrays were made.
        let fresh = arena(&names, &[0x8000 | 0x626f, 0x8000 | 0x6361, 0x8000 | 0x7368], 7);
        // After a game and a menu the counters have moved on.
        let mut after = arena(&names, &[0x8123, 0x9000, 0x8fff], 7);
        let hash = |block: &[u8]| hash_arena(block, None);
        assert_ne!(hash(&fresh), hash(&after));
        apply(&mut after);
        assert_eq!(hash(&fresh), hash(&after), "seeded: the arrays start alike");
        let seeds = salt_seeds(&fresh);
        assert_eq!(seeds.len(), 3);
        assert_eq!(seeds[0], (0x20 + layout::NEXT_SALT, 0x8000 | u16::from_le_bytes(*b"ob")));
    }

    #[test]
    fn the_known_slack_of_a_pool_does_not_reach_the_hash() {
        let mut block = vec![0u8; 0x9000];
        for (at, name) in [(0x20usize, "noncollideable object cluster r"), (0x8200, "flocks")] {
            block[at..at + name.len()].copy_from_slice(name.as_bytes());
            block[at + SIG_AT..at + SIG_AT + 4]
                .copy_from_slice(&guest_layout::datum::SIGNATURE.to_le_bytes());
        }
        let mut stale = block.clone();
        stale[0x20 + 0x7ed4..0x20 + 0x7ee0].fill(0x77);
        stale[0x8200 + 0x1e4..0x8200 + 0x1e6].copy_from_slice(&[0x2a, 0x01]);
        assert_ne!(block, stale);
        mask_render(&mut block);
        mask_render(&mut stale);
        assert_eq!(block, stale);
        // A byte next to the slack is not.
        let mut near = block.clone();
        near[0x20 + 0x7ee0] = 1;
        mask_render(&mut near);
        assert_ne!(near, block);
    }

    #[test]
    fn a_free_slot_holds_nothing_the_hash_sees() {
        // One array of four 8-byte elements right after its header, based
        // at 0x1000 in the guest.
        let base = 0x1000u32;
        let make = |stale: u8| {
            let mut block = arena(&["objects"], &[0x8000], 0);
            let at = 0x20;
            let data = at + 0x4c;
            block[at + layout::MAXIMUM_COUNT as usize..][..4].copy_from_slice(&4u32.to_le_bytes());
            block[at + layout::ELEMENT_SIZE as usize..][..4].copy_from_slice(&8u32.to_le_bytes());
            block[at + layout::DATA as usize..][..4].copy_from_slice(&(base + data as u32).to_le_bytes());
            for slot in 0..4 {
                let element = &mut block[data + slot * 8..data + slot * 8 + 8];
                if slot % 2 == 0 {
                    // live: salt, then its state
                    element.copy_from_slice(&[0x01, 0x80, 9, 9, 9, 9, 9, 9]);
                } else {
                    // free: no salt, and whatever a game left
                    element.copy_from_slice(&[0, 0, stale, stale, stale, stale, stale, stale]);
                }
            }
            block
        };
        let (mut fresh, mut after) = (make(0), make(0x5a));
        assert_ne!(fresh, after);
        assert_eq!(mask_free(&mut fresh, base), 2);
        assert_eq!(mask_free(&mut after, base), 2);
        assert_eq!(fresh, after);
        // A live element that differs is not masked.
        let mut moved = make(0);
        moved[0x20 + 0x4c + 3] = 1;
        mask_free(&mut moved, base);
        assert_ne!(moved, fresh);
        // An array whose storage is outside the block is left alone.
        let mut far = make(0x5a);
        assert_eq!(mask_free(&mut far, 0x9000_0000), 0);
    }

    #[test]
    fn a_one_letter_name_seeds_from_that_letter_alone() {
        let mut block = arena(&["x"], &[0], 0);
        block[0x20 + 2] = b'z'; // past the name's end
        assert_eq!(salt_seeds(&block), [(0x20 + layout::NEXT_SALT, 0x8000 | u16::from(b'x'))]);
    }
}
