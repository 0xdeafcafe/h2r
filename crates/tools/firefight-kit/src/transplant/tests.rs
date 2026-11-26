//! Transplanted classes on invented maps: no input files, no game data.
use super::*;
use crate::testkit::{self, DONOR, SHARED_WEAPON};
use crate::{Config, Labels, Mode, build_package_with, build_with_classes};
use h1campaign::ai::h2::ScenarioAi;

fn classes<'a>(
    donor: &'a [u8],
    schema: &'a Schema,
    names: &[&str],
    cut_to_fit: bool,
) -> Classes<'a> {
    let all = testkit::donor_classes();
    Classes {
        schema,
        donors: vec![Donor {
            name: DONOR,
            bytes: donor,
        }],
        specs: names
            .iter()
            .map(|n| all.iter().find(|c| c.name == *n).unwrap().clone())
            .collect(),
        cut_to_fit,
    }
}

fn build(base: &[u8], with: &Classes<'_>) -> Result<crate::Built> {
    build_with_classes(
        base,
        testkit::NAME,
        Mode::Kit,
        &Config::default(),
        &Labels::default(),
        Some(with),
    )
}

fn datum(bytes: &[u8], at: usize) -> TagId {
    TagId(u32::from_le_bytes(
        bytes[at + 4..at + 8].try_into().unwrap(),
    ))
}

#[test]
fn a_transplanted_closure_resolves() {
    let (base, donor, schema) = (testkit::court(), testkit::donor(), testkit::donor_schema());
    let with = classes(&donor, &schema, &["grunt", "jackal"], false);
    let built = build(&base, &with).unwrap();
    assert!(built.cut.is_empty());
    let mut map = Map::from_reader(Cursor::new(&built.bytes[..])).unwrap();
    map.validate_checksum().unwrap();
    let ai = ScenarioAi::read_map(&mut map).unwrap();
    let elite = map.tag(crate::ai::ELITE_PATH).unwrap().id;
    let grunt = map.tag(r"donor\characters\grunt").unwrap().id;
    let jackal = map.tag(r"donor\characters\jackal").unwrap().id;
    assert_eq!(ai.character_palette, [elite, grunt, jackal]);
    // Each char's references land on tags of the built map: its biped, and
    // the base's own plasma rifle, which the donor's copy did not replace.
    let rifle = map.tag(SHARED_WEAPON).unwrap().id;
    let rail = map.tag(r"donor\weapons\rail").unwrap().id;
    for (class, unit) in [(grunt, "grunt"), (jackal, "jackal")] {
        let bytes = map.read_tag_bytes(class).unwrap();
        let biped = datum(&bytes, 0);
        assert_eq!(
            map.tag(biped).unwrap().name.as_deref(),
            Some(format!(r"donor\units\{unit}").as_str())
        );
        assert_eq!(datum(&bytes, 8), rifle);
        let unit_bytes = map.read_tag_bytes(biped).unwrap();
        assert_eq!(datum(&unit_bytes, 0), rail);
    }
    // The rail and the plasma rifle each exist once.
    for name in [SHARED_WEAPON, r"donor\weapons\rail"] {
        let n = map
            .tags()
            .iter()
            .filter(|t| t.name.as_deref() == Some(name))
            .count();
        assert_eq!(n, 1, "{name}");
    }
    // The second class pays only for its char and biped: the rail came with
    // the first, the plasma rifle was the base's.
    assert_eq!(built.classes[0].tags, 3);
    assert_eq!(built.classes[1].tags, 2);
    assert!(built.classes[1].bytes < built.classes[0].bytes);
    assert_eq!(
        built.costs.classes,
        built.classes.iter().map(|c| c.bytes).sum::<usize>()
    );
}

#[test]
fn a_class_over_the_headroom_is_refused_with_its_cost() {
    let (base, donor, schema) = (testkit::court(), testkit::donor(), testkit::donor_schema());
    let base = testkit::tight(&base, 0x2000);
    crate::build(&base, testkit::NAME, Mode::Kit, &Config::default()).expect("the kit alone fits");
    let small = classes(&donor, &schema, &["grunt"], false);
    build(&base, &small).expect("a small class fits");
    let with = classes(&donor, &schema, &["grunt", "hunter"], false);
    let error = build(&base, &with).err().expect("must refuse").to_string();
    for part in [
        "exceeds tag headroom by ",
        "transplanted classes ",
        "the last cut first: grunt ",
        "hunter ",
        " tags)",
    ] {
        assert!(error.contains(part), "{error}");
    }
}

#[test]
fn the_cut_order_drops_the_last_class_first() {
    let (base, donor, schema) = (testkit::court(), testkit::donor(), testkit::donor_schema());
    let base = testkit::tight(&base, 0x2000);
    let with = classes(&donor, &schema, &["grunt", "jackal", "hunter"], true);
    let built = build(&base, &with).unwrap();
    assert_eq!(built.cut, ["hunter"]);
    assert_eq!(
        built
            .classes
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["grunt", "jackal"]
    );
    let mut map = Map::from_reader(Cursor::new(&built.bytes[..])).unwrap();
    assert!(map.tag(r"donor\characters\hunter").is_err());
    let ai = ScenarioAi::read_map(&mut map).unwrap();
    assert_eq!(ai.character_palette.len(), 3);
    // With room for all three, nothing is cut.
    let roomy = classes(&donor, &schema, &["grunt", "jackal", "hunter"], true);
    let built = build(&testkit::court(), &roomy).unwrap();
    assert!(built.cut.is_empty());
    assert_eq!(built.classes.len(), 3);
    assert!(built.classes[2].bytes > 0x10000);
}

#[test]
fn a_class_the_schema_cannot_copy_is_refused_by_name() {
    let (base, donor) = (testkit::court(), testkit::donor());
    let none = Schema::default();
    let with = classes(&donor, &none, &["grunt"], true);
    let error = build(&base, &with).err().unwrap().to_string();
    assert!(error.contains("class grunt"), "{error}");
    assert!(error.contains("NoSchema"), "{error}");
    // Cutting only answers the headroom, not a refusal.
    let schema = testkit::donor_schema();
    let mut with = classes(&donor, &schema, &["grunt"], true);
    with.specs[0].path = r"donor\characters\missing".into();
    let error = build(&base, &with).err().unwrap().to_string();
    assert!(error.contains("has no char"), "{error}");
}

#[test]
fn two_machines_build_equal_ids() {
    let base = testkit::court();
    let schema = testkit::donor_schema();
    let dir = std::env::temp_dir().join(format!("ff-classes-{}", std::process::id()));
    let id = |dir_name: &str, names: &[&str]| {
        // Each machine reads its own copy of the donor.
        let donor = testkit::donor();
        let with = classes(&donor, &schema, names, false);
        build_package_with(
            &base,
            testkit::NAME,
            Mode::Kit,
            &Config::default(),
            Some(&with),
            &dir,
            dir_name,
            false,
        )
        .unwrap()
    };
    let (a, b) = (id("a", &["grunt", "jackal"]), id("b", &["grunt", "jackal"]));
    assert_eq!(a.id, b.id);
    assert_eq!(a.costs, b.costs);
    assert_eq!(a.classes, b.classes);
    let stage_one = crate::build_package(
        &base,
        testkit::NAME,
        Mode::Kit,
        &Config::default(),
        &dir,
        "one",
        false,
    )
    .unwrap();
    assert_ne!(a.id, stage_one.id);
    // The class list is part of the id: another list, another package.
    assert_ne!(a.id, id("c", &["jackal", "grunt"]).id);
    assert_ne!(a.id, id("d", &["grunt"]).id);
    std::fs::remove_dir_all(dir).unwrap();
}
