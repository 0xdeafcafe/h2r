//! The sources parse and stay in the rig, the budget and the envelope; the
//! primitives make the triangles their parameters say, facing out; the
//! tags built from a source read back as the source. The `#[ignore]` tests
//! read the Halo 2 disc (`$H2_DISC`).

use std::collections::BTreeSet;
use std::io::Cursor;

use map::build::{Block, TagPayload, TagSet};
use map::{CacheType, FourCC, Map, RawPointer, ResourceFile, TagId};
use tags::model::RenderModelGeometry;

use crate::kit::{self, Facts};
use crate::mesh::{self, Mesh};
use crate::source::{self, Source};
use crate::{MARKERS, NODE_FRAME, NODE_PISTON, RIG, Style, texture};

/// The most triangles and nodes a stick has.
const MAX_TRIANGLES: usize = 2000;
const MAX_NODES: usize = 2;

/// The shaft's axis in y, and how far from it a stick reaches.
const AXIS_Y: f32 = 0.059;
const REACH_Y: f32 = 0.08;
const REACH_X: f32 = 0.05;
/// The envelope in z: the foot's underside to a little over the grips.
const LOW_Z: f32 = -0.361;
const HIGH_Z: f32 = 0.03;
/// Where every look's sleeve ends, and the widest a piston part passing
/// into it is.
const SLEEVE_END_Z: f32 = -0.292;
const ROD_RADIUS: f32 = 0.008;

fn close(a: [f32; 3], b: [f32; 3], eps: f32) -> bool {
    (0..3).all(|k| (a[k] - b[k]).abs() <= eps)
}

/// A source of one part on the frame.
fn one(part: &str) -> Mesh {
    let text = format!(
        "name: test\natlas: unsc\nnodes:\n  - {{ name: frame, at: [0, 0, 0] }}\nmarkers: []\nparts:\n  - {part}\n"
    );
    mesh::build(&source::parse(&text).expect("the source parses")).expect("the mesh builds")
}

/// Every triangle of a convex mesh faces away from its middle and agrees
/// with its vertices' normals.
fn faces_out(m: &Mesh) {
    let (lo, hi) = m.bounds();
    let middle = mesh::mul(mesh::add(lo, hi), 0.5);
    for p in &m.parts {
        for t in &p.triangles {
            let [a, b, c] = t.map(|i| p.vertices[usize::from(i)]);
            let face = mesh::cross(
                mesh::sub(b.position, a.position),
                mesh::sub(c.position, a.position),
            );
            let centre = mesh::mul(
                mesh::add(mesh::add(a.position, b.position), c.position),
                1.0 / 3.0,
            );
            assert!(
                mesh::dot(face, mesh::sub(centre, middle)) > 0.0,
                "a triangle at {centre:?} faces in"
            );
            for v in [a, b, c] {
                assert!(
                    mesh::dot(face, v.normal) > 0.0,
                    "a triangle at {centre:?} against its normal {:?}",
                    v.normal
                );
            }
        }
    }
}

#[test]
fn styles_index_as_the_script_enum() {
    for (i, s) in Style::ALL.into_iter().enumerate() {
        assert_eq!(s.index(), i);
        assert_eq!(Style::from_index(i as i32), Some(s));
    }
    assert_eq!(Style::from_index(-1), None);
    assert_eq!(Style::from_index(3), None);
    assert_eq!(
        Style::Unsc.tag_path(),
        r"objects\weapons\multiplayer\pogo\unsc\pogo_unsc"
    );
    assert_eq!(
        Style::Forerunner.tag_path(),
        r"objects\weapons\multiplayer\pogo\forerunner\pogo_forerunner"
    );
}

#[test]
fn every_source_has_the_rigs_nodes_and_markers() {
    for style in Style::ALL {
        let s = Source::builtin(style).unwrap();
        assert_eq!(s.name, format!("pogo_{}", style.id()));
        assert_eq!(s.nodes.len(), 2, "{}", s.name);
        assert_eq!(s.nodes[0].name, NODE_FRAME);
        assert_eq!(s.nodes[0].at, [0.0; 3]);
        assert_eq!(s.nodes[1].name, NODE_PISTON);
        assert_eq!(s.nodes[1].parent.as_deref(), Some(NODE_FRAME));
        assert_eq!(s.nodes[1].at, RIG.piston, "{}", s.name);
        assert_eq!(s.markers.len(), MARKERS.len(), "{}", s.name);
        for (name, node) in MARKERS {
            let m = s
                .markers
                .iter()
                .find(|m| m.name == name)
                .unwrap_or_else(|| panic!("{}: no marker {name}", s.name));
            assert_eq!(m.node, node, "{}: {name}", s.name);
            assert_eq!(Some(m.at), RIG.marker(name), "{}: {name}", s.name);
        }
    }
}

#[test]
fn every_source_is_in_budget() {
    for style in Style::ALL {
        let s = Source::builtin(style).unwrap();
        let m = mesh::build(&s).unwrap();
        assert!(s.nodes.len() <= MAX_NODES);
        assert!(
            m.triangles() <= MAX_TRIANGLES,
            "{}: {} triangles",
            s.name,
            m.triangles()
        );
        assert!(
            m.triangles() >= 300,
            "{}: {} triangles",
            s.name,
            m.triangles()
        );
        assert!(m.vertices() <= usize::from(u16::MAX));
    }
}

#[test]
fn every_source_is_in_the_envelope() {
    for style in Style::ALL {
        let s = Source::builtin(style).unwrap();
        let (lo, hi) = mesh::build(&s).unwrap().bounds();
        assert!(
            (lo[2] - LOW_Z).abs() < 1e-4,
            "{}: lowest z {}",
            s.name,
            lo[2]
        );
        assert!(
            hi[2] <= HIGH_Z && hi[2] > 0.0,
            "{}: highest z {}",
            s.name,
            hi[2]
        );
        assert!(
            lo[1] >= AXIS_Y - REACH_Y && hi[1] <= AXIS_Y + REACH_Y,
            "{}: y {}..{}",
            s.name,
            lo[1],
            hi[1]
        );
        assert!(
            lo[0] >= -REACH_X && hi[0] <= REACH_X,
            "{}: x {}..{}",
            s.name,
            lo[0],
            hi[0]
        );
    }
}

#[test]
fn texture_coordinates_stay_in_their_region() {
    for style in Style::ALL {
        let s = Source::builtin(style).unwrap();
        for (i, p) in mesh::build(&s).unwrap().parts.iter().enumerate() {
            let [u0, v0, u1, v1] = p.rect;
            for v in &p.vertices {
                assert!(
                    v.uv[0] >= u0 - 1e-5
                        && v.uv[0] <= u1 + 1e-5
                        && v.uv[1] >= v0 - 1e-5
                        && v.uv[1] <= v1 + 1e-5,
                    "{}: part {i}: {:?} outside {:?}",
                    s.name,
                    v.uv,
                    p.rect
                );
            }
        }
    }
}

#[test]
fn normals_are_unit_and_triangles_have_area() {
    for style in Style::ALL {
        let s = Source::builtin(style).unwrap();
        for (i, p) in mesh::build(&s).unwrap().parts.iter().enumerate() {
            for v in &p.vertices {
                for (what, n) in [
                    ("normal", v.normal),
                    ("tangent", v.tangent),
                    ("binormal", v.binormal),
                ] {
                    assert!(
                        (mesh::length(n) - 1.0).abs() < 1e-3,
                        "{}: part {i}: {what} {n:?}",
                        s.name
                    );
                }
                assert!(mesh::dot(v.normal, v.tangent).abs() < 1e-3);
            }
            for t in &p.triangles {
                let [a, b, c] = t.map(|k| p.vertices[usize::from(k)].position);
                let face = mesh::cross(mesh::sub(b, a), mesh::sub(c, a));
                assert!(mesh::length(face) > 0.0, "{}: part {i}", s.name);
            }
        }
    }
}

/// What the piston carries into the sleeve is thin enough to pass into
/// it; everything wider is below the sleeve's end.
#[test]
fn the_piston_passes_into_the_sleeve() {
    for style in Style::ALL {
        let s = Source::builtin(style).unwrap();
        let piston = s.node(NODE_PISTON).unwrap();
        let m = mesh::build(&s).unwrap();
        let mut any = false;
        for p in m.parts.iter().filter(|p| p.node == piston) {
            any = true;
            for v in &p.vertices {
                let [x, y, z] = v.position;
                let off = (x * x + (y - AXIS_Y) * (y - AXIS_Y)).sqrt();
                assert!(
                    z <= SLEEVE_END_Z + 1e-5 || off <= ROD_RADIUS,
                    "{}: a piston vertex {:?} is {off} off the axis above the sleeve's end",
                    s.name,
                    v.position
                );
                assert!(
                    z <= RIG.piston[2] - 0.04 + 1e-5,
                    "{}: {:?}",
                    s.name,
                    v.position
                );
            }
        }
        assert!(any, "{}: no part on the piston", s.name);
        // The foot marker is the piston's lowest point.
        let (lo, _) = m.bounds();
        assert!((lo[2] - RIG.foot[2]).abs() < 1e-4);
    }
}

#[test]
fn a_box_is_twelve_triangles_facing_out() {
    let m = one("box: { node: frame, region: body, min: [-1, -2, -3], max: [1, 2, 3] }");
    assert_eq!(m.triangles(), 12);
    assert_eq!(m.vertices(), 24);
    let (lo, hi) = m.bounds();
    assert!(close(lo, [-1.0, -2.0, -3.0], 1e-6) && close(hi, [1.0, 2.0, 3.0], 1e-6));
    faces_out(&m);
}

#[test]
fn a_bevelled_box_adds_its_edges_and_corners() {
    let m =
        one("box: { node: frame, region: body, min: [-1, -2, -3], max: [1, 2, 3], bevel: 0.25 }");
    // Six faces, twelve edge strips, eight corners.
    assert_eq!(m.triangles(), 6 * 2 + 12 * 2 + 8);
    let (lo, hi) = m.bounds();
    assert!(close(lo, [-1.0, -2.0, -3.0], 1e-6) && close(hi, [1.0, 2.0, 3.0], 1e-6));
    faces_out(&m);
}

#[test]
fn a_lathe_turns_its_profile() {
    // An open tube: two triangles per segment per profile edge.
    let m = one(
        "lathe: { node: frame, region: body, origin: [0, 0, 0], segments: 8, profile: [[1, 0], [1, 1], [1, 3]] }",
    );
    assert_eq!(m.triangles(), 2 * 8 * 2);
    let (lo, hi) = m.bounds();
    assert!(close(lo, [-1.0, -1.0, 0.0], 1e-5) && close(hi, [1.0, 1.0, 3.0], 1e-5));

    // Capped, each end is a fan: one triangle per segment.
    let m = one(
        "lathe: { node: frame, region: body, origin: [0, 0, 0], segments: 8, caps: true, profile: [[1, 0], [1, 2]] }",
    );
    assert_eq!(m.triangles(), 2 * 8 + 8 + 8);
    faces_out(&m);

    // The same profile given downwards is the same solid.
    let n = one(
        "lathe: { node: frame, region: body, origin: [0, 0, 0], segments: 8, caps: true, profile: [[1, 2], [1, 0]] }",
    );
    assert_eq!(n.triangles(), m.triangles());
    faces_out(&n);

    // About another axis.
    let m = one(
        "lathe: { node: frame, region: body, origin: [0, 5, 0], axis: [0, 1, 0], segments: 6, caps: true, profile: [[1, -1], [1, 1]] }",
    );
    let (lo, hi) = m.bounds();
    assert!((lo[1] - 4.0).abs() < 1e-5 && (hi[1] - 6.0).abs() < 1e-5);
    faces_out(&m);
}

#[test]
fn a_closed_lathe_is_a_ring() {
    let m = one(
        "lathe: { node: frame, region: body, origin: [0, 0, 0], segments: 10, closed: true, shade: flat, profile: [[2, 0], [3, 0], [3, 1], [2, 1]] }",
    );
    assert_eq!(m.triangles(), 2 * 10 * 4);
    let (lo, hi) = m.bounds();
    assert!(close(lo, [-3.0, -3.0, 0.0], 0.2) && close(hi, [3.0, 3.0, 1.0], 0.2));
}

#[test]
fn a_sweep_carries_its_section_along_the_path() {
    let m = one(
        "sweep: { node: frame, region: body, section: { circle: { radius: 0.5, sides: 6 } }, path: [[0, 0, 0], [0, 4, 0]] }",
    );
    // The tube, and a fan on each end.
    assert_eq!(m.triangles(), 2 * 6 + 2 * (6 - 2));
    let (lo, hi) = m.bounds();
    assert!((lo[1] - 0.0).abs() < 1e-5 && (hi[1] - 4.0).abs() < 1e-5);
    assert!(lo[0] >= -0.5 - 1e-5 && hi[0] <= 0.5 + 1e-5);
    faces_out(&m);

    let open = one(
        "sweep: { node: frame, region: body, section: { polygon: [[1, 1], [-1, 1], [-1, -1], [1, -1]] }, path: [[0, 0, 0], [0, 0, 2], [0, 0, 5]], caps: false, shade: flat }",
    );
    assert_eq!(open.triangles(), 2 * 4 * 2);
}

#[test]
fn a_source_with_an_unknown_key_or_region_is_refused() {
    let head = "name: test\natlas: unsc\nnodes:\n  - { name: frame, at: [0, 0, 0] }\nmarkers: []\nparts:\n";
    let good =
        format!("{head}  - box: {{ node: frame, region: body, min: [0, 0, 0], max: [1, 1, 1] }}\n");
    assert!(source::parse(&good).is_ok());
    for bad in [
        format!(
            "{head}  - box: {{ node: frame, region: body, min: [0, 0, 0], max: [1, 1, 1], colour: red }}\n"
        ),
        format!(
            "{head}  - box: {{ node: frame, region: chrome, min: [0, 0, 0], max: [1, 1, 1] }}\n"
        ),
        format!(
            "{head}  - box: {{ node: spring, region: body, min: [0, 0, 0], max: [1, 1, 1] }}\n"
        ),
        format!("{head}  - cone: {{ node: frame, region: body }}\n"),
        format!("{good}extra: 1\n"),
    ] {
        assert!(source::parse(&bad).is_err(), "{bad}");
    }
}

#[test]
fn the_class_is_the_pistols_where_every_player_holds_it() {
    let set = |pairs: &[(&str, &str)]| -> BTreeSet<(String, String)> {
        pairs
            .iter()
            .map(|(c, t)| ((*c).to_owned(), (*t).to_owned()))
            .collect()
    };
    let chief = set(&[("pistol", "hp"), ("rifle", "br"), ("missile", "rl")]);
    let elite = set(&[("pistol", "hp"), ("pistol", "pp"), ("rifle", "br")]);
    assert_eq!(
        kit::choose_class(&[chief.clone(), elite.clone()]),
        Some(("pistol".to_owned(), "hp".to_owned()))
    );
    // Without it, the first pair both hold, in name order.
    let other = set(&[("rifle", "br"), ("pistol", "pp"), ("missile", "rl")]);
    assert_eq!(
        kit::choose_class(&[chief.clone(), other]),
        Some(("missile".to_owned(), "rl".to_owned()))
    );
    assert_eq!(
        kit::choose_class(&[chief.clone(), set(&[("sword", "es")])]),
        None
    );
    assert_eq!(kit::choose_class(&[chief, BTreeSet::new()]), None);
    assert_eq!(kit::choose_class(&[]), None);
}

#[test]
fn the_atlases_are_the_size_the_regions_are_laid_on() {
    for style in Style::ALL {
        let a = texture::decode(source::atlas(style)).unwrap();
        assert_eq!(
            (a.width, a.height),
            (u32::from(source::atlas_size().0), u32::from(source::atlas_size().1)),
            "{}",
            style.id()
        );
        assert!(
            a.pixels.chunks_exact(4).all(|p| p[3] == 255),
            "{}: opaque",
            style.id()
        );
    }
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
    }
}

#[test]
fn a_mip_chain_halves_to_one_block() {
    assert_eq!(texture::mip_count(512, 256), 6);
    assert_eq!(texture::mip_count(4, 4), 0);
    let src = texture::Rgba {
        width: 4,
        height: 2,
        pixels: [[0u8, 0, 0, 255], [255, 255, 255, 255]]
            .iter()
            .cycle()
            .take(8)
            .flatten()
            .copied()
            .collect(),
    };
    let half = texture::halve(&src);
    assert_eq!((half.width, half.height), (2, 1));
    assert_eq!(half.pixels, [128, 128, 128, 255, 128, 128, 128, 255]);
    assert!(
        texture::decode(b"version https://git-lfs.github.com/spec/v1\noid sha256:0\nsize 1\n")
            .unwrap_err()
            .to_string()
            .contains("Git LFS")
    );
}

// ---------------------------------------------------------------------------
// Tags
// ---------------------------------------------------------------------------

pub(crate) struct Raw(pub(crate) Vec<u8>);
impl tags::geometry::RawSource for Raw {
    fn read_raw(&mut self, p: RawPointer, size: u32) -> tags::Result<Vec<u8>> {
        assert_eq!(p.file, ResourceFile::Local);
        Ok(self.0[p.offset as usize..(p.offset + size) as usize].to_vec())
    }
}

pub(crate) use crate::fixture::{FIXTURE_FIRST_PERSON, fixture, fixture_class};

fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

/// Whether a tag's bytes hold `datum` at a word boundary.
pub(crate) fn holds(bytes: &[u8], datum: TagId) -> bool {
    bytes.chunks_exact(4).any(|w| w == datum.0.to_le_bytes())
}

/// Follows a `weap` of `bytes` down to its bitmap, asserting each link and
/// that the render model reads back as `style`'s source.
pub(crate) fn check_chain(bytes: &[u8], weapon: TagId, style: Style) {
    let mut map = Map::from_reader(Cursor::new(bytes.to_vec())).unwrap();
    let group = |map: &Map<Cursor<Vec<u8>>>, id: TagId| map.tag(id).unwrap().group;
    let path = style.tag_path();
    let find = |map: &Map<Cursor<Vec<u8>>>, g: &[u8; 4]| {
        map.tags()
            .iter()
            .find(|t| t.group == FourCC(*g) && t.name.as_deref() == Some(path.as_str()))
            .map(|t| t.id)
            .unwrap_or_else(|| panic!("no {path}.{}", FourCC(*g)))
    };
    assert_eq!(find(&map, b"weap"), weapon);
    assert_eq!(group(&map, weapon), FourCC(*b"weap"));
    let weap = map.read_tag_bytes(weapon).unwrap();
    assert_eq!(word(&weap, 0) & 0xffff, 2, "the object type is a weapon's");
    let hlmt = TagId(word(&weap, 0x38));
    assert_eq!(hlmt, find(&map, b"hlmt"));
    let model = map.read_tag_bytes(hlmt).unwrap();
    let mode = TagId(word(&model, 4));
    assert_eq!(mode, find(&map, b"mode"));
    let shad = find(&map, b"shad");
    let bitm = find(&map, b"bitm");
    assert!(
        holds(&map.read_tag_bytes(mode).unwrap(), shad),
        "mode -> shad"
    );
    assert!(
        holds(&map.read_tag_bytes(shad).unwrap(), bitm),
        "shad -> bitm"
    );
    let shared = kit::shared_path();
    for g in [b"phmo", b"nhdt"] {
        let id = map
            .tags()
            .iter()
            .find(|t| t.group == FourCC(*g) && t.name.as_deref() == Some(shared.as_str()))
            .map(|t| t.id)
            .unwrap_or_else(|| panic!("no {shared}.{}", FourCC(*g)));
        let owner = if g == b"phmo" { &model } else { &weap };
        assert!(holds(owner, id), "{} is referenced", FourCC(*g));
    }
    check_model(&mut map, bytes, mode, style);
}

/// The `mode` at `id` reads back as `style`'s source: its nodes, markers,
/// triangles and extent.
fn check_model(map: &mut Map<Cursor<Vec<u8>>>, bytes: &[u8], id: TagId, style: Style) {
    let source = Source::builtin(style).unwrap();
    let built = mesh::build(&source).unwrap();
    let g =
        RenderModelGeometry::read(map, id, &mut Raw(bytes.to_vec())).expect("the model decodes");
    let names: Vec<_> = g
        .nodes
        .iter()
        .map(|n| n.name.clone().unwrap_or_default())
        .collect();
    assert_eq!(names, [NODE_FRAME, NODE_PISTON]);
    assert_eq!(g.nodes[0].wire.parent_index, -1);
    assert_eq!(g.nodes[1].wire.parent_index, 0);
    assert_eq!(g.nodes[1].wire.translation, RIG.piston);
    assert_eq!(g.sections.len(), 1);
    assert_eq!(g.materials.len(), 1);
    let s = &g.sections[0];
    let triangles: usize = s
        .geometry
        .parts
        .iter()
        .map(|p| s.geometry.part_triangles(p).len())
        .sum();
    assert_eq!(triangles, built.triangles(), "{}", source.name);
    assert_eq!(s.vertices.positions.len(), built.vertices());
    let (lo, hi) = built.bounds();
    let mut got = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in &s.vertices.positions {
        for a in 0..3 {
            got.0[a] = got.0[a].min(p[a]);
            got.1[a] = got.1[a].max(p[a]);
        }
    }
    assert!(
        close(got.0, lo, 1e-3) && close(got.1, hi, 1e-3),
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
    assert_eq!(count, MARKERS.len());
    for k in 0..count {
        let e = at + k * 0xc;
        let name = tags::StringId(word(&tag, e))
            .resolve(&strings)
            .unwrap()
            .unwrap()
            .to_owned();
        let (_, node) = MARKERS
            .into_iter()
            .find(|(n, _)| *n == name)
            .expect("a rig marker");
        assert_eq!(word(&tag, e + 4), 1, "{name}: one instance");
        let i = (word(&tag, e + 8) - base) as usize;
        let on = usize::from(tag[i + 2]);
        assert_eq!(names[on], node, "{name}");
        let local: [f32; 3] = std::array::from_fn(|c| f32::from_bits(word(&tag, i + 4 + 4 * c)));
        let world = mesh::add(local, source.nodes[on].at);
        assert!(
            close(world, RIG.marker(&name).unwrap(), 1e-6),
            "{name}: {world:?}"
        );
    }
}

/// The three looks installed on the fixture map, built.
fn kitted() -> (Vec<u8>, [TagId; 3]) {
    let original = fixture(0);
    let mut map = Map::from_reader(Cursor::new(original)).unwrap();
    let facts = Facts::read_with_class(&mut map, fixture_class()).unwrap();
    assert_eq!(facts.material, (kit::MATERIAL.to_owned(), 1));
    assert_eq!(facts.layout.stem, kit::LAYOUTS[0].stem);
    let mut set = TagSet::from_map(&mut map).unwrap();
    let looks = kit::install_all(&mut set, &facts).unwrap();
    (
        set.build().expect("the kitted map builds").0,
        looks.map(|t| t.weapon),
    )
}

#[test]
fn the_three_looks_read_back_from_a_built_map() {
    let (bytes, weapons) = kitted();
    for style in Style::ALL {
        check_chain(&bytes, weapons[style.index()], style);
    }
}

#[test]
fn the_weapon_is_inert_and_held_as_the_class() {
    let (bytes, weapons) = kitted();
    let mut map = Map::from_reader(Cursor::new(bytes)).unwrap();
    let strings = map.strings().to_vec();
    let weap = map.read_tag_bytes(weapons[0]).unwrap();
    let text = |at: usize| {
        tags::StringId(word(&weap, at))
            .resolve(&strings)
            .unwrap()
            .unwrap_or("")
            .to_owned()
    };
    assert_eq!((text(0x288), text(0x28c)), fixture_class());
    assert_eq!(
        word(&weap, 0x12c),
        1 << 16,
        "no binocular zoom, nothing else"
    );
    assert_eq!(f32::from_bits(word(&weap, 4)), kit::BOUNDING_RADIUS);
    // Held, it has the map's pistol's first-person view: both elements,
    // each a model and a graph the map has.
    let fp = kit::first_person_of(&mut map, weapons[0]).unwrap();
    let named = |group: &[u8; 4], path: &str| {
        map.tags()
            .iter()
            .find(|t| t.group == FourCC(*group) && t.name.as_deref() == Some(path))
            .map(|t| t.id)
            .unwrap()
    };
    let want: Vec<_> = FIXTURE_FIRST_PERSON
        .iter()
        .map(|(m, g)| kit::FirstPerson {
            model: named(b"mode", m),
            animations: named(b"jmad", g),
        })
        .collect();
    assert_eq!(fp, want);
    // A second kit on the same map is refused by name.
    let facts = Facts::read_with_class(&mut map, fixture_class()).unwrap();
    let mut set = TagSet::from_map(&mut map).unwrap();
    let e = kit::install_all(&mut set, &facts).unwrap_err().to_string();
    assert!(e.contains("already has"), "{e}");
}

#[test]
fn the_first_person_view_is_the_classs_weapons() {
    let id = |n: u32| TagId(0xe000_0000 | n);
    let weapons = vec![
        ("rifle".to_owned(), "ar".to_owned(), id(1)),
        ("pistol".to_owned(), "plasma".to_owned(), id(2)),
        ("pistol".to_owned(), "hp".to_owned(), id(3)),
    ];
    let class = |c: &str, n: &str| (c.to_owned(), n.to_owned());
    // The class and type exactly, wherever it is.
    assert_eq!(kit::first_person_source(&weapons, &class("pistol", "hp")), Some(id(3)));
    // Else the first of the class.
    assert_eq!(kit::first_person_source(&weapons, &class("pistol", "smg")), Some(id(2)));
    // None of the class: none.
    assert_eq!(kit::first_person_source(&weapons, &class("sword", "any")), None);
    assert_eq!(kit::first_person_source(&[], &class("pistol", "hp")), None);
}

#[test]
fn a_map_without_a_weapon_of_the_class_is_refused_by_name() {
    let mut map = Map::from_reader(Cursor::new(fixture(0))).unwrap();
    let e = Facts::read_with_class(&mut map, ("sword".to_owned(), "any".to_owned()))
        .unwrap_err()
        .to_string();
    assert!(e.contains("first-person") && e.contains("sword"), "{e}");
}

#[test]
fn a_map_without_a_shader_template_is_refused_by_name() {
    let mut set = TagSet::new(CacheType::Multiplayer, "bare", r"invented\bare");
    set.add_group(FourCC(*b"scnr"), None, None);
    set.scenario = set
        .add_tag(
            FourCC(*b"scnr"),
            r"invented\bare",
            TagPayload::Built(Block::zeroed(0x3e0)),
        )
        .unwrap();
    let bytes = set.build().unwrap().0;
    let mut map = Map::from_reader(Cursor::new(bytes)).unwrap();
    let e = Facts::read_with_class(&mut map, fixture_class())
        .unwrap_err()
        .to_string();
    assert!(e.contains("tex_bump"), "{e}");
}

// ---------------------------------------------------------------------------
// The disc
// ---------------------------------------------------------------------------

/// The disc image at `$H2_DISC`.
pub(crate) fn disc() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var_os("H2_DISC").expect("set H2_DISC to halo2.iso"))
}

/// Every multiplayer map of the disc, by stem, with its bytes.
pub(crate) fn disc_multiplayer_maps() -> Vec<(String, Vec<u8>)> {
    let file = std::fs::File::open(disc()).expect("the disc opens");
    let mut iso = xiso::Xiso::open(file).expect("the disc is an Xbox image");
    let entries: Vec<_> = iso
        .entries()
        .expect("the disc lists")
        .into_iter()
        .filter(|e| !e.is_dir && e.path.to_ascii_lowercase().ends_with(".map"))
        .collect();
    let mut out = Vec::new();
    for e in entries {
        let bytes = iso.read(&e).expect("the map reads");
        let Ok(map) = Map::from_reader(Cursor::new(bytes.as_slice())) else {
            continue;
        };
        if map.header().cache_type != CacheType::Multiplayer {
            continue;
        }
        let stem = e
            .path
            .rsplit(['\\', '/'])
            .next()
            .unwrap_or(&e.path)
            .trim_end_matches(".map")
            .to_ascii_lowercase();
        out.push((stem, bytes));
    }
    assert!(!out.is_empty(), "the disc has multiplayer maps");
    out
}

/// Both player graphs hold the chosen class on every multiplayer map, and
/// every map can supply the rest of what the kit reads.
#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn every_multiplayer_map_holds_the_class() {
    for (stem, bytes) in disc_multiplayer_maps() {
        let mut map = Map::from_reader(Cursor::new(bytes)).unwrap();
        let graphs = kit::player_graphs(&mut map).unwrap_or_else(|e| panic!("{stem}: {e:#}"));
        assert_eq!(
            graphs.len(),
            kit::PLAYER_BIPEDS.len(),
            "{stem}: both player bipeds"
        );
        let class = kit::player_class(&mut map).unwrap_or_else(|e| panic!("{stem}: {e:#}"));
        for (biped, pairs) in &graphs {
            assert!(pairs.contains(&class), "{stem}: {biped} lacks {class:?}");
        }
        let facts = Facts::read(&mut map).unwrap_or_else(|e| panic!("{stem}: {e:#}"));
        // The first-person view the stick is held in: one element a
        // player character, each graph with an idle the stick's ready
        // and hold play from.
        assert_eq!(facts.first_person.len(), 2, "{stem}: {:?}", facts.first_person);
        for fp in &facts.first_person {
            let idle = kit::graph_has_idle(&mut map, fp.animations)
                .unwrap_or_else(|e| panic!("{stem}: {e:#}"));
            assert!(idle, "{stem}: the first-person graph {:?} has no idle", fp.animations);
        }
        println!(
            "{stem}: class {:?}, shader {}, material {:?}, first person {:?}",
            facts.class, facts.layout.stem, facts.material, facts.first_person
        );
    }
}

/// The manifests (`content/<item>/kit.yaml`) say what the tables in code
/// said before the kits became content: the same atlas, the same regions
/// in the same order, and a region's texture rectangle the same to the bit
/// (the formula is the retired `source::region`'s), so the meshes' texture
/// coordinates, and so the tags built from them, are unchanged.
#[test]
fn manifests_are_the_retired_tables() {
    use crate::atlas::Manifest;

    fn retired(regions: &[(&str, [u16; 4])], name: &str) -> Option<[f32; 4]> {
        let (_, [x0, y0, x1, y1]) = regions.iter().find(|(n, _)| *n == name)?;
        let (w, h) = (512.0_f32, 256.0_f32);
        let inset = 2.0_f32;
        Some([
            (f32::from(*x0) + inset) / w,
            (f32::from(*y0) + inset) / h,
            (f32::from(*x1) - inset) / w,
            (f32::from(*y1) - inset) / h,
        ])
    }

    const POGO: [(&str, [u16; 4]); 8] = [
        ("body", [0, 0, 256, 128]),
        ("body_dark", [256, 0, 384, 128]),
        ("piston", [384, 0, 512, 128]),
        ("grip", [0, 128, 128, 192]),
        ("peg", [0, 192, 128, 256]),
        ("foot", [128, 128, 256, 256]),
        ("accent", [256, 128, 384, 256]),
        ("glow", [384, 128, 512, 256]),
    ];
    const FLOOD: [(&str, [u16; 4]); 6] = [
        ("flesh", [0, 0, 256, 128]),
        ("petal", [256, 0, 512, 128]),
        ("sac", [0, 128, 128, 256]),
        ("bone", [128, 128, 256, 256]),
        ("rib", [256, 128, 384, 256]),
        ("tendril", [384, 128, 512, 256]),
    ];

    for (manifest, table) in [
        (source::manifest(), &POGO[..]),
        (crate::flood_pod::source::manifest(), &FLOOD[..]),
    ] {
        assert_eq!(manifest.atlas.size(), (512, 256), "{}", manifest.item);
        assert_eq!(manifest.atlas.inset.to_bits(), 2.0_f32.to_bits());
        assert_eq!(manifest.atlas.regions(), table, "{}", manifest.item);
        for (name, _) in table {
            let got = manifest.atlas.region(name).unwrap();
            let want = retired(table, name).unwrap();
            assert_eq!(got.map(f32::to_bits), want.map(f32::to_bits), "{name}");
        }
        assert_eq!(manifest.atlas.region("nowhere"), None);
    }
    assert_eq!(source::manifest().max_triangles, None);
    assert_eq!(crate::flood_pod::source::manifest().max_triangles, Some(2000));

    // The check refuses a region off the atlas, a repeat and an overlap.
    let bad = |regions: &str| {
        Manifest::parse(&format!(
            "item: x\natlas:\n  width: 8\n  height: 8\n  inset: 0.0\n  regions:\n{regions}"
        ))
    };
    assert!(bad("    - { name: a, rect: [0, 0, 9, 4] }\n").is_err());
    assert!(bad("    - { name: a, rect: [0, 0, 4, 4] }\n    - { name: a, rect: [4, 0, 8, 4] }\n").is_err());
    assert!(bad("    - { name: a, rect: [0, 0, 4, 4] }\n    - { name: b, rect: [3, 0, 8, 4] }\n").is_err());
    assert!(bad("    - { name: a, rect: [0, 0, 4, 4] }\n    - { name: b, rect: [4, 0, 8, 4] }\n").is_ok());
}
