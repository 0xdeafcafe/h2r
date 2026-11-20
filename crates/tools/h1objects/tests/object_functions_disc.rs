//! Object functions and the attachments they scale: Halo 1's (`obje
//! +0x108` the A to D inputs, `+0x158` functions, `+0x140` attachments
//! with their primary scale at `+0x30`) against Halo 2's (`obje +0x64`
//! functions matched by their export name at `+8`, `+0x94` attachments
//! whose primary scale names one at `+0x10`). Reads `$H1_ISO` and
//! `$H2_DISC`; nothing is written.
//!
//! ```sh
//! H1_ISO=... H2_DISC=... cargo test --release -p h1objects --test object_functions_disc -- --ignored --nocapture
//! ```

use std::fs::File;
use std::io::{BufReader, Cursor};

use h1map::H1Map;
use h1objects::base::Base;
use h1objects::convert::{Converter, Tree};
use h1objects::Converted;
use map::{FourCC, Map, TagId};
use tags::defs::{self, Defs, Record, Str, StructDef, Target, Val};

fn disc(var: &str, name: &str) -> String {
    std::env::var(var).unwrap_or_else(|_| format!("{}/../../../disc/{name}", env!("CARGO_MANIFEST_DIR")))
}

fn h2_map(want: &str) -> (Map<Cursor<Vec<u8>>>, Vec<Option<String>>) {
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(disc("H2_DISC", "halo2.iso")).unwrap())).unwrap();
    let e = iso
        .entries()
        .unwrap()
        .into_iter()
        .find(|e| e.path.rsplit(['/', '\\']).next() == Some(&format!("{want}.map")))
        .unwrap();
    let m = Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap();
    let s = m.strings().to_vec();
    (m, s)
}

fn h1_level(name: &str) -> H1Map {
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(disc("H1_ISO", "halo1.iso")).unwrap())).unwrap();
    let d = h1map::disc::Disc::open(&mut iso).unwrap();
    let e = d.map(name).unwrap().clone();
    H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap()
}

fn def(g: &[u8; 4]) -> &'static StructDef {
    Defs::builtin().group(FourCC(*g)).unwrap()
}

fn text(x: &Record, field: &str, strings: &[Option<String>]) -> String {
    match x.at(field) {
        Some(Val::String(Str::Text(t))) => t.clone(),
        Some(Val::String(Str::Index(0))) => String::new(),
        Some(Val::String(Str::Index(w))) => {
            strings.get((w & 0xff_ffff) as usize).cloned().flatten().unwrap_or_else(|| format!("#{w:x}"))
        }
        // An empty or null name.
        Some(Val::String(_)) => String::new(),
        v => format!("{v:?}"),
    }
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_bits(u32_at(b, o))
}

#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn retail_vehicle_functions_survey() {
    let (mut m, strings) = h2_map("coagulation");
    let base = Base::read(&mut m).unwrap();
    for (kind, names) in engine_imports(&strings) {
        println!("{kind} imports: {names:?}");
    }
    for t in base.tags.iter().filter(|t| t.1 == FourCC(*b"vehi")) {
        let v = base.record(def(b"vehi"), t.0).unwrap();
        println!("== {}", t.2);
        if let Some(Val::Block(fs)) = v.at("unit.obje.functions") {
            for f in fs {
                let data = match f.at("data_14") {
                    Some(Val::Bytes(b)) => b.clone(),
                    _ => Vec::new(),
                };
                println!(
                    "  function {:?} import {:?} export {:?} off {:?} {:?} scale {:?} data {:02x?}",
                    f.at("unknown_0"),
                    text(f, "string_4", &strings),
                    text(f, "output", &strings),
                    text(f, "string_c", &strings),
                    f.at("unknown_10"),
                    text(f, "string_1c", &strings),
                    data
                );
            }
        }
        if let Some(Val::Block(a)) = v.at("unit.obje.attachments") {
            for x in a {
                let ty = match x.at("type") {
                    Some(Val::Ref { group, target }) => {
                        let id = match target { tags::defs::Target::Id(i) => Some(*i), _ => None };
                        format!("{} {}", FourCC::from_u32(*group), id.and_then(|i| base.tags.iter().find(|t| t.0 == i)).map_or("", |t| t.2.as_str()))
                    }
                    v => format!("{v:?}"),
                };
                println!(
                    "  attachment {ty} marker {:?} c {:?} primary {:?} secondary {:?}",
                    text(x, "marker", &strings),
                    x.at("change_colour"),
                    text(x, "string_10", &strings),
                    text(x, "string_14", &strings)
                );
            }
        }
    }
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn halo1_vehicle_functions_survey() {
    for level in ["bloodgulch", "sidewinder", "a10", "b30", "c10"] {
        let h1 = h1_level(level);
        for t in h1.tags().iter().filter(|t| t.group.0.to_be_bytes() == *b"vehi") {
            let b = h1.tag_data(t).unwrap();
            let ins = |o: usize| [u16_at(b, o), u16_at(b, o + 2), u16_at(b, o + 4), u16_at(b, o + 6)];
            println!(
                "== {level} {}: obje in {:?}; 0x198 {:?}; 0x2f8.. {:x?}",
                t.name,
                ins(0x108),
                ins(0x198),
                (0x2f8..0x340).step_by(2).map(|o| u16_at(b, o)).collect::<Vec<_>>()
            );
            for f in h1objects::h1::elements(&h1, b, 0x158, 0x168).unwrap() {
                println!(
                    "  function flags {:x} period {} scale-period {} fn {} scale-fn {} wobble {} {} {} sq {} steps {} map {} saw {} add {} scale-result {} bounds-mode {} bounds {:?} turn-off {} scale-by {} usage {:?}",
                    u32_at(f, 0), f32_at(f, 4), u16_at(f, 8), u16_at(f, 0xa), u16_at(f, 0xc), u16_at(f, 0xe), f32_at(f, 0x10), f32_at(f, 0x14),
                    f32_at(f, 0x18), u16_at(f, 0x1c), u16_at(f, 0x1e), u16_at(f, 0x20), u16_at(f, 0x22), u16_at(f, 0x24), u16_at(f, 0x26),
                    [f32_at(f, 0x28), f32_at(f, 0x2c)], u16_at(f, 0x36) as i16, f32_at(f, 0x38),
                    String::from_utf8_lossy(&f[0x148..0x168]).trim_end_matches('\0').to_string(),
                );
                let nz: Vec<(usize, u8)> = f.iter().enumerate().skip(0x3c).filter(|x| *x.1 != 0).take(16).map(|(i, x)| (i, *x)).collect();
                if !nz.is_empty() {
                    println!("    nonzero past 0x3c: {nz:x?}");
                }
            }
            for a in h1objects::h1::elements(&h1, b, 0x140, 0x48).unwrap() {
                let r = h1objects::h1::reference(&h1, a, 0);
                let name = r.and_then(|i| h1.tag(i).ok()).map_or("", |t| t.name.as_str());
                println!(
                    "  attachment {} {name} marker {:?} primary {} secondary {} colour {}",
                    FourCC::from_u32(u32_at(a, 0)),
                    String::from_utf8_lossy(&a[0x10..0x30]).trim_end_matches('\0'),
                    u16_at(a, 0x30), u16_at(a, 0x32), u16_at(a, 0x34),
                );
            }
        }
    }
}

/// The string IDs each object type's import handler compares (the
/// immediates of `0x000bcc20` object, `0x000c6b90` unit, `0x000f6d10`
/// vehicle, which `0x00108d90` calls in turn for a vehicle), by name
/// through the map's string table.
fn engine_imports(strings: &[Option<String>]) -> Vec<(&'static str, Vec<String>)> {
    const OBJECT: &[u32] = &[
        0x200011e, 0x30005a6, 0x40005a7, 0x500055b, 0x700055c, 0x700055d, 0x70006ba, 0x90006b5, 0xa0006b6,
        0xa0006b9, 0xd000558, 0xd0005ff, 0xf000559, 0xf0005aa, 0xf0005ac, 0x100006b4, 0x13000556, 0x130006bb,
        0x15000557, 0x170005f9, 0x170005fb, 0x1800055a, 0x180005fc, 0x1a00071c, 0x1a00071d, 0x1d0005fc, 0x1d0005fe,
    ];
    const UNIT: &[u32] = &[
        0x5000581, 0x6000087, 0x90005a1, 0x90006aa, 0xb0005a9, 0xb0006ab, 0xc000569, 0xd00059d, 0xe00059f,
        0xf0006bf, 0x11000088, 0x130005a8, 0x140005a2, 0x140005a3, 0x160005a0, 0x1800059e,
    ];
    const VEHICLE: &[u32] = &[
        0x4000580, 0x5000581, 0x5000596, 0x6000595, 0x800061e, 0x900000e, 0xa00000f, 0xa000598, 0xb00001b,
        0xb000587, 0xb000597, 0xb000713, 0xc000582, 0xc0005a5, 0xd00057b, 0xd00057f, 0xd00059a, 0xe00057a,
        0xe00057c, 0xe00057d, 0xf000584, 0x10000585, 0x10000594, 0x1300057e, 0x1300058a, 0x14000589, 0x1400058b,
        0x14000599, 0x1700058e, 0x17000592, 0x1800058c, 0x1800058f, 0x18000590, 0x1900058d, 0x19000591,
    ];
    [("object", OBJECT), ("unit", UNIT), ("vehicle", VEHICLE)]
        .into_iter()
        .map(|(k, ids)| {
            let names = ids
                .iter()
                .filter_map(|w| {
                    let n = strings.get((w & 0xff_ffff) as usize).cloned().flatten()?;
                    // The ID's top byte is the name's length.
                    (n.len() == (w >> 24) as usize).then_some(n)
                })
                .collect();
            (k, names)
        })
        .collect()
}

fn tree_name(tree: &Tree, id: TagId) -> String {
    if id.0 & 0xffff_0000 != h1objects::convert::local(0).0 {
        return String::new();
    }
    tree.tags.get(id.0 as usize & 0xffff).map(|t| t.0.clone()).unwrap_or_default()
}

fn local(tree: &Tree, id: TagId) -> Option<&Converted> {
    if id.0 & 0xffff_0000 != h1objects::convert::local(0).0 {
        return None;
    }
    tree.tags.get(id.0 as usize & 0xffff).map(|t| &t.1)
}

/// Every converted vehicle's functions import names a vehicle evaluates
/// (as every retail vehicle's do, the control), and every attachment
/// Halo 1 scales by an output names a function exporting it. On the
/// warthog the taillights scale by `jump` as retail's do, and the engine
/// by `engine_hack`; the pelican's three looping sounds by engine hack,
/// hover and thrust.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn converted_vehicles_scale_attachments_by_their_functions() {
    let (mut m, strings) = h2_map("coagulation");
    let base = Base::read(&mut m).unwrap();
    // The names the handlers compare outright, and `driver_seat_power`,
    // which retail imports though none does (the control below).
    let known: std::collections::BTreeSet<String> = engine_imports(&strings)
        .into_iter()
        .flat_map(|x| x.1)
        .chain(["driver_seat_power".into()])
        .collect();
    // The control: retail vehicles import only those names.
    let mut retail = 0;
    for t in base.tags.iter().filter(|t| t.1 == FourCC(*b"vehi")) {
        let v = base.record(def(b"vehi"), t.0).unwrap();
        let Some(Val::Block(fs)) = v.at("unit.obje.functions") else { continue };
        for f in fs {
            let i = text(f, "string_4", &strings);
            if !i.is_empty() {
                assert!(known.contains(&i), "retail {}: {i}", t.2);
                retail += 1;
            }
        }
    }
    assert!(retail >= 20, "{retail}");

    let mut scaled = 0;
    let mut seen = std::collections::BTreeMap::new();
    for level in ["bloodgulch", "a10", "b30"] {
        let h1 = h1_level(level);
        let mut c = Converter::new(&h1, &base);
        for t in h1.tags().iter().filter(|t| t.group.0.to_be_bytes() == *b"vehi") {
            let b = h1.tag_data(t).unwrap();
            let h1_attach = h1objects::h1::attachments(&h1, b).unwrap();
            let id = c.object(t).unwrap();
            let vehicle = local(&c.tree, id).unwrap();
            if vehicle.group != FourCC(*b"vehi") {
                continue;
            }
            let v = defs::from_built(def(b"vehi"), &vehicle.root).unwrap();
            let fs = match v.at("unit.obje.functions") {
                Some(Val::Block(f)) => f.clone(),
                _ => Vec::new(),
            };
            let mut exports = std::collections::BTreeMap::new();
            for f in &fs {
                let i = text(f, "string_4", &[]);
                assert!(known.contains(&i), "{level} {}: import {i}", t.name);
                let data = match f.at("data_14") {
                    Some(Val::Data(d)) => d.clone(),
                    x => panic!("{x:?}"),
                };
                tags::effects::function::Function::parse(&data).unwrap();
                exports.insert(text(f, "output", &[]), i);
            }
            let Some(Val::Block(atts)) = v.at("unit.obje.attachments") else { continue };
            // Attachments convert in Halo 1's order, skipping the ones
            // that don't; match them by marker and primary.
            for a in atts {
                let p = text(a, "string_10", &[]);
                if p.is_empty() {
                    continue;
                }
                let import = exports.get(&p).unwrap_or_else(|| panic!("{level} {}: {p} has no function", t.name));
                let marker = text(a, "marker", &[]);
                let sound = match a.at("type") {
                    Some(Val::Ref { target: Target::Id(i), .. }) => tree_name(&c.tree, *i),
                    _ => String::new(),
                };
                let _ = &sound;
                println!("{level} {} {marker:?} {sound}: {p} = {import}", t.name);
                scaled += 1;
            }
            // What Halo 1 scales each attachment by, through the
            // converted function of that output.
            for a in &h1_attach {
                let (Some(tag), 1..=4) = (a.tag, a.primary_scale) else { continue };
                let out = h1objects::functions::OUTPUT_NAMES[usize::from(a.primary_scale - 1)];
                if let Some(i) = exports.get(out) {
                    let name = h1.tag(tag).unwrap().name.clone();
                    seen.insert((t.name.clone(), name), i.clone());
                    assert!(atts.iter().any(|x| text(x, "string_10", &[]) == out), "{} {out}", t.name);
                }
            }
        }
    }
    let by = |vehicle: &str, sound: &str| {
        seen.iter()
            .find(|((v, s), _)| v == vehicle && s.contains(sound))
            .map(|x| x.1.as_str())
            .unwrap_or_else(|| panic!("{vehicle} {sound}: {seen:?}"))
            .to_owned()
    };
    assert_eq!(by("vehicles\\warthog\\warthog", "warthog_7_engine"), "engine_hack");
    assert_eq!(by("vehicles\\warthog\\warthog", "taillights"), "jump");
    assert_eq!(by("vehicles\\warthog\\warthog", "headlights"), "driver_seat_power");
    assert_eq!(by("vehicles\\pelican\\pelican", "pelican_engine"), "engine_hack");
    assert_eq!(by("vehicles\\pelican\\pelican", "pel_wind"), "hover");
    assert_eq!(by("vehicles\\pelican\\pelican", "jetsweet"), "thrust");
    assert!(scaled >= 10, "{scaled}");
}
