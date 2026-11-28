//! The viewport's world on the native renderer (docs/editor/editor.md, "Lighting"): one BSP
//! loaded with `native_render::load::load_world`, its base maps and
//! lightmap pages decoded by the renderer's own decodes, uploaded with
//! `GpuWorld::new` and drawn with the editor's camera into the viewport's
//! colour and depth targets. The look is base map times lightmap, drawn
//! as a translated pass (`native_render::base_look`; the renderer's
//! `world.wgsl` when that does not compile). The Lighting menu's modes
//! swap the images the world is uploaded with ([`Lighting`]).
//!
//! Step 2 (slice P15) adds the world layers 1, 5, 7, 8 and 20 with their
//! own translated passes ([`NativeLayers`]): loaded as the game loads
//! them, planned with no game, drawn in the Game mode around the base
//! look, and every material left out named with its cause in
//! `view.stats`.
use crate::geometry::{Camera, Marker};
use assets::ResourceFiles;
use map::{Map, TagId};
use native_render::base_look::{self, PassKey, SetKeys};
use native_render::camera::RenderCamera;
use native_render::cull::CullStats;
use native_render::gpu::{DepthRule, GpuWorld, Image, PassViewport};
use native_render::instances::ObjectInput;
use native_render::layers_load::{self, LayersView, StaticLayer};
use native_render::load;
use native_render::look::{HostStage, Stage};
use native_render::model::{Model, NodeMatrix};
use native_render::model_gpu::GpuModels;
use native_render::program::{CAMERA_GIVEN, ViewInputs};
use native_render::stage_images::{StageDecoder, StageImages};
use native_render::world::{LightmapKey, Owner, World, keep_draws};
use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Seek};
use std::sync::Arc;

/// What the viewport's world shows (the Lighting menu).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Lighting {
    /// The native renderer's picture: base map times lightmap.
    #[default]
    Game,
    /// Base maps at full brightness, no lightmap: geometry in dark places.
    Flat,
    /// The lightmap on white: the lighting alone.
    LightmapOnly,
    /// Flat at a third of its strength, for the collision layers on top.
    Collision,
}

impl Lighting {
    pub const ALL: [Self; 4] = [Self::Game, Self::Flat, Self::LightmapOnly, Self::Collision];

    /// The menu's name.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Game => "Game",
            Self::Flat => "Flat",
            Self::LightmapOnly => "Lightmap only",
            Self::Collision => "Collision",
        }
    }

    /// The id the socket API and the saved layout use.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Game => "game",
            Self::Flat => "flat",
            Self::LightmapOnly => "lightmap",
            Self::Collision => "collision",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|l| l.id() == id)
    }

    /// A material's base map as this mode draws it.
    pub fn base(self, image: &Image) -> Image {
        match self {
            Self::Game | Self::Flat => image.clone(),
            Self::LightmapOnly => Image::solid([255; 4]),
            Self::Collision => scaled(image, 1, 3),
        }
    }

    /// A lightmap page as this mode draws it; `None` draws white.
    pub fn page(self, image: &Image) -> Option<Image> {
        match self {
            Self::Game | Self::LightmapOnly => Some(image.clone()),
            Self::Flat | Self::Collision => None,
        }
    }
}

/// `rgba` (`width` by `height`) with `mips` further levels, each a box
/// filter of the one before (rounded to nearest, an odd edge repeating
/// its last row or column); `None` when `rgba` is not that size.
pub fn box_chain(width: u32, height: u32, rgba: &[u8], mips: usize) -> Option<Image> {
    if width == 0 || height == 0 || rgba.len() != width as usize * height as usize * 4 {
        return None;
    }
    let mut levels: Vec<Vec<u8>> = Vec::with_capacity(mips);
    let (mut w, mut h) = (width as usize, height as usize);
    for _ in 0..mips {
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        let prev: &[u8] = levels.last().map_or(rgba, Vec::as_slice);
        let mut next = Vec::with_capacity(nw * nh * 4);
        for y in 0..nh {
            for x in 0..nw {
                let xs = [(x * 2).min(w - 1), (x * 2 + 1).min(w - 1)];
                let ys = [(y * 2).min(h - 1), (y * 2 + 1).min(h - 1)];
                for c in 0..4 {
                    let sum: u32 = ys
                        .iter()
                        .flat_map(|&yy| xs.iter().map(move |&xx| (yy, xx)))
                        .map(|(yy, xx)| u32::from(prev[(yy * w + xx) * 4 + c]))
                        .sum();
                    next.push(((sum + 2) / 4) as u8);
                }
            }
        }
        levels.push(next);
        (w, h) = (nw, nh);
    }
    Some(Image::from_rgba(width, height, rgba.to_vec(), levels))
}

/// `image` with its colour (not alpha) times `num / den`, every level.
/// Its BC blocks are dropped: they no longer match the pixels.
fn scaled(image: &Image, num: u16, den: u16) -> Image {
    let scale = |rgba: &[u8]| -> Vec<u8> {
        rgba.chunks_exact(4)
            .flat_map(|p| {
                let c = |v: u8| (u16::from(v) * num / den) as u8;
                [c(p[0]), c(p[1]), c(p[2]), p[3]]
            })
            .collect()
    };
    Image::from_rgba(
        image.width,
        image.height,
        scale(&image.rgba),
        image.mips.iter().map(|m| scale(m)).collect(),
    )
}

/// The depth rule the editor's world writes: view depth over the far
/// plane floored to D24's step, the W-buffer's, which the translated base
/// look stores (`base_look`, `xps_w_depth`) and the `world.wgsl` pass,
/// the models and the overlays write too ([`depth_of`]), so a surface
/// drawn over the level at the level's depth passes its depth test.
pub const DEPTH_RULE: DepthRule = DepthRule {
    full_scale: native_render::camera::D24_SCALE,
    floor: true,
};

/// The depth [`DEPTH_RULE`] stores for world point `p` seen by `camera`.
pub fn depth_of(camera: &RenderCamera, p: [f32; 3]) -> f32 {
    native_render::camera::w_depth(camera.depth_of(p), camera.far, DEPTH_RULE.full_scale)
}

/// The editor's camera as the renderer's, for a `size` (width, height)
/// target. Its view-projection is the editor's ([`Camera::view_proj`]).
pub fn render_camera(camera: &Camera, [width, height]: [u32; 2]) -> RenderCamera {
    let [forward, _, up] = camera.basis();
    let (near, far) = camera.near_far();
    let side = |v: u32| v.clamp(1, i16::MAX as u32) as i16;
    RenderCamera {
        position: camera.eye(),
        forward,
        up,
        fov: camera.fov_y,
        projection_xy: None,
        viewport: [0, 0, side(height), side(width)],
        near,
        far,
    }
}

/// A map's raw resources for the loader.
struct Files<'a>(&'a mut ResourceFiles);

impl tags::geometry::RawSource for Files<'_> {
    fn read_raw(&mut self, pointer: map::RawPointer, size: u32) -> tags::Result<Vec<u8>> {
        self.0
            .read(pointer.file, pointer.offset, size)
            .map_err(|_| tags::Error::MissingResource(pointer.file))
    }
}

/// What the viewport's world counts ([`NativeLoad::stats`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NativeStats {
    pub draws: usize,
    pub batches: usize,
    pub materials: usize,
    pub lightmap_pages: usize,
    /// Draws left out: no base map decoded, or a lit draw whose page did
    /// not decode.
    pub untextured: usize,
    /// Sections that did not decode.
    pub failed_sections: u32,
}

/// One BSP's world and its decoded images, on the CPU: uploaded again
/// when the Lighting mode changes.
#[derive(Clone, Debug)]
pub struct NativeLoad {
    pub bsp: TagId,
    world: World,
    base: HashMap<u16, Image>,
    /// The bitmap each material's base map came from
    /// ([`NativeLoad::read`]; none for a load built from images), so the
    /// viewport follows a painted bitmap ([`NativeWorld::repaint_base`]).
    base_bitmaps: HashMap<u16, TagId>,
    pages: HashMap<LightmapKey, Image>,
    stats: NativeStats,
    /// Why the lightmap did not read, when it did not.
    pub lightmap_error: Option<String>,
    /// The world layers, when the BSP was read from a map
    /// ([`NativeLoad::read`]).
    layers: Option<Arc<NativeLayers>>,
    /// The base look's translated passes ([`base_passes_of`]), or why
    /// `world.wgsl` draws it instead.
    base_passes: Result<Arc<BasePasses>, String>,
    /// Why the GPU did not bind the base look's passes, when a
    /// [`NativeWorld`] found that it could not (the check above passed).
    base_unbound: Arc<std::sync::OnceLock<String>>,
}

/// The base look's programs by key ([`base_look::base_programs`]).
type BasePasses = HashMap<PassKey, Vec<native_render::fog_gpu::FogProgram>>;

/// `world`'s base look as translated passes, or why it cannot be: a
/// program that does not compile, or a placement with no model rows (the
/// rigid program reads them; `FogSet::new` leaves such a part out). The
/// world holds only draws with a base map, and a lit draw's page falls
/// back to white, so every other part binds.
fn base_passes_of(world: &World) -> Result<Arc<BasePasses>, String> {
    let programs = base_look::base_programs(
        world
            .draws
            .iter()
            .map(|d| (d.material, d.lightmap.is_some())),
    )
    .map_err(|why| format!("the base look does not compile: {why}"))?;
    for d in &world.draws {
        if let Owner::Instance(i) = d.owner
            && world.rows(i).is_none()
        {
            return Err(format!("placement {i} has no model rows"));
        }
    }
    Ok(Arc::new(programs))
}

impl NativeLoad {
    /// Load BSP `index` of the scenario with its base maps and pages.
    pub fn read<R: Read + Seek>(
        map: &mut Map<R>,
        files: &mut ResourceFiles,
        index: usize,
    ) -> Result<Self, String> {
        let loaded = load::load_world(map, &mut Files(files), index).map_err(|e| e.to_string())?;
        let mut base = HashMap::new();
        let mut base_bitmaps = HashMap::new();
        for m in loaded.world.materials() {
            let bitmap = loaded
                .shaders
                .get(usize::from(m))
                .and_then(|&shader| load::base_map(map, shader));
            let image = bitmap.and_then(|bitmap| load::decode_first(map, files, bitmap));
            if let (Some(bitmap), Some(image)) = (bitmap, image) {
                base.insert(m, image);
                base_bitmaps.insert(m, bitmap);
            }
        }
        let bitmap = loaded
            .lightmap_bitmap
            .and_then(|id| tags::DecodedTag::read(map, id).ok())
            .and_then(|tag| match tag.data {
                tags::TagData::Bitmap(b) => Some(b),
                _ => None,
            });
        let mut pages = HashMap::new();
        for k in loaded.world.lightmap_keys() {
            let image = bitmap
                .as_ref()
                .and_then(|b| b.images.get(usize::from(k.page)))
                .zip(load::page_palette(&loaded.palettes, k).ok())
                .and_then(|(image, palette)| {
                    load::decode_lightmap(image, palette, files, false).ok()
                });
            if let Some(image) = image {
                pages.insert(k, image);
            }
        }
        let layers = NativeLayers::read(
            map,
            files,
            &loaded.shaders,
            &loaded.world,
            &pages,
            loaded.lightmap_bitmap.is_none(),
        );
        let mut out = Self::new(loaded.bsp, loaded.world, base, pages);
        out.base_bitmaps = base_bitmaps;
        out.layers = Some(Arc::new(layers));
        out.stats.failed_sections = loaded.failed_sections;
        out.lightmap_error = loaded.lightmap_error;
        Ok(out)
    }

    /// A world and its images: the draws with no base map, or lit with no
    /// page, are left out.
    pub fn new(
        bsp: TagId,
        world: World,
        base: HashMap<u16, Image>,
        pages: HashMap<LightmapKey, Image>,
    ) -> Self {
        let all = world.draws.len();
        let world = keep_draws(world, &|d| {
            base.contains_key(&d.material) && d.lightmap.is_none_or(|k| pages.contains_key(&k))
        });
        let stats = NativeStats {
            draws: world.draws.len(),
            batches: world.batches.len(),
            materials: world.materials().len(),
            lightmap_pages: world.lightmap_keys().len(),
            untextured: all - world.draws.len(),
            failed_sections: 0,
        };
        let base_passes = base_passes_of(&world);
        Self {
            bsp,
            world,
            base,
            base_bitmaps: HashMap::new(),
            pages,
            stats,
            lightmap_error: None,
            layers: None,
            base_passes,
            base_unbound: Default::default(),
        }
    }

    /// Why `world.wgsl` draws this BSP's base look in place of its
    /// translated pass, when it does; the world layers are off then (their
    /// depths would not agree with it).
    pub fn base_fallback(&self) -> Option<&str> {
        match &self.base_passes {
            Err(why) => Some(why),
            Ok(_) => self.base_unbound.get().map(String::as_str),
        }
    }

    /// What `lighting` draws: in the Game mode the parts only layer 3
    /// draws (no base map) count as drawn, not left out.
    pub fn stats(&self, lighting: Lighting) -> NativeStats {
        let mut s = self.stats;
        let own = self.layer3_only(lighting).len();
        s.draws += own;
        s.untextured = s.untextured.saturating_sub(own);
        s
    }

    /// The world layers and their plans, when they were read.
    pub fn layers(&self) -> Option<&NativeLayers> {
        self.layers.as_deref()
    }

    /// The world as drawn: what picking and the stats read.
    pub fn world(&self) -> &World {
        &self.world
    }

    /// The parts `lighting` draws with their layer-3 pass (the Game mode
    /// only) that the drawn world leaves out (no base map, or a lit part
    /// with no page): counted in the stats and picked as drawn. Their
    /// index ranges are in the drawn world's buffers (a filter keeps every
    /// vertex and index).
    fn layer3_only(&self, lighting: Lighting) -> Vec<&native_render::world::WorldDraw> {
        let Some(l) = self.layers().filter(|_| lighting == Lighting::Game) else {
            return Vec::new();
        };
        let drawn: std::collections::HashSet<(Owner, u8)> =
            self.world.draws.iter().map(|d| (d.owner, d.part)).collect();
        base_parts(self, Some(&l.layer3.data))
            .into_iter()
            .filter(|(d, k)| matches!(k, PassKey::Layer3(_)) && !drawn.contains(&(d.owner, d.part)))
            .map(|(d, _)| d)
            .collect()
    }

    /// The triangles `lighting` draws in world space, for picking with the
    /// same ray code as the editor's world ([`crate::geometry::surface`]):
    /// the drawn world's and, in the Game mode, the parts only layer 3
    /// draws. What is picked is what is seen.
    pub fn triangles(&self, lighting: Lighting) -> Vec<crate::geometry::Tri> {
        use crate::geometry::{cross, normalize, sub};
        let w = &self.world;
        let at = |i: u32| w.vertices.get(i as usize).map(|v| v.position);
        w.draws
            .iter()
            .chain(self.layer3_only(lighting))
            .flat_map(|d| {
                let start = d.first_index as usize;
                let end = start + d.index_count as usize;
                w.indices.get(start..end).unwrap_or(&[]).chunks_exact(3)
            })
            .filter_map(|t| {
                let p = [at(t[0])?, at(t[1])?, at(t[2])?];
                let n = normalize(cross(sub(p[1], p[0]), sub(p[2], p[0])));
                Some(crate::geometry::Tri { p, n })
            })
            .collect()
    }
}

/// The layer drawn before the base look: the game draws layer 1 after the
/// view's clear and before layer 3 (docs/graphics/render-loop.md, "Base
/// layers"). Its plan is refused ([`LAYER_1_UNCHECKED`]), so nothing of
/// it is uploaded until it is checked.
pub const LAYERS_BEFORE: [u8; 1] = [1];
/// The layers drawn after it, in the game's order: 5, 8 and 20 with the
/// base layers, then 7 at the start of the middle passes.
pub const LAYERS_AFTER: [u8; 4] = [5, 8, 20, 7];

/// Why the editor draws nothing of layer 1: the game draws it in a
/// stencil mode, and the editor's stencil-free pass has not been checked
/// against the game's picture (the game's own native path leaves layer 1
/// to D3D while the stencil is on).
pub const LAYER_1_UNCHECKED: &str = "layer 1: stencil mode not checked against the game";

/// `plan` as the editor draws `layer`: layer 1's materials that would
/// draw are refused with [`LAYER_1_UNCHECKED`]; the other layers' plans
/// are kept.
pub fn editor_plan(layer: u8, mut plan: StaticLayer) -> StaticLayer {
    if layer == 1 && !plan.drawn.is_empty() {
        *plan
            .refused
            .entry(LAYER_1_UNCHECKED.to_owned())
            .or_default() += plan.drawn.len();
        plan.drawn.clear();
        plan.constants.clear();
        plan.parts.clear();
    }
    plan
}

/// One BSP's world layers 1, 5, 7, 8 and 20 for the viewport (slice
/// P15), on the CPU. Loaded as the game loads them
/// (`layers_load::load_with`) from the world's draws before the base-map
/// filter, with the view constants the editor's camera gives
/// (`program::CAMERA_GIVEN`: a pass reading another is refused, named
/// with the register) and no stage palettes (a palettized stage bitmap is
/// refused, named). Each layer is planned with no game
/// (`layers_load::plan_without_game` at the requested render time), and layer 1 is refused
/// whole ([`editor_plan`]). Layer 3 is loaded with its own translated
/// passes too (`layers_load::load_layer3`, planned by
/// `layers_load::plan_layer3`), drawn in the Game mode in the base look's
/// set ([`base_parts`]).
pub struct NativeLayers {
    view: LayersView,
    /// The world's draws and placements before the base-map filter: a
    /// layer draws a part whatever its base map, from the same buffers.
    world: World,
    images: StageImages,
    /// Layer 3 with its own translated passes
    /// (`layers_load::load_layer3`), which the Game mode draws in place of
    /// the base look; its plan is in `plans` under 3.
    layer3: layers_load::Layer3,
    plans: BTreeMap<u8, StaticLayer>,
}

impl NativeLayers {
    pub fn read<R: Read + Seek>(
        map: &mut Map<R>,
        files: &mut ResourceFiles,
        shaders: &[TagId],
        world: &World,
        pages: &HashMap<LightmapKey, Image>,
        unlit_bsp: bool,
    ) -> Self {
        Self::read_at(map, files, shaders, world, pages, unlit_bsp, 0.0)
    }

    /// [`Self::read`] with each layer planned at render time `time`
    /// seconds (the Shader tab's preview clock): the plan evaluates a
    /// pass's animated properties there.
    pub fn read_at<R: Read + Seek>(
        map: &mut Map<R>,
        files: &mut ResourceFiles,
        shaders: &[TagId],
        world: &World,
        pages: &HashMap<LightmapKey, Image>,
        unlit_bsp: bool,
        time: f64,
    ) -> Self {
        let world = World {
            draws: world.draws.clone(),
            batches: world.batches.clone(),
            transparents: world.transparents.clone(),
            placements: world.placements.clone(),
            ..Default::default()
        };
        let mut images = StageImages::new();
        let mut decoder = StageDecoder::new(Default::default());
        let view = layers_load::load_with(
            map,
            files,
            shaders,
            &world,
            pages,
            &mut images,
            &mut decoder,
            Some(&CAMERA_GIVEN),
        );
        let layer3 = layers_load::load_layer3(
            map,
            files,
            shaders,
            &world,
            pages,
            &mut images,
            &mut decoder,
            unlit_bsp,
            Some(&CAMERA_GIVEN),
        );
        let mut plans: BTreeMap<u8, StaticLayer> = native_render::layer_pass::LAYERS
            .into_iter()
            .map(|layer| {
                let plan = layers_load::plan_without_game(&view, shaders, layer, time);
                (layer, editor_plan(layer, plan))
            })
            .collect();
        plans.insert(layers_load::LAYER_3, layers_load::plan_layer3(&layer3, time));
        Self {
            view,
            world,
            images,
            layer3,
            plans,
        }
    }

    /// Each layer's plan: what compiled, what draws and why the rest
    /// does not.
    pub fn plans(&self) -> &BTreeMap<u8, StaticLayer> {
        &self.plans
    }

    /// The world materials that did not resolve, by cause, with how
    /// many: they have no layer passes.
    pub fn unresolved(&self) -> &BTreeMap<String, usize> {
        &self.view.unresolved
    }
}

impl std::fmt::Debug for NativeLayers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeLayers")
            .field("unresolved", &self.view.unresolved)
            .field("plans", &self.plans)
            .finish_non_exhaustive()
    }
}

/// What [`NativeWorld::render`] clears before it draws: the colour to a
/// colour, the depth to the far plane. The first world of a frame clears
/// (the colour only when no sky is behind it); later ones load.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Clear {
    /// In the target view's encoding: the viewport's native passes write
    /// its plain view (`render::Shading::Gamma`), so an sRGB-encoded
    /// colour.
    pub color: Option<[f64; 3]>,
    pub depth: bool,
}

impl Clear {
    pub const NONE: Self = Self {
        color: None,
        depth: false,
    };
    pub const fn all(color: [f64; 3]) -> Self {
        Self {
            color: Some(color),
            depth: true,
        }
    }
}

/// Every BSP of the scenario ([`NativeLoad::read`]), with why each one
/// that did not load did not, by BSP index.
pub fn read_all<R: Read + Seek>(
    map: &mut Map<R>,
    files: &mut ResourceFiles,
) -> (Vec<NativeLoad>, Vec<(usize, String)>) {
    let count = load::scenario_bsps(map).map_or(0, |b| b.len());
    let (mut loads, mut errors) = (Vec::new(), Vec::new());
    for index in 0..count {
        match NativeLoad::read(map, files, index) {
            Ok(l) => loads.push(l),
            Err(e) => errors.push((index, e)),
        }
    }
    (loads, errors)
}

/// A [`NativeLoad`] on the GPU in one Lighting mode.
pub struct NativeWorld {
    /// The BSP it draws.
    bsp: TagId,
    gpu: GpuWorld,
    lighting: Lighting,
    pool: gfx::ring::UploadPool,
    /// The base look as a translated pass ([`base_look`]): what it draws.
    /// `None` when it could not be ([`NativeLoad::base_fallback`]):
    /// `world.wgsl` draws the base look then, and no layer is drawn over it
    /// (their depths would not agree).
    base: Option<DrawnLayer>,
    /// The uploaded layers (the Game mode only): what each draws.
    layers: Vec<DrawnLayer>,
    /// The materials whose base map came from each bitmap, with the level
    /// count of the image uploaded for each ([`NativeWorld::repaint_base`]).
    base_of: HashMap<TagId, Vec<(u16, usize)>>,
}

/// The key the base look's set is held under in the `GpuWorld` (the
/// game's layer 3, which the base look stands in for).
const BASE_LAYER: u8 = 3;

/// The parts `load`'s base-look set draws, in the world's draw order,
/// each with what it draws: with `layer3` (the Game mode), a part its
/// material's layer-3 pass takes draws that pass, from the world before
/// the base-map filter; every other part of the drawn world draws its
/// material's base look, lit or not.
pub fn base_parts<'a>(
    load: &'a NativeLoad,
    layer3: Option<&layers_load::LayerData>,
) -> Vec<(&'a native_render::world::WorldDraw, PassKey)> {
    let base_key = |d: &native_render::world::WorldDraw| PassKey::Base {
        material: d.material,
        lit: d.lightmap.is_some(),
    };
    let (Some(layer3), Some(layers)) = (layer3, load.layers()) else {
        return load.world.draws.iter().map(|d| (d, base_key(d))).collect();
    };
    let own: std::collections::HashSet<(Owner, u8)> =
        layer3.parts.iter().map(|&(k, _)| k).collect();
    let drawn: std::collections::HashSet<(Owner, u8)> =
        load.world.draws.iter().map(|d| (d.owner, d.part)).collect();
    layers
        .world
        .draws
        .iter()
        .filter_map(|d| {
            let k = (d.owner, d.part);
            if own.contains(&k) {
                Some((d, PassKey::Layer3(d.material)))
            } else {
                drawn.contains(&k).then(|| (d, base_key(d)))
            }
        })
        .collect()
}

/// `load`'s base look in `lighting` as translated passes
/// ([`NativeLoad::base_passes`], [`base_parts`]): each draw's base map
/// (the mode's), times its lightmap page (the mode's, white where the
/// mode has none) or white when unlit; in the Game mode a part its
/// material's own layer-3 pass takes draws that pass instead. The cause
/// when the passes did not compile or a part does not bind.
fn base_set(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    load: &NativeLoad,
    lighting: Lighting,
) -> Result<(native_render::fog_gpu::FogSet, DrawnLayer), String> {
    let base = load.base_passes.as_ref().map_err(Clone::clone)?;
    let w = &load.world;
    let layer3 = load
        .layers()
        .filter(|_| lighting == Lighting::Game)
        .map(|l| &l.layer3);
    // Each part's key, through the set's slots (`FogSet` keys by `u16`).
    let mut keys = SetKeys::default();
    let mut programs = HashMap::new();
    let mut parts: Vec<layers_load::Part> = Vec::new();
    for (d, key) in base_parts(load, layer3.map(|l| &l.data)) {
        let slot = keys.slot(key)?;
        if let std::collections::hash_map::Entry::Vacant(entry) = programs.entry(slot) {
            let passes = match key {
                PassKey::Base { .. } => base.get(&key),
                PassKey::Layer3(m) => layer3.and_then(|l| l.data.programs.get(&m)),
            };
            entry.insert(passes.ok_or("a part has no pass")?.clone());
        }
        parts.push((
            (d.owner, d.part),
            (d.first_index, d.index_count, d.lightmap, slot),
        ));
    }
    let stage_images = load.layers().map(|l| &l.images);
    let white = Image::solid([255; 4]);
    let set = native_render::fog_gpu::FogSet::new(
        device,
        queue,
        w,
        parts.clone(),
        programs,
        &mut |s| match s {
            Stage::Host(HostStage::White) => Some(white.clone()),
            Stage::Host(HostStage::BaseMap(m)) => load.base.get(&m).map(|i| lighting.base(i)),
            // A layer-3 pass's own stage image.
            Stage::Bitmap { bitmap, image } => stage_images?.get(&(bitmap, image)).cloned(),
            Stage::Empty | Stage::Lightmap | Stage::Engine(_) => None,
        },
        &mut |k| {
            Some(
                load.pages
                    .get(&k)
                    .and_then(|i| lighting.page(i))
                    .unwrap_or_else(|| white.clone()),
            )
        },
    );
    let selected: Vec<(Owner, u8)> = parts
        .iter()
        .map(|&(k, _)| k)
        .filter(|&(owner, part)| set.draws_part(owner, part))
        .collect();
    if selected.len() != parts.len() {
        return Err(format!(
            "the base look binds {} of {} parts",
            selected.len(),
            parts.len()
        ));
    }
    let fogs = parts
        .iter()
        .map(|&(_, (_, _, _, m))| (m, (shaders::psh::fog::Fog::OFF, [0.0; 4])))
        .collect();
    Ok((
        set,
        DrawnLayer {
            layer: BASE_LAYER,
            constants: Default::default(),
            selected,
            fogs,
        },
    ))
}

/// One uploaded layer: the parts its set draws and the fog they draw
/// with.
struct DrawnLayer {
    layer: u8,
    constants: native_render::fog_gpu::LayerSamples,
    selected: Vec<(Owner, u8)>,
    fogs: layers_load::Fogs,
}

impl NativeWorld {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        load: &NativeLoad,
        lighting: Lighting,
    ) -> Self {
        let mut gpu = GpuWorld::new(
            device,
            queue,
            load.world.clone(),
            &mut |m| load.base.get(&m).map(|i| lighting.base(i)),
            &mut |k| load.pages.get(&k).and_then(|i| lighting.page(i)),
        );
        let base = match base_set(device, queue, load, lighting) {
            Ok((set, drawn)) => {
                gpu.set_layer(BASE_LAYER, set);
                Some(drawn)
            }
            Err(why) => {
                // Named in `view.stats` ([`NativeLoad::base_fallback`]).
                if load.base_passes.is_ok() {
                    let _ = load.base_unbound.set(why);
                }
                None
            }
        };
        // The layers are the game's look: the other modes draw the base
        // look alone. They draw only over the translated base look, whose
        // depth is theirs.
        let mut layers = Vec::new();
        if let Some(l) = load
            .layers()
            .filter(|_| lighting == Lighting::Game && base.is_some())
        {
            // Layer 3 draws in the base look's set ([`base_set`]).
            for (&layer, plan) in l.plans.iter().filter(|(k, _)| **k != BASE_LAYER) {
                let Some(set) = layers_load::upload_static(
                    &l.view,
                    layer,
                    plan,
                    &l.images,
                    device,
                    queue,
                    &l.world,
                    &mut |k| load.pages.get(&k).cloned(),
                ) else {
                    continue;
                };
                gpu.set_layer(layer, set.set);
                layers.push(DrawnLayer {
                    layer,
                    constants: plan.constants.clone(),
                    selected: set.selected,
                    fogs: set.fogs,
                });
            }
        }
        let mut base_of: HashMap<TagId, Vec<(u16, usize)>> = HashMap::new();
        for (&m, &bitmap) in &load.base_bitmaps {
            if let Some(image) = load.base.get(&m) {
                base_of.entry(bitmap).or_default().push((m, image.mips.len()));
            }
        }
        base_of.values_mut().for_each(|v| v.sort_unstable());
        Self {
            bsp: load.bsp,
            gpu,
            lighting,
            pool: Default::default(),
            base,
            layers,
            base_of,
        }
    }

    /// Whether some material's base map came from `bitmap`.
    pub fn draws_bitmap(&self, bitmap: TagId) -> bool {
        self.base_of.contains_key(&bitmap)
    }

    /// Redraw `bitmap`'s image 0 (`width` by `height`, `rgba` rows) as the
    /// base map of every material that took it, through the same Lighting
    /// mode's transform the upload used and with a box-filtered chain of
    /// the level count the upload had: the viewport follows a painted
    /// bitmap (docs/editor/realtime-edits.md, R15). Returns how many
    /// textures were written; an image of another size is refused by the
    /// GPU side and writes none. The mode that draws white keeps it.
    pub fn repaint_base(
        &self,
        queue: &wgpu::Queue,
        bitmap: TagId,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> usize {
        if self.lighting == Lighting::LightmapOnly {
            return 0;
        }
        let mut written = 0;
        for &(material, mips) in self.base_of.get(&bitmap).into_iter().flatten() {
            let Some(image) = box_chain(width, height, rgba, mips) else { continue };
            let image = self.lighting.base(&image);
            written += self.gpu.rewrite_base_map(queue, material, &image).unwrap_or(0);
        }
        written
    }

    /// The BSP this world draws.
    pub fn bsp(&self) -> TagId {
        self.bsp
    }

    /// Move one section's vertices (the Modeling tool's live preview):
    /// see `GpuWorld::set_section_vertices`.
    pub fn set_section_vertices(
        &mut self,
        queue: &wgpu::Queue,
        owner: Owner,
        positions: &[[f32; 3]],
        normals: Option<&[[f32; 3]]>,
    ) -> bool {
        self.gpu.set_section_vertices(queue, owner, positions, normals)
    }

    /// Whether this world drew a section for `owner`.
    pub fn has_section(&self, owner: Owner) -> bool {
        self.gpu.world().section(owner).is_some()
    }

    /// Records `layers`' draws that this world uploaded, in that order.
    fn encode_layers(
        &mut self,
        layers: &[u8],
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        color_format: wgpu::TextureFormat,
        depth: &wgpu::TextureView,
        depth_format: wgpu::TextureFormat,
        size: [u32; 2],
    ) {
        for &layer in layers {
            let Some(l) = self.layers.iter().find(|l| l.layer == layer) else {
                continue;
            };
            self.gpu.encode_layer(
                layer,
                device,
                encoder,
                color,
                color_format,
                depth,
                depth_format,
                (size[0], size[1]),
                &l.selected,
            );
        }
    }

    pub fn lighting(&self) -> Lighting {
        self.lighting
    }

    /// Clear what `clear` names (the colour to its colour, the depth to the
    /// far plane), then draw the world seen by `camera` into the whole
    /// `size` target: layer 1, the base look, then layers 5, 8, 20 and 7
    /// ([`LAYERS_BEFORE`], [`LAYERS_AFTER`]) where they were uploaded,
    /// with the camera's view constants (`ViewInputs::from_camera`). The
    /// world's cull runs on the GPU ([`NativeWorld::cull_stats`]).
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        color: &wgpu::TextureView,
        color_format: wgpu::TextureFormat,
        depth: &wgpu::TextureView,
        depth_format: wgpu::TextureFormat,
        size: [u32; 2],
        camera: &Camera,
        clear: Clear,
    ) {
        let view = render_camera(camera, size);
        let viewport = PassViewport {
            x: 0.0,
            y: 0.0,
            width: size[0] as f32,
            height: size[1] as f32,
        };
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("editor native world"),
        });
        clear_pass(&mut encoder, color, depth, clear);
        let arena = self.pool.begin();
        {
            let mut uploads = gfx::ring::EncoderUploads {
                device,
                queue,
                encoder: &mut encoder,
                arena: &arena,
            };
            let inputs = ViewInputs::from_camera(&view, (size[0], size[1]));
            for l in self.base.iter().chain(&self.layers) {
                self.gpu
                    .prepare_layer(l.layer, &mut uploads, &inputs, &l.fogs, &l.constants);
            }
            self.gpu.prepare(&mut uploads, &view, viewport, DEPTH_RULE);
        }
        self.encode_layers(
            &LAYERS_BEFORE,
            device,
            &mut encoder,
            color,
            color_format,
            depth,
            depth_format,
            size,
        );
        if let Some(b) = &self.base {
            self.gpu.encode_layer(
                BASE_LAYER,
                device,
                &mut encoder,
                color,
                color_format,
                depth,
                depth_format,
                (size[0], size[1]),
                &b.selected,
            );
        } else {
            self.gpu.encode(
                device,
                &mut encoder,
                color,
                color_format,
                depth,
                depth_format,
                viewport,
                (size[0], size[1]),
            );
        }
        self.encode_layers(
            &LAYERS_AFTER,
            device,
            &mut encoder,
            color,
            color_format,
            depth,
            depth_format,
            size,
        );
        let patches = arena.upload_commands(queue);
        queue.submit(patches.into_iter().chain([encoder.finish()]));
        queue.on_submitted_work_done(move || drop(arena));
    }

    /// What the last [`NativeWorld::render`]'s cull kept, read back from
    /// the GPU (blocking).
    pub fn cull_stats(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> CullStats {
        self.gpu.read_cull_stats(device, queue)
    }
}

/// Clear what `clear` names of the target, in a pass of its own.
fn clear_pass(
    encoder: &mut wgpu::CommandEncoder,
    color: &wgpu::TextureView,
    depth: &wgpu::TextureView,
    clear: Clear,
) {
    if clear.color.is_some() || clear.depth {
        drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("editor native clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: match clear.color {
                        Some([r, g, b]) => wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a: 1.0 }),
                        None => wgpu::LoadOp::Load,
                    },
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: if clear.depth {
                        wgpu::LoadOp::Clear(1.0)
                    } else {
                        wgpu::LoadOp::Load
                    },
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        }));
    }
}

/// A selected placement's model: mixed 0.45 of the way to the scene's
/// selection orange (`render.rs`, `Instance::at` and the scene shader's
/// `mix(.., vec3(1.0, 0.78, 0.25), tint.a)`), as RGBA8.
pub const SELECTED_HIGHLIGHT: [u8; 4] = [255, 199, 64, 115];

/// The inverse of a rigid node matrix with a uniform scale: `invert(m)
/// ∘ m` is the identity.
pub fn invert(m: &NodeMatrix) -> NodeMatrix {
    let s = if m.scale != 0.0 { 1.0 / m.scale } else { 0.0 };
    // The rows of the transposed rotation are the axes' components.
    let col = |k: usize| [m.forward[k], m.left[k], m.up[k]];
    let r = NodeMatrix {
        scale: s,
        forward: col(0),
        left: col(1),
        up: col(2),
        position: [0.0; 3],
    };
    let p = r.point(m.position);
    NodeMatrix {
        position: [-p[0], -p[1], -p[2]],
        ..r
    }
}

/// A placement's transform as a node matrix: its axes, scale and
/// position ([`crate::scene::model_to_world`]).
pub fn placement_matrix(m: &Marker) -> NodeMatrix {
    NodeMatrix {
        scale: m.scale,
        forward: m.axes[0],
        left: m.axes[1],
        up: m.axes[2],
        position: m.position,
    }
}

/// A model's nodes in their rest pose under `placement`: each node's
/// world matrix is the placement after the node's rest frame (the inverse
/// of its inverse rest frame), so every vertex lands where the placement
/// puts its model-space position.
pub fn rest_nodes(model: &Model, placement: &NodeMatrix) -> Vec<NodeMatrix> {
    let rest = |inv: &NodeMatrix| placement.compose(&invert(inv));
    if model.inverse_binds.is_empty() {
        vec![*placement]
    } else {
        model.inverse_binds.iter().map(rest).collect()
    }
}

/// One node of a render model's skeleton, as a pose needs it: the parent
/// link and the node's own default channels (`mode` node `+4` parent,
/// `+0xc` translation, `+0x18` orientation, `+0x28` scale).
/// [`super::native_view::preview::Piece`] carries one per node, in model
/// node order, which is the animation graph's node order too (the engine
/// writes an animation's channels by node index into the object's nodes).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkeletonNode {
    pub parent: i16,
    pub translation: [f32; 3],
    /// Unit quaternion `[x, y, z, w]`, the node's default orientation.
    pub rotation: [f32; 4],
    /// The channel's default scale: the `mode` node stores none (the
    /// words at `+0x28` are its inverse rest frame), so 1.
    pub scale: f32,
}

/// A column-major `T·R·S` matrix, as `tags::anim_sample` composes one, as
/// a node matrix: the rotation's columns as the axes, the uniform scale
/// their length, the translation the position.
fn mat4_node(m: &tags::anim_sample::Mat4) -> NodeMatrix {
    let length =
        |c: usize| (m[c][0] * m[c][0] + m[c][1] * m[c][1] + m[c][2] * m[c][2]).sqrt();
    let scale = length(0).max(length(1)).max(length(2));
    let axis = |c: usize| {
        let s = if scale > 0.0 { 1.0 / scale } else { 0.0 };
        [m[c][0] * s, m[c][1] * s, m[c][2] * s]
    };
    NodeMatrix {
        scale,
        forward: axis(0),
        left: axis(1),
        up: axis(2),
        position: [m[3][0], m[3][1], m[3][2]],
    }
}

/// A model's nodes under a sampled pose, in place of [`rest_nodes`]: the
/// animation's channels over the node defaults, composed up the render
/// model's node tree, then placed. `pose` is a clip's sample at a frame
/// ([`tags::anim_sample::AnimationClip::sample`]), merged with
/// `skeleton`'s defaults first (a channel the animation supplies none for
/// is the node's own); an overlay can have been applied to it already
/// with [`tags::anim_sample::Pose::add_overlay`]. This is the engine's
/// own composition (docs/engine/anim-sampling.md; the Halo 1 converter
/// writes rotations in this convention and its disc round-trip
/// `h1-assets/src/models/skin_tests.rs` checks it), settled for the pose
/// viewer by slice M1 of docs/editor/rich-editors.md.
pub fn posed_nodes(
    skeleton: &[SkeletonNode],
    pose: &tags::anim_sample::Pose,
    placement: &NodeMatrix,
) -> Result<Vec<NodeMatrix>, String> {
    let defaults: Vec<tags::anim_sample::NodeDefault> = skeleton
        .iter()
        .map(|n| tags::anim_sample::NodeDefault {
            parent: n.parent,
            rotation: n.rotation,
            translation: n.translation,
            scale: n.scale,
        })
        .collect();
    let merged = pose.merged(&defaults).map_err(|e| e.to_string())?;
    let parents: Vec<i16> = skeleton.iter().map(|n| n.parent).collect();
    let worlds = merged.world_matrices(&parents).map_err(|e| e.to_string())?;
    Ok(worlds
        .iter()
        .map(|w| placement.compose(&mat4_node(w)))
        .collect())
}

/// A model's bounding sphere under `placement`: its box's centre and
/// half diagonal, placed and scaled.
pub fn placed_sphere(model: &Model, placement: &NodeMatrix) -> ([f32; 3], f32) {
    let Some(b) = model.bounds else {
        return (placement.position, 0.0);
    };
    let c: [f32; 3] = std::array::from_fn(|k| (b.min[k] + b.max[k]) * 0.5);
    let h: f32 = (0..3)
        .map(|k| (b.max[k] - b.min[k]).powi(2))
        .sum::<f32>()
        .sqrt()
        * 0.5;
    (placement.point(c), h * placement.scale.abs())
}

/// A placed object's model for the native renderer, with the base map of
/// each material it draws.
#[derive(Clone, Debug)]
pub struct NativeModel {
    /// The object tag the model is drawn for (a collection's first entry).
    pub object: TagId,
    pub model: Model,
    base: HashMap<u16, Image>,
}

/// The models of the object tags the open map places, read once each
/// when the map opens.
#[derive(Clone, Debug, Default)]
pub struct NativeModels {
    /// Shared, so a copy (a tag read after the map opened, while the
    /// viewport holds the old set) copies no model.
    models: HashMap<TagId, std::sync::Arc<NativeModel>>,
    /// Each tag that has no model to draw, and why.
    pub errors: Vec<(TagId, String)>,
}

impl NativeModels {
    /// Read the model of every tag in `objects` (each once).
    pub fn read<R: Read + Seek>(
        map: &mut Map<R>,
        files: &mut ResourceFiles,
        objects: impl IntoIterator<Item = TagId>,
    ) -> Self {
        let mut out = Self::default();
        out.add(map, files, objects);
        out
    }

    /// Read the model of each tag in `objects` not read before (modelled
    /// or refused): a tag placed after the map opened. Nothing is read
    /// once the map's tag metadata has failed (its error is under
    /// `TagId::NULL`).
    pub fn add<R: Read + Seek>(
        &mut self,
        map: &mut Map<R>,
        files: &mut ResourceFiles,
        objects: impl IntoIterator<Item = TagId>,
    ) {
        let mut objects: Vec<TagId> = objects
            .into_iter()
            .filter(|&o| o != TagId::NULL && !self.has_read(o))
            .collect();
        if objects.is_empty() || self.has_read(TagId::NULL) {
            return;
        }
        objects.sort_by_key(|o| o.0);
        objects.dedup();
        let ctx = match tags::object::Context::read(map) {
            Ok(c) => c,
            Err(e) => {
                self.errors.push((TagId::NULL, e.to_string()));
                return;
            }
        };
        for object in objects {
            match read_model(map, &ctx, files, object) {
                Ok(m) => {
                    self.models.insert(object, std::sync::Arc::new(m));
                }
                Err(e) => self.errors.push((object, e)),
            }
        }
    }

    /// Whether `object`'s model was read (drawn or refused).
    pub fn has_read(&self, object: TagId) -> bool {
        self.models.contains_key(&object) || self.errors.iter().any(|(t, _)| *t == object)
    }

    /// Models made elsewhere (tests): `(object tag, model, base maps)`.
    pub fn from_models(
        models: impl IntoIterator<Item = (TagId, Model, HashMap<u16, Image>)>,
    ) -> Self {
        Self {
            models: models
                .into_iter()
                .map(|(object, model, base)| {
                    (
                        object,
                        std::sync::Arc::new(NativeModel {
                            object,
                            model,
                            base,
                        }),
                    )
                })
                .collect(),
            errors: Vec::new(),
        }
    }

    /// The model drawn for object tag `object`.
    pub fn of(&self, object: TagId) -> Option<&NativeModel> {
        self.models.get(&object).map(|m| m.as_ref())
    }

    pub fn len(&self) -> usize {
        self.models.len()
    }

    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }
}

/// One object tag's model: its collection resolved to the first entry,
/// its render model loaded by `native_render::model_load`, and the draws
/// whose base map decodes.
fn read_model<R: Read + Seek>(
    map: &mut Map<R>,
    ctx: &tags::object::Context,
    files: &mut ResourceFiles,
    object: TagId,
) -> Result<NativeModel, String> {
    let (definition, _) = crate::object_collision::resolve(ctx, object)?;
    let loaded =
        native_render::model_load::load_object_model(map, ctx, &mut Files(files), definition)
            .map_err(|e| e.to_string())?;
    let mut model = loaded.model;
    let mut maps = HashMap::new();
    for m in model.materials() {
        let Some(&shader) = loaded.shaders.get(usize::from(m)) else {
            continue;
        };
        let Some(bitmap) = load::base_map(map, shader) else {
            continue;
        };
        if let Some(image) = load::decode_first(map, files, bitmap) {
            maps.insert(m, image);
        }
    }
    model.draws.retain(|d| maps.contains_key(&d.material));
    if model.draws.is_empty() {
        return Err("no draw of the model has a base map".into());
    }
    Ok(NativeModel {
        object,
        model,
        base: maps,
    })
}

/// The placed objects on the GPU: every model of a [`NativeModels`] in
/// one Lighting mode, and the placements to draw, each in its rest pose.
pub struct NativeObjects {
    gpu: GpuModels,
    /// GPU model index per object tag.
    uploaded: HashMap<TagId, u32>,
    /// (GPU model, node matrices, world sphere, highlight) per placement.
    placed: Vec<(u32, Vec<NodeMatrix>, ([f32; 3], f32), [u8; 4])>,
    pool: gfx::ring::UploadPool,
}

impl NativeObjects {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        models: &NativeModels,
        lighting: Lighting,
    ) -> Self {
        let mut gpu = GpuModels::new(device, queue);
        let mut uploaded = HashMap::new();
        let mut objects: Vec<_> = models.models.values().collect();
        objects.sort_by_key(|m| m.object.0);
        for m in objects {
            let index = gpu.add_model(device, queue, m.model.clone(), &mut |k| {
                m.base.get(&k).map(|i| lighting.base(i))
            });
            uploaded.insert(m.object, index);
        }
        Self {
            gpu,
            uploaded,
            placed: Vec::new(),
            pool: Default::default(),
        }
    }

    /// The placements to draw: each one whose object tag has a model,
    /// the `selected` ones highlighted as the scene's models are.
    pub fn place<'a>(
        &mut self,
        models: &NativeModels,
        markers: impl IntoIterator<Item = &'a Marker>,
        selected: &[(crate::BlockKind, usize)],
    ) {
        self.placed = markers
            .into_iter()
            .filter_map(|m| {
                let object = m.object?;
                let index = *self.uploaded.get(&object)?;
                let model = &models.of(object)?.model;
                let placement = placement_matrix(m);
                let highlight = if selected.contains(&(m.block, m.index)) {
                    SELECTED_HIGHLIGHT
                } else {
                    [0; 4]
                };
                Some((
                    index,
                    rest_nodes(model, &placement),
                    placed_sphere(model, &placement),
                    highlight,
                ))
            })
            .collect();
    }

    /// The placements to draw with their node matrices given (a posed
    /// model, as [`posed_nodes`] builds them), in place of the rest pose
    /// [`Self::place`] computes: one per entry whose object tag has been
    /// uploaded, with the sphere the posed vertices lie in.
    pub fn place_posed<'a>(
        &mut self,
        entries: impl IntoIterator<Item = (&'a TagId, &'a [NodeMatrix], ([f32; 3], f32))>,
    ) {
        self.placed = entries
            .into_iter()
            .filter_map(|(object, nodes, sphere)| {
                let index = *self.uploaded.get(object)?;
                Some((index, nodes.to_vec(), sphere, [0; 4]))
            })
            .collect();
    }

    /// How many placements are drawn.
    pub fn placed(&self) -> usize {
        self.placed.len()
    }

    /// How many of them are highlighted as selected.
    pub fn highlighted(&self) -> usize {
        self.placed.iter().filter(|p| p.3[3] > 0).count()
    }

    /// Clear what `clear` names, then draw the placements over what
    /// `color` and `depth` hold, seen by `camera`, with the world pass's
    /// depth rule. Returns how many (object, part) draws were made.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        color: &wgpu::TextureView,
        color_format: wgpu::TextureFormat,
        depth: &wgpu::TextureView,
        depth_format: wgpu::TextureFormat,
        size: [u32; 2],
        camera: &Camera,
        clear: Clear,
    ) -> usize {
        let view = render_camera(camera, size);
        let objects: Vec<ObjectInput<'_>> = self
            .placed
            .iter()
            .map(|(model, nodes, sphere, highlight)| ObjectInput {
                model: *model,
                nodes,
                sphere: *sphere,
                highlight: *highlight,
            })
            .collect();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("editor native objects"),
        });
        clear_pass(&mut encoder, color, depth, clear);
        let arena = self.pool.begin();
        {
            let mut uploads = gfx::ring::EncoderUploads {
                device,
                queue,
                encoder: &mut encoder,
                arena: &arena,
            };
            self.gpu
                .prepare(device, &mut uploads, &view, DEPTH_RULE, &objects);
        }
        let drawn = self.gpu.encode(
            device,
            &mut encoder,
            color,
            color_format,
            depth,
            depth_format,
            PassViewport {
                x: 0.0,
                y: 0.0,
                width: size[0] as f32,
                height: size[1] as f32,
            },
        );
        let patches = arena.upload_commands(queue);
        queue.submit(patches.into_iter().chain([encoder.finish()]));
        queue.on_submitted_work_done(move || drop(arena));
        drawn
    }
}

pub mod animate;
pub mod code;
pub mod preview;

#[cfg(test)]
mod tests;
