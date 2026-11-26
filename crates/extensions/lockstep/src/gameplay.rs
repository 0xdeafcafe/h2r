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
use engine::datum::layout;

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
pub const IDLE_WORDS: &[(usize, usize)] = &[(0x28d8, 1), (0x28f8, 4)];

/// Whether the pool `name` (as [`find_pools`] names it) is render-side.
pub fn is_render(name: &str) -> bool {
    RENDER_ARRAYS.contains(&name.split('#').next().unwrap_or(name))
}

/// Zero every render-side pool of `block` and the [`IDLE_WORDS`]. Returns
/// how many bytes of pools.
pub fn mask_render(block: &mut [u8]) -> usize {
    let mut zeroed = 0;
    for &(at, len) in IDLE_WORDS {
        if let Some(word) = block.get_mut(at..at + len) {
            word.fill(0);
        }
    }
    for pool in find_pools(block) {
        if is_render(&pool.name) {
            let start = (pool.offset as usize).min(block.len());
            let end = (start + pool.len as usize).min(block.len());
            block[start..end].fill(0);
            zeroed += end - start;
        }
    }
    zeroed
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
                .copy_from_slice(&engine::datum::SIGNATURE.to_le_bytes());
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
    fn a_one_letter_name_seeds_from_that_letter_alone() {
        let mut block = arena(&["x"], &[0], 0);
        block[0x20 + 2] = b'z'; // past the name's end
        assert_eq!(salt_seeds(&block), [(0x20 + layout::NEXT_SALT, 0x8000 | u16::from(b'x'))]);
    }
}
