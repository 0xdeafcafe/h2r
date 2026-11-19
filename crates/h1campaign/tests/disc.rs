//! a30 converted onto its Halo 2 base, from the owner's discs (`$H1_ISO`,
//! `$H2_DISC`), and read back. Nothing is written to disk; counts go to
//! stdout.

use std::fs::File;
use std::io::{BufReader, Cursor};

use h1campaign::{Options, Scripting};
use map::{FourCC, Map, TagId};

fn disc(var: &str, name: &str) -> String {
    std::env::var(var)
        .unwrap_or_else(|_| format!("{}/../../disc/{name}", env!("CARGO_MANIFEST_DIR")))
}

fn h1_iso() -> xiso::Xiso<BufReader<File>> {
    xiso::Xiso::open(BufReader::new(
        File::open(disc("H1_ISO", "halo1.iso")).unwrap(),
    ))
    .unwrap()
}

fn h1_level(name: &str) -> h1map::H1Map {
    let mut iso = h1_iso();
    let d = h1map::disc::Disc::open(&mut iso).unwrap();
    let e = d.map(name).unwrap().clone();
    h1map::H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap()
}

fn h2_iso() -> xiso::Xiso<BufReader<File>> {
    xiso::Xiso::open(BufReader::new(
        File::open(disc("H2_DISC", "halo2.iso")).unwrap(),
    ))
    .unwrap()
}

fn h2_bytes(name: &str) -> Vec<u8> {
    let mut iso = h2_iso();
    let e = iso
        .entries()
        .unwrap()
        .into_iter()
        .find(|e| e.path.ends_with(&format!("/{name}.map")))
        .unwrap();
    iso.read(&e).unwrap()
}

fn h2_map(name: &str) -> Map<Cursor<Vec<u8>>> {
    Map::from_reader(Cursor::new(h2_bytes(name))).unwrap()
}

/// The donor maps (`h1campaign::donor::MAPS`) off the Halo 2 disc.
fn donors() -> h1campaign::donor::Donors {
    let mut d = h1campaign::donor::Donors::default();
    for name in h1campaign::donor::MAPS {
        d.maps.push((name.to_owned(), h2_bytes(name)));
    }
    d
}

fn scripting() -> Scripting {
    let mut iso = h1_iso();
    let e = iso.find("/default.xbe").unwrap();
    let h1_xbe = iso.read(&e).unwrap();
    let mut iso = h2_iso();
    let e = iso.find("default.xbe").unwrap();
    let h2_xbe = iso.read(&e).unwrap();
    let mut scenarios = Vec::new();
    for e in iso
        .entries()
        .unwrap()
        .into_iter()
        .filter(|e| !e.is_dir && e.path.ends_with(".map"))
    {
        let mut m = Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap();
        scenarios.push(hs::Scenario::read(&mut m).unwrap());
    }
    Scripting::new(&h1_xbe, &h2_xbe, &scenarios).unwrap()
}

/// Every tag reference in the tree of `root` at `address`: (offset path,
/// group as stored, datum).
fn tag_refs(
    s: &tags::schema::Struct,
    address: u32,
    count: u32,
    mem: &tags::schema::Memory,
    out: &mut Vec<(u32, FourCC, TagId)>,
    depth: usize,
) {
    if depth > 12 || count == 0 {
        return;
    }
    let Some(bytes) = mem.slice(address, s.size * count) else {
        return;
    };
    let bytes = bytes.to_vec();
    for i in 0..count {
        let e = (i * s.size) as usize;
        for (&at, f) in &s.fields {
            let w = |o: usize| u32::from_le_bytes(bytes[e + o..e + o + 4].try_into().unwrap());
            match f {
                tags::schema::Field::TagRef => {
                    let g = FourCC(w(at as usize).to_be_bytes());
                    out.push((address + e as u32 + at, g, TagId(w(at as usize + 4))));
                }
                tags::schema::Field::Block(child) => {
                    let n = w(at as usize);
                    let a = w(at as usize + 4);
                    if n > 0 && n < 100_000 {
                        tag_refs(child, a, n, mem, out, depth + 1);
                    }
                }
                _ => {}
            }
        }
    }
}

/// Groups whose tags are objects: their references are followed.
const OBJECT_GROUPS: [&[u8; 4]; 12] = [
    b"bipd", b"vehi", b"weap", b"eqip", b"garb", b"proj", b"scen", b"mach", b"ctrl", b"ssce",
    b"bloc", b"crea",
];

/// Per group, the root block fields (`tags::schema` offsets) that hold at
/// least one element on every retail tag of that group (groups with at
/// least 20 tags: 05a's five skies all have a bitmap layer, but 21 of the
/// disc's 91 have none). The engine indexes some such blocks without a count
/// check (a30's second fault: `mode +0x34` section groups, read at
/// `0x0013767a`), so a converted tag that leaves one empty is suspect.
type Filled = std::collections::HashMap<FourCC, std::collections::BTreeSet<u32>>;

fn map_memory(m: &mut Map<Cursor<Vec<u8>>>) -> tags::schema::Memory {
    let space = m.metadata_space().unwrap();
    let meta = m
        .read_file_range(space.file_offset, u64::from(space.size))
        .unwrap();
    tags::schema::Memory::from_regions(
        vec![(space.virtual_base, meta)],
        vec![(space.virtual_base, space.virtual_base + space.size)],
    )
}

fn root_blocks(st: &tags::schema::Struct) -> impl Iterator<Item = u32> + '_ {
    st.fields
        .iter()
        .filter(|(_, f)| matches!(f, tags::schema::Field::Block(_)))
        .map(|(at, _)| *at)
}

fn count_at(mem: &tags::schema::Memory, a: u32) -> u32 {
    mem.slice(a, 4)
        .map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()))
}

/// [`Filled`] from a retail map.
fn filled_profile(m: &mut Map<Cursor<Vec<u8>>>) -> Filled {
    let mem = map_memory(m);
    let schema = tags::schema::Schema::builtin();
    let mut seen: std::collections::HashMap<FourCC, (usize, std::collections::BTreeSet<u32>)> =
        Default::default();
    for t in m.tags() {
        let (Some(loc), Some(st)) = (t.location, schema.groups.get(&t.group)) else {
            continue;
        };
        let full: std::collections::BTreeSet<u32> = root_blocks(st)
            .filter(|at| count_at(&mem, loc.address + at) > 0)
            .collect();
        let e = seen.entry(t.group).or_insert_with(|| (0, full.clone()));
        e.0 += 1;
        e.1 = e.1.intersection(&full).copied().collect();
    }
    seen.into_iter()
        .filter(|(_, (n, s))| *n >= 20 && !s.is_empty())
        .map(|(g, (_, s))| (g, s))
        .collect()
}

/// Root blocks the base map's retail tags always fill but other retail
/// maps leave empty somewhere, so empty is a retail state: `vehi +0x1c8`,
/// the seats (06a_sentinelwalls has three vehicles with none; a10's Halo
/// ring and b30's falling box, built from the `vehi` definition, have
/// none in Halo 1 either).
const RETAIL_MAY_LEAVE_EMPTY: &[(FourCC, u32)] = &[(FourCC(*b"vehi"), 0x1c8)];

/// Root blocks of the tag `id` that every retail tag of its group fills
/// and this one leaves empty.
fn empty_where_retail_fills(
    m: &Map<Cursor<Vec<u8>>>,
    mem: &tags::schema::Memory,
    filled: &Filled,
    id: TagId,
) -> Vec<String> {
    let Ok(t) = m.tag(id) else { return Vec::new() };
    let (Some(loc), Some(want)) = (t.location, filled.get(&t.group)) else {
        return Vec::new();
    };
    want.iter()
        .filter(|at| !RETAIL_MAY_LEAVE_EMPTY.contains(&(t.group, **at)))
        .filter(|at| count_at(mem, loc.address + **at) == 0)
        .map(|at| {
            format!(
                "{} {}: block +{at:#x} is empty; every retail {} fills it",
                t.group,
                t.name.as_deref().unwrap_or("?"),
                t.group
            )
        })
        .collect()
}

/// The object sweep over a built map: from `roots` (palette entries and
/// AI characters), every object, `hlmt` and `char` reachable through tag
/// references is checked, and each reference it holds must name a tag of
/// its group (or a descendant) whose data is present. On each `hlmt`, the
/// node count must cover the render model's nodes and the physics model's
/// rigid-body nodes: `object_new` sizes an object's node matrices by the
/// `hlmt` count (at least one), and the model, graph and physics write one
/// per their node (a30's first crash: converted `hlmt`s had none).
/// Every tag reached, and every tag one of them names, is also checked
/// against `filled` (root blocks retail always fills).
/// Returns (tags checked, problems).
fn object_sweep(
    m: &Map<Cursor<Vec<u8>>>,
    mem: &tags::schema::Memory,
    roots: &[TagId],
    filled: &Filled,
) -> (usize, Vec<String>) {
    let mut shape_checked = std::collections::HashSet::new();
    let schema = tags::schema::Schema::builtin();
    let word = |a: u32| {
        mem.slice(a, 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    };
    let half = |a: u32| {
        mem.slice(a, 2)
            .map(|b| i16::from_le_bytes(b.try_into().unwrap()))
    };
    let descends = |g: FourCC, of: FourCC| {
        g == of
            || m.groups()
                .iter()
                .any(|x| x.class == g && (x.parent == Some(of) || x.grandparent == Some(of)))
    };
    let unloaded = |g: FourCC| g == FourCC(*b"sbsp") || g == FourCC(*b"ltmp");
    let follow = |g: FourCC| {
        OBJECT_GROUPS.iter().any(|o| g.0 == **o) || [*b"hlmt", *b"char", *b"sky "].contains(&g.0)
    };
    let mut queue: Vec<TagId> = roots
        .iter()
        .copied()
        .filter(|t| *t != TagId::NULL)
        .collect();
    let mut seen = std::collections::HashSet::new();
    let mut bad = Vec::new();
    while let Some(id) = queue.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Ok(t) = m.tag(id) else {
            bad.push(format!("{id:?} names no tag"));
            continue;
        };
        let label = format!("{} {}", t.group, t.name.as_deref().unwrap_or("?"));
        let Some(loc) = t.location else {
            bad.push(format!("{label}: no data"));
            continue;
        };
        let Some(st) = schema.groups.get(&t.group) else {
            continue;
        };
        if shape_checked.insert(id) {
            bad.extend(empty_where_retail_fills(m, mem, filled, id));
        }
        let mut refs = Vec::new();
        tag_refs(st, loc.address, 1, mem, &mut refs, 0);
        for (at, g, r) in refs {
            if r == TagId::NULL || g.0 == [0xff; 4] {
                continue;
            }
            if shape_checked.insert(r) {
                bad.extend(empty_where_retail_fills(m, mem, filled, r));
            }
            match m.tag(r) {
                Err(_) => bad.push(format!(
                    "{label} +{:#x}: {g} {r:?} names no tag",
                    at - loc.address
                )),
                Ok(rt) if !descends(rt.group, g) => bad.push(format!(
                    "{label} +{:#x}: {g} names a {}",
                    at - loc.address,
                    rt.group
                )),
                Ok(rt) if rt.location.is_none() && !unloaded(rt.group) => bad.push(format!(
                    "{label} +{:#x}: {} {} has no data",
                    at - loc.address,
                    rt.group,
                    rt.name.as_deref().unwrap_or("?")
                )),
                Ok(rt) if follow(rt.group) => queue.push(r),
                Ok(_) => {}
            }
        }
        if t.group == FourCC(*b"hlmt") {
            let a = loc.address;
            let nodes = word(a + 0x78).unwrap_or(0);
            let mode = TagId(word(a + 4).unwrap_or(u32::MAX));
            if let Some(ml) = m.tag(mode).ok().and_then(|t| t.location) {
                let mn = word(ml.address + 0x48).unwrap_or(0);
                if nodes < mn.max(1) {
                    bad.push(format!("{label}: {nodes} nodes, its render model has {mn}"));
                }
            }
            let phmo = TagId(word(a + 0x24).unwrap_or(u32::MAX));
            if let Some(pl) = m.tag(phmo).ok().and_then(|t| t.location) {
                let n = word(pl.address + 0x38).unwrap_or(0);
                let at = word(pl.address + 0x3c).unwrap_or(0);
                let top = (0..n)
                    .filter_map(|i| half(at + i * 0x90))
                    .max()
                    .unwrap_or(-1);
                if i64::from(top) >= i64::from(nodes.max(1)) {
                    bad.push(format!(
                        "{label}: physics rigid bodies reach node {top}, the hlmt has {nodes}"
                    ));
                }
            }
        }
    }
    (seen.len(), bad)
}

/// Every object palette entry of the scenario at `addr` (`bytes` its
/// root), by `tags::object::OBJECT_TYPES`.
fn palette_roots(bytes: &[u8], addr: u32, mem: &tags::schema::Memory) -> Vec<TagId> {
    let word = |b: &[u8], at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
    let _ = addr;
    let mut out = Vec::new();
    for ty in tags::object::OBJECT_TYPES {
        let Some((_, pal, _)) = ty.placement else {
            continue;
        };
        let n = word(bytes, pal as usize);
        let a = word(bytes, pal as usize + 4);
        if let Some(p) = mem.slice(a, n * 0x28) {
            out.extend((0..n as usize).map(|i| TagId(word(p, i * 0x28 + 4))));
        }
    }
    out
}

/// The sweep over a whole map: its scenario's palettes, skies and AI characters,
/// checked against `filled` (from the retail base).
fn sweep_map(m: &mut Map<Cursor<Vec<u8>>>, filled: &Filled) -> (usize, Vec<String>) {
    let scnr = m.scenario().unwrap().clone();
    let addr = scnr.location.unwrap().address;
    let mem = map_memory(m);
    let bytes = m.read_tag_bytes(scnr.id).unwrap();
    let mut roots = palette_roots(&bytes, addr, &mem);
    // The skies (`scnr +0x8`, a reference each): their models are converted too.
    let skies = count_at(&mem, addr + 8);
    let at = count_at(&mem, addr + 0xc);
    roots.extend((0..skies).map(|i| TagId(count_at(&mem, at + 8 * i + 4))));
    let view = tags::geometry::TagView::new(&bytes, addr);
    if let Ok(ai) = h1ai::h2::ScenarioAi::read(&view) {
        roots.extend(ai.character_palette.iter().copied());
    }
    object_sweep(m, &mem, &roots, filled)
}

/// Placement scales (`+0x20`, f32) of the weapons, equipment and vehicles
/// placed, by bit pattern: (zeros, ones, others). Zero is retail's usual
/// value (05a: 528 of 634 placements, all types), so a converted zero
/// is not a defect.
fn placement_scales(m: &mut Map<Cursor<Vec<u8>>>) -> Vec<(&'static str, [usize; 3])> {
    let scnr = m.scenario().unwrap().clone();
    let mem = map_memory(m);
    let root = scnr.location.unwrap().address;
    let mut out = Vec::new();
    for ty in tags::object::OBJECT_TYPES {
        let Some((at, _, stride)) = ty.placement else {
            continue;
        };
        let n = count_at(&mem, root + u32::from(at));
        let a = count_at(&mem, root + u32::from(at) + 4);
        let mut c = [0; 3];
        for i in 0..n {
            c[match count_at(&mem, a + i * u32::from(stride) + 0x20) {
                0 => 0,
                0x3f80_0000 => 1,
                _ => 2,
            }] += 1;
        }
        if n > 0 {
            out.push((ty.group, c));
        }
    }
    out
}

/// The sweep finds nothing on the base level as shipped, so a problem it
/// reports on a converted map is the converter's. With `$H1_SWEEP_MAP` (a
/// built `.map`), that map is swept too: an existing conversion is checked
/// without converting again.
/// Every Halo 1 BSP has a descriptor, in order, and per-BSP scenario
/// data; each descriptor leads to its region, the BSP validates (collision,
/// audibility, sound clusters), and its pathfinding decodes and validates.
/// Returns the pathfinding read back.
fn check_bsps(
    m: &mut Map<Cursor<Vec<u8>>>,
    h1: &h1map::H1Map,
    done: &h1campaign::Converted,
) -> Vec<tags::pathfinding::PathfindingData> {
    let scnr = m.scenario().unwrap().clone();
    let addr = scnr.location.unwrap().address;
    let bytes = m.read_tag_bytes(scnr.id).unwrap();
    let word = |b: &[u8], at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
    let n = h1.structure_bsps().unwrap().len();
    assert_eq!(done.report.bsps.len(), n, "every Halo 1 BSP converts");
    assert_eq!(word(&bytes, 0x210) as usize, n, "one descriptor per BSP");
    assert_eq!(word(&bytes, 0x290) as usize, n, "cluster data per BSP");
    assert_eq!(word(&bytes, 0x388) as usize, n, "points per BSP");
    let desc = (word(&bytes, 0x214) - addr) as usize;
    let clusters = (word(&bytes, 0x294) - addr) as usize;
    let mut decoded = Vec::new();
    let mut regions = Vec::new();
    for k in 0..n {
        let e = desc + k * 0x44;
        let sbsp = TagId(word(&bytes, e + 0x14));
        let t = m.tag(sbsp).unwrap().clone();
        assert_eq!(t.group, FourCC(*b"sbsp"));
        let ltmp = m.tag(TagId(word(&bytes, e + 0x1c))).unwrap().clone();
        assert_eq!(ltmp.group, FourCC(*b"ltmp"));
        let loc = t.location.unwrap();
        assert_eq!(
            loc.address,
            word(&bytes, e + 8) + 0x10,
            "bsp {k}: region header"
        );
        regions.push((word(&bytes, e), word(&bytes, e + 4)));
        let b = m.read_tag_bytes(sbsp).unwrap();
        // The cluster data matches the BSP: one entry per cluster, its
        // checksum.
        let c = clusters + k * 0x34;
        for at in [0x08, 0x10, 0x1c, 0x24, 0x2c] {
            assert_eq!(
                word(&bytes, c + at),
                word(&b, 0x9c),
                "bsp {k}: cluster data +{at:#x}"
            );
        }
        assert_eq!(word(&bytes, c + 0x18), word(&b, 0x8), "bsp {k}: checksum");
        let view = tags::geometry::TagView::new(&b, loc.address);
        let h = view.at(0, 0x200).unwrap();
        let coll = view
            .block(
                h,
                tags::pathfinding::SBSP_COLLISION_FIELD,
                tags::collision::Bsp::STRUCTURE_SIZE,
            )
            .unwrap();
        let off = coll[0].as_ptr() as usize - b.as_ptr() as usize;
        let col = tags::collision::Bsp::decode_structure(&b, loc.address, off).unwrap();
        // Surfaces with no material hold 0xffff in both games.
        let problems: Vec<String> = col
            .validate(word(&b, 0xc) as usize)
            .into_iter()
            .filter(|p| !p.ends_with("material 65535 out of range"))
            .collect();
        assert!(problems.is_empty(), "bsp {k}: {problems:?}");
        // The engine's unchecked reads every tick (audibility, sound
        // clusters): the first converted Blood Gulch faulted on both.
        h1convert::structure::check_audibility(&b, loc.address)
            .unwrap_or_else(|e| panic!("bsp {k}: {e}"));
        h1convert::structure::check_sound_clusters(&b, loc.address)
            .unwrap_or_else(|e| panic!("bsp {k}: {e}"));
        let pf = tags::pathfinding::PathfindingData::from_sbsp(&view)
            .unwrap()
            .expect("pathfinding present");
        let problems = pf.validate(Some(col.surfaces.len()));
        assert!(problems.is_empty(), "bsp {k} pathfinding: {problems:?}");
        assert_eq!(pf.sectors.len(), done.pathfinding[k].sectors.len());
        println!(
            "bsp {k}: {} surfaces, {} sectors, {} links read back",
            col.surfaces.len(),
            pf.sectors.len(),
            pf.links.len()
        );
        decoded.push(pf);
    }
    // BSP switch trigger volumes: Halo 1's, each on a converted volume
    // (named as Halo 1's) between two BSPs the scenario has.
    let h1_switches = h1campaign::scenario::h1::BSP_SWITCHES;
    let h1_volumes = h1campaign::scenario::trigger_volumes(h1).unwrap();
    let volumes = word(&bytes, 0x108) as usize;
    let parked = volumes - h1_volumes.len();
    let n_switches = word(&bytes, 0x130) as usize;
    assert_eq!(n_switches, done.report.scenario.bsp_switches);
    assert!(h1_switches.1 == 8 && n_switches > 0, "every level switches");
    let sw = (word(&bytes, 0x134).wrapping_sub(addr)) as usize;
    for k in 0..n_switches {
        let e = &bytes[sw + k * 0xe..][..0xe];
        let s = |o: usize| i16::from_le_bytes([e[o], e[o + 1]]);
        let v = s(0) as usize;
        assert!(v >= parked && v < volumes, "switch {k}: volume {v}");
        assert!((0..n as i16).contains(&s(2)) && (0..n as i16).contains(&s(4)));
    }
    // No two regions share file bytes.
    regions.sort();
    for w in regions.windows(2) {
        assert!(w[0].0 + w[0].1 <= w[1].0, "regions overlap: {w:x?}");
    }
    decoded
}

#[test]
#[ignore = "reads disc/halo2.iso"]
fn object_sweep_is_clean_on_the_retail_base() {
    let mut base = h2_map(h1campaign::BASE);
    let filled = filled_profile(&mut base);
    let (n, bad) = sweep_map(&mut base, &filled);
    println!("{}: {n} tags swept", h1campaign::BASE);
    println!(
        "placement scales (0, 1, other): {:?}",
        placement_scales(&mut base)
    );
    assert!(bad.is_empty(), "{bad:#?}");
    if let Ok(path) = std::env::var("H1_SWEEP_MAP") {
        let mut m = Map::from_reader(Cursor::new(std::fs::read(&path).unwrap())).unwrap();
        let (n, bad) = sweep_map(&mut m, &filled);
        println!("{path}: {n} tags swept, {} problems", bad.len());
        println!(
            "placement scales (0, 1, other): {:?}",
            placement_scales(&mut m)
        );
        assert!(bad.is_empty(), "{bad:#?}");
    }
}

#[test]
#[ignore = "reads disc/halo1.iso and disc/halo2.iso"]
fn h1_trigger_volumes_read() {
    let h1 = h1_level("a30");
    let vols = h1campaign::scenario::trigger_volumes(&h1).unwrap();
    println!("a30 trigger volumes: {}", vols.len());
    for v in vols.iter().take(5) {
        println!("  {v:?}");
    }
    assert!(!vols.is_empty());
    assert!(
        vols.iter().all(
            |v| !v.name.is_empty() && v.name.bytes().all(|b| b.is_ascii_graphic() || b == b' ')
        )
    );
    let unit = |v: [f32; 3]| ((v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt() - 1.0).abs() < 1e-3;
    for v in &vols {
        assert!(unit(v.forward) && unit(v.up), "{v:?}");
        assert!(v.extents.iter().all(|e| e.is_finite()), "{v:?}");
    }
}

#[test]
#[ignore = "reads disc/halo1.iso and disc/halo2.iso"]
fn a30_converts_onto_its_base_and_reads_back() {
    let h1 = h1_level("a30");
    let mut base = h2_map(h1campaign::BASE);
    let filled = filled_profile(&mut base);
    let layers = std::env::var("H1_LAYERS").unwrap_or_else(|_| "objects,ai,scripts".into());
    let s = layers.contains("scripts").then(scripting);
    let donors = donors();
    let opts = Options {
        objects: layers.contains("objects"),
        ai: layers.contains("ai"),
        scripts: s.as_ref(),
        sounds: std::env::var("H1_SOUNDS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1000),
        donors: Some(&donors),
        ..Default::default()
    };
    let done = h1campaign::convert(&h1, &mut base, &opts).unwrap();
    println!("{}", done.report);

    let mut m = Map::from_reader(Cursor::new(done.bytes.clone())).unwrap();
    m.validate_checksum().unwrap();
    assert_eq!(m.header().name, "a30");
    assert_eq!(m.header().scenario_path, done.scenario_path);
    let scnr = m.scenario().unwrap().clone();
    assert_eq!(scnr.group, FourCC(*b"scnr"));
    assert_eq!(scnr.name.as_deref(), Some(done.scenario_path.as_str()));
    let addr = scnr.location.unwrap().address;
    let bytes = m.read_tag_bytes(scnr.id).unwrap();
    let word = |b: &[u8], at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());

    // Every tag reference in the scenario resolves to a tag of its group.
    // The whole ordinary metadata, unowned bytes included (the base
    // scenario's old children are no tag's now).
    let space = m.metadata_space().unwrap();
    let meta = m
        .read_file_range(space.file_offset, u64::from(space.size))
        .unwrap();
    let mem = tags::schema::Memory::from_regions(
        vec![(space.virtual_base, meta)],
        vec![(space.virtual_base, space.virtual_base + space.size)],
    );
    let schema = tags::schema::Schema::builtin();
    let mut refs = Vec::new();
    tag_refs(
        &schema.groups[&FourCC(*b"scnr")],
        addr,
        1,
        &mem,
        &mut refs,
        0,
    );
    let (mut live, mut bad) = (0, Vec::new());
    for (at, g, id) in &refs {
        if *id == TagId::NULL {
            continue;
        }
        live += 1;
        match m.tag(*id) {
            Ok(t) if t.group == *g => {}
            Ok(t) => bad.push(format!("{at:#x}: {g} names a {}", t.group)),
            Err(_) => bad.push(format!("{at:#x}: {g} {id:?} names no tag")),
        }
    }
    println!("scenario tag references: {} ({live} live)", refs.len());
    assert!(bad.is_empty(), "{bad:#?}");

    let decoded = check_bsps(&mut m, &h1, &done);

    // The player starts are a30's and stand on generated pathfinding.
    let starts = h1.multiplayer().unwrap().starting_locations;
    let at = (word(&bytes, 0x104) - addr) as usize;
    assert_eq!(word(&bytes, 0x100) as usize, starts.len());
    let first = &bytes[at..at + 0x34];
    let pos: [f32; 3] =
        std::array::from_fn(|i| f32::from_le_bytes(first[4 * i..4 * i + 4].try_into().unwrap()));
    assert_eq!(pos, starts[0].position);
    let on = h1campaign::on_sector(&decoded[usize::from(starts[0].bsp)], pos, 1.0, 0.5);
    println!("player start {pos:?}: on a generated sector: {on}");
    assert!(on, "the player start is on no generated sector");

    // The sky.
    let skies = word(&bytes, 0x8);
    assert!(skies > 0);
    let sky = TagId(word(&bytes, (word(&bytes, 0xc) - addr) as usize + 4));
    assert_eq!(m.tag(sky).unwrap().group, FourCC(*b"sky "));

    // The campaign players (globals player representation 0 and 1) are
    // the converted Halo 1 cyborg.
    if opts.objects {
        let matg = m
            .tags()
            .iter()
            .find(|t| t.group == FourCC(*b"matg"))
            .unwrap()
            .clone();
        let g = m.read_tag_bytes(matg.id).unwrap();
        let ga = matg.location.unwrap().address;
        let rep = (word(&g, 0x13c) - ga) as usize;
        for i in 0..2 {
            let unit = TagId(word(&g, rep + i * 0xbc + 0xb4));
            let t = m.tag(unit).unwrap();
            assert_eq!(t.group, FourCC(*b"bipd"));
            assert_eq!(
                t.name.as_deref(),
                Some(h1campaign::scenario::PLAYER_BIPED),
                "player {i}"
            );
        }
        println!(
            "campaign players 0 and 1: {}",
            h1campaign::scenario::PLAYER_BIPED
        );
    }

    // Objects: every placement's palette entry names a converted tag.
    if opts.objects {
        for (k, (at, pal)) in [
            (0x90usize, 0x98usize),
            (0x80, 0x88),
            (0x70, 0x78),
            (0x50, 0x58),
            (0x60, 0x68),
        ]
        .into_iter()
        .enumerate()
        {
            let n = word(&bytes, at) as usize;
            let stride = [0x54, 0x38, 0x54, 0x5c, 0x54][k];
            let pn = word(&bytes, pal) as usize;
            let pa = (word(&bytes, pal + 4).wrapping_sub(addr)) as usize;
            let a = (word(&bytes, at + 4).wrapping_sub(addr)) as usize;
            // Halo 1's placements of this kind, in order: the converted ones
            // keep it, skipping those whose palette entry did not convert.
            let h1_kind = {
                use h1campaign::scenario::h1 as b;
                [b::WEAPONS, b::EQUIPMENT, b::VEHICLES, b::SCENERY, b::BIPEDS][k]
            };
            let scnr1 = h1.tag_data(h1.scenario()).unwrap();
            let h1_src = h1
                .tag_space()
                .block_at(scnr1, h1_kind.0, h1_kind.1)
                .unwrap();
            let mut h1_next = 0;
            let mut later = 0;
            for i in 0..n {
                let e = &bytes[a + i * stride..a + (i + 1) * stride];
                let p = u16::from_le_bytes([e[0], e[1]]) as usize;
                assert!(p < pn);
                let id = TagId(word(&bytes, pa + p * 0x28 + 4));
                let t = m.tag(id).unwrap();
                assert_eq!(
                    t.group,
                    FourCC([*b"weap", *b"eqip", *b"vehi", *b"scen", *b"bipd"][k]),
                    "placement {i}"
                );
                // Inside the level's bounds (any BSP), give or take.
                let p3: [f32; 3] = std::array::from_fn(|j| {
                    f32::from_le_bytes(e[8 + 4 * j..12 + 4 * j].try_into().unwrap())
                });
                assert!(p3.iter().all(|v| v.is_finite()));
                // Halo 1's "not placed automatically" is Halo 2's "not at
                // start" (bit 0 at +4), and no other bit is set.
                let src = loop {
                    let h = h1_src[h1_next];
                    h1_next += 1;
                    if h[8..20] == e[8..20] {
                        break h;
                    }
                };
                let want = u32::from(u16::from_le_bytes([src[4], src[5]]) & 1);
                assert_eq!(word(e, 4), want, "placement {i}: flags");
                later += want as usize;
            }
            println!(
                "object block {at:#x}: {n} placements over {pn} palette entries, {later} not at start"
            );
        }
    }

    // The object sweep: every object, model and character the scenario
    // names resolves, with data, and every hlmt covers its model's nodes.
    if opts.objects {
        let (n, bad) = sweep_map(&mut m, &filled);
        println!("object sweep: {n} tags");
        assert!(bad.is_empty(), "{bad:#?}");
    }

    // AI: decodes with h1ai's reader and its indices check.
    if opts.ai {
        let view = tags::geometry::TagView::new(&bytes, addr);
        let ai = h1ai::h2::ScenarioAi::read(&view).unwrap();
        assert!(ai.check_indices().is_empty(), "{:#?}", ai.check_indices());
        for c in &ai.character_palette {
            assert_eq!(m.tag(*c).unwrap().group, FourCC(*b"char"));
        }
        println!(
            "ai read back: {} squads, {} zones, {} characters",
            ai.squads.len(),
            ai.zones.len(),
            ai.character_palette.len()
        );
    }

    // Scripts: decode, codec symmetry, validation against Halo 2's table.
    if let Some(sc) = &s {
        let _ = sc;
        let hs_s = hs::Scenario::read(&mut m).unwrap();
        println!(
            "scripts read back: {} scripts, {} globals, {} nodes",
            hs_s.scripts().len(),
            hs_s.globals().len(),
            hs_s.nodes().len()
        );
        let r = done.report.scenario.scripts.as_ref().unwrap();
        assert_eq!(hs_s.scripts().len(), r.scripts);
        assert_eq!(hs_s.nodes().len(), r.nodes);
        // Halo 1's initially created encounters: a startup script places
        // their squad groups, each argument resolved to a group.
        if opts.ai && !r.initial.is_empty() {
            let sc = hs_s.script(h1campaign::scenario::INITIAL_SCRIPT).unwrap();
            assert_eq!(sc.kind(), 0, "startup");
            let mut placed = Vec::new();
            for n in hs_s.nodes() {
                if n.atom() || n.data() == u32::MAX {
                    continue;
                }
                let Ok(head) = hs_s.node(hs::Datum(n.data())) else {
                    continue;
                };
                if hs_s.string(head.string_offset()).ok() != Some("ai_place") {
                    continue;
                }
                let arg = hs_s.node(head.next()).unwrap();
                // One argument, as most retail calls (no count).
                assert_eq!(arg.next(), hs::Datum::NULL);
                placed.push(arg.data());
            }
            let view = tags::geometry::TagView::new(&bytes, addr);
            let ai = h1ai::h2::ScenarioAi::read(&view).unwrap();
            for g in &r.initial {
                let i = ai.squad_groups.iter().position(|x| &x.name == g).unwrap();
                assert!(placed.contains(&(0x4000_0000 | i as u32)), "group {g}");
            }
            println!("placed at startup: {:?}", r.initial);
        }
    }

    // Sounds: each converted one (named as in Halo 1) resolves through the
    // gestalt, and so do the base's own.
    if let Some(r) = done.report.scenario.scripts.as_ref() {
        let g = tags::sound::Gestalt::read(&mut m).unwrap();
        let snd = h1map::Group::new(b"snd!");
        let ids: Vec<(TagId, bool)> = m
            .tags()
            .iter()
            .filter(|t| t.group == FourCC(*b"snd!") && t.location.is_some())
            .map(|t| {
                let ours = t.name.as_deref().is_some_and(|n| h1.find(snd, n).is_some());
                (t.id, ours)
            })
            .collect();
        let mut ours = 0;
        for (id, mine) in &ids {
            let h = tags::sound::read_header(&mut m, *id).unwrap();
            let resolved = g.resolve(&h).unwrap();
            if *mine {
                ours += 1;
                assert!(!resolved.ranges.is_empty());
            }
        }
        println!("sounds read back: {ours} converted, {} in all", ids.len());
        // The scripts' own sounds and the looping sounds' tracks.
        assert!(ours >= r.sounds, "{ours} < {}", r.sounds);
        // Each looping sound the scripts name is in the map under its
        // Halo 1 name, or reported left out. The clusters' background
        // sounds (h1convert::ambience) are Halo 1 looping sounds too, and
        // not the scripts' unless a script also names one.
        let ambient: std::collections::HashSet<String> = h1convert::ambience::read(&h1)
            .backgrounds
            .into_iter()
            .map(|(_, l)| l.name)
            .collect();
        let lsnd = m
            .tags()
            .iter()
            .filter(|t| t.group == FourCC(*b"lsnd") && t.location.is_some())
            .filter(|t| {
                t.name
                    .as_deref()
                    .is_some_and(|n| h1.find(h1sounds::looping::LSND, n).is_some())
            })
            .filter(|t| !t.name.as_deref().is_some_and(|n| ambient.contains(n)))
            .count();
        println!(
            "looping sounds read back: {lsnd} ({} left out)",
            r.looping_dropped.len()
        );
        assert_eq!(lsnd, r.looping);
        assert!(r.looping > 0);
    }

    // Listed by the Halo 1 campaign manifest under the map's own path.
    let text = h1campaign::manifest::manifest_for(None, "a30").unwrap();
    h1campaign::manifest::check(&text, "a30").unwrap();
}

/// Every Halo 1 level's converted scripts only ask the retail interpreter
/// for casts and builtins it has (`hs::Scenario::unrunnable`, checked clean
/// on every retail scenario by `hs`'s `casts_disc`). b30's first fault was a
/// string literal where Halo 2 wants a `unit_seat_mapping`: the cast table
/// has no entry, and the interpreter called address 0. `$H1_LEVELS` (comma
/// separated) limits the levels.
#[test]
#[ignore = "reads disc/halo1.iso and disc/halo2.iso"]
fn retail_ai_control_hooks_match_spawn_follow_and_uncover_paths() {
    let mut source = h1_iso();
    let entry = source.find("/default.xbe").unwrap();
    let source = source.read(&entry).unwrap();
    let source = xbe::Xbe::parse(&source).unwrap();
    let mut target = h2_iso();
    let entry = target.find("default.xbe").unwrap();
    let target = target.read(&entry).unwrap();
    let target = xbe::Xbe::parse(&target).unwrap();
    let call = |image: &xbe::Xbe<'_>, at: u32, expected| {
        let bytes = image.bytes_at(at, 5).unwrap();
        assert_eq!(bytes[0], 0xe8);
        let displacement = u32::from_le_bytes(bytes[1..].try_into().unwrap());
        assert_eq!(at.wrapping_add(5).wrapping_add(displacement), expected);
    };
    call(&source, 0x001415af, 0x0013b270);
    call(&source, 0x0013e381, 0x0013b270);
    call(&source, 0x0013bd1f, 0x0013b500);
    call(&source, 0x0013eb01, 0x0013e280);
    call(&source, 0x0013eb0d, 0x0013c910);
    call(&target, 0x002016ab, 0x002039d0);
    call(&target, 0x00201912, 0x00203780);
    call(&target, 0x00274d61, 0x001a80e0);
    let uncover = target.read_u32(0x0046eeb8 + 35 * 4).unwrap();
    assert_eq!(uncover, 0x0047e908);
    assert_eq!(target.read_u32(uncover + 0x18).unwrap(), 0x001b8540);
    let presearch = target.read_u32(0x0046eeb8 + 28 * 4).unwrap();
    assert_eq!(target.read_u32(presearch + 0x10).unwrap(), 0x001b44d0);
}

fn check_authored_automatic_destinations(
    level: &str,
    source: &h1map::H1Map,
    converted: &hs::Scenario,
    scenario_tag: &[u8],
    catalog: &h1scripts::Catalog,
) {
    use hs::h1_ai_slots::{Operation, decode_words, migration as wire};

    let count = |offset| {
        u32::from_le_bytes(scenario_tag[offset..offset + 4].try_into().unwrap()) as usize
    };
    let (squads, groups, orders) = (count(0x160), count(0x158), count(0x240));
    let expected = if level == "a50" { 359 } else { 344 };
    assert_eq!(squads, expected, "{level}: retained physical squads");
    assert_eq!(engine::squad_pool::partition(squads, groups), Some((squads, groups)));

    let reachable = converted.reachable(converted.roots()).unwrap();
    let payload = reachable.into_iter().find_map(|datum| {
        let node = converted.node(datum).unwrap();
        if node.atom() || node.opcode() != hs::h1_ai_slots::CONTROL {
            return None;
        }
        let head = converted.node(hs::Datum(node.data())).unwrap();
        if head.data() != Operation::FollowWithSourcePositions.marker() {
            return None;
        }
        let base = converted.node(head.next()).unwrap();
        let offset = converted.node(base.next()).unwrap().data() as usize;
        converted.strings().get(offset..)?.split(|&b| b == 0).next()
    }).expect("runtime follow geometry");
    let ai = h1ai::h1::H1Ai::read(source).unwrap();
    let tables = h1scripts::Tables::read(source).unwrap();
    let subjects = h1scripts::aicontrol::retained_subjects(&tables, catalog);
    let mut shared = 0;
    for (index, encounter) in ai.encounters.iter().enumerate() {
        let at = index * 2 * wire::HEADER;
        let entries = wire::Location::decode(&payload[at..at + wire::HEADER]).unwrap();
        let points = wire::Location::decode(&payload[at + wire::HEADER..at + 2 * wire::HEADER]).unwrap();
        let expected = subjects.follow_encounters.as_ref()
            .is_none_or(|groups| groups.contains(&(index as u16)));
        let source: Vec<_> = encounter.squads.iter()
            .filter(|squad| expected && squad.flags & 0x20 != 0).collect();
        assert_eq!(usize::from(entries.rows), source.len(), "{level}: {}", encounter.name);
        if source.is_empty() {
            continue;
        }
        let at = entries.offset as usize;
        let size = usize::from(entries.rows) * wire::Route::WIDTH;
        let rows = decode_words(&payload[at..at + size]).unwrap();
        let mut groups = 0;
        for (row, source) in rows.chunks_exact(wire::Route::WIDTH / 4).zip(source) {
            let route = wire::Route::decode(row).unwrap();
            assert!(route.squad == u16::MAX || usize::from(route.squad) < squads);
            for order in route.orders {
                assert!(order == u16::MAX || usize::from(order) < orders);
            }
            if route.squad != u16::MAX && route.orders != [u16::MAX; 2] {
                shared += 1;
            }
            let attacking = source.groups[0] | source.groups[1] | source.groups[2];
            let defending = source.groups[3] | source.groups[4] | source.groups[5];
            assert_eq!(route.masks, [attacking, defending]);
            groups |= attacking | defending;
        }
        let at = points.offset as usize;
        let size = usize::from(points.rows) * wire::POINT_WIDTH;
        let words = decode_words(&payload[at..at + size]).unwrap();
        let encoded: Vec<_> = words.chunks_exact(wire::POINT_WIDTH / 4)
            .map(|row| wire::read_point(row).unwrap()).collect();
        let original: Vec<_> = encounter.firing_positions.iter()
            .filter(|point| point.group < 26 && groups & (1 << point.group) != 0)
            .map(|point| (usize::from(point.group), point.position)).collect();
        assert_eq!(encoded, original, "{level}: source firing positions for {}", encounter.name);
    }
    assert!(shared > 0, "{level}: expected reusable native squads");
    assert!(converted.globals().iter()
        .any(|global| global.name().ok() == Some(hs::h1_ai_slots::TELEPORT_STATE)));
}

#[test]
#[ignore = "reads disc/halo1.iso and disc/halo2.iso"]
fn every_level_s_scripts_run_on_the_retail_interpreter() {
    let s = scripting();
    let donors = donors();
    let mut iso = h2_iso();
    let e = iso.find("default.xbe").unwrap();
    let xbe_bytes = iso.read(&e).unwrap();
    let mut catalog = hs::Catalog::from_xbe(&xbe::Xbe::parse(&xbe_bytes).unwrap()).unwrap();
    for e in iso
        .entries()
        .unwrap()
        .into_iter()
        .filter(|e| !e.is_dir && e.path.ends_with(".map"))
    {
        let mut m = Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap();
        catalog
            .observe(&hs::Scenario::read(&mut m).unwrap())
            .unwrap();
    }
    let levels = std::env::var("H1_LEVELS")
        .unwrap_or_else(|_| "a10,a30,a50,b30,b40,c10,c20,c40,d20,d40".into());
    let mut bad = Vec::new();
    // Halo 1's `switch_bsp` is Halo 2's, so its literal index carries over.
    let switch = s
        .table
        .builtins
        .iter()
        .find(|t| t.name == "switch_bsp")
        .expect("Halo 1 has switch_bsp");
    assert_eq!(switch.kind, h1scripts::translate::Kind::Direct);
    for level in levels.split(',') {
        let h1 = h1_level(level);
        let mut base = h2_map(h1campaign::base_for(level));
        let opts = Options {
            objects: true,
            ai: true,
            scripts: Some(&s),
            sounds: 1000,
            donors: Some(&donors),
            ..Default::default()
        };
        let done =
            h1campaign::convert(&h1, &mut base, &opts).unwrap_or_else(|e| panic!("{level}: {e}"));
        let mut m = Map::from_reader(Cursor::new(done.bytes)).unwrap();
        let sc = hs::Scenario::read(&mut m).unwrap();
        let found = sc.unrunnable(&catalog).unwrap();
        // Every `switch_bsp` in Halo 1's scripts (whose literal the
        // translation keeps: Halo 2's `switch_bsp` takes the same short)
        // names a BSP the converted scenario has, the same one: the
        // descriptors keep Halo 1's order and count.
        let scnr = m.scenario().unwrap().clone();
        let bytes = m.read_tag_bytes(scnr.id).unwrap();
        if matches!(level, "a50" | "b40") {
            check_authored_automatic_destinations(level, &h1, &sc, &bytes, &s.catalog);
        }
        let bsps = u32::from_le_bytes(bytes[0x210..0x214].try_into().unwrap());
        assert_eq!(bsps as usize, h1.structure_bsps().unwrap().len());
        // Every reached atom naming a cutscene flag, camera point, title or
        // device group indexes its converted block: the engine reads these
        // unchecked (b40 faulted in `object_teleport` on flag -1).
        {
            use h1scripts::level::h2 as blk;
            let count = |at: u32| {
                u32::from_le_bytes(bytes[at as usize..at as usize + 4].try_into().unwrap())
            };
            let indexed: Vec<(u16, &str, u32)> = [
                ("cutscene_flag", blk::FLAGS.0),
                ("cutscene_camera_point", blk::CAMERA_POINTS.0),
                ("cutscene_title", blk::TITLES.0),
                ("device_group", blk::DEVICE_GROUPS.0),
            ]
            .into_iter()
            .filter_map(|(name, at)| {
                let ty = s.target.types.iter().position(|t| t == name)? as u16;
                Some((ty, name, count(at)))
            })
            .collect();
            assert_eq!(indexed.len(), 4);
            let reached = sc.reachable(sc.roots()).unwrap();
            let mut n = 0;
            for d in reached {
                let node = sc.node(d).unwrap();
                if !node.atom() || node.global() {
                    continue;
                }
                let Some(&(_, name, len)) = indexed.iter().find(|i| i.0 == node.value_type())
                else {
                    continue;
                };
                n += 1;
                if node.data() & 0xffff >= len {
                    bad.push(format!(
                        "{level}: {name} {:?} is {:#x}, block of {len}",
                        sc.string(node.string_offset()).unwrap_or("?"),
                        node.data()
                    ));
                }
            }
            println!("{level}: {n} flag, camera point, title and device group atoms checked");
        }
        let t = h1scripts::layout::Tables::read(&h1).unwrap();
        let src = h1scripts::decompile::decompile(&t, &s.catalog)
            .unwrap()
            .source;
        let mut switches = std::collections::BTreeSet::new();
        for (i, _) in src.match_indices("(switch_bsp ") {
            let rest = &src[i + "(switch_bsp ".len()..];
            let arg = &rest[..rest.find(')').unwrap_or(rest.len())];
            match arg.trim().parse::<u32>() {
                Ok(k) if k < bsps => {
                    switches.insert(k);
                }
                _ => bad.push(format!("{level}: (switch_bsp {arg}) with {bsps} BSPs")),
            }
        }
        println!("{level}: {bsps} BSPs, switch_bsp to {switches:?}");
        if let Some(r) = &done.report.scenario.scripts {
            let left: usize = r.unresolved.iter().map(|u| u.1).sum();
            println!(
                "{level}: hooks {}, left none {} (before level data and seats: {}): {:?}; samples {:?}",
                r.hooks,
                left,
                left + r.resolved_by_level,
                r.unresolved,
                r.samples
            );
        }
        println!(
            "{level}: {} reached nodes, {} unrunnable",
            sc.reachable(sc.roots()).unwrap().len(),
            found.len()
        );
        bad.extend(
            found
                .into_iter()
                .map(|u| format!("{level} node {}: {}", u.slot, u.what)),
        );
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

#[test]
#[ignore = "reads disc/halo1.iso and disc/halo2.iso"]
fn every_campaign_level_converts() {
    let s = scripting();
    let donors = donors();
    let only = std::env::var("H1_LEVELS").ok();
    let mut failed = Vec::new();
    for level in h1map::disc::CAMPAIGN_LEVELS {
        if only
            .as_deref()
            .is_some_and(|o| !o.split(',').any(|l| l == level))
        {
            continue;
        }
        let h1 = h1_level(level);
        let base_name = h1campaign::base_for(level);
        let mut base = h2_map(base_name);
        let filled = filled_profile(&mut base);
        let opts = Options {
            objects: true,
            ai: true,
            scripts: Some(&s),
            sounds: 1000,
            donors: Some(&donors),
            ..Default::default()
        };
        match h1campaign::convert(&h1, &mut base, &opts) {
            Ok(done) => {
                let mut m = Map::from_reader(Cursor::new(done.bytes.clone())).unwrap();
                m.validate_checksum().unwrap();
                println!("{}", done.report);
                check_bsps(&mut m, &h1, &done);
                let (n, bad) = sweep_map(&mut m, &filled);
                println!("  object sweep: {n} tags, {} problems", bad.len());
                if !bad.is_empty() {
                    failed.push(format!("{level}: sweep {bad:#?}"));
                }
                // Every hlmt holds the materials the engine reads
                // (h1objects::verify::hlmt_materials), beyond what the
                // retail base already breaks.
                let known = h1objects::verify::hlmt_materials(&mut base).unwrap();
                let lines = h1objects::verify::hlmt_materials(&mut m).unwrap();
                let new: Vec<_> = lines.into_iter().filter(|l| !known.contains(l)).collect();
                println!("  hlmt materials: {} problems", new.len());
                if !new.is_empty() {
                    failed.push(format!("{level}: hlmt materials {new:#?}"));
                }
                // The globals' grenades are Halo 1's, converted.
                let grenades = globals_grenades(&mut m);
                println!("  grenades: {grenades:?}");
                if grenades.len() != 2
                    || !grenades
                        .iter()
                        .all(|(e, p)| e.starts_with("weapons\\") && p.starts_with("weapons\\"))
                {
                    failed.push(format!("{level}: grenades {grenades:?}"));
                }
                for g in done.report.groups.iter().take(8) {
                    println!("  memory: {} built {:#x} pinned {:#x}", g.0, g.1, g.2);
                }
            }
            Err(e) => {
                println!("{level} on {base_name}: FAILED: {e}");
                failed.push(format!("{level}: {e}"));
            }
        }
    }
    assert!(failed.is_empty(), "{failed:#?}");
}

/// The equipment and projectile names of each grenade type of a map's
/// globals (`h1convert::globals`).
fn globals_grenades(m: &mut Map<Cursor<Vec<u8>>>) -> Vec<(String, String)> {
    use h1convert::globals::{GRENADE_EQUIPMENT, GRENADE_PROJECTILE, GRENADE_SIZE, GRENADES};
    let t = m.tags().iter().find(|t| t.group.0 == *b"matg").unwrap().clone();
    let b = m.read_tag_bytes(t.id).unwrap();
    let address = t.location.unwrap().address;
    let word = |at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
    let first = (word(GRENADES + 4) - address) as usize;
    let name = |m: &Map<Cursor<Vec<u8>>>, at: usize| {
        m.tag(TagId(word(at + 4)))
            .ok()
            .and_then(|t| t.name.clone())
            .unwrap_or_default()
    };
    (0..word(GRENADES) as usize)
        .map(|i| {
            let e = first + i * GRENADE_SIZE;
            (name(m, e + GRENADE_EQUIPMENT), name(m, e + GRENADE_PROJECTILE))
        })
        .collect()
}

struct Mem(Vec<u8>);
impl tags::geometry::RawSource for Mem {
    fn read_raw(&mut self, p: map::RawPointer, size: u32) -> tags::Result<Vec<u8>> {
        let s = p.offset as usize;
        self.0
            .get(s..s + size as usize)
            .map(<[u8]>::to_vec)
            .ok_or(tags::Error::Invalid("raw range"))
    }
}

/// What the engine reads of one converted structure's clusters when it
/// draws them, checked: the subpart mask (cluster `+0x24`) has a bit per
/// subpart; the visibility code and reorder table are laid out as the
/// volume pass reads them (`h1convert::structure::check_visibility`); and
/// every predicted resource (`+0x84`) resolves: a bitmap's image below its
/// image count (E `0x0016e663`, unchecked), a cluster below the count, no
/// lightmap buckets. Returns the problems.
fn check_clusters(m: &mut Map<Cursor<Vec<u8>>>, bytes: &[u8], sbsp: TagId) -> Vec<String> {
    let word = |b: &[u8], at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
    let half = |b: &[u8], at: usize| u16::from_le_bytes([b[at], b[at + 1]]);
    let render = tags::structure::StructureRender::read(m, sbsp).unwrap();
    let t = m.tag(sbsp).unwrap().clone();
    let b = m.read_tag_bytes(sbsp).unwrap();
    let view = tags::geometry::TagView::new(&b, t.location.unwrap().address);
    let mut raw = Mem(bytes.to_vec());
    let mut bad = Vec::new();
    for (i, c) in render.clusters.iter().enumerate() {
        let subparts = match tags::geometry::load_block(&c.geometry.block, &mut raw) {
            Ok(Some(block)) => {
                let sec = block.section();
                let n = word(sec, 8) as usize;
                let res = |field: u16| {
                    block
                        .resources()
                        .iter()
                        .find(|r| {
                            r.field_offset == field
                                && r.kind != tags::geometry::ResourceKind::VertexBuffer
                        })
                        .and_then(|r| block.resource(r))
                };
                if n > 0 {
                    match (res(0x28), res(0x30)) {
                        (Some(code), Some(table)) => {
                            if let Err(e) = h1convert::structure::check_visibility(n, code, table) {
                                bad.push(format!("cluster {i}: {e}"));
                            }
                        }
                        _ => bad.push(format!("cluster {i}: no visibility code or table")),
                    }
                }
                n
            }
            Ok(None) => 0,
            Err(e) => {
                bad.push(format!("cluster {i}: block: {e}"));
                continue;
            }
        };
        let mask = usize::from(half(&c.raw, 0x24));
        if mask != subparts {
            bad.push(format!("cluster {i}: mask of {mask}, {subparts} subparts"));
        }
        for e in view.block(&c.raw, 0x84, 8).unwrap() {
            let (kind, index, id) = (half(e, 0), half(e, 2), TagId(word(e, 4)));
            match kind {
                0 => {
                    let Ok(tag) = m.tag(id).cloned() else {
                        bad.push(format!("cluster {i}: bitmap {id:?} missing"));
                        continue;
                    };
                    if tag.group != FourCC(*b"bitm") {
                        bad.push(format!("cluster {i}: {id:?} is no bitmap"));
                        continue;
                    }
                    let images = word(&m.read_tag_bytes(id).unwrap(), 0x44);
                    if u32::from(index) >= images {
                        bad.push(format!("cluster {i}: image {index} of {images}"));
                    }
                }
                3 if usize::from(index) >= render.clusters.len() => {
                    bad.push(format!("cluster {i}: predicts cluster {index}"));
                }
                3 => {}
                k => bad.push(format!("cluster {i}: predicted type {k}")),
            }
        }
    }
    bad
}

/// Every bitmap image in `m` says "not cached" (the texture cache handles
/// `+0x28..=+0x30` none, as on every retail image): a handle of 0 reads as
/// resident in slot 0 and the pixels are never loaded. (The LOD pointers
/// `+0x20`, `+0x24` are none on converted images but not on the base's.)
/// And every image is a texture cache resource (flag `0x200`, as on every
/// retail image; `h1bitmaps::convert::IMAGE_CACHED`): one without it is
/// never loaded and its stage stays empty (the dark lightmap).
fn check_images(m: &mut Map<Cursor<Vec<u8>>>) -> Vec<String> {
    let ids: Vec<_> = m
        .tags()
        .iter()
        .filter(|t| t.group == FourCC(*b"bitm"))
        .filter_map(|t| Some((t.id, t.location?.address)))
        .collect();
    let mut bad = Vec::new();
    for (id, at) in ids {
        let b = m.read_tag_bytes(id).unwrap();
        let word = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        let (n, p) = (word(0x44) as usize, word(0x48));
        if n == 0 {
            continue;
        }
        let p = (p - at) as usize;
        for k in 0..n {
            for w in [0x28, 0x2c, 0x30] {
                if word(p + 0x74 * k + w) != u32::MAX {
                    bad.push(format!("bitmap {id:?} image {k} +{w:#x}"));
                }
            }
            let flags = word(p + 0x74 * k + 0xc) >> 16;
            if flags as u16 & h1bitmaps::convert::IMAGE_CACHED == 0 {
                bad.push(format!(
                    "bitmap {id:?} image {k} flags {flags:#x}: not a cache resource"
                ));
            }
        }
    }
    bad
}

#[test]
#[ignore = "reads disc/halo1.iso and disc/halo2.iso"]
fn every_campaign_level_s_clusters_are_drawn_as_the_engine_reads_them() {
    let only = std::env::var("H1_LEVELS").ok();
    let mut failed = Vec::new();
    let mut levels = 0;
    for level in h1map::disc::CAMPAIGN_LEVELS {
        if only
            .as_deref()
            .is_some_and(|o| !o.split(',').any(|l| l == level))
        {
            continue;
        }
        let h1 = h1_level(level);
        let mut base = h2_map(h1campaign::base_for(level));
        let opts = Options {
            objects: false,
            ai: false,
            ..Options::default()
        };
        let done = h1campaign::convert(&h1, &mut base, &opts).unwrap();
        let mut m = Map::from_reader(Cursor::new(done.bytes.clone())).unwrap();
        // The scenario's structures (+0x210, 0x44 each, the BSP at +0x14).
        let scnr = m.scenario().unwrap().clone();
        let s = m.read_tag_bytes(scnr.id).unwrap();
        let word = |at: usize| u32::from_le_bytes(s[at..at + 4].try_into().unwrap());
        let desc = (word(0x214) - scnr.location.unwrap().address) as usize;
        let sbsp: Vec<_> = (0..word(0x210) as usize)
            .map(|k| TagId(word(desc + k * 0x44 + 0x14)))
            .collect();
        assert_eq!(sbsp.len(), h1.structure_bsps().unwrap().len());
        let mut bad = Vec::new();
        for &id in &sbsp {
            bad.extend(check_clusters(&mut m, &done.bytes, id));
        }
        let (heard, leaves) = check_sound(&mut m, &h1, &sbsp, &mut bad);
        println!("{level}: {heard} of {leaves} collision leaves play Halo 1's sound");
        if heard != leaves {
            bad.push(format!(
                "{} of {leaves} leaves play another cluster's sound",
                leaves - heard
            ));
        }
        bad.extend(check_images(&mut m));
        println!("{level}: {} structures, {} problems", sbsp.len(), bad.len());
        if !bad.is_empty() {
            bad.truncate(20);
            failed.push(format!("{level}: {bad:#?}"));
        }
        levels += 1;
    }
    assert!(levels > 0);
    assert!(failed.is_empty(), "{failed:#?}");
}

/// Ambient sound as the engine reads it (`h1convert::structure::Sound`):
/// each structure's palettes (`sbsp +0xd4`, `+0xdc`) name the scenario's
/// tags (`+0x250`, `+0x258`), every cluster's background sound and sound
/// environment (`+0x72`, `+0x74`) is -1 or in its palette and matches the
/// scenario's cluster data (`+0x290`), and every collision leaf's cluster
/// is in range. Returns how many leaves play their Halo 1 cluster's
/// background sound and environment, of how many.
fn check_sound(
    m: &mut Map<Cursor<Vec<u8>>>,
    h1: &h1map::H1Map,
    sbsp: &[TagId],
    bad: &mut Vec<String>,
) -> (usize, usize) {
    let w = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    let half = |b: &[u8], o: usize| i16::from_le_bytes([b[o], b[o + 1]]);
    let block = |b: &[u8], at: u32, f: usize, len: usize| -> Vec<Vec<u8>> {
        let (n, p) = (w(b, f) as usize, w(b, f + 4));
        if n == 0 {
            return Vec::new();
        }
        let p = p.wrapping_sub(at) as usize;
        (0..n)
            .map(|k| b[p + k * len..p + (k + 1) * len].to_vec())
            .collect()
    };
    let scnr = m.scenario().unwrap().clone();
    let sat = scnr.location.unwrap().address;
    let s = m.read_tag_bytes(scnr.id).unwrap();
    let palettes = [block(&s, sat, 0x250, 0x64), block(&s, sat, 0x258, 0x48)];
    let data = block(&s, sat, 0x290, 0x34);
    let ambience = h1convert::ambience::read(h1);
    let written = h1convert::ambience::written(&ambience);
    if palettes[0].len() != written[0] || palettes[1].len() != written[1] {
        bad.push(format!(
            "scenario palettes {:?}, converted {written:?}",
            [palettes[0].len(), palettes[1].len()]
        ));
    }
    let h1_bsps = h1.structure_bsps().unwrap();
    let (mut heard, mut total) = (0, 0);
    for (k, &id) in sbsp.iter().enumerate() {
        let at = m.tag(id).unwrap().location.unwrap().address;
        let b = m.read_tag_bytes(id).unwrap();
        let own = [block(&b, at, 0xd4, 0x64), block(&b, at, 0xdc, 0x48)];
        for j in 0..2 {
            let tags = |p: &[Vec<u8>]| p.iter().map(|e| w(e, 0x24)).collect::<Vec<_>>();
            if tags(&own[j]) != tags(&palettes[j]) {
                bad.push(format!("bsp {k}: palette {j} is not the scenario's"));
            }
        }
        let clusters = block(&b, at, 0x9c, 0xb0);
        let zones: Vec<[i16; 2]> = clusters
            .iter()
            .map(|c| [half(c, 0x72), half(c, 0x74)])
            .collect();
        for (c, z) in zones.iter().enumerate() {
            for j in 0..2 {
                if z[j] < -1 || z[j] >= own[j].len() as i16 {
                    bad.push(format!("bsp {k} cluster {c}: sound {j} is {}", z[j]));
                }
            }
        }
        if let Some(d) = data.get(k) {
            let dat = |f: usize| {
                let (n, p) = (w(d, f) as usize, w(d, f + 4).wrapping_sub(sat) as usize);
                (0..n).map(|k| half(&s, p + 4 * k)).collect::<Vec<_>>()
            };
            let (bg, env) = (dat(0x08), dat(0x10));
            let want: (Vec<i16>, Vec<i16>) = zones.iter().map(|z| (z[0], z[1])).unzip();
            if (bg, env) != want {
                bad.push(format!(
                    "bsp {k}: the scenario's cluster data names other sounds"
                ));
            }
        }
        let leaves = block(&b, at, 0x2c, 8);
        // Halo 1's: each leaf's cluster (+0x08) and that cluster's zone.
        let src = h1.structure_bsp(&h1_bsps[k]).unwrap();
        let h1_clusters = src.space.block_at(src.root, 0x134, 0x68).unwrap();
        let h1_leaves = src.space.block_at(src.root, 0xe0, 0x10).unwrap();
        for (i, l) in leaves.iter().enumerate() {
            let c = half(l, 0) as usize;
            let Some(z) = zones.get(c) else {
                bad.push(format!("bsp {k} leaf {i}: cluster {c} of {}", zones.len()));
                continue;
            };
            total += 1;
            let want = h1_leaves
                .get(i)
                .and_then(|h| h1_clusters.get(half(h, 8) as usize))
                .map(|hc| ambience.index(k, [half(hc, 4), half(hc, 6)], written));
            if want == Some(*z) {
                heard += 1;
            }
        }
    }
    (heard, total)
}

/// Every sound of a converted campaign level (`H1_LEVELS`, default a10)
/// read back through the gestalt as the engine plays it
/// (`h1sounds::audit`): Halo 1 sounds (the scripts' dialogue and effects)
/// hold exactly their Halo 1 samples, as long as they played there, and
/// every other sound is byte for byte the base's or a donor's of the same
/// name. A sound pointing at the wrong or too few bytes plays as a short
/// repeating buzz.
#[test]
#[ignore = "reads disc/halo1.iso and disc/halo2.iso"]
fn every_campaign_sound_plays_what_it_was_made_from() {
    let s = scripting();
    let donors = donors();
    let levels = std::env::var("H1_LEVELS").unwrap_or_else(|_| "a10".into());
    let mut failed = Vec::new();
    for level in levels.split(',') {
        let h1 = h1_level(level);
        let base_name = h1campaign::base_for(level);
        let base_bytes = h2_bytes(base_name);
        let mut base = Map::from_reader(Cursor::new(base_bytes.clone())).unwrap();
        let opts = Options {
            scripts: Some(&s),
            sounds: 1000,
            donors: Some(&donors),
            ..Default::default()
        };
        let done = h1campaign::convert(&h1, &mut base, &opts).unwrap();
        let mut retail: Vec<(&str, &[u8])> = vec![(base_name, &base_bytes)];
        retail.extend(donors.maps.iter().map(|(n, b)| (n.as_str(), b.as_slice())));
        let a = h1sounds::audit::audit(&done.bytes, Some(&h1), &retail).unwrap();
        println!(
            "{level} on {base_name}: {} sounds, {} from Halo 1 ({:.1} s), {} retail, {} permutations; \
             {} unmatched {:?}",
            a.sounds,
            a.converted,
            a.converted_seconds,
            a.retail,
            a.permutations,
            a.unmatched.len(),
            &a.unmatched[..a.unmatched.len().min(10)]
        );
        for p in &a.problems {
            println!("  {p}");
        }
        if !a.problems.is_empty() || !a.unmatched.is_empty() {
            failed.push(format!(
                "{level}: {} problems, {} unmatched",
                a.problems.len(),
                a.unmatched.len()
            ));
        }
        // Halo 1's vehicles are built from the `vehi` definition and loop
        // no retail sound (the lifepods once hummed the pelican's engine
        // from its template's attachments).
        let mut m = Map::from_reader(Cursor::new(done.bytes.clone())).unwrap();
        let mem = map_memory(&mut m);
        let word = |a: u32| u32::from_le_bytes(mem.slice(a, 4).unwrap().try_into().unwrap());
        let (at, stride) = (
            h1objects::convert::h2::ATTACHMENTS as u32,
            h1objects::convert::h2::ATTACHMENT as u32,
        );
        let mut pods = 0;
        for t in m.tags().iter().filter(|t| t.group == FourCC(*b"vehi")) {
            let name = t.name.clone().unwrap_or_default();
            if name.starts_with("objects\\") {
                continue;
            }
            pods += 1;
            let a = t.location.unwrap().address;
            let (n, first) = (word(a + at), word(a + at + 4));
            let loops = (0..n)
                .filter(|i| word(first + i * stride) == u32::from_be_bytes(*b"lsnd"))
                .count();
            println!("  {name}: {n} attachments, {loops} looping sounds");
            if loops > 0 {
                failed.push(format!("{level}: {name} loops {loops} sounds"));
            }
        }
        println!("  {pods} Halo 1 vehicles");
    }
    assert!(failed.is_empty(), "{failed:#?}");
}

/// Every render model's materials (`mode +0x60`, `0x20` each) whose shader
/// reference (`+0x08`, datum `+0x0c`) is NONE, as "name: material k". The
/// shader draw begin `0x1bbf0` does not skip a NONE shader, and the draw
/// `0x16b90` then reads tag slot `0xffff` as a `shad` (h1convert::sky).
fn none_materials(m: &mut Map<Cursor<Vec<u8>>>) -> (usize, Vec<String>) {
    let mem = map_memory(m);
    let modes: Vec<(String, u32)> = m
        .tags()
        .iter()
        .filter(|t| t.group == FourCC(*b"mode"))
        .filter_map(|t| Some((t.name.clone().unwrap_or_default(), t.location?.address)))
        .collect();
    let mut bad = Vec::new();
    let mut n = 0;
    for (name, at) in &modes {
        let count = count_at(&mem, at + 0x60);
        let first = count_at(&mem, at + 0x64);
        for k in 0..count {
            n += 1;
            if count_at(&mem, first + k * 0x20 + 0xc) == u32::MAX {
                bad.push(format!("{name}: material {k}"));
            }
        }
    }
    (n, bad)
}

#[test]
#[ignore = "reads disc/halo1.iso and disc/halo2.iso"]
fn no_converted_model_on_any_campaign_level_has_a_none_material() {
    let donors = donors();
    let only = std::env::var("H1_LEVELS").ok();
    let mut failed = Vec::new();
    for level in h1map::disc::CAMPAIGN_LEVELS {
        if only
            .as_deref()
            .is_some_and(|o| !o.split(',').any(|l| l == level))
        {
            continue;
        }
        let h1 = h1_level(level);
        let mut base = h2_map(h1campaign::base_for(level));
        let opts = Options {
            objects: true,
            donors: Some(&donors),
            ..Default::default()
        };
        let done =
            h1campaign::convert(&h1, &mut base, &opts).unwrap_or_else(|e| panic!("{level}: {e}"));
        let mut m = Map::from_reader(Cursor::new(done.bytes)).unwrap();
        let (n, bad) = none_materials(&mut m);
        println!("{level}: {n} materials, {} name no shader", bad.len());
        if !bad.is_empty() {
            failed.push(format!("{level}: {bad:#?}"));
        }
    }
    assert!(failed.is_empty(), "{failed:#?}");
}

/// a10's cryotubes, built from the `vehi` definition (`h1objects::rebuild::vehicle`;
/// no Halo 2 template names them), are placed (fresh):
/// `cryotube_1` and `cryotube_2` point at vehicle placements whose palette
/// entry is the rebuilt cryotube. Read back from the converted map, the
/// tag decodes through its definition and encodes byte-identical; its
/// model chain resolves to tags in the map (`hlmt`, then `mode`, `coll`
/// and `jmad`); it has one seat labelled with the rider's mode for a10's
/// `CT-driver` (`c_turret_ap_d`, as the converted graphs name it; the
/// seat table matches the scripts' text to it); and its unit fields are
/// sane.
#[test]
#[ignore = "reads disc/halo1.iso and disc/halo2.iso"]
fn a10_cryotubes_are_placed_and_built_from_their_definition() {
    use tags::defs::{self, Defs, Str, Target, Val};
    use tags::repo::{MapSource, extract::SharedFiles};
    let s = scripting();
    let donors = donors();
    let h1 = h1_level("a10");
    let vehi = Defs::builtin().group(FourCC(*b"vehi")).unwrap();
    let hlmt = Defs::builtin().group(FourCC(*b"hlmt")).unwrap();
    // Fresh only, the default build: pinned a10 is over the file size cap
    // since the level render (converter version 40) and is not tested.
    for pinned in [false] {
        let mut base = h2_map(h1campaign::base_for("a10"));
        let opts = Options {
            objects: true,
            ai: true,
            scripts: Some(&s),
            sounds: 1000,
            donors: Some(&donors),
            pinned,
            ..Default::default()
        };
        let done = h1campaign::convert(&h1, &mut base, &opts).unwrap();
        assert!(
            !done.report.dropped.iter().any(|d| d.contains("cryotube")),
            "pinned {pinned}: {:?}",
            done.report.dropped
        );
        let bytes = done.bytes;
        let mut m = Map::from_reader(Cursor::new(bytes.clone())).unwrap();
        let mem = map_memory(&mut m);
        let scnr = m.scenario().unwrap().clone();
        let root = m.read_tag_bytes(scnr.id).unwrap();
        let word = |at: usize| u32::from_le_bytes(root[at..at + 4].try_into().unwrap());
        let (count, address) = (word(0x48), word(0x4c));
        let mut tubes = Vec::new();
        for i in 0..count {
            let e = mem.slice(address + i * 0x24, 0x24).unwrap();
            let text = String::from_utf8_lossy(&e[..32])
                .trim_end_matches('\0')
                .to_owned();
            if text != "cryotube_1" && text != "cryotube_2" {
                continue;
            }
            let ty = i16::from_le_bytes([e[0x20], e[0x21]]);
            let placement = i16::from_le_bytes([e[0x22], e[0x23]]);
            assert_eq!(ty, 1, "pinned {pinned}: {text} is a vehicle name");
            assert!(placement >= 0, "pinned {pinned}: {text} has no placement");
            // Vehicle placements (+0x70, 0x54 each) and palette (+0x78,
            // 0x28 each, the datum at +4).
            let e = mem
                .slice(word(0x74) + placement as u32 * 0x54, 0x54)
                .unwrap();
            let p = u16::from_le_bytes([e[0], e[1]]) as u32;
            assert!(p < word(0x78));
            let pe = mem.slice(word(0x7c) + p * 0x28, 0x28).unwrap();
            let id = TagId(u32::from_le_bytes(pe[4..8].try_into().unwrap()));
            let t = m.tag(id).unwrap().clone();
            assert_eq!(t.group, FourCC(*b"vehi"));
            let name = t.name.clone().unwrap_or_default();
            assert!(name.contains("cryotube"), "{name}");
            tubes.push((text, t.id));
        }
        tubes.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            tubes.iter().map(|t| t.0.as_str()).collect::<Vec<_>>(),
            ["cryotube_1", "cryotube_2"],
            "pinned {pinned}"
        );
        assert_eq!(tubes[0].1, tubes[1].1, "one cryotube definition");

        // Read back through the definitions.
        let schema = tags::schema::Schema::builtin();
        let mut shared = SharedFiles::default();
        let mut src = MapSource::new(
            schema,
            Map::from_reader(Cursor::new(bytes)).unwrap(),
            &mut shared,
        )
        .unwrap();
        let tag = src.extract(tubes[0].1).unwrap().unwrap();
        let r = defs::decode(vehi, &tag).unwrap();
        let back = defs::encode(vehi, tag.key.clone(), &r, Default::default()).unwrap();
        assert_eq!(
            defs::canonical(&back),
            defs::canonical(&tag),
            "pinned {pinned}: the cryotube re-encodes byte-identical"
        );
        let key = |v: Option<&Val>| match v {
            Some(Val::Ref {
                target: Target::Key(k),
                ..
            })
            | Some(Val::Datum(Target::Key(k))) => Some(k.clone()),
            _ => None,
        };
        let index = src.tags().to_vec();
        let find = |k: &tags::repo::TagKey| {
            index
                .iter()
                .find(|t| t.group == k.group && t.name.as_deref() == Some(k.path.as_str()))
                .map(|t| t.id)
        };
        assert_eq!(r.at("unit.obje.object_type"), Some(&Val::Enum(1)));
        assert_eq!(r.at("vehicle_type"), Some(&Val::Enum(6)), "turret");
        let Some(Val::Block(seats)) = r.at("block_1c8") else {
            panic!()
        };
        assert_eq!(seats.len(), 1, "pinned {pinned}: one seat");
        assert_eq!(
            seats[0].get("label"),
            Some(&Val::String(Str::Text("c_turret_ap_d".into())))
        );
        assert_eq!(
            seats[0].get("string_8"),
            Some(&Val::String(Str::Text("driver".into())))
        );
        let model = key(r.at("unit.obje.model")).expect("a model");
        assert_eq!(model.group, FourCC(*b"hlmt"));
        let hid = find(&model).expect("the hlmt is in the map");
        let ht = src.extract(hid).unwrap().unwrap();
        let h = defs::decode(hlmt, &ht).unwrap();
        for (path, group) in [
            ("render_model", *b"mode"),
            ("collision_model", *b"coll"),
            ("animation", *b"jmad"),
        ] {
            let k = key(h.at(path)).unwrap_or_else(|| panic!("hlmt {path}"));
            assert_eq!(k.group, FourCC(group), "{path}");
            assert!(
                find(&k).is_some(),
                "pinned {pinned}: {path} {} in the map",
                k.path
            );
        }
        let Some(Val::Block(nodes)) = h.at("nodes") else {
            panic!()
        };
        assert!(!nodes.is_empty(), "the hlmt has the model's nodes");
        println!(
            "a10 pinned {pinned}: cryotube_1/2 placed, {} seat, {} hlmt nodes",
            seats.len(),
            nodes.len()
        );
    }
}

/// Every reached `unit_open` and `unit_close` on a10 (fresh)
/// names a unit with a converted placement, and runs only when the unit
/// exists: `(if (> (list_count x) 0) (unit_open x))` with the same unit.
/// Halo 2's evaluators send the unit a message without a `none` check,
/// and the handler reads object header 0xffff's null pointer
/// (`0x00113eae`, `+0x33e`). a10's `fast_setup` opens `cryotube_1` as the
/// opening cinematic ends; a placed name is still `none` when its object
/// was not created (`a10_scripted_vehicles_can_be_created`)
/// (`h1scripts::compile::NONE_UNSAFE`).
#[test]
#[ignore = "reads disc/halo1.iso and disc/halo2.iso"]
fn a10_opens_and_closes_only_placed_units() {
    let s = scripting();
    let donors = donors();
    let h1 = h1_level("a10");
    let index = |n: &str| s.target.builtins[n].index;
    let ids: Vec<(u16, &str)> = ["unit_open", "unit_close"]
        .into_iter()
        .map(|n| (index(n), n))
        .collect();
    let (iff, gt, count_of) = (index("if"), index(">"), index("list_count"));
    let mut bad = Vec::new();
    // Fresh only, the default build: pinned a10 is over the file size cap
    // since the level render (converter version 40) and is not tested.
    for pinned in [false] {
        let mut base = h2_map(h1campaign::base_for("a10"));
        let opts = Options {
            objects: true,
            ai: true,
            scripts: Some(&s),
            sounds: 1000,
            donors: Some(&donors),
            pinned,
            ..Default::default()
        };
        let done = h1campaign::convert(&h1, &mut base, &opts).unwrap();
        let mut m = Map::from_reader(Cursor::new(done.bytes)).unwrap();
        let sc = hs::Scenario::read(&mut m).unwrap();
        let mem = map_memory(&mut m);
        let scnr = m.scenario().unwrap().clone();
        let bytes = m.read_tag_bytes(scnr.id).unwrap();
        let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let (count, address) = (word(0x48), word(0x4c));
        let name = |i: u32| -> (String, i16) {
            let e = mem.slice(address + i * 0x24, 0x24).unwrap();
            let text = String::from_utf8_lossy(&e[..32])
                .trim_end_matches('\0')
                .to_owned();
            (text, i16::from_le_bytes([e[0x22], e[0x23]]))
        };
        // A call's function and arguments.
        let call = |d: hs::Datum| -> Option<(u16, Vec<hs::Datum>)> {
            match sc.expression(d).ok()? {
                hs::Expression::Call {
                    function, children, ..
                } => Some((function, children.collect::<Result<_, _>>().ok()?)),
                _ => None,
            }
        };
        let reached = sc.reachable(sc.roots()).unwrap();
        let mut parent = std::collections::HashMap::new();
        for &d in &reached {
            if let Some((_, args)) = call(d) {
                for a in args {
                    parent.insert(a, d);
                }
            }
        }
        let mut calls = 0;
        for &d in &reached {
            let n = sc.node(d).unwrap();
            if n.atom() || n.script_call() {
                continue;
            }
            let Some(&(_, builtin)) = ids.iter().find(|i| i.0 == n.opcode()) else {
                continue;
            };
            let Some((_, args)) = call(d) else { continue };
            calls += 1;
            let arg = *sc.node(args[0]).unwrap();
            let token = sc.string(arg.string_offset()).unwrap_or("?").to_owned();
            // The guard: `(if (> (list_count x) 0) <this call>)`, x the
            // same unit.
            let guarded = parent
                .get(&d)
                .and_then(|&p| call(p))
                .is_some_and(|(f, br)| {
                    f == iff
                        && br.len() == 2
                        && br[1] == d
                        && call(br[0]).is_some_and(|(f, cmp)| {
                            f == gt
                                && cmp.len() == 2
                                && sc.node(cmp[1]).is_ok_and(|z| z.atom() && z.data() == 0)
                                && call(cmp[0]).is_some_and(|(f, l)| {
                                    f == count_of
                                        && l.len() == 1
                                        && sc.node(l[0]).is_ok_and(|x| {
                                            x.data() == arg.data()
                                                && sc.string(x.string_offset()).ok()
                                                    == Some(token.as_str())
                                        })
                                })
                        })
                });
            if !guarded {
                bad.push(format!(
                    "pinned {pinned}: ({builtin} {token}) is not guarded"
                ));
            }
            if !arg.atom() || arg.global() {
                continue;
            }
            let index = arg.data() & 0xffff;
            if arg.data() == u32::MAX || index >= count {
                bad.push(format!(
                    "pinned {pinned}: ({builtin} {token}) is {:#x}, {count} names",
                    arg.data()
                ));
                continue;
            }
            let (text, placement) = name(index);
            if placement < 0 {
                bad.push(format!(
                    "pinned {pinned}: ({builtin} {token}) names object name {index} {text:?}, placement {placement}"
                ));
            }
        }
        println!("a10 pinned {pinned}: {calls} unit_open/unit_close calls reached");
        if let Some(r) = &done.report.scenario.scripts {
            println!("a10 pinned {pinned}: stubbed {:?}", r.unindexed);
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

/// Every vehicle a10's scenario places (its vehicle palette) can be
/// created: Halo 2's `vehicle_new` (`0x000eeb80`, the `vehi` type's new
/// function) returns false when the definition's model (`hlmt`, the
/// object's `+0x38`) has no physics model (`phmo`, the `hlmt` reference at
/// `+0x20`; `cmp [edx+24h],-1` at `0x000eebcb`), and `object_new`
/// (`0x000b7b40`) then deletes the object and returns `none`, so the
/// placement's name stays `none` (a10's rebuilt cryotube: `unit_open
/// cryotube_1` faulted at `0x00113eae` with converter version 33). Every
/// retail vehicle's model has one; a vehicle whose model has none gets one
/// built from its Halo 1 collision (`h1objects::rebuild::physics_model`),
/// or a stand-in sphere with no solid collision (`sphere_physics`).
#[test]
#[ignore = "reads disc/halo1.iso and disc/halo2.iso"]
fn a10_scripted_vehicles_can_be_created() {
    let s = scripting();
    let donors = donors();
    let h1 = h1_level("a10");
    let mut bad = Vec::new();
    // Fresh only, the default build: pinned a10 is over the file size cap
    // since the level render (converter version 40) and is not tested.
    for pinned in [false] {
        let mut base = h2_map(h1campaign::base_for("a10"));
        let opts = Options {
            objects: true,
            ai: true,
            scripts: Some(&s),
            sounds: 1000,
            donors: Some(&donors),
            pinned,
            ..Default::default()
        };
        let done = h1campaign::convert(&h1, &mut base, &opts).unwrap();
        let mut m = Map::from_reader(Cursor::new(done.bytes)).unwrap();
        let mem = map_memory(&mut m);
        let scnr = m.scenario().unwrap().clone();
        let bytes = m.read_tag_bytes(scnr.id).unwrap();
        let word = |b: &[u8], at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
        // The vehicle palette: `scnr +0x78`, 0x28 per entry, the tag
        // reference first.
        let (count, address) = (word(&bytes, 0x78), word(&bytes, 0x7c));
        for i in 0..count {
            let e = mem.slice(address + i * 0x28, 0x28).unwrap();
            let id = TagId(word(e, 4));
            let Ok(vehi) = m.tag(id).cloned() else {
                continue;
            };
            let v = m.read_tag_bytes(id).unwrap();
            let hlmt = TagId(word(&v, 0x38));
            let phmo = m
                .read_tag_bytes(hlmt)
                .ok()
                .map_or(u32::MAX, |h| word(&h, 0x24));
            if phmo == u32::MAX {
                bad.push(format!(
                    "pinned {pinned}: palette {i} {:?}: its model has no physics model",
                    vehi.name
                ));
            }
        }
        println!("a10 pinned {pinned}: {count} vehicle palette entries");
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

/// Every Halo 1 machine, control and light fixture placement on every
/// level converts and lands in the scenario: machines then light fixtures
/// in the machine block (`+0xa8`, `0x48` each; Halo 2 has no `lifi` tags),
/// controls in theirs (`+0xb8`, `0x44`). Each lands with its palette entry
/// (a `mach` or `ctrl` built from the definitions that re-encodes
/// byte-identical), Halo 1's power and position groups (indices into the
/// converted device groups, `+0xa0`) and its flags; every Halo 1 name of a
/// device names its placement.
#[test]
#[ignore = "reads disc/halo1.iso and disc/halo2.iso"]
fn every_device_placement_lands_with_its_device_groups() {
    use h1campaign::scenario::h1 as h1s;
    use tags::defs::{self, Defs};
    use tags::repo::{MapSource, extract::SharedFiles};
    let s = scripting();
    let donors = donors();
    let only = std::env::var("H1_LEVELS").ok();
    let mut totals = [0usize; 4];
    for level in h1map::disc::CAMPAIGN_LEVELS {
        if only
            .as_deref()
            .is_some_and(|o| !o.split(',').any(|l| l == level))
        {
            continue;
        }
        let h1 = h1_level(level);
        let sd = h1.tag_data(h1.scenario()).unwrap().to_vec();
        let h1_elements = |(at, stride): (usize, usize)| -> Vec<Vec<u8>> {
            let b = h1map::Block::read(&sd, at).unwrap();
            h1.block(b, stride)
                .unwrap()
                .chunks_exact(stride)
                .map(<[u8]>::to_vec)
                .collect()
        };
        let machines = h1_elements(h1s::MACHINES);
        let fixtures = h1_elements(h1s::LIGHT_FIXTURES);
        let controls = h1_elements(h1s::CONTROLS);
        let groups = h1_elements(h1scripts::level::h1::DEVICE_GROUPS).len();
        let mut base = h2_map(h1campaign::base_for(level));
        let opts = Options {
            objects: true,
            ai: true,
            scripts: Some(&s),
            sounds: 1000,
            donors: Some(&donors),
            ..Default::default()
        };
        let done = h1campaign::convert(&h1, &mut base, &opts).unwrap();
        let bad: Vec<_> = done
            .report
            .dropped
            .iter()
            .filter(|d| {
                ["machines", "controls", "light fixtures"]
                    .iter()
                    .any(|k| d.starts_with(k))
            })
            .collect();
        assert!(bad.is_empty(), "{level}: {bad:#?}");
        let bytes = done.bytes;
        let mut m = Map::from_reader(Cursor::new(bytes.clone())).unwrap();
        let mem = map_memory(&mut m);
        let scnr = m.scenario().unwrap().clone();
        let root = m.read_tag_bytes(scnr.id).unwrap();
        let word = |at: usize| u32::from_le_bytes(root[at..at + 4].try_into().unwrap());
        assert_eq!(word(0xa0) as usize, groups, "{level}: device groups");
        let schema = tags::schema::Schema::builtin();
        let mut shared = SharedFiles::default();
        let mut src = MapSource::new(
            schema,
            Map::from_reader(Cursor::new(bytes)).unwrap(),
            &mut shared,
        )
        .unwrap();
        let mut checked = std::collections::HashSet::new();
        for (group, at, stride, sources) in [
            (*b"mach", 0xa8usize, 0x48u32, [&machines, &fixtures]),
            (*b"ctrl", 0xb8, 0x44, [&controls, &Vec::new()]),
        ] {
            let def = Defs::builtin().group(FourCC(group)).unwrap();
            let want: Vec<(&Vec<u8>, bool)> = sources[0]
                .iter()
                .map(|e| (e, true))
                .chain(sources[1].iter().map(|e| (e, false)))
                .collect();
            assert_eq!(
                word(at) as usize,
                want.len(),
                "{level}: {} placements",
                FourCC(group)
            );
            for (i, (e, own_flags)) in want.iter().enumerate() {
                let p = mem.slice(word(at + 4) + i as u32 * stride, stride).unwrap();
                let h16 = |b: &[u8], o: usize| i16::from_le_bytes([b[o], b[o + 1]]);
                let h32 = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
                assert_eq!(
                    &p[8..0x20],
                    &e[8..0x20],
                    "{level} {i}: position and rotation"
                );
                for (h1_at, h2_at) in [(0x28, 0x34), (0x2a, 0x36)] {
                    let (g, want) = (h16(p, h2_at), h16(e, h1_at));
                    assert_eq!(g, want, "{level} {i}: device group at {h2_at:#x}");
                    assert!(g == -1 || (g as usize) < groups, "{level} {i}: group {g}");
                }
                assert_eq!(h32(p, 0x38), h32(e, 0x2c), "{level} {i}: device flags");
                if *own_flags {
                    assert_eq!(h32(p, 0x3c), h32(e, 0x30), "{level} {i}: own flags");
                }
                let pal = u32::from(u16::from_le_bytes([p[0], p[1]]));
                assert!(pal < word(at + 8), "{level} {i}: palette {pal}");
                let pe = mem.slice(word(at + 12) + pal * 0x28, 0x28).unwrap();
                let id = TagId(h32(pe, 4));
                let t = m.tag(id).unwrap().clone();
                assert_eq!(t.group, FourCC(group), "{level} {i}");
                if checked.insert(id) {
                    let tag = src.extract(id).unwrap().unwrap();
                    let r = defs::decode(def, &tag).unwrap();
                    let back = defs::encode(def, tag.key.clone(), &r, Default::default()).unwrap();
                    assert_eq!(
                        defs::canonical(&back),
                        defs::canonical(&tag),
                        "{level}: {:?} re-encodes byte-identical",
                        t.name
                    );
                }
            }
        }
        // Halo 1's names of devices name their placements.
        let names = h1_elements(h1campaign::scenario::h1::OBJECT_NAMES);
        let mut named = 0;
        for (i, n) in names.iter().enumerate() {
            let ty = i16::from_le_bytes([n[0x20], n[0x21]]);
            let at = i16::from_le_bytes([n[0x22], n[0x23]]);
            if !(7..=9).contains(&ty) || at < 0 {
                continue;
            }
            let e = mem.slice(word(0x4c) + i as u32 * 0x24, 0x24).unwrap();
            let (to_ty, to) = (
                i16::from_le_bytes([e[0x20], e[0x21]]),
                i16::from_le_bytes([e[0x22], e[0x23]]),
            );
            let want = match ty {
                7 => (7, at),
                8 => (8, at),
                _ => (7, machines.len() as i16 + at),
            };
            assert_eq!((to_ty, to), want, "{level}: name {i}");
            named += 1;
        }
        println!(
            "{level}: {} machines, {} light fixtures, {} controls placed; {groups} device groups; {named} device names; {} device tags re-encoded",
            machines.len(),
            fixtures.len(),
            controls.len(),
            checked.len()
        );
        totals[0] += machines.len();
        totals[1] += fixtures.len();
        totals[2] += controls.len();
        totals[3] += named;
    }
    println!(
        "all: {} machines, {} light fixtures, {} controls, {} names",
        totals[0], totals[1], totals[2], totals[3]
    );
}

/// Every physics model in a converted map (`$H1_LEVELS`, default a10;
/// fresh, the default build) as the engine's `phmo` loader (`0x001ea9c0`)
/// reads it at load:
/// - the blocks it rewrites in place (rigid bodies `+0x38`, lists
///   `+0x90`, list children `+0x98`, MOPPs `+0xa0`) belong to one tag
///   each: it turns each `{type, index}` shape reference there into a
///   pointer, so a block two tags share is resolved twice, and the second
///   reads a pointer as a reference and gets null. Version 42's a10
///   faulted that way at `0x002fc139` (a list's first child), its cryotube
///   vehicle and machine sharing equal list children;
/// - every shape reference names a shape the resolver (`0x001eaf30`)
///   knows and an element in range; a list of up to four children keeps
///   them inline, a longer one takes `child_count` entries from the list
///   children in list order, and those runs fill the block exactly;
/// - polyhedra with more than three corner groups take them from the four
///   vectors in order, every polyhedron its planes from the plane
///   equations in order, and both runs fill their blocks exactly.
#[test]
#[ignore = "reads disc/halo1.iso and disc/halo2.iso"]
fn physics_models_load_as_the_engine_reads_them() {
    let s = scripting();
    let donors = donors();
    let levels = std::env::var("H1_LEVELS").unwrap_or_else(|_| "a10".into());
    let word = |b: &[u8], at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
    let half = |w: u32| ((w & 0xffff) as usize, (w >> 16) as usize);
    for level in levels.split(',') {
        let h1 = h1_level(level);
        let mut base = h2_map(h1campaign::base_for(level));
        let opts = Options {
            objects: true,
            ai: true,
            scripts: Some(&s),
            sounds: 1000,
            donors: Some(&donors),
            ..Default::default()
        };
        let done = h1campaign::convert(&h1, &mut base, &opts).unwrap();
        let mut m = Map::from_reader(Cursor::new(done.bytes)).unwrap();
        let mem = map_memory(&mut m);
        let phmos: Vec<_> = m
            .tags()
            .iter()
            .filter(|t| t.group == FourCC(*b"phmo"))
            .map(|t| (t.name.clone().unwrap_or_default(), t.index_address))
            .collect();
        let mut owner = std::collections::HashMap::<u32, String>::new();
        let mut bad = Vec::new();
        for (name, at) in &phmos {
            let r = mem.slice(*at, 0x110).unwrap();
            let block = |f: usize| (word(r, f) as usize, word(r, f + 4));
            for (f, what) in [
                (0x38, "rigid bodies"),
                (0x90, "lists"),
                (0x98, "list children"),
                (0xa0, "mopps"),
            ] {
                let (n, a) = block(f);
                if n == 0 {
                    continue;
                }
                if let Some(o) = owner.insert(a, name.clone()) {
                    bad.push(format!("{name}: {what} at {a:#x} shared with {o}"));
                }
            }
            // Element counts the resolver indexes, by shape type.
            let counts: Vec<(u16, usize)> = [
                (0u16, 0x48usize),
                (1, 0x58),
                (2, 0x60),
                (3, 0x68),
                (4, 0x70),
                (5, 0x50),
                (6, 0x108),
                (0xe, 0x90),
                (0xf, 0xa0),
            ]
            .iter()
            .map(|&(t, f)| (t, block(f).0))
            .collect();
            let valid = |reference: u32| {
                let (t, i) = half(reference);
                counts.iter().any(|&(k, n)| k as usize == t && i < n)
            };
            let (nb, rb) = block(0x38);
            for b in 0..nb {
                let e = mem.slice(rb + b as u32 * 0x90, 0x90).unwrap();
                if !valid(word(e, 0x38)) {
                    bad.push(format!("{name}: rigid body {b} shape {:#x}", word(e, 0x38)));
                }
            }
            let (nl, lists) = block(0x90);
            let (nc, children) = block(0x98);
            let mut cursor = 0;
            for l in 0..nl {
                let e = mem.slice(lists + l as u32 * 0x38, 0x38).unwrap();
                let (count, cap) = (word(e, 0x10) as usize, word(e, 0x14) & 0x7fff_ffff);
                let refs: Vec<u32> = if cap <= 4 {
                    (0..count.min(4)).map(|k| word(e, 0x18 + 8 * k)).collect()
                } else {
                    let run = (cursor..cursor + count)
                        .map(|k| word(mem.slice(children + k as u32 * 8, 8).unwrap(), 0))
                        .collect();
                    cursor += count;
                    run
                };
                if count == 0 || (cap <= 4 && count > 4) || refs.iter().any(|&x| !valid(x)) {
                    bad.push(format!(
                        "{name}: list {l} ({count} children, capacity {cap})"
                    ));
                }
            }
            if cursor != nc {
                bad.push(format!("{name}: lists take {cursor} of {nc} list children"));
            }
            let (np, polys) = block(0x70);
            let (nfv, nplanes) = (block(0x78).0, block(0x80).0);
            let (mut fv, mut planes) = (0, 0);
            for p in 0..np {
                let e = mem.slice(polys + p as u32 * 0x100, 0x100).unwrap();
                if word(e, 0x58) & 0x7fff_ffff > 3 {
                    fv += word(e, 0x54) as usize;
                }
                planes += word(e, 0xf8) as usize;
            }
            if (fv, planes) != (nfv, nplanes) {
                bad.push(format!(
                    "{name}: polyhedra take {fv} of {nfv} four vectors, {planes} of {nplanes} planes"
                ));
            }
        }
        println!("{level}: {} physics models", phmos.len());
        assert!(bad.is_empty(), "{level}: {bad:#?}");
    }
}

#[test]
#[ignore = "reads H2_DISC"]
fn migration_speech_events_use_retail_order_patterns() {
    let mut map = h2_map(h1campaign::base_for("a50"));
    let tag = map.tags().iter().find(|tag| tag.group == FourCC(*b"adlg")).unwrap().clone();
    let bytes = map.read_tag_bytes(tag.id).unwrap();
    let base = tag.location.unwrap().address;
    let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    let patterns = (word(0xc) - base) as usize;
    let index = (word(0x20) - base) as usize;
    for (event, pattern, name) in [
        (111, 138, "newordr_advance"),
        (114, 144, "newordr_retreat"),
    ] {
        let range = word(index + event * 4);
        assert_eq!(range & 0xffff, pattern);
        assert!(range >> 16 > 0);
        let string = word(patterns + pattern as usize * 0x40 + 4) as u16;
        assert_eq!(map.strings()[usize::from(string)].as_deref(), Some(name));
    }
}

#[test]
#[ignore = "reads H1_ISO and H2_DISC"]
fn rebuilt_crew_and_sentinel_characters_reach_their_own_animation_resources() {
    use h1objects::convert::h2::{HLMT_ANIMATION, MODEL};
    let reference = |bytes: &[u8], offset: usize| {
        TagId(u32::from_le_bytes(
            bytes[offset + 4..offset + 8].try_into().unwrap(),
        ))
    };
    for (level, kind) in [("a10", 8), ("c40", 13)] {
        let h1 = h1_level(level);
        let src = h1ai::h1::H1Ai::read(&h1).unwrap();
        let mut base = h2_map(h1campaign::BASE);
        let done = h1campaign::convert(
            &h1,
            &mut base,
            &Options {
                objects: false,
                ai: true,
                ..Default::default()
            },
        )
        .unwrap();
        let mut m = Map::from_reader(Cursor::new(done.bytes)).unwrap();
        let scenario = m.scenario().unwrap().clone();
        let bytes = m.read_tag_bytes(scenario.id).unwrap();
        let read = h1ai::h2::ScenarioAi::read(&tags::geometry::TagView::new(
            &bytes,
            scenario.location.unwrap().address,
        ))
        .unwrap();
        let mut seen = std::collections::HashSet::new();
        let mut graphs = std::collections::HashSet::new();
        for (i, v) in src.actor_palette.iter().enumerate() {
            if !src.actor_of(i).is_some_and(|a| a.kind.0 == kind) {
                continue;
            }
            let v = v.as_ref().unwrap();
            if !seen.insert(&v.name) {
                continue;
            }
            let c = read
                .character_palette
                .iter()
                .find_map(|&id| {
                    let t = m.tag(id).ok()?;
                    (t.name.as_deref() == Some(v.name.as_str())).then(|| t.clone())
                })
                .expect("actor variant in scenario char palette");
            let bytes = m.read_tag_bytes(c.id).unwrap();
            let decoded = h1ai::h2::Character::read(&tags::geometry::TagView::new(
                &bytes,
                c.location.unwrap().address,
            ))
            .unwrap();
            assert_eq!(decoded.kind, Some(kind));
            if !v.major_variant.id.is_none() {
                assert_eq!(
                    m.tag(decoded.major).unwrap().name.as_deref(),
                    Some(h1.tag(v.major_variant.id).unwrap().name.as_str())
                );
            }
            let cview = tags::geometry::TagView::new(&bytes, c.location.unwrap().address);
            let weapons = cview.block(&bytes, 0xcc, 0xcc).unwrap();
            if v.weapon.id.is_none() {
                assert!(weapons.is_empty());
            } else {
                assert_eq!(weapons.len(), 1);
                assert_eq!(
                    m.tag(reference(weapons[0], 4)).unwrap().name.as_deref(),
                    Some(h1.tag(v.weapon.id).unwrap().name.as_str())
                );
            }
            let b = m.read_tag_bytes(decoded.unit).unwrap();
            if kind == 13 {
                let source = h1.tag_data(h1.tag(v.unit.id).unwrap()).unwrap();
                assert_eq!(&b[0x2cc..0x2f8], &source[0x324..0x350]);
            }
            let hlmt = m.read_tag_bytes(reference(&b, MODEL)).unwrap();
            let graph_id = reference(&hlmt, HLMT_ANIMATION);
            if !graphs.insert(graph_id) {
                continue;
            }
            let source = h1objects::h1::Unit::read(&h1, h1.tag(v.unit.id).unwrap()).unwrap();
            let source_graph = h1.tag(source.object.animation.unwrap()).unwrap();
            assert_eq!(
                m.tag(graph_id).unwrap().name.as_deref(),
                Some(source_graph.name.as_str())
            );
            let graph = tags::anim::AnimationGraph::read(&mut m, graph_id).unwrap();
            assert_eq!(
                graph.animations.len(),
                h1models::graph_names(&h1, source_graph).unwrap().len()
            );
            if kind == 13 {
                assert_eq!(graph.animations.len(), 13);
                assert_eq!(graph.blend_screens.len(), 1);
                for stance in ["combat", "patrol", "crouch"] {
                    let strings = m.strings();
                    let mode = graph
                        .modes
                        .iter()
                        .find(|m| m.name.resolve(strings).unwrap() == Some(stance))
                        .unwrap();
                    let class = mode
                        .classes
                        .iter()
                        .find(|c| c.name.resolve(strings).unwrap() == Some("any"))
                        .unwrap();
                    let ty = class
                        .types
                        .iter()
                        .find(|t| t.name.resolve(strings).unwrap() == Some("any"))
                        .unwrap();
                    assert!(
                        ty.actions
                            .iter()
                            .any(|a| a.name.resolve(strings).unwrap() == Some("idle"))
                    );
                    assert!(
                        ty.overlays
                            .iter()
                            .any(|a| a.name.resolve(strings).unwrap() == Some("look"))
                    );
                    if stance != "crouch" {
                        assert!(
                            ty.overlays
                                .iter()
                                .any(|a| a.name.resolve(strings).unwrap() == Some("aim_still_up"))
                        );
                    }
                }
            }
            assert!(!graph.animations.is_empty());
            for (j, resource) in graph.resources.iter().enumerate() {
                assert_eq!(resource.owner, graph_id);
                let p = resource.pointer.unwrap();
                assert_eq!(p.file, map::ResourceFile::Local);
                let raw = m
                    .read_file_range(p.offset.into(), resource.size.into())
                    .unwrap();
                for (k, _) in graph
                    .animations
                    .iter()
                    .enumerate()
                    .filter(|(_, a)| a.resource_index as usize == j)
                {
                    assert!(
                        graph.engine_problems(k, &raw).is_empty(),
                        "{level} clip {k}"
                    );
                }
            }
            assert!(!graph.resources.is_empty());
        }
        assert!(!seen.is_empty());
        assert_eq!(graphs.len(), 1);
    }
}
