//! The source parses into the rig, stays in the triangle budget and has
//! its markers; the atlas is the one the painter paints; the tags built
//! from the source read back as the source, the same way every time; a map
//! without the room, with the kit already, or of another kind is refused.
//! The `#[ignore]` test reads the Halo 2 disc (`$H2_DISC`).

use std::io::Cursor;

use map::build::{Block, TagPayload, TagSet};
use map::{CacheType, FourCC, Map, RawPointer, ResourceFile, TagId};
use crate::fixture::fixture;
use crate::mesh;
use tags::model::RenderModelGeometry;

use super::inject::{self, add_flood_pod, add_flood_pod_within};
use super::kit::{self, GROUPS};
use super::source::{self, max_triangles};
use super::{
    KIT_VERSION, MARKER_IMPACT, NODE_SHELL, PETALS, RIG, TAG_PATH, petal_node, squad_exit_marker,
};

/// The hash `scripts/flood-pod-atlas.py` prints: FNV-1a, 64 bits, over the
/// atlas's RGBA bytes row by row.
const ATLAS_PIXELS: u64 = 0x9aa3_6f70_e25c_c63e;

fn fnv64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn close(a: [f32; 3], b: [f32; 3], eps: f32) -> bool {
    (0..3).all(|k| (a[k] - b[k]).abs() <= eps)
}

fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

/// Whether a tag's bytes hold `datum` at a word boundary.
fn holds(bytes: &[u8], datum: TagId) -> bool {
    bytes.chunks_exact(4).any(|w| w == datum.0.to_le_bytes())
}

struct Raw(Vec<u8>);
impl tags::geometry::RawSource for Raw {
    fn read_raw(&mut self, p: RawPointer, size: u32) -> tags::Result<Vec<u8>> {
        assert_eq!(p.file, ResourceFile::Local);
        Ok(self.0[p.offset as usize..(p.offset + size) as usize].to_vec())
    }
}

/// The built-in source's text with `from` replaced by `to`, which it must
/// hold.
fn edited(from: &str, to: &str) -> String {
    let text = source::text();
    assert!(text.contains(from), "the source has no {from:?}");
    text.replacen(from, to, 1)
}

#[test]
fn the_rig_is_four_petals_a_quarter_turn_apart() {
    assert!(close(RIG.hinge(0), [0.345, 0.0, 0.54], 1e-6));
    assert!(close(RIG.hinge(1), [0.0, 0.345, 0.54], 1e-6));
    assert!(close(RIG.hinge(2), [-0.345, 0.0, 0.54], 1e-6));
    assert!(close(RIG.hinge(3), [0.0, -0.345, 0.54], 1e-6));
    for k in 0..PETALS {
        let (d, axis) = (RIG.petal_direction(k), RIG.hinge_axis(k));
        assert!(mesh::dot(d, axis).abs() < 1e-6, "petal {k}");
        // A right-handed turn about the axis takes up to outward.
        assert!(
            close(mesh::cross(axis, [0.0, 0.0, 1.0]), d, 1e-6),
            "petal {k}"
        );
        let exit = RIG.squad_exit(k);
        assert_eq!(exit[2], 0.0);
        assert!((mesh::length(exit) - RIG.exit_radius).abs() < 1e-6);
        // Between this petal and the next.
        let next = RIG.petal_direction((k + 1) % PETALS);
        assert!(mesh::dot(exit, d) > 0.0 && mesh::dot(exit, next) > 0.0);
    }
    assert!(RIG.open > std::f32::consts::FRAC_PI_2 && RIG.open < std::f32::consts::PI);
}

#[test]
fn the_petal_nodes_are_parented_to_the_shell() {
    let s = source::builtin().unwrap();
    assert_eq!(
        (s.name.as_str(), s.atlas.as_str()),
        ("flood_pod", "flood_pod")
    );
    assert_eq!(s.nodes.len(), 1 + PETALS);
    assert_eq!(s.nodes[0].name, NODE_SHELL);
    assert_eq!(s.nodes[0].parent, None);
    assert_eq!(s.nodes[0].at, [0.0; 3]);
    for k in 0..PETALS {
        let n = &s.nodes[1 + k];
        assert_eq!(n.name, petal_node(k));
        assert_eq!(n.parent.as_deref(), Some(NODE_SHELL));
        assert!(close(n.at, RIG.hinge(k), 1e-3), "{}: {:?}", n.name, n.at);
    }

    let mesh = source::build(&s).unwrap();
    let model = kit::model(&s, &mesh).unwrap();
    let links: Vec<_> = model
        .nodes
        .iter()
        .map(|n| (n.parent, n.first_child, n.next_sibling))
        .collect();
    assert_eq!(
        links,
        [(-1, 1, -1), (0, -1, 2), (0, -1, 3), (0, -1, 4), (0, -1, -1)]
    );
    for k in 0..PETALS {
        assert!(close(model.nodes[1 + k].translation, RIG.hinge(k), 1e-3));
    }
}

#[test]
fn the_markers_are_present() {
    let s = source::builtin().unwrap();
    assert_eq!(s.markers.len(), 1 + PETALS);
    assert_eq!(s.marker(MARKER_IMPACT), Some([0.0; 3]));
    for k in 0..PETALS {
        let name = squad_exit_marker(k);
        let at = s
            .marker(&name)
            .unwrap_or_else(|| panic!("no marker {name}"));
        assert!(close(at, RIG.squad_exit(k), 1e-3), "{name}: {at:?}");
    }
    assert!(s.markers.iter().all(|m| m.node == NODE_SHELL));

    // The exits are on the ground, clear of the shut pod's body: no
    // triangle of the root, the collar, the sac or a petal is that far out.
    let mesh = source::build(&s).unwrap();
    for (part, built) in s.parts.iter().zip(&mesh.parts) {
        if part.region() == "tendril" {
            continue;
        }
        for v in &built.vertices {
            let out = v.position[0].hypot(v.position[1]);
            assert!(
                out < RIG.exit_radius - 0.2,
                "{out} out in {}",
                part.region()
            );
        }
    }
}

#[test]
fn a_source_off_the_rig_is_refused() {
    let e = |text: String| format!("{:#}", source::parse(&text).unwrap_err());
    let orphan = e(edited(
        "{ name: petal_2, parent: shell, at: [-0.345, 0, 0.54] }",
        "{ name: petal_2, at: [-0.345, 0, 0.54] }",
    ));
    assert!(orphan.contains("petal_2, a child of shell"), "{orphan}");
    let chained = e(edited(
        "{ name: petal_1, parent: shell,",
        "{ name: petal_1, parent: petal_0,",
    ));
    assert!(chained.contains("petal_1, a child of shell"), "{chained}");
    let moved = e(edited("at: [0, 0.345, 0.54] }", "at: [0, 0.4, 0.54] }"));
    assert!(moved.contains("its hinge is at"), "{moved}");
    let no_impact = e(edited("name: impact,", "name: landing,"));
    assert!(no_impact.contains("0 markers named impact"), "{no_impact}");
    let no_exit = e(edited(
        "  - { name: squad_exit_3, node: shell, at: [0.53, -0.53, 0] }\n",
        "",
    ));
    assert!(
        no_exit.contains("0 markers named squad_exit_3"),
        "{no_exit}"
    );
    let on_petal = e(edited(
        "{ name: squad_exit_0, node: shell,",
        "{ name: squad_exit_0, node: petal_0,",
    ));
    assert!(
        on_petal.contains("marker squad_exit_0 is on petal_0"),
        "{on_petal}"
    );
    let region = e(edited("region: flesh", "region: body"));
    assert!(
        region.contains("region \"body\", which the atlas lacks"),
        "{region}"
    );
    let unknown = e(edited("name: flood_pod\n", "name: flood_pod\nscale: 2\n"));
    assert!(unknown.contains("the pod's source"), "{unknown}");
}

#[test]
fn the_pod_is_in_the_triangle_budget() {
    let s = source::builtin().unwrap();
    let m = source::build(&s).unwrap();
    assert!(
        m.triangles() <= max_triangles(),
        "{} triangles",
        m.triangles()
    );
    assert!(m.triangles() >= 1000, "{} triangles", m.triangles());
    assert!(m.vertices() <= usize::from(u16::MAX));

    // One triangle over is refused by count.
    let mut over = s.clone();
    while mesh::build_on(&over, &source::region).unwrap().triangles() <= max_triangles() {
        over.parts.extend(s.parts.iter().cloned());
    }
    let e = format!("{:#}", source::build(&over).unwrap_err());
    assert!(e.contains("a pod has 2000 at most"), "{e}");
}

#[test]
fn the_pod_stands_on_its_tip_and_each_petal_is_on_its_side() {
    let s = source::builtin().unwrap();
    let m = source::build(&s).unwrap();
    let (lo, hi) = m.bounds();
    assert!(lo[2] >= -1e-4 && lo[2] < 0.01, "the tip is at {}", lo[2]);
    assert!(hi[2] > 1.0 && hi[2] < 1.4, "the top is at {}", hi[2]);
    for a in 0..2 {
        assert!(lo[a] > -1.0 && hi[a] < 1.0, "{lo:?}..{hi:?}");
    }
    // Everything is inside the object's bounding sphere, shut.
    let mut petals = [0usize; PETALS];
    for p in &m.parts {
        for v in &p.vertices {
            let from = mesh::length(mesh::sub(v.position, kit::BOUNDING_OFFSET));
            assert!(from < kit::BOUNDING_RADIUS, "{:?}", v.position);
        }
        // Node 0 is the shell; node 1 + k is petal k.
        let Some(k) = p.node.checked_sub(1) else {
            continue;
        };
        petals[k] += 1;
        let d = RIG.petal_direction(k);
        for v in &p.vertices {
            assert!(
                mesh::dot(v.position, d) > -0.02 && v.position[2] > RIG.hinge_height - 0.06,
                "petal {k}: {:?}",
                v.position
            );
        }
    }
    assert!(
        petals.iter().all(|n| *n == petals[0] && *n > 0),
        "{petals:?}"
    );
}

#[test]
fn every_region_is_drawn_and_inside_the_atlas() {
    let s = source::builtin().unwrap();
    for (name, [x0, y0, x1, y1]) in source::regions() {
        assert!(
            x0 < x1 && y0 < y1 && x1 <= source::atlas_size().0 && y1 <= source::atlas_size().1,
            "{name}"
        );
        let [u0, v0, u1, v1] = source::region(name).unwrap();
        assert!(
            0.0 < u0 && u0 < u1 && u1 < 1.0 && 0.0 < v0 && v0 < v1 && v1 < 1.0,
            "{name}"
        );
        assert!(
            s.parts.iter().any(|p| p.region() == name),
            "no part in {name}"
        );
    }
    // No two regions share a texel.
    let regions = source::regions();
    for (i, (a, ra)) in regions.iter().enumerate() {
        for (b, rb) in &regions[i + 1..] {
            let apart = ra[2] <= rb[0] || rb[2] <= ra[0] || ra[3] <= rb[1] || rb[3] <= ra[1];
            assert!(apart, "{a} and {b} overlap");
        }
    }
    let m = source::build(&s).unwrap();
    for (part, built) in s.parts.iter().zip(&m.parts) {
        let [u0, v0, u1, v1] = source::region(part.region()).unwrap();
        for v in &built.vertices {
            assert!(
                (u0 - 1e-5..=u1 + 1e-5).contains(&v.uv[0])
                    && (v0 - 1e-5..=v1 + 1e-5).contains(&v.uv[1]),
                "{}: {:?}",
                part.region(),
                v.uv
            );
        }
    }
}

#[test]
fn the_atlas_is_the_one_the_painter_paints() {
    let a = source::atlas().unwrap();
    assert_eq!(
        (a.width, a.height),
        (u32::from(source::atlas_size().0), u32::from(source::atlas_size().1))
    );
    assert!(a.pixels.chunks_exact(4).all(|p| p[3] == 255), "opaque");
    assert_eq!(
        fnv64(&a.pixels),
        ATLAS_PIXELS,
        "flood_pod.png is not what scripts/flood-pod-atlas.py paints: {:#018x}",
        fnv64(&a.pixels)
    );
    // Every region is painted: none is one flat colour.
    for (name, [x0, y0, x1, y1]) in source::regions() {
        let at = |x: u16, y: u16| {
            let i = (usize::from(y) * usize::from(source::atlas_size().0) + usize::from(x)) * 4;
            [a.pixels[i], a.pixels[i + 1], a.pixels[i + 2]]
        };
        let first = at(x0, y0);
        assert!(
            (y0..y1).any(|y| (x0..x1).any(|x| at(x, y) != first)),
            "{name} is flat"
        );
    }
}

/// The `mode` at `id` reads back as the source: its nodes, markers,
/// triangles and extent.
fn check_model(map: &mut Map<Cursor<Vec<u8>>>, bytes: &[u8], id: TagId) {
    let s = source::builtin().unwrap();
    let built = source::build(&s).unwrap();
    let g =
        RenderModelGeometry::read(map, id, &mut Raw(bytes.to_vec())).expect("the model decodes");
    let names: Vec<_> = g
        .nodes
        .iter()
        .map(|n| n.name.clone().unwrap_or_default())
        .collect();
    let mut want = vec![NODE_SHELL.to_owned()];
    want.extend((0..PETALS).map(petal_node));
    assert_eq!(names, want);
    assert_eq!(g.nodes[0].wire.parent_index, -1);
    for k in 0..PETALS {
        let n = &g.nodes[1 + k];
        assert_eq!(n.wire.parent_index, 0, "{}", names[1 + k]);
        assert!(close(n.wire.translation, RIG.hinge(k), 1e-3));
    }
    assert_eq!(g.sections.len(), 1);
    assert_eq!(g.materials.len(), 1);
    let sec = &g.sections[0];
    let triangles: usize = sec
        .geometry
        .parts
        .iter()
        .map(|p| sec.geometry.part_triangles(p).len())
        .sum();
    assert_eq!(triangles, built.triangles());
    assert_eq!(sec.vertices.positions.len(), built.vertices());
    let (lo, hi) = built.bounds();
    let mut got = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in &sec.vertices.positions {
        for a in 0..3 {
            got.0[a] = got.0[a].min(p[a]);
            got.1[a] = got.1[a].max(p[a]);
        }
    }
    assert!(
        close(got.0, lo, 2e-3) && close(got.1, hi, 2e-3),
        "{got:?} for {lo:?}..{hi:?}"
    );

    // The marker groups, which the tag decoder leaves alone: `+0x58`, each
    // a name, a count and its instances (node at `+2`, place at `+4`).
    let strings = map.strings().to_vec();
    let tag = map.read_tag_bytes(id).unwrap();
    let base = map.tag(id).unwrap().location.unwrap().address;
    let (count, at) = (
        word(&tag, 0x58) as usize,
        (word(&tag, 0x5c) - base) as usize,
    );
    assert_eq!(count, 1 + PETALS);
    let mut seen = Vec::new();
    for k in 0..count {
        let e = at + k * 0xc;
        let name = tags::StringId(word(&tag, e))
            .resolve(&strings)
            .unwrap()
            .unwrap()
            .to_owned();
        assert_eq!(word(&tag, e + 4), 1, "{name}: one instance");
        let i = (word(&tag, e + 8) - base) as usize;
        assert_eq!(tag[i + 2], 0, "{name} is on the shell");
        let local: [f32; 3] = std::array::from_fn(|c| f32::from_bits(word(&tag, i + 4 + 4 * c)));
        assert!(
            close(
                local,
                s.marker(&name).expect("a marker of the source"),
                1e-6
            ),
            "{name}: {local:?}"
        );
        seen.push(name);
    }
    seen.sort();
    let mut all = vec![MARKER_IMPACT.to_owned()];
    all.extend((0..PETALS).map(squad_exit_marker));
    all.sort();
    assert_eq!(seen, all);
}

/// Follows the `vehi` of `bytes` down to its bitmap, asserting each link.
fn check_chain(bytes: &[u8], pod: kit::Tags) {
    let mut map = Map::from_reader(Cursor::new(bytes.to_vec())).unwrap();
    for (group, id) in GROUPS.into_iter().zip([
        pod.bitmap,
        pod.shader,
        pod.render,
        pod.physics,
        pod.model,
        pod.vehicle,
    ]) {
        let t = map
            .tag(id)
            .unwrap_or_else(|e| panic!("no {}: {e}", FourCC(*group)));
        assert_eq!(t.group, FourCC(*group));
        assert_eq!(t.name.as_deref(), Some(TAG_PATH));
    }
    let vehi = map.read_tag_bytes(pod.vehicle).unwrap();
    assert_eq!(word(&vehi, 0) & 0xffff, 1, "the object type is a vehicle's");
    assert_eq!(TagId(word(&vehi, 0x38)), pod.model, "vehi -> hlmt");
    assert_eq!(f32::from_bits(word(&vehi, 4)), kit::BOUNDING_RADIUS);
    let hlmt = map.read_tag_bytes(pod.model).unwrap();
    assert_eq!(TagId(word(&hlmt, 4)), pod.render, "hlmt -> mode");
    assert!(holds(&hlmt, pod.physics), "hlmt -> phmo");
    assert!(
        holds(&map.read_tag_bytes(pod.render).unwrap(), pod.shader),
        "mode -> shad"
    );
    assert!(
        holds(&map.read_tag_bytes(pod.shader).unwrap(), pod.bitmap),
        "shad -> bitm"
    );
    check_model(&mut map, bytes, pod.render);
}

#[test]
fn the_kit_goes_into_a_map_and_reads_back() {
    let original = fixture(0);
    let added = add_flood_pod(&original, "court").unwrap();
    let map = Map::from_reader(Cursor::new(added.bytes.as_slice())).unwrap();
    assert_eq!(map.header().cache_type, CacheType::Multiplayer);
    let marker = format!("flood_pod_kit_v{KIT_VERSION}");
    assert_eq!(inject::marker_string(), marker);
    assert!(map.strings().iter().flatten().any(|s| *s == marker));
    // Every tag the map had is still there, by name and datum.
    let before = Map::from_reader(Cursor::new(original.as_slice())).unwrap();
    for t in before.tags() {
        let now = map.tag(t.id).unwrap();
        assert_eq!((now.group, &now.name), (t.group, &t.name));
    }
    assert_eq!(map.tags().len(), before.tags().len() + GROUPS.len());
    assert_eq!(added.costs.tags as usize, GROUPS.len());
    assert!(added.costs.tag_growth > 0 && added.costs.tag_growth <= added.costs.available);
    check_chain(&added.bytes, added.tags);
}

#[test]
fn the_pod_is_made_of_flesh_where_the_map_has_it() {
    let mut map = Map::from_reader(Cursor::new(fixture(0))).unwrap();
    let facts = kit::Facts::read(&mut map).unwrap();
    assert_eq!(facts.shading.material, (kit::MATERIAL.to_owned(), 0));
}

#[test]
fn the_same_map_builds_the_same_bytes() {
    let original = fixture(0);
    let a = add_flood_pod(&original, "court").unwrap();
    let b = add_flood_pod(&original, "court").unwrap();
    assert!(a.bytes == b.bytes, "two builds differ");
    assert_eq!((a.tags, a.costs), (b.tags, b.costs));
    let c = add_flood_pod(&fixture(0x400), "court").unwrap();
    assert!(c.bytes != a.bytes);
    assert_eq!(c.costs.tags, a.costs.tags);
}

#[test]
fn a_map_without_the_room_is_refused_by_name() {
    let original = fixture(0);
    let had = Map::from_reader(Cursor::new(original.as_slice()))
        .unwrap()
        .header()
        .meta_size;
    let e = format!(
        "{:#}",
        add_flood_pod_within(&original, "court", Some(had + 0x100)).unwrap_err()
    );
    assert!(
        e.contains("court") && e.contains("no room for the Flood pod"),
        "{e}"
    );
    assert!(e.contains("256 B free"), "{e}");
    // With exactly what it takes, it fits.
    let took = add_flood_pod(&original, "court").unwrap().costs.tag_growth;
    assert!(add_flood_pod_within(&original, "court", Some(had + took)).is_ok());
}

#[test]
fn a_map_that_already_has_the_kit_or_is_not_multiplayer_is_refused() {
    let once = add_flood_pod(&fixture(0), "court").unwrap();
    let e = format!("{:#}", add_flood_pod(&once.bytes, "court").unwrap_err());
    assert!(e.contains("already has"), "{e}");

    let mut set = TagSet::new(CacheType::Campaign, "level", r"invented\level");
    set.add_group(FourCC(*b"scnr"), None, None);
    set.scenario = set
        .add_tag(
            FourCC(*b"scnr"),
            r"invented\level",
            TagPayload::Built(Block::zeroed(0x3e0)),
        )
        .unwrap();
    let level = set.build().unwrap().0;
    let e = format!("{:#}", add_flood_pod(&level, "level").unwrap_err());
    assert!(e.contains("multiplayer"), "{e}");
}
