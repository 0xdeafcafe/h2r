//! Blood Gulch's ring (`$H1_ISO`, `$H2_DISC`): Blood Gulch converted onto
//! Coagulation, and its sky's ring checked the way the engine draws it.
//!
//! Halo 1 draws the ring with `sky\clear afternoon\shaders\ring clear
//! afternoon`, a transparent generic shader (`sotr`) with framebuffer blend
//! 0 (alpha blend), no stages and one map, `sky\ring\ring clear afternoon`
//! (DXT3, 512 × 512) at scale 1: an atlas of the ring's lit inner face, its
//! dark sides and the haze where it meets the horizon, cut out by its alpha.
//! The converted shader is the base's `sky_one_alpha_env` with that bitmap
//! on stage 3, alpha blended (source one, destination one minus source
//! alpha, over `t3.rgb × t3.a`): the same picture and blend.
//!
//! The sky draw (`0x0004c690`) draws every part of the sky's model as a
//! triangle strip (`0x0004c974` pushes primitive 6), whatever the part's
//! flags. Until converter version 93 the sky's parts were triangle lists:
//! drawn as strips, each triangle was joined to the next two, and the
//! ring's long triangles, spanning the haze, the inner face and the
//! sides, drew as white slabs smeared across the sky. The ring is the sky
//! piece where that shows; the dome's extra triangles lie on the dome.
//!
//! Only names, counts and positions are printed.
//!
//! H1_ISO=disc/halo1.iso H2_DISC=disc/halo2.iso cargo test --release -p h1convert --test bloodgulch_ring -- --ignored --nocapture

use std::fs::File;
use std::io::{BufReader, Cursor};

use h1convert::convert::{Sandbox, convert, known};
use h1convert::sky::{MATERIAL_SHADER, MODE_MATERIAL, MODE_MATERIALS};
use h1convert::sky_shaders::{self, Recipe, recipe};
use h1map::H1Map;
use map::{FourCC, Map, TagId};
use tags::geometry::TagView;
use tags::material::{ShaderLibrary, TextureSource};
use tags::model::RenderModelGeometry;

const RING_SHADER: &str = "sky\\clear afternoon\\shaders\\ring clear afternoon";
const RING_BITMAP: &str = "sky\\ring\\ring clear afternoon";

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

fn named<R: std::io::Read + std::io::Seek>(map: &Map<R>, group: &[u8; 4], name: &str) -> TagId {
    map.tags()
        .iter()
        .find(|t| t.group == FourCC(*group) && t.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("no {} {name}", String::from_utf8_lossy(group)))
        .id
}

fn name_of<R: std::io::Read + std::io::Seek>(map: &Map<R>, id: TagId) -> String {
    map.tag(id)
        .ok()
        .and_then(|t| t.name.clone())
        .unwrap_or_else(|| format!("{id}"))
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn blood_gulchs_ring_draws_as_halo_1_draws_it() {
    let h1_iso = std::env::var("H1_ISO").expect("set H1_ISO");
    let h2_iso = std::env::var("H2_DISC").expect("set H2_DISC");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&h1_iso).unwrap())).unwrap();
    let disc = h1map::disc::Disc::open(&mut iso).unwrap();
    let e = disc
        .map("bloodgulch")
        .cloned()
        .expect("Blood Gulch on the disc");
    let file = iso.read(&e).unwrap();
    let sha = content::Sha256::of(&file);
    let h1 = H1Map::from_reader(Cursor::new(file)).unwrap();
    let mut h2 = xiso::Xiso::open(BufReader::new(File::open(&h2_iso).unwrap())).unwrap();
    let base_name = format!("{}.map", known("bloodgulch").unwrap().1);
    let entry = h2
        .entries()
        .unwrap()
        .into_iter()
        .find(|e| e.path.rsplit(['/', '\\']).next() == Some(base_name.as_str()))
        .expect("the base on the Halo 2 disc");
    let mut base = Map::from_reader(Cursor::new(h2.read(&entry).unwrap())).unwrap();
    let done = convert(&h1, sha, &mut base, Sandbox::Halo2).unwrap();
    let mut map = Map::from_reader(Cursor::new(done.bytes.clone())).unwrap();

    // Halo 1's side: the level's sky, its model, the ring's shader.
    let skies = h1convert::sky::read(&h1).unwrap();
    let sky = skies.skies.first().expect("Blood Gulch has a sky");
    let (model_name, _) = sky.model.as_ref().expect("its sky has a model");
    let k = sky
        .shaders
        .iter()
        .position(|s| s.as_ref().is_some_and(|s| s.name == RING_SHADER))
        .expect("the sky model draws the ring");
    let ring = sky.shaders[k].as_ref().unwrap();
    assert!(!ring.chicago, "a transparent generic shader");
    assert!(!ring.additive, "framebuffer blend 0: alpha blended");
    assert_eq!(ring.maps.len(), 1);
    assert_eq!(ring.maps[0].scale, [1.0, 1.0]);
    assert!(ring.maps[0].alpha, "its alpha is the shader's");
    assert_eq!(h1.tag(ring.maps[0].bitmap).unwrap().name, RING_BITMAP);
    assert_eq!(recipe(ring), Some(Recipe::Picture(0)));
    assert_eq!(ring.opaque_picture(), None, "drawn with its own alpha");

    // The converted model's material k is the ring's shader.
    let model = named(&map, b"mode", model_name);
    let shad = named(&map, b"shad", RING_SHADER);
    let tag = map.tag(model).unwrap().clone();
    let bytes = map.read_tag_bytes(model).unwrap();
    let view = TagView::new(&bytes, tag.location.unwrap().address);
    let root = view.at(0, MODE_MATERIALS + 8).unwrap();
    let materials = view.block(root, MODE_MATERIALS, MODE_MATERIAL).unwrap();
    let at = MATERIAL_SHADER + 4;
    let material = u32::from_le_bytes(materials[k][at..at + 4].try_into().unwrap());
    assert_eq!(TagId(material), shad, "material {k} draws the ring");

    // The ring's shader: the sky picture template, Halo 1's bitmap on the
    // stage the picture is drawn from, the template's defaults elsewhere,
    // alpha blended.
    let mut lib = ShaderLibrary::read(&mut map).unwrap();
    let instance = lib.shader(shad).unwrap();
    assert_eq!(
        name_of(&map, instance.template.id),
        sky_shaders::ALPHA_TEMPLATE
    );
    let m = lib.material(shad, None).unwrap();
    let imp = &m.layers[0].passes[0].implementations[0];
    let stages: Vec<String> = imp
        .textures
        .iter()
        .map(|t| match t.map(|t| t.source) {
            Some(TextureSource::Bitmap { bitmap, .. }) => name_of(&map, bitmap),
            other => format!("{other:?}"),
        })
        .collect();
    println!("{RING_SHADER}: stages {stages:?}");
    assert_eq!(
        stages,
        [
            "shaders\\default_bitmaps\\bitmaps\\default_additive_cube_map",
            "shaders\\default_bitmaps\\bitmaps\\default_multiplicative",
            "shaders\\default_bitmaps\\bitmaps\\default_additive",
            RING_BITMAP,
        ]
    );
    let state = |name: &str| {
        imp.render_states
            .iter()
            .find(|r| tags::shader::render_state_name(r.state) == Some(name))
            .map(|r| r.value)
    };
    assert_eq!(state("ALPHABLENDENABLE"), Some(1));
    assert_eq!(state("SRCBLEND"), Some(1), "source one: t3.rgb × t3.a");
    assert_eq!(state("DESTBLEND"), Some(0x303), "one minus source alpha");
    assert_eq!(state("ZWRITEENABLE"), Some(0));

    // The sky's model: every part a strip, as the sky draw draws it, and
    // each strip is Halo 1's triangles, corner for corner, wound alike.
    let mut h1_model = h1models::model_from_map(
        &h1,
        h1.tag(sky.sky.model.id).unwrap(),
        &h1models::mode::ModelOptions::default(),
    )
    .unwrap()
    .0;
    h1sky::convert::drop_edge_on(&mut h1_model);
    let g = RenderModelGeometry::read(&mut map, model, &mut Mem(done.bytes.clone())).unwrap();
    assert_eq!(g.sections.len(), h1_model.geometries.len());
    let mut triangles = 0;
    for (si, (s, h)) in g.sections.iter().zip(&h1_model.geometries).enumerate() {
        assert_eq!(s.geometry.parts.len(), h.parts.len(), "section {si}");
        for (pi, (p, hp)) in s.geometry.parts.iter().zip(&h.parts).enumerate() {
            assert!(
                !p.is_triangle_list(),
                "section {si} part {pi}: a list, which the sky draw draws as a strip"
            );
            let got = s.geometry.part_triangles(p);
            let want = h1models::h1::strip_triangles(&hp.strip);
            assert_eq!(got.len(), want.len(), "section {si} part {pi}: triangles");
            for (a, b) in got.iter().zip(&want) {
                // Section-local vertices (the converter offsets each part's)
                // against the part's own.
                for c in 0..3 {
                    let q = s.vertices.positions[usize::from(a[c])];
                    let r = hp.vertices[usize::from(b[c])].position;
                    let d = (0..3).map(|x| (q[x] - r[x]).abs()).fold(0.0, f32::max);
                    // s16 positions over the model's bounds (about 200,000
                    // units on this sky): steps of about 3.
                    assert!(d < 4.0, "section {si} part {pi}: corner {q:?} for {r:?}");
                }
            }
            triangles += got.len();
        }
    }
    println!(
        "{model_name}: {} sections, {triangles} triangles, every part a strip",
        g.sections.len()
    );
}
