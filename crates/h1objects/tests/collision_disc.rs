//! Object collision against the discs: the retail BSP physics rule, and
//! every Halo 1 campaign collision model converted, read back and queried
//! against Halo 1's own tree. Reads `$H1_ISO` and `$H2_DISC` at run time;
//! prints counts and sizes only. Nothing is written.
//!
//! ```sh
//! H1_ISO=... H2_DISC=... cargo test -p h1objects --test collision_disc -- --ignored --nocapture
//! ```

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Cursor};

use h1map::{Group, H1Map};
use h1objects::base::Base;
use h1objects::coll::convert_collision;
use h1objects::convert::{Converter, h2};
use h1objects::h1::{self, Collision};
use h1objects::query::{Child, locate, ray};
use map::build::{FixupKind, TagSet};
use map::{CacheType, FourCC, Map, TagId};

fn disc(var: &str, name: &str) -> String {
    std::env::var(var)
        .unwrap_or_else(|_| format!("{}/../../disc/{name}", env!("CARGO_MANIFEST_DIR")))
}

fn h2_maps(only: &[&str]) -> Vec<(String, Map<Cursor<Vec<u8>>>)> {
    let mut iso = xiso::Xiso::open(BufReader::new(
        File::open(disc("H2_DISC", "halo2.iso")).unwrap(),
    ))
    .unwrap();
    let entries = iso.entries().unwrap();
    entries
        .into_iter()
        .filter_map(|e| {
            let name = e
                .path
                .rsplit(['/', '\\'])
                .next()?
                .strip_suffix(".map")?
                .to_owned();
            if !only.is_empty() && !only.contains(&name.as_str()) {
                return None;
            }
            let m = Map::from_reader(Cursor::new(iso.read(&e).ok()?)).ok()?;
            Some((name, m))
        })
        .collect()
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

const OBJECTS: [&[u8; 4]; 13] = [
    b"bipd", b"vehi", b"weap", b"eqip", b"garb", b"proj", b"scen", b"mach", b"ctrl", b"lifi",
    b"bloc", b"crea", b"ssce",
];

/// Retail: a `coll` has one BSP physics slot per BSP exactly when a
/// scenery object (`scen`) with no `phmo` uses it, and none otherwise
/// (weapons, equipment, devices and bipeds without a `phmo` have none;
/// nothing with a `phmo` has any). Each slot's box holds its BSP and its
/// MOPP code keys the BSP's surfaces `0..n`, each once. Prints the `hlmt`
/// region bytes seen (collision, physics) by whether the object has a
/// `coll` and a `phmo`.
#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn retail_static_scenery_carries_bsp_physics() {
    let only = std::env::var("H2_MAPS").unwrap_or_default();
    let only: Vec<&str> = only.split(',').filter(|s| !s.is_empty()).collect();
    let mut rows = BTreeMap::<String, usize>::new();
    let mut bad = Vec::new();
    for (name, mut m) in h2_maps(&only) {
        let Ok(base) = Base::read(&mut m) else {
            continue;
        };
        // Per coll: (used by a scen with no phmo, a user's name).
        let mut colls = BTreeMap::<u32, (bool, String)>::new();
        for (id, g, tag, _) in base.tags.clone() {
            if !OBJECTS.iter().any(|o| g == FourCC(**o)) {
                continue;
            }
            let Ok(root) = base.head(id, 0x40) else {
                continue;
            };
            let Some(hlmt) = base.reference(&root, h2::MODEL) else {
                continue;
            };
            let h = base.head(hlmt, 0x80).unwrap();
            let coll = base.reference(&h, h2::HLMT_COLLISION);
            let phmo = base.reference(&h, h2::HLMT_PHYSICS_MODEL).is_some();
            let mut i = 0;
            while let Some(r) = base.element(&h, h2::HLMT_REGIONS, 0x10, i).unwrap() {
                let (cr, pr) = (
                    r[h2::HLMT_REGION_COLLISION] as i8,
                    r[h2::HLMT_REGION_PHYSICS] as i8,
                );
                let k = format!(
                    "region bytes: coll {}, phmo {phmo}: collision {}, physics {}",
                    coll.is_some(),
                    if cr < 0 { "-1" } else { ">= 0" },
                    if pr < 0 { "-1" } else { ">= 0" }
                );
                *rows.entry(k).or_default() += 1;
                i += 1;
            }
            let Some(c) = coll else { continue };
            let e = colls.entry(c.0).or_insert((false, tag.clone()));
            e.0 |= g == FourCC(*b"scen") && !phmo;
        }
        for (c, (static_scenery, user)) in colls {
            let col = tags::collision::Collision::read(&mut m, TagId(c)).unwrap();
            for r in &col.regions {
                for p in &r.permutations {
                    let want = if static_scenery { p.bsps.len() } else { 0 };
                    let ok = p.bsp_physics.len() == want;
                    *rows
                        .entry(format!(
                            "permutations: scen without phmo {static_scenery}, slots {}",
                            if ok { "as the rule" } else { "off the rule" }
                        ))
                        .or_default() += 1;
                    if !ok {
                        bad.push(format!(
                            "{name} {user}: {} slots, {} bsps",
                            p.bsp_physics.len(),
                            p.bsps.len()
                        ));
                    }
                    for (b, s) in p.bsps.iter().zip(&p.bsp_physics) {
                        let code = mopp::Code::parse(&s.mopp).unwrap();
                        let mut keys = mopp::check::validate(&code.bytecode).unwrap().keys;
                        keys.sort();
                        keys.dedup();
                        if keys != (0..b.surfaces.len() as u32).collect::<Vec<_>>() {
                            bad.push(format!(
                                "{name} {user}: keys {} of {}",
                                keys.len(),
                                b.surfaces.len()
                            ));
                        }
                        let inside = b.vertices.iter().all(|v| {
                            (0..3).all(|a| {
                                (v.point[a] - s.center[a]).abs() <= s.half_extents[a] + 1e-3
                            })
                        });
                        if !inside {
                            bad.push(format!("{name} {user}: a vertex outside the slot box"));
                        }
                        *rows.entry("slots checked".into()).or_default() += 1;
                    }
                }
            }
        }
    }
    eprintln!("{rows:#?}");
    for b in bad.iter().take(60) {
        eprintln!("{b}");
    }
    assert!(rows.get("slots checked").copied().unwrap_or(0) > 0);
    assert!(bad.is_empty(), "{} off the rule", bad.len());
}

/// A deterministic spread of numbers in `[0, 1)`.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// Every collision model on a Halo 1 campaign level (`$H1_LEVELS`,
/// default a10 and b40): converted with BSP physics, installed, built,
/// read back and validated; every slot's code keys every surface; and on
/// every BSP, points and rays through its box get the same answer from the
/// read-back Halo 2 tree as from Halo 1's.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn campaign_collision_converts_and_answers_as_halo1() {
    let levels = std::env::var("H1_LEVELS").unwrap_or("a10,b40".into());
    for level in levels.split(',') {
        let h1 = h1_level(level);
        let mut set = TagSet::new(CacheType::Multiplayer, "h1coll", "invented\\h1coll");
        let mut built = Vec::new();
        let (mut bsps, mut slot_bytes, mut mopp_bytes) = (0, 0, 0);
        for t in h1.tags().iter().filter(|t| t.group == Group::new(b"coll")) {
            let c = Collision::read(&h1, t).unwrap_or_else(|e| panic!("{}: {e}", t.name));
            let (conv, r) =
                convert_collision(&c, true).unwrap_or_else(|e| panic!("{}: {e}", t.name));
            assert_eq!(r.bsp_physics, r.bsps, "{}", t.name);
            bsps += r.bsps;
            slot_bytes += r.bsp_physics * tags::collision::BSP_PHYSICS_SIZE;
            mopp_bytes += r.mopp_bytes;
            let id = conv.install(&mut set, &t.name).unwrap();
            built.push((t, c, id));
        }
        let (bytes, _) = set.build().unwrap();
        let mut out = Map::from_reader(Cursor::new(bytes)).unwrap();
        let mut rng = Lcg(0x1234_5678);
        let (mut rays, mut hits, mut points, mut crates) = (0usize, 0usize, 0usize, Vec::new());
        // Entries into solid space with no surface there (an open mesh).
        let mut open_entries = 0usize;
        for (t, c, id) in &built {
            let got = tags::collision::Collision::read(&mut out, *id).unwrap();
            let v = got.validate();
            assert!(v.is_empty(), "{}: {v:?}", t.name);
            for (ri, region) in c.regions.iter().enumerate() {
                let perms = &got.regions[ri].permutations;
                for (pi, _) in region.permutations.iter().enumerate() {
                    let p = &perms[pi];
                    assert_eq!(p.bsp_physics.len(), p.bsps.len(), "{}", t.name);
                    let srcs: Vec<_> = c
                        .nodes
                        .iter()
                        .filter(|n| n.region as usize == ri)
                        .filter_map(|n| n.bsps.get(pi))
                        .collect();
                    assert_eq!(srcs.len(), p.bsps.len());
                    for ((src, b), s) in srcs.iter().zip(&p.bsps).zip(&p.bsp_physics) {
                        assert_eq!(s.owner, id.0);
                        let code = mopp::Code::parse(&s.mopp).unwrap();
                        let mut keys = mopp::check::validate(&code.bytecode).unwrap().keys;
                        keys.sort();
                        keys.dedup();
                        assert_eq!(
                            keys,
                            (0..b.surfaces.len() as u32).collect::<Vec<_>>(),
                            "{}",
                            t.name
                        );
                        // Rays and points through the box, a quarter wider.
                        let pick = |rng: &mut Lcg| -> [f32; 3] {
                            std::array::from_fn(|a| {
                                s.center[a] + s.half_extents[a] * 2.5 * (rng.next() - 0.5)
                            })
                        };
                        for _ in 0..64 {
                            let (a, z) = (pick(&mut rng), pick(&mut rng));
                            let (h1r, h2r) = (ray(*src, a, z).unwrap(), ray(b, a, z).unwrap());
                            assert_eq!(h1r, h2r, "{}: ray {a:?} -> {z:?}", t.name);
                            rays += 1;
                            if let Some(f) = h2r
                                && f > 0.0
                            {
                                hits += 1;
                                // Havok's view: the code reaches a surface
                                // near where the tree says the ray enters.
                                // Every surface within a centimetre of
                                // the entry point is a key the code emits
                                // there.
                                let at: [f32; 3] =
                                    std::array::from_fn(|i| a[i] + (z[i] - a[i]) * f);
                                let r = 0.01;
                                let near = mopp::query_sphere(&code, at, r).unwrap();
                                let bx = mopp::Aabb {
                                    min: std::array::from_fn(|i| at[i] - r),
                                    max: std::array::from_fn(|i| at[i] + r),
                                };
                                let touching: Vec<u32> = (0..b.surfaces.len())
                                    .filter(|&k| {
                                        let poly: Vec<[f32; 3]> = b
                                            .surface_ring(k)
                                            .into_iter()
                                            .map(|v| b.vertices[v].point)
                                            .collect();
                                        mopp::geom::polygon_overlaps(&poly, &bx)
                                    })
                                    .map(|k| k as u32)
                                    .collect();
                                if touching.is_empty() {
                                    open_entries += 1;
                                }
                                for k in &touching {
                                    assert!(
                                        near.contains(k),
                                        "{}: surface {k} not keyed at {at:?}",
                                        t.name
                                    );
                                }
                            }
                            let q = pick(&mut rng);
                            assert_eq!(locate(*src, q), locate(b, q), "{}: point {q:?}", t.name);
                            points += 1;
                        }
                    }
                }
            }
            // Props a player bumps into (a10 has no tag named crate; its
            // round human props are the oxygen tanks).
            if ["crate", "oxy tank", "barricade"]
                .iter()
                .any(|w| t.name.contains(w))
            {
                // The player's walk: a level line through the middle of
                // the crate's first BSP box, from outside on -x.
                let s = &got.regions[0].permutations[0].bsp_physics[0];
                let b = &got.regions[0].permutations[0].bsps[0];
                let mid = s.center;
                let a = [mid[0] - 3.0 * s.half_extents[0] - 1.0, mid[1], mid[2]];
                let z = [mid[0] + 3.0 * s.half_extents[0] + 1.0, mid[1], mid[2]];
                let src = &c.nodes.iter().find(|n| n.region == 0).unwrap().bsps[0];
                let (r1, r2) = (ray(src, a, z).unwrap(), ray(b, a, z).unwrap());
                assert_eq!(r1, r2);
                assert!(r2.is_some(), "{}: the walk passes through", t.name);
                assert_eq!(locate(b, mid), locate(src, mid));
                crates.push(format!(
                    "{} (centre {:?})",
                    t.name,
                    locate(b, mid) == Some(Child::Solid)
                ));
            }
        }
        eprintln!(
            "{level}: {} coll tags, {bsps} BSPs; {rays} rays ({hits} hits, {open_entries} with no surface at the entry) and {points} points agree; slots {slot_bytes} bytes + MOPP {mopp_bytes} bytes",
            built.len()
        );
        for c in &crates {
            eprintln!("  walk stopped by: {c}");
        }
    }
}

/// A level's objects (`$H1_LEVEL`, default a10) through the converter on
/// its campaign base: BSP physics only on scenery `coll`s with no `phmo`
/// (since converter version 85 scenery whose Halo 1 collision is solid
/// gets a built `phmo` instead, `rebuild_disc.rs`), and region 0 of every
/// `hlmt` with a `coll` selects a collision region. Prints the bytes the
/// slots add.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn level_objects_select_collision_with_bsp_physics() {
    let h1 = h1_level(&std::env::var("H1_LEVEL").unwrap_or("a10".into()));
    let (_, mut base_map) = h2_maps(&["05a_deltaapproach"]).pop().unwrap();
    let base = Base::read(&mut base_map).unwrap();
    let mut conv = Converter::new(&h1, &base);
    let mut objects = BTreeMap::<String, usize>::new();
    for t in h1.tags() {
        let g = t.group.0.to_be_bytes();
        if !matches!(&g, b"scen" | b"weap" | b"eqip" | b"vehi" | b"bipd") {
            continue;
        }
        let has_coll = h1::Object::read(&h1, h1.tag_data(t).unwrap())
            .map(|o| o.collision.is_some())
            .unwrap_or(false);
        match conv.object(t) {
            Ok(_) => {
                let r = conv.tree.reports.last().unwrap();
                *objects
                    .entry(format!(
                        "{} coll {has_coll}, phmo {}",
                        String::from_utf8_lossy(&g),
                        r.physics
                            .as_deref()
                            .is_some_and(|p| p.starts_with("borrowed"))
                    ))
                    .or_default() += 1;
            }
            Err(e) => {
                *objects
                    .entry(format!(
                        "{} not converted: {e}",
                        String::from_utf8_lossy(&g)
                    ))
                    .or_default() += 1
            }
        }
    }
    eprintln!("{objects:#?}");
    let (mut slots, mut extra) = (0, 0);
    for (name, r) in &conv.coll_reports {
        assert!(r.bsp_physics == 0 || r.bsp_physics == r.bsps, "{name}");
        slots += r.bsp_physics;
        if r.bsp_physics > 0 {
            eprintln!(
                "  scenery coll: {name}: {} bsps, {} surfaces, {} MOPP bytes",
                r.bsps, r.surfaces, r.mopp_bytes
            );
        }
        extra += r.bsp_physics * tags::collision::BSP_PHYSICS_SIZE + r.mopp_bytes;
    }
    // Every scenery hlmt: collision region bytes set where it has a coll.
    let mut scenery = 0;
    for (name, t) in &conv.tree.tags {
        if t.group != FourCC(*b"hlmt") {
            continue;
        }
        let coll = t.root.fixups.iter().find_map(|f| match &f.kind {
            FixupKind::TagRef(id) if f.offset == h2::HLMT_COLLISION as u32 => Some(*id),
            _ => None,
        });
        let regions = t.root.fixups.iter().find_map(|f| match &f.kind {
            FixupKind::Block { child, .. } if f.offset == h2::HLMT_REGIONS as u32 => Some(child),
            _ => None,
        });
        let (Some(coll), Some(regions)) = (coll, regions) else {
            continue;
        };
        let r0 = &regions.bytes[..0x10];
        if coll == TagId::NULL {
            assert_eq!(r0[h2::HLMT_REGION_COLLISION] as i8, -1, "{name}");
        } else {
            assert_ne!(
                r0[h2::HLMT_REGION_COLLISION] as i8,
                -1,
                "{name}: region 0 selects no collision"
            );
            scenery += 1;
        }
    }
    eprintln!(
        "{} coll tags, {slots} BSP physics slots ({extra} bytes with their MOPP code); {scenery} hlmts select a collision region",
        conv.coll_reports.len()
    );
}

/// Halo 1's collision as convex pieces (`h1objects::physics`), on every
/// collision model of every campaign level (`$H1_LEVELS` to narrow):
/// - a missing child is solid: just outside a surface (along its outward
///   normal) Halo 1's tree finds open space, just inside it solid;
/// - the solid cells are that solid space: at points through each BSP's
///   box, Halo 1's tree says solid exactly when a cell holds the point;
/// - every converted vehicle and device with solid cells gets a physics
///   model built from them, which reads back through the `phmo`
///   definition with every corner inside its piece's planes.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn halo1_solid_space_becomes_physics_pieces() {
    use h1objects::physics::{Hull, solid_cells};
    use tags::defs::{self, Defs, Val};
    let levels =
        std::env::var("H1_LEVELS").unwrap_or_else(|_| h1map::disc::CAMPAIGN_LEVELS.join(","));
    let (_, mut base_map) = h2_maps(&["05a_deltaapproach"]).pop().unwrap();
    let base = Base::read(&mut base_map).unwrap();
    let phmo_def = Defs::builtin().group(FourCC(*b"phmo")).unwrap();
    let inside = |h: &Hull, p: [f32; 3]| {
        h.planes
            .iter()
            .all(|pl| pl[0] * p[0] + pl[1] * p[1] + pl[2] * p[2] + pl[3] <= 0.0)
    };
    let (mut out_open, mut out_solid, mut in_open, mut in_solid) = (0, 0, 0, 0);
    let (mut agree, mut disagree, mut cells_total) = (0usize, 0usize, 0usize);
    let mut built = BTreeMap::<String, usize>::new();
    for level in levels.split(',') {
        let h1 = h1_level(level);
        let mut rng = Lcg(0x5eed);
        for t in h1.tags().iter().filter(|t| t.group == Group::new(b"coll")) {
            let c = Collision::read(&h1, t).unwrap();
            for n in &c.nodes {
                let Some(src) = n.bsps.first() else { continue };
                let bsp = h1convert::collision::convert(src).unwrap();
                let cells = solid_cells(&bsp);
                cells_total += cells.len();
                for (si, s) in bsp.surfaces.iter().enumerate() {
                    let ring = bsp.surface_ring(si);
                    if ring.len() < 3 {
                        continue;
                    }
                    let mid: [f32; 3] = std::array::from_fn(|a| {
                        ring.iter().map(|&v| bsp.vertices[v].point[a]).sum::<f32>()
                            / ring.len() as f32
                    });
                    let pl = bsp.planes[usize::from(s.plane & 0x7fff)].0;
                    let sign = if s.plane & 0x8000 != 0 { -1.0 } else { 1.0 };
                    let at =
                        |k: f32| -> [f32; 3] { std::array::from_fn(|a| mid[a] + sign * k * pl[a]) };
                    match locate(src, at(0.01)) {
                        Some(Child::Solid) => out_solid += 1,
                        Some(Child::Leaf(_)) => out_open += 1,
                        _ => {}
                    }
                    match locate(src, at(-0.01)) {
                        Some(Child::Solid) => in_solid += 1,
                        Some(Child::Leaf(_)) => in_open += 1,
                        _ => {}
                    }
                }
                let (mid, half) = h1objects::physics::aabb(
                    &bsp.vertices.iter().map(|v| v.point).collect::<Vec<_>>(),
                );
                for _ in 0..64 {
                    let p: [f32; 3] =
                        std::array::from_fn(|a| mid[a] + half[a] * (rng.next() * 2.0 - 1.0));
                    let solid = matches!(locate(src, p), Some(Child::Solid));
                    if solid == cells.iter().any(|h| inside(h, p)) {
                        agree += 1;
                    } else {
                        disagree += 1;
                    }
                }
            }
        }

        for t in h1.tags() {
            let g = t.group.0.to_be_bytes();
            if !matches!(&g, b"vehi" | b"mach" | b"ctrl" | b"lifi") {
                continue;
            }
            let mut conv = Converter::new(&h1, &base);
            conv.object(t)
                .unwrap_or_else(|e| panic!("{level} {}: {e}", t.name));
            let how = conv
                .tree
                .reports
                .last()
                .unwrap()
                .physics
                .clone()
                .unwrap_or_default();
            let key = how.split(':').next().unwrap_or("").to_owned();
            *built
                .entry(format!("{} {key}", String::from_utf8_lossy(&g)))
                .or_default() += 1;
            let solid = h1::Object::read(&h1, h1.tag_data(t).unwrap())
                .unwrap()
                .collision
                .map(|c| {
                    let c = Collision::read(&h1, h1.tag(c).unwrap()).unwrap();
                    c.nodes.iter().filter_map(|n| n.bsps.first()).any(|b| {
                        !solid_cells(&h1convert::collision::convert(b).unwrap()).is_empty()
                    })
                })
                .unwrap_or(false);
            if &g == b"vehi" && !how.starts_with("borrowed") {
                eprintln!("{level} {}: {how}", t.name);
            }
            if how.starts_with("borrowed") {
                continue;
            }
            assert_eq!(how.starts_with("built"), solid, "{level} {}: {how}", t.name);
            if !solid {
                continue;
            }
            let (_, phmo) = conv
                .tree
                .tags
                .iter()
                .rev()
                .find(|(_, x)| x.group == FourCC(*b"phmo"))
                .unwrap();
            let r = defs::from_built(phmo_def, &phmo.root).unwrap();
            let again = defs::to_built(phmo_def, &r, &mut |_| None).unwrap();
            assert_eq!(again.bytes, phmo.root.bytes, "{}", t.name);
            let block = |p: &str| match r.at(p) {
                Some(Val::Block(b)) => b.clone(),
                _ => Vec::new(),
            };
            let fl = |e: &tags::defs::Record, f: &str| -> Vec<f32> {
                match e.get(f) {
                    Some(Val::List(v)) => v
                        .iter()
                        .map(|x| match x {
                            Val::Float(f) => *f,
                            _ => f32::NAN,
                        })
                        .collect(),
                    _ => Vec::new(),
                }
            };
            let int = |e: &tags::defs::Record, f: &str| match e.get(f) {
                Some(Val::Int(i)) => *i as usize,
                _ => usize::MAX,
            };
            let (fv, pe) = (block("four_vectors"), block("plane_equations"));
            let (mut fv_at, mut pe_at) = (0, 0);
            for p in block("polyhedra") {
                let (count, cap) = (
                    int(&p, "four_vector_count"),
                    int(&p, "four_vector_capacity"),
                );
                let groups: Vec<[Vec<f32>; 3]> = if cap & 0x7fff_ffff <= 3 {
                    (0..count)
                        .map(|g| ["x", "y", "z"].map(|a| fl(&p, &format!("inline_{a}_{g}"))))
                        .collect()
                } else {
                    fv_at += count;
                    fv[fv_at - count..fv_at]
                        .iter()
                        .map(|e| ["x", "y", "z"].map(|a| fl(e, a)))
                        .collect()
                };
                let planes = &pe[pe_at..pe_at + int(&p, "plane_count")];
                pe_at += planes.len();
                for g in &groups {
                    for ((x, y), z) in g[0].iter().zip(&g[1]).zip(&g[2]) {
                        for pl in planes {
                            let n = fl(pl, "normal");
                            let Some(Val::Float(w)) = pl.get("w") else {
                                panic!()
                            };
                            let d = n[0] * x + n[1] * y + n[2] * z + w;
                            assert!(d <= 1e-3, "{}: a corner {d} outside its piece", t.name);
                        }
                    }
                }
            }
            assert_eq!((fv_at, pe_at), (fv.len(), pe.len()), "{}", t.name);
        }
    }
    eprintln!(
        "{cells_total} cells; outside surfaces open {out_open} solid {out_solid}; inside open {in_open} solid {in_solid}; points agree {agree} disagree {disagree}"
    );
    eprintln!("{built:#?}");
    // Not every probe lands as the surface's side says: a part thinner
    // than the probe's step, and open meshes, put some on the far side.
    assert!(
        out_open > 25 * out_solid && in_solid > 25 * in_open,
        "a missing child is solid"
    );
    assert!(agree > 1000 * disagree, "the cells are the solid space");
}
