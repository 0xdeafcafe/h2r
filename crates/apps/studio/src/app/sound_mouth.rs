//! Mouth curves, write (docs/editor/rich-editors.md, "Sounds and facial
//! animation", slice A4): redrawing a permutation's curve on the Sound
//! window's track or with the API, and **From the sound**, which fills it
//! from the sound's loudness.
//!
//! **What is written.** A redraw has the curve's own length (a curve
//! follows the sound's length), so it is a fixed-size patch of the
//! extra-info resource in the map's raw data
//! ([`scenario_edit::RawPatch`], `map::MapEditor::patch_raw`, handed to the
//! save by [`App::push_sound_changes`]). Nothing else in the file changes
//! but the checksum. The edits live here until the map is saved; each is
//! one step of its sound's document beside the sound's own writes, numbered
//! with them ([`super::TagDoc::next_step`]), so the sound's tab undoes them
//! in the order they were made (docs/editor/studio-flow.md, "Shared
//! patterns"). `sound.mouth`'s `undo` takes the last redraw of the shown
//! map, any sound ([`App::sound_mouth_undo`]).
//!
//! **What is refused, by name** ([`check_writable`]): a sound whose
//! extra-info resource is in another cache file (the editor never writes
//! those); a permutation that is a split chain (more than one chunk: no
//! retail example of its segment timing); an unvoiced permutation, or a
//! curve of another length (raw data cannot be appended yet); a map that
//! is browsed, not open.
//!
//! **From the sound** ([`envelope`]) is our own generator and claims
//! nothing about how the retail curves were made: the RMS of each 30 Hz
//! tick of the decoded audio, scaled so the loudest tick is `gain`, then
//! smoothed forward by `smoothing`.
use std::collections::BTreeMap;
use std::sync::Arc;

use map::{ResourceFile, TagId};
use scenario_edit::{GeometryChanges, RawPatch};
use tags::sound::{MOUTH_HZ, MouthSpan};

use super::App;
use super::audition::{Clip, SoundError};

/// A permutation, in the map and sound the edit belongs to.
type Slot = (String, TagId, usize, usize);

/// One redrawn curve: where it is stored, as stored, and as drawn.
struct Edit {
    span: MouthSpan,
    drawn: Arc<[u8]>,
}

/// One redraw of one permutation's curve, a step of its sound's document:
/// the curve as drawn before it and after it (`None`: as the file stores
/// it), and its number among the document's steps.
struct Step {
    slot: Slot,
    span: MouthSpan,
    before: Option<Arc<[u8]>>,
    after: Option<Arc<[u8]>>,
    serial: u64,
}

/// The redrawn curves of the shown map and their steps, not undone and
/// undone.
#[derive(Default)]
pub struct Edits {
    edits: BTreeMap<Slot, Edit>,
    undo: Vec<Step>,
    redo: Vec<Step>,
}

impl Edits {
    /// Take the last step `of` accepts from the undo list (`undo`) or the
    /// redo list, and put it on the other: the slot, where its curve is
    /// stored, and the curve it shows now (`None`: as the file stores it).
    fn step_back(
        &mut self,
        undo: bool,
        of: impl Fn(&Step) -> bool,
    ) -> Option<(Slot, MouthSpan, Option<Arc<[u8]>>)> {
        let (from, to) = if undo {
            (&mut self.undo, &mut self.redo)
        } else {
            (&mut self.redo, &mut self.undo)
        };
        let i = from.iter().rposition(of)?;
        let step = from.remove(i);
        let shown = if undo {
            step.before.clone()
        } else {
            step.after.clone()
        };
        let place = (step.slot.clone(), step.span.clone());
        to.push(step);
        Some((place.0, place.1, shown))
    }

    /// Record `step` as made: the curve at its slot was `before` and is
    /// `after`. A new step leaves nothing of the same sound's to redo.
    fn note(&mut self, step: Step) {
        let (key, sound, ..) = &step.slot;
        self.redo.retain(|s| s.slot.0 != *key || s.slot.1 != *sound);
        self.undo.push(step);
    }
}

/// What a write did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MouthWrite {
    /// Ticks written.
    pub ticks: usize,
    /// Ticks whose byte changed from what the permutation showed.
    pub changed: usize,
    /// Whether the permutation now differs from the file.
    pub edited: bool,
}

/// Why `new` cannot be written over the curve stored at `span`, if it
/// cannot (the refusals above).
pub fn check_writable(span: &MouthSpan, chunks: u16, new: &[u8]) -> Result<(), String> {
    if span.resource.file != ResourceFile::Local {
        return Err(format!(
            "this sound's mouth data is in the shared cache {:?}, which the editor never writes",
            span.resource.file
        ));
    }
    if chunks > 1 {
        return Err(format!(
            "this permutation is a split chain of {chunks} chunks; its segment timing has no retail example, so its curve is not written"
        ));
    }
    if new.len() != span.curve.len() {
        return Err(format!(
            "the curve has {} ticks and {} were given; a curve keeps the sound's length, because raw data cannot be appended yet",
            span.curve.len(),
            new.len()
        ));
    }
    Ok(())
}

/// The mouth curve of `clip` for `ticks` 30 Hz ticks: per tick, the RMS of
/// the channel-averaged samples, divided by the loudest tick's, times
/// `gain` (0 to 4), clamped to one; then each tick is `smoothing` (0 to
/// 0.95) of the one before plus the rest of itself. A silent clip is all
/// zero.
pub fn envelope(clip: &Clip, ticks: usize, gain: f32, smoothing: f32) -> Vec<u8> {
    let (frames, ch) = (clip.frames(), usize::from(clip.channels.max(1)));
    let rate = u64::from(clip.rate.max(1));
    let level = |t: usize| -> f32 {
        let lo = (t as u64 * rate / u64::from(MOUTH_HZ)) as usize;
        let hi = (((t as u64 + 1) * rate / u64::from(MOUTH_HZ)) as usize).min(frames);
        if lo >= hi {
            return 0.0;
        }
        let sum: f64 = (lo..hi)
            .map(|f| {
                let m = clip.pcm[f * ch..(f + 1) * ch]
                    .iter()
                    .map(|&s| f64::from(s))
                    .sum::<f64>()
                    / ch as f64
                    / 32768.0;
                m * m
            })
            .sum();
        (sum / (hi - lo) as f64).sqrt() as f32
    };
    let levels: Vec<f32> = (0..ticks).map(level).collect();
    let loudest = levels.iter().copied().fold(0.0f32, f32::max);
    if loudest <= 0.0 {
        return vec![0; ticks];
    }
    let (gain, a) = (gain.clamp(0.0, 4.0), smoothing.clamp(0.0, 0.95));
    let mut y = 0.0f32;
    levels
        .iter()
        .map(|&l| {
            y = a * y + (1.0 - a) * (l / loudest * gain).min(1.0);
            (y.clamp(0.0, 1.0) * 255.0).round() as u8
        })
        .collect()
}

impl App {
    /// The redrawn curve of this permutation, if it has one.
    pub(super) fn mouth_edited(
        &self,
        id: TagId,
        range: usize,
        permutation: usize,
    ) -> Option<Arc<[u8]>> {
        let slot = (self.sound_key(id).0, id, range, permutation);
        self.audition
            .mouth
            .edits
            .get(&slot)
            .map(|e| e.drawn.clone())
    }

    /// Whether the permutation's curve is redrawn and not yet saved.
    pub fn sound_mouth_is_edited(&self, id: TagId, range: usize, permutation: usize) -> bool {
        self.mouth_edited(id, range, permutation).is_some()
    }

    /// Show `drawn` (`None`: the file's curve) as the curve at `slot`: what
    /// an undone or a redone step leaves.
    fn mouth_show(&mut self, slot: &Slot, span: &MouthSpan, drawn: Option<Arc<[u8]>>) {
        match drawn {
            Some(drawn) => {
                self.audition.mouth.edits.insert(
                    slot.clone(),
                    Edit {
                        span: span.clone(),
                        drawn,
                    },
                );
            }
            None => {
                self.audition.mouth.edits.remove(slot);
            }
        }
        self.push_sound_changes();
    }

    /// The number of sound `id`'s newest step not undone, in the map shown.
    pub(super) fn sound_mouth_step(&self, id: TagId) -> Option<u64> {
        let (key, ..) = self.sound_key(id);
        self.audition
            .mouth
            .undo
            .iter()
            .rev()
            .find(|s| s.slot.0 == key && s.slot.1 == id)
            .map(|s| s.serial)
    }

    /// The number of sound `id`'s oldest undone step in the map shown, the
    /// next a redo takes.
    pub(super) fn sound_mouth_redo_step(&self, id: TagId) -> Option<u64> {
        let (key, ..) = self.sound_key(id);
        self.audition
            .mouth
            .redo
            .iter()
            .rev()
            .find(|s| s.slot.0 == key && s.slot.1 == id)
            .map(|s| s.serial)
    }

    /// Undo (or redo) a step of sound `id`'s curve: the newest not undone,
    /// or the oldest undone. Whether one moved.
    pub(super) fn sound_mouth_step_in(&mut self, id: TagId, undo: bool) -> bool {
        let (key, ..) = self.sound_key(id);
        let Some((slot, span, shown)) = self
            .audition
            .mouth
            .step_back(undo, |s| s.slot.0 == key && s.slot.1 == id)
        else {
            return false;
        };
        self.mouth_show(&slot, &span, shown);
        true
    }

    /// Redraw the curve of permutation `permutation` of pitch range `range`
    /// of sound `id` as `curve` (one byte per tick, the stored length).
    /// One undo step; saved with the map. Refused by name where
    /// [`check_writable`] says.
    pub fn sound_mouth_write(
        &mut self,
        id: TagId,
        range: usize,
        permutation: usize,
        curve: &[u8],
    ) -> Result<MouthWrite, SoundError> {
        if self.browsing().is_some() || !self.session.is_open() {
            return Err(SoundError::Refused(
                "open the map in the Scenario tool first: a browsed map's sounds are shown, not edited"
                    .into(),
            ));
        }
        let Some((span, p)) = self.sound_mouth_place(id, range, permutation)? else {
            return Err(SoundError::Refused(
                "this permutation has no mouth curve, and raw data cannot be appended yet to give it one"
                    .into(),
            ));
        };
        check_writable(&span, p.chunk_count, curve).map_err(SoundError::Refused)?;
        let key = self.sound_key(id).0;
        // Other maps' edits are gone with their map.
        self.audition.mouth.edits.retain(|s, _| s.0 == key);
        self.audition.mouth.undo.retain(|s| s.slot.0 == key);
        self.audition.mouth.redo.retain(|s| s.slot.0 == key);
        let slot = (key, id, range, permutation);
        let shown: Arc<[u8]> = self
            .audition
            .mouth
            .edits
            .get(&slot)
            .map_or_else(|| span.curve.as_slice().into(), |e| e.drawn.clone());
        let changed = shown.iter().zip(curve).filter(|(a, b)| a != b).count();
        let before = self.mouth_edited(id, range, permutation);
        let edited = curve != span.curve.as_slice();
        self.audition.mouth.note(Step {
            serial: self.tags.next_step(),
            slot: slot.clone(),
            span: span.clone(),
            before,
            after: edited.then_some(curve.into()),
        });
        // Permutations that share a descriptor share the bytes: the last
        // drawing wins for all of them.
        let at = span.offset;
        self.audition
            .mouth
            .edits
            .retain(|s, e| *s == slot || !(s.1 == id && e.span.offset == at));
        if edited {
            self.audition.mouth.edits.insert(
                slot,
                Edit {
                    span,
                    drawn: curve.into(),
                },
            );
        } else {
            self.audition.mouth.edits.remove(&slot);
        }
        self.push_sound_changes();
        Ok(MouthWrite {
            ticks: curve.len(),
            changed,
            edited,
        })
    }

    /// Give the permutation the curve of its own loudness
    /// ([`envelope`]) at `gain` and `smoothing`, as one redraw.
    pub fn sound_mouth_from_sound(
        &mut self,
        id: TagId,
        range: usize,
        permutation: usize,
        gain: f32,
        smoothing: f32,
    ) -> Result<MouthWrite, SoundError> {
        let Some(shown) = self.sound_mouth(id, range, permutation)? else {
            return Err(SoundError::Refused(
                "this permutation has no mouth curve to fill".into(),
            ));
        };
        let clip = self.sound_clip(id, range, permutation)?;
        let curve = envelope(&clip, shown.len(), gain, smoothing);
        self.sound_mouth_write(id, range, permutation, &curve)
    }

    /// Put the permutation's curve back as the file has it, as one undo
    /// step. Whether it was redrawn.
    pub fn sound_mouth_revert(
        &mut self,
        id: TagId,
        range: usize,
        permutation: usize,
    ) -> Result<bool, SoundError> {
        let slot = (self.sound_key(id).0, id, range, permutation);
        let Some(old) = self.audition.mouth.edits.remove(&slot) else {
            return Ok(false);
        };
        self.audition.mouth.note(Step {
            serial: self.tags.next_step(),
            slot,
            span: old.span,
            before: Some(old.drawn),
            after: None,
        });
        self.push_sound_changes();
        Ok(true)
    }

    /// Undo the last redraw of the shown map. Which permutation it was, or
    /// `None` when there is nothing to undo.
    pub fn sound_mouth_undo(&mut self) -> Option<(TagId, usize, usize)> {
        let key = self.sound_key(TagId(0)).0;
        let (slot, span, shown) = self
            .audition
            .mouth
            .step_back(true, |s| s.slot.0 == key)?;
        self.mouth_show(&slot, &span, shown);
        Some((slot.1, slot.2, slot.3))
    }

    /// What the save writes for the redrawn curves: each one a raw patch of
    /// its bytes in the extra-info resource, handed to the session
    /// (replacing what it held). Nothing redrawn hands an empty set.
    pub(crate) fn push_sound_changes(&mut self) {
        if !self.session.is_open() {
            return;
        }
        let key = self.sound_key(TagId(0)).0;
        let mut c = GeometryChanges::default();
        for (slot, e) in &self.audition.mouth.edits {
            if slot.0 != key {
                continue;
            }
            match check_writable(&e.span, 0, &e.drawn) {
                Ok(()) => c.raw_patches.push(RawPatch {
                    block: e.span.resource,
                    block_size: e.span.resource_size,
                    offset: e.span.offset,
                    bytes: e.drawn.to_vec(),
                }),
                Err(why) => {
                    c.refusal = Some(why);
                    break;
                }
            }
        }
        let r = self.session.set_sound_changes(c);
        self.ok_or_log(r);
    }
}
