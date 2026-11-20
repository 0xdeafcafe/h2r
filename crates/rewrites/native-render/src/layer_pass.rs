//! A layer draw planned from the scene alone (docs/graphics/framescene.md,
//! slice 4): for each item of a [`frame_scene::LayerPass`] of layers 1, 5,
//! 7, 8 and 20 (the scene's and the first person's), the material passes
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
//!   function `0x0013b390` ([`Function::evaluate`]), then, unless the
//!   function's flags have a bit of `0xf0`, the result clamped to 0..1
//!   and mapped onto the function's range. A property driven by an object
//!   function calls the lighting record's callbacks and refuses.
//!
//! The stage applies the values of pixel- and vertex-constant animations;
//! a bitmap, render-state or stage-state animation refuses
//! ([`LayerRefusal::Animation`]).

use std::ops::Range;
use std::sync::Arc;

use frame_scene::{FrameScene, LayerInputs, LayerItem, LayerOwner, LayerPass, Lighting};
use tags::effects::function::{Function, FunctionType};
use tags::material::{AnimationKind, EngineInputs, InputReads, Material, ResolvedImplementation};
use tags::shader::AnimatedProperty;

use crate::object_lighting::{LIGHTING_SCALARS, LIGHTING_VECTORS, ShaderValues, scalar_input, vector_input};

/// The layers the stage plans: the scene's 1, 5, 7, 8 and 20 and the
/// first person's 1, 7 and 8.
pub const LAYERS: [u8; 5] = [1, 5, 7, 8, 20];

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

/// One pass of an item's draw: its index in the layer and the
/// implementation drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PassDraw {
    pub pass: usize,
    pub implementation: usize,
}

/// The value of one animated target's property components, in its
/// properties' order.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimatedValue {
    pub pass: usize,
    /// Index into the implementation's `animated`.
    pub target: usize,
    pub kind: AnimationKind,
    /// `(component, value)` per property reference.
    pub values: Vec<(i16, f32)>,
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
    /// The material resolved with [`Self::inputs`].
    pub material: Arc<Material>,
    pub passes: Vec<PassDraw>,
    pub inputs: EngineInputs,
    pub animation: Vec<AnimatedValue>,
    /// An object item's palette rows (into [`FrameScene::rows`]).
    pub rows: Range<u32>,
}

impl LayerDraw {
    /// The implementation drawn for `pass`.
    pub fn implementation(&self, pass: &PassDraw) -> Option<&ResolvedImplementation> {
        self.material
            .layers
            .iter()
            .find(|l| l.lod == 0 && l.layer == usize::from(self.layer))
            .and_then(|l| l.passes.get(pass.pass))
            .and_then(|p| p.implementations.get(pass.implementation))
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
    /// A pass animates a bitmap, a render state or a stage state.
    Animation { pass: usize, kind: AnimationKind },
    /// An animated property is driven by an object function, or is ranged
    /// on an object item: its inputs are the lighting record's callbacks.
    ObjectAnimation { property: usize },
    /// An animated target names a property the instance does not have, or
    /// a function that does not parse.
    Property { property: usize },
    /// The render time did not read.
    NoTime,
}

impl LayerRefusal {
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
    if !LAYERS.contains(&pass.layer) {
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
    let mut animation = Vec::new();
    for p in &passes {
        let imp = implementation(&material, pass.layer, p).expect("planned from this material");
        for (target, a) in imp.animated.iter().enumerate() {
            if !matches!(a.kind, AnimationKind::PixelConstant | AnimationKind::VertexConstant) {
                return Err(LayerRefusal::Animation { pass: p.pass, kind: a.kind });
            }
            let mut values = Vec::with_capacity(a.properties.len());
            for r in &a.properties {
                let index = usize::try_from(r.property).map_err(|_| LayerRefusal::Property { property: usize::MAX })?;
                let property = instance.properties.get(index).ok_or(LayerRefusal::Property { property: index })?;
                values.push((r.component, property_value(property, index, pass.inputs.time, object)?));
            }
            animation.push(AnimatedValue { pass: p.pass, target, kind: a.kind, values });
        }
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
        animation,
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

/// An animated property's value at render time `time` (`0x00017550`
/// through `0x0001b230`), or why not. `object`: the item has a lighting
/// record, whose callbacks give the range input.
pub fn property_value(property: &AnimatedProperty, index: usize, time: Option<f64>, object: bool) -> Result<f32, LayerRefusal> {
    if property.input != 0 {
        return Err(LayerRefusal::ObjectAnimation { property: index });
    }
    let function = Function::parse(&property.function).map_err(|_| LayerRefusal::Property { property: index })?;
    if object && function.is_ranged() {
        return Err(LayerRefusal::ObjectAnimation { property: index });
    }
    let time = time.ok_or(LayerRefusal::NoTime)?;
    let period = f32::from_bits(property.input_b);
    let mut x = (time / f64::from(period)) as f32;
    if function.kind != FunctionType::Periodic {
        x = (f64::from(x) % 1.0) as f32;
    }
    let v = function.evaluate(x, 0.0);
    if function.is_raw_output() {
        return Ok(v);
    }
    let v = if 0.0 > v {
        0.0
    } else if v > 1.0 {
        1.0
    } else {
        v
    };
    let [lo, hi] = function.range;
    Ok((hi - lo) * v + lo)
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
        if mem.read_u32(base + 4)? != u32::MAX {
            break; // an object's
        }
        let packed = mem.read_u32(base + 0xc)?;
        let owner = if mem.read_u32(base)? & (1 << 12) == 0 {
            Owner::Cluster((packed >> 9) & 0x1ff)
        } else {
            Owner::Instance((packed >> 18) & 0x7ff)
        };
        let found = SkipItem { slot: i, owner, part: arg as u8, shader: mem.read_u32(item + 8)? };
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

/// The world's owner of a layer item, `None` for an object's.
pub fn world_owner(owner: LayerOwner) -> Option<crate::world::Owner> {
    match owner {
        LayerOwner::Cluster(c) => Some(crate::world::Owner::Cluster(u32::from(c))),
        LayerOwner::Placement(p) => Some(crate::world::Owner::Instance(u32::from(p))),
        LayerOwner::Object(_) => None,
    }
}

/// Whether a planned draw is static: the material as resolved at the load
/// (no input read per draw) and no animation. Only these draw with the
/// programs compiled at the load (`crate::fog_gpu::FogSet`).
pub fn is_static(draw: &LayerDraw) -> bool {
    draw.animation.is_empty() && draw.passes.iter().all(|p| draw.implementation(p).is_some_and(|i| !i.per_draw_inputs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use frame_scene::{
        Defaults, InstanceKey, ItemKey, LIGHTING_WORDS, RenderCamera, SceneBuilder, SceneId, Stamp, ViewKey, ViewKind,
    };
    use map::TagId;
    use std::collections::HashMap;
    use tags::material::{AnimatedTarget, Origin, Register, ResolvedLayer, ResolvedPass};
    use tags::shader::{AnimationBinding, PropertyRef};

    const SHADER: u32 = 0xe000_0040;

    fn implementation(index: usize, reads: InputReads, animated: Vec<AnimatedTarget>) -> ResolvedImplementation {
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
        LayerItem { slot: 4, key: key(implementation), owner, model: u32::MAX, rows: 0..0, lighting }
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

    fn identity(range: [f32; 2], flags: u8) -> Vec<u8> {
        let mut b = vec![0u8, flags, 0, 0];
        b.extend(range[0].to_le_bytes());
        b.extend(range[1].to_le_bytes());
        b.extend([0u8; 8]);
        b
    }

    fn timed(period: f32, function: Vec<u8>) -> AnimatedProperty {
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

    fn animated(kind: AnimationKind, refs: &[(i16, i16)]) -> AnimatedTarget {
        AnimatedTarget {
            kind,
            binding: AnimationBinding { target: 0, b1: 0, b2: 0, properties: 0 },
            properties: refs.iter().map(|&(property, component)| PropertyRef { property, component }).collect(),
        }
    }

    #[test]
    fn constant_animations_take_their_values_and_other_kinds_are_named() {
        let a = animated(AnimationKind::VertexConstant, &[(0, 0), (1, 2)]);
        let m = material(8, vec![vec![implementation(0, InputReads::default(), vec![a])]], own_inputs());
        let mut lib = library(m);
        lib.instance.properties = vec![timed(4.0, identity([0.0, 2.0], 0)), timed(10.0, identity([0.0, 1.0], 0))];
        let d = plan_item(&pass(8, view_inputs()), &item(LayerOwner::Cluster(1), 0, None), &mut lib).unwrap();
        assert_eq!(d.animation.len(), 1);
        assert_eq!(d.animation[0].values, [(0, 1.0), (2, 0.0)], "2.5 wraps to 0.5 of 0..2; 1.0 wraps to 0");
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
    fn a_scene_layer_plans_its_items_in_order_and_names_the_rest() {
        let m = material(20, vec![vec![implementation(0, InputReads::default(), Vec::new())]], own_inputs());
        let mut lib = library(m);
        let camera = RenderCamera {
            position: [0.0; 3],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            fov: 1.0,
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
    }
}
