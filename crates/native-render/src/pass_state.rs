//! A pass implementation's render states (`ResolvedImplementation::render_states`,
//! applied on entry by `0x00018900`) as the native pass's pipeline state:
//! blend, colour writes, depth test and write, cull, alpha test
//! (docs/graphics/native-renderer.md, "Stage 3: transparents", order of
//! work 2). The values are the Xbox's (NV2A) and the slots build 5849's,
//! decoded by `gfx` as the D3D path decodes them.

use std::sync::OnceLock;

use gfx::state::{CompareFunc, DepthBufferMode, RenderState, RenderStateType as R};
use gfx::xdk::{HALO2_BUILD, RenderStateTable};
use tags::shader::RenderStateValue;

/// The native pipeline state of one pass implementation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PassState {
    /// `None`: no blending (the colour is replaced).
    pub blend: Option<wgpu::BlendState>,
    /// The render pass's blend constant, for the constant factors.
    pub blend_constant: wgpu::Color,
    pub color_writes: wgpu::ColorWrites,
    pub depth_write: bool,
    pub depth_compare: wgpu::CompareFunction,
    pub primitive: wgpu::PrimitiveState,
    /// Alpha test: discard unless `alpha <func> reference / 255`.
    pub alpha_test: Option<(wgpu::CompareFunction, u8)>,
}

/// Why an implementation's states keep it on the D3D path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StateRefusal {
    /// A state the pass depends on is not set by the implementation (it
    /// would inherit whatever the previous draw left).
    Unset(&'static str),
    /// A value `gfx` does not decode.
    Invalid { state: u8, value: u32 },
    /// Fixed-function fog (the native pass has none).
    Fog,
    /// The stencil test.
    Stencil,
    /// A depth bias.
    DepthBias,
    /// Layer 15 is translated with W-buffer depth.
    NonWDepth,
}

/// The pipeline state of an implementation with `states` applied on entry.
pub fn pass_state(states: &[RenderStateValue]) -> Result<PassState, StateRefusal> {
    decode(states, RenderState::default(), true)
}

fn decode(states: &[RenderStateValue], mut rs: RenderState, require_writes: bool) -> Result<PassState, StateRefusal> {
    let table = state_table();
    for s in states {
        let slot = u32::from(s.state);
        rs.set_guest(table, slot, s.value)
            .map_err(|_| StateRefusal::Invalid { state: s.state, value: s.value })?;
    }
    let has = |t: R| states.iter().any(|s| R::from_guest_index(table, u32::from(s.state)) == Some(t));
    if require_writes {
        // With depth disabled, ZWRITEENABLE has no effect.
        for (required, state, name) in [
            (true, R::AlphaBlendEnable, "ALPHABLENDENABLE"),
            (rs.blend_enable, R::SrcBlend, "SRCBLEND"),
            (rs.blend_enable, R::DestBlend, "DESTBLEND"),
            (rs.depth_mode != DepthBufferMode::Disabled, R::ZWriteEnable, "ZWRITEENABLE"),
        ] {
            if required && !has(state) {
                return Err(StateRefusal::Unset(name));
            }
        }
    }
    if rs.fog_enable {
        return Err(StateRefusal::Fog);
    }
    if rs.stencil.enable {
        return Err(StateRefusal::Stencil);
    }
    if rs.solid_offset_enable && (rs.depth_bias != 0.0 || rs.depth_bias_slope_scale != 0.0) {
        return Err(StateRefusal::DepthBias);
    }
    let (depth_write, depth_compare) = match rs.depth_mode {
        DepthBufferMode::Disabled => (false, wgpu::CompareFunction::Always),
        DepthBufferMode::Z | DepthBufferMode::W => (rs.depth_write, gfx::translate::compare_function(rs.depth_func)),
    };
    let alpha_test = (rs.alpha_test_enable && rs.alpha_func != CompareFunc::Always)
        .then(|| (gfx::translate::compare_function(rs.alpha_func), rs.alpha_ref));
    Ok(PassState {
        blend: gfx::translate::blend_state(&rs),
        blend_constant: gfx::translate::blend_constant(&rs),
        color_writes: gfx::translate::color_writes(rs.color_write),
        depth_write,
        depth_compare,
        primitive: gfx::translate::primitive_state(
            &rs,
            wgpu::PrimitiveTopology::TriangleList,
            gfx::translate::Capabilities::from_features(wgpu::Features::empty()),
        ),
        alpha_test,
    })
}

fn state_table() -> &'static RenderStateTable {
    static TABLE: OnceLock<RenderStateTable> = OnceLock::new();
    TABLE.get_or_init(|| RenderStateTable::for_build(HALO2_BUILD))
}

/// States layer 15 can leave to the incoming device state. Other states
/// keep the pass decoder's rules, including the explicit alpha-test
/// specialization and its fog, stencil and depth-bias refusals.
pub const TRANSPARENT_INHERITED: [R; 7] = [
    R::CullMode, R::FrontFace, R::ZFunc, R::ZWriteEnable,
    R::AlphaBlendEnable, R::SrcBlend, R::DestBlend,
];

/// A layer-15 implementation's writes and its load-time shader state.
/// At draw time explicit writes override the run's incoming state.
#[derive(Clone, Debug, PartialEq)]
pub struct TransparentState {
    writes: Vec<RenderStateValue>,
    depth_clamp: Option<bool>,
}

impl TransparentState {
    pub fn new(writes: &[RenderStateValue]) -> Result<Self, StateRefusal> {
        decode(writes, RenderState::default(), false)?;
        let last = |t: R| writes.iter().rev().find(|s| {
            R::from_guest_index(state_table(), u32::from(s.state)) == Some(t)
        }).map(|s| s.value);
        if let Some(mode) = last(R::ZEnable)
            && mode != DepthBufferMode::W.xbox_value()
        {
            return Err(StateRefusal::NonWDepth);
        }
        let depth_clamp = last(R::DepthClipControl).map(gfx::state::clamps_depth);
        Ok(Self { writes: writes.to_vec(), depth_clamp })
    }

    pub fn resolve(&self, incoming: &RenderState) -> PassState {
        let inherited = RenderState {
            cull_mode: incoming.cull_mode,
            front_face: incoming.front_face,
            depth_mode: incoming.depth_mode,
            depth_func: incoming.depth_func,
            depth_write: incoming.depth_write,
            blend_enable: incoming.blend_enable,
            src_blend: incoming.src_blend,
            dst_blend: incoming.dst_blend,
            ..Default::default()
        };
        decode(&self.writes, inherited, false).expect("validated transparent state writes")
    }

    /// The clip range comes from the device. An explicit clamp write
    /// in the implementation overrides the device's current clamp.
    pub(crate) fn view(&self, view: &crate::program::ViewInputs) -> crate::program::ViewInputs {
        let mut view = *view;
        if let Some(clamp) = self.depth_clamp {
            let range = view.w_clip.map_or([0.0, view.z_scale], |(range, _)| range);
            view.w_clip = Some((range, clamp));
        }
        view
    }
}

/// Hashable state that changes a render pipeline. The blend constant
/// and alpha reference are supplied separately.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PipelineState {
    pub blend: Option<wgpu::BlendState>,
    pub color_writes: wgpu::ColorWrites,
    pub depth_write: bool,
    pub depth_compare: wgpu::CompareFunction,
    pub primitive: wgpu::PrimitiveState,
}

impl From<PassState> for PipelineState {
    fn from(s: PassState) -> Self {
        Self { blend: s.blend, color_writes: s.color_writes, depth_write: s.depth_write,
            depth_compare: s.depth_compare, primitive: s.primitive }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfx::state::{BlendFactor, BlendOp, CullMode, FrontFace, RenderStateType};

    fn slot(t: RenderStateType) -> u8 {
        let table = RenderStateTable::for_build(HALO2_BUILD);
        (0..=255u8)
            .find(|&s| RenderStateType::from_guest_index(&table, u32::from(s)) == Some(t))
            .expect("a 5849 slot")
    }

    fn v(t: RenderStateType, value: u32) -> RenderStateValue {
        RenderStateValue { state: slot(t), value }
    }

    #[test]
    fn an_alpha_blended_pass_without_depth_writes() {
        let s = pass_state(&[
            v(RenderStateType::AlphaBlendEnable, 1),
            v(RenderStateType::SrcBlend, BlendFactor::SrcAlpha.xbox_value()),
            v(RenderStateType::DestBlend, BlendFactor::InvSrcAlpha.xbox_value()),
            v(RenderStateType::ZWriteEnable, 0),
            v(RenderStateType::CullMode, CullMode::None.xbox_value()),
        ])
        .unwrap();
        // D3D8 blends alpha with the colour's factors.
        let c = s.blend.unwrap();
        assert_eq!(c.color, wgpu::BlendState::ALPHA_BLENDING.color);
        assert_eq!(c.alpha, c.color);
        assert!(!s.depth_write);
        assert_eq!(s.depth_compare, wgpu::CompareFunction::LessEqual);
        assert_eq!(s.primitive.cull_mode, None);
        assert_eq!(s.alpha_test, None);
        assert_eq!(s.color_writes, wgpu::ColorWrites::ALL);
    }

    #[test]
    fn an_additive_pass_and_its_op() {
        let s = pass_state(&[
            v(RenderStateType::AlphaBlendEnable, 1),
            v(RenderStateType::SrcBlend, BlendFactor::One.xbox_value()),
            v(RenderStateType::DestBlend, BlendFactor::One.xbox_value()),
            v(RenderStateType::BlendOp, BlendOp::ReverseSubtract.xbox_value()),
            v(RenderStateType::ZWriteEnable, 0),
        ])
        .unwrap();
        let c = s.blend.unwrap().color;
        assert_eq!(
            (c.src_factor, c.dst_factor, c.operation),
            (wgpu::BlendFactor::One, wgpu::BlendFactor::One, wgpu::BlendOperation::ReverseSubtract)
        );
    }

    #[test]
    fn an_opaque_alpha_tested_pass_writes_depth() {
        let s = pass_state(&[
            v(RenderStateType::AlphaBlendEnable, 0),
            v(RenderStateType::ZWriteEnable, 1),
            v(RenderStateType::AlphaTestEnable, 1),
            v(RenderStateType::AlphaFunc, CompareFunc::Greater.xbox_value()),
            v(RenderStateType::AlphaRef, 0x7f),
        ])
        .unwrap();
        assert_eq!(s.blend, None);
        assert!(s.depth_write);
        assert_eq!(s.alpha_test, Some((wgpu::CompareFunction::Greater, 0x7f)));
    }

    #[test]
    fn a_transparent_pass_overrides_only_the_states_it_sets() {
        let pass = TransparentState::new(&[
            v(R::DestBlend, BlendFactor::InvSrcAlpha.xbox_value()),
            v(R::FrontFace, FrontFace::Ccw.xbox_value()),
        ]).unwrap();
        let incoming = RenderState {
            depth_mode: DepthBufferMode::W,
            depth_func: CompareFunc::Equal,
            depth_write: false,
            blend_enable: true,
            src_blend: BlendFactor::SrcAlpha,
            dst_blend: BlendFactor::Zero,
            cull_mode: CullMode::Cw,
            front_face: FrontFace::Cw,
            ..Default::default()
        };
        let state = pass.resolve(&incoming);
        assert_eq!(state.primitive.front_face, wgpu::FrontFace::Ccw);
        assert_eq!(state.primitive.cull_mode, Some(wgpu::Face::Back));
        assert_eq!(state.depth_compare, wgpu::CompareFunction::Equal);
        assert!(!state.depth_write);
        assert_eq!(state.blend.unwrap().color, wgpu::BlendState::ALPHA_BLENDING.color);
        let state = pass.resolve(&RenderState { blend_enable: false, cull_mode: CullMode::None, ..incoming });
        assert_eq!(state.blend, None);
        assert_eq!(state.primitive.cull_mode, None);
    }

    #[test]
    fn transparent_depth_clip_inherits_the_range_and_only_overrides_an_explicit_clamp() {
        let view = crate::program::ViewInputs {
            constants: [[0.0; 4]; crate::program::VIEW_CONSTANTS],
            guest_size: (64, 64), size: (64, 64), z_scale: 1024.0,
            w_clip: Some(([16.0, 512.0], true)),
        };
        let inherited = TransparentState::new(&[]).unwrap();
        let explicit = TransparentState::new(&[v(R::DepthClipControl, 0)]).unwrap();
        assert_eq!(inherited.view(&view).w_clip, view.w_clip);
        assert_eq!(explicit.view(&view).w_clip, Some(([16.0, 512.0], false)));
        for mode in [DepthBufferMode::Z, DepthBufferMode::Disabled] {
            assert_eq!(TransparentState::new(&[v(R::ZEnable, mode.xbox_value())]), Err(StateRefusal::NonWDepth));
        }
    }

    #[test]
    fn states_left_to_the_previous_draw_or_outside_the_pass_say_why() {
        assert_eq!(pass_state(&[]), Err(StateRefusal::Unset("ALPHABLENDENABLE")));
        assert_eq!(
            pass_state(&[v(RenderStateType::AlphaBlendEnable, 1), v(RenderStateType::ZWriteEnable, 0)]),
            Err(StateRefusal::Unset("SRCBLEND"))
        );
        assert_eq!(
            pass_state(&[v(RenderStateType::AlphaBlendEnable, 0)]),
            Err(StateRefusal::Unset("ZWRITEENABLE"))
        );
        let base = [v(RenderStateType::AlphaBlendEnable, 0), v(RenderStateType::ZWriteEnable, 0)];
        let with = |extra: RenderStateValue| {
            let mut s = base.to_vec();
            s.push(extra);
            pass_state(&s)
        };
        // A screen effect's: the depth buffer off, its write never set.
        let screen = pass_state(&[v(RenderStateType::AlphaBlendEnable, 0), v(RenderStateType::ZEnable, 0)]).unwrap();
        assert_eq!((screen.depth_write, screen.depth_compare), (false, wgpu::CompareFunction::Always));
        assert_eq!(
            pass_state(&[v(RenderStateType::AlphaBlendEnable, 0), v(RenderStateType::ZEnable, 1)]),
            Err(StateRefusal::Unset("ZWRITEENABLE")),
            "the depth buffer on"
        );
        assert_eq!(with(v(RenderStateType::FogEnable, 1)), Err(StateRefusal::Fog));
        assert_eq!(with(v(RenderStateType::StencilEnable, 1)), Err(StateRefusal::Stencil));
        let bad = v(RenderStateType::SrcBlend, 0x1234);
        assert_eq!(with(bad), Err(StateRefusal::Invalid { state: bad.state, value: 0x1234 }));
    }
}
