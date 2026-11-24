//! A test map as a package (`recipe.rs`): the edits between two invented
//! scenarios, and the map the package builds. No disc bytes.
use map::CacheType;
use map::build::{TagPayload, TagSet};
use tags::defs::{Defs, Record, Val, default_record, to_built};
use tags::repo::TagKey;

use super::*;
use crate::tests::cc;

const SCENARIO: &str = "invented\\scenarios\\tiny";

fn key() -> TagKey {
    TagKey::new(cc(b"scnr"), SCENARIO)
}

fn def() -> &'static tags::defs::StructDef {
    Defs::builtin().group(cc(b"scnr")).expect("scnr is defined")
}

fn spawn(x: f32, team: i64) -> Record {
    let child = def().child("starting_locations").expect("a block");
    let mut r = default_record(child);
    *r.at_mut("position").expect("position") =
        Val::List(vec![Val::Float(x), Val::Float(2.0), Val::Float(3.0)]);
    *r.at_mut("facing").expect("facing") = Val::Float(0.5);
    *r.at_mut("team").expect("team") = Val::Int(team);
    r
}

/// An invented scenario with starting locations at `xs`.
fn scenario(xs: &[f32]) -> Record {
    let mut rec = default_record(def());
    let Some(Val::Block(elems)) = rec.at_mut("starting_locations") else {
        panic!("starting_locations is a block");
    };
    *elems = xs.iter().map(|x| spawn(*x, 1)).collect();
    rec
}

fn parsed(text: &str) -> (TagEdit, usize) {
    parse_edits(text).unwrap().remove(0)
}

#[test]
fn placements_added_at_the_end_are_a_copy_and_their_fields() {
    let text = records_edits(&key(), &scenario(&[1.0]), &scenario(&[1.0, 7.0, 8.0]))
        .unwrap()
        .expect("they differ");
    let (edit, _) = parsed(&text);
    assert_eq!(edit.tag, format!("scnr:{SCENARIO}"));
    let ops: Vec<_> = edit.block.iter().map(|b| (b.op, b.like, b.index)).collect();
    assert_eq!(ops, vec![(Op::Add, Some(0), None); 2]);
    let paths: Vec<&str> = edit.set.iter().map(|s| s.path.as_str()).collect();
    assert_eq!(
        paths,
        ["starting_locations[1].position", "starting_locations[2].position"]
    );
}

#[test]
fn placements_cleared_are_removed_from_the_end() {
    let text = records_edits(&key(), &scenario(&[1.0, 2.0, 3.0]), &scenario(&[1.0]))
        .unwrap()
        .unwrap();
    let (edit, _) = parsed(&text);
    let ops: Vec<_> = edit.block.iter().map(|b| (b.op, b.index)).collect();
    assert_eq!(ops, vec![(Op::Remove, Some(2)), (Op::Remove, Some(1))]);
    assert!(edit.set.is_empty());
}

#[test]
fn a_changed_field_is_one_set_line_and_equal_scenarios_are_none() {
    let mut after = scenario(&[1.0, 2.0]);
    *after.at_mut("starting_locations[1].team").unwrap() = Val::Int(4);
    let text = records_edits(&key(), &scenario(&[1.0, 2.0]), &after)
        .unwrap()
        .unwrap();
    let (edit, _) = parsed(&text);
    assert!(edit.block.is_empty());
    assert_eq!(edit.set.len(), 1);
    assert_eq!(edit.set[0].path, "starting_locations[1].team");
    assert_eq!(
        records_edits(&key(), &scenario(&[1.0]), &scenario(&[1.0])).unwrap(),
        None
    );
}

#[test]
fn bytes_the_definition_does_not_lay_out_stop_it_naming_the_field() {
    let before = scenario(&[1.0]);
    let mut after = before.clone();
    let (name, bytes) = after
        .fields
        .iter_mut()
        .find_map(|(n, v)| match v {
            Val::Bytes(b) if !b.is_empty() => Some((n.clone(), b)),
            _ => None,
        })
        .expect("the scenario has an unknown span");
    bytes[0] ^= 0xff;
    let why = records_edits(&key(), &before, &after).unwrap_err();
    assert!(why.contains(&name) && why.contains("bytes"), "{why}");
}

/// A map file of only its globals and a scenario of `rec`.
fn map_with(rec: &Record) -> Vec<u8> {
    let mut set = TagSet::new(CacheType::Multiplayer, "tiny", SCENARIO);
    for g in [b"matg", b"scnr"] {
        set.add_group(cc(g), None, None);
    }
    set.add_tag(
        cc(b"matg"),
        "globals\\globals",
        TagPayload::Built(map::build::Block::zeroed(0x40)),
    )
    .unwrap();
    let block = to_built(def(), rec, &mut |_| None).unwrap();
    set.add_tag(cc(b"scnr"), SCENARIO, TagPayload::Built(block))
        .unwrap();
    set.build().expect("the invented map builds").0
}

struct Disc(Vec<u8>);

impl BaseMaps for Disc {
    fn read(&self, name: &str) -> Result<Vec<u8>, String> {
        (name == "tiny")
            .then(|| self.0.clone())
            .ok_or_else(|| format!("no map {name}"))
    }
}

#[test]
fn the_package_builds_the_map_the_edits_describe() {
    let before = map_with(&scenario(&[1.0]));
    let after = map_with(&scenario(&[1.0, 7.0]));
    let edits = scenario_edits(&before, &after).unwrap().expect("they differ");
    let package = test_map_package(
        &TestMap {
            id: "test-two-spawns",
            name: "test-two-spawns",
            base: "tiny",
            title: "Two spawns",
            purpose: "An invented rig.",
        },
        &edits,
    )
    .unwrap();
    assert!(check_package(&package).is_empty(), "{:?}", check_package(&package));
    let built = build_maps(&[package], &Disc(before.clone()), &NoFresh);
    assert!(built.failed.is_empty(), "{built:?}");
    let [m] = built.maps.as_slice() else {
        panic!("one map: {built:?}")
    };
    assert_eq!(m.dir, "test-two-spawns");
    assert_eq!(m.manifest.package.name, "Test: Two spawns");
    // The map has the second scenario's placements, and not the base's.
    let bytes = &m.files[0].1;
    assert_eq!(scenario_edits(bytes, &after).unwrap(), None);
    assert!(scenario_edits(&before, bytes).unwrap().is_some());
}

#[test]
fn a_map_with_no_scenario_edits_is_named() {
    let base = map_with(&scenario(&[1.0]));
    let why = scenario_edits(&base[..16], &base).unwrap_err();
    assert!(why.starts_with("the base map"), "{why}");
}
