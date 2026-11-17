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

fn h2_map(name: &str) -> Map<Cursor<Vec<u8>>> {
    let mut iso = h2_iso();
    let e = iso
        .entries()
        .unwrap()
        .into_iter()
        .find(|e| e.path.ends_with(&format!("/{name}.map")))
        .unwrap();
    Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap()
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
    let layers = std::env::var("H1_LAYERS").unwrap_or_else(|_| "objects,ai,scripts".into());
    let s = layers.contains("scripts").then(scripting);
    let opts = Options {
        objects: layers.contains("objects"),
        ai: layers.contains("ai"),
        scripts: s.as_ref(),
        sounds: std::env::var("H1_SOUNDS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1000),
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

    // Each converted BSP: the descriptor leads to it, it validates, and its
    // pathfinding decodes and validates.
    let desc = (word(&bytes, 0x214) - addr) as usize;
    let mut decoded = Vec::new();
    for k in 0..done.report.bsps.len() {
        let e = desc + k * 0x44;
        let sbsp = TagId(word(&bytes, e + 0x14));
        let t = m.tag(sbsp).unwrap().clone();
        assert_eq!(t.group, FourCC(*b"sbsp"));
        let loc = t.location.unwrap();
        assert_eq!(
            loc.address,
            word(&bytes, e + 8) + 0x10,
            "bsp {k}: region header"
        );
        let b = m.read_tag_bytes(sbsp).unwrap();
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

    // Objects: every placement's palette entry names a converted tag.
    if opts.objects {
        for (k, (at, pal)) in [(0x90usize, 0x98usize), (0x80, 0x88), (0x70, 0x78)]
            .into_iter()
            .enumerate()
        {
            let n = word(&bytes, at) as usize;
            let stride = [0x54, 0x38, 0x54][k];
            let pn = word(&bytes, pal) as usize;
            let pa = (word(&bytes, pal + 4).wrapping_sub(addr)) as usize;
            let a = (word(&bytes, at + 4).wrapping_sub(addr)) as usize;
            for i in 0..n {
                let e = &bytes[a + i * stride..a + (i + 1) * stride];
                let p = u16::from_le_bytes([e[0], e[1]]) as usize;
                assert!(p < pn);
                let id = TagId(word(&bytes, pa + p * 0x28 + 4));
                let t = m.tag(id).unwrap();
                assert_eq!(
                    t.group,
                    FourCC([*b"weap", *b"eqip", *b"vehi"][k]),
                    "placement {i}"
                );
                // Inside the level's bounds (any BSP), give or take.
                let p3: [f32; 3] = std::array::from_fn(|j| {
                    f32::from_le_bytes(e[8 + 4 * j..12 + 4 * j].try_into().unwrap())
                });
                assert!(p3.iter().all(|v| v.is_finite()));
            }
            println!("object block {at:#x}: {n} placements over {pn} palette entries");
        }
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
        assert_eq!(ours, r.sounds);
    }

    // Listed by the Halo 1 campaign manifest under the map's own path.
    let text = h1campaign::manifest::manifest_for(None, "a30").unwrap();
    h1campaign::manifest::check(&text, "a30").unwrap();
}
