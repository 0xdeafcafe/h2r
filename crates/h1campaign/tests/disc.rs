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
        // Halo 1 name, or reported left out.
        let lsnd = m
            .tags()
            .iter()
            .filter(|t| t.group == FourCC(*b"lsnd") && t.location.is_some())
            .filter(|t| {
                t.name
                    .as_deref()
                    .is_some_and(|n| h1.find(h1sounds::looping::LSND, n).is_some())
            })
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
        // Lifepods stand on the pelican and keep none of its looping
        // sounds (`h1objects::convert::QUIET_STAND_INS`).
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
            if !h1objects::convert::quiet_stand_in(&name) {
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
        println!("  {pods} lifepods");
    }
    assert!(failed.is_empty(), "{failed:#?}");
}
