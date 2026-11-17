use super::*;
use std::collections::BTreeMap;
use std::f32::consts::{PI, TAU};

#[derive(Default)]
struct Fake(BTreeMap<u32, u8>);
impl Gm for Fake {
    fn rd(&self, addr: u32, out: &mut [u8]) -> bool {
        for (i, b) in out.iter_mut().enumerate() {
            match self.0.get(&(addr + i as u32)) {
                Some(v) => *b = *v,
                None => return false,
            }
        }
        true
    }
    fn wr(&mut self, addr: u32, data: &[u8]) -> bool {
        for (i, b) in data.iter().enumerate() {
            self.0.insert(addr + i as u32, *b);
        }
        true
    }
}

impl Fake {
    fn f(&mut self, a: u32, v: f32) {
        self.set_u32(a, v.to_bits());
    }
    fn get_f(&self, a: u32) -> f32 {
        f32::from_bits(self.u32(a).unwrap())
    }
    fn u8_at(&mut self, a: u32, v: u8) {
        self.wr(a, &[v]);
    }
}

const PC_BASE: u32 = 0x10_0000;
const PC0: u32 = PC_BASE + PC_HEADER;
const HEADERS: u32 = 0x30_0000;
const TAGS: u32 = 0x50_0000;

/// An object of `kind` at slot `slot` (salt `0xe000 + slot`) with body at
/// `body`, whose definition is tag `tag` at `def`.
fn add_object(f: &mut Fake, slot: u32, kind: u8, body: u32, tag: u32, def: u32) -> u32 {
    let header = HEADERS + slot * 12;
    f.set_u16(header, 0xe000 + slot as u16);
    f.u8_at(header + 3, kind);
    f.set_u32(header + 8, body);
    f.set_u32(body, 0xf000_0000 | tag);
    f.set_u32(body + BODY_PARENT, NONE);
    f.set_u32(TAGS + tag * 16 + 8, def);
    (0xe000 + slot) << 16 | slot
}

/// Player one on foot with no weapon, yaw 1.0, pitch 0, limits +-1.
fn world() -> (Fake, u32) {
    let mut f = Fake::default();
    f.set_u32(PLAYER_CONTROL, PC_BASE);
    f.set_u32(OBJECT_HEADERS, 0x20_0000);
    f.set_u32(0x20_0044, HEADERS);
    f.set_u32(TAG_INSTANCES, TAGS);
    let unit = 0x40_0000;
    let datum = add_object(&mut f, 3, BIPED, unit, 9, 0x60_0000);
    f.set_u32(PC0 + PC_UNIT, datum);
    f.f(PC0 + PC_YAW, 1.0);
    f.f(PC0 + PC_PITCH, 0.0);
    f.f(PC0 + PC_PITCH_MIN, -1.0);
    f.f(PC0 + PC_PITCH_MAX, 1.0);
    f.set_u32(PC0 + PC_TARGET, NONE);
    f.f(PC0 + PC_AUTOAIM, 0.0);
    f.f(PC0 + PC_MAGNETISM, 0.0);
    f.u8_at(unit + UNIT_WEAPON, 0xff);
    f.u8_at(unit + UNIT_LEFT_WEAPON, 0xff);
    f.set_u16(unit + UNIT_SEAT, 0xffff);
    f.u8_at(LOOK_LOCKED, 0);
    (f, unit)
}

/// Gives the unit a weapon in slot `slot` of definition `def` (tag `tag`).
fn hold(f: &mut Fake, unit: u32, slot: u32, index: u32, tag: u32, def: u32) {
    let body = 0x70_0000 + slot * 0x1000;
    let datum = add_object(f, 10 + slot, WEAPON, body, tag, def);
    f.set_u32(unit + UNIT_WEAPONS + 4 * slot, datum);
    f.u8_at(unit + index, slot as u8);
}

/// A weapon definition with `zoom` levels, one trigger of `behaviour`
/// and one barrel firing every `recovery` seconds.
fn weapon(f: &mut Fake, def: u32, zoom: i16, behaviour: Option<u16>, recovery: f32) {
    f.set_u16(def + WEAP_ZOOM_LEVELS, zoom as u16);
    match behaviour {
        None => {
            f.set_u32(def + WEAP_TRIGGERS, 0);
        }
        Some(b) => {
            f.set_u32(def + WEAP_TRIGGERS, 1);
            f.set_u32(def + WEAP_TRIGGERS + 4, def + 0x1000);
            f.set_u16(def + 0x1000 + TRIGGER_BEHAVIOUR, b);
        }
    }
    f.set_u32(def + WEAP_BARRELS, 1);
    f.set_u32(def + WEAP_BARRELS + 4, def + 0x2000);
    f.f(def + 0x2000 + BARREL_RATE_MAX, 15.0);
    f.f(def + 0x2000 + BARREL_RECOVERY, recovery);
}

#[test]
fn no_game_is_seat_none() {
    let f = Fake::default();
    assert_eq!(context(&f, 0), Context::default());
    let (mut f, _) = world();
    f.set_u32(PC0 + PC_UNIT, NONE);
    assert_eq!(context(&f, 0).seat, Seat::None, "no unit (dead, cutscene)");
    let (mut f, _) = world();
    f.set_u16(HEADERS + 3 * 12, 0x1234);
    assert_eq!(context(&f, 0).seat, Seat::None, "a stale datum");
}

#[test]
fn on_foot_reads_the_weapon() {
    let (mut f, unit) = world();
    assert_eq!(
        context(&f, 0),
        Context {
            seat: Seat::OnFoot,
            trigger: Trigger::Melee,
            ..Context::default()
        },
        "empty-handed: nothing to shoot"
    );
    weapon(&mut f, 0x61_0000, 2, Some(1), 0.5);
    hold(&mut f, unit, 0, UNIT_WEAPON, 20, 0x61_0000);
    let c = context(&f, 0);
    assert!(c.zoom, "a sniper rifle zooms");
    assert_eq!(c.trigger, Trigger::SemiAuto { rate: 2.0 });
    assert!(!c.dual);
    // An SMG: automatic, no scope; a second SMG in the left hand.
    weapon(&mut f, 0x62_0000, 0, Some(0), 0.0);
    hold(&mut f, unit, 1, UNIT_WEAPON, 21, 0x62_0000);
    hold(&mut f, unit, 2, UNIT_LEFT_WEAPON, 21, 0x62_0000);
    let c = context(&f, 0);
    assert_eq!(
        (c.zoom, c.trigger, c.dual),
        (false, Trigger::Automatic, true)
    );
}

#[test]
fn the_trigger_behaviours() {
    let (mut f, _) = world();
    let def = 0x61_0000;
    for (b, want) in [
        (Some(0), Trigger::Automatic),
        (Some(1), Trigger::SemiAuto { rate: 4.0 }),
        (Some(4), Trigger::SemiAuto { rate: 4.0 }),
        (Some(5), Trigger::SemiAuto { rate: 4.0 }),
        (Some(2), Trigger::Charge { rate: 4.0 }),
        (Some(9), Trigger::Unknown),
        (None, Trigger::Melee),
    ] {
        weapon(&mut f, def, 0, b, 0.25);
        assert_eq!(trigger(&f, def), want, "{b:?}");
    }
    // No recovery time: the barrel's maximum rate.
    weapon(&mut f, def, 0, Some(1), 0.0);
    assert_eq!(trigger(&f, def), Trigger::SemiAuto { rate: 15.0 });
}

#[test]
fn seats_driver_and_passenger() {
    let (mut f, unit) = world();
    let vdef = 0x63_0000;
    let vehicle = add_object(&mut f, 5, VEHICLE, 0x45_0000, 30, vdef);
    f.set_u32(unit + BODY_PARENT, vehicle);
    f.set_u32(vdef + SEATS, 3);
    f.set_u32(vdef + SEATS + 4, 0x64_0000);
    f.set_u32(0x64_0000, 0x48014); // warthog driver
    f.set_u32(0x64_0000 + SEAT_STRIDE, 0x1198); // gunner
    f.set_u32(0x64_0000 + 2 * SEAT_STRIDE, 0x1070); // passenger
    for (seat, want) in [
        (0u16, Seat::Driver),
        (1, Seat::Passenger),
        (2, Seat::Passenger),
    ] {
        f.set_u16(unit + UNIT_SEAT, seat);
        assert_eq!(context(&f, 0).seat, want, "seat {seat}");
    }
    f.set_u16(unit + UNIT_SEAT, 7);
    assert_eq!(
        context(&f, 0).seat,
        Seat::Passenger,
        "an unknown seat is not driving"
    );
    // Parented to something that is not a vehicle: on foot.
    let (mut f, unit) = world();
    let other = add_object(&mut f, 6, WEAPON, 0x46_0000, 31, 0x65_0000);
    f.set_u32(unit + BODY_PARENT, other);
    assert_eq!(context(&f, 0).seat, Seat::OnFoot);
}

#[test]
fn on_target_needs_a_target_and_strength() {
    let (mut f, _) = world();
    assert!(!on_target(&f, PC0));
    f.set_u32(PC0 + PC_TARGET, 0xe001_0001);
    assert!(!on_target(&f, PC0), "a target with no strength");
    f.f(PC0 + PC_MAGNETISM, 0.4);
    assert!(on_target(&f, PC0));
    f.set_u32(PC0 + PC_TARGET, NONE);
    assert!(!on_target(&f, PC0));
}

#[test]
fn turning_right_lowers_yaw_and_wraps() {
    let (mut f, _) = world();
    assert!(turn(&mut f, PC0, (0.25, 0.0)));
    assert!((f.get_f(PC0 + PC_YAW) - 0.75).abs() < 1e-6);
    assert!(turn(&mut f, PC0, (1.0, 0.0)));
    let y = f.get_f(PC0 + PC_YAW);
    assert!((y - (TAU - 0.25)).abs() < 1e-5, "{y}");
    assert!(turn(&mut f, PC0, (-0.5, 0.0)));
    let y = f.get_f(PC0 + PC_YAW);
    assert!((0.0..TAU).contains(&y) && (y - 0.25).abs() < 1e-5, "{y}");
    // Many turns stay in range.
    for _ in 0..100 {
        turn(&mut f, PC0, (PI as f64 * 0.77, 0.0));
        let y = f.get_f(PC0 + PC_YAW);
        assert!((0.0..TAU).contains(&y), "{y}");
    }
}

#[test]
fn pitch_stays_inside_the_live_limits() {
    let (mut f, _) = world();
    assert!(turn(&mut f, PC0, (0.0, 0.5)));
    assert!((f.get_f(PC0 + PC_PITCH) - 0.5).abs() < 1e-6, "up looks up");
    turn(&mut f, PC0, (0.0, 5.0));
    assert_eq!(f.get_f(PC0 + PC_PITCH), 1.0);
    turn(&mut f, PC0, (0.0, -5.0));
    assert_eq!(f.get_f(PC0 + PC_PITCH), -1.0);
    // Nonsense limits: the engine's own bound.
    f.f(PC0 + PC_PITCH_MAX, f32::NAN);
    turn(&mut f, PC0, (0.0, 5.0));
    assert_eq!(f.get_f(PC0 + PC_PITCH), PITCH_LIMIT);
}

#[test]
fn the_tick_applies_the_turn_with_zoom_and_slowdown() {
    let (mut f, _) = world();
    tick(&mut f, (0.2, 0.0), 1.0);
    assert!((f.get_f(PC0 + PC_YAW) - 0.8).abs() < 1e-6);
    tick(&mut f, (0.2, 0.0), 2.0);
    assert!(
        (f.get_f(PC0 + PC_YAW) - 0.7).abs() < 1e-6,
        "zoomed 2x: half"
    );
    f.set_u32(PC0 + PC_TARGET, 0xe001_0001);
    f.f(PC0 + PC_AUTOAIM, 1.0);
    tick(&mut f, (0.2, 0.0), 1.0);
    let want = 0.7 - 0.2 * input::touch_look::ON_TARGET_SCALE as f32;
    assert!(
        (f.get_f(PC0 + PC_YAW) - want).abs() < 1e-6,
        "slowed on target"
    );
}

#[test]
fn the_tick_leaves_facing_alone_when_the_game_holds_look_or_no_unit() {
    let (mut f, _) = world();
    f.u8_at(LOOK_LOCKED, 1);
    tick(&mut f, (0.2, 0.0), 1.0);
    assert_eq!(f.get_f(PC0 + PC_YAW), 1.0);
    let (mut f, _) = world();
    f.set_u32(PC0 + PC_UNIT, NONE);
    tick(&mut f, (0.2, 0.0), 1.0);
    assert_eq!(f.get_f(PC0 + PC_YAW), 1.0);
}

/// Touch look and our skulls both hook the tick after it: the plan must
/// order them, or every boot refuses its features.
#[test]
fn touch_and_our_skulls_plan_together() {
    let mut r = feature::Registry::new(feature::Switches::defaults());
    r.add(&TouchGlue).unwrap();
    let store = std::sync::Arc::new(settings::Store::defaults(crate::setting_defs::ALL));
    r.add(&crate::skulls_custom::SkullsCustom(std::sync::Arc::new(
        crate::skulls_custom::Custom::new(store, None, None, false),
    )))
    .unwrap();
    let plan = r.build(&feature::Base).unwrap_or_else(|e| panic!("{e:?}"));
    assert!(plan.text().contains(FEATURE), "{}", plan.text());
}
