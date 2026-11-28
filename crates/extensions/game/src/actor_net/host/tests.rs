//! Scripted actor and object pools, including the two salted links between them.

use super::*;
use ::actor_net::{Replica, decode, encode};
use guest_layout::testing::FakeGuest;
use guest_layout::view::{Datums, fake, objects::Header};
use guest_layout::{DatumHandle, points};
use guest_layout::{Datum, Object, Unit, actor::Actor as Slot};

const ARENA: u32 = 0x10000;
const ARENA_LEN: u32 = 0x60000;
const ACTORS: u32 = ARENA + 0x1000;
const OBJECTS: u32 = 0x30000;
// Invented complete string IDs at the named selector operands.
const REST_MODE: u32 = 0x0c00_0084;
const ALERT_MODE: u32 = 0x0b00_0085;

struct Pool {
    mem: FakeGuest,
    actors: Datums<Slot>,
    objects: Datums<Header>,
}

impl Pool {
    fn new() -> Self {
        let mut mem = FakeGuest::new();
        mem.map(ARENA, ARENA_LEN)
            .put_u32(points::ACTORS.base, ACTORS)
            .put_u32(points::OBJECTS.base, OBJECTS)
            .map(0x0010_e000, 0x2000)
            .put_u32(0x0010_ef65, REST_MODE)
            .put_u32(0x0010_ef6d, ALERT_MODE);
        let actors = fake::datums(&mut mem, ACTORS, 32);
        let objects = fake::datums(&mut mem, OBJECTS, 32);
        Self {
            mem,
            actors,
            objects,
        }
    }

    fn add(&mut self, entity: u32, x: f32) -> (DatumHandle, Slot, DatumHandle) {
        let (id, slot) = fake::push(&mut self.mem, &self.actors);
        let (unit, header) = fake::push(&mut self.mem, &self.objects);
        let addr = 0x40000 + u32::from(unit.index()) * 0x1000;
        slot.set_animation_mode(&mut self.mem, REST_MODE).unwrap();
        let body = Unit::at(addr);
        let object = Object::at(addr);
        header.set_body(&mut self.mem, addr).unwrap();
        slot.set_unit(&mut self.mem, Some(unit)).unwrap();
        slot.set_team(&mut self.mem, 2).unwrap();
        slot.set_squad(&mut self.mem, 7).unwrap();
        slot.set_character(&mut self.mem, Some(DatumHandle::new(0x9710, 3)))
            .unwrap();
        body.set(&mut self.mem, Unit::ACTOR, Datum(id.raw()))
            .unwrap();
        body.set(&mut self.mem, Unit::PLAYER, Datum::NONE).unwrap();
        object.set(&mut self.mem, Object::ENTITY, entity).unwrap();
        object
            .set(&mut self.mem, Object::PARENT, Datum::NONE)
            .unwrap();
        object
            .set(&mut self.mem, Object::POSITION, [x, 0., 0.])
            .unwrap();
        object
            .set(&mut self.mem, Object::FORWARD, [1., 0., 0.])
            .unwrap();
        body.set(&mut self.mem, Unit::DESIRED_FACING, [0., 1., 0.])
            .unwrap();
        body.set(&mut self.mem, Unit::THROTTLE, [1., -1., 0.])
            .unwrap();
        (id, slot, unit)
    }

    /// Run the same synthetic arena through the production hash and memory adapter.
    fn guest(&self) -> runtime::Guest {
        let mut g = runtime::Guest::new();
        for (at, len) in [
            (ARENA, ARENA_LEN),
            (points::ACTORS.base & !0xfff, 0x1000),
            (points::OBJECTS.base & !0xfff, 0x1000),
            (0x0010_e000, 0x2000),
        ] {
            g.map_zeroed(at, len, runtime::guest::Permissions::RW)
                .unwrap();
            g.write_bytes(at, &self.mem.bytes(at, len as usize).unwrap(), 1)
                .unwrap();
        }
        g
    }
}

fn packet(host: &Host, budget: &mut Budget) -> Packet {
    let sent = host.schedule(budget, None, false).unwrap().unwrap();
    decode(&encode(&sent).unwrap()).unwrap()
}

#[test]
fn diff_reports_new_removed_and_posture() {
    let mut pool = Pool::new();
    let (removed, _, _) = pool.add(0xa100_0010, 10.);
    let (_, actor, speaker) = pool.add(0xc200_0010, 20.);
    let mut host = Host::default();
    let mut budget = Budget::default();
    let mut client = Replica::default();
    host.capture(&pool.mem, 10, false, &[]).unwrap();
    let initial = packet(&host, &mut budget);
    assert!(
        matches!(initial.records.as_slice(), [Record::Key { actors, .. }] if actors.len() == 2)
    );
    client.apply(&initial).unwrap();

    pool.actors.array.delete(&mut pool.mem, removed).unwrap();
    pool.add(0xb300_0010, 30.);
    actor.set_animation_mode(&mut pool.mem, ALERT_MODE).unwrap();
    let line = Line {
        unit: speaker.raw(),
        vocalization: 23,
        tick: 10,
    };
    host.capture(&pool.mem, 11, false, &[line]).unwrap();
    let delta = packet(&host, &mut budget);
    let [
        Record::Actor(ActorChange::Remove(0xa100_0010)),
        Record::Actor(ActorChange::Upsert(added)),
        Record::Line {
            entity: 0xc200_0010,
            vocalization: 23,
            tick: 10,
        },
        Record::Posture {
            entity: 0xc200_0010,
            posture,
        },
    ] = delta.records.as_slice()
    else {
        panic!("unexpected actor deltas: {:?}", delta.records)
    };
    assert_eq!(
        (added.entity, added.character, added.squad, added.team),
        (0xb300_0010, 3, 7, 2)
    );
    let expected_actor = Actor {
        posture: 0x85,
        ..client.set().actors()[1]
    };
    assert_eq!(client.set().actors()[1].posture, 0x84);
    assert_eq!(*posture, 0x85);
    client.apply(&delta).unwrap();
    assert_eq!(client.set().actors(), &[*added, expected_actor]);
    assert_eq!(client.set(), &host.set);
    // A new connection on this same tick gets the current snapshot.
    let join = packet(&host, &mut Budget::default());
    assert!(
        matches!(&join.records[0], Record::Key { actors, .. } if actors == client.set().actors())
    );
}

#[test]
fn a_fault_after_the_first_actor_preserves_the_previous_pool() {
    let mut pool = Pool::new();
    let (_, first, _) = pool.add(1, 0.);
    let (_, _, second) = pool.add(2, 2.);
    let header = pool.objects.get(&pool.mem, second).unwrap().unwrap();
    let body = header.body(&pool.mem).unwrap();
    let mut host = Host::default();
    let mut budget = Budget::default();
    host.capture(&pool.mem, 10, false, &[]).unwrap();
    let mut client = Replica::default();
    client.apply(&packet(&host, &mut budget)).unwrap();
    first.set_team(&mut pool.mem, 3).unwrap();
    header.set_body(&mut pool.mem, 0x8000_0000).unwrap();
    assert!(host.capture(&pool.mem, 11, false, &[]).is_err());
    assert_eq!(&host.set, client.set());
    header.set_body(&mut pool.mem, body).unwrap();
    host.capture(&pool.mem, 12, false, &[]).unwrap();
    let recovered = packet(&host, &mut budget);
    assert!(matches!(
        recovered.records.as_slice(),
        [Record::Actor(ActorChange::Upsert(Actor {
            entity: 1,
            team: 3,
            ..
        }))]
    ));
    client.apply(&recovered).unwrap();
    assert_eq!(&host.set, client.set());
}

#[test]
fn nearest_actors_get_control_first() {
    let mut pool = Pool::new();
    pool.add(41, 10.);
    pool.add(17, 20.);
    pool.add(23, 30.);
    let mut host = Host::default();
    host.capture(&pool.mem, 12, true, &[]).unwrap();
    for (x, expected) in [(0., [41, 17, 23]), (40., [23, 17, 41])] {
        let packet = host
            .schedule(&mut Budget::default(), Some([x, 0., 0.]), false)
            .unwrap()
            .unwrap();
        let observed: Vec<_> = packet
            .records
            .iter()
            .filter_map(|r| match r {
                Record::Control { entity, control } => {
                    assert_eq!(
                        *control,
                        Control::quantize(1., -1., false, std::f32::consts::FRAC_PI_2)
                    );
                    Some(*entity)
                }
                _ => None,
            })
            .collect();
        assert_eq!(observed, expected);
    }
    // Leave room for the header and just one more control in this window.
    // Each peer must spend those bytes on its own nearest actor.
    host.capture(&pool.mem, 13, true, &[]).unwrap();
    for (x, nearest) in [(0., 41), (40., 23)] {
        let mut budget = Budget::default();
        let mut traffic = vec![ActorSet::default().key()];
        traffic.extend(vec![Record::State(vec![1; 63]); 15]);
        let charged = budget.schedule(12, &traffic).unwrap().unwrap();
        assert_eq!(charged.encoded_len().unwrap(), 984);
        let sent = host
            .schedule(&mut budget, Some([x, 0., 0.]), false)
            .unwrap()
            .unwrap();
        assert!(
            matches!(sent.records.as_slice(), [Record::Control { entity, .. }] if *entity == nearest)
        );
    }
    host.capture(&pool.mem, 14, false, &[]).unwrap();
    let off = host
        .schedule(&mut Budget::default(), Some([0.; 3]), false)
        .unwrap()
        .unwrap();
    assert!(matches!(off.records.as_slice(), [Record::Key { .. }]));
}

#[test]
fn capture_leaves_the_arena_hash_unchanged() {
    let mut pool = Pool::new();
    let (_, _, unit) = pool.add(0xcd01_0042, 1.);
    let mut enabled = pool.guest();
    let mut disabled = pool.guest();
    let arena = lockstep::SimRegion::Fixed {
        base: ARENA,
        len: ARENA_LEN,
    };
    let mut host = Host::default();
    let mut budget = Budget::default();
    let mut previous = None;
    for tick in 1..=3 {
        // Identical simulated movement in both guests.
        let x = 0x40000 + Object::POSITION.offset;
        enabled.set_u32(x, (tick as f32).to_bits()).unwrap();
        disabled.set_u32(x, (tick as f32).to_bits()).unwrap();
        let line = Line {
            unit: unit.raw(),
            vocalization: 7,
            tick,
        };
        host.capture(&enabled, tick, true, &[line]).unwrap();
        let sent = host
            .schedule(&mut budget, Some([0.; 3]), false)
            .unwrap()
            .unwrap();
        assert!(sent.records.iter().any(|r| matches!(
            r,
            Record::Line {
                entity: 0xcd01_0042,
                ..
            }
        )));
        let hash = lockstep::hash_sim(&disabled, arena).unwrap();
        assert_ne!(previous, Some(hash));
        assert_eq!(lockstep::hash_sim(&enabled, arena), Some(hash));
        previous = Some(hash);
    }
}

#[test]
#[ignore = "timing benchmark; run in release mode on the benchmark host"]
fn capture_32_actors_for_15_clients() {
    let mut pool = Pool::new();
    for i in 0..32 {
        pool.add(0x4000_0000 + i, i as f32);
    }
    let guest = pool.guest();
    let mut host = Host::default();
    let mut clients: [Budget; 15] = std::array::from_fn(|_| Budget::default());
    let ticks = 3_000;
    let start = std::time::Instant::now();
    for tick in 0..ticks {
        host.capture(&guest, tick, true, &[]).unwrap();
        for (i, budget) in clients.iter_mut().enumerate() {
            let _ = std::hint::black_box(
                host.schedule(budget, Some([i as f32, 0., 0.]), false)
                    .unwrap(),
            );
        }
    }
    let us = start.elapsed().as_secs_f64() * 1e6 / f64::from(ticks);
    eprintln!("actor capture and scheduling: {us:.1} microseconds/tick (32 actors, 15 clients)");
    assert!(
        us < 100.,
        "AN3 capture budget exceeded: {us:.1} microseconds/tick"
    );
}

#[test]
fn the_mode_state_is_one_record_when_it_changes_and_whole_in_every_key() {
    let mut host = Host::default();
    host.tick = Some(5);
    assert_eq!(host.set_mode(&[]), Ok(()), "no mode, no record");
    assert!(host.changes.is_empty());
    host.set_mode(&[1, 2, 3]).unwrap();
    assert!(matches!(host.changes.as_slice(), [Record::State(s)] if s == &[1, 2, 3]));
    assert_eq!(host.set.state(), [1, 2, 3]);
    host.changes.clear();
    host.set_mode(&[1, 2, 3]).unwrap();
    assert!(host.changes.is_empty(), "unchanged, nothing to send");
    assert!(matches!(host.set.key(), Record::Key { state, .. } if state == [1, 2, 3]));
    host.set_mode(&[]).unwrap();
    assert!(matches!(host.changes.as_slice(), [Record::State(s)] if s.is_empty()));
}

#[test]
fn a_mode_state_over_the_blob_limit_is_refused_and_changes_nothing() {
    let mut host = Host::default();
    assert!(host.set_mode(&[0; 200]).is_err());
    assert!(host.changes.is_empty());
    assert!(host.set.state().is_empty());
}

fn shows(packet: &Packet) -> Vec<&Vec<u8>> {
    packet
        .records
        .iter()
        .filter_map(|r| match r {
            Record::Show(bytes) => Some(bytes),
            _ => None,
        })
        .collect()
}

/// The show state rides the first packet whole (with the key), then only
/// when it changes; the digest's hash is of the state the peer was last
/// offered; and a stopped script retracts the state with an empty record.
#[test]
fn the_show_state_rides_the_key_changes_with_the_state_and_repairs() {
    let mut host = Host::default();
    host.tick = Some(30);
    host.capture(&Pool::new().mem, 30, false, &[]).unwrap();

    // No script: no record, and the digest at tick 30 hashes the empty
    // state.
    host.set_show(None);
    let mut budget = Budget::default();
    let first = packet(&host, &mut budget);
    assert!(shows(&first).is_empty());
    let Record::Digest { show_hash, .. } = digests(&first)[0] else {
        unreachable!()
    };
    assert_eq!(*show_hash, ::actor_net::digest::state_hash(&[]));

    // The script starts: the state rides the next packet.
    host.tick = Some(31);
    host.set_show(Some(vec![7, 8]));
    let second = packet(&host, &mut budget);
    assert_eq!(shows(&second), vec![&vec![7, 8]]);

    // A change goes out; the same state twice sends once.
    host.tick = Some(32);
    host.set_show(Some(vec![9]));
    host.set_show(Some(vec![9]));
    let third = packet(&host, &mut budget);
    assert_eq!(shows(&third), vec![&vec![9]]);
    host.tick = Some(33);
    if let Some(sent) = host.schedule(&mut budget, None, false).unwrap() {
        assert!(shows(&sent).is_empty(), "unchanged, nothing to send");
    }

    // The digest at tick 60 hashes the last state offered.
    host.tick = Some(60);
    let sent = host.schedule(&mut budget, None, false).unwrap().unwrap();
    let Record::Digest { show_hash, .. } = digests(&sent)[0] else {
        unreachable!()
    };
    assert_eq!(*show_hash, ::actor_net::digest::state_hash(&[9]));

    // The script stops: an empty record retracts the state.
    host.tick = Some(61);
    host.set_show(None);
    let sent = host.schedule(&mut budget, None, false).unwrap().unwrap();
    assert_eq!(shows(&sent), vec![&Vec::<u8>::new()]);
    assert!(digests(&sent).is_empty());
}

fn digests(packet: &Packet) -> Vec<&Record> {
    packet
        .records
        .iter()
        .filter(|r| matches!(r, Record::Digest { .. }))
        .collect()
}

#[test]
fn a_digest_rides_every_thirtieth_host_tick_with_the_log_head() {
    let mut pool = Pool::new();
    pool.add(0x0100_0001, 1.);
    let mut host = Host::default();
    let mut budget = Budget::default();
    host.capture(&pool.mem, 29, false, &[]).unwrap();
    assert!(digests(&packet(&host, &mut budget)).is_empty());
    pool.add(0x0200_0002, 2.);
    host.capture(&pool.mem, 30, false, &[]).unwrap();
    host.set_mode(&[4, 2]).unwrap();
    host.set_log_head(Some([5; 32]));
    let sent = packet(&host, &mut budget);
    assert_eq!(
        digests(&sent),
        [&Record::Digest {
            log_head: [5; 32],
            state_hash: ::actor_net::digest::state_hash(&[4, 2]),
            actor_count: 2,
            show_hash: ::actor_net::digest::state_hash(&[]),
        }]
    );
    // No mode keeps a log: the head is all zeros.
    host.capture(&pool.mem, 60, false, &[]).unwrap();
    host.set_log_head(None);
    assert!(matches!(
        digests(&packet(&host, &mut budget)).as_slice(),
        [Record::Digest { log_head, .. }] if *log_head == [0; 32]
    ));
}

#[test]
fn a_lost_delta_is_found_by_the_next_digest_and_repaired_by_a_key() {
    use crate::actor_net::client::Client;
    let mut pool = Pool::new();
    pool.add(0x0100_0001, 1.);
    let mut host = Host::default();
    let mut budget = Budget::default();
    let mut client = Client::default();
    host.capture(&pool.mem, 29, false, &[]).unwrap();
    assert!(client.receive(&packet(&host, &mut budget), 1));

    // The tick that creates the second actor is lost on the way.
    pool.add(0x0200_0002, 2.);
    host.capture(&pool.mem, 30, false, &[]).unwrap();
    let lost = packet(&host, &mut budget);
    assert_eq!(digests(&lost).len(), 1);

    // The next digest disagrees with the client's one actor.
    host.capture(&pool.mem, 60, false, &[]).unwrap();
    let quiet = packet(&host, &mut budget);
    assert!(matches!(quiet.records.as_slice(), [Record::Digest { .. }]));
    assert!(client.receive(&quiet, 2));
    assert!(client.take_key_request());

    // The transport passes the request on; the key restores the set.
    host.capture(&pool.mem, 61, false, &[]).unwrap();
    let key = host.schedule(&mut budget, None, true).unwrap().unwrap();
    let key = decode(&encode(&key).unwrap()).unwrap();
    assert!(matches!(key.records.as_slice(), [Record::Key { actors, .. }] if actors.len() == 2));
    assert!(client.receive(&key, 3));
    host.capture(&pool.mem, 90, false, &[]).unwrap();
    assert!(client.receive(&packet(&host, &mut budget), 4));
    assert!(!client.take_key_request());
    assert_eq!(client.status().digest_misses, 0);
    assert!(!client.status().diverged);
}
