use crate::ext::*;

fn sample() -> ForgeExt {
    let mut x = ForgeExt {
        seed: 0xfeed_beef,
        ..ForgeExt::default()
    };
    x.emitters.insert(9, EmitterState { fired: 3, skipped: 1 });
    x.emitters.insert(2, EmitterState::default());
    x.nodes.insert(5, NodeState { on: true, value: -7 });
    x.switches.insert(4, false);
    x.switches.insert(1, true);
    x.occupy(8, &[30, 10, 20, 10]);
    x
}

#[test]
fn the_record_round_trips_and_is_canonical() {
    let x = sample();
    let bytes = x.encode().unwrap();
    assert_eq!(ForgeExt::decode(&bytes).unwrap(), x);
    assert_eq!(ForgeExt::decode(&bytes).unwrap().encode().unwrap(), bytes);
    assert_eq!(x.volumes[&8], [10, 20, 30]);
}

#[test]
fn an_empty_record_is_empty_and_round_trips() {
    let x = ForgeExt::default();
    assert!(x.is_empty());
    assert_eq!(ForgeExt::decode(&x.encode().unwrap()).unwrap(), x);
}

#[test]
fn bytes_the_encoder_would_not_write_are_refused() {
    let bytes = sample().encode().unwrap();
    assert_eq!(ForgeExt::decode(&bytes[..bytes.len() - 1]), Err(Error::Truncated));
    let mut long = bytes.clone();
    long.push(0);
    assert_eq!(ForgeExt::decode(&long), Err(Error::Trailing));
    // Emitters 2 then 9: swap their ids so the list descends.
    let mut swapped = bytes.clone();
    swapped[12..20].copy_from_slice(&9u64.to_le_bytes());
    swapped[12 + 16..20 + 16].copy_from_slice(&2u64.to_le_bytes());
    assert_eq!(ForgeExt::decode(&swapped), Err(Error::Order("emitters")));
    // A node's flag byte of 2.
    let mut flag = bytes.clone();
    let at = 8 + 4 + 2 * 16 + 4 + 8;
    flag[at] = 2;
    assert_eq!(ForgeExt::decode(&flag), Err(Error::Flag));
    assert_eq!(
        ForgeExt::decode(&vec![0; MAX_BYTES + 1]),
        Err(Error::TooLarge(MAX_BYTES + 1))
    );
}

#[test]
fn a_record_over_the_cap_is_refused() {
    let mut x = ForgeExt::default();
    for id in 0..5000 {
        x.emitters.insert(id, EmitterState::default());
    }
    assert!(matches!(x.encode(), Err(Error::TooLarge(_))));
}

#[test]
fn a_volume_reports_who_entered_and_who_left() {
    let mut x = ForgeExt::default();
    let c = x.occupy(1, &[5, 3]);
    assert_eq!((c.entered, c.left), (vec![3, 5], vec![]));
    let c = x.occupy(1, &[5, 9]);
    assert_eq!((c.entered, c.left), (vec![9], vec![3]));
    let c = x.occupy(1, &[]);
    assert_eq!((c.entered, c.left), (vec![], vec![5, 9]));
    assert!(x.volumes.is_empty(), "an empty volume holds no entry");
    let many: Vec<u32> = (0..MAX_OCCUPANTS as u32 + 10).collect();
    x.occupy(2, &many);
    assert_eq!(x.volumes[&2].len(), MAX_OCCUPANTS);
}

const E: Emitter = Emitter {
    period: 10,
    phase: 4,
    burst: 2,
    total: 3,
    alive_limit: 0,
};

fn run(e: &Emitter, ticks: std::ops::Range<u32>, alive: u32) -> (Vec<u32>, EmitterState) {
    let mut s = EmitterState::default();
    let mut at = Vec::new();
    for t in ticks {
        let shot = e.shot(&s, t, true, alive);
        if shot != Shot::None {
            at.push(t);
        }
        Emitter::record(&mut s, shot);
    }
    (at, s)
}

#[test]
fn an_emitter_fires_on_its_scheduled_ticks_and_stops_at_its_limit() {
    let (at, s) = run(&E, 0..100, 0);
    assert_eq!(at, [4, 14, 24]);
    assert_eq!(s, EmitterState { fired: 3, skipped: 0 });
    assert_eq!(E.shot(&EmitterState::default(), 4, true, 0), Shot::Fire(2));
}

#[test]
fn a_late_machine_with_the_same_count_agrees_on_the_next_shot() {
    let late = EmitterState { fired: 1, skipped: 0 };
    assert_eq!(E.shot(&late, 14, true, 0), Shot::Fire(2));
    assert_eq!(E.shot(&late, 15, true, 0), Shot::None);
}

#[test]
fn a_disabled_emitter_and_a_zero_period_never_fire() {
    assert_eq!(E.shot(&EmitterState::default(), 4, false, 0), Shot::None);
    let still = Emitter { period: 0, ..E };
    assert_eq!(run(&still, 0..50, 0).0, Vec::<u32>::new());
}

#[test]
fn an_emitter_at_its_live_limit_skips_the_shot_and_counts_it() {
    let capped = Emitter {
        alive_limit: 3,
        total: 0,
        ..E
    };
    let (at, s) = run(&capped, 0..35, 2);
    assert_eq!(at, [4, 14, 24, 34]);
    assert_eq!(s, EmitterState { fired: 0, skipped: 4 });
    assert_eq!(capped.shot(&s, 4, true, 1), Shot::Fire(2));
}
