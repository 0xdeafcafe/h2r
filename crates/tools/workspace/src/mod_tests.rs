//! AS6 (docs/engine/asset-store.md, "Build slices"): mod patches on the
//! invented map: a `set` line's `from`, `replaces`, the `tag-delta` item, the
//! conflict table, forks and the store's own bytes. No disc bytes.
use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;

use content::Sha256;
use content::package::{Kind, Lock, Manifest, Package};
use content::patch::write::bps;
use map::Map;
use map::build::{RawBlob, TagPayload, TagSet};
use tags::defs::{StructDef, Val, from_built};
use vfs::store_map::Levels as _;

use crate::assets::Assets as _;
use crate::level::{StoreLevels, assemble, level_id};
use crate::level_tests::{put_store, read_image, store, temp};
use crate::tests::{A, ADD_C, ADD_C_FILE, C, adds_c, base, cc, edit_text, edits, pkg, target};
use crate::{
    BuiltMap, Loaded, MapUnpin, Report, Served, Unpin, build, build_map, build_map_settled,
    check_package, check_set, delta::image, overrides_whole, store_findings, tag_hash,
};

const MISSING: &str = "invented\\dmg\\nowhere";
const OTHER: &str = "1111111111111111111111111111111111111111111111111111111111111111";

// ------------------------------------------------------------- packages

fn head(id: &str) -> String {
    format!("[package]\nformat = 1\nid = \"{id}\"\nname = \"{id}\"\n")
}

/// A package from its manifest's text and the files it names.
fn package(text: &str, files: &[(&str, &[u8])]) -> Loaded {
    let manifest = Manifest::from_toml(text).unwrap();
    let files: BTreeMap<String, Vec<u8>> = files
        .iter()
        .map(|(n, b)| ((*n).to_owned(), b.to_vec()))
        .collect();
    let lock = Lock::new(
        files
            .iter()
            .map(|(n, b)| (n.clone(), Sha256::of(b)))
            .collect(),
    );
    Loaded {
        package: Package::new(manifest, lock).unwrap(),
        files,
    }
}

/// A mod of one `edits` item for every map.
fn a_mod(id: &str, requires: &[&str], edits: &str) -> Loaded {
    let requires = if requires.is_empty() {
        String::new()
    } else {
        format!("requires = {requires:?}\n")
    };
    package(
        &format!(
            "{}{requires}[[item]]\nkind = \"edits\"\nsource = \"edits.yaml\"\n",
            head(id)
        ),
        &[("edits.yaml", edits.as_bytes())],
    )
}

/// One `set` line, with the value it replaces when `from` is given.
fn line(path: &str, from: Option<f32>, value: f32) -> String {
    let from = from.map_or(String::new(), |f| format!("from: {f}, "));
    format!("- {{path: {path}, {from}value: {value}}}\n")
}

/// An edit file of one entry, on `A`, made on the asset `was` names.
fn entry(was: Option<&str>, line: &str) -> String {
    let was = was.map_or(String::new(), |w| format!("  was: {w}\n"));
    format!("- tag: 'jpt!:{A}'\n{was}  set:\n    {line}")
}

/// An edit file of one entry on a key the map lacks.
fn missing() -> String {
    format!(
        "- tag: 'jpt!:{MISSING}'\n  set:\n    {}",
        line("damage_lower_bound", None, 1.0)
    )
}

/// An authored override of `A` whole: the asset with its lower bound at 7.
fn an_override(id: &str, replaces: &str) -> Loaded {
    let source = format!("like: 'jpt!:{A}'\nfields:\n  damage_lower_bound: 7.0\n");
    package(
        &format!(
            "{}[[item]]\nkind = \"tag\"\nsource = 'a.jpt!.yaml'\ntag = '{A}'\nreplaces = \"{replaces}\"\n",
            head(id)
        ),
        &[("a.jpt!.yaml", source.as_bytes())],
    )
}

/// A `tag-delta` of `A`: the item's patch, and the asset it was made on.
fn a_delta(patch: &[u8], was: Option<&str>) -> Loaded {
    let was = was.map_or(String::new(), |w| format!("was = \"{w}\"\n"));
    package(
        &format!(
            "{}[[item]]\nkind = \"tag-delta\"\nsource = \"a.jpt!.bps\"\ntag = '{A}'\n{was}",
            head("m")
        ),
        &[("a.jpt!.bps", patch)],
    )
}

fn applied(report: &Report) -> Vec<&str> {
    report.packages.iter().map(|(id, _)| id.as_str()).collect()
}

fn off(report: &Report) -> Vec<String> {
    report
        .refused
        .iter()
        .map(|r| format!("{}: {}", r.package, r.why))
        .collect()
}

fn refused(report: &Report) -> Vec<&str> {
    report.refused.iter().map(|r| r.package.as_str()).collect()
}

// ---------------------------------------------------------------- the map

fn def() -> &'static StructDef {
    crate::edit::def_of(cc(b"jpt!")).unwrap()
}

/// A field of `A` in a set of built tags.
fn field(set: &TagSet, path: &str) -> Val {
    let id = set.find(cc(b"jpt!"), A).expect("the tag is in the set");
    let TagPayload::Built(block) = &set.tag(id).unwrap().payload else {
        panic!("not built");
    };
    from_built(def(), block)
        .unwrap()
        .at(path)
        .cloned()
        .expect("the field")
}

/// A field of `A` in a map's bytes.
fn field_of(bytes: &[u8], path: &str) -> Val {
    from_built(def(), &tree_of(bytes))
        .unwrap()
        .at(path)
        .cloned()
        .expect("the field")
}

/// `A` of the map `bytes`, laid out anew as a build reads it.
fn tree_of(bytes: &[u8]) -> map::build::Block {
    let mut map = Map::from_reader(Cursor::new(bytes)).unwrap();
    let unpin = MapUnpin::new(&mut map).unwrap();
    let mut set = TagSet::from_map(&mut map).unwrap();
    let id = set.find(cc(b"jpt!"), A).expect("the tag is in the map");
    unpin.tree(id).unwrap()
}

/// The invented map as the game reads it.
fn map_bytes() -> Vec<u8> {
    base().build().expect("the invented map builds").0
}

/// The asset hash of `A` in the map `bytes`, as a build finds it.
fn asset_in(bytes: &[u8]) -> String {
    let mut map = Map::from_reader(Cursor::new(bytes)).unwrap();
    let set = TagSet::from_map(&mut map).unwrap();
    tag_hash(&set, set.find(cc(b"jpt!"), A).unwrap())
        .unwrap()
        .to_string()
}

/// The asset hash of `A` once `over` replaced it, as the build records it.
fn asset_after(bytes: &[u8], over: &Loaded) -> String {
    let mut map = Map::from_reader(Cursor::new(bytes)).unwrap();
    let unpin = MapUnpin::new(&mut map).ok();
    let mut set = TagSet::from_map(&mut map).unwrap();
    set.check_tag_memory = false;
    if let Some(p) = set.pinned.as_mut() {
        p.clear_index_tail();
    }
    let unpin = unpin.as_ref().map(|u| u as &dyn Unpin);
    build(&mut set, std::slice::from_ref(over), &target(), unpin).unwrap();
    tag_hash(&set, set.find(cc(b"jpt!"), A).unwrap())
        .unwrap()
        .to_string()
}

fn has_tag(bytes: &[u8], group: &[u8; 4], name: &str) -> bool {
    let map = Map::from_reader(Cursor::new(bytes)).unwrap();
    map.tags()
        .iter()
        .any(|t| t.group == cc(group) && t.name.as_deref() == Some(name))
}

/// The map with the set laid over it as the game does it.
fn settled(bytes: &[u8], packages: &[Loaded]) -> Result<BuiltMap, String> {
    build_map_settled(bytes, packages, &target(), &BTreeSet::new(), None)
}

/// The invented map with a raw resource and `A`'s lower bound at `value`.
fn map_with(value: f32) -> (Vec<u8>, (u64, u64)) {
    let mut set = base();
    build(
        &mut set,
        &[edits("tmp", &[], &edit_text(A, value))],
        &target(),
        None,
    )
    .unwrap();
    set.raw.push(RawBlob {
        bytes: (0..0x1800u32).map(|i| (i * 7 + i / 251) as u8).collect(),
        align: 0x10,
    });
    let (bytes, layout) = set.build().expect("the invented map builds");
    (bytes, (u64::from(layout.raw[0]), 0x1800))
}

// ------------------------------------------------------------ `from`, `replaces`

#[test]
fn a_from_is_checked_against_the_tag_as_the_build_found_it() {
    let one = edits("one", &[], &edit_text(A, 1.0));
    let two = a_mod(
        "two",
        &[],
        &entry(None, &line("damage_lower_bound", Some(0.0), 2.0)),
    );
    let mut set = base();
    let report = build(&mut set, &[one, two], &target(), None).unwrap();
    // The later line replaces what the store holds (0), not what `one` left:
    // its value wins and the override is listed.
    assert_eq!(field(&set, "damage_lower_bound"), Val::Float(2.0));
    assert_eq!(report.overrides.len(), 1);
    assert_eq!(
        (
            report.overrides[0].earlier.as_str(),
            report.overrides[0].later.as_str()
        ),
        ("one", "two")
    );

    // A `from` the field does not hold stops `h2 package build` with the line.
    let other = a_mod(
        "m",
        &[],
        &entry(None, &line("damage_lower_bound", Some(9.0), 2.0)),
    );
    let err = build(&mut base(), &[other], &target(), None).unwrap_err();
    assert_eq!(err.line, Some(3), "{err}");
    assert!(
        err.why
            .contains("`from` damage_lower_bound: is 9.0, but the field is 0"),
        "{err}"
    );
    // A `from` on a path that does not resolve is the line's own error.
    let bad_path = a_mod(
        "m",
        &[],
        &entry(None, &line("no_such_field", Some(9.0), 2.0)),
    );
    let err = build(&mut base(), &[bad_path], &target(), None).unwrap_err();
    assert!(err.why.contains("no_such_field"), "{err}");
}

#[test]
fn a_replaces_item_takes_the_asset_whole_when_the_map_holds_that_asset() {
    let set = base();
    let a = tag_hash(&set, set.find(cc(b"jpt!"), A).unwrap())
        .unwrap()
        .to_string();
    let over = an_override("m", &a);
    let mut built = set.clone();
    let report = build(&mut built, &[over], &target(), None).unwrap();
    assert!(report.added.is_empty());
    assert_eq!(
        report
            .changed
            .iter()
            .map(|k| k.path.as_str())
            .collect::<Vec<_>>(),
        [A]
    );
    assert_eq!(field(&built, "damage_lower_bound"), Val::Float(7.0));
    // It is the same tag (what names it still reaches it) with other content.
    let id = set.find(cc(b"jpt!"), A).unwrap();
    assert_eq!(built.find(cc(b"jpt!"), A), Some(id));
    assert_ne!(tag_hash(&built, id), tag_hash(&set, id));

    // For other content: the asset the map holds is not the one it replaces.
    let stranger = an_override("m", OTHER);
    let err = build(&mut base(), &[stranger], &target(), None).unwrap_err();
    assert!(
        err.why.contains(&format!(
            "it replaces the asset with hash {OTHER}, but the map's invented\\dmg\\a.jpt! is {a}"
        )),
        "{err}"
    );
    // A key the map lacks.
    let text = format!(
        "{}[[item]]\nkind = \"tag\"\nsource = 'a.jpt!.yaml'\ntag = '{MISSING}'\nreplaces = \"{a}\"\n",
        head("m")
    );
    let gone = package(&text, &[("a.jpt!.yaml", b"fields: {}\n")]);
    let err = build(&mut base(), &[gone], &target(), None).unwrap_err();
    assert!(
        err.why.contains("the map has no tag") && err.why.contains(MISSING),
        "{err}"
    );

    // A kind whose builder writes through the map: `check` refuses `replaces`.
    assert!(overrides_whole(Kind::Tag));
    assert!(!overrides_whole(Kind::Bitmap));
    let bitmap = package(
        &format!(
            "{}[[item]]\nkind = \"bitmap\"\nsource = \"p.png\"\ntag = 'invented\\ui\\pic'\nreplaces = \"{a}\"\n",
            head("m")
        ),
        &[("p.png", b"not a png")],
    );
    let found = check_package(&bitmap);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].what.contains("`replaces`") && found[0].what.contains("bitmap builder"),
        "{found:?}"
    );
}

// --------------------------------------------------------- the conflict table

#[test]
fn different_fields_of_one_asset_both_apply_and_the_same_field_goes_to_the_later() {
    let bytes = map_bytes();
    let one = a_mod(
        "a-lower",
        &[],
        &entry(None, &line("damage_lower_bound", None, 1.0)),
    );
    let two = a_mod(
        "b-cutoff",
        &[],
        &entry(None, &line("cutoff_scale", None, 2.5)),
    );
    let built = settled(&bytes, &[one.clone(), two]).unwrap();
    assert!(built.report.refused.is_empty());
    assert_eq!(applied(&built.report), ["a-lower", "b-cutoff"]);
    assert_eq!(
        field_of(&built.bytes, "damage_lower_bound"),
        Val::Float(1.0)
    );
    assert_eq!(field_of(&built.bytes, "cutoff_scale"), Val::Float(2.5));

    // The same field: the later package's value, listed with both names.
    let two = a_mod(
        "b-lower",
        &[],
        &entry(None, &line("damage_lower_bound", None, 2.0)),
    );
    let built = settled(&bytes, &[one, two]).unwrap();
    assert!(built.report.refused.is_empty());
    assert_eq!(
        field_of(&built.bytes, "damage_lower_bound"),
        Val::Float(2.0)
    );
    let [o] = &built.report.overrides[..] else {
        panic!("{:?}", built.report.overrides);
    };
    assert_eq!(
        (o.earlier.as_str(), o.later.as_str()),
        ("a-lower", "b-lower")
    );
    assert_eq!(o.path, "damage_lower_bound");
}

#[test]
fn a_from_was_or_replaces_that_does_not_match_leaves_its_package_off_with_the_reason() {
    let bytes = map_bytes();
    let a = asset_in(&bytes);
    let good = a_mod(
        "a-good",
        &[],
        &entry(None, &line("damage_lower_bound", None, 1.0)),
    );
    let from = a_mod(
        "b-from",
        &[],
        &entry(None, &line("damage_lower_bound", Some(9.0), 5.0)),
    );
    let was = a_mod(
        "c-was",
        &[],
        &entry(Some(OTHER), &line("damage_lower_bound", None, 5.0)),
    );
    let replaces = an_override("d-replaces", OTHER);
    let built = settled(
        &bytes,
        &[from.clone(), was.clone(), replaces.clone(), good.clone()],
    )
    .unwrap();
    // The other package still applies, and the map is what it gives alone.
    assert_eq!(applied(&built.report), ["a-good"]);
    assert_eq!(refused(&built.report), ["b-from", "c-was", "d-replaces"]);
    let why = off(&built.report);
    assert!(
        why[0].contains("`from` damage_lower_bound: is 9.0, but the field is 0"),
        "{why:?}"
    );
    assert!(
        why[1].contains("stale edit") && why[1].contains(OTHER) && why[1].contains(&a),
        "{why:?}"
    );
    assert!(
        why[2].contains("it replaces the asset with hash")
            && why[2].contains(OTHER)
            && why[2].contains(&a),
        "{why:?}"
    );
    assert!(built.bytes == settled(&bytes, &[good]).unwrap().bytes);
    // Under `h2 package build` each stops the build with its item.
    let said = |p: &[Loaded]| build_map(&bytes, p, &target()).unwrap_err();
    assert!(
        said(&[from.clone()]).contains("edits.yaml, line 3"),
        "{}",
        said(&[from.clone()])
    );
    assert!(
        said(&[was.clone()]).contains("edits.yaml, line 4"),
        "{}",
        said(&[was.clone()])
    );
    let third = said(&[replaces]);
    assert!(
        third.contains("a.jpt!.yaml") && third.contains("it replaces the asset"),
        "{third}"
    );
}

#[test]
fn a_patch_of_an_asset_an_earlier_package_overrides_whole_needs_its_hash_and_that_package() {
    let bytes = map_bytes();
    let a = asset_in(&bytes);
    let over = an_override("a-over", &a);
    let after = asset_after(&bytes, &over);
    assert_ne!(after, a);
    let stranger = a_mod(
        "b-stranger",
        &[],
        &entry(None, &line("cutoff_scale", None, 2.0)),
    );
    let old_base = a_mod(
        "b-oldbase",
        &[],
        &entry(Some(&a), &line("cutoff_scale", None, 2.0)),
    );
    let unrequired = a_mod(
        "b-unrequired",
        &[],
        &entry(Some(&after), &line("cutoff_scale", None, 2.0)),
    );
    let friend = a_mod(
        "c-friend",
        &["a-over"],
        &entry(Some(&after), &line("cutoff_scale", None, 2.0)),
    );
    let built = settled(&bytes, &[over, stranger, old_base, unrequired, friend]).unwrap();
    assert_eq!(applied(&built.report), ["a-over", "c-friend"]);
    assert_eq!(
        refused(&built.report),
        ["b-stranger", "b-oldbase", "b-unrequired"]
    );
    let why = off(&built.report);
    for (i, w) in why.iter().enumerate() {
        assert!(
            w.contains("\"a-over\"") && w.contains("overrides whole"),
            "{i}: {w}"
        );
    }
    assert!(
        why[0].contains("must name that hash and the package must require"),
        "{why:?}"
    );
    assert!(
        why[1].contains("must name that hash and the package must require"),
        "{why:?}"
    );
    assert!(why[2].contains("its `was` must name that hash"), "{why:?}");
    // The override stands, and the patch made on it applies over it.
    assert_eq!(
        field_of(&built.bytes, "damage_lower_bound"),
        Val::Float(7.0)
    );
    assert_eq!(field_of(&built.bytes, "cutoff_scale"), Val::Float(2.0));
}

#[test]
fn a_whole_override_after_a_patch_takes_the_asset_and_lists_the_field() {
    let bytes = map_bytes();
    let a = asset_in(&bytes);
    let patch = a_mod(
        "a-patch",
        &[],
        &entry(None, &line("damage_lower_bound", None, 1.0)),
    );
    let over = an_override("b-over", &a);
    let built = settled(&bytes, &[patch, over]).unwrap();
    assert!(built.report.refused.is_empty());
    assert_eq!(
        field_of(&built.bytes, "damage_lower_bound"),
        Val::Float(7.0)
    );
    let [o] = &built.report.overrides[..] else {
        panic!("{:?}", built.report.overrides);
    };
    assert_eq!(
        (o.earlier.as_str(), o.later.as_str()),
        ("a-patch", "b-over")
    );
    assert_eq!(o.path, "damage_lower_bound");
}

#[test]
fn two_packages_that_override_one_asset_whole_or_add_one_key_leave_the_later_off() {
    let bytes = map_bytes();
    let a = asset_in(&bytes);
    let one = an_override("a-one", &a);
    let two = an_override("b-two", &a);
    let built = settled(&bytes, &[one.clone(), two.clone()]).unwrap();
    assert_eq!(applied(&built.report), ["a-one"]);
    let why = off(&built.report);
    assert_eq!(why.len(), 1);
    assert!(
        why[0].contains("already overrides") && why[0].contains("\"a-one\""),
        "{why:?}"
    );
    assert_eq!(
        field_of(&built.bytes, "damage_lower_bound"),
        Val::Float(7.0)
    );

    let c_one = adds_c("a-c");
    let c_two = adds_c("b-c");
    let built = settled(&bytes, &[c_one.clone(), c_two.clone()]).unwrap();
    assert_eq!(applied(&built.report), ["a-c"]);
    let why = off(&built.report);
    assert_eq!(why.len(), 1);
    assert!(
        why[0].contains("already added by") && why[0].contains("package \"a-c\" item 0"),
        "{why:?}"
    );

    // `h2 package check` names both before any map is built.
    let found = check_set(&[one.clone(), two, c_one.clone(), c_two]);
    assert_eq!(found.len(), 2, "{found:?}");
    let said: Vec<String> = found
        .iter()
        .map(|f| format!("{}: {}", f.package, f.what))
        .collect();
    assert!(
        said[0].contains("b-two") && said[0].contains("overrides") && said[0].contains("\"a-one\""),
        "{said:?}"
    );
    assert!(
        said[1].contains("b-c") && said[1].contains("adds tag") && said[1].contains("\"a-c\""),
        "{said:?}"
    );
    // Two overrides of one asset for maps that cannot meet are no conflict.
    let scoped = package(
        &format!(
            "{}[[item]]\nkind = \"tag\"\nsource = 'a.jpt!.yaml'\ntag = '{A}'\nreplaces = \"{a}\"\nmaps = \"campaign\"\n",
            head("b-two")
        ),
        &[("a.jpt!.yaml", b"fields: {}\n")],
    );
    assert!(check_set(&[one, scoped]).is_empty());
}

#[test]
fn a_patch_whose_key_is_missing_is_off_with_the_key_and_nothing_of_its_package_applies() {
    let bytes = map_bytes();
    let gone = a_mod("a-missing", &[], &missing());
    let built = settled(&bytes, std::slice::from_ref(&gone)).unwrap();
    let why = off(&built.report);
    assert_eq!(why.len(), 1);
    assert!(
        why[0].contains("the map has no tag") && why[0].contains(MISSING),
        "{why:?}"
    );

    // Never in part: its first item added a tag, its second is refused.
    let late = package(
        &format!(
            "{}[[item]]\nkind = \"tag\"\nsource = \"c.jpt!.yaml\"\ntag = '{C}'\n\n[[item]]\nkind = \"edits\"\nsource = \"e.yaml\"\n",
            head("b-late")
        ),
        &[
            (ADD_C_FILE, ADD_C.as_bytes()),
            ("e.yaml", missing().as_bytes()),
        ],
    );
    let built = settled(&bytes, std::slice::from_ref(&late)).unwrap();
    assert_eq!(refused(&built.report), ["b-late"]);
    assert!(
        !has_tag(&built.bytes, b"jpt!", C),
        "the package's first item is not in the map"
    );
    assert!(built.bytes == settled(&bytes, &[]).unwrap().bytes);
}

#[test]
fn a_package_that_requires_one_off_for_the_map_is_off_too_and_names_it() {
    let bytes = map_bytes();
    let bad = a_mod("a-bad", &[], &missing());
    let child = a_mod(
        "b-child",
        &["a-bad"],
        &entry(None, &line("damage_lower_bound", None, 1.0)),
    );
    let grandchild = a_mod(
        "c-grandchild",
        &["b-child"],
        &entry(None, &line("cutoff_scale", None, 1.0)),
    );
    let built = settled(&bytes, &[bad, child, grandchild]).unwrap();
    assert!(applied(&built.report).is_empty());
    let why = off(&built.report);
    assert_eq!(refused(&built.report), ["a-bad", "b-child", "c-grandchild"]);
    assert!(
        why[1].contains("requires \"a-bad\"") && why[1].contains("no tag"),
        "{why:?}"
    );
    assert!(why[2].contains("requires \"b-child\""), "{why:?}");
    assert_eq!(
        field_of(&built.bytes, "damage_lower_bound"),
        Val::Float(0.0)
    );
    assert_eq!(field_of(&built.bytes, "cutoff_scale"), Val::Float(0.0));
}

#[test]
fn a_refusal_of_a_package_of_ours_stops_the_build() {
    let bytes = map_bytes();
    let bad = a_mod("a-bad", &[], &missing());
    let strict: BTreeSet<String> = ["a-bad".to_owned()].into();
    let why = build_map_settled(&bytes, std::slice::from_ref(&bad), &target(), &strict, None)
        .unwrap_err();
    assert!(
        why.contains("package \"a-bad\"") && why.contains(MISSING),
        "{why}"
    );
    // `h2 package build` holds every package to it.
    let err = build_map(&bytes, &[bad], &target()).unwrap_err();
    assert!(err.contains("the map has no tag"), "{err}");
    // A failure that is no refusal stops the build either way.
    let broken = pkg(
        "a-broken",
        &[],
        &[("edits", "e.yaml", None, "all")],
        &[(
            "e.yaml",
            b"- tag: 'jpt!:invented\\dmg\\a'\n  set:\n    - {path: no_such_field, value: 1}\n",
        )],
    );
    for strict in [&BTreeSet::new(), &strict] {
        let err = build_map_settled(
            &bytes,
            std::slice::from_ref(&broken),
            &target(),
            strict,
            None,
        )
        .unwrap_err();
        assert!(
            err.contains("no_such_field") && err.contains("line 3"),
            "{err}"
        );
    }
}

#[test]
fn a_was_and_a_replaces_name_the_asset_hash_of_the_base() {
    let bytes = map_bytes();
    let key = tags::repo::TagKey::new(cc(b"jpt!"), A);
    let asset = crate::assets::OfMap::of_map(&bytes)
        .hash(&key)
        .unwrap()
        .expect("the base holds the asset");
    // The tag's own hash is another thing.
    assert_ne!(asset.to_string(), asset_in(&bytes));
    let with = |p: &[Loaded]| {
        build_map_settled(
            &bytes,
            p,
            &target(),
            &BTreeSet::new(),
            Some(&mut crate::assets::OfMap::of_map(&bytes)),
        )
    };
    let patch = a_mod(
        "a-patch",
        &[],
        &entry(
            Some(&asset.to_string()),
            &line("damage_lower_bound", None, 1.0),
        ),
    );
    let built = with(std::slice::from_ref(&patch)).unwrap();
    assert!(
        built.report.refused.is_empty(),
        "{:?}",
        built.report.refused
    );
    assert_eq!(
        field_of(&built.bytes, "damage_lower_bound"),
        Val::Float(1.0)
    );
    // The tag's own hash names no asset here.
    let own = a_mod(
        "a-own",
        &[],
        &entry(
            Some(&asset_in(&bytes)),
            &line("damage_lower_bound", None, 1.0),
        ),
    );
    let built = with(std::slice::from_ref(&own)).unwrap();
    assert_eq!(refused(&built.report), ["a-own"]);
    assert!(built.report.refused[0].why.contains("stale edit"));
    // A whole override names the asset's hash, and leaves one a later patch names.
    let over = an_override("a-over", &asset.to_string());
    let built = with(std::slice::from_ref(&over)).unwrap();
    assert!(
        built.report.refused.is_empty(),
        "{:?}",
        built.report.refused
    );
    let [replaced] = &built.report.replaced[..] else {
        panic!("{:?}", built.report.replaced);
    };
    assert_eq!(
        (replaced.key.path.as_str(), replaced.by.as_str()),
        (A, "a-over")
    );
    assert_ne!(replaced.hash.to_string(), asset.to_string());
    let friend = a_mod(
        "b-friend",
        &["a-over"],
        &entry(
            Some(&replaced.hash.to_string()),
            &line("cutoff_scale", None, 2.0),
        ),
    );
    let built = with(&[over, friend]).unwrap();
    assert!(
        built.report.refused.is_empty(),
        "{:?}",
        built.report.refused
    );
    assert_eq!(
        field_of(&built.bytes, "damage_lower_bound"),
        Val::Float(7.0)
    );
    assert_eq!(field_of(&built.bytes, "cutoff_scale"), Val::Float(2.0));
}

// ------------------------------------------------------------- `tag-delta`

#[test]
fn a_delta_applied_to_its_base_gives_the_changed_tag_and_to_another_base_is_refused() {
    // A built tag, and the same tag with its lower bound changed.
    let set = base();
    let mut edited = base();
    build(
        &mut edited,
        &[edits("tmp", &[], &edit_text(A, 3.0))],
        &target(),
        None,
    )
    .unwrap();
    let tree = |set: &TagSet| {
        let id = set.find(cc(b"jpt!"), A).unwrap();
        let TagPayload::Built(block) = &set.tag(id).unwrap().payload else {
            panic!("built");
        };
        image(set, block).unwrap()
    };
    let (before, after) = (tree(&set), tree(&edited));
    let patch = bps(&before, &after);
    // The patch holds the change, not the asset.
    assert!(
        patch.len() < before.len() / 2,
        "{} {}",
        patch.len(),
        before.len()
    );
    let a = tag_hash(&set, set.find(cc(b"jpt!"), A).unwrap())
        .unwrap()
        .to_string();
    let mut built = set.clone();
    let report = build(&mut built, &[a_delta(&patch, Some(&a))], &target(), None).unwrap();
    assert!(report.added.is_empty() && report.refused.is_empty());
    assert_eq!(field(&built, "damage_lower_bound"), Val::Float(3.0));

    // Another base: a `was` that is not the asset's.
    let err = build(
        &mut base(),
        &[a_delta(&patch, Some(OTHER))],
        &target(),
        None,
    )
    .unwrap_err();
    assert!(err.why.contains("stale edit"), "{err}");
    // The asset's `was`, but a patch made on other bytes.
    let reversed = bps(&after, &before);
    let err = build(
        &mut base(),
        &[a_delta(&reversed, Some(&a))],
        &target(),
        None,
    )
    .unwrap_err();
    assert!(err.why.contains("not made on this asset"), "{err}");

    // The same on the map the game reads, through its build.
    let bytes = map_bytes();
    let changed = build_map(&bytes, &[edits("tmp", &[], &edit_text(A, 3.0))], &target())
        .unwrap()
        .0;
    let (before, after) = (image_of(&bytes), image_of(&changed));
    let patch = bps(&before, &after);
    let good = a_delta(&patch, Some(&asset_in(&bytes)));
    let built = settled(&bytes, std::slice::from_ref(&good)).unwrap();
    assert!(built.report.refused.is_empty());
    assert_eq!(
        field_of(&built.bytes, "damage_lower_bound"),
        Val::Float(3.0)
    );
    let stranger = a_delta(&patch, Some(OTHER));
    let built = settled(&bytes, std::slice::from_ref(&stranger)).unwrap();
    assert_eq!(refused(&built.report), ["m"]);
    assert!(off(&built.report)[0].contains("stale edit"));
    // Without `was`, the patch's own check stands; on other bytes it refuses.
    let bare = a_delta(&patch, None);
    let built = settled(&bytes, std::slice::from_ref(&bare)).unwrap();
    assert_eq!(
        field_of(&built.bytes, "damage_lower_bound"),
        Val::Float(3.0)
    );
    let wrong = a_delta(&bps(&after, &before), None);
    let built = settled(&bytes, std::slice::from_ref(&wrong)).unwrap();
    assert_eq!(refused(&built.report), ["m"]);
    assert!(
        off(&built.report)[0].contains("not made on this asset"),
        "{:?}",
        off(&built.report)
    );
}

/// The image of `A` in the map `bytes`.
fn image_of(bytes: &[u8]) -> Vec<u8> {
    let mut map = Map::from_reader(Cursor::new(bytes)).unwrap();
    let unpin = MapUnpin::new(&mut map).unwrap();
    let mut set = TagSet::from_map(&mut map).unwrap();
    let id = set.find(cc(b"jpt!"), A).unwrap();
    image(&set, &unpin.tree(id).unwrap()).unwrap()
}

#[test]
fn check_reads_a_delta_without_a_map() {
    let patch = bps(b"invented base", b"invented target");
    // (source, tag, what `check` names)
    for (source, tag, want, bytes) in [
        ("a.bps", Some(A), "<path>.<group>.bps", &patch[..]),
        ("a.jpt!.bps", None, "`tag`", &patch[..]),
        (
            "a.jpt!.bps",
            Some(A),
            "not a BPS patch",
            &b"not a patch"[..],
        ),
    ] {
        let mut text = format!(
            "{}[[item]]\nkind = \"tag-delta\"\nsource = \"{source}\"\n",
            head("m")
        );
        if let Some(t) = tag {
            text += &format!("tag = '{t}'\n");
        }
        let p = package(&text, &[(source, bytes)]);
        let found = check_package(&p);
        assert_eq!(found.len(), 1, "{source} {tag:?}: {found:?}");
        assert!(found[0].what.contains(want), "{found:?}");
    }
    // A well-formed item checks clean.
    let ok = a_delta(&patch, Some(OTHER));
    assert!(check_package(&ok).is_empty(), "{:?}", check_package(&ok));
    // A patch that would make the asset enormous.
    let grown = bps(&[], &vec![0u8; 70 << 20]);
    assert!(
        check_package(&a_delta(&grown, Some(OTHER)))[0]
            .what
            .contains("over twice its base"),
        "{:?}",
        check_package(&a_delta(&grown, Some(OTHER)))
    );
}

// ------------------------------------------------------------------ levels

#[test]
fn two_mods_on_one_weapons_different_fields_both_show_in_the_assembled_level() {
    let s = store("two-mods");
    let lower = a_mod(
        "a-lower",
        &[],
        &entry(None, &line("damage_lower_bound", None, 1.5)),
    );
    let cutoff = a_mod(
        "b-cutoff",
        &[],
        &entry(None, &line("cutoff_scale", None, 2.5)),
    );
    let level = assemble(&s.tags, &s.images, &[lower, cutoff], "tiny").unwrap();
    assert_eq!(level.packages, ["a-lower", "b-cutoff"]);
    assert!(level.refused.is_empty() && level.map_id.is_none());
    let image = read_image(&s, &level.id);
    assert_eq!(field_of(&image, "damage_lower_bound"), Val::Float(1.5));
    assert_eq!(field_of(&image, "cutoff_scale"), Val::Float(2.5));
}

#[test]
fn a_fork_is_an_item_for_one_map_and_changes_only_that_level() {
    let s = store("forks");
    let (bytes, raw) = map_with(0.0);
    put_store(&s.tags, "second", &bytes, raw);
    let wide = a_mod(
        "a-wide",
        &[],
        &entry(None, &line("damage_lower_bound", None, 1.0)),
    );
    let fork = package(
        &format!(
            "{}[[item]]\nkind = \"edits\"\nsource = \"e.yaml\"\nmaps = [\"Second\"]\n",
            head("b-fork")
        ),
        &[(
            "e.yaml",
            entry(None, &line("cutoff_scale", None, 2.0)).as_bytes(),
        )],
    );
    let tiny = assemble(&s.tags, &s.images, &[wide.clone(), fork.clone()], "tiny").unwrap();
    let second = assemble(&s.tags, &s.images, &[wide.clone(), fork.clone()], "second").unwrap();
    // The fork is laid over the map it names and over no other.
    assert_eq!(tiny.packages, ["a-wide"]);
    assert_eq!(second.packages, ["a-wide", "b-fork"]);
    assert_eq!(
        tiny.id,
        level_id("tiny", Sha256::of(&s.bytes), &[wide.clone()])
    );
    assert_eq!(
        second.id,
        level_id("second", Sha256::of(&bytes), &[wide.clone(), fork])
    );
    let alone = assemble(&s.tags, &s.images, std::slice::from_ref(&wide), "tiny").unwrap();
    assert_eq!(alone.id, tiny.id, "a fork moves no other level's id");
    assert_eq!(
        field_of(&read_image(&s, &second.id), "cutoff_scale"),
        Val::Float(2.0)
    );
}

#[test]
fn a_from_line_applies_where_the_field_has_that_value_and_leaves_the_package_off_elsewhere() {
    let s = store("variants");
    // A second map whose variant of `A` has another lower bound: the store's own fork.
    let (bytes, raw) = map_with(1.0);
    put_store(&s.tags, "variant", &bytes, raw);
    let m = a_mod(
        "a-mod",
        &[],
        &entry(None, &line("damage_lower_bound", Some(0.0), 5.0)),
    );
    let tiny = assemble(&s.tags, &s.images, std::slice::from_ref(&m), "tiny").unwrap();
    assert_eq!(tiny.packages, ["a-mod"]);
    assert!(tiny.refused.is_empty());
    assert_eq!(
        field_of(&read_image(&s, &tiny.id), "damage_lower_bound"),
        Val::Float(5.0)
    );
    let variant = assemble(&s.tags, &s.images, std::slice::from_ref(&m), "variant").unwrap();
    assert_eq!(refused_level(&variant), ["a-mod"]);
    assert!(variant.map_id.is_some() && variant.image.is_none());
    assert!(
        variant.refused[0]
            .why
            .contains("is 0.0, but the field is 1"),
        "{:?}",
        variant.refused
    );
}

fn refused_level(level: &crate::level::Level) -> Vec<&str> {
    level.refused.iter().map(|r| r.package.as_str()).collect()
}

#[test]
fn a_refused_package_moves_neither_the_level_id_nor_the_image() {
    let s = store("settled");
    let good = a_mod(
        "a-good",
        &[],
        &entry(None, &line("damage_lower_bound", None, 1.0)),
    );
    let bad = a_mod(
        "b-bad",
        &[],
        &entry(Some(OTHER), &line("cutoff_scale", None, 2.0)),
    );
    let clean = assemble(&s.tags, &s.images, std::slice::from_ref(&good), "tiny").unwrap();
    assert!(clean.built() && clean.refused.is_empty());

    // The same level with a mod for other content installed: the same id, so
    // the same image, and the mod off with its reason.
    let with_bad = assemble(&s.tags, &s.images, &[good.clone(), bad.clone()], "tiny").unwrap();
    assert_eq!(with_bad.id, clean.id);
    assert_eq!(with_bad.packages, ["a-good"]);
    assert_eq!(refused_level(&with_bad), ["b-bad"]);
    assert!(with_bad.refused[0].why.contains("stale edit"));
    assert_eq!(with_bad.image, clean.image);
    assert!(!with_bad.built(), "the image of the same id is a hit");

    // The settlement is kept: the next open names the id without building.
    let kept: Vec<String> = std::fs::read_dir(&s.images)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".set"))
        .collect();
    // One for each set the level was asked under: with the mod and without.
    assert_eq!(kept.len(), 2, "{kept:?}");
    let again = assemble(&s.tags, &s.images, &[good.clone(), bad.clone()], "tiny").unwrap();
    assert!(!again.built());
    assert_eq!(
        (again.id, again.image, again.refused),
        (clean.id, clean.image, with_bad.refused.clone())
    );

    // Only the refused mod: the disc's bytes, and the reason.
    let alone = assemble(&s.tags, &s.images, std::slice::from_ref(&bad), "tiny").unwrap();
    assert!(alone.map_id.is_some() && alone.image.is_none() && !alone.built());
    assert_eq!(refused_level(&alone), ["b-bad"]);

    // The game's record of the level carries them, for `packages.list`.
    let served = Served::default();
    let mut levels = StoreLevels::new(
        s.tags.clone(),
        s.images.clone(),
        vec![good, bad],
        served.clone(),
    );
    let image = levels.image("tiny").unwrap().expect("an image");
    let rec = served.lock().unwrap_or_else(|e| e.into_inner())["tiny"].clone();
    assert_eq!(rec.packages, ["a-good"]);
    assert_eq!(refused_level(&with_bad), ["b-bad"]);
    assert_eq!(rec.refused.len(), 1);
    assert_eq!(rec.refused[0].package, "b-bad");
    assert!(rec.error.is_none());
    assert_eq!(rec.key, image.id);
}

// -------------------------------------------------- the store's own bytes

#[test]
fn a_package_holding_a_store_blob_is_refused_by_file_name() {
    let root = temp("store-blob");
    let raw: Vec<u8> = (0..300u32).map(|i| (i * 11) as u8).collect();
    let tag = vec![7u8; 200];
    // As long as the store's tag file, and not it.
    let same_length = vec![8u8; 200];
    let ours = vec![9u8; 40];
    let hex = |b: &[u8]| Sha256::of(b).to_string();
    let raw_at = root.join("raw").join(&hex(&raw)[..2]);
    std::fs::create_dir_all(&raw_at).unwrap();
    std::fs::write(raw_at.join(hex(&raw)), &raw).unwrap();
    let tag_at = root.join("tags").join("invented").join("dmg");
    std::fs::create_dir_all(&tag_at).unwrap();
    std::fs::write(tag_at.join("a.jpt!"), &tag).unwrap();
    let p = package(
        &format!(
            "{}[[item]]\nkind = \"data\"\nsource = \"pics.bin\"\ninputs = [\"a.jpt!\"]\n\n[[item]]\nkind = \"data\"\nsource = \"same.bin\"\n\n[[item]]\nkind = \"data\"\nsource = \"ours.bin\"\n",
            head("m")
        ),
        &[
            ("pics.bin", raw.as_slice()),
            ("a.jpt!", tag.as_slice()),
            ("same.bin", same_length.as_slice()),
            ("ours.bin", ours.as_slice()),
        ],
    );
    let found = store_findings(&root, &p);
    let named: Vec<(&str, Option<usize>, &str)> = found
        .iter()
        .map(|f| (f.file.as_deref().unwrap(), f.item, f.what.as_str()))
        .collect();
    assert_eq!(named.len(), 2, "{named:?}");
    assert_eq!((named[0].0, named[0].1), ("pics.bin", Some(0)));
    assert!(named[0].2.contains("a raw resource") && named[0].2.contains(&hex(&raw)));
    assert_eq!((named[1].0, named[1].1), ("a.jpt!", Some(0)));
    assert!(named[1].2.contains("a tag file") && named[1].2.contains(&hex(&tag)));
    // A store that is not there names nothing.
    assert!(store_findings(&root.join("elsewhere"), &p).is_empty());
    let _ = std::fs::remove_dir_all(&root);
}
