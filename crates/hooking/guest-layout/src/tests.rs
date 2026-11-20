use super::*;
use crate::game::{GameEngine, PlayerStats, TeamStats};
use crate::global::GLOBALS;

/// Every view's declared fields, for the layout checks.
fn all_views() -> Vec<(&'static str, u32, &'static [FieldInfo])> {
    fn v<T: View>() -> (&'static str, u32, &'static [FieldInfo]) {
        (T::NAME, T::SIZE, T::FIELDS)
    }
    vec![
        v::<ObjectHeader>(),
        v::<Object>(),
        v::<ObjectDefinition>(),
        v::<DatumArray>(),
        v::<TagIndex>(),
        v::<TagEntry>(),
        v::<Unit>(),
        v::<Biped>(),
        v::<Weapon>(),
        v::<Device>(),
        v::<Player>(),
        v::<GameGlobals>(),
        v::<GameOptions>(),
        v::<GameVariant>(),
        v::<GameTime>(),
        v::<PlayerGlobals>(),
        v::<GameEngine>(),
        v::<PlayerStats>(),
        v::<TeamStats>(),
    ]
}

#[test]
fn fields_fit_their_structure_and_do_not_overlap() {
    for (name, size, fields) in all_views() {
        let mut spans: Vec<(u32, u32, &str)> = fields
            .iter()
            .map(|f| (f.offset, f.offset + f.size, f.name))
            .collect();
        spans.sort();
        for &(start, end, field) in &spans {
            assert!(end <= size, "{name}.{field} ends at {end:#x}, past {size:#x}");
            assert!(start < end, "{name}.{field} is empty");
        }
        for w in spans.windows(2) {
            assert!(
                w[0].1 <= w[1].0,
                "{name}.{} ({:#x}..{:#x}) overlaps {name}.{} at {:#x}",
                w[0].2,
                w[0].0,
                w[0].1,
                w[1].2,
                w[1].0
            );
        }
        let mut names: Vec<&str> = fields.iter().map(|f| f.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), fields.len(), "{name}: a field named twice");
    }
}

#[test]
fn a_subtype_starts_where_its_parent_ends() {
    // Bodies extend each other at the same address: a unit's fields begin
    // past the shared object fields, a biped's past the unit's.
    let first = |f: &[FieldInfo]| f.iter().map(|x| x.offset).min().unwrap();
    assert!(first(Unit::FIELDS) >= Object::SIZE);
    assert!(first(Biped::FIELDS) >= Unit::SIZE);
    assert!(first(Weapon::FIELDS) >= Object::SIZE);
    assert!(first(Device::FIELDS) >= Object::SIZE);
}

#[test]
fn every_global_is_named_once_with_a_doc() {
    let mut seen = std::collections::BTreeMap::new();
    for g in GLOBALS {
        assert!(seen.insert(g.base, g.name).is_none(), "{} twice", g.name);
        assert!(g.doc.starts_with("docs/"), "{}: doc {}", g.name, g.doc);
    }
    assert_eq!(global::by_base(0x004e_6948).map(|g| g.name), Some("GAME_GLOBALS"));
}

const UNIT: u32 = 0x8000_0000;

#[test]
fn getters_read_little_endian_at_the_field() {
    let mut m = FlatRam::new(UNIT, 0x400);
    let u = Unit::at(UNIT);
    u.set(&mut m, Unit::PLAYER, Datum(0xe123_0004)).unwrap();
    u.set(&mut m, Unit::TEAM, -1).unwrap();
    u.set(&mut m, Unit::AIM, [0.0, 1.0, -0.5]).unwrap();
    u.set(&mut m, Unit::GRENADE_COUNTS, [2, 1]).unwrap();
    assert_eq!(&m.bytes()[0x13c..0x140], &[0x04, 0x00, 0x23, 0xe1]);
    assert_eq!(u.player(&m), Ok(Datum(0xe123_0004)));
    assert_eq!(u.team(&m), Ok(-1));
    assert_eq!(u.aim(&m), Ok([0.0, 1.0, -0.5]));
    assert_eq!(u.grenade_counts(&m), Ok([2, 1]));
    assert_eq!(u.get(&m, Unit::TEAM), Ok(-1));
    assert_eq!(u.addr_of(Unit::PLAYER), UNIT + 0x13c);
    // The same body seen as an object and a biped.
    assert_eq!(u.object().addr, UNIT);
    assert_eq!(Object::at(UNIT).as_unit(), u);
}

#[test]
fn a_read_past_the_memory_faults_with_its_address() {
    let m = FlatRam::new(UNIT, 0x100);
    let fault = Unit::at(UNIT).player(&m).unwrap_err();
    assert_eq!(
        fault,
        Fault {
            addr: UNIT + 0x13c,
            len: 4,
            write: false
        }
    );
}

#[test]
fn the_held_weapon_is_the_slot_the_index_names() {
    let mut m = FlatRam::new(UNIT, 0x400);
    let u = Unit::at(UNIT);
    let w = [Datum(0xaaaa_0001), Datum::NONE, Datum(0xbbbb_0002), Datum::NONE];
    u.set(&mut m, Unit::WEAPONS, w).unwrap();
    u.set(&mut m, Unit::WEAPON_INDEX, 2).unwrap();
    assert_eq!(u.held_weapon(&m), Ok(Some(Datum(0xbbbb_0002))));
    u.set(&mut m, Unit::WEAPON_INDEX, 1).unwrap();
    assert_eq!(u.held_weapon(&m), Ok(None), "an empty slot");
    u.set(&mut m, Unit::WEAPON_INDEX, -1).unwrap();
    assert_eq!(u.held_weapon(&m), Ok(None), "no slot");
    assert_eq!(u.weapon(&m, 4), Ok(None), "out of range");
    assert_eq!(u.weapon(&m, 0), Ok(Some(Datum(0xaaaa_0001))));
}

#[test]
fn a_players_name_stops_at_its_nul() {
    let mut m = FlatRam::new(UNIT, 0x21c);
    let p = Player::at(UNIT);
    let mut units = [0u16; player::NAME_UNITS];
    for (u, c) in units.iter_mut().zip("Chief".encode_utf16()) {
        *u = c;
    }
    p.set(&mut m, Player::NAME, units).unwrap();
    p.set(&mut m, Player::SALT, 0xe123).unwrap();
    assert_eq!(p.name(&m).unwrap(), "Chief");
    assert!(p.listed(&m).unwrap());
    p.set(&mut m, Player::FLAGS, player::LEAVING).unwrap();
    assert!(!p.listed(&m).unwrap());
    // A full buffer is taken whole.
    p.set(&mut m, Player::NAME, [u16::from(b'x'); player::NAME_UNITS]).unwrap();
    assert_eq!(p.name(&m).unwrap().len(), player::NAME_UNITS);
}

#[test]
fn globals_lead_to_their_structures() {
    // A synthetic game: the pointer global, the globals, options and
    // variant, laid out by the views themselves.
    const GLOBALS_AT: u32 = 0x8010_0000;
    let mut m = FlatRam::new(0x004e_0000, 0x10_0000);
    assert_eq!(GameGlobals::current(&m), Ok(None), "null while no game");
    let mut big = FlatRam::new(GLOBALS_AT, 0x2000);
    let g = GameGlobals::at(GLOBALS_AT);
    let options = g.options();
    let variant = options.variant();
    options.set(&mut big, GameOptions::MODE, game::MODE_MULTIPLAYER).unwrap();
    options.set(&mut big, GameOptions::TICK_RATE, 30).unwrap();
    variant.set(&mut big, GameVariant::FLAGS, game::TEAM_PLAY).unwrap();
    g.set(&mut big, GameGlobals::GAME_ENDED, 1).unwrap();
    assert_eq!(options.addr, GLOBALS_AT + 8);
    assert_eq!(variant.addr, GLOBALS_AT + 8 + 0x134);
    assert_eq!(
        &big.bytes()[0x8 + 0x134 + 0x48..0x8 + 0x134 + 0x4c],
        &[1, 0, 0, 0]
    );
    assert_eq!(options.tick_rate(&big), Ok(30));
    assert_eq!(g.game_ended(&big), Ok(1));

    Off::<u32>::new(0)
        .write(&mut m, global::GAME_GLOBALS.base, GLOBALS_AT)
        .unwrap();
    assert_eq!(GameGlobals::current(&m), Ok(Some(g)));
}

#[test]
fn engine_statistics_are_rows_of_their_stride() {
    let e = GameEngine::at(0x1000);
    assert_eq!(e.player_stats(0).addr, 0x1000 + 0x308);
    assert_eq!(e.player_stats(3).addr, 0x1000 + 0x308 + 3 * 0x1c);
    assert_eq!(e.team_stats(2).addr, 0x1000 + 0x4c8 + 2 * 0x12);
}

#[test]
fn object_types_round_trip_and_name_their_group() {
    for (i, t) in ObjectType::ALL.iter().enumerate() {
        assert_eq!(ObjectType::from_u8(i as u8), Some(*t));
        assert_eq!(t.group().len(), 4);
    }
    assert_eq!(ObjectType::from_u8(13), None);
    assert!(ObjectType::Biped.is_unit() && ObjectType::Vehicle.is_unit());
    assert!(!ObjectType::Weapon.is_unit());
}

#[test]
fn node_matrices_are_found_from_the_body() {
    let mut m = FlatRam::new(UNIT, 0x200);
    let o = Object::at(UNIT);
    assert_eq!(o.nodes(&m), Ok(None), "no nodes");
    o.set(&mut m, Object::NODES_SIZE, 3 * 0x34).unwrap();
    o.set(&mut m, Object::NODES_OFFSET, 0x140).unwrap();
    assert_eq!(o.nodes(&m), Ok(Some((UNIT + 0x140, 3))));
    o.set(&mut m, Object::DAMAGE_FLAGS, object::DEPLETED).unwrap();
    assert_eq!(o.depleted(&m), Ok(true));
}

#[test]
fn datum_handles_split_into_salt_and_index() {
    let d = Datum(0xe123_0004);
    assert_eq!((d.salt(), d.index(), d.get()), (0xe123, 4, Some(0xe123_0004)));
    assert_eq!(Datum::NONE.get(), None);
    assert!(Datum::NONE.is_none());
    assert_eq!(format!("{:?}", Datum::NONE), "Datum(none)");
}
