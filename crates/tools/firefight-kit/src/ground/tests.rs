//! The ground build on the invented `court` (`testkit`): no disc bytes.
use std::io::Cursor;

use h1campaign::ai::h2::ScenarioAi;
use map::{Map, build::TagSet, writer::max_metadata_size};

use super::*;
use crate::Source;
use crate::testkit::{self, NAME};

fn recipe() -> Recipe {
    Recipe::new(Mode::Kit)
}

fn ai(bytes: &[u8]) -> ScenarioAi {
    let mut map = Map::from_reader(Cursor::new(bytes)).unwrap();
    ScenarioAi::read_map(&mut map).unwrap()
}

fn drop_at(x: f32, y: f32) -> Marker {
    Marker {
        position: [x, y, 0.],
        facing: 0.,
    }
}

fn base_at(x: f32, y: f32, team: Team) -> BaseMarker {
    BaseMarker {
        position: [x, y, 0.],
        team,
    }
}

/// Two drops well off the starts (3,13), (25,15) and (14,14).
fn placed() -> Labels {
    Labels {
        drops: vec![drop_at(9., 6.), drop_at(21., 22.)],
        ..Labels::default()
    }
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "ff-ground-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

#[test]
fn no_labels_gives_the_kit_the_generator_builds() {
    let base = testkit::court();
    let ground = build(&base, NAME, &Labels::default(), &recipe()).unwrap();
    let kit = crate::build(&base, NAME, Mode::Kit, &Config::default()).unwrap();
    assert_eq!(ground.bytes, kit.bytes);
    assert_eq!(ground.costs, kit.costs);
    assert_eq!(ground.zones, kit.zones);
    assert_eq!(ground.bases, kit.bases);
    assert!(ground.problems.is_empty());
    assert!((6..=12).contains(&ground.zones));
    // The same package as `build_package` installs.
    let dir = temp_dir("kit");
    let package = ground.install(&base, NAME, &dir, "ground", false).unwrap();
    let old = crate::build_package(
        &base,
        NAME,
        Mode::Kit,
        &Config::default(),
        &dir,
        "kit",
        false,
    )
    .unwrap();
    assert_eq!(package.id, old.id);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_placed_drop_is_its_zone_and_a_moved_one_moves_it() {
    let base = testkit::court();
    let first = build(&base, NAME, &placed(), &recipe()).unwrap();
    assert_eq!(first.zones, 2);
    assert!(first.problems.is_empty(), "{:?}", first.problems);
    let zones = ai(&first.bytes).zones;
    // The drops, then one zone per base.
    assert_eq!(zones.len(), 4);
    assert_eq!(zones[0].name, "ff_drop_00");
    assert_eq!(zones[1].name, "ff_drop_01");
    assert_eq!(zones[0].firing_positions[0].position, [9., 6., 0.]);
    assert_eq!(zones[1].firing_positions[0].position, [21., 22., 0.]);

    let mut labels = placed();
    labels.drops[1] = drop_at(21., 18.);
    let moved = build(&base, NAME, &labels, &recipe()).unwrap();
    let zones = ai(&moved.bytes).zones;
    assert_eq!(zones[0].firing_positions[0].position, [9., 6., 0.]);
    assert_eq!(zones[1].firing_positions[0].position, [21., 18., 0.]);
    assert_ne!(first.bytes, moved.bytes);
    assert_ne!(first.receipt, moved.receipt);
    // Every squad slot still starts at one of the two zones, and the
    // Elite's class record follows them.
    assert_eq!(ai(&moved.bytes).squads.len(), 17);
}

#[test]
fn the_same_inputs_give_the_same_bytes_in_any_label_order() {
    let base = testkit::court();
    let a = build(&base, NAME, &placed(), &recipe()).unwrap();
    let b = build(&base, NAME, &placed(), &recipe()).unwrap();
    assert_eq!(a.bytes, b.bytes);
    assert_eq!(a.receipt, b.receipt);
    let mut shuffled = placed();
    shuffled.drops.reverse();
    let c = build(&base, NAME, &shuffled, &recipe()).unwrap();
    assert_eq!(a.bytes, c.bytes);
    assert_eq!(a.receipt, c.receipt);
}

#[test]
fn placed_bases_replace_the_found_ones() {
    let base = testkit::court();
    let mut labels = placed();
    labels.bases = vec![base_at(22., 22., Team::Blue), base_at(6., 6., Team::Red)];
    let ground = build(&base, NAME, &labels, &recipe()).unwrap();
    assert_eq!(ground.bases.source, Source::Labels);
    assert_eq!(ground.bases.points, [[6., 6., 0.], [22., 22., 0.]]);
    let blue = build(&base, NAME, &labels, &recipe().home(Team::Blue)).unwrap();
    assert_eq!(blue.bases.points, [[22., 22., 0.], [6., 6., 0.]]);
    // The kit's own pick is the flag stands.
    let found = build(&base, NAME, &placed(), &recipe()).unwrap();
    assert_eq!(found.bases.source, Source::FlagStands);
    assert_ne!(found.bytes, ground.bytes);
    // One base alone is refused, by label.
    labels.bases.pop();
    let error = build(&base, NAME, &labels, &recipe()).err().unwrap();
    assert!(error.to_string().contains("ff_base"), "{error}");
}

#[test]
fn a_placed_cover_is_taken_first_and_one_off_the_ground_is_a_problem() {
    let base = testkit::court();
    let mut labels = placed();
    labels.covers = vec![drop_at(11., 7.)];
    let ground = build(&base, NAME, &labels, &recipe()).unwrap();
    assert!(ground.problems.is_empty(), "{:?}", ground.problems);
    let zone = &ai(&ground.bytes).zones[0];
    assert_eq!(zone.firing_positions[0].position, [9., 6., 0.]);
    assert_eq!(zone.firing_positions[1].position, [11., 7., 0.]);

    labels.covers.push(drop_at(200., 200.));
    let ground = build(&base, NAME, &labels, &recipe()).unwrap();
    let [problem] = &ground.problems[..] else {
        panic!("{:?}", ground.problems);
    };
    assert_eq!(
        (problem.label, problem.position),
        ("ff_cover", [200., 200., 0.])
    );
}

#[test]
fn a_drop_off_the_ground_is_a_problem_and_the_rest_build() {
    let base = testkit::court();
    let mut labels = placed();
    labels.drops.push(drop_at(200., 200.));
    let ground = build(&base, NAME, &labels, &recipe()).unwrap();
    assert_eq!(ground.zones, 2);
    let [problem] = &ground.problems[..] else {
        panic!("{:?}", ground.problems);
    };
    assert_eq!(problem.label, "ff_drop");
    assert_eq!(problem.position, [200., 200., 0.]);
    // All of them off the ground: nothing to build on.
    let none = Labels {
        drops: vec![drop_at(200., 200.)],
        ..Labels::default()
    };
    assert!(build(&base, NAME, &none, &recipe()).is_err());
}

#[test]
fn siege_labels_off_the_ground_are_problems() {
    let base = testkit::court();
    let mut labels = placed();
    labels.sites = vec![[14., 6., 0.], [200., 200., 0.]];
    labels.goals = vec![[14., 22., 0.]];
    labels.lines = vec![LineMarker {
        position: [-90., 3., 0.],
        phase: 1,
        team: Team::Blue,
    }];
    let ground = build(&base, NAME, &labels, &recipe()).unwrap();
    let seen: Vec<_> = ground.problems.iter().map(|p| (p.label, p.index)).collect();
    assert_eq!(seen, [("siege_site", 1), ("siege_line", 0)]);
}

#[test]
fn the_receipt_says_what_made_the_ground_stale() {
    let base = testkit::court();
    let sha = Sha256::of(&base);
    let labels = placed();
    let ground = build(&base, NAME, &labels, &recipe()).unwrap();
    let r = ground.receipt;
    assert_eq!(r.stale(sha, &labels, &recipe()), None);
    let mut shuffled = labels.clone();
    shuffled.drops.reverse();
    assert_eq!(r.stale(sha, &shuffled, &recipe()), None);

    let mut moved = labels.clone();
    moved.drops[0].position[0] += 1.;
    assert_eq!(r.stale(sha, &moved, &recipe()), Some(Stale::Labels));
    let mut added = labels.clone();
    added.sites.push([1., 1., 0.]);
    assert_eq!(r.stale(sha, &added, &recipe()), Some(Stale::Labels));
    assert_eq!(
        r.stale(sha, &labels, &recipe().home(Team::Blue)),
        Some(Stale::Recipe)
    );
    assert_eq!(
        r.stale(Sha256::of(b"another map"), &labels, &recipe()),
        Some(Stale::Collision)
    );
    let mut old = r;
    old.recipe.kit_version += 1;
    assert_eq!(old.stale(sha, &labels, &recipe()), Some(Stale::KitVersion));
}

#[test]
fn a_recipe_for_another_kit_and_bad_labels_are_refused() {
    let base = testkit::court();
    let mut old = recipe();
    old.kit_version += 1;
    let error = build(&base, NAME, &Labels::default(), &old).err().unwrap();
    assert!(error.to_string().contains("kit v"), "{error}");
    let mut labels = placed();
    labels.drops[0].position[1] = f32::NAN;
    let error = build(&base, NAME, &labels, &recipe()).err().unwrap();
    assert!(error.to_string().contains("ff_drop"), "{error}");
}

#[test]
fn ground_over_headroom_is_refused_with_its_biggest_part() {
    let base = testkit::court();
    let mut map = Map::from_reader(Cursor::new(&base)).unwrap();
    let mut set = TagSet::from_map(&mut map).unwrap();
    let max = max_metadata_size(crate::KIT_METADATA_BUDGET).unwrap();
    let pin = set.pinned.as_mut().unwrap();
    pin.metadata_size += max - map.header().meta_size;
    let full = set.build().unwrap().0;
    let error = build(&full, NAME, &placed(), &recipe())
        .expect_err("must refuse")
        .to_string();
    assert!(error.contains("exceeds tag headroom by "), "{error}");
    assert!(error.contains("the biggest part is "), "{error}");
}

/// The invented donor's classes `names`, in that order, over `donor`.
fn classes<'a>(
    donor: &'a [u8],
    schema: &'a tags::schema::Schema,
    names: &[&str],
    cut_to_fit: bool,
) -> Classes<'a> {
    let all = testkit::donor_classes();
    Classes {
        schema,
        donors: vec![crate::Donor {
            name: testkit::DONOR,
            bytes: donor,
        }],
        specs: names
            .iter()
            .map(|n| all.iter().find(|c| c.name == *n).unwrap().clone())
            .collect(),
        cut_to_fit,
    }
}

#[test]
fn ground_with_classes_equals_the_kits_stage_two_build() {
    let (base, donor, schema) = (testkit::court(), testkit::donor(), testkit::donor_schema());
    let with = classes(&donor, &schema, &["grunt", "jackal"], true);
    let ground = build_with(&base, NAME, &Labels::default(), &recipe(), Some(&with)).unwrap();
    let kit = crate::build_with_classes(
        &base,
        NAME,
        Mode::Kit,
        &Config::default(),
        &Labels::default(),
        Some(&with),
    )
    .unwrap();
    assert_eq!(ground.bytes, kit.bytes);
    assert_eq!(ground.costs, kit.costs);
    assert_eq!(ground.classes, kit.classes);
    assert!(ground.cut.is_empty());
    assert_eq!(
        ground.classes.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        ["grunt", "jackal"]
    );
    // The palette: the Elite, then the classes in keep order.
    assert_eq!(ai(&ground.bytes).character_palette.len(), 3);
    // With no classes it is the stage-one ground.
    let none = build_with(&base, NAME, &placed(), &recipe(), None).unwrap();
    assert_eq!(none.bytes, build(&base, NAME, &placed(), &recipe()).unwrap().bytes);
    assert!(none.classes.is_empty() && none.cut.is_empty());
    // Over placed labels the classes come along the same way.
    let labelled = build_with(&base, NAME, &placed(), &recipe(), Some(&with)).unwrap();
    assert_eq!(labelled.zones, none.zones);
    assert_eq!(labelled.classes, ground.classes);
    // The package it installs has the same id as the kit's stage two.
    let dir = temp_dir("classes");
    let package = ground.install(&base, NAME, &dir, "ground", false).unwrap();
    let stage_two = crate::build_package_with(
        &base,
        NAME,
        Mode::Kit,
        &Config::default(),
        Some(&with),
        &dir,
        "kit",
        false,
    )
    .unwrap();
    assert_eq!(package.id, stage_two.id);
    assert_eq!(package.classes, stage_two.classes);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn ground_cuts_classes_from_the_end_to_fit() {
    let (donor, schema) = (testkit::donor(), testkit::donor_schema());
    let base = testkit::tight(&testkit::court(), 0x2000);
    let with = classes(&donor, &schema, &["grunt", "jackal", "hunter"], true);
    let ground = build_with(&base, NAME, &Labels::default(), &recipe(), Some(&with)).unwrap();
    assert_eq!(ground.cut, ["hunter"]);
    assert_eq!(
        ground.classes.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        ["grunt", "jackal"]
    );
    // Without the cut the same build is refused with each class's cost.
    let strict = classes(&donor, &schema, &["grunt", "jackal", "hunter"], false);
    let error = build_with(&base, NAME, &Labels::default(), &recipe(), Some(&strict))
        .expect_err("must refuse")
        .to_string();
    assert!(error.contains("hunter "), "{error}");
}
