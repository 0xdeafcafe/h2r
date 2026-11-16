//! NV2A vertex program microcode ("XVU" in the Xbox SDK's terms).
//!
//! An Xbox vertex shader, as `XGAssembleShader` emits it and as Halo 2 stores
//! it in `vrtx` tags, is a 4-byte header followed by 16-byte instructions:
//!
//! ```text
//! u8  'x' (0x78)
//! u8  kind: 0x20 normal, 0x77 ('w') read/write, 0x73 ('s') state
//! u16 instruction count (little endian)
//! [u32; 4] per instruction; word 0 is unused and always zero
//! ```
//!
//! Each instruction drives the two NV2A vertex units at once: the MAC
//! (multiply/accumulate, vector) and the ILU (inverse/log unit, mostly
//! scalar). Both read the same three operands A, B, C; the ILU only ever reads
//! C. Bit positions are in [`decode`] and in `docs/shaders.md`.

mod disasm;
pub mod interp;
pub mod wgsl;

pub use disasm::disassemble;
pub use wgsl::{InputFetch, TranslateError, VertexOptions, VertexShader, VertexUniforms, translate};

use std::fmt;

/// Most instructions an NV2A program slot table holds.
pub const MAX_INSTRUCTIONS: usize = 136;
/// Constant registers `c[0..192]` (hardware numbering).
pub const CONSTANT_COUNT: usize = 192;
/// D3D numbers constants from -96; hardware index = D3D index + 96.
pub const D3D_CONSTANT_BIAS: i32 = 96;
/// Temporaries `r0..r11`; `r12` reads back `oPos`.
pub const TEMP_COUNT: usize = 12;
/// Vertex attribute registers `v0..v15`.
pub const INPUT_COUNT: usize = 16;

/// Malformed microcode.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    /// Shorter than the header or the declared instruction count.
    #[error("program truncated: need {need} bytes, have {have}")]
    Truncated { need: usize, have: usize },
    /// Header does not start with `'x'` or has an unknown kind byte.
    #[error("not an Xbox vertex program header: {0:02x?}")]
    BadHeader([u8; 2]),
    /// Instruction count of zero or above [`MAX_INSTRUCTIONS`].
    #[error("bad instruction count {0}")]
    BadCount(usize),
    /// An operand an opcode reads has mux value 0.
    #[error("instruction {index}: operand {operand} has no register file")]
    BadOperand { index: usize, operand: char },
    /// A temporary register outside `r0..r12`.
    #[error("instruction {index}: bad temporary r{reg}")]
    BadTemp { index: usize, reg: u8 },
    /// An output register address that names nothing.
    #[error("instruction {index}: bad output register {addr}")]
    BadOutput { index: usize, addr: u8 },
    /// A constant read without `a0.x` outside `c[0..192]` (hardware numbering).
    #[error("instruction {index}: constant c[{reg}] out of range")]
    BadConst { index: usize, reg: u8 },
    /// MAC opcode 14 or 15.
    #[error("instruction {index}: bad MAC opcode {op}")]
    BadOpcode { index: usize, op: u8 },
    /// No instruction has the final bit set.
    #[error("no final instruction")]
    NoFinal,
}

/// The header's kind byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgramKind {
    /// `0x2078`: an ordinary vertex shader. Constant writes are not allowed.
    Normal,
    /// `0x7778`: read/write; may write constant registers.
    ReadWrite,
    /// `0x7378`: a state shader, run once without vertex data.
    State,
}

/// A decoded program.
#[derive(Clone, Debug, PartialEq)]
pub struct Program {
    /// Header kind.
    pub kind: ProgramKind,
    /// Instructions up to and including the first one marked final.
    pub instructions: Vec<Instruction>,
}

/// MAC opcodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum MacOp {
    Nop,
    Mov,
    Mul,
    /// `A + C` (not `A + B`).
    Add,
    /// `A * B + C`.
    Mad,
    Dp3,
    Dph,
    Dp4,
    Dst,
    Min,
    Max,
    Slt,
    Sge,
    /// `a0.x = floor(A.x)`.
    Arl,
}

/// ILU opcodes. All read operand C only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum IluOp {
    Nop,
    Mov,
    Rcp,
    /// Reciprocal clamped away from zero and infinity.
    Rcc,
    Rsq,
    Exp,
    Log,
    Lit,
}

impl IluOp {
    /// Scalar ops read only the first swizzle component of C.
    pub fn is_scalar(self) -> bool {
        matches!(
            self,
            Self::Rcp | Self::Rcc | Self::Rsq | Self::Exp | Self::Log
        )
    }
}

/// Where an operand comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SrcReg {
    /// `r0..r11`, or `r12` which reads `oPos`.
    Temp(u8),
    /// `v0..v15`.
    Input(u8),
    /// `c[index]` (hardware numbering) or `c[a0.x + index]`.
    Const {
        /// Hardware index, 0..=191 (raw field is 8 bits).
        index: u8,
        /// Add `a0.x`.
        relative: bool,
    },
    /// Mux value 0. Never valid for an operand the opcode reads.
    Unused,
}

/// One MAC/ILU operand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Source {
    /// Register.
    pub reg: SrcReg,
    /// Component selected for x, y, z, w (0..=3).
    pub swizzle: [u8; 4],
    /// Negate after swizzling.
    pub neg: bool,
}

/// Four-bit write mask, x in bit 3 down to w in bit 0 (the hardware order).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Mask(pub u8);

impl Mask {
    /// All components.
    pub const XYZW: Mask = Mask(0xF);
    /// Whether component `i` (0 = x) is written.
    pub fn has(self, i: usize) -> bool {
        self.0 & (8 >> i) != 0
    }
    /// No components.
    pub fn is_empty(self) -> bool {
        self.0 & 0xF == 0
    }
    /// Per-component flags in x, y, z, w order.
    pub fn components(self) -> [bool; 4] {
        [self.has(0), self.has(1), self.has(2), self.has(3)]
    }
}

impl fmt::Display for Mask {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, c) in "xyzw".chars().enumerate() {
            if self.has(i) {
                write!(f, "{c}")?;
            }
        }
        Ok(())
    }
}

/// Output registers (`oPos`, colours, texture coordinates...).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum OutReg {
    Pos,
    D0,
    D1,
    Fog,
    Pts,
    B0,
    B1,
    T(u8),
}

impl OutReg {
    /// From the low four bits of the output address field.
    pub fn from_addr(addr: u8) -> Option<Self> {
        Some(match addr {
            0 => Self::Pos,
            3 => Self::D0,
            4 => Self::D1,
            5 => Self::Fog,
            6 => Self::Pts,
            7 => Self::B0,
            8 => Self::B1,
            9..=12 => Self::T(addr - 9),
            _ => return None,
        })
    }
    /// D3D assembler name.
    pub fn name(self) -> String {
        match self {
            Self::Pos => "oPos".into(),
            Self::D0 => "oD0".into(),
            Self::D1 => "oD1".into(),
            Self::Fog => "oFog".into(),
            Self::Pts => "oPts".into(),
            Self::B0 => "oB0".into(),
            Self::B1 => "oB1".into(),
            Self::T(n) => format!("oT{n}"),
        }
    }
    /// Index into [`interp::Outputs::regs`] and the generated register struct.
    pub fn slot(self) -> usize {
        match self {
            Self::Pos => 0,
            Self::D0 => 1,
            Self::D1 => 2,
            Self::Fog => 3,
            Self::Pts => 4,
            Self::B0 => 5,
            Self::B1 => 6,
            Self::T(n) => 7 + n as usize,
        }
    }
}

/// Which unit feeds the output (`o`/`c`) write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    /// The MAC result.
    Mac,
    /// The ILU result.
    Ilu,
}

/// Destination of the third (non-temporary) write port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutDest {
    /// An output register.
    Output(OutReg),
    /// A constant register (read/write and state programs only), hardware index.
    Const(u8),
}

/// The output-port write of an instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputWrite {
    /// Unit whose result is written.
    pub unit: Unit,
    /// Register written.
    pub dest: OutDest,
    /// Components written.
    pub mask: Mask,
}

/// One decoded instruction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Instruction {
    /// The raw words, kept for diagnostics.
    pub words: [u32; 4],
    /// MAC opcode.
    pub mac: MacOp,
    /// ILU opcode.
    pub ilu: IluOp,
    /// Operand A (MAC only).
    pub a: Source,
    /// Operand B (MAC only).
    pub b: Source,
    /// Operand C (MAC ADD/MAD and every ILU op).
    pub c: Source,
    /// Temporary written by the MAC.
    pub mac_temp: u8,
    /// MAC temporary write mask.
    pub mac_mask: Mask,
    /// Temporary written by the ILU: `r1` when paired with a MAC op.
    pub ilu_temp: u8,
    /// ILU temporary write mask.
    pub ilu_mask: Mask,
    /// Output-port write, if any component is enabled.
    pub out: Option<OutputWrite>,
    /// Last instruction of the program.
    pub is_final: bool,
}

impl Instruction {
    /// Whether the MAC temporary write takes effect. A MAC op paired with an
    /// ILU op cannot write `r1`: that port belongs to the ILU.
    pub fn mac_writes_temp(&self) -> bool {
        self.mac != MacOp::Nop
            && self.mac != MacOp::Arl
            && !self.mac_mask.is_empty()
            && !(self.ilu != IluOp::Nop && self.mac_temp == 1)
    }
    /// Whether the ILU temporary write takes effect.
    pub fn ilu_writes_temp(&self) -> bool {
        self.ilu != IluOp::Nop && !self.ilu_mask.is_empty()
    }
    /// Operands the MAC reads: (A, B, C).
    pub fn mac_reads(&self) -> (bool, bool, bool) {
        use MacOp::*;
        match self.mac {
            Nop => (false, false, false),
            Mov | Arl => (true, false, false),
            Add => (true, false, true),
            Mad => (true, true, true),
            _ => (true, true, false),
        }
    }
    /// Every operand this instruction actually reads.
    pub fn sources(&self) -> impl Iterator<Item = &Source> {
        let (a, b, c) = self.mac_reads();
        let c = c || self.ilu != IluOp::Nop;
        [(a, &self.a), (b, &self.b), (c, &self.c)]
            .into_iter()
            .filter(|(u, _)| *u)
            .map(|(_, s)| s)
    }
}

fn field(word: u32, shift: u32, bits: u32) -> u32 {
    (word >> shift) & ((1 << bits) - 1)
}

fn swizzle(word: u32, x_shift: u32) -> [u8; 4] {
    // x is the most significant pair: x at shift, y at shift-2, ...
    [0, 1, 2, 3].map(|i| field(word, x_shift - 2 * i, 2) as u8)
}

fn source(mux: u32, temp: u32, neg: u32, swz: [u8; 4], v: u32, constant: u32, rel: bool) -> Source {
    let reg = match mux {
        1 => SrcReg::Temp(temp as u8),
        2 => SrcReg::Input(v as u8),
        3 => SrcReg::Const {
            index: constant as u8,
            relative: rel,
        },
        _ => SrcReg::Unused,
    };
    Source {
        reg,
        swizzle: swz,
        neg: neg != 0,
    }
}

/// Decodes one instruction. `index` is only used in errors.
///
/// Field layout (word, low bit, width), taken from the field table in xemu's
/// `vsh-prog.c` and matching Cxbx-Reloaded's `XbVertexShader.cpp`:
///
/// | field | word | bit | width |
/// |---|---|---|---|
/// | ILU op | 1 | 25 | 3 |
/// | MAC op | 1 | 21 | 4 |
/// | const index | 1 | 13 | 8 |
/// | input index | 1 | 9 | 4 |
/// | A neg, swz xyzw | 1 | 8, 6/4/2/0 | 1, 2 each |
/// | A temp, mux | 2 | 28, 26 | 4, 2 |
/// | B neg, swz | 2 | 25, 23/21/19/17 | |
/// | B temp, mux | 2 | 13, 11 | 4, 2 |
/// | C neg, swz | 2 | 10, 8/6/4/2 | |
/// | C temp | 2 bits 0-1 (high), 3 bits 30-31 (low) | | 4 |
/// | C mux | 3 | 28 | 2 |
/// | MAC mask, temp, ILU mask, out mask | 3 | 24, 20, 16, 12 | 4 each |
/// | out is output (1) / constant (0) | 3 | 11 | 1 |
/// | out address | 3 | 3 | 8 |
/// | out from ILU | 3 | 2 | 1 |
/// | relative constant (a0.x) | 3 | 1 | 1 |
/// | final | 3 | 0 | 1 |
pub fn decode(words: [u32; 4], index: usize) -> Result<Instruction, Error> {
    let [_, w1, w2, w3] = words;
    let ilu = match field(w1, 25, 3) {
        0 => IluOp::Nop,
        1 => IluOp::Mov,
        2 => IluOp::Rcp,
        3 => IluOp::Rcc,
        4 => IluOp::Rsq,
        5 => IluOp::Exp,
        6 => IluOp::Log,
        _ => IluOp::Lit,
    };
    let mac = match field(w1, 21, 4) {
        0 => MacOp::Nop,
        1 => MacOp::Mov,
        2 => MacOp::Mul,
        3 => MacOp::Add,
        4 => MacOp::Mad,
        5 => MacOp::Dp3,
        6 => MacOp::Dph,
        7 => MacOp::Dp4,
        8 => MacOp::Dst,
        9 => MacOp::Min,
        10 => MacOp::Max,
        11 => MacOp::Slt,
        12 => MacOp::Sge,
        13 => MacOp::Arl,
        op => {
            return Err(Error::BadOpcode {
                index,
                op: op as u8,
            });
        }
    };
    let constant = field(w1, 13, 8);
    let v = field(w1, 9, 4);
    let rel = field(w3, 1, 1) != 0;
    let a = source(
        field(w2, 26, 2),
        field(w2, 28, 4),
        field(w1, 8, 1),
        swizzle(w1, 6),
        v,
        constant,
        rel,
    );
    let b = source(
        field(w2, 11, 2),
        field(w2, 13, 4),
        field(w2, 25, 1),
        swizzle(w2, 23),
        v,
        constant,
        rel,
    );
    let c_temp = (field(w2, 0, 2) << 2) | field(w3, 30, 2);
    let c = source(
        field(w3, 28, 2),
        c_temp,
        field(w2, 10, 1),
        swizzle(w2, 8),
        v,
        constant,
        rel,
    );

    let mac_mask = Mask(field(w3, 24, 4) as u8);
    let mac_temp = field(w3, 20, 4) as u8;
    let ilu_mask = Mask(field(w3, 16, 4) as u8);
    let out_mask = Mask(field(w3, 12, 4) as u8);
    let is_output = field(w3, 11, 1) != 0;
    let addr = field(w3, 3, 8) as u8;
    let unit = if field(w3, 2, 1) != 0 {
        Unit::Ilu
    } else {
        Unit::Mac
    };
    let is_final = field(w3, 0, 1) != 0;
    let ilu_temp = if mac != MacOp::Nop { 1 } else { mac_temp };

    let unit_active = match unit {
        Unit::Mac => mac != MacOp::Nop && mac != MacOp::Arl,
        Unit::Ilu => ilu != IluOp::Nop,
    };
    let out = if out_mask.is_empty() || !unit_active {
        None
    } else if is_output {
        let reg = OutReg::from_addr(addr & 0xF).ok_or(Error::BadOutput { index, addr })?;
        Some(OutputWrite {
            unit,
            dest: OutDest::Output(reg),
            mask: out_mask,
        })
    } else {
        Some(OutputWrite {
            unit,
            dest: OutDest::Const(addr),
            mask: out_mask,
        })
    };

    let inst = Instruction {
        words,
        mac,
        ilu,
        a,
        b,
        c,
        mac_temp,
        mac_mask,
        ilu_temp,
        ilu_mask,
        out,
        is_final,
    };
    for (name, src) in [('A', &inst.a), ('B', &inst.b), ('C', &inst.c)] {
        let (ra, rb, rc) = inst.mac_reads();
        let used = match name {
            'A' => ra,
            'B' => rb,
            _ => rc || ilu != IluOp::Nop,
        };
        if !used {
            continue;
        }
        match src.reg {
            SrcReg::Unused => {
                return Err(Error::BadOperand {
                    index,
                    operand: name,
                });
            }
            SrcReg::Temp(r) if r > 12 => return Err(Error::BadTemp { index, reg: r }),
            // Relative reads are clamped at run time; absolute ones have no
            // register above c[191] to name.
            SrcReg::Const {
                index: c,
                relative: false,
            } if c as usize >= CONSTANT_COUNT => {
                return Err(Error::BadConst { index, reg: c });
            }
            _ => {}
        }
    }
    if inst.mac_writes_temp() && mac_temp > 12 {
        return Err(Error::BadTemp {
            index,
            reg: mac_temp,
        });
    }
    if inst.ilu_writes_temp() && ilu_temp > 12 {
        return Err(Error::BadTemp {
            index,
            reg: ilu_temp,
        });
    }
    Ok(inst)
}

impl Program {
    /// Parses a header-prefixed program (`'x'`, kind, count, instructions).
    /// Trailing bytes are ignored.
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < 4 {
            return Err(Error::Truncated {
                need: 4,
                have: bytes.len(),
            });
        }
        let kind = match [bytes[0], bytes[1]] {
            [0x78, 0x20] => ProgramKind::Normal,
            [0x78, 0x77] => ProgramKind::ReadWrite,
            [0x78, 0x73] => ProgramKind::State,
            other => return Err(Error::BadHeader(other)),
        };
        let count = u16::from_le_bytes([bytes[2], bytes[3]]) as usize;
        if count == 0 || count > MAX_INSTRUCTIONS {
            return Err(Error::BadCount(count));
        }
        let need = 4 + count * 16;
        if bytes.len() < need {
            return Err(Error::Truncated {
                need,
                have: bytes.len(),
            });
        }
        let words: Vec<u32> = bytes[4..need]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| u32::from_le_bytes(*c))
            .collect();
        Self::from_words(kind, &words)
    }

    /// Decodes bare microcode (four words per instruction), stopping after
    /// the first instruction with the final bit.
    pub fn from_words(kind: ProgramKind, words: &[u32]) -> Result<Self, Error> {
        let mut instructions = Vec::new();
        for (index, chunk) in words.as_chunks::<4>().0.iter().enumerate() {
            let inst = decode(*chunk, index)?;
            let done = inst.is_final;
            instructions.push(inst);
            if done {
                return Ok(Self { kind, instructions });
            }
        }
        Err(Error::NoFinal)
    }

    /// Bit `n` set when `v<n>` is read.
    pub fn inputs_read(&self) -> u16 {
        let mut mask = 0u16;
        for i in &self.instructions {
            for s in i.sources() {
                if let SrcReg::Input(n) = s.reg {
                    mask |= 1 << n;
                }
            }
        }
        mask
    }

    /// Whether any instruction writes a constant register.
    pub fn writes_constants(&self) -> bool {
        self.instructions.iter().any(|i| {
            matches!(
                i.out,
                Some(OutputWrite {
                    dest: OutDest::Const(_),
                    ..
                })
            )
        })
    }

    /// Output registers written at least once.
    pub fn outputs_written(&self) -> Vec<OutReg> {
        let mut out: Vec<OutReg> = Vec::new();
        for i in &self.instructions {
            if let Some(OutputWrite {
                dest: OutDest::Output(r),
                ..
            }) = i.out
                && !out.contains(&r)
            {
                out.push(r);
            }
            for t in [
                i.mac_writes_temp().then_some(i.mac_temp),
                i.ilu_writes_temp().then_some(i.ilu_temp),
            ] {
                if t == Some(12) && !out.contains(&OutReg::Pos) {
                    out.push(OutReg::Pos);
                }
            }
        }
        out
    }
}

/// Builders for hand-written microcode, used by tests and tools. They produce
/// exactly the bit patterns [`decode`] reads.
pub mod encode {
    use super::*;

    /// An operand for [`Inst`].
    #[derive(Clone, Copy, Debug)]
    pub struct Operand {
        /// Register.
        pub reg: SrcReg,
        /// Swizzle, x first.
        pub swizzle: [u8; 4],
        /// Negate.
        pub neg: bool,
    }

    impl Operand {
        /// `r<n>`.
        pub fn r(n: u8) -> Self {
            Self {
                reg: SrcReg::Temp(n),
                swizzle: [0, 1, 2, 3],
                neg: false,
            }
        }
        /// `v<n>`.
        pub fn v(n: u8) -> Self {
            Self {
                reg: SrcReg::Input(n),
                swizzle: [0, 1, 2, 3],
                neg: false,
            }
        }
        /// `c[n]`, hardware numbering.
        pub fn c(n: u8) -> Self {
            Self {
                reg: SrcReg::Const {
                    index: n,
                    relative: false,
                },
                swizzle: [0, 1, 2, 3],
                neg: false,
            }
        }
        /// `c[a0.x + n]`.
        pub fn c_rel(n: u8) -> Self {
            Self {
                reg: SrcReg::Const {
                    index: n,
                    relative: true,
                },
                swizzle: [0, 1, 2, 3],
                neg: false,
            }
        }
        /// Swizzle from a string like `"xxyw"` or `"w"` (replicated).
        pub fn swz(mut self, s: &str) -> Self {
            let c: Vec<u8> = s
                .bytes()
                .map(|b| b"xyzw".iter().position(|&x| x == b).unwrap() as u8)
                .collect();
            for i in 0..4 {
                self.swizzle[i] = c[i.min(c.len() - 1)];
            }
            self
        }
        /// Negated.
        pub fn negated(mut self) -> Self {
            self.neg = !self.neg;
            self
        }
    }

    /// An instruction under construction.
    #[derive(Clone, Debug)]
    pub struct Inst {
        /// MAC opcode number.
        pub mac: u32,
        /// ILU opcode number.
        pub ilu: u32,
        /// Operands.
        pub a: Option<Operand>,
        /// Operand B.
        pub b: Option<Operand>,
        /// Operand C.
        pub c: Option<Operand>,
        /// MAC temp `(reg, mask)`.
        pub mac_temp: Option<(u8, u8)>,
        /// ILU temp `(reg, mask)`; the reg field is shared with the MAC.
        pub ilu_temp: Option<(u8, u8)>,
        /// Output `(unit is ILU, is output reg, address, mask)`.
        pub out: Option<(bool, bool, u8, u8)>,
        /// Final flag.
        pub last: bool,
    }

    impl Inst {
        /// A MAC op (`MacOp as u32` numbering) with operands.
        pub fn mac(op: MacOp, a: Option<Operand>, b: Option<Operand>, c: Option<Operand>) -> Self {
            Self {
                mac: op as u32,
                ilu: 0,
                a,
                b,
                c,
                mac_temp: None,
                ilu_temp: None,
                out: None,
                last: false,
            }
        }
        /// An ILU op reading C.
        pub fn ilu(op: IluOp, c: Operand) -> Self {
            Self {
                mac: 0,
                ilu: op as u32,
                a: None,
                b: None,
                c: Some(c),
                mac_temp: None,
                ilu_temp: None,
                out: None,
                last: false,
            }
        }
        /// Pair an ILU op with this MAC op (C must be shared).
        pub fn with_ilu(mut self, op: IluOp, c: Operand) -> Self {
            self.ilu = op as u32;
            self.c = Some(c);
            self
        }
        /// Write the MAC result to `r<reg>` with `mask` (hardware bit order, x = 8).
        pub fn to_temp(mut self, reg: u8, mask: u8) -> Self {
            self.mac_temp = Some((reg, mask));
            self
        }
        /// Write the ILU result to a temp (`r1` when paired).
        pub fn ilu_to_temp(mut self, reg: u8, mask: u8) -> Self {
            self.ilu_temp = Some((reg, mask));
            self
        }
        /// Write a unit's result to an output register.
        pub fn to_out(mut self, from_ilu: bool, reg: OutReg, mask: u8) -> Self {
            let addr = match reg {
                OutReg::Pos => 0,
                OutReg::D0 => 3,
                OutReg::D1 => 4,
                OutReg::Fog => 5,
                OutReg::Pts => 6,
                OutReg::B0 => 7,
                OutReg::B1 => 8,
                OutReg::T(n) => 9 + n,
            };
            self.out = Some((from_ilu, true, addr, mask));
            self
        }
        /// Write a unit's result to constant `c[index]`.
        pub fn to_const(mut self, from_ilu: bool, index: u8, mask: u8) -> Self {
            self.out = Some((from_ilu, false, index, mask));
            self
        }
        /// Mark final.
        pub fn last(mut self) -> Self {
            self.last = true;
            self
        }
        /// The four instruction words.
        pub fn words(&self) -> [u32; 4] {
            let mut w1 = (self.ilu << 25) | (self.mac << 21);
            let mut w2 = 0u32;
            let mut w3 = 0u32;
            let mut constant = None;
            let mut input = None;
            let mut rel = false;
            let mut put = |op: &Operand| -> (u32, u32) {
                match op.reg {
                    SrcReg::Temp(r) => (1, r as u32),
                    SrcReg::Input(v) => {
                        assert!(
                            input.is_none() || input == Some(v),
                            "one input register per instruction"
                        );
                        input = Some(v);
                        (2, 0)
                    }
                    SrcReg::Const { index, relative } => {
                        assert!(
                            constant.is_none() || constant == Some(index),
                            "one constant per instruction"
                        );
                        constant = Some(index);
                        rel |= relative;
                        (3, 0)
                    }
                    SrcReg::Unused => (0, 0),
                }
            };
            let swz = |op: &Operand| -> u32 {
                op.swizzle
                    .iter()
                    .fold(0u32, |acc, &s| (acc << 2) | s as u32)
            };
            if let Some(a) = &self.a {
                let (mux, r) = put(a);
                w1 |= ((a.neg as u32) << 8) | swz(a);
                w2 |= (r << 28) | (mux << 26);
            }
            if let Some(b) = &self.b {
                let (mux, r) = put(b);
                w2 |= ((b.neg as u32) << 25) | (swz(b) << 17) | (r << 13) | (mux << 11);
            }
            if let Some(c) = &self.c {
                let (mux, r) = put(c);
                w2 |= ((c.neg as u32) << 10) | (swz(c) << 2) | (r >> 2);
                w3 |= ((r & 3) << 30) | (mux << 28);
            }
            w1 |= (constant.unwrap_or(0) as u32) << 13 | (input.unwrap_or(0) as u32) << 9;
            let mut temp = 0u32;
            if let Some((r, m)) = self.mac_temp {
                temp = r as u32;
                w3 |= (m as u32) << 24;
            }
            if let Some((r, m)) = self.ilu_temp {
                if self.mac == 0 {
                    temp = r as u32;
                }
                w3 |= (m as u32) << 16;
            }
            w3 |= temp << 20;
            if let Some((ilu, is_out, addr, m)) = self.out {
                w3 |= ((m as u32) << 12)
                    | ((is_out as u32) << 11)
                    | ((addr as u32) << 3)
                    | ((ilu as u32) << 2);
            }
            w3 |= (rel as u32) << 1 | self.last as u32;
            [0, w1, w2, w3]
        }
    }

    /// A header-prefixed normal program from instructions; the last one is
    /// marked final.
    pub fn program(insts: &[Inst]) -> Vec<u8> {
        let mut out = vec![0x78, 0x20];
        out.extend_from_slice(&(insts.len() as u16).to_le_bytes());
        for (i, inst) in insts.iter().enumerate() {
            let mut inst = inst.clone();
            inst.last = i + 1 == insts.len();
            for w in inst.words() {
                out.extend_from_slice(&w.to_le_bytes());
            }
        }
        out
    }
}
