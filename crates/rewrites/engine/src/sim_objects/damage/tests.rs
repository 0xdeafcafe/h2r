//! The damage update on a synthetic guest: the frame and registers at its
//! callouts and returns, and the event recorders. Every value is made up
//! for the test; no game data.

use super::*;
use crate::sim_objects::{TABLE, drive, frame};
use guest_layout::DamageReport;
use runtime::TransferAbi;
use runtime::guest::Permissions;

const STACK: u32 = 0x8000;
const CALLER: u32 = 0xdead;
const ARRAY: u32 = 0x0060_0000;
const HEADERS: u32 = 0x0060_1000;
const BODY: u32 = 0x0060_2000;
const INFO: u32 = 0x0060_3000;
const GLOBALS: u32 = 0x0060_4000;
const TIME: u32 = 0x0060_5000;
const REPORT: u32 = 0x0060_6000;
const OBJECT: u32 = 0x8001_0001;

/// Object slot 1 (a weapon: no actor, no player), the game globals, game
/// time at 2 ticks a second, and the image constants the function reads,
/// with values chosen for the test.
fn world() -> (CpuAbi, Guest) {
    let mut g = Guest::new();
    g.map_zeroed(STACK - 0x1000, 0x2000, Permissions::RW).unwrap();
    g.map_zeroed(0x0045_d000, 0x2000, Permissions::RW).unwrap();
    g.map_zeroed(0x004e_0000, 0x4_0000, Permissions::RW).unwrap();
    g.map_zeroed(ARRAY, 0x1_0000, Permissions::RW).unwrap();
    g.set_u32(STACK, CALLER).unwrap();
    g.set_u32(STACK + 4, OBJECT).unwrap();
    g.set_u32(points::OBJECTS.base, ARRAY).unwrap();
    g.set_u32(ARRAY + 0x44, HEADERS).unwrap();
    g.set_u32(HEADERS + 12 + 8, BODY).unwrap();
    g.set_u8(BODY + 0xaa, 2).unwrap();
    g.set_u32(points::GAME_GLOBALS.base, GLOBALS).unwrap();
    g.set_u32(points::GAME_TIME.base, TIME).unwrap();
    g.set_u16(TIME + 2, 2).unwrap();
    g.set_u32(TIME + 4, 0.5f32.to_bits()).unwrap();
    g.set_u32(ONE, 1.0f32.to_bits()).unwrap();
    g.set_u32(THRESHOLD, 0).unwrap();
    let cpu = CpuAbi {
        esp: STACK,
        ebx: 0x1111,
        ebp: 0x2222,
        esi: 0x3333,
        edi: 0x4444,
        mxcsr: 0x1f80,
        ..Default::default()
    };
    (cpu, g)
}

/// Return from a callout that pops `cleanup` argument bytes.
fn returned(cpu: &mut CpuAbi, g: &Guest, cleanup: u32) -> u32 {
    let site = frame::pop(cpu, g).unwrap();
    cpu.esp += cleanup;
    site
}

fn f32_at(g: &Guest, addr: u32) -> f32 {
    f32::from_bits(g.u32(addr).unwrap())
}

#[test]
fn no_damage_information_returns_at_once_with_the_callers_registers() {
    let (mut cpu, mut g) = world();
    let out = drive(&mut cpu, &mut g, &TABLE, ENTRY).unwrap();
    assert_eq!(out, TransferAbi::call(callee::INFO, 0xd5e0d));
    assert_eq!(cpu.ecx, OBJECT, "the object in ecx");
    assert_eq!(cpu.ebp, BODY);
    // sub esp,70h; three pushes; the return address.
    assert_eq!(cpu.esp, STACK - 0x70 - 12 - 4);
    let site = returned(&mut cpu, &g, 0);
    cpu.eax = 0;
    let out = drive(&mut cpu, &mut g, &TABLE, site).unwrap();
    assert_eq!(out, TransferAbi::ret(CALLER));
    assert_eq!(cpu.esp, STACK + 8);
    assert_eq!((cpu.ebx, cpu.ebp, cpu.esi, cpu.edi), (0x1111, 0x2222, 0x3333, 0x4444));
    assert_eq!(cpu.eax & 0xff, 0, "al from dl, cleared");
}

#[test]
fn the_black_eye_skull_holds_a_players_shield_in_the_campaign() {
    let (mut cpu, mut g) = world();
    g.set_u8(BODY + 0xaa, 0).unwrap();
    g.set_u32(BODY + 0x13c, 0xe100_0000).unwrap();
    g.set_u32(GLOBALS + 8, 1).unwrap();
    g.set_u8(BLACK_EYE, 1).unwrap();
    drive(&mut cpu, &mut g, &TABLE, ENTRY).unwrap();
    let site = returned(&mut cpu, &g, 0);
    cpu.eax = 0;
    drive(&mut cpu, &mut g, &TABLE, site).unwrap();
    assert_eq!(g.u16(BODY + 0x104).unwrap(), 0xa);

    // Not in the campaign: untouched.
    let (mut cpu, mut g) = world();
    g.set_u8(BODY + 0xaa, 0).unwrap();
    g.set_u32(BODY + 0x13c, 0xe100_0000).unwrap();
    g.set_u32(GLOBALS + 8, 2).unwrap();
    g.set_u8(BLACK_EYE, 1).unwrap();
    drive(&mut cpu, &mut g, &TABLE, ENTRY).unwrap();
    let site = returned(&mut cpu, &g, 0);
    cpu.eax = 0;
    drive(&mut cpu, &mut g, &TABLE, site).unwrap();
    assert_eq!(g.u16(BODY + 0x104).unwrap(), 0);
}

#[test]
fn a_shield_below_its_ceiling_recharges_by_the_tick_and_is_replicated() {
    let (mut cpu, mut g) = world();
    // Shield vitality above the threshold, shield at a quarter, no stun,
    // recharge rate a half per second, the recharge effect off (bit 9).
    g.set_u32(BODY + 0xe8, 1.0f32.to_bits()).unwrap();
    g.set_u32(BODY + 0xf0, 0.25f32.to_bits()).unwrap();
    g.set_u16(BODY + 0x10a, 0x0200).unwrap();
    g.set_u32(INFO + 0xd0, 0.5f32.to_bits()).unwrap();

    drive(&mut cpu, &mut g, &TABLE, ENTRY).unwrap();
    let site = returned(&mut cpu, &g, 0);
    cpu.eax = INFO;
    let out = drive(&mut cpu, &mut g, &TABLE, site).unwrap();
    // `edi` is pushed once there is damage information.
    let frame = STACK - 0x70 - 16;
    assert_eq!(out, TransferAbi::call(callee::REPLICATE, 0xd63a4));
    assert_eq!((cpu.eax, g.u32(cpu.esp + 4).unwrap()), (OBJECT, 0x80));
    assert_eq!(cpu.esp, frame - 8);
    // 0.25 + 0.5 s * 0.5 per second, below the ceiling of 1.
    assert_eq!(f32_at(&g, BODY + 0xf0), 0.5);
    assert_eq!(g.u8(BODY + 0x10b).unwrap(), 0x02, "recharging");
    assert_eq!(g.u8(frame + 0x13).unwrap(), 1, "changed");
    assert_eq!(u32::from_le_bytes(cpu.xmm[2][..4].try_into().unwrap()), 1.0f32.to_bits());

    // The two recent-damage accumulators, body then shield.
    let site = returned(&mut cpu, &g, 4);
    let out = drive(&mut cpu, &mut g, &TABLE, site).unwrap();
    assert_eq!(out, TransferAbi::call(callee::RECENT_DAMAGE, 0xd65c1));
    assert_eq!((cpu.esi, cpu.ebx, cpu.edi), (BODY + 0x109, BODY + 0x100, BODY + 0xf8));
    assert_eq!(cpu.edx, 0, "no actor");
    let site = returned(&mut cpu, &g, 4);
    cpu.eax = 0;
    let out = drive(&mut cpu, &mut g, &TABLE, site).unwrap();
    assert_eq!(out, TransferAbi::call(callee::RECENT_DAMAGE, 0xd663d));
    assert_eq!((cpu.esi, cpu.ebx, cpu.edi), (BODY + 0x108, BODY + 0xfc, BODY + 0xf4));
    let site = returned(&mut cpu, &g, 4);
    cpu.eax = 0;
    let out = drive(&mut cpu, &mut g, &TABLE, site).unwrap();
    assert_eq!(out, TransferAbi::ret(CALLER));
    assert_eq!(cpu.eax & 0xff, 1, "something changed");
    assert_eq!(cpu.esp, STACK + 8);
    assert_eq!((cpu.ebx, cpu.ebp, cpu.esi, cpu.edi), (0x1111, 0x2222, 0x3333, 0x4444));
}

#[test]
fn a_call_from_a_site_it_does_not_own_is_a_fault() {
    let (mut cpu, mut g) = world();
    let mut cx = Cx { cpu: &mut cpu, g: &mut g };
    assert_eq!(update(&mut cx, 0xd5e10).unwrap_err().addr, 0xd5e10);
}

#[test]
fn the_recorders_append_damage_and_kill_events_and_the_tick_clears_them() {
    let (_, mut g) = world();
    {
        let m: &mut dyn GuestMemory = &mut g;
        let r = DamageReport::at(REPORT);
        r.set(m, DamageReport::CODE, 0x0d).unwrap();
        r.set(m, DamageReport::OWNER_PLAYER, Datum(0xe101_0001)).unwrap();
        r.set(m, DamageReport::BODY, 0.75).unwrap();
    }
    tick_starts();
    damage_report(&g, REPORT, OBJECT);
    // An unreadable report records nothing.
    damage_report(&g, 0x0100_0000, OBJECT);
    // PLAYER_KILLED's arguments, at its entry.
    let esp = STACK - 0x20;
    g.set_u32(esp + 4, 0xe101_0001).unwrap();
    g.set_u32(esp + 8, 0xe100_0000).unwrap();
    g.set_u32(esp + 16, 0x4d).unwrap();
    player_killed(&g, esp);
    let got = events::with(<[SimEvent]>::to_vec);
    assert_eq!(got.len(), 2, "{got:?}");
    let SimEvent::Damage(d) = got[0] else { panic!("{got:?}") };
    assert_eq!((d.victim, d.owner_player, d.code, d.body), (Datum(OBJECT), Datum(0xe101_0001), 0x0d, 0.75));
    let SimEvent::Kill(k) = got[1] else { panic!("{got:?}") };
    assert_eq!((k.attacker, k.victim, k.code), (Datum(0xe101_0001), Datum(0xe100_0000), 0x4d));
    tick_starts();
    assert!(events::with(<[SimEvent]>::is_empty));
}

const ACTORS: u32 = REPORT + 0x1000;
const ACTOR_DATA: u32 = ACTORS + 0x100;
const KILLER: u32 = 0xe101_0001;
const CHARACTER: u32 = 0xe200_0007;

fn actor_world() -> Guest {
    use guest_layout::{DatumArray, ObjectHeader, ObjectType, Unit, actor::Actor, global};

    let (_, mut g) = world();
    let bitmap = HEADERS + 0x100;
    g.set_u32(ARRAY + DatumArray::ELEMENT_SIZE.offset, ObjectHeader::SIZE).unwrap();
    g.set_u32(ARRAY + DatumArray::HIGH_WATER.offset, 2).unwrap();
    g.set_u32(ARRAY + DatumArray::BITMAP.offset, bitmap).unwrap();
    g.set_u32(bitmap, 1 << 1).unwrap();
    let header = HEADERS + ObjectHeader::SIZE;
    g.set_u16(header + ObjectHeader::SALT.offset, Datum(OBJECT).salt()).unwrap();
    g.set_u8(header + ObjectHeader::KIND.offset, ObjectType::Biped as u8).unwrap();
    g.set_u32(BODY + Unit::PLAYER.offset, u32::MAX).unwrap();
    g.set_u32(BODY + Unit::ACTOR.offset, 0xa000_0000).unwrap();
    g.set_u32(global::ACTORS.base, ACTORS).unwrap();
    g.set_u32(ACTORS + DatumArray::ELEMENT_SIZE.offset, Actor::SIZE).unwrap();
    g.set_u32(ACTORS + DatumArray::HIGH_WATER.offset, 1).unwrap();
    g.set_u32(ACTORS + DatumArray::DATA.offset, ACTOR_DATA).unwrap();
    g.set_u16(ACTOR_DATA, 0xa000).unwrap();
    g.set_u32(ACTOR_DATA + Actor::UNIT, OBJECT).unwrap();
    g.set_u32(ACTOR_DATA + Actor::CHARACTER, CHARACTER).unwrap();

    let m: &mut dyn GuestMemory = &mut g;
    let report = DamageReport::at(REPORT);
    report.set(m, DamageReport::FLAGS, guest_layout::damage::DEPLETING).unwrap();
    report.set(m, DamageReport::OWNER_PLAYER, Datum(KILLER)).unwrap();
    report.set(m, DamageReport::CODE, 0x4d).unwrap();
    g
}

#[test]
fn a_fatal_damage_to_an_actor_unit_is_one_actor_killed() {
    let mut g = actor_world();
    tick_starts();
    damage_report(&g, REPORT, OBJECT);
    // Another hit to the body carries no depletion transition.
    g.set_u32(REPORT + DamageReport::FLAGS.offset, 0).unwrap();
    damage_report(&g, REPORT, OBJECT);
    events::with(|got| {
        assert!(matches!(got, [SimEvent::Damage(_), SimEvent::ActorKilled { .. }, SimEvent::Damage(_)]));
        assert_eq!(
            got[1],
            SimEvent::ActorKilled {
                unit: Datum(OBJECT),
                character: Datum(CHARACTER),
                killer: Datum(KILLER),
                weapon: 0x4d,
            }
        );
    });
}

#[test]
fn a_player_death_is_not_an_actor_kill() {
    let mut g = actor_world();
    let player = 0xe100_0000;
    // A player takes precedence even if the unit retains an actor link.
    g.set_u32(BODY + guest_layout::Unit::PLAYER.offset, player).unwrap();
    let esp = STACK - 0x20;
    g.set_u32(esp + 4, KILLER).unwrap();
    g.set_u32(esp + 8, player).unwrap();
    g.set_u32(esp + 16, 0x4d).unwrap();

    tick_starts();
    damage_report(&g, REPORT, OBJECT);
    player_killed(&g, esp);
    events::with(|got| {
        assert!(matches!(got, [SimEvent::Damage(_), SimEvent::Kill(k)] if k.victim == Datum(player)));
    });
}

#[test]
fn a_stale_actor_link_does_not_credit_a_kill() {
    let mut g = actor_world();
    g.set_u16(ACTOR_DATA, 0xa001).unwrap();
    tick_starts();
    damage_report(&g, REPORT, OBJECT);
    events::with(|got| assert!(matches!(got, [SimEvent::Damage(_)])));
}
