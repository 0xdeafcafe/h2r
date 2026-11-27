//! The sound table: every sound the sim runs, as a record in a Rust pool
//! (docs/engine/sound-instances.md, "The table").
//!
//! [`Sounds`] is the `kind::SOUND` store: a [`Pool<Instance>`] of 384
//! slots with retail's salts and slot order, and the permutation
//! [`Chooser`]'s history. Both are saved and hashed. [`Sounds::admit`]
//! takes retail's draws from the shared [`Rng`] in retail's order
//! (`0x126000`); [`Sounds::step`] ends sounds on computed ticks and calls
//! the [`OwnerHost`] in retail's order. Nothing here knows a channel, a
//! voice or a device, and nothing reads guest memory: tags arrive as
//! [`SoundTags`], built from the map once.

use std::collections::BTreeMap;

use sim_world::{Fnv64, Pool, Reader, Record, RestoreError, Writer};
pub use sim_world::{Handle, Rng};

use crate::chooser::{ChooseError, Chooser};
use crate::events::TableEvent;
use crate::impulse::Selection;
use crate::owners::{Owner, OwnerHost, Phase, Reason, ramp_done};
use crate::schedule::end_tick;

/// Slots in the pool: retail's `0x180` (`0x125486`).
pub const CAPACITY: u32 = 384;
/// The pool's name seed: `"sound instances"`, whose first two bytes seed
/// the first salt (`0x4534b8`, `data_make_valid`'s copy at `0x16b6d2`).
pub const POOL_NAME: u16 = u16::from_le_bytes(*b"so");
/// `[0x45dc50]`, about `1/65535`: a draw's top 16 bits to `0..=1`.
pub const DRAW_SCALE: u32 = 0x3780_0080;
/// `[0x44a0b4]`: a request whose level plus gain is at or under this is silent.
pub const GAIN_FLOOR: f32 = -64.0;
/// `[0x440c48]`: the gain base of a request with no gain of its own.
pub const DEFAULT_GAIN_BASE: f32 = 0.0;
/// The game thread's x87 control word (53-bit precision, all masked): the
/// pitch draw's extended arithmetic runs under it.
pub const FPU_CONTROL: u16 = 0x027f;

/// A sound tag's datum. The table passes it through; [`SoundTags`] reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TagIndex(pub u32);

/// A draw's top 16 bits times [`DRAW_SCALE`], exactly as the x87 holds it
/// (16 bits by 24 is exact in an `f64`). Steps `rng` once.
pub fn unit_draw(rng: &mut Rng) -> f64 {
    f64::from(rng.next_u32() >> 16) * f64::from(f32::from_bits(DRAW_SCALE))
}

/// The gain draw, `0x125260`: `gain + range * r`, through an `f32`.
pub fn gain_draw(rng: &mut Rng, playback: &Playback) -> f32 {
    let r = unit_draw(rng) as f32;
    ((playback.gain_range - 0.0) * r + 0.0) + playback.gain
}

/// The pitch draw, `0x1252f0`: `lo + (hi - lo) * r` in extended precision
/// under [`FPU_CONTROL`], stored as an `f32` by the caller (`0x126116`).
pub fn pitch_draw(rng: &mut Rng, playback: &Playback) -> f32 {
    use x87::abi::{self as fp, arith_op as op, src_kind as src};
    let draw = rng.next_u32() >> 16;
    let mut fpu = fp::X87Abi::default();
    fp::ldcw(&mut fpu, FPU_CONTROL);
    fp::ld_int(&mut fpu, i64::from(playback.pitch[1]));
    fp::ld_int(&mut fpu, i64::from(playback.pitch[0]));
    fp::ld_int(&mut fpu, i64::from(draw));
    fp::arith(&mut fpu, op::MUL, 0, src::F32, DRAW_SCALE.into());
    fp::xch(&mut fpu, 2);
    fp::arith(&mut fpu, op::SUB, 0, src::ST, 1);
    fp::arith(&mut fpu, op::MUL | fp::ARITH_POP, 2, src::ST, 0);
    fp::xch(&mut fpu, 1);
    fp::arith(&mut fpu, op::ADD, 0, src::ST, 1);
    f32::from_bits(fp::st_f32(&fpu, 0).1.value)
}

/// A gestalt playback row (tag `+6` indexes it, `0x38` bytes a row).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Playback {
    /// `+0x10`: the gain draw's base.
    pub gain: f32,
    /// `+0x14`: the gain draw's spread.
    pub gain_range: f32,
    /// `+0x18`, `+0x1a`: the pitch draw's bounds.
    pub pitch: [i16; 2],
}

/// What admission and the chooser read of one sound tag.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SoundTag {
    /// Tag `+0`: bit 3 sets the instance's source flag bit 0.
    pub flags: u8,
    /// The tag's class's `+0xA` byte (class `+2` into the sound globals'
    /// classes): bit 2 sets the source flag `0x80`, bit 5 turns the
    /// chooser's history off.
    pub class_flags: u8,
    pub playback: Playback,
    /// Tag `+8`: the gestalt index of its first pitch range.
    pub first_range: i16,
    /// Tag `+10`.
    pub range_count: i8,
    /// Tag `+11`: the gestalt scale row.
    pub scale: i8,
    /// Tag `+12`: the promotion bank, `0xff` for none.
    pub promotion: u8,
}

impl SoundTag {
    pub fn history_off(&self) -> bool {
        self.class_flags & (1 << 5) != 0
    }
}

/// A gestalt pitch range with its bounds row inlined.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PitchRange {
    /// Bounds `+2`, `+4`: the distance a pitch outside every range is
    /// measured from.
    pub natural: [i16; 2],
    /// Bounds `+6`, `+8`: the pitches the range plays.
    pub playable: [i16; 2],
    /// Range `+8`: the gestalt index of its first permutation.
    pub first_permutation: i16,
    /// Range `+0xA`.
    pub count: i16,
}

/// A gestalt permutation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Permutation {
    /// `+2`: the chance, in 1/65535ths, a pick of it is skipped.
    pub skip: u16,
    /// Its length in milliseconds; `None` when the tag gives none.
    pub length_ms: Option<u32>,
}

/// A gestalt scale row (`20` bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Scale {
    /// `+8`, `+10`: the pitch a request's scale spans.
    pub pitch: [i16; 2],
}

/// The static tag data the table reads, built from the map once. Indexes
/// are the gestalt's absolute ones.
pub trait SoundTags {
    fn sound(&self, tag: TagIndex) -> Option<&SoundTag>;
    fn range(&self, index: u16) -> Option<&PitchRange>;
    fn permutation(&self, index: u16) -> Option<&Permutation>;
    fn scale(&self, index: i8) -> Option<&Scale>;
    /// The promotion bank's range for `rule`, as stored (`0xffff` for none).
    fn promotion(&self, bank: u8, rule: u16) -> Option<u16>;
    /// A looping sound tag's tracks and detail sounds.
    fn looping(&self, _tag: TagIndex) -> Option<&crate::looping::LoopTag> {
        None
    }
}

/// [`SoundTags`] as plain vectors.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TagTable {
    pub sounds: BTreeMap<TagIndex, SoundTag>,
    pub ranges: Vec<PitchRange>,
    pub permutations: Vec<Permutation>,
    pub scales: Vec<Scale>,
    /// Per bank, its rows' ranges (`0x218f10`: bank `28` bytes, row `16`).
    pub promotions: Vec<Vec<u16>>,
    pub loops: BTreeMap<TagIndex, crate::looping::LoopTag>,
}

impl SoundTags for TagTable {
    fn sound(&self, tag: TagIndex) -> Option<&SoundTag> {
        self.sounds.get(&tag)
    }
    fn range(&self, index: u16) -> Option<&PitchRange> {
        self.ranges.get(usize::from(index))
    }
    fn permutation(&self, index: u16) -> Option<&Permutation> {
        self.permutations.get(usize::from(index))
    }
    fn scale(&self, index: i8) -> Option<&Scale> {
        usize::try_from(index).ok().and_then(|i| self.scales.get(i))
    }
    fn promotion(&self, bank: u8, rule: u16) -> Option<u16> {
        self.promotions.get(usize::from(bank))?.get(usize::from(rule)).copied()
    }
    fn looping(&self, tag: TagIndex) -> Option<&crate::looping::LoopTag> {
        self.loops.get(&tag)
    }
}

/// The request's 17 source words (record `+0xC` to `+0x4F`, instance
/// `+0x18` to `+0x5B`): flags, scale, level offset, position and the rest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SourceParams(pub [u32; 17]);

impl SourceParams {
    /// Word 0's low half: bits 8 and 11 play at once, without delay.
    pub fn flags(&self) -> u16 {
        self.0[0] as u16
    }
    /// Word 1 (record `+0x10`): where in the tag's scale the sound plays.
    pub fn scale(&self) -> f32 {
        f32::from_bits(self.0[1])
    }
}

/// `0x1251e0`'s value for a sound the phase admits itself (loop segments,
/// detail sounds): the tag's and class's level for the source's scale. The
/// bridge reads the inputs (the tag's curves, the class's runtime state)
/// inside the phase; it draws nothing.
pub trait Levels {
    fn level(&self, tag: TagIndex, source: &SourceParams) -> f32;
}

/// The same level for every sound: tests, and hosts with no class state.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Flat(pub f32);

impl Levels for Flat {
    fn level(&self, _: TagIndex, _: &SourceParams) -> f32 {
        self.0
    }
}

/// The record fields admission copies unread, as retail stores them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Options {
    /// Instance `+4`: record `+8` with flag bit 1, else 0.
    pub flags: u16,
    /// Instance `+6`: record `+4`.
    pub word_06: u16,
    /// Instance `+8`: record `+0x98` with flag bit 8, else `-1`.
    pub word_08: u32,
    /// Instance `+0xA0`: the promotion rule argument's low byte.
    pub rule: u8,
    /// Instance `+0xA1`: the second argument's low byte.
    pub channel: u8,
    /// Instance `+0xA2`: record `+0x8e`'s low byte sign-extended with flag
    /// bit 6, else `-1`.
    pub word_a2: u16,
    /// Instance `+0xA4`: record `+0x90` with flag bit 9, else `0xff`.
    pub byte_a4: u8,
}

/// One admission request: the record at the start entries' `+0` plus their
/// arguments, read by the bridge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Request {
    pub tag: TagIndex,
    /// The second argument; its low byte is kept.
    pub channel: u8,
    /// The fourth argument: a promotion rule, `-1` for none.
    pub rule: i32,
    /// Record `+4`.
    pub word_04: u16,
    /// Record `+8` (flag bit 1).
    pub flags_08: Option<u32>,
    pub source: SourceParams,
    /// The callback table and payload (flag bit 5), classified.
    pub owner: Owner,
    /// The loop and track that hold it (flags 3 and 4).
    pub track: Option<(Handle, u8)>,
    /// Record `+0x94` (flag bit 7): the gain draw's base.
    pub gain_base: Option<f32>,
    /// Record `+0x9c`, `+0x9e` (flag bit 10): an explicit range and
    /// permutation. Nothing is drawn for them.
    pub selection: Option<(u16, u16)>,
    /// Record `+0x8e` (flag bit 6).
    pub extra_8e: Option<u16>,
    /// Record `+0x90` (flag bit 9).
    pub extra_90: Option<u8>,
    /// Record `+0x98` (flag bit 8).
    pub extra_98: Option<u32>,
    /// `0x1251e0`'s value for the request: the tag's and class's level for
    /// its scale, which reads the class's runtime state. It draws nothing;
    /// the bridge computes it. Level plus gain at or under [`GAIN_FLOOR`]
    /// is silent.
    pub level: f32,
    /// A looping sound's detail sound: where it plays is the host's
    /// (presentation draws from its own seed copy and reads the listener).
    pub detail: bool,
}

impl Request {
    /// A request for `tag` with every optional field absent.
    pub fn new(tag: TagIndex) -> Self {
        Self {
            tag,
            channel: 0,
            rule: -1,
            word_04: 0,
            flags_08: None,
            source: SourceParams::default(),
            owner: Owner::None,
            track: None,
            gain_base: None,
            selection: None,
            extra_8e: None,
            extra_90: None,
            extra_98: None,
            level: 0.0,
            detail: false,
        }
    }
}

/// Why admission returned no handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rejected {
    /// Level plus gain at or under the floor. The gain draw was taken.
    Silent,
    /// No free slot. The gain draw was taken.
    Full,
    /// The tag is not in [`SoundTags`]. Nothing was drawn.
    UnknownTag(TagIndex),
    /// The tag's scale row is missing. The gain and pitch draws were taken.
    NoScale(i8),
    /// The promotion bank or its row is missing.
    NoPromotion { bank: u8, rule: i32 },
    /// The chooser could not choose; the draws so far were taken.
    Choose(ChooseError),
}

/// One running sound.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Instance {
    pub tag: TagIndex,
    pub owner: Owner,
    /// Range and permutation relative to the tag (`+0x9C`, `+0x9D`). The
    /// chunk is the mixer host's and is always 0 here.
    pub selection: Selection,
    /// `+0x94`: the gain base plus the gain draw.
    pub gain: f32,
    /// `+0x90`: the pitch draw.
    pub pitch: f32,
    /// `+0x98`: the seed word after the pitch draw.
    pub seed: u32,
    pub source: SourceParams,
    pub options: Options,
    pub start: u32,
    /// The tick it plays out on. `None` only for a sound nothing ends but a
    /// stop; every admitted sound, loop segments included, has one.
    pub end: Option<u32>,
    pub phase: Phase,
    pub track: Option<(Handle, u8)>,
}

impl Record for Instance {
    fn write(&self, w: &mut Writer) {
        w.u32(self.tag.0);
        self.owner.write(w);
        w.u8(self.selection.range as u8);
        w.u8(self.selection.permutation as u8);
        w.u16(self.selection.chunk as u16);
        w.u32(self.gain.to_bits());
        w.u32(self.pitch.to_bits());
        w.u32(self.seed);
        for word in self.source.0 {
            w.u32(word);
        }
        let o = self.options;
        w.u16(o.flags);
        w.u16(o.word_06);
        w.u32(o.word_08);
        w.u8(o.rule);
        w.u8(o.channel);
        w.u16(o.word_a2);
        w.u8(o.byte_a4);
        w.u32(self.start);
        w.u8(u8::from(self.end.is_some()));
        w.u32(self.end.unwrap_or(0));
        match self.phase {
            Phase::Playing => w.u8(0),
            Phase::Fading { since, until } => {
                w.u8(1);
                w.u32(since);
                w.u32(until);
            }
        }
        w.u8(u8::from(self.track.is_some()));
        let (looping, track) = self.track.unwrap_or((Handle::NONE, 0));
        w.u32(looping.raw());
        w.u8(track);
    }

    fn read(r: &mut Reader<'_>) -> Result<Self, RestoreError> {
        let bad = |what| RestoreError::Corrupt { kind: sim_world::kind::SOUND, what };
        let flag = |r: &mut Reader<'_>| match r.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(bad("instance flag byte")),
        };
        let tag = TagIndex(r.u32()?);
        let owner = Owner::read(r)?;
        let selection =
            Selection { range: r.u8()? as i8, permutation: r.u8()? as i8, chunk: r.u16()? as i16 };
        let gain = f32::from_bits(r.u32()?);
        let pitch = f32::from_bits(r.u32()?);
        let seed = r.u32()?;
        let mut source = SourceParams::default();
        for word in &mut source.0 {
            *word = r.u32()?;
        }
        let options = Options {
            flags: r.u16()?,
            word_06: r.u16()?,
            word_08: r.u32()?,
            rule: r.u8()?,
            channel: r.u8()?,
            word_a2: r.u16()?,
            byte_a4: r.u8()?,
        };
        let start = r.u32()?;
        let has_end = flag(r)?;
        let end = r.u32()?;
        let phase = match r.u8()? {
            0 => Phase::Playing,
            1 => Phase::Fading { since: r.u32()?, until: r.u32()? },
            _ => return Err(bad("instance phase")),
        };
        let has_track = flag(r)?;
        let looping = Handle::from_raw(r.u32()?);
        let track = r.u8()?;
        Ok(Self {
            tag,
            owner,
            selection,
            gain,
            pitch,
            seed,
            source,
            options,
            start,
            end: has_end.then_some(end),
            phase,
            track: has_track.then_some((looping, track)),
        })
    }
}

/// What the table did, for `sim.ledger` (not saved, not hashed).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    pub admitted: u64,
    pub rejected_silent: u64,
    pub rejected_full: u64,
    /// Admitted with no permutation length: ended on the start tick.
    pub refused_no_length: u64,
    /// Finishes by [`Reason`], in [`Reason::ALL`] order.
    pub ended: [u64; 4],
}

impl Counters {
    pub fn ended(&self, reason: Reason) -> u64 {
        self.ended[Reason::ALL.iter().position(|&r| r == reason).unwrap_or(0)]
    }
}

/// The `kind::SOUND` store.
pub struct Sounds {
    pub(crate) instances: Pool<Instance>,
    pub(crate) chooser: Chooser,
    pub(crate) loops: Pool<crate::looping::Looping>,
    /// Starts and stops since the last step, in call order.
    pub(crate) pending: Vec<TableEvent>,
    pub(crate) counters: Counters,
}

impl Default for Sounds {
    fn default() -> Self {
        Self::new()
    }
}

impl Sounds {
    /// Section version of [`Sounds::encode`].
    pub const VERSION: u16 = 1;

    pub fn new() -> Self {
        Self {
            instances: Pool::new(CAPACITY, POOL_NAME),
            chooser: Chooser::new(),
            loops: Pool::new(crate::looping::CAPACITY, crate::looping::POOL_NAME),
            pending: Vec::new(),
            counters: Counters::default(),
        }
    }

    pub fn instances(&self) -> &Pool<Instance> {
        &self.instances
    }
    pub fn get(&self, handle: Handle) -> Option<&Instance> {
        self.instances.get(handle)
    }
    /// The handle names a running sound (retail's pool row `+2` test).
    pub fn live(&self, handle: Handle) -> bool {
        self.instances.get(handle).is_some()
    }
    pub fn chooser(&self) -> &Chooser {
        &self.chooser
    }
    pub fn chooser_mut(&mut self) -> &mut Chooser {
        &mut self.chooser
    }
    pub fn counters(&self) -> &Counters {
        &self.counters
    }

    /// Admit a request on `tick`: `0x126000`'s draws, tests and copies in
    /// its order (the doc's "Admission" table).
    pub fn admit(
        &mut self,
        rng: &mut Rng,
        tags: &dyn SoundTags,
        request: Request,
        tick: u32,
    ) -> Result<Handle, Rejected> {
        let tag = *tags.sound(request.tag).ok_or(Rejected::UnknownTag(request.tag))?;
        // 1, 2: the gain draw, then the level test.
        let draw = gain_draw(rng, &tag.playback);
        let gain = request.gain_base.unwrap_or(DEFAULT_GAIN_BASE) + draw;
        let audible = request.level + gain > GAIN_FLOOR;
        if !audible {
            self.counters.rejected_silent += 1;
            return Err(Rejected::Silent);
        }
        // 3: the slot. The record is filled once the selection is known.
        let placeholder = Instance {
            tag: request.tag,
            owner: request.owner,
            selection: Selection { range: 0, permutation: 0, chunk: 0 },
            gain,
            pitch: 0.0,
            seed: 0,
            source: request.source,
            options: Options::default(),
            start: tick,
            end: Some(tick),
            phase: Phase::Playing,
            track: request.track,
        };
        let Some(handle) = self.instances.alloc(placeholder) else {
            self.counters.rejected_full += 1;
            return Err(Rejected::Full);
        };
        match self.fill(rng, tags, &tag, &request, gain, tick) {
            Ok(instance) => {
                *self.instances.get_mut(handle).expect("just allocated") = instance;
                self.counters.admitted += 1;
                self.pending.push(TableEvent::Started {
                    handle,
                    tag: instance.tag,
                    selection: instance.selection,
                    gain: instance.gain,
                    pitch: instance.pitch,
                    source: instance.source,
                    looping: instance.track.is_some(),
                    detail: request.detail,
                    tick,
                });
                Ok(handle)
            }
            Err(e) => {
                let _ = self.instances.free(handle);
                Err(e)
            }
        }
    }

    /// Steps 4 to 7: the pitch draw, the copies, the selection, the end.
    fn fill(
        &mut self,
        rng: &mut Rng,
        tags: &dyn SoundTags,
        tag: &SoundTag,
        request: &Request,
        gain: f32,
        tick: u32,
    ) -> Result<Instance, Rejected> {
        let pitch = pitch_draw(rng, &tag.playback);
        let seed = rng.state();
        let mut source = request.source;
        if tag.flags & 8 != 0 {
            source.0[0] |= 1;
        }
        if tag.class_flags & 4 != 0 {
            source.0[0] |= 0x80;
        }
        let options = Options {
            flags: request.flags_08.map_or(0, |v| v as u16),
            word_06: request.word_04,
            word_08: request.extra_98.unwrap_or(u32::MAX),
            rule: request.rule as u8,
            channel: request.channel,
            word_a2: request.extra_8e.map_or(u16::MAX, |v| v as u8 as i8 as i16 as u16),
            byte_a4: request.extra_90.unwrap_or(0xff),
        };
        let (range, permutation) = match request.selection {
            Some((range, permutation)) => (range as u8 as i8, permutation as u8 as i8),
            None => {
                let range = if request.rule != -1 {
                    promoted_range(tags, tag, request.rule)?
                } else {
                    let scale = tags.scale(tag.scale).ok_or(Rejected::NoScale(tag.scale))?;
                    let (lo, hi) = (f32::from(scale.pitch[0]), f32::from(scale.pitch[1]));
                    let wanted = ((hi - lo) * request.source.scale() + pitch) + lo;
                    pitch_range(tags, tag, -1, wanted).map_err(Rejected::Choose)?
                };
                let permutation = self
                    .chooser
                    .choose(rng, tags, tag, range as i16)
                    .map_err(Rejected::Choose)?;
                (range as u8 as i8, permutation as u8 as i8)
            }
        };
        let selection = Selection { range, permutation, chunk: 0 };
        let end = match length_ms(tags, tag, selection) {
            Some(ms) => Some(end_tick(tick, ms)),
            None => {
                self.counters.refused_no_length += 1;
                Some(tick)
            }
        };
        Ok(Instance {
            tag: request.tag,
            owner: request.owner,
            selection,
            gain,
            pitch,
            seed,
            source,
            options,
            start: tick,
            end,
            phase: Phase::Playing,
            track: request.track,
        })
    }

    /// Stop a sound (`0x126360`): the owner hears `stopping`, then it fades
    /// for `fade_ticks` and ends with [`Reason::Faded`]. False when the
    /// handle is stale or the sound is already fading.
    pub fn stop(&mut self, handle: Handle, tick: u32, fade_ticks: u32, owners: &mut dyn OwnerHost) -> bool {
        let Some(instance) = self.instances.get(handle).copied() else { return false };
        if instance.phase != Phase::Playing {
            return false;
        }
        if instance.owner.has_host() {
            owners.stopping(&instance.owner, handle, instance.tag);
        }
        if let Some(i) = self.instances.get_mut(handle) {
            i.phase = Phase::Fading { since: tick, until: tick.saturating_add(fade_ticks) };
        }
        self.pending.push(TableEvent::Stopped { handle, fade_ticks, tick });
        true
    }

    /// Finish a sound: `ended` with it still in the pool, then free it
    /// (`0x127320`, `0x127390`).
    pub fn finish(&mut self, handle: Handle, reason: Reason, tick: u32, owners: &mut dyn OwnerHost) -> Option<TableEvent> {
        let instance = *self.instances.get(handle)?;
        if instance.owner.has_host() {
            owners.ended(self, &instance.owner, handle, instance.tag, reason);
        }
        if let Owner::Loop { looping, track } = instance.owner {
            self.unlink(looping, track, handle);
        }
        self.instances.free(handle).ok()?;
        self.counters.ended[Reason::ALL.iter().position(|&r| r == reason).unwrap_or(0)] += 1;
        Some(TableEvent::Ended { handle, reason, tick })
    }

    /// The table's part of the sound phase on `tick`. Each instance in slot
    /// order: a finished fade ends with `Faded`, a gone owner with
    /// `OwnerGone`, a due end with `PlayedOut`. Then each running owned
    /// instance gets its mouth. Returns the starts and stops since the last
    /// step in call order, then the ends in slot order.
    pub fn step(&mut self, tick: u32, owners: &mut dyn OwnerHost) -> Vec<TableEvent> {
        let mut out = std::mem::take(&mut self.pending);
        for handle in self.instances.handles() {
            let Some(instance) = self.instances.get(handle).copied() else { continue };
            let reason = if ramp_done(instance.phase, tick) {
                Some(Reason::Faded)
            } else if instance.owner.has_host() && !owners.alive(&instance.owner) {
                Some(Reason::OwnerGone)
            } else if instance.end.is_some_and(|end| end <= tick) {
                Some(Reason::PlayedOut)
            } else {
                None
            };
            if let Some(reason) = reason {
                out.extend(self.finish(handle, reason, tick, owners));
            }
        }
        for (_, i) in self.instances.iter() {
            if i.owner.has_host() {
                owners.mouth(&i.owner, i.tag, i.selection, tick.saturating_sub(i.start));
            }
        }
        out
    }

    /// The canonical bytes: the instance pool, the chooser, the loop pool.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        let pool = self.instances.encode();
        w.u32(pool.len() as u32);
        w.bytes(&pool);
        self.chooser.encode(&mut w);
        let loops = self.loops.encode();
        w.u32(loops.len() as u32);
        w.bytes(&loops);
        w.finish()
    }

    pub fn decode(version: u16, body: &[u8]) -> Result<Self, RestoreError> {
        let kind = sim_world::kind::SOUND;
        if version != Self::VERSION {
            return Err(RestoreError::SectionVersion { kind, version });
        }
        let mut r = Reader::new(body);
        let len = r.u32()? as usize;
        let pool = r.take(len)?;
        let instances = Pool::decode(kind, CAPACITY, POOL_NAME, 1, pool)?;
        let chooser = Chooser::decode(&mut r)?;
        let len = r.u32()? as usize;
        let loops = r.take(len)?;
        let loops =
            Pool::decode(kind, crate::looping::CAPACITY, crate::looping::POOL_NAME, 1, loops)?;
        r.end(kind)?;
        Ok(Self { instances, chooser, loops, pending: Vec::new(), counters: Counters::default() })
    }

    pub fn section_hash(&self) -> u64 {
        Fnv64::of(&self.encode())
    }
}

/// The promoted range for `rule`, `0x218f10`: the bank's row, or range 0
/// when the tag has no bank or the row holds none.
pub fn promoted_range(tags: &dyn SoundTags, tag: &SoundTag, rule: i32) -> Result<i32, Rejected> {
    if tag.promotion == 0xff {
        return Ok(0);
    }
    let missing = Rejected::NoPromotion { bank: tag.promotion, rule };
    let row = u16::try_from(rule).map_err(|_| missing)?;
    match tags.promotion(tag.promotion, row).ok_or(missing)? {
        0xffff => Ok(0),
        range => Ok(i32::from(range as i16)),
    }
}

fn plays(range: &PitchRange, pitch: f32) -> bool {
    f32::from(range.playable[0]) <= pitch && pitch <= f32::from(range.playable[1])
}

/// The pitch range for `pitch`, `0x218f50`: the preferred range when it
/// plays the pitch, else the first range in tag order that does, else the
/// nearest by natural bounds. `-1` when the tag has no range with
/// permutations.
pub fn pitch_range(tags: &dyn SoundTags, tag: &SoundTag, preferred: i16, pitch: f32) -> Result<i32, ChooseError> {
    let count = i32::from(tag.range_count);
    let row = |i: i32| {
        let absolute = i32::from(tag.first_range) + i;
        u16::try_from(absolute).ok().and_then(|a| tags.range(a)).ok_or(ChooseError::NoRange { range: absolute })
    };
    if preferred != -1 && i32::from(preferred) < count {
        let r = row(i32::from(preferred))?;
        if plays(r, pitch) && r.count > 0 {
            return Ok(i32::from(preferred));
        }
    }
    let mut best = f32::MAX;
    let mut result = -1;
    for i in 0..count {
        let r = row(i)?;
        if r.count <= 0 {
            continue;
        }
        if plays(r, pitch) {
            return Ok(i);
        }
        let high = f32::from(r.natural[1]);
        let distance = if pitch > high { pitch - high } else { f32::from(r.natural[0]) - pitch };
        if best > distance {
            result = i;
            best = distance;
        }
    }
    Ok(result)
}

/// The length of a selection's permutation, if the tag gives one.
pub fn length_ms(tags: &dyn SoundTags, tag: &SoundTag, selection: Selection) -> Option<u32> {
    let range = u16::try_from(selection.range_index(tag.first_range)).ok()?;
    let range = tags.range(range)?;
    let permutation = u16::try_from(selection.permutation_index(range.first_permutation)).ok()?;
    tags.permutation(permutation)?.length_ms
}

#[cfg(test)]
mod tests;
