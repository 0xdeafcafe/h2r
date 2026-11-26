//! Sample a pass's constant registers after its instance and engine writes.

use std::sync::Arc;

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

/// A bitmap animation's component 0: the slot's frame (`+0x18`), which
/// `0x00018afb` copies to each stage bound from the slot and `0x0001b0a0`
/// turns into the bound image ([`frame_image`]).
pub const FRAME_COMPONENT: i16 = 0;

/// A bitmap animation's components 8 and 9: the slot's `+0x38` and
/// `+0x3c`, which `0x00017642` writes (`[ctx + 0x338 + 0x40 slot + 4
/// component]`) and nothing reads. Every instruction in `.text` that forms
/// a slot or stage record's address from the setup context
/// (`0x00485b48`: slots at `+0x320`, stages at `+0x1320`, 0x40 bytes each)
/// is in the material setup: the bitmap parameters `0x00016fab` write
/// `+0`, `+0x10`, `+0x14`, `+0x18`; the texture bindings `0x00018ae8`/
/// `0x00018aef` copy the slot whole (`rep movsd`, `0x00018afb`) and
/// `0x00018b11`..`0x00018d2e` touch `+0`..`+0x18`; the stage resolve
/// `0x0001b0d2` reads `+0`, `+4`, `+0x10`, `+0x14`, `+0x18`; the transform
/// `0x0001b7bd` reads `+0x1c`..`+0x34`. No displacement of `0x358`/`0x35c`
/// or `0x1358`/`0x135c` from the context occurs. So these writes change
/// nothing a draw binds and are taken without effect.
pub const UNREAD_COMPONENTS: [i16; 2] = [8, 9];

/// `[0x0045dbdc]`, 1e-4: a frame closer to zero than this binds image 0
/// (`0x0001b0e4`..`0x0001b0fe`).
const SMALL_FRAME: f32 = 1e-4;

/// The image `0x0001b0a0` binds for a stage whose frame is `frame` and
/// whose bitmap has `count` images: 0 when `|frame|` is below
/// [`SMALL_FRAME`], else the floor of the CRT `fmod(frame, count)`
/// (`0x00373b6a`; exact for an f32 frame and an integer count). `None`
/// when `0x000143c0` binds the rasterizer default texture instead (a
/// negative remainder keeps the frame's sign; with no images even image 0
/// is out of range), or when the frame is not finite (the CRT's error
/// path, which the native world resolver also leaves to the guest).
pub fn frame_image(frame: f32, count: u32) -> Option<u32> {
    if !frame.is_finite() || count == 0 {
        return None;
    }
    if SMALL_FRAME > frame.abs() {
        return Some(0);
    }
    let remainder = (f64::from(frame) % f64::from(count)) as f32;
    let image = remainder.floor();
    (image >= 0.0 && image < count as f32).then_some(image as u32)
}

/// The frames a draw's animation writes, per texture stage: `Some` where
/// a bitmap animation's component 0 reaches the stage
/// (`tags::material::AnimatedTarget::stages`), the frame before
/// [`frame_image`] wraps it.
pub type StageFrames = [Option<f32>; 4];

/// The selected implementation's register image at this clock. Bindings remain
/// attached so the caller can distinguish a per-frame upload from the baseline.
/// An animated bitmap frame refuses ([`LayerRefusal::Animation`]): these
/// callers bind one image per stage ([`sample_with_frames`] binds each).
pub fn sample_constants(
    imp: &ResolvedImplementation,
    properties: &[AnimatedProperty],
    time: Option<f64>,
    object: bool,
    pass: usize,
) -> Result<ResolvedImplementation, LayerRefusal> {
    sample_at(imp, properties, time, object, pass, None)
}

/// [`sample_constants`] for a caller that binds the image an animated
/// frame selects: the registers and each stage's frame.
pub fn sample_with_frames(
    imp: &ResolvedImplementation,
    properties: &[AnimatedProperty],
    time: Option<f64>,
    object: bool,
    pass: usize,
) -> Result<(ResolvedImplementation, StageFrames), LayerRefusal> {
    let mut frames = StageFrames::default();
    let out = sample_at(imp, properties, time, object, pass, Some(&mut frames))?;
    Ok((out, frames))
}

fn sample_at(
    imp: &ResolvedImplementation,
    properties: &[AnimatedProperty],
    time: Option<f64>,
    object: bool,
    pass: usize,
    mut frames: Option<&mut StageFrames>,
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
                    if UNREAD_COMPONENTS.contains(&r.component) {
                        continue;
                    }
                    let v = sample(r.property)?.scalar();
                    if r.component != FRAME_COMPONENT {
                        transforms.set(binding.target, r.component, v).ok_or_else(unsupported)?;
                        continue;
                    }
                    let frames = frames.as_deref_mut().ok_or_else(unsupported)?;
                    for s in &target.stages {
                        // The binding's size refresh (`0x00018b31`) reads
                        // the animated frame too, so a stage size an
                        // engine constant reads would follow the clock.
                        if imp.dimension_bindings.iter().any(|d| d.reads_size() && usize::from(d.stage) == usize::from(s.stage) + 1) {
                            return Err(LayerRefusal::FrameSize { pass, slot: binding.target, stage: s.stage });
                        }
                        frames[usize::from(s.stage)] = Some(v);
                    }
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

/// One compiled pass's animated constants, kept for its draws: the
/// implementation with its animated targets and the instance's
/// properties, sampled again at each draw's clock ([`sample_constants`]).
/// The structure's layer planner, the object passes and the world's
/// layer 3 all draw through it.
#[derive(Clone, Debug)]
pub struct PassAnimation {
    implementation: ResolvedImplementation,
    properties: Arc<[AnimatedProperty]>,
    object: bool,
    pass: usize,
    /// The draws bind the image an animated frame selects
    /// ([`Self::baseline_with_frames`]).
    frames: bool,
}

impl PassAnimation {
    /// What a pass compiles from: `imp` sampled at clock 0 with its
    /// animated targets cleared, so a register only an animation writes
    /// is set when the translation checks what the program reads; and the
    /// animation, `None` when `imp` animates nothing. `object`: the pass
    /// draws an object's item (a ranged curve refuses,
    /// [`LayerRefusal::ObjectAnimation`]). A target the sampler cannot
    /// apply refuses the pass.
    pub fn baseline(
        imp: &ResolvedImplementation,
        properties: &[AnimatedProperty],
        object: bool,
        pass: usize,
    ) -> Result<(ResolvedImplementation, Option<Self>), LayerRefusal> {
        Self::baseline_of(imp, properties, object, pass, false)
    }

    /// [`Self::baseline`] for draws that bind the image an animated frame
    /// selects ([`Self::sample_with_frames`]): a bitmap animation's
    /// component 0 does not refuse.
    pub fn baseline_with_frames(
        imp: &ResolvedImplementation,
        properties: &[AnimatedProperty],
        object: bool,
        pass: usize,
    ) -> Result<(ResolvedImplementation, Option<Self>), LayerRefusal> {
        Self::baseline_of(imp, properties, object, pass, true)
    }

    fn baseline_of(
        imp: &ResolvedImplementation,
        properties: &[AnimatedProperty],
        object: bool,
        pass: usize,
        frames: bool,
    ) -> Result<(ResolvedImplementation, Option<Self>), LayerRefusal> {
        if imp.animated.is_empty() {
            return Ok((imp.clone(), None));
        }
        let mut scratch = StageFrames::default();
        let mut baseline = sample_at(imp, properties, Some(0.0), object, pass, frames.then_some(&mut scratch))?;
        baseline.animated.clear();
        let animation = Self { implementation: imp.clone(), properties: properties.into(), object, pass, frames };
        Ok((baseline, Some(animation)))
    }

    /// The pass's registers at `time` (the render clock, `0x004858a0`).
    pub fn sample(&self, time: Option<f64>) -> Result<crate::fog_gpu::LayerConstants, LayerRefusal> {
        self.sample_with_frames(time).map(|(constants, _)| constants)
    }

    /// [`Self::sample`] and each stage's animated frame (none unless made
    /// by [`Self::baseline_with_frames`]).
    pub fn sample_with_frames(
        &self,
        time: Option<f64>,
    ) -> Result<(crate::fog_gpu::LayerConstants, StageFrames), LayerRefusal> {
        let mut frames = StageFrames::default();
        let at = sample_at(
            &self.implementation, &self.properties, time, self.object, self.pass,
            self.frames.then_some(&mut frames),
        )?;
        Ok((crate::fog_gpu::LayerConstants::from_implementation(&at), frames))
    }

    /// The image count of each stage an animated frame reaches
    /// (`tags::material::FrameStage::images`, 0 for a slot with no instance
    /// bitmap), `None` for the others: what a draw wraps the frame by and
    /// how many images the stage preloads.
    pub fn frame_counts(&self) -> [Option<u32>; 4] {
        let mut counts = [None; 4];
        if !self.frames {
            return counts;
        }
        for target in &self.implementation.animated {
            if target.kind != AnimationKind::Bitmap
                || !target.properties.iter().any(|r| r.component == FRAME_COMPONENT)
            {
                continue;
            }
            for s in &target.stages {
                counts[usize::from(s.stage)] = Some(s.images);
            }
        }
        counts
    }
}

/// The render clock (`0x004858a0`, a double, seconds) as the layer
/// inputs read it (`scene_extract::layout::layer_inputs::render_time`):
/// what a pass outside the planned layers samples its animation at.
pub fn render_time(mem: &dyn scene_extract::GuestRead) -> Option<f64> {
    scene_extract::layout::layer_inputs::render_time(mem)
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
