use std::sync::Mutex;

use ::actor_net::{ActorChange, Control, Record};
use guest_layout::entry::testing::{Run, run};
use guest_layout::entry::{Regs, Ret};
use guest_layout::testing::FakeGuest;
use guest_layout::view::{Datums, fake, objects};
use guest_layout::{GuestMemory, tags};

use super::*;

const GLOBALS: u32 = 0x10000;
const OBJECTS: u32 = 0x12000;
const WORLD: u32 = 0x18000;
const BODY: u32 = 0x30000;
const STACK: u32 = 0x50000;
const ENTITY: u32 = 0x4400_0003;
const OTHER_ENTITY: u32 = 0x5500_0004;
const SYNTHETIC_MODE: u32 = 0x1234_0086;
const VOCALIZATION: u16 = 2;
const SOUND: u32 = 0x7123_0001;
const VOICE: u32 = 0x6123_0000;
const INSTANCE: u32 = 0x4545_0021;

fn guest() -> (FakeGuest, Datums<objects::Header>) {
    let mut g = FakeGuest::new();
    let pool = fake::datums(&mut g, OBJECTS, 8);
    g.map(WORLD + 0x20ac, 0x8000);
    g.put_u32(points::SIM_GLOBALS.base, GLOBALS)
        .put_u32(GLOBALS + 4, WORLD)
        .put_u32(GLOBALS + 8, 5)
        .put_u32(points::OBJECTS.base, OBJECTS)
        .put_u32(0x0010_ef1d, SYNTHETIC_MODE);
    (g, pool)
}

fn entity_row(entity: u32) -> u32 {
    WORLD + 0x20ac + (entity & 0x3ff) * 0x20
}

fn bind(g: &mut FakeGuest, pool: &Datums<objects::Header>, entity: u32, body: u32) -> DatumHandle {
    let (unit, header) = fake::push(g, pool);
    header.set_body(g, body).unwrap();
    header.set_kind(g, objects::BIPED).unwrap();
    let row = entity_row(entity);
    g.map(body, 0x500)
        .put_u32(row, entity)
        .put_u16(row + 4, 9)
        .put_u32(row + 8, unit.raw())
        .put_u32(body + 0xd4, entity)
        .put_u32(body + 0x13c, u32::MAX)
        .put_f32(body + objects::Body::FORWARD, 1.0);
    unit
}

fn voice(g: &mut FakeGuest, body: u32) {
    let index = 0x62000;
    let tags = 0x63000;
    let bank = 0x64000;
    let rows = 0x64800;
    g.put_u32(tags::TAG_INDEX_GLOBAL, index)
        .put_u32(tags::TAG_ENTRIES_GLOBAL, tags)
        .put_u32(index + tags::index_layout::TAG_COUNT, 2)
        .put_u32(
            tags + tags::entry_layout::GROUP,
            u32::from_be_bytes(*b"udlg"),
        )
        .put_u32(tags + tags::entry_layout::DATUM, VOICE)
        .put_u32(tags + tags::entry_layout::ADDRESS, bank)
        .put_u32(
            tags + 0x10 + tags::entry_layout::GROUP,
            u32::from_be_bytes(*b"snd!"),
        )
        .put_u32(tags + 0x10 + tags::entry_layout::DATUM, SOUND)
        .put_u32(tags + 0x10 + tags::entry_layout::ADDRESS, 0x65000)
        .put_u16(body + 0x342, 0x400)
        .put_u32(body + 0x400, VOICE)
        .put_u32(bank + 0xc, u32::from(VOCALIZATION) + 1)
        .put_u32(bank + 0x10, rows)
        .put_u32(rows + u32::from(VOCALIZATION) * 0x10 + 0xc, SOUND);
}

fn actor(entity: u32, posture: u8) -> Actor {
    Actor {
        entity,
        character: 5,
        squad: 7,
        team: 9,
        posture,
    }
}

fn upsert(a: Actor) -> Record {
    Record::Actor(ActorChange::Upsert(a))
}

fn key(actors: Vec<Actor>) -> Record {
    Record::Key {
        actors,
        state: vec![],
    }
}

fn line(tick: u32) -> Record {
    Record::Line {
        entity: ENTITY,
        vocalization: VOCALIZATION,
        tick,
    }
}

fn packet(tick: u32, records: Vec<Record>) -> Packet {
    Packet { tick, records }
}

fn tick(client: Client, guest: FakeGuest, local_tick: u32) -> (Client, Run) {
    let state = Arc::new(Mutex::new(Some(client)));
    let moved = Arc::clone(&state);
    let entry = move |cx: Cx| -> guest_layout::entry::EntryFuture {
        let mut client = moved.lock().unwrap().take().unwrap();
        let moved = Arc::clone(&moved);
        Box::pin(async move {
            let result = client.before_tick(&cx, local_tick).await;
            *moved.lock().unwrap() = Some(client);
            result.map(|_| Ret::new(0, 0))
        })
    };
    let result = run(
        &entry,
        guest,
        Regs {
            esp: STACK,
            ..Regs::default()
        },
        |call, _| {
            let eax = if call.target == points::SOUND_START_AT_MARKER.base {
                INSTANCE
            } else {
                0
            };
            Regs {
                eax,
                ..Regs::default()
            }
        },
    );
    assert_eq!(result.result, Ok(Ret::new(0, 0)));
    let client = state.lock().unwrap().take().unwrap();
    (client, result)
}

#[test]
fn a_record_waits_for_its_entity_then_applies() {
    let (g, pool) = guest();
    let mut client = Client::default();
    let a = actor(ENTITY, 0x86);
    assert!(client.apply(&packet(500, vec![upsert(a)]), 100).unwrap());
    let (client, result) = tick(client, g, 100);
    assert!(result.calls.is_empty());
    assert_eq!(client.puppets().count(), 0);

    let mut g = result.guest;
    let unit = bind(&mut g, &pool, ENTITY, BODY);
    let (client, result) = tick(client, g, 129);
    assert_eq!(
        client.puppets().collect::<Vec<_>>(),
        [Puppet { actor: a, unit }]
    );
    assert_eq!(result.calls.len(), 1);
    assert_eq!(result.calls[0].target, points::UNIT_APPLY_CONTROL.base);
    assert_eq!(result.calls[0].eax, Some(unit.raw()));
    assert_eq!(result.calls[0].args[0], SYNTHETIC_MODE);
    assert!(tick(client, result.guest, 130).1.calls.is_empty());
}

#[test]
fn an_unknown_entity_is_dropped_after_30_ticks() {
    let (g, pool) = guest();
    let mut client = Client::default();
    let a = actor(ENTITY, 0x86);
    let arrival = u32::MAX - 12;
    client
        .apply(&packet(88, vec![upsert(a), line(88)]), arrival)
        .unwrap();
    let (client, result) = tick(client, g, arrival.wrapping_add(29));
    assert_eq!(client.pending_lines.len(), 1);
    let mut g = result.guest;
    let unit = bind(&mut g, &pool, ENTITY, BODY);
    voice(&mut g, BODY);

    let (client, result) = tick(client, g, arrival.wrapping_add(30));
    assert_eq!(
        client.puppets().collect::<Vec<_>>(),
        [Puppet { actor: a, unit }]
    );
    assert!(result.calls.is_empty());
    assert!(client.pending_lines.is_empty());
    assert!(
        tick(client, result.guest, arrival.wrapping_add(31))
            .1
            .calls
            .is_empty()
    );
}

#[test]
fn a_client_writes_no_damage_or_position() {
    let (mut g, pool) = guest();
    let unit = bind(&mut g, &pool, ENTITY, BODY);
    g.put_f32(BODY + objects::Body::HEALTH, 0.75)
        .put_f32(BODY + objects::Body::SHIELD, 0.625)
        .put_f32(BODY + objects::Body::POSITION, 12.0)
        .put_u16(BODY + objects::Body::DAMAGE_FLAGS, 0x100)
        .put_u32(BODY + 0x148, 0x500)
        .put_u32(BODY + 0x214, 0x0102_3456)
        .put_f32(BODY + 0x15c, -0.75);
    let before = g.bytes(BODY, 0x500).unwrap();
    let before_world = g.bytes(WORLD + 0x20ac, 0x8000).unwrap();
    let turn = Control::quantize(1.0, -1.0, true, std::f32::consts::PI);
    let mut client = Client::default();
    client
        .apply(
            &packet(
                11,
                vec![
                    upsert(actor(ENTITY, 0x86)),
                    Record::Posture {
                        entity: ENTITY,
                        posture: 0x96,
                    },
                    Record::Control {
                        entity: ENTITY,
                        control: turn,
                    },
                ],
            ),
            90,
        )
        .unwrap();
    let (_, result) = tick(client, g, 90);
    assert_eq!(result.guest.bytes(BODY, 0x500).unwrap(), before);
    assert_eq!(
        result.guest.bytes(WORLD + 0x20ac, 0x8000).unwrap(),
        before_world
    );
    assert_eq!(result.calls.len(), 1);
    let call = &result.calls[0];
    assert_eq!(call.target, points::UNIT_APPLY_CONTROL.base);
    assert_eq!(call.eax, Some(unit.raw()));
    assert_eq!(call.ecx, Some(STACK - 0x7c));
    assert_eq!(call.args.len(), 31);
    assert_eq!(call.args[0], SYNTHETIC_MODE);
    assert_eq!(call.args[4], 0x501);
    assert_eq!(call.args[5..8], [1.0f32.to_bits(), (-1.0f32).to_bits(), 0]);
    assert!((f32::from_bits(call.args[10]) + 1.0).abs() < 1e-6);
    assert_eq!(call.args[13], (-0.75f32).to_bits());
    let packet: Vec<_> = call
        .args
        .iter()
        .flat_map(|word| word.to_le_bytes())
        .collect();
    assert_eq!(&packet[6..10], &0x0102_3456u32.to_le_bytes());
    assert_eq!(&packet[10..12], &[0xff, 0xff]);
}

#[test]
fn key_replaces_the_puppet_table() {
    let (mut g, pool) = guest();
    bind(&mut g, &pool, ENTITY, BODY);
    let unit = bind(&mut g, &pool, OTHER_ENTITY, BODY + 0x1000);
    let mut client = Client::default();
    client
        .apply(&packet(3, vec![key(vec![actor(ENTITY, 0x86)])]), 20)
        .unwrap();
    let (mut client, first) = tick(client, g, 20);
    assert_eq!(client.puppets().count(), 1);
    let next = actor(OTHER_ENTITY, 0xff);
    client.apply(&packet(4, vec![key(vec![next])]), 21).unwrap();
    assert!(!client.apply(&packet(3, vec![key(vec![])]), 21).unwrap());

    let (client, result) = tick(client, first.guest, 21);
    assert_eq!(
        client.puppets().collect::<Vec<_>>(),
        [Puppet { actor: next, unit }]
    );
    assert_eq!(result.calls.len(), 1);
    assert_eq!(result.calls[0].eax, Some(unit.raw()));
    assert_eq!(result.calls[0].args[0], u32::MAX);
}

#[test]
fn posture_applies_crouch_and_releases_it_without_control_records() {
    let (mut g, pool) = guest();
    bind(&mut g, &pool, ENTITY, BODY);
    g.put_u32(BODY + 0x148, 0x20);
    let mut client = Client::default();
    client
        .apply(&packet(1, vec![key(vec![actor(ENTITY, 0xef)])]), 100)
        .unwrap();
    let (mut client, result) = tick(client, g, 100);
    assert_eq!(result.calls.len(), 1);
    assert_eq!(result.calls[0].args[0], u32::MAX);
    assert_eq!(result.calls[0].args[4], 0x21);

    let mut g = result.guest;
    g.put_u32(BODY + 0x148, 0x21);
    client
        .apply(
            &packet(
                2,
                vec![Record::Posture {
                    entity: ENTITY,
                    posture: 0x86,
                }],
            ),
            101,
        )
        .unwrap();
    let (_, result) = tick(client, g, 101);
    assert_eq!(result.calls.len(), 1);
    assert_eq!(result.calls[0].args[0], SYNTHETIC_MODE);
    assert_eq!(result.calls[0].args[4], 0x20);
}

#[test]
fn a_repeated_line_starts_once_at_the_units_head() {
    let (g, pool) = guest();
    let mut client = Client::default();
    let a = actor(ENTITY, 0x86);
    client
        .apply(&packet(13, vec![upsert(a), line(13)]), 100)
        .unwrap();
    client.apply(&packet(14, vec![line(13)]), 101).unwrap();
    let (mut client, result) = tick(client, g, 101);
    assert!(result.calls.is_empty());

    let mut g = result.guest;
    let unit = bind(&mut g, &pool, ENTITY, BODY);
    voice(&mut g, BODY);
    client
        .apply(&packet(15, vec![key(vec![a]), line(13)]), 102)
        .unwrap();
    let (mut client, result) = tick(client, g, 102);
    assert_eq!(result.calls.len(), 3);
    assert_eq!(
        &result.calls[1..],
        &[
            GuestCall::new(points::SOUND_START_AT_MARKER.base)
                .esi(unit.raw())
                .eax(0x0400_0095)
                .args([STACK - 16, u32::MAX, SOUND, 1.0f32.to_bits(), 0]),
            GuestCall::new(points::BIPED_DIALOGUE_LIP_SYNC.base)
                .ebx(unit.raw())
                .args([SOUND, INSTANCE]),
        ]
    );

    client.apply(&packet(16, vec![line(13)]), 103).unwrap();
    let (mut client, result) = tick(client, result.guest, 103);
    assert!(result.calls.is_empty());
    client.apply(&packet(17, vec![line(17)]), 104).unwrap();
    let (_, result) = tick(client, result.guest, 104);
    assert_eq!(result.calls.len(), 2);
    assert_eq!(result.calls[0].target, points::SOUND_START_AT_MARKER.base);
}

#[test]
fn recycled_slots_and_non_client_worlds_do_not_receive_puppet_calls() {
    let (mut g, pool) = guest();
    let unit = bind(&mut g, &pool, ENTITY, BODY);
    let other_generation = ENTITY ^ 0x1000_0000;
    let row = entity_row(ENTITY);
    let mut client = Client::default();
    client
        .apply(&packet(5, vec![upsert(actor(ENTITY, 0x86))]), 100)
        .unwrap();

    g.put_u32(GLOBALS + 8, 4);
    let (client, result) = tick(client, g, 100);
    assert!(result.calls.is_empty());
    let mut g = result.guest;
    g.put_u32(GLOBALS + 8, 5).put_u32(row, other_generation);
    let (client, result) = tick(client, g, 101);
    assert!(result.calls.is_empty());
    let mut g = result.guest;
    g.put_u32(row, ENTITY)
        .put_u32(row + 8, unit.raw() ^ 0x10000);
    let (client, result) = tick(client, g, 102);
    assert!(result.calls.is_empty());
    let mut g = result.guest;
    g.put_u32(row + 8, unit.raw())
        .put_u32(BODY + 0x13c, 0x7123_0000);
    let (client, result) = tick(client, g, 103);
    assert!(result.calls.is_empty());
    assert_eq!(client.puppets().count(), 0);
}

#[test]
fn a_rejected_packet_does_not_queue_its_line_or_change_posture() {
    let (mut g, pool) = guest();
    bind(&mut g, &pool, ENTITY, BODY);
    voice(&mut g, BODY);
    let mut client = Client::default();
    let a = actor(ENTITY, 0x86);
    client.apply(&packet(20, vec![upsert(a)]), 50).unwrap();
    let (mut client, result) = tick(client, g, 50);
    let invalid = packet(
        21,
        vec![
            line(21),
            Record::Posture {
                entity: ENTITY,
                posture: 0xef,
            },
            Record::State(vec![0; 64]),
        ],
    );
    assert!(client.apply(&invalid, 51).is_err());
    assert!(!client.apply(&packet(20, vec![line(20)]), 51).unwrap());
    let (mut client, result) = tick(client, result.guest, 51);
    assert!(result.calls.is_empty());
    assert_eq!(client.puppets().next().unwrap().actor, a);
    assert!(client.apply(&packet(21, vec![line(21)]), 52).unwrap());
    assert_eq!(tick(client, result.guest, 52).1.calls.len(), 2);
}

#[test]
fn a_cue_split_across_three_packets_shows_once() {
    let mut client = Client::default();
    let record = Record::Cue {
        tick: 40,
        sequence: 2,
        kind: 0,
        bytes: vec![1, 2, 3],
    };
    // The same cue in three consecutive packets: one presentation.
    for packet_tick in 40..=42 {
        assert!(
            client
                .apply(&packet(packet_tick, vec![record.clone()]), packet_tick)
                .unwrap()
        );
    }
    assert_eq!(
        client.pending_cues,
        vec![ReceivedCue {
            tick: 40,
            sequence: 2,
            kind: 0,
            bytes: vec![1, 2, 3],
        }]
    );
    // A different sequence of the same tick is a new cue; a repeated or
    // older packet is not applied.
    let next = Record::Cue {
        tick: 40,
        sequence: 3,
        kind: 0,
        bytes: vec![1, 2, 3],
    };
    assert!(client.apply(&packet(43, vec![next]), 43).unwrap());
    assert_eq!(client.pending_cues.len(), 2);
    assert!(!client.apply(&packet(42, vec![record]), 44).unwrap());
    assert_eq!(client.pending_cues.len(), 2);
}

#[test]
fn a_show_state_is_queued_once_per_packet_and_a_digest_mismatch_asks_for_a_key() {
    let mut client = Client::default();
    assert!(
        client
            .apply(&packet(1, vec![Record::Show(vec![4, 5])]), 1)
            .unwrap()
    );
    assert_eq!(client.pending_show, [vec![4, 5]]);
    // An older packet's state is not applied again.
    assert!(
        !client
            .apply(&packet(1, vec![Record::Show(vec![6])]), 2)
            .unwrap()
    );
    assert_eq!(client.pending_show.len(), 1);
    client.pending_show.clear();

    // The digest's show hash beside the actor set's: the empty state the
    // client holds agrees with the empty state the hash was made of, and
    // a state the client never received asks for a key. A machine without
    // the `rules` feature checks none of it.
    #[cfg(feature = "rules")]
    {
        let empty = ::actor_net::digest::state_hash(&[]);
        let digest = |show_hash| Record::Digest {
            log_head: [0; 32],
            state_hash: empty,
            actor_count: 0,
            show_hash,
        };
        assert!(client.receive(&packet(30, vec![digest(empty)]), 3));
        assert!(!client.take_key_request());
        assert!(client.receive(&packet(60, vec![digest(empty + 1)]), 4));
        assert!(client.take_key_request());
    }
}

#[test]
fn the_mode_state_is_the_hosts_last_and_a_key_replaces_it() {
    let mut client = Client::default();
    assert!(client.mode_state().is_empty());
    client
        .apply(&packet(1, vec![Record::State(vec![4, 5, 6])]), 1)
        .unwrap();
    assert_eq!(client.mode_state(), [4, 5, 6]);
    client
        .apply(
            &packet(
                2,
                vec![Record::Key {
                    actors: vec![],
                    state: vec![7],
                }],
            ),
            2,
        )
        .unwrap();
    assert_eq!(client.mode_state(), [7]);
}

fn digest_of(actors: &[Actor], state: &[u8]) -> Record {
    let mut set = ::actor_net::ActorSet::default();
    set.apply(&Record::Key {
        actors: actors.to_vec(),
        state: state.to_vec(),
    })
    .unwrap();
    set.digest([3; 32], ::actor_net::digest::state_hash(&[]))
}

#[test]
fn an_agreeing_digest_asks_for_nothing_and_keeps_the_hosts_log_head() {
    let mut client = Client::default();
    let a = actor(ENTITY, 0x86);
    assert!(client.receive(&packet(30, vec![key(vec![a]), digest_of(&[a], &[])]), 1));
    assert!(!client.take_key_request());
    let status = client.status();
    assert_eq!(status.digest_misses, 0);
    assert_eq!(status.host_log_head, Some([3; 32]));
    assert!(!status.diverged);
}

#[test]
fn a_wrong_digest_asks_for_a_key() {
    let mut client = Client::default();
    let (a, b) = (actor(ENTITY, 0x86), actor(OTHER_ENTITY, 0x86));
    client.receive(&packet(1, vec![key(vec![a])]), 1);
    // The upsert of `b` was lost: the host's digest counts two actors.
    assert!(client.receive(&packet(30, vec![digest_of(&[a, b], &[])]), 2));
    assert_eq!(client.status().digest_misses, 1);
    assert!(client.take_key_request());
    assert!(!client.take_key_request(), "one request per need");
    // The key repairs the replica and the next digest agrees.
    client.receive(&packet(31, vec![key(vec![a, b])]), 3);
    assert!(client.receive(&packet(60, vec![digest_of(&[a, b], &[])]), 4));
    assert_eq!(client.status().digest_misses, 0);
    assert!(!client.take_key_request());
}

#[test]
fn a_lost_mode_state_is_caught_by_the_digest() {
    let mut client = Client::default();
    client.receive(&packet(1, vec![Record::State(vec![1])]), 1);
    assert!(client.receive(&packet(30, vec![digest_of(&[], &[2])]), 2));
    assert!(client.take_key_request());
    assert_eq!(client.mode_state(), [1], "the digest changes nothing");
}

#[test]
fn a_packet_that_cannot_apply_asks_for_a_key() {
    let mut client = Client::default();
    let full: Vec<Actor> = (0..32).map(|i| actor(0x0100_0000 + i, 0x86)).collect();
    client.receive(&packet(1, vec![key(full)]), 1);
    // A creation into a full set follows a lost removal.
    assert!(!client.receive(&packet(2, vec![upsert(actor(ENTITY, 0x86))]), 2));
    assert!(client.take_key_request());
}

#[test]
fn three_mismatches_end_the_session() {
    let mut client = Client::default();
    let a = actor(ENTITY, 0x86);
    let host_key = || Record::Key {
        actors: vec![a],
        state: vec![9],
    };
    client.receive(&packet(1, vec![host_key()]), 1);
    // A host whose digests never agree with its own keys.
    let wrong = digest_of(&[a], &[8]);
    for (n, host_tick) in [30u32, 60].into_iter().enumerate() {
        assert!(client.receive(&packet(host_tick, vec![wrong.clone()]), 2 + n as u32));
        assert!(client.take_key_request());
        assert!(!client.status().diverged);
        assert!(client.receive(&packet(host_tick + 1, vec![host_key()]), 2 + n as u32));
    }
    assert!(client.receive(&packet(90, vec![wrong]), 4));
    let status = client.status();
    assert!(status.diverged);
    assert_eq!(status.digest_misses, ::actor_net::digest::DIVERGED_AFTER);
    assert!(
        !client.take_key_request(),
        "a closed channel asks for nothing"
    );
    assert!(client.mode_state().is_empty(), "the HUD's blob is dropped");
    // The rest of the match's packets are dropped.
    assert!(!client.receive(&packet(91, vec![host_key()]), 5));
    assert!(client.replica.set().actors().is_empty());
    assert!(client.presentation.is_empty());
}

#[test]
fn a_key_still_on_its_way_is_asked_for_again_without_ending_the_channel() {
    let mut client = Client::default();
    client.receive(&packet(1, vec![Record::State(vec![9])]), 1);
    let wrong = digest_of(&[], &[8]);
    for host_tick in [30, 60, 90, 120] {
        assert!(client.receive(&packet(host_tick, vec![wrong.clone()]), 2));
        assert!(client.take_key_request());
    }
    assert!(!client.status().diverged);
    assert_eq!(client.status().digest_misses, 1);
}

#[test]
fn a_new_match_reopens_a_closed_channel() {
    let mut client = Client::default();
    let host_key = || Record::Key {
        actors: vec![],
        state: vec![9],
    };
    let wrong = digest_of(&[], &[8]);
    for host_tick in [30, 60, 90] {
        client.receive(&packet(host_tick - 1, vec![host_key()]), 1);
        client.receive(&packet(host_tick, vec![wrong.clone()]), 1);
    }
    assert!(client.status().diverged);
    // `install` starts each match with a fresh client.
    let mut client = Client::default();
    assert!(client.receive(&packet(1, vec![Record::State(vec![8])]), 1));
    assert!(client.receive(&packet(30, vec![wrong]), 2));
    assert!(!client.status().diverged);
    assert_eq!(client.status().digest_misses, 0);
}

#[test]
fn the_report_lists_bound_puppets_and_what_waits_for_a_body() {
    let (mut g, pool) = guest();
    let mut client = Client::default();
    let (a, b) = (actor(ENTITY, 0x86), actor(OTHER_ENTITY, 0x86));
    let unit = bind(&mut g, &pool, ENTITY, BODY);
    assert!(client.receive(
        &packet(
            30,
            vec![
                key(vec![a, b]),
                Record::State(vec![1, 2, 3]),
                digest_of(&[a, b], &[1, 2, 3]),
            ]
        ),
        10
    ));
    let (client, _) = tick(client, g, 10);
    let report = client.report();
    assert_eq!(report.actors, 2);
    assert_eq!(
        report.puppets,
        [agent::actors::Puppet {
            entity: ENTITY,
            character: 5,
            squad: 7,
            team: 9,
            posture: 0x86,
            unit: unit.raw(),
        }]
    );
    assert_eq!(report.waiting, 1, "the second body has not arrived");
    assert_eq!(report.mode_state_bytes, 3);
    assert_eq!(report.digest_misses, 0);
    assert!(!report.diverged);
    assert_eq!(report.host_log_head, Some("03".repeat(32)));
}
