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

fn variant(file: &str, name: &str, firefight: bool, siege: bool) -> Variant {
    Variant { file: file.into(), name: name.into(), support: Support { firefight, siege } }
}

/// Lockout has a kit package and Siege's sites, Midship Siege's sites,
/// Beaver Creek a variant with AI ground, Coagulation (a built-in kit map)
/// two variants that add Siege's sites and one that adds nothing.
fn view(played: Played) -> (Catalogue, View) {
    let c = cat();
    let v = analyse(&c, played, None, &|m| m.name == "Midship" || m.name == "Lockout", &|m| match m.name.as_str() {
        "Beaver Creek" => vec![variant("creek-nights", "Creek Nights", true, false)],
        "Coagulation" => vec![
            variant("zulu", "Zulu Fort", false, true),
            variant("plain", "Blood Plain", false, false),
            variant("keep", "Castle Keep", false, true),
        ],
        _ => Vec::new(),
    });
    (c, v)
}

#[test]
fn the_pane_lists_the_modes_a_map_supports() {
    let (c, v) = view(Played::Other);
    let id = |n: &str| by_name(&c, n).id;
    assert_eq!(v.pane(id("Lockout")).as_deref(), Some("Supports\nFirefight\nSiege"));
    assert_eq!(v.pane(id("Midship")).as_deref(), Some("Supports\nSiege"));
    assert_eq!(v.refusal(id("Midship")), None, "any other game type takes every map");
    assert!(!v.is_empty());
    assert_eq!(View::default().pane(1), None, "nothing is said outside a picker");
    let none = analyse(&c, Played::Other, None, &|_| false, &|_| Vec::new());
    assert_eq!(none.pane(id("Midship")), None, "a map that supports neither has no list");
}

#[test]
fn a_mode_only_a_variant_gives_names_the_variant() {
    let (c, v) = view(Played::Other);
    let id = |n: &str| by_name(&c, n).id;
    // One variant gives it.
    assert_eq!(v.pane(id("Beaver Creek")).as_deref(), Some("Supports\nFirefight on Creek Nights"));
    // The map plays Firefight itself; two variants give Siege, by name.
    assert_eq!(
        v.pane(id("Coagulation")).as_deref(),
        Some("Supports\nFirefight\nSiege on Castle Keep and 1 more")
    );
    assert_eq!(v.own(id("Coagulation")), Support { firefight: true, siege: false });
    assert_eq!(v.support(id("Coagulation")), Support { firefight: true, siege: true }, "the row's is the union");
    assert_eq!(v.own(id("Beaver Creek")), Support::default());
    assert!(v.support(id("Beaver Creek")).firefight);
}

#[test]
fn a_map_with_variants_has_a_page_the_map_first_then_each_by_name() {
    let (c, v) = view(Played::Siege);
    let coag = by_name(&c, "Coagulation");
    let page = v.page(coag.id, &coag.name);
    let names: Vec<&str> = page.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["Coagulation", "Blood Plain", "Castle Keep", "Zulu Fort"]);
    assert_eq!(page[0].file, None, "the map itself");
    assert_eq!(page[2].file.as_deref(), Some("keep"));
    // Each row has its own list, and the lobby's mode refuses a row by
    // what that row plays.
    assert_eq!(page_pane(page[0].support, Played::Siege).as_deref(), Some("No Siege sites or flag stand\nSupports\nFirefight"));
    assert_eq!(page_pane(page[1].support, Played::Siege).as_deref(), Some("No Siege sites or flag stand"));
    assert_eq!(page_pane(page[2].support, Played::Siege).as_deref(), Some("Supports\nSiege"));
    assert_eq!(refusal(page[0].support, Played::Siege), Some(Reason::MapNoSiege));
    assert_eq!(refusal(page[2].support, Played::Siege), None);
    // The root row plays Siege through its variants.
    assert!(v.allows(coag.id));
    assert_eq!(v.refusal(coag.id), None);
    // A map without variants has no page: A picks it.
    let midship = by_name(&c, "Midship");
    assert!(v.page(midship.id, &midship.name).is_empty());
    assert!(v.variants(midship.id).is_empty());
}

#[test]
fn a_page_lists_no_more_variants_than_it_has_rows_for() {
    let c = cat();
    let many = |_: &Map| -> Vec<Variant> {
        (0..40).map(|i| variant(&format!("v{i:02}"), &format!("Variant {i:02}"), false, false)).collect()
    };
    let v = analyse(&c, Played::Other, None, &|_| false, &many);
    let midship = by_name(&c, "Midship");
    assert_eq!(v.page(midship.id, "Midship").len(), 1 + PAGE_VARIANTS);
    let kit = by_name(&c, "Firefight \u{2013} Orphan");
    assert!(v.variants(kit.id).is_empty(), "a kit package has no variants of its own");
}

#[test]
fn a_variants_ground_plays_firefight_only_with_a_wave_set_the_map_takes() {
    let c = cat();
    let Some(prefix) = ::firefight::builtin::variants().iter().find_map(|v| v.map.map(|_| v.prefix)) else {
        return;
    };
    let own = ::firefight::builtin::variant(prefix).unwrap().map.unwrap();
    let v = analyse(&c, Played::Firefight, Some(prefix), &|_| false, &|m| {
        (m.name == "Midship").then(|| variant("pit", "The Pit", true, false)).into_iter().collect()
    });
    let midship = by_name(&c, "Midship");
    assert_eq!(v.support(midship.id).firefight, own.eq_ignore_ascii_case("midship"));
}

#[test]
fn a_description_has_the_pane_under_it() {
    assert_eq!(described("A ring.", Some("Supports\nSiege")), "A ring.\n\nSupports\nSiege");
    assert_eq!(described("", Some("Supports\nSiege")), "Supports\nSiege");
    assert_eq!(described("A ring. ", None), "A ring.");
}

#[test]
fn the_sample_shows_names_only_and_the_union_on_the_root_row() {
    let rows = sample(Played::Siege);
    let names: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
    assert!(names.iter().all(|n| !n.contains('(')), "{names:?}");
    // Lockout plays Siege through Castle Keep, so it lists with Midship,
    // first, and is not refused.
    assert_eq!(&names[..2], ["Lockout", "Midship"]);
    let lockout = &rows[0];
    assert!(!lockout.1);
    assert!(lockout.2.ends_with("Supports\nFirefight\nSiege on Castle Keep"), "{}", lockout.2);
    assert!(rows[2..].iter().all(|r| r.1), "the rest are refused");
    // Its page: the map itself cannot play Siege, Castle Keep can.
    let (map, page) = sample_page(Played::Siege);
    assert_eq!(map, "Lockout");
    let page: Vec<(&str, bool)> = page.iter().map(|r| (r.0.as_str(), r.1)).collect();
    assert_eq!(page, [("Lockout", true), ("Castle Keep", false), ("Lockout Towers", true)]);
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
    let creek = by_name(&c, "Beaver Creek");
    assert_eq!(
        v.pane(creek.id).as_deref(),
        Some("No Siege sites or flag stand\nSupports\nFirefight on Creek Nights")
    );
    let (cf, vf) = view(Played::Firefight);
    let midship = by_name(&cf, "Midship");
    assert_eq!(vf.pane(midship.id).as_deref(), Some("No Firefight kit or AI ground\nSupports\nSiege"));
}

#[test]
fn choosing_a_map_the_mode_lacks_is_refused_with_a_named_reason() {
    let (c, v) = view(Played::Firefight);
    let midship = by_name(&c, "Midship");
    assert_eq!(v.refusal(midship.id), Some(Reason::MapNoFirefight));
    assert!(Reason::MapNoFirefight.text().contains("Firefight"));
    let (c, v) = view(Played::Siege);
    let creek = by_name(&c, "Beaver Creek");
    assert_eq!(v.refusal(creek.id), Some(Reason::MapNoSiege));
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
