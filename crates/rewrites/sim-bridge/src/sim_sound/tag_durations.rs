//! What the sound simulation reads of a sound tag in guest memory: the
//! length the engine runs a sound for, and the tag a marker start names
//! (docs/engine/sound-sim.md, "Tag durations").
//!
//! The engine's own path is `0x18a13b` (the dialogue tracker a start feeds)
//! and `0x18a1c0`: it reads the `snd!` metadata's `+0x10` word, the tag's
//! maximum play time in milliseconds, and turns it into ticks
//! ([`sim_sound::schedule::duration_ticks`]). It reads no permutation or
//! pitch range: the word already holds the longest permutation's length
//! (docs/formats/sound.md, "Samples per Xbox ADPCM block"). A `snd!` tag
//! does not loop; the looping sound group (`lsnd`) is a sound that ends only
//! by a stop.

use runtime::Guest;
use sim_sound::events::SoundDef;

use super::Durations;
use engine::points;

/// `snd!`, as the tag entry stores a group.
pub const SND: u32 = u32::from_be_bytes(*b"snd!");
/// `lsnd`, the looping sound group.
pub const LSND: u32 = u32::from_be_bytes(*b"lsnd");
/// The datum that names no tag.
pub const NONE: u32 = u32::MAX;

/// Tag entry layout (docs/engine/tag_get_checked.md): `0x10` bytes, the
/// group at `+0`, the metadata address at `+8`.
const ENTRY_STRIDE: u32 = 0x10;
const ENTRY_GROUP: u32 = 0;
const ENTRY_ADDRESS: u32 = 8;
/// `snd!` metadata `+0x10`: the maximum play time in milliseconds.
const MAX_PLAY_TIME: u32 = 0x10;

/// `SOUND_START_AT_MARKER`'s parameter block, the four words its one stack
/// argument points to (`0x1890c0` reads them): `+0` a datum the start
/// resolves through `0x18d5b0` (`u32::MAX` for a dialogue line), `+4` the
/// sound tag, `+8` an `f32` scale (`1.0`), `+0xc` a word (`0`).
pub const MARKER_TAG: u32 = 4;

/// The definition of sound tag `datum`, reading words through `read`
/// (`None` for an unreadable address). A datum that is no tag, or a tag of
/// another group, has no duration and ends on the tick it starts.
pub fn read_def(read: &dyn Fn(u32) -> Option<u32>, entries: u32, datum: u32) -> SoundDef {
    let none = SoundDef { tag: datum, duration_ms: None, looping: false };
    if datum == NONE {
        return none;
    }
    let entry = entries.wrapping_add((datum & 0xffff).wrapping_mul(ENTRY_STRIDE));
    let (Some(group), Some(address)) = (read(entry + ENTRY_GROUP), read(entry + ENTRY_ADDRESS)) else {
        return none;
    };
    match group {
        LSND => SoundDef { looping: true, ..none },
        SND if address != 0 => {
            SoundDef { duration_ms: read(address.wrapping_add(MAX_PLAY_TIME)), ..none }
        }
        _ => none,
    }
}

/// The tag a marker start's parameter block at `params` names.
pub fn read_marker_tag(read: &dyn Fn(u32) -> Option<u32>, params: u32) -> Option<u32> {
    read(params.wrapping_add(MARKER_TAG)).filter(|&t| t != NONE)
}

/// [`Durations`] over the tags the guest has loaded.
#[derive(Clone, Copy, Debug, Default)]
pub struct GuestDurations;

impl Durations for GuestDurations {
    fn def(&self, tag: u32) -> SoundDef {
        SoundDef { tag, duration_ms: None, looping: false }
    }

    fn guest_def(&self, g: &Guest, tag: u32) -> SoundDef {
        let Ok(entries) = g.u32(points::TAG_ENTRIES.base) else {
            return self.def(tag);
        };
        read_def(&|a| g.u32(a).ok(), entries, tag)
    }

    fn marker_tag(&self, g: &Guest, params: u32) -> Option<u32> {
        read_marker_tag(&|a| g.u32(a).ok(), params)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim_sound::schedule::{duration_ticks, end_tick};
    use std::collections::HashMap;

    const ENTRIES: u32 = 0x1000;

    /// A table of tags: `(datum index, group, metadata address, max play ms)`.
    fn memory(tags: &[(u32, u32, u32, u32)]) -> HashMap<u32, u32> {
        let mut m = HashMap::new();
        for &(i, group, address, ms) in tags {
            m.insert(ENTRIES + i * ENTRY_STRIDE + ENTRY_GROUP, group);
            m.insert(ENTRIES + i * ENTRY_STRIDE + ENTRY_ADDRESS, address);
            if address != 0 {
                m.insert(address + MAX_PLAY_TIME, ms);
            }
        }
        m
    }

    #[test]
    fn durations_equal_the_engines_for_a_fixture_of_tags() {
        let m = memory(&[
            (0, SND, 0x2000, 352),
            (1, SND, 0x2100, 1000),
            (2, SND, 0x2200, 0),
            (3, LSND, 0x2300, 9999),
            (4, u32::from_be_bytes(*b"effe"), 0x2400, 500),
            (5, SND, 0, 700),
        ]);
        let read = |a: u32| m.get(&a).copied();
        let def = |datum: u32| read_def(&read, ENTRIES, 0xe1e1_0000 | datum);
        // Datum salts are ignored: the engine indexes by the low 16 bits.
        assert_eq!(def(0).duration_ms, Some(352));
        assert_eq!(end_tick(10, def(0).duration_ms.unwrap()), 10 + 11);
        assert_eq!(def(1).duration_ms, Some(1000));
        assert_eq!(end_tick(0, def(1).duration_ms.unwrap()), 30);
        assert_eq!(def(2).duration_ms, Some(0));
        assert_eq!(end_tick(5, 0), 5);
        // A looping group has no duration and no end.
        let l = def(3);
        assert!(l.looping && l.duration_ms.is_none());
        // Another group, no metadata, an unreadable entry, no datum.
        assert_eq!(def(4).duration_ms, None);
        assert_eq!(def(5).duration_ms, None);
        assert_eq!(def(6).duration_ms, None);
        assert_eq!(read_def(&read, ENTRIES, NONE).duration_ms, None);
        // The tag is passed through.
        assert_eq!(def(0).tag, 0xe1e1_0000);
    }

    #[test]
    fn ticks_round_to_nearest_as_the_engine_does() {
        // 0x18a13b: f32(ms) * 0.001f32 * 30.0f32, fistp (nearest, ties even).
        for (ms, ticks) in [(0, 0), (1, 0), (16, 0), (17, 1), (33, 1), (51, 2), (352, 11), (1000, 30), (2067, 62)] {
            assert_eq!(duration_ticks(ms), ticks, "{ms} ms");
        }
    }

    #[test]
    fn the_marker_block_names_its_tag_at_word_one() {
        // [NONE, tag, 1.0f32, 0], as a dialogue line passes it.
        let params = 0x5000;
        let m: HashMap<u32, u32> =
            [(params, NONE), (params + 4, 0xe1e1_0007), (params + 8, 1.0f32.to_bits()), (params + 12, 0)].into();
        let read = |a: u32| m.get(&a).copied();
        assert_eq!(read_marker_tag(&read, params), Some(0xe1e1_0007));
        let none: HashMap<u32, u32> = [(params + 4, NONE)].into();
        assert_eq!(read_marker_tag(&|a| none.get(&a).copied(), params), None);
        assert_eq!(read_marker_tag(&|_| None, params), None);
    }
}
