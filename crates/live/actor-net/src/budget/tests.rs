use super::*;
use crate::wire::{Actor, ActorChange, Control, MAX_ACTORS, MAX_STATE_BYTES, decode, encode};

fn actor(entity: u32) -> Actor {
    Actor {
        entity,
        character: 2,
        squad: 7,
        team: 1,
        posture: 0,
    }
}

fn key() -> Record {
    Record::Key {
        actors: (0..MAX_ACTORS as u32).map(actor).collect(),
        state: vec![45; MAX_STATE_BYTES],
    }
}

fn control(entity: u32) -> Record {
    Record::Control {
        entity,
        control: Control::quantize(0.0, 1.0, false, 0.5),
    }
}

fn line(tick: u32) -> Record {
    Record::Line {
        entity: 42,
        vocalization: 3,
        tick,
    }
}

#[test]
fn budget_never_exceeds_its_bytes() {
    let mut clients: [Budget; 16] = std::array::from_fn(|_| Budget::default());
    let mut sizes = [[0usize; 30]; 16];
    let mut totals = [0usize; 16];
    let mut key_due_since = [None; 16];
    for tick in 0..240u32 {
        for (seat, client) in clients.iter_mut().enumerate() {
            if tick % 150 == seat as u32 {
                key_due_since[seat] = Some(tick);
            }
            let mut candidates = Vec::new();
            for index in 0..MAX_ACTORS as u32 {
                // Each client has a different nearest actor and duty cycle.
                let entity = (index + seat as u32) % MAX_ACTORS as u32;
                candidates.push(control(entity));
                candidates.push(Record::Posture {
                    entity,
                    posture: tick as u8,
                });
                if (tick + entity).is_multiple_of(3) {
                    candidates.push(Record::Actor(ActorChange::Upsert(actor(entity))));
                }
            }
            candidates.push(line(tick));
            candidates.push(Record::State(vec![seat as u8; MAX_STATE_BYTES]));
            if key_due_since[seat].is_some() {
                candidates.push(key());
            }

            let packet = client.schedule(tick, &candidates).unwrap();
            let bytes = packet.map_or(0, |packet| {
                let bytes = encode(&packet).unwrap();
                let received = decode(&bytes).unwrap();
                assert_eq!(received.tick, tick);
                if received
                    .records
                    .iter()
                    .any(|r| matches!(r, Record::Key { .. }))
                {
                    let owed = key_due_since[seat].take().unwrap();
                    assert!(tick - owed <= TICKS_PER_SECOND);
                }
                bytes.len()
            });
            if let Some(owed) = key_due_since[seat] {
                assert!(tick - owed < TICKS_PER_SECOND);
            }
            sizes[seat][(tick % TICKS_PER_SECOND) as usize] = bytes;
            assert!(
                sizes[seat].iter().sum::<usize>() <= BYTES_PER_SECOND,
                "seat {seat}, tick {tick}"
            );
            totals[seat] += bytes;
        }
    }
    assert!(key_due_since.iter().all(Option::is_none));
    assert!(totals.iter().all(|&bytes| bytes > 6 * BYTES_PER_SECOND));
}

#[test]
fn line_survives_two_lost_packets_in_three() {
    let mut budget = Budget::default();
    let event = line(27);
    let mut receipts = Vec::new();
    let mut contenders: Vec<_> = (0..MAX_CANDIDATES as u32)
        .map(|id| Record::Actor(ActorChange::Upsert(actor(id))))
        .collect();

    for tick in 27..=30 {
        let offered = if tick == 27 {
            std::slice::from_ref(&event)
        } else {
            &contenders[..]
        };
        let packet = budget.schedule(tick, offered).unwrap();
        receipts.push(packet.map(|p| decode(&encode(&p).unwrap()).unwrap()));
        contenders.rotate_left(1);
    }
    for survivor in 0..3 {
        let lines: Vec<_> = receipts
            .iter()
            .enumerate()
            .filter(|(index, _)| *index == survivor || *index >= 3)
            .filter_map(|(_, packet)| packet.as_ref())
            .flat_map(|packet| &packet.records)
            .filter(|record| matches!(record, Record::Line { .. }))
            .collect();
        assert_eq!(lines, vec![&event]);
    }
}

#[test]
fn records_follow_priority_with_nearest_controls_first() {
    let mut budget = Budget::default();
    let key = key();
    let actor = Record::Actor(ActorChange::Upsert(actor(11)));
    let posture = Record::Posture {
        entity: 11,
        posture: 1,
    };
    let line = line(7);
    let digest = Record::Digest {
        log_head: [2; 32],
        state_hash: 8,
        actor_count: 1,
    };
    let state = Record::State(vec![1]);
    let near = control(21);
    let far = control(3);
    let offers = [
        near.clone(),
        digest.clone(),
        state.clone(),
        far.clone(),
        posture.clone(),
        line.clone(),
        actor.clone(),
        key.clone(),
    ];
    let packet = budget.schedule(7, &offers).unwrap().unwrap();
    assert_eq!(
        packet.records,
        vec![key, actor, line, posture, state, digest, near, far]
    );
}

#[test]
fn controls_follow_a_three_tick_period_across_wrap_and_duplicates() {
    let mut budget = Budget::default();
    let start = u32::MAX - 1;
    for elapsed in 0..60 {
        let tick = start.wrapping_add(elapsed);
        let records = [control(5), control(5)];
        let sent = budget.schedule(tick, &records).unwrap();
        if elapsed.is_multiple_of(CONTROL_PERIOD) {
            assert_eq!(sent.unwrap().records, vec![control(5)]);
        } else {
            assert_eq!(sent, None);
        }
        assert_eq!(budget.schedule(tick, &[line(tick)]).unwrap(), None);
        assert_eq!(
            budget
                .schedule(tick.wrapping_sub(1), &[line(tick)])
                .unwrap(),
            None
        );
    }
}

#[test]
fn invalid_offers_leave_the_tick_and_repeats_intact() {
    let mut budget = Budget::default();
    let event = line(5);
    assert!(
        budget
            .schedule(5, std::slice::from_ref(&event))
            .unwrap()
            .is_some()
    );
    assert!(matches!(
        budget.schedule(10, &vec![line(10); MAX_CANDIDATES + 1]),
        Err(BudgetError::TooManyCandidates)
    ));
    assert!(matches!(
        budget.schedule(10, &[key(), key()]),
        Err(BudgetError::MultipleKeys)
    ));
    assert!(matches!(
        budget.schedule(10, &[Record::State(vec![1; MAX_STATE_BYTES + 1])]),
        Err(BudgetError::InvalidRecord(_))
    ));
    // A gap with no packets still gets copies in the next two packets.
    for tick in [10, 510] {
        let packet = budget.schedule(tick, &[]).unwrap().unwrap();
        assert_eq!(packet.records, vec![event.clone()]);
    }
    assert_eq!(budget.schedule(511, &[]).unwrap(), None);
}

#[test]
fn largest_periodic_key_is_admitted_on_its_due_tick_under_load() {
    let mut client = Budget::default();
    let start = u32::MAX - crate::KEY_PERIOD_TICKS;
    let mut sent_lines = 0;
    for elapsed in 0..=3 * crate::KEY_PERIOD_TICKS {
        let tick = start.wrapping_add(elapsed);
        let due = elapsed.is_multiple_of(crate::KEY_PERIOD_TICKS);
        assert_eq!(client.key_due(tick), due);
        let mut offers = Vec::new();
        for id in 0..MAX_ACTORS as u32 {
            offers.push(control(id));
            offers.push(Record::Line {
                entity: id,
                vocalization: id as u16,
                tick,
            });
            offers.push(Record::Posture {
                entity: id,
                posture: tick as u8,
            });
        }
        if due {
            offers.push(key());
        }
        let packet = client.schedule(tick, &offers).unwrap();
        let received = packet.map(|packet| decode(&encode(&packet).unwrap()).unwrap());
        let has_key = received
            .as_ref()
            .is_some_and(|packet| packet.records.first() == Some(&key()));
        assert_eq!(has_key, due, "elapsed {elapsed}");
        if let Some(received) = received {
            sent_lines += received
                .records
                .iter()
                .filter(|r| matches!(r, Record::Line { .. }))
                .count();
        }
    }
    assert!(sent_lines > MAX_ACTORS);
}

#[test]
fn promised_lines_and_due_key_both_fit_after_skipping_ticks() {
    let mut client = Budget::default();
    client.schedule(0, &[key()]).unwrap().unwrap();
    let offers: Vec<_> = (0..MAX_CANDIDATES as u32)
        .map(|entity| Record::Line {
            entity,
            vocalization: 2,
            tick: 80,
        })
        .collect();
    let first = client.schedule(80, &offers).unwrap().unwrap();
    assert!(first.records.len() > 1);
    let due_tick = crate::KEY_PERIOD_TICKS;
    assert!(client.key_due(due_tick));
    let second = client.schedule(due_tick, &[key()]).unwrap().unwrap();
    assert_eq!(second.records.first(), Some(&key()));
    assert_eq!(&second.records[1..], first.records.as_slice());
    let third = client.schedule(due_tick + 1, &[]).unwrap().unwrap();
    assert_eq!(third.records, first.records);
    assert!(encode(&second).unwrap().len() + encode(&third).unwrap().len() <= BYTES_PER_SECOND);
}
