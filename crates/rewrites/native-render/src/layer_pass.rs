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

mod animation;
mod pixel;
mod texture_transform;

pub use animation::{PassAnimation, property_value, render_time, sample_constants};

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

/// One pass of an item's draw: its index in the layer and the
/// implementation drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PassDraw {
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
    /// The material draws nothing in the layer at the first level of
    /// detail.
    NoPass,
    /// The item's implementation is not one of the pass's.
    Implementation { pass: usize, count: usize, wanted: usize },
    /// A pass reads an input the scene does not give.
    Input { pass: usize, input: Input },
    /// A binding cannot be applied, e.g. an animated frame or pipeline state.
    Animation { pass: usize, kind: AnimationKind },
    /// A vertex binding needs texture-slot fields the pass does not supply.
    TextureTransform { pass: usize, slot: u8, mode: u8 },
    /// An animated property is driven by an object function, or is ranged
    /// on an object item: its inputs are the lighting record's callbacks.
    ObjectAnimation { property: usize },
    /// An animated target names a property the instance does not have, or
    /// a function that does not parse.
    Property { property: usize },
    /// A periodic curve needs a wave table unavailable to the native evaluator.
    Wave { property: usize, wave: u8 },
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
            LayerRefusal::Implementation { .. } => "implementation",
            LayerRefusal::Input { .. } => "input",
            LayerRefusal::Animation { .. } => "animation",
            LayerRefusal::ObjectAnimation { .. } => "object_animation",
            LayerRefusal::Property { .. } => "property",
            LayerRefusal::TextureTransform { .. } => "texture_transform",
            LayerRefusal::Wave { .. } => "wave",
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
    for item in scene.items_of(pass) {
        match plan_item(pass, item, materials) {
            Ok(draw) => out.draws.push(draw),
            Err(r) => out.refused.push((item.slot, r)),
        }
    }
    out
}

/// One item's draw.
pub fn plan_item(pass: &LayerPass, item: &LayerItem, materials: &mut dyn Materials) -> Result<LayerDraw, LayerRefusal> {
    if !planned(pass.layer) {
        return Err(LayerRefusal::Layer(pass.layer));
    }
    let shader = item.key.shader;
    let own = materials.material(shader, None).map_err(LayerRefusal::Material)?;
    let instance = materials.instance(shader).map_err(LayerRefusal::Instance)?;
    let passes = passes_of(&own, pass.layer, item.key.implementation)?;
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
    let passes = passes_of(&material, pass.layer, item.key.implementation)?;
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

/// The passes `layer` draws for `material` (first level of detail), each
/// with its implementation: the only one, or the item's.
pub fn passes_of(material: &Material, layer: u8, item_implementation: usize) -> Result<Vec<PassDraw>, LayerRefusal> {
    let layer = material
        .layers
        .iter()
        .find(|l| l.lod == 0 && l.layer == usize::from(layer))
        .filter(|l| !l.passes.is_empty())
        .ok_or(LayerRefusal::NoPass)?;
    let mut out = Vec::with_capacity(layer.passes.len());
    for (n, p) in layer.passes.iter().enumerate() {
        let count = p.implementations.len();
        let implementation = if count == 1 { 0 } else { item_implementation };
        let Some(imp) = p.implementations.get(implementation) else {
            return Err(LayerRefusal::Implementation { pass: n, count, wanted: implementation });
        };
        if imp.vertex_shader.is_some() {
            out.push(PassDraw { pass: n, implementation });
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
        .find(|l| l.lod == 0 && l.layer == usize::from(layer))?
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

/// One structure item of a layer draw's list as the skip sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SkipItem {
    /// Its index in the list ([`LayerItem::slot`]).
    pub slot: u32,
    pub owner: crate::world::Owner,
    /// The low byte of its render argument: the index into its section's
    /// parts the build `0x0004cbf0` draws.
    pub part: u8,
    /// The item's shader datum (`+8`).
    pub shader: u32,
}

/// One item of a layer draw's list as the skip sees it: a structure
/// item, or an object's by its slot and shader datum (what the scene's
/// [`LayerItem`] for it holds: the plan decides it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrefixItem {
    World(SkipItem),
    Object { slot: u32, shader: u32 },
}

impl PrefixItem {
    pub fn slot(self) -> u32 {
        match self {
            PrefixItem::World(i) => i.slot,
            PrefixItem::Object { slot, .. } => slot,
        }
    }
}

/// Clears `layer`'s bit on the longest run of the current list's items,
/// from its first, that are structure geometry items `native` accepts:
/// the native pass records before the guest's layer draw, so what it
/// takes must come first in the list's order. The walk stops at the first
/// item of the layer that is not one (an object, another build, a refused
/// part); that item and the rest keep their D3D draws. `None` when the
/// layer is not current or the list does not read; nothing is written
/// then. Callers plan the writes in a `skip::MaskPreview`.
pub fn clear_prefix(
    mem: &mut dyn crate::camera::GuestWrite,
    layer: u32,
    native: &dyn Fn(SkipItem) -> bool,
) -> Option<(Vec<SkipItem>, crate::skip::Cleared)> {
    let (drawn, cleared) = clear_layer_prefix(mem, layer, &|item| match item {
        PrefixItem::World(i) => native(i),
        PrefixItem::Object { .. } => false,
    })?;
    let drawn = drawn
        .into_iter()
        .map(|i| match i {
            PrefixItem::World(i) => i,
            PrefixItem::Object { .. } => unreachable!("refused above"),
        })
        .collect();
    Some((drawn, cleared))
}

/// [`clear_prefix`] over structure and object items alike: the run from
/// the list's first item of the layer that are geometry items `native`
/// accepts, a structure item's with its owner and part, an object's by
/// its slot and shader. The walk stops at the first item of the layer it
/// refuses or that is another build.
pub fn clear_layer_prefix(
    mem: &mut dyn crate::camera::GuestWrite,
    layer: u32,
    native: &dyn Fn(PrefixItem) -> bool,
) -> Option<(Vec<PrefixItem>, crate::skip::Cleared)> {
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
    let mut prefix = Vec::new();
    for i in first..count {
        let item = items + ITEM_SIZE * i;
        let mask = mem.read_u32(item + 0x10)?;
        if mask & bit == 0 {
            continue;
        }
        if mem.read_u32(item)? != GEOMETRY_BUILD {
            break;
        }
        let arg = mem.read_u32(item + 0xc)?;
        let record = (arg >> 8) as u16 as i16;
        if record < 0 {
            break;
        }
        let base = RENDER_RECORDS + RENDER_RECORD_SIZE * record as u32;
        let shader = mem.read_u32(item + 8)?;
        let found = if mem.read_u32(base + 4)? != u32::MAX {
            PrefixItem::Object { slot: i, shader }
        } else {
            let packed = mem.read_u32(base + 0xc)?;
            let owner = if mem.read_u32(base)? & (1 << 12) == 0 {
                Owner::Cluster((packed >> 9) & 0x1ff)
            } else {
                Owner::Instance((packed >> 18) & 0x7ff)
            };
            PrefixItem::World(SkipItem { slot: i, owner, part: arg as u8, shader })
        };
        if !native(found) {
            break;
        }
        prefix.push((item, mask, found));
    }
    let mut cleared = crate::skip::Cleared { layer, items: Vec::with_capacity(prefix.len()) };
    let mut drawn = Vec::with_capacity(prefix.len());
    for (item, mask, found) in prefix {
        mem.write_u32(item + 0x10, mask & !bit)?;
        cleared.items.push(item);
        drawn.push(found);
    }
    Some((drawn, cleared))
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
            slot: 4, key: key(implementation), owner, model: u32::MAX, rows: 0..0, lighting,
            flags: frame_scene::PartFlags::NONE, subparts: None, refusal: None, prt: None,
        }
    }

    #[test]
    fn a_static_material_plans_each_pass_with_its_own_inputs() {
        let m = material(5, vec![vec![implementation(0, InputReads::default(), Vec::new())]], own_inputs());
        let mut lib = library(m);
        let d = plan_item(&pass(5, view_inputs()), &item(LayerOwner::Cluster(3), 0, None), &mut lib).unwrap();
        assert_eq!(d.passes, [PassDraw { pass: 0, implementation: 0 }]);
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
        assert_eq!(passes_of(&m, 7, 0).unwrap(), [PassDraw { pass: 0, implementation: 0 }, PassDraw { pass: 1, implementation: 0 }]);
        assert_eq!(passes_of(&m, 7, 1).unwrap(), [PassDraw { pass: 1, implementation: 0 }], "pass 0's implementation 1 draws nothing");
        assert_eq!(passes_of(&m, 7, 2).unwrap_err(), LayerRefusal::Implementation { pass: 0, count: 2, wanted: 2 });
        assert_eq!(passes_of(&m, 8, 0).unwrap_err(), LayerRefusal::NoPass);
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

    pub(crate) fn animated(kind: AnimationKind, refs: &[(i16, i16)]) -> AnimatedTarget {
        AnimatedTarget {
            kind,
            pixel_register: (kind == AnimationKind::PixelConstant).then_some((false, 0)),
            binding: AnimationBinding { target: 0, b1: 0, b2: 0, properties: 0 },
            properties: refs.iter().map(|&(property, component)| PropertyRef { property, component }).collect(),
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

    #[test]
    fn the_skip_takes_the_accepted_structure_items_up_to_the_first_it_cannot() {
        use crate::skip::{CURRENT_LAYER, CURRENT_LIST, GEOMETRY_BUILD, ITEM_SIZE, RENDER_RECORD_SIZE, RENDER_RECORDS};
        use crate::world::Owner;
        use scene_extract::GuestRead;
        use scene_extract::testing::Mem;
        const LIST: u32 = 0x0060_0000;
        const ITEMS: u32 = 0x0061_0000;
        let mut m = Mem::default();
        m.w(CURRENT_LAYER, 5);
        m.w(CURRENT_LIST, LIST);
        m.w(LIST + 4, 5);
        m.w(LIST + 0xc, 0);
        m.w(LIST + 0x14, ITEMS);
        // Records 1 and 2 clusters 7 and 8, record 3 an object's, record 4
        // placement 9.
        for (r, flags, model, packed) in [(1, 0, u32::MAX, 7 << 9), (2, 0, u32::MAX, 8 << 9), (3, 0, 0xe100_0001, 0), (4, 1 << 12, u32::MAX, 9 << 18)] {
            let base = RENDER_RECORDS + RENDER_RECORD_SIZE * r;
            m.w(base, flags);
            m.w(base + 4, model);
            m.w(base + 0xc, packed);
        }
        // Item 1 is not in layer 5; item 3 the object; item 4 after it.
        for (i, record, mask) in [(0u32, 1u32, 1u32 << 5), (1, 3, 1 << 3), (2, 2, 1 << 5 | 1 << 3), (3, 3, 1 << 5), (4, 4, 1 << 5)] {
            let at = ITEMS + ITEM_SIZE * i;
            m.w(at, GEOMETRY_BUILD);
            m.w(at + 8, 0xe000_0010 + i);
            m.w(at + 0xc, record << 8 | i);
            m.w(at + 0x10, mask);
        }
        let (drawn, cleared) = clear_prefix(&mut m, 5, &|_| true).unwrap();
        assert_eq!(drawn.iter().map(|d| d.owner).collect::<Vec<_>>(), [Owner::Cluster(7), Owner::Cluster(8)]);
        assert_eq!(drawn[1], SkipItem { slot: 2, owner: Owner::Cluster(8), part: 2, shader: 0xe000_0012 });
        assert_eq!(cleared.items, [ITEMS, ITEMS + 2 * ITEM_SIZE]);
        assert_eq!(m.read_u32(ITEMS + 0x10), Some(0), "its layer-5 bit cleared");
        assert_eq!(m.read_u32(ITEMS + 2 * ITEM_SIZE + 0x10), Some(1 << 3), "only layer 5's");
        assert_eq!(m.read_u32(ITEMS + 4 * ITEM_SIZE + 0x10), Some(1 << 5), "after the object: the guest's");
        // A refused first item leaves everything to the guest.
        m.w(ITEMS + 0x10, 1 << 5);
        m.w(ITEMS + 2 * ITEM_SIZE + 0x10, 1 << 5);
        let (drawn, cleared) = clear_prefix(&mut m, 5, &|i| i.owner != Owner::Cluster(7)).unwrap();
        assert!(drawn.is_empty() && cleared.items.is_empty());
        assert_eq!(clear_prefix(&mut m, 7, &|_| true), None, "not the current layer");
    }

    #[test]
    fn the_skip_takes_object_items_the_plan_drew_in_the_same_run() {
        use crate::skip::{CURRENT_LAYER, CURRENT_LIST, GEOMETRY_BUILD, ITEM_SIZE, RENDER_RECORD_SIZE, RENDER_RECORDS};
        use crate::world::Owner;
        use scene_extract::GuestRead;
        use scene_extract::testing::Mem;
        const LIST: u32 = 0x0060_0000;
        const ITEMS: u32 = 0x0061_0000;
        let mut m = Mem::default();
        m.w(CURRENT_LAYER, 7);
        m.w(CURRENT_LIST, LIST);
        m.w(LIST + 4, 4);
        m.w(LIST + 0xc, 0);
        m.w(LIST + 0x14, ITEMS);
        // Record 1 cluster 7, records 2 and 3 objects'.
        for (r, model, packed) in [(1, u32::MAX, 7 << 9), (2, 0xe100_0001, 0), (3, 0xe100_0002, 0)] {
            let base = RENDER_RECORDS + RENDER_RECORD_SIZE * r;
            m.w(base, 0);
            m.w(base + 4, model);
            m.w(base + 0xc, packed);
        }
        for (i, record) in [(0u32, 1u32), (1, 2), (2, 1), (3, 3)] {
            let at = ITEMS + ITEM_SIZE * i;
            m.w(at, GEOMETRY_BUILD);
            m.w(at + 8, 0xe000_0010 + i);
            m.w(at + 0xc, record << 8);
            m.w(at + 0x10, 1 << 7);
        }
        // The plan drew the object item at slot 1, not the one at 3.
        let (drawn, cleared) = clear_layer_prefix(&mut m, 7, &|i| match i {
            PrefixItem::World(_) => true,
            PrefixItem::Object { slot, shader } => slot == 1 && shader == 0xe000_0011,
        })
        .unwrap();
        assert_eq!(drawn.iter().map(|i| i.slot()).collect::<Vec<_>>(), [0, 1, 2], "the object between the structure's");
        assert_eq!(drawn[1], PrefixItem::Object { slot: 1, shader: 0xe000_0011 });
        assert!(matches!(drawn[2], PrefixItem::World(SkipItem { owner: Owner::Cluster(7), .. })));
        assert_eq!(cleared.items.len(), 3);
        assert_eq!(m.read_u32(ITEMS + 3 * ITEM_SIZE + 0x10), Some(1 << 7), "the refused object keeps its draw");
        // The structure-only skip stops at the object.
        for i in 0..4 {
            m.w(ITEMS + ITEM_SIZE * i + 0x10, 1 << 7);
        }
        let (drawn, _) = clear_prefix(&mut m, 7, &|_| true).unwrap();
        assert_eq!(drawn.len(), 1);
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
        assert_eq!(d.passes, [PassDraw { pass: 0, implementation: 0 }]);
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
