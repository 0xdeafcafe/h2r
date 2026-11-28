//! The table's [`SoundTags`] read from the map in guest memory
//! (docs/engine/sound-instances.md, "Admission"): for one call, the rows
//! the admission or the chooser reads, in a [`Snapshot`]. The map's tags
//! and gestalt are static while it is loaded, so a snapshot read at the
//! call is the same as one read at load; reading per call needs no
//! invalidation when the map changes.
//!
//! | Data | Where |
//! |---|---|
//! | tag definition | `[0x4e3b44] + 16 * index + 8`: flags `+0`, class `+2`, playback row `+6`, first range `+8`, range count `+0xa`, scale `+0xb`, promotion bank `+0xc` |
//! | class flags | the sound globals tag's classes (`0x5c` bytes a class), `+0xa` (`0x221810`) |
//! | playback rows | gestalt `+4`, `0x38` bytes: gain `+0x10`, spread `+0x14`, pitch `+0x18`, `+0x1a` (`0x125260`) |
//! | scale rows | gestalt `+0xc`, 20 bytes: pitch `+8`, `+0xa` |
//! | bounds rows | gestalt `+0x1c`, 10 bytes: natural `+2`, `+4`, playable `+6`, `+8` (`0x218f50`) |
//! | pitch ranges | gestalt `+0x24`, 12 bytes: bounds row `+2`, first permutation `+8`, count `+0xa` |
//! | permutations | gestalt `+0x2c`, 16 bytes: skip `+2`, length in ms `+6` (`0x218c60`) |
//! | promotion banks | gestalt `+0x4c`, `0x1c` bytes: rows at `+4`, 16 bytes a row, its range `+0` (`0x218f10`) |
//! | looping sound (`lsnd`) | the tag's definition: flags `+0` (bit 7 one loop a tag), tracks `+0x1c` (count, rows), details `+0x24` |
//! | loop tracks | `0x58` bytes: flags `+4`, fade in `+0xc`, fade out `+0x10`, segment references at [`SEGMENT_REFS`] (the index at `+4`), crossfade `+0x50` (`0x21a250`) |
//! | loop details | `0x34` bytes: the sound's index `+8`, interval `+0xc`, `+0x10`, flags `+0x18` (`0x21b6d0`) |

use std::collections::BTreeMap;

use guest_layout::points;
use guest_layout::{GuestMemory, MemFault};
use sim_sound::instances::{Permutation, PitchRange, Playback, Scale, SoundTag, SoundTags, TagIndex};
use sim_sound::looping::{DetailTag, LoopTag, Segment, TrackTag};

/// `[0x51ebd4]`: the sound gestalt.
pub const GESTALT: u32 = 0x0051_ebd4;

/// A track's segment references by [`Segment`], `0x44a188`: in, loop,
/// out, alt trans in, alt loop, alt trans out, alt out.
pub const SEGMENT_REFS: [u32; Segment::COUNT] = [0x14, 0x1c, 0x24, 0x40, 0x2c, 0x48, 0x34];
const TRACK: u32 = 0x58;
const DETAIL: u32 = 0x34;

/// The rows one call reads, keyed by the gestalt's absolute indexes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub sounds: BTreeMap<TagIndex, SoundTag>,
    pub ranges: BTreeMap<u16, PitchRange>,
    pub permutations: BTreeMap<u16, Permutation>,
    pub scales: BTreeMap<i8, Scale>,
    pub promotions: BTreeMap<(u8, u16), u16>,
    pub loops: BTreeMap<TagIndex, LoopTag>,
}

impl SoundTags for Snapshot {
    fn sound(&self, tag: TagIndex) -> Option<&SoundTag> {
        self.sounds.get(&tag)
    }
    fn range(&self, index: u16) -> Option<&PitchRange> {
        self.ranges.get(&index)
    }
    fn permutation(&self, index: u16) -> Option<&Permutation> {
        self.permutations.get(&index)
    }
    fn scale(&self, index: i8) -> Option<&Scale> {
        self.scales.get(&index)
    }
    fn promotion(&self, bank: u8, rule: u16) -> Option<u16> {
        self.promotions.get(&(bank, rule)).copied()
    }
    fn looping(&self, tag: TagIndex) -> Option<&LoopTag> {
        self.loops.get(&tag)
    }
}

fn i16_at(m: &dyn GuestMemory, at: u32) -> Result<i16, MemFault> {
    Ok(m.u16(at)? as i16)
}

/// A tag's definition, 0 for an index with none.
pub fn definition(m: &dyn GuestMemory, tag: u32) -> Result<u32, MemFault> {
    let entries = m.u32(points::TAG_ENTRIES.base)?;
    m.u32(entries.wrapping_add((tag & 0xffff) << 4).wrapping_add(8))
}

/// The tag's class row (`0x221810`, `0x5c` bytes a class), from its
/// definition.
pub fn class_row(m: &dyn GuestMemory, def: u32) -> Result<u32, MemFault> {
    let globals = m.u32(points::GLOBALS_TAG.base)?;
    let block = if m.u32(globals.wrapping_add(0xc0))? != 0 { m.u32(globals.wrapping_add(0xc4))? } else { 0 };
    let classes = m.u32(definition(m, m.u32(block.wrapping_add(4))?)?.wrapping_add(4))?;
    let class = m.u8(def.wrapping_add(2))? as i8 as i32;
    Ok(classes.wrapping_add((class * 0x5c) as u32))
}

/// The tag's class flags (`0x221810`'s row, `+0xa`), from its definition.
pub fn class_flags(m: &dyn GuestMemory, def: u32) -> Result<u8, MemFault> {
    m.u8(class_row(m, def)?.wrapping_add(0xa))
}

/// The tag's minimum and maximum distance (`0x124fc0`, `0x125010` with
/// no source override): tag `+1` bit 2 takes the minimum from the class
/// row's `+0x18`, bit 3 the maximum from `+0x1c`; else playback row `p`'s
/// `+0` and `+4`.
pub fn distance(m: &dyn GuestMemory, def: u32, p: u32) -> Result<[f32; 2], MemFault> {
    let flags = m.u8(def.wrapping_add(1))?;
    let class = if flags & 0b1100 != 0 { class_row(m, def)? } else { 0 };
    let min = if flags & 4 != 0 { m.f32(class.wrapping_add(0x18))? } else { m.f32(p)? };
    let max = if flags & 8 != 0 { m.f32(class.wrapping_add(0x1c))? } else { m.f32(p.wrapping_add(4))? };
    Ok([min, max])
}

/// `0x124f90` on the tag's class row: its word `+8` sets bit 1 or 3, or
/// its word `+0xa` bit 11. A start's sound whose owner is gone keeps its
/// start from updating then (`0x127f8b`).
pub fn class_holds(m: &dyn GuestMemory, tag: TagIndex) -> Result<bool, MemFault> {
    let row = class_row(m, definition(m, tag.0)?)?;
    Ok(m.u16(row.wrapping_add(8))? & 0b1010 != 0 || m.u16(row.wrapping_add(0xa))? & 0x800 != 0)
}

/// A block's rows: its count (none below 1) and its first row.
fn block(m: &dyn GuestMemory, at: u32) -> Result<(u32, u32), MemFault> {
    Ok(((m.u32(at)? as i32).max(0) as u32, m.u32(at.wrapping_add(4))?))
}

/// A looping sound tag's tracks and detail sounds, `None` for an index
/// with no definition. A detail with no sound is left out: retail skips
/// it at each step (`0x21b6dd`).
pub fn loop_tag(m: &dyn GuestMemory, tag: TagIndex) -> Result<Option<LoopTag>, MemFault> {
    let def = definition(m, tag.0)?;
    if def == 0 {
        return Ok(None);
    }
    let mut looping = LoopTag::default();
    let (count, rows) = block(m, def.wrapping_add(0x1c))?;
    for i in 0..count {
        let t = rows.wrapping_add(TRACK * i);
        let mut segments = [None; Segment::COUNT];
        for (slot, &at) in segments.iter_mut().zip(SEGMENT_REFS.iter()) {
            let index = m.u32(t.wrapping_add(at).wrapping_add(4))?;
            *slot = (index != u32::MAX).then_some(TagIndex(index));
        }
        looping.tracks.push(TrackTag {
            flags: m.u32(t.wrapping_add(4))?,
            segments,
            fade_in: m.f32(t.wrapping_add(0xc))?,
            crossfade: m.f32(t.wrapping_add(0x50))?,
            fade_out: m.f32(t.wrapping_add(0x10))?,
        });
    }
    let (count, rows) = block(m, def.wrapping_add(0x24))?;
    for i in 0..count {
        let d = rows.wrapping_add(DETAIL * i);
        let index = m.u32(d.wrapping_add(8))?;
        if index == u32::MAX {
            continue;
        }
        looping.details.push(DetailTag {
            tag: TagIndex(index),
            interval: [m.f32(d.wrapping_add(0xc))?, m.f32(d.wrapping_add(0x10))?],
            flags: m.u8(d.wrapping_add(0x18))?,
        });
    }
    Ok(Some(looping))
}

/// Tag flag bit 7 of a looping sound: one loop a tag, keyed `tag | 0x6000`
/// for a source with a spatial type (`0x21a2bd`).
pub fn loop_shared(m: &dyn GuestMemory, tag: TagIndex) -> Result<bool, MemFault> {
    let def = definition(m, tag.0)?;
    Ok(def != 0 && m.u8(def)? & 0x80 != 0)
}

impl Snapshot {
    /// Add an absolute pitch range, its bounds and its permutations.
    pub fn read_range(&mut self, m: &dyn GuestMemory, absolute: i32) -> Result<(), MemFault> {
        let gestalt = m.u32(GESTALT)?;
        let at = m.u32(gestalt.wrapping_add(0x24))?.wrapping_add(12u32.wrapping_mul(absolute as u32));
        let bounds = i32::from(i16_at(m, at.wrapping_add(2))?);
        let b = m.u32(gestalt.wrapping_add(0x1c))?.wrapping_add(10u32.wrapping_mul(bounds as u32));
        let first = i16_at(m, at.wrapping_add(8))?;
        let count = i16_at(m, at.wrapping_add(0xa))?;
        let range = PitchRange {
            natural: [i16_at(m, b.wrapping_add(2))?, i16_at(m, b.wrapping_add(4))?],
            playable: [i16_at(m, b.wrapping_add(6))?, i16_at(m, b.wrapping_add(8))?],
            first_permutation: first,
            count,
        };
        let perms = m.u32(gestalt.wrapping_add(0x2c))?;
        for i in 0..count.max(0) {
            let index = (i32::from(first) + i32::from(i)) as u16;
            let p = perms.wrapping_add(16 * u32::from(index));
            let permutation = Permutation { skip: m.u16(p.wrapping_add(2))?, length_ms: Some(u32::from(m.u16(p.wrapping_add(6))?)) };
            self.permutations.insert(index, permutation);
        }
        self.ranges.insert(absolute as u16, range);
        Ok(())
    }

    /// Add `tag`'s definition, every range it has, its scale row and its
    /// promotion bank's row for `rule` (`-1` for none). A tag index with
    /// no definition adds nothing: admission refuses it as unknown.
    pub fn read_tag(&mut self, m: &dyn GuestMemory, tag: TagIndex, rule: i32) -> Result<(), MemFault> {
        let def = definition(m, tag.0)?;
        if def == 0 {
            return Ok(());
        }
        let gestalt = m.u32(GESTALT)?;
        let row = i32::from(i16_at(m, def.wrapping_add(6))?);
        let p = m.u32(gestalt.wrapping_add(4))?.wrapping_add((row as u32).wrapping_mul(0x38));
        let sound = SoundTag {
            flags: m.u8(def)?,
            class_flags: class_flags(m, def)?,
            playback: Playback {
                gain: m.f32(p.wrapping_add(0x10))?,
                gain_range: m.f32(p.wrapping_add(0x14))?,
                pitch: [i16_at(m, p.wrapping_add(0x18))?, i16_at(m, p.wrapping_add(0x1a))?],
            },
            first_range: i16_at(m, def.wrapping_add(8))?,
            range_count: m.u8(def.wrapping_add(0xa))? as i8,
            scale: m.u8(def.wrapping_add(0xb))? as i8,
            promotion: m.u8(def.wrapping_add(0xc))?,
            distance: distance(m, def, p)?,
        };
        for i in 0..i32::from(sound.range_count).max(0) {
            self.read_range(m, i32::from(sound.first_range) + i)?;
        }
        if sound.scale >= 0 {
            let s = m.u32(gestalt.wrapping_add(0xc))?.wrapping_add(20 * sound.scale as u32);
            self.scales.insert(sound.scale, Scale { pitch: [i16_at(m, s.wrapping_add(8))?, i16_at(m, s.wrapping_add(0xa))?] });
        }
        if sound.promotion != 0xff && rule >= 0 {
            let bank = m.u32(gestalt.wrapping_add(0x4c))?.wrapping_add((sound.promotion as i8 as i32 * 0x1c) as u32);
            let rows = m.u32(bank.wrapping_add(4))?;
            self.promotions.insert((sound.promotion, rule as u16), m.u16(rows.wrapping_add(16 * rule as u32))?);
        }
        self.sounds.insert(tag, sound);
        Ok(())
    }

    /// Add the looping sound `tag` and every sound its tracks and detail
    /// sounds name. A tag with no definition adds nothing.
    pub fn read_loop(&mut self, m: &dyn GuestMemory, tag: TagIndex) -> Result<(), MemFault> {
        let Some(looping) = loop_tag(m, tag)? else { return Ok(()) };
        let sounds = looping.tracks.iter().flat_map(|t| t.segments.iter().flatten()).chain(looping.details.iter().map(|d| &d.tag));
        for &sound in sounds {
            if !self.sounds.contains_key(&sound) {
                self.read_tag(m, sound, -1)?;
            }
        }
        self.loops.insert(tag, looping);
        Ok(())
    }

    /// The snapshot of one looping sound and its sounds.
    pub fn of_loop(m: &dyn GuestMemory, tag: TagIndex) -> Result<Self, MemFault> {
        let mut s = Self::default();
        s.read_loop(m, tag)?;
        Ok(s)
    }

    /// The snapshot of one tag.
    pub fn of_tag(m: &dyn GuestMemory, tag: TagIndex, rule: i32) -> Result<Self, MemFault> {
        let mut s = Self::default();
        s.read_tag(m, tag, rule)?;
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    const ENTRIES: u32 = 0x0060_0000;
    const DEF: u32 = 0x0060_1000;
    const GLOBALS: u32 = 0x0060_2000;
    const CLASSES: u32 = 0x0060_3000;
    const G: u32 = 0x0060_4000;
    const PLAYBACK: u32 = 0x0060_5000;
    const SCALES: u32 = 0x0060_5800;
    const BOUNDS: u32 = 0x0060_6000;
    const RANGES: u32 = 0x0060_6800;
    const PERMS: u32 = 0x0060_7000;
    const BANKS: u32 = 0x0060_7800;
    const ROWS: u32 = 0x0060_7c00;
    const TAG: TagIndex = TagIndex(0xe100_0003);

    /// A tag with two ranges (2 and 3), scale row 1, bank 0.
    pub(crate) fn guest() -> FakeGuest {
        let mut g = FakeGuest::new();
        for page in [points::TAG_ENTRIES.base, points::GLOBALS_TAG.base, GESTALT] {
            g.map(page & !0xfff, 0x1000);
        }
        g.map(ENTRIES, 0x8000);
        g.put_u32(points::TAG_ENTRIES.base, ENTRIES).put_u32(ENTRIES + 3 * 16 + 8, DEF).put_u32(ENTRIES + 9 * 16 + 8, CLASSES - 0x10);
        g.put_u32(points::GLOBALS_TAG.base, GLOBALS).put_u32(GLOBALS + 0xc0, 1).put_u32(GLOBALS + 0xc4, GLOBALS + 0x100).put_u32(GLOBALS + 0x104, 9);
        g.put_u32(CLASSES - 0x10 + 4, CLASSES).put_u8(CLASSES + 4 * 0x5c + 0xa, 0x24);
        g.put_u8(DEF, 8).put_u8(DEF + 2, 4).put_u16(DEF + 6, 1).put_u16(DEF + 8, 2).put_u8(DEF + 0xa, 2).put_u8(DEF + 0xb, 1).put_u8(DEF + 0xc, 0);
        g.put_u32(GESTALT, G).put_u32(G + 4, PLAYBACK).put_u32(G + 0xc, SCALES).put_u32(G + 0x1c, BOUNDS);
        g.put_u32(G + 0x24, RANGES).put_u32(G + 0x2c, PERMS).put_u32(G + 0x4c, BANKS);
        g.put_f32(PLAYBACK + 0x38 + 0x10, -2.0).put_f32(PLAYBACK + 0x38 + 0x14, 1.5).put_u16(PLAYBACK + 0x38 + 0x18, (-100i16) as u16).put_u16(PLAYBACK + 0x38 + 0x1a, 100);
        g.put_u16(SCALES + 20 + 8, 10).put_u16(SCALES + 20 + 0xa, 20);
        for (r, bounds, first, count) in [(2u32, 1u16, 5u16, 2u16), (3, 0, 7, 1)] {
            g.put_u16(RANGES + 12 * r + 2, bounds).put_u16(RANGES + 12 * r + 8, first).put_u16(RANGES + 12 * r + 0xa, count);
        }
        g.put_u16(BOUNDS + 10 + 2, 1).put_u16(BOUNDS + 10 + 4, 2).put_u16(BOUNDS + 10 + 6, 3).put_u16(BOUNDS + 10 + 8, 4);
        for (p, skip, ms) in [(5u32, 0u16, 1000u16), (6, 0x8000, 250), (7, 0, 40)] {
            g.put_u16(PERMS + 16 * p + 2, skip).put_u16(PERMS + 16 * p + 6, ms);
        }
        g.put_u32(BANKS + 4, ROWS).put_u16(ROWS + 16 * 2, 1);
        g
    }

    #[test]
    fn a_tag_reads_into_the_rows_admission_reads() {
        let g = guest();
        let s = Snapshot::of_tag(&g, TAG, 2).unwrap();
        let t = s.sound(TAG).unwrap();
        assert_eq!((t.flags, t.class_flags, t.first_range, t.range_count, t.scale, t.promotion), (8, 0x24, 2, 2, 1, 0));
        assert_eq!(t.playback, Playback { gain: -2.0, gain_range: 1.5, pitch: [-100, 100] });
        assert_eq!(s.range(2), Some(&PitchRange { natural: [1, 2], playable: [3, 4], first_permutation: 5, count: 2 }));
        assert_eq!(s.range(3).map(|r| (r.first_permutation, r.count)), Some((7, 1)));
        assert_eq!(s.permutation(6), Some(&Permutation { skip: 0x8000, length_ms: Some(250) }));
        assert_eq!(s.scale(1), Some(&Scale { pitch: [10, 20] }));
        assert_eq!(s.promotion(0, 2), Some(1));
        assert!(t.history_off(), "class flag bit 5");
    }

    #[test]
    fn a_tags_distances_come_from_its_playback_row_or_its_class() {
        let mut g = guest();
        g.put_f32(PLAYBACK + 0x38, 2.0).put_f32(PLAYBACK + 0x38 + 4, 40.0);
        g.put_f32(CLASSES + 4 * 0x5c + 0x18, 5.0).put_f32(CLASSES + 4 * 0x5c + 0x1c, 90.0);
        let read = |g: &FakeGuest| Snapshot::of_tag(g, TAG, -1).unwrap().sound(TAG).unwrap().distance;
        assert_eq!(read(&g), [2.0, 40.0]);
        g.put_u8(DEF + 1, 4);
        assert_eq!(read(&g), [5.0, 40.0], "flag 0x400: the class's minimum");
        g.put_u8(DEF + 1, 8);
        assert_eq!(read(&g), [2.0, 90.0], "flag 0x800: the class's maximum");
    }

    #[test]
    fn a_looping_sound_reads_its_tracks_details_and_their_sounds() {
        let mut g = guest();
        let (def, tracks, details) = (ENTRIES + 0xc00, ENTRIES + 0xd00, ENTRIES + 0xe00);
        let looping = TagIndex(0xe200_0005);
        g.put_u32(ENTRIES + 5 * 16 + 8, def).put_u8(def, 0x80);
        g.put_u32(def + 0x1c, 1).put_u32(def + 0x20, tracks).put_u32(def + 0x24, 2).put_u32(def + 0x28, details);
        g.put_u32(tracks + 4, 5).put_f32(tracks + 0xc, 0.1).put_f32(tracks + 0x10, 0.3).put_f32(tracks + 0x50, 0.2);
        for at in SEGMENT_REFS {
            g.put_u32(tracks + at + 4, u32::MAX);
        }
        g.put_u32(tracks + SEGMENT_REFS[usize::from(Segment::LOOP.0)] + 4, TAG.0);
        g.put_u32(details + 8, u32::MAX);
        g.put_u32(details + DETAIL + 8, TAG.0).put_f32(details + DETAIL + 0xc, 1.0).put_f32(details + DETAIL + 0x10, 2.0).put_u8(details + DETAIL + 0x18, 2);
        let s = Snapshot::of_loop(&g, looping).unwrap();
        let l = s.looping(looping).unwrap();
        let mut segments = [None; Segment::COUNT];
        segments[usize::from(Segment::LOOP.0)] = Some(TAG);
        assert_eq!(l.tracks, [TrackTag { flags: 5, segments, fade_in: 0.1, crossfade: 0.2, fade_out: 0.3 }]);
        assert_eq!(l.details, [DetailTag { tag: TAG, interval: [1.0, 2.0], flags: 2 }], "the detail with no sound is left out");
        assert!(s.sound(TAG).is_some(), "its sounds are read with it");
        assert!(loop_shared(&g, looping).unwrap());
        assert!(!loop_shared(&g, TAG).unwrap());
        assert_eq!(Snapshot::of_loop(&g, TagIndex(0xe200_0004)).unwrap(), Snapshot::default());
    }

    #[test]
    fn a_tag_with_no_definition_reads_as_unknown() {
        let g = guest();
        let s = Snapshot::of_tag(&g, TagIndex(0xe100_0004), -1).unwrap();
        assert_eq!(s, Snapshot::default());
    }
}
