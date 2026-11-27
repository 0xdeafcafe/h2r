//! The shared "function" (curve) data embedded in effects, particles and
//! particle movement, and a port of the engine's evaluator.
//!
//! Evidence labels (see `docs/formats/effects.md`): **E** the engine's evaluator
//! (`0x0013b390`, with the helpers `0x0013bec0`, `0x0017c900`, `0x0017ca10` and
//! the property wrapper `0x00246cd0`) reads this; **V** checked on every
//! instance of the retail disc; **S** a name taken from public vocabulary only.
//!
//! A function blob is a `0x14`-byte header followed by two *sets* of per-type
//! data (set 0 is always used; set 1 only when header flag bit 0 is set, in
//! which case the two results are blended by the "range input"). The blob
//! length is `0x14 + 8 * sets_len(type)` for every type; the engine reads
//! nothing outside it. Every evaluation result is clamped to `[0, 1]`;
//! [`Property::evaluate`] then optionally maps that onto the header range.
use super::mem::Property;
use crate::{Error, Result};
use std::sync::OnceLock;

/// Bytes before the per-type data.
pub const HEADER_LEN: usize = 0x14;
/// Upper bound accepted for a blob of an unknown (unevaluated) type.
pub const MAX_UNKNOWN_LEN: usize = 1 << 16;
/// The wave a new periodic set names (cosine, one of the procedural ones).
const DEFAULT_WAVE: u8 = 2;

/// Function type, header byte 0 (**E**: the jump table has eleven entries).
/// Names are **S** vocabulary; behaviour is **E**.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FunctionType {
    /// 0: output is the input.
    Identity,
    /// 1: output is 0, or the range input when ranged (the header range then
    /// supplies the constant).
    Constant,
    /// 2: eased interpolation between two values by a shape curve.
    Transition,
    /// 3: periodic wave.
    Periodic,
    /// 4: straight line `a * x + b`.
    Linear,
    /// 5: piecewise-linear through four keys (three clamped ramps).
    LinearKey,
    /// 6: the engine has no evaluator (returns 0); preserved.
    MultiLinearKey,
    /// 7: cubic polynomial.
    Spline,
    /// 8: the engine has no evaluator (returns 0); preserved.
    MultiSpline,
    /// 9: power curve (no instance on the disc; layout from the engine only).
    Exponent,
    /// 10: cubic polynomial with its input re-parameterised from four points.
    Spline2,
    Unknown(u8),
}
impl FunctionType {
    /// The word the curve editor and the agent API name the type by
    /// (`periodic`); a type outside the list is `unknown_<byte>`.
    pub fn word(self) -> String {
        match self {
            Self::Identity => "identity".into(),
            Self::Constant => "constant".into(),
            Self::Transition => "transition".into(),
            Self::Periodic => "periodic".into(),
            Self::Linear => "linear".into(),
            Self::LinearKey => "linear_key".into(),
            Self::MultiLinearKey => "multi_linear_key".into(),
            Self::Spline => "spline".into(),
            Self::MultiSpline => "multi_spline".into(),
            Self::Exponent => "exponent".into(),
            Self::Spline2 => "spline_2".into(),
            Self::Unknown(n) => format!("unknown_{n}"),
        }
    }
    /// The type a word names (the curve editor's picker and
    /// `function.set` take it).
    pub fn from_word(word: &str) -> Option<Self> {
        Some(match word {
            "identity" => Self::Identity,
            "constant" => Self::Constant,
            "transition" => Self::Transition,
            "periodic" => Self::Periodic,
            "linear" => Self::Linear,
            "linear_key" => Self::LinearKey,
            "multi_linear_key" => Self::MultiLinearKey,
            "spline" => Self::Spline,
            "multi_spline" => Self::MultiSpline,
            "exponent" => Self::Exponent,
            "spline_2" => Self::Spline2,
            other => Self::Unknown(other.strip_prefix("unknown_")?.parse().ok()?),
        })
    }
    pub fn from_byte(b: u8) -> Self {
        match b {
            0 => Self::Identity,
            1 => Self::Constant,
            2 => Self::Transition,
            3 => Self::Periodic,
            4 => Self::Linear,
            5 => Self::LinearKey,
            6 => Self::MultiLinearKey,
            7 => Self::Spline,
            8 => Self::MultiSpline,
            9 => Self::Exponent,
            10 => Self::Spline2,
            n => Self::Unknown(n),
        }
    }
    pub fn to_byte(self) -> u8 {
        match self {
            Self::Identity => 0,
            Self::Constant => 1,
            Self::Transition => 2,
            Self::Periodic => 3,
            Self::Linear => 4,
            Self::LinearKey => 5,
            Self::MultiLinearKey => 6,
            Self::Spline => 7,
            Self::MultiSpline => 8,
            Self::Exponent => 9,
            Self::Spline2 => 10,
            Self::Unknown(n) => n,
        }
    }
    /// Per-set size in 4-byte words (**E**: the table at `0x0044178c`, read by
    /// the engine's point fetch helper; **V** against every blob size).
    fn set_words(self) -> Option<usize> {
        Some(match self {
            Self::Identity => 0,
            Self::Constant => 1,
            Self::Transition => 2,
            Self::Periodic => 4,
            Self::Linear => 6,
            Self::LinearKey => 0x14,
            Self::MultiLinearKey => 0x20,
            Self::Spline | Self::Spline2 => 12,
            Self::MultiSpline => 4,
            Self::Exponent => 3,
            Self::Unknown(_) => return None,
        })
    }
    /// Exact blob length for a known type.
    pub fn blob_len(self) -> Option<usize> {
        self.set_words().map(|w| HEADER_LEN + 8 * w)
    }
    /// Whether this engine build evaluates the type (6 and 8 return 0).
    pub fn is_evaluated(self) -> bool {
        !matches!(
            self,
            Self::MultiLinearKey | Self::MultiSpline | Self::Unknown(_)
        )
    }
}

/// One key of a linear key set (type 5): the value the key adds, ramping
/// from `position` over `inverse_span` (the ramp's inverse length).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Key {
    pub value: f32,
    pub position: f32,
    pub inverse_span: f32,
}

/// One set of a function's per-type payload, typed: the words the engine's
/// evaluator reads, named as the curve editor's handles
/// (docs/editor/rich-editors.md, "Curves"). Word indexes are the
/// evaluator's own ([`Function::evaluate_with`]; the table in
/// docs/formats/effects.md). Words the evaluator does not read (a
/// constant's, a linear's first four, a spline's first eight, a linear
/// key's first nine and word 12) are not part of the typed value:
/// [`Function::write_set`] keeps them, so a typed edit round-trips
/// through [`Function::to_bytes`].
#[derive(Clone, Debug, PartialEq)]
pub enum Set {
    /// Type 0 (identity): no words.
    Identity,
    /// Types 1 (constant), 6 (multi linear key) and 8 (multi spline): a
    /// layout but no word the engine's evaluator reads, given as they
    /// are. Types 6 and 8 have no evaluator, so the curve editor shows
    /// them read-only.
    Words(Vec<f32>),
    /// Type 2 (transition): the two end values.
    Transition { from: f32, to: f32 },
    /// Type 3 (periodic): `wave(frequency * x + phase)` mapped between
    /// minimum and maximum.
    Periodic {
        frequency: f32,
        phase: f32,
        minimum: f32,
        maximum: f32,
    },
    /// Type 4 (linear): `slope * x + intercept`.
    Linear { slope: f32, intercept: f32 },
    /// Type 5 (linear key): the value before the first key, then the
    /// three keys.
    LinearKey { base: f32, keys: [Key; 3] },
    /// Types 7 and 10 (spline, spline 2): the cubic
    /// `c3 t³ + c2 t² + c1 t + c0` and the four points spline 2's input
    /// re-parameterisation reads (type 7 keeps them unread).
    Spline {
        points: [[f32; 2]; 4],
        coefficients: [f32; 4],
    },
    /// Type 9 (exponent): `minimum + (maximum - minimum) * x ^ power`.
    Exponent {
        minimum: f32,
        maximum: f32,
        power: f32,
    },
}

/// A parsed function blob. Round-trips: [`Function::to_bytes`] reproduces the
/// parsed bytes exactly.
#[derive(Clone, Debug, PartialEq)]
pub struct Function {
    pub kind: FunctionType,
    /// Header byte 1. Bit 0 (**E**): ranged, i.e. set 1 is used and blended by
    /// the range input. Any bit in `0xF0` (**E**, property wrapper): the
    /// output is used as is instead of being mapped onto [`Self::range`].
    pub flags: u8,
    /// Header bytes 2 and 3 (**E**): the wave / shape selector of set 0 and
    /// set 1 for the periodic and transition types; unused by the others.
    pub sub_types: [u8; 2],
    /// Header floats at +4 and +8 (**E**): output range, applied by the
    /// property wrapper when `flags & 0xF0 == 0`.
    pub range: [f32; 2],
    /// Header +0xc..+0x14: retained as bytes. Shader colour outputs can use both words as packed colours.
    pub unknown_0c: [u8; 8],
    /// Everything after the header (two sets).
    pub payload: Vec<u8>,
}

impl Function {
    /// Parse and size-check a function blob. Known types must have exactly
    /// [`FunctionType::blob_len`] bytes; unknown types keep any length from the
    /// header size up to [`MAX_UNKNOWN_LEN`] (the engine evaluates them as 0).
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < HEADER_LEN {
            return Err(Error::Invalid("function data shorter than its header"));
        }
        let kind = FunctionType::from_byte(bytes[0]);
        match kind.blob_len() {
            Some(n) if bytes.len() != n => {
                return Err(Error::Invalid("function data length differs from its type"));
            }
            None if bytes.len() > MAX_UNKNOWN_LEN => {
                return Err(Error::Invalid("unknown function type is too large"));
            }
            _ => {}
        }
        let f = |o: usize| f32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
        Ok(Self {
            kind,
            flags: bytes[1],
            sub_types: [bytes[2], bytes[3]],
            range: [f(4), f(8)],
            unknown_0c: bytes[0xc..0x14].try_into().expect("header length checked"),
            payload: bytes[HEADER_LEN..].to_vec(),
        })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(HEADER_LEN + self.payload.len());
        v.extend([
            self.kind.to_byte(),
            self.flags,
            self.sub_types[0],
            self.sub_types[1],
        ]);
        v.extend(self.range[0].to_le_bytes());
        v.extend(self.range[1].to_le_bytes());
        v.extend(self.unknown_0c);
        v.extend(&self.payload);
        v
    }

    /// True when set 1 is used and blended by the range input.
    pub fn is_ranged(&self) -> bool {
        self.flags & 1 != 0
    }
    /// True when the property wrapper uses the output unmapped.
    pub fn is_raw_output(&self) -> bool {
        self.flags & 0xf0 != 0
    }

    /// Packed ARGB from a shader colour property (0x0013bc00). The high
    /// nibble selects how many colours the function header supplies.
    pub fn output_colour(&self, raw: f32) -> u32 {
        let [a, b] = self.range.map(f32::to_bits);
        let c = u32::from_le_bytes(self.unknown_0c[..4].try_into().unwrap());
        let d = u32::from_le_bytes(self.unknown_0c[4..].try_into().unwrap());
        let kind = self.flags >> 4;
        if self.kind == FunctionType::Constant && (!self.is_ranged() || kind <= 1) {
            return a;
        }
        let x = raw.clamp(0.0, 1.0);
        let stops: &[u32] = match kind {
            0 => {
                let grey = (x * 255.0).round_ties_even() as u32;
                return 0xff00_0000 | (grey * 0x010101);
            }
            1 => return a,
            2 => &[a, d],
            3 => &[a, b, d],
            4 => &[a, b, c, d],
            _ => return u32::MAX,
        };
        let at = x * (stops.len() - 1) as f32;
        let i = (at as usize).min(stops.len() - 2);
        // The MMX interpolation scales the weight to 14 fractional bits.
        let weight = ((at - i as f32) * 16384.0).round_ties_even() as i32;
        let [left, right] = [stops[i], stops[i + 1]];
        let mut colour = 0;
        for shift in [0, 8, 16, 24] {
            let lo = ((left >> shift) & 255) as i32;
            let hi = ((right >> shift) & 255) as i32;
            let channel = lo + (((hi - lo) * weight + 8192) >> 14);
            colour |= (channel.clamp(0, 255) as u32) << shift;
        }
        colour
    }

    /// Set `i`'s typed values (0 or 1), from the words the engine's
    /// evaluator reads. Types 6 and 8 have a layout but no evaluator, so
    /// their words are given unnamed; an unknown type has no layout, so
    /// it has no sets.
    pub fn set_values(&self, i: usize) -> Option<Set> {
        let words = self.kind.set_words()?;
        let base = i.checked_mul(words).filter(|_| i < 2)?;
        let w = |k: usize| self.w(base + k);
        Some(match self.kind {
            FunctionType::Identity => Set::Identity,
            FunctionType::Constant | FunctionType::MultiLinearKey | FunctionType::MultiSpline => {
                Set::Words((0..words).map(&w).collect())
            }
            FunctionType::Transition => Set::Transition {
                from: w(0),
                to: w(1),
            },
            FunctionType::Periodic => Set::Periodic {
                frequency: w(0),
                phase: w(1),
                minimum: w(2),
                maximum: w(3),
            },
            FunctionType::Linear => Set::Linear {
                slope: w(4),
                intercept: w(5),
            },
            FunctionType::LinearKey => Set::LinearKey {
                base: w(16),
                keys: [0, 1, 2].map(|k| Key {
                    value: w(17 + k),
                    position: w(9 + k),
                    inverse_span: w(13 + k),
                }),
            },
            FunctionType::Spline | FunctionType::Spline2 => Set::Spline {
                points: [0, 1, 2, 3].map(|k| [w(2 * k), w(2 * k + 1)]),
                coefficients: [w(8), w(9), w(10), w(11)],
            },
            FunctionType::Exponent => Set::Exponent {
                minimum: w(0),
                maximum: w(1),
                power: w(2),
            },
            FunctionType::Unknown(_) => return None,
        })
    }

    /// Write `values` into set `i`'s words (0 or 1): the type's own named
    /// words only, every other word kept, so the function round-trips
    /// through [`Self::to_bytes`]. Refused for a set the type does not
    /// have, a value of another type's shape, and an unknown type (no
    /// layout).
    pub fn write_set(&mut self, i: usize, values: &Set) -> Result<()> {
        let words = self
            .kind
            .set_words()
            .ok_or(Error::Invalid("an unknown function type has no layout"))?;
        if i > 1 {
            return Err(Error::Invalid("a function has two sets: 0 and 1"));
        }
        let base = i * words;
        let mut put = |k: usize, v: f32| {
            let at = 4 * (base + k);
            self.payload[at..at + 4].copy_from_slice(&v.to_le_bytes());
        };
        match values {
            Set::Identity if self.kind == FunctionType::Identity => {}
            Set::Words(list) => {
                let named = matches!(
                    self.kind,
                    FunctionType::Constant
                        | FunctionType::MultiLinearKey
                        | FunctionType::MultiSpline
                );
                if !named {
                    return Err(Error::Invalid(
                        "this type's set is not given as unnamed words",
                    ));
                }
                if list.len() != words {
                    return Err(Error::Invalid("the word count differs from the type's set"));
                }
                for (k, v) in list.iter().enumerate() {
                    put(k, *v);
                }
            }
            Set::Transition { from, to } if self.kind == FunctionType::Transition => {
                put(0, *from);
                put(1, *to);
            }
            Set::Periodic {
                frequency,
                phase,
                minimum,
                maximum,
            } if self.kind == FunctionType::Periodic => {
                put(0, *frequency);
                put(1, *phase);
                put(2, *minimum);
                put(3, *maximum);
            }
            Set::Linear { slope, intercept } if self.kind == FunctionType::Linear => {
                put(4, *slope);
                put(5, *intercept);
            }
            Set::LinearKey { base, keys } if self.kind == FunctionType::LinearKey => {
                put(16, *base);
                for (k, key) in keys.iter().enumerate() {
                    put(17 + k, key.value);
                    put(9 + k, key.position);
                    put(13 + k, key.inverse_span);
                }
            }
            Set::Spline {
                points,
                coefficients,
            } if matches!(self.kind, FunctionType::Spline | FunctionType::Spline2) => {
                for (k, [x, y]) in points.iter().enumerate() {
                    put(2 * k, *x);
                    put(2 * k + 1, *y);
                }
                for (k, c) in coefficients.iter().enumerate() {
                    put(8 + k, *c);
                }
            }
            Set::Exponent {
                minimum,
                maximum,
                power,
            } if self.kind == FunctionType::Exponent => {
                put(0, *minimum);
                put(1, *maximum);
                put(2, *power);
            }
            _ => return Err(Error::Invalid("the value is another type's shape")),
        }
        Ok(())
    }

    /// This function given another type: the header's flags, range and
    /// unread bytes kept, the payload rebuilt at the new type's length
    /// ([`FunctionType::blob_len`]) with each set at a plain default that
    /// draws a visible curve: a transition from 0 to 1 by the linear
    /// shape, a periodic cosine of one cycle between 0 and 1, a line
    /// through the origin of slope 1, a linear key ramping from 0 to 1,
    /// a cubic that is `t`, an exponent that is `x`, a constant of 0.
    /// The sub-type bytes are the type's own defaults (the shape or wave
    /// each set names), 0 for a type that uses none. Refused for a type
    /// the engine does not evaluate (6, 8, unknown): it has no editable
    /// layout.
    pub fn with_kind(&self, kind: FunctionType) -> Result<Self> {
        if !kind.is_evaluated() {
            return Err(Error::Invalid("the game does not evaluate that type"));
        }
        let len = kind
            .blob_len()
            .ok_or(Error::Invalid("an unknown function type has no layout"))?;
        let mut f = Self {
            kind,
            flags: self.flags,
            sub_types: match kind {
                FunctionType::Periodic => [DEFAULT_WAVE; 2],
                _ => [0; 2],
            },
            range: self.range,
            unknown_0c: self.unknown_0c,
            payload: vec![0; len - HEADER_LEN],
        };
        let default = match kind {
            FunctionType::Transition => Set::Transition { from: 0.0, to: 1.0 },
            FunctionType::Periodic => Set::Periodic {
                frequency: 1.0,
                phase: 0.0,
                minimum: 0.0,
                maximum: 1.0,
            },
            FunctionType::Linear => Set::Linear {
                slope: 1.0,
                intercept: 0.0,
            },
            FunctionType::LinearKey => {
                let off = Key {
                    value: 0.0,
                    position: 0.0,
                    inverse_span: 0.0,
                };
                Set::LinearKey {
                    base: 0.0,
                    keys: [
                        Key {
                            value: 1.0,
                            position: 0.0,
                            inverse_span: 1.0,
                        },
                        off,
                        off,
                    ],
                }
            }
            FunctionType::Spline | FunctionType::Spline2 => Set::Spline {
                points: [
                    [0.0, 0.0],
                    [1.0 / 3.0, 1.0 / 3.0],
                    [2.0 / 3.0, 2.0 / 3.0],
                    [1.0, 1.0],
                ],
                coefficients: [0.0, 0.0, 1.0, 0.0],
            },
            FunctionType::Exponent => Set::Exponent {
                minimum: 0.0,
                maximum: 1.0,
                power: 1.0,
            },
            FunctionType::Constant => Set::Words(vec![0.0]),
            _ => Set::Identity,
        };
        for i in 0..2 {
            f.write_set(i, &default)?;
        }
        Ok(f)
    }

    /// Word `i` of the payload as a float (0.0 if out of range, which the
    /// size check makes impossible for known types).
    fn w(&self, i: usize) -> f32 {
        self.payload
            .get(i * 4..i * 4 + 4)
            .map_or(0.0, |b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// Evaluate with the built-in procedural tables ([`Tables::procedural`]).
    /// Waves that need engine-only tables evaluate as 0; see [`Tables`].
    pub fn evaluate(&self, input: f32, range_input: f32) -> f32 {
        self.evaluate_with(input, range_input, Tables::procedural())
    }

    /// Port of the engine evaluator `0x0013b390`. `input` is the function
    /// argument; `range_input` blends set 0 and set 1 when ranged. Types the
    /// engine does not evaluate (6, 8, unknown) return 0, exactly as it does.
    /// Results are clamped to `[0, 1]` (NaN passes through, as in the engine).
    pub fn evaluate_with(&self, x: f32, r: f32, tables: &Tables) -> f32 {
        let ranged = self.is_ranged();
        // The engine blends as (1 - r) * set0 + set1 * r everywhere.
        let blend = |a: f32, b: f32| (1.0 - r) * a + b * r;
        let res = match self.kind {
            FunctionType::Identity => x,
            FunctionType::Constant => {
                if !ranged {
                    return 0.0;
                }
                r
            }
            FunctionType::Transition => {
                let set = |base: usize, sub: u8| {
                    let s = shape(tables, sub, x);
                    (self.w(base + 1) - self.w(base)) * s + self.w(base)
                };
                let a = set(0, self.sub_types[0]);
                if ranged {
                    blend(a, set(2, self.sub_types[1]))
                } else {
                    a
                }
            }
            FunctionType::Periodic => {
                let set = |base: usize, sub: u8| {
                    let w = wave(tables, sub, self.w(base) * x + self.w(base + 1));
                    (self.w(base + 3) - self.w(base + 2)) * w + self.w(base + 2)
                };
                let a = set(0, self.sub_types[0]);
                if ranged {
                    blend(a, set(4, self.sub_types[1]))
                } else {
                    a
                }
            }
            FunctionType::Linear => {
                let a = self.w(4) * x + self.w(5);
                if ranged {
                    blend(a, self.w(10) * x + self.w(11))
                } else {
                    a
                }
            }
            FunctionType::LinearKey => {
                let set = |b: usize| {
                    let ramp = |k: usize| clamp01((x - self.w(b + 9 + k)) * self.w(b + 13 + k));
                    let (t1, t2, t3) = (ramp(0), ramp(1), ramp(2));
                    let mut s = self.w(b + 19) * t3;
                    s += self.w(b + 18) * t2;
                    s += self.w(b + 17) * t1;
                    s + self.w(b + 16)
                };
                let a = set(0);
                if ranged { blend(a, set(0x14)) } else { a }
            }
            FunctionType::Spline | FunctionType::Spline2 => {
                let remap = self.kind == FunctionType::Spline2;
                let cubic = |b: usize, t: f32| {
                    let t2 = t * t;
                    let t3 = t2 * t;
                    let mut s = self.w(b + 1) * t2;
                    s += self.w(b + 2) * t;
                    s += self.w(b) * t3;
                    s + self.w(b + 3)
                };
                // xmm0 in the engine: carried from set 0's remap into set 1's.
                let mut t = x;
                if remap {
                    t = self.spline_param(0, x, t);
                }
                let a = cubic(8, t);
                if ranged {
                    if remap {
                        t = self.spline_param(1, x, t);
                    }
                    blend(a, cubic(20, t))
                } else {
                    a
                }
            }
            FunctionType::Exponent => {
                let set = |b: usize| -> f32 {
                    let p = self.w(b + 2);
                    if 1e-4 > p.abs() || (0.0 > p && 1e-4 > x.abs()) {
                        return 1.0;
                    }
                    let pw = f64::from(x).powf(f64::from(p));
                    ((f64::from(self.w(b + 1)) - f64::from(self.w(b))) * pw + f64::from(self.w(b)))
                        as f32
                };
                let a = set(0);
                if ranged { blend(a, set(3)) } else { a }
            }
            FunctionType::MultiLinearKey | FunctionType::MultiSpline | FunctionType::Unknown(_) => {
                return 0.0;
            }
        };
        // Final clamp (0x0013b5f2): below 0 -> 0, above 1 -> 1, NaN unchanged.
        clamp01(res)
    }

    /// Spline2 input re-parameterisation for `set` (0 or 1): the engine's
    /// approximate sqrt-ratio of two point distances used as a power.
    /// `carry` is the engine's running parameter (its value when no remap
    /// applies), `x` the original input.
    fn spline_param(&self, set: usize, x: f32, carry: f32) -> f32 {
        let pt = |k: usize| {
            let o = 12 * set + 2 * k;
            (self.w(o), self.w(o + 1))
        };
        let (p0, p1, p2, p3) = (pt(0), pt(1), pt(2), pt(3));
        let dist2 = |a: (f32, f32), b: (f32, f32)| {
            let dx = a.0 - b.0;
            let dy = a.1 - b.1;
            dx * dx + dy * dy
        };
        let d1 = dist2(p2, p3);
        if d1 <= 1e-4 || d1.is_nan() {
            return carry;
        }
        let d0 = dist2(p0, p1);
        if 1e-4 > d0.abs() {
            return 1.0;
        }
        if 1e-4 > (d0 - d1).abs() {
            return carry;
        }
        // The engine approximates sqrt(d0) by an exponent-halving bit trick and
        // 1/sqrt(d1) by the hardware rsqrtss (relative error <= 1.5 * 2^-12);
        // an exact reciprocal square root is used here.
        let sqrt_d0 = f32::from_bits(((d0.to_bits() as i32 >> 1).wrapping_add(0x1fc0_0000)) as u32);
        let ratio = (1.0 / d1.sqrt()) * sqrt_d0;
        let pw = f64::from(x).powf(f64::from(ratio)) as f32;
        pw.clamp(0.0, 1.0)
    }
}

fn clamp01(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

/// x87 `fistp` with the default rounding mode (nearest, ties to even); out of
/// range and NaN give the "integer indefinite" value.
fn fistp(v: f32) -> i32 {
    if v.is_nan() || v.abs() >= 2_147_483_648.0 {
        i32::MIN
    } else {
        v.round_ties_even() as i32
    }
}

/// Number of table entries per curve.
pub const TABLE_LEN: usize = 1024;
const WAVE_SLOTS: usize = 12;
const SHAPE_SLOTS: usize = 6;
/// Cycles per 1024 table entries is 28; the engine multiplies by this.
const WAVE_SCALE: f32 = 36.57143;
const INV255: f32 = 0.003_921_569;

/// The engine's wave and shape lookup tables (1024 bytes each, **E**: static
/// data at `0x00468d48` (12 waves, slot 0 unused) and `0x0046bd48` (6 shapes)).
///
/// All six shapes and waves 1 (zero), 2 (cosine), 4 (triangle) and 6
/// (sawtooth) are reproduced procedurally and match the engine bytes exactly
/// (**V**, checked against the retail executable's data). The remaining waves
/// (3, 5, 7..=11) are pre-baked noise-like data that cannot be derived; they
/// must be supplied from the user's own executable with
/// [`Tables::with_engine_slab`] (this repository never contains them). Until
/// then they evaluate as 0.
#[derive(Clone, Debug)]
pub struct Tables {
    wave: [Option<Box<[u8; TABLE_LEN]>>; WAVE_SLOTS],
    shape: [Option<Box<[u8; TABLE_LEN]>>; SHAPE_SLOTS],
}
/// The engine's tables, installed once from a running game
/// ([`Tables::install_engine_slab`]).
static ENGINE: OnceLock<Tables> = OnceLock::new();

impl Tables {
    /// The tables that can be generated exactly.
    pub fn procedural() -> &'static Tables {
        static T: OnceLock<Tables> = OnceLock::new();
        T.get_or_init(Tables::build)
    }
    fn build() -> Self {
        let mut t = Tables {
            wave: Default::default(),
            shape: Default::default(),
        };
        let gen_shape = |f: fn(f64) -> f64| {
            let mut a = Box::new([0u8; TABLE_LEN]);
            for (i, v) in a.iter_mut().enumerate() {
                *v = (255.0 * f(i as f64 / 1023.0)).floor() as u8;
            }
            a
        };
        t.shape[0] = Some(gen_shape(|x| x));
        t.shape[1] = Some(gen_shape(f64::sqrt));
        t.shape[2] = Some(gen_shape(|x| x.powf(0.25)));
        t.shape[3] = Some(gen_shape(|x| x * x));
        t.shape[4] = Some(gen_shape(|x| x.powi(4)));
        t.shape[5] = Some(gen_shape(|x| {
            (1.0 - (std::f64::consts::PI * x).cos()) / 2.0
        }));
        let gen_wave = |f: fn(f32) -> f64| {
            let mut a = Box::new([0u8; TABLE_LEN]);
            for (i, v) in a.iter_mut().enumerate() {
                let q = i as f32 / WAVE_SCALE;
                let phase = q - q.floor();
                *v = (255.0 * f(phase)).floor() as u8;
            }
            a
        };
        t.wave[1] = Some(Box::new([0; TABLE_LEN]));
        let mut cos = Box::new([0u8; TABLE_LEN]);
        for (i, v) in cos.iter_mut().enumerate() {
            let phase = i as f64 / 36.571428571;
            *v = (255.0 * (0.5 + 0.5 * (2.0 * std::f64::consts::PI * phase).cos())).floor() as u8;
        }
        t.wave[2] = Some(cos);
        t.wave[4] = Some(gen_wave(|p| 1.0 - (2.0 * f64::from(p) - 1.0).abs()));
        t.wave[6] = Some(gen_wave(f64::from));
        t
    }
    /// Add the engine's own tables from a slab copied out of the user's
    /// executable: `0x468d48..0x46d548` (12 waves then 6 shapes, 18432 bytes).
    /// Present tables are replaced.
    pub fn with_engine_slab(mut self, slab: &[u8]) -> Result<Self> {
        if slab.len() != (WAVE_SLOTS + SHAPE_SLOTS) * TABLE_LEN {
            return Err(Error::RecordSize {
                actual: slab.len(),
                expected: (WAVE_SLOTS + SHAPE_SLOTS) * TABLE_LEN,
            });
        }
        let take = |i: usize| -> Box<[u8; TABLE_LEN]> {
            let mut a = Box::new([0u8; TABLE_LEN]);
            a.copy_from_slice(&slab[i * TABLE_LEN..(i + 1) * TABLE_LEN]);
            a
        };
        for i in 1..WAVE_SLOTS {
            self.wave[i] = Some(take(i));
        }
        for i in 0..SHAPE_SLOTS {
            self.shape[i] = Some(take(WAVE_SLOTS + i));
        }
        Ok(self)
    }
    /// A copy of the procedural tables, to extend with [`Self::with_engine_slab`].
    pub fn procedural_owned() -> Self {
        Self::procedural().clone()
    }
    /// The bytes [`Self::with_engine_slab`] takes: the engine's tables at
    /// `0x00468d48` (12 waves, then 6 shapes at `0x0046bd48`).
    pub const ENGINE_SLAB: std::ops::Range<u32> =
        0x0046_8d48..0x0046_8d48 + ((WAVE_SLOTS + SHAPE_SLOTS) * TABLE_LEN) as u32;
    /// Makes the engine's own tables, read from the running game's image
    /// ([`Self::ENGINE_SLAB`]), the ones [`Self::engine`] gives for the rest
    /// of the process. The tables are static data of the executable, the
    /// same for every map, so the first slab installed stays. Errors on a
    /// slab of the wrong size.
    pub fn install_engine_slab(slab: &[u8]) -> Result<()> {
        if ENGINE.get().is_none() {
            let _ = ENGINE.set(Self::procedural_owned().with_engine_slab(slab)?);
        }
        Ok(())
    }
    /// The engine's tables once a game has installed them
    /// ([`Self::install_engine_slab`]), else [`Self::procedural`]: what a
    /// native evaluation of a material's animation reads, so a wave that
    /// cannot be generated (3, 5, 7..=11) has its value where the game runs.
    pub fn engine() -> &'static Tables {
        ENGINE.get().unwrap_or_else(|| Self::procedural())
    }
    /// Whether [`Self::engine`] holds the engine's own tables.
    pub fn engine_installed() -> bool {
        ENGINE.get().is_some()
    }
    /// Wave table `sub` (1..=11), if present.
    pub fn wave_table(&self, sub: u8) -> Option<&[u8; TABLE_LEN]> {
        self.wave.get(usize::from(sub)).and_then(|t| t.as_deref())
    }
    /// Shape table `sub` (0..=5), if present.
    pub fn shape_table(&self, sub: u8) -> Option<&[u8; TABLE_LEN]> {
        self.shape.get(usize::from(sub)).and_then(|t| t.as_deref())
    }
    /// Whether wave `sub` is available.
    pub fn has_wave(&self, sub: u8) -> bool {
        sub == 0 || self.wave.get(usize::from(sub)).is_some_and(Option::is_some)
    }
}

/// `0x0017c900`: periodic wave value in `[0, 1]` (sub-type 0 is constant 1).
fn wave(tables: &Tables, sub: u8, arg: f32) -> f32 {
    if sub == 0 {
        return 1.0;
    }
    let Some(Some(t)) = tables.wave.get(usize::from(sub)) else {
        return 0.0;
    };
    let v = arg * WAVE_SCALE;
    let frac = (f64::from(v) % 1.0) as f32; // x87 fmod(v, 1.0), stored as f32
    let idx = (fistp(v - frac) & 0x3ff) as usize;
    let a = f32::from(t[idx]) * INV255;
    let mut b = f32::from(t[(idx + 1) & 0x3ff]) * INV255;
    if (1u32 << (u32::from(sub) & 31)) & 0xc0 != 0 {
        // slide waves wrap: lerp across the seam, then fold back below 1.
        if a > 0.75 && 0.25 > b {
            b += 1.0;
        }
        let r = (1.0 - frac) * a + b * frac;
        if r > 1.0 { r - 1.0 } else { r }
    } else {
        (1.0 - frac) * a + b * frac
    }
}

/// `0x0017ca10`: shape curve value in `[0, 1]` (sub-type 0 is linear).
fn shape(tables: &Tables, sub: u8, x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    if sub == 0 {
        return x;
    }
    let Some(Some(t)) = tables.shape.get(usize::from(sub)) else {
        return 0.0;
    };
    let v = x * 1023.0;
    let frac = (f64::from(v) % 1.0) as f32;
    // The engine rounds (v - 0.1) to nearest rather than flooring v.
    let idx = fistp(v - 0.1) as i16;
    let r = if idx == 0x3ff {
        f32::from(t[0x3ff]) * INV255
    } else {
        let i = (idx as usize) & 0x3ff;
        let a = f32::from(t[i]) * INV255 * (1.0 - frac);
        let b = f32::from(t[(i + 1) & 0x3ff]) * INV255 * frac;
        a + b
    };
    r.clamp(0.0, 1.0)
}

impl Property {
    /// Parse this property's function bytes.
    pub fn function(&self) -> Result<Function> {
        Function::parse(&self.data)
    }

    /// Port of the property evaluator `0x00246cd0` using the procedural
    /// tables. `inputs` is the evaluation input table indexed by
    /// [`Self::input`], [`Self::range_input`] and [`Self::modifier_input`];
    /// an index that is negative or past the table reads as 0 (the engine
    /// reads whatever follows its table). Call with an already parsed
    /// [`Function`] (see [`Self::function`]) to avoid re-parsing per sample.
    pub fn evaluate(&self, function: &Function, inputs: &[f32]) -> f32 {
        self.evaluate_with(function, inputs, Tables::procedural())
    }

    pub fn evaluate_with(&self, function: &Function, inputs: &[f32], tables: &Tables) -> f32 {
        let get = |i: i16| {
            usize::try_from(i)
                .ok()
                .and_then(|i| inputs.get(i))
                .copied()
                .unwrap_or(0.0)
        };
        let mut out = function.evaluate_with(get(self.input), get(self.range_input), tables);
        if !function.is_raw_output() {
            let out_c = out.clamp(0.0, 1.0);
            let [lo, hi] = function.range;
            out = (hi - lo) * out_c + lo;
        }
        if self.modifier_input >= 0 {
            let m = get(self.modifier_input);
            match self.output_modifier {
                1 => out += m,
                2 => out *= m,
                _ => {}
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a blob: header + payload words.
    fn blob(kind: u8, flags: u8, subs: [u8; 2], range: [f32; 2], words: &[f32]) -> Vec<u8> {
        let mut v = vec![kind, flags, subs[0], subs[1]];
        v.extend(range[0].to_le_bytes());
        v.extend(range[1].to_le_bytes());
        v.extend([0u8; 8]);
        for w in words {
            v.extend(w.to_le_bytes());
        }
        v
    }
    #[test]
    fn shader_colour_interpolates_the_header_stops_selected_by_the_high_nibble() {
        let mut bytes = blob(0, 0x40, [0; 2], [0.; 2], &[]);
        for (i, c) in [0xff00_0000u32, 0xff40_8020, 0xff10_2030, 0xff80_8040].into_iter().enumerate() {
            bytes[4 + 4*i..8 + 4*i].copy_from_slice(&c.to_le_bytes());
        }
        let mut f = Function::parse(&bytes).unwrap();
        assert_eq!(f.output_colour(0.5), 0xff28_5028);
        f.flags = 0x30;
        assert_eq!(f.output_colour(0.5), 0xff40_8020);
        f.flags = 0x20;
        assert_eq!(f.output_colour(1.), 0xff80_8040);
        f.flags = 0x10;
        assert_eq!(f.output_colour(1.), 0xff00_0000);
        f.flags = 0;
        assert_eq!(f.output_colour(0.5), 0xff80_8080);
    }

    fn pad(words: &mut Vec<f32>, n: usize) {
        words.resize(n, 0.0);
    }
    fn ev(bytes: &[u8], x: f32, r: f32) -> f32 {
        Function::parse(bytes).unwrap().evaluate(x, r)
    }

    /// A type change rebuilds the payload at the new length, keeps the
    /// header's flags, range and unread bytes, round-trips, and its
    /// defaults are visible curves; types without an evaluator are refused.
    #[test]
    fn with_kind_rebuilds_at_the_new_length() {
        let mut from = blob(2, 1, [3, 1], [0.25, 0.75], &[0.0; 4]);
        from[0xc..0x14].copy_from_slice(&[9; 8]);
        let from = Function::parse(&from).unwrap();
        for byte in [0u8, 1, 2, 3, 4, 5, 7, 9, 10] {
            let kind = FunctionType::from_byte(byte);
            let f = from.with_kind(kind).unwrap();
            let bytes = f.to_bytes();
            assert_eq!(bytes.len(), kind.blob_len().unwrap(), "type {byte}");
            assert_eq!(Function::parse(&bytes).unwrap(), f, "type {byte}");
            assert_eq!((f.flags, f.range, f.unknown_0c), (1, [0.25, 0.75], [9; 8]));
        }
        // The defaults draw: a ramp from 0 to 1 for the types that can,
        // a cosine for the periodic.
        for byte in [2u8, 4, 5, 7, 9, 10] {
            let f = from.with_kind(FunctionType::from_byte(byte)).unwrap();
            for r in [0.0, 1.0] {
                for x in [0.0f32, 0.5, 1.0] {
                    assert!((f.evaluate(x, r) - x).abs() < 3e-3, "type {byte} at {x}");
                }
            }
        }
        let p = from.with_kind(FunctionType::Periodic).unwrap();
        assert_eq!(p.sub_types, [2, 2]);
        assert!(p.set_values(1).is_some());
        for kind in [
            FunctionType::MultiLinearKey,
            FunctionType::MultiSpline,
            FunctionType::Unknown(0x42),
        ] {
            assert!(from.with_kind(kind).is_err());
        }
    }

    /// Ranged functions blend in the second set, not set 0 again. Each case
    /// leaves set 0 at 0.25 and set 1 at 0.75, so the 0.5 blend only appears
    /// if set 1 is read from its own offset.
    #[test]
    fn ranged_types_read_their_second_set() {
        let ranged =
            |kind: u8, words: &[f32]| ev(&blob(kind, 1, [0, 0], [0.0, 1.0], words), 0.5, 0.5);
        let mut w = vec![0.0; 8];
        w[3] = 0.25;
        w[7] = 0.75;
        assert_eq!(ranged(3, &w), 0.5, "periodic");
        let mut w = vec![0.0; 40];
        w[16] = 0.25;
        w[36] = 0.75;
        assert_eq!(ranged(5, &w), 0.5, "linear key");
        assert_eq!(ranged(9, &[0.0, 0.5, 1.0, 0.0, 1.5, 1.0]), 0.5, "exponent");
        for kind in [7u8, 10] {
            let mut w = vec![0.0; 24];
            w[11] = 0.25;
            w[23] = 0.75;
            assert_eq!(ranged(kind, &w), 0.5, "spline type {kind}");
        }
    }

    /// Spline2 feeds set 0's remapped parameter into set 1's remap.
    #[test]
    fn spline2_carries_parameter_into_second_set() {
        let mut w = vec![0.0; 24];
        w[4] = 1.0;
        w[10] = 1.0;
        w[22] = 1.0;
        assert_eq!(ev(&blob(10, 1, [0, 0], [0.0, 1.0], &w), 0.4, 1.0), 1.0);
    }

    #[test]
    fn sizes_per_type() {
        for (k, len) in [
            (0u8, 20),
            (1, 28),
            (2, 36),
            (3, 52),
            (4, 68),
            (5, 180),
            (7, 116),
            (10, 116),
            (9, 44),
        ] {
            assert_eq!(FunctionType::from_byte(k).blob_len(), Some(len));
            let ok = vec![k; 0].into_iter().chain(blob(
                k,
                0,
                [0; 2],
                [0.0; 2],
                &vec![0.0; (len - 20) / 4],
            ));
            assert!(Function::parse(&ok.collect::<Vec<_>>()).is_ok());
            let short = blob(k, 0, [0; 2], [0.0; 2], &vec![0.0; (len - 20) / 4 + 1]);
            assert!(Function::parse(&short).is_err(), "oversized type {k}");
            if len > 20 {
                let short = blob(k, 0, [0; 2], [0.0; 2], &vec![0.0; (len - 20) / 4 - 1]);
                assert!(Function::parse(&short).is_err(), "short type {k}");
            }
        }
        assert!(Function::parse(&[0u8; 19]).is_err());
        assert!(Function::parse(&[]).is_err());
    }

    #[test]
    fn unknown_type_preserved() {
        let b = blob(0x42, 3, [1, 2], [0.5, 2.0], &[1.0, 2.0, 3.0]);
        let f = Function::parse(&b).unwrap();
        assert_eq!(f.kind, FunctionType::Unknown(0x42));
        assert_eq!(f.to_bytes(), b);
        assert_eq!(f.evaluate(0.3, 0.3), 0.0);
        let huge = blob(0x42, 0, [0; 2], [0.0; 2], &vec![0.0; MAX_UNKNOWN_LEN / 4]);
        assert!(Function::parse(&huge).is_err());
    }

    #[test]
    fn unevaluated_types_return_zero() {
        for (k, words) in [(6u8, 0x40usize), (8, 8)] {
            let f = Function::parse(&blob(k, 0, [0; 2], [0.0; 2], &vec![1.0; words])).unwrap();
            assert!(!f.kind.is_evaluated());
            assert_eq!(f.evaluate(0.5, 0.5), 0.0);
        }
    }

    #[test]
    fn roundtrip_and_header() {
        let mut w = vec![1.5, 2.5];
        pad(&mut w, 4);
        let b = blob(2, 1, [3, 4], [5.0, 8.0], &w);
        let mut b2 = b.clone();
        b2[0xc] = 0xaa; // unknown header bytes survive
        let f = Function::parse(&b2).unwrap();
        assert_eq!((f.flags, f.sub_types, f.range), (1, [3, 4], [5.0, 8.0]));
        assert!(f.is_ranged() && !f.is_raw_output());
        assert_eq!(f.to_bytes(), b2);
    }

    #[test]
    fn identity_and_clamp() {
        let b = blob(0, 0, [0; 2], [0.0; 2], &[]);
        assert_eq!(ev(&b, 0.25, 0.0), 0.25);
        assert_eq!(ev(&b, -3.0, 0.0), 0.0);
        assert_eq!(ev(&b, 7.0, 0.0), 1.0);
        assert!(ev(&b, f32::NAN, 0.0).is_nan());
    }

    #[test]
    fn constant_uses_range_input_only_when_ranged() {
        let w = [0.0, 0.0];
        assert_eq!(ev(&blob(1, 0, [0; 2], [0.0; 2], &w), 0.5, 0.75), 0.0);
        assert_eq!(ev(&blob(1, 1, [0; 2], [0.0; 2], &w), 0.5, 0.75), 0.75);
    }

    #[test]
    fn transition_linear_and_ranged() {
        // shape 0 = linear: lerp(2, 4, x) = 0.2 -> 2x+... values stay in [0,1] after clamp
        let b = blob(2, 0, [0, 0], [0.0; 2], &[0.1, 0.5, 0.0, 1.0]);
        assert!((ev(&b, 0.5, 0.0) - 0.3).abs() < 1e-6);
        // ranged: set1 lerp(0,1,x) = x; r = 0.5 blends (0.3 + 0.5)/2
        let b = blob(2, 1, [0, 0], [0.0; 2], &[0.1, 0.5, 0.0, 1.0]);
        assert!((ev(&b, 0.5, 0.5) - 0.4).abs() < 1e-6);
        assert!((ev(&b, 0.5, 0.0) - 0.3).abs() < 1e-6);
        assert!((ev(&b, 0.5, 1.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn transition_shapes() {
        // shape 3 is x^2, shape 1 sqrt (to table resolution)
        let b = blob(2, 0, [3, 0], [0.0; 2], &[0.0, 1.0, 0.0, 0.0]);
        assert!((ev(&b, 0.5, 0.0) - 0.25).abs() < 0.01);
        let b = blob(2, 0, [1, 0], [0.0; 2], &[0.0, 1.0, 0.0, 0.0]);
        assert!((ev(&b, 0.25, 0.0) - 0.5).abs() < 0.01);
        let b = blob(2, 0, [3, 0], [0.0; 2], &[0.0, 1.0, 0.0, 0.0]);
        assert_eq!(ev(&b, 1.0, 0.0), 1.0);
        assert_eq!(ev(&b, 0.0, 0.0), 0.0);
        assert_eq!(ev(&b, 9.0, 0.0), 1.0); // input clamped before the table
    }

    #[test]
    fn periodic_cosine() {
        // wave 2 (cosine), freq 1/36.57143 so argument*36.57143 = table index.
        let words = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0];
        let b = blob(3, 0, [2, 0], [0.0; 2], &words);
        // x = 0: table[0] = 255 -> 1.0
        assert!((ev(&b, 0.0, 0.0) - 1.0).abs() < 1e-6);
        // half a cycle of the table (18.28571 entries): near 0
        assert!(ev(&b, 18.285715 / 36.57143, 0.0) < 0.01);
        // sub-type 0 is constant one; sub-type 1 (zero table) is zero
        assert_eq!(ev(&blob(3, 0, [0, 0], [0.0; 2], &words), 0.3, 0.0), 1.0);
        assert_eq!(ev(&blob(3, 0, [1, 0], [0.0; 2], &words), 0.3, 0.0), 0.0);
    }

    #[test]
    fn periodic_missing_engine_wave_is_zero_until_supplied() {
        let words = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0];
        let f = Function::parse(&blob(3, 0, [8, 0], [0.0; 2], &words)).unwrap();
        assert!(!Tables::procedural().has_wave(8));
        assert_eq!(f.evaluate(0.2, 0.0), 0.0);
        let mut slab = vec![0u8; 18 * TABLE_LEN];
        slab[8 * TABLE_LEN..9 * TABLE_LEN].fill(255);
        let t = Tables::procedural_owned().with_engine_slab(&slab).unwrap();
        assert!(t.has_wave(8));
        assert_eq!(f.evaluate_with(0.2, 0.0, &t), 1.0);
        assert!(
            Tables::procedural_owned()
                .with_engine_slab(&slab[1..])
                .is_err()
        );
    }

    /// A running game installs its own tables once
    /// ([`Tables::install_engine_slab`]); [`Tables::engine`] then gives the
    /// waves the procedural tables lack, and a second slab changes nothing.
    #[test]
    fn the_installed_engine_slab_gives_its_waves_for_the_process() {
        let words = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0];
        let f = Function::parse(&blob(3, 0, [9, 0], [0.0; 2], &words)).unwrap();
        assert!(!Tables::engine_installed());
        assert!(!Tables::engine().has_wave(9));
        assert_eq!(Tables::ENGINE_SLAB, 0x0046_8d48..0x0046_d548);
        let mut slab = vec![0u8; Tables::ENGINE_SLAB.len()];
        slab[9 * TABLE_LEN..10 * TABLE_LEN].fill(255);
        assert!(Tables::install_engine_slab(&slab[1..]).is_err());
        assert!(!Tables::engine_installed());
        Tables::install_engine_slab(&slab).unwrap();
        assert!(Tables::engine().has_wave(9));
        assert_eq!(f.evaluate_with(0.2, 0.0, Tables::engine()), 1.0);
        // The procedural tables are untouched, and the first slab stays.
        assert!(!Tables::procedural().has_wave(9));
        Tables::install_engine_slab(&vec![0u8; Tables::ENGINE_SLAB.len()]).unwrap();
        assert_eq!(f.evaluate_with(0.2, 0.0, Tables::engine()), 1.0);
    }

    #[test]
    fn slide_wave_folds_below_one() {
        let words = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0];
        let b = blob(3, 0, [6, 0], [0.0; 2], &words);
        for i in 0..200 {
            let v = ev(&b, i as f32 * 0.137, 0.0);
            assert!((0.0..=1.0).contains(&v), "{v}");
        }
        // linear ramp part: index 9 of the sawtooth is floor(255 * 9 / 36.57143) = 62
        let v = ev(&b, 9.0 / 36.57143, 0.0);
        assert!((v - 62.0 / 255.0).abs() < 0.01, "{v}");
    }

    #[test]
    fn linear() {
        let mut w = vec![0.0; 12];
        w[4] = 0.5;
        w[5] = 0.25;
        w[10] = -1.0;
        w[11] = 1.0;
        let b = blob(4, 0, [0; 2], [0.0; 2], &w);
        assert!((ev(&b, 0.5, 0.0) - 0.5).abs() < 1e-6);
        let b = blob(4, 1, [0; 2], [0.0; 2], &w);
        assert!((ev(&b, 0.5, 1.0) - 0.5).abs() < 1e-6); // set 1: -0.5 + 1
        assert!((ev(&b, 1.0, 1.0) - 0.0).abs() < 1e-6);
        assert!((ev(&b, 1.0, 0.0) - 0.75).abs() < 1e-6);
    }

    #[test]
    fn linear_key() {
        // keys at 0.25, 0.5, 0.75 with inverse span 4: three ramps; base 0.
        let mut w = vec![0.0; 40];
        for (k, p) in [0.25f32, 0.5, 0.75].iter().enumerate() {
            w[9 + k] = *p;
            w[13 + k] = 4.0;
        }
        w[16] = 0.0;
        w[17] = 0.5;
        w[18] = -0.25;
        w[19] = 0.75;
        let b = blob(5, 0, [0; 2], [0.0; 2], &w);
        assert_eq!(ev(&b, 0.0, 0.0), 0.0);
        assert!((ev(&b, 0.25 + 0.125, 0.0) - 0.25).abs() < 1e-6);
        assert!((ev(&b, 0.5, 0.0) - 0.5).abs() < 1e-6);
        assert!((ev(&b, 0.75, 0.0) - 0.25).abs() < 1e-6);
        assert!((ev(&b, 1.0, 0.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn spline_cubic() {
        // 0.5*t^3 + 0.25*t^2 + 0.125*t + 0.0625 at t = 0.5 -> 0.0625+0.0625+0.0625+0.0625
        let mut w = vec![0.0; 24];
        w[8] = 0.5;
        w[9] = 0.25;
        w[10] = 0.125;
        w[11] = 0.0625;
        let b = blob(7, 0, [0; 2], [0.0; 2], &w);
        assert!((ev(&b, 0.5, 0.0) - 0.25).abs() < 1e-6);
    }

    #[test]
    fn spline2_degenerate_points_use_plain_input() {
        let mut w = vec![0.0; 24];
        w[10] = 1.0; // c * t
        let b = blob(10, 0, [0; 2], [0.0; 2], &w);
        // all points equal -> |p2-p3|^2 <= 1e-4: parameter stays x
        assert!((ev(&b, 0.4, 0.0) - 0.4).abs() < 1e-6);
        // d1 > 0 but d0 ~ 0: parameter forced to 1
        let mut w2 = w.clone();
        w2[4] = 1.0; // p2.x = 1, p3 = 0
        let b = blob(10, 0, [0; 2], [0.0; 2], &w2);
        assert!((ev(&b, 0.4, 0.0) - 1.0).abs() < 1e-6);
        // remap exponent: |p0p1| = 2|p2p3| -> t = x^~2
        let mut w3 = w.clone();
        w3[4] = 1.0; // d1 = 1
        w3[0] = 2.0; // d0 = 4
        let b = blob(10, 0, [0; 2], [0.0; 2], &w3);
        let v = ev(&b, 0.5, 0.0);
        assert!((v - 0.25).abs() < 0.03, "{v}");
    }

    #[test]
    fn exponent() {
        let mut w = vec![0.0; 6];
        w[0] = 0.0;
        w[1] = 1.0;
        w[2] = 2.0;
        let b = blob(9, 0, [0; 2], [0.0; 2], &w);
        assert!((ev(&b, 0.5, 0.0) - 0.25).abs() < 1e-6);
        // exponent ~ 0 is forced to 1
        w[2] = 0.0;
        assert_eq!(ev(&blob(9, 0, [0; 2], [0.0; 2], &w), 0.5, 0.0), 1.0);
    }

    fn prop(input: i16, range: i16, mode: u16, minput: i16, f: &[u8]) -> Property {
        Property {
            input,
            range_input: range,
            output_modifier: mode,
            modifier_input: minput,
            data: f.to_vec(),
        }
    }

    #[test]
    fn property_range_lerp_and_modifier() {
        let inputs = [0.5, 0.25, 2.0];
        // identity, header range 10..20
        let b = blob(0, 0, [0; 2], [10.0, 20.0], &[]);
        let f = Function::parse(&b).unwrap();
        assert_eq!(prop(0, 0, 0, -1, &b).evaluate(&f, &inputs), 15.0);
        // add / multiply modifiers with input 2
        assert_eq!(prop(0, 0, 1, 2, &b).evaluate(&f, &inputs), 17.0);
        assert_eq!(prop(0, 0, 2, 2, &b).evaluate(&f, &inputs), 30.0);
        // unknown modifier mode and negative index leave it alone
        assert_eq!(prop(0, 0, 3, 2, &b).evaluate(&f, &inputs), 15.0);
        assert_eq!(prop(0, 0, 1, -1, &b).evaluate(&f, &inputs), 15.0);
        // a raw-output flag skips the range mapping
        let b = blob(0, 0x10, [0; 2], [10.0, 20.0], &[]);
        let f = Function::parse(&b).unwrap();
        assert_eq!(prop(1, 0, 0, -1, &b).evaluate(&f, &inputs), 0.25);
        // out-of-table inputs read as zero
        assert_eq!(prop(9, -1, 0, -1, &b).evaluate(&f, &inputs), 0.0);
    }

    #[test]
    fn property_function_parse_error() {
        assert!(prop(0, 0, 0, -1, &[1, 2, 3]).function().is_err());
        assert!(
            prop(0, 0, 0, -1, &blob(0, 0, [0; 2], [0.0; 2], &[]))
                .function()
                .is_ok()
        );
    }

    #[test]
    fn procedural_tables_endpoints() {
        let t = Tables::procedural();
        for s in 0..6 {
            let v = t.shape[s].as_ref().unwrap();
            assert_eq!(v[0], 0);
            assert_eq!(v[1023], 255);
        }
        let cos = t.wave[2].as_ref().unwrap();
        assert_eq!(cos[0], 255);
        let saw = t.wave[6].as_ref().unwrap();
        assert_eq!(saw[0], 0);
        assert!(t.has_wave(0) && t.has_wave(2) && !t.has_wave(3) && !t.has_wave(12));
    }

    /// Every payload word distinct, so a typed read picks its own words
    /// out by value.
    fn typed_blob(kind: u8) -> Vec<u8> {
        let n = FunctionType::from_byte(kind).blob_len().unwrap();
        let words: Vec<f32> = (0..(n - HEADER_LEN) / 4).map(|i| i as f32 * 0.25).collect();
        blob(kind, 0, [0; 2], [0.0, 1.0], &words)
    }

    #[test]
    fn the_type_words_round_trip() {
        for b in 0u8..=10 {
            let t = FunctionType::from_byte(b);
            assert_eq!(FunctionType::from_word(&t.word()), Some(t), "{b}");
        }
        let t = FunctionType::Unknown(0x42);
        assert_eq!(FunctionType::from_word(&t.word()), Some(t));
        assert_eq!(FunctionType::from_word("nonsense"), None);
        assert_eq!(FunctionType::from_word("unknown_x"), None);
    }

    /// The typed set reads the evaluator's own words, per set: word `k`
    /// of the blob is `k * 0.25`.
    #[test]
    fn typed_sets_read_the_evaluator_words() {
        let f = |kind: u8| Function::parse(&typed_blob(kind)).unwrap();
        let w = |k: usize| k as f32 * 0.25;
        assert_eq!(f(0).set_values(0), Some(Set::Identity));
        assert_eq!(f(1).set_values(1), Some(Set::Words(vec![w(1)])));
        assert_eq!(
            f(2).set_values(0),
            Some(Set::Transition {
                from: w(0),
                to: w(1)
            })
        );
        assert_eq!(
            f(2).set_values(1),
            Some(Set::Transition {
                from: w(2),
                to: w(3)
            })
        );
        assert_eq!(
            f(3).set_values(1),
            Some(Set::Periodic {
                frequency: w(4),
                phase: w(5),
                minimum: w(6),
                maximum: w(7)
            })
        );
        // A linear's slope and intercept are the set's words 4 and 5, not
        // its first.
        assert_eq!(
            f(4).set_values(0),
            Some(Set::Linear {
                slope: w(4),
                intercept: w(5)
            })
        );
        assert_eq!(
            f(4).set_values(1),
            Some(Set::Linear {
                slope: w(10),
                intercept: w(11)
            })
        );
        assert_eq!(
            f(5).set_values(0),
            Some(Set::LinearKey {
                base: w(16),
                keys: [
                    Key {
                        value: w(17),
                        position: w(9),
                        inverse_span: w(13)
                    },
                    Key {
                        value: w(18),
                        position: w(10),
                        inverse_span: w(14)
                    },
                    Key {
                        value: w(19),
                        position: w(11),
                        inverse_span: w(15)
                    }
                ]
            })
        );
        for kind in [7u8, 10] {
            assert_eq!(
                f(kind).set_values(1),
                Some(Set::Spline {
                    points: [12, 13, 14, 15, 16, 17, 18, 19]
                        .map(&w)
                        .chunks(2)
                        .map(|c| [c[0], c[1]])
                        .collect::<Vec<_>>()
                        .try_into()
                        .unwrap(),
                    coefficients: [w(20), w(21), w(22), w(23)],
                }),
                "spline type {kind}"
            );
        }
        assert_eq!(
            f(9).set_values(1),
            Some(Set::Exponent {
                minimum: w(3),
                maximum: w(4),
                power: w(5)
            })
        );
        assert_eq!(
            f(6).set_values(0),
            Some(Set::Words((0..32).map(w).collect()))
        );
        assert_eq!(
            f(8).set_values(1),
            Some(Set::Words((4..8).map(w).collect()))
        );
        // A type with no layout has no sets, and no set index past 1.
        let unknown = Function::parse(&blob(0x42, 0, [0; 2], [0.0, 1.0], &[1.0])).unwrap();
        assert_eq!(unknown.set_values(0), None);
        assert_eq!(f(3).set_values(2), None);
    }

    /// A typed edit round-trips: writing the values back leaves the bytes
    /// as they were; writing them into a zeroed copy sets only the words
    /// the type names (a linear's first four stay zero).
    #[test]
    fn typed_edits_round_trip_and_keep_unread_words() {
        fn w(k: usize) -> f32 {
            k as f32 * 0.25
        }
        let bytes = typed_blob(4);
        let mut f = Function::parse(&bytes).unwrap();
        let values = f.set_values(1).unwrap();
        f.write_set(1, &values).unwrap();
        assert_eq!(f.to_bytes(), bytes);
        let mut zeroed = Function::parse(&blob(4, 0, [0; 2], [0.0, 1.0], &[0.0; 12])).unwrap();
        zeroed.write_set(1, &values).unwrap();
        let out = zeroed.to_bytes();
        let word = |b: &[u8], k: usize| {
            f32::from_le_bytes(
                b[HEADER_LEN + 4 * k..HEADER_LEN + 4 * k + 4]
                    .try_into()
                    .unwrap(),
            )
        };
        assert_eq!((word(&out, 10), word(&out, 11)), (10.0 * 0.25, 11.0 * 0.25));
        for k in [0, 1, 2, 3, 4, 5, 6, 7, 8, 9] {
            assert_eq!(word(&out, k), 0.0, "word {k} is not the type's own");
        }
        // A shape of another type's is refused, and so is a set past 1.
        assert!(
            f.write_set(0, &Set::Transition { from: 0.0, to: 1.0 })
                .is_err()
        );
        assert!(f.write_set(2, &values).is_err());
        // Types 6 and 8 take their words as they are; an unknown type
        // refuses (no layout).
        let mut six = Function::parse(&typed_blob(6)).unwrap();
        six.write_set(1, &Set::Words((32..64).map(w).collect()))
            .unwrap();
        assert_eq!(
            six.set_values(1),
            Some(Set::Words((32..64).map(w).collect()))
        );
        let mut unknown = Function::parse(&blob(0x42, 0, [0; 2], [0.0, 1.0], &[1.0])).unwrap();
        assert!(unknown.write_set(0, &Set::Words(vec![0.0])).is_err());
    }
}
