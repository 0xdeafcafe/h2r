//! A pass implementation's render states (`ResolvedImplementation::render_states`,
//! applied on entry by `0x00018900`) as the native pass's pipeline state:
//! blend, colour writes, depth test and write, cull, alpha test
//! (docs/graphics/native-renderer.md, "Stage 3: transparents", order of
//! work 2). The values are the Xbox's (NV2A) and the slots build 5849's,
//! decoded by `gfx` as the D3D path decodes them.
//!
//! What a draw inherits from the draws before it is not modelled: the
//! states not set here take `gfx::RenderState`'s defaults, so the states a
//! transparent pass depends on (blend enable, its factors, depth write)
//! must be set by the implementation itself, or it stays on the D3D path
//! (S: that an implementation's restore states bring the device back to
//! those defaults between draws is not traced).

use gfx::state::{CompareFunc, DepthBufferMode, RenderState, RenderStateType};
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
}

/// The pipeline state of an implementation with `states` applied on entry.
pub fn pass_state(states: &[RenderStateValue]) -> Result<PassState, StateRefusal> {
    let table = RenderStateTable::for_build(HALO2_BUILD);
    let mut rs = RenderState::default();
    let mut set = Vec::with_capacity(states.len());
    for s in states {
        let slot = u32::from(s.state);
        rs.set_guest(&table, slot, s.value)
            .map_err(|_| StateRefusal::Invalid { state: s.state, value: s.value })?;
        if let Some(t) = RenderStateType::from_guest_index(&table, slot) {
            set.push(t);
        }
    }
    let has = |t: RenderStateType| set.contains(&t);
    if !has(RenderStateType::AlphaBlendEnable) {
        return Err(StateRefusal::Unset("ALPHABLENDENABLE"));
    }
    if rs.blend_enable && !has(RenderStateType::SrcBlend) {
        return Err(StateRefusal::Unset("SRCBLEND"));
    }
    if rs.blend_enable && !has(RenderStateType::DestBlend) {
        return Err(StateRefusal::Unset("DESTBLEND"));
    }
    // A pass that turns the depth buffer off (`ZENABLE` 0, the screen
    // effects') neither tests nor writes it, whatever ZWRITEENABLE holds.
    let depth_off = has(RenderStateType::ZEnable) && rs.depth_mode == DepthBufferMode::Disabled;
    if !has(RenderStateType::ZWriteEnable) && !depth_off {
        return Err(StateRefusal::Unset("ZWRITEENABLE"));
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

#[cfg(test)]
mod tests {
    use super::*;
    use gfx::state::{BlendFactor, BlendOp, CullMode};

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
