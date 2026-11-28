//! The stage's own tags, added to a tag set: the grid and ruler bitmaps,
//! their two shaders, the lightmap bitmap ([`crate::stage::light`]), the sky
//! ([`crate::stage::sky`]: its picture, shader, model and tag, named by the
//! scenario), and the structure BSP and lightmap written over the two
//! placeholder tags the set holds for them.
//!
//! What the tags take from the base is read first ([`Facts::read`]) from
//! the base's tags, never from a figure in the repository: the player's
//! size from the multiplayer biped, the floor's global material by name
//! from the globals, the opaque stem and the default bitmaps the stem
//! binds, with each default bitmap's LOD offset from its size.
//!
//! The structure is written by `h1convert::structure::Structure::write`,
//! the writer every converted Halo 1 level goes through: one cluster, the
//! render geometry of [`crate::stage::mesh`] packed into sections of one lightmap
//! page, the collision compiled from the same faces with its MOPP, the
//! stage's sky (cluster sky 0), no portals and no ambient sound.

use std::collections::HashMap;

use h1convert::clusters::Visibility;
use h1convert::shad::{self, Bound, Fixed, ILLUM, Library, Role, TEX_BUMP};
use h1convert::structure::{
    self, COLLISION_MATERIAL, Mesh as Part, Sound, Structure, Templates, Vertex,
};
use h1map::bitmap::{
    Bitmap, FLAG_POWER_OF_TWO, FLAG_SWIZZLED, IMAGE_LEN, Image, ImageKind, PixelFormat, Sequence,
};
use map::build::{TagPayload, TagSet};
use map::{FourCC, TagId};
use tags::repo::{LooseTag, TagKey, TagSource, Tree, Value};

use crate::stage::layout::{Layout, Player};
use crate::stage::light::{self, Baked};
use crate::stage::mesh::{Mesh, Surface, V3};
use crate::stage::sky;
use crate::stage::texture::{self, Image as Picture};
use crate::stage::{Error, Result};

pub const BITM: FourCC = FourCC(*b"bitm");
pub const SHAD: FourCC = FourCC(*b"shad");
pub const SBSP: FourCC = FourCC(*b"sbsp");
pub const LTMP: FourCC = FourCC(*b"ltmp");
pub const SCNR: FourCC = FourCC(*b"scnr");
pub const STEM: FourCC = FourCC(*b"stem");
pub const BIPD: FourCC = FourCC(*b"bipd");
pub const SKY: FourCC = FourCC(*b"sky ");
pub const MODE: FourCC = FourCC(*b"mode");

/// The multiplayer player's biped, whose size the scale references take
/// (`biped` in `content/stage/kit.yaml`).
pub fn biped() -> &'static str {
    &crate::stage::data::data().biped
}
/// `bipd +0x268`: standing height (tags/src/defs/unit.txt).
const HEIGHT_STANDING: usize = 0x268;
/// `bipd +0x270`: radius (tags/src/defs/unit.txt).
const RADIUS: usize = 0x270;

/// The global material every stage surface is made of: footsteps, impacts
/// and decals as on stone (`floor_material` in `content/stage/kit.yaml`).
pub fn floor_material() -> &'static str {
    &crate::stage::data::data().floor_material
}
/// `matg +0x150`: the global materials, `0xb4` each, the name's string ID
/// at `+0` (`h1convert::materials::GLOBAL_MATERIALS`).
const GLOBAL_MATERIALS: u32 = 0x150;
const GLOBAL_MATERIAL: u32 = 0xb4;

/// `bitm +0x44`: the images; an image's width and height at `+4`, `+6`.
const BITMAP_IMAGES: u32 = 0x44;

/// The default bitmaps [`TEX_BUMP`] binds where a shader gives none: the
/// flat bump, the base colour, the detail and the fade ramp.
pub const FIXED: [Fixed; 4] = [
    Fixed::DefaultVector,
    Fixed::ColorWhite,
    Fixed::DefaultDetail,
    Fixed::CornerFade,
];

/// Where the stage's own tags live, beside the scenario.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keys {
    pub scenario: TagKey,
    pub sbsp: TagKey,
    pub ltmp: TagKey,
    /// The lightmap bitmap.
    pub lightmap: TagKey,
    /// The sky: its tag, model, picture and shader.
    pub sky: TagKey,
    pub sky_model: TagKey,
    pub sky_bitmap: TagKey,
    pub sky_shader: TagKey,
    /// Per [`Surface`], its bitmap and shader.
    pub bitmaps: [TagKey; 2],
    pub shaders: [TagKey; 2],
}

impl Keys {
    /// The keys of a stage whose scenario is at `path`.
    pub fn new(path: &str) -> Keys {
        let dir = path.rsplit_once('\\').map_or("", |(d, _)| d);
        let at = |what: &str| {
            if dir.is_empty() {
                what.to_owned()
            } else {
                format!("{dir}\\{what}")
            }
        };
        let surface = |group: FourCC, kind: &str| {
            Surface::ALL.map(|s| TagKey::new(group, at(&format!("{kind}\\{}", s.name()))))
        };
        Keys {
            scenario: TagKey::new(SCNR, path),
            sbsp: TagKey::new(SBSP, path),
            ltmp: TagKey::new(LTMP, format!("{path}_lightmap")),
            lightmap: TagKey::new(BITM, format!("{path}_lightmap")),
            sky: TagKey::new(SKY, at("sky\\stage")),
            sky_model: TagKey::new(MODE, at("sky\\stage")),
            sky_bitmap: TagKey::new(BITM, at("sky\\stage")),
            sky_shader: TagKey::new(SHAD, at("sky\\stage")),
            bitmaps: surface(BITM, "bitmaps"),
            shaders: surface(SHAD, "shaders"),
        }
    }

    /// Every key the stage adds, in the order [`install`] adds them, the
    /// scenario, structure and lightmap first.
    pub fn all(&self) -> Vec<TagKey> {
        let mut out = vec![self.scenario.clone(), self.sbsp.clone(), self.ltmp.clone()];
        for i in 0..Surface::ALL.len() {
            out.push(self.bitmaps[i].clone());
            out.push(self.shaders[i].clone());
        }
        out.push(self.lightmap.clone());
        out.extend([
            self.sky_bitmap.clone(),
            self.sky_shader.clone(),
            self.sky_model.clone(),
            self.sky.clone(),
        ]);
        out
    }
}

/// What the stage takes from the base's tags.
#[derive(Clone, Debug, PartialEq)]
pub struct Facts {
    pub player: Player,
    /// [`floor_material`]'s index in the globals.
    pub material: u16,
    /// The opaque stem.
    pub stem: TagKey,
    /// The self-illuminated stem the sky's dome is drawn with: it takes
    /// no light from a lightmap.
    pub illum: TagKey,
    /// Each of [`FIXED`]: its bitmap and LOD offset.
    pub fixed: Vec<(Fixed, TagKey, f32)>,
}

fn read_tree(src: &mut dyn TagSource, key: &TagKey) -> Result<Tree> {
    let tag: LooseTag = src
        .read(key)?
        .ok_or_else(|| Error::Base(format!("the base has no {key}")))?;
    match tag.body {
        tags::repo::Body::Tree(t) => Ok(t),
        tags::repo::Body::Alias { owner, .. } => Err(Error::Base(format!(
            "{key} is an alias of {owner}, not a tag of its own"
        ))),
    }
}

fn child(tree: &Tree, node: u32, offset: u32) -> Option<u32> {
    tree.nodes
        .get(node as usize)?
        .fields
        .iter()
        .find_map(|(o, v)| match v {
            Value::Block(c) if *o == offset => Some(*c),
            _ => None,
        })
}

fn f32_at(bytes: &[u8], at: usize) -> Option<f32> {
    Some(f32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

impl Facts {
    /// Reads the facts (module docs) from `src`, whose globals are
    /// `globals`. Anything missing is an error naming it.
    pub fn read(src: &mut dyn TagSource, globals: &TagKey) -> Result<Facts> {
        Self::read_for(src, globals, biped())
    }

    /// [`Facts::read`] with the player's size taken from the biped at
    /// `biped`: a campaign base has the campaign biped, not
    /// [`biped`] (starter-map, docs/editor/editor.md, "New maps").
    pub fn read_for(src: &mut dyn TagSource, globals: &TagKey, biped: &str) -> Result<Facts> {
        let biped = TagKey::new(BIPD, biped);
        let tree = read_tree(src, &biped)?;
        let root = &tree.nodes[0].bytes;
        let (Some(height), Some(radius)) = (f32_at(root, HEIGHT_STANDING), f32_at(root, RADIUS))
        else {
            return Err(Error::Base(format!(
                "{biped}: the root is too short for its size"
            )));
        };
        if !(height > 0.0 && radius > 0.0) {
            return Err(Error::Base(format!(
                "{biped}: standing height {height} and radius {radius} are not a size"
            )));
        }
        let player = Player { height, radius };

        let matg = read_tree(src, globals)?;
        let list = child(&matg, 0, GLOBAL_MATERIALS)
            .ok_or_else(|| Error::Base(format!("{globals}: no global materials")))?;
        let material = matg.nodes[list as usize]
            .fields
            .iter()
            .find_map(|(o, v)| match v {
                Value::String(s) if s == floor_material() && o % GLOBAL_MATERIAL == 0 => {
                    Some(o / GLOBAL_MATERIAL)
                }
                _ => None,
            })
            .ok_or_else(|| {
                Error::Base(format!("{globals}: no global material {:?}", floor_material()))
            })?;
        let material = u16::try_from(material)
            .map_err(|_| Error::Base(format!("{globals}: global material {material}")))?;

        let stem = TagKey::new(STEM, TEX_BUMP.stem);
        if src.read(&stem)?.is_none() {
            return Err(Error::Base(format!("the base has no {stem}")));
        }
        let illum = TagKey::new(STEM, ILLUM.stem);
        if src.read(&illum)?.is_none() {
            return Err(Error::Base(format!("the base has no {illum}")));
        }
        let mut fixed = Vec::new();
        for f in FIXED {
            let key = TagKey::new(BITM, f.path());
            let tree = read_tree(src, &key)?;
            let images = child(&tree, 0, BITMAP_IMAGES)
                .ok_or_else(|| Error::Base(format!("{key}: no images")))?;
            let first = &tree.nodes[images as usize].bytes;
            let (Some(w), Some(h)) = (u16_at(first, 4), u16_at(first, 6)) else {
                return Err(Error::Base(format!("{key}: no first image")));
            };
            fixed.push((f, key, shad::lod_offset(w, h)));
        }
        Ok(Facts {
            player,
            material,
            stem,
            illum,
            fixed,
        })
    }
}

/// What [`install`] added.
#[derive(Clone, Debug, PartialEq)]
pub struct Installed {
    pub sbsp: TagId,
    pub ltmp: TagId,
    /// The BSP section [`Structure::write`] added.
    pub section: usize,
    pub bitmaps: [TagId; 2],
    pub shaders: [TagId; 2],
    pub lightmap: TagId,
    /// The lightmap page's side, texels, and its texels to the unit on a
    /// day face ([`light::Baked`]).
    pub lightmap_side: u32,
    pub lightmap_density: f32,
    /// The sky, its model, picture and shader.
    pub sky: TagId,
    pub sky_model: TagId,
    pub sky_bitmap: TagId,
    pub sky_shader: TagId,
    /// Render triangles and vertices, collision surfaces.
    pub triangles: usize,
    pub vertices: usize,
    pub surfaces: usize,
}

fn fail(what: &str, e: impl std::fmt::Display) -> Error {
    Error::Build(format!("{what}: {e}"))
}

/// A picture of ours as a DXT5 `bitm` (`crate::texture::bitmap`).
fn colour_bitmap(img: &Picture) -> Result<h1_assets::models::Converted> {
    crate::texture::bitmap(&crate::texture::Rgba {
        width: img.width,
        height: img.height,
        pixels: img.pixels.clone(),
    })
    .map_err(|e| fail("a stage texture", e))
}

/// The baked lightmap page ([`light::bake`]) as a `bitm` laid out as a
/// converted Halo 1 level's lightmap is (`h1_assets::bitmaps::atlas::
/// compose`): one 2D image a page, 32-bit colour, swizzled, no mipmaps, a
/// sequence a page; usage 4 (light map), encoding 4 (32-bit colour).
fn lightmap_bitmap(baked: &Baked) -> Result<h1_assets::models::Converted> {
    let page = &baked.page;
    // X8R8G8B8 is B, G, R, A in memory; the Xbox stores the rows swizzled.
    let texels = gfx::swizzle::swizzle(&baked.texels(), page.width, page.height, 1, 4)
        .map_err(|e| fail("the lightmap bitmap", format!("{e:?}")))?;
    let image = Image {
        width: page.width as u16,
        height: page.height as u16,
        depth: 1,
        kind: ImageKind::D2,
        format: PixelFormat::X8R8G8B8,
        flags: FLAG_POWER_OF_TWO | FLAG_SWIZZLED,
        registration: [0, 0],
        mip_count: 0,
        pixel_offset: 0,
        pixel_size: texels.len() as u32,
        raw: [0; IMAGE_LEN],
    };
    let bitmap = Bitmap {
        kind: 0,
        encoding: 4,
        usage: 4,
        flags: 0,
        detail_fade: 0.0,
        sharpen: 0.0,
        bump_height: 0.0,
        sprite_budget: [0, 0],
        color_plate: [0, 0],
        blur: 0.0,
        alpha_bias: 0.0,
        mipmap_count: 0,
        sprite_usage: 0,
        sprite_spacing: 0,
        sequences: vec![Sequence {
            name: String::new(),
            first_image: 0,
            image_count: 1,
            sprites: Vec::new(),
        }],
        images: vec![image],
    };
    let (converted, _) = h1_assets::bitmaps::convert::convert_with(&bitmap, |_| Ok(texels.clone()))
        .map_err(|e| fail("the lightmap bitmap", e))?;
    Ok(converted)
}

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale(a: V3, k: f32) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}

fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn unit(a: V3) -> Option<V3> {
    let l = dot(a, a).sqrt();
    (l > 1e-6).then(|| scale(a, 1.0 / l))
}

/// A face's tangent frame from its first three corners and their
/// coordinates: the directions of increasing `u` (tangent) and `v`
/// (binormal), made square to the normal.
fn frame(points: &[V3], uvs: &[[f32; 2]], n: V3) -> (V3, V3) {
    let (e1, e2) = (sub(points[1], points[0]), sub(points[2], points[0]));
    let (du1, dv1) = (uvs[1][0] - uvs[0][0], uvs[1][1] - uvs[0][1]);
    let (du2, dv2) = (uvs[2][0] - uvs[0][0], uvs[2][1] - uvs[0][1]);
    let det = du1 * dv2 - du2 * dv1;
    let flat = |v: V3| sub(v, scale(n, dot(v, n)));
    let (t, b) = if det.abs() > 1e-12 {
        let r = 1.0 / det;
        (
            scale(sub(scale(e1, dv2), scale(e2, dv1)), r),
            scale(sub(scale(e2, du1), scale(e1, du2)), r),
        )
    } else {
        (e1, e2)
    };
    let t = unit(flat(t)).unwrap_or([1.0, 0.0, 0.0]);
    let b = unit(flat(b)).unwrap_or([0.0, 1.0, 0.0]);
    (t, b)
}

/// The render meshes, one a surface (its material index), every vertex
/// at its place on the lightmap page `baked` and lit from the direction
/// `baked` gives it.
pub fn render_meshes(mesh: &Mesh, baked: &Baked) -> Vec<Part> {
    use h1_assets::models::mode::pack_normal;
    Surface::ALL
        .iter()
        .map(|&s| {
            let mut vertices = Vec::new();
            let mut triangles = Vec::new();
            for (face, f) in mesh
                .faces
                .iter()
                .enumerate()
                .filter(|(_, f)| f.surface == s)
            {
                let (t, b) = frame(&f.points, &f.uvs, f.normal);
                let first = vertices.len() as u32;
                for (p, uv) in f.points.iter().zip(&f.uvs) {
                    vertices.push(Vertex {
                        position: *p,
                        frame: [pack_normal(f.normal), pack_normal(b), pack_normal(t)],
                        texcoord: *uv,
                        lightmap: baked.uv(face, *p),
                        incident: pack_normal(baked.incident(face, f.normal)),
                    });
                }
                for k in 1..f.points.len() as u32 - 1 {
                    triangles.push([first, first + k, first + k + 1]);
                }
            }
            Part {
                shader: s.name().to_owned(),
                page: Some(0),
                cluster: 0,
                vertices,
                triangles,
            }
        })
        .collect()
}

/// FNV-1a over the positions: the structure checksum.
fn checksum(sections: &[structure::Section]) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for s in sections {
        for v in &s.vertices {
            for c in v.position {
                for b in c.to_le_bytes() {
                    h ^= u32::from(b);
                    h = h.wrapping_mul(0x0100_0193);
                }
            }
        }
    }
    h
}

/// Adds the stage's own tags to `set` (module docs). `set` holds the
/// base's tags, the scenario and placeholders for `keys.sbsp` and
/// `keys.ltmp`.
pub fn install(set: &mut TagSet, layout: &Layout, facts: &Facts, keys: &Keys) -> Result<Installed> {
    let find = |set: &TagSet, k: &TagKey| {
        set.find(k.group, &k.path)
            .ok_or_else(|| Error::Build(format!("the set has no {k}")))
    };
    let sbsp = find(set, &keys.sbsp)?;
    let ltmp = find(set, &keys.ltmp)?;
    let stem = find(set, &facts.stem)?;
    let mut fixed = Vec::new();
    for (f, key, lod_offset) in &facts.fixed {
        fixed.push((
            *f,
            Bound {
                id: find(set, key)?,
                scale: [1.0, 1.0],
                lod_offset: *lod_offset,
            },
        ));
    }
    let illum = find(set, &facts.illum)?;
    let library = Library::new(&[(TEX_BUMP.stem, stem), (ILLUM.stem, illum)], &fixed);

    let pictures = [texture::grid(layout), texture::ruler()];
    let mut bitmaps = [TagId::NULL; 2];
    let mut shaders = [TagId::NULL; 2];
    set.add_group(SHAD, None, None);
    for (i, s) in Surface::ALL.iter().enumerate() {
        let pic = &pictures[i];
        let id = colour_bitmap(pic)?
            .install(set, &keys.bitmaps[i].path)
            .map_err(|e| fail(s.name(), e))?;
        bitmaps[i] = id;
        let maps = HashMap::from([(
            Role::Base,
            Bound {
                id,
                scale: [1.0, 1.0],
                lod_offset: shad::lod_offset(pic.width as u16, pic.height as u16),
            },
        )]);
        let tree = library
            .build(&TEX_BUMP, &maps)
            .map_err(|e| fail(s.name(), e))?;
        shaders[i] = set
            .add_tag(SHAD, &keys.shaders[i].path, TagPayload::Built(tree))
            .map_err(|e| fail(s.name(), e))?;
    }
    let mesh = Mesh::build(layout);
    let baked = light::bake(layout, &mesh)?;
    let lightmap = lightmap_bitmap(&baked)?
        .install(set, &keys.lightmap.path)
        .map_err(|e| fail("the lightmap bitmap", e))?;

    // The sky: its picture on a self-illuminated shader, the dome that
    // draws it, the tag, and the scenario's skies block naming it.
    let picture = sky::picture(layout);
    let sky_bitmap = colour_bitmap(&picture)?
        .install(set, &keys.sky_bitmap.path)
        .map_err(|e| fail("the sky's picture", e))?;
    let maps = HashMap::from([(
        Role::Base,
        Bound {
            id: sky_bitmap,
            scale: [1.0, 1.0],
            lod_offset: shad::lod_offset(picture.width as u16, picture.height as u16),
        },
    )]);
    let tree = library
        .build(&ILLUM, &maps)
        .map_err(|e| fail("the sky's shader", e))?;
    let sky_shader = set
        .add_tag(SHAD, &keys.sky_shader.path, TagPayload::Built(tree))
        .map_err(|e| fail("the sky's shader", e))?;
    let sky_model = sky::convert(sky_shader)?
        .install(set, &keys.sky_model.path)
        .map_err(|e| fail("the sky's model", e))?;
    let sky_tag = sky::tag(layout, sky_model)?
        .install(set, &keys.sky.path)
        .map_err(|e| fail("the sky", e))?;
    let scenario = find(set, &keys.scenario)?;
    match set.tag_mut(scenario).map(|t| &mut t.payload) {
        Some(TagPayload::Built(root)) => sky::name_sky(root, sky_tag),
        _ => {
            return Err(Error::Build(format!(
                "{} is not a tag the stage can name its sky in",
                keys.scenario
            )));
        }
    }

    let compiled = mesh.collision()?;
    let mopp = h1convert::convert::structure_mopp(&compiled.bsp)
        .map_err(|e| fail("the collision's MOPP", e))?;
    let parts = render_meshes(&mesh, &baked);
    let material: Vec<u16> = Surface::ALL.iter().map(|s| s.index()).collect();
    let sections = structure::pack(&parts, &material);

    let mut templates = Templates::default();
    for (i, _) in Surface::ALL.iter().enumerate() {
        let shader = shaders[i].0.to_le_bytes();
        let mut head = [0u8; 16];
        head[4..8].copy_from_slice(&TagId::NULL.0.to_le_bytes());
        head[8..12].copy_from_slice(&SHAD.as_u32().to_le_bytes());
        head[12..16].copy_from_slice(&shader);
        templates.material_heads.push(head);
        templates
            .part_kinds
            .push(structure::part_kind(TEX_BUMP.stem));
        let mut row = [0u8; COLLISION_MATERIAL];
        row[4..8].copy_from_slice(&TagId::NULL.0.to_le_bytes());
        row[8..10].copy_from_slice(&facts.material.to_le_bytes());
        row[10..12].copy_from_slice(&(-1i16).to_le_bytes());
        row[12..16].copy_from_slice(&SHAD.as_u32().to_le_bytes());
        row[16..20].copy_from_slice(&shader);
        templates.collision_materials.push(row);
    }
    let materials: Vec<usize> = (0..Surface::ALL.len()).collect();
    let mut bounds = mesh.bounds();
    for a in 0..3 {
        bounds[2 * a] -= 1.0;
        bounds[2 * a + 1] += 1.0;
    }
    let sound = Sound::default();
    let visibility = Visibility::default();
    let structure = Structure {
        collision: &compiled.bsp,
        collision_materials: Surface::ALL.len(),
        sections: &sections,
        materials: &materials,
        bounds,
        mopp: &mopp,
        checksum: checksum(&sections),
        templates: &templates,
        lightmap_bitmap: lightmap,
        sound: &sound,
        visibility: &visibility,
        atmosphere: &[],
    };
    let section = structure
        .write(set, sbsp, ltmp)
        .map_err(|e| fail("the structure", e))?;
    Ok(Installed {
        sbsp,
        ltmp,
        section,
        bitmaps,
        shaders,
        lightmap,
        lightmap_side: baked.side(),
        lightmap_density: baked.density,
        sky: sky_tag,
        sky_model,
        sky_bitmap,
        sky_shader,
        triangles: parts.iter().map(|p| p.triangles.len()).sum(),
        vertices: parts.iter().map(|p| p.vertices.len()).sum(),
        surfaces: compiled.bsp.surfaces.len(),
    })
}
