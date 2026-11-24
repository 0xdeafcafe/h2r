//! Sample a pass's constant registers after its instance and engine writes.

use tags::effects::function::{Function, FunctionType, Tables};
use tags::material::{AnimationKind, Origin, Register, ResolvedImplementation};
use tags::shader::AnimatedProperty;

use super::{LayerRefusal, pixel, texture_transform::TextureTransforms};

struct Sample {
    function: Function,
    raw: f32,
}

impl Sample {
    fn read(property: &AnimatedProperty, index: usize, time: Option<f64>, object: bool) -> Result<Self, LayerRefusal> {
        let bad = LayerRefusal::Property { property: index };
        if property.input != 0 {
            return Err(LayerRefusal::ObjectAnimation { property: index });
        }
        let function = Function::parse(&property.function).map_err(|_| bad.clone())?;
        if object && function.is_ranged() {
            return Err(LayerRefusal::ObjectAnimation { property: index });
        }
        let time = time.ok_or(LayerRefusal::NoTime)?;
        let period = f32::from_bits(property.input_b);
        if !(period.is_finite() && period > 0.0 && time.is_finite()) {
            return Err(bad);
        }
        let mut x = (time / f64::from(period)) as f32;
        if function.kind == FunctionType::Periodic {
            let wave = function.sub_types[0];
            if !Tables::procedural().has_wave(wave) {
                return Err(LayerRefusal::Wave { property: index, wave });
            }
        } else {
            x = (f64::from(x) % 1.0) as f32;
        }
        let raw = function.evaluate(x, 0.0);
        Ok(Self { raw, function })
    }

    fn scalar(&self) -> f32 {
        if self.function.is_raw_output() {
            return self.raw;
        }
        let [lo, hi] = self.function.range;
        (hi - lo) * self.raw.clamp(0.0, 1.0) + lo
    }

    fn rgb(&self) -> [f32; 3] {
        let colour = self.function.output_colour(self.raw);
        [16, 8, 0].map(|shift| ((colour >> shift) & 255u32) as f32 * (1.0 / 255.0))
    }
}

/// Scalar value at the render clock, including the header range when used.
pub fn property_value(property: &AnimatedProperty, index: usize, time: Option<f64>, object: bool) -> Result<f32, LayerRefusal> {
    Ok(Sample::read(property, index, time, object)?.scalar())
}

/// The selected implementation's register image at this clock. Bindings remain
/// attached so the caller can distinguish a per-frame upload from the baseline.
pub fn sample_constants(
    imp: &ResolvedImplementation,
    properties: &[AnimatedProperty],
    time: Option<f64>,
    object: bool,
    pass: usize,
) -> Result<ResolvedImplementation, LayerRefusal> {
    let mut out = imp.clone();
    let mut transforms = TextureTransforms::default();
    let sample = |property: i16| {
        let index = usize::try_from(property).map_err(|_| LayerRefusal::Property { property: usize::MAX })?;
        Sample::read(properties.get(index).ok_or(LayerRefusal::Property { property: index })?, index, time, object)
    };
    for target in &imp.animated {
        let binding = target.binding;
        let unsupported = || LayerRefusal::Animation { pass, kind: target.kind };
        let first = || sample(target.properties.first().ok_or_else(unsupported)?.property);
        match target.kind {
            AnimationKind::Bitmap => {
                for r in &target.properties {
                    let v = sample(r.property)?.scalar();
                    transforms.set(binding.target, r.component, v).ok_or_else(unsupported)?;
                }
            }
            AnimationKind::PixelConstant => {
                let Some(register) = target.pixel_register else { continue };
                let v = first()?;
                let (scalar, rgb) = if binding.b2 & 7 == 4 {
                    (0.0, v.rgb())
                } else {
                    (v.scalar(), [0.0; 3])
                };
                pixel::apply(&mut out, register, binding.b2, scalar, rgb).ok_or_else(unsupported)?;
            }
            AnimationKind::VertexConstant => {
                let word = u16::from_le_bytes([binding.target, binding.b1]);
                let mode = ((word >> 5) & 31) as u8;
                let reg = vertex_register(&mut out, binding.target);
                let mask = match mode {
                    0..=3 => {
                        reg.value[usize::from(mode)] = first()?.scalar();
                        1 << mode
                    }
                    4 => {
                        reg.value[..3].copy_from_slice(&first()?.rgb());
                        7
                    }
                    5..=20 => {
                        let slot = ((word >> 10) & 7) as u8;
                        transforms.write(slot, mode, &mut reg.value)
                            .ok_or(LayerRefusal::TextureTransform { pass, slot, mode })?
                    }
                    _ => return Err(unsupported()),
                };
                reg.written |= mask;
            }
            AnimationKind::RenderState | AnimationKind::StageState => return Err(unsupported()),
        }
    }
    Ok(out)
}

fn vertex_register(out: &mut ResolvedImplementation, dst: u8) -> &mut Register {
    let slot = dst & 15;
    let (bank, index) = if dst & 16 == 0 {
        (&mut out.vertex_defaults, slot)
    } else {
        (&mut out.vertex_constants, 18 + slot)
    };
    let i = match bank.binary_search_by_key(&index, |r| r.index) {
        Ok(i) => i,
        Err(i) => {
            bank.insert(i, Register { index, value: [0.0; 4], written: 0, origin: Origin::Instance });
            i
        }
    };
    &mut bank[i]
}
