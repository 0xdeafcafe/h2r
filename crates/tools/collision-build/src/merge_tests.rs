//! Adding a solid to a level (slice 11) on invented shapes: the merged
//! level validates, old segment queries keep their answers away from the
//! solid and hit it inside, added leaves stand in the leaf they replace,
//! and the rebuilt MOPP returns every surface a sampled box touches.

use std::collections::HashSet;

use mopp::geom::{Surface, polygon_overlaps, structure_key};
use mopp::{Aabb, Code, query_aabb};
use tags::collision::{Bsp, Child};

use crate::coll::{BLOCK_NAMES, stage_blocks};
use crate::fixtures::{Brute, Rng, brute, cube, l_room};
use crate::level::{LEAF_SIZE, Level, rebuild_mopp};
use crate::merge::{BOX_MARGIN, Merged, merge};
use crate::query::{locate, ring, vector};
use crate::{Compiled, Error, Options, Polygon, build, verify};

const MATERIALS: usize = 4;

/// A closed box room, open inside `[lo, hi]`: [`cube`]'s faces turned in.
fn box_room(lo: [f32; 3], hi: [f32; 3]) -> Vec<Polygon> {
    cube(lo, hi)
        .into_iter()
        .map(|mut p| {
            p.points.reverse();
            p.material = 1;
            p
        })
        .collect()
}

fn compile(polys: &[Polygon]) -> Compiled {
    let c = build(polys).expect("compiles");
    let problems = verify::check(&c.bsp, MATERIALS);
    assert!(problems.is_empty(), "{problems:#?}");
    c
}

/// The level, the solid, the merged BSP and what the merge added.
struct Case {
    room: Vec<Polygon>,
    level: Compiled,
    solid: Vec<Polygon>,
    piece: Compiled,
    merged: Bsp,
    added: Merged,
}

fn case(room: Vec<Polygon>, solid: Vec<Polygon>) -> Case {
    let level = compile(&room);
    let piece = compile(&solid);
    let mut merged = level.bsp.clone();
    let added = merge(&mut merged, &piece.bsp).expect("merges");
    assert!(added.slots > 0, "the solid reaches the room");
    Case {
        room,
        level,
        solid,
        piece,
        merged,
        added,
    }
}

impl Case {
    /// The input polygon a merged surface came from: the room's first, then
    /// the solid's.
    fn source(&self, s: usize) -> usize {
        if s < self.added.first_surface {
            self.level.surface_source[s]
        } else {
            self.room.len() + self.piece.surface_source[s - self.added.first_surface]
        }
    }

    fn polygons(&self) -> Vec<Polygon> {
        self.room.iter().chain(&self.solid).cloned().collect()
    }

    /// The solid's gate box.
    fn gate(&self) -> ([f32; 3], [f32; 3]) {
        let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        for v in &self.piece.bsp.vertices {
            for a in 0..3 {
                lo[a] = lo[a].min(v.point[a] - BOX_MARGIN);
                hi[a] = hi[a].max(v.point[a] + BOX_MARGIN);
            }
        }
        (lo, hi)
    }
}

/// Every block of `old` is a prefix of the same block of `new`, but the 3D
/// nodes, where only the parents of gated leaves change a child, to a new
/// node.
fn appended(old: &Bsp, new: &Bsp) {
    assert_eq!(new.planes[..old.planes.len()], old.planes[..]);
    assert_eq!(new.leaves[..old.leaves.len()], old.leaves[..]);
    assert_eq!(new.refs[..old.refs.len()], old.refs[..]);
    assert_eq!(new.nodes_2d[..old.nodes_2d.len()], old.nodes_2d[..]);
    assert_eq!(new.surfaces[..old.surfaces.len()], old.surfaces[..]);
    assert_eq!(new.edges[..old.edges.len()], old.edges[..]);
    assert_eq!(new.vertices[..old.vertices.len()], old.vertices[..]);
    for (i, (o, n)) in old.nodes.iter().zip(&new.nodes).enumerate() {
        assert_eq!(o.plane, n.plane, "node {i}");
        for (oc, nc) in [(o.back, n.back), (o.front, n.front)] {
            match nc {
                Child::Node(k) if k as usize >= old.nodes.len() => {
                    assert!(matches!(oc, Child::Leaf(_)), "node {i}: a gate replaces a leaf")
                }
                _ => assert_eq!(oc, nc, "node {i}"),
            }
        }
    }
}

#[test]
fn a_solid_in_a_box_room_validates_and_keeps_every_old_index() {
    let c = case(
        box_room([0.0; 3], [6.0, 6.0, 4.0]),
        cube([1.0, 1.0, 1.0], [2.0, 2.0, 2.0]),
    );
    assert!(c.merged.validate(MATERIALS).is_empty());
    // In one level leaf, so every reference is on its path too.
    let problems = verify::check(&c.merged, MATERIALS);
    assert!(problems.is_empty(), "{problems:#?}");
    appended(&c.level.bsp, &c.merged);
    assert_eq!(c.added.slots, 1);
    assert_eq!(c.added.first_leaf, c.level.bsp.leaves.len());
    assert_eq!(c.added.first_surface, c.level.bsp.surfaces.len());
    assert_eq!(c.added.stand_ins.len(), c.piece.bsp.leaves.len());
    let open = locate(&c.level.bsp, [3.0, 3.0, 2.0]).expect("the room is open");
    assert!(c.added.stand_ins.iter().all(|&l| l == open));
    // Inside the solid is solid now; beside it is an added leaf.
    assert_eq!(locate(&c.merged, [1.5, 1.5, 1.5]), None);
    let beside = locate(&c.merged, [2.02, 1.5, 1.5]).expect("open beside the solid");
    assert!(beside >= c.added.first_leaf);
    assert_eq!(locate(&c.merged, [5.0, 5.0, 3.0]), Some(open));
    // The solid's surfaces keep their material, with no breakable surface.
    for s in &c.merged.surfaces[c.added.first_surface..] {
        assert_eq!((s.material, s.breakable_surface, s.flags & 0x8), (0, 0xff, 0));
    }
}

/// A box floating in the L room across its `y = 2` split, so it stands in
/// more than one of the room's leaves.
fn l_case() -> Case {
    case(l_room(), cube([0.8, 1.2, 1.0], [1.8, 2.8, 2.0]))
}

#[test]
fn a_solid_across_leaves_validates_and_keeps_every_old_index() {
    let c = l_case();
    assert!(c.merged.validate(MATERIALS).is_empty(), "{:#?}", c.merged.validate(MATERIALS));
    appended(&c.level.bsp, &c.merged);
    assert!(c.added.slots >= 1);
    for (hosts, &stand_in) in c.added.hosts.iter().zip(&c.added.stand_ins) {
        assert_eq!(hosts.first(), Some(&stand_in));
    }
}

#[test]
fn old_segments_keep_their_answers_away_from_the_solid_and_hit_it_inside() {
    for c in [
        case(box_room([0.0; 3], [6.0, 6.0, 4.0]), cube([1.0, 1.0, 1.0], [2.0, 2.0, 2.0])),
        l_case(),
    ] {
        let polys = c.polygons();
        let (glo, ghi) = c.gate();
        let (lo, hi) = {
            let mut lo = [f32::INFINITY; 3];
            let mut hi = [f32::NEG_INFINITY; 3];
            for v in &c.level.bsp.vertices {
                for a in 0..3 {
                    lo[a] = lo[a].min(v.point[a] - 0.5);
                    hi[a] = hi[a].max(v.point[a] + 0.5);
                }
            }
            (lo, hi)
        };
        let mut rng = Rng(11);
        let (mut away, mut called, mut solid_hits) = (0, 0, 0);
        for i in 0..4000 {
            let (a, b) = (rng.point(lo, hi), rng.point(lo, hi));
            let new = vector(&c.merged, a, b);
            // Away from the solid: the same answer, leaf and all.
            let misses_gate = (0..3).any(|k| a[k].max(b[k]) < glo[k] || a[k].min(b[k]) > ghi[k]);
            if misses_gate {
                away += 1;
                let old = vector(&c.level.bsp, a, b);
                assert_eq!(old.hit, new.hit, "segment {i} {a:?} -> {b:?}");
            }
            // Everywhere: the first polygon the brute force meets.
            let want = brute(&polys, a, b);
            if want == Brute::Unsure {
                continue;
            }
            called += 1;
            match (want, new.hit) {
                (Brute::Clear, None) => {}
                (Brute::Hit(t, poly), Some(h)) => {
                    assert_eq!(
                        c.source(h.surface),
                        poly,
                        "segment {i} {a:?} -> {b:?}: surface {} against polygon {poly}",
                        h.surface
                    );
                    assert!((f64::from(h.t) - t).abs() < 1e-4, "segment {i}: t {} against {t}", h.t);
                    if h.surface >= c.added.first_surface {
                        solid_hits += 1;
                    }
                }
                (want, got) => panic!("segment {i} {a:?} -> {b:?}: brute force {want:?}, walk {got:?}"),
            }
            assert!(new.missed.is_empty(), "segment {i} {a:?} -> {b:?}: {:?}", new.missed);
        }
        assert!(away > 500, "{away}");
        assert!(called > 2000, "{called}");
        assert!(solid_hits > 50, "{solid_hits}");
    }
}

#[test]
fn a_segment_at_the_solid_from_the_room_hits_its_face() {
    let c = l_case();
    // From the open room towards the box's +x face (x = 1.8).
    let h = vector(&c.merged, [3.5, 1.5, 1.5], [1.3, 1.5, 1.5]).hit.expect("hits");
    assert!(h.surface >= c.added.first_surface);
    let r = ring(&c.merged, h.surface);
    assert!(r.iter().all(|p| (p[0] - 1.8).abs() < 1e-5), "{r:?}");
    // From inside the box out to the room's -x wall (x = 0).
    let h = vector(&c.merged, [1.3, 1.5, 1.5], [-0.5, 1.5, 1.5]).hit.expect("hits");
    assert!(h.surface < c.added.first_surface);
    assert!(ring(&c.merged, h.surface).iter().all(|p| p[0].abs() < 1e-5));
}

#[test]
fn added_leaves_stand_in_the_leaf_whose_space_they_take() {
    let c = l_case();
    let (glo, ghi) = c.gate();
    let mut rng = Rng(5);
    let mut seen = 0;
    for _ in 0..4000 {
        let p = rng.point(glo, ghi);
        let Some(l) = locate(&c.merged, p) else {
            continue;
        };
        if l < c.added.first_leaf {
            continue;
        }
        seen += 1;
        let old = locate(&c.level.bsp, p).expect("an added leaf is in the level's open space");
        assert!(c.added.hosts[l - c.added.first_leaf].contains(&old), "{p:?}: leaf {l} over {old}");
    }
    assert!(seen > 500, "{seen}");
}

/// The per-leaf block of `bsp` with a distinct element per leaf: the leaf
/// index as the `i16` cluster, then six bytes that differ per leaf.
fn leaf_block(bsp: &Bsp) -> Vec<u8> {
    (0..bsp.leaves.len())
        .flat_map(|k| {
            let mut e = (k as i16).to_le_bytes().to_vec();
            e.extend((0..6).map(|i| (k * 7 + i) as u8 ^ 0x5a));
            e
        })
        .collect()
}

fn level_of(room: &[Polygon], mopp: Vec<u8>) -> Level {
    let bsp = compile(room).bsp;
    Level {
        leaves: leaf_block(&bsp),
        bsp,
        mopp,
        materials: MATERIALS,
    }
}

#[test]
fn added_leaves_take_their_stand_ins_element_and_cluster() {
    let level = level_of(&l_room(), Vec::new());
    let added = level
        .add_solid(&cube([0.8, 1.2, 1.0], [1.8, 2.8, 2.0]), &Options::default())
        .unwrap();
    let new = &added.level;
    assert_eq!(new.leaves.len(), new.bsp.leaves.len() * LEAF_SIZE);
    assert_eq!(new.leaves[..level.leaves.len()], level.leaves[..]);
    for (i, &host) in added.merged.stand_ins.iter().enumerate() {
        let at = (added.merged.first_leaf + i) * LEAF_SIZE;
        let element = &new.leaves[at..at + LEAF_SIZE];
        assert_eq!(element, &level.leaves[host * LEAF_SIZE..(host + 1) * LEAF_SIZE]);
        assert_eq!(i16::from_le_bytes([element[0], element[1]]), host as i16);
    }
    // The BSP is the merge's.
    let mut bsp = level.bsp.clone();
    merge(&mut bsp, &added.piece.bsp).unwrap();
    assert_eq!(bsp, new.bsp);
}

/// Boxes around the merged level's vertices, from 2 cm to a couple of
/// units across, some spanning the room.
fn sample_boxes(bsp: &Bsp, n: usize, seed: u64) -> Vec<Aabb> {
    let mut rng = Rng(seed);
    (0..n)
        .map(|i| {
            let v = bsp.vertices[(rng.next() % bsp.vertices.len() as u64) as usize].point;
            let half = if i % 16 == 0 { 2.0 + 3.0 * rng.range(0.0, 1.0) } else { 0.02 * 100f32.powf(rng.range(0.0, 1.0)) };
            let c: [f32; 3] = std::array::from_fn(|a| v[a] + rng.range(-1.0, 1.0) * half);
            let h: [f32; 3] = std::array::from_fn(|_| half * rng.range(0.25, 1.25));
            Aabb {
                min: std::array::from_fn(|a| c[a] - h[a]),
                max: std::array::from_fn(|a| c[a] + h[a]),
            }
        })
        .collect()
}

#[test]
fn the_rebuilt_mopp_returns_every_touching_surface_and_every_other_key() {
    // The old program: the room's surfaces and one instanced-geometry key
    // (key type 2) over a box in the room.
    let room = l_room();
    let old_bsp = compile(&room).bsp;
    let other_key = (2u32 << 29) | 7;
    let mut inputs = mopp::geom::structure_surfaces(&old_bsp);
    inputs.push(Surface {
        key: other_key,
        vertices: vec![[3.0, 0.5, 0.5], [3.5, 1.0, 2.0]],
    });
    let old_block = mopp::build(&inputs).to_block();
    let level = level_of(&room, old_block.clone());
    let added = level
        .add_solid(&cube([0.8, 1.2, 1.0], [1.8, 2.8, 2.0]), &Options::default())
        .unwrap();
    let new = &added.level;
    assert_eq!(new.mopp, rebuild_mopp(&new.bsp, &old_block).unwrap());
    assert_eq!(new.mopp.len() % 16, 0);
    let ours = Code::parse(&new.mopp).unwrap();
    mopp::check::validate(&ours.bytecode).unwrap();
    let theirs = Code::parse(&old_block).unwrap();
    let rings: Vec<Vec<[f32; 3]>> = (0..new.bsp.surfaces.len()).map(|s| ring(&new.bsp, s)).collect();
    let (mut truth, mut solid, mut other) = (0, 0, 0);
    for b in sample_boxes(&new.bsp, 1500, 9) {
        let got: HashSet<u32> = query_aabb(&ours, &b).unwrap().into_iter().collect();
        for (s, r) in rings.iter().enumerate() {
            if polygon_overlaps(r, &b) {
                truth += 1;
                solid += usize::from(s >= added.merged.first_surface);
                assert!(got.contains(&structure_key(s)), "surface {s} touches {b:?}");
            }
        }
        if query_aabb(&theirs, &b).unwrap().contains(&other_key) {
            other += 1;
            assert!(got.contains(&other_key), "the instanced key at {b:?}");
        }
    }
    assert!(truth > 1000 && solid > 200 && other > 0, "{truth} {solid} {other}");
}

#[test]
fn a_level_with_no_mopp_gets_one_over_its_surfaces() {
    let level = level_of(&box_room([0.0; 3], [6.0, 6.0, 4.0]), Vec::new());
    let added = level
        .add_solid(&cube([1.0; 3], [2.0; 3]), &Options::default())
        .unwrap();
    let code = Code::parse(&added.level.mopp).unwrap();
    let all = query_aabb(&code, &Aabb { min: [-1.0; 3], max: [7.0, 7.0, 5.0] }).unwrap();
    let keys: HashSet<u32> = all.into_iter().collect();
    assert_eq!(keys, (0..added.level.bsp.surfaces.len()).map(structure_key).collect::<HashSet<u32>>());
}

#[test]
fn the_stage_is_the_eight_blocks_the_leaf_block_and_the_mopp() {
    let level = level_of(&l_room(), Vec::new());
    let added = level
        .add_solid(&cube([0.8, 1.2, 1.0], [1.8, 2.8, 2.0]), &Options::default())
        .unwrap();
    let stage = added.stage();
    assert_eq!(stage.blocks, stage_blocks(&added.level.bsp));
    assert_eq!(stage.blocks.iter().map(|b| b.name).collect::<Vec<_>>(), BLOCK_NAMES);
    assert_eq!(stage.blocks[2].count as usize, added.level.bsp.leaves.len());
    assert_eq!(stage.leaves.count as usize, added.level.bsp.leaves.len());
    assert_eq!(stage.leaves.bytes, added.level.leaves);
    assert_eq!(stage.mopp, added.level.mopp);
    // The blocks decode back to the merged BSP.
    let bytes = added.level.bsp.encode_blocks(0x1000);
    assert_eq!(Bsp::decode_blocks(&bytes, 0x1000, 0).unwrap(), added.level.bsp);
}

#[test]
fn what_cannot_be_added_is_refused_and_changes_nothing() {
    let level = level_of(&box_room([0.0; 3], [6.0, 6.0, 4.0]), Vec::new());
    let opts = Options::default();
    // A material past the structure's list.
    let mut far = cube([1.0; 3], [2.0; 3]);
    far[3].material = MATERIALS as u16;
    assert!(matches!(level.add_solid(&far, &opts), Err(Error::Material { surface: _, material: 4, materials: 4 })));
    // A solid wholly in the level's solid, outside the room.
    let outside = cube([10.0; 3], [11.0; 3]);
    let e = level.add_solid(&outside, &opts).unwrap_err();
    assert!(e.to_string().contains("no open space"), "{e}");
    let mut bsp = level.bsp.clone();
    assert_eq!(merge(&mut bsp, &compile(&outside).bsp).unwrap(), Merged::default());
    assert_eq!(bsp, level.bsp);
    // A per-leaf block that does not match the leaves.
    let mut short = level.clone();
    short.leaves.pop();
    assert!(short.add_solid(&cube([1.0; 3], [2.0; 3]), &opts).is_err());
    // A program that does not parse is named, not dropped.
    let mut broken = level.clone();
    broken.mopp = vec![0; 8];
    let e = broken.add_solid(&cube([1.0; 3], [2.0; 3]), &opts).unwrap_err();
    assert!(e.to_string().contains("MOPP"), "{e}");
}

#[test]
fn a_second_solid_merges_after_the_first() {
    let level = level_of(&box_room([0.0; 3], [6.0, 6.0, 4.0]), Vec::new());
    let opts = Options::default();
    let one = level.add_solid(&cube([1.0; 3], [2.0; 3]), &opts).unwrap();
    let two = one.level.add_solid(&cube([4.0, 4.0, 1.0], [5.0, 5.0, 2.0]), &opts).unwrap();
    appended(&one.level.bsp, &two.level.bsp);
    assert!(two.level.bsp.validate(MATERIALS).is_empty());
    assert_eq!(locate(&two.level.bsp, [1.5; 3]), None);
    assert_eq!(locate(&two.level.bsp, [4.5, 4.5, 1.5]), None);
    assert!(locate(&two.level.bsp, [3.0, 3.0, 3.0]).is_some());
}
