//! Halo 1 particles (`part`, `pctl`) and contrails (`cont`) as Halo 2
//! `prt3` and `cont`: surveys of the Halo 1 layouts and the proofs of the
//! conversion, from the owner's discs (`$H1_ISO`, `$H2_DISC`).
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Cursor};

use h1map::{Block, Group, H1Map, TagRef};

fn disc(var: &str, name: &str) -> String {
    std::env::var(var)
        .unwrap_or_else(|_| format!("{}/../../disc/{name}", env!("CARGO_MANIFEST_DIR")))
}

/// Every Halo 1 map on the disc (`H1_LEVELS` for some), by stem.
fn h1_maps() -> Vec<(String, H1Map)> {
    let mut iso = xiso::Xiso::open(BufReader::new(
        File::open(disc("H1_ISO", "halo1.iso")).unwrap(),
    ))
    .unwrap();
    let entries = iso.entries().unwrap();
    let mut out = Vec::new();
    for e in h1map::disc::maps(&entries) {
        let stem = h1map::disc::map_stem(&e).to_ascii_lowercase();
        if let Ok(only) = std::env::var("H1_LEVELS")
            && !only.split(',').any(|l| l == stem)
        {
            continue;
        }
        let bytes = iso.read(&e).unwrap();
        if let Ok(m) = H1Map::from_reader(Cursor::new(bytes)) {
            out.push((stem, m));
        }
    }
    out
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// Survey (`H1_GROUP`, default `part`; `H1_SPAN` bytes, default 0x180):
/// per 4-byte offset of every distinct tag of the group, the references
/// by group, the block fields (count range), and the words: nonzero
/// count, float range, distinct values when few.
#[test]
#[ignore]
fn halo1_layout_survey() {
    let g = std::env::var("H1_GROUP").unwrap_or_else(|_| "part".into());
    let group = Group::new(g.as_bytes().try_into().unwrap());
    let span = std::env::var("H1_SPAN")
        .ok()
        .and_then(|s| usize::from_str_radix(s.trim_start_matches("0x"), 16).ok())
        .unwrap_or(0x180);
    let mut seen = std::collections::BTreeSet::new();
    let mut tags = 0;
    let mut refs: BTreeMap<usize, BTreeMap<String, usize>> = BTreeMap::new();
    let mut words: BTreeMap<usize, BTreeMap<u32, usize>> = BTreeMap::new();
    let mut lens: BTreeMap<usize, usize> = BTreeMap::new();
    for (_, m) in h1_maps() {
        for t in m.tags().iter().filter(|t| t.group == group) {
            if !seen.insert(t.name.clone()) {
                continue;
            }
            let b = m.tag_data(t).unwrap();
            tags += 1;
            *lens.entry(b.len()).or_default() += 1;
            for at in (0..span.min(b.len().saturating_sub(4))).step_by(4) {
                *words.entry(at).or_default().entry(u32_at(b, at)).or_default() += 1;
                if at + 16 <= b.len()
                    && let Some(r) = TagRef::read(b, at)
                    && !r.id.is_none()
                    && let Ok(x) = m.tag(r.id)
                    && x.group == r.group
                {
                    let tail = x.name.rsplit('\\').next().unwrap_or("").to_owned();
                    *refs.entry(at).or_default().entry(format!("{} {tail}", r.group)).or_default() += 1;
                }
                if let Some(k) = Block::read(b, at)
                    && k.count > 0
                    && k.count < 0x400
                    && k.address >> 28 == 8
                {
                    *refs.entry(at).or_default().entry(format!("block? count {}", k.count)).or_default() += 1;
                }
            }
        }
    }
    println!("{g}: {tags} distinct tags, lengths {:?}", lens.iter().take(8).collect::<Vec<_>>());
    for (at, w) in &words {
        let nz: usize = w.iter().filter(|(v, _)| **v != 0).map(|(_, c)| c).sum();
        if nz == 0 {
            continue;
        }
        let floats: Vec<f32> = w
            .keys()
            .map(|v| f32::from_bits(*v))
            .filter(|f| f.is_finite() && f.abs() < 1e7 && (f.abs() > 1e-6 || *f == 0.0))
            .collect();
        let (lo, hi) = floats
            .iter()
            .fold((f32::MAX, f32::MIN), |(a, b), f| (a.min(*f), b.max(*f)));
        let few: Vec<String> = if w.len() <= 6 {
            w.iter().map(|(v, c)| format!("{v:#x}x{c}")).collect()
        } else {
            vec![format!("{} distinct", w.len())]
        };
        let r = refs.get(at).map(|r| {
            let mut v: Vec<_> = r.iter().collect();
            v.sort_by_key(|(_, c)| std::cmp::Reverse(**c));
            format!("{:?}", &v[..v.len().min(3)])
        });
        println!(
            "{at:#05x}: nonzero {nz}, floats {}/{} {lo} ..= {hi}, {few:?} {}",
            floats.len(),
            w.len(),
            r.unwrap_or_default()
        );
    }
}

fn h2_map(want: &str) -> map::Map<Cursor<Vec<u8>>> {
    let mut iso = xiso::Xiso::open(BufReader::new(
        File::open(disc("H2_DISC", "halo2.iso")).unwrap(),
    ))
    .unwrap();
    let e = iso
        .entries()
        .unwrap()
        .into_iter()
        .find(|e| e.path.to_ascii_lowercase().ends_with(&format!("{want}.map")))
        .unwrap();
    map::Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap()
}

fn words(b: &[u8], from: usize, to: usize) -> String {
    (from..to.min(b.len()))
        .step_by(4)
        .map(|o| {
            let w = u32_at(b, o);
            let f = f32::from_bits(w);
            if w == 0 {
                "0".to_owned()
            } else if f.is_finite() && f.abs() > 1e-4 && f.abs() < 1e6 {
                format!("{f}")
            } else {
                format!("{w:#x}")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Survey: contrails side by side, every distinct Halo 1 `cont` (root
/// `+0..+0x40`, `+0xa8..+0xe0`, its first point) and every Halo 2 `cont`
/// on `H2_BASE` (root `+0..+0x40`, `+0x40..+0xe8`, its first point).
#[test]
#[ignore]
fn contrail_survey() {
    let mut seen = std::collections::BTreeSet::new();
    for (_, m) in h1_maps() {
        for t in m.tags().iter().filter(|t| t.group == Group::new(b"cont")) {
            if !seen.insert(t.name.clone()) {
                continue;
            }
            let b = m.tag_data(t).unwrap();
            println!("h1 {}\n  {}\n  a8: {}\n  pt: {}", t.name, words(b, 0, 0x40), words(b, 0xa8, 0xe0), words(b, 0x144, 0x144 + 0x68));
        }
    }
    let mut m = h2_map(&std::env::var("H2_BASE").unwrap_or_else(|_| "05a_deltaapproach".into()));
    let ids: Vec<_> = m
        .tags()
        .iter()
        .filter(|t| t.group == map::FourCC(*b"cont"))
        .map(|t| (t.id, t.name.clone()))
        .collect();
    for (id, name) in ids {
        let Ok(b) = m.read_tag_bytes(id) else { continue };
        println!("h2 {name:?}\n  {}\n  40: {}\n  pt: {}", words(&b, 0, 0x40), words(&b, 0x40, 0xe8), words(&b, 0xf0, 0x130));
    }
}

fn h1_level(name: &str) -> Option<H1Map> {
    let mut iso = xiso::Xiso::open(BufReader::new(
        File::open(disc("H1_ISO", "halo1.iso")).unwrap(),
    ))
    .unwrap();
    let d = h1map::disc::Disc::open(&mut iso).unwrap();
    let e = d.map(name)?.clone();
    Some(H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap())
}

/// The ten campaign levels on 05a, the multiplayer maps on their bases.
fn level_sets() -> Vec<(&'static str, Vec<String>)> {
    let mut sets: Vec<(&str, Vec<String>)> = vec![(
        "05a_deltaapproach",
        h1map::disc::CAMPAIGN_LEVELS.iter().map(|s| s.to_string()).collect(),
    )];
    for mp in ["coagulation", "beavercreek"] {
        sets.push((
            mp,
            h1convert::convert::MAPS
                .iter()
                .filter(|m| m.2 == mp)
                .map(|m| m.0.to_string())
                .collect(),
        ));
    }
    sets
}

/// Every Halo 1 contrail of the campaign and multiplayer maps converts
/// from the `cont` definition: it reads back with Halo 1's spans at Halo
/// 2's offsets, its bitmap a converted one, no pinned bytes; and every
/// rebuilt projectile and vehicle whose Halo 1 attachments name a
/// contrail names the converted one.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn every_contrail_converts() {
    use h1objects::base::Base;
    use h1objects::convert::Converter;
    use h1objects::particles::{CONT_POINT_SPANS, CONT_SPANS, H1Contrail};
    use tags::defs::{self, Defs, Target, Val};
    let d = Defs::builtin().group(map::FourCC(*b"cont")).unwrap();
    let bytes = |r: &defs::Record, p: &str| match r.at(p) {
        Some(Val::Bytes(b)) => b.clone(),
        x => panic!("{p}: {x:?}"),
    };
    let (mut conts, mut attached) = (0usize, 0usize);
    for (base_name, levels) in level_sets() {
        let mut base_map = h2_map(base_name);
        let base = Base::read(&mut base_map).unwrap();
        for level in levels {
            let Some(m) = h1_level(&level) else { continue };
            let mut c = Converter::new(&m, &base);
            let mut got = Vec::new();
            for t in m.tags().iter().filter(|t| t.group == Group::new(b"cont")) {
                let id = c.contrail(t).unwrap_or_else(|e| panic!("{level} {}: {e}", t.name));
                got.push((t, id));
            }
            let mut owners = Vec::new();
            for t in m.tags().iter().filter(|t| {
                t.group == Group::new(b"proj") || t.group == Group::new(b"vehi")
            }) {
                let b = m.tag_data(t).unwrap();
                let names: Vec<_> = h1objects::h1::attachments(&m, b)
                    .unwrap()
                    .into_iter()
                    .filter(|a| a.group == Group::new(b"cont"))
                    .filter_map(|a| a.tag)
                    .collect();
                if names.is_empty() {
                    continue;
                }
                if let Ok(id) = c.object(t) {
                    owners.push((t, id, names));
                }
            }
            let tree = &c.tree;
            for (t, id) in &got {
                let name = format!("{level} {}", t.name);
                let h = H1Contrail::read(&m, t).unwrap();
                let built = &tree.tags[id.0 as usize & 0xffff].1;
                assert_eq!(built.group, map::FourCC(*b"cont"), "{name}");
                assert!(built.raw.is_empty());
                let r = defs::from_built(d, &built.root).unwrap_or_else(|e| panic!("{name}: {e}"));
                for (f, at, len) in CONT_SPANS {
                    assert_eq!(bytes(&r, f), h.root[*at..at + len], "{name} {f}");
                }
                let Some(Val::Block(pts)) = r.at("points") else { panic!() };
                assert_eq!(pts.len(), h.points.len(), "{name}");
                for (x, p) in pts.iter().zip(&h.points) {
                    for (f, at, len) in CONT_POINT_SPANS {
                        assert_eq!(bytes(x, f), p[*at..at + len], "{name} point {f}");
                    }
                }
                if h.bitmap.is_some() {
                    assert!(
                        matches!(r.at("bitmap"), Some(Val::Ref { target: Target::Id(_), .. })),
                        "{name}: bitmap"
                    );
                }
                conts += 1;
            }
            for (t, id, names) in &owners {
                let b = &tree.tags[id.0 as usize & 0xffff].1;
                let od = Defs::builtin().group(b.group).unwrap();
                let r = defs::from_built(od, &b.root).unwrap();
                let path = if b.group == map::FourCC(*b"proj") { "object.attachments" } else { "unit.obje.attachments" };
                let Some(Val::Block(att)) = r.at(path) else { panic!("{level} {}: {path}", t.name) };
                for n in names {
                    let want = got.iter().find(|g| g.0.id == *n).map(|g| g.1).unwrap();
                    assert!(
                        att.iter().any(|a| matches!(a.at("type"), Some(Val::Ref { target: Target::Id(x), .. }) if *x == want)),
                        "{level} {}: contrail attachment",
                        t.name
                    );
                    attached += 1;
                }
            }
        }
    }
    println!("{conts} contrails converted, {attached} contrail attachments on rebuilt projectiles and vehicles");
    assert!(conts > 0 && attached > 0);
}

/// Survey: every retail `prt3` on `H2_BASE` by its shader template: the
/// stem's compiled counts (lods, layers, passes, implementations,
/// bindings and their sources), the particle's parameter names, and its
/// postprocess block counts and bitmap datums against its parameters.
#[test]
#[ignore]
fn particle_shader_survey() {
    let mut m = h2_map(&std::env::var("H2_BASE").unwrap_or_else(|_| "05a_deltaapproach".into()));
    let meta = tags::shader::Metadata::read(&mut m).unwrap();
    let strings: Vec<String> = m.strings().iter().map(|s| s.clone().unwrap_or_default()).collect();
    let name_of = |w: u32| strings.get((w & 0xffff) as usize).cloned().unwrap_or_else(|| format!("{w:#x}"));
    let tags: Vec<_> = m.tags().iter().map(|t| (t.id, t.group, t.name.clone().unwrap_or_default(), t.location.map(|l| l.address))).collect();
    let by_id = |id: u32| tags.iter().find(|t| t.0.0 == id).map(|t| t.2.clone()).unwrap_or_else(|| format!("{id:#x}"));
    let blk = |b: &[u8], o: usize, size: usize| -> Vec<Vec<u8>> {
        let (n, a) = (u32_at(b, o) as usize, u32_at(b, o + 4));
        (0..n).map(|k| meta.bytes(a + (k * size) as u32, size).unwrap().to_vec()).collect()
    };
    let mut sigs: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (_, g, name, addr) in &tags {
        if *g != map::FourCC(*b"prt3") { continue }
        let Some(addr) = addr else { continue };
        let r = meta.bytes(*addr, 0xbc).unwrap().to_vec();
        let stem = u32_at(&r, 0x10);
        let params: Vec<String> = blk(&r, 0x14, 0x28).iter().map(|p| {
            let bm = u32_at(p, 0xc);
            format!("{}{}={} {}", name_of(u32_at(p, 0)), if u32_at(p, 4) != 0 { "*" } else { "" }, if bm == u32::MAX { "-".into() } else { by_id(bm).rsplit('\\').next().unwrap().to_string() }, words(p, 0x10, 0x20))
        }).collect();
        let pp = blk(&r, 0x8c, 0x7c);
        let p = &pp[0];
        let counts: Vec<usize> = (0..15).map(|k| u32_at(p, 4 + k * 8) as usize).collect();
        let bitmaps: Vec<String> = blk(p, 0x4, 0xc).iter().map(|b| {
            let d = u32_at(b, 0);
            let dims = tags.iter().find(|t| t.0.0 == d).and_then(|t| t.3).map(|a| {
                let root = meta.bytes(a, 0x4c).unwrap().to_vec();
                blk(&root, 0x44, 0x74).iter().map(|i| format!("{}x{}", u16::from_le_bytes([i[4], i[5]]), u16::from_le_bytes([i[6], i[7]]))).collect::<Vec<_>>().join(",")
            }).unwrap_or_default();
            let n = f32::from_bits(u32_at(b, 8)) / (std::f32::consts::LN_2 * std::f32::consts::LN_2);
            format!("{}:{}:{}:n={n:.3}:{dims}", if d == u32::MAX { "-".into() } else { by_id(d).rsplit('\\').next().unwrap().to_string() }, u32_at(b, 4) as i32, f32::from_bits(u32_at(b, 8)))
        }).collect();
        let colors: Vec<String> = blk(p, 0xc, 4).iter().map(|c| format!("{:#x}", u32_at(c, 0))).collect();
        let vectors: Vec<String> = blk(p, 0x14, 0x10).iter().map(|v| words(v, 0, 0x10)).collect();
        let lods: Vec<String> = blk(p, 0x1c, 8).iter().map(|v| words(v, 0, 8)).collect();
        let pbm: Vec<String> = blk(p, 0x5c, 4).iter().map(|v| format!("{}/{}", i16::from_le_bytes([v[0], v[1]]), i16::from_le_bytes([v[2], v[3]]))).collect();
        let u16s = |o: usize, size: usize| -> String {
            blk(p, o, size).iter().map(|v| v.chunks(2).map(|c| format!("{:x}", u16::from_le_bytes([c[0], c[1]]))).collect::<Vec<_>>().join(".")).collect::<Vec<_>>().join(" ")
        };
        let detail = format!(
            "layers [{}] passes [{}] impls [{}] pcol [{}] pscal [{}]",
            u16s(0x24, 2), u16s(0x2c, 4), u16s(0x34, 0xa),
            blk(p, 0x64, 0xc).iter().map(|v| words(v, 0, 0xc)).collect::<Vec<_>>().join(" | "),
            blk(p, 0x6c, 4).iter().map(|v| words(v, 0, 4)).collect::<Vec<_>>().join(" ")
        );
        let st = by_id(stem);
        let key = format!("stem {st}");
        sigs.entry(key).or_default().push(format!(
            "  {name}\n    flags {:#x} vs {} params [{}]\n    counts {counts:?}\n    bitmaps [{}] colors [{}] vectors [{}] lods [{}] pbm [{}]\n    {detail}",
            u32_at(&r, 0), u16::from_le_bytes([r[4], r[5]]), params.join("; "), bitmaps.join(" "), colors.join(" "), vectors.join(" | "), lods.join(" | "), pbm.join(" ")
        ));
    }
    for (stem, list) in &sigs {
        println!("{stem}: {} particles", list.len());
        let st_addr = tags.iter().find(|t| format!("stem {}", t.2) == *stem).and_then(|t| t.3);
        if let Some(a) = st_addr {
            let t = tags::shader::ShaderTemplate::decode(&meta, a).unwrap();
            let c = t.compiled().unwrap();
            println!("  template: lods {:?}\n  layers {:?}\n  passes {:?}\n  impls {:?}\n  bindings {:?}", c.lods, c.layers, c.passes.iter().map(|p| (by_id(p.pass.id.0), p.implementations)).collect::<Vec<_>>(), c.implementations, c.bindings);
        }
        for l in list.iter().take(std::env::var("N").ok().and_then(|n| n.parse().ok()).unwrap_or(4)) {
            println!("{l}");
        }
    }
}

/// The compiled particle shader rule (`h1objects::particles`, "particle
/// shader") against every retail `prt3` with no animated parameter on the
/// bases (`H2_MAPS`, default the three the converter builds on): rebuilt
/// from its stem, its bitmap and the stem's role, the postprocess equals
/// retail's but for the two words nothing reads; the colour and vector
/// values are compared where the particle's only parameter is its bitmap.
#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn retail_particle_shaders_compile_from_their_stems() {
    use h1objects::base::Base;
    use h1objects::particles::{particle_postprocess, particle_shaders};
    use tags::defs::{Defs, Record, Target, Val};
    let maps = std::env::var("H2_MAPS")
        .unwrap_or_else(|_| "05a_deltaapproach,coagulation,beavercreek".into());
    let d = Defs::builtin().group(map::FourCC(*b"prt3")).unwrap();
    let bitm = Defs::builtin().group(map::FourCC(*b"bitm")).unwrap();
    let blk = |r: &Record, p: &str| -> Vec<Record> {
        match r.at(p) {
            Some(Val::Block(b)) => b.clone(),
            _ => Vec::new(),
        }
    };
    let (mut same, mut animated, mut values_checked) = (0usize, 0usize, 0usize);
    for name in maps.split(',') {
        let mut m = h2_map(name);
        let base = Base::read(&mut m).unwrap();
        let shaders = particle_shaders(&base).unwrap();
        for t in base.tags.iter().filter(|t| t.1 == map::FourCC(*b"prt3")) {
            let r = base.record(d, t.0).unwrap();
            let mut pp = blk(&r, "postprocess").remove(0);
            if !blk(&pp, "animations").is_empty() || !blk(&pp, "properties").is_empty() {
                animated += 1;
                continue;
            }
            let Some(Val::Ref { target: Target::Id(stem), .. }) = r.at("shader_template") else {
                panic!("{}: stem", t.2)
            };
            let s = shaders.iter().find(|s| s.stem == *stem).unwrap();
            let bitmap = blk(&pp, "bitmaps").first().and_then(|b| match b.at("bitmap") {
                Some(Val::Datum(Target::Id(id))) => {
                    let br = base.record(bitm, *id).unwrap();
                    let i = &blk(&br, "images")[0];
                    let wh = |f: &str| match i.at(f) {
                        Some(Val::Int(v)) => *v as u16,
                        x => panic!("{f}: {x:?}"),
                    };
                    Some((*id, wh("width"), wh("height")))
                }
                _ => None,
            });
            let mut got = particle_postprocess(s, bitmap).unwrap();
            // The two words nothing reads.
            for (list, field) in [("lods", "unknown_6"), ("passes", "unknown_2")] {
                let mut e = blk(&pp, list);
                for x in &mut e {
                    x.set(field, Val::Bytes(vec![0, 0])).unwrap();
                }
                pp.set(list, Val::Block(e)).unwrap();
            }
            if blk(&r, "parameters").len() == 1 {
                values_checked += 1;
            } else {
                for f in ["colors", "vectors"] {
                    got.set(f, pp.at(f).unwrap().clone()).unwrap();
                }
            }
            if got != pp {
                for (k, ((n, a), (_, b))) in got.fields.iter().zip(&pp.fields).enumerate() {
                    if a != b {
                        println!("{name} {}: field {k} {n}:\n  built  {a:?}\n  retail {b:?}", t.2);
                    }
                }
                panic!("{name} {}: postprocess differs", t.2);
            }
            same += 1;
        }
    }
    println!("{same} retail particles compile from their stems ({values_checked} with their values), {animated} with animated parameters not compared");
    assert!(same > 0);
}

/// Survey: retail `prt3` root words and properties on `H2_BASE`, tallied:
/// per property slot its input words and function header; the flags,
/// vertex shader, input mask, quad and UV window. And every Halo 1 `part`
/// (by name): its flags, sequence and blend words and its floats.
#[test]
#[ignore]
fn particle_root_survey() {
    use h1objects::base::Base;
    use tags::defs::{Defs, Val};
    let mut m = h2_map(&std::env::var("H2_BASE").unwrap_or_else(|_| "05a_deltaapproach".into()));
    let base = Base::read(&mut m).unwrap();
    let d = Defs::builtin().group(map::FourCC(*b"prt3")).unwrap();
    let mut tally: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    for t in base.tags.iter().filter(|t| t.1 == map::FourCC(*b"prt3")) {
        let r = base.record(d, t.0).unwrap();
        for f in ["flags", "flags_2", "vertex_shader", "first_sequence", "sequence_count", "input_mask", "quad", "uv_window"] {
            *tally.entry(f.into()).or_default().entry(format!("{:?}", r.at(f).unwrap())).or_default() += 1;
        }
        for p in ["color", "alpha", "size", "rotation", "frame"] {
            let ins: Vec<String> = ["input", "range_input", "output_modifier", "modifier_input"]
                .iter()
                .map(|k| format!("{:?}", r.at(&format!("{p}.{k}")).unwrap()))
                .collect();
            let f = match r.at(&format!("{p}.function")) {
                Some(Val::Data(b)) => {
                    match tags::effects::function::Function::parse(b) {
                        Ok(h) => format!("{:?} f{:#x} {:?} len {}", h.kind, h.flags, h.range, b.len()),
                        Err(_) => format!("{} bytes", b.len()),
                    }
                }
                x => format!("{x:?}"),
            };
            *tally.entry(format!("{p} inputs")).or_default().entry(ins.join(" ")).or_default() += 1;
            *tally.entry(format!("{p} function")).or_default().entry(f).or_default() += 1;
        }
    }
    for (k, v) in &tally {
        let mut v: Vec<_> = v.iter().collect();
        v.sort_by_key(|x| std::cmp::Reverse(*x.1));
        println!("{k}:");
        for (val, n) in v.iter().take(8) {
            println!("  {n:4} {val}");
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    for (_, m) in h1_maps() {
        for t in m.tags().iter().filter(|t| t.group == Group::new(b"part")) {
            if !seen.insert(t.name.clone()) {
                continue;
            }
            let b = m.tag_data(t).unwrap();
            let h = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
            println!(
                "part {}: flags {:#x} seq {} {} {} {} orient {} d4 {:#x} blend {} {} dc {:#x} | {}",
                t.name, u32_at(b, 0), h(0x98), h(0x9a), h(0x9c), h(0x9e), h(0xac), u32_at(b, 0xd4), h(0xd8), h(0xda), u32_at(b, 0xdc),
                words(b, 0x38, 0x48) + " | " + &words(b, 0x68, 0x94) + " | " + &words(b, 0xa8, 0xac)
            );
        }
    }
}

/// Every Halo 1 particle of the campaign and multiplayer maps converts to
/// a `prt3` from the definition: it reads back and writes the same, pins
/// nothing, names the stem its blend function pairs with (where the base
/// has it), its converted bitmap as the shader's bitmap parameter and
/// compiled bitmap, Halo 1's sequences, and converted effects.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn every_particle_converts() {
    use h1objects::base::Base;
    use h1objects::convert::Converter;
    use h1objects::particles::{H1Particle, particle_stem};
    use tags::defs::{self, Defs, Target, Val};
    let d = Defs::builtin().group(map::FourCC(*b"prt3")).unwrap();
    let (mut parts, mut stems_named) = (0usize, BTreeMap::<String, usize>::new());
    for (base_name, levels) in level_sets() {
        let mut base_map = h2_map(base_name);
        let base = Base::read(&mut base_map).unwrap();
        for level in levels {
            let Some(m) = h1_level(&level) else { continue };
            let mut c = Converter::new(&m, &base);
            let mut got = Vec::new();
            for t in m.tags().iter().filter(|t| t.group == Group::new(b"part")) {
                let id = c
                    .particle(t)
                    .unwrap_or_else(|e| panic!("{level} {}: {e}", t.name))
                    .unwrap_or_else(|| panic!("{base_name}: no particle role"));
                got.push((t, id));
            }
            for (t, id) in &got {
                let name = format!("{level} {}", t.name);
                let p = H1Particle::read(&m, t).unwrap();
                let built = &c.tree.tags[id.0 as usize & 0xffff].1;
                assert_eq!(built.group, map::FourCC(*b"prt3"), "{name}");
                assert!(built.raw.is_empty());
                let r = defs::from_built(d, &built.root).unwrap_or_else(|e| panic!("{name}: {e}"));
                let again = defs::to_built(d, &r, &mut |_| None).unwrap();
                assert_eq!(defs::from_built(d, &again).unwrap(), r, "{name}: writes the same");
                let Some(Val::Ref { target: Target::Id(stem), .. }) = r.at("shader_template") else {
                    panic!("{name}: stem")
                };
                let stem_name = base.name(*stem).unwrap().1.to_owned();
                let want = particle_stem(p.blend).0;
                if base.find(b"stem", want).is_some() {
                    assert_eq!(stem_name, want, "{name}");
                }
                *stems_named.entry(stem_name).or_default() += 1;
                let (first, count) = p.sequence_range();
                assert_eq!(r.at("first_sequence"), Some(&Val::Int(first.into())), "{name}");
                assert_eq!(r.at("sequence_count"), Some(&Val::Int(count.into())), "{name}");
                let Some(Val::Block(params)) = r.at("parameters") else { panic!() };
                let Some(Val::Block(pp)) = r.at("postprocess") else { panic!() };
                let Some(Val::Block(bitmaps)) = pp[0].at("bitmaps") else { panic!() };
                let param = match params[0].at("bitmap") {
                    Some(Val::Ref { target: Target::Id(b), .. }) => Some(*b),
                    _ => None,
                };
                let compiled = match bitmaps[0].at("bitmap") {
                    Some(Val::Datum(Target::Id(b))) => Some(*b),
                    _ => None,
                };
                assert_eq!(param, compiled, "{name}: bitmap parameter and compiled bitmap");
                assert_eq!(param.is_some(), p.bitmap.is_some(), "{name}: bitmap");
                if let Some(b) = param {
                    assert_eq!(c.tree.tags[b.0 as usize & 0xffff].1.group, map::FourCC(*b"bitm"), "{name}");
                }
                for f in ["collision_effect", "death_effect"] {
                    if let Some(Val::Ref { target: Target::Id(e), .. }) = r.at(f) {
                        assert!(e.0 & 0xffff_0000 != 0 || base.name(*e).is_some(), "{name}: {f}");
                    }
                }
                parts += 1;
            }
        }
    }
    println!("{parts} particles converted; stems {stems_named:?}");
    assert!(parts > 0);
}
