use std::sync::Mutex;

use ::actor_net::{ActorChange, Control, Record};
use engine::entry::testing::{Run, run};
use engine::entry::{Regs, Ret};
use engine::testing::FakeGuest;
use engine::view::{Datums, fake, objects};
use engine::{GuestMemory, tags};

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
    let entry = move |cx: Cx| -> engine::entry::EntryFuture {
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
