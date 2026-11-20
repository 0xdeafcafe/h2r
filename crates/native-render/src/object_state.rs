//! The render state an object's layer-3 draw takes, field by field
//! (docs/graphics/native-renderer.md, "Object programs"): what its
//! implementation sets on entry (`0x00018900`), and for every other field
//! the value the device holds when layer 3 is drawn, read from the guest's
//! render-state array at the layer-3 call ([`EntryStates`]). A field the
//! draw depends on that neither gives (the slot does not read, or holds a
//! value outside its enum) refuses the draw, as does a state the native
//! pass does not reproduce: blending, a depth bias, a fill mode other than solid,
//! Z-buffering set by the implementation (the programs write w; the
//! device's own mode, `SET_CONTROL0`, the host checks), or an alpha test
//! other than the one the program was compiled with. Nothing is taken
//! from defaults.
//!
//! S: the layer's earlier draws leave the fields an implementation does
//! not set as they were at the layer-3 call (as the world's inherited
//! cull, `look::WorldPass::inherits_cull`).

use gfx::state::{CompareFunc, DepthBufferMode, FillMode, RenderState, RenderStateType as R, StencilState};
use gfx::xdk::{HALO2_BUILD, RenderStateTable};
use tags::shader::RenderStateValue;

/// The fields read at the layer-3 call: every one a draw's pipeline, its
/// alpha test or its W clip depends on, and those whose being on refuses
/// it.
pub const FIELDS: [R; 29] = [
    R::ZEnable,
    R::ZFunc,
    R::ZWriteEnable,
    R::AlphaTestEnable,
    R::AlphaFunc,
    R::AlphaRef,
    R::AlphaBlendEnable,
    R::SrcBlend,
    R::DestBlend,
    R::BlendOp,
    R::BlendColor,
    R::ColorWriteEnable,
    R::CullMode,
    R::FrontFace,
    R::FillMode,
    R::BackFillMode,
    R::StencilEnable,
    R::StencilFunc,
    R::StencilRef,
    R::StencilMask,
    R::StencilWriteMask,
    R::StencilFail,
    R::StencilZFail,
    R::StencilPass,
    R::FogEnable,
    R::SolidOffsetEnable,
    R::PolygonOffsetZOffset,
    R::PolygonOffsetZSlopeScale,
    R::DepthClipControl,
];

/// Fields every draw depends on; the rest only when one of these turns
/// them on (alpha function and reference with the alpha test, the
/// offsets with the solid offset), or not at all because being on
/// refuses the draw (blend, stencil). Fog is resolved separately into the shader uniforms.
const REQUIRED: [R; 14] = [
    R::ZEnable,
    R::ZFunc,
    R::ZWriteEnable,
    R::AlphaTestEnable,
    R::AlphaBlendEnable,
    R::ColorWriteEnable,
    R::CullMode,
    R::FrontFace,
    R::FillMode,
    R::BackFillMode,
    R::StencilEnable,
    R::FogEnable,
    R::SolidOffsetEnable,
    R::DepthClipControl,
];

/// The raw values of [`FIELDS`] the device held at the layer-3 call (a
/// field that did not read is absent).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EntryStates(pub Vec<(R, u32)>);

impl EntryStates {
    /// Reads each of [`FIELDS`] through `read`.
    pub fn read(read: impl Fn(R) -> Option<u32>) -> Self {
        Self(FIELDS.iter().filter_map(|&t| Some((t, read(t)?))).collect())
    }

    pub fn get(&self, t: R) -> Option<u32> {
        self.0.iter().find(|(s, _)| *s == t).map(|&(_, v)| v)
    }
}

/// What one draw is issued with: its pipeline's primitive, depth and
/// colour writes, its W clip's clamp (`D3DRS_DEPTHCLIPCONTROL`), and its
/// alpha test (checked against the program's).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DrawState {
    pub primitive: wgpu::PrimitiveState,
    pub depth_write: bool,
    pub stencil: StencilState,
    pub depth_compare: wgpu::CompareFunction,
    pub color_writes: wgpu::ColorWrites,
    pub w_clamp: bool,
    pub alpha_test: Option<(wgpu::CompareFunction, u8)>,
}

/// The state a draw of an implementation setting `own` takes over `entry`,
/// or why it is refused. `compiled`: the alpha test its program was
/// compiled with.
pub fn resolve(own: &[RenderStateValue], entry: &EntryStates, compiled: Option<(wgpu::CompareFunction, u8)>) -> Result<DrawState, String> {
    resolve_states(own, entry, compiled, false)
}

/// The third-person scene path supplies the fog uniforms and handles a
/// zero-reference stencil test. First-person callers use `resolve`.
pub fn resolve_scene(own: &[RenderStateValue], entry: &EntryStates, compiled: Option<(wgpu::CompareFunction, u8)>) -> Result<DrawState, String> {
    resolve_states(own, entry, compiled, true)
}

fn resolve_states(
    own: &[RenderStateValue], entry: &EntryStates, compiled: Option<(wgpu::CompareFunction, u8)>,
    scene: bool,
) -> Result<DrawState, String> {
    let table = RenderStateTable::for_build(HALO2_BUILD);
    let mut rs = RenderState::default();
    let mut known: Vec<R> = Vec::new();
    let set: Vec<R> = own.iter().filter_map(|s| R::from_guest_index(&table, u32::from(s.state))).collect();
    for &t in &FIELDS {
        if set.contains(&t) {
            continue;
        }
        if let Some(v) = entry.get(t)
            && rs.set_raw(t, v).is_ok()
        {
            known.push(t);
        }
    }
    for s in own {
        rs.set_guest(&table, u32::from(s.state), s.value)
            .map_err(|_| format!("its render state slot {} holds {:#x}, which does not decode", s.state, s.value))?;
    }
    let sets_z = set.contains(&R::ZEnable);
    known.extend(set);
    let need = |t: R| -> Result<(), String> {
        if known.contains(&t) {
            Ok(())
        } else {
            Err(format!("{t:?} is inherited and the device's does not read or decode"))
        }
    };
    for t in REQUIRED {
        need(t)?;
    }
    if rs.blend_enable {
        return Err("it blends (ALPHABLENDENABLE)".into());
    }
    if rs.fog_enable && !scene {
        return Err("fog is on (FOGENABLE)".into());
    }
    if rs.stencil.enable {
        if !scene {
            return Err("the stencil test is on (STENCILENABLE)".into());
        }
        for field in [R::StencilFunc, R::StencilRef, R::StencilMask, R::StencilWriteMask,
            R::StencilFail, R::StencilZFail, R::StencilPass]
        {
            need(field)?;
        }
        // The external bundle encoder uses the render pass's default ref.
        if rs.stencil.reference != 0 {
            return Err("the stencil reference is nonzero".into());
        }
    }
    if rs.solid_offset_enable {
        need(R::PolygonOffsetZOffset)?;
        need(R::PolygonOffsetZSlopeScale)?;
        if rs.depth_bias != 0.0 || rs.depth_bias_slope_scale != 0.0 {
            return Err("a depth bias (SOLIDOFFSETENABLE)".into());
        }
    }
    if rs.fill_mode != FillMode::Solid || rs.back_fill_mode != FillMode::Solid {
        return Err(format!("fill mode {:?}/{:?}", rs.fill_mode, rs.back_fill_mode));
    }
    let (depth_write, depth_compare) = match rs.depth_mode {
        DepthBufferMode::Disabled => (false, wgpu::CompareFunction::Always),
        // Inherited, W or Z is the device's (`SET_CONTROL0`, which the
        // host checks at the layer-3 call); set by the implementation, Z
        // is Z-buffered.
        DepthBufferMode::Z if sets_z => return Err("Z-buffered (its ZENABLE): the programs write w".into()),
        DepthBufferMode::W | DepthBufferMode::Z => (rs.depth_write, gfx::translate::compare_function(rs.depth_func)),
    };
    let alpha_test = if rs.alpha_test_enable {
        need(R::AlphaFunc)?;
        need(R::AlphaRef)?;
        (rs.alpha_func != CompareFunc::Always).then(|| (gfx::translate::compare_function(rs.alpha_func), rs.alpha_ref))
    } else {
        None
    };
    if alpha_test != compiled {
        return Err(format!("its alpha test {alpha_test:?} is not the one its program was compiled with ({compiled:?})"));
    }
    Ok(DrawState {
        primitive: gfx::translate::primitive_state(
            &rs,
            wgpu::PrimitiveTopology::TriangleList,
            gfx::translate::Capabilities::from_features(wgpu::Features::empty()),
        ),
        depth_write,
        stencil: if rs.stencil.enable { rs.stencil } else { StencilState::default() },
        depth_compare,
        color_writes: gfx::translate::color_writes(rs.color_write),
        w_clamp: rs.depth_clamp,
        alpha_test,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfx::state::{CullMode, FrontFace};

    fn slot(t: R) -> u8 {
        let table = RenderStateTable::for_build(HALO2_BUILD);
        (0..=255u8).find(|&s| R::from_guest_index(&table, u32::from(s)) == Some(t)).expect("a slot")
    }

    fn own(states: &[(R, u32)]) -> Vec<RenderStateValue> {
        states.iter().map(|&(t, value)| RenderStateValue { state: slot(t), value }).collect()
    }

    /// The device at a layer-3 call: W-buffered, depth LessEqual written,
    /// all colour channels, `CULL_CW` with front face `CW`, nothing else on.
    fn entry() -> EntryStates {
        EntryStates::read(|t| {
            Some(match t {
                R::ZEnable => DepthBufferMode::W.xbox_value(),
                R::ZFunc => CompareFunc::LessEqual.xbox_value(),
                R::ZWriteEnable => 1,
                R::ColorWriteEnable => 0x0101_0101,
                R::CullMode => CullMode::Cw.xbox_value(),
                R::FrontFace => FrontFace::Cw.xbox_value(),
                R::FillMode | R::BackFillMode => FillMode::Solid.xbox_value(),
                R::AlphaFunc => CompareFunc::Always.xbox_value(),
                R::StencilFunc => CompareFunc::Always.xbox_value(),
                R::SrcBlend => 1,
                R::DestBlend => 0,
                R::BlendOp => 0x8006,
                R::StencilFail | R::StencilZFail | R::StencilPass => 0x1e00,
                _ => 0,
            })
        })
    }

    #[test]
    fn a_field_the_implementation_leaves_is_the_devices_at_the_layer_3_call() {
        // The rebar's shape: depth Equal, no writes; cull, winding, colour
        // mask and Z mode inherited.
        let rebar = own(&[(R::ZFunc, CompareFunc::Equal.xbox_value()), (R::ZWriteEnable, 0), (R::AlphaBlendEnable, 0)]);
        let s = resolve(&rebar, &entry(), None).unwrap();
        assert_eq!((s.depth_compare, s.depth_write), (wgpu::CompareFunction::Equal, false));
        let device = gfx::translate::primitive_state(
            &RenderState { cull_mode: CullMode::Cw, front_face: FrontFace::Cw, ..RenderState::default() },
            wgpu::PrimitiveTopology::TriangleList,
            gfx::translate::Capabilities::from_features(wgpu::Features::empty()),
        );
        assert_eq!((s.primitive.cull_mode, s.primitive.front_face), (device.cull_mode, device.front_face), "the device's cull and winding");
        assert_eq!(s.color_writes, wgpu::ColorWrites::ALL);
        // Its own cull wins, with the device's winding.
        let own_cull = own(&[(R::CullMode, CullMode::None.xbox_value())]);
        assert_eq!(resolve(&own_cull, &entry(), None).unwrap().primitive.cull_mode, None);
        let mut ccw = entry();
        ccw.0.retain(|(t, _)| *t != R::FrontFace);
        ccw.0.push((R::FrontFace, FrontFace::Ccw.xbox_value()));
        let cw_cull = own(&[(R::CullMode, CullMode::Cw.xbox_value())]);
        let culls = |e: &EntryStates| {
            let p = resolve(&cw_cull, e, None).unwrap().primitive;
            (p.cull_mode, p.front_face)
        };
        assert_ne!(culls(&ccw), culls(&entry()), "the winding is inherited, not the default");
    }

    #[test]
    fn a_field_that_cannot_be_established_refuses_the_draw() {
        let mut e = entry();
        e.0.retain(|(t, _)| *t != R::CullMode);
        let err = resolve(&[], &e, None).unwrap_err();
        assert!(err.contains("CullMode"), "{err}");
        assert!(resolve(&own(&[(R::CullMode, CullMode::None.xbox_value())]), &e, None).is_ok(), "set by the implementation");
        // A value outside its enum reads as unknown.
        let mut bad = entry();
        bad.0.retain(|(t, _)| *t != R::ZFunc);
        bad.0.push((R::ZFunc, 0xdead));
        assert!(resolve(&[], &bad, None).unwrap_err().contains("ZFunc"));
    }

    #[test]
    fn states_the_native_pass_does_not_draw_refuse_it() {
        let with = |t: R, v: u32| {
            let mut e = entry();
            e.0.retain(|(s, _)| *s != t);
            e.0.push((t, v));
            e
        };
        assert!(resolve(&[], &with(R::StencilEnable, 1), None).unwrap_err().contains("stencil"));
        assert!(resolve(&[], &with(R::FogEnable, 1), None).is_err());
        assert!(resolve_scene(&[], &with(R::FogEnable, 1), None).is_ok());
        assert!(resolve(&[], &with(R::AlphaBlendEnable, 1), None).unwrap_err().contains("blends"));
        assert!(resolve(&own(&[(R::ZEnable, DepthBufferMode::Z.xbox_value())]), &entry(), None).unwrap_err().contains("Z-buffered"));
        assert!(resolve(&[], &with(R::ZEnable, DepthBufferMode::Z.xbox_value()), None).is_ok(), "inherited: the device's mode, checked by the host");
        let off = resolve(&[], &with(R::ZEnable, DepthBufferMode::Disabled.xbox_value()), None).unwrap();
        assert_eq!((off.depth_write, off.depth_compare), (false, wgpu::CompareFunction::Always));
        // An inherited alpha test the program was not compiled with.
        let mut tested = with(R::AlphaTestEnable, 1);
        tested.0.retain(|(s, _)| *s != R::AlphaFunc);
        tested.0.push((R::AlphaFunc, CompareFunc::Greater.xbox_value()));
        let err = resolve(&[], &tested, None).unwrap_err();
        assert!(err.contains("alpha test"), "{err}");
        let compiled = Some((wgpu::CompareFunction::Greater, 0));
        assert_eq!(resolve(&[], &tested, compiled).unwrap().alpha_test, compiled);
        // The clamp of the W clip, inherited.
        assert!(resolve(&[], &with(R::DepthClipControl, 0x10), None).unwrap().w_clamp);
    }
}
