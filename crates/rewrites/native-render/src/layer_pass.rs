//! A layer draw planned from the scene alone (docs/graphics/framescene.md,
//! slice 4): for each item of a [`frame_scene::LayerPass`] of layers 1, 5,
//! 7, 8 and 20 (the scene's and the first person's), of layer 2
//! ([`OVERLAY_LAYER`], the scene's and the first person's) and of the
//! scene's layer 17 ([`BLOOM_LAYER`]), the material passes
//! the layer draws for it, the implementation of each, the engine inputs
//! the item's draw evaluates and the values of the material's animated
//! properties, from the pass's [`LayerInputs`], the item's owner and
//! lighting and the resolved material. Nothing here reads guest memory
//! or a resolved D3D draw: it is what a stage drawing the layer without
//! the guest's draws under the layer call takes per item.
//!
//! Research points (Code, `h2 disasm`):
//!
//! - `0x00016610` walks the template's pass list for the layer and draws
//!   each implementation with a vertex shader (`+0x100 != -1`); a pass
//!   whose implementation has none draws nothing. The implementation of
//!   a pass with more than one is the item's (`ItemKey::implementation`,
//!   the build's choice, S for layers other than 3).
//! - Before each draw `0x0001ae70` evaluates the implementation's input
//!   masks: the view's inputs ([`LayerInputs`]), the lighting's
//!   ([`crate::object_lighting`]; a structure item draws with no lighting,
//!   `[0x00485a58]` 0), and the shader's own (integers 4 and 5, scalar 21,
//!   copied by `0x0001bbf0`). The material is resolved again with them
//!   ([`Materials::material`]): the resolver bakes engine-driven constants
//!   from its [`EngineInputs`], so the planned implementation's registers
//!   are the ones the draw uploads.
//! - `0x00017550` evaluates an animated property through `0x0001b230`: a
//!   game-time one (`+0` zero) at the render time over its period (`+8`,
//!   an `f32`), wrapped into `[0, 1)` (`fmod` by 1.0, the double at
//!   `0x0045dcc0`) unless its function is periodic (type 3); the range
//!   input is 0 without a lighting record, else the record's callback's
//!   (not carried: a ranged function on an object item refuses). The
//!   function `0x0013b390` ([`tags::effects::function::Function::evaluate`]), then, unless the
//!   function's flags have a bit of `0xf0`, the result clamped to 0..1
//!   and mapped onto the function's range. A property driven by an object
//!   function calls the lighting record's callbacks and refuses.
//!
//! The stage applies the values of pixel- and vertex-constant animations;
//! a bitmap frame, render-state or stage-state animation refuses
//! ([`LayerRefusal::Animation`]).

pub use crate::layer_items::{ListItem, SceneLayerSkip, SkipItem, WORLD_LAYERS, clear_scene_layer, world_slot};

mod animation;
mod pixel;
mod texture_transform;

pub use animation::{
    ObjectInputs, PassAnimation, StageFrames, frame_image, property_value, render_time, sample_constants, sample_with_frames,
    sample_with_inputs,
};

use std::ops::Range;
use std::sync::Arc;

use frame_scene::{FrameScene, LayerInputs, LayerItem, LayerOwner, LayerPass, Lighting};
use tags::material::{AnimationKind, EngineInputs, InputReads, Material, ResolvedImplementation};
use tags::shader::AnimatedProperty;

use crate::object_lighting::{LIGHTING_SCALARS, LIGHTING_VECTORS, ShaderValues, scalar_input, vector_input};

/// The layers the stage plans: the scene's 1, 5, 7, 8 and 20 and the
/// first person's 1, 7, 8 and 20.
pub const LAYERS: [u8; 5] = [1, 5, 7, 8, 20];

/// The scene's layer 17 (S: bloom), drawn late in `render_scene_passes`
/// (the layer draw at `0x0002c306`, returning `0x0002c30b`) when the
/// caller's colour byte is set, the colour byte of `render_screen_effects`
/// (docs/graphics/native-post-hud.md). Its begin (`0x00044370`) draws it
/// when `[0x004ba024]` is set and the scene list's mask has it, binds the
/// view's target (the descriptor at `0x0043fa1c` gives -1), clears the
/// channels of the descriptor's flags `0x10` first unless `[0x00485a75]`
/// or `[0x00485a76]` is set, sets the byte `[0x00485a74]` the colour
/// pass's combiners read (`engine::glow::COMBINER_BYTE`), and stencil mode
/// 0. Planned like [`LAYERS`]: the scene's caller only.
pub const BLOOM_LAYER: u8 = 17;

/// Layer 2, the late overlay: the scene draws it after the dynamic lights
/// and the decals of mode 1 (the layer draw at `0x0002c05b`, returning
/// `0x0002c060`), the first person after its layer 7 when its begin says
/// so (`0x00040284`, returning `0x00040289`). The alternate view draws it at `0x0002bda3`. All three take their setup from
/// the general layer begin `0x00044370`, as layers 5, 7, 8 and 20 do; its
/// items draw under the device's stencil ([`item_stencil`]). Its passes
/// multiply the destination (S: the materials' blend, not the layer's).
/// Planned like [`LAYERS`].
pub const OVERLAY_LAYER: u8 = 2;

/// The layers planned besides [`LAYERS`]: [`OVERLAY_LAYER`] and
/// [`BLOOM_LAYER`].
pub const OTHER_LAYERS: [u8; 2] = [OVERLAY_LAYER, BLOOM_LAYER];

/// Every layer the stage plans, [`LAYERS`] then [`OTHER_LAYERS`]: the
/// layers the world's load compiles (`layers_load`).
pub fn planned_layers() -> impl Iterator<Item = u8> {
    LAYERS.into_iter().chain(OTHER_LAYERS)
}

/// Whether the stage plans `layer`: one of [`LAYERS`] or [`OTHER_LAYERS`].
pub fn planned(layer: u8) -> bool {
    LAYERS.contains(&layer) || OTHER_LAYERS.contains(&layer)
}

/// The scalar input the shader instance gives (`shad +0x44`).
const SHADER_SCALARS: u64 = 1 << 21;
/// The integer inputs the shader instance gives (`shad +0x3e`, `+0x40`).
const SHADER_INTS: u32 = 1 << 4 | 1 << 5;

/// What the stage reads of a shader: its material, resolved, and its
/// instance's animated properties and input scales. The glue's tag
/// library answers; tests give synthetic ones.
pub trait Materials {
    /// The shader's material resolved with `inputs` (`None`: the
    /// instance's own, `EngineInputs::for_shader`).
    fn material(&mut self, shader: u32, inputs: Option<&EngineInputs>) -> Result<Arc<Material>, String>;
    /// The instance's animated properties (`shad +0x3c`) and its two input
    /// scales (`+0x44`, `+0x48`).
    fn instance(&mut self, shader: u32) -> Result<Arc<Instance>, String>;
}

/// A shader instance's values the stage reads beyond its material.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Instance {
    pub properties: Vec<AnimatedProperty>,
    pub values: ShaderValues,
}

/// A shader instance's animated properties (`shad +0x3c`, its first
/// postprocess block's): what its passes' animated constants read. Empty
/// when the instance does not read or animates nothing.
pub fn instance_properties(lib: &tags::material::ShaderLibrary, shader: map::TagId) -> Vec<AnimatedProperty> {
    lib.shader(shader)
        .ok()
        .and_then(|instance| instance.postprocess.into_iter().next())
        .map(|p| p.properties)
        .unwrap_or_default()
}

/// One pass of an item's draw: the shader level it is of, its index in
/// the level's layer and the implementation drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PassDraw {
    pub level: usize,
    pub pass: usize,
    pub implementation: usize,
}

/// One item's draw, planned.
#[derive(Clone, Debug)]
pub struct LayerDraw {
    /// The item's place in the layer's list.
    pub slot: u32,
    pub owner: LayerOwner,
    pub section: u16,
    pub part: u8,
    pub shader: u32,
    pub layer: u8,
    /// The baseline resolved with [`Self::inputs`]. Use [`Self::implementation`] for the draw.
    pub material: Arc<Material>,
    pub passes: Vec<PassDraw>,
    pub inputs: EngineInputs,
    /// Only passes with animated constants need a private register image.
    sampled: Vec<(PassDraw, ResolvedImplementation)>,
    /// An object item's palette rows (into [`FrameScene::rows`]).
    pub rows: Range<u32>,
}

impl LayerDraw {
    /// The implementation drawn for `pass`.
    pub fn implementation(&self, pass: &PassDraw) -> Option<&ResolvedImplementation> {
        self.sampled.iter().find(|(p, _)| p == pass).map(|(_, imp)| imp)
            .or_else(|| implementation(&self.material, self.layer, pass))
    }
}

/// Which input an item's pass reads that the scene does not give.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    Scalar(u32),
    Vector(u32),
    Integer(u32),
}

/// Why an item stays out of the stage: a named cause, counted per layer.
#[derive(Clone, Debug, PartialEq)]
pub enum LayerRefusal {
    /// The layer is not one the stage plans.
    Layer(u8),
    /// The material does not resolve.
    Material(String),
    /// The shader's instance does not read.
    Instance(String),
    /// The material draws nothing in the layer at the item's level, or
    /// the item's pass has no vertex shader.
    NoPass,
    /// The item names a pass past the layer's at its level.
    Pass { pass: usize, count: usize },
    /// The item's implementation is not one of the pass's.
    Implementation { pass: usize, count: usize, wanted: usize },
    /// A pass reads an input the scene does not give.
    Input { pass: usize, input: Input },
    /// A binding cannot be applied, e.g. an animated frame or pipeline state.
    Animation { pass: usize, kind: AnimationKind },
    /// A vertex binding needs texture-slot fields the pass does not supply.
    TextureTransform { pass: usize, slot: u8, mode: u8 },
    /// An animated bitmap frame of `slot` reaches `stage`, whose size an
    /// engine vertex constant reads: the binding's size refresh
    /// (`0x00018b31`) takes the animated frame, so the size would follow
    /// the clock.
    FrameSize { pass: usize, slot: u8, stage: u8 },
    /// An animated property is driven by an object function, or is ranged
    /// on an object item: its inputs are the lighting record's callbacks.
    ObjectAnimation { property: usize },
    /// An animated target names a property the instance does not have, or
    /// a function that does not parse.
    Property { property: usize },
    /// A periodic curve needs a wave table unavailable to the native evaluator.
    Wave { property: usize, wave: u8 },
    /// A later object can inherit a structure pass's c[18] upload.
    MaterialConstant18,
    /// The render time did not read.
    NoTime,
}

impl LayerRefusal {
    /// Whether the cause is the sampling of an animated constant: what
    /// `native_shader_animation_*_refused` counts.
    pub fn is_animation(&self) -> bool {
        matches!(
            self,
            LayerRefusal::Animation { .. }
                | LayerRefusal::TextureTransform { .. }
                | LayerRefusal::FrameSize { .. }
                | LayerRefusal::ObjectAnimation { .. }
                | LayerRefusal::Property { .. }
                | LayerRefusal::Wave { .. }
                | LayerRefusal::NoTime
        )
    }

    /// The cause's name, as `render.scene` counts it.
    pub fn name(&self) -> &'static str {
        match self {
            LayerRefusal::Layer(_) => "layer",
            LayerRefusal::Material(_) => "material",
            LayerRefusal::Instance(_) => "instance",
            LayerRefusal::NoPass => "no_pass",
            LayerRefusal::Pass { .. } => "pass",
            LayerRefusal::Implementation { .. } => "implementation",
            LayerRefusal::Input { .. } => "input",
            LayerRefusal::Animation { .. } => "animation",
            LayerRefusal::ObjectAnimation { .. } => "object_animation",
            LayerRefusal::Property { .. } => "property",
            LayerRefusal::TextureTransform { .. } => "texture_transform",
            LayerRefusal::FrameSize { .. } => "frame_size",
            LayerRefusal::Wave { .. } => "wave",
            LayerRefusal::MaterialConstant18 => "material_c18",
            LayerRefusal::NoTime => "time",
        }
    }
}

/// A layer draw's plan: the items the stage draws, in the list's order,
/// and the ones it leaves, each with its cause.
#[derive(Clone, Debug, Default)]
pub struct LayerPlan {
    pub draws: Vec<LayerDraw>,
    pub refused: Vec<(u32, LayerRefusal)>,
}

/// Plans `pass` of `scene`: every item's draw, or why not.
pub fn plan(scene: &FrameScene, pass: &LayerPass, materials: &mut dyn Materials) -> LayerPlan {
    let mut out = LayerPlan::default();
    let mut last_inheritor = None;
    for item in scene.items_of(pass) {
        let result = plan_item(pass, item, materials);
        // An unresolved object, or a selected pass without its own row,
        // can depend on an earlier material's upload.
        if matches!(pass.layer, 1 | BLOOM_LAYER) && matches!(item.owner, LayerOwner::Object(_))
            && !result.as_ref().is_ok_and(|d| d.passes.iter().all(|p| d.implementation(p).is_some_and(writes_material_c18)))
        {
            last_inheritor = Some(item.slot);
        }
        match result {
            Ok(draw) => out.draws.push(draw),
            Err(r) => out.refused.push((item.slot, r)),
        }
    }
    if let Some(last) = last_inheritor {
        out.draws.retain(|draw| {
            if draw.slot < last && !matches!(draw.owner, LayerOwner::Object(_))
                && draw.passes.iter().any(|p| draw.implementation(p).is_some_and(writes_material_c18))
            {
                out.refused.push((draw.slot, LayerRefusal::MaterialConstant18));
                return false;
            }
            true
        });
    }
    out
}

pub(crate) fn writes_material_c18(imp: &ResolvedImplementation) -> bool {
    imp.vertex_constants.iter().any(|r| r.index == 18 && r.written != 0)
}

/// One item's draw.
pub fn plan_item(pass: &LayerPass, item: &LayerItem, materials: &mut dyn Materials) -> Result<LayerDraw, LayerRefusal> {
    if !planned(pass.layer) {
        return Err(LayerRefusal::Layer(pass.layer));
    }
    let shader = item.key.shader;
    let own = materials.material(shader, None).map_err(LayerRefusal::Material)?;
    let instance = materials.instance(shader).map_err(LayerRefusal::Instance)?;
    let selected = item_pass(item);
    let passes = passes_of(&own, pass.layer, selected, item.key.implementation)?;
    let lighting = match item.owner {
        LayerOwner::Object(_) => item.lighting.as_ref(),
        LayerOwner::Cluster(_) | LayerOwner::Placement(_) => None,
    };
    let object = matches!(item.owner, LayerOwner::Object(_));
    // Every pass reads its own masks; the draw's inputs are their union.
    let mut inputs = own.inputs.clone();
    let mut per_draw = false;
    for p in &passes {
        let imp = implementation(&own, pass.layer, p).expect("planned from this material");
        per_draw |= imp.per_draw_inputs;
        give(&mut inputs, imp.input_reads, &pass.inputs, lighting, instance.values)
            .map_err(|input| LayerRefusal::Input { pass: p.pass, input })?;
    }
    let material = if per_draw { materials.material(shader, Some(&inputs)).map_err(LayerRefusal::Material)? } else { own };
    let passes = passes_of(&material, pass.layer, selected, item.key.implementation)?;
    let mut sampled = Vec::new();
    for p in &passes {
        let imp = implementation(&material, pass.layer, p).expect("planned from this material");
        if !imp.animated.is_empty() {
            match sample_constants(imp, &instance.properties, pass.inputs.time, object, p.pass) {
                Ok(at_time) => sampled.push((*p, at_time)),
                Err(r) => {
                    if !object {
                        timeline::native::ShaderAnimation::LayersRefused.count(1);
                    }
                    return Err(r);
                }
            }
        }
    }
    // An object's item draws through the object passes, which count it.
    if !object && !sampled.is_empty() {
        timeline::native::ShaderAnimation::LayersDrawn.count(sampled.len() as u64);
    }
    Ok(LayerDraw {
        slot: item.slot,
        owner: item.owner,
        section: item.key.section,
        part: item.key.part,
        shader,
        layer: pass.layer,
        material,
        passes,
        inputs,
        sampled,
        rows: item.rows.clone(),
    })
}

/// Which passes an item's draw plans: an object item's own pass at its
/// level (its `+0x14` and `+0x15`, `frame_scene::LayerItem::pass`,
/// `level`), as the shader begin `0x00016b90` selects it and the object
/// passes draw it; `None` for a structure item, whose layer set draws
/// every pass of the first level for the part (`fog_gpu::FogSet::draw`).
pub fn item_pass(item: &LayerItem) -> Option<(u8, u8)> {
    matches!(item.owner, LayerOwner::Object(_)).then_some((item.level, item.pass))
}

/// The passes an item draws for `material` in `layer`, each with its
/// implementation: the only one, or the item's. With `selected` (level,
/// pass), that one pass at that level: the guest's list holds one object
/// item per pass. Without, every pass of the first level with a vertex
/// shader. A pass whose implementation has no vertex shader draws
/// nothing ([`LayerRefusal::NoPass`] when nothing is left).
pub fn passes_of(
    material: &Material, layer: u8, selected: Option<(u8, u8)>, item_implementation: usize,
) -> Result<Vec<PassDraw>, LayerRefusal> {
    let level = selected.map_or(0, |(level, _)| usize::from(level));
    let layer = material
        .layers
        .iter()
        .find(|l| l.lod == level && l.layer == usize::from(layer))
        .filter(|l| !l.passes.is_empty())
        .ok_or(LayerRefusal::NoPass)?;
    let wanted: Vec<usize> = match selected {
        Some((_, pass)) => {
            let n = usize::from(pass);
            if n >= layer.passes.len() {
                return Err(LayerRefusal::Pass { pass: n, count: layer.passes.len() });
            }
            vec![n]
        }
        None => (0..layer.passes.len()).collect(),
    };
    let mut out = Vec::with_capacity(wanted.len());
    for n in wanted {
        let p = &layer.passes[n];
        let count = p.implementations.len();
        let implementation = if count == 1 { 0 } else { item_implementation };
        let Some(imp) = p.implementations.get(implementation) else {
            return Err(LayerRefusal::Implementation { pass: n, count, wanted: implementation });
        };
        if imp.vertex_shader.is_some() {
            out.push(PassDraw { level, pass: n, implementation });
        }
    }
    if out.is_empty() {
        return Err(LayerRefusal::NoPass);
    }
    Ok(out)
}

fn implementation<'a>(material: &'a Material, layer: u8, p: &PassDraw) -> Option<&'a ResolvedImplementation> {
    material
        .layers
        .iter()
        .find(|l| l.lod == p.level && l.layer == usize::from(layer))?
        .passes
        .get(p.pass)?
        .implementations
        .get(p.implementation)
}

/// Puts into `inputs` every input `reads` names: the lighting's from the
/// item's lighting (`None` for the structure's), the view's from
/// `view`; the shader's own stay as resolved. The first input given by
/// neither, as the cause.
pub fn give(
    inputs: &mut EngineInputs,
    reads: InputReads,
    view: &LayerInputs,
    lighting: Option<&Lighting>,
    shader: ShaderValues,
) -> Result<(), Input> {
    for n in bits(reads.scalars) {
        if SHADER_SCALARS >> n & 1 != 0 {
            continue;
        }
        let value = if LIGHTING_SCALARS >> n & 1 != 0 {
            scalar_input(n, lighting).map(f32::from_bits)
        } else {
            view.scalar(n as usize)
        };
        inputs.scalars[n as usize] = value.ok_or(Input::Scalar(n))?;
    }
    for n in bits(reads.vectors) {
        let value = if LIGHTING_VECTORS >> n & 1 != 0 {
            let defaults = view.defaults.ok_or(Input::Vector(n))?;
            vector_input(n, lighting, &defaults, shader).map(|v| v.map(f32::from_bits))
        } else {
            view.vector(n as usize)
        };
        inputs.vectors[n as usize] = value.ok_or(Input::Vector(n))?;
    }
    for n in bits(u64::from(reads.ints)) {
        if SHADER_INTS >> n & 1 != 0 {
            continue;
        }
        if n as usize >= inputs.ints.len() {
            return Err(Input::Integer(n));
        }
        inputs.ints[n as usize] = view.int(n as usize).ok_or(Input::Integer(n))?;
    }
    Ok(())
}

fn bits(mut mask: u64) -> impl Iterator<Item = u32> {
    std::iter::from_fn(move || {
        if mask == 0 {
            return None;
        }
        let n = mask.trailing_zeros();
        mask &= mask - 1;
        Some(n)
    })
}

/// What the skip learns of one item of the layer from its caller.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ItemSkip {
    /// The native pass can draw it, and skipping its guest draw leaves
    /// the device as drawing it would ([`skip_holds`]).
    pub take: bool,
    /// Its guest draw is order-free: every pass of it leaves the device
    /// as it found it ([`skip_holds`]) and draws [`order_free`]. Unknown
    /// items (a cause the plan or extraction names before any pass is
    /// resolved) are not.
    pub free: bool,
}

/// What [`clear_layer_items`] cleared.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayerSkip {
    /// The items taken, in the list's order.
    pub drawn: Vec<ListItem>,
    pub cleared: crate::skip::Cleared,
    /// Items the caller would take that keep their guest draw because
    /// taking them would move them before a kept item they do not
    /// commute with: the `order` count.
    pub order: usize,
}

/// Clears `layer`'s bit on each item of the current list the native pass
/// takes, a structure geometry item's with its owner and part, an
/// object's by its slot and shader. The native pass records before the
/// guest's layer draw, so every item it takes draws before every item
/// the guest keeps. An item the caller would take therefore stays the
/// guest's when a kept item comes before it in the list, unless that
/// reordering cannot change the frame or the device:
///
/// - the frame: the taken item and every kept item before it are
///   [`ItemSkip::free`] ([`order_free`]: opaque, depth-tested and
///   depth-writing with the same comparison family, every colour
///   channel written, no stencil write but an idempotent one), so each
///   pixel ends with the nearest fragment whatever the order;
/// - the device: the shader draw `0x00016b90` switches implementations
///   by restoring the cached one's list and setting the new one's
///   ([`skip_holds`]). With every kept item before the taken one
///   restoring what it sets to the values held at the layer call, the
///   device at the taken item's place holds those values on every state
///   it touches, so drawing it there or not leaves the next kept draw
///   the same device; a kept item whose effect is not known stops this.
///
/// An item of the layer that is not a geometry build (or draws no
/// record) is a kept item of unknown effect. `None` when the layer is not
/// current or the list does not read; nothing is written then. Callers
/// plan the writes in a `skip::MaskPreview`.
pub fn clear_layer_items(
    mem: &mut dyn crate::camera::GuestWrite,
    layer: u32,
    ask: &dyn Fn(ListItem) -> ItemSkip,
) -> Option<LayerSkip> {
    use crate::skip::{CURRENT_LAYER, CURRENT_LIST, GEOMETRY_BUILD, ITEM_SIZE, MAX_ITEMS, RENDER_RECORD_SIZE, RENDER_RECORDS};
    use crate::world::Owner;
    if layer >= 32 || mem.read_u32(CURRENT_LAYER)? != layer {
        return None;
    }
    let list = mem.read_u32(CURRENT_LIST)?;
    if list == 0 {
        return None;
    }
    let count = mem.read_u32(list.checked_add(4)?)?;
    let first = mem.read_u32(list.checked_add(0xc)?)?;
    let items = mem.read_u32(list.checked_add(0x14)?)?;
    if first > count || count - first > MAX_ITEMS {
        return None;
    }
    items.checked_add(count.checked_mul(ITEM_SIZE)?)?;
    let bit = 1u32 << layer;
    let mut taken = Vec::new();
    let mut order = 0;
    // A kept item seen, and whether every kept item so far is free.
    let mut kept = false;
    let mut kept_free = true;
    for i in first..count {
        let item = items + ITEM_SIZE * i;
        let mask = mem.read_u32(item + 0x10)?;
        if mask & bit == 0 {
            continue;
        }
        // Not a geometry build, or no record: kept, its effect unknown.
        let geometry = mem.read_u32(item)? == GEOMETRY_BUILD;
        let arg = if geometry { mem.read_u32(item + 0xc)? } else { 0 };
        let record = (arg >> 8) as u16 as i16;
        if !geometry || record < 0 {
            kept = true;
            kept_free = false;
            continue;
        }
        let base = RENDER_RECORDS + RENDER_RECORD_SIZE * record as u32;
        let shader = mem.read_u32(item + 8)?;
        let found = if mem.read_u32(base + 4)? != u32::MAX {
            ListItem::Object { slot: i, shader }
        } else {
            let packed = mem.read_u32(base + 0xc)?;
            let owner = if mem.read_u32(base)? & (1 << 12) == 0 {
                Owner::Cluster((packed >> 9) & 0x1ff)
            } else {
                Owner::Instance((packed >> 18) & 0x7ff)
            };
            ListItem::World(SkipItem { slot: i, owner, part: arg as u8, shader })
        };
        let skip = ask(found);
        if skip.take && (!kept || (kept_free && skip.free)) {
            taken.push((item, mask, found));
            continue;
        }
        if skip.take {
            order += 1;
        }
        kept = true;
        kept_free &= skip.free;
    }
    let mut cleared = crate::skip::Cleared { layer, items: Vec::with_capacity(taken.len()) };
    let mut drawn = Vec::with_capacity(taken.len());
    for (item, mask, found) in taken {
        mem.write_u32(item + 0x10, mask & !bit)?;
        cleared.items.push(item);
        drawn.push(found);
    }
    Some(LayerSkip { drawn, cleared, order })
}

/// LOGICOP's canonical index, which [`gfx::state::RenderStateType`] does
/// not model: `0` (none) draws the colour, anything else combines it
/// with the target's.
const LOGICOP: u32 = 150;

/// Whether a pass of implementation `imp`, drawn under `stencil` (the
/// item's, [`item_stencil`]) over a device whose render state at a guest
/// slot `read` gives (at the layer call), draws the same frame before or
/// after another such pass: each pixel ends with the nearest fragment
/// whatever the order. That holds when, over the device's values with
/// the implementation's list applied:
///
/// - the depth test is on (`ZENABLE` Z or W), writes (`ZWRITEENABLE`)
///   and compares `LESS` or `LESSEQUAL` (assumed: fragments of two
///   different items do not tie in depth);
/// - nothing blends (`ALPHABLENDENABLE` off, `LOGICOP` none) and every
///   colour channel is written (`COLORWRITEENABLE`: a masked channel
///   keeps a farther fragment's value only in one order);
/// - the stencil is off, writes nothing, or writes the same reference on
///   pass under `ALWAYS` and keeps on a depth failure (the stencil then
///   says whether any fragment passed, which the nearest one does in
///   every order), and the implementation sets no stencil state itself;
/// - no engine-driven render value targets one of these states (it is
///   decided per draw).
///
/// The alpha test, the cull and the depth bias change which fragments
/// exist, not how two items' fragments combine. A state that does not
/// read or decode is not free.
pub fn order_free(imp: &ResolvedImplementation, read: &dyn Fn(u32) -> Option<u32>, stencil: &gfx::state::StencilState) -> bool {
    use gfx::state::{CompareFunc, DepthBufferMode, RenderState, RenderStateType as T, StencilOp};
    let table = gfx::xdk::RenderStateTable::for_build(gfx::xdk::HALO2_BUILD);
    let canonical = |guest: u8| table.guest_to_canonical(u32::from(guest));
    const STENCIL: [T; 8] =
        [T::StencilEnable, T::StencilFunc, T::StencilRef, T::StencilMask, T::StencilWriteMask, T::StencilFail, T::StencilZFail, T::StencilPass];
    const ORDER: [T; 5] = [T::ZEnable, T::ZWriteEnable, T::ZFunc, T::AlphaBlendEnable, T::ColorWriteEnable];
    let watched = |c: u32| c == LOGICOP || ORDER.iter().chain(&STENCIL).any(|t| t.index() == c);
    if imp.render_states.iter().any(|s| canonical(s.state).is_some_and(|c| STENCIL.iter().any(|t| t.index() == c))) {
        return false;
    }
    if !imp.engine_render_states.is_empty() {
        let Some((render, _)) = tags::material::engine_states(imp) else {
            return false;
        };
        if render.iter().any(|s| canonical(s.state).is_none_or(watched)) {
            return false;
        }
    }
    // The value at a canonical index: the list's last write, else the device's.
    let value = |c: u32| -> Option<u32> {
        let guest = table.canonical_to_guest(c)?;
        imp.render_states.iter().rev().find(|s| u32::from(s.state) == guest).map(|s| s.value).or_else(|| read(guest))
    };
    let mut rs = RenderState::default();
    for t in ORDER {
        let Some(v) = value(t.index()) else { return false };
        if rs.set_raw(t, v).is_err() {
            return false;
        }
    }
    if value(LOGICOP) != Some(0) {
        return false;
    }
    let depth = rs.depth_mode != DepthBufferMode::Disabled
        && rs.depth_write
        && matches!(rs.depth_func, CompareFunc::Less | CompareFunc::LessEqual);
    let colour = !rs.blend_enable && rs.color_write == gfx::state::ColorWriteMask::ALL;
    let stencil_free = !stencil.enable
        || stencil.write_mask == 0
        || (stencil.func == CompareFunc::Always
            && stencil.depth_fail == StencilOp::Keep
            && matches!(stencil.pass, StencilOp::Keep | StencilOp::Replace));
    depth && colour && stencil_free
}

/// The stencil a world layer's items draw under, and whether the device
/// must already hold stencil mode 1 for the guest's draws to be skipped
/// state-neutrally: the geometry draw's prologue (`0x0004d640`) sets a
/// stencil mode only in layers 1 and 3, mode 0 when the stencil-off flags
/// are set and mode 2 for a marked object's record, and its epilogue
/// (`0x0004d720`) leaves mode 1 after either. So in layer 1 with the
/// stencil off the items draw with the stencil disabled and the device
/// must hold mode 1; with a marked item the device must hold mode 1 (the
/// marked item's draw takes mode 2, `object_state::mark`). Otherwise, and
/// in every other layer, the items draw under the device's stencil.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ItemStencil {
    /// The stencil the unmarked items draw with.
    pub stencil: gfx::state::StencilState,
    /// The device must hold mode 1 (`object_state::structure_stencil(0)`).
    pub needs_mode_1: bool,
}

/// [`ItemStencil`] for layer `layer`, the pass's `stencil_off`, the
/// device's stencil at the layer call, and whether an item the pass
/// takes is marked.
pub fn item_stencil(layer: u8, stencil_off: bool, device: gfx::state::StencilState, marked: bool) -> ItemStencil {
    if layer != 1 && layer != 3 {
        return ItemStencil { stencil: device, needs_mode_1: false };
    }
    if stencil_off {
        return ItemStencil { stencil: gfx::state::StencilState { enable: false, ..device }, needs_mode_1: true };
    }
    ItemStencil { stencil: device, needs_mode_1: marked }
}

/// Whether skipping the guest's draw of a pass with implementation `imp`
/// leaves the device as drawing it would (`read`: the device's render
/// state at a guest slot, at the layer call), as layer 3's walk requires
/// of the parts it skips (docs/graphics/framescene.md, slice 4, "The
/// stage"; **Code**, `h2 disasm`, docs/graphics/layer-draw.md):
///
/// - the shader draw `0x00016b90` compares the draw's implementation and
///   template with the cached ones (`[0x00485b48 + 0x24]`, `+0x20`). On
///   a change it restores the cached implementation's restore list
///   (`0x00018900` on its `+0x11c`, when one is cached), caches the new
///   one and sets its render-state list (`0x00018900` on its `+2`), with
///   no comparison against the device. Draws of the cached
///   implementation set nothing more;
/// - every draw then sets the engine-driven values (`0x0001a170`, from
///   the inputs `0x0001ae70` evaluates);
/// - the layer setup `0x00016b10` restores the cached implementation's
///   list and clears the cache at the next layer draw (and at each
///   layer's first shader begin, `0x0001bbf0`).
///
/// The layer draw runs the setup before any item, so a skipped run from
/// the list's first begins on a cleared cache, and the first item the
/// guest keeps begins on one too: its draw switches, and sets its own
/// list. Skipped, the run leaves the device unchanged. Drawn, each
/// implementation's list is restored before another's is set, and the
/// last one's when the first kept item's draw switches or the next layer
/// begins; a kept item of the same implementation switches nothing, but
/// sets the same list and draws over it either way. So both agree exactly
/// when every state of the restore list already holds and every state of
/// the render-state list either already holds or is one the restore list
/// writes back. A state that does not read refuses. A pass with
/// engine-driven render values the restore list does not write back sets
/// values decided per draw, and refuses here ([`skip_holds_with`] takes
/// the draw's inputs). The first shader begin's fixed states and its
/// cleared texture caches are redone by the first shader begin the guest
/// keeps (the byte `0x004670bc` stays set), or by the next layer draw's.
pub fn skip_holds(imp: &ResolvedImplementation, read: &dyn Fn(u32) -> Option<u32>) -> bool {
    skip_holds_with(imp, None, read)
}

/// [`skip_holds`] for a draw whose engine inputs are `inputs`: the draw
/// also sets each engine-driven render value (`0x0001a170`, after the
/// static list), so each the restore list does not write back must hold
/// too, computed over the value the static list leaves (its own where it
/// sets the state, else the device's: `EngineState::value`). A value
/// whose source slot is not one of the inputs `0x0001ae70` evaluates (the
/// implementation's input masks, `ResolvedImplementation::input_reads`)
/// is whatever the context last held, and refuses, as does a pass whose
/// inputs change per draw (`per_draw_inputs`: `inputs` would not be the
/// guest's) and any such pass without `inputs`.
pub fn skip_holds_with(imp: &ResolvedImplementation, inputs: Option<&EngineInputs>, read: &dyn Fn(u32) -> Option<u32>) -> bool {
    if !imp.restore_states.iter().all(|s| read(u32::from(s.state)) == Some(s.value)) {
        return false;
    }
    let restored = |state: u8| imp.restore_states.iter().any(|r| r.state == state);
    let leaves = |s: &tags::shader::RenderStateValue| {
        let held = read(u32::from(s.state));
        held.is_some() && (held == Some(s.value) || restored(s.state))
    };
    if !imp.render_states.iter().all(leaves) {
        return false;
    }
    if imp.engine_render_states.is_empty() {
        return true;
    }
    let Some((render, _)) = tags::material::engine_states(imp) else {
        return false;
    };
    let mut kept = render.iter().filter(|s| !restored(s.state)).peekable();
    if kept.peek().is_none() {
        return render.iter().all(|s| read(u32::from(s.state)).is_some());
    }
    let Some(inputs) = inputs.filter(|_| !imp.per_draw_inputs) else {
        return false;
    };
    let evaluated = |s: &tags::material::EngineState| match s.source {
        tags::material::EngineStateSource::Scalar(n) => n < 64 && imp.input_reads.scalars & (1u64 << n) != 0,
        tags::material::EngineStateSource::Colour(n) => n < 64 && imp.input_reads.vectors & (1u64 << n) != 0,
    };
    kept.all(|s| {
        let held = read(u32::from(s.state));
        let base = imp.render_states.iter().rev().find(|r| r.state == s.state).map(|r| r.value).or(held);
        evaluated(s) && held.is_some() && base.and_then(|b| s.value(inputs, b)) == held
    })
}

/// The world's owner of a layer item, `None` for an object's.
pub fn world_owner(owner: LayerOwner) -> Option<crate::world::Owner> {
    match owner {
        LayerOwner::Cluster(c) => Some(crate::world::Owner::Cluster(u32::from(c))),
        LayerOwner::Placement(p) => Some(crate::world::Owner::Instance(u32::from(p))),
        LayerOwner::Object(_) => None,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use frame_scene::{
        Defaults, InstanceKey, ItemKey, LIGHTING_WORDS, RenderCamera, SceneBuilder, SceneId, Stamp, ViewKey, ViewKind,
    };
    use map::TagId;
    use std::collections::HashMap;
    use tags::material::{AnimatedTarget, Origin, Register, ResolvedLayer, ResolvedPass};
    use tags::shader::{AnimationBinding, PropertyRef};

    const SHADER: u32 = 0xe000_0040;

    pub(crate) fn implementation(index: usize, reads: InputReads, animated: Vec<AnimatedTarget>) -> ResolvedImplementation {
        ResolvedImplementation {
            index,
            vertex_shader: Some(TagId(0xe000_0002)),
            textures: [None; 4],
            stage_states: Vec::new(),
            render_states: Vec::new(),
            restore_states: Vec::new(),
            pixel_shader: shaders::PixelShaderDef::default().to_bytes(),
            variant: false,
            c0_written: 0,
            c1_written: 0,
            pixel_constants_from_engine: false,
            per_draw_inputs: reads != InputReads::default(),
            input_reads: reads,
            vector_bindings: Vec::new(),
            scalar_bindings: Vec::new(),
            vertex_constants: vec![Register { index: 18, value: [0.0; 4], written: 0xf, origin: Origin::Engine }],
            vertex_defaults: Vec::new(),
            animated,
            engine_render_states: Vec::new(),
            engine_stage_states: Vec::new(),
            dimension_bindings: Vec::new(),
            copies_target: false,
        }
    }

    fn material(layer: usize, passes: Vec<Vec<ResolvedImplementation>>, inputs: EngineInputs) -> Material {
        Material {
            shader: TagId(SHADER),
            template: TagId(0xe000_0041),
            inputs,
            layers: vec![ResolvedLayer {
                lod: 0,
                layer,
                passes: passes
                    .into_iter()
                    .enumerate()
                    .map(|(index, implementations)| ResolvedPass { index, pass: TagId(0xe000_0200), implementations })
                    .collect(),
            }],
        }
    }

    /// A library of one shader: it resolves with whatever inputs it is
    /// given (recorded, and put in the material's `inputs`, as the
    /// resolver keeps them).
    struct Library {
        material: Material,
        instance: Instance,
        asked: Vec<Option<EngineInputs>>,
    }

    impl Materials for Library {
        fn material(&mut self, shader: u32, inputs: Option<&EngineInputs>) -> Result<Arc<Material>, String> {
            if shader != SHADER {
                return Err("no such shader".into());
            }
            self.asked.push(inputs.cloned());
            let mut m = self.material.clone();
            if let Some(i) = inputs {
                m.inputs = i.clone();
            }
            Ok(Arc::new(m))
        }
        fn instance(&mut self, _shader: u32) -> Result<Arc<Instance>, String> {
            Ok(Arc::new(self.instance.clone()))
        }
    }

    fn library(material: Material) -> Library {
        Library { material, instance: Instance::default(), asked: Vec::new() }
    }

    fn own_inputs() -> EngineInputs {
        let mut i = EngineInputs::default();
        i.ints[4] = 2;
        i.scalars[21] = 0.5;
        i
    }

    fn view_inputs() -> LayerInputs {
        let mut v = LayerInputs {
            time: Some(10.0),
            defaults: Some(Defaults { vector: [0.25f32.to_bits(); 3], colour: [0.75f32.to_bits(); 3] }),
            ..LayerInputs::default()
        };
        v.set_scalar(12, 1.0f32.to_bits());
        v.set_vector(0, [1.0f32.to_bits(), 2.0f32.to_bits(), 3.0f32.to_bits()]);
        v.set_int(0, 3);
        v
    }

    fn lighting() -> Lighting {
        let mut words = [0u32; LIGHTING_WORDS];
        for (k, w) in words.iter_mut().enumerate() {
            *w = (k as f32 * 0.01).to_bits();
        }
        words[0x74 / 4] = 0; // no colour slot in use
        Lighting { words, root: Some(0.5f32.to_bits()) }
    }

    fn key(implementation: usize) -> ItemKey {
        ItemKey { record: 1, shader: SHADER, section: 3, part: 2, implementation }
    }

    fn pass(layer: u8, inputs: LayerInputs) -> LayerPass {
        LayerPass { layer, stencil_off: false, texaccum: false, items: 0..0, remainder: 0, inputs }
    }

    fn item(owner: LayerOwner, implementation: usize, lighting: Option<Lighting>) -> LayerItem {
        LayerItem {
            slot: 4, key: key(implementation), level: 0, pass: 0, owner, model: u32::MAX, rows: 0..0, lighting,
            flags: frame_scene::PartFlags::NONE, subparts: None, refusal: None, prt: None,
        }
    }

    #[test]
    fn a_static_material_plans_each_pass_with_its_own_inputs() {
        let m = material(5, vec![vec![implementation(0, InputReads::default(), Vec::new())]], own_inputs());
        let mut lib = library(m);
        let d = plan_item(&pass(5, view_inputs()), &item(LayerOwner::Cluster(3), 0, None), &mut lib).unwrap();
        assert_eq!(d.passes, [PassDraw { level: 0, pass: 0, implementation: 0 }]);
        assert_eq!((d.section, d.part, d.slot), (3, 2, 4));
        assert_eq!(d.inputs, own_inputs());
        assert_eq!(lib.asked, [None], "resolved once, with the instance's own inputs");
        assert!(d.implementation(&d.passes[0]).is_some());
    }

    #[test]
    fn the_items_implementation_picks_among_several_and_one_without_a_vertex_shader_draws_nothing() {
        let mut silent = implementation(1, InputReads::default(), Vec::new());
        silent.vertex_shader = None;
        let several = vec![implementation(0, InputReads::default(), Vec::new()), silent];
        let one = vec![implementation(0, InputReads::default(), Vec::new())];
        let m = material(7, vec![several, one], own_inputs());
        let draw = |pass, implementation| PassDraw { level: 0, pass, implementation };
        // A structure item: every pass of the first level, as its set draws them.
        assert_eq!(passes_of(&m, 7, None, 0).unwrap(), [draw(0, 0), draw(1, 0)]);
        assert_eq!(passes_of(&m, 7, None, 1).unwrap(), [draw(1, 0)], "pass 0's implementation 1 draws nothing");
        assert_eq!(passes_of(&m, 7, None, 2).unwrap_err(), LayerRefusal::Implementation { pass: 0, count: 2, wanted: 2 });
        // An object item: its own pass alone.
        assert_eq!(passes_of(&m, 7, Some((0, 0)), 0).unwrap(), [draw(0, 0)], "pass 0 alone: the list's next item draws pass 1");
        assert_eq!(passes_of(&m, 7, Some((0, 1)), 0).unwrap(), [draw(1, 0)]);
        assert_eq!(passes_of(&m, 7, Some((0, 1)), 1).unwrap(), [draw(1, 0)], "pass 1's only implementation");
        assert_eq!(passes_of(&m, 7, Some((0, 0)), 1).unwrap_err(), LayerRefusal::NoPass, "pass 0's implementation 1 draws nothing");
        assert_eq!(passes_of(&m, 7, Some((0, 0)), 2).unwrap_err(), LayerRefusal::Implementation { pass: 0, count: 2, wanted: 2 });
        assert_eq!(passes_of(&m, 7, Some((0, 2)), 0).unwrap_err(), LayerRefusal::Pass { pass: 2, count: 2 });
        assert_eq!(passes_of(&m, 7, Some((1, 0)), 0).unwrap_err(), LayerRefusal::NoPass, "no level 1");
        assert_eq!(passes_of(&m, 8, None, 0).unwrap_err(), LayerRefusal::NoPass);
    }

    /// The guest's list holds one object item per pass, at the level the
    /// item names (`+0x14`, `+0x15`): each object item plans that one pass
    /// of that level, so a pass-0 item does not draw the later passes the
    /// list's own items draw again. A structure item plans every pass of
    /// the first level, which its layer set draws for the part.
    #[test]
    fn an_object_item_draws_only_its_own_pass_at_its_own_level() {
        let one = || vec![implementation(0, InputReads::default(), Vec::new())];
        let mut m = material(5, vec![one(), one()], own_inputs());
        let mut far = m.layers[0].clone();
        far.lod = 1;
        far.passes.truncate(1);
        far.passes[0].pass = TagId(0xe000_0300);
        m.layers.push(far);
        let mut lib = library(m);
        let five = pass(5, view_inputs());
        let planned = |lib: &mut Library, level, n| {
            plan_item(&five, &LayerItem { level, pass: n, ..item(LayerOwner::Object(InstanceKey(9)), 0, Some(lighting())) }, lib)
        };
        let zero = planned(&mut lib, 0, 0).unwrap();
        assert_eq!(zero.passes, [PassDraw { level: 0, pass: 0, implementation: 0 }], "pass 0 only, not pass 1 again");
        let first = planned(&mut lib, 0, 1).unwrap();
        assert_eq!(first.passes, [PassDraw { level: 0, pass: 1, implementation: 0 }]);
        let far = planned(&mut lib, 1, 0).unwrap();
        assert_eq!(far.passes, [PassDraw { level: 1, pass: 0, implementation: 0 }]);
        assert!(far.implementation(&far.passes[0]).is_some(), "level 1's pass resolves");
        assert_eq!(planned(&mut lib, 1, 1).unwrap_err(), LayerRefusal::Pass { pass: 1, count: 1 }, "level 1 has one pass");
        let structure = LayerItem { level: 1, pass: 1, ..item(LayerOwner::Cluster(3), 0, None) };
        let d = plan_item(&five, &structure, &mut lib).unwrap();
        assert_eq!(d.passes.iter().map(|p| (p.level, p.pass)).collect::<Vec<_>>(), [(0, 0), (0, 1)], "a structure item: the set's passes");
    }

    #[test]
    fn view_and_lighting_inputs_are_given_and_the_material_is_resolved_with_them() {
        let reads = InputReads { scalars: 1 << 12 | 1 << 21 | 1 << 23, vectors: 1 << 0 | 1 << 15 | 1 << 3, ints: 1 << 0 | 1 << 4 };
        let m = material(1, vec![vec![implementation(0, reads, Vec::new())]], own_inputs());
        let mut lib = library(m);
        let l = lighting();
        let d = plan_item(&pass(1, view_inputs()), &item(LayerOwner::Object(InstanceKey(9)), 0, Some(l)), &mut lib).unwrap();
        assert_eq!(d.inputs.scalars[12], 1.0, "the view's");
        assert_eq!(d.inputs.scalars[21], 0.5, "the shader's own stays");
        assert_eq!(d.inputs.scalars[23], f32::from_bits(scalar_input(23, Some(&l)).unwrap()), "the lighting's");
        assert_eq!(d.inputs.vectors[0], [1.0, 2.0, 3.0]);
        assert_eq!(d.inputs.vectors[15], l.vector(0x30).map(f32::from_bits));
        assert_eq!(d.inputs.vectors[3], [0.75; 3], "no colour slot in use: the default colour");
        assert_eq!((d.inputs.ints[0], d.inputs.ints[4]), (3, 2));
        assert_eq!(lib.asked.len(), 2, "resolved again with the draw's inputs");
        assert_eq!(lib.asked[1].as_ref(), Some(&d.inputs));
        assert_eq!(d.material.inputs, d.inputs);
    }

    #[test]
    fn a_structure_item_draws_with_no_lighting_and_an_input_nothing_gives_is_named() {
        let reads = InputReads { scalars: 1 << 23, vectors: 1 << 14, ints: 0 };
        let m = material(5, vec![vec![implementation(0, reads, Vec::new())]], own_inputs());
        let d = plan_item(&pass(5, view_inputs()), &item(LayerOwner::Placement(1), 0, Some(lighting())), &mut library(m)).unwrap();
        assert_eq!(d.inputs.scalars[23], 1.0, "1 without a lighting");
        assert_eq!(d.inputs.vectors[14], [0.25; 3], "the default vector");
        let reads = InputReads { scalars: 1 << 17, ..Default::default() };
        let m = material(5, vec![vec![implementation(0, reads, Vec::new())]], own_inputs());
        let r = plan_item(&pass(5, view_inputs()), &item(LayerOwner::Cluster(1), 0, None), &mut library(m)).unwrap_err();
        assert_eq!(r, LayerRefusal::Input { pass: 0, input: Input::Scalar(17) });
        let reads = InputReads { scalars: 1 << 37, ..Default::default() };
        let m = material(5, vec![vec![implementation(0, reads, Vec::new())]], own_inputs());
        let r = plan_item(&pass(5, view_inputs()), &item(LayerOwner::Cluster(1), 0, None), &mut library(m)).unwrap_err();
        assert_eq!(r, LayerRefusal::Input { pass: 0, input: Input::Scalar(37) }, "the root value needs a lighting");
    }

    pub(crate) fn identity(range: [f32; 2], flags: u8) -> Vec<u8> {
        let mut b = vec![0u8, flags, 0, 0];
        b.extend(range[0].to_le_bytes());
        b.extend(range[1].to_le_bytes());
        b.extend([0u8; 8]);
        b
    }

    pub(crate) fn timed(period: f32, function: Vec<u8>) -> AnimatedProperty {
        AnimatedProperty { input: 0, input_a: 0, input_b: period.to_bits(), function }
    }

    #[test]
    fn a_game_time_property_wraps_its_time_over_its_period_and_maps_onto_its_range() {
        let p = timed(4.0, identity([10.0, 20.0], 0));
        // 10 s over 4 s is 2.5: wrapped to 0.5, mapped onto 10..20.
        assert_eq!(property_value(&p, 0, Some(10.0), false), Ok(15.0));
        let raw = timed(4.0, identity([10.0, 20.0], 0x10));
        assert_eq!(property_value(&raw, 0, Some(10.0), false), Ok(0.5), "a raw output is not mapped");
        assert_eq!(property_value(&p, 0, None, false), Err(LayerRefusal::NoTime));
        let object = AnimatedProperty { input: 1, ..p.clone() };
        assert_eq!(property_value(&object, 3, Some(10.0), false), Err(LayerRefusal::ObjectAnimation { property: 3 }));
        let ranged = timed(4.0, identity([0.0, 1.0], 1));
        assert!(property_value(&ranged, 0, Some(10.0), false).is_ok(), "no lighting record: range input 0");
        assert_eq!(property_value(&ranged, 0, Some(10.0), true), Err(LayerRefusal::ObjectAnimation { property: 0 }));
        let bad = timed(4.0, vec![0u8; 3]);
        assert_eq!(property_value(&bad, 2, Some(10.0), false), Err(LayerRefusal::Property { property: 2 }));
    }

    /// A property on a wave the procedural tables lack (the hologram's scan
    /// noise, wave 9) refuses until a game installs the engine's tables,
    /// then samples them (`Tables::engine`).
    #[test]
    fn an_engine_wave_samples_once_the_game_installs_its_tables() {
        use tags::effects::function::{TABLE_LEN, Tables};
        let mut f = vec![3u8, 0, 9, 0];
        f.extend(0.0f32.to_le_bytes());
        f.extend(1.0f32.to_le_bytes());
        f.extend([0u8; 8]);
        for w in [1.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0] {
            f.extend(w.to_le_bytes());
        }
        let p = timed(4.0, f);
        assert_eq!(property_value(&p, 1, Some(1.0), true), Err(LayerRefusal::Wave { property: 1, wave: 9 }));
        let mut slab = vec![0u8; Tables::ENGINE_SLAB.len()];
        slab[9 * TABLE_LEN..10 * TABLE_LEN].fill(255);
        Tables::install_engine_slab(&slab).unwrap();
        assert_eq!(property_value(&p, 1, Some(1.0), true), Ok(1.0));
    }

    pub(crate) fn animated(kind: AnimationKind, refs: &[(i16, i16)]) -> AnimatedTarget {
        AnimatedTarget {
            kind,
            pixel_register: (kind == AnimationKind::PixelConstant).then_some((false, 0)),
            binding: AnimationBinding { target: 0, b1: 0, b2: 0, properties: 0 },
            properties: refs.iter().map(|&(property, component)| PropertyRef { property, component }).collect(),
            stages: Vec::new(),
        }
    }

    #[test]
    fn constant_animations_take_their_values_and_other_kinds_are_named() {
        // Only the first reference feeds a direct constant. The register and
        // component come entirely from the target word (c[18].z here).
        let mut a = animated(AnimationKind::VertexConstant, &[(0, 3), (99, 2)]);
        a.binding.target = 0x50;
        a.binding.b2 = 0xff;
        let m = material(8, vec![vec![implementation(0, InputReads::default(), vec![a])]], own_inputs());
        let mut lib = library(m);
        lib.instance.properties = vec![timed(4.0, identity([0.0, 2.0], 0))];
        let item0 = item(LayerOwner::Cluster(1), 0, None);
        for (seconds, expected) in [(10., 1.), (11., 1.5), (8., 0.)] {
            let mut pass = pass(8, view_inputs());
            pass.inputs.time = Some(seconds);
            let d = plan_item(&pass, &item0, &mut lib).unwrap();
            assert_eq!(d.implementation(&d.passes[0]).unwrap().vertex_constants[0].value, [0., 0., expected, 0.]);
            assert_eq!(d.material.layers[0].passes[0].implementations[0].vertex_constants[0].value, [0.; 4]);
        }
        let b = animated(AnimationKind::Bitmap, &[(0, 0)]);
        let m = material(8, vec![vec![implementation(0, InputReads::default(), vec![b])]], own_inputs());
        let mut lib = library(m);
        lib.instance.properties = vec![timed(4.0, identity([0.0, 2.0], 0))];
        let r = plan_item(&pass(8, view_inputs()), &item(LayerOwner::Cluster(1), 0, None), &mut lib).unwrap_err();
        assert_eq!(r, LayerRefusal::Animation { pass: 0, kind: AnimationKind::Bitmap });
        let c = animated(AnimationKind::PixelConstant, &[(5, 0)]);
        let m = material(8, vec![vec![implementation(0, InputReads::default(), vec![c])]], own_inputs());
        let r = plan_item(&pass(8, view_inputs()), &item(LayerOwner::Cluster(1), 0, None), &mut library(m)).unwrap_err();
        assert_eq!(r, LayerRefusal::Property { property: 5 }, "the instance has no property 5");
    }

    /// `c[18].z` over 0..2 on a 4 s period.
    pub(crate) fn animated_z() -> (AnimatedTarget, Vec<AnimatedProperty>) {
        let mut a = animated(AnimationKind::VertexConstant, &[(0, 0)]);
        a.binding.target = 0x50;
        (a, vec![timed(4.0, identity([0.0, 2.0], 0))])
    }

    #[test]
    fn a_pass_animation_compiles_at_clock_zero_and_samples_each_draws_clock() {
        let still = implementation(0, InputReads::default(), Vec::new());
        let (base, none) = PassAnimation::baseline(&still, &[], false, 0).unwrap();
        assert!(none.is_none(), "nothing animates");
        assert_eq!(base, still);

        let (target, props) = animated_z();
        let imp = implementation(0, InputReads::default(), vec![target]);
        let (base, animation) = PassAnimation::baseline(&imp, &props, false, 0).unwrap();
        assert!(base.animated.is_empty(), "compiled without its targets");
        assert_eq!(base.vertex_constants[0].value, [0.0; 4], "clock 0");
        let animation = animation.unwrap();
        let at = animation.sample(Some(11.0)).unwrap();
        let v = at.vertex(&shaders::vsh::VertexUniforms::default());
        assert_eq!(v.constants[18][2], 1.5, "11 s over 4 s wraps to 0.75 of 0..2");
        assert_eq!(animation.sample(None).unwrap_err(), LayerRefusal::NoTime);
        assert!(LayerRefusal::NoTime.is_animation() && !LayerRefusal::NoPass.is_animation());

        // Without the instance's property the pass does not compile.
        let r = PassAnimation::baseline(&imp, &[], false, 0).unwrap_err();
        assert_eq!(r, LayerRefusal::Property { property: 0 });
        // A ranged curve refuses on an object only.
        let mut ranged = props.clone();
        ranged[0].function[1] = 1;
        assert!(PassAnimation::baseline(&imp, &ranged, false, 0).is_ok());
        assert_eq!(
            PassAnimation::baseline(&imp, &ranged, true, 0).unwrap_err(),
            LayerRefusal::ObjectAnimation { property: 0 }
        );
    }

    #[test]
    fn colour_and_scalar_bindings_share_a_default_and_preserve_each_others_lanes() {
        let mut rgb = animated(AnimationKind::VertexConstant, &[(0, 99)]);
        rgb.binding.target = (4 << 5) | 9;
        let mut alpha = animated(AnimationKind::VertexConstant, &[(1, 99)]);
        alpha.binding.target = (3 << 5) | 9;
        let imp = implementation(0, InputReads::default(), vec![alpha, rgb]);
        let mut palette = identity([f32::from_bits(0xff00_8000), 0.], 0x20);
        palette[16..20].copy_from_slice(&0xff80_0080u32.to_le_bytes());
        let props = [timed(2., palette), timed(2., identity([2., 8.], 0))];
        let out = sample_constants(&imp, &props, Some(1.), false, 0).unwrap();
        let reg = out.vertex_defaults[0];
        assert_eq!((reg.index, reg.written), (9, 15));
        assert_eq!(reg.value, [64.0 / 255.0, 64.0 / 255.0, 64.0 / 255.0, 5.0]);
    }

    #[test]
    fn transform_binding_uses_the_bitmap_slots_fields_without_sampling_its_own_property() {
        let mut slot = animated(AnimationKind::Bitmap, &[(0, 3), (1, 6)]);
        slot.binding.target = 3;
        let mut vertex = animated(AnimationKind::VertexConstant, &[(9, 0)]);
        let word: u16 = 4 | 16 | (20 << 5) | (3 << 10);
        [vertex.binding.target, vertex.binding.b1] = word.to_le_bytes();
        let props = [timed(2., identity([3., 5.], 0)), timed(2., identity([8., 12.], 0))];
        let imp = implementation(0, InputReads::default(), vec![slot, vertex]);
        let out = sample_constants(&imp, &props, Some(1.), false, 0).unwrap();
        let reg = out.vertex_constants.iter().find(|r| r.index == 22).unwrap();
        assert_eq!(reg.value, [0., 0., 4., 10.]);
        assert_eq!(reg.written, 15);
    }

    /// `0x0001b0a0`'s frame to image: below 1e-4 image 0, else the floor of
    /// the remainder over the image count; a negative remainder, a
    /// non-finite frame or no images bind the rasterizer default texture
    /// (`None`).
    #[test]
    fn a_frame_selects_the_floor_of_its_remainder_over_the_image_count() {
        assert_eq!(frame_image(0.0, 4), Some(0));
        assert_eq!(frame_image(5e-5, 4), Some(0), "too small to wrap");
        assert_eq!(frame_image(-5e-5, 4), Some(0));
        assert_eq!(frame_image(-0.001, 4), None, "past 1e-4 a negative remainder");
        assert_eq!(frame_image(2.75, 4), Some(2));
        assert_eq!(frame_image(5.5, 4), Some(1), "wraps");
        assert_eq!(frame_image(4.0, 4), Some(0));
        assert_eq!(frame_image(3.0, 1), Some(0), "a one-image bitmap always binds image 0");
        assert_eq!(frame_image(0.9, 1), Some(0));
        assert_eq!(frame_image(-1.5, 4), None, "fmod keeps the sign: the default texture");
        assert_eq!(frame_image(2.0, 0), None, "no images");
        assert_eq!(frame_image(f32::NAN, 4), None);
        assert_eq!(frame_image(f32::INFINITY, 4), None);
        assert_eq!(frame_image(f32::from_bits(4.0f32.to_bits() - 1), 4), Some(3), "just below the count");
        assert_eq!(frame_image(1e-5, 0), None, "no images: even image 0 is the default texture");
    }

    fn frame_target(stages: &[(u8, u32)], refs: &[(i16, i16)]) -> AnimatedTarget {
        let mut t = animated(AnimationKind::Bitmap, refs);
        t.binding.target = 5;
        t.stages = stages.iter().map(|&(stage, images)| tags::material::FrameStage { stage, images }).collect();
        t
    }

    /// A bitmap animation's component 0 is each of its stages' frame;
    /// components 1..7 stay the slot's transform; 8 and 9 (slot `+0x38`,
    /// `+0x3c`, never read) are taken without effect, even where frames
    /// are not bound.
    #[test]
    fn a_bitmap_animation_mixes_frame_transform_and_unread_components() {
        let props = [
            timed(4.0, identity([0.0, 8.0], 0)),
            timed(2.0, identity([3.0, 5.0], 0)),
            // Never sampled: a function that does not parse.
            timed(2.0, vec![0xff; 2]),
        ];
        let target = frame_target(&[(0, 6), (2, 6)], &[(0, 0), (1, 1), (2, 8), (2, 9)]);
        let imp = implementation(0, InputReads::default(), vec![target]);
        let (_, frames) = sample_with_frames(&imp, &props, Some(2.0), false, 0).unwrap();
        assert_eq!(frames, [Some(4.0), None, Some(4.0), None], "2 s over 4 s: half of 0..8");
        // Without frames bound, component 0 refuses as before.
        assert_eq!(
            sample_constants(&imp, &props, Some(2.0), false, 0).unwrap_err(),
            LayerRefusal::Animation { pass: 0, kind: AnimationKind::Bitmap }
        );
        // Only the unread components: nothing to bind, nothing refused.
        let unread = frame_target(&[(0, 6)], &[(2, 8), (2, 9)]);
        let imp = implementation(0, InputReads::default(), vec![unread]);
        assert!(sample_constants(&imp, &props, Some(2.0), false, 0).is_ok());
        let (_, frames) = sample_with_frames(&imp, &props, Some(2.0), false, 0).unwrap();
        assert_eq!(frames, [None; 4]);
    }

    /// A pass made for frames samples them per draw and names each frame
    /// stage's image count; one made without them refuses the frame.
    #[test]
    fn a_frame_pass_animation_samples_frames_and_counts_images() {
        let props = [timed(4.0, identity([0.0, 8.0], 0))];
        let imp = implementation(0, InputReads::default(), vec![frame_target(&[(1, 3)], &[(0, 0)])]);
        let (_, animation) = PassAnimation::baseline_with_frames(&imp, &props, true, 0).unwrap();
        let animation = animation.unwrap();
        assert_eq!(animation.frame_counts(), [None, Some(3), None, None]);
        let (_, frames) = animation.sample_with_frames(Some(3.0)).unwrap();
        assert_eq!(frames[1], Some(6.0));
        assert_eq!(frame_image(6.0, 3), Some(0));
        assert_eq!(
            PassAnimation::baseline(&imp, &props, true, 0).unwrap_err(),
            LayerRefusal::Animation { pass: 0, kind: AnimationKind::Bitmap }
        );
    }

    /// The binding's size refresh reads the animated frame, so a stage
    /// size an engine constant reads would follow the clock: refused.
    #[test]
    fn an_animated_frame_under_a_size_constant_is_refused() {
        let props = [timed(4.0, identity([0.0, 8.0], 0))];
        let mut imp = implementation(0, InputReads::default(), vec![frame_target(&[(1, 3)], &[(0, 0)])]);
        imp.dimension_bindings.push(tags::material::EngineVertexConstant { bank: 1, reg: 2, mode: 9, stage: 2, source: 0 });
        assert_eq!(
            sample_with_frames(&imp, &props, Some(1.0), false, 3).unwrap_err(),
            LayerRefusal::FrameSize { pass: 3, slot: 5, stage: 1 }
        );
        imp.dimension_bindings[0].stage = 1; // stage 0's size: not the frame's
        assert!(sample_with_frames(&imp, &props, Some(1.0), false, 3).is_ok());
    }

    /// A list of `layer` in a test memory: render records `(record, flags,
    /// model, packed)` and items `(record, mask)`, item `i`'s shader
    /// `0xe000_0010 + i` and its part `i`.
    fn layer_list(layer: u32, records: &[(u32, u32, u32, u32)], items: &[(u32, u32)]) -> scene_extract::testing::Mem {
        use crate::skip::{CURRENT_LAYER, CURRENT_LIST, GEOMETRY_BUILD, ITEM_SIZE, RENDER_RECORD_SIZE, RENDER_RECORDS};
        let mut m = scene_extract::testing::Mem::default();
        m.w(CURRENT_LAYER, layer);
        m.w(CURRENT_LIST, LIST);
        m.w(LIST + 4, items.len() as u32);
        m.w(LIST + 0xc, 0);
        m.w(LIST + 0x14, ITEMS);
        for &(r, flags, model, packed) in records {
            let base = RENDER_RECORDS + RENDER_RECORD_SIZE * r;
            m.w(base, flags);
            m.w(base + 4, model);
            m.w(base + 0xc, packed);
        }
        for (i, &(record, mask)) in items.iter().enumerate() {
            let i = i as u32;
            let at = ITEMS + ITEM_SIZE * i;
            m.w(at, GEOMETRY_BUILD);
            m.w(at + 8, 0xe000_0010 + i);
            m.w(at + 0xc, record << 8 | i);
            m.w(at + 0x10, mask);
        }
        m
    }

    const LIST: u32 = 0x0060_0000;
    const ITEMS: u32 = 0x0061_0000;
    const TAKE: ItemSkip = ItemSkip { take: true, free: true };
    const KEEP: ItemSkip = ItemSkip { take: false, free: false };

    fn mask_at(m: &scene_extract::testing::Mem, slot: u32) -> Option<u32> {
        use scene_extract::GuestRead;
        m.read_u32(ITEMS + crate::skip::ITEM_SIZE * slot + 0x10)
    }

    #[test]
    fn the_skip_takes_the_accepted_structure_items_before_the_first_kept_one_of_unknown_effect() {
        use crate::skip::ITEM_SIZE;
        use crate::world::Owner;
        // Records 1 and 2 clusters 7 and 8, record 3 an object's, record 4
        // placement 9. Item 1 is not in layer 5; item 3 the object; item 4
        // after it.
        let records = [(1, 0, u32::MAX, 7 << 9), (2, 0, u32::MAX, 8 << 9), (3, 0, 0xe100_0001, 0), (4, 1 << 12, u32::MAX, 9 << 18)];
        let items = [(1, 1 << 5), (3, 1 << 3), (2, 1 << 5 | 1 << 3), (3, 1 << 5), (4, 1 << 5)];
        let mut m = layer_list(5, &records, &items);
        // The object keeps its draw and nothing says what it leaves.
        let structure = |i: ListItem| if matches!(i, ListItem::World(_)) { TAKE } else { KEEP };
        let skip = clear_layer_items(&mut m, 5, &structure).unwrap();
        let owners: Vec<_> = skip.drawn.iter().map(|d| match d {
            ListItem::World(i) => i.owner,
            ListItem::Object { .. } => panic!("an object"),
        }).collect();
        assert_eq!(owners, [Owner::Cluster(7), Owner::Cluster(8)]);
        assert_eq!(skip.drawn[1], ListItem::World(SkipItem { slot: 2, owner: Owner::Cluster(8), part: 2, shader: 0xe000_0012 }));
        assert_eq!(skip.cleared.items, [ITEMS, ITEMS + 2 * ITEM_SIZE]);
        assert_eq!(skip.order, 1, "the placement after the object");
        assert_eq!(mask_at(&m, 0), Some(0), "its layer-5 bit cleared");
        assert_eq!(mask_at(&m, 2), Some(1 << 3), "only layer 5's");
        assert_eq!(mask_at(&m, 4), Some(1 << 5), "after the object: the guest's");
        // A refused first item of unknown effect leaves everything to the
        // guest.
        let mut m = layer_list(5, &records, &items);
        let skip = clear_layer_items(&mut m, 5, &|i| match i {
            ListItem::World(SkipItem { owner: Owner::Cluster(7), .. }) => KEEP,
            i => structure(i),
        })
        .unwrap();
        assert!(skip.drawn.is_empty() && skip.cleared.items.is_empty());
        assert_eq!(skip.order, 2);
        assert_eq!(clear_layer_items(&mut m, 7, &|_| TAKE), None, "not the current layer");
    }

    #[test]
    fn the_skip_takes_object_items_the_plan_drew_in_the_same_run() {
        // Record 1 cluster 7, records 2 and 3 objects'.
        let records = [(1, 0, u32::MAX, 7 << 9), (2, 0, 0xe100_0001, 0), (3, 0, 0xe100_0002, 0)];
        let items = [(1, 1 << 7), (2, 1 << 7), (1, 1 << 7), (3, 1 << 7)];
        let mut m = layer_list(7, &records, &items);
        // The plan drew the object item at slot 1, not the one at 3.
        let skip = clear_layer_items(&mut m, 7, &|i| match i {
            ListItem::World(_) => TAKE,
            ListItem::Object { slot, shader } if slot == 1 && shader == 0xe000_0011 => TAKE,
            ListItem::Object { .. } => KEEP,
        })
        .unwrap();
        assert_eq!(skip.drawn.iter().map(|i| i.slot()).collect::<Vec<_>>(), [0, 1, 2], "the object between the structure's");
        assert_eq!(skip.drawn[1], ListItem::Object { slot: 1, shader: 0xe000_0011 });
        assert!(matches!(skip.drawn[2], ListItem::World(SkipItem { owner: crate::world::Owner::Cluster(7), .. })));
        assert_eq!(skip.cleared.items.len(), 3);
        assert_eq!(skip.order, 0);
        assert_eq!(mask_at(&m, 3), Some(1 << 7), "the refused object keeps its draw");
        // Without the objects, the structure item after the first one
        // stays the guest's, counted.
        let mut m = layer_list(7, &records, &items);
        let skip = clear_layer_items(&mut m, 7, &|i| if matches!(i, ListItem::World(_)) { TAKE } else { KEEP }).unwrap();
        assert_eq!(skip.drawn.len(), 1);
        assert_eq!(skip.order, 1);
    }

    #[test]
    fn world_layers_draw_in_their_slots() {
        use frame_scene::ViewSlot;
        for layer in [1, 3, 5, 8, 20] {
            assert_eq!(world_slot(layer), Some(ViewSlot::OpaqueBase), "layer {layer}");
        }
        assert_eq!(world_slot(7), Some(ViewSlot::ShadowsAndLayer7), "after the object shadows");
        assert_eq!(world_slot(2), Some(ViewSlot::Layer2));
        assert!(world_slot(7) > world_slot(1) && world_slot(2) > world_slot(7));
        for layer in [4, 6, 10, 15, 16, 17, 18] {
            assert_eq!(world_slot(layer), None, "layer {layer} is not a world layer");
        }
        assert!(WORLD_LAYERS.iter().all(|&l| world_slot(l).is_some()));
    }

    /// No item keeps its guest draw: what the scene does not draw is
    /// cleared and counted, in any order, and an unreadable record is a
    /// drop of its own.
    #[test]
    fn an_unreadable_part_is_a_named_drop() {
        use crate::skip::ITEM_SIZE;
        // Record 1 cluster 7, record 2 an object's, record 3 placement 9;
        // item 3's record 9 was never written.
        let records = [(1, 0, u32::MAX, 7 << 9), (2, 0, 0xe100_0001, 0), (3, 1 << 12, u32::MAX, 9 << 18)];
        let items = [(1, 1 << 5), (2, 1 << 5), (3, 1 << 5), (9, 1 << 5), (1, 1 << 3)];
        let mut m = layer_list(5, &records, &items);
        // The scene draws the placement only: the cluster is refused, the
        // object is left (layer 3's rule), the item of layer 3 is not ours.
        let skip = clear_scene_layer(&mut m, 5, false, &|i| matches!(i, ListItem::World(SkipItem { owner: crate::world::Owner::Instance(9), .. }))).unwrap();
        assert_eq!(skip.drawn.iter().map(|i| i.slot()).collect::<Vec<_>>(), [2]);
        assert_eq!((skip.refused, skip.unreadable), (1, 1));
        assert_eq!(skip.cleared.items, [ITEMS, ITEMS + 2 * ITEM_SIZE, ITEMS + 3 * ITEM_SIZE]);
        assert_eq!(mask_at(&m, 0), Some(0), "the refused part draws nothing");
        assert_eq!(mask_at(&m, 3), Some(0), "the unreadable part draws nothing");
        assert_eq!(mask_at(&m, 1), Some(1 << 5), "objects are the walk's when asked");
        assert_eq!(mask_at(&m, 4), Some(1 << 3), "another layer's bit");
        // With objects, the refused object is dropped too.
        let mut m = layer_list(5, &records, &items);
        let skip = clear_scene_layer(&mut m, 5, true, &|_| false).unwrap();
        assert_eq!((skip.drawn.len(), skip.refused, skip.unreadable), (0, 3, 1));
        assert!((0..4).all(|i| mask_at(&m, i) == Some(0)));
        assert_eq!(clear_scene_layer(&mut m, 7, true, &|_| true), None, "not the current layer");
    }

    /// Five structure items; the middle one keeps its guest draw. When it
    /// and the items after it are order-free, the later ones are taken.
    #[test]
    fn a_kept_item_in_the_middle_leaves_later_order_free_items_to_the_native_pass() {
        let records: Vec<_> = (1..=5).map(|r| (r, 0, u32::MAX, (r + 6) << 9)).collect();
        let items: Vec<_> = (1..=5).map(|r| (r, 1 << 1)).collect();
        let mut m = layer_list(1, &records, &items);
        let skip = clear_layer_items(&mut m, 1, &|i| if i.slot() == 2 { ItemSkip { take: false, free: true } } else { TAKE }).unwrap();
        assert_eq!(skip.drawn.iter().map(|i| i.slot()).collect::<Vec<_>>(), [0, 1, 3, 4]);
        assert_eq!(skip.order, 0);
        assert_eq!(skip.cleared.items.len(), 4);
        assert_eq!(mask_at(&m, 2), Some(1 << 1), "the kept item's draw stays");
        assert_eq!(mask_at(&m, 3), Some(0), "the item after it is skipped");
        assert_eq!(mask_at(&m, 4), Some(0));
    }

    /// Where the order between a kept item and a later one matters, the
    /// later one keeps its guest draw and is counted as `order`.
    #[test]
    fn an_order_dependent_item_after_a_kept_one_stays_the_guests_and_is_counted() {
        let records: Vec<_> = (1..=5).map(|r| (r, 0, u32::MAX, (r + 6) << 9)).collect();
        let items: Vec<_> = (1..=5).map(|r| (r, 1 << 1)).collect();
        // Item 3 blends (not free): it stays, and so does item 4 after it.
        let mut m = layer_list(1, &records, &items);
        let skip = clear_layer_items(&mut m, 1, &|i| match i.slot() {
            2 => ItemSkip { take: false, free: true },
            3 => ItemSkip { take: true, free: false },
            _ => TAKE,
        })
        .unwrap();
        assert_eq!(skip.drawn.iter().map(|i| i.slot()).collect::<Vec<_>>(), [0, 1]);
        assert_eq!(skip.order, 2);
        assert_eq!(mask_at(&m, 3), Some(1 << 1));
        assert_eq!(mask_at(&m, 4), Some(1 << 1), "after a kept item that is not free");
        // A kept item of unknown effect (refused before any pass resolved)
        // keeps every later one.
        let mut m = layer_list(1, &records, &items);
        let skip = clear_layer_items(&mut m, 1, &|i| if i.slot() == 2 { KEEP } else { TAKE }).unwrap();
        assert_eq!(skip.drawn.iter().map(|i| i.slot()).collect::<Vec<_>>(), [0, 1]);
        assert_eq!(skip.order, 2);
        // An item that is not a geometry build is one too.
        let mut m = layer_list(1, &records, &items);
        m.w(ITEMS + crate::skip::ITEM_SIZE * 2, 0);
        let skip = clear_layer_items(&mut m, 1, &|_| TAKE).unwrap();
        assert_eq!(skip.drawn.iter().map(|i| i.slot()).collect::<Vec<_>>(), [0, 1]);
        assert_eq!(skip.order, 2);
    }

    #[test]
    fn an_opaque_depth_writing_pass_is_order_free_and_a_blended_or_stencil_counting_one_is_not() {
        use gfx::state::{CompareFunc, RenderStateType as T, StencilOp, StencilState};
        use tags::shader::RenderStateValue;
        let table = gfx::xdk::RenderStateTable::for_build(gfx::xdk::HALO2_BUILD);
        let g = |t: T| table.canonical_to_guest(t.index()).unwrap();
        let logicop = table.canonical_to_guest(LOGICOP).unwrap();
        let device = |slot: u32| {
            Some(match slot {
                s if s == g(T::ZEnable) => 1,
                s if s == g(T::ZWriteEnable) => 1,
                s if s == g(T::ZFunc) => 0x203,
                s if s == g(T::ColorWriteEnable) => 0x0101_0101,
                s if s == logicop => 0,
                _ => 0,
            })
        };
        let off = StencilState::default();
        let imp = implementation(0, InputReads::default(), Vec::new());
        assert!(order_free(&imp, &device, &off));
        let set = |t: T, value: u32| {
            let mut imp = imp.clone();
            imp.render_states.push(RenderStateValue { state: g(t) as u8, value });
            imp
        };
        assert!(!order_free(&set(T::AlphaBlendEnable, 1), &device, &off), "blended");
        assert!(!order_free(&set(T::ZWriteEnable, 0), &device, &off), "no depth write");
        assert!(!order_free(&set(T::ZFunc, 0x202), &device, &off), "depth-equal");
        assert!(!order_free(&set(T::ZEnable, 0), &device, &off), "no depth test");
        assert!(!order_free(&set(T::ColorWriteEnable, 0x0001_0101), &device, &off), "alpha masked");
        assert!(order_free(&set(T::ZFunc, 0x201), &device, &off), "LESS");
        assert!(order_free(&set(T::ZEnable, 2), &device, &off), "W-buffered");
        assert!(!order_free(&set(T::StencilEnable, 0), &device, &off), "the pass sets a stencil state");
        let blending = |slot: u32| if slot == g(T::AlphaBlendEnable) { Some(1) } else { device(slot) };
        assert!(!order_free(&imp, &blending, &off), "the device blends and the pass does not turn it off");
        assert!(order_free(&set(T::AlphaBlendEnable, 0), &blending, &off), "the pass turns it off");
        assert!(!order_free(&imp, &|slot| if slot == logicop { Some(0x1503) } else { device(slot) }, &off), "a logic op");
        assert!(!order_free(&imp, &|slot| if slot == g(T::ZFunc) { None } else { device(slot) }, &off), "unread");
        // Layer 1's mode 1: ALWAYS, REPLACE on pass, KEEP on a depth fail.
        let mode_1 = StencilState {
            enable: true,
            func: CompareFunc::Always,
            reference: 0,
            fail: StencilOp::Keep,
            depth_fail: StencilOp::Keep,
            pass: StencilOp::Replace,
            ..StencilState::default()
        };
        assert!(order_free(&imp, &device, &mode_1));
        assert!(!order_free(&imp, &device, &StencilState { pass: StencilOp::Incr, ..mode_1 }), "counts");
        assert!(!order_free(&imp, &device, &StencilState { depth_fail: StencilOp::Replace, ..mode_1 }), "writes on a depth fail");
        assert!(!order_free(&imp, &device, &StencilState { func: CompareFunc::Equal, ..mode_1 }), "tests what another writes");
        assert!(order_free(&imp, &device, &StencilState { func: CompareFunc::Equal, write_mask: 0, ..mode_1 }), "writes nothing");
        // An engine-driven BLENDCOLOR (target 0) changes nothing unblended;
        // one past the table does not read.
        let mut driven = imp.clone();
        driven.engine_render_states = vec![tags::shader::ExternalBinding { bytes: [0, 0, 0, 2] }];
        assert!(order_free(&driven, &device, &off));
        driven.engine_render_states = vec![tags::shader::ExternalBinding { bytes: [0x40, 1, 0, 3] }];
        assert!(!order_free(&driven, &device, &off));
    }

    #[test]
    fn a_skipped_pass_holds_when_what_it_sets_is_already_held_or_restored() {
        use tags::shader::RenderStateValue;
        let mut imp = implementation(0, InputReads::default(), Vec::new());
        imp.render_states = vec![RenderStateValue { state: 11, value: 13 }];
        imp.restore_states = vec![RenderStateValue { state: 19, value: 20 }];
        let device = |slot: u32| match slot {
            11 => Some(13),
            19 => Some(20),
            _ => Some(0),
        };
        assert!(skip_holds(&imp, &device));
        assert!(!skip_holds(&imp, &|slot| if slot == 19 { Some(21) } else { device(slot) }), "a restore that would change the device");
        assert!(!skip_holds(&imp, &|slot| if slot == 11 { None } else { device(slot) }), "a state that does not read");
        assert!(!skip_holds(&imp, &|slot| if slot == 11 { Some(12) } else { device(slot) }), "a set the restore list does not write back");
        imp.engine_render_states = vec![tags::shader::ExternalBinding { bytes: [0; 4] }];
        assert!(!skip_holds(&imp, &device), "per-draw states");
    }

    /// A blended pass sets its blend and the restore list writes it back:
    /// drawn, the switch away from it (the next implementation's draw or
    /// the next layer's setup) leaves the device as it was, as skipping
    /// it does.
    #[test]
    fn a_pass_whose_restore_list_writes_back_what_it_sets_holds() {
        use tags::shader::{ExternalBinding, RenderStateValue};
        // ALPHABLENDENABLE on, ZWRITEENABLE off; the restore list turns
        // them back and the device holds them back.
        let mut imp = implementation(0, InputReads::default(), Vec::new());
        imp.per_draw_inputs = false;
        imp.render_states = vec![RenderStateValue { state: 0x0e, value: 1 }, RenderStateValue { state: 0x15, value: 0 }];
        imp.restore_states = vec![RenderStateValue { state: 0x0e, value: 0 }, RenderStateValue { state: 0x15, value: 1 }];
        let device = |slot: u32| match slot {
            0x15 => Some(1),
            _ => Some(0),
        };
        assert!(skip_holds(&imp, &device), "set, then written back to what the device holds");
        let blending = |slot: u32| if slot == 0x0e { Some(1) } else { device(slot) };
        assert!(!skip_holds(&imp, &blending), "the restore would turn off a blend the device holds");
        let unread = |slot: u32| if slot == 0x0e { None } else { device(slot) };
        assert!(!skip_holds(&imp, &unread), "a restored state that does not read");
        // A state set and not restored must already hold.
        let mut unrestored = imp.clone();
        unrestored.restore_states.truncate(1);
        assert!(!skip_holds(&unrestored, &device), "ZWRITEENABLE stays off after the draw");
        // An engine-driven value the restore list writes back needs no
        // inputs; one it does not, as before.
        let mut driven = imp.clone();
        // ALPHAREF (0x3d) from scalar 3.
        driven.engine_render_states = vec![ExternalBinding { bytes: [2, 1, 0, 3] }];
        assert!(!skip_holds(&driven, &device), "ALPHAREF is not written back and no inputs give it");
        driven.restore_states.push(RenderStateValue { state: 0x3d, value: 0 });
        assert!(skip_holds(&driven, &device), "written back to the device's ALPHAREF");
        assert!(!skip_holds(&driven, &|slot| if slot == 0x3d { Some(7) } else { device(slot) }), "a restore to another ALPHAREF");
    }

    /// With the draw's inputs, an engine-driven render value holds when
    /// the device already holds it, computed over the device's value.
    #[test]
    fn a_skipped_pass_with_engine_states_holds_when_their_values_do() {
        use tags::shader::{ExternalBinding, RenderStateValue};
        let mut imp = implementation(0, InputReads { scalars: 1 << 3, vectors: 1 << 2, ints: 0 }, Vec::new());
        imp.per_draw_inputs = false;
        imp.render_states = vec![RenderStateValue { state: 11, value: 13 }];
        // ALPHAREF (0x3d) from scalar 3; BLENDCOLOR (0x4b) from colour 2.
        imp.engine_render_states = vec![ExternalBinding { bytes: [2, 1, 0, 3] }, ExternalBinding { bytes: [0, 0, 0, 2] }];
        let mut inputs = EngineInputs::default();
        inputs.scalars[3] = 100.0;
        inputs.vectors[2] = [0.0, 1.0, 0.0];
        let device = |slot: u32| match slot {
            11 => Some(13),
            0x3d => Some(100),
            // Green, over a top byte the draw keeps.
            0x4b => Some(0x7700_ff00),
            _ => Some(0),
        };
        assert!(skip_holds_with(&imp, Some(&inputs), &device));
        assert!(!skip_holds(&imp, &device), "no inputs");
        let other = |slot: u32| if slot == 0x3d { Some(99) } else { device(slot) };
        assert!(!skip_holds_with(&imp, Some(&inputs), &other), "the draw would change ALPHAREF");
        let mut unread = imp.clone();
        unread.input_reads.vectors = 0;
        assert!(!skip_holds_with(&unread, Some(&inputs), &device), "a slot 0x0001ae70 does not fill");
        let mut per_draw = imp.clone();
        per_draw.per_draw_inputs = true;
        assert!(!skip_holds_with(&per_draw, Some(&inputs), &device), "inputs that change per draw");
    }

    #[test]
    fn layer_1_items_draw_unstenciled_with_the_stencil_off_and_need_mode_1_to_skip() {
        let device = crate::object_state::structure_stencil(0);
        let off = item_stencil(1, true, device, false);
        assert!(!off.stencil.enable && off.needs_mode_1, "the prologue's mode 0, the epilogue's mode 1");
        assert_eq!(item_stencil(1, false, device, false), ItemStencil { stencil: device, needs_mode_1: false });
        assert!(item_stencil(1, false, device, true).needs_mode_1, "a marked item's epilogue leaves mode 1");
        for layer in [5, 7, 8, 20] {
            assert_eq!(item_stencil(layer, true, device, true), ItemStencil { stencil: device, needs_mode_1: false }, "layer {layer}");
        }
    }

    #[test]
    fn a_world_write_before_an_inheriting_object_keeps_its_material_upload() {
        for layer in [1, 17] {
            let writer = implementation(0, InputReads::default(), vec![]);
            let mut reader = implementation(1, InputReads::default(), vec![]);
            reader.vertex_constants.clear();
            let mut lib = library(material(layer as usize, vec![vec![writer, reader]], own_inputs()));
            let mut object = item(LayerOwner::Object(InstanceKey(1)), 1, None);
            object.slot = 5;
            let world = item(LayerOwner::Cluster(2), 0, None);
            let after = LayerItem { slot: 6, ..world.clone() };
            let pass = LayerPass { items: 0..3, ..pass(layer, view_inputs()) };
            let mut scene = FrameScene { layer_items: vec![world, object, after], ..Default::default() };
            let got = plan(&scene, &pass, &mut lib);
            assert_eq!(got.refused, [(4, LayerRefusal::MaterialConstant18)]);
            assert_eq!(got.draws.iter().map(|d| d.slot).collect::<Vec<_>>(), [5, 6]);
            scene.layer_items[1].key.implementation = 0;
            assert!(plan(&scene, &pass, &mut lib).refused.is_empty());
            scene.layer_items[1].key.shader ^= 1;
            assert!(plan(&scene, &pass, &mut lib).refused.contains(&(4, LayerRefusal::MaterialConstant18)));
        }
    }

    #[test]
    fn a_scene_layer_plans_its_items_in_order_and_names_the_rest() {
        let m = material(20, vec![vec![implementation(0, InputReads::default(), Vec::new())]], own_inputs());
        let mut lib = library(m);
        let camera = RenderCamera {
            position: [0.0; 3],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            fov: 1.0,
            projection_xy: None,
            viewport: [0, 0, 480, 640],
            near: 0.1,
            far: 100.0,
        };
        let mut b = SceneBuilder::default();
        b.begin(SceneId::default());
        b.open_view(ViewKey { kind: ViewKind::Game, player: 0, view: 0 }, camera).unwrap();
        b.push_layer(pass(20, view_inputs())).unwrap();
        b.push_layer_item(item(LayerOwner::Cluster(1), 0, None), &[]).unwrap();
        let other = LayerItem { slot: 6, key: ItemKey { shader: 0xe000_0099, ..key(0) }, ..item(LayerOwner::Cluster(2), 0, None) };
        b.push_layer_item(other, &[]).unwrap();
        let s = b.seal(Stamp::default()).unwrap();
        let p = &s.layers_of(&s.views[0])[0];
        let got = plan(&s, p, &mut lib);
        assert_eq!(got.draws.iter().map(|d| d.slot).collect::<Vec<_>>(), [4]);
        assert_eq!(got.refused.len(), 1);
        assert_eq!((got.refused[0].0, got.refused[0].1.name()), (6, "material"));
        let mut by_cause: HashMap<&str, u32> = HashMap::new();
        for (_, r) in &got.refused {
            *by_cause.entry(r.name()).or_default() += 1;
        }
        assert_eq!(by_cause["material"], 1);
        let three = pass(3, view_inputs());
        assert_eq!(plan_item(&three, &item(LayerOwner::Cluster(1), 0, None), &mut lib).unwrap_err(), LayerRefusal::Layer(3));
        assert!(planned(BLOOM_LAYER) && !planned(16) && !planned(3), "layer 17 is planned; 3 and 16 are other stages'");
        assert!(planned(OVERLAY_LAYER), "layer 2 is planned");
    }

    #[test]
    fn every_planned_layer_is_loaded_once_and_layer_3_and_the_late_passes_are_not() {
        let all: Vec<u8> = planned_layers().collect();
        assert_eq!(all, [1, 5, 7, 8, 20, 2, 17]);
        assert!(all.iter().all(|&l| planned(l)));
        for other in [3, 6, 15, 16, 18, 19, 21] {
            assert!(!planned(other), "layer {other} is another stage's");
        }
    }

    #[test]
    fn overlay_uses_the_current_views_inputs_for_each_layer_call() {
        let reads = InputReads { scalars: 1 << 12, ..Default::default() };
        let mut lib = library(material(2, vec![vec![implementation(0, reads, Vec::new())]], own_inputs()));
        let placement = item(LayerOwner::Placement(1), 0, None);
        for value in [0.25_f32, 0.75] {
            let mut inputs = view_inputs();
            inputs.set_scalar(12, value.to_bits());
            let draw = plan_item(&pass(OVERLAY_LAYER, inputs), &placement, &mut lib).unwrap();
            assert_eq!(draw.inputs.scalars[12], value);
        }
    }

    #[test]
    fn a_layer_2_item_plans_as_the_other_scene_layers_and_draws_under_the_devices_stencil() {
        let m = material(2, vec![vec![implementation(0, InputReads::default(), Vec::new())]], own_inputs());
        let mut lib = library(m);
        let d = plan_item(&pass(OVERLAY_LAYER, view_inputs()), &item(LayerOwner::Cluster(3), 0, None), &mut lib).unwrap();
        assert_eq!(d.layer, OVERLAY_LAYER);
        assert_eq!(d.passes, [PassDraw { level: 0, pass: 0, implementation: 0 }]);
        assert_eq!(d.inputs, own_inputs());
        let other = plan_item(&pass(OVERLAY_LAYER, view_inputs()), &LayerItem { key: ItemKey { shader: 0xe000_0099, ..key(0) }, ..item(LayerOwner::Cluster(3), 0, None) }, &mut lib);
        assert_eq!(other.unwrap_err().name(), "material", "a shader that does not resolve is named");
        let device = crate::object_state::structure_stencil(0);
        assert_eq!(
            item_stencil(OVERLAY_LAYER, true, device, true),
            ItemStencil { stencil: device, needs_mode_1: false },
            "the geometry draw sets no stencil mode outside layers 1 and 3"
        );
    }
}
