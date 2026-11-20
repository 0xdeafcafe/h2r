//! Converted collision checked against its Halo 1 source on real maps from
//! `$H1_MAP_DIR` (each `*.map` in it, as stored on the disc).

use std::io::Cursor;

use h1convert::collision::convert;
use h1convert::query::{first_hit, leaf_at};
use h1map::H1Map;

/// A small deterministic generator, so a failure names a reproducible case.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
    fn point(&mut self, b: [f32; 6]) -> [f32; 3] {
        // A margin outside the bounds too, so solid outside the level counts.
        std::array::from_fn(|i| {
            let (lo, hi) = (b[2 * i] - 2.0, b[2 * i + 1] + 2.0);
            lo + (hi - lo) * self.next()
        })
    }
}

#[test]
#[ignore = "reads Halo 1 maps from $H1_MAP_DIR"]
fn converted_collision_agrees_with_the_source_on_points_rays_and_surfaces() {
    let dir = std::env::var("H1_MAP_DIR").expect("set H1_MAP_DIR");
    let mut maps = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "map") {
            continue;
        }
        let m = H1Map::from_reader(Cursor::new(std::fs::read(&path).unwrap())).unwrap();
        let name = m.header().name.clone();
        for r in m.structure_bsps().unwrap() {
            let s = m.structure_bsp(&r).unwrap();
            let materials = s.collision_materials().unwrap().len();
            let bounds = s.world_bounds();
            for src in s.collision_bsps().unwrap() {
                let h2 = convert(&src).unwrap();
                // Through Halo 2's own encoding and decoder.
                let bytes = h2.encode_blocks(0x8010_0000);
                let h2 = tags::collision::Bsp::decode_blocks(&bytes, 0x8010_0000, 0).unwrap();
                // 0xffff (no material) is kept, as Halo 2's structure BSPs
                // have it too (h2_sbsp.rs); anything else must validate.
                let bad: Vec<_> = h2
                    .validate(materials)
                    .into_iter()
                    .filter(|p| !p.ends_with("material 65535 out of range"))
                    .collect();
                assert_eq!(bad, Vec::<String>::new(), "{name}");

                // Every surface's polygon is the same.
                for i in 0..src.surfaces.len() {
                    let a: Vec<[f32; 3]> = src
                        .surface_ring(i)
                        .unwrap()
                        .into_iter()
                        .map(|v| src.vertices[v].point)
                        .collect();
                    let b: Vec<[f32; 3]> = h2
                        .surface_ring(i)
                        .into_iter()
                        .map(|v| h2.vertices[v].point)
                        .collect();
                    assert_eq!(a, b, "{name}: surface {i}");
                }

                let mut rng = Lcg(0x1234_5678 ^ maps as u64);
                let (mut open, mut hits) = (0, 0);
                const POINTS: usize = 50_000;
                const RAYS: usize = 20_000;
                for _ in 0..POINTS {
                    let p = rng.point(bounds);
                    let (a, b) = (leaf_at(&src, p), leaf_at(&h2, p));
                    assert_eq!(a, b, "{name}: point {p:?}");
                    assert_eq!(a, src.leaf_at(p).map(|l| l as usize));
                    open += a.is_some() as usize;
                }
                for _ in 0..RAYS {
                    let (p, q) = (rng.point(bounds), rng.point(bounds));
                    let (a, b) = (first_hit(&src, p, q), first_hit(&h2, p, q));
                    assert_eq!(a, b, "{name}: segment {p:?} -> {q:?}");
                    hits += a.is_some() as usize;
                }
                // Segments that start in open space, where a hit is a wall.
                let (mut from_open, mut walls) = (0, 0);
                while from_open < RAYS {
                    let p = rng.point(bounds);
                    if leaf_at(&src, p).is_none() {
                        continue;
                    }
                    let q = rng.point(bounds);
                    let (a, b) = (first_hit(&src, p, q), first_hit(&h2, p, q));
                    assert_eq!(a, b, "{name}: segment {p:?} -> {q:?}");
                    assert_ne!(a, Some(0.0));
                    from_open += 1;
                    walls += a.is_some() as usize;
                }
                eprintln!("{name}: {walls}/{RAYS} segments from open space hit a wall; all agree");
                assert!(walls > 0 && walls < RAYS);
                // Straight down from every spawn: ground within a metre.
                for sp in m.multiplayer().unwrap().starting_locations {
                    let [x, y, z] = sp.position;
                    let (from, to) = ([x, y, z + 0.3], [x, y, z - 1.0]);
                    let a = first_hit(&src, from, to);
                    assert!(a.is_some(), "{name}: no ground under {:?}", sp.position);
                    assert_eq!(a, first_hit(&h2, from, to));
                }
                eprintln!(
                    "{name}: {} nodes, {} surfaces; {open}/{POINTS} points open, {hits}/{RAYS} segments hit; all agree",
                    src.nodes.len(),
                    src.surfaces.len()
                );
                // Some of each, or the test proves nothing.
                assert!(open > 0 && open < POINTS && hits > 0 && hits < RAYS);
            }
        }
        maps += 1;
    }
    assert!(maps > 0, "no maps in {dir}");
}

#[test]
#[ignore = "reads Halo 1 maps from $H1_MAP_DIR"]
fn the_halo_2_sandbox_plan_of_each_map() {
    let dir = std::env::var("H1_MAP_DIR").expect("set H1_MAP_DIR");
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "map") {
            continue;
        }
        let m = H1Map::from_reader(Cursor::new(std::fs::read(&path).unwrap())).unwrap();
        let mp = m.multiplayer().unwrap();
        let p = h1convert::placements::plan_map(&m).unwrap();
        eprintln!(
            "{}: {} spawns, {} flags, {} items; dropped {}",
            m.header().name,
            p.spawns.len(),
            p.flags.len(),
            p.items.len(),
            p.dropped.len()
        );
        for d in &p.dropped {
            eprintln!("  dropped {d}");
        }
        let mut roles: Vec<_> = p.items.iter().map(|i| format!("{:?}", i.item)).collect();
        roles.sort();
        roles.dedup();
        eprintln!("  roles {roles:?}");
        assert_eq!(p.spawns.len(), mp.starting_locations.len());
        // Every item spawn is placed or reported.
        assert_eq!(
            p.items.len() + p.dropped.iter().filter(|d| !d.starts_with("flag")).count(),
            mp.equipment.len() + mp.vehicles.len()
        );
        // Nothing unrecognised: every drop is a known Halo 2 gap.
        for d in p.dropped.iter().filter(|d| !d.starts_with("flag")) {
            assert!(
                [
                    "ammo",
                    "health pack",
                    "double speed",
                    "full-spectrum vision"
                ]
                .iter()
                .any(|w| d.contains(w)),
                "{d}"
            );
        }
    }
}

/// The Halo 2 sandbox plan written into a retail Halo 2 base map's scenario
/// (`$H2_DISC`, base `$H2_BASE`, default `beavercreek`): every role must
/// resolve to one of the base's collections, and the result reads back.
#[test]
#[ignore = "reads $H1_MAP_DIR and the Halo 2 disc ($H2_DISC)"]
fn the_plan_applies_to_a_retail_base() {
    use scenario_edit::{BlockKind, Scenario};
    let disc = std::env::var("H2_DISC").expect("set H2_DISC");
    let base_name = std::env::var("H2_BASE").unwrap_or("beavercreek".into());
    let mut iso =
        xiso::Xiso::open(std::io::BufReader::new(std::fs::File::open(&disc).unwrap())).unwrap();
    let entry = iso
        .entries()
        .unwrap()
        .into_iter()
        .find(|e| e.path.ends_with(&format!("{base_name}.map")))
        .unwrap();
    let base_bytes = iso.read(&entry).unwrap();
    let dir = std::env::var("H1_MAP_DIR").expect("set H1_MAP_DIR");
    let path = std::path::Path::new(&dir).join("beavercreek.map");
    let h1 = H1Map::from_reader(Cursor::new(std::fs::read(path).unwrap())).unwrap();
    let plan = h1convert::placements::plan_map(&h1).unwrap();
    let mut base = map::Map::from_reader(Cursor::new(base_bytes)).unwrap();
    let tags: Vec<_> = base
        .tags()
        .iter()
        .map(|t| (t.group, t.id, t.name.clone().unwrap_or_default()))
        .collect();
    let mut s = Scenario::read(&mut base).unwrap();
    let done = h1convert::apply::apply(&mut s, &plan, &tags, &mut vec![]).unwrap();
    eprintln!("{done:?}");
    for (g, _, n) in tags
        .iter()
        .filter(|(g, _, _)| g.0 == *b"itmc" || g.0 == *b"vehc")
    {
        eprintln!("  base {} {n}", String::from_utf8_lossy(&g.0));
    }
    // Every weapon resolves; a powerup the base lacks (camouflage on some
    // bases) is reported, not placed.
    assert!(
        done.unresolved.iter().all(|u| u.starts_with("Equipment")),
        "{:?}",
        done.unresolved
    );
    let mut out = Cursor::new(Vec::new());
    scenario_edit::save::write_patched(&mut base, &s, &mut out).unwrap();
    let mut m = map::Map::from_reader(Cursor::new(out.into_inner())).unwrap();
    let back = Scenario::read(&mut m).unwrap();
    assert_eq!(
        back.block(BlockKind::StartingLocations).len(),
        plan.spawns.len()
    );
    assert_eq!(back.block(BlockKind::NetgameFlags).len(), plan.flags.len());
    assert_eq!(
        back.block(BlockKind::NetgameEquipment).len(),
        done.counts[2]
    );
    assert_eq!(done.counts[2] + done.unresolved.len(), plan.items.len());
}

/// Test only: a MOPP block that loads but emits nothing. It never goes into
/// a map for play (`convert` refuses without real MOPP code); it lets the
/// rest of a converted map be read back.
fn stand_in_mopp() -> Vec<u8> {
    let mut m = vec![0u8; 0x31];
    m[0x20] = 1;
    m
}

#[test]
#[ignore = "reads beavercreek from $H1_MAP_DIR and the Halo 2 disc ($H2_DISC)"]
fn beavercreek_converts_onto_its_halo_2_base_and_reads_back() {
    use h1convert::convert::{Sandbox, convert, prepare};
    use tags::structure::{Lightmap, StructureRender};
    let disc = std::env::var("H2_DISC").expect("set H2_DISC");
    let h1_name = std::env::var("H1_MAP").unwrap_or("beavercreek".into());
    let base_name = h1convert::convert::known(&h1_name).map_or("beavercreek", |k| k.1);
    let mut iso =
        xiso::Xiso::open(std::io::BufReader::new(std::fs::File::open(&disc).unwrap())).unwrap();
    let entry = iso
        .entries()
        .unwrap()
        .into_iter()
        .find(|e| e.path.ends_with(&format!("{base_name}.map")))
        .unwrap();
    let base_bytes = iso.read(&entry).unwrap();
    let dir = std::env::var("H1_MAP_DIR").expect("set H1_MAP_DIR");
    let file = std::fs::read(std::path::Path::new(&dir).join(format!("{h1_name}.map"))).unwrap();
    let sha = content::Sha256::of(&file);
    let h1 = H1Map::from_reader(Cursor::new(file)).unwrap();
    let mut base = map::Map::from_reader(Cursor::new(base_bytes)).unwrap();

    // The real entry point writes a whole map, which reads back.
    let done = convert(&h1, sha, &mut base, Sandbox::Halo2).unwrap();
    let back = map::Map::from_reader(Cursor::new(done.bytes.clone())).unwrap();
    assert!(!back.tags().is_empty());
    assert!(matches!(
        prepare(&h1, sha, &mut base, Sandbox::Halo1).unwrap_err(),
        h1convert::Error::Halo1Sandbox
    ));

    let p = prepare(&h1, sha, &mut base, Sandbox::Halo2).unwrap();
    let vertices: usize = p.sections.iter().map(|s| s.vertices.len()).sum();
    let triangles: usize = p.sections.iter().map(|s| s.indices.len() / 3).sum();
    eprintln!(
        "{h1_name}: {} clusters, {vertices} vertices, {triangles} triangles, {} materials, \
         {} collision materials, flipped {}, placements {:?}, unresolved {:?}, dropped {:?}",
        p.sections.len(),
        p.materials.len(),
        p.collision_materials,
        p.flipped,
        p.applied.counts,
        p.applied.unresolved,
        p.dropped
    );
    let done = p.build(&mut base, &stand_in_mopp()).unwrap();
    assert_eq!(done.name, format!("h1_{h1_name}"));
    eprintln!("{} bytes", done.bytes.len());

    let mut m = map::Map::from_reader(Cursor::new(done.bytes.clone())).unwrap();
    m.validate_checksum().unwrap();
    assert_eq!(m.header().name, done.name);
    assert_eq!(
        m.header().scenario_path,
        format!("scenarios\\multi\\{0}\\{0}", done.name)
    );
    // The scenario's descriptor leads to the new BSP and lightmap.
    let sbsp = m
        .tags()
        .iter()
        .find(|t| t.group.0 == *b"sbsp")
        .unwrap()
        .clone();
    let ltmp = m
        .tags()
        .iter()
        .find(|t| t.group.0 == *b"ltmp")
        .unwrap()
        .clone();
    let render = StructureRender::read(&mut m, sbsp.id).unwrap();
    assert_eq!(render.clusters.len(), p.sections.len());
    assert_eq!(render.material_count as usize, p.materials.len());
    let file = done.bytes.clone();
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
    let mut raw = Mem(file);
    let mut loaded = 0;
    for c in &render.clusters {
        let s = c
            .geometry
            .load(&mut raw, Some(render.material_count as usize))
            .unwrap()
            .unwrap();
        loaded += s.indices.len() / 3;
    }
    assert_eq!(loaded, triangles);
    let light = Lightmap::read(&mut m, ltmp.id).unwrap();
    assert_eq!(light.groups[0].structure_checksum, render.checksum);
    assert_eq!(
        light.groups[0].cluster_render_info.len(),
        render.clusters.len()
    );
    let bytes = m.read_tag_bytes(sbsp.id).unwrap();
    let at = sbsp.location.unwrap().address;
    let w = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
    let col = tags::collision::Bsp::decode_blocks(&bytes, at, (w(0x18) - at) as usize).unwrap();
    assert_eq!(col, p.collision);

    // Every trigger volume of the base moved out of the level.
    let scnr = m.scenario().unwrap().clone();
    let sb = m.read_tag_bytes(scnr.id).unwrap();
    let sa = scnr.location.unwrap().address;
    let sw = |o: usize| u32::from_le_bytes(sb[o..o + 4].try_into().unwrap());
    assert_eq!(sw(0x108) as usize, p.trigger_volumes);
    let tv = (sw(0x10c).wrapping_sub(sa)) as usize;
    for i in 0..p.trigger_volumes {
        let z = f32::from_bits(sw(tv + 0x44 * i + 0x2c));
        assert_eq!(z, -1.0e6);
    }
    eprintln!("{} trigger volumes parked", p.trigger_volumes);

    // The placements survived the rebuild.
    let back = scenario_edit::Scenario::read(&mut m).unwrap();
    assert_eq!(
        back.block(scenario_edit::BlockKind::StartingLocations)
            .len(),
        p.applied.counts[0]
    );

    // The manifest: a derived map with its origin.
    let man = &done.manifest;
    man.validate().unwrap();
    assert!(man.base.derived);
    assert_eq!(man.base.map, done.name);
    assert!(man.patches.is_empty());
    let v = &man.variants[0];
    assert_eq!(v.file, format!("h2/{}.map", done.name));
    assert_eq!(v.sha256, content::Sha256::of(&done.bytes));
    assert_eq!(man.select_variant(None).patches[0].file, v.file);
    let o = &v.origin;
    assert_eq!(
        (o.game.as_str(), o.map.as_str(), o.sandbox.as_str()),
        ("halo1", h1_name.as_str(), "h2")
    );
    assert_eq!(o.source_sha256, sha);
    eprintln!("{}", man.to_toml());
}
