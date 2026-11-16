//! Player and weapon models for the 3D game viewer: `models/biped/<species>.glb`
//! and `models/weapon/<stem>.glb` (docs/live-art.md, "Player and weapon
//! models"; the players contract, section 2).
//!
//! Runs inside the art extraction (`live_art::extract`) from the owner's own
//! disc. A biped is posed once: a frame of its animation graph's rifle idle is
//! sampled, composed up the render model's node tree and baked into the
//! vertices by linear blend skinning against the model's rest pose. Bipeds
//! are also skinned to that pose and carry `<weapon_class>:<state>` clips
//! sampled from the graph (contract section 10); weapons and vehicles stay
//! static. The pure parts (pose, skin, change colour, glTF) take plain data
//! so the tests can build invented models.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tags::anim_sample::{Mat4, NodeTransform, Pose, mat_mul};

use std::fs::File;
use std::io::BufReader;

use anyhow::{Context, Result, anyhow};
use assets::ResourceFiles;
use map::{CacheType, FourCC, Map, RawPointer, TagId};
use tags::anim::{Action, AnimationGraph};
use tags::geometry::{RawSource, TagView};
use tags::model::RenderModelGeometry;
use tags::sound::ResourceReader;
use tags::wire::StringId;
use tags::{DecodedTag, TagData};

use crate::live_art::Disc;
use crate::live_geometry::{GENERATOR, cross, dot, normalize, to_gltf};

/// The model files' format (contract `models.version`, `models_version`).
/// 2: bipeds are skinned and carry clips; weapons name their class.
pub const MODELS_VERSION: u32 = 2;
/// The largest texture side written into a glb; bigger ones are halved.
pub const MAX_TEXTURE: u32 = 512;
/// Base colour of a material whose diffuse map could not be decoded.
pub const GREY: [f32; 4] = [0.5, 0.5, 0.5, 1.0];
/// Glass without a picture of its own: a dark blue-grey, mostly see-through.
pub const GLASS: [f32; 4] = [0.18, 0.22, 0.26, 0.35];

/// One file in the manifest's `models` object.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GlbRef {
    /// Relative to the manifest, `/`-separated.
    pub glb: String,
    /// Bipeds: the glb's animations, `<weapon_class>:<state>`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clips: Vec<String>,
    /// Weapons: the clip set to hold it with (`rifle`, `pistol`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    /// Weapons: what its shots are, how fast they fly and how often
    /// (crate::live_weapons), for the viewer's tracers and bolts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ballistics: Option<crate::live_weapons::Ballistics>,
}

/// The manifest's `models` object.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelsEntry {
    pub version: u32,
    /// By species: `spartan`, `elite`.
    pub bipeds: BTreeMap<String, GlbRef>,
    /// By weapon tag file stem (`battle_rifle`, ...).
    pub weapons: BTreeMap<String, GlbRef>,
    /// By vehicle tag file stem (`warthog`, `ghost`, ...).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub vehicles: BTreeMap<String, GlbRef>,
}

impl ModelsEntry {
    /// `None` when nothing was extracted (the manifest then has no `models`).
    pub fn new(bipeds: &[String], weapons: &[String], vehicles: &[String]) -> Option<Self> {
        if bipeds.is_empty() && weapons.is_empty() && vehicles.is_empty() {
            return None;
        }
        let refs = |dir: &str, names: &[String]| {
            names
                .iter()
                .map(|n| {
                    let glb = format!("models/{dir}/{n}.glb");
                    let r = GlbRef {
                        glb,
                        clips: Vec::new(),
                        class: None,
                        ballistics: None,
                    };
                    (n.clone(), r)
                })
                .collect()
        };
        Some(Self {
            version: MODELS_VERSION,
            bipeds: refs("biped", bipeds),
            weapons: refs("weapon", weapons),
            vehicles: refs("vehicle", vehicles),
        })
    }
}

// ---- pose -----------------------------------------------------------------

/// A render-model node at rest: its transform relative to its parent.
#[derive(Clone, Debug, PartialEq)]
pub struct Bone {
    pub name: String,
    pub parent: Option<usize>,
    pub translation: [f32; 3],
    /// Unit quaternion `[x, y, z, w]`.
    pub rotation: [f32; 4],
}

/// A marker: a named frame attached to a node.
#[derive(Clone, Debug, PartialEq)]
pub struct Marker {
    pub name: String,
    pub node: usize,
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
}

fn trs(translation: [f32; 3], rotation: [f32; 4], scale: f32) -> Mat4 {
    NodeTransform {
        rotation,
        translation,
        scale,
        ..NodeTransform::default()
    }
    .matrix()
}

/// World matrices from local ones and parent links (any node order).
pub fn compose(locals: &[Mat4], parents: &[Option<usize>]) -> Vec<Mat4> {
    let mut world: Vec<Option<Mat4>> = vec![None; locals.len()];
    fn resolve(
        i: usize,
        locals: &[Mat4],
        parents: &[Option<usize>],
        world: &mut [Option<Mat4>],
        depth: usize,
    ) -> Mat4 {
        if let Some(m) = world[i] {
            return m;
        }
        let m = match parents[i] {
            Some(p) if p < locals.len() && p != i && depth < locals.len() => {
                mat_mul(&resolve(p, locals, parents, world, depth + 1), &locals[i])
            }
            _ => locals[i],
        };
        world[i] = Some(m);
        m
    }
    (0..locals.len())
        .map(|i| resolve(i, locals, parents, &mut world, 0))
        .collect()
}

/// The skeleton's rest pose in model space.
pub fn rest_world(bones: &[Bone]) -> Vec<Mat4> {
    let locals: Vec<Mat4> = bones
        .iter()
        .map(|b| trs(b.translation, b.rotation, 1.0))
        .collect();
    compose(&locals, &parents(bones))
}

fn parents(bones: &[Bone]) -> Vec<Option<usize>> {
    bones.iter().map(|b| b.parent).collect()
}

/// The skeleton posed by an animation frame. `pose.nodes[j]` belongs to the
/// graph node named `graph_names[j]`; a bone takes each channel the frame
/// supplies for the node of its own name and keeps its rest value otherwise.
pub fn posed_world(bones: &[Bone], graph_names: &[String], pose: &Pose) -> Vec<Mat4> {
    compose(&posed_locals(bones, graph_names, pose), &parents(bones))
}

/// Each bone's frame relative to its parent in an animation frame (see
/// [`posed_world`]).
pub fn posed_locals(bones: &[Bone], graph_names: &[String], pose: &Pose) -> Vec<Mat4> {
    let slot = graph_slots(bones, graph_names);
    bones
        .iter()
        .zip(slot)
        .map(|(b, j)| match j.and_then(|j| pose.nodes.get(j)) {
            Some(t) => trs(
                if t.has_translation {
                    t.translation
                } else {
                    b.translation
                },
                if t.has_rotation {
                    t.rotation
                } else {
                    b.rotation
                },
                if t.has_scale { t.scale } else { 1.0 },
            ),
            None => trs(b.translation, b.rotation, 1.0),
        })
        .collect()
}

/// For each bone, the graph node of the same name.
fn graph_slots(bones: &[Bone], graph_names: &[String]) -> Vec<Option<usize>> {
    bones
        .iter()
        .map(|b| graph_names.iter().position(|n| *n == b.name))
        .collect()
}

/// Inverse of a rigid (rotation, translation, uniform scale) matrix.
pub fn invert_rigid(m: &Mat4) -> Mat4 {
    let s2 = dot([m[0][0], m[0][1], m[0][2]], [m[0][0], m[0][1], m[0][2]]);
    let k = if s2 > 0.0 { 1.0 / s2 } else { 0.0 };
    let mut out = [[0.0; 4]; 4];
    for c in 0..3 {
        for r in 0..3 {
            out[c][r] = m[r][c] * k;
        }
    }
    let t: [f32; 3] = std::array::from_fn(|r| -(0..3).map(|c| out[c][r] * m[3][c]).sum::<f32>());
    out[3] = [t[0], t[1], t[2], 1.0];
    out
}

pub fn transform_point(m: &Mat4, p: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|r| m[0][r] * p[0] + m[1][r] * p[1] + m[2][r] * p[2] + m[3][r])
}

pub fn transform_vector(m: &Mat4, v: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|r| m[0][r] * v[0] + m[1][r] * v[1] + m[2][r] * v[2])
}

/// Per-bone skinning matrices: posed world times inverse rest world.
pub fn skin_matrices(rest: &[Mat4], posed: &[Mat4]) -> Vec<Mat4> {
    rest.iter()
        .zip(posed)
        .map(|(r, p)| mat_mul(p, &invert_rigid(r)))
        .collect()
}

/// A vertex's posed position and normal: the weighted sum of its nodes'
/// skinning matrices (weights renormalised; no weight -> node 0).
pub fn skin_vertex(
    skin: &[Mat4],
    nodes: [usize; 4],
    weights: [f32; 4],
    p: [f32; 3],
    n: [f32; 3],
) -> ([f32; 3], [f32; 3]) {
    let total: f32 = (0..4)
        .filter(|&k| nodes[k] < skin.len())
        .map(|k| weights[k].max(0.0))
        .sum();
    if total <= 0.0 || skin.is_empty() {
        let m = skin.first().copied().unwrap_or(IDENTITY);
        return (transform_point(&m, p), normalize(transform_vector(&m, n)));
    }
    let (mut op, mut on) = ([0.0f32; 3], [0.0f32; 3]);
    for k in 0..4 {
        let w = weights[k].max(0.0) / total;
        if w == 0.0 || nodes[k] >= skin.len() {
            continue;
        }
        let tp = transform_point(&skin[nodes[k]], p);
        let tn = transform_vector(&skin[nodes[k]], n);
        for a in 0..3 {
            op[a] += w * tp[a];
            on[a] += w * tn[a];
        }
    }
    (op, normalize(on))
}

/// A vertex's JOINTS_0 and WEIGHTS_0: the weights [`skin_vertex`] uses
/// (renormalised, nodes past `joints` dropped; none left -> node 0 alone).
pub fn joint_weights(nodes: [usize; 4], weights: [f32; 4], joints: usize) -> ([u16; 4], [f32; 4]) {
    let w: [f32; 4] = std::array::from_fn(|k| {
        if nodes[k] < joints && nodes[k] <= usize::from(u16::MAX) {
            weights[k].max(0.0)
        } else {
            0.0
        }
    });
    let total: f32 = w.iter().sum();
    if total <= 0.0 {
        return ([0; 4], [1.0, 0.0, 0.0, 0.0]);
    }
    (
        std::array::from_fn(|k| if w[k] > 0.0 { nodes[k] as u16 } else { 0 }),
        w.map(|v| v / total),
    )
}

pub const IDENTITY: Mat4 = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

/// A marker's frame in model space, given its node's world matrix.
pub fn marker_world(node_world: &Mat4, m: &Marker) -> Mat4 {
    mat_mul(node_world, &trs(m.translation, m.rotation, 1.0))
}

// ---- change colour and textures --------------------------------------------

/// An RGBA8 picture.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    /// Bilinear sample with wrapping (`u`, `v` in texture space, origin at
    /// the top-left), channels in 0..1.
    pub fn sample(&self, uv: [f32; 2]) -> [f32; 4] {
        let (w, h) = (self.width as usize, self.height as usize);
        if w == 0 || h == 0 || self.rgba.len() < w * h * 4 {
            return [0.0; 4];
        }
        let fx = uv[0] * w as f32 - 0.5;
        let fy = uv[1] * h as f32 - 0.5;
        if !fx.is_finite() || !fy.is_finite() {
            return [0.0; 4];
        }
        let (x0, y0) = (fx.floor(), fy.floor());
        let (tx, ty) = (fx - x0, fy - y0);
        let wrap = |v: f32, n: usize| (v as i64).rem_euclid(n as i64) as usize;
        let px = |x: usize, y: usize, c: usize| f32::from(self.rgba[(y * w + x) * 4 + c]) / 255.0;
        let (xa, xb) = (wrap(x0, w), wrap(x0 + 1.0, w));
        let (ya, yb) = (wrap(y0, h), wrap(y0 + 1.0, h));
        std::array::from_fn(|c| {
            let top = px(xa, ya, c) * (1.0 - tx) + px(xb, ya, c) * tx;
            let bottom = px(xa, yb, c) * (1.0 - tx) + px(xb, yb, c) * tx;
            top * (1.0 - ty) + bottom * ty
        })
    }

    /// Halved (2 x 2 box filter) until neither side exceeds `max`.
    pub fn fit(mut self, max: u32) -> Image {
        while (self.width > max || self.height > max) && self.width > 1 && self.height > 1 {
            let (w, h) = ((self.width / 2) as usize, (self.height / 2) as usize);
            let sw = self.width as usize;
            let mut out = vec![0u8; w * h * 4];
            for y in 0..h {
                for x in 0..w {
                    for c in 0..4 {
                        let s: u32 = [(0, 0), (1, 0), (0, 1), (1, 1)]
                            .iter()
                            .map(|(dx, dy)| {
                                u32::from(self.rgba[((2 * y + dy) * sw + 2 * x + dx) * 4 + c])
                            })
                            .sum();
                        out[(y * w + x) * 4 + c] = ((s + 2) / 4) as u8;
                    }
                }
            }
            self = Image {
                width: w as u32,
                height: h as u32,
                rgba: out,
            };
        }
        self
    }
}

/// COLOR_0 at a vertex: red = primary change-colour weight, green =
/// secondary, from the change-colour map's red and green channels.
pub fn change_colour(map: &Image, uv: [f32; 2]) -> [f32; 4] {
    let s = map.sample(uv);
    [s[0], s[1], 0.0, 1.0]
}

// ---- the model and its glb ---------------------------------------------------

/// A material: a diffuse texture when one was decoded, else grey, and what
/// else its shader says about the surface ([`material_look`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Material {
    pub name: String,
    /// The base map. Its alpha is kept only where the look uses it
    /// (alpha-tested and blended surfaces); elsewhere it is opaque.
    pub texture: Option<Image>,
    /// The change-colour map, per pixel: red is the primary colour's
    /// weight, green the secondary's (blue and alpha, the third and fourth
    /// colours, are not tinted).
    pub change_colour: Option<Image>,
    /// What glows: the self-illumination map (or, for a lit display or an
    /// additive surface, the base map).
    pub emissive: Option<Image>,
    /// A glTF metallic-roughness texture (green roughness, blue metalness)
    /// made from the base map's specular mask, for shiny and
    /// environment-mapped shaders.
    pub specular: Option<Image>,
    pub blend: Blend,
    /// The shader template's file stem (`tex_bump_env_four_change_color`).
    pub template: String,
}

/// How a surface is composited.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Blend {
    #[default]
    Opaque,
    /// Cut out where the base map's alpha is under one half.
    Mask,
    /// Blended by the base map's alpha (decals) or see-through (glass).
    Alpha,
    /// Added to what is behind it (lights, vents, glows).
    Additive,
}

/// Largest side of the change-colour, self-illumination and specular maps.
pub const MAX_AUX_TEXTURE: u32 = 256;

/// A surface's look from its shader: the template's name and its bitmap
/// parameters (each the bitmap tag's name and, when it decoded, its first
/// image), in the shader's order. Returns the material and the
/// change-colour map at full size (for the per-vertex weights).
///
/// The template names say what the shader does (`tex_bump_env_*`,
/// `*_change_color*`, `*_illum*`, `transparent\*_add_*`, `*alpha_test*`),
/// and each bitmap's role comes from its own name ([`bitmap_role`]). Halo 2
/// keeps the specular (environment) mask in the base map's alpha for its
/// bump-env and shiny templates; it becomes metalness and smoothness here.
/// Bump maps (8-bit palettized normals whose palette is the engine's, not
/// the tag's) and detail maps are not used.
pub fn material_look(
    name: &str,
    template: &str,
    bitmaps: Vec<(String, Option<Image>)>,
) -> (Material, Option<Image>) {
    let lower = template.to_ascii_lowercase();
    let stem = last_component(&lower).to_owned();
    let transparent = lower.split('\\').any(|c| c == "transparent");
    let blend = if transparent && stem.contains("add") {
        Blend::Additive
    } else if transparent || stem.contains("overlay") {
        Blend::Alpha
    } else if stem.contains("alpha_test") {
        Blend::Mask
    } else {
        Blend::Opaque
    };
    let (mut diffuse, mut cc, mut illum) = (None, None, None);
    for (bname, image) in bitmaps {
        let Some(image) = image else { continue };
        match bitmap_role(&bname) {
            BitmapRole::Diffuse if diffuse.is_none() => diffuse = Some(image),
            BitmapRole::ChangeColour if cc.is_none() => cc = Some(image),
            BitmapRole::SelfIllum if illum.is_none() => illum = Some(image),
            _ => {}
        }
    }
    let mut base = diffuse.map(|d| d.fit(MAX_TEXTURE));
    let shiny = blend == Blend::Opaque && (stem.contains("env") || stem.contains("shiny"));
    let specular = base
        .as_ref()
        .filter(|_| shiny)
        .map(|b| specular_map(b, if stem.contains("env") { 0.6 } else { 0.35 }));
    // Glass: the base map is the vehicle's own skin, not what the glass
    // looks like; a tinted, smooth see-through surface reads better.
    if blend == Blend::Alpha && transparent {
        base = None;
    }
    if blend == Blend::Opaque
        && let Some(b) = base.as_mut()
    {
        for px in b.rgba.as_chunks_mut::<4>().0 {
            px[3] = 255;
        }
    }
    let lit_display = stem == "illum" || stem.contains("meter") || stem.contains("plasma");
    let emissive = match illum {
        Some(i) => Some(i.fit(MAX_AUX_TEXTURE)),
        None if blend == Blend::Additive || lit_display => base.clone(),
        None => None,
    };
    if blend == Blend::Additive {
        base = None;
    }
    let material = Material {
        name: name.to_owned(),
        texture: base,
        change_colour: cc.clone().map(|c| c.fit(MAX_AUX_TEXTURE)),
        emissive,
        specular,
        blend,
        template: stem,
    };
    (material, cc)
}

/// The metallic-roughness texture for a base map whose alpha is the
/// specular mask: where the mask is full the surface is `metal` metallic
/// and smooth (roughness 0.3); where it is empty, matte (0.85).
pub fn specular_map(base: &Image, metal: f32) -> Image {
    let small = base.clone().fit(MAX_AUX_TEXTURE);
    let mut rgba = small.rgba;
    for px in rgba.as_chunks_mut::<4>().0 {
        let a = f32::from(px[3]) / 255.0;
        let rough = 0.85 - 0.55 * a;
        px[0] = 255;
        px[1] = (rough * 255.0).round() as u8;
        px[2] = (a * metal * 255.0).round() as u8;
        px[3] = 255;
    }
    Image { rgba, ..small }
}

/// Triangles of one material, in game coordinates.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Primitive {
    pub material: usize,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// COLOR_0 (same length as `positions`) or empty.
    pub colours: Vec<[f32; 4]>,
    /// JOINTS_0 and WEIGHTS_0 (skeleton joint indices, weights summing to
    /// one); written only when the model has a [`Skeleton`].
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
}

/// A joint of a skinned model: a render-model node.
#[derive(Clone, Debug, PartialEq)]
pub struct Joint {
    pub name: String,
    pub parent: Option<usize>,
    /// Frame relative to the parent (the model, for a root) in the default
    /// pose, game coordinates. The mesh is stored in this pose.
    pub local: Mat4,
}

/// An animation: every joint's local frame at every frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Clip {
    /// `<weapon_class>:<state>`.
    pub name: String,
    pub fps: f32,
    /// `frames[f][j]`: joint `j`'s local frame (as [`Joint::local`]).
    pub frames: Vec<Vec<Mat4>>,
}

/// What makes a model skinned and animated.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Skeleton {
    pub joints: Vec<Joint>,
    /// Empty nodes (of [`Model::nodes`]) carried by a joint, by name.
    pub attach: BTreeMap<String, usize>,
    pub clips: Vec<Clip>,
}

impl Skeleton {
    /// Joint world frames from per-joint locals (the default pose's or a
    /// clip frame's).
    pub fn world(&self, locals: &[Mat4]) -> Vec<Mat4> {
        let parents: Vec<Option<usize>> = self.joints.iter().map(|j| j.parent).collect();
        compose(locals, &parents)
    }
    pub fn default_locals(&self) -> Vec<Mat4> {
        self.joints.iter().map(|j| j.local).collect()
    }
}

/// A posed (or rest) model ready to write.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Model {
    /// Mesh node name: `body` for bipeds, `weapon` for weapons.
    pub mesh_name: String,
    pub primitives: Vec<Primitive>,
    pub materials: Vec<Material>,
    /// Empty nodes, game coordinates.
    pub nodes: Vec<(String, Mat4)>,
    /// Added to `asset.extras` (besides `generator` and `models_version`).
    pub extras: serde_json::Map<String, Value>,
    /// Bipeds: the skeleton the mesh is skinned to, and the clips.
    pub skeleton: Option<Skeleton>,
    /// Weapons: the animation graphs' weapon class (`rifle`, `pistol`, ...).
    pub class: Option<String>,
    /// Weapons: their shots (crate::live_weapons).
    pub ballistics: Option<crate::live_weapons::Ballistics>,
}

impl Model {
    pub fn triangles(&self) -> usize {
        self.primitives.iter().map(|p| p.indices.len() / 3).sum()
    }

    /// Lowest and highest point of the mesh, game coordinates.
    pub fn bounds(&self) -> Option<([f32; 3], [f32; 3])> {
        let mut it = self.primitives.iter().flat_map(|p| p.positions.iter());
        let first = *it.next()?;
        Some(it.fold((first, first), |(lo, hi), p| {
            (
                std::array::from_fn(|k| lo[k].min(p[k])),
                std::array::from_fn(|k| hi[k].max(p[k])),
            )
        }))
    }

    /// Moves everything by `d` (game coordinates).
    pub fn translate(&mut self, d: [f32; 3]) {
        for p in &mut self.primitives {
            for v in &mut p.positions {
                *v = std::array::from_fn(|k| v[k] + d[k]);
            }
        }
        let shift = |m: &mut Mat4| {
            for k in 0..3 {
                m[3][k] += d[k];
            }
        };
        self.nodes.iter_mut().for_each(|(_, m)| shift(m));
        // Joints follow; a root joint's frame is relative to the model.
        if let Some(sk) = &mut self.skeleton {
            let roots: Vec<usize> = (0..sk.joints.len())
                .filter(|&j| sk.joints[j].parent.is_none())
                .collect();
            for &j in &roots {
                shift(&mut sk.joints[j].local);
                for c in &mut sk.clips {
                    c.frames.iter_mut().for_each(|f| shift(&mut f[j]));
                }
            }
        }
    }
}

/// The game-to-glTF axis change as a matrix: `(x, y, z)` -> `(x, z, -y)`.
const AXES: Mat4 = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 0.0, -1.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

/// A game-space rigid frame as glTF translation and rotation.
pub fn gltf_trs(m: &Mat4) -> ([f32; 3], [f32; 4]) {
    let g = to_gltf_frame(m);
    // Orthonormalise the rotation part, then convert.
    let x = normalize([g[0][0], g[0][1], g[0][2]]);
    let y0 = [g[1][0], g[1][1], g[1][2]];
    let z = normalize(cross(x, y0));
    let y = cross(z, x);
    ([g[3][0], g[3][1], g[3][2]], quat_from_axes(x, y, z))
}

/// A game-space frame in glTF axes, as a matrix.
fn to_gltf_frame(m: &Mat4) -> Mat4 {
    mat_mul(&mat_mul(&AXES, m), &invert_rigid(&AXES))
}

/// Quaternions with each one's sign flipped where needed to stay in the
/// previous one's hemisphere, so linear interpolation takes the short way.
pub fn continuous(qs: impl Iterator<Item = [f32; 4]>) -> Vec<[f32; 4]> {
    let mut out: Vec<[f32; 4]> = Vec::new();
    for q in qs {
        let flip = out
            .last()
            .is_some_and(|p| (0..4).map(|k| p[k] * q[k]).sum::<f32>() < 0.0);
        out.push(if flip { q.map(|v| -v) } else { q });
    }
    out
}

/// The unit quaternion `[x, y, z, w]` of a rotation with these columns.
pub fn quat_from_axes(x: [f32; 3], y: [f32; 3], z: [f32; 3]) -> [f32; 4] {
    let (m00, m11, m22) = (x[0], y[1], z[2]);
    let trace = m00 + m11 + m22;
    let q = if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        [
            (y[2] - z[1]) / s,
            (z[0] - x[2]) / s,
            (x[1] - y[0]) / s,
            0.25 * s,
        ]
    } else if m00 > m11 && m00 > m22 {
        let s = (1.0 + m00 - m11 - m22).sqrt() * 2.0;
        [
            0.25 * s,
            (y[0] + x[1]) / s,
            (z[0] + x[2]) / s,
            (y[2] - z[1]) / s,
        ]
    } else if m11 > m22 {
        let s = (1.0 + m11 - m00 - m22).sqrt() * 2.0;
        [
            (y[0] + x[1]) / s,
            0.25 * s,
            (z[1] + y[2]) / s,
            (z[0] - x[2]) / s,
        ]
    } else {
        let s = (1.0 + m22 - m00 - m11).sqrt() * 2.0;
        [
            (z[0] + x[2]) / s,
            (z[1] + y[2]) / s,
            0.25 * s,
            (x[1] - y[0]) / s,
        ]
    };
    let l = q.iter().map(|v| v * v).sum::<f32>().sqrt();
    q.map(|v| v / l)
}

/// The glb: a root node named after the model holding the mesh node and the
/// empty nodes; one primitive per material (POSITION, NORMAL, TEXCOORD_0,
/// COLOR_0 when present, u32 indices); embedded PNG textures.
pub fn write_glb(model: &Model) -> Vec<u8> {
    let mut bin: Vec<u8> = Vec::new();
    let mut views: Vec<Value> = Vec::new();
    let mut accessors: Vec<Value> = Vec::new();
    let mut view = |bin: &mut Vec<u8>, bytes: &[u8], target: Option<u32>| -> usize {
        while !bin.len().is_multiple_of(4) {
            bin.push(0);
        }
        let mut v = json!({"buffer": 0, "byteOffset": bin.len(), "byteLength": bytes.len()});
        if let Some(t) = target {
            v["target"] = json!(t);
        }
        views.push(v);
        bin.extend_from_slice(bytes);
        views.len() - 1
    };
    let floats =
        |v: &mut dyn Iterator<Item = f32>| -> Vec<u8> { v.flat_map(f32::to_le_bytes).collect() };

    let mut images = Vec::new();
    let mut textures = Vec::new();
    let mut materials: Vec<Value> = Vec::new();
    // Each distinct picture is stored once (materials sharing a shader, or
    // a base map that is also the emission, share it).
    let mut stored: Vec<(Image, usize)> = Vec::new();
    let mut texture = |bin: &mut Vec<u8>, t: &Image| -> usize {
        if let Some(&(_, k)) = stored.iter().find(|(o, _)| o == t) {
            return k;
        }
        let mut png = Vec::new();
        assets::png::write_rgba_fast(&mut png, t.width, t.height, &t.rgba)
            .expect("PNG into memory");
        let bv = view(bin, &png, None);
        images.push(json!({"bufferView": bv, "mimeType": "image/png"}));
        textures.push(json!({"sampler": 0, "source": images.len() - 1}));
        let k = textures.len() - 1;
        stored.push((t.clone(), k));
        k
    };
    for m in &model.materials {
        let mut pbr = json!({"metallicFactor": 0.0, "roughnessFactor": 0.8});
        let mut mat = json!({"name": m.name});
        match (&m.texture, m.blend) {
            (Some(t), _) => pbr["baseColorTexture"] = json!({"index": texture(&mut bin, t)}),
            // Glass: tinted, smooth and see-through.
            (None, Blend::Alpha) => {
                pbr["baseColorFactor"] = json!(GLASS);
                pbr["metallicFactor"] = json!(0.8);
                pbr["roughnessFactor"] = json!(0.12);
            }
            // Only its glow shows (added by readers that honour extras.h2.blend).
            (None, Blend::Additive) => pbr["baseColorFactor"] = json!([0.0, 0.0, 0.0, 1.0]),
            (None, _) => pbr["baseColorFactor"] = json!(GREY),
        }
        if let Some(t) = &m.specular {
            pbr["metallicRoughnessTexture"] = json!({"index": texture(&mut bin, t)});
            pbr["metallicFactor"] = json!(1.0);
            pbr["roughnessFactor"] = json!(1.0);
        }
        if let Some(t) = &m.emissive {
            mat["emissiveTexture"] = json!({"index": texture(&mut bin, t)});
            mat["emissiveFactor"] = json!([1.0, 1.0, 1.0]);
        }
        match m.blend {
            Blend::Opaque => {}
            Blend::Mask => {
                mat["alphaMode"] = json!("MASK");
                mat["alphaCutoff"] = json!(0.5);
                mat["doubleSided"] = json!(true);
            }
            Blend::Alpha | Blend::Additive => {
                mat["alphaMode"] = json!("BLEND");
                mat["doubleSided"] = json!(true);
            }
        }
        let mut h2 = serde_json::Map::new();
        if !m.template.is_empty() {
            h2.insert("template".into(), json!(m.template));
        }
        if let Some(t) = &m.change_colour {
            h2.insert("change_colour".into(), json!(texture(&mut bin, t)));
        }
        if m.blend == Blend::Additive {
            h2.insert("blend".into(), json!("add"));
        }
        if !h2.is_empty() {
            mat["extras"] = json!({ "h2": h2 });
        }
        mat["pbrMetallicRoughness"] = pbr;
        materials.push(mat);
    }

    let skeleton = model.skeleton.as_ref().filter(|s| !s.joints.is_empty());
    let joint_count = skeleton.map_or(0, |s| s.joints.len());
    let mut prims = Vec::new();
    for p in model.primitives.iter().filter(|p| !p.indices.is_empty()) {
        let pos: Vec<[f32; 3]> = p.positions.iter().map(|&v| to_gltf(v)).collect();
        let nrm: Vec<[f32; 3]> = p.normals.iter().map(|&v| to_gltf(v)).collect();
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for v in &pos {
            for k in 0..3 {
                lo[k] = lo[k].min(v[k]);
                hi[k] = hi[k].max(v[k]);
            }
        }
        let mut attributes = serde_json::Map::new();
        let mut add = |name: &str, bytes: Vec<u8>, ty: &str, count: usize, minmax: bool| {
            let bv = view(&mut bin, &bytes, Some(34962));
            let mut a =
                json!({"bufferView": bv, "componentType": 5126, "count": count, "type": ty});
            if minmax {
                a["min"] = json!(lo);
                a["max"] = json!(hi);
            }
            accessors.push(a);
            attributes.insert(name.into(), json!(accessors.len() - 1));
        };
        add(
            "POSITION",
            floats(&mut pos.iter().flatten().copied()),
            "VEC3",
            pos.len(),
            true,
        );
        if nrm.len() == pos.len() {
            add(
                "NORMAL",
                floats(&mut nrm.iter().flatten().copied()),
                "VEC3",
                pos.len(),
                false,
            );
        }
        if p.uvs.len() == pos.len() {
            add(
                "TEXCOORD_0",
                floats(&mut p.uvs.iter().flatten().copied()),
                "VEC2",
                pos.len(),
                false,
            );
        }
        if p.colours.len() == pos.len() {
            add(
                "COLOR_0",
                floats(&mut p.colours.iter().flatten().copied()),
                "VEC4",
                pos.len(),
                false,
            );
        }
        if joint_count > 0 && p.joints.len() == pos.len() && p.weights.len() == pos.len() {
            add(
                "WEIGHTS_0",
                floats(&mut p.weights.iter().flatten().copied()),
                "VEC4",
                pos.len(),
                false,
            );
            let bytes: Vec<u8> = p
                .joints
                .iter()
                .flatten()
                .map(|&j| if usize::from(j) < joint_count { j } else { 0 })
                .flat_map(u16::to_le_bytes)
                .collect();
            let bv = view(&mut bin, &bytes, Some(34962));
            accessors.push(json!({"bufferView": bv, "componentType": 5123, "count": pos.len(), "type": "VEC4"}));
            attributes.insert("JOINTS_0".into(), json!(accessors.len() - 1));
        }
        let iv = view(
            &mut bin,
            &p.indices
                .iter()
                .flat_map(|i| i.to_le_bytes())
                .collect::<Vec<_>>(),
            Some(34963),
        );
        accessors.push(json!({"bufferView": iv, "componentType": 5125, "count": p.indices.len(), "type": "SCALAR"}));
        let mut prim = json!({"attributes": attributes, "indices": accessors.len() - 1, "mode": 4});
        if p.material < materials.len() {
            prim["material"] = json!(p.material);
        }
        prims.push(prim);
    }
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }

    let mut extras = serde_json::Map::new();
    extras.insert("generator".into(), json!(GENERATOR));
    extras.insert("models_version".into(), json!(MODELS_VERSION));
    extras.extend(model.extras.clone());
    let mut nodes = vec![json!({"name": "root"})];
    let mut root_children = Vec::new();
    if !prims.is_empty() {
        nodes.push(json!({"name": model.mesh_name, "mesh": 0}));
        root_children.push(1);
    }
    let joint_base = nodes.len();
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); joint_count];
    let mut skins = Vec::new();
    let mut animations = Vec::new();
    let world = skeleton.map_or_else(Vec::new, |sk| sk.world(&sk.default_locals()));
    if let Some(sk) = skeleton {
        for (j, joint) in sk.joints.iter().enumerate() {
            let (t, r) = gltf_trs(&joint.local);
            nodes.push(
                json!({"name": format!("joint_{}", joint.name), "translation": t, "rotation": r}),
            );
            match joint.parent {
                Some(p) if p < j => children[p].push(joint_base + j),
                _ => root_children.push(joint_base + j),
            }
        }
        // Bound in the default pose: skinning there changes nothing.
        let ibm = floats(
            &mut world
                .iter()
                .flat_map(|w| invert_rigid(&to_gltf_frame(w)))
                .flatten(),
        );
        let bv = view(&mut bin, &ibm, None);
        accessors.push(
            json!({"bufferView": bv, "componentType": 5126, "count": joint_count, "type": "MAT4"}),
        );
        skins.push(json!({
            "inverseBindMatrices": accessors.len() - 1,
            "joints": (joint_base..joint_base + joint_count).collect::<Vec<_>>(),
        }));
        if !prims.is_empty() {
            nodes[1]["skin"] = json!(0);
        }
        // A one-key time line for channels that never change.
        let still = {
            let bv = view(&mut bin, &0f32.to_le_bytes(), None);
            accessors.push(json!({"bufferView": bv, "componentType": 5126, "count": 1, "type": "SCALAR", "min": [0.0], "max": [0.0]}));
            accessors.len() - 1
        };
        for clip in sk.clips.iter().filter(|c| !c.frames.is_empty()) {
            let n = clip.frames.len();
            let times: Vec<f32> = (0..n).map(|f| f as f32 / clip.fps).collect();
            let bv = view(&mut bin, &floats(&mut times.iter().copied()), None);
            accessors.push(json!({"bufferView": bv, "componentType": 5126, "count": n, "type": "SCALAR", "min": [0.0], "max": [times[n - 1]]}));
            let timeline = accessors.len() - 1;
            let (mut samplers, mut channels) = (Vec::new(), Vec::new());
            for j in 0..joint_count {
                let keys: Vec<([f32; 3], [f32; 4])> = clip
                    .frames
                    .iter()
                    .map(|f| gltf_trs(f.get(j).unwrap_or(&sk.joints[j].local)))
                    .collect();
                let rotations = continuous(keys.iter().map(|k| k.1));
                let translations: Vec<[f32; 3]> = keys.iter().map(|k| k.0).collect();
                let rot_still = rotations
                    .iter()
                    .all(|r| (0..4).all(|k| (r[k] - rotations[0][k]).abs() < 1e-4));
                let pos_still = translations
                    .iter()
                    .all(|t| (0..3).all(|k| (t[k] - translations[0][k]).abs() < 1e-4));
                let keep = |still_: bool| if still_ { 1 } else { n };
                let r = &rotations[..keep(rot_still)];
                let bytes: Vec<u8> = r
                    .iter()
                    .flatten()
                    .map(|v| (v.clamp(-1.0, 1.0) * 32767.0).round() as i16)
                    .flat_map(i16::to_le_bytes)
                    .collect();
                let bv = view(&mut bin, &bytes, None);
                accessors.push(json!({"bufferView": bv, "componentType": 5122, "normalized": true, "count": r.len(), "type": "VEC4"}));
                samplers.push(json!({"input": if rot_still { still } else { timeline }, "output": accessors.len() - 1, "interpolation": "LINEAR"}));
                channels.push(json!({"sampler": samplers.len() - 1, "target": {"node": joint_base + j, "path": "rotation"}}));
                let t = &translations[..keep(pos_still)];
                let bv = view(&mut bin, &floats(&mut t.iter().flatten().copied()), None);
                accessors.push(json!({"bufferView": bv, "componentType": 5126, "count": t.len(), "type": "VEC3"}));
                samplers.push(json!({"input": if pos_still { still } else { timeline }, "output": accessors.len() - 1, "interpolation": "LINEAR"}));
                channels.push(json!({"sampler": samplers.len() - 1, "target": {"node": joint_base + j, "path": "translation"}}));
            }
            animations.push(json!({"name": clip.name, "samplers": samplers, "channels": channels}));
        }
    }
    for (name, m) in &model.nodes {
        let joint = skeleton
            .and_then(|sk| sk.attach.get(name))
            .copied()
            .filter(|&j| j < joint_count);
        let frame = match joint {
            Some(j) => mat_mul(&invert_rigid(&world[j]), m),
            None => *m,
        };
        let (t, r) = gltf_trs(&frame);
        nodes.push(json!({"name": name, "translation": t, "rotation": r}));
        match joint {
            Some(j) => children[j].push(nodes.len() - 1),
            None => root_children.push(nodes.len() - 1),
        }
    }
    nodes[0]["children"] = json!(root_children);
    for (j, c) in children.into_iter().enumerate() {
        if !c.is_empty() {
            nodes[joint_base + j]["children"] = json!(c);
        }
    }
    let mut doc = json!({
        "asset": {"version": "2.0", "generator": GENERATOR, "extras": extras},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": nodes,
    });
    if !prims.is_empty() {
        doc["meshes"] = json!([{"name": model.mesh_name, "primitives": prims}]);
        doc["accessors"] = json!(accessors);
    }
    if !materials.is_empty() {
        doc["materials"] = json!(materials);
    }
    if !skins.is_empty() {
        doc["skins"] = json!(skins);
        // Accessors of the skin (and clips) even without a mesh.
        doc["accessors"] = json!(accessors);
    }
    if !animations.is_empty() {
        doc["animations"] = json!(animations);
    }
    if !images.is_empty() {
        doc["images"] = json!(images);
        doc["textures"] = json!(textures);
        // Repeat wrapping, linear filtering with mipmaps.
        doc["samplers"] =
            json!([{"magFilter": 9729, "minFilter": 9987, "wrapS": 10497, "wrapT": 10497}]);
    }
    if !bin.is_empty() {
        doc["bufferViews"] = json!(views);
        doc["buffers"] = json!([{"byteLength": bin.len()}]);
    }
    let mut json = serde_json::to_vec(&doc).expect("glTF JSON");
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    let total = 12 + 8 + json.len() + if bin.is_empty() { 0 } else { 8 + bin.len() };
    let mut out = Vec::with_capacity(total);
    out.extend(0x4654_6c67u32.to_le_bytes()); // "glTF"
    out.extend(2u32.to_le_bytes());
    out.extend((total as u32).to_le_bytes());
    out.extend((json.len() as u32).to_le_bytes());
    out.extend(0x4e4f_534au32.to_le_bytes()); // "JSON"
    out.extend(&json);
    if !bin.is_empty() {
        out.extend((bin.len() as u32).to_le_bytes());
        out.extend(0x004e_4942u32.to_le_bytes()); // "BIN\0"
        out.extend(&bin);
    }
    out
}

// ---- from model parts to a posed model ---------------------------------------

/// One drawn piece of a render model: a section's triangles of one material,
/// in model space (rest pose), with the model nodes skinning each vertex.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Piece {
    pub material: usize,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// Model node indices (not section-local).
    pub nodes: Vec<[usize; 4]>,
    pub weights: Vec<[f32; 4]>,
    pub triangles: Vec<[u32; 3]>,
}

/// A render model as plain data.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Source {
    pub bones: Vec<Bone>,
    pub markers: Vec<Marker>,
    pub pieces: Vec<Piece>,
    pub materials: Vec<Material>,
    /// Per material: its shader's change-colour map, when it has one.
    pub change_colour: Vec<Option<Image>>,
}

impl Source {
    pub fn marker(&self, names: &[&str]) -> Option<&Marker> {
        names
            .iter()
            .find_map(|n| self.markers.iter().find(|m| m.name == *n))
    }
    pub fn bone(&self, names: &[&str]) -> Option<usize> {
        names
            .iter()
            .find_map(|n| self.bones.iter().position(|b| b.name == *n))
    }
}

/// The mesh with every vertex moved from the rest pose to `posed` (node
/// world matrices); one primitive per material. COLOR_0 is written for every
/// primitive when any material has a change-colour map (zero weights where
/// its own has none).
pub fn pose_mesh(src: &Source, posed: &[Mat4]) -> Vec<Primitive> {
    let rest = rest_world(&src.bones);
    let skin = skin_matrices(&rest, posed);
    let any_cc = src.change_colour.iter().any(Option::is_some);
    let mut prims: BTreeMap<usize, Primitive> = BTreeMap::new();
    for piece in &src.pieces {
        let prim = prims.entry(piece.material).or_insert_with(|| Primitive {
            material: piece.material,
            ..Primitive::default()
        });
        let base = prim.positions.len() as u32;
        let cc = src
            .change_colour
            .get(piece.material)
            .and_then(Option::as_ref);
        for i in 0..piece.positions.len() {
            let n = piece.normals.get(i).copied().unwrap_or([0.0, 0.0, 1.0]);
            let nodes = piece.nodes.get(i).copied().unwrap_or([0; 4]);
            let weights = piece
                .weights
                .get(i)
                .copied()
                .unwrap_or([1.0, 0.0, 0.0, 0.0]);
            let (p, n) = skin_vertex(&skin, nodes, weights, piece.positions[i], n);
            let (j, w) = joint_weights(nodes, weights, skin.len());
            prim.joints.push(j);
            prim.weights.push(w);
            let uv = piece.uvs.get(i).copied().unwrap_or([0.0; 2]);
            prim.positions.push(p);
            prim.normals.push(n);
            prim.uvs.push(uv);
            if any_cc {
                prim.colours.push(match cc {
                    Some(map) => change_colour(map, uv),
                    None => [0.0, 0.0, 0.0, 1.0],
                });
            }
        }
        for t in &piece.triangles {
            if t.iter().all(|&i| (i as usize) < piece.positions.len()) {
                prim.indices.extend(t.map(|i| base + i));
            }
        }
    }
    prims
        .into_values()
        .filter(|p| !p.indices.is_empty())
        .collect()
}

/// Names a biped's empty nodes are looked up by, in order of preference.
pub const WEAPON_MARKERS: &[&str] = &["right_hand", "right_hand_elite"];
pub const WEAPON_BONES: &[&str] = &["r_hand", "right_hand"];
pub const HEAD_MARKERS: &[&str] = &["head"];
pub const HEAD_BONES: &[&str] = &["head"];
pub const MUZZLE_MARKERS: &[&str] = &["primary_trigger", "primary trigger", "primary_muzzle"];

/// One animation as sampled poses, ready to become a [`Clip`].
#[derive(Clone, Debug, Default)]
pub struct ClipPoses {
    /// `<weapon_class>:<state>`.
    pub name: String,
    pub fps: f32,
    /// The node names of the graph the poses come from (see [`posed_world`]).
    pub graph_names: Vec<String>,
    pub poses: Vec<Pose>,
    /// An overlay: each frame's change from the first is added to the
    /// default pose (a weapon's recoil over the idle).
    pub overlay: bool,
}

/// Quaternion product `a * b` (`[x, y, z, w]`).
pub fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

fn quat_conj(q: [f32; 4]) -> [f32; 4] {
    [-q[0], -q[1], -q[2], q[3]]
}

/// A bone's translation and rotation relative to its parent.
type Local = ([f32; 3], [f32; 4]);

/// Each bone's channels in one frame: the frame's own where it has them,
/// else `base`'s. For an overlay, the frame's change since `first` is
/// applied to `base` instead.
pub fn clip_locals(
    bones: &[Bone],
    graph_names: &[String],
    pose: &Pose,
    base: &[Local],
    overlay: Option<&Pose>,
) -> Vec<Mat4> {
    let slot = graph_slots(bones, graph_names);
    (0..bones.len())
        .map(|b| {
            let (mut t, mut q) = base[b];
            let node = slot[b].and_then(|j| Some((pose.nodes.get(j)?, j)));
            if let Some((n, j)) = node {
                match overlay.and_then(|f| f.nodes.get(j)) {
                    Some(first) => {
                        if n.has_translation && first.has_translation {
                            t = std::array::from_fn(|k| {
                                t[k] + n.translation[k] - first.translation[k]
                            });
                        }
                        if n.has_rotation && first.has_rotation {
                            q = quat_mul(q, quat_mul(quat_conj(first.rotation), n.rotation));
                        }
                    }
                    None => {
                        if n.has_translation {
                            t = n.translation;
                        }
                        if n.has_rotation {
                            q = n.rotation;
                        }
                    }
                }
            }
            rigid(&trs(t, q, 1.0))
        })
        .collect()
}

/// Each bone's channels in `pose`, its rest values where it has none.
fn base_locals(bones: &[Bone], graph_names: &[String], pose: &Pose) -> Vec<Local> {
    let slot = graph_slots(bones, graph_names);
    bones
        .iter()
        .zip(slot)
        .map(|(b, j)| {
            let n = j.and_then(|j| pose.nodes.get(j));
            (
                n.filter(|n| n.has_translation)
                    .map_or(b.translation, |n| n.translation),
                n.filter(|n| n.has_rotation)
                    .map_or(b.rotation, |n| n.rotation),
            )
        })
        .collect()
}

/// A frame without its scale: the rotation columns made orthonormal.
pub fn rigid(m: &Mat4) -> Mat4 {
    let x = normalize([m[0][0], m[0][1], m[0][2]]);
    let z = normalize(cross(x, [m[1][0], m[1][1], m[1][2]]));
    let y = cross(z, x);
    [
        [x[0], x[1], x[2], 0.0],
        [y[0], y[1], y[2], 0.0],
        [z[0], z[1], z[2], 0.0],
        m[3],
    ]
}

/// A biped posed by one animation frame, standing on the origin: the mesh,
/// and `weapon` and `head` nodes from the markers (or bones) of those names.
/// The mesh is skinned to the render model's nodes, whose default pose is
/// that frame, and `clips` animate them; `weapon` and `head` ride on their
/// nodes.
pub fn biped(
    src: &Source,
    graph_names: &[String],
    pose: &Pose,
    clips: &[ClipPoses],
) -> Result<Model> {
    let posed = posed_world(&src.bones, graph_names, pose);
    let mut model = Model {
        mesh_name: "body".into(),
        primitives: pose_mesh(src, &posed),
        materials: src.materials.clone(),
        ..Model::default()
    };
    if model.primitives.is_empty() {
        anyhow::bail!("no drawn triangles");
    }
    let frame = |markers: &[&str], bones: &[&str]| -> Option<(Mat4, usize)> {
        match src.marker(markers) {
            Some(m) if m.node < posed.len() => Some((marker_world(&posed[m.node], m), m.node)),
            _ => src.bone(bones).map(|b| (posed[b], b)),
        }
    };
    // Creatures without the usual markers fall back to a name with `hand`
    // (right first), then the root; the head to the top of the mesh.
    let (lo, hi) = model.bounds().expect("non-empty");
    let weapon = frame(WEAPON_MARKERS, WEAPON_BONES)
        .or_else(|| {
            let hand = |right: bool| {
                src.bones.iter().position(|b| {
                    let n = b.name.to_ascii_lowercase();
                    n.contains("hand") && (!right || n.starts_with('r') || n.contains("right"))
                })
            };
            hand(true).or_else(|| hand(false)).map(|b| (posed[b], b))
        })
        .map_or_else(
            || (posed.first().copied().unwrap_or(IDENTITY), Some(0)),
            |(m, b)| (m, Some(b)),
        );
    let head = frame(HEAD_MARKERS, HEAD_BONES).map_or_else(
        || {
            let mut m = IDENTITY;
            m[3] = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, hi[2], 1.0];
            (m, None)
        },
        |(m, b)| (m, Some(b)),
    );
    model.nodes.push(("weapon".into(), weapon.0));
    model.nodes.push(("head".into(), head.0));
    model.skeleton = skeleton(src, graph_names, pose, clips);
    if let Some(sk) = &mut model.skeleton {
        for (name, joint) in [("weapon", weapon.1), ("head", head.1)] {
            if let Some(j) = joint.filter(|&j| j < sk.joints.len()) {
                sk.attach.insert(name.into(), j);
            }
        }
    }
    model.translate([0.0, 0.0, -lo[2]]);
    model.extras.insert("height".into(), json!(hi[2] - lo[2]));
    Ok(model)
}

/// The render model's nodes as joints in the pose `pose`, and the clips.
/// A node whose parent comes after it is made a root, so the joints form a
/// tree; `None` without nodes.
fn skeleton(
    src: &Source,
    graph_names: &[String],
    pose: &Pose,
    clips: &[ClipPoses],
) -> Option<Skeleton> {
    if src.bones.is_empty() {
        return None;
    }
    let locals = posed_locals(&src.bones, graph_names, pose);
    let joints = src
        .bones
        .iter()
        .zip(&locals)
        .enumerate()
        .map(|(j, (b, l))| Joint {
            name: b.name.clone(),
            parent: b.parent.filter(|&p| p < j),
            local: rigid(l),
        })
        .collect();
    // Channels a clip lacks (an upper-body replacement's legs) keep the
    // default pose's.
    let base = base_locals(&src.bones, graph_names, pose);
    let clips = clips
        .iter()
        .filter(|c| !c.poses.is_empty() && c.fps > 0.0)
        .map(|c| Clip {
            name: c.name.clone(),
            fps: c.fps,
            frames: c
                .poses
                .iter()
                .map(|p| {
                    let first = c.overlay.then(|| &c.poses[0]);
                    clip_locals(&src.bones, &c.graph_names, p, &base, first)
                })
                .collect(),
        })
        .collect();
    Some(Skeleton {
        joints,
        attach: BTreeMap::new(),
        clips,
    })
}

/// A weapon in its rest pose and own model space, with a `muzzle` node.
pub fn weapon(src: &Source) -> Result<Model> {
    let rest = rest_world(&src.bones);
    let mut model = Model {
        mesh_name: "weapon".into(),
        primitives: pose_mesh(src, &rest),
        materials: src.materials.clone(),
        ..Model::default()
    };
    if model.primitives.is_empty() {
        anyhow::bail!("no drawn triangles");
    }
    // The weapon's colour comes from its texture; it has no team colour.
    for p in &mut model.primitives {
        p.colours.clear();
    }
    let muzzle = match src.marker(MUZZLE_MARKERS) {
        Some(m) if m.node < rest.len() => marker_world(&rest[m.node], m),
        _ => {
            // No marker: the front of the model, at mid height.
            let (lo, hi) = model.bounds().expect("non-empty");
            let mut m = IDENTITY;
            m[3] = [hi[0], (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0, 1.0];
            m
        }
    };
    model.nodes.push(("muzzle".into(), muzzle));
    Ok(model)
}

/// Fewer drawn triangles than this: not a real vehicle model.
pub const MIN_VEHICLE_TRIANGLES: usize = 8;

/// Marker names that are seats: the first word is a seat word and no word
/// says it is something else at the seat (entry point, camera, hand, ...).
/// Generic words, matched at run time.
const SEAT_WORDS: &[&str] = &["driver", "gunner", "passenger", "seat", "pilot", "rider"];
const NOT_SEAT_WORDS: &[&str] = &[
    "enter",
    "entry",
    "exit",
    "camera",
    "target",
    "damage",
    "damaged",
    "destroyed",
    "grenade",
    "cinematic",
    "effect",
    "hand",
    "foot",
    "light",
];

/// The `seat_<name>` node name of a marker, if the marker is a seat.
pub fn seat_node(marker: &str) -> Option<String> {
    let lower = marker.to_ascii_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let first = words.first()?;
    // `enter01` is `enter`.
    let bare =
        |w: &&str| NOT_SEAT_WORDS.contains(&w.trim_end_matches(|c: char| c.is_ascii_digit()));
    if !SEAT_WORDS.contains(first) || words.iter().any(bare) {
        return None;
    }
    let rest: Vec<&str> = if *first == "seat" && words.len() > 1 {
        words[1..].to_vec()
    } else {
        words
    };
    let id = format!("seat_{}", rest.join("_"));
    (id.len() <= 40).then_some(id)
}

/// A vehicle in its own model space (its object origin), posed by `pose`
/// (empty: the rest pose; see [`pick_vehicle_pose`]), with a `seat_<name>`
/// node per seat marker (first marker of a name).
pub fn vehicle(src: &Source, graph_names: &[String], pose: &Pose) -> Result<Model> {
    let rest = posed_world(&src.bones, graph_names, pose);
    let mut model = Model {
        mesh_name: "vehicle".into(),
        primitives: pose_mesh(src, &rest),
        materials: src.materials.clone(),
        ..Model::default()
    };
    // Some vehicle tags are placeholders around a single triangle.
    if model.triangles() < MIN_VEHICLE_TRIANGLES {
        anyhow::bail!("{} drawn triangles: a placeholder", model.triangles());
    }
    for m in &src.markers {
        let Some(name) = seat_node(&m.name) else {
            continue;
        };
        if m.node >= rest.len() || model.nodes.iter().any(|(n, _)| *n == name) {
            continue;
        }
        model.nodes.push((name, marker_world(&rest[m.node], m)));
    }
    Ok(model)
}

// ---- files -----------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Biped,
    Weapon,
    Vehicle,
}

impl Kind {
    fn dir(self) -> &'static str {
        match self {
            Kind::Biped => "biped",
            Kind::Weapon => "weapon",
            Kind::Vehicle => "vehicle",
        }
    }
}

/// Counts for the report.
#[derive(Clone, Debug)]
pub struct ModelStats {
    pub key: String,
    pub triangles: usize,
    pub glb_bytes: usize,
}

/// What [`write_models`] (and [`extract`]) did.
#[derive(Debug, Default)]
pub struct Written {
    pub entry: Option<ModelsEntry>,
    pub skipped: BTreeMap<String, String>,
    pub stats: Vec<ModelStats>,
}

/// Writes every model that was built under `dir/models/`, and lists every
/// one that was not under `skipped` as `models/<kind>/<name>`.
pub fn write_models(built: Vec<(Kind, String, Result<Model>)>, dir: &Path) -> Written {
    let mut out = Written::default();
    let (mut bipeds, mut weapons, mut vehicles) = (Vec::new(), Vec::new(), Vec::new());
    let mut extra = Vec::new();
    for (kind, name, model) in built {
        let key = format!("models/{}/{name}", kind.dir());
        let model = match model {
            Ok(m) => m,
            Err(e) => {
                out.skipped.insert(key, format!("{e:#}"));
                continue;
            }
        };
        let glb = write_glb(&model);
        let path = dir.join(format!("{key}.glb"));
        let written = path
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| fs::write(&path, &glb));
        if let Err(e) = written {
            out.skipped.insert(key, format!("writing: {e}"));
            continue;
        }
        out.stats.push(ModelStats {
            key,
            triangles: model.triangles(),
            glb_bytes: glb.len(),
        });
        let clips: Vec<String> = model
            .skeleton
            .iter()
            .flat_map(|s| s.clips.iter().map(|c| c.name.clone()))
            .collect();
        match kind {
            Kind::Biped => bipeds.push(name.clone()),
            Kind::Weapon => weapons.push(name.clone()),
            Kind::Vehicle => vehicles.push(name.clone()),
        }
        extra.push((kind, name, clips, model.class, model.ballistics));
    }
    out.entry = ModelsEntry::new(&bipeds, &weapons, &vehicles);
    if let Some(entry) = &mut out.entry {
        for (kind, name, clips, class, ballistics) in extra {
            let table = match kind {
                Kind::Biped => &mut entry.bipeds,
                Kind::Weapon => &mut entry.weapons,
                Kind::Vehicle => &mut entry.vehicles,
            };
            if let Some(r) = table.get_mut(&name) {
                r.clips = clips;
                r.class = class;
                r.ballistics = ballistics;
            }
        }
    }
    out
}

#[cfg(test)]
#[path = "live_models_tests.rs"]
mod tests;

// ---- the disc ---------------------------------------------------------------

type MapReader = Map<BufReader<File>>;

/// The species written, and the biped tag file stems they come from (the
/// multiplayer one first). Matched against tag names read at run time.
pub const SPECIES: &[(&str, &[&str])] = &[
    ("spartan", &["masterchief_mp", "masterchief"]),
    ("elite", &["elite_mp", "elite"]),
];
/// Weapons are the `weap` tags under this folder (not vehicle guns), minus
/// fixed emplacements.
const WEAPON_FOLDER: &str = "objects\\weapons\\";
const FIXED_FOLDER: &str = "\\fixed\\";
/// The pose: mode, weapon classes in order of preference, weapon type,
/// action, frame.
const POSE_MODE: &str = "combat";
const POSE_CLASSES: &[&str] = &["rifle", "pistol", "unarmed"];
const POSE_TYPE: &str = "any";
const POSE_ACTION: &str = "idle";
/// Triangle budget per biped: the highest detail level under it is used.
pub const MAX_BIPED_TRIANGLES: usize = 15_000;

const BIPD: FourCC = FourCC(*b"bipd");
const WEAP: FourCC = FourCC(*b"weap");

/// Raw blocks and animation resources through the art extractor's files.
struct Res<'a>(&'a mut ResourceFiles);
impl Res<'_> {
    fn read(&mut self, p: RawPointer, size: u32) -> tags::Result<Vec<u8>> {
        self.0.read(p.file, p.offset, size).map_err(|e| match e {
            assets::Error::Map(e) => tags::Error::Map(e),
            assets::Error::Io(e) => tags::Error::Map(e.into()),
            assets::Error::MissingResource(f) => tags::Error::MissingResource(f),
            _ => tags::Error::Invalid("raw read rejected"),
        })
    }
}
impl RawSource for Res<'_> {
    fn read_raw(&mut self, p: RawPointer, size: u32) -> tags::Result<Vec<u8>> {
        self.read(p, size)
    }
}
impl ResourceReader for Res<'_> {
    fn check(&self, _: RawPointer, _: u32) -> tags::Result<()> {
        // `read` checks bounds and the size cap itself.
        Ok(())
    }
    fn read_raw(&mut self, p: RawPointer, size: u32) -> tags::Result<Vec<u8>> {
        self.read(p, size)
    }
}

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}
fn f32s<const N: usize>(b: &[u8], o: usize) -> Option<[f32; N]> {
    let mut out = [0.0; N];
    for (k, v) in out.iter_mut().enumerate() {
        *v = f32::from_bits(u32_at(b, o + 4 * k)?);
    }
    Some(out)
}

/// An object tag's render model and animation graph: `obje +0x34` is its
/// `hlmt` reference (docs/objects.md), whose `+0x00` is the render model and
/// `+0x10` the animation graph (datums at +4 of each reference).
fn object_model(map: &mut MapReader, object: TagId) -> Result<(TagId, Option<TagId>)> {
    let b = map.read_tag_bytes(object)?;
    let hlmt = TagId(u32_at(&b, 0x38).context("object too short")?);
    let h = map
        .read_tag_bytes(hlmt)
        .context("reading the object's model")?;
    let mode = TagId(u32_at(&h, 4).context("model too short")?);
    let jmad = TagId(u32_at(&h, 0x14).context("model too short")?);
    let jmad = (jmad != TagId::NULL && map.tag(jmad).is_ok()).then_some(jmad);
    Ok((mode, jmad))
}

/// Render-model markers: header `+0x58` is a block of marker groups (0xc
/// bytes: string-ID name, block of markers); a marker is 0x24 bytes with the
/// node index at `+2`, translation at `+4` and rotation at `+0x10`. D: on the
/// retail bipeds and weapons every name resolves and every node index is in
/// range (checked on the retail disc).
fn markers(map: &mut MapReader, mode: TagId, nodes: usize) -> Result<Vec<Marker>> {
    let address = map.tag(mode)?.location.context("no payload")?.address;
    let bytes = map.read_tag_bytes(mode)?;
    let view = TagView::new(&bytes, address);
    let strings = map.strings();
    let mut out = Vec::new();
    for group in view.block(&bytes[..0x84.min(bytes.len())], 0x58, 0xc)? {
        let name = StringId(u32_at(group, 0).unwrap_or(u32::MAX))
            .resolve(strings)?
            .unwrap_or_default()
            .to_owned();
        for m in view.block(group, 4, 0x24)? {
            let node = usize::from(m[2]);
            if node >= nodes {
                return Err(anyhow!("marker {name} on node {node} of {nodes}"));
            }
            out.push(Marker {
                name: name.clone(),
                node,
                translation: f32s(m, 4).context("marker")?,
                rotation: f32s(m, 0x10).context("marker")?,
            });
        }
    }
    Ok(out)
}

/// A shader's bitmaps: `shad +0x20` is the compiled postprocess (one
/// element), whose `+4` block lists the bitmap parameters (0xc bytes, datum
/// first; docs/shader-instances.md).
pub(crate) fn shader_bitmaps(map: &mut MapReader, shader: TagId) -> Result<(String, Vec<TagId>)> {
    let tag = map.tag(shader)?.clone();
    let address = tag.location.context("no payload")?.address;
    let bytes = map.read_tag_bytes(shader)?;
    let view = TagView::new(&bytes, address);
    let template = TagId(u32_at(&bytes, 4).context("shader too short")?);
    let template = map
        .tag(template)
        .ok()
        .and_then(|t| t.name.clone())
        .unwrap_or_default();
    let post = view.block(&bytes[..0x54.min(bytes.len())], 0x20, 0x7c)?;
    let post = post.first().context("no compiled postprocess")?;
    let ids = view
        .block(post, 4, 0xc)?
        .iter()
        .filter_map(|b| u32_at(b, 0).map(TagId))
        .collect();
    Ok((template, ids))
}

fn last_component(name: &str) -> &str {
    name.rsplit('\\').next().unwrap_or(name)
}

/// What a shader's bitmap is for, from words in its tag name read at run
/// time: change-colour maps end in `_cc` or say `change_color` (or
/// `color_change`); self-illumination maps say `illum`; the diffuse map is
/// the first bitmap that is not one of those or a bump, detail, reflection
/// or engine map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BitmapRole {
    Diffuse,
    ChangeColour,
    SelfIllum,
    Other,
}

pub fn bitmap_role(name: &str) -> BitmapRole {
    let lower = name.to_ascii_lowercase();
    let last = last_component(&lower);
    let engine = ["default_bitmaps", "rasterizer"];
    if lower.split('\\').any(|c| engine.contains(&c)) {
        return BitmapRole::Other;
    }
    if last.ends_with("_cc")
        || [
            "change_color",
            "change_colour",
            "color_change",
            "colour_change",
        ]
        .iter()
        .any(|w| last.contains(w))
    {
        return BitmapRole::ChangeColour;
    }
    if last.contains("illum") && !last.contains("bump") {
        return BitmapRole::SelfIllum;
    }
    let folders = [
        "detail_maps",
        "reflection_maps",
        "default_bitmaps",
        "rasterizer",
        "cube",
    ];
    let words = [
        "bump",
        "detail",
        "reflection",
        "cube",
        "fade",
        "normal",
        "illum",
        "self",
        "specular",
        "mask",
    ];
    if folders.iter().any(|f| lower.split('\\').any(|c| c == *f))
        || words.iter().any(|w| last.contains(w))
    {
        return BitmapRole::Other;
    }
    BitmapRole::Diffuse
}

/// The first image of a bitmap tag, decoded to RGBA.
pub(crate) fn decode_bitmap(
    map: &mut MapReader,
    files: &mut ResourceFiles,
    id: TagId,
) -> Result<Image> {
    let bitmap = match DecodedTag::read(map, id)?.data {
        TagData::Bitmap(b) => b,
        _ => return Err(anyhow!("not a bitmap")),
    };
    let image = bitmap.images.first().context("bitmap has no images")?;
    let decoded = assets::decode_image(image, files)?;
    let base = decoded
        .levels
        .into_iter()
        .find(|l| l.face == 0)
        .context("no decoded level")?;
    Ok(Image {
        width: base.width,
        height: base.height,
        rgba: base.rgba,
    })
}

/// A vertex's model nodes from its section-local indices through the
/// section's node map. A vertex without indices (a section whose vertices
/// carry no nodes) belongs to the section's first mapped node: such a
/// section is a rigid piece moved by that one node (a banshee's cab, a
/// wraith's hatch), not by the root.
pub fn vertex_nodes(node_map: &[u8], local: Option<[u8; 4]>) -> [usize; 4] {
    let node = |l: u8| -> usize {
        if node_map.is_empty() {
            usize::from(l)
        } else {
            node_map.get(usize::from(l)).map_or(0, |&m| usize::from(m))
        }
    };
    local.unwrap_or([0; 4]).map(node)
}

/// The pieces of the chosen sections, with section-local node indices
/// mapped to model nodes and unused vertices dropped.
fn pieces(geo: &RenderModelGeometry, sections: &[usize], skip: &[bool]) -> Vec<Piece> {
    let mut out = Vec::new();
    for &si in sections {
        let Some(section) = geo.sections.get(si) else {
            continue;
        };
        let v = &section.vertices;
        let n = v.positions.len();
        for part in section.geometry.parts.iter().filter(|p| p.is_drawn()) {
            let material = usize::from(part.material);
            if skip.get(material).copied().unwrap_or(false) {
                continue;
            }
            let mut piece = Piece {
                material,
                ..Piece::default()
            };
            let mut remap: BTreeMap<u16, u32> = BTreeMap::new();
            for t in section.geometry.part_triangles(part) {
                if t.iter().any(|&i| usize::from(i) >= n)
                    || t[0] == t[1]
                    || t[1] == t[2]
                    || t[0] == t[2]
                {
                    continue;
                }
                let tri = t.map(|i| {
                    *remap.entry(i).or_insert_with(|| {
                        let i = usize::from(i);
                        piece.positions.push(v.positions[i]);
                        piece
                            .normals
                            .push(v.normals.get(i).copied().unwrap_or([0.0, 0.0, 1.0]));
                        piece
                            .uvs
                            .push(v.texcoords.get(i).copied().unwrap_or([0.0; 2]));
                        piece.nodes.push(vertex_nodes(
                            &section.node_map,
                            v.node_indices.get(i).copied(),
                        ));
                        piece.weights.push(
                            v.node_weights
                                .get(i)
                                .copied()
                                .unwrap_or([1.0, 0.0, 0.0, 0.0]),
                        );
                        (piece.positions.len() - 1) as u32
                    })
                });
                piece.triangles.push(tri);
            }
            if !piece.triangles.is_empty() {
                out.push(piece);
            }
        }
    }
    out
}

/// Per region, the permutation named `base` (else the first) at detail
/// level `lod` (else the nearest lower one present, else its best).
fn chosen_sections(geo: &RenderModelGeometry, lod: usize) -> Vec<usize> {
    let mut out = Vec::new();
    for r in &geo.regions {
        let perm = r
            .permutations
            .iter()
            .find(|p| p.name.as_deref() == Some("base"))
            .or_else(|| r.permutations.first());
        let Some(perm) = perm else { continue };
        let s = perm.lod_sections[..=lod]
            .iter()
            .rev()
            .flatten()
            .next()
            .copied()
            .or_else(|| perm.best_section());
        if let Some(s) = s
            && !out.contains(&s)
        {
            out.push(s);
        }
    }
    out
}

/// A render model as [`Source`], at the highest detail level whose drawn
/// triangles fit `budget`.
pub(crate) fn read_source(
    map: &mut MapReader,
    files: &mut ResourceFiles,
    mode: TagId,
    budget: usize,
) -> Result<Source> {
    let geo = RenderModelGeometry::read(map, mode, &mut Res(files))
        .context("reading the render model")?;
    let bones: Vec<Bone> = geo
        .nodes
        .iter()
        .map(|n| Bone {
            name: n.name.clone().unwrap_or_default(),
            parent: usize::try_from(n.wire.parent_index).ok(),
            translation: n.wire.translation,
            rotation: n.wire.orientation,
        })
        .collect();
    let markers = markers(map, mode, bones.len())?;
    let mut materials = Vec::new();
    let mut change_colour = Vec::new();
    let mut skip = Vec::new();
    for (i, m) in geo.materials.iter().enumerate() {
        let shader = m.shader.id;
        let name = map
            .tag(shader)
            .ok()
            .and_then(|t| t.name.clone())
            .map_or_else(|| format!("material{i}"), |n| last_component(&n).to_owned());
        let (template, bitmaps) = shader_bitmaps(map, shader).unwrap_or_default();
        // Emblem decals sit on the armour and take the player's emblem.
        skip.push(last_component(&template).contains("emblem"));
        let mut found = Vec::new();
        for id in bitmaps {
            let Some(bname) = map.tag(id).ok().and_then(|t| t.name.clone()) else {
                continue;
            };
            // Only the roles a look uses are decoded, each once.
            let role = bitmap_role(&bname);
            let wanted = role != BitmapRole::Other
                && !found
                    .iter()
                    .any(|(n, i): &(String, Option<Image>)| i.is_some() && bitmap_role(n) == role);
            let image = if wanted {
                decode_bitmap(map, files, id).ok()
            } else {
                None
            };
            found.push((bname, image));
        }
        let (material, cc) = material_look(&name, &template, found);
        materials.push(material);
        change_colour.push(cc);
    }
    let mut chosen = Vec::new();
    for lod in (0..6).rev() {
        chosen = pieces(&geo, &chosen_sections(&geo, lod), &skip);
        let tris: usize = chosen.iter().map(|p| p.triangles.len()).sum();
        if tris <= budget {
            break;
        }
    }
    Ok(Source {
        bones,
        markers,
        pieces: chosen,
        materials,
        change_colour,
    })
}

/// One action of a graph, by name: mode, weapon class, weapon type, action.
pub type Stance = [String; 4];

/// The action to pose a biped with: the rifle idle of the combat mode (else
/// the pistol or unarmed one); else any `idle` action, combat mode first;
/// else any action whose name contains `idle`. `None`: keep the rest pose.
pub fn pick_idle(stances: &[Stance]) -> Option<usize> {
    let find = |f: &dyn Fn(&Stance) -> bool| stances.iter().position(f);
    POSE_CLASSES
        .iter()
        .find_map(|c| {
            find(&|s| s[0] == POSE_MODE && s[1] == *c && s[2] == POSE_TYPE && s[3] == POSE_ACTION)
        })
        .or_else(|| find(&|s| s[0] == POSE_MODE && s[3] == POSE_ACTION))
        .or_else(|| find(&|s| s[3] == POSE_ACTION))
        .or_else(|| find(&|s| s[3].contains(POSE_ACTION)))
}

/// Every action of a graph that has an animation, by name, with the action.
fn graph_stances(graph: &AnimationGraph, strings: &[Option<String>]) -> Vec<(Stance, Action)> {
    let name = |id: StringId| id.resolve(strings).ok().flatten().unwrap_or("").to_owned();
    let mut out = Vec::new();
    for m in &graph.modes {
        for c in &m.classes {
            for t in &c.types {
                for a in &t.actions {
                    if a.animation_index >= 0 {
                        out.push(([name(m.name), name(c.name), name(t.name), name(a.name)], *a));
                    }
                }
            }
        }
    }
    out
}

/// The first frame of the action `pick` chooses from the graph (see
/// [`pick_idle`], [`pick_vehicle_pose`]) with the graph's node names, and
/// the action's name; the rest pose (no names) when it chooses none.
fn graph_pose(
    map: &mut MapReader,
    files: &mut ResourceFiles,
    jmad: TagId,
    pick: fn(&[Stance]) -> Option<(usize, Frame)>,
) -> Result<(Vec<String>, Pose, String)> {
    let graph = AnimationGraph::read(map, jmad).context("reading the animation graph")?;
    let strings = map.strings().to_vec();
    let (stances, actions): (Vec<Stance>, Vec<Action>) =
        graph_stances(&graph, &strings).into_iter().unzip();
    let Some((i, frame)) = pick(&stances) else {
        return Ok((Vec::new(), Pose { nodes: Vec::new() }, "rest pose".into()));
    };
    let action = actions[i];
    let target = if action.graph_index < 0 {
        graph
    } else {
        let r = graph
            .inheritance
            .get(action.graph_index as usize)
            .context("action graph index out of range")?;
        AnimationGraph::read(map, r.graph.id).context("reading the inherited graph")?
    };
    let clip = target.load_clip(action.animation_index as usize, &mut Res(files))?;
    let (n, label) = match frame {
        Frame::First => (0, "first frame"),
        Frame::Last => (clip.frame_count().saturating_sub(1), "last frame"),
    };
    let pose = clip.sample_frame(n)?;
    let names = target
        .nodes
        .iter()
        .map(|n| {
            n.name
                .resolve(&strings)
                .ok()
                .flatten()
                .unwrap_or("")
                .to_owned()
        })
        .collect();
    Ok((names, pose, format!("{}, {label}", stances[i].join(":"))))
}

fn idle_pose(
    map: &mut MapReader,
    files: &mut ResourceFiles,
    jmad: TagId,
) -> Result<(Vec<String>, Pose, String)> {
    graph_pose(map, files, jmad, |s| {
        pick_idle(s).map(|i| (i, Frame::First))
    })
}

/// Which frame of the chosen action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Frame {
    First,
    Last,
}

/// A vehicle's neutral pose. Vehicles with a hatch or canopy rest with it
/// open (a banshee's whole nose is tilted up), so the end of `closing` comes
/// first, then the start of `opening`; then an `idle` (turrets, sentinels);
/// `None` keeps the rest pose (most vehicles have no actions at all).
pub fn pick_vehicle_pose(stances: &[Stance]) -> Option<(usize, Frame)> {
    let find = |a: &str| {
        stances
            .iter()
            .position(|s| s[3] == a && s[0] == POSE_MODE)
            .or_else(|| stances.iter().position(|s| s[3] == a))
    };
    find("closing")
        .map(|i| (i, Frame::Last))
        .or_else(|| find("opening").map(|i| (i, Frame::First)))
        .or_else(|| find(POSE_ACTION).map(|i| (i, Frame::First)))
}

// ---- clips -------------------------------------------------------------------

/// The weapon classes a biped's clips cover: those multiplayer weapons are
/// held with (`dual`, the vehicle seats and the AI-only classes are left out).
pub const CLIP_CLASSES: &[&str] = &[
    "rifle", "pistol", "missile", "support", "sword", "unarmed", "flag", "ball",
];
/// Weapon types that stand in for their class where a state is animated only
/// per type (battle rifle, magnum, rocket launcher, brute shot); otherwise
/// the first type by name.
pub const CLIP_TYPES: &[&str] = &["br", "hp", "rl", "bs"];
/// The animation graphs' frame rate.
pub const ANIMATION_FPS: f32 = 30.0;

/// Where a state's animation is found.
#[derive(Clone, Copy, Debug)]
enum Want {
    /// An action of the graph: mode, the class (`None`: the class itself),
    /// the action.
    Action(&'static str, Option<&'static str>, &'static str),
    /// A combat animation of the class by name, any of these actions.
    Named(&'static [&'static str]),
}

/// The states of contract section 10 and where each comes from.
const STATES: &[(&str, Want)] = &[
    ("idle", Want::Action("combat", None, "idle")),
    ("walk", Want::Action("combat", None, "walk_front")),
    ("run", Want::Action("combat", None, "move_front")),
    ("crouch", Want::Action("crouch", None, "idle")),
    ("airborne", Want::Action("combat", None, "airborne")),
    ("land", Want::Action("combat", None, "land_soft")),
    ("land_hard", Want::Action("combat", None, "land_hard")),
    ("grenade", Want::Named(&["throw_grenade"])),
    ("melee", Want::Named(&["melee_strike_1", "melee"])),
    ("reload", Want::Named(&["reload_1"])),
    ("fire", Want::Named(&["fire_1"])),
    ("switch", Want::Named(&["ready"])),
];

/// The class every class shares: its clips are written once, as
/// `any:<state>`, and a reader falls back to them for any weapon class.
pub const SHARED_CLASS: &str = "any";

/// Where a shared state's animation is found.
#[derive(Clone, Copy, Debug)]
enum Shared {
    /// An animation by its whole name, any of these, maybe with `:varN`
    /// (the first variant by name).
    Named(&'static [&'static str]),
    /// An action of mode `combat`, class and type `any`.
    Action(&'static str),
}

/// The states every weapon class shares: dying (a death animation of the
/// graph's damage table, `s_kill` a soft kill from the front, else a hard
/// one, else the dead pose) and lying dead (`landing_dead`: the body on the
/// ground, held). Each state takes the first source the graph has.
const SHARED_STATES: &[(&str, &[Shared])] = &[
    (
        "die",
        &[
            Shared::Named(&["s_kill:front:gut", "h_kill:front:gut"]),
            Shared::Action("landing_dead"),
        ],
    ),
    ("dead", &[Shared::Action("landing_dead")]),
];

/// Where a clip's animation is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipFrom {
    /// An action, by index into the stances.
    Action(usize),
    /// An animation, by index into the names.
    Animation(usize),
}

/// The clips a graph can give, `(<class>:<state>, source)`, from its actions
/// ([`Stance`]s, type `any`) and its animations' names (`mode:class:action`
/// or `mode:class:type:action`, maybe with `:varN`). States missing from
/// the graph are left out.
pub fn plan_clips(stances: &[Stance], animations: &[String]) -> Vec<(String, ClipFrom)> {
    let mut out = Vec::new();
    // The shared clips first, so dropping classes for size never takes them.
    for (state, sources) in SHARED_STATES {
        let from = sources.iter().find_map(|source| match *source {
            Shared::Named(names) => whole_name(animations, names).map(ClipFrom::Animation),
            Shared::Action(action) => stances
                .iter()
                .position(|s| {
                    s[0] == POSE_MODE && s[1] == SHARED_CLASS && s[2] == POSE_TYPE && s[3] == action
                })
                .map(ClipFrom::Action),
        });
        if let Some(from) = from {
            out.push((format!("{SHARED_CLASS}:{state}"), from));
        }
    }
    for class in CLIP_CLASSES {
        for (state, want) in STATES {
            let from = match *want {
                Want::Action(mode, c, action) => stances
                    .iter()
                    .position(|s| {
                        s[0] == mode
                            && s[1] == c.unwrap_or(class)
                            && s[2] == POSE_TYPE
                            && s[3] == action
                    })
                    .map(ClipFrom::Action),
                Want::Named(actions) => {
                    named_animation(animations, class, actions).map(ClipFrom::Animation)
                }
            };
            if let Some(from) = from {
                out.push((format!("{class}:{state}"), from));
            }
        }
    }
    out
}

/// The animation named one of `names` (in that order of preference),
/// itself or a `:varN` of it, the first by name.
fn whole_name(animations: &[String], names: &[&str]) -> Option<usize> {
    names.iter().find_map(|base| {
        animations
            .iter()
            .enumerate()
            .filter(|(_, n)| {
                n.as_str() == *base
                    || n.strip_prefix(base)
                        .is_some_and(|rest| rest.starts_with(":var"))
            })
            .min_by(|a, b| a.1.cmp(b.1))
            .map(|(i, _)| i)
    })
}

/// The combat animation of `class` doing one of `actions`: the class's own
/// (or its `any` type's), else a [`CLIP_TYPES`] type's, else any type's.
fn named_animation(animations: &[String], class: &str, actions: &[&str]) -> Option<usize> {
    let is = |name: &str, base: &str| {
        name == base
            || name
                .strip_prefix(base)
                .is_some_and(|rest| rest.starts_with(":var"))
    };
    let find = |ty: Option<&str>| {
        actions.iter().find_map(|a| {
            let base = match ty {
                Some(t) => format!("combat:{class}:{t}:{a}"),
                None => format!("combat:{class}:{a}"),
            };
            animations.iter().position(|n| is(n, &base))
        })
    };
    find(None).or_else(|| find(Some(POSE_TYPE))).or_else(|| {
        CLIP_TYPES.iter().find_map(|t| find(Some(t))).or_else(|| {
            let mut types: Vec<&str> = animations
                .iter()
                .filter_map(|n| {
                    let rest = n.strip_prefix("combat:")?.strip_prefix(class)?;
                    let mut parts = rest.strip_prefix(':')?.split(':');
                    let ty = parts.next()?;
                    let action = parts.next()?;
                    actions.contains(&action).then_some(ty)
                })
                .collect();
            types.sort_unstable();
            types.into_iter().find_map(|t| find(Some(t)))
        })
    })
}

/// A graph and the graphs it inherits from, with every node and animation
/// name.
struct Graphs {
    graphs: Vec<AnimationGraph>,
    /// Per graph, its node names.
    nodes: Vec<Vec<String>>,
    /// Every animation of every graph, own first: `(graph, index, name)`.
    animations: Vec<(usize, usize, String)>,
}

impl Graphs {
    fn read(map: &mut MapReader, jmad: TagId) -> Result<Self> {
        let own = AnimationGraph::read(map, jmad).context("reading the animation graph")?;
        let mut graphs = vec![own];
        for i in 0..graphs[0].inheritance.len() {
            let id = graphs[0].inheritance[i].graph.id;
            graphs.push(AnimationGraph::read(map, id).context("reading an inherited graph")?);
        }
        let strings = map.strings();
        let name = |id: StringId| id.resolve(strings).ok().flatten().unwrap_or("").to_owned();
        let nodes = graphs
            .iter()
            .map(|g| g.nodes.iter().map(|n| name(n.name)).collect())
            .collect();
        let animations = graphs
            .iter()
            .enumerate()
            .flat_map(|(g, graph)| {
                graph
                    .animations
                    .iter()
                    .enumerate()
                    .map(move |(i, a)| (g, i, a.name))
            })
            .map(|(g, i, n)| (g, i, name(n)))
            .collect();
        Ok(Self {
            graphs,
            nodes,
            animations,
        })
    }

    /// The graph (by position) and animation an action plays.
    fn action(&self, a: &Action) -> Option<(usize, usize)> {
        let g = if a.graph_index < 0 {
            0
        } else {
            a.graph_index as usize + 1
        };
        (g < self.graphs.len()).then_some((g, a.animation_index as usize))
    }

    /// Every frame of an animation, sampled; aim grids give `None`.
    fn clip(
        &self,
        files: &mut ResourceFiles,
        (g, i): (usize, usize),
        name: String,
    ) -> Result<Option<ClipPoses>> {
        let graph = &self.graphs[g];
        let clip = graph.load_clip(i, &mut Res(files))?;
        if clip.is_aim_grid() || clip.frame_count() == 0 {
            return Ok(None);
        }
        let poses = (0..clip.frame_count())
            .map(|f| clip.sample_frame(f))
            .collect::<tags::Result<Vec<_>>>()?;
        Ok(Some(ClipPoses {
            name,
            fps: ANIMATION_FPS,
            graph_names: self.nodes[g].clone(),
            poses,
            overlay: graph.animations[i].anim_type == tags::anim::AnimationType::Overlay,
        }))
    }
}

/// A biped's clips (see [`plan_clips`]); an animation that fails to decode
/// is left out and named in the second list.
fn biped_clips(
    map: &mut MapReader,
    files: &mut ResourceFiles,
    jmad: TagId,
) -> Result<(Vec<ClipPoses>, Vec<String>)> {
    let graphs = Graphs::read(map, jmad)?;
    let strings = map.strings().to_vec();
    let (stances, actions): (Vec<Stance>, Vec<Action>) = graph_stances(&graphs.graphs[0], &strings)
        .into_iter()
        .unzip();
    let names: Vec<String> = graphs.animations.iter().map(|a| a.2.clone()).collect();
    let (mut clips, mut failed) = (Vec::new(), Vec::new());
    for (name, from) in plan_clips(&stances, &names) {
        let at = match from {
            ClipFrom::Action(i) => graphs.action(&actions[i]),
            ClipFrom::Animation(i) => Some((graphs.animations[i].0, graphs.animations[i].1)),
        };
        let Some(at) = at else { continue };
        match graphs.clip(files, at, name.clone()) {
            Ok(Some(c)) => clips.push(c),
            Ok(None) => {}
            Err(e) => failed.push(format!("{name}: {e}")),
        }
    }
    Ok((clips, failed))
}

/// What the disc gave for one biped, for the report and the disc test.
#[derive(Clone, Debug)]
pub struct BipedInfo {
    pub species: String,
    pub tag: String,
    pub map: String,
    pub animation: String,
}

/// The largest biped glb; past it whole weapon classes are dropped, last
/// of [`CLIP_CLASSES`] first. A quarter MiB over the contract's ~3 MB, so
/// the elite keeps its sword clips beside its change-colour, glow and
/// specular maps.
pub const MAX_BIPED_GLB: usize = (3 << 20) + (1 << 18);

/// One biped tag posed, skinned and animated. The animation note names the
/// pose, the clip count and any clips left out.
fn biped_from(
    map: &mut MapReader,
    files: &mut ResourceFiles,
    id: TagId,
) -> Result<(Model, String)> {
    let (mode, jmad) = object_model(map, id)?;
    let src = read_source(map, files, mode, MAX_BIPED_TRIANGLES)?;
    let Some(jmad) = jmad else {
        let model = biped(&src, &[], &Pose { nodes: Vec::new() }, &[])?;
        return Ok((model, "rest pose".into()));
    };
    let (names, pose, mut note) = idle_pose(map, files, jmad)?;
    let (mut clips, failed) = biped_clips(map, files, jmad)?;
    let mut dropped = Vec::new();
    let model = loop {
        let model = biped(&src, &names, &pose, &clips)?;
        if clips.is_empty() || write_glb(&model).len() <= MAX_BIPED_GLB {
            break model;
        }
        let last = clips.last().map(|c| clip_class(&c.name).to_owned());
        if let Some(class) = last {
            clips.retain(|c| clip_class(&c.name) != class);
            dropped.push(class);
        }
    };
    note += &format!("; {} clips", clips.len());
    if !dropped.is_empty() {
        note += &format!(", dropped {} for size", dropped.join(", "));
    }
    if !failed.is_empty() {
        note += &format!(", failed {}", failed.join(", "));
    }
    Ok((model, note))
}

/// The weapon class of a clip name (`rifle:run` -> `rifle`).
pub fn clip_class(name: &str) -> &str {
    name.split(':').next().unwrap_or(name)
}

fn build_biped(disc: &Disc, stems: &[&str]) -> Result<(Model, BipedInfo)> {
    for stem in stems {
        for m in disc.maps_of(CacheType::Multiplayer) {
            let (mut map, mut files) = disc.open(m)?;
            let found = map
                .tags()
                .iter()
                .find(|t| {
                    t.group == BIPD
                        && t.location.is_some()
                        && t.name
                            .as_deref()
                            .is_some_and(|n| last_component(n).eq_ignore_ascii_case(stem))
                })
                .map(|t| (t.id, t.name.clone().unwrap_or_default()));
            let Some((id, tag)) = found else { continue };
            let (model, animation) = biped_from(&mut map, &mut files, id)?;
            let info = BipedInfo {
                species: String::new(),
                tag,
                map: m.name().to_owned(),
                animation,
            };
            return Ok((model, info));
        }
    }
    Err(anyhow!(
        "no biped named {} on a multiplayer map",
        stems.join(" or ")
    ))
}

/// The species key of a biped tag: its directory's name, lower case
/// (`objects\\characters\\grunt\\grunt` -> `grunt`), as telemetry sends it.
pub fn species_of(tag: &str) -> Option<String> {
    let mut parts = tag.rsplit('\\');
    parts.next()?;
    let dir = parts.next()?.to_ascii_lowercase();
    is_id(&dir).then_some(dir)
}

/// Per species, the biped tag to draw it with, from every biped tag name and
/// the number of maps it is on: the one named like its directory, else the
/// most common, else the shortest name (then the first alphabetically).
pub fn choose_bipeds(tags: &BTreeMap<String, usize>) -> BTreeMap<String, String> {
    let mut by_species: BTreeMap<String, Vec<(&String, usize)>> = BTreeMap::new();
    for (name, &count) in tags {
        if let Some(sp) = species_of(name) {
            by_species.entry(sp).or_default().push((name, count));
        }
    }
    by_species
        .into_iter()
        .map(|(sp, mut names)| {
            names.sort_by(|a, b| {
                let own = |n: &str| !last_component(n).eq_ignore_ascii_case(&sp);
                own(a.0)
                    .cmp(&own(b.0))
                    .then(b.1.cmp(&a.1))
                    .then(a.0.len().cmp(&b.0.len()))
                    .then(a.0.cmp(b.0))
            });
            (sp, names[0].0.clone())
        })
        .collect()
}

/// Campaign characters: every biped tag on the campaign maps, one per
/// species (see [`choose_bipeds`]), skipping species in `taken`.
fn build_ai_bipeds(disc: &Disc, taken: &[&str]) -> Vec<(String, Result<(Model, BipedInfo)>)> {
    let maps: Vec<_> = disc.maps_of(CacheType::Campaign).collect();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut first: BTreeMap<String, usize> = BTreeMap::new();
    for (i, m) in maps.iter().enumerate() {
        let Ok((map, _)) = disc.open(m) else { continue };
        for t in map
            .tags()
            .iter()
            .filter(|t| t.group == BIPD && t.location.is_some())
        {
            if let Some(n) = &t.name {
                *counts.entry(n.clone()).or_default() += 1;
                first.entry(n.clone()).or_insert(i);
            }
        }
    }
    let chosen: Vec<(String, String)> = choose_bipeds(&counts)
        .into_iter()
        .filter(|(sp, _)| !taken.contains(&sp.as_str()))
        .collect();
    let build = |(species, tag): &(String, String)| -> (String, Result<(Model, BipedInfo)>) {
        let m = maps[first[tag]];
        let result = (|| {
            let (mut map, mut files) = disc.open(m)?;
            let id = map.tag(map::TagQuery::NameInGroup(tag, BIPD))?.id;
            let (model, animation) = biped_from(&mut map, &mut files, id)?;
            Ok((
                model,
                BipedInfo {
                    species: species.clone(),
                    tag: tag.clone(),
                    map: m.name().to_owned(),
                    animation,
                },
            ))
        })();
        (species.clone(), result)
    };
    parallel(&chosen, build)
}

/// `f` over `items` on all CPUs, results in order.
fn parallel<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let chunk = items.len().div_ceil(threads).max(1);
    let f = &f;
    std::thread::scope(|s| {
        let workers: Vec<_> = items
            .chunks(chunk)
            .map(|c| s.spawn(move || c.iter().map(f).collect::<Vec<_>>()))
            .collect();
        workers
            .into_iter()
            .flat_map(|w| w.join().expect("a model builder panicked"))
            .collect()
    })
}

/// Triangle budget per vehicle (the highest detail level under it).
pub const MAX_VEHICLE_TRIANGLES: usize = 40_000;
const VEHI: FourCC = FourCC(*b"vehi");

/// Every vehicle tag of the shared, multiplayer and campaign maps, by file
/// stem, first one found.
fn build_vehicles(disc: &Disc) -> Vec<(String, Result<Model>)> {
    let maps: Vec<_> = disc
        .maps_of(CacheType::Shared)
        .chain(disc.maps_of(CacheType::Multiplayer))
        .chain(disc.maps_of(CacheType::Campaign))
        .collect();
    let mut chosen: Vec<(String, usize, String)> = Vec::new();
    for (i, m) in maps.iter().enumerate() {
        let Ok((map, _)) = disc.open(m) else { continue };
        for t in map
            .tags()
            .iter()
            .filter(|t| t.group == VEHI && t.location.is_some())
        {
            let Some(name) = &t.name else { continue };
            let stem = last_component(name).to_ascii_lowercase();
            if is_id(&stem) && !chosen.iter().any(|(s, ..)| *s == stem) {
                chosen.push((stem, i, name.clone()));
            }
        }
    }
    parallel(&chosen, |(stem, i, tag)| {
        let result = (|| {
            let (mut map, mut files) = disc.open(maps[*i])?;
            let id = map.tag(map::TagQuery::NameInGroup(tag, VEHI))?.id;
            let (mode, jmad) = object_model(&mut map, id)?;
            let src = read_source(&mut map, &mut files, mode, MAX_VEHICLE_TRIANGLES)?;
            let (names, pose, animation) = match jmad {
                Some(jmad) => graph_pose(&mut map, &mut files, jmad, pick_vehicle_pose)?,
                None => (Vec::new(), Pose { nodes: Vec::new() }, "rest pose".into()),
            };
            let mut model = vehicle(&src, &names, &pose)?;
            model.extras.insert("pose".into(), json!(animation));
            Ok(model)
        })();
        (stem.clone(), result)
    })
}

/// Every multiplayer weapon: the `weap` tags under the weapons folder of the
/// shared map and the multiplayer maps, by file stem, first one found.
fn build_weapons(disc: &Disc) -> Vec<(String, Result<Model>)> {
    let mut out: Vec<(String, Result<Model>)> = Vec::new();
    let maps = disc
        .maps_of(CacheType::Shared)
        .chain(disc.maps_of(CacheType::Multiplayer));
    for m in maps {
        let Ok((mut map, mut files)) = disc.open(m) else {
            continue;
        };
        let weapons: Vec<(TagId, String)> = map
            .tags()
            .iter()
            .filter(|t| t.group == WEAP && t.location.is_some())
            .filter_map(|t| Some((t.id, t.name.clone()?)))
            .filter(|(_, n)| {
                let n = n.to_ascii_lowercase();
                n.starts_with(WEAPON_FOLDER) && !n.contains(FIXED_FOLDER)
            })
            .collect();
        for (id, name) in weapons {
            let stem = last_component(&name).to_ascii_lowercase();
            if out.iter().any(|(s, _)| *s == stem) || !is_id(&stem) {
                continue;
            }
            let class = weapon_class(&mut map, id);
            let ballistics = crate::live_weapons::ballistics(&mut map, id);
            let model = object_model(&mut map, id)
                .and_then(|(mode, _)| read_source(&mut map, &mut files, mode, usize::MAX))
                .and_then(|src| weapon(&src))
                .map(|mut m| {
                    m.class = class;
                    m.ballistics = ballistics;
                    m
                });
            out.push((stem, model));
        }
    }
    out
}

/// Offset in a `weap` tag of the string ID naming the animation graphs'
/// weapon class it is held with. D: on the disc every multiplayer weapon's
/// names a class of the player graphs (`rifle` for the battle rifle,
/// `missile` for the rocket launcher, `fixed` for vehicle guns).
const WEAPON_CLASS: usize = 0x288;

/// A weapon's class (see [`WEAPON_CLASS`]), when it reads as an id.
fn weapon_class(map: &mut MapReader, weapon: TagId) -> Option<String> {
    let b = map.read_tag_bytes(weapon).ok()?;
    let id = StringId(u32_at(&b, WEAPON_CLASS)?);
    let name = id.resolve(map.strings()).ok().flatten()?;
    is_id(name).then(|| name.to_owned())
}

/// The contract's id rule: `[a-z0-9_]{1,32}`.
pub fn is_id(s: &str) -> bool {
    (1..=32).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// Builds and writes the bipeds and weapons under `out/models/`.
pub fn extract(disc: &Disc, out: &Path) -> (Written, Vec<BipedInfo>) {
    let mut built = Vec::new();
    let mut infos = Vec::new();
    let mut add_biped = |built: &mut Vec<_>, species: String, r: Result<(Model, BipedInfo)>| {
        let model = r.map(|(m, mut info)| {
            info.species = species.clone();
            infos.push(info);
            m
        });
        built.push((Kind::Biped, species, model));
    };
    for (species, stems) in SPECIES {
        add_biped(&mut built, species.to_string(), build_biped(disc, stems));
    }
    // Campaign characters; the player species keep their multiplayer
    // models (`elite` is both).
    let taken: Vec<&str> = SPECIES.iter().map(|(s, _)| *s).collect();
    for (species, r) in build_ai_bipeds(disc, &taken) {
        add_biped(&mut built, species, r);
    }
    for (stem, model) in build_weapons(disc) {
        built.push((Kind::Weapon, stem, model));
    }
    for (stem, model) in build_vehicles(disc) {
        built.push((Kind::Vehicle, stem, model));
    }
    (write_models(built, out), infos)
}
