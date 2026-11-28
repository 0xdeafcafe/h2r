//! Rows for `audio.loops` (docs/runtime/agent-api.md), read from the sound
//! table's loop pool (`Sounds::loops`, docs/engine/sound-instances.md).

use guest_layout::GuestMemory;
use serde::Serialize;
use sim_bridge::sim_sound::entries::{SHARED, lock};
use sim_bridge::sim_sound::guest_tags::Snapshot;
use sim_sound::instances::{SoundTags, Sounds, TagIndex};
use sim_sound::looping::{LoopTag, Segment as Stage};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Segment {
    In,
    Loop,
    Out,
    AltIn,
    Alt,
    AltTransitionOut,
    AltOut,
    Unknown,
}

impl Segment {
    // The table's segment indexes, `sim_sound::looping::Segment`.
    fn from_stage(stage: Stage) -> Self {
        match stage {
            Stage::IN => Self::In,
            Stage::LOOP => Self::Loop,
            Stage::OUT => Self::Out,
            Stage::ALT_TRANS_IN => Self::AltIn,
            Stage::ALT_LOOP => Self::Alt,
            Stage::ALT_TRANS_OUT => Self::AltTransitionOut,
            Stage::ALT_OUT => Self::AltOut,
            _ => Self::Unknown,
        }
    }

    /// Old and new segments can coexist. Label a sound by its tag among the
    /// track's references, and by the track's current segment only when two
    /// segments share that tag.
    fn for_sound(def: &LoopTag, track: usize, sound: TagIndex, current: Stage) -> Self {
        let Some(tag) = def.tracks.get(track) else {
            return Self::Unknown;
        };
        let mut matches = (0..Stage::COUNT as u8)
            .filter(|&i| tag.segments[usize::from(i)] == Some(sound))
            .map(Stage);
        let Some(first) = matches.next() else {
            return Self::Unknown;
        };
        let Some(second) = matches.next() else {
            return Self::from_stage(first);
        };
        if first == current || second == current || matches.any(|s| s == current) {
            Self::from_stage(current)
        } else {
            Self::Unknown
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Track {
    pub(crate) loop_datum: u32,
    pub(crate) tag_datum: u32,
    pub(crate) track: u8,
    pub(crate) segment: Segment,
    pub(crate) sound_instance: u32,
    pub(crate) sound_datum: u32,
    pub(crate) pitch_range: Option<u8>,
    pub(crate) permutation: Option<u8>,
    pub(crate) chunk: Option<u16>,
}

/// Every sound of every track of every loop, in loop slot, track and link
/// order. `definition` reads a looping sound tag's tracks.
pub(crate) fn rows(
    sounds: &Sounds,
    mut definition: impl FnMut(TagIndex) -> Option<LoopTag>,
) -> Vec<Track> {
    let mut rows = Vec::new();
    for (handle, looping) in sounds.loops().iter() {
        let def = definition(looping.tag);
        for (index, track) in looping.tracks.iter().enumerate() {
            for member in &track.members {
                let Some(instance) = sounds.get(*member) else {
                    continue;
                };
                rows.push(Track {
                    loop_datum: handle.raw(),
                    tag_datum: looping.tag.0,
                    track: index as u8,
                    segment: def.as_ref().map_or(Segment::Unknown, |def| {
                        Segment::for_sound(def, index, instance.tag, track.current)
                    }),
                    sound_instance: member.raw(),
                    sound_datum: instance.tag.0,
                    pitch_range: instance.selection.range.try_into().ok(),
                    permutation: instance.selection.permutation.try_into().ok(),
                    chunk: instance.selection.chunk.try_into().ok(),
                });
            }
        }
    }
    rows
}

/// The process's table, with each loop's tag read from the map in `m`.
pub(crate) fn read(m: &dyn GuestMemory) -> Vec<Track> {
    let table = lock(&SHARED);
    rows(&table.sounds, |tag| {
        Snapshot::of_loop(m, tag)
            .ok()
            .and_then(|s| s.looping(tag).cloned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim_sound::instances::{
        Flat, Permutation, PitchRange, Playback, Rng, Scale, SoundTag, SourceParams, TagTable,
    };
    use sim_sound::looping::TrackTag;

    const LOOP_TAG: TagIndex = TagIndex(0xe200_0001);
    const IN: TagIndex = TagIndex(0xe100_0010);
    const LOOP: TagIndex = TagIndex(0xe100_0011);

    /// An in and a loop, 100 ms and 200 ms, one range and permutation each.
    fn tags(shared: bool) -> TagTable {
        let mut t = TagTable::default();
        for (i, (tag, ms)) in [(IN, 100), (LOOP, 200)].into_iter().enumerate() {
            t.sounds.insert(
                tag,
                SoundTag {
                    playback: Playback { gain: 0.0, gain_range: 0.0, pitch: [0, 0] },
                    first_range: i as i16,
                    range_count: 1,
                    promotion: 0xff,
                    ..SoundTag::default()
                },
            );
            t.ranges.push(PitchRange {
                natural: [-1, 1],
                playable: [-1, 1],
                first_permutation: i as i16,
                count: 1,
            });
            t.permutations.push(Permutation { skip: 0, length_ms: Some(ms) });
        }
        t.scales.push(Scale { pitch: [0, 0] });
        let mut segments = [None; Stage::COUNT];
        segments[usize::from(Stage::IN.0)] = Some(IN);
        segments[usize::from(Stage::LOOP.0)] = Some(LOOP);
        if shared {
            segments[usize::from(Stage::ALT_LOOP.0)] = Some(LOOP);
        }
        let track = TrackTag { flags: 0, segments, fade_in: 0.1, crossfade: 0.2, fade_out: 0.1 };
        t.loops.insert(LOOP_TAG, LoopTag { tracks: vec![track], details: vec![] });
        t
    }

    fn started(t: &TagTable) -> Sounds {
        let mut s = Sounds::new();
        let mut rng = Rng::new(1);
        s.loop_start(&mut rng, t, &Flat(0.0), LOOP_TAG, 7, SourceParams::default(), false, 0)
            .unwrap();
        s
    }

    #[test]
    fn audio_loops_rows_come_from_the_table() {
        let t = tags(false);
        let s = started(&t);
        let rows = rows(&s, |tag| t.loops.get(&tag).cloned());
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.tag_datum, LOOP_TAG.0);
        assert_eq!(row.track, 0);
        assert_eq!(row.segment, Segment::In);
        assert_eq!(row.sound_datum, IN.0);
        let handle = s.loops().iter().next().unwrap().0;
        assert_eq!(row.loop_datum, handle.raw());
        assert_eq!(row.sound_instance, s.loops().get(handle).unwrap().tracks[0].members[0].raw());
        assert!(serde_json::to_value(row).unwrap().get("ds_stream").is_none());
    }

    #[test]
    fn a_tag_the_map_does_not_define_is_unknown() {
        let t = tags(false);
        let s = started(&t);
        assert_eq!(rows(&s, |_| None)[0].segment, Segment::Unknown);
    }

    #[test]
    fn a_shared_tag_takes_the_tracks_current_segment_or_stays_unknown() {
        let t = tags(true);
        let def = t.loops[&LOOP_TAG].clone();
        assert_eq!(Segment::for_sound(&def, 0, LOOP, Stage::ALT_LOOP), Segment::Alt);
        assert_eq!(Segment::for_sound(&def, 0, LOOP, Stage::LOOP), Segment::Loop);
        assert_eq!(Segment::for_sound(&def, 0, LOOP, Stage::OUT), Segment::Unknown);
        assert_eq!(Segment::for_sound(&def, 0, IN, Stage::OUT), Segment::In);
    }
}
