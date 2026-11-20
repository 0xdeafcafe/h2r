//! Layers of a level that are not solid level geometry but are drawn over
//! it or into the air: decals and overlays (stains, cracks, painted
//! letters and signs), and effect surfaces (glows, light shafts and
//! volumes, electric arcs, waterfalls and spray). `map.glb` keeps them in
//! their own `decals_<n>` and `effects_<n>` child nodes, textured, with
//! how each blends (docs/live/live-art.md, "Layers").
//!
//! What a surface is comes from generic rendering words in its shader
//! template's tag name, read at run time ([`Layer::from_template_words`]);
//! its picture is the shader's diffuse map, decoded with the same code as
//! the player models' textures. Nothing from the game is named here.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use assets::ResourceFiles;
use map::TagId;

use crate::live_geometry::{MapReader, to_gltf};
use crate::live_models::{self, BitmapRole, Image};

/// Which of the two layer nodes a surface goes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Group {
    /// On a level surface: drawn just in front of it.
    Decal,
    /// In the air: glows, light volumes, waterfalls.
    Effect,
}

/// How a layer is put over what is behind it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Blend {
    /// Over, by the texture's alpha.
    Alpha,
    /// Darkens what is behind by the texture's colour (white leaves it).
    Multiply,
    /// Twice the texture's colour times what is behind: mid-grey leaves
    /// it, darker darkens, lighter lightens (an opaque decal picture
    /// centred on grey, as stains, scorch marks and painted letters are).
    Double,
    /// Cut out where the texture's alpha is under a half; opaque elsewhere.
    Test,
    /// Added to what is behind (black adds nothing); no depth written.
    Additive,
}

/// A layer: its group and blend.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Layer {
    pub group: Group,
    pub blend: Blend,
}

impl Layer {
    pub const fn decal(blend: Blend) -> Layer {
        Layer {
            group: Group::Decal,
            blend,
        }
    }

    pub const fn effect(blend: Blend) -> Layer {
        Layer {
            group: Group::Effect,
            blend,
        }
    }

    /// The layer a shader template is, from the words of its last path
    /// component (lower case), or `None` when it is not a layer by its
    /// name alone:
    ///
    /// - named `decal...` or `overlay...` first: a decal. An `overlay`
    ///   multiplies (stains, cracks and painted marks are dark on white);
    ///   a template that says `test` is cut out by alpha; any other decal
    ///   blends by alpha;
    /// - any other template with the word `add`: an additive effect.
    pub fn from_template_words(words: &[&str]) -> Option<Layer> {
        let has = |w: &str| words.contains(&w);
        match words.first() {
            Some(&("decal" | "overlay")) => Some(Layer::decal(if has("overlay") {
                Blend::Multiply
            } else if has("test") {
                Blend::Test
            } else {
                Blend::Alpha
            })),
            _ if has("add") => Some(Layer::effect(Blend::Additive)),
            _ => None,
        }
    }

    /// `decal` or `effect` (material `extras.layer`).
    pub fn group_name(self) -> &'static str {
        match self.group {
            Group::Decal => "decal",
            Group::Effect => "effect",
        }
    }

    /// `alpha`, `multiply`, `test` or `additive` (material `extras.blend`).
    pub fn blend_name(self) -> &'static str {
        match self.blend {
            Blend::Alpha => "alpha",
            Blend::Multiply => "multiply",
            Blend::Double => "double",
            Blend::Test => "test",
            Blend::Additive => "additive",
        }
    }

    /// Whether a layer without a picture is still worth drawing: only a
    /// glow (a faint additive sheet). A decal or alpha card without its
    /// texture would be a solid slab.
    pub fn drawn_without_texture(self) -> bool {
        self.blend == Blend::Additive
    }

    /// The layer as its picture says it is drawn. A decal picture with no
    /// alpha (opaque throughout) cannot blend by alpha or be cut out, and
    /// one that never gets much past mid-grey would only darken if it
    /// multiplied: such a picture is centred on grey, so it doubles
    /// ([`Blend::Double`]). A decal picture with alpha blends by it unless
    /// cut out. Effects are as named.
    pub fn with_picture(self, image: &Image) -> Layer {
        if self.group != Group::Decal || image.rgba.is_empty() {
            return self;
        }
        let (px, _) = image.rgba.as_chunks::<4>();
        let opaque = px.iter().all(|p| p[3] >= 250);
        if !opaque {
            return match self.blend {
                Blend::Test => self,
                _ => Layer::decal(Blend::Alpha),
            };
        }
        // Mid-grey-centred: few pixels much brighter than half.
        let bright = px
            .iter()
            .filter(|p| p[..3].iter().any(|&c| c > 160))
            .count();
        if bright * 4 <= px.len() {
            Layer::decal(Blend::Double)
        } else {
            Layer::decal(Blend::Multiply)
        }
    }
}

/// One triangle of a layer, world space (game coordinates): positions,
/// vertex normals, texture coordinates and the BSP material it is drawn
/// with.
#[derive(Clone, Copy, Debug)]
pub struct LayerTri {
    pub p: [[f32; 3]; 3],
    pub n: [[f32; 3]; 3],
    pub uv: [[f32; 2]; 3],
    pub material: usize,
}

/// One primitive of a layer node: one material's triangles, welded.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LayerPrim {
    /// Index into the level's [`LayerMaterial`]s.
    pub material: usize,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

impl LayerPrim {
    pub fn triangles(&self) -> usize {
        self.indices.len() / 3
    }
}

/// A decoded picture, shared by the materials that use it.
#[derive(Clone, Debug, PartialEq)]
pub struct Texture {
    /// The bitmap tag it came from: one image per bitmap in the file.
    pub bitmap: u32,
    pub image: Image,
}

/// A layer material: the shader's own name (last path component), its
/// layer, and its picture.
#[derive(Clone, Debug, PartialEq)]
pub struct LayerMaterial {
    /// The shader's tag ID: one material per shader in a level.
    pub shader: u32,
    pub name: String,
    pub layer: Layer,
    pub texture: Option<Arc<Texture>>,
}

/// The largest side of a layer's picture in `map.glb`.
pub const MAX_LAYER_TEXTURE: u32 = 256;

/// Welds a layer's triangles: a primitive per material (in material
/// order), vertices shared where position, normal and texture coordinate
/// agree (1/1024 unit, 1/64, 1/4096). Non-finite and degenerate triangles
/// are dropped; a missing or reversed normal becomes the face normal.
pub fn build_layer_prims(tris: &[LayerTri]) -> Vec<LayerPrim> {
    type Key = ([i32; 3], [i8; 3], [i32; 2]);
    let mut by: BTreeMap<usize, (LayerPrim, HashMap<Key, u32>)> = BTreeMap::new();
    for t in tris {
        if !t
            .p
            .iter()
            .flatten()
            .chain(t.uv.iter().flatten())
            .all(|v| v.is_finite())
        {
            continue;
        }
        let e1 = sub(t.p[1], t.p[0]);
        let e2 = sub(t.p[2], t.p[0]);
        let f = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
        let len = (f[0] * f[0] + f[1] * f[1] + f[2] * f[2]).sqrt();
        // Also drops a NaN length.
        if len.is_nan() || len < 1e-6 {
            continue;
        }
        let face = f.map(|v| v / len);
        let (prim, index) = by.entry(t.material).or_insert_with(|| {
            (
                LayerPrim {
                    material: t.material,
                    ..LayerPrim::default()
                },
                HashMap::new(),
            )
        });
        let mut idx = [0u32; 3];
        for (k, slot) in idx.iter_mut().enumerate() {
            let n = t.n[k];
            let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            let n = if l > 0.5
                && n.iter().all(|v| v.is_finite())
                && n[0] * face[0] + n[1] * face[1] + n[2] * face[2] > 0.0
            {
                n.map(|v| v / l)
            } else {
                face
            };
            let key = (
                t.p[k].map(|v| (v * 1024.0).round() as i32),
                n.map(|v| (v * 64.0).round() as i8),
                t.uv[k].map(|v| (v * 4096.0).round() as i32),
            );
            *slot = *index.entry(key).or_insert_with(|| {
                prim.positions.push(t.p[k]);
                prim.normals.push(n);
                prim.uvs.push(t.uv[k]);
                (prim.positions.len() - 1) as u32
            });
        }
        if idx[0] == idx[1] || idx[1] == idx[2] || idx[0] == idx[2] {
            continue;
        }
        prim.indices.extend(idx);
    }
    by.into_values()
        .map(|(p, _)| p)
        .filter(|p| !p.indices.is_empty())
        .collect()
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// The picture a layer is drawn with, from the shader's bitmaps by the
/// words of their tag names: the diffuse map ([`live_models::bitmap_role`]);
/// else, for a glow, whose picture is often named for self-illumination,
/// the first bitmap that is not an engine, bump, normal, cube or
/// reflection map.
pub fn pick_bitmap(names: &[(u32, String)], layer: Layer) -> Option<u32> {
    if let Some((id, _)) = names
        .iter()
        .find(|(_, n)| live_models::bitmap_role(n) == BitmapRole::Diffuse)
    {
        return Some(*id);
    }
    if layer.blend != Blend::Additive {
        return None;
    }
    names
        .iter()
        .find(|(_, n)| {
            let lower = n.to_ascii_lowercase();
            let engine = lower
                .split('\\')
                .any(|c| c == "default_bitmaps" || c == "rasterizer");
            let last = lower.rsplit('\\').next().unwrap_or(&lower);
            !engine
                && !["bump", "normal", "cube", "reflection"]
                    .iter()
                    .any(|w| last.contains(w))
        })
        .map(|(id, _)| *id)
}

/// Reads a layer shader's picture (decoded once per bitmap; `cache` is
/// per level).
pub fn shader_texture(
    map: &mut MapReader,
    files: &mut ResourceFiles,
    shader: TagId,
    layer: Layer,
    cache: &mut HashMap<u32, Option<Arc<Texture>>>,
) -> Option<Arc<Texture>> {
    let (_, bitmaps) = live_models::shader_bitmaps(map, shader).ok()?;
    let names: Vec<(u32, String)> = bitmaps
        .iter()
        .filter_map(|&id| Some((id.0, map.tag(id).ok()?.name.clone()?)))
        .collect();
    let id = pick_bitmap(&names, layer)?;
    cache
        .entry(id)
        .or_insert_with(|| {
            live_models::decode_bitmap(map, files, TagId(id))
                .ok()
                .map(|image| {
                    Arc::new(Texture {
                        bitmap: id,
                        image: image.fit(MAX_LAYER_TEXTURE),
                    })
                })
        })
        .clone()
}

/// A layer primitive's glTF parts: positions and normals in glTF axes,
/// texture coordinates as stored.
pub fn gltf_vertices(p: &LayerPrim) -> (Vec<[f32; 3]>, Vec<[f32; 3]>) {
    (
        p.positions.iter().map(|&v| to_gltf(v)).collect(),
        p.normals.iter().map(|&v| to_gltf(v)).collect(),
    )
}

/// The glTF material of a layer (`extras.layer`, `extras.blend`): base
/// colour from its texture (white factor) or, for a glow without one, a
/// faint grey; `BLEND`, or `MASK` at 0.5 for a cut-out; double-sided for
/// effects; unlit (`KHR_materials_unlit`) when it multiplies, doubles or
/// adds.
pub fn gltf_material(m: &LayerMaterial, texture: Option<usize>) -> serde_json::Value {
    use serde_json::json;
    let mut pbr = json!({"metallicFactor": 0.0, "roughnessFactor": 0.9});
    match texture {
        Some(t) => {
            pbr["baseColorFactor"] = json!([1.0, 1.0, 1.0, 1.0]);
            pbr["baseColorTexture"] = json!({"index": t});
        }
        None => pbr["baseColorFactor"] = json!([0.15, 0.15, 0.15, 1.0]),
    }
    let mut out = json!({
        "name": m.name,
        "pbrMetallicRoughness": pbr,
        "extras": {"layer": m.layer.group_name(), "blend": m.layer.blend_name()},
    });
    match m.layer.blend {
        Blend::Test => {
            out["alphaMode"] = json!("MASK");
            out["alphaCutoff"] = json!(0.5);
        }
        _ => out["alphaMode"] = json!("BLEND"),
    }
    if m.layer.group == Group::Effect {
        out["doubleSided"] = json!(true);
    }
    if matches!(
        m.layer.blend,
        Blend::Multiply | Blend::Double | Blend::Additive
    ) {
        out["extensions"] = json!({"KHR_materials_unlit": {}});
    }
    out
}
