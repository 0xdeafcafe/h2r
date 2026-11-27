use super::*;
use crate::owners::{NullOwners, Own, Owner, OwnerHost, Reason};

const EFFECT: u32 = 0x0080_1000;

fn sound(raw: u32) -> Handle {
    Handle::from_raw(raw)
}

fn snd(tag: u32, gain: f32, flags: u32) -> Component {
    Component {
        group: SOUND_GROUP,
        tag: TagIndex(tag),
        gain,
        flags,
    }
}

fn lsnd(tag: u32, gain: f32, flags: u32) -> Component {
    Component {
        group: LOOP_GROUP,
        tag: TagIndex(tag),
        gain,
        flags,
    }
}

fn no_flags(_: TagIndex) -> u32 {
    0
}

#[test]
fn start_rows_take_datum_new_handles_and_the_pool_holds_sixteen() {
    let mut s = Sounds::new();
    let a = s.start_new(Start::tag(EFFECT, 1.0, 0)).unwrap();
    let b = s.start_new(Start::admitting(EFFECT)).unwrap();
    // `"so"` seeds the salt: 0x6f73 | 0x8000; slots in order.
    assert_eq!([a, b].map(Handle::raw), [0xef73_0000, 0xef74_0001]);
    for _ in 2..CAPACITY {
        assert!(s.start_new(Start::tag(EFFECT, 1.0, 0)).is_some());
    }
    assert_eq!(s.start_new(Start::tag(EFFECT, 1.0, 0)), None);
    // A refused admission frees its row at once; the slot is reused first.
    assert!(s.start_free(b));
    assert!(!s.start_free(b));
    let c = s.start_new(Start::admitting(EFFECT)).unwrap();
    assert_eq!(c.index(), 1);
    assert_ne!(c, b);
}

#[test]
fn an_admitted_sound_makes_a_sound_start_with_the_requests_word() {
    let mut s = Sounds::new();
    let h = s.start_new(Start::admitting(EFFECT)).unwrap();
    s.start_release(h);
    assert!(s.start_attach(h, sound(0xef73_0004), 0x0102));
    let row = *s.starts().get(h).unwrap();
    assert_eq!(row.of, StartOf::Sound(sound(0xef73_0004)));
    assert_eq!((row.word, row.effect, row.flags()), (0x0102, EFFECT, 0x08));
    assert_eq!(row.loop_owner(h), 0xef73_0004);
    // A tag start keys its loops on its own handle.
    let t = Start::tag(EFFECT, 0.5, 0);
    assert_eq!(t.loop_owner(h), h.raw() | OWNER_BIT);
}

#[test]
fn a_stop_marks_the_row_and_the_next_walk_frees_it() {
    let mut s = Sounds::new();
    let h = s.start_new(Start::tag(EFFECT, 1.0, 0)).unwrap();
    // Running: a tag start updates every walk.
    assert_eq!(s.starts_walk(&mut |_, _| OwnerPoll::default()), vec![h]);
    assert!(s.start_stop_begin(h));
    assert_eq!(s.starts().get(h).unwrap().flags(), 0x18);
    // Between the two halves of the stop a walk does not update it.
    assert!(s.starts_walk(&mut |_, _| OwnerPoll::default()).is_empty());
    s.start_stop_end(h);
    assert_eq!(s.starts().get(h).unwrap().flags(), 0x19);
    assert!(s.starts_walk(&mut |_, _| OwnerPoll::default()).is_empty());
    assert!(s.starts().get(h).is_none());
    assert!(s.starts().is_empty());
}

#[test]
fn a_sound_starts_end_reason_sets_cut_and_the_stop_all_does_nothing() {
    let mut s = Sounds::new();
    let played = s.start_new(Start::admitting(EFFECT)).unwrap();
    let stopped = s.start_new(Start::admitting(EFFECT)).unwrap();
    let cleared = s.start_new(Start::admitting(EFFECT)).unwrap();
    for h in [played, stopped, cleared] {
        s.start_attach(h, sound(0xef73_0000), 0);
    }
    assert!(s.start_sound_ended(played, 2));
    assert!(s.start_sound_ended(stopped, 1));
    assert!(!s.start_sound_ended(cleared, 0xA));
    let flags = |h| s.starts().get(h).unwrap().flags();
    assert_eq!(
        [flags(played), flags(stopped), flags(cleared)],
        [0x18, 0x1a, 0x00]
    );
}

#[test]
fn the_walk_polls_only_a_sound_start_that_is_running() {
    let mut s = Sounds::new();
    let tag = s.start_new(Start::tag(EFFECT, 1.0, 0)).unwrap();
    let alive = s.start_new(Start::admitting(EFFECT)).unwrap();
    let gone = s.start_new(Start::admitting(EFFECT)).unwrap();
    let stopping = s.start_new(Start::admitting(EFFECT)).unwrap();
    for (h, raw) in [(alive, 1), (gone, 2), (stopping, 3)] {
        s.start_attach(h, sound(raw), 0);
    }
    s.start_stop_begin(stopping);
    let mut polled = Vec::new();
    let update = s.starts_walk(&mut |h, row| {
        polled.push(h);
        match row.of {
            StartOf::Sound(x) if x == sound(2) => OwnerPoll {
                update: false,
                gone: true,
            },
            _ => OwnerPoll {
                update: true,
                gone: false,
            },
        }
    });
    assert_eq!(polled, vec![alive, gone]);
    assert_eq!(update, vec![tag, alive, stopping]);
    assert!(s.starts().get(gone).unwrap().released);
    // Released, it updates without a poll from then on.
    let update = s.starts_walk(&mut |h, _| {
        assert_eq!(h, alive);
        OwnerPoll::default()
    });
    assert_eq!(update, vec![tag, gone, stopping]);
}

#[test]
fn the_plan_plays_each_component_in_its_mode() {
    let h = sound(0xef73_0002);
    let start = Start {
        word: 0x0003,
        ..Start::tag(EFFECT, 1.0, 0)
    };
    let components = [
        snd(0xe100_0001, -3.0, 0), // starts
        snd(0xe100_0002, -6.0, 1), // skipped at the start
        snd(0xe100_0003, -9.0, 2), // starts and stops
        lsnd(0xe200_0004, 2.0, 0), // a loop
        snd(u32::MAX, 0.0, 0),     // no tag
        Component {
            group: 0x6566_6665,
            tag: TagIndex(5),
            gain: 0.0,
            flags: 0,
        },
    ];
    let looped = |mode, flags| Action::Loop {
        tag: TagIndex(0xe200_0004),
        word: 3,
        owner: h.raw() | OWNER_BIT,
        gain: 2.0,
        mode,
        flags,
    };
    let admit = |tag, gain, owned| Action::Admit {
        tag: TagIndex(tag),
        word: 3,
        gain,
        owned,
    };
    assert_eq!(
        plan(
            h,
            &start,
            Mode::Start,
            &components,
            &no_flags,
            DialogueRow::default()
        ),
        vec![
            admit(0xe100_0001, -3.0, true),
            admit(0xe100_0003, -9.0, true),
            looped(0, 0)
        ]
    );
    assert_eq!(
        plan(
            h,
            &start,
            Mode::Update,
            &components,
            &no_flags,
            DialogueRow::default()
        ),
        vec![looped(1, 0)]
    );
    assert_eq!(
        plan(
            h,
            &start,
            Mode::Stop,
            &components,
            &no_flags,
            DialogueRow::default()
        ),
        vec![admit(0xe100_0003, -9.0, false), looped(2, 2)]
    );
}

#[test]
fn a_loop_takes_its_alternate_and_its_tags_word_mask() {
    let h = sound(0xef73_0000);
    let mut start = Start {
        word: 0x0003,
        ..Start::tag(EFFECT, 1.0, 0)
    };
    let flags_of = |p: Vec<Action>| match p[0] {
        Action::Loop { word, flags, .. } => (word, flags),
        Action::Admit { .. } => unreachable!(),
    };
    // Component flag bit 3: always the alternate.
    assert_eq!(
        flags_of(plan(
            h,
            &start,
            Mode::Start,
            &[lsnd(1, 0.0, 8)],
            &no_flags,
            DialogueRow::default()
        )),
        (3, 1)
    );
    // Bit 4: the alternate only when a cut-short stop is under way.
    let cut_end = [lsnd(1, 0.0, 0x10)];
    assert_eq!(
        flags_of(plan(
            h,
            &start,
            Mode::Stop,
            &cut_end,
            &no_flags,
            DialogueRow::default()
        )),
        (3, 2)
    );
    start.stopping = true;
    start.cut = true;
    assert_eq!(
        flags_of(plan(
            h,
            &start,
            Mode::Stop,
            &cut_end,
            &no_flags,
            DialogueRow::default()
        )),
        (3, 3)
    );
    // The loop tag's flag bits 3 and 6 clear the word's bits 0 and 1.
    assert_eq!(
        flags_of(plan(
            h,
            &start,
            Mode::Start,
            &[lsnd(1, 0.0, 0)],
            &|_| 8,
            DialogueRow::default()
        ))
        .0,
        2
    );
    assert_eq!(
        flags_of(plan(
            h,
            &start,
            Mode::Start,
            &[lsnd(1, 0.0, 0)],
            &|_| 0x48,
            DialogueRow::default()
        ))
        .0,
        0
    );
}

#[test]
fn a_dialogue_loop_takes_its_mode_from_the_row() {
    let h = sound(0xef73_0000);
    let start = Start {
        of: StartOf::Dialogue(0xe300_0007),
        ..Start::tag(EFFECT, 1.0, 0)
    };
    let row = |mode, alternate| DialogueRow { mode, alternate };
    let c = [lsnd(1, 0.0, 0x20)];
    let mode_of = |p: Vec<Action>| match p.first() {
        Some(Action::Loop {
            mode, flags, owner, ..
        }) => Some((*mode, *flags, *owner)),
        _ => None,
    };
    assert_eq!(
        mode_of(plan(h, &start, Mode::Start, &c, &no_flags, row(1, true))),
        Some((1, 1, 0xe300_0007))
    );
    assert_eq!(
        mode_of(plan(h, &start, Mode::Start, &c, &no_flags, row(-1, false))),
        Some((2, 0, 0xe300_0007))
    );
    assert_eq!(
        mode_of(plan(h, &start, Mode::Start, &c, &no_flags, row(4, false))),
        None
    );
    // Without component flag bit 5 the row is not read.
    assert_eq!(
        mode_of(plan(
            h,
            &start,
            Mode::Stop,
            &[lsnd(1, 0.0, 0)],
            &no_flags,
            row(0, true)
        )),
        Some((2, 2, 0xe300_0007))
    );
}

#[test]
fn save_round_trips_the_start_rows() {
    let mut s = Sounds::new();
    let a = s.start_new(Start::tag(EFFECT, 0.25, 7)).unwrap();
    let b = s.start_new(Start::admitting(EFFECT + 0x10)).unwrap();
    let c = s.start_new(Start::admitting(EFFECT)).unwrap();
    s.start_attach(b, sound(0xef80_0003), 9);
    s.start_sound_ended(b, 4);
    s.start_free(c);
    let bytes = s.encode();
    let back = Sounds::decode(Sounds::VERSION, &bytes).unwrap();
    assert_eq!(back.encode(), bytes);
    assert_eq!(back.section_hash(), s.section_hash());
    let rows = |x: &Sounds| x.starts().iter().map(|(h, r)| (h, *r)).collect::<Vec<_>>();
    assert_eq!(rows(&back), rows(&s));
    assert_eq!(
        rows(&back).iter().map(|(h, _)| *h).collect::<Vec<_>>(),
        vec![a, b]
    );
    // The hash moves with a row.
    let mut moved = Sounds::decode(Sounds::VERSION, &bytes).unwrap();
    moved.start_stop_end(a);
    assert_ne!(moved.section_hash(), s.section_hash());
    assert!(Sounds::decode(Sounds::VERSION, &bytes[..bytes.len() - 1]).is_err());
}

const TAG: TagIndex = TagIndex(0xe100_0001);

fn tags() -> crate::instances::TagTable {
    use crate::instances::{Permutation, PitchRange, SoundTag, TagTable};
    let mut t = TagTable::default();
    t.sounds.insert(
        TAG,
        SoundTag {
            range_count: 1,
            promotion: 0xff,
            ..SoundTag::default()
        },
    );
    t.ranges.push(PitchRange {
        natural: [0, 0],
        playable: [0, 0],
        first_permutation: 0,
        count: 1,
    });
    t.permutations.push(Permutation {
        skip: 0,
        length_ms: Some(100),
    });
    t
}

fn admit(s: &mut Sounds, owner: Owner, source: u32) -> Handle {
    use crate::instances::{Request, Rng, SourceParams};
    let mut words = [0; 17];
    words[3] = source;
    let request = Request {
        owner,
        source: SourceParams(words),
        selection: Some((0, 0)),
        level: 10.0,
        ..Request::new(TAG)
    };
    s.admit(&mut Rng::new(1), &tags(), request, 0).unwrap()
}

/// The owners the host heard, in order.
#[derive(Default)]
struct Heard(Vec<(&'static str, Owner, Option<Reason>)>);

impl OwnerHost for Heard {
    fn alive(&mut self, _: &Owner) -> bool {
        true
    }
    fn mouth(&mut self, owner: &Owner, _: TagIndex, _: crate::impulse::Selection, _: u32) {
        self.0.push(("mouth", *owner, None));
    }
    fn ended(&mut self, _: &Sounds, owner: &Owner, _: Handle, _: TagIndex, reason: Reason) {
        self.0.push(("ended", *owner, Some(reason)));
    }
    fn stopping(&mut self, owner: &Owner, _: Handle, _: TagIndex) {
        self.0.push(("stopping", *owner, None));
    }
}

/// A sound start with its sound admitted under `0x44a1c0`.
fn sound_start(s: &mut Sounds, own: Own) -> (Handle, Handle) {
    let start = s.start_new(Start::admitting(EFFECT)).unwrap();
    let h = admit(s, Owner::Start { start, own }, 0);
    s.start_attach(start, h, 0);
    (start, h)
}

#[test]
fn a_start_sounds_end_tells_its_own_owner_and_queues_the_stop() {
    let mut s = Sounds::new();
    let (start, _) = sound_start(&mut s, Own::Unit(7));
    let mut heard = Heard::default();
    // The mouth goes to the owner it kept (`0x21da50`).
    s.step(1, &mut heard);
    assert_eq!(heard.0, vec![("mouth", Owner::Unit(7), None)]);
    assert!(s.take_start_stops().is_empty());
    // Played out (100 ms is 3 ticks): its owner hears it, the start stops uncut.
    heard.0.clear();
    s.step(3, &mut heard);
    assert_eq!(heard.0, vec![("ended", Owner::Unit(7), Some(Reason::PlayedOut))]);
    assert_eq!(s.take_start_stops(), vec![start]);
    assert_eq!(s.starts().get(start).unwrap().flags(), 0x18);
    assert!(s.take_start_stops().is_empty(), "taken once");
}

#[test]
fn the_stop_all_tells_a_start_sounds_owners_nothing() {
    let mut s = Sounds::new();
    let (start, _) = sound_start(&mut s, Own::ScriptLine(None));
    let mut heard = Heard::default();
    s.finish_all(Reason::Cleared, 1, &mut heard);
    assert!(heard.0.is_empty());
    assert!(s.take_start_stops().is_empty());
    assert_eq!(s.starts().get(start).unwrap().flags(), 0);
}

#[test]
fn a_start_sounds_stop_cuts_the_start_and_gives_the_sound_back() {
    let mut s = Sounds::new();
    let (start, h) = sound_start(&mut s, Own::Unit(7));
    let mut heard = Heard::default();
    assert!(s.stop(h, 1, 9, &mut heard));
    // `0x21db30`: reason 1 to the kept owner, never its `stopping`.
    assert_eq!(heard.0, vec![("ended", Owner::Unit(7), Some(Reason::Stopped))]);
    assert_eq!(s.take_start_stops(), vec![start]);
    assert_eq!(s.starts().get(start).unwrap().flags(), 0x1a);
    assert_eq!(s.get(h).unwrap().owner, Owner::Unit(7));
    // Its fade's end is the kept owner's alone.
    heard.0.clear();
    s.step(10, &mut heard);
    assert_eq!(heard.0, vec![("ended", Owner::Unit(7), Some(Reason::Faded))]);
    assert!(s.take_start_stops().is_empty());
}

#[test]
fn a_start_sound_lives_while_its_start_is_not_stopping() {
    let mut s = Sounds::new();
    let (start, h) = sound_start(&mut s, Own::None);
    s.start_stop_begin(start);
    let events = s.step(1, &mut NullOwners);
    assert!(events.iter().any(|e| matches!(e, crate::events::TableEvent::Ended { handle, reason: Reason::OwnerGone, .. } if *handle == h)));
    // Its end begins the stop again (`0x21da90`), cut.
    assert_eq!(s.take_start_stops(), vec![start]);
    assert!(s.starts().get(start).unwrap().cut);
}

#[test]
fn a_started_sound_follows_its_start_until_the_start_is_done() {
    let mut s = Sounds::new();
    let (start, parent) = sound_start(&mut s, Own::None);
    let child = admit(&mut s, Owner::Started(start), 0);
    // The parent moves; the child takes its source at the step (`0x21d970`).
    s.instances.get_mut(parent).unwrap().source.0[3] = 42;
    s.step(1, &mut NullOwners);
    assert_eq!(s.get(child).unwrap().source.0[3], 42);
    // Stopping, the start still holds it, but no longer moves it.
    s.start_stop_begin(start);
    s.instances.get_mut(parent).unwrap().source.0[3] = 43;
    let tag_start = s.start_new(Start::tag(EFFECT, 1.0, 0)).unwrap();
    let orphan = admit(&mut s, Owner::Started(tag_start), 5);
    s.step(1, &mut NullOwners);
    assert_eq!(s.get(child).unwrap().source.0[3], 42);
    assert!(s.get(child).is_some());
    // Done, it ends with reason 3; a tag start's sound keeps its own source.
    s.start_stop_end(start);
    s.step(2, &mut NullOwners);
    assert!(s.get(child).is_none());
    assert_eq!(s.get(orphan).unwrap().source.0[3], 5);
    assert_eq!(s.counters().ended(Reason::OwnerGone), 2, "the parent and the child");
}
