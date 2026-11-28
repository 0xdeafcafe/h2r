//! `GuestLevels`: the table's [`Levels`] while sound tags and class state
//! live in guest memory (docs/engine/sound-instances.md, "Levels").
//!
//! It reads what `0x1251e0` and its callees read, in their order, and
//! hands the inputs to [`sim_sound::levels::level`]. It writes nothing and
//! calls nothing. `sim_bridge::sim_sound::levels` re-exports it.

use std::cell::Cell;

use crate::memory::{GuestMemory, MemFault};
use crate::points::{GLOBALS_TAG, SOUND_GLOBALS, TAG_ENTRIES};
use sim_sound::instances::{Levels, SourceParams, TagIndex};
use sim_sound::levels::{Constants, Duck, Ducking, Inputs, level};

/// `[0x51ebd4]`: the sound gestalt; `+0xC` its scale rows (20 bytes a row).
pub const GESTALT: u32 = 0x0051_ebd4;
/// `[0x502118]`: per class, 16 bytes: `+4` the level, `+0xC` flags.
pub const CLASS_STATES: u32 = 0x0050_2118;
/// Globals tag `+0xC0`/`+0xC4`: the sound globals reference; its tag's
/// `+4` holds the classes, `0x5C` bytes a class.
const CLASS_ROW: u32 = 0x5c;
/// Class `+0x28`: the duck rows.
const DUCKS: u32 = 0x28;
/// Sound globals `+0x1f8`: the duck state.
const DUCK_STATE: u32 = 0x1f8;

/// `0x221810(class)`: the class's row in the sound globals' classes. The
/// class index is the low word sign-extended (`movsx`).
pub fn class_row(m: &dyn GuestMemory, class: i32) -> Result<u32, MemFault> {
    let globals = m.u32(GLOBALS_TAG.base)?;
    let reference = if m.u32(globals.wrapping_add(0xc0))? != 0 { m.u32(globals.wrapping_add(0xc4))? } else { 0 };
    let tag = m.u32(reference.wrapping_add(4))?;
    let entries = m.u32(TAG_ENTRIES.base)?;
    let classes = m.u32(m.u32(entries.wrapping_add((tag & 0xffff) << 4).wrapping_add(8))?.wrapping_add(4))?;
    let class = i32::from(class as i16);
    Ok(classes.wrapping_add((class as u32).wrapping_mul(CLASS_ROW)))
}

pub struct GuestLevels<'a> {
    m: &'a dyn GuestMemory,
    /// Levels a read fault stopped (`sound.level_faults`): each plays at
    /// the floor.
    pub faults: Cell<u64>,
}

impl<'a> GuestLevels<'a> {
    pub fn new(m: &'a dyn GuestMemory) -> Self {
        Self { m, faults: Cell::new(0) }
    }

    /// The tag's definition: `[0x4e3b44] + 16 * (tag & 0xffff) + 8`.
    fn definition(&self, index: u32) -> Result<u32, MemFault> {
        let entries = self.m.u32(TAG_ENTRIES.base)?;
        self.m.u32(entries.wrapping_add((index & 0xffff) << 4).wrapping_add(8))
    }

    fn class_row(&self, class: i32) -> Result<u32, MemFault> {
        class_row(self.m, class)
    }

    fn duck(&self, class_row: u32, index: i32) -> Result<Duck, MemFault> {
        if index == -1 {
            return Ok(Duck::default());
        }
        let at = class_row.wrapping_add(DUCKS).wrapping_add((index as u32) << 4);
        let word = |o: u32| self.m.u32(at.wrapping_add(o));
        Ok(Duck { level: word(0)?, rise: word(4)?, hold: word(8)?, fall: word(0xc)? })
    }

    /// The inputs of `0x1251e0` for `tag` played with `source`.
    pub fn inputs(&self, tag: TagIndex, source: &SourceParams) -> Result<Inputs, MemFault> {
        let def = self.definition(tag.0)?;
        let class = i32::from(self.m.u8(def.wrapping_add(2))? as i8);
        let scale_row = i32::from(self.m.u8(def.wrapping_add(0xb))? as i8);
        let class_row = self.class_row(class)?;
        let scales = self.m.u32(self.m.u32(GESTALT)?.wrapping_add(0xc))?;
        let scale_at = scales.wrapping_add((scale_row as u32).wrapping_mul(20));
        let state = self.m.u32(CLASS_STATES)?.wrapping_add((class as u32) << 4);
        let ducked = self.m.u8(state.wrapping_add(0xc))? & 1 == 0;
        let ducking = if ducked {
            let at = self.m.u32(SOUND_GLOBALS.base)?.wrapping_add(DUCK_STATE);
            let word = |o: u32| self.m.u32(at.wrapping_add(o));
            let (current, previous) = (word(0)? as i32, word(8)? as i32);
            Some(Ducking {
                current,
                current_time: word(4)?,
                previous,
                previous_time: word(0xc)?,
                current_row: self.duck(class_row, current)?,
                previous_row: self.duck(class_row, previous)?,
            })
        } else {
            None
        };
        Ok(Inputs {
            class_bounds: [self.m.u32(class_row.wrapping_add(0x20))?, self.m.u32(class_row.wrapping_add(0x24))?],
            scale_bounds: [self.m.u32(scale_at)?, self.m.u32(scale_at.wrapping_add(4))?],
            class_level: self.m.u32(state.wrapping_add(4))?,
            ducking,
            offset: source.0[2],
            scale: source.0[1],
        })
    }
}

impl GuestLevels<'_> {
    /// `0x1251e0`'s answer as bits, or the fault that stopped its reads.
    pub fn level_bits(&self, tag: TagIndex, source: &SourceParams) -> Result<u32, MemFault> {
        Ok(level(&Constants::RETAIL, &self.inputs(tag, source)?))
    }
}

impl Levels for GuestLevels<'_> {
    fn level(&self, tag: TagIndex, source: &SourceParams) -> f32 {
        let k = Constants::RETAIL;
        match self.inputs(tag, source) {
            Ok(inputs) => f32::from_bits(level(&k, &inputs)),
            Err(_) => {
                self.faults.set(self.faults.get() + 1);
                f32::from_bits(k.floor)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeGuest;

    const TAG: TagIndex = TagIndex(0xe100_0003);
    const ENTRIES: u32 = 0x0060_0000;
    const SOUND: u32 = 0x0060_1000;
    const GLOBALS: u32 = 0x0060_2000;
    const CLASSES_TAG: u32 = 0x0060_3000;
    const CLASSES: u32 = 0x0060_4000;
    const GESTALT_AT: u32 = 0x0060_5000;
    const SCALES: u32 = 0x0060_6000;
    const STATES: u32 = 0x0060_7000;
    const SOUND_STATE: u32 = 0x0060_8000;
    const CLASS: u32 = 5;

    fn guest() -> FakeGuest {
        let mut g = FakeGuest::new();
        for page in [TAG_ENTRIES.base, GLOBALS_TAG.base, SOUND_GLOBALS.base, GESTALT, CLASS_STATES] {
            g.map(page & !0xfff, 0x1000);
        }
        g.map(ENTRIES, 0x9000);
        g.put_u32(TAG_ENTRIES.base, ENTRIES)
            .put_u32(ENTRIES + 3 * 16 + 8, SOUND)
            .put_u32(ENTRIES + 7 * 16 + 8, CLASSES_TAG)
            .put_u32(GLOBALS_TAG.base, GLOBALS)
            .put_u32(GLOBALS + 0xc0, 1)
            .put_u32(GLOBALS + 0xc4, GLOBALS + 0x100)
            .put_u32(GLOBALS + 0x104, 7)
            .put_u32(CLASSES_TAG + 4, CLASSES)
            .put_u32(GESTALT, GESTALT_AT)
            .put_u32(GESTALT_AT + 0xc, SCALES)
            .put_u32(CLASS_STATES, STATES)
            .put_u32(SOUND_GLOBALS.base, SOUND_STATE)
            .put_u8(SOUND + 2, CLASS as u8)
            .put_u8(SOUND + 0xb, 2);
        let row = CLASSES + CLASS * CLASS_ROW;
        g.put_u32(row + 0x20, (-6.0f32).to_bits())
            .put_u32(row + 0x24, (-2.0f32).to_bits())
            .put_u32(SCALES + 2 * 20, (-1.0f32).to_bits())
            .put_u32(SCALES + 2 * 20 + 4, 0)
            .put_u32(STATES + CLASS * 16 + 4, (-3.0f32).to_bits())
            .put_u32(SOUND_STATE + DUCK_STATE, 1)
            .put_u32(SOUND_STATE + DUCK_STATE + 4, 0.5f32.to_bits())
            .put_u32(SOUND_STATE + DUCK_STATE + 8, u32::MAX);
        let duck = row + DUCKS + 16;
        g.put_u32(duck, (-10.0f32).to_bits()).put_u32(duck + 4, 1.0f32.to_bits());
        g
    }

    fn source(scale: f32, offset: f32) -> SourceParams {
        let mut s = SourceParams::default();
        s.0[1] = scale.to_bits();
        s.0[2] = offset.to_bits();
        s
    }

    #[test]
    fn it_reads_the_class_scale_row_state_and_duck_of_the_tag() {
        let g = guest();
        let levels = GuestLevels::new(&g);
        let i = levels.inputs(TAG, &source(0.25, -1.5)).unwrap();
        assert_eq!(i.class_bounds, [(-6.0f32).to_bits(), (-2.0f32).to_bits()]);
        assert_eq!(i.scale_bounds, [(-1.0f32).to_bits(), 0]);
        assert_eq!(i.class_level, (-3.0f32).to_bits());
        let d = i.ducking.unwrap();
        assert_eq!((d.current, d.previous), (1, -1));
        assert_eq!(d.current_row.level, (-10.0f32).to_bits());
        assert_eq!(d.previous_row, Duck::default());
        assert_eq!((i.scale, i.offset), (0.25f32.to_bits(), (-1.5f32).to_bits()));
    }

    #[test]
    fn a_class_flagged_bit_0_reads_no_ducking() {
        let mut g = guest();
        g.put_u8(STATES + CLASS * 16 + 0xc, 1);
        let levels = GuestLevels::new(&g);
        assert_eq!(levels.inputs(TAG, &source(0.0, 0.0)).unwrap().ducking, None);
    }

    #[test]
    fn the_level_adds_the_duck_class_level_and_offset_to_the_mix() {
        let g = guest();
        let levels = GuestLevels::new(&g);
        // Scale 0: the low bound, -1 + -6 = -7 dB; duck -10 * 0.5; class -3; offset -1.5.
        let level = levels.level(TAG, &source(0.0, -1.5));
        assert!((level - (-7.0 - 5.0 - 3.0 - 1.5)).abs() < 1e-4, "{level}");
        assert_eq!(levels.faults.get(), 0);
    }

    #[test]
    fn an_unmapped_tag_counts_a_fault_and_plays_at_the_floor() {
        let mut g = guest();
        g.put_u32(ENTRIES + 3 * 16 + 8, 0x0100_0000);
        let levels = GuestLevels::new(&g);
        assert_eq!(levels.level(TAG, &source(0.0, 0.0)), -64.0);
        assert_eq!(levels.faults.get(), 1);
    }
}
