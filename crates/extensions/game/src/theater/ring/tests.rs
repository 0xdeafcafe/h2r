use super::fakes::Fake;
use super::*;
use crate::theater::start_point_state::fakes::{ARENA, guest};

fn fresh(cap: usize) -> (Ring, Fake) {
    let fake = Fake::default();
    (Ring::new(cap, Box::new(fake.clone())), fake)
}

fn ticks(ring: &Ring) -> Vec<u64> {
    ring.keyframes().ring
}

fn jump(target: u64) -> Jump {
    Jump { target, from: 5000, requested: None }
}

fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
    move |key| pairs.iter().find(|(name, _)| *name == key).map(|(_, value)| (*value).to_owned())
}

#[test]
fn an_entry_is_taken_once_for_each_stretch_of_150_ticks_the_player_passes() {
    let g = guest(1);
    let (mut ring, _) = fresh(usize::MAX);
    for tick in 1..=449 {
        ring.passed(&g, tick, tick);
    }
    // None before the first stretch: the film's start is a map load away.
    assert_eq!(ticks(&ring), [150, 300]);
    // Passed again after a jump back: each stretch keeps its one entry.
    for tick in 151..=450 {
        ring.passed(&g, tick, tick);
    }
    assert_eq!(ticks(&ring), [150, 300, 450]);

    // A game tick that applies its update twice skips a film tick: the
    // stretch's entry is at the first tick a game tick ends on in it.
    let (mut skipping, _) = fresh(usize::MAX);
    for tick in [148, 149, 151, 152] {
        skipping.passed(&g, tick, tick);
    }
    assert_eq!(ticks(&skipping), [151]);

    // A jump loads the latest entry at or before its target.
    assert_eq!(ring.nearest(149), None);
    assert_eq!(ring.nearest(150), Some(150));
    assert_eq!(ring.nearest(449), Some(300));
    assert_eq!(ring.nearest(10_000), Some(450));
}

#[test]
fn past_its_cap_the_ring_drops_every_second_entry_of_its_older_half() {
    let g = guest(1);
    let (mut one, _) = fresh(usize::MAX);
    one.passed(&g, 150, 150);
    let entry = one.keyframes().ring_bytes as usize;
    assert!(
        entry > 0 && entry < arena::arena_len() / 4,
        "an entry is stored compressed: {entry} bytes"
    );

    let (mut ring, _) = fresh(4 * entry);
    for tick in (150..=600).step_by(150) {
        ring.passed(&g, tick, tick);
    }
    assert_eq!(ticks(&ring), [150, 300, 450, 600]);
    ring.passed(&g, 750, 750);
    // Five entries past a cap of four: the older half is 150 and 300, and
    // its second goes.
    assert_eq!(ticks(&ring), [150, 450, 600, 750]);
    for tick in (900..=3000).step_by(150) {
        ring.passed(&g, tick, tick);
        let k = ring.keyframes();
        assert!(k.ring_bytes <= k.ring_cap, "{k:?}");
        assert_eq!(k.ring.len(), 4, "{k:?}");
        assert_eq!(k.ring.last(), Some(&tick), "the newest entry stays");
    }
    assert_eq!(ticks(&ring)[0], 150, "the oldest entry stays");
}

#[test]
fn a_load_puts_back_the_arena_records_and_monitors_of_the_latest_entry_at_or_before_the_target() {
    let (mut ring, fake) = fresh(usize::MAX);
    let mut g = guest(1);
    fake.ext.0.lock().unwrap().bytes = vec![1, 2, 3];
    ring.passed(&g, 150, 140);
    let at_150 = arena::current_hash(&g, &fake.ext).unwrap();
    // The game goes on: another arena and other records at 300, and again
    // after it.
    g.write(ARENA, &[0xee; 64]).unwrap();
    fake.ext.0.lock().unwrap().bytes = vec![7];
    ring.passed(&g, 300, 290);
    let at_300 = arena::current_hash(&g, &fake.ext).unwrap();
    g.write(ARENA, &[0x55; 64]).unwrap();
    fake.ext.0.lock().unwrap().bytes = vec![9, 9];

    assert_eq!(ring.load(jump(299), &mut g).unwrap(), Loaded { tick: 150, game_ticks: 140 });
    assert_eq!(arena::current_hash(&g, &fake.ext).unwrap(), at_150);
    assert_eq!(fake.ext.0.lock().unwrap().bytes, [1, 2, 3]);
    assert_eq!(fake.puts(), 1, "the monitors go back with the entry");
    assert!(ring.restore_ms().is_some());

    assert_eq!(ring.load(jump(300), &mut g).unwrap(), Loaded { tick: 300, game_ticks: 290 });
    assert_eq!(arena::current_hash(&g, &fake.ext).unwrap(), at_300);

    // No entry at or before the target: nothing is written.
    assert!(ring.load(jump(149), &mut g).is_err());
    assert_eq!(arena::current_hash(&g, &fake.ext).unwrap(), at_300);
}

#[test]
fn an_entry_that_hashes_wrong_is_refused_and_a_divergence_answers_for_a_load_up_to_the_target_or_the_next_stretch() {
    let (mut ring, _) = fresh(usize::MAX);
    let mut g = guest(1);
    ring.passed(&g, 150, 150);
    ring.passed(&g, 300, 300);
    ring.spoil(300);
    let why = ring.load(jump(320), &mut g).unwrap_err();
    assert!(why.contains("differs"), "{why}");

    // A jump to 200 loads 150: a divergence up to 300, the next stretch's
    // tick, is the ring's, and one after it is the film's.
    ring.load(jump(200), &mut g).unwrap();
    assert_eq!(ring.diverged(301, "digest"), None);
    assert_eq!(ring.keyframes().off, None);
    ring.load(jump(299), &mut g).unwrap();
    assert_eq!(ring.diverged(300, "digest"), Some(jump(299)));
    let k = ring.keyframes();
    assert!(k.off.as_deref().is_some_and(|why| why.contains("diverged")), "{k:?}");
    assert!(k.ring.is_empty() && k.ring_bytes == 0);
    // Off for the film: nothing is taken or loaded again.
    ring.passed(&g, 450, 450);
    assert_eq!(ring.nearest(10_000), None);
    assert!(ring.load(jump(450), &mut g).is_err());
}

#[test]
fn the_switches_turn_the_ring_off_and_set_its_cap() {
    let on = Ring::configured(env(&[]), Box::new(Fake::default()));
    let k = on.keyframes();
    assert_eq!((k.ring_cap, k.off), (DEFAULT_CAP_MIB << 20, None));
    let small = Ring::configured(env(&[("H2_THEATER_RING_MIB", "8")]), Box::new(Fake::default()));
    assert_eq!(small.keyframes().ring_cap, 8 << 20);

    let mut off = Ring::configured(env(&[("H2_THEATER_RING", "0")]), Box::new(Fake::default()));
    assert_eq!(off.keyframes().off.as_deref(), Some("H2_THEATER_RING=0"));
    off.passed(&guest(1), 150, 150);
    assert!(off.keyframes().ring.is_empty() && off.nearest(150).is_none());
}
