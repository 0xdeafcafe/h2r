use super::*;
use crate::events::TableEvent;
use crate::instances::{Flat, Permutation, PitchRange, Playback, Scale, SoundTag, TagTable};
use crate::owners::{NullOwners, Reason};

const LOOP_TAG: TagIndex = TagIndex(0xe200_0001);
const IN: TagIndex = TagIndex(0xe100_0010);
const LOOP: TagIndex = TagIndex(0xe100_0011);
const OUT: TagIndex = TagIndex(0xe100_0012);
const ALT_IN: TagIndex = TagIndex(0xe100_0013);
const ALT_LOOP: TagIndex = TagIndex(0xe100_0014);
const DETAIL: TagIndex = TagIndex(0xe100_0015);

/// Each sound one range of one permutation: in, out and detail 100 ms
/// (3 ticks), the loops 200 ms (6 ticks).
fn tags(flags: u32, tracks: usize, details: Vec<DetailTag>) -> TagTable {
    let mut t = TagTable::default();
    for (i, (tag, ms)) in [(IN, 100), (LOOP, 200), (OUT, 100), (ALT_IN, 100), (ALT_LOOP, 200), (DETAIL, 100)]
        .into_iter()
        .enumerate()
    {
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
        t.ranges.push(PitchRange { natural: [-1, 1], playable: [-1, 1], first_permutation: i as i16, count: 1 });
        t.permutations.push(Permutation { skip: 0, length_ms: Some(ms) });
    }
    t.scales.push(Scale { pitch: [0, 0] });
    let mut segments = [None; Segment::COUNT];
    segments[Segment::IN.0 as usize] = Some(IN);
    segments[Segment::LOOP.0 as usize] = Some(LOOP);
    segments[Segment::OUT.0 as usize] = Some(OUT);
    segments[Segment::ALT_TRANS_IN.0 as usize] = Some(ALT_IN);
    segments[Segment::ALT_LOOP.0 as usize] = Some(ALT_LOOP);
    let track = TrackTag { flags, segments, fade_in: 0.1, crossfade: 0.2, fade_out: 0.1 };
    t.loops.insert(LOOP_TAG, LoopTag { tracks: vec![track; tracks], details });
    t
}

fn tags_of(s: &Sounds, h: Handle) -> Vec<TagIndex> {
    let l = s.loops().get(h).unwrap();
    l.tracks[0].members.iter().map(|m| s.get(*m).unwrap().tag).collect()
}

fn primary_tag(s: &Sounds, h: Handle, track: usize) -> Option<TagIndex> {
    let p = s.loops().get(h).unwrap().tracks[track].primary?;
    Some(s.get(p).unwrap().tag)
}

/// One sound phase: loops first, then the table.
fn tick(s: &mut Sounds, rng: &mut Rng, t: &TagTable, tick: u32) -> Vec<TableEvent> {
    s.step_loops(rng, t, &Flat(0.0), tick);
    s.step(tick, &mut NullOwners)
}

#[test]
fn a_loop_plays_in_then_loop_then_out() {
    let t = tags(0, 1, vec![]);
    let mut s = Sounds::new();
    let mut rng = Rng::new(1);
    let h = s.loop_start(&mut rng, &t, &Flat(0.0), LOOP_TAG, 7, SourceParams::default(), false, 0).unwrap();
    assert_eq!(primary_tag(&s, h, 0), Some(IN));
    assert_eq!(s.loops().get(h).unwrap().tracks[0].current, Segment::IN);
    tick(&mut s, &mut rng, &t, 2);
    assert_eq!(primary_tag(&s, h, 0), Some(IN));
    // The in's end tick moves the track to the loop, and the in ends.
    tick(&mut s, &mut rng, &t, 3);
    assert_eq!(primary_tag(&s, h, 0), Some(LOOP));
    assert_eq!(tags_of(&s, h), [LOOP]);
    // The loop repeats on its own end tick.
    let first = s.loops().get(h).unwrap().tracks[0].primary;
    tick(&mut s, &mut rng, &t, 9);
    let again = s.loops().get(h).unwrap().tracks[0].primary;
    assert_ne!(first, again);
    assert_eq!(primary_tag(&s, h, 0), Some(LOOP));
    // The stop plays the out and fades the loop; the loop is freed after.
    assert!(s.loop_stop(&mut rng, &t, &Flat(0.0), h, 10, &mut NullOwners));
    assert_eq!(primary_tag(&s, h, 0), Some(OUT));
    assert_eq!(s.loops().get(h).unwrap().tracks[0].current, Segment::OUT);
    for n in 10..=13 {
        tick(&mut s, &mut rng, &t, n);
    }
    tick(&mut s, &mut rng, &t, 14);
    assert!(s.loops().get(h).is_none());
    assert!(s.instances().is_empty());
}

#[test]
fn an_overlay_in_plays_on_top_of_the_loop() {
    let t = tags(1, 1, vec![]);
    let mut s = Sounds::new();
    let h = s.loop_start(&mut Rng::new(2), &t, &Flat(0.0), LOOP_TAG, 7, SourceParams::default(), false, 0).unwrap();
    assert_eq!(tags_of(&s, h), [IN, LOOP]);
    assert_eq!(primary_tag(&s, h, 0), Some(LOOP));
}

#[test]
fn the_alternate_swaps_segments_with_overlap() {
    // Flag bit 2: the alternate transitions are overlays and the swap crossfades.
    let t = tags(4, 1, vec![]);
    let mut s = Sounds::new();
    let mut rng = Rng::new(3);
    let h = s.loop_start(&mut rng, &t, &Flat(0.0), LOOP_TAG, 7, SourceParams::default(), false, 0).unwrap();
    tick(&mut s, &mut rng, &t, 3);
    let old = s.loops().get(h).unwrap().tracks[0].primary.unwrap();
    assert!(s.loop_set_alternate(&mut rng, &t, &Flat(0.0), h, true, 4, &mut NullOwners));
    // The transition plays on top of the alternate loop, while the old loop
    // fades over the crossfade (0.2 s: 6 ticks).
    assert_eq!(tags_of(&s, h), [LOOP, ALT_IN, ALT_LOOP]);
    assert_eq!(primary_tag(&s, h, 0), Some(ALT_LOOP));
    assert_eq!(s.loops().get(h).unwrap().tracks[0].current, Segment::ALT_LOOP);
    assert_eq!(s.get(old).unwrap().phase, Phase::Fading { since: 4, until: 10 });
    // Asking again for the sequence it plays does nothing.
    let before = s.instances().len();
    assert!(s.loop_set_alternate(&mut rng, &t, &Flat(0.0), h, true, 5, &mut NullOwners));
    assert_eq!(s.instances().len(), before);
}

#[test]
fn a_detail_sound_starts_on_its_drawn_deadline() {
    let detail = DetailTag { tag: DETAIL, interval: [1.0, 1.0], flags: 0 };
    let t = tags(0, 1, vec![detail]);
    let mut s = Sounds::new();
    let mut rng = Rng::new(4);
    let h = s.loop_start(&mut rng, &t, &Flat(0.0), LOOP_TAG, 7, SourceParams::default(), false, 0).unwrap();
    // One second from the clock at the start.
    assert_eq!(s.loops().get(h).unwrap().details, [1000]);
    let playing = |s: &Sounds| s.instances().iter().filter(|(_, i)| i.tag == DETAIL).count();
    tick(&mut s, &mut rng, &t, 30);
    assert_eq!(playing(&s), 0, "the clock at tick 30 is 1000 ms: not after the deadline");
    tick(&mut s, &mut rng, &t, 31);
    assert_eq!(playing(&s), 1);
    assert_eq!(s.loops().get(h).unwrap().details, [clock_ms(31) + 1000]);
    // A detail gated to the alternate does not play outside it.
    let gated = tags(0, 1, vec![DetailTag { flags: 2, ..detail }]);
    let mut s = Sounds::new();
    s.loop_start(&mut rng, &gated, &Flat(0.0), LOOP_TAG, 7, SourceParams::default(), false, 0).unwrap();
    tick(&mut s, &mut rng, &gated, 40);
    assert_eq!(playing(&s), 0);
}

#[test]
fn the_deadline_draw_is_retail_formula() {
    let mut rng = Rng::new(0x1234_5678);
    let mut probe = Rng::new(0x1234_5678);
    let r = f64::from(probe.next_u32() >> 16) * f64::from(f32::from_bits(crate::instances::DRAW_SCALE));
    let expected = ((0.5 + (2.0 - 0.5) * r) * 1000.0 + 250.0) as i32;
    assert_eq!(detail_deadline(&mut rng, [0.5, 2.0], 250), expected);
    assert_eq!(rng.state(), probe.state());
}

#[test]
fn a_stopped_loop_fades_every_track_instance() {
    let mut t = tags(1, 2, vec![]);
    for track in &mut t.loops.get_mut(&LOOP_TAG).unwrap().tracks {
        track.segments[Segment::OUT.0 as usize] = None;
    }
    let mut s = Sounds::new();
    let mut rng = Rng::new(5);
    let h = s.loop_start(&mut rng, &t, &Flat(0.0), LOOP_TAG, 7, SourceParams::default(), false, 0).unwrap();
    let members: Vec<Handle> = s.loops().get(h).unwrap().tracks.iter().flat_map(|t| t.members.clone()).collect();
    assert_eq!(members.len(), 4, "an in and a loop on each of two tracks");
    assert!(s.loop_stop(&mut rng, &t, &Flat(0.0), h, 1, &mut NullOwners));
    assert!(!s.loop_stop(&mut rng, &t, &Flat(0.0), h, 1, &mut NullOwners));
    for m in &members {
        assert_eq!(s.get(*m).unwrap().phase, Phase::Fading { since: 1, until: 4 });
    }
    // The ins play out on tick 3, before their fades end; the loops fade out.
    let ended: Vec<Reason> = (1..=4)
        .flat_map(|n| tick(&mut s, &mut rng, &t, n))
        .filter_map(|e| match e {
            TableEvent::Ended { reason, .. } => Some(reason),
            _ => None,
        })
        .collect();
    assert_eq!(ended, [Reason::PlayedOut, Reason::PlayedOut, Reason::Faded, Reason::Faded]);
    tick(&mut s, &mut rng, &t, 5);
    assert!(s.loops().is_empty());
}

#[test]
fn loops_step_in_slot_order() {
    let t = tags(0, 1, vec![]);
    let mut s = Sounds::new();
    let mut rng = Rng::new(6);
    let a = s.loop_start(&mut rng, &t, &Flat(0.0), LOOP_TAG, 1, SourceParams::default(), false, 0).unwrap();
    let b = s.loop_start(&mut rng, &t, &Flat(0.0), LOOP_TAG, 2, SourceParams::default(), false, 0).unwrap();
    assert_eq!([a.index(), b.index()], [0, 1]);
    s.step(0, &mut NullOwners);
    // Both ins end on tick 3: loop 0's loop is admitted before loop 1's.
    let events = tick(&mut s, &mut rng, &t, 3);
    let starts: Vec<Handle> = events
        .iter()
        .filter_map(|e| match e {
            TableEvent::Started { handle, .. } => Some(*handle),
            _ => None,
        })
        .collect();
    assert_eq!(starts, [s.loops().get(a).unwrap().tracks[0].primary.unwrap(), s.loops().get(b).unwrap().tracks[0].primary.unwrap()]);
    // A freed row is taken first by the next loop, and steps first.
    s.loop_stop(&mut rng, &t, &Flat(0.0), a, 4, &mut NullOwners);
    for n in 4..12 {
        tick(&mut s, &mut rng, &t, n);
    }
    assert!(s.loops().get(a).is_none());
    let c = s.loop_start(&mut rng, &t, &Flat(0.0), LOOP_TAG, 3, SourceParams::default(), false, 12).unwrap();
    assert_eq!(c.index(), 0);
    assert_eq!(s.loops().handles(), [c, b]);
}

#[test]
fn a_track_drops_an_instance_when_it_ends() {
    let t = tags(1, 1, vec![]);
    let mut s = Sounds::new();
    let mut rng = Rng::new(7);
    let h = s.loop_start(&mut rng, &t, &Flat(0.0), LOOP_TAG, 7, SourceParams::default(), false, 0).unwrap();
    let intro = s.loops().get(h).unwrap().tracks[0].members[0];
    assert_eq!(s.get(intro).unwrap().tag, IN);
    assert_eq!(s.get(intro).unwrap().owner, Owner::Loop { looping: h, track: 0 });
    tick(&mut s, &mut rng, &t, 3);
    assert!(!s.live(intro));
    assert_eq!(tags_of(&s, h), [LOOP]);
}

#[test]
fn the_tables_are_retails() {
    // In moves to the loop, which repeats; the alternate goes through its
    // transitions and back.
    assert_eq!(Segment::IN.next(false), Segment::LOOP);
    assert_eq!(Segment::LOOP.next(false), Segment::LOOP);
    assert_eq!(Segment::LOOP.next(true), Segment::ALT_TRANS_IN);
    assert_eq!(Segment::ALT_TRANS_IN.next(true), Segment::ALT_LOOP);
    assert_eq!(Segment::ALT_LOOP.next(false), Segment::ALT_TRANS_OUT);
    assert_eq!(Segment::ALT_TRANS_OUT.next(false), Segment::LOOP);
    assert!(Segment::ALT_LOOP.in_alternate() && !Segment::ALT_TRANS_OUT.in_alternate());
    assert_eq!([seconds_to_ticks(0.1), seconds_to_ticks(1.0), seconds_to_ticks(-1.0)], [3, 30, 0]);
}

#[test]
fn save_round_trips_loops() {
    let t = tags(1, 2, vec![DetailTag { tag: DETAIL, interval: [0.5, 3.0], flags: 0 }]);
    let mut s = Sounds::new();
    let mut rng = Rng::new(8);
    let h = s.loop_start(&mut rng, &t, &Flat(0.0), LOOP_TAG, 7, SourceParams([3; 17]), true, 0).unwrap();
    s.loop_stop(&mut rng, &t, &Flat(0.0), h, 1, &mut NullOwners);
    let bytes = s.encode();
    let back = Sounds::decode(Sounds::VERSION, &bytes).unwrap();
    assert_eq!(back.encode(), bytes);
    assert_eq!(back.loops().get(h), s.loops().get(h));
    assert_eq!(s.start_unknown(&mut rng, &t), Err(LoopError::UnknownTag(TagIndex(9))));
}

impl Sounds {
    fn start_unknown(&mut self, rng: &mut Rng, t: &TagTable) -> Result<Handle, LoopError> {
        self.loop_start(rng, t, &Flat(0.0), TagIndex(9), 0, SourceParams::default(), false, 0)
    }
}

#[test]
fn a_loop_is_found_by_tag_and_owner_a_playing_one_first() {
    let t = tags(0, 1, vec![]);
    let mut s = Sounds::new();
    let mut rng = Rng::new(5);
    let a = s.loop_start(&mut rng, &t, &Flat(0.0), LOOP_TAG, 7, SourceParams::default(), false, 0).unwrap();
    assert_eq!(s.loop_find(LOOP_TAG, 7), Some(a));
    assert_eq!(s.loop_find(LOOP_TAG, 8), None);
    assert_eq!(s.loop_find(IN, 7), None);
    assert!(s.loop_stop(&mut rng, &t, &Flat(0.0), a, 1, &mut NullOwners));
    assert_eq!(s.loop_find(LOOP_TAG, 7), Some(a), "a stopped loop is still found");
    let b = s.loop_start(&mut rng, &t, &Flat(0.0), LOOP_TAG, 7, SourceParams::default(), false, 1).unwrap();
    assert_eq!(s.loop_find(LOOP_TAG, 7), Some(b), "the playing one before the stopped one");
}

#[test]
fn a_new_source_is_where_the_next_segment_plays() {
    let t = tags(0, 1, vec![]);
    let mut s = Sounds::new();
    let mut rng = Rng::new(6);
    let h = s.loop_start(&mut rng, &t, &Flat(0.0), LOOP_TAG, 7, SourceParams::default(), false, 0).unwrap();
    let mut moved = SourceParams::default();
    moved.0[5] = 0x4120_0000;
    assert!(s.loop_set_source(h, moved));
    tick(&mut s, &mut rng, &t, 3);
    let p = s.loops().get(h).unwrap().tracks[0].primary.unwrap();
    assert_eq!(s.get(p).unwrap().source, moved);
}

#[test]
fn a_faded_loop_plays_no_out_and_is_freed_once_empty() {
    let t = tags(0, 1, vec![]);
    let mut s = Sounds::new();
    let mut rng = Rng::new(7);
    let h = s.loop_start(&mut rng, &t, &Flat(0.0), LOOP_TAG, 7, SourceParams::default(), false, 0).unwrap();
    assert!(!s.loop_free_silent(h), "the in plays");
    let intro = s.loops().get(h).unwrap().tracks[0].primary.unwrap();
    assert!(s.loop_fade_out(h, 1, 3, &mut NullOwners));
    assert!(!s.loop_fade_out(h, 1, 3, &mut NullOwners), "once");
    assert_eq!(tags_of(&s, h), [IN], "no out");
    assert_eq!(s.get(intro).unwrap().phase, Phase::Fading { since: 1, until: 4 });
    for n in 1..=5 {
        tick(&mut s, &mut rng, &t, n);
    }
    assert!(s.loops().get(h).is_none());
    assert!(s.instances().is_empty());
}

#[test]
fn a_loop_with_no_sound_is_freed_at_once() {
    let mut t = tags(0, 1, vec![]);
    t.loops.get_mut(&LOOP_TAG).unwrap().tracks[0].segments = [None; Segment::COUNT];
    let mut s = Sounds::new();
    let h = s.loop_start(&mut Rng::new(8), &t, &Flat(0.0), LOOP_TAG, 7, SourceParams::default(), false, 0).unwrap();
    assert!(s.loop_free_silent(h));
    assert!(s.loops().get(h).is_none());
}

/// What one event did to which sound: the order the host hears.
fn order(events: &[TableEvent]) -> Vec<(&'static str, Handle)> {
    events
        .iter()
        .map(|e| match e {
            TableEvent::Started { handle, .. } => ("start", *handle),
            TableEvent::Stopped { handle, .. } => ("stop", *handle),
            TableEvent::Ended { handle, .. } => ("end", *handle),
        })
        .collect()
}

#[test]
fn a_tick_runs_its_calls_then_the_loops_update_then_the_ends_as_the_guest_frame_does() {
    // The guest's order: the tick's calls (starts, stops) before the frame,
    // then in the frame `0x21b0e0` (a loop's next segment) before
    // `0x1290d0` (the instances' ends, in slot order).
    let t = tags(0, 1, vec![]);
    let mut s = Sounds::new();
    let mut rng = Rng::new(1);
    let h = s.loop_start(&mut rng, &t, &Flat(0.0), LOOP_TAG, 7, SourceParams::default(), false, 0).unwrap();
    let intro = s.loops().get(h).unwrap().tracks[0].primary.unwrap();
    let early = s.admit(&mut rng, &t, Request::new(OUT), 0).unwrap();
    let long = s.admit(&mut rng, &t, Request::new(LOOP), 0).unwrap();
    let first = tick(&mut s, &mut rng, &t, 0);
    assert_eq!(order(&first), [("start", intro), ("start", early), ("start", long)]);
    match first[0] {
        TableEvent::Started { track, follows, .. } => assert_eq!((track, follows), (Some((h, 0)), None)),
        _ => unreachable!(),
    }
    let shot = s.admit(&mut rng, &t, Request::new(DETAIL), 3).unwrap();
    assert!(s.stop(long, 3, 3, &mut NullOwners));
    let events = tick(&mut s, &mut rng, &t, 3);
    let next = s.loops().get(h).unwrap().tracks[0].primary.unwrap();
    assert_eq!(
        order(&events),
        [("start", shot), ("stop", long), ("start", next), ("end", intro), ("end", early)],
    );
    match events[2] {
        TableEvent::Started { track, follows, .. } => assert_eq!((track, follows), (Some((h, 0)), Some(intro)), "the loop continues the in"),
        _ => unreachable!(),
    }
    match events[0] {
        TableEvent::Started { track, follows, .. } => assert_eq!((track, follows), (None, None)),
        _ => unreachable!(),
    }
}

#[test]
fn a_track_sound_plays_at_its_loops_source_as_the_owner_moves_it() {
    let t = tags(0, 1, vec![]);
    let mut s = Sounds::new();
    let mut rng = Rng::new(1);
    let h = s.loop_start(&mut rng, &t, &Flat(0.0), LOOP_TAG, 7, SourceParams::default(), false, 0).unwrap();
    let shot = s.admit(&mut rng, &t, Request::new(OUT), 0).unwrap();
    let mut moved = SourceParams::default();
    moved.0[3] = 5.0f32.to_bits();
    assert!(s.loop_set_source(h, moved));
    let intro = s.loops().get(h).unwrap().tracks[0].primary.unwrap();
    assert_eq!(s.sources(), [(intro, moved), (shot, SourceParams::default())]);
}

#[test]
fn a_loop_its_owner_left_is_freed_and_its_sounds_play_out() {
    let t = tags(0, 1, vec![]);
    let mut s = Sounds::new();
    let mut rng = Rng::new(1);
    let h = s.loop_start(&mut rng, &t, &Flat(0.0), LOOP_TAG, 7, SourceParams::default(), false, 0).unwrap();
    let intro = s.loops().get(h).unwrap().tracks[0].primary.unwrap();
    // Started this tick: reached.
    assert!(s.free_unrefreshed_loops().is_empty());
    tick(&mut s, &mut rng, &t, 1);
    s.loop_touch(h, false);
    assert!(s.free_unrefreshed_loops().is_empty(), "reached again");
    // The next tick nothing reaches it: freed, and the in plays to its end.
    assert_eq!(s.free_unrefreshed_loops(), [h]);
    assert!(s.loops().get(h).is_none());
    assert!(s.live(intro));
    let events = tick(&mut s, &mut rng, &t, 3);
    assert_eq!(order(&events), [("end", intro)], "no next segment");
    assert!(s.instances().is_empty());
}

#[test]
fn a_kept_loop_lives_while_a_sound_of_it_plays() {
    let t = tags(0, 1, vec![]);
    let mut s = Sounds::new();
    let mut rng = Rng::new(1);
    let h = s.loop_start(&mut rng, &t, &Flat(0.0), LOOP_TAG, 7, SourceParams::default(), false, 0).unwrap();
    s.loop_touch(h, true);
    assert!(s.free_unrefreshed_loops().is_empty());
    assert!(s.free_unrefreshed_loops().is_empty(), "flag bit 1 and the in plays");
    assert!(s.loops().get(h).is_some());
    // A stopping loop is left to its outs and its own free.
    assert!(s.loop_stop(&mut rng, &t, &Flat(0.0), h, 1, &mut NullOwners));
    s.loops.get_mut(h).unwrap().keep = false;
    assert!(s.free_unrefreshed_loops().is_empty());
}
