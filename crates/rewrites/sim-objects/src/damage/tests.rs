use super::*;
use crate::synth::Pool;
use guest_layout::MemMut;
use guest_layout::object::header;

// Free synthetic memory past the pool's tags: the players array, the
// game-time record and a damage report. Slot 3's body holds the
// game-time global's address (`0x510c54`), so no test uses slot 3.
const PLAYERS: u32 = 0x0055_6000;
const PLAYER_DATA: u32 = 0x0055_6100;
const TIME: u32 = 0x0055_f000;
const REPORT: u32 = 0x0055_e000;

fn word(p: &mut Pool, addr: u32, v: u32) {
    p.ram.write_bytes(addr, &v.to_le_bytes()).unwrap();
}

/// A biped in slot 1 driven by player 0, a weapon in slot 2, two players
/// (player 1 between lives), the game at tick 1234.
fn world() -> (Pool, Datum, Datum, Datum, Datum) {
    let mut p = Pool::new();
    let biped = p.add(1, 0x8001, ObjectType::Biped, header::ACTIVE | header::AWAKE);
    let weapon = p.add(2, 0x8002, ObjectType::Weapon, header::ACTIVE);
    p.body(1).set(&mut p.ram, Object::POSITION, [1.0, 2.0, 3.0]).unwrap();
    p.body(2).set(&mut p.ram, Object::POSITION, [4.0, 5.0, 6.0]).unwrap();

    word(&mut p, guest_layout::global::PLAYERS.base, PLAYERS);
    let a = DatumArray::at(PLAYERS);
    a.set(&mut p.ram, DatumArray::MAXIMUM_COUNT, 16).unwrap();
    a.set(&mut p.ram, DatumArray::ELEMENT_SIZE, Player::SIZE as i32).unwrap();
    a.set(&mut p.ram, DatumArray::DATA, PLAYER_DATA).unwrap();
    let player = |i: u16| Player::at(PLAYER_DATA + u32::from(i) * Player::SIZE);
    let (p0, p1) = (Datum(0xe100_0000), Datum(0xe101_0001));
    player(0).set(&mut p.ram, Player::SALT, p0.salt()).unwrap();
    player(0).set(&mut p.ram, Player::UNIT, biped).unwrap();
    player(1).set(&mut p.ram, Player::SALT, p1.salt()).unwrap();
    player(1).set(&mut p.ram, Player::UNIT, Datum::NONE).unwrap();
    Unit::at(p.body(1).addr).set(&mut p.ram, Unit::PLAYER, p0).unwrap();

    word(&mut p, guest_layout::global::GAME_TIME.base, TIME);
    GameTime::at(TIME).set(&mut p.ram, GameTime::TICK, 1234).unwrap();
    (p, biped, weapon, p0, p1)
}

/// A report crediting player 1's object with a sniper-type hit that
/// depletes the body.
fn report(p: &mut Pool, owner_player: Datum) {
    let r = DamageReport::at(REPORT);
    r.set(&mut p.ram, DamageReport::CODE, 0x4d).unwrap();
    r.set(&mut p.ram, DamageReport::FLAGS, 1).unwrap();
    r.set(&mut p.ram, DamageReport::EFFECT, Datum(0xe200_0007)).unwrap();
    r.set(&mut p.ram, DamageReport::OWNER_PLAYER, owner_player).unwrap();
    r.set(&mut p.ram, DamageReport::OWNER_OBJECT, Datum(0x8009_0009)).unwrap();
    r.set(&mut p.ram, DamageReport::OWNER_TEAM, 3).unwrap();
    r.set(&mut p.ram, DamageReport::DIRECTION, [0.0, 1.0, 0.0]).unwrap();
    r.set(&mut p.ram, DamageReport::ORIGIN, [7.0, 8.0, 9.0]).unwrap();
    r.set(&mut p.ram, DamageReport::BODY, 0.75).unwrap();
    r.set(&mut p.ram, DamageReport::SHIELD, 0.5).unwrap();
}

#[test]
fn a_damage_event_carries_the_report_the_victim_and_the_tick() {
    let (mut p, biped, _, p0, p1) = world();
    report(&mut p, p1);
    let d = damage(&p.ram, REPORT, biped).unwrap();
    assert_eq!(
        d,
        Damage {
            tick: Some(1234),
            victim: biped,
            victim_kind: ObjectType::Biped as u8,
            victim_player: p0,
            owner_player: p1,
            owner_object: Datum(0x8009_0009),
            owner_team: 3,
            code: 0x4d,
            effect: Datum(0xe200_0007),
            flags: 1,
            body: 0.75,
            shield: 0.5,
            direction: [0.0, 1.0, 0.0],
            origin: [7.0, 8.0, 9.0],
            position: Some([1.0, 2.0, 3.0]),
        }
    );
    assert!(d.depleting());
    assert_eq!(guest_layout::damage::reporting_type(d.code), 13);
    assert_eq!(guest_layout::damage::modifier(d.code), 1);
}

#[test]
fn a_victim_that_is_not_a_unit_has_no_player_and_a_stale_one_has_no_position() {
    let (mut p, _, weapon, _, p1) = world();
    report(&mut p, p1);
    let d = damage(&p.ram, REPORT, weapon).unwrap();
    assert_eq!((d.victim_kind, d.victim_player), (ObjectType::Weapon as u8, Datum::NONE));
    assert_eq!(d.position, Some([4.0, 5.0, 6.0]));
    // The same slot under another salt: the report still reads, the
    // object does not.
    let stale = Datum(0x7777_0000 | u32::from(weapon.index()));
    let d = damage(&p.ram, REPORT, stale).unwrap();
    assert_eq!((d.victim_kind, d.victim_player, d.position), (u8::MAX, Datum::NONE, None));
    assert_eq!(d.owner_player, p1);
}

#[test]
fn an_unreadable_report_is_a_fault_and_no_game_time_is_no_tick() {
    let (mut p, biped, _, _, p1) = world();
    assert!(damage(&p.ram, 0x0100_0000, biped).is_err());
    word(&mut p, guest_layout::global::GAME_TIME.base, 0);
    report(&mut p, p1);
    assert_eq!(damage(&p.ram, REPORT, biped).unwrap().tick, None);
    assert_eq!(tick(&p.ram), None);
}

#[test]
fn a_kill_carries_both_players_their_units_and_positions() {
    let (p, biped, _, p0, p1) = world();
    // Player 1 (between lives) killed player 0.
    let k = kill(&p.ram, p1, p0, 0x0d);
    assert_eq!(
        k,
        Kill {
            tick: Some(1234),
            attacker: p1,
            victim: p0,
            code: 0x0d,
            victim_unit: biped,
            attacker_unit: Datum::NONE,
            position: Some([1.0, 2.0, 3.0]),
            attacker_position: None,
        }
    );
    // No attacker (the world), and a victim datum whose salt is stale.
    let k = kill(&p.ram, Datum::NONE, Datum(0x1234_0000), 1);
    assert_eq!((k.attacker_unit, k.attacker_position), (Datum::NONE, None));
    assert_eq!((k.victim_unit, k.position), (Datum::NONE, None));
    // A player index past the array's capacity.
    let k = kill(&p.ram, Datum(0xe100_0040), p0, 1);
    assert_eq!(k.attacker_unit, Datum::NONE);
}
