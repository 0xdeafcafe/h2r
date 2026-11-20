//! An object's lighting as its layer-3 pass reads it: the engine inputs
//! the game computes from the object's render-state cache record, ported
//! (docs/graphics/native-renderer.md, "Object lighting").
//!
//! Research points (Code, `h2 disasm`):
//!
//! - The object geometry draw `0x0004d0b0` calls `0x0004d9c0`, which hands
//!   the render record's lighting pointer (`+0x10`, a cache record's
//!   `+0x44`; [`crate::objects::record_owner`]) to `0x0001bd50`. That stores
//!   it as the current lighting `[0x00485a58]` and marks the engine inputs
//!   that read it dirty; the shader draw `0x0001bbf0` then evaluates the
//!   implementation's dirty inputs (`0x0001ae70`): scalars through the
//!   jump table at `0x000336f0`, vectors through `0x00033980` (`EAX` the
//!   input, `ESI` where the three floats go).
//! - An implementation's engine vertex constants (implementation `+0x124`,
//!   records of the pass's `+0x38`, applied by `0x00019be0`): a word whose
//!   bits 0..3 are the register, bit 4 the bank (1: hardware `c[18 + i]`,
//!   uploaded by `0x00018e80`; 0: the vertex attribute defaults, by
//!   `0x00018d70`), bits 5..9 the mode (0..3 one scalar into that
//!   component, 4 a vector's xyz, 5 and up the stage sizes), and byte 3 the
//!   input; `0x0001b2c0` writes it. So a lit object's vertex program reads
//!   its lighting as `c[18 + i]` registers its implementation names.
//! - The build `0x0004cbf0` picks the implementation of an object's pass
//!   from the lighting: 0 when the byte `+0x76` is not `0xff`, 1 when it
//!   is, 2 for a shader flag (`0x0004cd2e`..`0x0004cd41`).
//! - The single-node transform goes separately: `0x0004d830` uploads the
//!   record's `+0x1c` matrix as three registers at hardware `c[50]`.
//!
//! The inputs that read the lighting `L` (vectors: three floats; `D` the
//! default vector at `[0x004686d4] + 4`, used without a lighting):
//!
//! | input | value |
//! |---|---|
//! | vector 14, 15, 16, 17 | `L+0x3c`, `L+0x30`, `L+0x54`, `L+0x48` (or `D`) |
//! | vector 23 | `L+0x30` (or `D`) times the shader's scalar 21 (`shad +0x44`, `[0x00485a6c]`), each clamped to 0..1 |
//! | vector 35 | the same times 0.5, clamped |
//! | vector 24 | `L+0x10` (or `D`) times 1 plus `shad +0x48` (`[0x00485a70]`), clamped |
//! | vector 3, 4, 21, 22 and 42..45 | colour slot 0, 1, 2, 3 and 0..3: the packed `0x00RRGGBB` word at `L+0x64 + 4·slot` over 255 when the slot is below the count byte `L+0x74`, else the default colour `[0x00468710]` (`0x00131bb0` unpacks) |
//! | vector 40 (all three), scalar 37 | the root object's `+0x2b0` clamped to 0..1, the object whose handle is `L+0` walked up its parents (`0x00033670`, [`root_value`]); 0 when a handle does not resolve |
//! | scalar 23 | `0.299·(L+0x10 + L+0x30) + 0.587·(…+4) + 0.114·(…+8)`, summed blue, green, red in that order, clamped to 0..1; 1 without a lighting |
//!
//! What the inputs mean (which is ambient, which a light's colour or
//! direction) is the vertex programs' business and not read yet; the port
//! gives the values the programs get.

use crate::camera::GuestRead;

/// The current lighting pointer the draw sets (`0x0001bd50`).
pub const CURRENT_LIGHTING: u32 = 0x0048_5a58;
/// Pointer to the default record; its `+4` is the default vector.
pub const DEFAULT_VECTOR: u32 = 0x0046_86d4;
/// Pointer to the default colour (three floats).
pub const DEFAULT_COLOUR: u32 = 0x0046_8710;
/// The shader's scalar 21 (`shad +0x44`), copied per draw by `0x0001bbf0`.
pub const SHADER_SCALAR_21: u32 = 0x0048_5a6c;
/// The shader's `+0x48`, copied per draw: the source of vector input 24.
pub const SHADER_VECTOR_24: u32 = 0x0048_5a70;
/// The scalar input evaluator (`EAX` the input; the value in `XMM0`).
pub const SCALAR_INPUTS: u32 = 0x0003_36f0;
/// The vector input evaluator (`EAX` the input, `ESI` the destination).
pub const VECTOR_INPUTS: u32 = 0x0003_3980;
/// Bytes of the lighting the inputs read, from `+0`.
pub const LIGHTING_BYTES: u32 = 0x78;
const WORDS: usize = (LIGHTING_BYTES / 4) as usize;

/// Vector inputs that read the lighting (bit `n`: input `n`).
pub const LIGHTING_VECTORS: u64 = 1 << 3
    | 1 << 4
    | 1 << 14
    | 1 << 15
    | 1 << 16
    | 1 << 17
    | 1 << 21
    | 1 << 22
    | 1 << 23
    | 1 << 24
    | 1 << 35
    | 1 << 40
    | 1 << 42
    | 1 << 43
    | 1 << 44
    | 1 << 45;
/// Scalar inputs that read the lighting.
pub const LIGHTING_SCALARS: u64 = 1 << 23 | 1 << 37;

/// The object data array (`[0x004e0300]`): `+0x24` the element size,
/// `+0x38` the element count, `+0x44` the elements; an element's `+0` is
/// its salt (`u16`), `+3` its type, `+8` the object.
pub const OBJECT_ARRAY: u32 = 0x004e_0300;
/// The object lookup by handle and type mask (`EAX` the handle).
pub const OBJECT_LOOKUP: u32 = 0x000b_adc0;
/// Vector input 40's and scalar input 37's value (`EAX` the handle).
pub const ROOT_VALUE: u32 = 0x0003_3670;
/// An object's attached flag (byte): its parent's value is read instead.
pub const OBJECT_ATTACHED: u32 = 0xaa;
/// An object's parent handle.
pub const OBJECT_PARENT: u32 = 0x14;
/// The root object's value the inputs read.
pub const OBJECT_ROOT_VALUE: u32 = 0x2b0;
/// The parent walk's element stride (`0x00033670` uses 12, not the
/// array's `+0x24`).
const WALK_STRIDE: u32 = 12;
/// Parents walked before the port gives up (the game has no bound; a
/// longer chain is a cycle or garbage).
const MAX_PARENTS: usize = 64;

/// One object's lighting: a copy of the `0x78` bytes the inputs read,
/// taken when the hook meets the object (the cache record is relit and
/// reused after the frame).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lighting {
    pub words: [u32; WORDS],
    /// [`root_value`] of the object `+0` names, read with the words;
    /// `None` when the walk did not finish (unreadable memory, a cycle):
    /// inputs 40 and 37 are then not given.
    pub root: Option<u32>,
}

impl Lighting {
    /// Copies the lighting at `at` (a cache record's `+0x44`) and its
    /// object's [`root_value`].
    pub fn read(mem: &dyn GuestRead, at: u32) -> Option<Self> {
        let mut words = [0u32; WORDS];
        for (i, w) in words.iter_mut().enumerate() {
            *w = mem.read_u32(at.checked_add(4 * i as u32)?)?;
        }
        Some(Self { words, root: root_value(mem, words[0]) })
    }

    fn word(&self, offset: u32) -> u32 {
        self.words[(offset / 4) as usize]
    }

    fn vector(&self, offset: u32) -> [u32; 3] {
        [self.word(offset), self.word(offset + 4), self.word(offset + 8)]
    }

    /// The colour slots in use (`+0x74`).
    pub fn colour_count(&self) -> u8 {
        self.word(0x74) as u8
    }

    /// The byte `+0x76` the build picks the implementation by.
    pub fn implementation_byte(&self) -> u8 {
        (self.word(0x74) >> 16) as u8
    }
}

/// What the inputs fall back on without a lighting (or past the colour
/// count), read from the globals at the same time as the lighting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Defaults {
    pub vector: [u32; 3],
    pub colour: [u32; 3],
}

impl Defaults {
    pub fn read(mem: &dyn GuestRead) -> Option<Self> {
        let three = |at: u32| -> Option<[u32; 3]> {
            Some([mem.read_u32(at)?, mem.read_u32(at.checked_add(4)?)?, mem.read_u32(at.checked_add(8)?)?])
        };
        let vector = three(mem.read_u32(DEFAULT_VECTOR)?.checked_add(4)?)?;
        let colour = three(mem.read_u32(DEFAULT_COLOUR)?)?;
        Some(Self { vector, colour })
    }
}

/// The shader instance's two values the inputs scale by (`shad +0x44`,
/// `+0x48`), as raw bits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShaderValues {
    pub scalar_21: u32,
    pub vector_24: u32,
}

/// `0.0 ..= 1.0` as the evaluators clamp (`comiss`): below 0 is 0, above
/// 1 is 1, and NaN stays.
fn clamp01(x: f32) -> f32 {
    if 0.0 > x {
        0.0
    } else if x > 1.0 {
        1.0
    } else {
        x
    }
}

fn floats(v: [u32; 3]) -> [f32; 3] {
    v.map(f32::from_bits)
}

fn bits(v: [f32; 3]) -> [u32; 3] {
    v.map(f32::to_bits)
}

/// `0x000badc0` with the mask -1: the object of `handle`, or 0 when the
/// handle is -1, its index is past the array's count (a signed compare),
/// or its element's salt is 0 or not the handle's high half. The type
/// test always passes with every bit of the mask set.
pub fn object(mem: &dyn GuestRead, handle: u32) -> Option<u32> {
    if handle == u32::MAX {
        return Some(0);
    }
    let array = mem.read_u32(OBJECT_ARRAY)?;
    let count = mem.read_u32(array.checked_add(0x38)?)? as i32;
    let index = handle & 0xffff;
    if index as i32 >= count {
        return Some(0);
    }
    let size = mem.read_u32(array.checked_add(0x24)?)?;
    let element = size.wrapping_mul(index).wrapping_add(mem.read_u32(array.checked_add(0x44)?)?);
    let salt = mem.read_u32(element)? as u16;
    if salt == 0 || i32::from(salt as i16) != (handle as i32) >> 16 {
        return Some(0);
    }
    mem.read_u32(element.checked_add(8)?)
}

/// `0x00033670`: the value vector input 40 (in all three components) and
/// scalar input 37 give for the object `handle` (the lighting's `+0`), as
/// raw float bits. From the object, while its byte `+0xaa` is set, go to
/// its parent (`+0x14`; the element read straight from the array's
/// `+0x44` at a stride of 12, no salt check); at the first object without
/// it, re-read the object of the last handle the same way and take its
/// `+0x2b0`, clamped to 0..1 (NaN stays). 0 when the handle does not
/// resolve, a parent is -1, or a parent's element holds no object.
/// `None` when memory does not read or the chain does not end.
pub fn root_value(mem: &dyn GuestRead, handle: u32) -> Option<u32> {
    root_word(mem, handle).map(|v| clamp01(f32::from_bits(v)).to_bits())
}

/// Root object input before the evaluator's two COMISS instructions.
/// The live material binder applies them under the current guest MXCSR.
pub fn root_word(mem: &dyn GuestRead, handle: u32) -> Option<u32> {
    let zero = Some(0.0f32.to_bits());
    let mut body = object(mem, handle)?;
    if body == 0 {
        return zero;
    }
    let elements = || -> Option<u32> { mem.read_u32(mem.read_u32(OBJECT_ARRAY)?.checked_add(0x44)?) };
    let at = |h: u32| -> Option<u32> {
        let e = WALK_STRIDE.wrapping_mul(h & 0xffff).wrapping_add(elements()?);
        mem.read_u32(e.checked_add(8)?)
    };
    let mut last = handle;
    for _ in 0..MAX_PARENTS {
        let word = mem.read_u32(body.checked_add(OBJECT_ATTACHED & !3)?)?;
        if (word >> (8 * (OBJECT_ATTACHED & 3))) & 0xff == 0 {
            let root = at(last)?;
            return mem.read_u32(root.checked_add(OBJECT_ROOT_VALUE)?);
        }
        let parent = mem.read_u32(body.checked_add(OBJECT_PARENT)?)?;
        if parent == u32::MAX {
            return zero;
        }
        last = parent;
        body = at(parent)?;
        if body == 0 {
            return zero;
        }
    }
    None
}

/// `0x00131bb0`: a packed `0x00RRGGBB` word to three floats over 255.
pub fn unpack_colour(c: u32) -> [u32; 3] {
    let channel = |shift: u32| ((((c >> shift) & 0xff) as f32) * (1.0f32 / 255.0)).to_bits();
    [channel(16), channel(8), channel(0)]
}

/// The colour slot of vector input `input`, if it is one.
fn colour_slot(input: u32) -> Option<u32> {
    match input {
        3 => Some(0),
        4 => Some(1),
        21 => Some(2),
        22 => Some(3),
        42..=45 => Some(input - 42),
        _ => None,
    }
}

/// Vector input `input` as `0x00033980` computes it with `lighting` the
/// current lighting (`None`: `[0x00485a58]` null), as raw float bits.
/// `None` for an input that does not read the lighting, or that the port
/// cannot give (40 without a lighting or its [`Lighting::root`]).
pub fn vector_input(input: u32, lighting: Option<&Lighting>, defaults: &Defaults, shader: ShaderValues) -> Option<[u32; 3]> {
    if let Some(slot) = colour_slot(input) {
        return Some(match lighting {
            Some(l) if slot < u32::from(l.colour_count()) => unpack_colour(l.word(0x64 + 4 * slot)),
            _ => defaults.colour,
        });
    }
    if input == 40 {
        // `0x00033edd` reads `[[0x00485a58]]` without a null check: no
        // lighting is not a case the game draws.
        return lighting?.root.map(|r| [r; 3]);
    }
    let base = |offset: u32| lighting.map_or(defaults.vector, |l| l.vector(offset));
    let scaled = |offset: u32, f: &dyn Fn(f32) -> f32| bits(floats(base(offset)).map(|c| clamp01(f(c))));
    let s21 = f32::from_bits(shader.scalar_21);
    let s24 = f32::from_bits(shader.vector_24);
    Some(match input {
        14 => base(0x3c),
        15 => base(0x30),
        16 => base(0x54),
        17 => base(0x48),
        23 => scaled(0x30, &|c| c * s21),
        35 => scaled(0x30, &|c| c * s21 * 0.5),
        24 => scaled(0x10, &|c| (s24 + 1.0) * c),
        _ => return None,
    })
}

/// Scalar input `input` as `0x000336f0` computes it from the lighting, as
/// raw float bits; `None` for one that does not read the lighting or that
/// the port cannot give (37 without a lighting or its [`Lighting::root`];
/// `0x00033850` reads `[[0x00485a58]]` without a null check).
pub fn scalar_input(input: u32, lighting: Option<&Lighting>) -> Option<u32> {
    if input == 37 {
        return lighting?.root;
    }
    if input != 23 {
        return None;
    }
    let Some(l) = lighting else { return Some(1.0f32.to_bits()) };
    let f = |o: u32| f32::from_bits(l.word(o));
    let red = f(0x10) + f(0x30);
    let green = f(0x14) + f(0x34);
    let blue = f(0x18) + f(0x38);
    Some(clamp01(blue * 0.114 + green * 0.587 + red * 0.299).to_bits())
}

/// Whether it reads the lighting at all.
pub fn reads_lighting(vectors: u64, scalars: u64) -> bool {
    vectors & LIGHTING_VECTORS != 0 || scalars & LIGHTING_SCALARS != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::tests::Mem;

    const L: u32 = 0x0070_0000;

    fn lighting(m: &mut Mem) -> Lighting {
        for i in 0..WORDS as u32 {
            m.w(L + 4 * i, (i as f32 * 0.25).to_bits());
        }
        // Two colour slots; +0x76 = 0xff.
        m.w(L + 0x64, 0x00ff_8000);
        m.w(L + 0x68, 0x0000_00ff);
        m.w(L + 0x74, 0x00ff_0002);
        Lighting::read(m, L).unwrap()
    }

    fn defaults() -> Defaults {
        Defaults {
            vector: [0.5f32, 0.25, 2.0].map(f32::to_bits),
            colour: [0.1f32, 0.2, 0.3].map(f32::to_bits),
        }
    }

    fn v(x: Option<[u32; 3]>) -> [f32; 3] {
        floats(x.expect("ported"))
    }

    #[test]
    fn the_lighting_vectors_are_its_words_or_the_default() {
        let mut m = Mem::default();
        let l = lighting(&mut m);
        let d = defaults();
        let s = ShaderValues::default();
        // Word n holds n / 4: +0x3c is word 15.
        assert_eq!(v(vector_input(14, Some(&l), &d, s)), [3.75, 4.0, 4.25]);
        assert_eq!(v(vector_input(15, Some(&l), &d, s)), [3.0, 3.25, 3.5]);
        assert_eq!(v(vector_input(16, Some(&l), &d, s)), [5.25, 5.5, 5.75]);
        assert_eq!(v(vector_input(17, Some(&l), &d, s)), [4.5, 4.75, 5.0]);
        for input in [14, 15, 16, 17] {
            assert_eq!(vector_input(input, None, &d, s), Some(d.vector), "input {input}");
        }
        assert_eq!(vector_input(5, Some(&l), &d, s), None, "not a lighting input");
        assert_eq!(vector_input(40, None, &d, s), None, "no lighting: the game does not draw it");
        assert_eq!((l.colour_count(), l.implementation_byte()), (2, 0xff));
    }

    #[test]
    fn scaled_inputs_clamp_to_one_and_keep_nan() {
        let mut m = Mem::default();
        let l = lighting(&mut m);
        let d = defaults();
        let s = ShaderValues {
            scalar_21: 0.25f32.to_bits(),
            vector_24: (-0.5f32).to_bits(),
        };
        // +0x30: 3, 3.25, 3.5; times 0.25 then clamped.
        assert_eq!(v(vector_input(23, Some(&l), &d, s)), [0.75, 0.8125, 0.875]);
        assert_eq!(v(vector_input(35, Some(&l), &d, s)), [0.375, 0.40625, 0.4375]);
        // +0x10: 1, 1.25, 1.5; times (1 - 0.5).
        assert_eq!(v(vector_input(24, Some(&l), &d, s)), [0.5, 0.625, 0.75]);
        // The default, scaled the same way: 2 * 0.25 = 0.5.
        assert_eq!(v(vector_input(23, None, &d, s)), [0.125, 0.0625, 0.5]);
        let big = ShaderValues { scalar_21: 4.0f32.to_bits(), ..s };
        assert_eq!(v(vector_input(23, Some(&l), &d, big)), [1.0, 1.0, 1.0]);
        let negative = ShaderValues { scalar_21: (-1.0f32).to_bits(), ..s };
        assert_eq!(v(vector_input(23, Some(&l), &d, negative)), [0.0, 0.0, 0.0]);
        let nan = ShaderValues { scalar_21: f32::NAN.to_bits(), ..s };
        assert!(v(vector_input(23, Some(&l), &d, nan)).iter().all(|c| c.is_nan()));
    }

    #[test]
    fn colour_slots_unpack_below_the_count_and_default_past_it() {
        let mut m = Mem::default();
        let l = lighting(&mut m);
        let d = defaults();
        let s = ShaderValues::default();
        let red = [1.0, 128.0 / 255.0, 0.0];
        for input in [3, 42] {
            let c = v(vector_input(input, Some(&l), &d, s));
            assert!(c.iter().zip(red).all(|(a, b)| (a - b).abs() < 1e-6), "input {input}: {c:?}");
        }
        assert_eq!(v(vector_input(4, Some(&l), &d, s)), [0.0, 0.0, 1.0]);
        assert_eq!(vector_input(43, Some(&l), &d, s), vector_input(4, Some(&l), &d, s));
        // Slots 2 and 3 are past the count; no lighting: the default.
        for input in [21, 22, 44, 45] {
            assert_eq!(vector_input(input, Some(&l), &d, s), Some(d.colour), "input {input}");
        }
        assert_eq!(vector_input(3, None, &d, s), Some(d.colour));
        assert_eq!(unpack_colour(0x00ff_ffff), [1.0f32.to_bits(); 3]);
    }

    #[test]
    fn scalar_23_is_the_summed_colours_luminance() {
        let mut m = Mem::default();
        let mut l = lighting(&mut m);
        assert_eq!(scalar_input(23, None), Some(1.0f32.to_bits()));
        for w in l.words.iter_mut() {
            *w = 0;
        }
        // +0x10 + +0x30: red 0.5 + 0.5, green 0, blue 0.
        l.words[4] = 0.5f32.to_bits();
        l.words[12] = 0.5f32.to_bits();
        assert_eq!(scalar_input(23, Some(&l)), Some(0.299f32.to_bits()));
        l.words[5] = 2.0f32.to_bits();
        assert_eq!(scalar_input(23, Some(&l)), Some(1.0f32.to_bits()), "clamped");
        assert_eq!(scalar_input(21, Some(&l)), None);
    }

    #[test]
    fn the_masks_cover_the_ported_and_the_rest() {
        assert!(reads_lighting(1 << 15, 0) && reads_lighting(0, 1 << 23));
        assert!(!reads_lighting(1 << 5, 1 << 21));
        let d = Defaults::default();
        let l = Lighting { words: [0; WORDS], root: Some(0) };
        for input in 0..64 {
            let ported = vector_input(input, Some(&l), &d, ShaderValues::default()).is_some();
            assert_eq!(ported, LIGHTING_VECTORS & (1u64 << input) != 0, "vector input {input}");
            let ported = scalar_input(input, Some(&l)).is_some();
            assert_eq!(ported, LIGHTING_SCALARS & (1u64 << input) != 0, "scalar input {input}");
        }
    }
    const ARRAY: u32 = 0x0071_0000;
    const ELEMENTS: u32 = 0x0071_1000;

    /// An object array of 12-byte elements; `objects[i]` = (salt, object,
    /// attached, parent, value).
    fn objects(m: &mut Mem, objects: &[(u16, u32, bool, u32, f32)]) {
        m.w(OBJECT_ARRAY, ARRAY);
        m.w(ARRAY + 0x24, 12);
        m.w(ARRAY + 0x38, objects.len() as u32);
        m.w(ARRAY + 0x44, ELEMENTS);
        for (i, &(salt, body, attached, parent, value)) in objects.iter().enumerate() {
            let e = ELEMENTS + 12 * i as u32;
            m.w(e, u32::from(salt));
            m.w(e + 8, body);
            if body != 0 {
                m.w(body + 0xa8, u32::from(attached) << 16);
                m.w(body + OBJECT_PARENT, parent);
                m.w(body + OBJECT_ROOT_VALUE, value.to_bits());
            }
        }
    }

    fn handle(salt: u16, index: u32) -> u32 {
        u32::from(salt) << 16 | index
    }

    #[test]
    fn the_root_value_walks_attached_objects_to_their_root() {
        let mut m = Mem::default();
        objects(
            &mut m,
            &[
                (0x8001, 0x0072_0000, false, u32::MAX, 0.25),
                (0x8002, 0x0072_1000, true, handle(0x8001, 0), 0.75),
                (0x8003, 0x0072_2000, true, handle(0x8002, 1), 0.5),
                (0x8004, 0x0072_3000, false, u32::MAX, 3.0),
                (0x8005, 0x0072_4000, true, u32::MAX, 0.5),
                (0x8006, 0x0072_5000, false, u32::MAX, f32::NAN),
            ],
        );
        let f = |h| root_value(&m, h).map(f32::from_bits);
        assert_eq!(f(handle(0x8001, 0)), Some(0.25), "a root is its own");
        assert_eq!(f(handle(0x8003, 2)), Some(0.25), "two parents up");
        assert_eq!(f(handle(0x8004, 3)), Some(1.0), "clamped");
        assert_eq!(f(handle(0x8005, 4)), Some(0.0), "attached to nothing");
        assert!(f(handle(0x8006, 5)).unwrap().is_nan());
        assert_eq!(f(handle(0x8002, 3)), Some(0.0), "wrong salt");
        assert_eq!(f(handle(0x8001, 9)), Some(0.0), "past the count");
        assert_eq!(f(u32::MAX), Some(0.0));
        let l = Lighting { words: [0; WORDS], root: f(handle(0x8003, 2)).map(f32::to_bits) };
        let d = Defaults::default();
        assert_eq!(v(vector_input(40, Some(&l), &d, ShaderValues::default())), [0.25; 3]);
        assert_eq!(scalar_input(37, Some(&l)), Some(0.25f32.to_bits()));
        assert_eq!(scalar_input(37, None), None);
        // Read with the words: +0 is the object's handle.
        for i in 0..WORDS as u32 {
            m.w(0x0073_0000 + 4 * i, 0);
        }
        m.w(0x0073_0000, handle(0x8002, 1));
        assert_eq!(Lighting::read(&m, 0x0073_0000).unwrap().root, Some(0.25f32.to_bits()));
    }

    #[test]
    fn a_parent_cycle_is_not_given() {
        let mut m = Mem::default();
        objects(
            &mut m,
            &[(0x8001, 0x0072_0000, true, handle(0x8002, 1), 0.0), (0x8002, 0x0072_1000, true, handle(0x8001, 0), 0.0)],
        );
        assert_eq!(root_value(&m, handle(0x8001, 0)), None);
        let l = Lighting { words: [0; WORDS], root: None };
        assert_eq!(vector_input(40, Some(&l), &Defaults::default(), ShaderValues::default()), None);
    }
}
