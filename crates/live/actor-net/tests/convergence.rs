use actor_net::wire::{MAX_ACTORS, MAX_STATE_BYTES};
use actor_net::{
    Actor, ActorChange, ActorSet, Budget, Control, KEY_PERIOD_TICKS, Packet, Record, Replica,
    SetError, decode, encode,
};

fn actor(entity: u32) -> Actor {
    Actor {
        entity,
        character: 600,
        squad: 17,
        team: 1,
        posture: 4,
    }
}

fn upsert(entity: u32) -> Record {
    Record::Actor(ActorChange::Upsert(actor(entity)))
}

fn received(tick: u32, records: Vec<Record>) -> Packet {
    decode(&encode(&Packet { tick, records }).unwrap()).unwrap()
}

#[test]
fn key_then_deltas_equals_host_set() {
    // Prefer high bits in entity names so truncating to a pool slot collides.
    let a = 0x0100_0007;
    let b = 0x0200_0007;
    let mut host = ActorSet::default();
    for record in [upsert(b), upsert(a), Record::State(vec![8, 0, 3])] {
        host.apply(&record).unwrap();
    }
    assert_eq!(host.actors(), &[actor(a), actor(b)]);
    let mut client = Replica::default();
    assert!(client.apply(&received(u32::MAX, vec![host.key()])).unwrap());

    let changes = vec![
        Record::Actor(ActorChange::Remove(a)),
        Record::Posture {
            entity: b,
            posture: 11,
        },
        Record::Posture {
            entity: a,
            posture: 12,
        },
        upsert(a + 1),
        Record::State(vec![]),
        Record::Control {
            entity: b,
            control: Control::quantize(-1., 1., true, 0.),
        },
        Record::Line {
            entity: b,
            vocalization: 800,
            tick: 0,
        },
        Record::Digest {
            log_head: [7; 32],
            state_hash: 3,
            actor_count: 2,
        },
    ];
    for change in &changes {
        host.apply(change).unwrap();
    }
    let packet = received(0, changes);
    assert!(client.apply(&packet).unwrap());
    assert!(!client.apply(&packet).unwrap());
    assert!(
        !client
            .apply(&received(u32::MAX, vec![ActorSet::default().key()]))
            .unwrap()
    );
    assert_eq!(client.set(), &host);
    assert_eq!(host.actors()[1].posture, 11);
    assert!(host.state().is_empty());
}

struct Noise(u32);

impl Noise {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }
}

#[test]
fn lost_packets_converge_after_the_next_key() {
    for seed in 1..=10_000 {
        let mut noise = Noise(seed);
        let mut host = ActorSet::default();
        let mut client = Replica::default();
        let mut budget = Budget::default();
        let base = if seed % 2 == 0 {
            u32::MAX - noise.next() % KEY_PERIOD_TICKS
        } else {
            noise.next()
        };
        host.apply(&upsert(65_537)).unwrap();
        let mut held = None;
        for step in 0..12 {
            let entity = noise.next() % 8 + 0x1234_0000;
            let (elapsed, record) = if step == 0 {
                (0, host.key())
            } else {
                let record = match noise.next() % 5 {
                    0 => Record::Actor(ActorChange::Remove(entity)),
                    1 => Record::Posture {
                        entity,
                        posture: noise.next() as u8,
                    },
                    2 => Record::State(vec![(noise.next() % 200) as u8]),
                    _ => upsert(entity),
                };
                (1 + (step - 1) * 12 + noise.next() % 12, record)
            };
            host.apply(&record).unwrap();
            let packet = budget
                .schedule(base.wrapping_add(elapsed), &[record])
                .unwrap()
                .unwrap();
            let bytes = encode(&packet).unwrap();
            match noise.next() % 4 {
                0 => {} // lost
                1 => held = Some(bytes),
                _ => {
                    client.apply(&decode(&bytes).unwrap()).unwrap();
                    if let Some(old) = held.take() {
                        client.apply(&decode(&old).unwrap()).unwrap();
                    }
                }
            }
        }

        // Miss both a replacement actor and a mode change. The late packet
        // arrives after the key, as can happen on an unordered datagram link.
        let changes = [
            Record::Actor(ActorChange::Remove(65_537)),
            upsert(65_538),
            Record::State(vec![0xfa]),
        ];
        for record in &changes {
            host.apply(record).unwrap();
        }
        let due = base.wrapping_add(KEY_PERIOD_TICKS);
        let late = budget
            .schedule(due.wrapping_sub(1), &changes)
            .unwrap()
            .unwrap();
        assert_ne!(client.set().actors(), host.actors(), "seed {seed}");
        assert_ne!(client.set().state(), host.state(), "seed {seed}");
        assert!(budget.key_due(due));
        let key = budget.schedule(due, &[host.key()]).unwrap().unwrap();
        assert!(
            client
                .apply(&decode(&encode(&key).unwrap()).unwrap())
                .unwrap()
        );
        assert!(
            !client
                .apply(&decode(&encode(&late).unwrap()).unwrap())
                .unwrap()
        );
        assert_eq!(client.set(), &host, "seed {seed}");
    }
}

#[test]
fn failed_packet_leaves_set_and_receive_tick_intact() {
    let mut host = ActorSet::default();
    for entity in 0..MAX_ACTORS as u32 {
        host.apply(&upsert(entity)).unwrap();
    }
    let mut replica = Replica::default();
    replica.apply(&received(8, vec![host.key()])).unwrap();
    let overflowing = received(9, vec![Record::State(vec![1]), upsert(200)]);
    assert_eq!(replica.apply(&overflowing), Err(SetError::Full));
    assert_eq!(replica.set(), &host);
    let empty = ActorSet::default();
    assert!(replica.apply(&received(9, vec![empty.key()])).unwrap());
    assert_eq!(replica.set(), &empty);
}

#[test]
fn invalid_key_preserves_previous_actors_and_mode() {
    let mut set = ActorSet::default();
    set.apply(&upsert(8)).unwrap();
    set.apply(&Record::State(vec![7])).unwrap();
    let snapshot = set.clone();
    for bad in [
        Record::Key {
            actors: vec![actor(1), actor(1)],
            state: vec![],
        },
        Record::Key {
            actors: vec![],
            state: vec![0; MAX_STATE_BYTES + 1],
        },
        upsert(u32::MAX),
    ] {
        assert!(set.apply(&bad).is_err());
        assert_eq!(set, snapshot);
    }
}
