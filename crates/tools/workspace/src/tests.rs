//! Tests on an invented map: tags of invented names, built from the
//! definitions' defaults. No disc bytes.
use content::Sha256;
use content::package::{Kind, Lock, Manifest, MapScope, Package};
use map::build::{TagPayload, TagSet};
use map::{CacheType, FourCC, TagId};
use std::collections::BTreeMap;
use tags::defs::{Defs, Record, Target as RefTarget, Val, default_record, from_built, to_built};
use tags::repo::{Body, LooseTag, Node, RawRes, Region, TagKey, TagSource, Tree};

use super::*;

pub(crate) const A: &str = "invented\\dmg\\a";
const B: &str = "invented\\dmg\\b";
pub(crate) const C: &str = "invented\\dmg\\c";
const BOOM: &str = "invented\\snd\\boom";

pub(crate) fn cc(s: &[u8; 4]) -> FourCC {
    FourCC(*s)
}

fn jpt() -> &'static tags::defs::StructDef {
    Defs::builtin().group(cc(b"jpt!")).expect("jpt! is defined")
}

fn zeroed_tag(set: &mut TagSet, group: &[u8; 4], name: &str) -> TagId {
    let def = jpt();
    let block = to_built(def, &default_record(def), &mut |_| None).unwrap();
    set.add_tag(cc(group), name, TagPayload::Built(block))
        .unwrap()
}

/// globals, two damage tags and a sound, all built.
pub(crate) fn base() -> TagSet {
    let mut set = TagSet::new(CacheType::Multiplayer, "tiny", "invented\\scenarios\\tiny");
    for g in [b"matg", b"jpt!", b"snd!"] {
        set.add_group(cc(g), None, None);
    }
    set.add_tag(
        cc(b"matg"),
        "globals\\globals",
        TagPayload::Built(map::build::Block::zeroed(0x40)),
    )
    .unwrap();
    zeroed_tag(&mut set, b"jpt!", A);
    zeroed_tag(&mut set, b"jpt!", B);
    set.add_tag(
        cc(b"snd!"),
        BOOM,
        TagPayload::Built(map::build::Block::zeroed(0x20)),
    )
    .unwrap();
    set
}

pub(crate) fn target() -> Target<'static> {
    Target {
        name: "tiny",
        scope: MapScope::Multiplayer,
    }
}

/// A package: items are (kind, source, tag, maps).
pub(crate) fn pkg(
    id: &str,
    requires: &[&str],
    items: &[(&str, &str, Option<&str>, &str)],
    files: &[(&str, &[u8])],
) -> Loaded {
    let mut text = format!("[package]\nformat = 1\nid = \"{id}\"\nname = \"{id}\"\n");
    if !requires.is_empty() {
        let list: Vec<String> = requires.iter().map(|r| format!("\"{r}\"")).collect();
        text += &format!("requires = [{}]\n", list.join(", "));
    }
    for (kind, source, tag, maps) in items {
        text += &format!("[[item]]\nkind = \"{kind}\"\nsource = \"{source}\"\n");
        if let Some(tag) = tag {
            text += &format!("tag = '{tag}'\n");
        }
        let maps = if maps.starts_with('[') {
            (*maps).to_owned()
        } else {
            format!("\"{maps}\"")
        };
        text += &format!("maps = {maps}\n");
    }
    let manifest = Manifest::from_toml(&text).unwrap();
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

fn field(set: &TagSet, name: &str, path: &str) -> Val {
    let id = set.find(cc(b"jpt!"), name).expect("tag in the map");
    let TagPayload::Built(block) = &set.tag(id).unwrap().payload else {
        panic!("not built");
    };
    let rec: Record = from_built(jpt(), block).unwrap();
    rec.at(path).expect("field").clone()
}

fn float(v: f32) -> Val {
    Val::Float(v)
}

pub(crate) const ADD_C: &str = "fields:\n  damage_lower_bound: 5.0\n";
pub(crate) const ADD_C_FILE: &str = "dmg_c.jpt!.yaml";

pub(crate) fn adds_c(id: &str) -> Loaded {
    pkg(
        id,
        &[],
        &[("tag", ADD_C_FILE, Some(C), "all")],
        &[(ADD_C_FILE, ADD_C.as_bytes())],
    )
}

pub(crate) fn edits(id: &str, requires: &[&str], text: &str) -> Loaded {
    pkg(
        id,
        requires,
        &[("edits", "edits.yaml", None, "multiplayer")],
        &[("edits.yaml", text.as_bytes())],
    )
}

pub(crate) fn edit_text(tag: &str, value: f32) -> String {
    format!("- tag: 'jpt!:{tag}'\n  set:\n    - {{path: damage_lower_bound, value: {value}}}\n")
}

// ---------------------------------------------------------------- the build

#[test]
fn two_packages_build_in_order() {
    let a = adds_c("alpha");
    let b = edits("beta", &["alpha"], &edit_text(C, 9.0));
    let mut set = base();
    let report = build(&mut set, &[a.clone(), b.clone()], &target(), None).unwrap();
    assert_eq!(report.added, vec![TagKey::new(cc(b"jpt!"), C)]);
    assert_eq!(
        report.packages,
        vec![("alpha".to_owned(), 1), ("beta".to_owned(), 1)]
    );
    assert_eq!(field(&set, C, "damage_lower_bound"), float(9.0));

    // The other order edits a tag that is not in the map yet.
    let mut set = base();
    let err = build(&mut set, &[b, a], &target(), None).unwrap_err();
    assert_eq!(err.package, "beta");
    assert!(err.why.contains("the map has no tag"), "{err}");
    assert_eq!(err.line, Some(1));
}

#[test]
fn the_same_tag_path_twice_names_both_packages() {
    let (a, b) = (adds_c("alpha"), adds_c("beta"));
    let err = build(&mut base(), &[a.clone(), b.clone()], &target(), None).unwrap_err();
    assert_eq!(err.package, "beta");
    assert!(err.why.contains("package \"alpha\" item 0"), "{err}");
    assert!(err.to_string().contains("beta"), "{err}");

    // The static check says the same before any map is built.
    let found = check_set(&[a, b]);
    assert_eq!(found.len(), 1);
    assert!(
        found[0].what.contains("package \"alpha\" item 0"),
        "{:?}",
        found[0]
    );

    // A tag the base has is a conflict too.
    let dup = pkg(
        "gamma",
        &[],
        &[("tag", "dmg_a.jpt!.yaml", Some(A), "all")],
        &[("dmg_a.jpt!.yaml", b"{}")],
    );
    let err = build(&mut base(), &[dup], &target(), None).unwrap_err();
    assert!(err.why.contains("already in the map"), "{err}");
}

#[test]
fn items_for_other_maps_do_not_run_or_conflict() {
    let only_other = pkg(
        "other",
        &[],
        &[("tag", ADD_C_FILE, Some(C), "campaign")],
        &[(ADD_C_FILE, ADD_C.as_bytes())],
    );
    let named = pkg(
        "named",
        &[],
        &[("tag", ADD_C_FILE, Some(C), "[\"Tiny\"]")],
        &[(ADD_C_FILE, ADD_C.as_bytes())],
    );
    let mut set = base();
    let report = build(&mut set, &[only_other, named], &target(), None).unwrap();
    assert_eq!(report.packages, vec![("named".to_owned(), 1)]);
    assert!(set.find(cc(b"jpt!"), C).is_some());
}

#[test]
fn a_stale_was_stops_with_its_line() {
    let ok = tag_hash(&base(), base().find(cc(b"jpt!"), A).unwrap()).unwrap();
    let text = |was: &str| {
        format!(
            "# edits\n- tag: 'jpt!:{A}'\n  was: {ok}\n  set:\n    - {{path: damage_lower_bound, value: 1.0}}\n- tag: 'jpt!:{B}'\n  was: {was}\n  set:\n    - {{path: damage_lower_bound, value: 2.0}}\n"
        )
    };
    // B's `was` is the hash of some other tag: B's entry is on line 6.
    let stale = Sha256::of(b"another tag");
    let err = build(
        &mut base(),
        &[edits("e", &[], &text(&stale.to_string()))],
        &target(),
        None,
    )
    .unwrap_err();
    assert_eq!(err.line, Some(6), "{err}");
    assert!(err.why.contains("stale edit"), "{err}");
    assert!(err.to_string().contains("line 6"), "{err}");

    // With B's own hash both entries apply.
    let b_hash = tag_hash(&base(), base().find(cc(b"jpt!"), B).unwrap()).unwrap();
    let mut set = base();
    build(
        &mut set,
        &[edits("e", &[], &text(&b_hash.to_string()))],
        &target(),
        None,
    )
    .unwrap();
    assert_eq!(field(&set, B, "damage_lower_bound"), float(2.0));
}

#[test]
fn an_edit_that_does_not_resolve_stops_with_its_line() {
    let text = format!(
        "- tag: 'jpt!:{A}'\n  set:\n    - {{path: damage_lower_bound, value: 1.0}}\n- tag: 'jpt!:{B}'\n  set:\n    - {{path: no_such_field, value: 2.0}}\n"
    );
    let e = edits("e", &[], &text);
    let err = build(&mut base(), &[e.clone()], &target(), None).unwrap_err();
    assert_eq!(err.line, Some(4), "{err}");
    assert!(err.why.contains("no_such_field"), "{err}");
    // Check finds it without a map.
    let found = check_package(&e);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].line, Some(4));

    // Missing tag, wrong value kind, out of range.
    for (set_line, want) in [
        ("{path: damage_lower_bound, value: hello}", "a number"),
        ("{path: damage_lower_bound, value: [1, 2]}", "a number"),
        ("{path: player_responses, value: 1}", "`block` lines"),
    ] {
        let text = format!("- tag: 'jpt!:{A}'\n  set:\n    - {set_line}\n");
        let err = build(&mut base(), &[edits("e", &[], &text)], &target(), None).unwrap_err();
        assert!(err.why.contains(want), "{want}: {err}");
    }
}

#[test]
fn onto_output_differs_from_the_base_only_in_changed_tags() {
    let before = base();
    let mut after = before.clone();
    let report = build(
        &mut after,
        &[edits("e", &[], &edit_text(A, 3.5))],
        &target(),
        None,
    )
    .unwrap();
    assert_eq!(
        report.changed.iter().cloned().collect::<Vec<_>>(),
        vec![TagKey::new(cc(b"jpt!"), A)]
    );
    assert_eq!(before.slots.len(), after.slots.len());
    let mut differing = Vec::new();
    for (x, y) in before.slots.iter().zip(&after.slots) {
        if x != y {
            differing.push(x.name.clone().unwrap());
        }
    }
    assert_eq!(differing, vec![A.to_owned()]);
    assert_eq!(before.groups, after.groups);
    assert_eq!(before.raw, after.raw);
    assert_eq!(field(&after, A, "damage_lower_bound"), float(3.5));
    assert_eq!(field(&before, A, "damage_lower_bound"), float(0.0));
}

#[test]
fn the_later_package_wins_a_field_and_the_override_is_listed() {
    let one = edits("one", &[], &edit_text(A, 1.0));
    let two = edits("two", &["one"], &edit_text(A, 2.0));
    let mut set = base();
    let report = build(&mut set, &[one, two], &target(), None).unwrap();
    assert_eq!(field(&set, A, "damage_lower_bound"), float(2.0));
    assert_eq!(report.overrides.len(), 1);
    let o = &report.overrides[0];
    assert_eq!((o.earlier.as_str(), o.later.as_str()), ("one", "two"));
    assert_eq!(o.path, "damage_lower_bound");
    // One package setting a field twice overrides nothing.
    let twice = edits(
        "twice",
        &[],
        &format!("{}{}", edit_text(A, 1.0), edit_text(A, 2.0)),
    );
    let report = build(&mut base(), &[twice], &target(), None).unwrap();
    assert!(report.overrides.is_empty());
}

#[test]
fn a_tag_item_starts_from_like_and_blocks_and_names_tags_in_the_map() {
    let text = format!(
        "like: 'jpt!:{A}'\nblock:\n  - {{path: player_responses, op: add}}\nfields:\n  damage_lower_bound: 7.0\n  \"player_responses[0].duration\": 0.5\n  sound: 'snd!:{BOOM}'\n"
    );
    let p = pkg(
        "p",
        &[],
        &[("tag", ADD_C_FILE, Some(C), "all")],
        &[(ADD_C_FILE, text.as_bytes())],
    );
    assert!(check_package(&p).is_empty(), "{:?}", check_package(&p));
    let mut set = base();
    build(&mut set, &[p], &target(), None).unwrap();
    assert_eq!(field(&set, C, "damage_lower_bound"), float(7.0));
    assert_eq!(field(&set, C, "player_responses[0].duration"), float(0.5));
    let Val::Block(responses) = field(&set, C, "player_responses") else {
        panic!("not a block");
    };
    assert_eq!(responses.len(), 1);
    let boom = set.find(cc(b"snd!"), BOOM).unwrap();
    assert!(
        matches!(field(&set, C, "sound"), Val::Ref { target: RefTarget::Id(id), .. } if id == boom)
    );
    // The tag it was like is untouched.
    assert_eq!(field(&set, A, "damage_lower_bound"), float(0.0));
}

#[test]
fn a_reference_to_a_tag_that_is_not_there_stops() {
    let text = "fields:\n  sound: 'snd!:invented\\snd\\nowhere'\n";
    let p = pkg(
        "p",
        &[],
        &[("tag", ADD_C_FILE, Some(C), "all")],
        &[(ADD_C_FILE, text.as_bytes())],
    );
    let err = build(&mut base(), &[p], &target(), None).unwrap_err();
    assert!(err.why.contains("not in the set"), "{err}");
}

#[test]
fn a_pinned_tag_needs_a_map_reader() {
    let mut set = base();
    let id = set.find(cc(b"jpt!"), A).unwrap();
    set.tag_mut(id).unwrap().payload = TagPayload::Pinned {
        address: 0x1000,
        bytes: vec![0; 0xc8],
    };
    let err = build(
        &mut set,
        &[edits("e", &[], &edit_text(A, 1.0))],
        &target(),
        None,
    )
    .unwrap_err();
    assert!(err.why.contains("pinned"), "{err}");
}

#[test]
fn a_kind_with_no_builder_stops_by_name() {
    let p = pkg(
        "p",
        &[],
        &[("convert", "m.toml", None, "all")],
        &[("m.toml", b"")],
    );
    let err = build(&mut base(), &[p.clone()], &target(), None).unwrap_err();
    assert_eq!(err.kind, Kind::Convert);
    assert!(err.why.contains("no builder"), "{err}");
    assert!(check_package(&p)[0].what.contains("no builder"));
    let kinds: Vec<Kind> = registry().iter().map(|b| b.kind).collect();
    assert_eq!(
        kinds,
        vec![
            Kind::Tag,
            Kind::Edits,
            Kind::Bitmap,
            Kind::Model,
            Kind::Animation,
            Kind::Sound,
            Kind::Data,
            Kind::Hud,
            Kind::Screens,
            Kind::ScreenEdits,
            Kind::Rules,
            Kind::Variant,
            Kind::Playlist,
        ]
    );
}

// ---------------------------------------------------------------- bitmap

fn png_bytes(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut e = png::Encoder::new(&mut out, w, h);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        let mut writer = e.write_header().unwrap();
        writer.write_image_data(rgba).unwrap();
        writer.finish().unwrap();
    }
    out
}

#[test]
fn a_png_builds_to_a_bitmap_tag() {
    let rgba: Vec<u8> = (0..3 * 2 * 4).map(|i| (i * 7) as u8).collect();
    let png = png_bytes(3, 2, &rgba);
    let pic = decode_png(&png).unwrap();
    assert_eq!((pic.width, pic.height), (3, 2));
    assert_eq!(pic.rgba, rgba);

    let p = pkg(
        "art",
        &[],
        &[("bitmap", "pic.png", Some("invented\\ui\\pic"), "all")],
        &[("pic.png", &png)],
    );
    assert!(check_package(&p).is_empty());
    let mut set = base();
    let raws = set.raw.len();
    let report = build(&mut set, &[p], &target(), None).unwrap();
    let id = set
        .find(cc(b"bitm"), "invented\\ui\\pic")
        .expect("the bitmap");
    assert_eq!(
        report.added,
        vec![TagKey::new(cc(b"bitm"), "invented\\ui\\pic")]
    );
    assert_eq!(set.raw.len(), raws + 1);
    let pixels = tags::rgba_bitmap::argb8888_linear(3, 2, &rgba).unwrap();
    assert_eq!(set.raw.last().unwrap().bytes, pixels);
    let TagPayload::Built(root) = &set.tag(id).unwrap().payload else {
        panic!("not built");
    };
    // One image, 3 x 2, ARGB8888.
    let (count, child) = root
        .fixups
        .iter()
        .find_map(|f| match &f.kind {
            map::build::FixupKind::Block { count, child } if f.offset == 0x44 => {
                Some((*count, child))
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(u16::from_le_bytes([child.bytes[4], child.bytes[5]]), 3);
    assert_eq!(u16::from_le_bytes([child.bytes[6], child.bytes[7]]), 2);
    assert_eq!(
        u16::from_le_bytes([child.bytes[0xc], child.bytes[0xd]]),
        tags::rgba_bitmap::FORMAT_ARGB8888
    );

    // Not a PNG, and the same tag twice.
    let bad = pkg(
        "bad",
        &[],
        &[("bitmap", "pic.png", Some("invented\\ui\\pic"), "all")],
        &[("pic.png", b"not a png")],
    );
    assert!(check_package(&bad)[0].what.contains("not a PNG"));
}

// ---------------------------------------------------------------- the key

#[test]
fn the_cache_key_follows_what_the_output_depends_on() {
    let base_hash = Sha256::of(b"base map");
    let a = adds_c("alpha");
    let key = |h: &Sha256, p: &[Loaded], t: &Target<'_>| cache_key(h, p, t);
    let k = key(&base_hash, &[a.clone()], &target());
    assert_eq!(k, key(&base_hash, &[a.clone()], &target()));

    // A source byte, the base, the map's name and class, the order.
    let changed = pkg(
        "alpha",
        &[],
        &[("tag", ADD_C_FILE, Some(C), "all")],
        &[(ADD_C_FILE, b"fields:\n  damage_lower_bound: 6.0\n")],
    );
    assert_ne!(k, key(&base_hash, &[changed], &target()));
    assert_ne!(k, key(&Sha256::of(b"other base"), &[a.clone()], &target()));
    let other_map = Target {
        name: "other",
        scope: MapScope::Multiplayer,
    };
    assert_ne!(k, key(&base_hash, &[a.clone()], &other_map));
    let other_scope = Target {
        name: "tiny",
        scope: MapScope::Campaign,
    };
    assert_ne!(k, key(&base_hash, &[a.clone()], &other_scope));
    let b = edits("beta", &[], &edit_text(A, 1.0));
    assert_ne!(
        key(&base_hash, &[a.clone(), b.clone()], &target()),
        key(&base_hash, &[b.clone(), a.clone()], &target())
    );
    // The case of the map's name does not matter.
    let upper = Target {
        name: "TINY",
        scope: MapScope::Multiplayer,
    };
    assert_eq!(k, key(&base_hash, &[a.clone()], &upper));

    // A package with nothing for this map leaves the key alone.
    let elsewhere = pkg(
        "elsewhere",
        &[],
        &[("tag", ADD_C_FILE, Some(C), "campaign")],
        &[(ADD_C_FILE, ADD_C.as_bytes())],
    );
    assert_eq!(k, key(&base_hash, &[a.clone(), elsewhere], &target()));
}

#[test]
fn a_tag_hash_follows_the_tag() {
    let set = base();
    let a = set.find(cc(b"jpt!"), A).unwrap();
    let b = set.find(cc(b"jpt!"), B).unwrap();
    assert_eq!(tag_hash(&set, a), tag_hash(&set.clone(), a));
    // Equal content, same group: equal hashes.
    assert_eq!(tag_hash(&set, a), tag_hash(&set, b));
    let mut edited = set.clone();
    build(
        &mut edited,
        &[edits("e", &[], &edit_text(A, 3.0))],
        &target(),
        None,
    )
    .unwrap();
    assert_ne!(tag_hash(&set, a), tag_hash(&edited, a));
    assert_eq!(tag_hash(&set, b), tag_hash(&edited, b));
}

// ---------------------------------------------------------------- Layered

struct Mem(Vec<LooseTag>);

fn loose(group: &[u8; 4], path: &str, marker: u8) -> LooseTag {
    LooseTag {
        key: TagKey::new(cc(group), path),
        body: Body::Tree(Tree {
            region: Region::Metadata,
            nodes: vec![Node {
                bytes: vec![marker; 4],
                count: 1,
                size: 4,
                align: 4,
                fields: Vec::new(),
            }],
            raws: Vec::new(),
            losses: Vec::new(),
        }),
    }
}

impl TagSource for Mem {
    fn keys(&self) -> Vec<TagKey> {
        self.0.iter().map(|t| t.key.clone()).collect()
    }
    fn read(&mut self, key: &TagKey) -> tags::repo::Result<Option<LooseTag>> {
        Ok(self.0.iter().find(|t| &t.key == key).cloned())
    }
    fn raw(&mut self, _: &RawRes) -> tags::repo::Result<Vec<u8>> {
        Err(tags::repo::Error::Invalid("no raw".into()))
    }
    fn groups(&self) -> Vec<map::TagGroup> {
        let mut out: Vec<map::TagGroup> = Vec::new();
        for t in &self.0 {
            if !out.iter().any(|g| g.class == t.key.group) {
                out.push(map::TagGroup {
                    class: t.key.group,
                    parent: None,
                    grandparent: None,
                });
            }
        }
        out
    }
}

fn marker(tag: &LooseTag) -> u8 {
    match &tag.body {
        Body::Tree(t) => t.nodes[0].bytes[0],
        Body::Alias { .. } => panic!("alias"),
    }
}

#[test]
fn layered_gives_the_first_hit_and_lists_each_key_once() {
    let top = Mem(vec![loose(b"jpt!", "x", 1), loose(b"jpt!", "only_top", 1)]);
    let low = Mem(vec![loose(b"jpt!", "x", 2), loose(b"snd!", "only_low", 2)]);
    let mut layers = Layered::new().with(top).with(low);
    assert_eq!(layers.len(), 2);
    let x = TagKey::new(cc(b"jpt!"), "x");
    assert_eq!(marker(&layers.read(&x).unwrap().unwrap()), 1);
    let low_only = TagKey::new(cc(b"snd!"), "only_low");
    assert_eq!(marker(&layers.read(&low_only).unwrap().unwrap()), 2);
    assert!(
        layers
            .read(&TagKey::new(cc(b"jpt!"), "nowhere"))
            .unwrap()
            .is_none()
    );
    assert_eq!(layers.layer_of(&x), Some(0));
    assert_eq!(layers.layer_of(&low_only), Some(1));
    let keys: Vec<String> = layers.keys().iter().map(|k| k.path.clone()).collect();
    assert_eq!(keys, vec!["x", "only_top", "only_low"]);
    let groups: Vec<FourCC> = layers.groups().iter().map(|g| g.class).collect();
    assert_eq!(groups, vec![cc(b"jpt!"), cc(b"snd!")]);
    // A closure over the layers reads through them.
    let c = tags::repo::closure(&mut layers, &[x]).unwrap();
    assert_eq!(c.tags.len(), 1);
    assert_eq!(marker(&c.tags[0]), 1);
    assert!(c.missing.is_empty());
}

// ---------------------------------------------------------------- check

#[test]
fn check_names_missing_sources_and_bad_yaml() {
    let p = pkg(
        "p",
        &[],
        &[
            ("edits", "edits.yaml", None, "all"),
            ("tag", "dmg_c.jpt!.yaml", Some(C), "all"),
        ],
        &[
            ("edits.yaml", b"- tag: [unclosed"),
            ("dmg_c.jpt!.yaml", b"fields: {}"),
        ],
    );
    let mut p = p;
    p.files.remove("dmg_c.jpt!.yaml");
    let found = check_package(&p);
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found[0].what.starts_with("edits"), "{:?}", found[0]);
    assert!(found[1].what.contains("no file"), "{:?}", found[1]);
    assert_eq!(found[1].item, Some(1));

    // A tag file whose name has no group.
    let nameless = pkg(
        "q",
        &[],
        &[("tag", "dmg.yaml", Some(C), "all")],
        &[("dmg.yaml", b"")],
    );
    assert!(check_package(&nameless)[0].what.contains("<group>"));
}

#[test]
fn the_group_is_the_second_extension() {
    assert_eq!(group_of_source("tags/pogo_unsc.weap.yaml"), Ok(cc(b"weap")));
    assert_eq!(group_of_source("a/b.jpt!.yml"), Ok(cc(b"jpt!")));
    assert!(group_of_source("pogo.yaml").is_err());
    assert!(group_of_source("pogo.weapon.yaml").is_err());
}

#[test]
fn flags_enums_and_lists_are_set_by_name_or_number() {
    let def = jpt();
    // `radius` is two floats: a list.
    let text = format!(
        "- tag: 'jpt!:{A}'\n  set:\n    - {{path: radius, value: [1.5, 2.5]}}\n    - {{path: general_damage, value: boom}}\n"
    );
    let mut set = base();
    build(&mut set, &[edits("e", &[], &text)], &target(), None).unwrap();
    assert_eq!(
        field(&set, A, "radius"),
        Val::List(vec![float(1.5), float(2.5)])
    );
    assert!(matches!(
        field(&set, A, "general_damage"),
        Val::String(tags::defs::Str::Text(ref s)) if s == "boom"
    ));
    assert!(def.field("radius").is_some());
    // A list of the wrong length.
    let text = format!("- tag: 'jpt!:{A}'\n  set:\n    - {{path: radius, value: [1.5]}}\n");
    let err = build(&mut base(), &[edits("e", &[], &text)], &target(), None).unwrap_err();
    assert!(err.why.contains("where the field has 2"), "{err}");
}

// ------------------------------------------------------------------- hud

const HUD_YAML: &str = "name: invented\nelements:\n  - {id: i.line, kind: prompt, longest: Hello}\n  - id: i.bar\n    kind: meter\n    art: {type: strip-meter-end, columns: 9}\n    at: {anchor: bottom-left, full: [4, -8]}\n";

#[test]
fn a_hud_item_reads_and_changes_no_tag() {
    let p = pkg(
        "h",
        &[],
        &[("hud", "i.hud.yaml", None, "all")],
        &[("i.hud.yaml", HUD_YAML.as_bytes())],
    );
    assert_eq!(check_package(&p), vec![]);
    let mut set = base();
    let report = build(&mut set, &[p.clone()], &target(), None).unwrap();
    assert!(report.added.is_empty() && report.changed.is_empty());
    assert_eq!(report.packages, vec![("h".to_owned(), 1)]);
    let read = huds(&[p]);
    assert_eq!(read.len(), 1);
    assert_eq!(read[0].0, "h");
    assert_eq!(read[0].1.name, "invented");
    assert_eq!(read[0].1.elements.len(), 2);
}

#[test]
fn a_bad_hud_stops_by_name_with_its_line() {
    let bad = "name: invented\nelements:\n  - {id: i.line, kind: prompt}\n";
    let p = pkg(
        "h",
        &[],
        &[("hud", "i.hud.yaml", None, "all")],
        &[("i.hud.yaml", bad.as_bytes())],
    );
    let found = check_package(&p);
    assert_eq!(found.len(), 1);
    assert!(found[0].what.starts_with("hud: "), "{:?}", found[0]);
    assert!(found[0].what.contains("needs `longest`"), "{:?}", found[0]);
    let err = build(&mut base(), &[p.clone()], &target(), None).unwrap_err();
    assert_eq!(err.kind, Kind::Hud);
    assert!(err.why.contains("needs `longest`"), "{err}");
    assert!(huds(&[p]).is_empty());
    let tagged = pkg(
        "t",
        &[],
        &[("hud", "i.hud.yaml", Some(C), "all")],
        &[("i.hud.yaml", HUD_YAML.as_bytes())],
    );
    assert!(check_package(&tagged)[0].what.contains("adds no tag"));
}

#[test]
fn the_first_party_huds_read() {
    // The packages as the repository holds them: every `hud` item reads.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../content");
    let mut seen = Vec::new();
    for id in ["superbounce", "forge", "modes"] {
        let dir = root.join(id);
        let mut files = BTreeMap::new();
        for rel in ["package.toml"].into_iter().chain(match id {
            "superbounce" => vec![
                "huds/superbounce.hud.yaml",
                "huds/superbounce-readout.hud.yaml",
                "huds/pill-block.hud.yaml",
            ],
            "forge" => vec!["huds/forge-readout.hud.yaml"],
            _ => vec!["huds/firefight.hud.yaml"],
        }) {
            files.insert(rel.to_owned(), std::fs::read(dir.join(rel)).unwrap());
        }
        let loaded = Loaded::from_files(&format!("content/{id}"), &files).unwrap();
        assert_eq!(check_package(&loaded), vec![], "{id}");
        seen.extend(huds(&[loaded]).into_iter().map(|(_, h)| h.name));
    }
    assert_eq!(
        seen,
        [
            "superbounce",
            "superbounce-readout",
            "pill-block",
            "forge-readout",
            "firefight"
        ]
    );
}
