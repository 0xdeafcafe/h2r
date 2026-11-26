//! The dropship on invented maps: no input files, no game data.
use super::*;
use crate::testkit::{self, DONOR, PHANTOM_BULK};
use crate::{Built, Classes, Config, Donor, Labels, Mode, build_with_classes};
use h1campaign::ai::h2::Area;
use tags::schema::Schema;
use std::io::Cursor;

fn kit(
    base: &[u8],
    dropship: Dropship,
    classes: Option<&Classes<'_>>,
) -> Result<Built> {
    build_with_classes(
        base,
        testkit::NAME,
        Mode::Kit,
        &Config {
            dropship,
            ..Config::default()
        },
        &Labels::default(),
        classes,
    )
}

/// The donor's classes with the first `names` of the three, whose map also
/// holds the Phantom.
fn classes<'a>(
    donor: &'a [u8],
    schema: &'a tags::schema::Schema,
    names: &[&str],
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
        cut_to_fit: true,
    }
}

/// What the guest reads of the ship: the placement `ff_ship` by its object
/// name, the palette row it names, and the point sets.
struct Found {
    placements: usize,
    palette_tag: String,
    position: [f32; 3],
    sets: Vec<PointSet>,
    names: usize,
    zones: Vec<Zone>,
}

fn read(bytes: &[u8]) -> Found {
    let mut map = Map::from_reader(Cursor::new(bytes)).unwrap();
    let scenario = map.scenario().unwrap().id;
    let root = map.read_tag_bytes(scenario).unwrap();
    let (placement_at, palette_at, stride) = tags::object::OBJECT_TYPES[1].placement.unwrap();
    let names = metadata_block(&mut map, &root, OBJECT_NAMES as usize, OBJECT_NAME).unwrap();
    let ship = names
        .iter()
        .find(|n| n.starts_with(SHIP_NAME.as_bytes()) && n[SHIP_NAME.len()] == 0)
        .expect("an object named ff_ship");
    assert_eq!(u16::from_le_bytes([ship[0x20], ship[0x21]]), 1, "a vehicle");
    let at = usize::from(u16::from_le_bytes([ship[0x22], ship[0x23]]));
    let placements = metadata_block(&mut map, &root, placement_at.into(), stride.into()).unwrap();
    let place = &placements[at];
    let f = |o: usize| f32::from_le_bytes(place[o..o + 4].try_into().unwrap());
    let row = usize::from(u16::from_le_bytes([place[0], place[1]]));
    // The placement names itself, and its flag says it is there at the start.
    assert_eq!(u16::from_le_bytes([place[2], place[3]]) as usize, names.iter().position(|n| std::ptr::eq(n, ship)).unwrap());
    assert_eq!(u32::from_le_bytes(place[4..8].try_into().unwrap()) & 1, 0);
    let palette = metadata_block(&mut map, &root, palette_at.into(), PALETTE).unwrap();
    let tag = TagId(u32::from_le_bytes(palette[row][4..8].try_into().unwrap()));
    let ai = ScenarioAi::read_map(&mut map).unwrap();
    Found {
        placements: placements.len(),
        palette_tag: map.tag(tag).unwrap().name.clone().unwrap(),
        position: [f(8), f(12), f(16)],
        sets: ai.point_sets,
        names: names.len(),
        zones: ai.zones,
    }
}

#[test]
fn the_kit_places_one_parked_ship_and_a_path_per_clear_zone() {
    let (base, donor, schema) = (testkit::court(), testkit::donor(), testkit::donor_schema());
    let with = classes(&donor, &schema, &[]);
    let built = kit(&base, Dropship::Auto, Some(&with)).unwrap();
    let report = built.ship.clone().expect("a dropship report");
    assert!(report.kept, "{report}");
    assert_eq!(report.tag, PHANTOM);
    // The donor's Phantom is one tag; the placement and the paths add more.
    assert_eq!(report.tags, 1);
    assert!(report.bytes > PHANTOM_BULK, "{report}");
    assert_eq!(built.costs.ship, report.bytes);
    assert_eq!(built.costs.classes, 0);

    let got = read(&built.bytes);
    assert_eq!(got.palette_tag, PHANTOM);
    assert_eq!((got.placements, got.names), (1, 1));
    assert!(report.paths >= 1 && report.paths <= built.zones, "{report}");
    assert_eq!(got.sets.len(), report.paths);
    for set in &got.sets {
        let zone: usize = set.name.strip_prefix(PATH_PREFIX).unwrap().parse().unwrap();
        assert!(zone < built.zones, "{}", set.name);
        let names: Vec<_> = set.points.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, POINTS);
        let [run_in, hover, run_out] = [0, 1, 2].map(|i| set.points[i].position);
        // Hover is the drop height over the zone's mean, the runs are
        // outside the play area and above it.
        let mean = got.zones[zone].areas[0].mean;
        assert_eq!(hover, [mean[0], mean[1], mean[2] + DROP_HEIGHT]);
        for run in [run_in, run_out] {
            assert!(run[2] > hover[2], "{}", set.name);
            assert!(libm::hypotf(run[0] - hover[0], run[1] - hover[1]) > RUN_MARGIN);
        }
    }
    // Parked at the first path's run-in.
    assert_eq!(got.position, got.sets[0].points[0].position);
}

#[test]
fn a_map_holding_the_spirit_uses_it_without_a_donor() {
    let base = testkit::court_with_spirit();
    let built = kit(&base, Dropship::Auto, None).unwrap();
    let report = built.ship.clone().unwrap();
    assert!(report.kept, "{report}");
    assert_eq!((report.tag.as_str(), report.tags), (SPIRIT[0], 0));
    let got = read(&built.bytes);
    assert_eq!(got.palette_tag, SPIRIT[0]);
    assert_eq!(got.sets.len(), report.paths);
    // The Phantom was never asked for: the map is not asked to hold it.
    let error = kit(&testkit::court(), Dropship::Spirit, None)
        .err()
        .expect("no Spirit in the map")
        .to_string();
    assert!(error.contains("spirit"), "{error}");
}

fn zone(mean: V3) -> Zone {
    Zone {
        name: "ff_drop_00".into(),
        flags: 0,
        manual_bsp: 0,
        firing_positions: Vec::new(),
        areas: vec![Area {
            name: "a".into(),
            flags: 0,
            mean,
            reference_frame: -1,
            deviation: 0.0,
            first: 0,
            count: 0,
            clusters: [0; 16],
            manual_reference_frame: -1,
        }],
    }
}

/// A square of two triangles at height `z`.
fn square(z: f32, lo: f32, hi: f32) -> [[V3; 3]; 2] {
    let p = |x, y| [x, y, z];
    [
        [p(lo, lo), p(hi, lo), p(hi, hi)],
        [p(lo, lo), p(hi, hi), p(lo, hi)],
    ]
}

#[test]
fn a_zone_under_a_roof_gets_no_path() {
    // A 100-unit floor with a roof 10 over its south-west corner.
    let tris: Vec<_> = square(0.0, 0.0, 100.0)
        .into_iter()
        .chain(square(10.0, 0.0, 20.0))
        .collect();
    let walls = Walls::new(tris);
    let under = zone([10.0, 10.0, 0.0]);
    let open = zone([80.0, 80.0, 0.0]);
    let runs = plan(&walls, &[under, open]);
    assert_eq!(runs[0], None, "the roof is over the first zone");
    let run = runs[1].expect("the open zone has a path");
    assert_eq!(run.points[1], [80.0, 80.0, DROP_HEIGHT]);
    // The runs start above the tallest collision, outside the floor.
    for p in [run.points[0], run.points[2]] {
        assert!(p[2] >= 10.0 + SKY_CLEARANCE - 1e-3, "{p:?}");
        assert!(p[..2].iter().any(|c| !(0.0..=100.0).contains(c)), "{p:?}");
    }
    // A zone with no collision at all has none either.
    assert_eq!(plan(&Walls::new(Vec::new()), &[zone([0.0; 3])]), [None]);
}

#[test]
fn the_ship_is_cut_before_a_class_when_headroom_is_short() {
    let (base, donor, schema) = (testkit::court(), testkit::donor(), testkit::donor_schema());
    let with = classes(&donor, &schema, &["grunt", "jackal"]);
    let none = kit(&base, Dropship::None, Some(&with)).unwrap();
    let both = kit(&base, Dropship::Auto, Some(&with)).unwrap();
    assert!(both.ship.as_ref().unwrap().kept);
    assert!(both.costs.tag_growth >= none.costs.tag_growth + PHANTOM_BULK as u32);
    // Room for the classes and a little more, not for the ship.
    let plain = crate::build(&base, testkit::NAME, Mode::Kit, &Config::default()).unwrap();
    let room = none.costs.tag_growth - plain.costs.tag_growth + 0x1000;
    let tight = testkit::tight(&base, room);
    let built = kit(&tight, Dropship::Auto, Some(&with)).unwrap();
    assert!(built.cut.is_empty(), "classes cut: {:?}", built.cut);
    assert_eq!(built.classes.len(), 2);
    let report = built.ship.unwrap();
    assert!(!report.kept);
    assert_eq!(report.why.as_deref(), Some("over the tag headroom"));
    assert_eq!(report.to_string(), "dropship: cut (over the tag headroom)");
    // The cut kit carries no ship, placement or path.
    let mut map = Map::from_reader(Cursor::new(&built.bytes[..])).unwrap();
    assert!(map.tag(PHANTOM).is_err());
    assert!(ScenarioAi::read_map(&mut map).unwrap().point_sets.is_empty());
}

#[test]
fn no_dropship_leaves_the_kit_as_today() {
    let base = testkit::court();
    let today = crate::build(&base, testkit::NAME, Mode::Kit, &Config::default()).unwrap();
    // Nothing to build a ship from: `auto` is the kit as it was, with a
    // report that says why.
    let auto = kit(&base, Dropship::Auto, None).unwrap();
    assert_eq!(auto.bytes, today.bytes);
    assert!(!auto.ship.unwrap().kept);
    // Asked for none: the same bytes, no report, even where the map holds
    // the Spirit.
    let none = kit(&base, Dropship::None, None).unwrap();
    assert_eq!(none.bytes, today.bytes);
    assert_eq!(none.ship, None);
    let spirit = testkit::court_with_spirit();
    let off = kit(&spirit, Dropship::None, None).unwrap();
    assert_eq!(off.ship, None);
    assert_eq!(off.costs.ship, 0);
    let mut map = Map::from_reader(Cursor::new(&off.bytes[..])).unwrap();
    assert!(ScenarioAi::read_map(&mut map).unwrap().point_sets.is_empty());
    let scenario = map.scenario().unwrap().id;
    let root = map.read_tag_bytes(scenario).unwrap();
    assert!(metadata_block(&mut map, &root, OBJECT_NAMES as usize, OBJECT_NAME).unwrap().is_empty());
    // The costs line is the one it was.
    assert!(!off.costs.to_string().contains("dropship"));
}

#[test]
fn the_scenario_holds_the_ship_and_its_paths_by_name() {
    // What the guest's `read_kit` finds: `ff_ship` by object name,
    // `ff_path_NN` by zone number, a point set a zone.
    let built = kit(&testkit::court_with_spirit(), Dropship::Auto, None).unwrap();
    let got = read(&built.bytes);
    let zones: Vec<usize> = got
        .sets
        .iter()
        .map(|s| s.name.strip_prefix(PATH_PREFIX).unwrap().parse().unwrap())
        .collect();
    assert!(zones.windows(2).all(|w| w[0] < w[1]), "{zones:?}");
    assert!(got.zones.iter().enumerate().all(|(i, z)| z.name.starts_with("ff_drop_") == (i < built.zones)));
    assert_eq!(got.placements, 1);
}

/// A floor inside four cliff walls 40 high, 25 from the origin.
fn canyon() -> Vec<[V3; 3]> {
    let wall = |x: f32| {
        let p = |y, z| [x, y, z];
        [[p(-200.0, 0.0), p(200.0, 0.0), p(200.0, 40.0)], [p(-200.0, 0.0), p(200.0, 40.0), p(-200.0, 40.0)]]
    };
    let mut tris: Vec<_> = square(0.0, -100.0, 100.0).into_iter().collect();
    for x in [25.0, -25.0] {
        tris.extend(wall(x));
        // The same wall turned a quarter: along y at +-x.
        tris.extend(wall(x).map(|t| t.map(|v| [v[1], v[0], v[2]])));
    }
    tris
}

#[test]
fn a_column_clear_of_roofs_is_flown_steeply_when_cliffs_block_the_long_run() {
    // Cliffs 25 round, 40 high: the long shallow run from
    // the sky at 50 meets them, a dive from 30 away does not.
    let walls = Walls::new(canyon());
    let run = plan(&walls, &[zone([0.0, 0.0, 0.0])])[0].expect("a steep run gets in");
    assert_eq!(run.points[1], [0.0, 0.0, DROP_HEIGHT]);
    let along = libm::hypotf(run.points[0][0], run.points[0][1]);
    assert!(along <= STEEP_REACH[0] + 1e-3, "{along}");
}

#[test]
fn a_zone_boxed_in_by_a_roof_the_height_of_the_cliffs_has_no_path() {
    let mut tris = canyon();
    tris.extend(square(30.0, -30.0, 30.0));
    assert_eq!(plan(&Walls::new(tris), &[zone([0.0, 0.0, 0.0])]), [None]);
}
