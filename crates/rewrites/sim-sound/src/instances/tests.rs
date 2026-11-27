use super::*;
use crate::chooser::History;
use crate::owners::NullOwners;

const TAG: TagIndex = TagIndex(0xe100_0004);

/// One tag: two ranges of three and one permutation, 100 ms to 900 ms.
fn tags() -> TagTable {
    let mut t = TagTable::default();
    t.sounds.insert(
        TAG,
        SoundTag {
            playback: Playback { gain: -2.0, gain_range: 4.0, pitch: [-100, 100] },
            range_count: 2,
            promotion: 0xff,
            ..SoundTag::default()
        },
    );
    t.ranges = vec![
        PitchRange { natural: [-1200, 0], playable: [-1200, 0], first_permutation: 0, count: 3 },
        PitchRange { natural: [1, 1200], playable: [1, 1200], first_permutation: 3, count: 1 },
    ];
    t.permutations = [100, 200, 300, 900].map(|ms| Permutation { skip: 0, length_ms: Some(ms) }).to_vec();
    t.scales = vec![Scale { pitch: [0, 0] }];
    t
}

fn explicit(range: u16, permutation: u16) -> Request {
    Request { selection: Some((range, permutation)), ..Request::new(TAG) }
}

/// The states `n` draws on from `seed`.
fn after(seed: u32, n: usize) -> u32 {
    let mut r = Rng::new(seed);
    for _ in 0..n {
        r.next_u32();
    }
    r.state()
}

#[test]
fn a_silent_request_takes_the_gain_draw_and_no_slot() {
    let mut s = Sounds::new();
    let mut rng = Rng::new(42);
    let quiet = Request { level: -1000.0, ..explicit(0, 0) };
    assert_eq!(s.admit(&mut rng, &tags(), quiet, 0), Err(Rejected::Silent));
    assert_eq!(rng.state(), after(42, 1));
    assert!(s.instances().is_empty());
    assert_eq!(s.counters().rejected_silent, 1);
    // A NaN level is silent too: retail's compare is unordered-low.
    let nan = Request { level: f32::NAN, ..explicit(0, 0) };
    assert_eq!(s.admit(&mut rng, &tags(), nan, 0), Err(Rejected::Silent));
}

#[test]
fn a_full_pool_stops_after_one_draw() {
    let mut s = Sounds::new();
    let mut rng = Rng::new(7);
    for _ in 0..CAPACITY {
        s.admit(&mut rng, &tags(), explicit(0, 0), 0).unwrap();
    }
    let before = rng.state();
    assert_eq!(s.admit(&mut rng, &tags(), Request::new(TAG), 0), Err(Rejected::Full));
    assert_eq!(rng.state(), after(before, 1));
    assert_eq!(s.counters().rejected_full, 1);
}

#[test]
fn an_explicit_selection_draws_no_permutation() {
    let mut s = Sounds::new();
    let mut rng = Rng::new(99);
    let h = s.admit(&mut rng, &tags(), explicit(1, 0), 5).unwrap();
    assert_eq!(rng.state(), after(99, 2), "the gain and pitch draws only");
    let i = s.get(h).unwrap();
    assert_eq!((i.selection.range, i.selection.permutation), (1, 0));
    assert_eq!(i.seed, rng.state());
    assert_eq!(s.chooser().history(1), History::default(), "an explicit selection leaves no history");
    // The chooser's path: two more draws at least (an index and a skip).
    let mut rng = Rng::new(99);
    s.admit(&mut rng, &tags(), Request::new(TAG), 5).unwrap();
    assert_eq!(rng.state(), after(99, 4));
}

#[test]
fn the_draws_are_retail_formulas() {
    let mut rng = Rng::new(0x6591_c412);
    let p = Playback { gain: -2.0, gain_range: 4.0, pitch: [-100, 100] };
    let seed = after(0x6591_c412, 1);
    let r = ((seed >> 16) as f64 * f64::from(f32::from_bits(DRAW_SCALE))) as f32;
    assert_eq!(gain_draw(&mut rng, &p).to_bits(), (4.0 * r + -2.0).to_bits());
    let seed = after(seed, 1);
    let r = (seed >> 16) as f64 * f64::from(f32::from_bits(DRAW_SCALE));
    assert_eq!(pitch_draw(&mut rng, &p), (-100.0 + 200.0 * r) as f32);
    // The top of the draw is 1 to within the scale's error.
    assert!((f64::from(0xffffu32) * f64::from(f32::from_bits(DRAW_SCALE)) - 1.0).abs() < 1e-6);
}

#[test]
fn a_pitch_picks_its_range_and_the_nearest_when_none_plays_it() {
    let t = tags();
    let tag = t.sounds[&TAG];
    assert_eq!(pitch_range(&t, &tag, -1, -5.0), Ok(0));
    assert_eq!(pitch_range(&t, &tag, -1, 5.0), Ok(1));
    assert_eq!(pitch_range(&t, &tag, -1, 5000.0), Ok(1));
    assert_eq!(pitch_range(&t, &tag, -1, -5000.0), Ok(0));
    assert_eq!(pitch_range(&t, &tag, 1, -5.0), Ok(0), "a preferred range that does not play it");
    assert_eq!(pitch_range(&t, &tag, -1, f32::NAN), Ok(-1));
}

#[test]
fn a_one_shot_ends_on_its_computed_tick_in_slot_order() {
    let mut s = Sounds::new();
    let mut rng = Rng::new(1);
    let t = tags();
    let long = s.admit(&mut rng, &t, explicit(0, 2), 10).unwrap(); // 300 ms: 9 ticks
    let short = s.admit(&mut rng, &t, explicit(0, 0), 10).unwrap(); // 100 ms: 3 ticks
    let same = s.admit(&mut rng, &t, explicit(0, 2), 10).unwrap();
    assert_eq!(s.get(long).unwrap().end, Some(19));
    assert_eq!(s.get(short).unwrap().end, Some(13));
    let first = s.step(10, &mut NullOwners);
    assert_eq!(first.iter().map(TableEvent::handle).collect::<Vec<_>>(), [long, short, same]);
    assert!(s.step(12, &mut NullOwners).is_empty());
    assert_eq!(s.step(13, &mut NullOwners), [TableEvent::Ended { handle: short, reason: Reason::PlayedOut, tick: 13 }]);
    // Due on the same tick: slot order.
    assert_eq!(
        s.step(19, &mut NullOwners),
        [
            TableEvent::Ended { handle: long, reason: Reason::PlayedOut, tick: 19 },
            TableEvent::Ended { handle: same, reason: Reason::PlayedOut, tick: 19 },
        ]
    );
    assert!(s.instances().is_empty());
    assert_eq!(s.counters().ended(Reason::PlayedOut), 3);
}

#[test]
fn a_missing_length_ends_on_the_start_tick_and_is_counted() {
    let mut t = tags();
    t.permutations[0].length_ms = None;
    let mut s = Sounds::new();
    let h = s.admit(&mut Rng::new(3), &t, explicit(0, 0), 4).unwrap();
    assert_eq!(s.get(h).unwrap().end, Some(4));
    assert_eq!(s.counters().refused_no_length, 1);
}

/// Records every call, and checks the instance is live during `ended`.
#[derive(Default)]
struct Recorder {
    calls: Vec<String>,
    gone: Option<u32>,
}

impl OwnerHost for Recorder {
    fn alive(&mut self, owner: &Owner) -> bool {
        owner.unit() != self.gone
    }
    fn mouth(&mut self, owner: &Owner, _: TagIndex, _: Selection, ticks: u32) {
        self.calls.push(format!("mouth {:?} {ticks}", owner.unit()));
    }
    fn ended(&mut self, sounds: &Sounds, owner: &Owner, handle: Handle, tag: TagIndex, reason: Reason) {
        assert!(sounds.live(handle), "ended runs with the instance still in the pool");
        self.calls.push(format!("ended {:?} {:#x} {}", owner.unit(), tag.0, reason.name()));
    }
    fn stopping(&mut self, owner: &Owner, _: Handle, _: TagIndex) {
        self.calls.push(format!("stopping {:?}", owner.unit()));
    }
}

#[test]
fn ended_runs_before_the_slot_is_free() {
    let mut s = Sounds::new();
    let mut host = Recorder::default();
    let line = Request { owner: Owner::Unit(0x55), ..explicit(0, 0) };
    let h = s.admit(&mut Rng::new(2), &tags(), line, 0).unwrap();
    s.step(1, &mut host);
    assert_eq!(host.calls, ["mouth Some(85) 1"]);
    s.step(3, &mut host);
    assert_eq!(host.calls[1], "ended Some(85) 0xe1000004 played_out");
    assert!(!s.live(h));
    // A gone owner ends its line with reason 3 on the next step.
    let h = s.admit(&mut Rng::new(2), &tags(), line, 10).unwrap();
    host.gone = Some(0x55);
    s.step(11, &mut host);
    assert_eq!(host.calls.last().unwrap(), "ended Some(85) 0xe1000004 owner_gone");
    assert!(!s.live(h));
}

#[test]
fn a_stop_fades_then_ends_with_faded() {
    let mut s = Sounds::new();
    let mut host = Recorder::default();
    let line = Request { owner: Owner::ScriptLine(Some(9)), ..explicit(1, 0) }; // 900 ms
    let h = s.admit(&mut Rng::new(4), &tags(), line, 0).unwrap();
    s.step(0, &mut host);
    assert!(s.stop(h, 2, 3, &mut host));
    assert!(!s.stop(h, 2, 3, &mut host), "a fading sound is not stopped twice");
    assert_eq!(s.get(h).unwrap().phase, Phase::Fading { since: 2, until: 5 });
    assert_eq!(s.step(2, &mut host), [TableEvent::Stopped { handle: h, fade_ticks: 3, tick: 2 }]);
    assert!(s.step(4, &mut host).is_empty());
    assert_eq!(s.step(5, &mut host), [TableEvent::Ended { handle: h, reason: Reason::Faded, tick: 5 }]);
    assert!(host.calls.contains(&"stopping Some(9)".to_string()));
    assert_eq!(host.calls.last().unwrap(), "ended Some(9) 0xe1000004 faded");
    assert!(!s.stop(h, 6, 0, &mut host), "a finished sound has a stale handle");
}

#[test]
fn handles_match_datum_new() {
    let mut s = Sounds::new();
    let mut rng = Rng::new(5);
    let t = tags();
    let a = s.admit(&mut rng, &t, explicit(0, 0), 0).unwrap();
    let b = s.admit(&mut rng, &t, explicit(1, 0), 0).unwrap();
    let c = s.admit(&mut rng, &t, explicit(0, 0), 0).unwrap();
    // `"so"` seeds the salt: 0x6f73 | 0x8000; slots in order.
    assert_eq!([a, b, c].map(Handle::raw), [0xef73_0000, 0xef74_0001, 0xef75_0002]);
    // A finished slot is reused first, with the next salt.
    s.step(3, &mut NullOwners);
    assert!(!s.live(a) && s.live(b) && !s.live(c));
    let d = s.admit(&mut rng, &t, explicit(0, 0), 3).unwrap();
    assert_eq!(d.raw(), 0xef76_0000);
}

#[test]
fn save_round_trips_instances_and_history() {
    let mut s = Sounds::new();
    let mut rng = Rng::new(11);
    let t = tags();
    s.chooser_mut().set_history(1, History { last: 0, next: -1, used: 0 });
    let a = s.admit(&mut rng, &t, Request { owner: Owner::Unit(3), ..Request::new(TAG) }, 0).unwrap();
    let b = s.admit(&mut rng, &t, explicit(1, 0), 0).unwrap();
    let looped = Request { track: Some((Handle::new(0x8123, 4), 1)), ..explicit(0, 1) };
    s.admit(&mut rng, &t, looped, 1).unwrap();
    s.stop(b, 1, 2, &mut NullOwners);
    s.step(1, &mut NullOwners);
    let _ = a;
    let bytes = s.encode();
    let back = Sounds::decode(Sounds::VERSION, &bytes).unwrap();
    assert_eq!(back.encode(), bytes);
    assert_eq!(back.section_hash(), s.section_hash());
    assert_eq!(back.chooser(), s.chooser());
    assert_eq!(back.instances().iter().map(|(h, i)| (h, *i)).collect::<Vec<_>>(), s.instances().iter().map(|(h, i)| (h, *i)).collect::<Vec<_>>());
    // The hash moves with the state.
    let mut moved = Sounds::decode(Sounds::VERSION, &bytes).unwrap();
    moved.chooser_mut().set_history(0, History { last: 2, next: -1, used: 4 });
    assert_ne!(moved.section_hash(), s.section_hash());
    assert!(matches!(Sounds::decode(2, &bytes), Err(RestoreError::SectionVersion { .. })));
    assert!(Sounds::decode(Sounds::VERSION, &bytes[..bytes.len() - 1]).is_err());
}
