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
    list
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
