//! What the viewport draws when the map's art is at hand: the structure
//! BSP textured with each shader's base map, and the placed objects' render
//! models, read from the user's own map at run time. UI-free and GPU-free;
//! [`crate::render::Viewport::set_scene`] uploads it.
//!
//! - The base map of a shader is the first bitmap of its compiled
//!   postprocess whose tag name reads as diffuse ([`bitmap_role`]), the
//!   same rule as `cli::live_models` and `cli::live_looks` for the Live
//!   site's level art (docs/live/live-art.md, "Textures").
//! - Bitmaps are decoded by `assets::decode_image`, once each, shrunk to at
//!   most [`MAX_TEXTURE`] a side, with a box-filtered mip chain.
//! - Objects resolve through `tags::object` (object → `hlmt` → `mode`) and
//!   draw each region's first permutation at its best LOD
//!   (`RenderModelGeometry::default_sections`), in model space; the
//!   viewport places them by each placement's transform. The objects the
//!   model's variant attaches (a vehicle's turret is its own vehicle tag on
//!   the hull's `turret` marker) join the model at rest
//!   ([`object_model_of`], [`markers_in`]).
//!
//! - The BSP's lightmap pages ([`geometry::world_parts_lit`]) are decoded
//!   from `P8` through their group's palette ([`lightmap_page`]) and light
//!   the parts that have them; the rest are lit by the map's sun
//!   ([`Atmosphere`], from its sky), and placed objects by the lightmap of
//!   the floor under them ([`Scene::light_at`]).
//! - See-through shaders blend as their passes or template name say
//!   ([`material_blend`], [`template_blend`]); the sky's fog ([`Fog`])
//!   covers the level.
//! - [`Scene::materials`], [`Scene::lighting`] and [`Scene::atmosphere`]
//!   are what `view.stats` reports (docs/editor/editor.md, "What the
//!   viewport shows").
use crate::geometry::{
    self, Bounds, Marker, Ray, Vec3, WorldPart, add, dot, normalize, scale, sub,
};
use crate::layout::BlockKind;
use crate::save::MapSource;
use assets::{Region, ResourceFiles};
use map::{FourCC, Map, ResourceFile, TagId};
use std::collections::HashMap;
use std::io::{Read, Seek};
use tags::anim_sample::{Mat4, NodeTransform, mat_mul};
use tags::geometry::{RawSource, TagView};
use tags::model::RenderModelGeometry;

/// The largest side of an uploaded texture.
pub const MAX_TEXTURE: u32 = 512;
/// The mean linear brightness [`Image::lift_factor`] raises dark textures to.
pub const LIFT_TARGET: f32 = 0.16;
/// The most [`Image::lift_factor`] multiplies a texture by.
pub const MAX_LIFT: f32 = 3.0;
/// Placed models are skipped past this many triangles each (none on the
/// disc comes close; it bounds a hostile map).
pub const MAX_MODEL_TRIANGLES: usize = 200_000;

/// One vertex of a textured mesh.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SceneVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    /// Coordinates in the batch's lightmap page.
    pub lightmap_uv: [f32; 2],
}

/// How a batch is blended. The see-through ones ([`Blend::see_through`])
/// are drawn after everything solid, without depth writes, in this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Blend {
    /// Solid: the texture's alpha (often a specular mask) is ignored.
    Opaque,
    /// Cut out where the texture's alpha is low (grates, leaves).
    Mask,
    /// See-through at a fixed opacity: water, and see-through templates
    /// whose blend neither their passes nor their name tell.
    Translucent,
    /// Blended by the picture's alpha (`SRCALPHA`, `INVSRCALPHA`): clouds,
    /// glass, decals.
    Alpha,
    /// The picture times what is behind it (`DESTCOLOR`, `ZERO`).
    Multiply,
    /// Twice the picture times what is behind it (`DESTCOLOR`, `SRCCOLOR`):
    /// mid-grey leaves the surface as it is.
    Double,
    /// Added to what is behind it (`ONE` or `SRCALPHA`, `ONE`): lights,
    /// light shafts, glows. Black adds nothing.
    Additive,
}

impl Blend {
    /// Drawn after the solid surfaces, over them, without writing depth,
    /// and neither framed nor picked.
    pub fn see_through(self) -> bool {
        !matches!(self, Blend::Opaque | Blend::Mask)
    }

    /// The order a mesh's batches are drawn in: solid, then cut-out, then
    /// every see-through batch together, so blended and added layers keep
    /// the order of their parts (a sky's additive starfield under its
    /// alpha-blended planets).
    pub fn draw_order(self) -> u8 {
        match self {
            Blend::Opaque => 0,
            Blend::Mask => 1,
            _ => 2,
        }
    }

    /// Its name in `view.stats`.
    pub fn name(self) -> &'static str {
        match self {
            Blend::Opaque => "opaque",
            Blend::Mask => "mask",
            Blend::Translucent => "translucent",
            Blend::Alpha => "alpha",
            Blend::Multiply => "multiply",
            Blend::Double => "double",
            Blend::Additive => "additive",
        }
    }

    /// Every blend, in draw order.
    pub const ALL: [Blend; 7] = [
        Blend::Opaque,
        Blend::Mask,
        Blend::Translucent,
        Blend::Alpha,
        Blend::Multiply,
        Blend::Double,
        Blend::Additive,
    ];
}

/// How a shader template draws its base map, from its name: see-through
/// for templates in a `transparent` or `water` folder and for plain
/// `overlay` decals, cut out when the name says `alpha_test`, else solid
/// (after the rule of `cli::live_looks`, but `tex_bump_detail_overlay`,
/// Coagulation's rock, is solid). A see-through template's words say how:
/// `add` or `additive` adds (`one_add_illum_detail`, mainmenu's light
/// shafts, the template converted Halo 1 additive sky shaders use), `mult`
/// multiplies, `alpha` blends by the picture's alpha
/// (`sky_two_alpha_clouds`); water and the rest are [`Blend::Translucent`].
/// The material's own passes, where they say, win over the name
/// ([`material_blend`]).
pub fn template_blend(template: &str) -> Blend {
    let lower = template.to_ascii_lowercase();
    let stem = lower.rsplit('\\').next().unwrap_or(&lower);
    let water = lower.split('\\').any(|c| c == "water");
    if water {
        Blend::Translucent
    } else if lower.split('\\').any(|c| c == "transparent") || stem == "overlay" {
        let words: Vec<&str> = stem.split('_').collect();
        if words.iter().any(|w| *w == "add" || *w == "additive") {
            Blend::Additive
        } else if words.iter().any(|w| w.starts_with("mult")) {
            Blend::Multiply
        } else if words.contains(&"alpha") {
            Blend::Alpha
        } else {
            Blend::Translucent
        }
    } else if stem.contains("alpha_test") {
        Blend::Mask
    } else {
        Blend::Opaque
    }
}

/// Whether a template lives in a `water` folder (its blend stays
/// [`Blend::Translucent`]: the water passes are not reproduced).
pub fn is_water_template(template: &str) -> bool {
    template
        .to_ascii_lowercase()
        .split('\\')
        .any(|c| c == "water")
}

/// Whether a template is the opaque self-illuminated one (`opaque\illum`:
/// mainmenu's lit building windows): its picture is its own light, drawn
/// as it is and not shaded.
pub fn template_emits(template: &str) -> bool {
    let lower = template.to_ascii_lowercase();
    lower.rsplit('\\').next() == Some("illum")
}

/// A blend state (`native_render::pass_state`, the pass's own
/// `ALPHABLENDENABLE`, `SRCBLEND` and `DESTBLEND` as the native renderer
/// reads them) as the viewport's [`Blend`]; `None` for factors it does not
/// draw. The pairs are the Live site's (`cli::live_models::blend_of_pass`).
pub fn blend_of_state(state: &wgpu::BlendState) -> Option<Blend> {
    use wgpu::BlendFactor as F;
    let c = state.color;
    if c.operation != wgpu::BlendOperation::Add {
        return None;
    }
    match (c.src_factor, c.dst_factor) {
        (F::Dst, F::Src) | (F::Src, F::Dst) => Some(Blend::Double),
        (F::Dst, F::Zero) | (F::Zero, F::Src) => Some(Blend::Multiply),
        (F::One | F::SrcAlpha, F::One) => Some(Blend::Additive),
        (F::SrcAlpha, F::OneMinusSrcAlpha) => Some(Blend::Alpha),
        _ => None,
    }
}

/// How a resolved material composites: the first pass of its first level
/// of detail that blends in a way [`blend_of_state`] knows, read through
/// the native renderer's render-state decoder
/// (`native_render::pass_state::pass_state`, each pass's first
/// implementation). `None` when no pass says.
pub fn material_blend(material: &tags::material::Material) -> Option<Blend> {
    material
        .layers
        .iter()
        .filter(|l| l.lod == 0)
        .flat_map(|l| &l.passes)
        .filter_map(|p| p.implementations.first())
        .filter_map(|imp| native_render::pass_state::pass_state(&imp.render_states).ok())
        .find_map(|state| state.blend.as_ref().and_then(blend_of_state))
}

/// Whether a bitmap is an engine one (the `rasterizer` and
/// `default_bitmaps` folders), never a surface's own picture.
pub fn is_engine_bitmap(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower
        .split('\\')
        .any(|c| c == "rasterizer" || c == "default_bitmaps")
}

/// Whether a shader template draws its base map clamped to its edges: a
/// template (or its passes) named `..._clamped`, as the sky's horizon
/// gradients are (`sky_one_alpha_env_clamped`; their pass sets the base
/// map's sampler to clamp). Repeated, a gradient's solid edge wraps over
/// the whole sky.
pub fn template_clamps(template: &str) -> bool {
    let lower = template.to_ascii_lowercase();
    let stem = lower.rsplit('\\').next().unwrap_or(&lower);
    stem.contains("clamped")
}

/// What one implementation of a material binds per texture stage: the
/// bitmap, and the scale of the texture coordinates it is sampled at.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StageBinding {
    pub bitmap: Option<TagId>,
    /// `c[18 + stage]`.xy when the material writes both: the texture
    /// vertex shaders scale stage `n`'s coordinates by that constant (the
    /// template's base map at 1, a detail map at 16 or 40, a bump map at
    /// its own scale, as `h2 shader` shows the passes).
    pub scale: Option<[f32; 2]>,
}

/// Every implementation of a resolved material, in layer and pass order,
/// as [`StageBinding`]s.
pub fn stage_bindings(material: &tags::material::Material) -> Vec<[StageBinding; 4]> {
    use tags::material::TextureSource;
    let mut out = Vec::new();
    for layer in &material.layers {
        for pass in &layer.passes {
            for imp in &pass.implementations {
                let stages: [StageBinding; 4] = std::array::from_fn(|k| {
                    let bitmap = match imp.textures[k].map(|t| t.source) {
                        Some(TextureSource::Bitmap { bitmap, .. }) if bitmap != TagId::NULL => {
                            Some(bitmap)
                        }
                        _ => None,
                    };
                    let scale = imp
                        .vertex_constants
                        .iter()
                        .find(|r| usize::from(r.index) == k && r.written & 3 == 3)
                        .map(|r| [r.value[0], r.value[1]])
                        .filter(|s| s.iter().all(|v| v.is_finite() && *v != 0.0));
                    StageBinding { bitmap, scale }
                });
                out.push(stages);
            }
        }
    }
    out
}

/// How a shader's base map is drawn with its detail maps.
#[derive(Clone, Debug, PartialEq)]
pub struct DetailPlan {
    pub base_scale: [f32; 2],
    /// Detail bitmaps in stage order, each with its scale; at most
    /// [`MAX_DETAILS`].
    pub details: Vec<(TagId, [f32; 2])>,
}

impl Default for DetailPlan {
    fn default() -> Self {
        Self {
            base_scale: [1.0, 1.0],
            details: Vec::new(),
        }
    }
}

/// The first implementation that samples `base`: the base map's scale
/// there, and the other bitmaps it samples that are detail maps
/// (`is_detail`), each at its stage's scale. The templates multiply
/// those into the base map doubled (`t0 * t1 x2`, and again for a second
/// detail map in the `two_detail` ones). No implementation sampling
/// `base`: the base map alone.
pub fn detail_plan(
    implementations: &[[StageBinding; 4]],
    base: TagId,
    is_detail: impl Fn(TagId) -> bool,
) -> DetailPlan {
    let Some((stages, at)) = implementations.iter().find_map(|stages| {
        let at = stages.iter().position(|s| s.bitmap == Some(base))?;
        Some((stages, at))
    }) else {
        return DetailPlan::default();
    };
    let one = [1.0, 1.0];
    DetailPlan {
        base_scale: stages[at].scale.unwrap_or(one),
        details: stages
            .iter()
            .enumerate()
            .filter(|(k, _)| *k != at)
            .filter_map(|(_, s)| Some((s.bitmap?, s.scale.unwrap_or(one))))
            .filter(|(b, _)| *b != base && is_detail(*b))
            .take(MAX_DETAILS)
            .collect(),
    }
}

/// The [`Look`] of a base map (`texture`) drawn by `plan`: its scale, and
/// each detail map that decodes (`texture_of`) in order.
pub fn look_of(
    texture: Option<usize>,
    plan: &DetailPlan,
    mut texture_of: impl FnMut(TagId) -> Option<usize>,
) -> Look {
    let mut look = Look::plain(texture);
    if texture.is_none() {
        return look;
    }
    look.scale = plan.base_scale;
    let layers = plan.details.iter().filter_map(|&(bitmap, scale)| {
        Some(Layer {
            texture: texture_of(bitmap)?,
            scale,
        })
    });
    for (slot, layer) in look.details.iter_mut().zip(layers) {
        *slot = Some(layer);
    }
    look
}

/// Whether a bitmap reads as a detail map by its tag name: in a
/// `detail_maps` folder or named `detail`, and not a bump, normal,
/// reflection or engine default map.
pub fn is_detail_map(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let last = lower.rsplit('\\').next().unwrap_or(&lower);
    let folders: Vec<&str> = lower.split('\\').collect();
    let excluded = ["default_bitmaps", "rasterizer", "reflection_maps"];
    let not = [
        "bump",
        "_bmp",
        "normal",
        "reflection",
        "cube",
        "illum",
        "mask",
    ];
    (folders.contains(&"detail_maps") || last.contains("detail"))
        && !folders.iter().any(|f| excluded.contains(f))
        && !not.iter().any(|w| last.contains(w))
}

/// What a shader's bitmap is for, from the words of its tag name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BitmapRole {
    Diffuse,
    Other,
}

/// One part of art read outside a Halo 2 map (a Halo 1 BSP) for
/// [`reference_scene`]: its geometry and what it draws with.
#[derive(Clone, Debug)]
pub struct ReferencePart {
    pub part: WorldPart,
    /// Index into the scene's textures.
    pub texture: Option<usize>,
    /// Index into the scene's lightmap pages.
    pub lightmap: Option<usize>,
    pub blend: Blend,
}

/// A scene from pictures and parts read outside a Halo 2 map, drawn as a
/// Halo 2 BSP is: base maps fitted, mipmapped and lifted like
/// [`SceneBuilder`]'s (those a [`Blend::Mask`] part uses have their alpha
/// stretched), lightmap pages unlifted. `shaders` and `textured` are for
/// the status bar.
pub fn reference_scene(
    textures: Vec<Image>,
    lightmaps: Vec<Image>,
    parts: &[ReferencePart],
    shaders: usize,
    textured: usize,
) -> Scene {
    let mut scene = Scene::default();
    let cutouts: std::collections::HashSet<usize> = parts
        .iter()
        .filter(|p| p.blend == Blend::Mask)
        .filter_map(|p| p.texture)
        .collect();
    for (i, mut image) in textures.into_iter().enumerate() {
        if cutouts.contains(&i) {
            image.stretch_alpha();
        }
        let image = image.fit(MAX_TEXTURE);
        scene.lifts.push(image.lift_factor());
        scene.textures.push(image);
    }
    let pages = scene.textures.len();
    scene.stats.lightmap_pages = lightmaps.len();
    for page in lightmaps {
        scene.lifts.push(1.0);
        scene.textures.push(page);
    }
    for p in parts {
        let lightmap = p.lightmap.map(|l| pages + l);
        scene.world.push(p.texture, lightmap, p.blend, &p.part);
    }
    scene.stats.shaders = shaders;
    scene.stats.textured = textured;
    scene
}

/// The rule of `cli::live_models::bitmap_role`, reduced to "is this the
/// diffuse map": not an engine, change-colour, self-illumination, bump,
/// detail, reflection or mask map.
pub fn bitmap_role(name: &str) -> BitmapRole {
    let lower = name.to_ascii_lowercase();
    let last = lower.rsplit('\\').next().unwrap_or(&lower);
    let folders = [
        "default_bitmaps",
        "rasterizer",
        "detail_maps",
        "reflection_maps",
        "cube",
    ];
    let words = [
        "_cc",
        "change_color",
        "change_colour",
        "color_change",
        "colour_change",
        "bump",
        "_bmp",
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
    if lower.split('\\').any(|c| folders.contains(&c))
        || last.ends_with("_cc")
        || words.iter().any(|w| last.contains(w))
    {
        BitmapRole::Other
    } else {
        BitmapRole::Diffuse
    }
}

/// An RGBA8 picture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    /// Half the size (each side at least 1), box filtered.
    pub fn half(&self) -> Image {
        let (w, h) = ((self.width / 2).max(1), (self.height / 2).max(1));
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        let at = |x: u32, y: u32, c: u32| -> u32 {
            let x = x.min(self.width - 1);
            let y = y.min(self.height - 1);
            u32::from(self.rgba[((y * self.width + x) * 4 + c) as usize])
        };
        for y in 0..h {
            for x in 0..w {
                for c in 0..4 {
                    let (sx, sy) = (x * 2, y * 2);
                    let sum = at(sx, sy, c)
                        + at(sx + 1, sy, c)
                        + at(sx, sy + 1, c)
                        + at(sx + 1, sy + 1, c);
                    rgba[((y * w + x) * 4 + c) as usize] = ((sum + 2) / 4) as u8;
                }
            }
        }
        Image {
            width: w,
            height: h,
            rgba,
        }
    }

    /// Halved until neither side exceeds `max`.
    pub fn fit(mut self, max: u32) -> Image {
        while self.width > max || self.height > max {
            self = self.half();
        }
        self
    }

    /// This image and its mips down to 1x1.
    pub fn mip_chain(self) -> Vec<Image> {
        let mut out = vec![self];
        while let Some(last) = out.last()
            && (last.width > 1 || last.height > 1)
        {
            let next = last.half();
            out.push(next);
        }
        out
    }

    /// How much to brighten it (at most [`MAX_LIFT`]) for its mean to be at
    /// least [`LIFT_TARGET`] in linear light, when it is not lightmapped. The game draws base maps under
    /// detail maps that double them and under bright lightmaps; drawn
    /// alone by one sun, dark metals (Ascension's Forerunner panels, a
    /// mean near 0.06) read as black.
    pub fn lift_factor(&self) -> f32 {
        let linear: [f32; 256] = std::array::from_fn(|c| (c as f32 / 255.0).powf(2.2));
        let n = (self.rgba.len() / 4).max(1) as f32;
        let mean = self
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| (linear[p[0] as usize] + linear[p[1] as usize] + linear[p[2] as usize]) / 3.0)
            .sum::<f32>()
            / n;
        if mean <= 0.0 || mean >= LIFT_TARGET {
            return 1.0;
        }
        (LIFT_TARGET / mean).min(MAX_LIFT)
    }

    /// Alpha stretched so the strongest is opaque, then doubled: a cut at
    /// a half keeps what is at least a quarter of the strongest (leaves
    /// keep little alpha at a half; `cli::live_looks::cut_at_a_quarter`).
    pub fn stretch_alpha(&mut self) {
        let max = self
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| p[3])
            .max()
            .unwrap_or(0);
        if max == 0 {
            return;
        }
        for px in self.rgba.as_chunks_mut::<4>().0 {
            px[3] = (u32::from(px[3]) * 510 / u32::from(max)).min(255) as u8;
        }
    }
}

/// The most detail maps a [`Look`] multiplies in (the `two_detail`
/// templates use two).
pub const MAX_DETAILS: usize = 2;

/// A detail map of a [`Look`]: a texture and the scale of the part's
/// texture coordinates it is sampled at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layer {
    /// Index into [`Scene::textures`].
    pub texture: usize,
    pub scale: [f32; 2],
}

/// How a shader draws, as the viewport reproduces it: its base map at a
/// scale of the texture coordinates, times each detail map at its own
/// scale, doubled (the game's `t0 * t1 x2` combiners, in gamma space).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    /// Index into [`Scene::textures`]; `None` draws untextured grey.
    pub texture: Option<usize>,
    pub scale: [f32; 2],
    pub details: [Option<Layer>; MAX_DETAILS],
    /// Self-illuminated ([`template_emits`]): drawn as it is, unshaded.
    pub emissive: bool,
}

impl Look {
    /// A base map alone, unscaled (or no texture).
    pub const fn plain(texture: Option<usize>) -> Self {
        Self {
            texture,
            scale: [1.0, 1.0],
            details: [None; MAX_DETAILS],
            emissive: false,
        }
    }
}

/// Triangles sharing one look and blend.
#[derive(Clone, Debug, PartialEq)]
pub struct Batch {
    /// Index into [`Scene::textures`]; `None` draws untextured grey.
    pub texture: Option<usize>,
    /// The scale the base map is sampled at ([`Look::scale`]).
    pub scale: [f32; 2],
    /// Detail maps multiplied in ([`Look::details`]).
    pub details: [Option<Layer>; MAX_DETAILS],
    /// [`Look::emissive`].
    pub emissive: bool,
    /// The lightmap page lighting it, an index into [`Scene::textures`];
    /// `None` is lit by the sun.
    pub lightmap: Option<usize>,
    pub blend: Blend,
    pub vertices: Vec<SceneVertex>,
    pub indices: Vec<u32>,
}

impl Batch {
    /// The look it draws with.
    pub fn look(&self) -> Look {
        Look {
            texture: self.texture,
            scale: self.scale,
            details: self.details,
            emissive: self.emissive,
        }
    }
}

/// Batches in one space (the world, or one model's).
#[derive(Clone, Debug, PartialEq)]
pub struct Mesh {
    pub batches: Vec<Batch>,
    pub bounds: Bounds,
}

impl Default for Mesh {
    fn default() -> Self {
        Self {
            batches: Vec::new(),
            bounds: Bounds::EMPTY,
        }
    }
}

impl Mesh {
    /// Add one part's triangles under `(texture, blend)`, merged into the
    /// batch with that key. Triangles with out-of-range indices are dropped.
    pub fn push(
        &mut self,
        texture: Option<usize>,
        lightmap: Option<usize>,
        blend: Blend,
        part: &WorldPart,
    ) {
        self.push_look(&Look::plain(texture), lightmap, blend, part);
    }

    /// [`Self::push`] with a whole [`Look`] (base map, its scale and
    /// detail maps), merged into the batch with the same look.
    pub fn push_look(
        &mut self,
        look: &Look,
        lightmap: Option<usize>,
        blend: Blend,
        part: &WorldPart,
    ) {
        // Lightmap coordinates are only meaningful with the part's own page.
        let lightmap = lightmap.filter(|_| part.lightmap_uvs.len() == part.positions.len());
        let i = match self
            .batches
            .iter()
            .position(|b| b.look() == *look && b.lightmap == lightmap && b.blend == blend)
        {
            Some(i) => i,
            None => {
                self.batches.push(Batch {
                    texture: look.texture,
                    scale: look.scale,
                    details: look.details,
                    emissive: look.emissive,
                    lightmap,
                    blend,
                    vertices: Vec::new(),
                    indices: Vec::new(),
                });
                self.batches.len() - 1
            }
        };
        let b = &mut self.batches[i];
        let base = b.vertices.len() as u32;
        for (k, &p) in part.positions.iter().enumerate() {
            if !blend.see_through() {
                self.bounds.add(p);
            }
            b.vertices.push(SceneVertex {
                position: p,
                normal: part.normals.get(k).copied().unwrap_or([0.0, 0.0, 1.0]),
                uv: part.uvs.get(k).copied().unwrap_or([0.0; 2]),
                lightmap_uv: part.lightmap_uvs.get(k).copied().unwrap_or([0.0; 2]),
            });
        }
        let n = part.positions.len() as u32;
        for t in &part.tris {
            if t.iter().all(|&i| i < n) {
                b.indices.extend(t.map(|i| base + i));
            }
        }
    }

    /// The batches in the order they are drawn ([`Blend::draw_order`]):
    /// solid, cut-out, then see-through, each group in the order its
    /// first part was pushed.
    pub fn draw_sequence(&self) -> Vec<&Batch> {
        let mut order: Vec<&Batch> = self.batches.iter().collect();
        order.sort_by_key(|b| b.blend.draw_order());
        order
    }

    pub fn triangle_count(&self) -> usize {
        self.batches.iter().map(|b| b.indices.len() / 3).sum()
    }

    /// Distance along `ray` to the nearest triangle that is not see-through.
    pub fn hit(&self, ray: &Ray) -> Option<f32> {
        let mut best: Option<f32> = None;
        for b in self
            .batches
            .iter()
            .filter(|b| !b.blend.see_through())
        {
            for t in b.indices.as_chunks::<3>().0 {
                let p = [0, 1, 2].map(|k| b.vertices[t[k] as usize].position);
                if let Some(d) = ray.triangle(&p)
                    && best.is_none_or(|b| d < b)
                {
                    best = Some(d);
                }
            }
        }
        best
    }
}

/// Counts for the status bar and the log.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SceneStats {
    pub shaders: usize,
    /// Shaders drawn with a base map.
    pub textured: usize,
    pub bitmaps_failed: usize,
    pub objects: usize,
    /// Objects drawn with a model.
    pub modelled: usize,
    pub models_failed: usize,
    /// Lightmap pages decoded.
    pub lightmap_pages: usize,
}

/// A map's drawable art.
#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub textures: Vec<Image>,
    /// Per texture, how much the sun-lit shading brightens it
    /// ([`Image::lift_factor`]); 1 for lightmap pages. Lightmapped
    /// surfaces are not lifted.
    pub lifts: Vec<f32>,
    pub world: Mesh,
    pub models: Vec<Mesh>,
    /// Placed object tag → index into [`Self::models`].
    pub objects: HashMap<TagId, usize>,
    /// The level's sky, drawn behind everything at infinity.
    pub sky: Option<SceneSky>,
    /// Textures drawn clamped to their edges, not repeated
    /// ([`template_clamps`]).
    pub clamped: std::collections::HashSet<usize>,
    pub stats: SceneStats,
    /// Every shader the scene drew, in the order first met, with how it
    /// draws (`view.stats`).
    pub materials: Vec<MaterialInfo>,
    /// Each scenario BSP and how it is lit.
    pub lighting: Vec<BspLightingInfo>,
    /// The map's own sun and fog, from its sky.
    pub atmosphere: Atmosphere,
    /// The lightmapped floor under placed objects ([`Scene::light_at`]).
    pub floor: LightFloor,
}

/// One shader as the scene draws it.
#[derive(Clone, Debug, PartialEq)]
pub struct MaterialInfo {
    /// The shader's tag name.
    pub shader: String,
    /// Its template's tag name (empty when it does not read).
    pub template: String,
    pub blend: Blend,
    /// Whether [`Self::blend`] came from the material's passes
    /// ([`material_blend`]) rather than the template's name.
    pub blend_from_passes: bool,
    pub emissive: bool,
    /// The bitmap drawn as its picture, when one decoded.
    pub base: Option<String>,
    /// Detail maps multiplied in.
    pub details: usize,
}

/// One scenario BSP's lighting, for `view.stats` and the log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BspLightingInfo {
    /// The BSP's tag name.
    pub bsp: String,
    pub lighting: geometry::BspLighting,
}

/// The sky's light, for surfaces no lightmap lights.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sun {
    /// Unit vector toward the light, above the horizon.
    pub direction: Vec3,
    /// The light's colour as the tag stores it.
    pub color: [f32; 3],
    pub power: f32,
}

/// The sky's fog (its `fog_a` block's first element, the one
/// `0x00131030` blends into the view; docs/halo1/sky.md, "Halo 2's sky
/// fog").
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fog {
    /// Clamped to 0..1, as the tag stores it (gamma space).
    pub color: [f32; 3],
    /// Up to 1: the most the fog covers.
    pub density: f32,
    /// Distance where the fog starts.
    pub start: f32,
    /// Distance where it reaches its density; past `start`.
    pub opaque: f32,
}

/// What the map's sky says about light and fog.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Atmosphere {
    /// The `sky ` tag read (its file name).
    pub sky: Option<String>,
    pub sun: Option<Sun>,
    pub fog: Option<Fog>,
    /// The sky's fallback object-lighting colour (`sky +0x34`).
    pub fallback_rgb: Option<[f32; 3]>,
}

/// The direction toward a sky light from its yaw and pitch (radians, the
/// light record's `+0x0c`, `+0x10`): z up, yaw about z from +x. A light
/// whose vector points below the horizon is turned above it: the
/// viewport's sun lights from above.
pub fn sun_direction(yaw: f32, pitch: f32) -> Vec3 {
    let d = [pitch.cos() * yaw.cos(), pitch.cos() * yaw.sin(), pitch.sin()];
    let d = if d[2] < 0.0 { scale(d, -1.0) } else { d };
    normalize(d)
}

/// The sun's colour scaled so its brightest channel is 1: the viewport
/// keeps its own brightness for unlit surfaces and takes the hue. Black
/// or not finite: white.
pub fn sun_tint(color: [f32; 3]) -> [f32; 3] {
    let max = color.iter().copied().fold(0.0f32, f32::max);
    if !(max.is_finite() && max > 1e-4) || color.iter().any(|c| !c.is_finite()) {
        return [1.0; 3];
    }
    color.map(|c| (c / max).max(0.0))
}

/// The engine's check of a blended fog (`0x001301c0`): a density above
/// zero keeps the fog, its colour clamped to 0..1, its opaque distance
/// raised to at least one unit past its start and its density to at most
/// 1. A density of zero (or not finite values) is no fog.
pub fn fog_of(color: [f32; 3], density: f32, start: f32, opaque: f32) -> Option<Fog> {
    if !(density.is_finite() && start.is_finite() && opaque.is_finite()) || density <= 0.0 {
        return None;
    }
    if color.iter().any(|c| !c.is_finite()) {
        return None;
    }
    let start = start.max(0.0);
    Some(Fog {
        color: color.map(|c| c.clamp(0.0, 1.0)),
        density: density.min(1.0),
        start,
        opaque: opaque.max(start + 1.0),
    })
}

/// How much of the fog covers a point `distance` from the eye: none
/// before its start, rising evenly to its density at its opaque distance.
pub fn fog_amount(fog: &Fog, distance: f32) -> f32 {
    let t = ((distance - fog.start) / (fog.opaque - fog.start)).clamp(0.0, 1.0);
    fog.density * t
}

/// A lightmapped floor triangle: its corners, their lightmap coordinates
/// and the page (an index into [`Scene::textures`]).
#[derive(Clone, Debug, PartialEq)]
struct FloorTri {
    p: [Vec3; 3],
    uv: [[f32; 2]; 3],
    page: usize,
}

/// The lightmapped solid world's triangles by column (a grid over x and
/// y), for finding the floor under a point.
#[derive(Clone, Debug, Default)]
pub struct LightFloor {
    tris: Vec<FloorTri>,
    cells: HashMap<(i32, i32), Vec<u32>>,
    /// Triangles too wide to list per cell, tested by every query.
    wide: Vec<u32>,
}

/// The side of a [`LightFloor`] cell, in world units.
pub const FLOOR_CELL: f32 = 8.0;
/// The most cells a triangle spans per axis before it is tested by every
/// query instead.
const FLOOR_SPAN: i32 = 16;
/// How far above a point a floor may be and still be under it (an
/// object's origin can sit a little into the ground).
pub const FLOOR_ABOVE: f32 = 0.5;
/// How far below a point the floor under it is looked for.
pub const FLOOR_BELOW: f32 = 64.0;

impl LightFloor {
    /// The lightmapped solid batches of `mesh`.
    pub fn of(mesh: &Mesh) -> Self {
        let mut floor = LightFloor::default();
        for b in mesh.batches.iter().filter(|b| !b.blend.see_through()) {
            let Some(page) = b.lightmap else {
                continue;
            };
            for t in b.indices.as_chunks::<3>().0 {
                let Some(v) = t.iter().map(|&i| b.vertices.get(i as usize)).collect::<Option<Vec<_>>>() else {
                    continue;
                };
                floor.push(FloorTri {
                    p: [v[0].position, v[1].position, v[2].position],
                    uv: [v[0].lightmap_uv, v[1].lightmap_uv, v[2].lightmap_uv],
                    page,
                });
            }
        }
        floor
    }

    fn cell(x: f32) -> i32 {
        (x / FLOOR_CELL).floor() as i32
    }

    fn push(&mut self, t: FloorTri) {
        let i = self.tris.len() as u32;
        let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
        for p in &t.p {
            for k in 0..2 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        if !(lo.iter().chain(&hi).all(|v| v.is_finite())) {
            return;
        }
        self.tris.push(t);
        let (x0, x1, y0, y1) = (Self::cell(lo[0]), Self::cell(hi[0]), Self::cell(lo[1]), Self::cell(hi[1]));
        if x1 - x0 >= FLOOR_SPAN || y1 - y0 >= FLOOR_SPAN {
            self.wide.push(i);
            return;
        }
        for x in x0..=x1 {
            for y in y0..=y1 {
                self.cells.entry((x, y)).or_default().push(i);
            }
        }
    }

    /// The highest floor point under `p` (at most [`FLOOR_ABOVE`] above
    /// it, [`FLOOR_BELOW`] below): its page and lightmap coordinates.
    pub fn under(&self, p: Vec3) -> Option<(usize, [f32; 2])> {
        let cell = (Self::cell(p[0]), Self::cell(p[1]));
        let listed = self.cells.get(&cell).map_or(&[][..], Vec::as_slice);
        let mut best: Option<(f32, usize, [f32; 2])> = None;
        for &i in listed.iter().chain(&self.wide) {
            let t = &self.tris[i as usize];
            let Some(w) = barycentric_xy(&t.p, [p[0], p[1]]) else {
                continue;
            };
            let z = w[0] * t.p[0][2] + w[1] * t.p[1][2] + w[2] * t.p[2][2];
            if z > p[2] + FLOOR_ABOVE || z < p[2] - FLOOR_BELOW {
                continue;
            }
            if best.is_none_or(|b| z > b.0) {
                let uv = [0, 1].map(|k| w[0] * t.uv[0][k] + w[1] * t.uv[1][k] + w[2] * t.uv[2][k]);
                best = Some((z, t.page, uv));
            }
        }
        best.map(|(_, page, uv)| (page, uv))
    }
}

/// The barycentric weights of `q` in a triangle seen from above (x, y),
/// when it is inside; `None` outside or for a triangle seen edge-on
/// (a wall).
pub fn barycentric_xy(t: &[Vec3; 3], q: [f32; 2]) -> Option<[f32; 3]> {
    let (a, b, c) = (t[0], t[1], t[2]);
    let det = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1]);
    if det.abs() < 1e-6 {
        return None;
    }
    let w0 = ((b[1] - c[1]) * (q[0] - c[0]) + (c[0] - b[0]) * (q[1] - c[1])) / det;
    let w1 = ((c[1] - a[1]) * (q[0] - c[0]) + (a[0] - c[0]) * (q[1] - c[1])) / det;
    let w2 = 1.0 - w0 - w1;
    const EDGE: f32 = -1e-4;
    (w0 >= EDGE && w1 >= EDGE && w2 >= EDGE).then_some([w0, w1, w2])
}

/// An sRGB-encoded channel (0..255) in linear light.
pub fn srgb_to_linear(c: u8) -> f32 {
    let c = f32::from(c) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

impl Image {
    /// The colour at texture coordinates `uv` (clamped to the edges),
    /// bilinearly filtered, in linear light: what sampling the uploaded
    /// sRGB texture gives.
    pub fn sample_linear(&self, uv: [f32; 2]) -> [f32; 3] {
        if self.width == 0 || self.height == 0 || self.rgba.len() < (self.width * self.height * 4) as usize {
            return [0.0; 3];
        }
        let (w, h) = (self.width as f32, self.height as f32);
        let x = (uv[0].clamp(0.0, 1.0) * w - 0.5).clamp(0.0, w - 1.0);
        let y = (uv[1].clamp(0.0, 1.0) * h - 0.5).clamp(0.0, h - 1.0);
        let (x0, y0) = (x.floor() as u32, y.floor() as u32);
        let (x1, y1) = ((x0 + 1).min(self.width - 1), (y0 + 1).min(self.height - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let texel = |x: u32, y: u32| -> [f32; 3] {
            let i = ((y * self.width + x) * 4) as usize;
            [0, 1, 2].map(|c| srgb_to_linear(self.rgba[i + c]))
        };
        let (a, b, c, d) = (texel(x0, y0), texel(x1, y0), texel(x0, y1), texel(x1, y1));
        [0, 1, 2].map(|k| {
            let top = a[k] + (b[k] - a[k]) * fx;
            let bottom = c[k] + (d[k] - c[k]) * fx;
            top + (bottom - top) * fy
        })
    }
}

/// The light a lightmap texel (linear) gives, as the scene shader applies
/// it to a lightmapped surface: doubled in gamma space
/// ([`crate::render::LIGHTMAP_SCALE`]) plus the faint fill
/// ([`crate::render::LIGHTMAP_FILL`]).
pub fn lightmap_light(texel: [f32; 3]) -> [f32; 3] {
    texel.map(|c| c * crate::render::LIGHTMAP_SCALE + crate::render::LIGHTMAP_FILL)
}

/// The scenario's sky as the viewport draws it: its render model in
/// model space, centred on the eye, turning with the camera only, over
/// its clear colour.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneSky {
    /// The `sky ` tag's file name.
    pub name: String,
    pub mesh: Mesh,
    /// Linear RGB behind the model ([`sky_clear`]).
    pub clear: [f32; 3],
}

/// `sky +0x10`: flags; bit 5 says the clear colour at [`SKY_CLEAR`]
/// replaces the view's (docs/formats/structure-bsp.md, cluster `+6e`).
pub const SKY_FLAGS: usize = 0x10;
/// `sky +0xa0`: the clear colour, RGB 0..1.
pub const SKY_CLEAR: usize = 0xa0;
/// The [`SKY_FLAGS`] bit that turns [`SKY_CLEAR`] on.
pub const SKY_CLEAR_FLAG: u32 = 1 << 5;

/// What the viewport clears to behind a sky model, from the `sky ` tag's
/// root bytes: its clear colour when its flag is set and the colour reads
/// (finite, clamped to 0..1; the game's gamma-space value, returned
/// linear for the sRGB target), else black. Black, not the editor's
/// gradient: a sky's additive layers (Halo 1's space dome and galaxy) add
/// onto what is behind them, and a pale horizon added under a starfield
/// is not the game's picture.
pub fn sky_clear(root: &[u8]) -> [f32; 3] {
    let word = |at: usize| {
        root.get(at..at + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let on = word(SKY_FLAGS).is_some_and(|f| f & SKY_CLEAR_FLAG != 0);
    let rgb = [0, 1, 2].map(|k| word(SKY_CLEAR + 4 * k).map(f32::from_bits));
    match rgb {
        [Some(r), Some(g), Some(b)] if on && [r, g, b].iter().all(|c| c.is_finite()) => {
            [r, g, b].map(|c| c.clamp(0.0, 1.0).powf(2.2))
        }
        _ => [0.0; 3],
    }
}

impl Scene {
    /// The light on the floor under `p` (the lightmap texel there, as a
    /// lightmapped surface is lit: [`lightmap_light`]), when a lightmapped
    /// solid surface is under it. What a placed object is lit by: the game
    /// lights objects from the lightmap they stand on.
    pub fn light_at(&self, p: Vec3) -> Option<[f32; 3]> {
        let (page, uv) = self.floor.under(p)?;
        let image = self.textures.get(page)?;
        Some(lightmap_light(image.sample_linear(uv)))
    }

    /// For the status line: how the BSPs without a lightmap are lit
    /// (`""` when all are lightmapped), e.g. ` (mainmenu: empty lightmap
    /// group, lit by its sky's sun)`.
    pub fn unlit_note(&self) -> String {
        let unlit: Vec<String> = self
            .lighting
            .iter()
            .filter(|l| l.lighting != geometry::BspLighting::Lightmapped)
            .map(|l| {
                let name = l.bsp.rsplit(['\\', '/']).next().unwrap_or(&l.bsp);
                format!("{name}: {}", l.lighting.name())
            })
            .collect();
        if unlit.is_empty() {
            return String::new();
        }
        let by = if self.atmosphere.sun.is_some() {
            "its sky's sun"
        } else {
            "the editor's sun"
        };
        format!(" ({}, lit by {by})", unlit.join(", "))
    }

    /// The model a placement draws, if any.
    pub fn model_of(&self, m: &Marker) -> Option<usize> {
        m.object.and_then(|o| self.objects.get(&o).copied())
    }

    /// The nearest placement whose model (or glyph) the ray hits.
    pub fn pick(&self, markers: &[Marker], ray: &Ray) -> Option<(BlockKind, usize)> {
        let mut best = geometry::pick_glyph(markers, ray);
        for m in markers {
            let Some(model) = self.model_of(m).and_then(|i| self.models.get(i)) else {
                continue;
            };
            if model.bounds.is_empty() {
                continue;
            }
            // Broad phase on the model's bounding sphere, placed.
            let c = model_to_world(m, model.bounds.center());
            if ray.sphere(c, model.bounds.radius() * m.scale).is_none() {
                continue;
            }
            // The ray in model space: axes are orthonormal, scaled by `scale`.
            let local = sub(ray.origin, m.position);
            let s = m.scale.max(1e-6);
            let to_local = |v: Vec3| m.axes.map(|a| dot(v, a));
            let lray = Ray {
                origin: scale(to_local(local), 1.0 / s),
                dir: to_local(ray.dir),
            };
            if let Some(t) = model.hit(&lray) {
                let t = t * s;
                if best.is_none_or(|b| t < b.0) {
                    best = Some((t, m.block, m.index));
                }
            }
        }
        best.map(|(_, b, i)| (b, i))
    }
}

/// A model-space point placed by a marker's transform.
pub fn model_to_world(m: &Marker, p: Vec3) -> Vec3 {
    let [f, l, u] = m.axes;
    let s = m.scale;
    add(
        m.position,
        add(
            scale(f, p[0] * s),
            add(scale(l, p[1] * s), scale(u, p[2] * s)),
        ),
    )
}

/// The files a map's raw pointers select, for `assets::decode_image`: the
/// map itself and the shared caches beside it. A map held in memory (a
/// package resolved over its base, or a converted map) is read from those
/// bytes: its local raw offsets are its own, and the base map on disk (for a
/// converted Halo 1 map, Coagulation or Beaver Creek) has other data there.
pub fn resource_files(source: &MapSource) -> ResourceFiles {
    let mut files = ResourceFiles::new();
    let region = |s: &MapSource| -> Option<Region> {
        let (base, length) = match s.region {
            Some(r) => r,
            None => (0, std::fs::metadata(&s.path).ok()?.len()),
        };
        Some(Region {
            path: s.path.clone(),
            base,
            length,
        })
    };
    let mut set = |file, s: &MapSource| {
        if let Some(r) = region(s) {
            let _ = files.set(file, r);
        }
    };
    if source.bytes.is_none() {
        set(ResourceFile::Local, &source.base());
    }
    for (file, s) in geometry::shared_sources(source) {
        set(file, &s);
    }
    if let Some(b) = &source.bytes {
        files.set_bytes(ResourceFile::Local, b.clone());
    }
    files
}

const MODE: FourCC = FourCC(*b"mode");

/// The scenario's skies block: `(count, address)` at `scnr+0x8`, 8-byte
/// tag references with the ID at +4 (docs/live/live-art.md, "Sky").
pub const SCENARIO_SKIES: usize = 0x8;

/// The skies a scenario payload (`bytes` at `address`) names, in order.
pub fn skies_in(bytes: &[u8], address: u32) -> Vec<TagId> {
    let view = TagView::new(bytes, address);
    let Ok(root) = view.at(0, SCENARIO_SKIES + 8) else {
        return Vec::new();
    };
    view.block(root, SCENARIO_SKIES, 8)
        .map(|refs| {
            refs.into_iter()
                .map(|b| TagId(u32::from_le_bytes([b[4], b[5], b[6], b[7]])))
                .collect()
        })
        .unwrap_or_default()
}

/// The open map's scenario's skies ([`skies_in`]).
pub fn scenario_skies<R: Read + Seek>(map: &mut Map<R>) -> Vec<TagId> {
    let Some((id, address)) = map
        .scenario()
        .and_then(|s| Some((s.id, s.location.as_ref()?.address)))
    else {
        return Vec::new();
    };
    map.read_tag_bytes(id)
        .map(|bytes| skies_in(&bytes, address))
        .unwrap_or_default()
}

/// Builds a [`Scene`]: shaders read once, bitmaps decoded once.
pub struct SceneBuilder<'a, R> {
    map: &'a mut Map<R>,
    raw: &'a mut dyn RawSource,
    files: ResourceFiles,
    scene: Scene,
    /// Shader → (look, blend).
    looks: HashMap<TagId, (Look, Blend)>,
    /// (bitmap, cut out, clamped) → texture.
    bitmaps: HashMap<(TagId, bool, bool), Option<usize>>,
    /// The map's shader library, read the first time a shader's detail
    /// maps are asked for (`None` inside when it does not read).
    library: Option<Option<tags::material::ShaderLibrary>>,
}

impl<'a, R: Read + Seek> SceneBuilder<'a, R> {
    pub fn new(map: &'a mut Map<R>, raw: &'a mut dyn RawSource, files: ResourceFiles) -> Self {
        Self {
            map,
            raw,
            files,
            scene: Scene::default(),
            looks: HashMap::new(),
            bitmaps: HashMap::new(),
            library: None,
        }
    }

    /// The textured world from [`geometry::world_parts_lit`], lit by its
    /// lightmap pages where it has them.
    pub fn world(&mut self, parts: &[WorldPart], lightmaps: &[geometry::LightmapPages]) {
        let mut pages: HashMap<geometry::LightmapRef, Option<usize>> = HashMap::new();
        let mut bitmaps: HashMap<TagId, Option<Box<tags::Bitmap>>> = HashMap::new();
        for part in parts {
            let (look, mut blend) = self.look(part.shader);
            if part.transparent && !blend.see_through() {
                blend = Blend::Translucent;
            }
            let lightmap = part.lightmap.and_then(|r| {
                *pages.entry(r).or_insert_with(|| {
                    let src = lightmaps.get(r.pages)?;
                    let bitmap = bitmaps
                        .entry(src.bitmap)
                        .or_insert_with(|| {
                            match tags::DecodedTag::read(self.map, src.bitmap).ok()?.data {
                                tags::TagData::Bitmap(b) => Some(b),
                                _ => None,
                            }
                        })
                        .as_ref()?;
                    let image = bitmap.images.get(usize::from(r.page))?;
                    let palette = src.palettes.get(usize::from(r.palette))?;
                    let page = lightmap_page(image, palette, &mut self.files)?;
                    self.scene.stats.lightmap_pages += 1;
                    self.scene.lifts.push(1.0);
                    self.scene.textures.push(page);
                    Some(self.scene.textures.len() - 1)
                })
            });
            self.scene.world.push_look(&look, lightmap, blend, part);
        }
    }

    /// A model for every placed object tag that resolves to one, with the
    /// objects its model variant attaches ([`object_model_of`]).
    pub fn objects(&mut self, objects: impl IntoIterator<Item = TagId>) {
        let ctx = match tags::object::Context::read(self.map) {
            Ok(c) => c,
            Err(_) => return,
        };
        let multiplayer = self.map.header().cache_type == map::CacheType::Multiplayer;
        let mut by_model: HashMap<ObjectModel, Option<usize>> = HashMap::new();
        let mut seen = std::collections::HashSet::new();
        for object in objects {
            if !seen.insert(object) {
                continue;
            }
            self.scene.stats.objects += 1;
            let Some(wanted) = object_model_of(&ctx, object, multiplayer) else {
                continue;
            };
            let model = match by_model.get(&wanted) {
                Some(m) => *m,
                None => {
                    let m = self.object_mesh(&wanted).map(|mesh| {
                        self.scene.models.push(mesh);
                        self.scene.models.len() - 1
                    });
                    if m.is_none() {
                        self.scene.stats.models_failed += 1;
                    }
                    by_model.insert(wanted, m);
                    m
                }
            };
            if let Some(m) = model {
                self.scene.objects.insert(object, m);
                self.scene.stats.modelled += 1;
            }
        }
    }

    pub fn finish(mut self) -> Scene {
        self.scene.floor = LightFloor::of(&self.scene.world);
        self.scene
    }

    /// How each scenario BSP is lit ([`geometry::world_parts_lit`]), named.
    pub fn lighting(&mut self, bsps: &[geometry::BspLit]) {
        for b in bsps {
            let bsp = self
                .map
                .tag(b.bsp)
                .ok()
                .and_then(|t| t.name.clone())
                .unwrap_or_else(|| format!("{}", b.bsp));
            self.scene.lighting.push(BspLightingInfo {
                bsp,
                lighting: b.lighting.clone(),
            });
        }
    }

    /// The sun and fog of the first of the scenario's skies that reads
    /// (`h1_assets::sky::h2`, the Halo 2 `sky ` reader): its first light
    /// with parameters is the sun of surfaces no lightmap lights, its
    /// first `fog_a` element ([`fog_of`]) the fog over the level.
    pub fn atmosphere(&mut self) {
        for id in scenario_skies(self.map) {
            if id == TagId::NULL {
                continue;
            }
            let Ok(sky) = h1_assets::sky::h2::Sky::read_from_map(self.map, id) else {
                continue;
            };
            let name = self
                .map
                .tag(id)
                .ok()
                .and_then(|t| t.name.clone())
                .unwrap_or_default();
            let sun = sky.lights.iter().find_map(|l| {
                let p = l.params.as_ref()?;
                Some(Sun {
                    direction: sun_direction(l.yaw, l.pitch),
                    color: p.color,
                    power: p.power,
                })
            });
            let fog = sky
                .gradients_a
                .first()
                .and_then(|g| fog_of(g.color, g.fraction, g.tail[0], g.tail[1]));
            self.scene.atmosphere = Atmosphere {
                sky: Some(name.rsplit(['\\', '/']).next().unwrap_or(&name).to_owned()),
                sun,
                fog,
                fallback_rgb: Some(sky.fallback_rgb),
            };
            return;
        }
    }

    /// The first of the scenario's skies ([`scenario_skies`]) whose
    /// `sky ` tag names a render model that builds, as [`Scene::sky`].
    /// The model is read like an object's ([`Self::objects`]): each
    /// region's first permutation at its best detail, textured by its
    /// shaders' base maps.
    pub fn sky(&mut self) {
        for id in scenario_skies(self.map) {
            if id == TagId::NULL {
                continue;
            }
            let Ok(tags::lighting::Atmosphere::Sky(sky)) =
                tags::lighting::Atmosphere::read(self.map, id)
            else {
                continue;
            };
            let mode = sky.model.id;
            if mode == TagId::NULL || self.map.tag(mode).map(|t| t.group).ok() != Some(MODE) {
                continue;
            }
            let Some(mesh) = self.model_mesh(mode) else {
                continue;
            };
            let name = self
                .map
                .tag(id)
                .ok()
                .and_then(|t| t.name.clone())
                .unwrap_or_default();
            let name = name.rsplit(['\\', '/']).next().unwrap_or(&name).to_owned();
            let clear = sky_clear(&sky.opaque);
            self.scene.sky = Some(SceneSky { name, mesh, clear });
            return;
        }
    }

    fn model_mesh(&mut self, mode: TagId) -> Option<Mesh> {
        self.object_mesh(&ObjectModel {
            mode,
            attached: Vec::new(),
        })
    }

    /// A model and what it carries, in the model's space: each attached
    /// object's model with its marker (or origin) on the parent's marker,
    /// at rest. An attachment whose marker or model is missing is left
    /// off; the whole is refused past [`MAX_MODEL_TRIANGLES`].
    fn object_mesh(&mut self, model: &ObjectModel) -> Option<Mesh> {
        let mut mesh = Mesh::default();
        let mut tris = 0;
        self.push_model(model, &IDENTITY, "", &mut mesh, &mut tris)?;
        (tris <= MAX_MODEL_TRIANGLES && !mesh.batches.is_empty()).then_some(mesh)
    }

    /// Add `model`'s parts to `mesh`, placed by `at` after moving its
    /// `anchor` marker (none: its origin) to the origin. `None` when the
    /// model does not read; `tris` counts every triangle added.
    fn push_model(
        &mut self,
        model: &ObjectModel,
        at: &Mat4,
        anchor: &str,
        mesh: &mut Mesh,
        tris: &mut usize,
    ) -> Option<()> {
        let geo = RenderModelGeometry::read(self.map, model.mode, self.raw).ok()?;
        let frames = if anchor.is_empty() && model.attached.is_empty() {
            HashMap::new()
        } else {
            marker_frames(self.map, model.mode, &geo)
        };
        let at = attachment_transform(
            at,
            &IDENTITY,
            frames.get(anchor).filter(|_| !anchor.is_empty()),
        );
        let moved = at != IDENTITY;
        let shaders: Vec<TagId> = geo.materials.iter().map(|m| m.shader.id).collect();
        for si in geo.default_sections() {
            let Some(section) = geo.sections.get(si) else {
                continue;
            };
            let v = &section.vertices;
            for part in section.geometry.parts.iter().filter(|p| p.is_drawn()) {
                let mut out = WorldPart {
                    shader: shaders
                        .get(usize::from(part.material))
                        .copied()
                        .unwrap_or(TagId::NULL),
                    transparent: part.kind == geometry::PART_TRANSPARENT,
                    ..WorldPart::default()
                };
                let mut remap = HashMap::new();
                for t in section.geometry.part_triangles(part) {
                    if t.iter().any(|&i| usize::from(i) >= v.positions.len())
                        || t[0] == t[1]
                        || t[1] == t[2]
                        || t[0] == t[2]
                    {
                        continue;
                    }
                    let tri = t.map(|i| {
                        *remap.entry(i).or_insert_with(|| {
                            let i = usize::from(i);
                            out.positions.push(v.positions[i]);
                            out.normals
                                .push(v.normals.get(i).map_or([0.0, 0.0, 1.0], |&n| normalize(n)));
                            out.uvs
                                .push(v.texcoords.get(i).copied().unwrap_or([0.0; 2]));
                            (out.positions.len() - 1) as u32
                        })
                    });
                    out.tris.push(tri);
                }
                *tris += out.tris.len();
                if *tris > MAX_MODEL_TRIANGLES {
                    return Some(());
                }
                if v.normals.is_empty() {
                    out.normals = geometry::vertex_normals(&out.positions, &out.tris);
                }
                if moved {
                    place_part(&mut out, &at);
                }
                let (look, mut blend) = self.look(out.shader);
                // Part kind 4 is see-through; an added layer stays added.
                if out.transparent && !blend.see_through() {
                    blend = Blend::Translucent;
                }
                mesh.push_look(&look, None, blend, &out);
            }
        }
        for a in &model.attached {
            let parent = if a.parent_marker.is_empty() {
                IDENTITY
            } else {
                let Some(m) = frames.get(&a.parent_marker) else {
                    continue;
                };
                *m
            };
            let placed = attachment_transform(&at, &parent, None);
            // A child that does not read is left off; the parent stays.
            let _ = self.push_model(&a.model, &placed, &a.child_marker, mesh, tris);
            if *tris > MAX_MODEL_TRIANGLES {
                return Some(());
            }
        }
        Some(())
    }

    /// A shader's base map and blend, read the first time it is asked for
    /// (and listed in [`Scene::materials`]). A see-through shader's blend
    /// is its passes' ([`material_blend`]) where they say, else its
    /// template name's ([`template_blend`]). A see-through or
    /// self-illuminated shader whose bitmaps have no diffuse name draws its
    /// first own bitmap: a light shaft's picture is named for what it is.
    fn look(&mut self, shader: TagId) -> (Look, Blend) {
        if shader == TagId::NULL {
            return (Look::plain(None), Blend::Opaque);
        }
        if let Some(l) = self.looks.get(&shader) {
            return *l;
        }
        self.scene.stats.shaders += 1;
        let name_of = |map: &Map<R>, id: TagId| -> Option<String> {
            map.tag(id).ok().and_then(|t| t.name.clone())
        };
        let mut info = MaterialInfo {
            shader: name_of(&*self.map, shader).unwrap_or_else(|| format!("{shader}")),
            template: String::new(),
            blend: Blend::Opaque,
            blend_from_passes: false,
            emissive: false,
            base: None,
            details: 0,
        };
        let look = match shader_bitmaps(self.map, shader) {
            Ok((template, bitmaps)) => {
                let mut blend = template_blend(&template);
                if blend.see_through()
                    && !is_water_template(&template)
                    && let Some(b) = self.blend_of_passes(shader)
                {
                    blend = b;
                    info.blend_from_passes = true;
                }
                let emissive = template_emits(&template);
                let named = |id: TagId| name_of(&*self.map, id);
                let base = bitmaps
                    .iter()
                    .find(|id| named(**id).is_some_and(|n| bitmap_role(&n) == BitmapRole::Diffuse))
                    .or_else(|| {
                        (blend.see_through() || emissive).then(|| {
                            bitmaps
                                .iter()
                                .find(|id| named(**id).is_some_and(|n| !is_engine_bitmap(&n)))
                        })?
                    })
                    .copied();
                let clamp = template_clamps(&template);
                let texture = base.and_then(|b| self.texture(b, blend == Blend::Mask, clamp));
                let mut look = match (base, texture) {
                    (Some(base), Some(_)) => {
                        let plan = self.detail_plan(shader, base);
                        look_of(texture, &plan, |b| self.texture(b, false, false))
                    }
                    _ => Look::plain(texture),
                };
                look.emissive = emissive;
                info.template = template;
                info.blend = blend;
                info.emissive = emissive;
                info.base = base.filter(|_| texture.is_some()).and_then(|b| name_of(&*self.map, b));
                info.details = look.details.iter().flatten().count();
                (look, blend)
            }
            Err(_) => (Look::plain(None), Blend::Opaque),
        };
        if look.0.texture.is_some() {
            self.scene.stats.textured += 1;
        }
        self.scene.materials.push(info);
        self.looks.insert(shader, look);
        look
    }

    /// [`material_blend`] of a shader's resolved material, through the
    /// map's shader library; `None` when it does not resolve.
    fn blend_of_passes(&mut self, shader: TagId) -> Option<Blend> {
        let library = self
            .library
            .get_or_insert_with(|| tags::material::ShaderLibrary::read(&mut *self.map).ok());
        let material = library.as_mut()?.material(shader, None).ok()?;
        material_blend(&material)
    }

    /// The scales and detail maps of a shader's base map `base`, from its
    /// resolved material ([`detail_plan`]); the base map alone when the
    /// material does not resolve.
    fn detail_plan(&mut self, shader: TagId, base: TagId) -> DetailPlan {
        let library = self
            .library
            .get_or_insert_with(|| tags::material::ShaderLibrary::read(&mut *self.map).ok());
        let Some(material) = library.as_mut().and_then(|l| l.material(shader, None).ok()) else {
            return DetailPlan::default();
        };
        let map = &*self.map;
        detail_plan(&stage_bindings(&material), base, |id| {
            map.tag(id)
                .ok()
                .and_then(|t| t.name.as_deref())
                .is_some_and(is_detail_map)
        })
    }

    fn texture(&mut self, bitmap: TagId, cutout: bool, clamp: bool) -> Option<usize> {
        if let Some(t) = self.bitmaps.get(&(bitmap, cutout, clamp)) {
            return *t;
        }
        let t = match decode_bitmap(self.map, &mut self.files, bitmap) {
            Some(mut image) => {
                if cutout {
                    image.stretch_alpha();
                }
                let image = image.fit(MAX_TEXTURE);
                self.scene.lifts.push(image.lift_factor());
                self.scene.textures.push(image);
                let t = self.scene.textures.len() - 1;
                if clamp {
                    self.scene.clamped.insert(t);
                }
                Some(t)
            }
            None => {
                self.scene.stats.bitmaps_failed += 1;
                None
            }
        };
        self.bitmaps.insert((bitmap, cutout, clamp), t);
        t
    }
}

/// A lightmap page as RGBA: its `P8` indices (which `assets` decodes as
/// `L8`, the index in each channel) through the group's palette of
/// `A8R8G8B8` entries.
pub fn lightmap_page(
    image: &tags::Image,
    palette: &[u32; 256],
    files: &mut ResourceFiles,
) -> Option<Image> {
    if image.format != tags::PixelFormat::P8 {
        return None;
    }
    let decoded = assets::decode_image(image, files).ok()?;
    let level = decoded.levels.into_iter().find(|l| l.face == 0)?;
    Some(Image {
        width: level.width,
        height: level.height,
        rgba: apply_palette(&level.rgba, palette),
    })
}

/// Replace each texel (an index in its red channel) by its palette entry.
pub fn apply_palette(indices: &[u8], palette: &[u32; 256]) -> Vec<u8> {
    indices
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| {
            let c = palette[usize::from(p[0])];
            [(c >> 16) as u8, (c >> 8) as u8, c as u8, 255]
        })
        .collect()
}

/// An object's render model: object → `hlmt` → `mode`. An item or vehicle
/// collection (`itmc`, `vehc`, what netgame equipment names) stands for its
/// first entry.
pub fn render_model_of(ctx: &tags::object::Context, object: TagId) -> Option<TagId> {
    let group = ctx.catalog.get(object)?.group;
    let object = if group == FourCC(*b"itmc") || group == FourCC(*b"vehc") {
        let tag = ctx.decode(object).ok()?;
        tag.layers
            .first()?
            .elements("entries")
            .iter()
            .filter_map(|e| e.tag_ref("object"))
            .map(|r| r.id)
            .find(|id| *id != TagId::NULL)?
    } else {
        object
    };
    let resolved = ctx.resolve_object(object).ok()?;
    let mode = resolved.model?.render_model?;
    (mode.group == FourCC(*b"mode")).then_some(mode.id)
}

/// What a placed object draws: its render model and the objects its model
/// variant attaches on markers (a Warthog's chaingun, a Wraith's mortar,
/// a Scorpion's or Spectre's turret: each its own vehicle tag, named by
/// hlmt variant `+0x1c`, not part of the hull's render model).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ObjectModel {
    pub mode: TagId,
    pub attached: Vec<Attached>,
}

/// One attached object ([`ObjectModel::attached`]).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Attached {
    /// The parent's marker it sits on; empty for the parent's origin.
    pub parent_marker: String,
    /// Its own marker put there; empty for its origin.
    pub child_marker: String,
    pub model: ObjectModel,
}

/// How deep attachments are followed (a turret on a vehicle, and what the
/// turret carries).
pub const MAX_ATTACH_DEPTH: usize = 2;

/// An object's [`ObjectModel`]: [`render_model_of`]'s model, and the
/// objects of the variant it is drawn with. That variant is the
/// collection entry's (`vehc`/`itmc`), else `multiplayer` on a
/// multiplayer map when the model has one (the variant the map's vehicles
/// spawn with; the Live art's rule, `cli::live_models::pick_variant`), else
/// the object's default variant as the engine picks it.
pub fn object_model_of(
    ctx: &tags::object::Context,
    object: TagId,
    multiplayer: bool,
) -> Option<ObjectModel> {
    object_model_at(ctx, object, multiplayer, 0)
}

fn object_model_at(
    ctx: &tags::object::Context,
    object: TagId,
    multiplayer: bool,
    depth: usize,
) -> Option<ObjectModel> {
    let group = ctx.catalog.get(object)?.group;
    let (object, entry_variant) = if group == FourCC(*b"itmc") || group == FourCC(*b"vehc") {
        let tag = ctx.decode(object).ok()?;
        tag.layers
            .first()?
            .elements("entries")
            .iter()
            .filter_map(|e| {
                let r = e.tag_ref("object")?;
                let variant = e.sid("variant").map(|s| s.0);
                Some((r.id, variant))
            })
            .find(|(id, _)| *id != TagId::NULL)?
    } else {
        (object, None)
    };
    let resolved = ctx.resolve_object(object).ok()?;
    let mut model = resolved.model?;
    let mode = model.render_model.as_ref()?;
    if mode.group != FourCC(*b"mode") {
        return None;
    }
    let mode = mode.id;
    let names: Vec<(tags::wire::StringId, Option<String>)> = model
        .tag
        .layers
        .first()
        .map(|root| root.elements("variants"))
        .unwrap_or(&[])
        .iter()
        .filter_map(|v| v.sid("name"))
        .map(|(id, s)| (id, s.map(str::to_owned)))
        .collect();
    let has = |v: tags::wire::StringId| names.iter().any(|(id, _)| *id == v);
    let multiplayer_variant = names
        .iter()
        .find(|(_, s)| s.as_deref() == Some("multiplayer"))
        .map(|(id, _)| *id);
    let preferred = entry_variant
        .filter(|v| v.0 != 0 && has(*v))
        .or(multiplayer_variant.filter(|_| multiplayer));
    if let Some(v) = preferred
        && let Ok(m) = ctx.resolve_model(model.id, v)
    {
        model = m;
    }
    let attached = if depth < MAX_ATTACH_DEPTH {
        model
            .objects
            .iter()
            .filter_map(|o| {
                Some(Attached {
                    parent_marker: o.parent_marker.clone(),
                    child_marker: o.child_marker.clone(),
                    model: object_model_at(ctx, o.object, multiplayer, depth + 1)?,
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    Some(ObjectModel { mode, attached })
}

/// Column-major 4x4 identity.
const IDENTITY: Mat4 = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

fn transform_point(m: &Mat4, p: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|r| m[0][r] * p[0] + m[1][r] * p[1] + m[2][r] * p[2] + m[3][r])
}

fn transform_vector(m: &Mat4, v: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|r| m[0][r] * v[0] + m[1][r] * v[1] + m[2][r] * v[2])
}

/// Where an attached object's model space lands in its parent's: the
/// parent's own placement `at`, then the parent's marker frame, then the
/// child's anchor marker (if it names one) moved back to the origin.
pub fn attachment_transform(at: &Mat4, parent_marker: &Mat4, child_anchor: Option<&Mat4>) -> Mat4 {
    let on_marker = mat_mul(at, parent_marker);
    match child_anchor {
        Some(c) => mat_mul(&on_marker, &invert_rigid(c)),
        None => on_marker,
    }
}

/// A part moved by `at` (positions and normals).
pub fn place_part(part: &mut WorldPart, at: &Mat4) {
    for p in &mut part.positions {
        *p = transform_point(at, *p);
    }
    for n in &mut part.normals {
        *n = normalize(transform_vector(at, *n));
    }
}

/// The inverse of a rotation, uniform scale and translation.
fn invert_rigid(m: &Mat4) -> Mat4 {
    let s2: f32 = (0..3).map(|r| m[0][r] * m[0][r]).sum();
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

/// A local frame: rotation (unit quaternion `[x, y, z, w]`) then
/// translation.
fn frame(translation: [f32; 3], rotation: [f32; 4]) -> Mat4 {
    NodeTransform {
        rotation,
        translation,
        ..NodeTransform::default()
    }
    .matrix()
}

/// The render model's nodes at rest, in model space: each node's default
/// translation and orientation (`ModeNode +0xc`, `+0x18`) relative to its
/// parent, composed down from the roots.
pub fn rest_nodes(geo: &RenderModelGeometry) -> Vec<Mat4> {
    let nodes: Vec<RestNode> = geo
        .nodes
        .iter()
        .map(|n| RestNode {
            parent: n.wire.parent_index,
            translation: n.wire.translation,
            orientation: n.wire.orientation,
        })
        .collect();
    rest_frames(&nodes)
}

/// A render-model node at rest, relative to its parent (`-1`: a root).
#[derive(Clone, Copy, Debug)]
pub struct RestNode {
    pub parent: i16,
    pub translation: [f32; 3],
    /// Unit quaternion `[x, y, z, w]`.
    pub orientation: [f32; 4],
}

/// [`rest_nodes`] over plain nodes: a node whose parent is out of range
/// (or itself) is a root; one in a cycle is left at the origin.
pub fn rest_frames(nodes: &[RestNode]) -> Vec<Mat4> {
    let n = nodes.len();
    let mut world: Vec<Option<Mat4>> = vec![None; n];
    // Parents may follow their children; resolve up to `n` passes.
    for _ in 0..n {
        let mut progress = false;
        for (i, node) in nodes.iter().enumerate() {
            if world[i].is_some() {
                continue;
            }
            let local = frame(node.translation, node.orientation);
            let parent = usize::try_from(node.parent).ok();
            world[i] = match parent {
                None => Some(local),
                Some(p) if p == i || p >= n => Some(local),
                Some(p) => world[p].map(|pw| mat_mul(&pw, &local)),
            };
            progress |= world[i].is_some();
        }
        if !progress {
            break;
        }
    }
    world.into_iter().map(|m| m.unwrap_or(IDENTITY)).collect()
}

/// The render model's markers in model space at rest, by name (the first
/// marker of each group): header `+0x58` is a block of marker groups (0xc:
/// string-ID name, block of markers); a marker is 0x24 bytes with its node
/// at `+2`, translation `+4` and rotation `+0x10`, relative to the node
/// (docs/live/live-art.md, "Markers").
pub fn marker_frames<R: Read + Seek>(
    map: &mut Map<R>,
    mode: TagId,
    geo: &RenderModelGeometry,
) -> HashMap<String, Mat4> {
    let out = HashMap::new();
    let Some(address) = map
        .tag(mode)
        .ok()
        .and_then(|t| t.location.as_ref())
        .map(|l| l.address)
    else {
        return out;
    };
    let Ok(bytes) = map.read_tag_bytes(mode) else {
        return out;
    };
    markers_in(&bytes, address, map.strings(), &rest_nodes(geo))
}

/// [`marker_frames`] over a render model's payload (`bytes` at
/// `address`), its nodes at rest in model space.
pub fn markers_in(
    bytes: &[u8],
    address: u32,
    strings: &[Option<String>],
    nodes: &[Mat4],
) -> HashMap<String, Mat4> {
    let mut out = HashMap::new();
    let view = TagView::new(bytes, address);
    let Ok(root) = view.at(0, 0x60) else {
        return out;
    };
    let Ok(groups) = view.block(root, 0x58, 0xc) else {
        return out;
    };
    let f32_at = |b: &[u8], o: usize| f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    for g in groups {
        let id = tags::wire::StringId(u32::from_le_bytes([g[0], g[1], g[2], g[3]]));
        let Some(name) = id.resolve(strings).ok().flatten() else {
            continue;
        };
        if out.contains_key(name) {
            continue;
        }
        let Some(m) = view.block(g, 4, 0x24).ok().and_then(|m| m.first().copied()) else {
            continue;
        };
        let Some(node) = nodes.get(usize::from(m[2])) else {
            continue;
        };
        let translation = [f32_at(m, 4), f32_at(m, 8), f32_at(m, 0xc)];
        let rotation = [
            f32_at(m, 0x10),
            f32_at(m, 0x14),
            f32_at(m, 0x18),
            f32_at(m, 0x1c),
        ];
        out.insert(
            name.to_owned(),
            mat_mul(node, &frame(translation, rotation)),
        );
    }
    out
}

/// A shader's template name and bitmaps: `shad +0` is the template
/// reference, `shad +0x20` the compiled postprocess (one element), whose
/// `+4` block lists the bitmap parameters (0xc bytes, datum first;
/// docs/formats/shader-instances.md).
///
/// The compiler shares identical compiled data between shaders: a
/// shader whose postprocess matches another's points into that one's
/// payload (many retail shaders, Containment's blue base banner and lights
/// among them), so the blocks are read wherever in the metadata they lie
/// ([`metadata_block`]), as `cli::live_models::shader_bitmaps` does.
pub fn shader_bitmaps<R: Read + Seek>(
    map: &mut Map<R>,
    shader: TagId,
) -> crate::Result<(String, Vec<TagId>)> {
    let tag = map.tag(shader)?.clone();
    if tag.group != FourCC(*b"shad") {
        return Err(crate::Error::Invalid("not a shader"));
    }
    if tag.location.is_none() {
        return Err(crate::Error::Invalid("shader has no payload"));
    }
    let bytes = map.read_tag_bytes(shader)?;
    if bytes.len() < 0x54 {
        return Err(crate::Error::Invalid("shader too short"));
    }
    let word = |b: &[u8], o: usize| -> Option<u32> {
        Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
    };
    let template = word(&bytes, 4)
        .and_then(|id| map.tag(TagId(id)).ok())
        .and_then(|t| t.name.clone())
        .unwrap_or_default();
    let post = metadata_block(map, &bytes, 0x20, 0x7c)?;
    let post = post
        .first()
        .ok_or(crate::Error::Invalid("no compiled postprocess"))?;
    let ids = metadata_block(map, post, 4, 0xc)?
        .iter()
        .filter_map(|b| word(b, 0).map(TagId))
        .filter(|id| *id != TagId::NULL)
        .collect();
    Ok((template, ids))
}

/// The largest tag block [`metadata_block`] reads.
const MAX_METADATA_BLOCK: u64 = 1 << 20;

/// The elements of the tag block whose (count, address) pair is at
/// `field` in `record`, read wherever in the ordinary metadata they lie.
pub fn metadata_block<R: Read + Seek>(
    map: &mut Map<R>,
    record: &[u8],
    field: usize,
    stride: usize,
) -> crate::Result<Vec<Vec<u8>>> {
    let pair = record
        .get(field..field + 8)
        .ok_or(crate::Error::Invalid("tag block field outside its record"))?;
    let count = u32::from_le_bytes([pair[0], pair[1], pair[2], pair[3]]);
    let address = u32::from_le_bytes([pair[4], pair[5], pair[6], pair[7]]);
    if count == 0 || stride == 0 {
        return Ok(Vec::new());
    }
    let len = u64::from(count) * stride as u64;
    if len > MAX_METADATA_BLOCK {
        return Err(crate::Error::Invalid("tag block too large"));
    }
    let at = map.file_offset(address, len as u32)?;
    let bytes = map.read_file_range(at, len)?;
    Ok(bytes.chunks_exact(stride).map(<[u8]>::to_vec).collect())
}

/// The first image of a bitmap tag, its largest decoded level, as RGBA.
pub fn decode_bitmap<R: Read + Seek>(
    map: &mut Map<R>,
    files: &mut ResourceFiles,
    id: TagId,
) -> Option<Image> {
    let bitmap = match tags::DecodedTag::read(map, id).ok()?.data {
        tags::TagData::Bitmap(b) => b,
        _ => return None,
    };
    let decoded = assets::decode_image(bitmap.images.first()?, files).ok()?;
    let level = decoded
        .levels
        .into_iter()
        .find(|l| l.face == 0 && l.depth <= 1)?;
    (level.rgba.len() == (level.width * level.height * 4) as usize).then_some(Image {
        width: level.width,
        height: level.height,
        rgba: level.rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reference art: base maps first, then lightmap pages (unlifted), a
    /// part's page index shifted past the base maps, cut-out alpha
    /// stretched, and the counts carried to the stats.
    #[test]
    fn reference_parts_index_their_textures_and_pages() {
        let pic = |c: u8, a: u8| Image {
            width: 2,
            height: 2,
            rgba: [c, c, c, a].repeat(4),
        };
        let part = |lit: bool| WorldPart {
            positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            uvs: vec![[0.0; 2]; 3],
            lightmap_uvs: if lit { vec![[0.5; 2]; 3] } else { Vec::new() },
            tris: vec![[0, 1, 2]],
            ..WorldPart::default()
        };
        let parts = [
            ReferencePart {
                part: part(true),
                texture: Some(1),
                lightmap: Some(0),
                blend: Blend::Mask,
            },
            ReferencePart {
                part: part(false),
                texture: Some(0),
                lightmap: Some(0),
                blend: Blend::Opaque,
            },
        ];
        let s = reference_scene(
            vec![pic(40, 255), pic(200, 64)],
            vec![pic(90, 255)],
            &parts,
            3,
            2,
        );
        assert_eq!(s.textures.len(), 3);
        assert_eq!(s.lifts[2], 1.0);
        assert_eq!(
            s.textures[1].rgba[3], 255,
            "the cut-out's alpha is stretched"
        );
        let lit = s
            .world
            .batches
            .iter()
            .find(|b| b.texture == Some(1))
            .unwrap();
        assert_eq!(lit.lightmap, Some(2));
        assert_eq!(lit.vertices[0].lightmap_uv, [0.5; 2]);
        // A part without lightmap coordinates is not lit by the page.
        let unlit = s
            .world
            .batches
            .iter()
            .find(|b| b.texture == Some(0))
            .unwrap();
        assert_eq!(unlit.lightmap, None);
        assert_eq!(
            (s.stats.shaders, s.stats.textured, s.stats.lightmap_pages),
            (3, 2, 1)
        );
    }

    #[test]
    fn templates_and_bitmap_names_choose_blend_and_base_map() {
        assert_eq!(
            template_blend("shaders\\shader_templates\\opaque\\tex_bump"),
            Blend::Opaque
        );
        assert_eq!(
            template_blend("shaders\\shader_templates\\opaque\\tex_bump_alpha_test"),
            Blend::Mask
        );
        assert_eq!(
            template_blend("shaders\\shader_templates\\transparent\\one_add"),
            Blend::Additive
        );
        // mainmenu's light shafts and the converted Halo 1 sky domes add;
        // cloud decks blend by alpha.
        assert_eq!(
            template_blend("shaders\\shader_templates\\transparent\\one_add_illum_detail"),
            Blend::Additive
        );
        assert_eq!(
            template_blend("shaders\\shader_templates\\transparent\\sky_two_alpha_clouds"),
            Blend::Alpha
        );
        assert_eq!(
            template_blend("shaders\\shader_templates\\transparent\\one_mult"),
            Blend::Multiply
        );
        // A see-through template whose name does not say: fixed opacity.
        assert_eq!(
            template_blend("shaders\\shader_templates\\transparent\\meter"),
            Blend::Translucent
        );
        // A word of the stem, not any `add` in it.
        assert_eq!(
            template_blend("shaders\\shader_templates\\transparent\\address_x"),
            Blend::Translucent
        );
        assert_eq!(
            template_blend("shaders\\shader_templates\\transparent\\two_alpha_clouds"),
            Blend::Alpha
        );
        assert_eq!(
            template_blend("shaders\\shader_templates\\transparent\\ladder_glass"),
            Blend::Translucent
        );
        assert_eq!(
            template_blend("shaders\\shader_templates\\opaque\\one_add"),
            Blend::Opaque
        );
        assert_eq!(
            template_blend("shaders\\shader_templates\\opaque\\tex_bump_detail_overlay"),
            Blend::Opaque
        );
        assert_eq!(
            template_blend("shaders\\shader_templates\\water\\water_pool"),
            Blend::Translucent
        );
        assert_eq!(
            bitmap_role("scenarios\\bitmaps\\a\\metal_panels"),
            BitmapRole::Diffuse
        );
        for other in [
            "scenarios\\bitmaps\\a\\metal_panels_bump",
            "scenarios\\bitmaps\\a\\panels_vertical_outdoor_bmp",
            "scenarios\\bitmaps\\detail_maps\\grain",
            "rasterizer\\x",
            "objects\\x\\body_cc",
            "objects\\x\\self_illum",
        ] {
            assert_eq!(bitmap_role(other), BitmapRole::Other, "{other}");
        }
    }

    /// The native renderer's blend factors read as the viewport's blends;
    /// factors it does not draw are none.
    #[test]
    fn blend_states_map_to_blends() {
        use wgpu::{BlendComponent, BlendFactor as F, BlendOperation, BlendState};
        let state = |src, dst| BlendState {
            color: BlendComponent {
                src_factor: src,
                dst_factor: dst,
                operation: BlendOperation::Add,
            },
            alpha: BlendComponent::REPLACE,
        };
        assert_eq!(blend_of_state(&state(F::One, F::One)), Some(Blend::Additive));
        assert_eq!(blend_of_state(&state(F::SrcAlpha, F::One)), Some(Blend::Additive));
        assert_eq!(
            blend_of_state(&state(F::SrcAlpha, F::OneMinusSrcAlpha)),
            Some(Blend::Alpha)
        );
        assert_eq!(blend_of_state(&state(F::Dst, F::Zero)), Some(Blend::Multiply));
        assert_eq!(blend_of_state(&state(F::Zero, F::Src)), Some(Blend::Multiply));
        assert_eq!(blend_of_state(&state(F::Dst, F::Src)), Some(Blend::Double));
        assert_eq!(blend_of_state(&state(F::One, F::Zero)), None);
        let mut sub = state(F::One, F::One);
        sub.color.operation = BlendOperation::ReverseSubtract;
        assert_eq!(blend_of_state(&sub), None);
        assert!(Blend::Additive.see_through() && !Blend::Mask.see_through());
        assert_eq!(Blend::ALL.len(), 7);
        assert!(Blend::ALL.windows(2).all(|w| w[0] < w[1]), "draw order");
    }

    #[test]
    fn illum_templates_emit_and_engine_bitmaps_are_never_pictures() {
        assert!(template_emits("shaders\\shader_templates\\opaque\\illum"));
        assert!(!template_emits(
            "shaders\\shader_templates\\opaque\\tex_bump_env_illum_combined"
        ));
        assert!(is_water_template("shaders\\shader_templates\\water\\water_pool"));
        assert!(!is_water_template("shaders\\shader_templates\\transparent\\one_add"));
        assert!(is_engine_bitmap("shaders\\default_bitmaps\\bitmaps\\gray_50_percent"));
        assert!(is_engine_bitmap("rasterizer\\vector_normalization"));
        assert!(!is_engine_bitmap("scenarios\\bitmaps\\solo\\light_volume"));
    }

    fn close3(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4)
    }

    #[test]
    fn sky_lights_point_up_and_tint_by_hue() {
        // Yaw 0, pitch 90 degrees: straight up.
        assert!(close3(sun_direction(0.0, std::f32::consts::FRAC_PI_2), [0.0, 0.0, 1.0]));
        // Pitch -45 degrees along +y: turned above the horizon.
        let d = sun_direction(std::f32::consts::FRAC_PI_2, -std::f32::consts::FRAC_PI_4);
        let h = std::f32::consts::FRAC_1_SQRT_2;
        assert!(close3(d, [0.0, -h, h]), "{d:?}");
        assert!(close3(sun_tint([2.0, 1.0, 0.5]), [1.0, 0.5, 0.25]));
        assert_eq!(sun_tint([0.0; 3]), [1.0; 3]);
        assert_eq!(sun_tint([f32::NAN, 1.0, 1.0]), [1.0; 3]);
    }

    /// The engine's fog check (`0x001301c0`): no density, no fog; colour
    /// and density clamped; the opaque distance past the start.
    #[test]
    fn fog_follows_the_engines_check() {
        assert_eq!(fog_of([0.5; 3], 0.0, 0.0, 200.0), None);
        assert_eq!(fog_of([0.5; 3], f32::NAN, 0.0, 200.0), None);
        let f = fog_of([1.5, 0.25, -1.0], 2.0, 10.0, 5.0).unwrap();
        assert_eq!(f.color, [1.0, 0.25, 0.0]);
        assert_eq!(f.density, 1.0);
        assert_eq!((f.start, f.opaque), (10.0, 11.0));
        let f = fog_of([0.5; 3], 0.6, 5.0, 205.0).unwrap();
        assert_eq!(fog_amount(&f, 0.0), 0.0);
        assert!((fog_amount(&f, 105.0) - 0.3).abs() < 1e-6);
        assert!((fog_amount(&f, 1000.0) - 0.6).abs() < 1e-6);
    }

    #[test]
    fn colour_conversion_and_bilinear_samples_are_linear() {
        assert_eq!(srgb_to_linear(0), 0.0);
        assert!((srgb_to_linear(255) - 1.0).abs() < 1e-6);
        // sRGB 188 is about linear 0.5.
        assert!((srgb_to_linear(188) - 0.5).abs() < 0.01);
        let img = image(2, 1, |x, _| if x == 0 { [0, 0, 0, 255] } else { [255, 255, 255, 255] });
        // Texel centres, and halfway between them.
        assert!(close3(img.sample_linear([0.25, 0.5]), [0.0; 3]));
        assert!(close3(img.sample_linear([0.75, 0.5]), [1.0; 3]));
        assert!(close3(img.sample_linear([0.5, 0.5]), [0.5; 3]));
        // Clamped at the edges.
        assert!(close3(img.sample_linear([-3.0, 9.0]), [0.0; 3]));
        let lit = lightmap_light([0.5; 3]);
        let want = 0.5 * crate::render::LIGHTMAP_SCALE + crate::render::LIGHTMAP_FILL;
        assert!(close3(lit, [want; 3]));
    }

    #[test]
    fn barycentric_weights_find_points_inside_a_floor_triangle() {
        let t = [[0.0, 0.0, 1.0], [4.0, 0.0, 1.0], [0.0, 4.0, 1.0]];
        let w = barycentric_xy(&t, [1.0, 1.0]).unwrap();
        assert!(close3(w, [0.5, 0.25, 0.25]), "{w:?}");
        assert_eq!(barycentric_xy(&t, [3.0, 3.0]), None, "outside");
        let wall = [[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [4.0, 0.0, 3.0]];
        assert_eq!(barycentric_xy(&wall, [2.0, 0.0]), None, "edge-on");
    }

    /// A floor of two lightmapped quads at different heights, one page
    /// dark and one bright: a point takes the highest floor under it (not
    /// one above it), at its interpolated lightmap coordinates.
    #[test]
    fn objects_take_the_light_of_the_floor_under_them() {
        let quad = |z: f32, x0: f32| WorldPart {
            positions: vec![[x0, 0.0, z], [x0 + 10.0, 0.0, z], [x0 + 10.0, 10.0, z], [x0, 10.0, z]],
            normals: vec![[0.0, 0.0, 1.0]; 4],
            uvs: vec![[0.0; 2]; 4],
            lightmap_uvs: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            tris: vec![[0, 1, 2], [0, 2, 3]],
            ..WorldPart::default()
        };
        let dark = image(1, 1, |_, _| [64, 64, 64, 255]);
        let bright = image(2, 1, |x, _| if x == 0 { [255, 0, 0, 255] } else { [0, 0, 255, 255] });
        let mut scene = Scene {
            textures: vec![dark, bright],
            lifts: vec![1.0; 2],
            ..Scene::default()
        };
        scene.world.push(None, Some(0), Blend::Opaque, &quad(0.0, 0.0));
        scene.world.push(None, Some(1), Blend::Opaque, &quad(5.0, 0.0));
        // A see-through deck above is no floor.
        scene.world.push(None, Some(0), Blend::Translucent, &quad(6.0, 0.0));
        scene.floor = LightFloor::of(&scene.world);
        // On the upper floor: its left texel (red) at u = 0.1.
        let on_top = scene.light_at([1.0, 5.0, 5.2]).unwrap();
        assert!(on_top[0] > on_top[2] * 4.0, "{on_top:?}");
        // At its right edge, the blue texel.
        let right = scene.light_at([9.9, 5.0, 5.0]).unwrap();
        assert!(right[2] > right[0] * 4.0, "{right:?}");
        // Under the upper floor (between the two): the lower, dark one.
        let below = scene.light_at([5.0, 5.0, 2.0]).unwrap();
        let want = lightmap_light([srgb_to_linear(64); 3]);
        assert!(close3(below, want), "{below:?}");
        // Off the floor: nothing.
        assert_eq!(scene.light_at([50.0, 50.0, 0.0]), None);
        // Far above both (past FLOOR_BELOW): nothing.
        assert_eq!(scene.light_at([5.0, 5.0, 500.0]), None);
    }

    /// A triangle wider than the grid's span is still found.
    #[test]
    fn wide_floor_triangles_are_found_anywhere_under_them() {
        let big = WorldPart {
            positions: vec![[-1000.0, -1000.0, 0.0], [1000.0, -1000.0, 0.0], [0.0, 1000.0, 0.0]],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            uvs: vec![[0.0; 2]; 3],
            lightmap_uvs: vec![[0.5; 2]; 3],
            tris: vec![[0, 1, 2]],
            ..WorldPart::default()
        };
        let mut scene = Scene {
            textures: vec![image(1, 1, |_, _| [128, 128, 128, 255])],
            lifts: vec![1.0],
            ..Scene::default()
        };
        scene.world.push(None, Some(0), Blend::Opaque, &big);
        scene.floor = LightFloor::of(&scene.world);
        assert!(scene.light_at([300.0, -200.0, 1.0]).is_some());
    }

    #[test]
    fn clamped_templates_clamp_their_base_map() {
        assert!(template_clamps(
            "shaders\\shader_templates\\transparent\\sky_one_alpha_env_clamped"
        ));
        assert!(!template_clamps(
            "shaders\\shader_templates\\transparent\\sky_one_alpha_env"
        ));
        assert!(!template_clamps(
            "shaders\\clamped_folder\\opaque\\tex_bump"
        ));
    }

    fn image(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Image {
        let mut rgba = Vec::new();
        for y in 0..h {
            for x in 0..w {
                rgba.extend(f(x, y));
            }
        }
        Image {
            width: w,
            height: h,
            rgba,
        }
    }

    #[test]
    fn images_halve_fit_and_chain_to_one_texel() {
        let img = image(4, 2, |x, _| {
            if x < 2 {
                [0, 0, 0, 255]
            } else {
                [200, 100, 50, 255]
            }
        });
        let h = img.clone().half();
        assert_eq!((h.width, h.height), (2, 1));
        assert_eq!(&h.rgba, &[0, 0, 0, 255, 200, 100, 50, 255]);
        let fit = image(1024, 256, |_, _| [1; 4]).fit(512);
        assert_eq!((fit.width, fit.height), (512, 128));
        let chain = img.mip_chain();
        let sizes: Vec<_> = chain.iter().map(|i| (i.width, i.height)).collect();
        assert_eq!(sizes, [(4, 2), (2, 1), (1, 1)]);
    }

    #[test]
    fn dark_textures_are_lifted_and_bright_ones_kept() {
        // 40 is 0.017 linear: tripled (the cap).
        assert_eq!(image(2, 1, |_, _| [40, 40, 40, 7]).lift_factor(), MAX_LIFT);
        let half = image(1, 1, |_, _| [97, 97, 97, 255]).lift_factor();
        assert!((half - 1.36).abs() < 0.05, "{half}");
        assert_eq!(image(1, 1, |_, _| [200, 180, 160, 255]).lift_factor(), 1.0);
    }

    #[test]
    fn lightmap_indices_go_through_the_palette() {
        let mut palette = [0u32; 256];
        palette[3] = 0xff10_2030;
        palette[255] = 0x80ff_eedd;
        let out = apply_palette(&[3, 3, 3, 255, 255, 255, 255, 255], &palette);
        assert_eq!(out, [0x10, 0x20, 0x30, 255, 0xff, 0xee, 0xdd, 255]);
    }

    #[test]
    fn cutout_alpha_is_stretched_to_its_strongest() {
        let mut img = image(2, 1, |x, _| [0, 0, 0, if x == 0 { 64 } else { 16 }]);
        img.stretch_alpha();
        assert_eq!((img.rgba[3], img.rgba[7]), (255, 127));
    }

    fn quad(transparent: bool) -> WorldPart {
        WorldPart {
            shader: TagId::NULL,
            transparent,
            positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0]],
            normals: vec![[0.0, 0.0, 1.0]; 4],
            uvs: vec![[0.0; 2]; 4],
            tris: vec![[0, 1, 2], [1, 3, 2], [0, 1, 9]],
            ..WorldPart::default()
        }
    }

    #[test]
    fn meshes_merge_by_texture_and_blend_and_drop_bad_triangles() {
        let mut m = Mesh::default();
        m.push(Some(0), None, Blend::Opaque, &quad(false));
        m.push(Some(0), None, Blend::Opaque, &quad(false));
        m.push(None, None, Blend::Translucent, &quad(true));
        assert_eq!(m.batches.len(), 2);
        assert_eq!(m.batches[0].vertices.len(), 8);
        assert_eq!(m.batches[0].indices.len(), 12);
        assert_eq!(m.batches[0].indices[6..9], [4, 5, 6]);
        assert_eq!(m.triangle_count(), 6);
        assert_eq!(m.bounds.max, [1.0, 1.0, 0.0]);
        // See-through batches are not hit.
        let down = Ray {
            origin: [0.25, 0.25, 5.0],
            dir: [0.0, 0.0, -1.0],
        };
        assert_eq!(m.hit(&down), Some(5.0));
        let mut glass = Mesh::default();
        glass.push(None, None, Blend::Translucent, &quad(true));
        assert_eq!(glass.hit(&down), None);
        assert!(glass.bounds.is_empty());
    }

    /// See-through batches are drawn in the order of their parts, alpha
    /// and added alike: a sky's added dome and galaxy, then its
    /// alpha-blended planet, then another added layer. Solid batches
    /// come first, cut-outs next. Added ones are neither framed nor hit.
    #[test]
    fn see_through_batches_keep_their_part_order() {
        let mut m = Mesh::default();
        m.push(Some(0), None, Blend::Additive, &quad(true));
        m.push(Some(1), None, Blend::Additive, &quad(true));
        m.push(Some(2), None, Blend::Translucent, &quad(true));
        m.push(Some(3), None, Blend::Additive, &quad(true));
        m.push(Some(4), None, Blend::Mask, &quad(false));
        m.push(Some(5), None, Blend::Opaque, &quad(false));
        let order: Vec<(Option<usize>, Blend)> = m
            .draw_sequence()
            .iter()
            .map(|b| (b.texture, b.blend))
            .collect();
        assert_eq!(
            order,
            [
                (Some(5), Blend::Opaque),
                (Some(4), Blend::Mask),
                (Some(0), Blend::Additive),
                (Some(1), Blend::Additive),
                (Some(2), Blend::Translucent),
                (Some(3), Blend::Additive),
            ]
        );
        let mut glow = Mesh::default();
        glow.push(None, None, Blend::Additive, &quad(true));
        let down = Ray {
            origin: [0.25, 0.25, 5.0],
            dir: [0.0, 0.0, -1.0],
        };
        assert_eq!(glow.hit(&down), None);
        assert!(glow.bounds.is_empty());
        assert!(Blend::Additive.see_through() && Blend::Translucent.see_through());
        assert!(!Blend::Opaque.see_through() && !Blend::Mask.see_through());
    }

    /// The sky's clear colour is used only when its flag (bit 5 of
    /// `+0x10`) is set; otherwise, or when it doesn't read, black.
    #[test]
    fn the_sky_clears_to_its_colour_only_when_flagged() {
        let mut root = vec![0u8; 0xac];
        let put = |root: &mut Vec<u8>, at: usize, v: u32| {
            root[at..at + 4].copy_from_slice(&v.to_le_bytes());
        };
        for (k, c) in [1.0f32, 0.5, 0.0].into_iter().enumerate() {
            put(&mut root, SKY_CLEAR + 4 * k, c.to_bits());
        }
        // A colour without the flag (a converted Halo 1 sky writes 0).
        assert_eq!(sky_clear(&root), [0.0; 3]);
        put(&mut root, SKY_FLAGS, SKY_CLEAR_FLAG | 1);
        let c = sky_clear(&root);
        assert_eq!(c[0], 1.0);
        assert!((c[1] - 0.5f32.powf(2.2)).abs() < 1e-6, "linear: {c:?}");
        assert_eq!(c[2], 0.0);
        // Other flag bits alone don't turn it on.
        put(&mut root, SKY_FLAGS, !SKY_CLEAR_FLAG);
        assert_eq!(sky_clear(&root), [0.0; 3]);
        // Out of range is clamped; not finite, or cut short, is black.
        put(&mut root, SKY_FLAGS, SKY_CLEAR_FLAG);
        put(&mut root, SKY_CLEAR, 4.0f32.to_bits());
        assert_eq!(sky_clear(&root)[0], 1.0);
        put(&mut root, SKY_CLEAR, f32::NAN.to_bits());
        assert_eq!(sky_clear(&root), [0.0; 3]);
        put(&mut root, SKY_CLEAR, 1.0f32.to_bits());
        assert_eq!(sky_clear(&root[..SKY_CLEAR + 8]), [0.0; 3]);
    }

    #[test]
    fn models_are_picked_through_their_placement() {
        let mut mesh = Mesh::default();
        mesh.push(None, None, Blend::Opaque, &quad(false));
        let mut scene = Scene::default();
        scene.models.push(mesh);
        scene.objects.insert(TagId(7), 0);
        // Turned 90° about z, doubled, at (10, 0, 0): the quad covers
        // x in [8, 10], y in [0, 2].
        let marker = Marker {
            block: BlockKind::Scenery,
            index: 3,
            position: [10.0, 0.0, 0.0],
            axes: geometry::rotation([std::f32::consts::FRAC_PI_2, 0.0, 0.0]),
            scale: 2.0,
            object: Some(TagId(7)),
        };
        let ray = |x: f32, y: f32| Ray {
            origin: [x, y, 3.0],
            dir: [0.0, 0.0, -1.0],
        };
        assert_eq!(
            scene.pick(&[marker], &ray(8.5, 1.5)),
            Some((BlockKind::Scenery, 3))
        );
        assert_eq!(scene.pick(&[marker], &ray(11.5, 1.5)), None);
        let p = model_to_world(&marker, [1.0, 1.0, 0.0]);
        assert!(
            (p[0] - 8.0).abs() < 1e-5 && (p[1] - 2.0).abs() < 1e-5,
            "{p:?}"
        );
    }

    fn near(a: [f32; 3], b: [f32; 3]) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-5)
    }

    /// A turret on a hull, all invented: the hull's root node and a mount
    /// node one unit up, turned a quarter about z; the hull's `turret`
    /// marker half a unit along the mount's x. The turret's own `mount`
    /// marker sits a quarter below its origin.
    #[test]
    fn an_attached_object_sits_on_its_parents_marker() {
        let q = std::f32::consts::FRAC_1_SQRT_2;
        let nodes = rest_frames(&[
            RestNode {
                parent: -1,
                translation: [0.0; 3],
                orientation: [0.0, 0.0, 0.0, 1.0],
            },
            RestNode {
                parent: 0,
                translation: [0.0, 0.0, 1.0],
                orientation: [0.0, 0.0, q, q],
            },
        ]);
        // The hull's payload: a 0x60-byte header whose `+0x58` block holds
        // two marker groups, `turret` (two markers: the first counts) and
        // `spare` (none, so no frame).
        let strings: Vec<Option<String>> = ["", "turret", "spare", "mount"]
            .iter()
            .map(|s| Some((*s).to_owned()))
            .collect();
        let sid = |i: usize| ((strings[i].as_ref().unwrap().len() as u32) << 24) | i as u32;
        let address = 0x8000_0000u32;
        let mut b = vec![0u8; 0x60 + 2 * 0xc + 2 * 0x24];
        let put = |b: &mut Vec<u8>, at: usize, v: u32| {
            b[at..at + 4].copy_from_slice(&v.to_le_bytes());
        };
        let putf = |b: &mut Vec<u8>, at: usize, v: &[f32]| {
            for (k, x) in v.iter().enumerate() {
                b[at + 4 * k..at + 4 * k + 4].copy_from_slice(&x.to_le_bytes());
            }
        };
        put(&mut b, 0x58, 2);
        put(&mut b, 0x5c, address + 0x60);
        put(&mut b, 0x60, sid(1));
        put(&mut b, 0x64, 2);
        put(&mut b, 0x68, address + 0x78);
        put(&mut b, 0x6c, sid(2));
        let m0 = 0x78;
        b[m0 + 2] = 1;
        putf(&mut b, m0 + 4, &[0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0]);
        let m1 = m0 + 0x24;
        putf(&mut b, m1 + 4, &[9.0, 9.0, 9.0, 0.0, 0.0, 0.0, 1.0]);
        let frames = markers_in(&b, address, &strings, &nodes);
        assert_eq!(frames.len(), 1, "{:?}", frames.keys());
        let turret = frames["turret"];
        assert!(near(transform_point(&turret, [0.0; 3]), [0.0, 0.5, 1.0]));
        assert!(near(
            transform_vector(&turret, [1.0, 0.0, 0.0]),
            [0.0, 1.0, 0.0]
        ));

        // The turret's anchor lands on the hull's marker, and its axes
        // turn with it.
        let anchor = frame([0.0, 0.0, -0.25], [0.0, 0.0, 0.0, 1.0]);
        let at = attachment_transform(&IDENTITY, &turret, Some(&anchor));
        let mut part = WorldPart {
            positions: vec![[0.0, 0.0, -0.25], [1.0, 0.0, -0.25]],
            normals: vec![[1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            ..WorldPart::default()
        };
        place_part(&mut part, &at);
        assert!(
            near(part.positions[0], [0.0, 0.5, 1.0]),
            "{:?}",
            part.positions
        );
        assert!(
            near(part.positions[1], [0.0, 1.5, 1.0]),
            "{:?}",
            part.positions
        );
        assert!(near(part.normals[0], [0.0, 1.0, 0.0]), "{:?}", part.normals);
        assert!(near(part.normals[1], [0.0, 0.0, 1.0]), "{:?}", part.normals);
        // Without an anchor its origin is on the marker; a placed parent
        // carries it along.
        let lifted = frame([0.0, 0.0, 10.0], [0.0, 0.0, 0.0, 1.0]);
        let at = attachment_transform(&lifted, &turret, None);
        assert!(near(transform_point(&at, [0.0; 3]), [0.0, 0.5, 11.0]));
    }

    #[test]
    fn rest_frames_compose_in_any_order_and_survive_bad_links() {
        let still = [0.0, 0.0, 0.0, 1.0];
        let node = |parent: i16, z: f32| RestNode {
            parent,
            translation: [0.0, 0.0, z],
            orientation: still,
        };
        // The child comes first; a self-parented node is a root; two nodes
        // in a cycle stay at the origin.
        let f = rest_frames(&[
            node(1, 1.0),
            node(-1, 2.0),
            node(2, 5.0),
            node(4, 1.0),
            node(3, 1.0),
        ]);
        let z = |m: &Mat4| transform_point(m, [0.0; 3])[2];
        assert_eq!(
            f.iter().map(z).collect::<Vec<_>>(),
            [3.0, 2.0, 5.0, 0.0, 0.0]
        );
    }

    /// Two invented shaders: `own` holds its compiled postprocess and
    /// bitmap list in its own payload; `shared` has no postprocess of its
    /// own and points at `own`'s, as the compiler shares identical
    /// compiled data. Both read the same template and bitmaps.
    #[test]
    fn a_shader_sharing_anothers_postprocess_reads_its_bitmaps() {
        use map::build::{Block, FixupKind, TagPayload, TagSet};
        let cc = |s: &[u8; 4]| FourCC(*s);
        let mut set = TagSet::new(
            map::CacheType::Multiplayer,
            "invented",
            "invented\\scenarios\\invented",
        );
        for g in [b"stem", b"bitm", b"shad"] {
            set.add_group(cc(g), None, None);
        }
        let template = set
            .add_tag(
                cc(b"stem"),
                "invented\\templates\\opaque\\tex_bump",
                TagPayload::Built(Block::zeroed(0x10)),
            )
            .unwrap();
        let bitmap = |set: &mut TagSet, name: &str| {
            set.add_tag(cc(b"bitm"), name, TagPayload::Built(Block::zeroed(0x10)))
                .unwrap()
        };
        let bump = bitmap(&mut set, "invented\\bitmaps\\wall_bump");
        let diffuse = bitmap(&mut set, "invented\\bitmaps\\wall");
        let own = set
            .add_tag(cc(b"shad"), "invented\\shaders\\own", TagPayload::Empty)
            .unwrap();
        // The root (0x54), its postprocess element at 0x58 (0x7c), and
        // the two bitmap parameters at 0xd4 (0xc each), in one payload.
        let mut b = Block::zeroed(0xec);
        b.tag_ref(0, template)
            .fixup(
                0x20,
                FixupKind::TagOffset {
                    count: 1,
                    tag: own,
                    offset: 0x58,
                },
            )
            .fixup(
                0x58 + 4,
                FixupKind::TagOffset {
                    count: 2,
                    tag: own,
                    offset: 0xd4,
                },
            )
            .tag_id(0xd4, bump)
            .tag_id(0xe0, diffuse);
        set.tag_mut(own).unwrap().payload = TagPayload::Built(b);
        let mut b = Block::zeroed(0x54);
        b.tag_ref(0, template).fixup(
            0x20,
            FixupKind::TagOffset {
                count: 1,
                tag: own,
                offset: 0x58,
            },
        );
        let shared = set
            .add_tag(
                cc(b"shad"),
                "invented\\shaders\\shared",
                TagPayload::Built(b),
            )
            .unwrap();
        let (bytes, _) = set.build().unwrap();
        let mut map = Map::from_reader(std::io::Cursor::new(bytes)).unwrap();
        let want = (
            "invented\\templates\\opaque\\tex_bump".to_owned(),
            vec![bump, diffuse],
        );
        assert_eq!(shader_bitmaps(&mut map, own).unwrap(), want);
        assert_eq!(shader_bitmaps(&mut map, shared).unwrap(), want);
        assert!(shader_bitmaps(&mut map, template).is_err(), "not a shader");
    }

    /// An invented material in the shape of a lightmapped `tex_bump`
    /// pass: stage 0 the base map at 1, stage 1 a detail map at 40, stage
    /// 2 a bump map (a detail-folder name, but a bump) at 10, stage 3 the
    /// lightmap; an earlier implementation does not sample the base.
    #[test]
    fn a_shaders_detail_map_and_scales_reach_its_batch() {
        let (base, detail, bump, other) = (TagId(0x10), TagId(0x11), TagId(0x12), TagId(0x13));
        let stage = |bitmap: Option<TagId>, s: Option<f32>| StageBinding {
            bitmap,
            scale: s.map(|s| [s, s]),
        };
        let imps = [
            [
                stage(Some(other), Some(3.0)),
                stage(None, None),
                stage(None, None),
                stage(None, None),
            ],
            [
                stage(Some(base), Some(1.0)),
                stage(Some(detail), Some(40.0)),
                stage(Some(bump), Some(10.0)),
                stage(None, None),
            ],
        ];
        let names = |id: TagId| match id.0 {
            0x10 => "invented\\bitmaps\\terrain",
            0x11 => "invented\\bitmaps\\detail_sand",
            0x12 => "invented\\bitmaps\\detail_maps\\rock_bump",
            _ => "invented\\bitmaps\\other",
        };
        let plan = detail_plan(&imps, base, |id| is_detail_map(names(id)));
        assert_eq!(
            plan,
            DetailPlan {
                base_scale: [1.0, 1.0],
                details: vec![(detail, [40.0, 40.0])],
            }
        );
        // Scene textures: the base map is 0, the detail map 1.
        let look = look_of(Some(0), &plan, |b| (b == detail).then_some(1));
        assert_eq!(
            look,
            Look {
                texture: Some(0),
                scale: [1.0, 1.0],
                details: [
                    Some(Layer {
                        texture: 1,
                        scale: [40.0, 40.0]
                    }),
                    None
                ],
                emissive: false,
            }
        );
        let mut mesh = Mesh::default();
        mesh.push_look(&look, None, Blend::Opaque, &quad(false));
        mesh.push(Some(0), None, Blend::Opaque, &quad(false));
        assert_eq!(mesh.batches.len(), 2, "the plain base map is another batch");
        let b = &mesh.batches[0];
        assert_eq!((b.texture, b.scale), (Some(0), [1.0, 1.0]));
        assert_eq!(b.details, look.details);
        assert_eq!(mesh.batches[1].details, [None; MAX_DETAILS]);

        // A base map scaled itself, two detail maps (the `two_detail`
        // form), and a stage with no scale written (1).
        let imps = [[
            stage(Some(base), Some(2.0)),
            stage(Some(detail), Some(16.0)),
            stage(Some(other), None),
            stage(None, None),
        ]];
        let plan = detail_plan(&imps, base, |id| id == detail || id == other);
        assert_eq!(plan.base_scale, [2.0, 2.0]);
        assert_eq!(plan.details, [(detail, [16.0, 16.0]), (other, [1.0, 1.0])]);
        // No implementation samples the base map: alone, unscaled.
        assert_eq!(detail_plan(&imps, bump, |_| true), DetailPlan::default());
        // No base texture: no details either.
        assert_eq!(look_of(None, &plan, |_| Some(5)), Look::plain(None));
    }

    #[test]
    fn detail_maps_are_told_by_name() {
        for d in [
            "scenarios\\bitmaps\\multi\\a\\detail_sandstone_a",
            "scenarios\\bitmaps\\detail_maps\\nature\\rock\\sandstone",
        ] {
            assert!(is_detail_map(d), "{d}");
        }
        for n in [
            "scenarios\\bitmaps\\detail_maps\\nature\\rock\\rock_cave1_bump",
            "shaders\\default_bitmaps\\bitmaps\\default_detail",
            "scenarios\\bitmaps\\multi\\a\\terrain",
            "scenarios\\bitmaps\\reflection_maps\\noise",
        ] {
            assert!(!is_detail_map(n), "{n}");
        }
    }
}
