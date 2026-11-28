//! Looping sounds on the sound table (docs/engine/sound-instances.md,
//! "Looping sounds").
//!
//! A [`Looping`] is one playing looping sound: up to [`TRACKS`] tracks,
//! each a chain of segments, and its detail sounds' deadlines. It lives in
//! the table's loop pool (retail's `"looping sounds"` array, 128 rows) and
//! every sound it plays is an ordinary [`Instance`](crate::instances::Instance)
//! owned by [`Owner::Loop`]. The segment chain is retail's (the transition
//! tables at `0x44a150` and `0x44a16c`, [`playing::select`]); when a
//! segment moves on is the table's: on its instance's computed end tick, in
//! [`Sounds::step_loops`], never on the device's channel.
//!
//! ```text
//!        normal                     alternate
//! in ──> loop ⟲            in ──> alt trans in ──> alt loop ⟲
//! alt loop ─> alt trans out ─> loop      loop ──> alt trans in
//! stop: out (or alt out from the alternate sequence), others fade
//! ```

use sim_world::{Handle, Pool, Reader, Record, RestoreError, Rng, Writer};

use crate::instances::{FPU_CONTROL, Levels, Request, SoundTags, Sounds, SourceParams, TagIndex};
use crate::owners::{Owner, OwnerHost, Phase};
use crate::schedule::duration_ticks;
use crate::segments::playing::{self, Segments};
use crate::segments::stopping::OutSegment;

/// Rows in the loop pool: retail's `0x80` (`0x21a0ca`).
pub const CAPACITY: u32 = 128;
/// `"looping sounds"` (`0x4570ec`) seeds the first salt.
pub const POOL_NAME: u16 = u16::from_le_bytes(*b"lo");
/// Tracks a loop holds.
pub const TRACKS: usize = 3;
/// `[0x45dccc]`: seconds to the mixer clock's milliseconds.
pub const CLOCK_SCALE: u32 = 0x447a_0000;

/// A track segment: its index into the track tag's sound references
/// (`0x44a188`: `+0x14`, `+0x1c`, `+0x24`, `+0x40`, `+0x2c`, `+0x48`, `+0x34`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Segment(pub u8);

impl Segment {
    pub const IN: Segment = Segment(0);
    pub const LOOP: Segment = Segment(1);
    pub const OUT: Segment = Segment(2);
    pub const ALT_TRANS_IN: Segment = Segment(3);
    pub const ALT_LOOP: Segment = Segment(4);
    pub const ALT_TRANS_OUT: Segment = Segment(5);
    pub const ALT_OUT: Segment = Segment(6);
    pub const COUNT: usize = 7;

    /// The normal sequence's successors, `0x44a150`.
    pub const NORMAL_NEXT: [u8; 7] = [1, 1, 2, 5, 5, 1, 6];
    /// The alternate sequence's successors, `0x44a16c`.
    pub const ALTERNATE_NEXT: [u8; 7] = [3, 3, 2, 4, 4, 3, 6];

    /// The segment after this one in the normal or the alternate sequence.
    /// A segment that is its own successor repeats.
    pub fn next(self, alternate: bool) -> Segment {
        let table = if alternate { Self::ALTERNATE_NEXT } else { Self::NORMAL_NEXT };
        Segment(table.get(usize::from(self.0)).copied().unwrap_or(self.0))
    }

    /// In the alternate sequence (alt trans in, alt loop, alt out).
    pub fn in_alternate(self) -> bool {
        Segments::alternate(self.0)
    }
}

/// What the table reads of one track of a looping sound tag.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TrackTag {
    /// Track `+4`: bit 0 plays the in on top of the loop, bit 1 the outs on
    /// top, bit 2 the alternate transitions on top and crossfades a swap,
    /// bit 4 fades out with no alt out sound.
    pub flags: u32,
    /// The sound per [`Segment`], `None` where the tag has none.
    pub segments: [Option<TagIndex>; Segment::COUNT],
    /// Track `+0xc`: the fade of a sound replaced at a start, seconds.
    pub fade_in: f32,
    /// Track `+0x50`: the crossfade of an alternate swap, seconds.
    pub crossfade: f32,
    /// The fade of every sound of the track at a stop, seconds.
    pub fade_out: f32,
}

impl TrackTag {
    fn sound(&self, segment: Segment) -> Option<TagIndex> {
        self.segments.get(usize::from(segment.0)).copied().flatten()
    }
}

/// One detail sound of a looping sound tag (`+0x28` rows of `0x34` bytes).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DetailTag {
    pub tag: TagIndex,
    /// `+12`, `+16`: the seconds between plays.
    pub interval: [f32; 2],
    /// `+0x18`: bit 0 plays only outside the alternate, bit 1 only in it.
    pub flags: u8,
}

/// A looping sound tag (`lsnd`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LoopTag {
    pub tracks: Vec<TrackTag>,
    pub details: Vec<DetailTag>,
}

/// One track's state (`+0x88` rows): the segment it plays and its sounds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Track {
    /// `+0xc`: the segment playing.
    pub current: Segment,
    /// `+0xd`: the segment the chain moved to.
    pub queued: Segment,
    /// The segment's sound: the one whose end moves the chain on.
    pub primary: Option<Handle>,
    /// Every sound the track holds, in link order: the primary, overlays
    /// and sounds still fading out.
    pub members: Vec<Handle>,
}

/// One playing looping sound.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Looping {
    pub tag: TagIndex,
    /// The object or datum it plays on.
    pub owner: u32,
    pub source: SourceParams,
    /// `+0x50` bit 0: the alternate sequence is wanted.
    pub alternate: bool,
    /// Stopped: the tracks play their outs and fade, then the loop is freed.
    pub stopping: bool,
    /// Its owner reached it this tick (`0x21a250` stamps `+0x53` with the
    /// sound globals' frame byte `+0x7f`). One its owner left is freed by
    /// [`Sounds::free_unrefreshed_loops`].
    pub touched: bool,
    /// `+0x50` bit 1, the last call's flag bit 1: a loop its owner left
    /// lives on while a sound of it plays.
    pub keep: bool,
    pub tracks: Vec<Track>,
    /// `+0x58`: per detail sound, the mixer clock (ms) it next plays after.
    pub details: Vec<i32>,
}

/// Why a loop did not start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopError {
    /// The tag is not a looping sound in [`SoundTags`].
    UnknownTag(TagIndex),
    /// More tracks than a loop holds.
    TooManyTracks(usize),
    /// The loop pool is full.
    Full,
}

/// The mixer clock on `tick`, in milliseconds: what the engine advances from the wall clock (`0x126a34`), at 30 ticks a second.
pub fn clock_ms(tick: u32) -> i32 {
    (u64::from(tick) * 1000 / 30) as i32
}

/// Seconds to ticks: truncated to milliseconds as the fades do
/// (`cvttss2si` of seconds times 1000), then the end-tick rounding.
pub fn seconds_to_ticks(seconds: f32) -> u32 {
    let ms = (seconds * f32::from_bits(CLOCK_SCALE)) as i32;
    duration_ticks(ms.max(0) as u32)
}

/// A detail sound's next deadline, `0x21b751`: one draw, the interval
/// sampled in extended precision, times 1000, plus the clock, truncated.
pub fn detail_deadline(rng: &mut Rng, interval: [f32; 2], now_ms: i32) -> i32 {
    use x87::abi as fp;
    let draw = (rng.next_u32() >> 16) as u16;
    let mut fpu = fp::X87Abi::default();
    fp::ldcw(&mut fpu, FPU_CONTROL);
    crate::loops::detail_deadline(
        &mut fpu,
        [interval[0].to_bits(), interval[1].to_bits()],
        draw,
        crate::instances::DRAW_SCALE,
        CLOCK_SCALE,
        now_ms,
    );
    fp::st_i32_trunc(&fpu, 0).1.value
}

const NONE: u32 = u32::MAX;

impl Sounds {
    pub fn loops(&self) -> &Pool<Looping> {
        &self.loops
    }

    /// Start a looping sound on `tick`: each track plays its in (with the
    /// loop under it when the in is an overlay) or its first segment, and
    /// each detail sound draws its first deadline.
    #[allow(clippy::too_many_arguments)]
    pub fn loop_start(
        &mut self,
        rng: &mut Rng,
        tags: &dyn SoundTags,
        levels: &dyn Levels,
        tag: TagIndex,
        owner: u32,
        source: SourceParams,
        alternate: bool,
        tick: u32,
    ) -> Result<Handle, LoopError> {
        let def = tags.looping(tag).ok_or(LoopError::UnknownTag(tag))?.clone();
        if def.tracks.len() > TRACKS {
            return Err(LoopError::TooManyTracks(def.tracks.len()));
        }
        let looping = Looping {
            tag,
            owner,
            source,
            alternate,
            stopping: false,
            touched: true,
            keep: false,
            tracks: vec![Track::default(); def.tracks.len()],
            details: vec![0; def.details.len()],
        };
        let handle = self.loops.alloc(looping).ok_or(LoopError::Full)?;
        for (i, track) in def.tracks.iter().enumerate() {
            let intro = track.sound(Segment::IN);
            let overlay = Segments::from_track_flags(track.flags).overlay(0);
            match intro {
                Some(sound) if !overlay => {
                    let primary = self.play(rng, tags, levels, handle, i, sound, None, tick);
                    self.set_track(handle, i, Segment::IN, primary);
                }
                _ => {
                    if let Some(sound) = intro {
                        self.play(rng, tags, levels, handle, i, sound, None, tick);
                    }
                    self.advance(rng, tags, levels, handle, i, Segment::IN, None, tick);
                }
            }
        }
        let now = clock_ms(tick);
        for (d, detail) in def.details.iter().enumerate() {
            let deadline = detail_deadline(rng, detail.interval, now);
            if let Some(l) = self.loops.get_mut(handle) {
                l.details[d] = deadline;
            }
        }
        Ok(handle)
    }

    /// Ask for the alternate sequence or the normal one. A track whose
    /// segment is in the other sequence moves on at once: its transition
    /// plays, the new segment starts, and with track flag bit 2 the old
    /// segment crossfades out over the track's crossfade.
    #[allow(clippy::too_many_arguments)]
    pub fn loop_set_alternate(
        &mut self,
        rng: &mut Rng,
        tags: &dyn SoundTags,
        levels: &dyn Levels,
        handle: Handle,
        alternate: bool,
        tick: u32,
        owners: &mut dyn OwnerHost,
    ) -> bool {
        let Some(l) = self.loops.get_mut(handle) else { return false };
        if l.stopping {
            return false;
        }
        l.alternate = alternate;
        let tag = l.tag;
        let Some(def) = tags.looping(tag).cloned() else { return false };
        for (i, track) in def.tracks.iter().enumerate() {
            let Some(t) = self.loops.get(handle).and_then(|l| l.tracks.get(i)) else { continue };
            if t.current.in_alternate() == alternate {
                continue;
            }
            let old = t.primary;
            let current = t.current;
            self.advance(rng, tags, levels, handle, i, current, None, tick);
            if let Some(old) = old
                && track.flags & 4 != 0
                && self.loops.get(handle).and_then(|l| l.tracks.get(i)).is_some_and(|t| t.primary != Some(old))
            {
                self.stop(old, tick, seconds_to_ticks(track.crossfade), owners);
            }
        }
        true
    }

    /// Stop a looping sound: each track plays its out (the alt out from
    /// the alternate sequence when it has one or fades without one,
    /// `OutSegment`), every other sound of the track fades over its fade
    /// out, and the loop is freed once its tracks are empty.
    pub fn loop_stop(
        &mut self,
        rng: &mut Rng,
        tags: &dyn SoundTags,
        levels: &dyn Levels,
        handle: Handle,
        tick: u32,
        owners: &mut dyn OwnerHost,
    ) -> bool {
        let Some(l) = self.loops.get_mut(handle) else { return false };
        if l.stopping {
            return false;
        }
        l.stopping = true;
        let tag = l.tag;
        let Some(def) = tags.looping(tag).cloned() else { return true };
        for (i, track) in def.tracks.iter().enumerate() {
            let Some(t) = self.loops.get(handle).and_then(|l| l.tracks.get(i)).cloned() else { continue };
            for member in &t.members {
                self.stop(*member, tick, seconds_to_ticks(track.fade_out), owners);
            }
            let selector = OutSegment::alternate_selector(t.current.0);
            let out = match OutSegment::select(selector, track.flags, track.sound(Segment::ALT_OUT).map(|s| s.0)) {
                OutSegment::Alternate => Segment::ALT_OUT,
                OutSegment::Normal => Segment::OUT,
            };
            let played = track.sound(out).and_then(|sound| self.play(rng, tags, levels, handle, i, sound, None, tick));
            self.set_track(handle, i, out, played);
        }
        true
    }

    /// The loop of `tag` on `owner` (`0x21cf80`): a playing one before a
    /// stopped one, each the first in slot order.
    pub fn loop_find(&self, tag: TagIndex, owner: u32) -> Option<Handle> {
        let of = || self.loops.iter().filter(move |(_, l)| l.tag == tag && l.owner == owner);
        of().find(|(_, l)| !l.stopping).or_else(|| of().next()).map(|(h, _)| h)
    }

    /// Its owner reached the loop this tick (`0x21a250`'s stamp, `0x21a521`),
    /// with the call's flag bit 1 (`0x21abb0`).
    pub fn loop_touch(&mut self, handle: Handle, keep: bool) {
        if let Some(l) = self.loops.get_mut(handle) {
            l.touched = true;
            l.keep = keep;
        }
    }

    /// `0x21b0e0`'s first test, in slot order, before the loops step: a
    /// playing loop its owner did not reach since the last call is freed
    /// (`0x21b070`), unless it keeps (flag bit 1) and a sound of it plays.
    /// Its sounds play out on their own end ticks; it starts no more. Then
    /// every loop waits for its owner again. Freed loops' handles return.
    pub fn free_unrefreshed_loops(&mut self) -> Vec<Handle> {
        let mut freed = Vec::new();
        for handle in self.loops.handles() {
            let Some(l) = self.loops.get_mut(handle) else { continue };
            let sounding = l.tracks.iter().any(|t| !t.members.is_empty());
            if !l.stopping && !l.touched && !(l.keep && sounding) {
                let _ = self.loops.free(handle);
                freed.push(handle);
                continue;
            }
            l.touched = false;
        }
        freed
    }

    /// A loop's new source: the sounds it admits from here on play there.
    pub fn loop_set_source(&mut self, handle: Handle, source: SourceParams) -> bool {
        let Some(l) = self.loops.get_mut(handle) else { return false };
        l.source = source;
        true
    }

    /// Free a loop that holds no sound at once (`0x21b070`, for a loop
    /// that started none). False, and nothing freed, while one plays.
    pub fn loop_free_silent(&mut self, handle: Handle) -> bool {
        let silent = self.loops.get(handle).is_some_and(|l| l.tracks.iter().all(|t| t.members.is_empty()));
        silent && self.loops.free(handle).is_ok()
    }

    /// Stop a looping sound with no outs: every sound of every track fades
    /// over `fade_ticks` (`0x21aa1f`, a stop with a fade given), and the
    /// loop is freed once its tracks are empty.
    pub fn loop_fade_out(&mut self, handle: Handle, tick: u32, fade_ticks: u32, owners: &mut dyn OwnerHost) -> bool {
        let Some(l) = self.loops.get_mut(handle) else { return false };
        if l.stopping {
            return false;
        }
        l.stopping = true;
        let members: Vec<Handle> = l.tracks.iter().flat_map(|t| t.members.iter().copied()).collect();
        for member in members {
            self.stop(member, tick, fade_ticks, owners);
        }
        true
    }

    /// The loops' part of the sound phase, first in it: each loop in slot
    /// order moves each track whose segment sound's end tick has come on to
    /// its next segment (a repeating segment plays again), then plays each
    /// detail sound whose deadline has passed and draws its next one. A
    /// stopped loop with no sounds left is freed.
    pub fn step_loops(&mut self, rng: &mut Rng, tags: &dyn SoundTags,
        levels: &dyn Levels, tick: u32) {
        for handle in self.loops.handles() {
            let Some(l) = self.loops.get(handle).cloned() else { continue };
            if l.stopping {
                if l.tracks.iter().all(|t| t.members.is_empty()) {
                    let _ = self.loops.free(handle);
                }
                continue;
            }
            for (i, t) in l.tracks.iter().enumerate() {
                let due = t.primary.and_then(|p| self.instances.get(p)).is_some_and(|p| {
                    p.phase == Phase::Playing && p.end.is_some_and(|end| end <= tick)
                });
                // The next segment continues the one ending now.
                if due {
                    self.advance(rng, tags, levels, handle, i, t.current, t.primary, tick);
                }
            }
            let Some(def) = tags.looping(l.tag) else { continue };
            let now = clock_ms(tick);
            for (d, detail) in def.details.iter().enumerate() {
                let Some(&deadline) = l.details.get(d) else { continue };
                let gated = (detail.flags & 1 != 0 && l.alternate) || (detail.flags & 2 != 0 && !l.alternate);
                if gated || deadline >= now {
                    continue;
                }
                let mut request = Request { source: l.source, detail: true, ..Request::new(detail.tag) };
                request.level = levels.level(detail.tag, &l.source);
                if self.admit(rng, tags, request, tick).is_ok() {
                    let next = detail_deadline(rng, detail.interval, now);
                    if let Some(l) = self.loops.get_mut(handle) {
                        l.details[d] = next;
                    }
                }
            }
        }
    }

    /// Move track `i` on from `from`: the chain's overlay, then its primary.
    /// A chain that stays put replays the segment's own sound. `follows` is
    /// the primary ending on this tick, which the new primary continues.
    #[allow(clippy::too_many_arguments)]
    fn advance(&mut self, rng: &mut Rng, tags: &dyn SoundTags,
        levels: &dyn Levels, handle: Handle, i: usize, from: Segment, follows: Option<Handle>, tick: u32) {
        let Some(l) = self.loops.get(handle) else { return };
        let alternate = l.alternate;
        let Some(track) = tags.looping(l.tag).and_then(|d| d.tracks.get(i)).copied() else { return };
        let options = Segments::from_track_flags(track.flags);
        let choice = playing::select(
            u32::from(from.0),
            NONE,
            options,
            |k| Ok::<_, ()>(u32::from(Segment(k as u8).next(alternate).0)),
            |k| Ok(track.sound(Segment(k as u8)).map(|s| s.0)),
        )
        .expect("the chain reads no fallible state");
        let segment = Segment(choice.segment as u8);
        if choice.overlay != NONE {
            self.play(rng, tags, levels, handle, i, TagIndex(choice.overlay), None, tick);
        }
        let primary = if choice.primary != NONE {
            self.play(rng, tags, levels, handle, i, TagIndex(choice.primary), follows, tick)
        } else if segment == from && from.next(alternate) == from {
            track.sound(from).and_then(|sound| self.play(rng, tags, levels, handle, i, sound, follows, tick))
        } else {
            None
        };
        self.set_track(handle, i, segment, primary);
    }

    fn set_track(&mut self, handle: Handle, i: usize, segment: Segment, primary: Option<Handle>) {
        if let Some(t) = self.loops.get_mut(handle).and_then(|l| l.tracks.get_mut(i)) {
            t.current = segment;
            t.queued = segment;
            t.primary = primary;
        }
    }

    /// Admit one sound for track `i` and link it to the track.
    #[allow(clippy::too_many_arguments)]
    fn play(&mut self, rng: &mut Rng, tags: &dyn SoundTags,
        levels: &dyn Levels, handle: Handle, i: usize, sound: TagIndex, follows: Option<Handle>, tick: u32) -> Option<Handle> {
        let source = self.loops.get(handle)?.source;
        let owner = Owner::Loop { looping: handle, track: i as u8 };
        let level = levels.level(sound, &source);
        let request = Request { owner, track: Some((handle, i as u8)), source, level, follows, ..Request::new(sound) };
        let played = self.admit(rng, tags, request, tick).ok()?;
        if let Some(t) = self.loops.get_mut(handle).and_then(|l| l.tracks.get_mut(i)) {
            t.members.push(played);
        }
        Some(played)
    }

    /// An instance of track `track` ended: drop it from the track.
    pub(crate) fn unlink(&mut self, looping: Handle, track: u8, handle: Handle) {
        if let Some(t) = self.loops.get_mut(looping).and_then(|l| l.tracks.get_mut(usize::from(track))) {
            t.members.retain(|&m| m != handle);
            if t.primary == Some(handle) {
                t.primary = None;
            }
        }
    }
}

fn handle_opt(w: &mut Writer, h: Option<Handle>) {
    w.u8(u8::from(h.is_some()));
    w.u32(h.unwrap_or(Handle::NONE).raw());
}

fn read_opt(r: &mut Reader<'_>) -> Result<Option<Handle>, RestoreError> {
    let some = r.u8()?;
    let raw = r.u32()?;
    match some {
        0 => Ok(None),
        1 => Ok(Some(Handle::from_raw(raw))),
        _ => Err(RestoreError::Corrupt { kind: sim_world::kind::SOUND, what: "loop handle flag" }),
    }
}

impl Record for Looping {
    fn write(&self, w: &mut Writer) {
        w.u32(self.tag.0);
        w.u32(self.owner);
        for word in self.source.0 {
            w.u32(word);
        }
        w.u8(u8::from(self.alternate) | (u8::from(self.stopping) << 1) | (u8::from(self.touched) << 2) | (u8::from(self.keep) << 3));
        w.u8(self.tracks.len() as u8);
        for t in &self.tracks {
            w.u8(t.current.0);
            w.u8(t.queued.0);
            handle_opt(w, t.primary);
            w.u16(t.members.len() as u16);
            for m in &t.members {
                w.u32(m.raw());
            }
        }
        w.u16(self.details.len() as u16);
        for d in &self.details {
            w.u32(*d as u32);
        }
    }

    fn read(r: &mut Reader<'_>) -> Result<Self, RestoreError> {
        let bad = |what| RestoreError::Corrupt { kind: sim_world::kind::SOUND, what };
        let tag = TagIndex(r.u32()?);
        let owner = r.u32()?;
        let mut source = SourceParams::default();
        for word in &mut source.0 {
            *word = r.u32()?;
        }
        let flags = r.u8()?;
        if flags & !0xf != 0 {
            return Err(bad("loop flags"));
        }
        let count = usize::from(r.u8()?);
        if count > TRACKS {
            return Err(bad("loop track count"));
        }
        let mut tracks = Vec::with_capacity(count);
        for _ in 0..count {
            let current = Segment(r.u8()?);
            let queued = Segment(r.u8()?);
            let primary = read_opt(r)?;
            let n = r.u16()?;
            let members = (0..n).map(|_| r.u32().map(Handle::from_raw)).collect::<Result<_, _>>()?;
            tracks.push(Track { current, queued, primary, members });
        }
        let n = r.u16()?;
        let details = (0..n).map(|_| r.u32().map(|v| v as i32)).collect::<Result<_, _>>()?;
        Ok(Self {
            tag,
            owner,
            source,
            alternate: flags & 1 != 0,
            stopping: flags & 2 != 0,
            touched: flags & 4 != 0,
            keep: flags & 8 != 0,
            tracks,
            details,
        })
    }
}

#[cfg(test)]
mod tests;
