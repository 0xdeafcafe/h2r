//! A pass implementation's render states (`ResolvedImplementation::render_states`,
//! applied on entry by `0x00018900`) as the native pass's pipeline state:
//! blend, colour writes, depth test and write, cull and alpha test.
//! The values are the Xbox's (NV2A) and the slots build 5849's,
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
    /// The stencil the implementation sets itself, whole ([`own_stencil`]):
    /// its pipeline and reference replace the device's at the layer call
    /// ([`PassState::stencil_under`]). `None`: the pass draws under the
    /// device's (`fog_gpu::layer_stencil`).
    pub stencil: Option<gfx::state::StencilState>,
    /// The polygon offset the implementation sets ([`DepthOffset`]):
    /// `None` when it sets none, or sets it to zero. A builder that draws
    /// it adds it to the fragment's W depth ([`DepthOffset::apply`]); one
    /// that does not refuses it ([`PassState::no_depth_offset`]).
    pub depth_offset: Option<DepthOffset>,
}

/// A pass's polygon offset (`SOLIDOFFSETENABLE`, `POLYGONOFFSETZOFFSET`,
/// `POLYGONOFFSETZSLOPESCALE`), in the depth buffer's units, as the NV2A
/// adds it to the stored depth: `units` plus `slope` times the fragment's
/// depth slope. The native programs write W depth themselves
/// (`PixelOptions::w_buffer`), so the offset is the fragment's
/// (`PixelUniforms::depth` y and z), not the pipeline's depth bias.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DepthOffset {
    pub units: f32,
    pub slope: f32,
}

impl DepthOffset {
    /// `pixel` with this offset: the frame's depth scale is
    /// `program::frame_pixel`'s, which keeps the offset set here.
    pub fn apply(offset: Option<Self>, pixel: &mut shaders::psh::PixelUniforms) {
        let o = offset.unwrap_or(DepthOffset { units: 0.0, slope: 0.0 });
        pixel.depth[1] = o.units;
        pixel.depth[2] = o.slope;
    }
}

impl PassState {
    /// The state for a native pass that does not draw a polygon offset:
    /// a pass that sets one is refused ([`StateRefusal::DepthBias`]), not
    /// drawn without it.
    pub fn no_depth_offset(self) -> Result<Self, StateRefusal> {
        match self.depth_offset {
            Some(_) => Err(StateRefusal::DepthBias),
            None => Ok(self),
        }
    }

    /// The stencil the pass draws with where the device holds `device`:
    /// its own when it sets one, else the device's.
    pub fn stencil_under(&self, device: &gfx::state::StencilState) -> gfx::state::StencilState {
        self.stencil.unwrap_or(*device)
    }

    /// The stencil reference the pass's draws set over the device's: its
    /// own stencil's when it turns the test on. `None`: the device's (a
    /// pass with the test off reads none).
    pub fn own_stencil_reference(&self) -> Option<u32> {
        self.stencil.filter(|s| s.enable).map(|s| u32::from(s.reference))
    }

    /// Whether the pass writes depth, or the stencil it sets itself
    /// (`fog_gpu::stencil_writes`).
    pub fn writes_depth_stencil(&self) -> bool {
        self.depth_write || self.stencil.is_some_and(|s| crate::fog_gpu::stencil_writes(&s))
    }

    /// Whether a live draw's stencil is the one this pass draws with where
    /// the layer's own is off (stencil mode 0, the lights' layers): the
    /// pass's own, or the test off.
    pub fn stencil_matches_off_layer(&self, live: &gfx::state::StencilState) -> bool {
        match self.stencil {
            Some(own) if own.enable => own == *live,
            _ => !live.enable,
        }
    }

    /// The state for a native pass that draws only under the device's
    /// stencil: a pass that sets its own is refused
    /// ([`StateRefusal::Stencil`]), not drawn under the device's.
    pub fn device_stencil_only(self) -> Result<Self, StateRefusal> {
        match self.stencil {
            Some(_) => Err(StateRefusal::Stencil),
            None => Ok(self),
        }
    }
}

/// Why an implementation's states keep it on the D3D path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StateRefusal {
    /// A state the pass depends on is not set by the implementation (it
    /// would inherit whatever the previous draw left).
    Unset(&'static str),
    /// A value `gfx` does not decode.
    Invalid { state: u8, value: u32 },
    /// The stencil test, set by the pass itself, where the native pass
    /// that would draw it takes only the device's: the screen, shadow,
    /// water and layer 1 and 3 geometry passes. The world layers', the
    /// lights' and the decorators' passes draw their own
    /// ([`PassState::stencil`]). No `shad` pass on the retail maps sets
    /// one (docs/graphics/e3-lighting.md, "Stencil: every use, named").
    Stencil,
    /// A polygon offset ([`PassState::depth_offset`]) where the native
    /// pass that would draw it does not, or one that is not finite.
    DepthBias,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StateSource {
    Implementation,
    Geometry,
    LiveObject,
}

/// The pipeline state of an implementation with `states` applied on entry.
pub fn pass_state(states: &[RenderStateValue]) -> Result<PassState, StateRefusal> {
    decode_states(states, StateSource::Implementation)
}

/// Geometry with an explicit empty colour mask only updates depth and stencil.
pub(crate) fn geometry_pass_state(states: &[RenderStateValue]) -> Result<PassState, StateRefusal> {
    decode_states(states, StateSource::Geometry)
}

/// An object program compiles its alpha test at load. At issue the object
/// planner resolves its blend, depth and stencil from the live device.
pub(crate) fn object_program_state(states: &[RenderStateValue]) -> Result<PassState, StateRefusal> {
    decode_states(states, StateSource::LiveObject)
}

fn decode_states(states: &[RenderStateValue], source: StateSource) -> Result<PassState, StateRefusal> {
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
    let live_object_state = source == StateSource::LiveObject;
    let masked = source == StateSource::Geometry && has(RenderStateType::ColorWriteEnable)
        && gfx::translate::color_writes(rs.color_write).is_empty();
    if masked {
        rs.blend_enable = false;
    }
    if !live_object_state && !masked && !has(RenderStateType::AlphaBlendEnable) {
        return Err(StateRefusal::Unset("ALPHABLENDENABLE"));
    }
    if !live_object_state && rs.blend_enable && !has(RenderStateType::SrcBlend) {
        return Err(StateRefusal::Unset("SRCBLEND"));
    }
    if !live_object_state && rs.blend_enable && !has(RenderStateType::DestBlend) {
        return Err(StateRefusal::Unset("DESTBLEND"));
    }
    // A pass that turns the depth buffer off (`ZENABLE` 0, the screen
    // effects') neither tests nor writes it, whatever ZWRITEENABLE holds.
    let depth_off = has(RenderStateType::ZEnable) && rs.depth_mode == DepthBufferMode::Disabled;
    if !live_object_state && !has(RenderStateType::ZWriteEnable) && !depth_off {
        return Err(StateRefusal::Unset("ZWRITEENABLE"));
    }
    // The fog words (`FOGENABLE` and the fog unit's, `fog_gpu::FogWords`)
    // are not the pipeline's: the NV2A applies fog only where a combiner
    // reads the fog register, and every native builder programs its own
    // final combiner. A builder that draws a fog read feeds the register
    // per view (the world layers' `FogWords`, the screen passes'
    // `PostRaster::fog`); the others refuse the read
    // (`program::pixel_parity`).
    let stencil = if live_object_state { None } else { own_stencil(&rs, &has)? };
    let depth_offset = depth_offset(&rs, &has)?;
    if live_object_state && depth_offset.is_some() {
        // An object's offset is resolved at issue (`object_state`), which
        // draws none.
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
        stencil,
        depth_offset,
    })
}

/// The polygon offset of the fill mode in effect, when the implementation
/// turns it on with a non-zero offset. Both values must then be set by the
/// pass itself (`Unset`), or it would draw with the previous draw's.
fn depth_offset(
    rs: &RenderState,
    has: &dyn Fn(RenderStateType) -> bool,
) -> Result<Option<DepthOffset>, StateRefusal> {
    use gfx::state::FillMode;
    use RenderStateType as R;
    let enabled = match rs.fill_mode {
        FillMode::Point => rs.point_offset_enable,
        FillMode::Wireframe => rs.wireframe_offset_enable,
        FillMode::Solid => rs.solid_offset_enable,
    };
    if !enabled {
        return Ok(None);
    }
    if !has(R::PolygonOffsetZOffset) {
        return Err(StateRefusal::Unset("POLYGONOFFSETZOFFSET"));
    }
    if !has(R::PolygonOffsetZSlopeScale) {
        return Err(StateRefusal::Unset("POLYGONOFFSETZSLOPESCALE"));
    }
    if !rs.depth_bias.is_finite() || !rs.depth_bias_slope_scale.is_finite() {
        return Err(StateRefusal::DepthBias);
    }
    Ok((rs.depth_bias != 0.0 || rs.depth_bias_slope_scale != 0.0)
        .then_some(DepthOffset { units: rs.depth_bias, slope: rs.depth_bias_slope_scale }))
}

/// The stencil words of an implementation that sets any: `STENCILENABLE`
/// 0 turns the test off whatever the device holds; 1 takes the test whole
/// from the pass, so every word it reads must be set there, or the pass
/// would inherit the previous draw's (`Unset`). A word set without
/// `STENCILENABLE` is refused too: the device's enable decides whether it
/// is read.
fn own_stencil(
    rs: &RenderState,
    has: &dyn Fn(RenderStateType) -> bool,
) -> Result<Option<gfx::state::StencilState>, StateRefusal> {
    use RenderStateType as R;
    const WORDS: [(RenderStateType, &str); 7] = [
        (R::StencilFunc, "STENCILFUNC"),
        (R::StencilRef, "STENCILREF"),
        (R::StencilMask, "STENCILMASK"),
        (R::StencilWriteMask, "STENCILWRITEMASK"),
        (R::StencilFail, "STENCILFAIL"),
        (R::StencilZFail, "STENCILZFAIL"),
        (R::StencilPass, "STENCILPASS"),
    ];
    if !has(R::StencilEnable) {
        return match WORDS.iter().any(|&(t, _)| has(t)) {
            true => Err(StateRefusal::Unset("STENCILENABLE")),
            false => Ok(None),
        };
    }
    if !rs.stencil.enable {
        return Ok(Some(gfx::state::StencilState { enable: false, ..Default::default() }));
    }
    if let Some(&(_, name)) = WORDS.iter().find(|&&(t, _)| !has(t)) {
        return Err(StateRefusal::Unset(name));
    }
    Ok(Some(rs.stencil))
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
    fn geometry_without_colour_writes_can_inherit_blending() {
        use RenderStateType as R;
        let mut states = vec![v(R::ColorWriteEnable, 0), v(R::ZWriteEnable, 1)];
        for enable in [None, Some(1)] {
            if let Some(value) = enable { states.push(v(R::AlphaBlendEnable, value)); }
            let state = geometry_pass_state(&states).unwrap();
            assert!(state.color_writes.is_empty() && state.depth_write);
            assert_eq!(state.blend, None);
        }
        states.push(v(R::ColorWriteEnable, 1));
        assert_eq!(geometry_pass_state(&states), Err(StateRefusal::Unset("SRCBLEND")));
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
    fn a_pass_that_sets_its_own_stencil_draws_it_whole() {
        use gfx::state::{StencilOp, StencilState};
        use RenderStateType as R;
        let base = [v(R::AlphaBlendEnable, 0), v(R::ZWriteEnable, 0)];
        assert_eq!(pass_state(&base).unwrap().stencil, None, "no stencil word: the device's");
        let device = StencilState { enable: true, reference: 9, ..Default::default() };
        assert_eq!(pass_state(&base).unwrap().stencil_under(&device), device);

        let mut off = base.to_vec();
        off.push(v(R::StencilEnable, 0));
        let off = pass_state(&off).unwrap();
        assert_eq!(off.stencil, Some(StencilState { enable: false, ..Default::default() }));
        assert!(!off.stencil_under(&device).enable, "the pass's 0 turns the device's test off");

        let mut own = base.to_vec();
        own.extend([
            v(R::StencilEnable, 1),
            v(R::StencilFunc, CompareFunc::Equal.xbox_value()),
            v(R::StencilRef, 0x80),
            v(R::StencilMask, 0xf0),
            v(R::StencilWriteMask, 0),
            v(R::StencilFail, StencilOp::Keep.xbox_value()),
            v(R::StencilZFail, StencilOp::Keep.xbox_value()),
            v(R::StencilPass, StencilOp::Keep.xbox_value()),
        ]);
        let s = pass_state(&own).unwrap();
        let want = StencilState {
            enable: true,
            func: CompareFunc::Equal,
            reference: 0x80,
            read_mask: 0xf0,
            write_mask: 0,
            fail: StencilOp::Keep,
            depth_fail: StencilOp::Keep,
            pass: StencilOp::Keep,
        };
        assert_eq!(s.stencil, Some(want));
        assert_eq!(s.stencil_under(&device), want, "the pass's own over the device's");
        assert_eq!(s.own_stencil_reference(), Some(0x80));
        assert_eq!(off.own_stencil_reference(), None, "the test off reads no reference");
        assert_eq!(pass_state(&base).unwrap().own_stencil_reference(), None, "the device's");
        for n in 1..own.len() - base.len() {
            let mut missing = own.clone();
            missing.remove(base.len() + n);
            assert!(matches!(pass_state(&missing), Err(StateRefusal::Unset(_))), "word {n} unset");
        }
        assert_eq!(object_program_state(&own).unwrap().stencil, None, "an object resolves it at issue");
    }

    #[test]
    fn a_polygon_offset_is_the_pass_state_and_set_whole() {
        use RenderStateType as R;
        let base = [v(R::AlphaBlendEnable, 0), v(R::ZWriteEnable, 0)];
        let with = |extra: &[RenderStateValue]| {
            let mut s = base.to_vec();
            s.extend_from_slice(extra);
            pass_state(&s)
        };
        let on = v(R::SolidOffsetEnable, 1);
        let units = v(R::PolygonOffsetZOffset, (-4.0f32).to_bits());
        let slope = v(R::PolygonOffsetZSlopeScale, 2.0f32.to_bits());
        let s = with(&[on, units, slope]).unwrap();
        assert_eq!(s.depth_offset, Some(DepthOffset { units: -4.0, slope: 2.0 }));
        assert_eq!(s.no_depth_offset(), Err(StateRefusal::DepthBias), "a builder that does not draw it says so");
        let mut pixel = shaders::psh::PixelUniforms::default();
        DepthOffset::apply(s.depth_offset, &mut pixel);
        assert_eq!((pixel.depth[1], pixel.depth[2]), (-4.0, 2.0));

        assert_eq!(with(&[units, slope]).unwrap().depth_offset, None, "the enable off: no offset");
        let zero = v(R::PolygonOffsetZOffset, 0);
        let flat = v(R::PolygonOffsetZSlopeScale, 0);
        assert_eq!(with(&[on, zero, flat]).unwrap().depth_offset, None, "a zero offset is none");
        assert_eq!(with(&[on, slope]), Err(StateRefusal::Unset("POLYGONOFFSETZOFFSET")));
        assert_eq!(with(&[on, units]), Err(StateRefusal::Unset("POLYGONOFFSETZSLOPESCALE")));
        let nan = v(R::PolygonOffsetZOffset, f32::NAN.to_bits());
        assert_eq!(with(&[on, nan, slope]), Err(StateRefusal::DepthBias));
        assert_eq!(
            object_program_state(&[on, units, slope]),
            Err(StateRefusal::DepthBias),
            "an object's offset is the live device's"
        );
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
        let fog = with(v(RenderStateType::FogEnable, 1)).expect("fog is the combiner's, not the pipeline's");
        assert_eq!(fog, pass_state(&base).unwrap(), "the fog words change no pipeline state");
        assert_eq!(with(v(RenderStateType::StencilEnable, 1)), Err(StateRefusal::Unset("STENCILFUNC")));
        assert_eq!(with(v(RenderStateType::StencilRef, 3)), Err(StateRefusal::Unset("STENCILENABLE")));
        let bad = v(RenderStateType::SrcBlend, 0x1234);
        assert_eq!(with(bad), Err(StateRefusal::Invalid { state: bad.state, value: 0x1234 }));
    }
}
