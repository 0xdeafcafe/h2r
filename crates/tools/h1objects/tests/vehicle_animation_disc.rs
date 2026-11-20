//! Blood Gulch's vehicles as converted, against the discs: their animation
//! graphs laid out as Halo 2's vehicle graphs, the warthog's gun held
//! under a class its graph aims, and its driver no gunner. Reads `$H1_ISO`
//! and `$H2_DISC`; nothing is written.
//!
//! ```sh
//! H1_ISO=... H2_DISC=... cargo test --release -p h1objects --test it vehicle_animation_disc:: -- --ignored --nocapture
//! ```
//!
//! - `blood_gulch_vehicle_graphs_take_halo_2s_layout`: the warthog's,
//!   ghost's and scorpion's converted graphs hold their steering as an
//!   overlay of `any` with the steering screen, their aim as
//!   `combat:fixed:any` overlay `aim_still_up` with the aiming screen,
//!   their suspension in `+0x3c` naming markers the converted model has
//!   (four on the warthog, eight on the scorpion, depths positive), the
//!   warthog's `firing` as an object overlay on `barrel_spin`; no
//!   `vehicle`, `suspension`, `tread` or `firing` mode; and only base
//!   animations among the actions, as retail's (`retail_vehicle_graphs`).
//! - `the_warthog_gun_is_held_and_only_its_gunner_fires`: the converted
//!   warthog gun's class is `fixed`, type `cg`; the warthog's driver seat
//!   lacks both gunner bits and its gunner seat holds `SEAT_GUNNER`.
//! - `retail_vehicle_graphs`: on Coagulation, every vehicle graph's actions
//!   are base animations and its overlays overlay or replacement
//!   animations (the wraith's `crouch` is a replacement); the
//!   warthog's steering overlay is in `any:any:any` and names a blend
//!   screen; the chaingun's `firing` object overlay names `barrel_spin`.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Cursor};

use h1map::H1Map;
use h1objects::base::Base;
use h1objects::convert::{Converter, Tree};
use h1objects::rebuild::{SEAT_FLAGS, SEAT_GUNNER, SEAT_PLAYER_GUNNER, VEHI_SEATS};
use h1objects::{Converted, h1};
use map::{FourCC, Map, TagId};
use tags::defs::{self, Defs, Record, Str, StructDef, Target, Val};

fn disc(var: &str, name: &str) -> String {
    std::env::var(var)
        .unwrap_or_else(|_| format!("{}/../../../disc/{name}", env!("CARGO_MANIFEST_DIR")))
}

fn h2_map(want: &str) -> Map<Cursor<Vec<u8>>> {
    let mut iso = xiso::Xiso::open(BufReader::new(
        File::open(disc("H2_DISC", "halo2.iso")).unwrap(),
    ))
    .unwrap();
    let e = iso
        .entries()
        .unwrap()
        .into_iter()
        .find(|e| e.path.rsplit(['/', '\\']).next() == Some(&format!("{want}.map")))
        .unwrap();
    Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap()
}

fn h1_level(name: &str) -> H1Map {
    let mut iso = xiso::Xiso::open(BufReader::new(
        File::open(disc("H1_ISO", "halo1.iso")).unwrap(),
    ))
    .unwrap();
    let d = h1map::disc::Disc::open(&mut iso).unwrap();
    let e = d.map(name).unwrap().clone();
    H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap()
}

fn def(g: &[u8; 4]) -> &'static StructDef {
    Defs::builtin().group(FourCC(*g)).unwrap()
}

fn local(tree: &Tree, id: TagId) -> Option<&Converted> {
    if id.0 & 0xffff_0000 != h1objects::convert::local(0).0 {
        return None;
    }
    tree.tags.get(id.0 as usize & 0xffff).map(|t| &t.1)
}

fn reference(r: &Record, path: &str) -> Option<TagId> {
    match r.at(path)? {
        Val::Ref {
            target: Target::Id(id),
            ..
        } => Some(*id),
        Val::Datum(Target::Id(id)) => Some(*id),
        _ => None,
    }
}

/// A string ID field, by its text (built records) or its index (`strings`).
fn text(v: Option<&Val>, strings: &[Option<String>]) -> String {
    match v {
        Some(Val::String(Str::Text(t))) => t.clone(),
        Some(Val::String(Str::Index(w))) => strings
            .get((w & 0xff_ffff) as usize)
            .cloned()
            .flatten()
            .unwrap_or_default(),
        _ => String::new(),
    }
}

fn block(r: &Record, path: &str) -> Vec<Record> {
    match r.get(path).or_else(|| r.at(path)) {
        Some(Val::Block(b)) => b.clone(),
        _ => Vec::new(),
    }
}

fn word(r: &Record, field: &str) -> u32 {
    match r.get(field) {
        Some(Val::Bytes(b)) if b.len() >= 4 => u32::from_le_bytes(b[..4].try_into().unwrap()),
        Some(Val::Int(i)) => *i as u32,
        v => panic!("{field}: {v:?}"),
    }
}

fn float(r: &Record, field: &str) -> f32 {
    f32::from_bits(word(r, field))
}

/// What a graph's tree holds: per `mode:class:type`, its actions and its
/// overlays as `(name, animation index)`.
type TreeView = BTreeMap<String, (Vec<(String, i16)>, Vec<(String, i16)>)>;

fn tree(r: &Record, strings: &[Option<String>]) -> TreeView {
    let mut out = TreeView::new();
    let entries = |t: &Record, p: &str| -> Vec<(String, i16)> {
        block(t, p)
            .iter()
            .map(|a| {
                let i = match a.get("animation_index") {
                    Some(Val::Int(i)) => *i as i16,
                    _ => (word(a, "unknown_4") >> 16) as i16,
                };
                (
                    text(a.get("name").or_else(|| a.get("string_0")), strings),
                    i,
                )
            })
            .collect()
    };
    for m in block(r, "modes") {
        for c in block(&m, "classes") {
            for t in block(&c, "types") {
                let key = format!(
                    "{}:{}:{}",
                    text(m.get("name"), strings),
                    text(c.get("name"), strings),
                    text(t.get("name"), strings)
                );
                out.insert(key, (entries(&t, "actions"), entries(&t, "block_c")));
            }
        }
    }
    out
}

/// Each animation's `(name, type, blend screen)`: type 0 base, 1 overlay.
fn animations(r: &Record, strings: &[Option<String>]) -> Vec<(String, i64, i8)> {
    block(r, "animations")
        .iter()
        .map(|a| {
            let kind = match a.get("animation_type") {
                Some(Val::Int(i)) => *i,
                Some(Val::Enum(i)) => i64::from(*i),
                v => panic!("animation_type {v:?}"),
            };
            let screen = match a.get("unknown_12") {
                Some(Val::Bytes(b)) => b[0] as i8,
                v => panic!("unknown_12 {v:?}"),
            };
            (text(a.get("name"), strings), kind, screen)
        })
        .collect()
}

/// Retail vehicle graphs: actions are base animations, overlays overlay
/// or replacement animations; the warthog's steering is an overlay of `any` that names a
/// blend screen; the chaingun's object overlay names `barrel_spin`.
#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn retail_vehicle_graphs() {
    let mut m = h2_map("coagulation");
    let base = Base::read(&mut m).unwrap();
    let strings = m.strings().to_vec();
    let mut graphs = 0;
    for t in base
        .tags
        .iter()
        .filter(|t| t.1 == FourCC(*b"jmad") && t.2.contains("\\vehicles\\"))
    {
        let r = base.record(def(b"jmad"), t.0).unwrap();
        let anims = animations(&r, &strings);
        for (key, (actions, overlays)) in tree(&r, &strings) {
            for (a, i) in &actions {
                assert_eq!(anims[*i as usize].1, 0, "{} {key} action {a}", t.2);
            }
            // Overlays are never base animations; the wraith's `crouch`
            // in `any:any:any` is a replacement (type 2). No Halo 1
            // vehicle graph has a replacement (all 86 on the disc's maps
            // hold types 0 and 1 only), so the converter's overlays are
            // its type 1 animations.
            for (a, i) in &overlays {
                assert_ne!(anims[*i as usize].1, 0, "{} {key} overlay {a}", t.2);
            }
        }
        if t.2 == "objects\\vehicles\\warthog\\warthog" {
            let (_, o) = &tree(&r, &strings)["any:any:any"];
            let (_, i) = o.iter().find(|(n, _)| n == "steering").expect("steering");
            assert!(anims[*i as usize].2 >= 0, "steering names a screen");
            assert_eq!(block(&r, "block_3c").len(), 4);
        }
        if t.2.ends_with("\\chaingun\\chaingun") {
            let o = block(&r, "block_44");
            assert_eq!(text(o[0].get("string_c"), &strings), "barrel_spin");
        }
        graphs += 1;
    }
    assert!(graphs >= 10, "{graphs} vehicle graphs");
}

/// Blood Gulch on Coagulation, its vehicles and their guns converted.
fn convert_blood_gulch() -> (&'static H1Map, Converter<'static>) {
    let h1: &'static H1Map = Box::leak(Box::new(h1_level("bloodgulch")));
    let mut m = h2_map("coagulation");
    let base: &'static Base = Box::leak(Box::new(Base::read(&mut m).unwrap()));
    (h1, Converter::new(h1, base))
}

fn h1_tag<'a>(h1: &'a H1Map, group: &[u8; 4], name: &str) -> &'a h1map::Tag {
    h1.tags()
        .iter()
        .find(|t| t.name == name && t.group.0.to_be_bytes() == *group)
        .unwrap_or_else(|| panic!("no {name}"))
}

/// The converted `vehi`, its graph and its render model.
fn vehicle_parts(c: &mut Converter, h1: &H1Map, name: &str) -> (Record, Record, Record) {
    let id = c
        .object(h1_tag(h1, b"vehi", name))
        .unwrap_or_else(|e| panic!("{name}: {e}"));
    let v = defs::from_built(def(b"vehi"), &local(&c.tree, id).unwrap().root).unwrap();
    let h = defs::from_built(
        def(b"hlmt"),
        &local(&c.tree, reference(&v, "unit.obje.model").unwrap())
            .unwrap()
            .root,
    )
    .unwrap();
    let Some(Val::Datum(Target::Id(a))) = h.at("animation") else {
        panic!("{name}: no graph")
    };
    let g = defs::from_built(def(b"jmad"), &local(&c.tree, *a).unwrap().root).unwrap();
    let mode = defs::from_built(
        def(b"mode"),
        &local(&c.tree, reference(&h, "render_model").unwrap())
            .unwrap()
            .root,
    )
    .unwrap();
    (v, g, mode)
}

fn marker_names(mode: &Record) -> Vec<String> {
    block(mode, "marker_groups")
        .iter()
        .map(|g| text(g.get("name"), &[]))
        .collect()
}

/// How many of Halo 1's suspension elements of `name` ride a powered mass
/// point (those have a marker on the converted model).
fn powered_suspension(h1: &H1Map, name: &str) -> usize {
    let u = h1::Unit::read(h1, h1_tag(h1, b"vehi", name)).unwrap();
    let graph = h1_assets::models::graph_from_map(
        h1,
        h1.tag(u.object.animation.unwrap()).unwrap(),
        u.object.model.map(|m| h1.tag(m).unwrap()),
    )
    .unwrap()
    .0;
    let phys = h1::Physics::read(h1, h1.tag(u.object.physics.unwrap()).unwrap()).unwrap();
    graph.vehicle.map_or(0, |v| {
        v.suspension
            .iter()
            .filter(|s| {
                usize::try_from(s.mass_point)
                    .ok()
                    .and_then(|m| phys.mass_points.get(m))
                    .is_some_and(|m| m.powered_mass_point >= 0)
            })
            .count()
    })
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn blood_gulch_vehicle_graphs_take_halo_2s_layout() {
    let (h1, mut c) = convert_blood_gulch();
    // (vehicle, steers, has a barrel overlay)
    for (name, steers, firing) in [
        ("vehicles\\warthog\\warthog", true, true),
        ("vehicles\\ghost\\ghost_mp", true, false),
        ("vehicles\\scorpion\\scorpion_mp", false, false),
    ] {
        let (_, g, mode) = vehicle_parts(&mut c, h1, name);
        let anims = animations(&g, &[]);
        let t = tree(&g, &[]);
        println!("{name}: {t:?}");
        assert!(
            !t.keys().any(|k| k.starts_with("vehicle:")),
            "{name}: a vehicle mode"
        );
        for (key, (actions, overlays)) in &t {
            for (a, i) in actions {
                assert_eq!(anims[*i as usize].1, 0, "{name} {key}: action {a}");
            }
            for (a, i) in overlays {
                assert_eq!(anims[*i as usize].1, 1, "{name} {key}: overlay {a}");
            }
        }
        let screens = block(&g, "blend_screens");
        let screen = |i: i16| {
            let s = anims[i as usize].2;
            assert!(s >= 0 && (s as usize) < screens.len(), "{name}: screen {s}");
            text(screens[s as usize].get("name"), &[])
        };
        if steers {
            let (_, o) = &t["any:any:any"];
            let (_, i) = o
                .iter()
                .find(|(n, _)| n == "steering")
                .expect("steering overlay");
            assert_eq!(screen(*i), "steering", "{name}");
        }
        // Aim, under the class its gun is held by.
        let (_, o) = &t["combat:fixed:any"];
        let (_, i) = o
            .iter()
            .find(|(n, _)| n == "aim_still_up")
            .expect("aim overlay");
        assert_eq!(screen(*i), "aiming", "{name}");
        // Suspension, naming markers the model has, depths positive.
        let markers = marker_names(&mode);
        let susp = block(&g, "block_3c");
        let want = powered_suspension(h1, name);
        assert_eq!(susp.len(), want, "{name}: suspension");
        if name.contains("warthog") {
            assert_eq!(want, 4);
        }
        for s in &susp {
            let m = text(s.get("string_8"), &[]);
            assert!(
                markers.contains(&m),
                "{name}: marker {m} not on the model {markers:?}"
            );
            let (ext, comp) = (float(s, "unknown_10"), float(s, "unknown_14"));
            assert!(ext > comp && comp > 0.0, "{name} {m}: {ext} {comp}");
            // +4: graph index -1, then the animation index.
            let i = (word(s, "unknown_4") >> 16) as i16;
            assert_eq!(anims[i as usize].1, 1, "{name} {m}: an overlay");
            println!("{name}: suspension {m} {ext} {comp}");
        }
        let objects = block(&g, "block_44");
        assert_eq!(!objects.is_empty(), firing, "{name}: object overlays");
        for o in &objects {
            assert_eq!(text(o.get("string_0"), &[]), "firing");
            assert_eq!(text(o.get("string_c"), &[]), "barrel_spin");
        }
    }
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn the_warthog_gun_is_held_and_only_its_gunner_fires() {
    let (h1, mut c) = convert_blood_gulch();
    let name = "vehicles\\warthog\\warthog";
    let (v, g, _) = vehicle_parts(&mut c, h1, name);
    let gun = c
        .object(h1_tag(h1, b"weap", "vehicles\\warthog\\warthog gun"))
        .unwrap();
    let w = defs::from_built(def(b"weap"), &local(&c.tree, gun).unwrap().root).unwrap();
    let (class, kind) = (
        text(w.at(h1objects::arsenal::WEAPON_CLASS), &[]),
        text(w.at(h1objects::arsenal::WEAPON_TYPE_NAME), &[]),
    );
    assert_eq!((class.as_str(), kind.as_str()), ("fixed", "cg"));
    // The graph lists the gun's pair, as retail's turret graphs list theirs.
    let pairs: Vec<(String, String)> = block(&g, "block_54")
        .iter()
        .map(|p| (text(p.get("string_0"), &[]), text(p.get("string_4"), &[])))
        .collect();
    assert!(pairs.contains(&(kind, class)), "{pairs:?}");
    let u = h1::Unit::read(h1, h1_tag(h1, b"vehi", name)).unwrap();
    let Some(Val::Block(seats)) = v.at(VEHI_SEATS) else {
        panic!("seats")
    };
    let mut gunners = 0;
    for (x, s) in seats.iter().zip(&u.seats) {
        let f = word(x, SEAT_FLAGS);
        println!("{}: {f:#x}", s.label);
        if s.label == "W-gunner" {
            assert_ne!(f & SEAT_GUNNER, 0, "gunner {f:#x}");
            gunners += 1;
        } else {
            assert_eq!(
                f & (SEAT_GUNNER | SEAT_PLAYER_GUNNER),
                0,
                "{} {f:#x}",
                s.label
            );
        }
    }
    assert_eq!(gunners, 1);
}
