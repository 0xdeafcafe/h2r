use super::*;
use crate::lobby_tabs::Reason;

fn map(id: u32, name: &str, path: &str) -> Map {
    Map { id, name: name.into(), sort: id as i32, path: path.into() }
}

fn cat() -> Catalogue {
    Catalogue::new([
        map(1, "Coagulation", r"d:\maps\coagulation"),
        map(2, "Lockout", r"d:\maps\lockout"),
        map(3, "Beaver Creek", r"d:\maps\beavercreek"),
        map(4, "Midship", r"d:\maps\midship"),
        map(10, "Firefight \u{2013} Lockout", r"t:\$C\4d53006400020010\lockout"),
        map(11, "Firefight \u{2013} Orphan", r"t:\$C\4d53006400020011\orphan"),
    ])
}

fn by_name(c: &Catalogue, name: &str) -> Map {
    c.all().find(|m| m.name == name).cloned().unwrap()
}

#[test]
fn firefight_is_a_kit_package_an_installed_or_built_in_kit_or_ground() {
    let c = cat();
    // A kit package.
    assert!(firefight_support(&c, &by_name(&c, "Firefight \u{2013} Lockout"), false, None));
    // An original map an installed kit package names.
    assert!(firefight_support(&c, &by_name(&c, "Lockout"), false, None));
    // A map the game builds a kit for, before its package is installed.
    assert!(firefight_support(&c, &by_name(&c, "Coagulation"), false, None));
    // A map with a saved variant that has AI ground.
    assert!(firefight_support(&c, &by_name(&c, "Beaver Creek"), true, None));
    // Nothing of those.
    assert!(!firefight_support(&c, &by_name(&c, "Midship"), false, None));
}

#[test]
fn a_wave_set_made_for_one_map_needs_that_map() {
    let c = cat();
    let prefix = ::firefight::builtin::variants()
        .iter()
        .find_map(|v| v.map.map(|_| v.prefix))
        .expect("a built-in set is made for one map");
    let own = ::firefight::builtin::variant(prefix).unwrap().map.unwrap();
    let lockout = by_name(&c, "Lockout");
    assert_eq!(
        firefight_support(&c, &lockout, false, Some(prefix)),
        own.eq_ignore_ascii_case("lockout")
    );
    assert!(!firefight_support(&c, &by_name(&c, "Midship"), true, Some(prefix)) || own.eq_ignore_ascii_case("midship"));
}

#[cfg(feature = "rules")]
#[test]
fn siege_is_a_site_a_territory_a_goal_or_the_defenders_flag_stand() {
    use ::rules::labels::{Census, Class, Member};
    let census = |label: &str, team: Option<u8>| {
        let mut c = Census::default();
        c.labels.insert(label.into(), vec![Member { class: Class::Flag, number: None, team }]);
        c
    };
    assert!(crate::rules::gate::siege_objectives(&census("siege_site", None)));
    assert!(crate::rules::gate::siege_objectives(&census("territory", None)));
    assert!(crate::rules::gate::siege_objectives(&census("siege_goal", None)));
    assert!(crate::rules::gate::siege_objectives(&census("ctf_flag_spawn", Some(1))));
    assert!(!crate::rules::gate::siege_objectives(&census("ctf_flag_spawn", Some(0))), "the attackers' stand is not a defence");
    assert!(!crate::rules::gate::siege_objectives(&census("siege_line", None)));
    assert!(!crate::rules::gate::siege_objectives(&Census::default()));
}

fn view(played: Played) -> (Catalogue, View) {
    let c = cat();
    let v = analyse(&c, played, None, &|m| m.name == "Beaver Creek", &|m| m.name == "Midship" || m.name == "Lockout");
    (c, v)
}

#[test]
fn each_map_row_says_which_modes_it_supports() {
    let (c, v) = view(Played::Other);
    let name = |n: &str| by_name(&c, n);
    assert_eq!(v.label(name("Lockout").id, "Lockout").as_deref(), Some("Lockout  Firefight  Siege"));
    assert_eq!(v.label(name("Midship").id, "Midship").as_deref(), Some("Midship  Siege"));
    assert_eq!(v.label(name("Beaver Creek").id, "Beaver Creek").as_deref(), Some("Beaver Creek  Firefight"));
    assert_eq!(v.refusal(name("Midship").id), None, "any other game type takes every map");
    let (_, plain) = view(Played::Other);
    assert!(!plain.is_empty());
    assert_eq!(View::default().label(1, "Lockout"), None);
}

#[test]
fn a_kit_package_hides_under_its_original_map_which_takes_its_id_in_firefight() {
    let (c, v) = view(Played::Firefight);
    let lockout = by_name(&c, "Lockout");
    let kit = by_name(&c, "Firefight \u{2013} Lockout");
    assert!(v.hides(kit.id));
    assert!(!v.hides(by_name(&c, "Firefight \u{2013} Orphan").id), "a kit with no original stays");
    assert_eq!(v.row_id(lockout.id), kit.id);
    let (_, other) = view(Played::Other);
    assert_eq!(other.row_id(lockout.id), lockout.id);
    assert_eq!(v.support(kit.id), v.support(lockout.id));
}

#[test]
fn a_mode_lists_its_maps_first_and_marks_the_rest() {
    let (c, v) = view(Played::Siege);
    let rows: Vec<u32> = c.all().map(|m| m.id).collect();
    let ordered = v.first_supported(rows.clone(), |id| *id);
    let ok = |id: &u32| v.allows(*id);
    let split = ordered.iter().position(|id| !ok(id)).unwrap_or(ordered.len());
    assert!(ordered[..split].iter().all(ok) && ordered[split..].iter().all(|id| !ok(id)));
    assert_eq!(ordered.len(), rows.len(), "every map stays visible");
    let coag = by_name(&c, "Coagulation");
    assert_eq!(v.label(coag.id, "Coagulation").as_deref(), Some("Coagulation  (no Siege sites)"));
    let (cf, vf) = view(Played::Firefight);
    let midship = by_name(&cf, "Midship");
    assert_eq!(vf.label(midship.id, "Midship").as_deref(), Some("Midship  (no Firefight kit)"));
}

#[test]
fn choosing_a_map_the_mode_lacks_is_refused_with_a_named_reason() {
    let (c, v) = view(Played::Firefight);
    let midship = by_name(&c, "Midship");
    assert_eq!(v.refusal(midship.id), Some(Reason::MapNoFirefight));
    assert!(Reason::MapNoFirefight.text().contains("Firefight"));
    let (c, v) = view(Played::Siege);
    let coag = by_name(&c, "Coagulation");
    assert_eq!(v.refusal(coag.id), Some(Reason::MapNoSiege));
    assert!(Reason::MapNoSiege.text().contains("Siege"));
    assert_eq!(v.refusal(by_name(&c, "Midship").id), None);
    assert_eq!(refusal(Support::default(), Played::Other), None);
}

#[test]
fn a_game_type_that_the_map_lacks_moves_to_the_last_used_else_the_first() {
    let rows = [(7, true), (8, true), (9, false), (12, true)];
    // The current map plays it: no move.
    assert_eq!(pick(&rows, Some(8), Some(12)), None);
    // It does not: the last used map, when it plays it.
    assert_eq!(pick(&rows, Some(9), Some(12)), Some(12));
    // The last used map does not play it, or none was: the first.
    assert_eq!(pick(&rows, Some(9), Some(9)), Some(7));
    assert_eq!(pick(&rows, Some(9), None), Some(7));
    assert_eq!(pick(&rows, None, None), Some(7));
    // No map plays it: nowhere to go.
    assert_eq!(pick(&[(1, false)], Some(1), None), None);
}

#[test]
fn the_last_map_used_is_kept_per_game_type() {
    remember(Played::Siege, 41);
    remember(Played::Firefight, 42);
    remember(Played::Other, 43);
    assert_eq!(last(Played::Siege), Some(41));
    assert_eq!(last(Played::Firefight), Some(42));
    assert_eq!(last(Played::Other), None);
}
