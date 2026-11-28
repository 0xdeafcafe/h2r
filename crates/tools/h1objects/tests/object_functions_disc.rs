//! Object functions and the attachments they scale: Halo 1's (`obje
//! +0x108` the A to D inputs, `+0x158` functions, `+0x140` attachments
//! with their primary scale at `+0x30`) against Halo 2's (`obje +0x64`
//! functions matched by their export name at `+8`, `+0x94` attachments
//! whose primary scale names one at `+0x10`). Reads `$H1_ISO` and
//! `$H2_DISC`; nothing is written.
//!
//! ```sh
//! H1_ISO=... H2_DISC=... cargo test --release -p h1objects --test it object_functions_disc:: -- --ignored --nocapture
//! ```

use tags::bytes::{f32_at, u16_at, u32_at};
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
                let data = match f.at("default_function") {
                    Some(Val::Bytes(b)) => b.clone(),
                    _ => Vec::new(),
                };
                println!(
                    "  function {:?} import {:?} export {:?} off {:?} {:?} scale {:?} data {:02x?}",
                    f.at("flags"),
                    text(f, "import_name", &strings),
                    text(f, "output", &strings),
                    text(f, "turn_off_with", &strings),
                    f.at("minimum_value"),
                    text(f, "scale_by", &strings),
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
                    text(x, "primary_scale", &strings),
                    text(x, "secondary_scale", &strings)
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
            let i = text(f, "import_name", &strings);
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
                let i = text(f, "import_name", &[]);
                assert!(known.contains(&i), "{level} {}: import {i}", t.name);
                let data = match f.at("default_function") {
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
                let p = text(a, "primary_scale", &[]);
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
                    assert!(atts.iter().any(|x| text(x, "primary_scale", &[]) == out), "{} {out}", t.name);
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

/// Every Halo 1 object that attaches something named for the needler's
/// flare converts each output its attachments scale by. Prints the
/// object's functions and the converter's notes: the shape of one that
/// does not convert is the input to modelling it (`functions::one`).
/// Reads `$H1_ISO`, and `$H1_LEVELS` (default: every map).
///
/// ```sh
/// H1_ISO=... cargo test --release -p h1objects --test it object_functions_disc::needler_flare_functions_convert -- --ignored --nocapture
/// ```
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn needler_flare_functions_convert() {
    use h1objects::functions::{convert, primary_scale};
    use h1objects::h1;

    let mut iso = xiso::Xiso::open(BufReader::new(File::open(disc("H1_ISO", "halo1.iso")).unwrap())).unwrap();
    let entries = iso.entries().unwrap();
    let mut seen = std::collections::BTreeSet::new();
    let mut failures = Vec::new();
    for e in h1map::disc::maps(&entries) {
        let stem = h1map::disc::map_stem(&e).to_ascii_lowercase();
        if let Ok(only) = std::env::var("H1_LEVELS")
            && !only.split(',').any(|l| l == stem)
        {
            continue;
        }
        let Ok(map) = H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())) else { continue };
        for tag in map.tags() {
            if !h1::OBJECT_TYPES.iter().any(|g| tag.group == h1map::Group::new(g)) {
                continue;
            }
            let Ok(b) = map.tag_data(tag) else { continue };
            let Ok(obj) = h1::Object::read(&map, b) else { continue };
            let Ok(attachments) = h1::elements(&map, b, 0x140, 0x48) else { continue };
            let flares: Vec<_> = attachments
                .into_iter()
                .filter_map(|a| {
                    let name = h1::reference(&map, a, 0).and_then(|i| map.tag(i).ok())?.name.clone();
                    let lower = name.to_ascii_lowercase();
                    (lower.contains("needle") && (lower.contains("flare") || lower.contains("lens") || lower.contains("light")))
                        .then(|| (name, u16_at(a, 0x30), u16_at(a, 0x32)))
                })
                .collect();
            if flares.is_empty() || !seen.insert(tag.name.clone()) {
                continue;
            }
            let functions = h1::functions(&map, b).unwrap();
            let mut notes = Vec::new();
            let got = convert(&functions, &h1::FunctionInputs::read(b, obj.object_type), &mut notes);
            println!("== {stem} {} ({})", tag.name, tag.group);
            for (k, f) in functions.iter().enumerate() {
                println!("  output {} source function: {f:?}", k + 1);
            }
            println!("  notes: {notes:?}");
            for (name, primary, secondary) in flares {
                println!("  attachment {name}: primary {primary} secondary {secondary}");
                for (kind, index) in [("primary", primary), ("secondary", secondary)] {
                    if let Err(why) = primary_scale(index, &got) {
                        failures.push(format!("{} {name} {kind} scale: {why}", tag.name));
                    }
                }
            }
        }
    }
    assert!(!seen.is_empty(), "no object attaches a needle flare");
    assert!(failures.is_empty(), "{failures:#?}");
}

/// The retail `default.xbe` from the disc image in `$var`.
fn xbe_bytes(var: &str, name: &str) -> Vec<u8> {
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(disc(var, name)).unwrap())).unwrap();
    let e = iso
        .entries()
        .unwrap()
        .into_iter()
        .find(|e| e.path.trim_start_matches(['/', '\\']).eq_ignore_ascii_case("default.xbe"))
        .unwrap();
    iso.read(&e).unwrap()
}

/// Halo 1's periodic waves (function kinds 2 to 11) are Halo 2's: Halo
/// 2's wave slots 1 to 11 (`0x00468d48`, 1024 bytes each, slot 0 unused)
/// lie in Halo 1's executable in the same order, so a converted function
/// of kind *k* reads Halo 1's table through Halo 2's periodic wave *k*
/// (`h1objects::functions`, `periodic`).
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn halo1_wave_table_is_halo2s() {
    let h2 = xbe_bytes("H2_DISC", "halo2.iso");
    let h2 = xbe::Xbe::parse(&h2).unwrap();
    let waves = h2.bytes_at(0x0046_8d48, 12 * 1024).unwrap();
    let h1 = xbe_bytes("H1_ISO", "halo1.iso");
    let slots = &waves[1024..];
    let at = h1
        .windows(slots.len())
        .position(|w| w == slots)
        .unwrap_or_else(|| {
            let found: Vec<usize> = (1..12)
                .filter(|&k| h1.windows(1024).any(|w| w == &waves[k * 1024..(k + 1) * 1024]))
                .collect();
            panic!("Halo 2's waves 1 to 11 are not in Halo 1's executable in order; slots found alone: {found:?}")
        });
    let h1x = xbe::Xbe::parse(&h1).unwrap();
    let va = h1x
        .sections
        .iter()
        .find(|s| (s.raw_addr as usize..(s.raw_addr + s.raw_size) as usize).contains(&at))
        .map(|s| (s.name.clone(), s.va + (at as u32 - s.raw_addr) - 1024));
    println!("Halo 1's wave slot 0 at file offset {:#x} ({va:x?})", at - 1024);
}

/// Every function the conversion of `tree` holds whose blob is a periodic
/// wave, as (tag, export, wave).
fn periodic_functions(tree: &Tree) -> Vec<(String, String, u8)> {
    use tags::effects::function::{Function, FunctionType};
    let mut out = Vec::new();
    for (name, t) in &tree.tags {
        let Some(d) = Defs::builtin().group(t.group) else { continue };
        let Ok(r) = defs::from_built(d, &t.root) else { continue };
        for path in ["object.functions", "item.obje.functions", "unit.obje.functions", "devi.obje.functions"] {
            let Some(Val::Block(fs)) = r.at(path) else { continue };
            for f in fs {
                let Some(Val::Data(data)) = f.at("default_function") else { continue };
                let Ok(blob) = Function::parse(data) else { continue };
                if blob.kind == FunctionType::Periodic {
                    out.push((name.clone(), text(f, "output", &[]), blob.sub_types[0]));
                }
            }
        }
    }
    out
}

/// The objects the import stopped on for a Halo 1 periodic function
/// (the needler's flare, the crashed lifepod's smoke, the burning flame,
/// the monitor's ambient sound) convert, and their conversion exports a
/// periodic wave by name. The ones it stopped on for an input or a change
/// colour (the energy sword, the sentinel's engine, b30's holo panel)
/// convert, or fail naming the reason: an input by its object type and
/// Halo 1 source, a change colour by its function's reason.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn periodic_and_input_objects_convert_or_name_their_reason() {
    let (mut m, _) = h2_map("coagulation");
    let base = Base::read(&mut m).unwrap();
    let periodic = [
        ("a30", "weapons\\needler\\needler"),
        ("a30", "scenery\\vehicles\\lifepod_crashed\\lifepod_crashed"),
        ("a10", "scenery\\emitters\\burning_flame\\burning_flame"),
        ("c10", "characters\\monitor\\monitor"),
    ];
    let inputs = [
        ("b30", "weapons\\energy sword\\energy sword"),
        ("c10", "characters\\sentinel\\sentinel"),
        ("b30", "levels\\b30\\devices\\interior tech objects\\holo panel\\holo panel"),
    ];
    for (level, name, wants_wave) in periodic
        .iter()
        .map(|x| (x.0, x.1, true))
        .chain(inputs.iter().map(|x| (x.0, x.1, false)))
    {
        let h1 = h1_level(level);
        let t = h1.tags().iter().find(|t| t.name == name).unwrap_or_else(|| panic!("{level}: no {name}")).clone();
        let mut c = Converter::new(&h1, &base);
        match c.object(&t) {
            Ok(_) => {
                let waves = periodic_functions(&c.tree);
                println!("{level} {name}: converted; waves {waves:?}");
                assert!(!wants_wave || !waves.is_empty(), "{level} {name}: no periodic function");
            }
            Err(e) => {
                let e = e.to_string();
                println!("{level} {name}: {e}");
                assert!(!wants_wave, "{level} {name}: {e}");
                assert!(!e.contains("is not one"), "{e}");
                let named = e.contains(" source ") || !e.ends_with("did not convert");
                assert!(named, "{level} {name}: the reason is not named: {e}");
            }
        }
    }
}
