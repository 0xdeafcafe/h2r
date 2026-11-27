//! The permutation chooser (`0x219070`, `0x219110`) on Rust-owned history
//! (docs/engine/sound-instances.md, "RNG order kept").
//!
//! Retail keeps the history in the sound gestalt's runtime bytes: per pitch
//! range `last` at `+4`, a queued `next` at `+5`, and a used set in the bit
//! array at gestalt `+0x3C`. Here it is a [`Chooser`], saved and hashed
//! with the table.
//!
//! One choice, in retail's order:
//!
//! ```text
//! next queued ─ take it, clear it, last = it          (no draw)
//! otherwise   ─ used = the range's set, last = its last
//!   per try:  index draw ─ a full used set is cleared, keeping only last
//!                        ─ redraw while the index is in the used set
//!             skip draw  ─ taken unless under the permutation's skip fraction
//!             at most 2 x count tries; the last index drawn is the answer
//!   store used and last
//! ```
//!
//! A class with the history flag (class `+0xA` bit 5) reads `last` and
//! `next` as none and the used set as empty, and writes nothing back.

use std::collections::BTreeMap;

use sim_world::{Reader, RestoreError, Rng, Writer};

use crate::instances::{DRAW_SCALE, PitchRange, SoundTag, SoundTags, unit_draw};

/// One pitch range's history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct History {
    /// The permutation last played, `-1` for none.
    pub last: i8,
    /// A permutation queued to play next, `-1` for none.
    pub next: i8,
    /// The permutations played since the set was last cleared, bit per
    /// permutation. Only the bytes retail stores (`(count + 7) / 8`) are kept.
    pub used: u32,
}

impl Default for History {
    fn default() -> Self {
        Self { last: -1, next: -1, used: 0 }
    }
}

/// What one try loop returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pick {
    /// The permutation, relative to the range.
    pub index: i32,
    /// The used set, after the pick.
    pub used: u32,
    /// Every permutation of the range is in the used set after the pick
    /// (the flag `0x219110` writes through its third argument).
    pub exhausted: bool,
}

/// Why a choice could not be made. Retail reads past its tables or spins
/// for these; the table refuses them by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChooseError {
    /// The tag's first range plus the relative range names no range.
    NoRange { range: i32 },
    /// A range with no permutations: retail's try loop never ends.
    EmptyRange { range: i32 },
    /// The range's first permutation plus the index names no permutation.
    NoPermutation { permutation: i32 },
}

/// Per pitch range history, keyed by the gestalt's absolute range index.
/// A range at the default history is not stored, so equal histories save
/// to equal bytes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Chooser {
    ranges: BTreeMap<u16, History>,
}

/// The bytes of the used set retail copies for a range of `count`.
fn used_mask(count: i16) -> u32 {
    match (i32::from(count) + 7) >> 3 {
        n if n >= 4 => u32::MAX,
        n if n <= 0 => 0,
        n => (1u32 << (n * 8)) - 1,
    }
}

/// x86 `shl` by a register: the count is the low byte masked to 5 bits.
fn bit(shift: i32) -> u32 {
    1u32.wrapping_shl(u32::from(shift as u8))
}

impl Chooser {
    pub const VERSION: u16 = 1;

    pub fn new() -> Self {
        Self::default()
    }

    /// The history of an absolute range index.
    pub fn history(&self, range: u16) -> History {
        self.ranges.get(&range).copied().unwrap_or_default()
    }

    /// Replace a range's history (a map's initial bytes, or a test's).
    pub fn set_history(&mut self, range: u16, history: History) {
        if history == History::default() {
            self.ranges.remove(&range);
        } else {
            self.ranges.insert(range, history);
        }
    }

    /// Queue `permutation` to play next on `range` (`0x219310` with a
    /// value): the next choice takes it without a draw. A class with the
    /// history flag stores nothing.
    pub fn queue(&mut self, tag: &SoundTag, range: u16, permutation: i8) {
        if !tag.history_off() {
            let mut h = self.history(range);
            h.next = permutation;
            self.set_history(range, h);
        }
    }

    /// Choose a permutation of `range` (relative to the tag's first range):
    /// `0x219070`. Returns the permutation relative to the range.
    pub fn choose(
        &mut self,
        rng: &mut Rng,
        tags: &dyn SoundTags,
        tag: &SoundTag,
        range: i16,
    ) -> Result<i32, ChooseError> {
        let absolute = i32::from(tag.first_range) + i32::from(range);
        let row = range_of(tags, absolute)?;
        let off = tag.history_off();
        let key = absolute as u16;
        let mut h = self.history(key);
        let next = if off { -1 } else { i32::from(h.next) };
        if next != -1 && next as u16 != 0xffff {
            h.next = -1;
            h.last = next as i8;
            self.set_history(key, h);
            return Ok(next);
        }
        let keeps = !off && row.count > 1;
        let used = if keeps { h.used & used_mask(row.count) } else { 0 };
        let last = if off { -1 } else { i32::from(h.last) };
        let pick = pick(rng, tags, row, absolute, last, used, false)?;
        if !off {
            if keeps {
                h.used = pick.used & used_mask(row.count);
            }
            h.last = pick.index as i8;
            self.set_history(key, h);
        }
        Ok(pick.index)
    }

    pub fn encode(&self, w: &mut Writer) {
        w.u32(self.ranges.len() as u32);
        for (&range, h) in &self.ranges {
            w.u16(range);
            w.u8(h.last as u8);
            w.u8(h.next as u8);
            w.u32(h.used);
        }
    }

    pub fn decode(r: &mut Reader<'_>) -> Result<Self, RestoreError> {
        let bad = |what| RestoreError::Corrupt { kind: sim_world::kind::SOUND, what };
        let mut out = Chooser::new();
        let mut previous = None;
        for _ in 0..r.u32()? {
            let range = r.u16()?;
            let h = History { last: r.u8()? as i8, next: r.u8()? as i8, used: r.u32()? };
            if previous.is_some_and(|p| p >= range) {
                return Err(bad("chooser ranges out of order"));
            }
            if h == History::default() {
                return Err(bad("chooser stores a default history"));
            }
            previous = Some(range);
            out.ranges.insert(range, h);
        }
        Ok(out)
    }
}

fn range_of(tags: &dyn SoundTags, absolute: i32) -> Result<&PitchRange, ChooseError> {
    u16::try_from(absolute)
        .ok()
        .and_then(|i| tags.range(i))
        .ok_or(ChooseError::NoRange { range: absolute })
}

/// The try loop, `0x219110`. `last` is the permutation to keep out of a
/// cleared set (`-1` for none); `invert` folds skip fractions over one half
/// (its fourth argument, 0 from the chooser).
pub fn pick(
    rng: &mut Rng,
    tags: &dyn SoundTags,
    row: &PitchRange,
    absolute: i32,
    mut last: i32,
    mut used: u32,
    invert: bool,
) -> Result<Pick, ChooseError> {
    let count = i32::from(row.count);
    if count <= 0 {
        return Err(ChooseError::EmptyRange { range: absolute });
    }
    let all = bit(count).wrapping_sub(1);
    let mut tries = 0i32;
    loop {
        let index = loop {
            let draw = u32::from((rng.next_u32() >> 16) as u16);
            let index = (draw.wrapping_mul(count as u32) >> 16) as u16 as i16 as i32;
            if !used & all == 0 {
                used = 0;
                if count > 1 && last as u16 != 0xffff {
                    used = bit(last);
                }
                last = -1;
            }
            if used & bit(index) == 0 {
                break index;
            }
        };
        used |= bit(index);
        let permutation = i32::from(row.first_permutation) + index;
        let skip = u16::try_from(permutation)
            .ok()
            .and_then(|p| tags.permutation(p))
            .ok_or(ChooseError::NoPermutation { permutation })?
            .skip;
        let mut fraction = f32::from(skip) * f32::from_bits(DRAW_SCALE);
        if invert && fraction >= 0.5 {
            fraction = 1.0 - fraction;
        }
        if unit_draw(rng) >= f64::from(fraction) {
            return Ok(Pick { index, used, exhausted: !used & all == 0 });
        }
        tries += 1;
        if tries == count * 2 {
            return Ok(Pick { index, used, exhausted: !used & all == 0 });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instances::{Permutation, TagTable};

    fn table(count: i16, skips: &[u16]) -> TagTable {
        let mut t = TagTable::default();
        t.ranges.push(PitchRange { first_permutation: 0, count, ..PitchRange::default() });
        t.permutations = skips.iter().map(|&skip| Permutation { skip, length_ms: Some(100) }).collect();
        t
    }

    /// The index the try loop's first draw would give from `seed`.
    fn index_after(seed: u32, count: u32) -> u32 {
        let mut r = Rng::new(seed);
        ((r.next_u32() >> 16) * count) >> 16
    }

    #[test]
    fn the_chooser_redraws_past_used_permutations() {
        let t = table(4, &[0; 4]);
        let tag = SoundTag::default();
        // Find a seed whose first index draw is 1, then mark 1 used.
        let seed = (0..).find(|&s| index_after(s, 4) == 1).unwrap();
        let mut c = Chooser::new();
        c.set_history(0, History { last: -1, next: -1, used: 0b0010 });
        let mut rng = Rng::new(seed);
        let p = c.choose(&mut rng, &t, &tag, 0).unwrap();
        assert_ne!(p, 1);
        // Redraws take index draws until a free index, then one skip draw.
        let mut probe = Rng::new(seed);
        let mut draws = 0;
        loop {
            draws += 1;
            let i = ((probe.next_u32() >> 16) * 4) >> 16;
            if i != 1 {
                break;
            }
        }
        probe.next_u32();
        assert_eq!(rng.state(), probe.state(), "{draws} index draws then the skip draw");
        let h = c.history(0);
        assert_eq!(h.used, 0b0010 | (1 << p));
        assert_eq!(i32::from(h.last), p);
    }

    #[test]
    fn a_full_used_set_keeps_only_the_last() {
        let t = table(3, &[0; 3]);
        let tag = SoundTag::default();
        let mut c = Chooser::new();
        c.set_history(0, History { last: 2, next: -1, used: 0b111 });
        let mut rng = Rng::new(77);
        let p = c.choose(&mut rng, &t, &tag, 0).unwrap();
        assert_ne!(p, 2, "the last permutation is kept out of the cleared set");
        assert_eq!(c.history(0).used, (1 << 2) | (1 << p));
        // One permutation: the cleared set keeps nothing.
        let one = table(1, &[0]);
        let mut c = Chooser::new();
        c.set_history(0, History { last: 0, next: -1, used: 1 });
        assert_eq!(c.choose(&mut Rng::new(5), &one, &tag, 0), Ok(0));
        assert_eq!(c.history(0).used, 1, "a single-permutation range stores no set");
    }

    #[test]
    fn a_queued_next_is_taken_without_a_draw() {
        let t = table(4, &[0; 4]);
        let tag = SoundTag::default();
        let mut c = Chooser::new();
        c.queue(&tag, 0, 3);
        let mut rng = Rng::new(9);
        assert_eq!(c.choose(&mut rng, &t, &tag, 0), Ok(3));
        assert_eq!(rng.state(), 9);
        assert_eq!(c.history(0), History { last: 3, next: -1, used: 0 });
    }

    #[test]
    fn the_history_flag_reads_none_and_writes_nothing() {
        let t = table(4, &[0; 4]);
        let tag = SoundTag { class_flags: 1 << 5, ..SoundTag::default() };
        let mut c = Chooser::new();
        let kept = History { last: 1, next: 2, used: 0b1111 };
        c.set_history(0, kept);
        c.queue(&tag, 0, 0);
        let p = c.choose(&mut Rng::new(3), &t, &tag, 0).unwrap();
        assert!((0..4).contains(&p));
        assert_eq!(c.history(0), kept);
    }

    #[test]
    fn a_skip_fraction_spends_tries_up_to_twice_the_count() {
        // Every permutation skips always (fraction ~1): 2 x count tries,
        // each one index draw (the set clears as it fills) and one skip draw.
        let t = table(2, &[0xffff, 0xffff]);
        let mut rng = Rng::new(1234);
        let row = t.ranges[0];
        let pick = pick(&mut rng, &t, &row, 0, -1, 0, false).unwrap();
        assert!((0..2).contains(&pick.index));
        let mut probe = Rng::new(1234);
        let mut used = 0u32;
        let mut last = -1i32;
        for _ in 0..4 {
            loop {
                let i = (((probe.next_u32() >> 16) * 2) >> 16) as i32;
                if used == 0b11 {
                    used = if last != -1 { 1 << last } else { 0 };
                    last = -1;
                }
                if used & (1 << i) == 0 {
                    used |= 1 << i;
                    break;
                }
            }
            probe.next_u32();
        }
        assert_eq!(rng.state(), probe.state());
    }

    #[test]
    fn refusals_are_named() {
        let t = table(0, &[]);
        let tag = SoundTag::default();
        let mut c = Chooser::new();
        assert_eq!(c.choose(&mut Rng::new(1), &t, &tag, 0), Err(ChooseError::EmptyRange { range: 0 }));
        assert_eq!(c.choose(&mut Rng::new(1), &t, &tag, 4), Err(ChooseError::NoRange { range: 4 }));
        let short = table(2, &[]);
        assert!(matches!(
            c.choose(&mut Rng::new(1), &short, &tag, 0),
            Err(ChooseError::NoPermutation { .. })
        ));
    }

    #[test]
    fn history_round_trips() {
        let mut c = Chooser::new();
        c.set_history(4, History { last: 1, next: -1, used: 3 });
        c.set_history(2, History { last: -1, next: 0, used: 0 });
        c.set_history(9, History::default());
        let mut w = Writer::new();
        c.encode(&mut w);
        let bytes = w.finish();
        let mut r = Reader::new(&bytes);
        assert_eq!(Chooser::decode(&mut r).unwrap(), c);
        assert!(r.is_empty());
    }
}
