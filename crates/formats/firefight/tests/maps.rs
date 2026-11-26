//! The map-specific wave sets (docs/game/firefight.md, "Map-specific
//! sets"): every one validates and compiles like a built-in, and the
//! lobby's menu, store values and fallback rule behave as the design
//! says.

use firefight::{Def, Limits, builtin, validate};

fn elite_kit() -> Limits {
    Limits::new(["elite"])
}

/// Each map-specific set is a structurally valid definition the elite kit
/// admits, whose director script builds and whose canonical form decodes
/// to the same set — the checks a lobby makes of a wave set.
#[test]
fn each_map_set_validates_compiles_and_round_trips() {
    for (map, file, text) in builtin::MAP_SETS {
        let def: Def =
            serde_json::from_str(text).unwrap_or_else(|e| panic!("{file} is not a set: {e}"));
        validate(&def, &elite_kit()).unwrap_or_else(|e| panic!("{file}: {e}"));
        firefight::script::load(&def).unwrap_or_else(|e| panic!("{file}: {e:?}"));
        let json = def.canonical_json().unwrap();
        assert_eq!(Def::from_bytes(json.as_bytes(), &elite_kit()).unwrap(), def, "{file}");
        assert_eq!(builtin::map_sets(map), vec![def.clone()], "{map}");
        assert_eq!(builtin::map_sets(map.to_uppercase().as_str()), Vec::<Def>::new(), "{map}");
    }
}

/// The lobby's Wave set row offers the map's own sets first, then the
/// built-ins; a map without its own gets the built-ins alone.
#[test]
fn the_menu_offers_the_maps_sets_first_then_the_built_ins() {
    let creek = builtin::menu(Some("beavercreek"));
    let none = builtin::menu(None);
    assert_eq!(creek[0].0, builtin::MAP_SET_BASE, "the map set's value");
    assert_eq!(creek[0].1.name, "Basin");
    assert_eq!(&creek[1..], &none[..], "then the built-ins");
    assert_eq!(none.len(), 5);
    for (value, def) in &none {
        assert!(
            creek.iter().any(|(v, d)| v == value && d == def),
            "{} ({value}) is offered once",
            def.name
        );
    }
    assert_eq!(builtin::menu(Some("coagulation")), none);
}

/// A store value resolves to what it names on the map: the built-in by
/// its value, the map's set on its map, the built-ins where the map has
/// no such set; a value no set names resolves to nothing.
#[test]
fn a_store_value_resolves_per_map_and_falls_back_to_the_built_ins() {
    let builtins = builtin::definitions();
    for (value, def) in builtins.iter().enumerate() {
        assert_eq!(builtin::resolve(value as i64, None).as_ref(), Some(def));
        assert_eq!(builtin::resolve(value as i64, Some("beavercreek")).as_ref(), Some(def));
        assert_eq!(builtin::resolve(value as i64, Some("coagulation")).as_ref(), Some(def));
    }
    let basin = &builtin::map_sets("beavercreek")[0];
    assert_eq!(
        builtin::resolve(builtin::MAP_SET_BASE, Some("beavercreek")).as_ref(),
        Some(basin)
    );
    // The fallback: the map has no such set, so the built-ins play.
    assert_eq!(builtin::resolve(builtin::MAP_SET_BASE, None), Some(builtins[0].clone()));
    assert_eq!(
        builtin::resolve(builtin::MAP_SET_BASE, Some("coagulation")),
        Some(builtins[0].clone())
    );
    // No set names these.
    assert_eq!(builtin::resolve(-1, None), None);
    assert_eq!(builtin::resolve(99, None), None);
    assert_eq!(
        builtin::resolve(builtin::MAP_SET_BASE + 1, Some("beavercreek")),
        None
    );
    assert!(!builtin::names_set(-1));
    assert!(!builtin::names_set(builtin::MAP_SET_BASE + builtin::MAP_SETS.len() as i64));
    assert!(!builtin::names_set(99));
    for value in 0..builtin::SETS.len() as i64 {
        assert!(builtin::names_set(value), "{value}");
    }
    assert!(builtin::names_set(builtin::MAP_SET_BASE), "the map set's value");
}
