//! An implementation's engine-driven render and texture-stage states
//! (`tags::material::EngineState`, docs/formats/shader-instances.md) as
//! the native pass's state, for one draw's engine inputs.
//!
//! The draw sets the implementation's render states on entry
//! (`0x00018900`) and its stage states into the deferred texture-state
//! array (`0x000189a0`, `0x404dd0`), then the engine-driven values over
//! them (`0x0001a170`, `0x0001ab50`). So [`apply`] puts each render value
//! after the static list, where [`crate::pass_state::pass_state`] reads it
//! as the D3D path would, and writes each stage value into its stage's
//! [`StageStates`]. Where each target lands:
//!
//! - `BLENDCOLOR`: `PassState::blend_constant`, a dynamic state of the
//!   render pass;
//! - `ALPHAREF`: `PassState::alpha_test`'s reference, a pixel uniform;
//! - `POLYGONOFFSETZSLOPESCALE`, `POLYGONOFFSETZOFFSET`: the depth bias,
//!   which `pass_state` refuses while `SOLIDOFFSETENABLE` is on;
//! - `FOGCOLOR`: inert with fog off; with it on the fog colour a native
//!   pass uses is not the pass's, so it refuses
//!   ([`EngineStateRefusal::FogColour`]);
//! - `LINEWIDTH`: inert, native passes draw triangles;
//! - `MIPMAPLODBIAS`: `StageStates::mip_lod_bias`, the bias the
//!   translated fetch adds (`crate::program::lod_bias`);
//! - `BORDERCOLOR`, `COLORKEYCOLOR`: their `StageStates` fields; the
//!   checks on those (border addressing in `crate::program::pixel_parity`)
//!   see the driven values;
//! - `BUMPENV*`: no field; inert while the program has no bump texture
//!   mode, which `pixel_parity` refuses.
//!
//! The passes translated once at the load (decorators, the world's layer
//! 3, lights, screen effects, fog) take [`apply_static`], whose sources
//! are the instance's own inputs only.

use gfx::state::RenderStateType;
use gfx::xdk::{HALO2_BUILD, RenderStateTable};
use std::collections::HashMap;
use tags::material::{
    EngineInputs, EngineState, EngineStateKind, EngineStateSource, INSTANCE_SCALARS, ResolvedImplementation, engine_states,
};
use tags::shader::{RenderStateValue, StageStates};

/// `D3DRS_FOGCOLOR`'s 5849 slot (`ANIMATED_RENDER_STATES`).
const FOGCOLOR: u8 = 0x8a;
/// The stage states the engine drives (`ANIMATED_STAGE_STATES`).
const MIPMAPLODBIAS: u8 = 0x06;
const BUMPENV_FIRST: u8 = 0x16;
const BUMPENV_LAST: u8 = 0x1b;
const BORDERCOLOR: u8 = 0x1d;
const COLORKEYCOLOR: u8 = 0x1e;

/// An implementation's states with its engine-driven values applied.
#[derive(Clone, Debug, PartialEq)]
pub struct EngineApplied {
    /// The render states, each engine-driven value after the static list
    /// in the order the draw sets them: what `pass_state` takes.
    pub render_states: Vec<RenderStateValue>,
    /// The stage states, the driven fields in place.
    pub stage_states: Vec<StageStates>,
}

/// Why an implementation's engine-driven states keep it on the D3D path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineStateRefusal {
    /// A binding past the state tables or the four stages.
    Undecoded,
    /// A source slot past the engine inputs.
    Slot { state: u8, stage: Option<u8> },
    /// A colour state written in part (its top byte, or its low 24 bits)
    /// that the implementation does not set: the rest is what the draws
    /// before it left.
    Inherits { state: u8 },
    /// A stage state for a stage the implementation sets no states for.
    Stage { stage: u8 },
    /// `FOGCOLOR` driven with fog on.
    FogColour,
    /// A source that is not the same for every draw of the material
    /// ([`apply_static`]).
    PerDraw { state: u8, stage: Option<u8> },
}

/// The bits of its state a value writes: a scalar into a colour its top
/// byte, a colour its low 24 bits (`EngineState::value`), else all.
fn written(s: EngineState) -> u32 {
    match (s.source, s.kind) {
        (EngineStateSource::Scalar(_), EngineStateKind::Colour) => 0xff00_0000,
        (EngineStateSource::Colour(_), _) => 0x00ff_ffff,
        (EngineStateSource::Scalar(_), _) => u32::MAX,
    }
}

/// `imp`'s states with its engine-driven values for `inputs` applied
/// (module docs), or why not.
pub fn apply(imp: &ResolvedImplementation, inputs: &EngineInputs) -> Result<EngineApplied, EngineStateRefusal> {
    let (render, stage) = engine_states(imp).ok_or(EngineStateRefusal::Undecoded)?;
    let mut render_states = imp.render_states.clone();
    // The bits of each state the list decides: all of a state it sets.
    let mut known: HashMap<u8, u32> = imp.render_states.iter().map(|s| (s.state, u32::MAX)).collect();
    for s in &render {
        let held = render_states.iter().rev().find(|v| v.state == s.state).map_or(0, |v| v.value);
        let value = s.value(inputs, held).ok_or(EngineStateRefusal::Slot { state: s.state, stage: None })?;
        render_states.push(RenderStateValue { state: s.state, value });
        *known.entry(s.state).or_default() |= written(*s);
    }
    if let Some(s) = render.iter().find(|s| known.get(&s.state) != Some(&u32::MAX)) {
        return Err(EngineStateRefusal::Inherits { state: s.state });
    }
    if render.iter().any(|s| s.state == FOGCOLOR) && fog_on(&imp.render_states) {
        return Err(EngineStateRefusal::FogColour);
    }
    let mut stage_states = imp.stage_states.clone();
    for s in &stage {
        let n = s.stage.unwrap_or_default();
        let rec = stage_states.get_mut(usize::from(n)).ok_or(EngineStateRefusal::Stage { stage: n })?;
        let field = match s.state {
            MIPMAPLODBIAS => Some(&mut rec.mip_lod_bias),
            BORDERCOLOR => Some(&mut rec.border_color),
            COLORKEYCOLOR => Some(&mut rec.color_key_color),
            BUMPENV_FIRST..=BUMPENV_LAST => None,
            _ => return Err(EngineStateRefusal::Undecoded),
        };
        let held = field.as_ref().map_or(0, |f| **f);
        let value = s.value(inputs, held).ok_or(EngineStateRefusal::Slot { state: s.state, stage: s.stage })?;
        if let Some(f) = field {
            *f = value;
        }
    }
    Ok(EngineApplied { render_states, stage_states })
}

/// [`apply`] for a pass translated once at the load, its states the same
/// for every draw of the material: each source must be an input the
/// shader instance supplies (`INSTANCE_SCALARS`, in `inputs` from
/// `EngineInputs::for_shader`) that the implementation reads, so
/// `0x0001ae70` fills its slot. Any other source is an input the draw
/// supplies (time, lights, the object), which changes per draw, or a slot
/// the context holds from an earlier draw; a colour source is a vector
/// input, and the instance supplies none.
pub fn apply_static(imp: &ResolvedImplementation, inputs: &EngineInputs) -> Result<EngineApplied, EngineStateRefusal> {
    let (render, stage) = engine_states(imp).ok_or(EngineStateRefusal::Undecoded)?;
    let fixed = |s: &EngineState| match s.source {
        EngineStateSource::Scalar(n) => n < 64 && INSTANCE_SCALARS & imp.input_reads.scalars & (1u64 << n) != 0,
        EngineStateSource::Colour(_) => false,
    };
    if let Some(s) = render.iter().chain(&stage).find(|s| !fixed(s)) {
        return Err(EngineStateRefusal::PerDraw { state: s.state, stage: s.stage });
    }
    apply(imp, inputs)
}

/// Whether `states` leave fog on (`FOGENABLE`; unset, the default off,
/// as `pass_state` reads it).
fn fog_on(states: &[RenderStateValue]) -> bool {
    let table = RenderStateTable::for_build(HALO2_BUILD);
    states
        .iter()
        .rev()
        .find(|s| RenderStateType::from_guest_index(&table, u32::from(s.state)) == Some(RenderStateType::FogEnable))
        .is_some_and(|s| s.value != 0)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use map::TagId;
    use tags::material::{InputReads, Origin, Register};
    use tags::shader::ExternalBinding;

    /// A synthetic implementation with `render` static states and
    /// `stages` stage records.
    pub(crate) fn implementation(render: Vec<RenderStateValue>, stages: usize) -> ResolvedImplementation {
        let stage = StageStates {
            address_u: 1,
            address_v: 1,
            address_w: 1,
            mag_filter: 2,
            min_filter: 2,
            mip_filter: 2,
            mip_lod_bias: 0,
            max_mip_level: 0,
            max_anisotropy: 1,
            color_key_op: 0,
            color_sign: 0,
            alpha_kill: 0,
            border_color: 0x4400_0000,
            color_key_color: 0,
            unread: [0; 2],
        };
        ResolvedImplementation {
            index: 0,
            vertex_shader: Some(TagId(0xe000_0002)),
            textures: [None; 4],
            stage_states: vec![stage; stages],
            render_states: render,
            restore_states: Vec::new(),
            pixel_shader: shaders::PixelShaderDef::default().to_bytes(),
            variant: false,
            c0_written: 0,
            c1_written: 0,
            pixel_constants_from_engine: false,
            per_draw_inputs: false,
            input_reads: InputReads::default(),
            vector_bindings: Vec::new(),
            scalar_bindings: Vec::new(),
            vertex_constants: vec![Register { index: 18, value: [0.0; 4], written: 0xf, origin: Origin::Instance }],
            vertex_defaults: Vec::new(),
            animated: Vec::new(),
            engine_render_states: Vec::new(),
            engine_stage_states: Vec::new(),
            dimension_bindings: Vec::new(),
        }
    }

    fn b(bytes: [u8; 4]) -> ExternalBinding {
        ExternalBinding { bytes }
    }

    fn inputs() -> EngineInputs {
        let mut e = EngineInputs::default();
        e.scalars[1] = 0.5;
        e.scalars[2] = 64.0;
        e.scalars[3] = -1.0;
        e.vectors[0] = [1.0, 0.0, 0.0];
        e
    }

    /// Render values follow the static list; a blend colour whose two
    /// halves both come from the engine needs no static value.
    #[test]
    fn render_values_follow_the_static_list() {
        let rs = |state, value| RenderStateValue { state, value };
        let mut imp = implementation(vec![rs(0x3d, 10), rs(0x4b, 0x0000_00ff)], 0);
        // ALPHAREF from scalar 2; BLENDCOLOR's top byte from scalar 1.
        imp.engine_render_states = vec![b([2, 1, 0, 2]), b([0, 1, 0, 1])];
        let a = apply(&imp, &inputs()).unwrap();
        assert_eq!(&a.render_states[2..], &[rs(0x3d, 64), rs(0x4b, 0x8000_00ff)]);
        // Without the static blend colour its low 24 bits are inherited...
        imp.render_states.pop();
        assert_eq!(apply(&imp, &inputs()), Err(EngineStateRefusal::Inherits { state: 0x4b }));
        // ...unless the other entry writes them from a colour.
        imp.engine_render_states.push(b([1, 0, 0, 0]));
        let a = apply(&imp, &inputs()).unwrap();
        assert_eq!(a.render_states.last(), Some(&rs(0x4b, 0x80ff_0000)));
    }

    #[test]
    fn a_driven_fog_colour_refuses_only_with_fog_on() {
        let rs = |state, value| RenderStateValue { state, value };
        let table = RenderStateTable::for_build(HALO2_BUILD);
        let fog = (0..=255u8)
            .find(|&s| RenderStateType::from_guest_index(&table, u32::from(s)) == Some(RenderStateType::FogEnable))
            .expect("a 5849 slot");
        let mut imp = implementation(vec![rs(fog, 0)], 0);
        imp.engine_render_states = vec![b([6, 1, 0, 1]), b([6, 0, 0, 0])];
        assert!(apply(&imp, &inputs()).is_ok(), "fog off: inert");
        imp.render_states = vec![rs(fog, 1)];
        assert_eq!(apply(&imp, &inputs()), Err(EngineStateRefusal::FogColour));
    }

    #[test]
    fn stage_values_land_in_their_stage_fields() {
        let mut imp = implementation(Vec::new(), 2);
        // Stage 1 LOD bias from scalar 3, its border colour's low 24 bits
        // from colour 0, and a bump matrix entry (no field).
        imp.engine_stage_states = vec![b([1, 0, 1, 3]), b([1, 2, 0, 0]), b([1, 4, 1, 1])];
        let a = apply(&imp, &inputs()).unwrap();
        assert_eq!(a.stage_states[0], imp.stage_states[0]);
        assert_eq!(f32::from_bits(a.stage_states[1].mip_lod_bias), -1.0);
        assert_eq!(a.stage_states[1].border_color, 0x44ff_0000, "the top byte kept");
        imp.engine_stage_states = vec![b([2, 0, 1, 3])];
        assert_eq!(apply(&imp, &inputs()), Err(EngineStateRefusal::Stage { stage: 2 }));
        imp.engine_stage_states = vec![b([1, 0, 1, 64])];
        assert_eq!(apply(&imp, &inputs()), Err(EngineStateRefusal::Slot { state: 6, stage: Some(1) }));
        imp.engine_stage_states = vec![b([4, 0, 1, 0])];
        assert_eq!(apply(&imp, &inputs()), Err(EngineStateRefusal::Undecoded));
    }

    /// At the load only the instance's scalar 21, read by the
    /// implementation, is the same for every draw.
    #[test]
    fn a_static_pass_takes_only_the_instance_scalar() {
        let rs = |state, value| RenderStateValue { state, value };
        let mut imp = implementation(vec![rs(0x3d, 1)], 0);
        imp.input_reads.scalars = 1 << 21;
        let mut e = EngineInputs::default();
        e.scalars[21] = 33.0;
        imp.engine_render_states = vec![b([2, 1, 0, 21])];
        assert_eq!(apply_static(&imp, &e).unwrap().render_states.last(), Some(&rs(0x3d, 33)));
        imp.input_reads.scalars = 0;
        assert_eq!(apply_static(&imp, &e), Err(EngineStateRefusal::PerDraw { state: 0x3d, stage: None }), "a slot 0x0001ae70 leaves");
        imp.input_reads.scalars = 1 << 21 | 1 << 3;
        imp.engine_render_states = vec![b([2, 1, 0, 3])];
        assert_eq!(apply_static(&imp, &e), Err(EngineStateRefusal::PerDraw { state: 0x3d, stage: None }), "a draw's input");
        imp.engine_render_states = vec![b([6, 0, 0, 0])];
        assert!(matches!(apply_static(&imp, &e), Err(EngineStateRefusal::PerDraw { .. })), "a vector");
    }
}
