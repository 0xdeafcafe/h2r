//! A GPU particle system's [`Definition`] built from the `prt3` tag
//! (docs/graphics/native-particles.md, "GPU particle systems"). Pure: the
//! tag, the bitmap's sprite sequences and the values the guest reads from
//! its image go in, the uniform comes out, or a named [`Refused`].
//!
//! Every field is the one `0x00224550` reads (`engine::particle_emission`):
//! the flags at `+0`, the five properties at `+0x1c..+0x5c`, the sequence
//! pick at `+8` and `+0xa`, the fixed corners and window at `+0x9c` and
//! `+0xac`, and the input mask at `+0x98`, which checks that every input a
//! property reads is one the guest computes. A tag function is baked to
//! the two [`CURVE`] curves by evaluating it at range 0 and range 1
//! (`tags::effects::function`); the colour function's stops are baked to
//! the [`GRADIENT`] stops. An input the GPU evaluator does not have is a
//! refusal, never a stand-in.

use tags::effects::function::{Function, FunctionType, Tables};
use tags::effects::particle::{PROPERTY_COUNT, Particle};
use tags::effects::Property as TagProperty;

use crate::particle_system::{
    CURVE, ColourProperty, Definition, GRADIENT, Input, NO_INPUT, Property, Sprite, modifier, pack,
};

/// The guest's input indices (`0x00248df0`) the GPU has, and the lane each
/// is: 0 the age over the lifetime, 1 the age in seconds, 2 and 3 the
/// particle's `+0x14` and `+0x16` words, 14 and 15 its `+0x18` and `+0x1a`.
pub const INPUT_LANES: [(i16, Input); 6] = [
    (0, Input::Life),
    (1, Input::Age),
    (2, Input::Random0),
    (3, Input::Random1),
    (14, Input::Random2),
    (15, Input::Random3),
];

/// The inputs the callback computes (`+0x98 & 0xf80f`).
const CALLBACK_INPUTS: u32 = 0xf80f;
/// The bitmap type whose sequences hold sprites (`bitmap + 0 == 3`).
pub const SPRITES: u16 = 3;

/// A property of the root, by its offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    /// `+0x1c`.
    Colour,
    /// `+0x2c`.
    Alpha,
    /// `+0x3c`.
    Size,
    /// `+0x4c`.
    Rotation,
    /// `+0x5c`.
    Frame,
}

impl Slot {
    pub fn name(self) -> &'static str {
        match self {
            Slot::Colour => "colour",
            Slot::Alpha => "alpha",
            Slot::Size => "size",
            Slot::Rotation => "rotation",
            Slot::Frame => "frame",
        }
    }
}

/// Which of a property's three inputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Input,
    Range,
    Modifier,
}

/// Why a tag has no GPU definition. Each has a counter-style [`Self::name`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The root holds other than five properties.
    PropertyCount(usize),
    /// A property the callback evaluates has no function data (the guest
    /// faults on a scalar one and uses a global default for colour).
    NoFunction(Slot),
    /// The function data does not parse.
    BadFunction(Slot),
    /// A property reads an input index the GPU evaluator has no lane for
    /// (system, effect or camera values, or outside the table).
    InputTheGpuLacks { slot: Slot, role: Role, input: i16 },
    /// A property reads an input the definition's `+0x98` mask leaves
    /// uncomputed.
    InputOutsideTheMask { slot: Slot, role: Role, input: i16 },
    /// The function reads the age in seconds, which passes 1, and does not
    /// stay at its value at 1 there; the baked curve spans 0..1 only.
    AgeBeyondTheCurve(Slot),
    /// A wave or shape the tables passed in do not hold.
    MissingTable { slot: Slot, wave: bool, index: u8 },
    /// The bitmap's sequences hold different numbers of sprites; the GPU
    /// table is one count a sequence.
    UnevenSequences,
}

impl Refused {
    pub fn name(&self) -> &'static str {
        match self {
            Refused::PropertyCount(_) => "property_count",
            Refused::NoFunction(_) => "no_function",
            Refused::BadFunction(_) => "bad_function",
            Refused::InputTheGpuLacks { .. } => "input_the_gpu_lacks",
            Refused::InputOutsideTheMask { .. } => "input_outside_the_mask",
            Refused::AgeBeyondTheCurve(_) => "age_beyond_the_curve",
            Refused::MissingTable { .. } => "missing_table",
            Refused::UnevenSequences => "uneven_sequences",
        }
    }
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::PropertyCount(n) => write!(f, "{n} properties, not {PROPERTY_COUNT}"),
            Refused::NoFunction(s) => write!(f, "{} has no function", s.name()),
            Refused::BadFunction(s) => write!(f, "{}'s function does not parse", s.name()),
            Refused::InputTheGpuLacks { slot, role, input } => {
                write!(f, "{} {role:?} reads input {input}, which the GPU lacks", slot.name())
            }
            Refused::InputOutsideTheMask { slot, role, input } => {
                write!(f, "{} {role:?} reads input {input}, outside the +0x98 mask", slot.name())
            }
            Refused::AgeBeyondTheCurve(s) => write!(f, "{} reads the age past 1", s.name()),
            Refused::MissingTable { slot, wave, index } => {
                let kind = if *wave { "wave" } else { "shape" };
                write!(f, "{} needs {kind} {index}, not in the tables", slot.name())
            }
            Refused::UnevenSequences => write!(f, "sequences of different sprite counts"),
        }
    }
}

impl std::error::Error for Refused {}

/// A sprites bitmap (type [`SPRITES`]): its sequences' sprites in order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sheet {
    pub sequences: Vec<Vec<Sprite>>,
}

impl Sheet {
    /// The bitmap's sprites, `bounds` (`+8`, `(u0, u1, v0, v1)`) as the
    /// window and `+0x18` as the registration; `None` unless it is a
    /// sprites bitmap, which the callback draws with the whole texture.
    pub fn of(bitmap: &tags::Bitmap) -> Option<Self> {
        if bitmap.header.kind.0 != SPRITES {
            return None;
        }
        let sequences = bitmap
            .sequences
            .iter()
            .map(|s| {
                s.sprites
                    .iter()
                    .map(|p| Sprite { window: p.bounds, registration: p.registration, _pad: [0.0; 2] })
                    .collect()
            })
            .collect();
        Some(Self { sequences })
    }

    /// The table [`crate::particle_compute`] takes: sequence after sequence.
    pub fn sprites(&self) -> Vec<Sprite> {
        self.sequences.iter().flatten().copied().collect()
    }
}

/// What the guest reads from its image rather than the tag.
#[derive(Clone, Copy, Debug)]
pub struct Image<'a> {
    /// Rotation's scale (`0x0043f17c`).
    pub angle_scale: f32,
    /// Flag `0x2000`'s rotation offset (`0x004123ec`).
    pub spin: f32,
    /// The wave and shape tables the functions sample.
    pub tables: &'a Tables,
}

/// The tag's definition. The emitter's and movement's values (`tint`,
/// `size_scale`, `gravity`, `drag`) are left at one and zero for the
/// system's owner to set.
pub fn definition(tag: &Particle, sheet: Option<&Sheet>, image: &Image<'_>) -> Result<Definition, Refused> {
    if tag.properties.len() != PROPERTY_COUNT {
        return Err(Refused::PropertyCount(tag.properties.len()));
    }
    let flags = u32::from(tag.flags) | u32::from(tag.unknown_02) << 16;
    let (frames, sequences) = match sheet {
        None => (0, 0),
        Some(s) => {
            let frames = s.sequences.first().map_or(0, Vec::len);
            if s.sequences.iter().any(|q| q.len() != frames) {
                return Err(Refused::UnevenSequences);
            }
            (frames as u32, s.sequences.len() as u32)
        }
    };
    let b = Baker { mask: tag.flags_98 & CALLBACK_INPUTS, tables: image.tables };
    let p = |s: Slot| &tag.properties[s as usize];
    // The frame property is read only on the sprite path.
    let sprite_path = flags & crate::particle_system::flag::FIXED_WINDOW == 0 && frames > 0 && sequences > 0;
    let frame = if sprite_path { b.scalar(Slot::Frame, p(Slot::Frame))? } else { Property::constant(0.0) };
    let [first, count] = tag.unknown_08.map(|v| v as i16);
    Ok(Definition {
        flags,
        sequence_first: i32::from(first),
        sequence_pick: count.max(1) as u32,
        frames,
        sequences,
        angle_scale: image.angle_scale,
        spin: image.spin,
        size_scale: 1.0,
        tint: [1.0; 4],
        gravity: [0.0; 3],
        drag: 0.0,
        fixed_window: tag.uv,
        fixed_corners: tag.quad,
        rotation: b.scalar(Slot::Rotation, p(Slot::Rotation))?,
        alpha: b.scalar(Slot::Alpha, p(Slot::Alpha))?,
        size: b.scalar(Slot::Size, p(Slot::Size))?,
        frame,
        colour: b.colour(p(Slot::Colour))?,
    })
}

struct Baker<'a> {
    mask: u32,
    tables: &'a Tables,
}

impl Baker<'_> {
    fn function(&self, slot: Slot, p: &TagProperty) -> Result<Function, Refused> {
        if p.data.is_empty() {
            return Err(Refused::NoFunction(slot));
        }
        let f = Function::parse(&p.data).map_err(|_| Refused::BadFunction(slot))?;
        let ranged = f.is_ranged();
        let check = |wave: bool, index: u8| {
            let have = if wave { self.tables.has_wave(index) } else { index == 0 || self.tables.shape_table(index).is_some() };
            if have { Ok(()) } else { Err(Refused::MissingTable { slot, wave, index }) }
        };
        if matches!(f.kind, FunctionType::Periodic | FunctionType::Transition) {
            let wave = f.kind == FunctionType::Periodic;
            check(wave, f.sub_types[0])?;
            if ranged {
                check(wave, f.sub_types[1])?;
            }
        }
        Ok(f)
    }

    /// The GPU lane of guest input `i`, read in `role`.
    fn lane(&self, slot: Slot, role: Role, i: i16) -> Result<Input, Refused> {
        let lane = INPUT_LANES
            .iter()
            .find(|(g, _)| *g == i)
            .map(|&(_, l)| l)
            .ok_or(Refused::InputTheGpuLacks { slot, role, input: i })?;
        if self.mask & 1 << i == 0 {
            return Err(Refused::InputOutsideTheMask { slot, role, input: i });
        }
        Ok(lane)
    }

    /// The lane the function's argument reads, `Zero` when its value does
    /// not depend on it.
    fn argument(&self, slot: Slot, f: &Function, i: i16) -> Result<Input, Refused> {
        use FunctionType::*;
        if matches!(f.kind, Constant | MultiLinearKey | MultiSpline | Unknown(_)) {
            return Ok(Input::Zero);
        }
        let lane = self.lane(slot, Role::Input, i)?;
        // Identity clamps the argument and Transition's shape does, so both
        // hold their value at 1 past it.
        if lane == Input::Age && !matches!(f.kind, Identity | Transition) {
            return Err(Refused::AgeBeyondTheCurve(slot));
        }
        Ok(lane)
    }

    fn range(&self, slot: Slot, f: &Function, i: i16) -> Result<Input, Refused> {
        if f.is_ranged() && f.kind.is_evaluated() { self.lane(slot, Role::Range, i) } else { Ok(Input::Zero) }
    }

    /// The modifier's lane and operation; none unless it adds or multiplies.
    fn modifier(&self, slot: Slot, p: &TagProperty) -> Result<(u32, u32), Refused> {
        if p.modifier_input < 0 || !matches!(u32::from(p.output_modifier), modifier::ADD | modifier::MULTIPLY) {
            return Ok((NO_INPUT, modifier::NONE));
        }
        Ok((self.lane(slot, Role::Modifier, p.modifier_input)? as u32, u32::from(p.output_modifier)))
    }

    /// The function at range 0 and range 1 over its argument 0..1.
    fn curves(&self, f: &Function) -> ([[f32; 4]; CURVE / 4], [[f32; 4]; CURVE / 4]) {
        let at = |r: f32| pack(std::array::from_fn(|i| f.evaluate_with(i as f32 / (CURVE - 1) as f32, r, self.tables)));
        (at(0.0), at(1.0))
    }

    /// `0x00246cd0`.
    fn scalar(&self, slot: Slot, p: &TagProperty) -> Result<Property, Refused> {
        let f = self.function(slot, p)?;
        let (modifier_input, modifier) = self.modifier(slot, p)?;
        let (curve_lo, curve_hi) = self.curves(&f);
        Ok(Property {
            input: self.argument(slot, &f, p.input)? as u32,
            range: self.range(slot, &f, p.range_input)? as u32,
            modifier_input,
            modifier,
            lo: f.range[0],
            hi: f.range[1],
            map: u32::from(!f.is_raw_output()),
            _pad: 0,
            curve_lo,
            curve_hi,
        })
    }

    /// `0x00246d80`: the function's value into its colour stops
    /// (`Function::output_colour`), sampled at the gradient's stops.
    fn colour(&self, p: &TagProperty) -> Result<ColourProperty, Refused> {
        let slot = Slot::Colour;
        let f = self.function(slot, p)?;
        let stops = f.flags >> 4;
        // The packed colour that ignores the value: a constant function
        // (unless ranged over several stops), one stop, or white.
        let fixed = (f.kind == FunctionType::Constant && (!f.is_ranged() || stops <= 1)) || stops == 1 || stops >= 5;
        let gradient = std::array::from_fn(|i| {
            let c = f.output_colour(i as f32 / (GRADIENT - 1) as f32);
            let byte = |s: u32| ((c >> s) & 0xff) as f32 / 255.0;
            [byte(16), byte(8), byte(0), 0.0]
        });
        let (curve_lo, curve_hi) = self.curves(&f);
        if fixed {
            return Ok(ColourProperty {
                input: Input::Zero as u32,
                range: Input::Zero as u32,
                modifier_input: NO_INPUT,
                modifier: modifier::NONE,
                curve_lo,
                curve_hi,
                gradient,
            });
        }
        let (modifier_input, modifier) = self.modifier(slot, p)?;
        Ok(ColourProperty {
            input: self.argument(slot, &f, p.input)? as u32,
            range: self.range(slot, &f, p.range_input)? as u32,
            modifier_input,
            modifier,
            curve_lo,
            curve_hi,
            gradient,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::particle_system::flag;
    use map::{FourCC, TagId};
    use tags::effects::TagRef;

    const NONE: TagRef = TagRef { group: FourCC(*b"bitm"), id: TagId(u32::MAX) };

    /// A function blob: kind, flags, sub-types, range, then payload words.
    fn blob(kind: u8, flags: u8, range: [f32; 2], words: &[f32]) -> Vec<u8> {
        let mut v = vec![kind, flags, 0, 0];
        v.extend(range[0].to_le_bytes());
        v.extend(range[1].to_le_bytes());
        v.extend([0u8; 8]);
        for w in words {
            v.extend(w.to_le_bytes());
        }
        v
    }

    /// Linear `a * x + b` (set 0) and `c * x + d` (set 1, when ranged).
    fn linear(flags: u8, range: [f32; 2], set0: [f32; 2], set1: [f32; 2]) -> Vec<u8> {
        let mut w = [0.0; 12];
        w[4] = set0[0];
        w[5] = set0[1];
        w[10] = set1[0];
        w[11] = set1[1];
        blob(4, flags, range, &w)
    }

    fn property(input: i16, data: Vec<u8>) -> TagProperty {
        TagProperty { input, range_input: 0, output_modifier: 0, modifier_input: -1, data }
    }

    /// Every property the identity of the life, every input in the mask.
    fn tag() -> Particle {
        let identity = || blob(0, 0, [0.0, 1.0], &[]);
        Particle {
            flags: 0,
            unknown_02: 0,
            unknown_04: 0,
            unknown_08: [0, 0],
            shader_template: NONE,
            parameters: vec![],
            properties: (0..PROPERTY_COUNT).map(|_| property(0, identity())).collect(),
            ref_6c: NONE,
            ref_74: NONE,
            unknown_7c: vec![],
            systems: vec![],
            postprocess: vec![],
            unknown_94: 0,
            flags_98: CALLBACK_INPUTS,
            quad: [-0.5, -0.5, 0.5, 0.5],
            uv: [0.0, 1.0, 0.0, 1.0],
        }
    }

    fn image() -> Image<'static> {
        Image { angle_scale: std::f32::consts::TAU, spin: 1.5707964, tables: Tables::procedural() }
    }

    fn sprite(u0: f32) -> Sprite {
        Sprite { window: [u0, u0 + 0.25, 0.0, 1.0], registration: [0.5, 0.5], _pad: [0.0; 2] }
    }

    fn unpack(c: [[f32; 4]; CURVE / 4]) -> [f32; CURVE] {
        std::array::from_fn(|i| c[i / 4][i % 4])
    }

    #[test]
    fn a_property_bakes_its_function_at_both_range_ends() {
        let mut t = tag();
        // Ranged: 0.5x + 0.25 at range 0, -x + 1 at range 1, mapped to 2..6.
        t.properties[Slot::Size as usize] = TagProperty {
            input: 0,
            range_input: 2,
            output_modifier: 2,
            modifier_input: 3,
            data: linear(1, [2.0, 6.0], [0.5, 0.25], [-1.0, 1.0]),
        };
        let d = definition(&t, None, &image()).unwrap();
        let s = d.size;
        let (lo, hi) = (unpack(s.curve_lo), unpack(s.curve_hi));
        for i in 0..CURVE {
            let x = i as f32 / 15.0;
            assert!((lo[i] - (0.5 * x + 0.25)).abs() < 1e-6, "range 0 at {x}");
            assert!((hi[i] - (1.0 - x)).abs() < 1e-6, "range 1 at {x}");
        }
        assert_eq!((s.input, s.range), (Input::Life as u32, Input::Random0 as u32));
        assert_eq!((s.modifier_input, s.modifier), (Input::Random1 as u32, modifier::MULTIPLY));
        assert_eq!((s.lo, s.hi, s.map), (2.0, 6.0, 1));
    }

    #[test]
    fn an_unranged_property_reads_no_range_and_bakes_one_curve() {
        let mut t = tag();
        t.properties[Slot::Alpha as usize] = TagProperty { range_input: 9, ..property(15, linear(0x10, [0.0, 1.0], [1.0, 0.0], [0.0; 2])) };
        let a = definition(&t, None, &image()).unwrap().alpha;
        assert_eq!((a.input, a.range), (Input::Random3 as u32, Input::Zero as u32));
        assert_eq!(a.curve_lo, a.curve_hi);
        assert_eq!(a.map, 0, "a raw-output function keeps its value");
    }

    #[test]
    fn the_fixed_window_comes_from_the_tag() {
        let mut t = tag();
        t.unknown_02 = 1;
        t.flags = 0x2006;
        t.uv = [0.25, 0.5, 0.0, 0.125];
        t.quad = [-1.0, -1.0, 2.0, 2.0];
        let d = definition(&t, None, &image()).unwrap();
        assert_eq!(d.flags, flag::FIXED_WINDOW | flag::SPIN | flag::FLIP_U | flag::FLIP_V);
        assert_eq!(d.fixed_window, [0.25, 0.5, 0.0, 0.125]);
        assert_eq!(d.fixed_corners, [-1.0, -1.0, 2.0, 2.0]);
        assert_eq!((d.angle_scale, d.spin), (image().angle_scale, image().spin), "scales from the image");
    }

    #[test]
    fn the_sequence_pick_and_frames_come_from_the_tag_and_sheet() {
        let mut t = tag();
        t.flags = flag::RANDOM_SEQUENCE as u16;
        t.unknown_08 = [2, 0];
        let sheet = Sheet { sequences: (0..3).map(|s| (0..4).map(|f| sprite((s * 4 + f) as f32)).collect()).collect() };
        let d = definition(&t, Some(&sheet), &image()).unwrap();
        assert_eq!((d.sequence_first, d.sequence_pick), (2, 1), "a count below one picks one");
        assert_eq!((d.frames, d.sequences), (4, 3));
        assert_eq!(sheet.sprites().len(), 12);
        assert_eq!(sheet.sprites()[5].window[0], 5.0, "sequence after sequence");
    }

    #[test]
    fn sequences_of_different_lengths_are_refused() {
        let sheet = Sheet { sequences: vec![vec![sprite(0.0); 2], vec![sprite(0.0); 3]] };
        assert_eq!(definition(&tag(), Some(&sheet), &image()), Err(Refused::UnevenSequences));
    }

    #[test]
    fn an_input_the_gpu_lacks_is_a_named_refusal() {
        for input in [4, 6, 7, 11, 12, 13, 16, -1] {
            let mut t = tag();
            t.properties[Slot::Rotation as usize].input = input;
            let e = definition(&t, None, &image()).unwrap_err();
            assert_eq!(e, Refused::InputTheGpuLacks { slot: Slot::Rotation, role: Role::Input, input });
            assert_eq!(e.name(), "input_the_gpu_lacks");
        }
    }

    #[test]
    fn an_input_outside_the_mask_is_refused() {
        let mut t = tag();
        t.flags_98 = 1;
        t.properties[Slot::Alpha as usize].modifier_input = 14;
        t.properties[Slot::Alpha as usize].output_modifier = 1;
        let e = definition(&t, None, &image()).unwrap_err();
        assert_eq!(e, Refused::InputOutsideTheMask { slot: Slot::Alpha, role: Role::Modifier, input: 14 });
    }

    #[test]
    fn an_input_a_function_ignores_is_not_read() {
        let mut t = tag();
        // A constant ignores its argument; an unused modifier reads nothing.
        t.properties[Slot::Rotation as usize] =
            TagProperty { output_modifier: 0, modifier_input: 12, ..property(12, blob(1, 0, [0.25, 0.25], &[0.0, 0.0])) };
        let r = definition(&t, None, &image()).unwrap().rotation;
        assert_eq!((r.input, r.modifier_input, r.modifier), (Input::Zero as u32, NO_INPUT, modifier::NONE));
        assert!(unpack(r.curve_lo).iter().all(|&v| v == 0.0) && r.lo == 0.25);
    }

    #[test]
    fn a_curve_over_the_age_past_one_is_refused() {
        let mut t = tag();
        t.properties[Slot::Size as usize] = property(1, linear(0, [0.0, 1.0], [0.25, 0.0], [0.0; 2]));
        assert_eq!(definition(&t, None, &image()), Err(Refused::AgeBeyondTheCurve(Slot::Size)));
        t.properties[Slot::Size as usize] = property(1, blob(0, 0, [0.0, 1.0], &[]));
        assert_eq!(definition(&t, None, &image()).unwrap().size.input, Input::Age as u32, "identity holds at 1");
    }

    #[test]
    fn a_wave_the_tables_lack_is_refused() {
        let mut t = tag();
        let mut f = blob(3, 0, [0.0, 1.0], &[0.0; 8]);
        f[2] = 3; // a baked wave the procedural tables do not hold
        t.properties[Slot::Alpha as usize] = property(0, f);
        assert_eq!(
            definition(&t, None, &image()),
            Err(Refused::MissingTable { slot: Slot::Alpha, wave: true, index: 3 })
        );
    }

    #[test]
    fn a_property_without_a_function_is_refused_unless_unread() {
        let mut t = tag();
        t.properties[Slot::Frame as usize].data.clear();
        assert!(definition(&t, None, &image()).is_ok(), "no sprites: the frame is not read");
        let sheet = Sheet { sequences: vec![vec![sprite(0.0); 2]] };
        assert_eq!(definition(&t, Some(&sheet), &image()), Err(Refused::NoFunction(Slot::Frame)));
        t.properties[Slot::Colour as usize].data.clear();
        assert_eq!(definition(&t, None, &image()), Err(Refused::NoFunction(Slot::Colour)));
    }

    #[test]
    fn the_colour_gradient_is_the_functions_stops() {
        let mut t = tag();
        // Two stops (flags 0x20): range word a, then the second word of +0xc.
        let mut f = blob(0, 0x20, [0.0; 2], &[]);
        f[4..8].copy_from_slice(&0xff00_00ffu32.to_le_bytes());
        f[0x10..0x14].copy_from_slice(&0xffff_0000u32.to_le_bytes());
        t.properties[Slot::Colour as usize] = property(2, f);
        let c = definition(&t, None, &image()).unwrap().colour;
        assert_eq!(c.input, Input::Random0 as u32);
        assert_eq!(c.gradient[0], [0.0, 0.0, 1.0, 0.0], "blue first");
        assert_eq!(c.gradient[GRADIENT - 1], [1.0, 0.0, 0.0, 0.0], "red last");
        assert!((c.gradient[3][0] - 3.0 / 7.0).abs() < 0.01, "a straight blend between");
    }

    #[test]
    fn a_constant_colour_reads_no_input() {
        let mut t = tag();
        let mut f = blob(1, 0x20, [0.0; 2], &[0.0, 0.0]);
        f[4..8].copy_from_slice(&0xff80_4020u32.to_le_bytes());
        t.properties[Slot::Colour as usize] = property(13, f);
        let c = definition(&t, None, &image()).unwrap().colour;
        assert_eq!((c.input, c.range, c.modifier_input), (Input::Zero as u32, Input::Zero as u32, NO_INPUT));
        assert!(c.gradient.iter().all(|g| *g == c.gradient[0]));
        assert!((c.gradient[0][0] - 128.0 / 255.0).abs() < 1e-6);
    }
}
