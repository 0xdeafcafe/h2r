//! Tag layout schemas against the owner's retail maps. Opt-in: needs the
//! disc in `H2_DISC` (and optionally the map pack in `H2_MAPPACK`). Nothing
//! is written unless `H2_SCHEMA_OUT` names a file for the inferred schema.
//! `H2_SCHEMA_MAPS` picks maps by stem (comma-separated; default all).
//!
//! ```sh
//! H2_DISC=disc/halo2.iso H2_MAPPACK=disc/mappack.iso \
//!   cargo test --release -p tags --test tag_schema -- --ignored --nocapture
//! ```

use map::{FourCC, Map};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Cursor};
use tags::schema::infer::{MapSample, infer};
use tags::schema::{Memory, Refusal, Relocator, Schema, read_tree, relocated_variant};

type Res<T> = Result<T, Box<dyn std::error::Error>>;
type DiscMap = Map<BufReader<File>>;

/// Every `.map` on the disc and the map pack, as (stem, map).
fn maps() -> Res<Vec<(String, DiscMap)>> {
    let wanted: Option<Vec<String>> = std::env::var("H2_SCHEMA_MAPS")
        .ok()
        .map(|v| v.split(',').map(str::to_owned).collect());
    let mut out = Vec::new();
    for var in ["H2_DISC", "H2_MAPPACK"] {
        let Some(disc) = std::env::var_os(var) else {
            continue;
        };
        let mut iso = xiso::Xiso::open(BufReader::new(File::open(&disc)?))?;
        let mut entries = iso.entries()?;
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        for e in entries {
            let file = e.path.rsplit(['/', '\\']).next().unwrap_or("");
            let Some(stem) = file.strip_suffix(".map") else {
                continue;
            };
            if e.is_dir
                || wanted
                    .as_ref()
                    .is_some_and(|w| !w.iter().any(|x| x == stem))
            {
                continue;
            }
            let base = iso.partition_offset() + u64::from(e.sector) * xiso::SECTOR;
            let map = Map::from_region(BufReader::new(File::open(&disc)?), base, e.size.into())?;
            out.push((stem.to_owned(), map));
        }
    }
    assert!(!out.is_empty(), "set H2_DISC (and H2_MAPPACK) to the ISOs");
    Ok(out)
}

/// Infer the schema from every map; write it to `H2_SCHEMA_OUT` if set.
#[test]
#[ignore = "needs the owner's ISOs"]
fn infer_from_retail_maps() -> Res<()> {
    let started = std::time::Instant::now();
    let mut samples = Vec::new();
    for (stem, mut map) in maps()? {
        samples.push(MapSample::read(&stem, &mut map)?);
    }
    let schema = infer(&mut samples);
    println!(
        "inferred {} groups from {} maps in {:?}",
        schema.groups.len(),
        samples.len(),
        started.elapsed()
    );
    let text = schema.to_text();
    assert_eq!(Schema::parse(&text)?, schema);
    if let Some(out) = std::env::var_os("H2_SCHEMA_OUT") {
        std::fs::write(out, text)?;
    }
    Ok(())
}

/// Per group: tags, relocatable, refusals by reason.
#[derive(Default)]
struct Coverage {
    tags: u32,
    ok: u32,
    refused: BTreeMap<Refusal, u32>,
    /// (reason, path, offset, value in the tag?) -> count
    places: BTreeMap<(Refusal, Vec<u32>, u32, bool), u32>,
}

fn coverage_of(map: &mut DiscMap, cov: &mut BTreeMap<FourCC, Coverage>) -> Res<Relocator<'static>> {
    let r = Relocator::new(Schema::builtin(), map)?;
    for t in map.tags().to_vec() {
        let c = cov.entry(t.group).or_default();
        c.tags += 1;
        match r.check(t.id) {
            Ok(_) => c.ok += 1,
            Err(d) => {
                let inside = t
                    .location
                    .is_some_and(|l| (l.address..l.address + l.size).contains(&d.value));
                *c.places
                    .entry((d.refusal.clone(), d.path, d.offset, inside))
                    .or_default() += 1;
                *c.refused.entry(d.refusal).or_default() += 1;
            }
        }
    }
    Ok(r)
}

/// Coverage report: per group, the share of tags the checked-in schema can
/// relocate, across all maps.
#[test]
#[ignore = "needs the owner's ISOs"]
fn coverage_report() -> Res<()> {
    let mut cov = BTreeMap::new();
    for (_, mut map) in maps()? {
        coverage_of(&mut map, &mut cov)?;
    }
    let (mut all, mut ok) = (0, 0);
    println!("group      tags  relocatable  refusals");
    for (g, c) in &cov {
        all += c.tags;
        ok += c.ok;
        println!(
            "{:<8} {:>6} {:>6} {:>5.1}%  {:?}",
            g.to_string(),
            c.tags,
            c.ok,
            100.0 * f64::from(c.ok) / f64::from(c.tags),
            c.refused
        );
        if std::env::var_os("H2_SCHEMA_WHY").is_some() {
            let mut places: Vec<_> = c.places.iter().collect();
            places.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
            for ((r, path, off, inside), n) in places.into_iter().take(4) {
                println!("    {n:>6} {r:?} at {path:x?} +{off:#x} (value in tag: {inside})");
            }
        }
    }
    let full = cov.values().filter(|c| c.ok == c.tags).count();
    println!(
        "total: {ok}/{all} tags ({:.1}%), {full}/{} groups fully relocatable",
        100.0 * f64::from(ok) / f64::from(all),
        cov.len()
    );
    Ok(())
}

/// Shuffle: every relocatable ordinary-metadata tag of every map becomes a
/// built tree appended after the old metadata (new addresses, children in
/// the builder's depth-first order). The rebuilt map must read back with
/// the same index, every pinned tag byte-identical, every moved tag's
/// address-free tree identical, and the existing decoders giving the same
/// results apart from addresses.
#[test]
#[ignore = "needs the owner's ISOs"]
fn shuffle_every_relocatable_tag() -> Res<()> {
    let (mut failed, mut at_bound) = (Vec::new(), Vec::new());
    for (stem, mut map) in maps()? {
        match shuffle(&stem, &mut map) {
            Ok(()) => {}
            // Maps already at the bound (the largest of their type) have no
            // room for the few KiB of alignment padding a relayout adds.
            Err(e) if e.to_string().contains("tag memory") => {
                println!("{stem}: at the tag-memory bound: {e}");
                at_bound.push(stem);
            }
            Err(e) => {
                println!("{stem}: FAILED: {e}");
                failed.push(stem);
            }
        }
    }
    println!("at the tag-memory bound: {at_bound:?}");
    assert!(failed.is_empty(), "failed: {failed:?}");
    Ok(())
}

/// `H2_SHUFFLE_LAYOUT=depth` lays children out depth first (the builder's
/// own order, as a fresh map would); the default keeps each tag's source
/// layout.
fn source_layout() -> bool {
    std::env::var("H2_SHUFFLE_LAYOUT").as_deref() != Ok("depth")
}

fn shuffle(stem: &str, map: &mut DiscMap) -> Res<()> {
    let started = std::time::Instant::now();
    let mut cov = BTreeMap::new();
    let relocator = coverage_of(map, &mut cov)?;
    // Freed space is reused, in reverse slot order so tags land elsewhere;
    // if fragmentation overflows the tag-memory bound, in slot order.
    let mut order = "reverse";
    let (ids, bytes) = match relocated_variant(map, true, source_layout())
        .and_then(|(set, ids)| Ok((ids, set.build()?.0)))
    {
        Ok(r) => r,
        Err(e) => {
            println!("{stem}: reverse order: {e}");
            order = "slot";
            let (mut set, ids) = relocated_variant(map, false, source_layout())?;
            match set.build() {
                Ok(b) => (ids, b.0),
                Err(e) => {
                    // How far over, and where the growth is.
                    set.check_tag_memory = false;
                    let over = Map::from_reader(Cursor::new(set.build()?.0))?;
                    let (mut before, mut after) = (0u64, 0u64);
                    for id in &ids {
                        before += u64::from(map.tag(*id)?.location.map_or(0, |l| l.size));
                        after += u64::from(over.tag(*id)?.location.map_or(0, |l| l.size));
                    }
                    println!(
                        "{stem}: slot order: {e}: memory {:#x} -> {:#x}, bound {:#x}; moved tags {before:#x} -> {after:#x} bytes",
                        map.header().meta_size,
                        over.header().meta_size,
                        map::writer::max_metadata_size(map.header().cache_type)?
                    );
                    return Err(e.into());
                }
            }
        }
    };
    let mut moved = Map::from_reader(Cursor::new(bytes))?;
    moved.validate_checksum()?;
    let new_mem = Memory::of(&mut moved)?;
    assert_eq!(moved.tags().len(), map.tags().len());
    assert_eq!(moved.strings(), map.strings());
    let schema = Schema::builtin();
    let moving: std::collections::HashSet<_> = ids.iter().copied().collect();
    let (mut same_decode, mut decoded, mut stayed) = (0, 0, 0);
    for t in map.tags().to_vec() {
        let u = moved.tag(t.id)?.clone();
        assert_eq!((t.group, &t.name), (u.group, &u.name));
        let (Some(a), Some(b)) = (t.location, u.location) else {
            assert_eq!(t.location.is_some(), u.location.is_some());
            continue;
        };
        if a.size == 0 {
            // An alias (another tag's bytes): it follows its owner.
            let owner = map.tags().iter().find(|o| {
                o.location.is_some_and(|l| {
                    l.space == a.space
                        && l.size > 0
                        && (l.address..l.address + l.size).contains(&a.address)
                })
            });
            let want = match owner {
                Some(o) => {
                    let (lo, ln) = (o.location.unwrap(), moved.tag(o.id)?.location.unwrap());
                    ln.address + (a.address - lo.address)
                }
                None => a.address,
            };
            assert_eq!(b.address, want, "{} alias lost its owner", t.id);
            continue;
        }
        if !moving.contains(&t.id) {
            let before = map.read_tag_bytes(t.id)?;
            let after = moved.read_tag_bytes(t.id)?;
            assert_eq!(a.address, b.address, "{} pinned tag moved", t.id);
            assert!(before == after, "{} pinned bytes changed", t.id);
            continue;
        }
        if a.address == b.address {
            stayed += 1;
        }
        let root = &schema.groups[&t.group];
        let x = read_tree(root, a.address, relocator.memory()).ok_or("old tree")?;
        let y = read_tree(root, b.address, &new_mem).ok_or("new tree")?;
        if x != y {
            return Err(format!("{} {} tree differs", t.group, t.id).into());
        }
        if tags::DecodedTag::supports(t.group) {
            decoded += 1;
            let p = tags::DecodedTag::read(map, t.id).map(|d| masked(&d.data));
            let q = tags::DecodedTag::read(&mut moved, t.id).map(|d| masked(&d.data));
            match (p, q) {
                (Ok(p), Ok(q)) if p == q => same_decode += 1,
                (Err(_), Err(_)) => same_decode += 1,
                _ => return Err(format!("{} {} decodes differently", t.group, t.id).into()),
            }
        }
    }
    println!(
        "{stem}: rebuilt {}/{} tags ({} layout) in {order} order ({stayed} at their old address), \
         trees identical, {same_decode}/{decoded} decoder results identical ({:?})",
        ids.len(),
        map.tags().len(),
        if source_layout() {
            "source"
        } else {
            "depth-first"
        },
        started.elapsed()
    );
    Ok(())
}

/// Debug form of decoded data with every address-looking hex value masked.
fn masked(data: &tags::TagData) -> String {
    let text = format!("{data:?}");
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(i) = rest.find("address: ") {
        out.push_str(&rest[..i + 9]);
        rest = &rest[i + 9..];
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric()))
            .unwrap_or(rest.len());
        out.push('_');
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// Print root words of one group: `H2_SCHEMA_PROBE=hlmt:0x58`.
#[test]
#[ignore = "needs the owner's ISOs"]
fn probe() -> Res<()> {
    let Ok(spec) = std::env::var("H2_SCHEMA_PROBE") else {
        return Ok(());
    };
    let (g, o) = spec.split_once(':').ok_or("group:offset")?;
    let g: FourCC = g.parse()?;
    let o = usize::from_str_radix(o.trim_start_matches("0x"), 16)?;
    let mut shown = 0;
    for (stem, mut map) in maps()? {
        let space = map.metadata_space();
        for t in map.tags().to_vec() {
            if t.group != g || shown > 40 {
                continue;
            }
            let Some(loc) = t.location else { continue };
            let b = map.read_tag_bytes(t.id)?;
            let w = |k: usize| u32::from_le_bytes(b[k..k + 4].try_into().unwrap());
            if b.len() < o + 12 {
                continue;
            }
            let owner = |v: u32| {
                map.tags()
                    .iter()
                    .find(|u| {
                        u.location
                            .is_some_and(|l| (l.address..l.address + l.size).contains(&v))
                    })
                    .map(|u| format!("{} {}", u.group, u.name.clone().unwrap_or_default()))
            };
            let odd = std::env::var_os("H2_SCHEMA_ODD").is_some();
            if odd && (w(o) == 0 || owner(w(o)).is_some()) {
                continue;
            }
            println!(
                "{stem} {:?} @{:#x}+{:#x}: {:#x} {:#x} {:#x} {:#x}  -> {:?} (meta {:x?})",
                t.name,
                loc.address,
                loc.size,
                w(o - 4),
                w(o),
                w(o + 4),
                w(o + 8),
                owner(w(o)),
                space.map(|s| (s.virtual_base, s.size))
            );
            shown += 1;
        }
    }
    Ok(())
}

/// Whole map files by stem, for raw reads.
#[derive(Clone, Default)]
struct Files([Vec<u8>; 4]);

fn sel(f: map::ResourceFile) -> usize {
    match f {
        map::ResourceFile::Local => 0,
        map::ResourceFile::MainMenu => 1,
        map::ResourceFile::Shared => 2,
        map::ResourceFile::SinglePlayerShared => 3,
    }
}

impl Files {
    fn insert(&mut self, f: map::ResourceFile, bytes: Vec<u8>) {
        self.0[sel(f)] = bytes;
    }
    fn read(&self, p: map::RawPointer, size: u32) -> tags::Result<Vec<u8>> {
        Some(&self.0[sel(p.file)])
            .and_then(|f| f.get(p.offset as usize..p.offset as usize + size as usize))
            .map(<[u8]>::to_vec)
            .ok_or(tags::Error::MissingResource(p.file))
    }
}
impl tags::geometry::RawSource for Files {
    fn read_raw(&mut self, p: map::RawPointer, size: u32) -> tags::Result<Vec<u8>> {
        self.read(p, size)
    }
}
impl tags::sound::ResourceReader for Files {
    fn check(&self, p: map::RawPointer, size: u32) -> tags::Result<()> {
        self.read(p, size).map(|_| ())
    }
    fn read_raw(&mut self, p: map::RawPointer, size: u32) -> tags::Result<Vec<u8>> {
        self.read(p, size)
    }
}

/// A tag's address-free canonical form: every element's bytes in visit
/// order, block addresses zeroed, tag references and datums as the target's
/// group and name, string IDs as text, raw pointers as their resource bytes
/// (paired length).
fn canonical(
    schema: &Schema,
    map: &Map<impl std::io::Read + std::io::Seek>,
    mem: &Memory,
    files: &Files,
    id: map::TagId,
) -> Res<Vec<String>> {
    use tags::schema::Field;
    let t = map.tag(id)?;
    let l = t.location.ok_or("no data")?;
    let root = &schema.groups[&t.group];
    let name_of = |d: u32| {
        map.tags()
            .iter()
            .find(|u| u.id.0 == d)
            .map_or("null".to_owned(), |u| format!("{} {:?}", u.group, u.name))
    };
    let mut out = Vec::new();
    let w =
        |e: &[u8], o: u32| u32::from_le_bytes(e[o as usize..o as usize + 4].try_into().unwrap());
    tags::schema::visit(root, l.address, mem, &mut |s, e, path| {
        let mut bytes = e.to_vec();
        let mut notes = Vec::new();
        for (&o, f) in &s.fields {
            let blank =
                |b: &mut Vec<u8>, at: u32, n: u32| b[at as usize..(at + n) as usize].fill(0);
            match f {
                Field::Block(_) => blank(&mut bytes, o + 4, 4),
                Field::TagRef => {
                    notes.push(name_of(w(e, o + 4)));
                    blank(&mut bytes, o + 4, 4);
                }
                Field::TagId => {
                    notes.push(name_of(w(e, o)));
                    blank(&mut bytes, o, 4);
                }
                Field::StringId => {
                    let v = w(e, o);
                    notes.push(
                        map.strings()
                            .get((v & 0xff_ffff) as usize)
                            .cloned()
                            .flatten()
                            .unwrap_or_default(),
                    );
                    blank(&mut bytes, o, 4);
                }
                Field::Raw => {
                    let size = s.fields.iter().find_map(|(&k, f)| {
                        matches!(f, Field::RawSize(of) if *of == o).then(|| w(e, k))
                    });
                    let v = w(e, o);
                    if let (Some(n), Some(p)) = (size, map::RawPointer::decode(v)) {
                        let data = files.read(p, n).map_or_else(
                            |e| format!("{e}"),
                            |d| {
                                use std::hash::{Hash, Hasher};
                                let mut h = std::collections::hash_map::DefaultHasher::new();
                                d.hash(&mut h);
                                format!("{:016x}", h.finish())
                            },
                        );
                        notes.push(data);
                    }
                    blank(&mut bytes, o, 4);
                }
                _ => {}
            }
        }
        out.push(format!("{path:x?} {bytes:02x?} {notes:?}"));
    })
    .ok_or("visit")?;
    Ok(out)
}

/// Transplant: copy a weapon (the SMG by default) and every tag it
/// references from one retail map into another that lacks it, offline,
/// raw resources included; build; read back. Every copied tag must have the
/// same canonical form (references by name, strings by text, raw resources
/// by their bytes) and the same decoder results (bitmap levels of detail,
/// render model geometry, animation resources) as in the source.
/// `H2_TRANSPLANT=group:name` picks another tag, `H2_TRANSPLANT_FROM` the
/// source map (default: the first that has it); the destination is the
/// first map of the same cache type without it.
#[test]
#[ignore = "needs the owner's ISOs"]
fn transplant_a_weapon_between_maps() -> Res<()> {
    let spec = std::env::var("H2_TRANSPLANT")
        .unwrap_or_else(|_| "weap:objects\\weapons\\rifle\\smg\\smg".to_owned());
    let (g, name) = spec.split_once(':').ok_or("group:name")?;
    let group: FourCC = g.parse()?;
    let mut all = maps()?;
    let mut shared = Files::default();
    for (stem, m) in &mut all {
        let file = match stem.as_str() {
            "mainmenu" => map::ResourceFile::MainMenu,
            "shared" => map::ResourceFile::Shared,
            "single_player_shared" => map::ResourceFile::SinglePlayerShared,
            _ => continue,
        };
        shared.insert(file, whole(m)?);
    }
    let has = |m: &DiscMap| {
        m.tags()
            .iter()
            .any(|t| t.group == group && t.name.as_deref() == Some(name))
    };
    let from = std::env::var("H2_TRANSPLANT_FROM").ok();
    let si = all
        .iter()
        .position(|(stem, m)| has(m) && from.as_ref().is_none_or(|f| f == stem))
        .ok_or("no map has the tag")?;
    let kind = all[si].1.header().cache_type;
    let di = all
        .iter()
        .position(|(_, m)| m.header().cache_type == kind && !has(m))
        .ok_or("every map of that type has the tag")?;
    let (src_stem, dst_stem) = (all[si].0.clone(), all[di].0.clone());
    let (mut src, mut dst) = if si < di {
        let d = all.swap_remove(di).1;
        (all.swap_remove(si).1, d)
    } else {
        let s = all.swap_remove(si).1;
        (s, all.swap_remove(di).1)
    };
    let root = src
        .tags()
        .iter()
        .find(|t| t.group == group && t.name.as_deref() == Some(name))
        .unwrap()
        .id;

    let started = std::time::Instant::now();
    let mut set = map::build::TagSet::from_map(&mut dst)?;
    let before = dst.tags().len();
    let schema = Schema::builtin();
    println!("{src_stem} -> {dst_stem}");
    let report = match tags::schema::transplant::transplant(schema, &mut src, root, &mut set)? {
        Ok(r) => r,
        Err(refused) => {
            for r in &refused {
                println!(
                    "refused: {} {} ({:?} at {:x?} +{:#x}, value {:#x})",
                    r.group,
                    r.name,
                    r.detail.refusal,
                    r.detail.path,
                    r.detail.offset,
                    r.detail.value
                );
            }
            return Err(format!("{} tags of the tree are not relocatable", refused.len()).into());
        }
    };
    let (bytes, _) = set.build().map_err(|e| format!("build: {e}"))?;
    let mut out = Map::from_reader(Cursor::new(bytes.clone()))?;
    out.validate_checksum()?;
    let reused = report.ids.values().filter(|d| d.index() < before).count();
    assert_eq!(out.tags().len(), before + report.ids.len() - reused);

    let mut src_files = shared.clone();
    let mut out_files = shared;
    src_files.insert(map::ResourceFile::Local, whole(&mut src)?);
    out_files.insert(map::ResourceFile::Local, bytes);
    let src_mem = Relocator::new(schema, &mut src)?.memory().clone();
    let out_mem = Memory::of(&mut out)?;
    let (mut decoded, mut raw_checked) = (0, 0usize);
    for &id in &report.added {
        let new = report.ids[&id];
        let g = src.tag(id)?.group;
        let a = canonical(schema, &src, &src_mem, &src_files, id)?;
        let b = canonical(schema, &out, &out_mem, &out_files, new)?;
        if a != b {
            let i = a
                .iter()
                .zip(&b)
                .position(|(x, y)| x != y)
                .unwrap_or(a.len().min(b.len()));
            return Err(format!(
                "{g} {:?}: canonical form differs at element {i}:\n {:?}\n {:?}",
                src.tag(id)?.name,
                a.get(i),
                b.get(i)
            )
            .into());
        }
        match &g.0 {
            b"bitm" => {
                let (Ok(x), Ok(y)) = (
                    tags::DecodedTag::read(&mut src, id),
                    tags::DecodedTag::read(&mut out, new),
                ) else {
                    return Err(format!("bitmap {:?} does not decode", src.tag(id)?.name).into());
                };
                let (tags::TagData::Bitmap(x), tags::TagData::Bitmap(y)) = (&x.data, &y.data)
                else {
                    unreachable!()
                };
                assert_eq!(x.images.len(), y.images.len());
                for (p, q) in x.images.iter().zip(&y.images) {
                    for (r, s) in p.lods.iter().zip(&q.lods) {
                        match (r, s) {
                            (Some(r), Some(s)) => {
                                assert_eq!(r.size, s.size);
                                assert_eq!(
                                    src_files.read(r.pointer, r.size)?,
                                    out_files.read(s.pointer, s.size)?,
                                    "bitmap level bytes"
                                );
                                raw_checked += 1;
                            }
                            (None, None) => {}
                            _ => panic!("level of detail lost"),
                        }
                    }
                }
                decoded += 1;
            }
            b"mode" => {
                let x = tags::model::RenderModelGeometry::read(&mut src, id, &mut src_files)?;
                let y = tags::model::RenderModelGeometry::read(&mut out, new, &mut out_files)?;
                // Names by text; geometry, node maps and vertices as decoded
                // (pointers and IDs differ by design).
                let names = |m: &tags::model::RenderModelGeometry| {
                    (
                        m.name.clone(),
                        m.regions.iter().map(|r| r.name.clone()).collect::<Vec<_>>(),
                        m.nodes.iter().map(|n| n.name.clone()).collect::<Vec<_>>(),
                    )
                };
                assert_eq!(names(&x), names(&y), "render model names");
                assert_eq!(x.sections.len(), y.sections.len());
                for (p, q) in x.sections.iter().zip(&y.sections) {
                    let (g, h) = (&p.geometry, &q.geometry);
                    assert_eq!(g.section_struct(), h.section_struct(), "section struct");
                    assert_eq!(format!("{:?}", g.parts), format!("{:?}", h.parts), "parts");
                    assert_eq!(
                        format!("{:?}", g.subparts),
                        format!("{:?}", h.subparts),
                        "subparts"
                    );
                    assert_eq!(g.indices, h.indices, "indices");
                    assert_eq!(g.vertex_buffers.len(), h.vertex_buffers.len());
                    for (v, w) in g.vertex_buffers.iter().zip(&h.vertex_buffers) {
                        assert_eq!(g.data(v), h.data(w), "vertex data");
                    }
                    assert_eq!(p.node_map, q.node_map);
                    raw_checked += 1;
                }
                decoded += 1;
            }
            b"jmad" => {
                let x = tags::anim::AnimationGraph::read(&mut src, id)?;
                let y = tags::anim::AnimationGraph::read(&mut out, new)?;
                for i in 0..x.resources.len() {
                    if x.resources[i].pointer.is_none() {
                        continue;
                    }
                    assert_eq!(
                        x.read_resource(i, &mut src_files)?,
                        y.read_resource(i, &mut out_files)?,
                        "animation resource"
                    );
                    raw_checked += 1;
                }
                decoded += 1;
            }
            _ => {}
        }
    }
    let mut by_group: BTreeMap<String, u32> = BTreeMap::new();
    for id in &report.added {
        *by_group.entry(src.tag(*id)?.group.to_string()).or_default() += 1;
    }
    println!(
        "{group} {name}: {src_stem} -> {dst_stem}: {} tags in the tree, {} copied ({by_group:?}), {} reused, {} dropped; {} raw resources ({} bytes) copied, {} external kept; canonical forms identical; {decoded} decoded identically, {raw_checked} raw resources compared ({:?})",
        report.ids.len() + report.dropped.len(),
        report.added.len(),
        report.ids.len() - report.added.len(),
        report.dropped.len(),
        report.raw.0,
        report.raw.1,
        report.external,
        started.elapsed()
    );
    Ok(())
}

/// A map's whole file, in reads under the reader's cap.
fn whole(m: &mut DiscMap) -> Res<Vec<u8>> {
    let n = u64::from(m.header().file_size);
    let mut out = Vec::with_capacity(n as usize);
    let mut at = 0;
    while at < n {
        let k = (n - at).min(32 << 20);
        out.extend(m.read_file_range(at, k)?);
        at += k;
    }
    Ok(out)
}
