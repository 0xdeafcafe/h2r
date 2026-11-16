//! Intel SDM Vol. 2, instruction Operation / Flags Affected sections.
//! These are EFLAGS positions, unlike iced's internal RflagsBits numbering.
//! See docs/lifter/oracle.md for the source edition and the complete rules table.
use iced_x86::{Instruction, Mnemonic, OpKind, RflagsBits};

pub const CF: u32 = 1;
pub const PF: u32 = 1 << 2;
pub const AF: u32 = 1 << 4;
pub const ZF: u32 = 1 << 6;
pub const SF: u32 = 1 << 7;
pub const OF: u32 = 1 << 11;
pub const DF: u32 = 1 << 10;
pub const ARITHMETIC: u32 = CF | PF | AF | ZF | SF | OF;
pub const EFLAGS_MASK: u32 = 0x003f_7fd7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlagEffect {
    pub modified: u32,
    pub undefined: u32,
}

pub(crate) fn from_iced(bits: u32) -> u32 {
    [
        (RflagsBits::CF, CF),
        (RflagsBits::PF, PF),
        (RflagsBits::AF, AF),
        (RflagsBits::ZF, ZF),
        (RflagsBits::SF, SF),
        (RflagsBits::OF, OF),
        (RflagsBits::DF, DF),
        (RflagsBits::IF, 1 << 9),
        (RflagsBits::AC, 1 << 18),
    ]
    .into_iter()
    .fold(0, |m, (a, b)| m | if bits & a != 0 { b } else { 0 })
}

/// Count-sensitive SDM masks for common integer instructions. `cl` is from
/// pre-state; width comes from the decoded destination. Other instructions use
/// iced's independent metadata; iced never executes a result in this crate.
pub fn effect(i: &Instruction, cl: u8) -> FlagEffect {
    use Mnemonic::*;
    let mut e = FlagEffect {
        modified: from_iced(i.rflags_modified()),
        undefined: from_iced(i.rflags_undefined()),
    };
    match i.mnemonic() {
        Add | Adc | Sub | Sbb | Cmp | Neg | Cmpsb | Cmpsw | Cmpsd | Scasb | Scasw | Scasd => {
            e = FlagEffect {
                modified: ARITHMETIC,
                undefined: 0,
            }
        }
        Inc | Dec => {
            e = FlagEffect {
                modified: ARITHMETIC & !CF,
                undefined: 0,
            }
        }
        And | Or | Xor | Test => {
            e = FlagEffect {
                modified: ARITHMETIC,
                undefined: AF,
            }
        }
        Mul | Imul => {
            e = FlagEffect {
                modified: ARITHMETIC,
                undefined: PF | AF | ZF | SF,
            }
        }
        Div | Idiv => {
            e = FlagEffect {
                modified: ARITHMETIC,
                undefined: ARITHMETIC,
            }
        }
        Bt | Btc | Btr | Bts => {
            e = FlagEffect {
                modified: CF | OF | SF | AF | PF,
                undefined: OF | SF | AF | PF,
            }
        }
        Bsf | Bsr => {
            e = FlagEffect {
                modified: ARITHMETIC,
                undefined: ARITHMETIC & !ZF,
            }
        }
        Shl | Shr | Sar | Shld | Shrd | Rol | Ror | Rcl | Rcr => {
            let count_op = i.op_count() - 1;
            let raw = match i.op_kind(count_op) {
                OpKind::Immediate8 => i.immediate8(),
                _ => cl,
            };
            let masked = u32::from(raw & 31);
            let width = if i.op0_kind() == OpKind::Register {
                i.op0_register().size() as u32 * 8
            } else {
                i.memory_size().size() as u32 * 8
            };
            let rotate = matches!(i.mnemonic(), Rol | Ror | Rcl | Rcr);
            let through_carry = matches!(i.mnemonic(), Rcl | Rcr);
            // ROL/ROR with a nonzero count equal to the width still update CF.
            let count = if through_carry && width < 32 {
                masked % (width + 1)
            } else {
                masked
            };
            e = FlagEffect {
                modified: 0,
                undefined: 0,
            };
            if count != 0 {
                e.modified = if rotate { CF | OF } else { ARITHMETIC };
                if masked != 1 {
                    e.undefined |= OF;
                }
                if !rotate {
                    e.undefined |= AF;
                    if matches!(i.mnemonic(), Shl | Shr) && count >= width {
                        e.undefined |= CF;
                    }
                    if matches!(i.mnemonic(), Shld | Shrd) && count > width {
                        e.undefined = ARITHMETIC;
                    }
                }
            }
        }
        _ => {}
    }
    e
}
