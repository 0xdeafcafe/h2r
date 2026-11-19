//! Real-data proof that placed Halo 1 scenery is solid as structure
//! (`h1convert::scenery_collision`): the engine gives scenery no Havok
//! body, so each placed object's collision is merged into the converted
//! level's. Reads `$H1_ISO` and `$H2_DISC`; prints names and counts only.
//! Nothing is written.
//!
//! ```sh
//! H1_ISO=... H2_DISC=... cargo test --release -p h1objects --test scenery_collision_disc -- --ignored --nocapture
//! ```
//!
//! - `bloodgulch_boulder_doublewide_is_level_collision`: a point inside
//!   `boulder_doublewide` that Halo 1's level alone leaves open is solid
//!   in the converted level; a ray from above stops on the rock's top,
//!   not the ground under it; the structure MOPP code returns that
//!   surface there; around the rock (in its box, off it) the ground is
//!   found as before; away from every placed object every vertical ray
//!   meets the level exactly as the level alone does, and the merged BSP
//!   validates with the level's blocks first and unchanged.
//! - `every_multiplayer_map_merges_its_placed_scenery`: every multiplayer
//!   map on its base, Halo 1 sandbox: every placement's collision merges
//!   (none refused), and the BSP validates and keys its MOPP code.

use std::fs::File;
use std::io::{BufReader, Cursor};

use h1convert::convert::halo1::{Halo1Objects, prepare_halo1};
use h1convert::convert::{MAPS, structure_mopp};
use h1convert::scenery_collision::{Pose, transformed};
use h1convert::surface_query::{Hit, Outcome, locate, vector};
use h1map::H1Map;
use h1objects::base::Base;
use h1objects::sandbox::Objects;
use map::Map;
use tags::collision::Bsp;

fn h1_map(name: &str) -> Option<H1Map> {
    let path = std::env::var("H1_ISO").expect("set H1_ISO");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(path).unwrap())).unwrap();
    let disc = h1map::disc::Disc::open(&mut iso).unwrap();
    let e = disc.map(name)?.clone();
    Some(H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap())
}

fn h2_map(name: &str) -> Map<Cursor<Vec<u8>>> {
    let path = std::env::var("H2_DISC").expect("set H2_DISC");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(path).unwrap())).unwrap();
    let e = iso
        .entries()
        .unwrap()
        .into_iter()
        .find(|e| e.path.rsplit(['/', '\\']).next() == Some(&format!("{name}.map")))
        .unwrap();
    Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap()
}

/// Halo 1's level collision alone, converted (what the level was before
/// scenery joined it).
fn level_only(h1: &H1Map) -> Bsp {
    let refs = h1.structure_bsps().unwrap();
    let src = h1.structure_bsp(&refs[0]).unwrap();
    let col = src.collision_bsps().unwrap().into_iter().next().unwrap();
    h1convert::collision::convert(&col).unwrap()
}

fn bounds(b: &Bsp) -> ([f32; 3], [f32; 3]) {
    let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for v in &b.vertices {
        for a in 0..3 {
            lo[a] = lo[a].min(v.point[a]);
            hi[a] = hi[a].max(v.point[a]);
        }
    }
    (lo, hi)
}

fn first_hit(b: &Bsp, from: [f32; 3], to: [f32; 3]) -> Option<Hit> {
    match vector(b, from, to).outcome {
        Outcome::Hit(h) => Some(h),
        Outcome::Clear => None,
    }
}

/// Every placed scenery object there from the start with collision: its
/// name and its BSPs where it stands.
fn placed_collision(
    objects: &mut Objects<'_>,
    scenery: &h1convert::convert::halo1::Scenery,
) -> Vec<(String, Vec<Bsp>)> {
    let mut out = Vec::new();
    for s in scenery.placed.iter().filter(|s| s.at_start) {
        let bsps = objects.collision(*b"scen", &s.name).unwrap();
        if bsps.is_empty() {
            continue;
        }
        let pose = Pose::placement(s.position, s.rotation);
        out.push((
            s.name.clone(),
            bsps.iter()
                .map(|b| transformed(b, &pose).unwrap())
                .collect(),
        ));
    }
    out
}

/// The level's own blocks come first in the merged BSP, unchanged.
fn keeps_the_level(before: &Bsp, after: &Bsp) {
    assert_eq!(after.planes[..before.planes.len()], before.planes[..]);
    assert_eq!(after.surfaces[..before.surfaces.len()], before.surfaces[..]);
    assert_eq!(after.edges[..before.edges.len()], before.edges[..]);
    assert_eq!(after.vertices[..before.vertices.len()], before.vertices[..]);
    assert_eq!(after.nodes_2d[..before.nodes_2d.len()], before.nodes_2d[..]);
    assert_eq!(after.refs[..before.refs.len()], before.refs[..]);
    assert_eq!(after.leaves[..before.leaves.len()], before.leaves[..]);
}

/// What the merged BSP's validator finds, less the level's own surfaces
/// with no material (`0xffff`, which retail Halo 2 structure BSPs hold too,
/// `h1convert/tests/h2_sbsp.rs`; the object validator doesn't allow it);
/// and every surface the merge added names material 0.
fn problems(before: &Bsp, after: &Bsp, materials: usize) -> Vec<String> {
    let n = before.surfaces.len();
    assert!(
        after.surfaces[n..].iter().all(|s| s.material == 0),
        "merged surfaces take material 0"
    );
    after
        .validate(materials)
        .into_iter()
        .filter(|p| {
            let level_none = p
                .strip_prefix("surface ")
                .and_then(|r| r.split_once(':'))
                .and_then(|(i, rest)| Some((i.parse::<usize>().ok()?, rest)))
                .is_some_and(|(i, rest)| {
                    i < n
                        && rest == " material 65535 out of range"
                        && before.surfaces[i].material == u16::MAX
                });
            !level_none
        })
        .collect()
}

/// Growth of the merged BSP over the level's: node count and encoded bytes
/// (what the structure's tag space grows by, MOPP code aside).
fn growth(before: &Bsp, after: &Bsp) -> (f64, usize, usize) {
    let (a, b) = (before.encode_blocks(0).len(), after.encode_blocks(0).len());
    (after.nodes.len() as f64 / before.nodes.len() as f64, a, b)
}

fn sizes(b: &Bsp) -> String {
    format!(
        "{} nodes, {} planes (of 32,768), {} leaves, {} references (of 65,535), {} 2D nodes (of 32,768), {} surfaces (of 32,767), {} edges, {} vertices",
        b.nodes.len(),
        b.planes.len(),
        b.leaves.len(),
        b.refs.len(),
        b.nodes_2d.len(),
        b.surfaces.len(),
        b.edges.len(),
        b.vertices.len()
    )
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn bloodgulch_boulder_doublewide_is_level_collision() {
    let h1 = h1_map("bloodgulch").expect("bloodgulch on the disc");
    let mut base_map = h2_map("coagulation");
    let templates = Base::read(&mut base_map).unwrap();
    let mut objects = Objects::new(&h1, &templates);
    let p = prepare_halo1(
        &h1,
        content::Sha256::of(b"bloodgulch"),
        &mut base_map,
        &mut objects,
    )
    .unwrap();
    let before = level_only(&h1);
    let after = &p.collision;
    let scenery = p.scenery.as_ref().expect("scenery planned");
    eprintln!(
        "{} scenery placed, {} merged into the level; before: {}; after: {}",
        scenery.placed.len(),
        scenery.solid,
        sizes(&before),
        sizes(after)
    );
    let refused: Vec<_> = p
        .applied
        .unresolved
        .iter()
        .filter(|u| u.contains("collision not"))
        .collect();
    assert!(refused.is_empty(), "{refused:#?}");
    assert_eq!(
        problems(&before, after, p.collision_materials),
        Vec::<String>::new()
    );
    keeps_the_level(&before, after);
    let (ratio, bytes_before, bytes_after) = growth(&before, after);
    eprintln!(
        "nodes x{ratio:.2}; collision BSP {bytes_before} bytes, merged {bytes_after} (+{})",
        bytes_after - bytes_before
    );
    assert!(ratio < 2.0, "the merge is proportionate: nodes x{ratio:.2}");
    assert_eq!(p.sound_leaves.len(), after.leaves.len());

    let placed = placed_collision(&mut objects, scenery);
    assert!(scenery.solid > 0 && scenery.solid <= placed.len());
    let (name, rock) = placed
        .iter()
        .find(|(n, _)| n.ends_with("boulder_doublewide"))
        .expect("a boulder_doublewide placement");
    let (lo, hi) = bounds(&rock[0]);
    eprintln!("{name}: box {lo:?} to {hi:?}");

    // A point inside the rock that the level alone leaves open: the
    // highest such point on a grid over the rock's box.
    let mut inside = None;
    for i in 1..16 {
        for j in 1..16 {
            for k in 1..16 {
                let p = [
                    lo[0] + (hi[0] - lo[0]) * i as f32 / 16.0,
                    lo[1] + (hi[1] - lo[1]) * j as f32 / 16.0,
                    lo[2] + (hi[2] - lo[2]) * k as f32 / 16.0,
                ];
                let in_rock = rock.iter().any(|b| locate(b, p).is_none());
                if in_rock
                    && locate(&before, p).is_some()
                    && inside.is_none_or(|q: [f32; 3]| p[2] > q[2])
                {
                    inside = Some(p);
                }
            }
        }
    }
    let inside = inside.expect("a point of the rock above the ground");
    eprintln!("inside {name}: {inside:?}");
    assert!(locate(&before, inside).is_some(), "open in the level alone");
    assert_eq!(locate(after, inside), None, "solid once the rock is merged");

    // From above, the rock's top, not the ground under it.
    let from = [inside[0], inside[1], hi[2] + 2.0];
    let to = [inside[0], inside[1], lo[2] - 10.0];
    let ground = first_hit(&before, from, to).expect("the ground under the rock");
    let top = first_hit(after, from, to).expect("a hit from above");
    let own = rock
        .iter()
        .filter_map(|b| first_hit(b, from, to))
        .map(|h| h.t)
        .fold(f32::INFINITY, f32::min);
    eprintln!(
        "from above: ground at t {}, rock alone {own}, merged {top:?}",
        ground.t
    );
    assert!(
        top.surface >= before.surfaces.len(),
        "a rock surface: {top:?}"
    );
    assert!(top.t < ground.t, "above the ground");
    assert!((top.t - own).abs() < 1e-3, "where the rock alone is hit");

    // Havok's view: the structure MOPP code returns that surface there.
    let code = mopp::Code::parse(&structure_mopp(after).unwrap()).unwrap();
    let at: [f32; 3] = std::array::from_fn(|k| from[k] + (to[k] - from[k]) * top.t);
    let keys = mopp::query_aabb(
        &code,
        &mopp::Aabb {
            min: at.map(|v| v - 0.1),
            max: at.map(|v| v + 0.1),
        },
    )
    .unwrap();
    assert!(
        keys.contains(&mopp::geom::structure_key(top.surface)),
        "MOPP keys at {at:?}: {keys:x?}"
    );

    // Around the rock, in its box but off it: the ground as before.
    let mut around = 0;
    for i in 0..=12 {
        for j in 0..=12 {
            let (x, y) = (
                lo[0] - 0.2 + (hi[0] - lo[0] + 0.4) * i as f32 / 12.0,
                lo[1] - 0.2 + (hi[1] - lo[1] + 0.4) * j as f32 / 12.0,
            );
            let (from, to) = ([x, y, hi[2] + 2.0], [x, y, lo[2] - 10.0]);
            let near_rock = placed
                .iter()
                .flat_map(|(_, b)| b)
                .any(|b| first_hit(b, from, to).is_some() || locate(b, to).is_none());
            if near_rock {
                continue;
            }
            let (Some(a), Some(b)) = (first_hit(after, from, to), first_hit(&before, from, to))
            else {
                panic!("the ground at ({x}, {y})");
            };
            assert_eq!(a.surface, b.surface, "the ground at ({x}, {y})");
            assert!((a.t - b.t).abs() < 1e-5);
            around += 1;
        }
    }
    eprintln!("{around} rays beside {name} find the ground as before");
    assert!(around > 0);

    // Away from every placed object: every vertical ray as before.
    let boxes: Vec<_> = placed
        .iter()
        .flat_map(|(_, b)| b)
        .map(|b| {
            let (lo, hi) = bounds(b);
            (lo.map(|v| v - 0.5), hi.map(|v| v + 0.5))
        })
        .collect();
    let [x0, x1, y0, y1, z0, z1] = p.bounds;
    let mut same = 0;
    for i in 0..=120 {
        for j in 0..=120 {
            let x = x0 + (x1 - x0) * i as f32 / 120.0;
            let y = y0 + (y1 - y0) * j as f32 / 120.0;
            if boxes
                .iter()
                .any(|(lo, hi)| (lo[0]..=hi[0]).contains(&x) && (lo[1]..=hi[1]).contains(&y))
            {
                continue;
            }
            let (from, to) = ([x, y, z1 + 1.0], [x, y, z0 - 1.0]);
            assert_eq!(
                vector(after, from, to),
                vector(&before, from, to),
                "({x}, {y})"
            );
            same += 1;
        }
    }
    eprintln!("{same} vertical rays away from the scenery meet the level as before");
    assert!(same > 1000);
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC); converts every multiplayer map"]
fn every_multiplayer_map_merges_its_placed_scenery() {
    let mut maps = 0;
    for (name, _, base_name) in MAPS {
        let Some(h1) = h1_map(name) else {
            eprintln!("{name}: not on the disc");
            continue;
        };
        let mut base_map = h2_map(base_name);
        let templates = Base::read(&mut base_map).unwrap();
        let mut objects = Objects::new(&h1, &templates);
        let p = prepare_halo1(
            &h1,
            content::Sha256::of(name.as_bytes()),
            &mut base_map,
            &mut objects,
        )
        .unwrap_or_else(|e| panic!("{name}: {e}"));
        let before = level_only(&h1);
        let after = &p.collision;
        let scenery = p.scenery.as_ref().expect("scenery planned");
        let with = placed_collision(&mut objects, scenery).len();
        eprintln!(
            "{name}: {} scenery placed, {with} with collision, {} merged; {}; nodes x{:.2}, collision BSP {} to {} bytes",
            scenery.placed.len(),
            scenery.solid,
            sizes(after),
            growth(&before, after).0,
            growth(&before, after).1,
            growth(&before, after).2
        );
        let refused: Vec<_> = p
            .applied
            .unresolved
            .iter()
            .filter(|u| u.contains("collision not"))
            .collect();
        assert!(refused.is_empty(), "{name}: {refused:#?}");
        assert_eq!(
            problems(&before, after, p.collision_materials),
            Vec::<String>::new(),
            "{name}"
        );
        keeps_the_level(&before, after);
        assert_eq!(p.sound_leaves.len(), after.leaves.len(), "{name}");
        let code = mopp::Code::parse(&structure_mopp(after).unwrap()).unwrap();
        let keys = mopp::check::validate(&code.bytecode)
            .unwrap_or_else(|e| panic!("{name}: MOPP code {e:?}"))
            .keys;
        // Every surface keyed, the rocks' among them.
        let mut keys: Vec<_> = keys.into_iter().map(|k| k & 0xffff).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), after.surfaces.len(), "{name}: MOPP keys");
        maps += 1;
    }
    assert!(maps > 0);
}
