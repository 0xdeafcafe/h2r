//! Reproducible synthetic inputs. Expected machine results come only from run().
use iced_x86::{Decoder, DecoderOptions, Mnemonic, OpKind};
use serde::{Deserialize, Serialize};

use crate::{CpuState, Outcome, flags};

pub const SCHEMA_VERSION: u32 = 1;
pub const DATA: u32 = 0x20_0000;
pub const DEFAULT_SEED: u64 = 0x4841_4c4f_325f_3836;
pub const EDGES: &[u32] = &[
    0,
    1,
    2,
    7,
    8,
    9,
    15,
    16,
    17,
    31,
    32,
    33,
    63,
    64,
    65,
    127,
    128,
    255,
    256,
    0x7fff,
    0x8000,
    0xffff,
    0x7fff_ffff,
    0x8000_0000,
    0xffff_ffff,
];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Encoding {
    pub name: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Fixture {
    pub schema_version: u32,
    pub name: String,
    pub case: String,
    pub seed: u64,
    pub bytes: Vec<u8>,
    pub initial: CpuState,
    pub memory: Vec<(u32, Vec<u8>)>,
    pub outcome: Outcome,
}

pub fn encodings() -> Vec<Encoding> {
    let mut list = Vec::new();
    let mut add = |name: String, bytes: Vec<u8>| list.push(Encoding { name, bytes });
    for (name, op) in [
        ("add", 0x00),
        ("or", 0x08),
        ("adc", 0x10),
        ("sbb", 0x18),
        ("and", 0x20),
        ("sub", 0x28),
        ("xor", 0x30),
        ("cmp", 0x38),
        ("test", 0x84),
    ] {
        for (width, prefix, delta) in [(8, vec![], 0), (16, vec![0x66], 1), (32, vec![], 1)] {
            let mut b = prefix;
            b.extend([op + delta, 0xd8]); // AL/AX/EAX, BL/BX/EBX
            add(format!("{name}-{width}"), b);
        }
    }
    for (name, extension) in [
        ("not", 2),
        ("neg", 3),
        ("mul", 4),
        ("imul", 5),
        ("div", 6),
        ("idiv", 7),
    ] {
        for (width, prefix, opcode) in [
            (8, vec![], 0xf6),
            (16, vec![0x66], 0xf7),
            (32, vec![], 0xf7),
        ] {
            let mut b = prefix;
            b.extend([opcode, 0xc3 + (extension << 3)]); // r/m = BL/BX/EBX
            add(format!("{name}-{width}"), b);
        }
    }
    for (name, extension) in [("inc", 0), ("dec", 1)] {
        for (width, prefix, opcode) in [
            (8, vec![], 0xfe),
            (16, vec![0x66], 0xff),
            (32, vec![], 0xff),
        ] {
            let mut b = prefix;
            b.extend([opcode, 0xc0 + (extension << 3)]);
            add(format!("{name}-{width}"), b);
        }
    }
    for (name, ext) in [
        ("rol", 0),
        ("ror", 1),
        ("rcl", 2),
        ("rcr", 3),
        ("shl", 4),
        ("shr", 5),
        ("sar", 7),
    ] {
        for (width, prefix) in [(8, vec![]), (16, vec![0x66]), (32, vec![])] {
            let mut cl = prefix.clone();
            cl.extend([if width == 8 { 0xd2 } else { 0xd3 }, 0xc0 + (ext << 3)]);
            add(format!("{name}-{width}-cl"), cl);
            for count in [0, 1, 2, 8, 9, 16, 17, 31, 32, 33, 63, 64, 255] {
                let mut b = prefix.clone();
                b.extend([
                    if width == 8 { 0xc0 } else { 0xc1 },
                    0xc0 + (ext << 3),
                    count,
                ]);
                add(format!("{name}-{width}-imm-{count}"), b);
            }
        }
    }
    for (name, op) in [("shld", 0xa4), ("shrd", 0xac)] {
        for (width, prefix) in [(16, vec![0x66]), (32, vec![])] {
            let mut cl = prefix.clone();
            cl.extend([0x0f, op + 1, 0xd8]);
            add(format!("{name}-{width}-cl"), cl);
            for count in [0, 1, 16, 17, 31, 32, 33, 255] {
                let mut b = prefix.clone();
                b.extend([0x0f, op, 0xd8, count]);
                add(format!("{name}-{width}-imm-{count}"), b);
            }
        }
    }
    for (name, op) in [
        ("bt", 0xa3),
        ("bts", 0xab),
        ("btr", 0xb3),
        ("btc", 0xbb),
        ("bsf", 0xbc),
        ("bsr", 0xbd),
    ] {
        add(format!("{name}-32"), vec![0x0f, op, 0xc3]);
        add(format!("{name}-16"), vec![0x66, 0x0f, op, 0xc3]);
        add(format!("{name}-memory"), vec![0x0f, op, 0x07]); // [EDI], EAX or EAX, [EDI]
    }
    for cc in 0..16 {
        add(format!("setcc-{cc}"), vec![0x0f, 0x90 + cc, 0xc4]); // AH
        add(format!("cmovcc-{cc}"), vec![0x0f, 0x40 + cc, 0xc3]);
    }
    for (name, b) in [
        ("imul-two", vec![0x0f, 0xaf, 0xc3]),
        ("imul-three", vec![0x6b, 0xc3, 0xff]),
        ("mov-al", vec![0xb0, 0x5a]),
        ("mov-ah", vec![0xb4, 0x5a]),
        ("mov-ax", vec![0x66, 0xb8, 0x34, 0x12]),
        ("xchg-ah-bl", vec![0x86, 0xdc]),
        ("movzx-al", vec![0x0f, 0xb6, 0xc0]),
        ("movsx-al", vec![0x0f, 0xbe, 0xc0]),
        ("store-eax", vec![0x89, 0x07]),
        ("load-eax", vec![0x8b, 0x07]),
        ("push-pop", vec![0x50, 0x5b]),
        ("carry-chain", vec![0x01, 0xd8, 0x11, 0xca]),
        ("ud2", vec![0x0f, 0x0b]),
        ("unmapped-read", vec![0xa1, 0, 0, 0, 0x70]),
        ("unmapped-write", vec![0xa3, 0, 0, 0, 0x70]),
        ("x87-fld1", vec![0xd9, 0xe8]),
        ("x87-add", vec![0xd9, 0xe8, 0xd9, 0xe8, 0xde, 0xc1]),
        ("sse-addss", vec![0xf3, 0x0f, 0x58, 0xc1]),
        ("sse-xorps", vec![0x0f, 0x57, 0xc1]),
    ] {
        add(name.into(), b);
    }
    for (name, op) in [
        ("movsb", 0xa4),
        ("movsd", 0xa5),
        ("stosb", 0xaa),
        ("stosd", 0xab),
        ("lodsb", 0xac),
        ("lodsd", 0xad),
        ("cmpsb", 0xa6),
        ("cmpsd", 0xa7),
        ("scasb", 0xae),
        ("scasd", 0xaf),
    ] {
        add(name.into(), vec![op]);
        add(format!("rep-{name}"), vec![0xf3, op]);
        add(format!("rep-{name}-addr16"), vec![0x67, 0xf3, op]);
        if name.starts_with("cmps") || name.starts_with("scas") {
            add(format!("repne-{name}"), vec![0xf2, op]);
        }
    }
    x87(&mut add);
    list
}

/// x87 sequences over the random data at ESI/EDI: every operand family,
/// compares into EFLAGS and through `fnstsw`, stack faults, environment
/// images, and each rounding and precision control on a few operations.
/// Appended last so the inputs of the families above stay as they were.
fn x87(add: &mut impl FnMut(String, Vec<u8>)) {
    for (name, b) in [
        ("x87-fld-m32-fstp-m32", vec![0xd9, 0x06, 0xd9, 0x5f, 0x10]),
        ("x87-fld-m64-fstp-m32", vec![0xdd, 0x07, 0xd9, 0x5f, 0x10]),
        ("x87-fld-m64-fst-m64", vec![0xdd, 0x07, 0xdd, 0x57, 0x10]),
        ("x87-fld-m80-fstp-m80", vec![0xdb, 0x2e, 0xdb, 0x7f, 0x10]),
        ("x87-fld-m80-fstp-m64", vec![0xdb, 0x2e, 0xdd, 0x5f, 0x10]),
        ("x87-fild-m16", vec![0xdf, 0x06, 0xdd, 0x5f, 0x10]),
        ("x87-fild-m32", vec![0xdb, 0x06]),
        ("x87-fild-m64", vec![0xdf, 0x2e]),
        ("x87-fbld-fbstp", vec![0xdf, 0x26, 0xdf, 0x77, 0x10]),
        ("x87-fistp-m16", vec![0xdd, 0x07, 0xdf, 0x5f, 0x10]),
        ("x87-fist-m32", vec![0xdd, 0x07, 0xdb, 0x57, 0x10]),
        ("x87-fistp-m64", vec![0xdd, 0x07, 0xdf, 0x7f, 0x10]),
        ("x87-fbstp", vec![0xdd, 0x07, 0xdf, 0x77, 0x10]),
        (
            "x87-consts",
            vec![
                0xd9, 0xe8, 0xd9, 0xe9, 0xd9, 0xea, 0xd9, 0xeb, 0xd9, 0xec, 0xd9, 0xed, 0xd9, 0xee,
            ],
        ),
        ("x87-fadd-m32", vec![0xd9, 0x06, 0xd8, 0x46, 0x04]),
        ("x87-fsub-m32", vec![0xd9, 0x06, 0xd8, 0x66, 0x04]),
        ("x87-fsubr-m32", vec![0xd9, 0x06, 0xd8, 0x6e, 0x04]),
        ("x87-fmul-m64", vec![0xdd, 0x07, 0xdc, 0x4f, 0x08]),
        ("x87-fdiv-m32", vec![0xd9, 0x06, 0xd8, 0x76, 0x04]),
        ("x87-fdivr-m64", vec![0xdd, 0x07, 0xdc, 0x7f, 0x08]),
        ("x87-fiadd-m32", vec![0xdd, 0x07, 0xda, 0x46, 0x04]),
        ("x87-fimul-m16", vec![0xdd, 0x07, 0xde, 0x4e, 0x04]),
        ("x87-fidiv-m32", vec![0xdd, 0x07, 0xda, 0x76, 0x04]),
        ("x87-faddp", vec![0xd9, 0x06, 0xdd, 0x47, 0x08, 0xde, 0xc1]),
        ("x87-fsubp", vec![0xd9, 0x06, 0xdd, 0x47, 0x08, 0xde, 0xe9]),
        ("x87-fsubrp", vec![0xd9, 0x06, 0xdd, 0x47, 0x08, 0xde, 0xe1]),
        ("x87-fmulp", vec![0xd9, 0x06, 0xdd, 0x47, 0x08, 0xde, 0xc9]),
        ("x87-fdivp", vec![0xd9, 0x06, 0xdd, 0x47, 0x08, 0xde, 0xf9]),
        ("x87-fdivrp", vec![0xd9, 0x06, 0xdd, 0x47, 0x08, 0xde, 0xf1]),
        (
            "x87-fadd-st0-st1",
            vec![0xd9, 0x06, 0xdd, 0x47, 0x08, 0xd8, 0xc1],
        ),
        (
            "x87-fsub-st1-st0",
            vec![0xd9, 0x06, 0xdd, 0x47, 0x08, 0xdc, 0xe9],
        ),
        ("x87-fsqrt", vec![0xdd, 0x07, 0xd9, 0xfa]),
        ("x87-fabs-fchs", vec![0xdd, 0x07, 0xd9, 0xe1, 0xd9, 0xe0]),
        ("x87-frndint", vec![0xdd, 0x07, 0xd9, 0xfc]),
        ("x87-fscale", vec![0xd9, 0x46, 0x04, 0xdd, 0x07, 0xd9, 0xfd]),
        ("x87-fxtract", vec![0xdd, 0x07, 0xd9, 0xf4]),
        ("x87-ftst", vec![0xdd, 0x07, 0xd9, 0xe4]),
        ("x87-fxam", vec![0xdd, 0x07, 0xd9, 0xe5]),
        ("x87-fxam-empty", vec![0xd9, 0xe5]),
        ("x87-fcom-m32", vec![0xd9, 0x06, 0xd8, 0x56, 0x04]),
        ("x87-fcomp-m32", vec![0xd9, 0x06, 0xd8, 0x5e, 0x04]),
        (
            "x87-fcom-st1",
            vec![0xd9, 0x06, 0xdd, 0x47, 0x08, 0xd8, 0xd1],
        ),
        ("x87-fcompp", vec![0xd9, 0x06, 0xdd, 0x47, 0x08, 0xde, 0xd9]),
        (
            "x87-fucom-st1",
            vec![0xd9, 0x06, 0xdd, 0x47, 0x08, 0xdd, 0xe1],
        ),
        (
            "x87-fucompp",
            vec![0xd9, 0x06, 0xdd, 0x47, 0x08, 0xda, 0xe9],
        ),
        ("x87-ficom-m32", vec![0xdd, 0x07, 0xda, 0x56, 0x04]),
        ("x87-fcomi", vec![0xd9, 0x06, 0xdd, 0x47, 0x08, 0xdb, 0xf1]),
        ("x87-fcomip", vec![0xd9, 0x06, 0xdd, 0x47, 0x08, 0xdf, 0xf1]),
        (
            "x87-fucomip",
            vec![0xd9, 0x06, 0xdd, 0x47, 0x08, 0xdf, 0xe9],
        ),
        (
            "x87-fcom-fnstsw-sahf",
            vec![0xd9, 0x06, 0xd8, 0x56, 0x04, 0xdf, 0xe0, 0x9e],
        ),
        (
            "x87-fucompp-fnstsw-test",
            vec![
                0xd9, 0x06, 0xdd, 0x47, 0x08, 0xda, 0xe9, 0xdf, 0xe0, 0xf6, 0xc4, 0x45,
            ],
        ),
        (
            "x87-ftst-fnstsw-m16",
            vec![0xdd, 0x07, 0xd9, 0xe4, 0xdd, 0x7f, 0x10],
        ),
        (
            "x87-fxch",
            vec![0xd9, 0x06, 0xdd, 0x47, 0x08, 0xd9, 0xc9, 0xd9, 0x5f, 0x10],
        ),
        (
            "x87-ffree-fincstp",
            vec![0xd9, 0x06, 0xdd, 0x47, 0x08, 0xdd, 0xc1, 0xd9, 0xf7],
        ),
        ("x87-fdecstp", vec![0xd9, 0x06, 0xd9, 0xf6]),
        (
            "x87-fst-st2",
            vec![0xd9, 0x06, 0xdd, 0x47, 0x08, 0xdd, 0xd2],
        ),
        (
            "x87-fstp-st1",
            vec![0xd9, 0x06, 0xdd, 0x47, 0x08, 0xdd, 0xd9],
        ),
        ("x87-fcmovb", vec![0xd9, 0x06, 0xdd, 0x47, 0x08, 0xda, 0xc1]),
        ("x87-fcmove", vec![0xd9, 0x06, 0xdd, 0x47, 0x08, 0xda, 0xc9]),
        (
            "x87-fcmovnbe",
            vec![0xd9, 0x06, 0xdd, 0x47, 0x08, 0xdb, 0xd1],
        ),
        ("x87-fcmovu", vec![0xd9, 0x06, 0xdd, 0x47, 0x08, 0xda, 0xd9]),
        ("x87-underflow-fadd", vec![0xde, 0xc1]),
        (
            "x87-overflow-push",
            vec![
                0xd9, 0xe8, 0xd9, 0xe8, 0xd9, 0xe8, 0xd9, 0xe8, 0xd9, 0xe8, 0xd9, 0xe8, 0xd9, 0xe8,
                0xd9, 0xe8, 0xd9, 0xe8,
            ],
        ),
        ("x87-fnstcw", vec![0xd9, 0x7f, 0x10]),
        (
            "x87-fninit-fnclex",
            vec![0xd9, 0x06, 0xdb, 0xe3, 0xdb, 0xe2],
        ),
        ("x87-fnstenv", vec![0xd9, 0x06, 0xd9, 0x77, 0x20]),
        (
            "x87-fnstenv-fldenv",
            vec![0xd9, 0x06, 0xd9, 0x77, 0x20, 0xd9, 0x67, 0x20],
        ),
        (
            "x87-fnsave",
            vec![0xd9, 0x06, 0xdd, 0x47, 0x08, 0xdd, 0x77, 0x20],
        ),
        (
            "x87-fnsave-frstor",
            vec![
                0xd9, 0x06, 0xdd, 0x47, 0x08, 0xdd, 0x77, 0x20, 0xdd, 0x67, 0x20,
            ],
        ),
        ("x87-fwait-fnop", vec![0x9b, 0xd9, 0xd0]),
    ] {
        add(name.into(), b);
    }
    // mov word [esi], cw; fldcw [esi]; then the operation.
    let cw = |v: u16| vec![0x66, 0xc7, 0x06, v as u8, (v >> 8) as u8, 0xd9, 0x2e];
    for (name, v) in [
        ("rc-down", 0x077f),
        ("rc-up", 0x0b7f),
        ("rc-chop", 0x0f7f),
        ("pc-24", 0x007f),
        ("pc-53", 0x027f),
    ] {
        for (op, tail) in [
            (
                "fadd",
                &[0xd9, 0x07, 0xd8, 0x47, 0x04, 0xd9, 0x5f, 0x10][..],
            ),
            (
                "fdiv-m64",
                &[0xdd, 0x07, 0xdc, 0x77, 0x08, 0xdd, 0x5f, 0x10],
            ),
            ("fistp", &[0xdd, 0x07, 0xdb, 0x5f, 0x10]),
            ("frndint", &[0xdd, 0x07, 0xd9, 0xfc]),
        ] {
            add(format!("x87-{name}-{op}"), [cw(v), tail.to_vec()].concat());
        }
    }
    add(
        "x87-fldcw-fnstcw".into(),
        [cw(0x0c3f), vec![0xd9, 0x7f, 0x10]].concat(),
    );
}

/// Stable xorshift64*; using an explicit PRNG avoids rand-version drift.
pub struct Inputs {
    rng: u64,
}

impl Inputs {
    pub fn new(seed: u64) -> Self {
        Self { rng: seed.max(1) }
    }
    fn next(&mut self) -> u32 {
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        (self.rng.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 32) as u32
    }

    pub fn state(
        &mut self,
        edge: Option<usize>,
        encoding: &Encoding,
    ) -> (CpuState, Vec<(u32, Vec<u8>)>) {
        let instruction = Decoder::new(32, &encoding.bytes, DecoderOptions::NONE).decode();
        let address16 = (0..instruction.op_count()).any(|n| {
            matches!(
                instruction.op_kind(n),
                OpKind::MemorySegSI | OpKind::MemoryESDI
            )
        });
        let mut s = CpuState {
            eax: self.next(),
            ebx: self.next(),
            ecx: self.next(),
            edx: self.next(),
            ebp: self.next(),
            esi: DATA + 64,
            edi: DATA + 256,
            esp: DATA + 3072,
            ..CpuState::default()
        };
        s.eflags |= self.next() & (flags::ARITHMETIC | flags::DF | (1 << 9));
        for reg in &mut s.xmm {
            for byte in reg {
                *byte = self.next() as u8;
            }
        }
        if let Some(n) = edge {
            let value = EDGES[n % EDGES.len()];
            s.eax = value;
            s.ebx = value;
            s.ecx = value;
            s.edx = if n % 2 == 0 { 0 } else { u32::MAX };
            s.eflags = 2 | if n % 2 == 0 {
                0
            } else {
                flags::ARITHMETIC | flags::DF
            };
        }
        if matches!(instruction.mnemonic(), Mnemonic::Div | Mnemonic::Idiv) {
            match edge {
                Some(0) => {
                    s.eax = 1;
                    s.edx = 0;
                    s.ebx = 0;
                } // zero divisor
                Some(1) => {
                    s.eax = 0;
                    s.edx = 1;
                    s.ebx = 1;
                } // unsigned quotient overflow
                Some(2) => {
                    s.eax = 0x8000_0000;
                    s.edx = u32::MAX;
                    s.ebx = u32::MAX;
                } // signed overflow
                Some(3) => {
                    s.eax = 0xffff_fff9;
                    s.edx = u32::MAX;
                    s.ebx = 3;
                } // signed -7 / 3
                Some(4) => {
                    s.eax = 148;
                    s.edx = 0;
                    s.ebx = 0xffff_fffc;
                }
                _ => {}
            }
        }
        let mut memory = (0..512).map(|_| self.next() as u8).collect::<Vec<_>>();
        // Short, bounded REP including zero; addr16 cases seed low pages as well.
        if instruction.is_string_instruction() {
            s.ecx = edge.map_or_else(|| self.next() % 17, |n| [0, 1, 2, 8, 16][n % 5]);
            if s.eflags & flags::DF != 0 {
                s.esi += 64;
                s.edi += 64;
            }
            if address16 {
                s.esi = 0x1040;
                s.edi = 0x1100;
            }
            if edge == Some(5) {
                memory.fill(s.eax as u8);
            }
        }
        let address = if address16 { 0x1000 } else { DATA };
        (s, vec![(address, memory)])
    }
}

pub fn evaluate(
    encoding: &Encoding,
    case: String,
    seed: u64,
    initial: CpuState,
    memory: Vec<(u32, Vec<u8>)>,
) -> Fixture {
    let outcome = crate::run(&encoding.bytes, initial.clone(), &memory);
    Fixture {
        schema_version: SCHEMA_VERSION,
        name: encoding.name.clone(),
        case,
        seed,
        bytes: encoding.bytes.clone(),
        initial,
        memory,
        outcome,
    }
}
