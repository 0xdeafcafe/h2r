//! The kit on the invented map: every tag it makes decodes through `tags`
//! and reads back as the definition's geometry, the same definition gives
//! the same bytes, and the cost is what the builder places. No disc.

use std::io::Cursor;

use collision_build::{Polygon, query};
use forge_shapes::{Material, Materials, PieceDef, Role, Shape, TagKey};
use map::build::TagSet;
use map::{FourCC, Map, RawPointer, ResourceFile, TagId};
use tags::collision::{Collision, PhysicsModel};
use tags::model::RenderModelGeometry;

use crate::blockout::fixture::{MATERIALS, ROCK_SHADER, fixture};
use crate::blockout::install::tag_size;
use crate::blockout::mesh::{self, Paint, View};
use crate::blockout::{Facts, Installed, collision, dev, install, make_index_room, physics};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

struct Raw(Vec<u8>);
impl tags::geometry::RawSource for Raw {
    fn read_raw(&mut self, p: RawPointer, size: u32) -> tags::Result<Vec<u8>> {
        assert_eq!(p.file, ResourceFile::Local);
        Ok(self.0[p.offset as usize..(p.offset + size) as usize].to_vec())
    }
}

fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

/// Whether a tag's bytes hold `datum` at a word boundary.
fn holds(bytes: &[u8], datum: TagId) -> bool {
    bytes.chunks_exact(4).any(|w| w == datum.0.to_le_bytes())
}

/// The fixture's facts and a set decomposed from it, ready for tags.
fn start() -> (Facts, TagSet) {
    let mut map = Map::from_reader(Cursor::new(fixture())).unwrap();
    let facts = Facts::read(&mut map).unwrap();
    let mut set = TagSet::from_map(&mut map).unwrap();
    make_index_room(&mut set);
    (facts, set)
}

/// `defs` installed on the fixture, built.
fn built(defs: &[PieceDef]) -> (Vec<u8>, Vec<Installed>) {
    let (facts, mut set) = start();
    let installed = defs
        .iter()
        .map(|d| install(&mut set, d, &facts).unwrap_or_else(|e| panic!("{}: {e:#}", d.name())))
        .collect();
    (set.build().expect("the map builds").0, installed)
}

fn open(bytes: &[u8]) -> Map<Cursor<Vec<u8>>> {
    Map::from_reader(Cursor::new(bytes.to_vec())).unwrap()
}

fn with(shape: Shape, size: [u16; 3], param: u16, materials: Materials) -> PieceDef {
    PieceDef {
        shape,
        size,
        param,
        materials,
    }
}

/// A seeded generator (SplitMix64).
struct Rng(u64);

impl Rng {
    fn step(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        let u = (self.step() >> 40) as f32 / (1u64 << 24) as f32;
        lo + (hi - lo) * u
    }

    fn point(&mut self, lo: [f32; 3], hi: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|a| self.range(lo[a], hi[a]))
    }
}

// ---------------------------------------------------------------------------
// The brute-force ray
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
enum Brute {
    /// First hit: the fraction along the segment and the polygon.
    Hit(f64, usize),
    Clear,
    /// Too near an edge, a corner or a plane to call.
    Unsure,
}

type V = [f64; 3];

fn dot(a: V, b: V) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn sub(a: V, b: V) -> V {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: V, b: V) -> V {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// How close to an edge, corner or plane a segment may pass and be called.
const MARGIN: f64 = 2e-3;

/// The first polygon the segment `a`-`b` meets from its outer side.
fn brute(polys: &[Polygon], a: [f32; 3], b: [f32; 3]) -> Brute {
    let (a, b) = (a.map(f64::from), b.map(f64::from));
    let mut best: Option<(f64, usize)> = None;
    for (i, poly) in polys.iter().enumerate() {
        let pts: Vec<V> = poly.points.iter().map(|p| p.map(f64::from)).collect();
        let mut nv = [0.0; 3];
        for k in 0..pts.len() {
            let c = cross(pts[k], pts[(k + 1) % pts.len()]);
            nv = [nv[0] + c[0], nv[1] + c[1], nv[2] + c[2]];
        }
        let l = dot(nv, nv).sqrt();
        let n = nv.map(|c| c / l);
        let d = dot(n, pts[0]);
        let (da, db) = (dot(n, a) - d, dot(n, b) - d);
        if da.abs() < MARGIN || db.abs() < MARGIN {
            if da.signum() != db.signum() || da.abs() < MARGIN && db.abs() < MARGIN {
                return Brute::Unsure;
            }
            continue;
        }
        if !(da > 0.0 && db < 0.0) {
            continue;
        }
        let t = da / (da - db);
        let p: V = std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t);
        let mut inside = f64::INFINITY;
        for k in 0..pts.len() {
            let (e0, e1) = (pts[k], pts[(k + 1) % pts.len()]);
            let e = sub(e1, e0);
            let el = dot(e, e).sqrt();
            let inward = cross(n, e).map(|c| c / el);
            inside = inside.min(dot(inward, sub(p, e0)));
        }
        if inside.abs() < MARGIN {
            return Brute::Unsure;
        }
        if inside > 0.0 && best.is_none_or(|(bt, _)| t < bt) {
            best = Some((t, i));
        }
    }
    match best {
        Some((t, i)) => Brute::Hit(t, i),
        None => Brute::Clear,
    }
}

// ---------------------------------------------------------------------------
// coll
// ---------------------------------------------------------------------------

#[test]
fn every_starter_coll_passes_its_check_and_agrees_with_brute_force() {
    for (n, def) in forge_shapes::starter().iter().enumerate() {
        let solid = def.generate().unwrap();
        let compiled = collision::compile(&solid).unwrap_or_else(|e| panic!("{}: {e:#}", def.name()));
        assert!(collision_build::verify::check(&compiled.bsp, 1).is_empty());
        let polys = collision::polygons(&solid);
        let (lo, hi) = solid.bounds();
        let pad = 0.25;
        let (lo, hi) = (lo.map(|v| v - pad), hi.map(|v| v + pad));
        let mut rng = Rng(0x5eed_0000 + n as u64);
        let (mut called, mut hits) = (0, 0);
        for i in 0..1000 {
            let (a, b) = (rng.point(lo, hi), rng.point(lo, hi));
            let want = brute(&polys, a, b);
            if want == Brute::Unsure {
                continue;
            }
            called += 1;
            let trace = query::vector(&compiled.bsp, a, b);
            match (want, trace.hit) {
                (Brute::Clear, None) => {}
                (Brute::Hit(t, poly), Some(h)) => {
                    hits += 1;
                    assert_eq!(
                        compiled.surface_source[h.surface],
                        poly,
                        "{}: segment {i}: the walk hit another face",
                        def.name()
                    );
                    assert!((f64::from(h.t) - t).abs() < 1e-4, "{}: segment {i}", def.name());
                }
                (want, got) => panic!("{}: segment {i} {a:?} -> {b:?}: brute {want:?}, walk {got:?}", def.name()),
            }
            assert!(trace.missed.is_empty(), "{}: segment {i} entered solid unreported", def.name());
        }
        assert!(called > 500, "{}: only {called} segments called", def.name());
        assert!(hits > 0, "{}: no segment hit", def.name());
    }
}

#[test]
fn the_coll_decodes_to_the_compiled_bsp() {
    let defs = forge_shapes::starter();
    let (bytes, installed) = built(&defs);
    let mut map = open(&bytes);
    let strings = map.strings().to_vec();
    for (def, got) in defs.iter().zip(&installed) {
        let coll = Collision::read(&mut map, got.collision).expect("the coll decodes");
        let compiled = collision::compile(&def.generate().unwrap()).unwrap();
        assert_eq!(coll.materials.len(), 1);
        assert_eq!(
            coll.materials[0].resolve(&strings).unwrap(),
            Some(crate::blockout::data::data().collision_material.as_str())
        );
        assert_eq!(coll.regions.len(), 1);
        let perm = &coll.regions[0].permutations;
        assert_eq!(perm.len(), 1);
        assert_eq!(perm[0].bsps, [compiled.bsp], "{}", def.name());
        assert!(perm[0].bsp_physics.is_empty(), "a coll with a phmo has no slots");
        assert_eq!(coll.nodes.len(), 1);
    }
}

// ---------------------------------------------------------------------------
// phmo
// ---------------------------------------------------------------------------

#[test]
fn the_phmo_decodes_to_the_hulls_corners_as_one_fixed_body() {
    let defs = forge_shapes::starter();
    let (bytes, installed) = built(&defs);
    let mut map = open(&bytes);
    for (def, got) in defs.iter().zip(&installed) {
        let solid = def.generate().unwrap();
        let phmo = PhysicsModel::read(&mut map, got.physics).expect("the phmo decodes");
        let problems = phmo.validate();
        assert!(problems.is_empty(), "{}: {problems:?}", def.name());
        assert_eq!(phmo.rigid_bodies.len(), 1);
        let body = &phmo.rigid_bodies[0];
        assert_eq!(body.node, -1);
        let flags = u16::from_le_bytes([body.raw[0x18], body.raw[0x19]]);
        let motion = u16::from_le_bytes([body.raw[0x1a], body.raw[0x1b]]);
        assert_eq!((flags, motion), (
            crate::blockout::data::data().physics.flags,
            crate::blockout::data::data().physics.motion
        ));
        assert_eq!(phmo.polyhedra.len(), solid.hulls.len(), "{}", def.name());
        for (p, hull) in phmo.polyhedra.iter().zip(&solid.hulls) {
            assert_eq!(p.vertices, hull.points, "{}", def.name());
            assert_eq!(p.planes.len(), hull.faces.len());
            // Every corner is on or behind every plane.
            for v in &hull.points {
                for pl in &p.planes {
                    let s = pl[0] * v[0] + pl[1] * v[1] + pl[2] * v[2] + pl[3];
                    assert!(s <= 1e-4, "{}: a corner {s} outside a plane", def.name());
                }
            }
        }
    }
}

#[test]
fn a_box_hull_has_the_box_volume_and_inertia() {
    let def = PieceDef::dev(Shape::Block, [200, 100, 50], 0);
    let solid = def.generate().unwrap();
    let piece = physics::piece(&solid.hulls[0]).unwrap();
    let (a, b, c) = (2.0f32, 1.0f32, 0.5f32);
    let close = |x: f32, y: f32| (x - y).abs() <= 1e-5 * y.abs().max(1.0);
    assert!(close(piece.volume, a * b * c), "{}", piece.volume);
    assert!(piece.centroid.iter().zip([1.0, 0.5, 0.25]).all(|(g, w)| close(*g, w)));
    let m = a * b * c;
    let want = [m * (b * b + c * c) / 12.0, m * (a * a + c * c) / 12.0, m * (a * a + b * b) / 12.0];
    for k in 0..3 {
        assert!(close(piece.inertia[k][k], want[k]), "axis {k}: {:?}", piece.inertia);
        for j in 0..3 {
            if j != k {
                assert!(piece.inertia[k][j].abs() < 1e-5);
            }
        }
    }
}

#[test]
fn the_size_class_follows_the_retail_bands() {
    assert_eq!(physics::size_class(0.4), 3);
    assert_eq!(physics::size_class(1.0), 4);
    assert_eq!(physics::size_class(1.5), 5);
    assert_eq!(physics::size_class(2.13), 6);
    assert_eq!(physics::size_class(20.0), 6);
}

// ---------------------------------------------------------------------------
// mode
// ---------------------------------------------------------------------------

/// The render model at `id`, decoded.
fn geometry(map: &mut Map<Cursor<Vec<u8>>>, bytes: &[u8], id: TagId) -> RenderModelGeometry {
    RenderModelGeometry::read(map, id, &mut Raw(bytes.to_vec())).expect("the mode decodes")
}

#[test]
fn the_mode_decodes_to_the_mesh_within_quantisation() {
    let defs = forge_shapes::starter();
    let (bytes, installed) = built(&defs);
    let mut map = open(&bytes);
    for (def, got) in defs.iter().zip(&installed) {
        let solid = def.generate().unwrap();
        let mesh = mesh::build(def, &solid, View::Look);
        let g = geometry(&mut map, &bytes, got.render.expect("a dev block has a mode"));
        assert_eq!(g.sections.len(), 1);
        assert_eq!(g.materials.len(), mesh.parts.len());
        let s = &g.sections[0];
        assert_eq!(s.vertices.positions.len(), mesh.vertices(), "{}", def.name());
        let (lo, hi) = solid.bounds();
        let extent = (0..3).map(|k| hi[k] - lo[k]).fold(0.0f32, f32::max);
        let eps = extent / 32767.0 + 1e-5;
        let mut base = 0usize;
        for (k, part) in mesh.parts.iter().enumerate() {
            let decoded = s
                .geometry
                .parts
                .iter()
                .find(|p| usize::from(p.material) == k)
                .unwrap_or_else(|| panic!("{}: no part of material {k}", def.name()));
            let tris = s.geometry.part_triangles(decoded);
            let want: Vec<[u16; 3]> = part
                .triangles
                .iter()
                .map(|t| t.map(|i| i + base as u16))
                .collect();
            assert_eq!(tris, want, "{}: part {k}", def.name());
            for (i, v) in part.vertices.iter().enumerate() {
                let p = s.vertices.positions[base + i];
                assert!(
                    (0..3).all(|a| (p[a] - v.position[a]).abs() <= eps),
                    "{}: vertex {i} of part {k}: {p:?} for {:?}",
                    def.name(),
                    v.position
                );
            }
            base += part.vertices.len();
        }
    }
}

#[test]
fn the_glyph_cells_of_each_plate_read_back_as_its_text() {
    let defs = forge_shapes::starter();
    let (bytes, installed) = built(&defs);
    let mut map = open(&bytes);
    let atlas = dev_texture::font::font();
    let mut plates = 0;
    for (def, got) in defs.iter().zip(&installed) {
        let solid = def.generate().unwrap();
        let mesh = mesh::build(def, &solid, View::Dev);
        let Some(font) = mesh.parts.iter().position(|p| p.paint == Paint::Font) else {
            continue;
        };
        let base: usize = mesh.parts[..font].iter().map(|p| p.vertices.len()).sum();
        let g = geometry(&mut map, &bytes, got.dev_render.unwrap());
        let uv = &g.sections[0].vertices.texcoords;
        for fp in &mesh.plates {
            plates += 1;
            let want = dev_texture::label::rows(&mesh::label_name(def.shape), fp.width, fp.height);
            let mut read = Vec::new();
            for row in 0..dev_texture::label::ROWS {
                let mut text = String::new();
                for col in 0..dev_texture::label::COLUMNS {
                    let at = base + fp.first_vertex + 4 * (row * dev_texture::label::COLUMNS + col);
                    let (u, v) = (0..4).fold((0.0, 0.0), |(u, v), k| (u + uv[at + k][0] / 4.0, v + uv[at + k][1] / 4.0));
                    let per = dev_texture::font::CELLS_PER_ROW as f32;
                    let index = (v * per) as u32 * dev_texture::font::CELLS_PER_ROW + (u * per) as u32;
                    text.push(dev_texture::font::read_cell(&atlas, index as u8).unwrap_or('?'));
                }
                read.push(text.trim().to_owned());
            }
            assert_eq!(read, want, "{}: face {}", def.name(), fp.face);
            assert_eq!(fp.plate.text().to_vec(), want);
        }
    }
    assert!(plates > 24, "only {plates} plates");
}

#[test]
fn a_plate_is_cut_out_of_its_face_and_keeps_its_area() {
    let def = PieceDef::dev(Shape::Block, [256, 256, 256], 0);
    let solid = def.generate().unwrap();
    let mesh = mesh::build(&def, &solid, View::Dev);
    assert_eq!(mesh.plates.len(), 6, "a plate on every face of a big block");
    let area = |p: &mesh::Part| -> f64 {
        p.triangles
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| p.vertices[usize::from(i)].position.map(f64::from));
                let n = cross(sub(b, a), sub(c, a));
                dot(n, n).sqrt() / 2.0
            })
            .sum()
    };
    let total: f64 = mesh.parts.iter().map(area).sum();
    assert!((total - 6.0 * 2.56 * 2.56).abs() < 1e-3, "{total}");
    // Every triangle faces out of the block's middle.
    for p in &mesh.parts {
        for t in &p.triangles {
            let [a, b, c] = t.map(|i| p.vertices[usize::from(i)].position.map(f64::from));
            let n = cross(sub(b, a), sub(c, a));
            let mid: V = std::array::from_fn(|k| (a[k] + b[k] + c[k]) / 3.0 - 1.28);
            assert!(dot(n, mid) > 0.0, "a triangle faces in");
        }
    }
}

#[test]
fn a_nodraw_role_is_in_the_coll_and_not_the_mode() {
    let def = with(
        Shape::Block,
        [64, 64, 64],
        0,
        Materials::Each([Material::Dev, Material::Dev, Material::Nodraw, Material::Dev]),
    );
    let (bytes, installed) = built(std::slice::from_ref(&def));
    let got = &installed[0];
    let mut map = open(&bytes);
    let coll = Collision::read(&mut map, got.collision).unwrap();
    assert_eq!(coll.regions[0].permutations[0].bsps[0].surfaces.len(), 6);
    let g = geometry(&mut map, &bytes, got.render.unwrap());
    let s = &g.sections[0];
    let p = &s.vertices.positions;
    for part in &s.geometry.parts {
        for t in s.geometry.part_triangles(part) {
            let [a, b, c] = t.map(|i| p[usize::from(i)].map(f64::from));
            let n = cross(sub(b, a), sub(c, a));
            assert!(n[2] >= 0.0, "a face of the Nodraw underside is drawn");
        }
    }
    // Its dev twin draws the underside.
    assert_ne!(got.render, got.dev_render);
    let dev = geometry(&mut map, &bytes, got.dev_render.unwrap());
    let s = &dev.sections[0];
    let p = &s.vertices.positions;
    let down = s.geometry.parts.iter().any(|part| {
        s.geometry.part_triangles(part).iter().any(|t| {
            let [a, b, c] = t.map(|i| p[usize::from(i)].map(f64::from));
            cross(sub(b, a), sub(c, a))[2] < 0.0
        })
    });
    assert!(down);
}

#[test]
fn a_barrier_has_no_mode() {
    let def = with(Shape::Block, [128, 16, 128], 0, Materials::All(Material::Clip));
    let (bytes, installed) = built(std::slice::from_ref(&def));
    let got = &installed[0];
    assert_eq!((got.render, got.dev_render), (None, None));
    let mut map = open(&bytes);
    assert!(
        !map.tags()
            .iter()
            .any(|t| t.group == FourCC(*b"mode") && t.name.as_deref() == Some(def.tag_path().as_str())),
        "a barrier has a render model"
    );
    let hlmt = map.read_tag_bytes(got.model).unwrap();
    assert_eq!(TagId(word(&hlmt, 4)), TagId::NULL, "the hlmt names no render model");
    assert!(holds(&hlmt, got.collision) && holds(&hlmt, got.physics));
    // It still collides.
    let coll = Collision::read(&mut map, got.collision).unwrap();
    assert_eq!(coll.regions[0].permutations[0].bsps[0].surfaces.len(), 6);
    // And it adds no dev set.
    assert!(!map.tags().iter().any(|t| t.name.as_deref() == Some(dev::font_path().as_str())));
}

#[test]
fn a_map_material_draws_with_the_levels_shader_and_shares_the_dev_twin() {
    let rock = Material::Map(TagKey::shader(ROCK_SHADER));
    let floor = with(
        Shape::Block,
        [128, 128, 128],
        0,
        Materials::Each([rock.clone(), Material::Dev, Material::Dev, Material::Dev]),
    );
    let walls = with(
        Shape::Block,
        [128, 128, 128],
        0,
        Materials::Each([Material::Dev, rock.clone(), Material::Dev, Material::Dev]),
    );
    let (bytes, installed) = built(&[floor, walls]);
    assert_ne!(installed[0].render, installed[1].render);
    assert_eq!(installed[0].dev_render, installed[1].dev_render, "one dev twin");
    let mut map = open(&bytes);
    let rock_id = map
        .tags()
        .iter()
        .find(|t| t.group == FourCC(*b"shad") && t.name.as_deref() == Some(ROCK_SHADER))
        .unwrap()
        .id;
    let look = map.read_tag_bytes(installed[0].render.unwrap()).unwrap();
    assert!(holds(&look, rock_id), "the look names the level's shader");
    let twin = map.read_tag_bytes(installed[0].dev_render.unwrap()).unwrap();
    assert!(!holds(&twin, rock_id), "the dev twin is all dev");
}

#[test]
fn a_map_material_the_base_lacks_is_refused_by_name_and_adds_nothing() {
    let (facts, mut set) = start();
    let def = with(
        Shape::Block,
        [64, 64, 64],
        0,
        Materials::All(Material::Map(TagKey::shader(r"invented\shaders\none"))),
    );
    let slots = set.slots.len();
    let e = install(&mut set, &def, &facts).unwrap_err();
    assert!(format!("{e:#}").contains(r"no shader invented\shaders\none"), "{e:#}");
    assert_eq!(set.slots.len(), slots);
}

#[test]
fn a_definition_already_in_the_set_is_refused() {
    let (facts, mut set) = start();
    let def = PieceDef::dev(Shape::Ramp, [128, 128, 64], 0);
    install(&mut set, &def, &facts).unwrap();
    let e = install(&mut set, &def, &facts).unwrap_err().to_string();
    assert!(e.contains("already has"), "{e}");
}

// ---------------------------------------------------------------------------
// The whole set
// ---------------------------------------------------------------------------

#[test]
fn every_generated_tag_decodes_through_tags() {
    let defs = forge_shapes::starter();
    let (bytes, installed) = built(&defs);
    let mut map = open(&bytes);
    let ctx = tags::object::Context::read(&mut map).unwrap();
    for got in &installed {
        let bloc = ctx.decode(got.crate_tag).expect("the bloc decodes");
        assert_eq!(bloc.group, FourCC(*b"bloc"));
        ctx.decode(got.model).expect("the hlmt decodes");
        let bloc_bytes = map.read_tag_bytes(got.crate_tag).unwrap();
        assert!(holds(&bloc_bytes, got.model), "bloc -> hlmt");
        let hlmt = map.read_tag_bytes(got.model).unwrap();
        assert_eq!(Some(TagId(word(&hlmt, 4))), got.render, "hlmt -> mode");
    }
    let meta = tags::shader::Metadata::read(&mut map).unwrap();
    let stem = map
        .tags()
        .iter()
        .find(|t| t.group == FourCC(*b"stem"))
        .unwrap()
        .id;
    let dev_tags: Vec<_> = map
        .tags()
        .iter()
        .filter(|t| t.name.as_deref().is_some_and(|n| n.starts_with(crate::blockout::data::data().dev_folder.as_str())))
        .map(|t| (t.id, t.group, t.location.unwrap().address))
        .collect();
    assert_eq!(dev_tags.len(), 10, "four tiles and the font, a bitm and a shad each");
    for (id, group, address) in dev_tags {
        match &group.0 {
            b"shad" => {
                let s = tags::shader::ShaderInstance::decode(&meta, address).expect("the shad decodes");
                assert_eq!(s.template.id, stem);
            }
            b"bitm" => {
                let d = tags::DecodedTag::read(&mut map, id).expect("the bitm decodes");
                let tags::TagData::Bitmap(b) = d.data else {
                    panic!("not a bitmap")
                };
                let side = b.images[0].dimensions[0];
                assert!(side == 512 || side == 128, "{side}");
            }
            g => panic!("a dev tag of group {}", FourCC(*g)),
        }
    }
    assert_eq!(
        Facts::read(&mut map).unwrap().material,
        (MATERIALS[1].to_owned(), 1)
    );
}

#[test]
fn two_installs_are_byte_equal() {
    let defs = forge_shapes::starter();
    let (a, _) = built(&defs);
    let (b, _) = built(&defs);
    assert!(a == b, "two builds of one set of definitions differ");
}

#[test]
fn the_cost_equals_the_bytes_added() {
    let (facts, mut set) = start();
    let mut ranges = Vec::new();
    for def in forge_shapes::starter().iter().take(6) {
        let (slots, raw) = (set.slots.len(), set.raw.len());
        let got = install(&mut set, def, &facts).unwrap();
        ranges.push((slots..set.slots.len(), raw..set.raw.len(), got.cost));
    }
    // The first install brought the dev set.
    assert!(ranges[0].2.tags > ranges[1].2.tags);
    let (_, layout) = set.build().unwrap();
    for (slots, raw, cost) in ranges {
        assert_eq!(cost.tags as usize, slots.len());
        let placed: u64 = slots.clone().map(|i| u64::from(layout.tags[i].1)).sum();
        assert_eq!(cost.tag_memory, placed, "the builder placed other sizes");
        let raw_bytes: u64 = set.raw[raw].iter().map(|r| r.bytes.len() as u64).sum();
        assert_eq!(cost.raw, raw_bytes);
        for i in slots {
            if let Some(map::build::TagPayload::Built(root)) = set.slots[i].tag.as_ref().map(|t| &t.payload) {
                assert_eq!(tag_size(root), u64::from(layout.tags[i].1));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The dev set
// ---------------------------------------------------------------------------

#[test]
fn the_dev_rasters_round_trip_through_the_cache() {
    let rasters = dev::encoded().unwrap();
    assert_eq!(dev::read_cache(&dev::write_cache(rasters)).as_ref(), Some(rasters));
    let mut stale = dev::write_cache(rasters);
    stale[8] ^= 1;
    assert_eq!(dev::read_cache(&stale), None, "another version's cache is not read");
    let dir = std::env::temp_dir().join(format!(
        "blockout-kit-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let first = dev::encoded_in(&dir).unwrap();
    assert!(dev::cache_path(&dir).is_file());
    let second = dev::encoded_in(&dir).unwrap();
    assert_eq!(first, second);
    assert_eq!(&first, rasters);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_dev_block_uses_one_tile_a_role_and_the_font() {
    let def = PieceDef::dev(Shape::Stairs, [256, 64, 128], 8);
    let solid = def.generate().unwrap();
    let mesh = mesh::build(&def, &solid, View::Look);
    let paints: Vec<_> = mesh.parts.iter().map(|p| p.paint.clone()).collect();
    for p in &paints {
        assert!(matches!(p, Paint::Tile(_) | Paint::Font), "{p:?}");
    }
    assert!(paints.contains(&Paint::Tile(Role::Trim)), "the risers are trim");
    assert!(paints.contains(&Paint::Tile(Role::Floor)), "the treads are floor");
}

mod manifest {
    use crate::blockout::data::{BlockoutData, SOURCE, data};

    /// Every value `content/blockout/kit.yaml` holds is the constant the
    /// builders had before it was data, so the built tags are the same bytes.
    #[test]
    fn manifest_is_the_retired_constants() {
        let d = data();
        assert_eq!(d.item, "blockout");
        assert_eq!(d.node, "piece");
        assert_eq!(d.region, "default");
        assert_eq!(d.crate_type, 11);
        assert_eq!(d.collision_material, "blockout");
        assert_eq!(d.dev_folder, r"forge\blockout\dev");
        assert_eq!((d.physics.flags, d.physics.motion, d.physics.mass), (0, 5, 1000.0));
        assert_eq!(d.palette.max_tile, 1 << 15);
        assert_eq!(
            d.palette.rock_and_ground,
            [
                "rock", "stone", "boulder", "cliff", "dirt", "ground", "terrain", "sand", "gravel",
                "grass", "mud", "snow", "soil"
            ]
        );
    }

    #[test]
    fn an_unknown_or_missing_key_is_refused() {
        let extra = format!("{SOURCE}\nmystery: 1\n");
        assert!(BlockoutData::parse(&extra).is_err(), "an unknown key");
        let nested = SOURCE.replacen("  mass: 1000.0", "  mass: 1000.0\n  extra: 1", 1);
        assert_ne!(nested, SOURCE);
        assert!(BlockoutData::parse(&nested).is_err(), "an unknown nested key");
        let missing = SOURCE.replacen("node: piece\n", "", 1);
        assert_ne!(missing, SOURCE);
        assert!(BlockoutData::parse(&missing).is_err(), "a missing key");
        let wrong = SOURCE.replacen("item: blockout", "item: other", 1);
        assert!(BlockoutData::parse(&wrong).is_err(), "another item");
    }
}
