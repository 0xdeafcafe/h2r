//! Texture inputs for the layer-15 plasma pass used by lift beams.
//! The web viewer uses the two scrolling fields and their stationary mask
//! in a single unlit additive draw.
use map::TagId;
use serde::Serialize;
use shaders::psh::{PixelShaderDef, argb};
use tags::effects::function::{Function, FunctionType};
use tags::material::{AnimationKind, ResolvedImplementation, TextureSource};
use tags::shader::{AnimatedProperty, RenderStateValue, render_state_name};

use crate::live_geometry::MapReader;

pub fn is_template(name: &str) -> bool {
    let mut path = name.rsplit(['\\', '/']);
    matches!((path.next(), path.next()), (Some(stem), Some(folder))
        if stem.eq_ignore_ascii_case("plasma_alpha") && folder.eq_ignore_ascii_case("transparent"))
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Scroll {
    pub period: f32,
    pub from: f32,
    pub to: f32,
}

/// A time-driven constant or an unclipped linear ramp. The reversed linear
/// mapping on the Covenant beam and the forward transitions on Lockout are
/// both remapped to the function's specified output range.
fn endpoints(property: &AnimatedProperty) -> Option<(f32, f32, f32)> {
    let period = f32::from_bits(property.input_b);
    if property.input != 0 || !period.is_finite() || period <= 0.0 {
        return None;
    }
    let f = Function::parse(&property.function).ok()?;
    if f.is_ranged() {
        return None;
    }
    // Parse verifies the exact payload length for each supported kind.
    let word = |i: usize| {
        let p = i * 4;
        f32::from_le_bytes(f.payload[p..p + 4].try_into().unwrap())
    };
    let [a, b] = match f.kind {
        FunctionType::Constant => [0.0, 0.0],
        FunctionType::Identity => [0.0, 1.0],
        FunctionType::Linear => [word(5), word(4) + word(5)],
        FunctionType::Transition if f.sub_types[0] == 0 => [word(0), word(1)],
        _ => return None,
    };
    if ![a, b].iter().all(|v| (0.0..=1.0).contains(v)) {
        return None;
    }
    let value = |x| {
        if f.is_raw_output() {
            x
        } else {
            let [lo, hi] = f.range;
            lo + x * (hi - lo)
        }
    };
    let [from, to] = [value(a), value(b)];
    (from.is_finite() && to.is_finite()).then_some((from, to, period))
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Transform {
    pub scale: [f32; 2],
    pub offset: [f32; 2],
    /// An animated axis replaces its static offset.
    pub scroll: [Option<Scroll>; 2],
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PlasmaLook {
    /// Noise A, noise B, and coverage mask, in sampler order.
    pub uv: [Transform; 3],
    /// Stage 3's two tints, followed by the light added at stage 6.
    pub colors: [[f32; 3]; 3],
}

fn is_additive(states: &[RenderStateValue]) -> bool {
    let state = |name| {
        states
            .iter()
            .rev()
            .find(|r| render_state_name(r.state) == Some(name))
            .map(|r| r.value)
    };
    // Xbox D3DBLEND_ONE and D3DBLENDOP_ADD, with depth writes disabled.
    state("ALPHABLENDENABLE") == Some(1)
        && state("SRCBLEND") == Some(1)
        && state("DESTBLEND") == Some(1)
        && state("BLENDOP") == Some(0x8006)
        && state("ZWRITEENABLE") == Some(0)
}

fn transform(
    imp: &ResolvedImplementation,
    properties: &[AnimatedProperty],
    stage: u8,
) -> Option<Transform> {
    // The paired c[20..25] rows supply (u, v, 1) coefficients for
    // texture stages 1 to 3. Retail lift UV transforms have no rotation.
    let reg = 18 + 2 * stage;
    let row = |index| {
        imp.vertex_constants
            .iter()
            .find(|r| r.index == index)
            .map(|r| r.value)
    };
    let [sx, skew_u, _, u] = row(reg)?;
    let [skew_v, sy, _, v] = row(reg + 1)?;
    if skew_u != 0.0 || skew_v != 0.0 || [sx, sy, u, v].iter().any(|x| !x.is_finite()) {
        return None;
    }
    let mut uv = Transform {
        scale: [sx, sy],
        offset: [u, v],
        scroll: [None; 2],
    };
    for axis in 0..2u8 {
        let target = (reg - 18 + axis) | 0x10;
        let binding = imp.animated.iter().find(|a| {
            let word = (u16::from(a.binding.b1) << 8) | u16::from(a.binding.target);
            a.kind == AnimationKind::VertexConstant
                && (a.binding.target & 0x1f) == target
                && ((word >> 5) & 0x1f) == 14 + u16::from(axis)
        });
        let Some(binding) = binding else { continue };
        // Component numbers are explicit in the bindings. Ascension and
        // Lockout store the animation properties in different orders.
        let component = |index| -> Option<(f32, f32, f32)> {
            let p = binding.properties.iter().find(|p| p.component == index)?;
            endpoints(properties.get(usize::try_from(p.property).ok()?)?)
        };
        let axis = usize::from(axis);
        let (scale, scale_end, _) = component(1 + axis as i16)?;
        if scale != scale_end {
            return None;
        }
        let (from, to, period) = component(4 + axis as i16)?;
        uv.scale[axis] = scale;
        uv.offset[axis] = from;
        uv.scroll[axis] = (from != to).then_some(Scroll { from, to, period });
    }
    Some(uv)
}

/// Resolve the template's sorted transparent pass. The supported pass
/// uses frame zero for each bitmap.
pub fn read(
    map: &MapReader,
    lib: &mut tags::material::ShaderLibrary,
    shader: TagId,
) -> Option<(PlasmaLook, [TagId; 3])> {
    let material = lib.material(shader, None).ok()?;
    let layer = material
        .layers
        .iter()
        .find(|l| l.lod == 0 && l.layer == 15)?;
    let [pass] = layer.passes.as_slice() else {
        return None;
    };
    let name = map.tag(pass.pass).ok()?.name.as_deref()?;
    if !name
        .rsplit(['\\', '/'])
        .next()?
        .eq_ignore_ascii_case("plasma_alpha")
    {
        return None;
    }
    let [imp] = pass.implementations.as_slice() else {
        return None;
    };
    if !imp.engine_render_states.is_empty() || !is_additive(&imp.render_states) {
        return None;
    }
    let bitmap = |stage: usize| match imp.textures[stage]?.source {
        TextureSource::Bitmap {
            bitmap, image: 0, ..
        } => Some(bitmap),
        _ => None,
    };
    let instance = lib.shader(shader).ok()?;
    let props = &instance.compiled().ok()?.properties;
    let ps = PixelShaderDef::from_bytes(&imp.pixel_shader).ok()?;
    let rgb = |c| {
        let [r, g, b, _] = argb(c);
        [r, g, b]
    };
    Some((
        PlasmaLook {
            uv: [
                transform(imp, props, 1)?,
                transform(imp, props, 2)?,
                transform(imp, props, 3)?,
            ],
            colors: [
                rgb(ps.constant0[3]),
                rgb(ps.constant1[3]),
                rgb(ps.constant0[6]),
            ],
        },
        [bitmap(1)?, bitmap(2)?, bitmap(3)?],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plasma_template_uses_the_layer_15_plasma_path() {
        assert!(is_template(r"shaders\transparent\plasma_alpha"));
        assert!(!is_template(r"shaders\transparent\one_add_illum"));
        assert!(!is_template(r"shaders\transparent\plasma_alpha_overlay"));
    }

    fn property(kind: FunctionType, values: &[f32]) -> AnimatedProperty {
        AnimatedProperty {
            input: 0,
            input_a: 0,
            input_b: 3.0f32.to_bits(),
            function: Function {
                kind,
                flags: 0,
                sub_types: [0; 2],
                range: [0.25, -0.75],
                unknown_0c: [0; 8],
                payload: values.iter().flat_map(|f| f.to_le_bytes()).collect(),
            }
            .to_bytes(),
        }
    }

    #[test]
    fn plasma_requires_one_plus_one_blending_without_depth_writes() {
        let mut states = [
            RenderStateValue {
                state: 59,
                value: 1,
            },
            RenderStateValue {
                state: 62,
                value: 1,
            },
            RenderStateValue {
                state: 63,
                value: 1,
            },
            RenderStateValue {
                state: 64,
                value: 0,
            },
            RenderStateValue {
                state: 74,
                value: 0x8006,
            },
        ];
        assert!(is_additive(&states));
        states[3].value = 1;
        assert!(!is_additive(&states));
        states[3].value = 0;
        states[1].value = 0x0302;
        assert!(
            !is_additive(&states),
            "source alpha would dim the already masked glow"
        );
    }

    #[test]
    fn plasma_scroll_preserves_period_range_and_direction() {
        let forward = property(FunctionType::Transition, &[0.0, 1.0, 0.0, 0.0]);
        assert_eq!(endpoints(&forward), Some((0.25, -0.75, 3.0)));
        let mut values = [0.0; 12];
        values[4] = -1.0;
        values[5] = 1.0;
        let mut reverse = property(FunctionType::Linear, &values);
        assert_eq!(endpoints(&reverse), Some((-0.75, 0.25, 3.0)));
        reverse.input = 1;
        assert_eq!(endpoints(&reverse), None);
        reverse.input = 0;
        reverse.input_b = f32::NAN.to_bits();
        assert_eq!(endpoints(&reverse), None);
    }
}
