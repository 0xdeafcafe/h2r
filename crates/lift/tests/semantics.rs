//! Lowering checked against known x86 results.
//!
//! Each case assembles a few instructions by hand (encodings in the
//! comments), lowers them, runs the IR on the reference interpreter, and
//! compares registers, memory and flags with values computed by hand from
//! the Intel SDM. The expected values are the documentation: each case says
//! why the answer is what it is.
//!
//! Only flags the SDM defines are compared; [`check_undefined`] asserts the
//! ones it leaves undefined are marked so.

use iced_x86::{Decoder, DecoderOptions};
use ir::interp::{self, Cpu, Exit, Flow, SparseMemory};
use ir::{Fault, Flag, Gpr};
use lift::{Env, Site, lower};

const CODE: u32 = 0x1000;
const STACK: u32 = 0x8000;
const DATA: u32 = 0x4000;

use Flag::*;
use Gpr::*;

/// Lower and run `code` at [`CODE`], following jumps inside it, until
/// execution leaves it. Returns the last flow, or the exit that stopped it.
fn run_with(code: &[u8], cpu: &mut Cpu, mem: &mut SparseMemory) -> Result<Option<Flow>, Exit> {
    let insns: Vec<_> = Decoder::with_ip(32, code, CODE as u64, DecoderOptions::NONE).into_iter().collect();
    let env = Env::default();
    let mut pc = CODE;
    let mut last = None;
    let mut steps = 0;
    while let Some(insn) = insns.iter().find(|i| i.ip32() == pc) {
        let ir = lower(insn, &env, Site::Plain);
        ir.validate().unwrap_or_else(|e| panic!("{e}\n{ir}"));
        assert!(ir.is_supported(), "unsupported:\n{ir}");
        let flow = interp::step(cpu, mem, &ir)?;
        pc = flow.target();
        last = Some(flow);
        steps += 1;
        assert!(steps < 10_000, "runaway");
    }
    Ok(last)
}

fn cpu() -> Cpu {
    let mut c = Cpu::default();
    c.set_reg(Esp, STACK);
    // Where a fault leaves EIP, so before/after comparisons line up.
    c.eip = CODE;
    c
}

fn mem() -> SparseMemory {
    let mut m = SparseMemory::new();
    m.map(STACK - 0x100, 0x200);
    m.map(DATA, 0x100);
    m
}

fn run(code: &[u8], setup: impl FnOnce(&mut Cpu, &mut SparseMemory)) -> (Cpu, SparseMemory) {
    let (mut c, mut m) = (cpu(), mem());
    setup(&mut c, &mut m);
    run_with(code, &mut c, &mut m).unwrap_or_else(|e| panic!("exited: {e:?}"));
    (c, m)
}

#[track_caller]
fn flags(c: &Cpu, expect: &[(Flag, bool)]) {
    for &(f, v) in expect {
        assert!(c.flag_defined(f), "{f:?} should be defined");
        assert_eq!(c.flag(f), v, "{f:?}");
    }
}

#[track_caller]
fn check_undefined(c: &Cpu, list: &[Flag]) {
    for f in list {
        assert!(!c.flag_defined(*f), "{f:?} should be undefined");
    }
}

fn set_flags(c: &mut Cpu, list: &[(Flag, bool)]) {
    for &(f, v) in list {
        c.set_flag(f, v);
    }
}

// ---- add/adc/sub/sbb ------------------------------------------------------

#[test]
fn adc_carries_through_all_ones() {
    // adc eax, 0 (83 d0 00): 0xffffffff + 0 + CF(1) = 0x1_00000000.
    // The result wraps to 0 with a carry out; no signed overflow (-1 + 1);
    // AF because the low nibble 0xf + 1 carries.
    let (c, _) = run(&[0x83, 0xD0, 0x00], |c, _| {
        c.set_reg(Eax, 0xFFFF_FFFF);
        c.set_flag(Cf, true);
    });
    assert_eq!(c.reg(Eax), 0);
    flags(&c, &[(Cf, true), (Zf, true), (Sf, false), (Of, false), (Af, true), (Pf, true)]);
}

#[test]
fn adc_signed_overflow_from_carry_in() {
    // adc al, bl (10 d8): 0x7f + 0 + 1 = 0x80. Positive + positive gave
    // negative, so OF; no unsigned carry out of bit 7. Bits 16..31 and AH
    // stay as they were.
    let (c, _) = run(&[0x10, 0xD8], |c, _| {
        c.set_reg(Eax, 0x1234_567F);
        c.set_reg(Ebx, 0);
        c.set_flag(Cf, true);
    });
    assert_eq!(c.reg(Eax), 0x1234_5680);
    flags(&c, &[(Cf, false), (Of, true), (Sf, true), (Zf, false), (Af, true), (Pf, false)]);
}

#[test]
fn add_flags() {
    // add eax, ebx (01 d8): 0x80000000 + 0x80000000 = 0 with carry and
    // signed overflow (negative + negative = positive).
    let (c, _) = run(&[0x01, 0xD8], |c, _| {
        c.set_reg(Eax, 0x8000_0000);
        c.set_reg(Ebx, 0x8000_0000);
    });
    assert_eq!(c.reg(Eax), 0);
    flags(&c, &[(Cf, true), (Of, true), (Zf, true), (Sf, false), (Af, false), (Pf, true)]);
}

#[test]
fn sbb_borrows_through_zero() {
    // sbb eax, ebx (19 d8): 0 - 0 - CF(1) = -1: borrow out, no overflow,
    // borrow out of bit 3 as well.
    let (c, _) = run(&[0x19, 0xD8], |c, _| c.set_flag(Cf, true));
    assert_eq!(c.reg(Eax), 0xFFFF_FFFF);
    flags(&c, &[(Cf, true), (Of, false), (Sf, true), (Zf, false), (Af, true), (Pf, true)]);
}

#[test]
fn sbb_signed_overflow_from_borrow_in() {
    // sbb al, 0 (1c 00): 0x80 - 0 - 1 = 0x7f. -128 - 1 overflows; no
    // unsigned borrow (0x80 >= 1).
    let (c, _) = run(&[0x1C, 0x00], |c, _| {
        c.set_reg(Eax, 0x80);
        c.set_flag(Cf, true);
    });
    assert_eq!(c.reg(Eax), 0x7F);
    flags(&c, &[(Cf, false), (Of, true), (Sf, false), (Zf, false), (Af, true), (Pf, false)]);
}

#[test]
fn sbb_with_equal_operands_and_borrow() {
    // sbb eax, ebx (19 d8) with eax = ebx = 5, CF = 1: 5 - 5 - 1 = -1.
    // The borrow-in alone produces the borrow-out.
    let (c, _) = run(&[0x19, 0xD8], |c, _| {
        c.set_reg(Eax, 5);
        c.set_reg(Ebx, 5);
        c.set_flag(Cf, true);
    });
    assert_eq!(c.reg(Eax), 0xFFFF_FFFF);
    flags(&c, &[(Cf, true), (Zf, false)]);
}

#[test]
fn cmp_sets_flags_without_writing() {
    // cmp eax, 1 (83 f8 01) with eax = 0: unsigned below (CF), signed less
    // (SF != OF); eax untouched.
    let (c, _) = run(&[0x83, 0xF8, 0x01], |_, _| {});
    assert_eq!(c.reg(Eax), 0);
    flags(&c, &[(Cf, true), (Zf, false), (Sf, true), (Of, false)]);
}

#[test]
fn inc_preserves_carry() {
    // stc; inc eax (f9 40): 0x7fffffff + 1 overflows signed; CF stays 1.
    let (c, _) = run(&[0xF9, 0x40], |c, _| c.set_reg(Eax, 0x7FFF_FFFF));
    assert_eq!(c.reg(Eax), 0x8000_0000);
    flags(&c, &[(Cf, true), (Of, true), (Sf, true), (Zf, false), (Af, true)]);
}

#[test]
fn dec_to_zero() {
    // clc; dec ecx (f8 49): 1 - 1 = 0, CF stays 0.
    let (c, _) = run(&[0xF8, 0x49], |c, _| c.set_reg(Ecx, 1));
    assert_eq!(c.reg(Ecx), 0);
    flags(&c, &[(Cf, false), (Zf, true), (Of, false), (Af, false)]);
}

#[test]
fn neg_flags() {
    // neg eax (f7 d8): CF is "operand was nonzero"; negating INT_MIN
    // overflows and gives INT_MIN back.
    let (c, _) = run(&[0xF7, 0xD8], |_, _| {});
    flags(&c, &[(Cf, false), (Zf, true), (Of, false)]);
    let (c, _) = run(&[0xF7, 0xD8], |c, _| c.set_reg(Eax, 0x8000_0000));
    assert_eq!(c.reg(Eax), 0x8000_0000);
    flags(&c, &[(Cf, true), (Of, true), (Sf, true)]);
    let (c, _) = run(&[0xF7, 0xD8], |c, _| c.set_reg(Eax, 1));
    assert_eq!(c.reg(Eax), 0xFFFF_FFFF);
    flags(&c, &[(Cf, true), (Of, false), (Af, true)]);
}

#[test]
fn logic_clears_cf_of_and_undefines_af() {
    // xor eax, eax (31 c0): zero, PF set, CF/OF cleared.
    let (c, _) = run(&[0x31, 0xC0], |c, _| {
        c.set_reg(Eax, 0xDEAD_BEEF);
        set_flags(c, &[(Cf, true), (Of, true)]);
    });
    assert_eq!(c.reg(Eax), 0);
    flags(&c, &[(Cf, false), (Of, false), (Zf, true), (Sf, false), (Pf, true)]);
    check_undefined(&c, &[Af]);
    // test al, 0x80 (a8 80) with al = 0x81: nonzero, negative, odd parity
    // of the *result* (0x80: one bit set).
    let (c, _) = run(&[0xA8, 0x80], |c, _| c.set_reg(Eax, 0x81));
    assert_eq!(c.reg(Eax), 0x81);
    flags(&c, &[(Zf, false), (Sf, true), (Pf, false)]);
}

// ---- shifts and rotates ---------------------------------------------------

#[test]
fn shift_count_is_masked_to_five_bits() {
    // shl eax, cl (d3 e0) with cl = 33: the count is 33 & 31 = 1.
    // 0x80000001 << 1 = 2; CF = old bit 31 = 1; OF (1-bit) = MSB ^ CF = 1.
    let (c, _) = run(&[0xD3, 0xE0], |c, _| {
        c.set_reg(Eax, 0x8000_0001);
        c.set_reg(Ecx, 33);
    });
    assert_eq!(c.reg(Eax), 2);
    flags(&c, &[(Cf, true), (Of, true), (Zf, false), (Sf, false), (Pf, false)]);
}

#[test]
fn shift_by_masked_zero_changes_nothing() {
    // shl eax, cl with cl = 32: masked count 0, so no result and no flag
    // changes at all.
    let (c, _) = run(&[0xD3, 0xE0], |c, _| {
        c.set_reg(Eax, 0x8000_0001);
        c.set_reg(Ecx, 32);
        set_flags(c, &[(Cf, false), (Zf, true), (Of, true), (Sf, false), (Pf, false)]);
    });
    assert_eq!(c.reg(Eax), 0x8000_0001);
    flags(&c, &[(Cf, false), (Zf, true), (Of, true), (Sf, false), (Pf, false)]);
    // Immediate zero (c1 e0 20: shl eax, 32) likewise.
    let (c, _) = run(&[0xC1, 0xE0, 0x20], |c, _| {
        c.set_reg(Eax, 7);
        c.set_flag(Zf, true);
    });
    assert_eq!(c.reg(Eax), 7);
    flags(&c, &[(Zf, true)]);
}

#[test]
fn shr_and_sar_by_one() {
    // shr eax, 1 (d1 e8) with eax = 1: 0, CF = 1, OF = old MSB = 0.
    let (c, _) = run(&[0xD1, 0xE8], |c, _| c.set_reg(Eax, 1));
    assert_eq!(c.reg(Eax), 0);
    flags(&c, &[(Cf, true), (Zf, true), (Of, false)]);
    check_undefined(&c, &[Af]);
    // shr eax, 1 with eax = 0x80000000: OF = old MSB = 1.
    let (c, _) = run(&[0xD1, 0xE8], |c, _| c.set_reg(Eax, 0x8000_0000));
    assert_eq!(c.reg(Eax), 0x4000_0000);
    flags(&c, &[(Cf, false), (Of, true)]);
    // sar al, 1 (d0 f8) with al = 0x81: 0xc0 (sign kept), CF = 1, OF = 0.
    let (c, _) = run(&[0xD0, 0xF8], |c, _| c.set_reg(Eax, 0x81));
    assert_eq!(c.reg(Eax), 0xC0);
    flags(&c, &[(Cf, true), (Of, false), (Sf, true)]);
}

#[test]
fn sar_by_large_count_fills_with_sign() {
    // sar eax, 31 (c1 f8 1f): -8 >> 31 = -1; CF = bit 30 = 1. OF is
    // undefined for counts other than 1.
    let (c, _) = run(&[0xC1, 0xF8, 0x1F], |c, _| c.set_reg(Eax, 0xFFFF_FFF8));
    assert_eq!(c.reg(Eax), 0xFFFF_FFFF);
    flags(&c, &[(Cf, true), (Sf, true)]);
    check_undefined(&c, &[Of, Af]);
}

#[test]
fn shl_byte_by_more_than_its_width() {
    // shl al, 9 (c0 e0 09): the 5-bit count is 9, more than 8, so AL is 0.
    // CF is architecturally undefined here.
    let (c, _) = run(&[0xC0, 0xE0, 0x09], |c, _| c.set_reg(Eax, 0xFF));
    assert_eq!(c.reg(Eax), 0);
    check_undefined(&c, &[Cf, Of]);
    flags(&c, &[(Zf, true)]);
}

#[test]
fn rotates() {
    // rol al, 1 (d0 c0) with al = 0x80: 0x01, CF = new LSB = 1,
    // OF = MSB ^ CF = 1. SF/ZF untouched by rotates.
    let (c, _) = run(&[0xD0, 0xC0], |c, _| {
        c.set_reg(Eax, 0x80);
        c.set_flag(Zf, true);
    });
    assert_eq!(c.reg(Eax), 0x01);
    flags(&c, &[(Cf, true), (Of, true), (Zf, true)]);
    // ror eax, 4 (c1 c8 04): 0x12345678 -> 0x81234567, CF = new MSB = 1.
    let (c, _) = run(&[0xC1, 0xC8, 0x04], |c, _| c.set_reg(Eax, 0x1234_5678));
    assert_eq!(c.reg(Eax), 0x8123_4567);
    flags(&c, &[(Cf, true)]);
    check_undefined(&c, &[Of]);
    // rol al, 8 (c0 c0 08): rotating a byte by 8 leaves it unchanged, but
    // the count is nonzero, so CF = LSB of the result.
    let (c, _) = run(&[0xC0, 0xC0, 0x08], |c, _| c.set_reg(Eax, 0x81));
    assert_eq!(c.reg(Eax), 0x81);
    flags(&c, &[(Cf, true)]);
}

#[test]
fn rotates_through_carry() {
    // rcr eax, 1 (d1 d8) with eax = 1, CF = 1: CF enters at the top,
    // bit 0 leaves into CF. OF = old MSB ^ old CF = 1.
    let (c, _) = run(&[0xD1, 0xD8], |c, _| {
        c.set_reg(Eax, 1);
        c.set_flag(Cf, true);
    });
    assert_eq!(c.reg(Eax), 0x8000_0000);
    flags(&c, &[(Cf, true), (Of, true)]);
    // rcl al, 1 (d0 d0) with al = 0x80, CF = 0: al = 0, CF = 1,
    // OF = new MSB ^ new CF = 1.
    let (c, _) = run(&[0xD0, 0xD0], |c, _| c.set_reg(Eax, 0x80));
    assert_eq!(c.reg(Eax), 0);
    flags(&c, &[(Cf, true), (Of, true)]);
    // rcl al, 9 (c0 d0 09): a 9-bit rotate by 9 is the identity.
    let (c, _) = run(&[0xC0, 0xD0, 0x09], |c, _| {
        c.set_reg(Eax, 0x5A);
        c.set_flag(Cf, true);
    });
    assert_eq!(c.reg(Eax), 0x5A);
    flags(&c, &[(Cf, true)]);
}

#[test]
fn double_shifts() {
    // shld eax, edx, 8 (0f a4 d0 08): eax = 0x11223344, edx = 0xaabbccdd ->
    // 0x223344aa; CF = last bit out of eax = bit 24 of 0x11223344 = 1.
    let (c, _) = run(&[0x0F, 0xA4, 0xD0, 0x08], |c, _| {
        c.set_reg(Eax, 0x1122_3344);
        c.set_reg(Edx, 0xAABB_CCDD);
    });
    assert_eq!(c.reg(Eax), 0x2233_44AA);
    flags(&c, &[(Cf, true), (Sf, false), (Zf, false)]);
    // shrd eax, edx, cl (0f ad d0) with cl = 4: 0xd1122334, CF = bit 3 of
    // 0x11223344 = 0.
    let (c, _) = run(&[0x0F, 0xAD, 0xD0], |c, _| {
        c.set_reg(Eax, 0x1122_3344);
        c.set_reg(Edx, 0xAABB_CCDD);
        c.set_reg(Ecx, 4);
    });
    assert_eq!(c.reg(Eax), 0xD112_2334);
    flags(&c, &[(Cf, false), (Sf, true)]);
}

// ---- multiply and divide --------------------------------------------------

#[test]
fn imul_two_operand_overflow() {
    // imul eax, ebx (0f af c3): 0x10000 * 0x10000 = 2^32, truncated to 0;
    // doesn't fit in 32 signed bits, so CF = OF = 1.
    let (c, _) = run(&[0x0F, 0xAF, 0xC3], |c, _| {
        c.set_reg(Eax, 0x10000);
        c.set_reg(Ebx, 0x10000);
    });
    assert_eq!(c.reg(Eax), 0);
    flags(&c, &[(Cf, true), (Of, true)]);
    check_undefined(&c, &[Sf, Zf, Af, Pf]);
    // -2 * 3 = -6 fits: CF = OF = 0.
    let (c, _) = run(&[0x0F, 0xAF, 0xC3], |c, _| {
        c.set_reg(Eax, 0xFFFF_FFFE);
        c.set_reg(Ebx, 3);
    });
    assert_eq!(c.reg(Eax), 0xFFFF_FFFA);
    flags(&c, &[(Cf, false), (Of, false)]);
}

#[test]
fn imul_three_operand() {
    // imul eax, ebx, -1 (6b c3 ff): -INT_MIN does not fit.
    let (c, _) = run(&[0x6B, 0xC3, 0xFF], |c, _| c.set_reg(Ebx, 0x8000_0000));
    assert_eq!(c.reg(Eax), 0x8000_0000);
    flags(&c, &[(Cf, true), (Of, true)]);
    // imul ecx, [DATA], 10 (6b 0d <DATA> 0a), [DATA] = -7: -70.
    let mut code = vec![0x6B, 0x0D];
    code.extend(DATA.to_le_bytes());
    code.push(0x0A);
    let (c, _) = run(&code, |_, m| m.load(DATA, &(-7i32).to_le_bytes()));
    assert_eq!(c.reg(Ecx) as i32, -70);
    flags(&c, &[(Cf, false), (Of, false)]);
}

#[test]
fn widening_multiplies() {
    // imul ecx (f7 e9): 0x7fffffff * 2 = 0xfffffffe in EDX:EAX = 0:fffffffe.
    // As a signed 64-bit value it is not the sign extension of EAX
    // (0xfffffffe would mean -2), so CF = OF = 1.
    let (c, _) = run(&[0xF7, 0xE9], |c, _| {
        c.set_reg(Eax, 0x7FFF_FFFF);
        c.set_reg(Ecx, 2);
    });
    assert_eq!((c.reg(Edx), c.reg(Eax)), (0, 0xFFFF_FFFE));
    flags(&c, &[(Cf, true), (Of, true)]);
    // imul ecx with -1 * -1 = 1: fits.
    let (c, _) = run(&[0xF7, 0xE9], |c, _| {
        c.set_reg(Eax, 0xFFFF_FFFF);
        c.set_reg(Ecx, 0xFFFF_FFFF);
    });
    assert_eq!((c.reg(Edx), c.reg(Eax)), (0, 1));
    flags(&c, &[(Cf, false), (Of, false)]);
    // mul ecx (f7 e1): 0xffffffff * 2 = 1:fffffffe; high half nonzero.
    let (c, _) = run(&[0xF7, 0xE1], |c, _| {
        c.set_reg(Eax, 0xFFFF_FFFF);
        c.set_reg(Ecx, 2);
    });
    assert_eq!((c.reg(Edx), c.reg(Eax)), (1, 0xFFFF_FFFE));
    flags(&c, &[(Cf, true), (Of, true)]);
    // mul cl (f6 e1): AX = AL * CL = 0x10 * 0x10 = 0x100, AH nonzero.
    let (c, _) = run(&[0xF6, 0xE1], |c, _| {
        c.set_reg(Eax, 0xAAAA_0010);
        c.set_reg(Ecx, 0x10);
    });
    assert_eq!(c.reg(Eax), 0xAAAA_0100);
    flags(&c, &[(Cf, true), (Of, true)]);
}

#[test]
fn idiv_rounds_toward_zero() {
    // cdq; idiv ecx (99 f7 f9): -7 / 2 = -3 remainder -1.
    let (c, _) = run(&[0x99, 0xF7, 0xF9], |c, _| {
        c.set_reg(Eax, -7i32 as u32);
        c.set_reg(Ecx, 2);
    });
    assert_eq!(c.reg(Eax) as i32, -3);
    assert_eq!(c.reg(Edx) as i32, -1);
    check_undefined(&c, &[Cf, Of, Sf, Zf, Af, Pf]);
}

#[test]
fn tls_index_arithmetic_from_startup() {
    // LIFTER.md §9: the entry computes the TLS index as 0x94 / -4 = -37.
    // mov eax, 0x94; cdq; mov ecx, -4; idiv ecx
    let (c, _) = run(&[0xB8, 0x94, 0, 0, 0, 0x99, 0xB9, 0xFC, 0xFF, 0xFF, 0xFF, 0xF7, 0xF9], |_, _| {});
    assert_eq!(c.reg(Eax), 0xFFFF_FFDB);
    assert_eq!(c.reg(Edx), 0);
}

#[test]
fn idiv_by_zero_faults_before_writing() {
    let mut c = cpu();
    c.set_reg(Eax, 5);
    c.set_reg(Edx, 6);
    c.set_flag(Zf, true);
    let before = c.clone();
    let e = run_with(&[0xF7, 0xF9], &mut c, &mut mem()).unwrap_err();
    assert_eq!(e, Exit::Fault { pc: CODE, fault: Fault::DivideError, next: None });
    assert_eq!(c, before, "a faulting divide changes nothing");
}

#[test]
fn idiv_quotient_overflow_faults() {
    // EDX:EAX = -2^31, divided by -1, is 2^31: doesn't fit in EAX.
    let mut c = cpu();
    c.set_reg(Eax, 0x8000_0000);
    c.set_reg(Edx, 0xFFFF_FFFF);
    c.set_reg(Ecx, 0xFFFF_FFFF);
    let before = c.clone();
    let e = run_with(&[0xF7, 0xF9], &mut c, &mut mem()).unwrap_err();
    assert!(matches!(e, Exit::Fault { fault: Fault::DivideError, .. }));
    assert_eq!(c, before);
    // 8-bit: idiv cl (f6 f9) with AX = -128, CL = -1: 128 doesn't fit AL.
    let mut c = cpu();
    c.set_reg(Eax, 0xFF80);
    c.set_reg(Ecx, 0xFF);
    assert!(run_with(&[0xF6, 0xF9], &mut c, &mut mem()).is_err());
    // ...while AX = -7, CL = 2 gives AL = -3, AH = -1.
    let (c, _) = run(&[0xF6, 0xF9], |c, _| {
        c.set_reg(Eax, 0xFFF9);
        c.set_reg(Ecx, 2);
    });
    assert_eq!(c.reg(Eax), 0xFFFD);
}

#[test]
fn div_unsigned() {
    // div ecx (f7 f1): 10 / 3 = 3 remainder 1.
    let (c, _) = run(&[0xF7, 0xF1], |c, _| {
        c.set_reg(Eax, 10);
        c.set_reg(Ecx, 3);
    });
    assert_eq!((c.reg(Eax), c.reg(Edx)), (3, 1));
    // EDX:EAX = 2^32, / 1: quotient doesn't fit.
    let mut c = cpu();
    c.set_reg(Edx, 1);
    c.set_reg(Ecx, 1);
    assert!(matches!(
        run_with(&[0xF7, 0xF1], &mut c, &mut mem()),
        Err(Exit::Fault { fault: Fault::DivideError, .. })
    ));
}

// ---- partial registers -----------------------------------------------------

#[test]
fn partial_register_writes() {
    // mov ah, 0x12 (b4 12): only bits 8..15 change.
    let (c, _) = run(&[0xB4, 0x12], |c, _| c.set_reg(Eax, 0xAABB_CCDD));
    assert_eq!(c.reg(Eax), 0xAABB_12DD);
    // mov ax, 0x1234 (66 b8 34 12): bits 16..31 preserved.
    let (c, _) = run(&[0x66, 0xB8, 0x34, 0x12], |c, _| c.set_reg(Eax, 0xAABB_CCDD));
    assert_eq!(c.reg(Eax), 0xAABB_1234);
    // mov al, ah (88 e0).
    let (c, _) = run(&[0x88, 0xE0], |c, _| c.set_reg(Eax, 0xAABB_CCDD));
    assert_eq!(c.reg(Eax), 0xAABB_CCCC);
    // xchg al, ah (86 e0).
    let (c, _) = run(&[0x86, 0xE0], |c, _| c.set_reg(Eax, 0x1234));
    assert_eq!(c.reg(Eax), 0x3412);
    // movzx eax, ah (0f b6 c4); movsx ecx, al (0f be c8).
    let (c, _) = run(&[0x0F, 0xB6, 0xC4, 0x0F, 0xBE, 0xC8], |c, _| c.set_reg(Eax, 0xFFFF_8081));
    assert_eq!(c.reg(Eax), 0x80);
    assert_eq!(c.reg(Ecx), 0xFFFF_FF80);
    // inc bh (fe c7): 0xff -> 0, carry not out of the slice into EBX.
    let (c, _) = run(&[0xFE, 0xC7], |c, _| c.set_reg(Ebx, 0x0000_FF00));
    assert_eq!(c.reg(Ebx), 0);
    flags(&c, &[(Zf, true)]);
}

#[test]
fn lahf_sahf() {
    // lahf packs AH = SF ZF 0 AF 0 PF 1 CF. After xor eax, eax; stc; lahf
    // (31 c0 f9 9f): ZF=1 PF=1 CF=1, SF=0, AF undefined (stored as 0), so
    // AH = 0b0100_0111 = 0x47.
    let (c, _) = run(&[0x31, 0xC0, 0xF9, 0x9F], |_, _| {});
    assert_eq!(c.reg(Eax), 0x4700);
    // mov ah, 0x80; sahf (b4 80 9e): SF=1, the rest 0.
    let (c, _) = run(&[0xB4, 0x80, 0x9E], |c, _| set_flags(c, &[(Zf, true), (Cf, true)]));
    flags(&c, &[(Sf, true), (Zf, false), (Cf, false), (Pf, false), (Af, false)]);
}

// ---- stack and control ------------------------------------------------------

#[test]
fn push_esp_pushes_old_value_and_pop_esp_loads() {
    // push esp (54) stores the value before the decrement.
    let (c, m) = run(&[0x54], |_, _| {});
    assert_eq!(c.reg(Esp), STACK - 4);
    assert_eq!(m.read_u32(STACK - 4), Some(STACK));
    // pop esp (5c): ESP ends as the loaded value, not loaded + 4.
    let (c, _) = run(&[0x5C], |_, m| m.load(STACK, &0x1234u32.to_le_bytes()));
    assert_eq!(c.reg(Esp), 0x1234);
}

#[test]
fn call_and_ret_imm() {
    // call +0 (e8 00000000): pushes 0x1005 and transfers to 0x1005.
    let mut c = cpu();
    let mut m = mem();
    let insn = Decoder::with_ip(32, &[0xE8, 0, 0, 0, 0], CODE as u64, DecoderOptions::NONE).decode();
    let ir = lower(&insn, &Env::default(), Site::Plain);
    let flow = interp::step(&mut c, &mut m, &ir).unwrap();
    assert_eq!(flow, Flow::Call { target: 0x1005, continuation: 0x1005 });
    assert_eq!(c.reg(Esp), STACK - 4);
    assert_eq!(m.read_u32(STACK - 4), Some(0x1005));
    // ret 8 (c2 08 00): pops the return address and 8 bytes of arguments.
    let insn = Decoder::with_ip(32, &[0xC2, 0x08, 0x00], CODE as u64, DecoderOptions::NONE).decode();
    let ir = lower(&insn, &Env::default(), Site::Plain);
    let flow = interp::step(&mut c, &mut m, &ir).unwrap();
    assert_eq!(flow, Flow::Return { target: 0x1005 });
    assert_eq!(c.reg(Esp), STACK + 8);
}

#[test]
fn call_through_kernel_slot_keeps_the_load() {
    // call [0x4115c4] (ff 15 c4 15 41 00), with the slot as a kernel import:
    // the target still comes from memory, so a rewritten slot is honoured.
    let insn = Decoder::with_ip(32, &[0xFF, 0x15, 0xC4, 0x15, 0x41, 0x00], 0x2d21ff, DecoderOptions::NONE).decode();
    let mut env = Env::default();
    env.kernel_slots.insert(0x4115c4, 255);
    let ir = lower(&insn, &env, Site::Call { returns: true });
    match &ir.control {
        ir::Control::Call { target: ir::Target::Dynamic(_), continuation: 0x2d2205, kind, .. } => {
            assert_eq!(*kind, ir::CallKind::Kernel { slot: 0x4115c4, ordinal: 255 })
        }
        c => panic!("{c:?}"),
    }
    let mut c = cpu();
    let mut m = mem();
    m.load(0x4115c4, &0x8000_1234u32.to_le_bytes());
    let flow = interp::step(&mut c, &mut m, &ir).unwrap();
    assert_eq!(flow, Flow::Call { target: 0x8000_1234, continuation: 0x2d2205 });
    assert_eq!(m.read_u32(STACK - 4), Some(0x2d2205));
}

#[test]
fn faulting_push_leaves_esp() {
    // push [0xdead0000] (ff 35 ...): the load faults; nothing changes.
    let mut c = cpu();
    let before = c.clone();
    let e = run_with(&[0xFF, 0x35, 0x00, 0x00, 0xAD, 0xDE], &mut c, &mut mem()).unwrap_err();
    assert!(matches!(e, Exit::Memory { .. }));
    assert_eq!(c, before);
}

#[test]
fn enter_and_leave() {
    // enter 0x10, 0 (c8 10 00 00); leave (c9).
    let (c, m) = run(&[0xC8, 0x10, 0x00, 0x00], |c, _| c.set_reg(Ebp, 0xAAAA));
    assert_eq!(c.reg(Ebp), STACK - 4);
    assert_eq!(c.reg(Esp), STACK - 4 - 0x10);
    assert_eq!(m.read_u32(STACK - 4), Some(0xAAAA));
    let (c, _) = run(&[0xC8, 0x10, 0x00, 0x00, 0xC9], |c, _| c.set_reg(Ebp, 0xAAAA));
    assert_eq!((c.reg(Ebp), c.reg(Esp)), (0xAAAA, STACK));
}

#[test]
fn conditional_branches_signed_and_unsigned() {
    // cmp eax, 1; jb +2; mov bl, 1; (target) jl +2; mov cl, 1
    // 83 f8 01 / 72 02 / b3 01 / 7c 02 / b1 01
    let code = [0x83, 0xF8, 0x01, 0x72, 0x02, 0xB3, 0x01, 0x7C, 0x02, 0xB1, 0x01];
    // eax = 0: below and less, both taken: neither mov runs.
    let (c, _) = run(&code, |_, _| {});
    assert_eq!((c.reg(Ebx), c.reg(Ecx)), (0, 0));
    // eax = -1: unsigned huge (jb not taken), signed less (jl taken).
    let (c, _) = run(&code, |c, _| c.set_reg(Eax, 0xFFFF_FFFF));
    assert_eq!((c.reg(Ebx), c.reg(Ecx)), (1, 0));
    // eax = 5: neither.
    let (c, _) = run(&code, |c, _| c.set_reg(Eax, 5));
    assert_eq!((c.reg(Ebx), c.reg(Ecx)), (1, 1));
}

#[test]
fn setcc_and_cmov() {
    // cmp eax, ebx; setg cl; cmovl edx, ebx
    // 39 d8 / 0f 9f c1 / 0f 4c d3
    let code = [0x39, 0xD8, 0x0F, 0x9F, 0xC1, 0x0F, 0x4C, 0xD3];
    let (c, _) = run(&code, |c, _| {
        c.set_reg(Eax, 0xFFFF_FFFF);
        c.set_reg(Ebx, 1);
        c.set_reg(Ecx, 0xFFFF_FFFF);
        c.set_reg(Edx, 7);
    });
    assert_eq!(c.reg(Ecx), 0xFFFF_FF00, "setg writes only CL");
    assert_eq!(c.reg(Edx), 1, "-1 < 1 so cmovl moves");
}

#[test]
fn loop_counts_down() {
    // mov ecx, 3; (top) inc eax; loop top
    // b9 03 00 00 00 / 40 / e2 fd
    let (c, _) = run(&[0xB9, 0x03, 0, 0, 0, 0x40, 0xE2, 0xFD], |_, _| {});
    assert_eq!((c.reg(Eax), c.reg(Ecx)), (3, 0));
}

// ---- string instructions ----------------------------------------------------

#[test]
fn rep_movsd_copies_and_advances() {
    // rep movsd (f3 a5): 3 dwords from DATA to DATA+0x40.
    let (c, m) = run(&[0xF3, 0xA5], |c, m| {
        m.load(DATA, &[1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0]);
        c.set_reg(Esi, DATA);
        c.set_reg(Edi, DATA + 0x40);
        c.set_reg(Ecx, 3);
    });
    assert_eq!(c.reg(Ecx), 0);
    assert_eq!(c.reg(Esi), DATA + 12);
    assert_eq!(c.reg(Edi), DATA + 0x4C);
    assert_eq!(m.read_u32(DATA + 0x48), Some(3));
}

#[test]
fn rep_with_zero_count_does_nothing() {
    // rep stosb (f3 aa) with ECX = 0: no store (EDI points at unmapped
    // memory to prove it), no change.
    let mut c = cpu();
    c.set_reg(Edi, 0xDEAD_0000);
    let before = c.clone();
    let mut m = mem();
    run_with(&[0xF3, 0xAA], &mut c, &mut m).unwrap();
    assert_eq!(c.gpr, before.gpr);
    assert_eq!(m.read_u8(0xDEAD_0000), None);
}

#[test]
fn std_reverses_string_direction() {
    // std; rep stosb (fd f3 aa): fill 4 bytes downward from DATA+3.
    let (c, m) = run(&[0xFD, 0xF3, 0xAA], |c, _| {
        c.set_reg(Eax, 0x5A);
        c.set_reg(Edi, DATA + 3);
        c.set_reg(Ecx, 4);
    });
    assert_eq!(c.reg(Edi), DATA - 1);
    assert_eq!(m.read_u32(DATA), Some(0x5A5A_5A5A));
}

#[test]
fn repne_scasb_finds_terminator() {
    // The strlen idiom: or ecx, -1; xor eax, eax; repne scasb
    // 83 c9 ff / 31 c0 / f2 ae. "abc\0": four iterations, the last matches.
    let (c, _) = run(&[0x83, 0xC9, 0xFF, 0x31, 0xC0, 0xF2, 0xAE], |c, m| {
        m.load(DATA, b"abc\0");
        c.set_reg(Edi, DATA);
    });
    assert_eq!(c.reg(Edi), DATA + 4);
    assert_eq!(c.reg(Ecx), 0xFFFF_FFFF - 4);
    flags(&c, &[(Zf, true)]);
}

#[test]
fn repe_cmpsb_stops_at_mismatch() {
    // repe cmpsb (f3 a6): "abcd" vs "abxd", 4 max: stops after the third.
    let (c, _) = run(&[0xF3, 0xA6], |c, m| {
        m.load(DATA, b"abcd");
        m.load(DATA + 0x10, b"abxd");
        c.set_reg(Esi, DATA);
        c.set_reg(Edi, DATA + 0x10);
        c.set_reg(Ecx, 4);
    });
    assert_eq!(c.reg(Ecx), 1);
    assert_eq!(c.reg(Esi), DATA + 3);
    // 'c' - 'x' borrows.
    flags(&c, &[(Zf, false), (Cf, true)]);
}

// ---- atomics and bit operations ---------------------------------------------

#[test]
fn cmpxchg_success_and_failure() {
    // lock cmpxchg [DATA], ecx (f0 0f b1 0d <DATA>)
    let mut code = vec![0xF0, 0x0F, 0xB1, 0x0D];
    code.extend(DATA.to_le_bytes());
    let insn = Decoder::with_ip(32, &code, CODE as u64, DecoderOptions::NONE).decode();
    assert!(lower(&insn, &Env::default(), Site::Plain).atomic.is_some());
    let (c, m) = run(&code, |c, m| {
        m.load(DATA, &5u32.to_le_bytes());
        c.set_reg(Eax, 5);
        c.set_reg(Ecx, 9);
    });
    assert_eq!(m.read_u32(DATA), Some(9));
    assert_eq!(c.reg(Eax), 5);
    flags(&c, &[(Zf, true)]);
    let (c, m) = run(&code, |c, m| {
        m.load(DATA, &5u32.to_le_bytes());
        c.set_reg(Eax, 4);
        c.set_reg(Ecx, 9);
    });
    assert_eq!(m.read_u32(DATA), Some(5));
    assert_eq!(c.reg(Eax), 5);
    flags(&c, &[(Zf, false), (Cf, true)]);
    // Register form with the accumulator as destination: cmpxchg eax, ecx
    // (0f b1 c8) compares eax with itself, so it always stores ecx.
    let (c, _) = run(&[0x0F, 0xB1, 0xC8], |c, _| {
        c.set_reg(Eax, 3);
        c.set_reg(Ecx, 9);
    });
    assert_eq!(c.reg(Eax), 9);
}

#[test]
fn xadd_and_xchg() {
    // lock xadd [DATA], eax (f0 0f c1 05 <DATA>)
    let mut code = vec![0xF0, 0x0F, 0xC1, 0x05];
    code.extend(DATA.to_le_bytes());
    let (c, m) = run(&code, |c, m| {
        m.load(DATA, &10u32.to_le_bytes());
        c.set_reg(Eax, 3);
    });
    assert_eq!(m.read_u32(DATA), Some(13));
    assert_eq!(c.reg(Eax), 10);
    // xchg [DATA], ecx (87 0d <DATA>) is atomic without a prefix.
    let mut code = vec![0x87, 0x0D];
    code.extend(DATA.to_le_bytes());
    let insn = Decoder::with_ip(32, &code, CODE as u64, DecoderOptions::NONE).decode();
    assert!(lower(&insn, &Env::default(), Site::Plain).atomic.is_some());
    let (c, m) = run(&code, |c, m| {
        m.load(DATA, &1u32.to_le_bytes());
        c.set_reg(Ecx, 2);
    });
    assert_eq!((m.read_u32(DATA), c.reg(Ecx)), (Some(2), 1));
}

#[test]
fn bit_test_and_scan() {
    // bt eax, 5 (0f ba e0 05): CF = bit 5.
    let (c, _) = run(&[0x0F, 0xBA, 0xE0, 0x05], |c, _| c.set_reg(Eax, 0x20));
    flags(&c, &[(Cf, true)]);
    // bts [DATA], ecx (0f ab 0d <DATA>) with ecx = 35: a register offset
    // reaches the next dword: bit 3 of [DATA+4].
    let mut code = vec![0x0F, 0xAB, 0x0D];
    code.extend(DATA.to_le_bytes());
    let (c, m) = run(&code, |c, _| c.set_reg(Ecx, 35));
    assert_eq!(m.read_u32(DATA + 4), Some(8));
    flags(&c, &[(Cf, false)]);
    // bsf ecx, eax / bsr edx, eax (0f bc c8 / 0f bd d0) on 0x00f0.
    let (c, _) = run(&[0x0F, 0xBC, 0xC8, 0x0F, 0xBD, 0xD0], |c, _| c.set_reg(Eax, 0xF0));
    assert_eq!((c.reg(Ecx), c.reg(Edx)), (4, 7));
    flags(&c, &[(Zf, false)]);
    // bsf on zero sets ZF and leaves the destination.
    let (c, _) = run(&[0x0F, 0xBC, 0xC8], |c, _| c.set_reg(Ecx, 0x77));
    assert_eq!(c.reg(Ecx), 0x77);
    flags(&c, &[(Zf, true)]);
}

#[test]
fn fs_relative_access_uses_fs_base() {
    // mov eax, fs:[0x28] (64 a1 28 00 00 00).
    let (c, _) = run(&[0x64, 0xA1, 0x28, 0, 0, 0], |c, m| {
        c.fs_base = DATA;
        m.load(DATA + 0x28, &0xCAFEu32.to_le_bytes());
    });
    assert_eq!(c.reg(Eax), 0xCAFE);
}

#[test]
fn traps() {
    let mut c = cpu();
    // int3 (cc): the saved EIP is the next instruction.
    assert_eq!(
        run_with(&[0xCC], &mut c, &mut mem()),
        Err(Exit::Fault { pc: CODE, fault: Fault::Breakpoint, next: Some(CODE + 1) })
    );
    // ud2 (0f 0b).
    assert!(matches!(run_with(&[0x0F, 0x0B], &mut c, &mut mem()), Err(Exit::Fault { fault: Fault::InvalidOpcode, next: None, .. })));
}

#[test]
fn unsupported_is_explicit() {
    // fldenv m14 (66 d9 20): the 16-bit environment layout is not lowered.
    let insn = Decoder::with_ip(32, &[0x66, 0xD9, 0x20], CODE as u64, DecoderOptions::NONE).decode();
    let ir = lower(&insn, &Env::default(), Site::Plain);
    assert_eq!(ir.unsupported().map(|u| u.0), Some("fldenv"));
    let mut c = cpu();
    assert!(matches!(interp::step(&mut c, &mut mem(), &ir), Err(Exit::Unsupported { .. })));
}

// ---- SSE bit moves ----------------------------------------------------------

const X0: u128 = 0x4444_4444_3333_3333_2222_2222_1111_1111;
const X1: u128 = 0xDDDD_DDDD_CCCC_CCCC_BBBB_BBBB_AAAA_AAAA;

#[test]
fn movss_merges_or_zeroes() {
    // movss xmm0, xmm1 (f3 0f 10 c1): only lane 0 changes.
    let (c, _) = run(&[0xF3, 0x0F, 0x10, 0xC1], |c, _| {
        c.xmm[0] = X0;
        c.xmm[1] = X1;
    });
    assert_eq!(c.xmm[0], 0x4444_4444_3333_3333_2222_2222_AAAA_AAAA);
    // movss xmm0, [DATA] (f3 0f 10 05 <DATA>): a load zeroes lanes 1-3.
    let mut code = vec![0xF3, 0x0F, 0x10, 0x05];
    code.extend(DATA.to_le_bytes());
    let (c, _) = run(&code, |c, m| {
        c.xmm[0] = X0;
        m.load(DATA, &0x3F80_0000u32.to_le_bytes());
    });
    assert_eq!(c.xmm[0], 0x3F80_0000);
    // movss [DATA], xmm1 (f3 0f 11 0d <DATA>): stores 4 bytes only.
    let mut code = vec![0xF3, 0x0F, 0x11, 0x0D];
    code.extend(DATA.to_le_bytes());
    let (_, m) = run(&code, |c, _| c.xmm[1] = X1);
    assert_eq!((m.read_u32(DATA), m.read_u32(DATA + 4)), (Some(0xAAAA_AAAA), Some(0)));
}

#[test]
fn movaps_alignment_faults() {
    // movaps xmm0, [eax] (0f 28 00) with eax = DATA + 4: #GP, xmm0 kept.
    let mut c = cpu();
    c.set_reg(Eax, DATA + 4);
    c.xmm[0] = X0;
    let e = run_with(&[0x0F, 0x28, 0x00], &mut c, &mut mem()).unwrap_err();
    assert!(matches!(e, Exit::Fault { fault: Fault::GeneralProtection, .. }));
    assert_eq!(c.xmm[0], X0);
    // movups xmm0, [eax] (0f 10 00) has no alignment rule.
    let (c, _) = run(&[0x0F, 0x10, 0x00], |c, m| {
        c.set_reg(Eax, DATA + 4);
        m.load(DATA + 4, &X1.to_le_bytes());
    });
    assert_eq!(c.xmm[0], X1);
}

#[test]
fn sse_halves_logic_shuffles() {
    // movhlps xmm0, xmm1 (0f 12 c1): xmm0.lo = xmm1.hi.
    let (c, _) = run(&[0x0F, 0x12, 0xC1], |c, _| {
        c.xmm[0] = X0;
        c.xmm[1] = X1;
    });
    assert_eq!(c.xmm[0], 0x4444_4444_3333_3333_DDDD_DDDD_CCCC_CCCC);
    // movlhps xmm0, xmm1 (0f 16 c1): xmm0.hi = xmm1.lo.
    let (c, _) = run(&[0x0F, 0x16, 0xC1], |c, _| {
        c.xmm[0] = X0;
        c.xmm[1] = X1;
    });
    assert_eq!(c.xmm[0], 0xBBBB_BBBB_AAAA_AAAA_2222_2222_1111_1111);
    // xorps xmm0, xmm0 (0f 57 c0): zero.
    let (c, _) = run(&[0x0F, 0x57, 0xC0], |c, _| c.xmm[0] = X0);
    assert_eq!(c.xmm[0], 0);
    // andnps xmm0, xmm1 (0f 55 c1): !xmm0 & xmm1.
    let (c, _) = run(&[0x0F, 0x55, 0xC1], |c, _| {
        c.xmm[0] = X0;
        c.xmm[1] = X1;
    });
    assert_eq!(c.xmm[0], !X0 & X1);
    // shufps xmm0, xmm1, 0x1b (0f c6 c1 1b): lanes d3 d2 s1 s0.
    let (c, _) = run(&[0x0F, 0xC6, 0xC1, 0x1B], |c, _| {
        c.xmm[0] = X0;
        c.xmm[1] = X1;
    });
    assert_eq!(c.xmm[0], 0xAAAA_AAAA_BBBB_BBBB_3333_3333_4444_4444);
    // unpcklps xmm0, xmm1 (0f 14 c1): d0 s0 d1 s1.
    let (c, _) = run(&[0x0F, 0x14, 0xC1], |c, _| {
        c.xmm[0] = X0;
        c.xmm[1] = X1;
    });
    assert_eq!(c.xmm[0], 0xBBBB_BBBB_2222_2222_AAAA_AAAA_1111_1111);
    // movmskps eax, xmm1 (0f 50 c1): lanes A,B,C,D all have the sign set.
    let (c, _) = run(&[0x0F, 0x50, 0xC1], |c, _| c.xmm[1] = X1 & !(1u128 << 63));
    assert_eq!(c.reg(Eax), 0b1101);
}

// ---- review fixes: terminal unsupported, transfers, policies -------------

fn lower_one(code: &[u8], env: &Env, site: Site<'_>) -> ir::IrInsn {
    let insn = Decoder::with_ip(32, code, CODE as u64, DecoderOptions::NONE).decode();
    let ir = lower(&insn, env, site);
    ir.validate().unwrap_or_else(|e| panic!("{e}\n{ir}"));
    ir
}

#[test]
fn unsupported_is_a_terminal_stop() {
    // in al, dx (ec: port I/O is the D3D overrides' business), a far jmp to
    // a selector other than the flat code segment, lidt (0f 01 18) and the
    // 16-bit sgdt (66 0f 01 01) are not lowered: they stop, they never fall through.
    for code in [&[0xEC][..], &[0xEA, 0, 0, 0, 0, 0x10, 0][..], &[0x0F, 0x01, 0x18][..], &[0x66, 0x0F, 0x01, 0x01][..]] {
        let ir = lower_one(code, &Env::default(), Site::Plain);
        assert!(!ir.is_supported());
        assert_eq!(ir.control, ir::Control::Stop(ir::StopReason::Unsupported));
    }
}

#[test]
fn flat_segment_model_lowers_cs_reads_sgdt_and_the_cs_reload() {
    use ir::interp::Memory;
    // mov [ecx], cs (8c 09) and sgdt [ecx] (0f 01 01).
    let (mut cpu, mut mem) = (Cpu::default(), SparseMemory::default());
    cpu.gpr[1] = 0x2010;
    for (code, size) in [(&[0x8C, 0x09][..], 2), (&[0x0F, 0x01, 0x01][..], 6)] {
        let ir = lower_one(code, &Env::default(), Site::Plain);
        assert!(ir.is_supported(), "{code:x?}");
        interp::step(&mut cpu, &mut mem, &ir).unwrap_or_else(|e| panic!("{e:?}"));
        let mut got = vec![0; size];
        mem.read(0x2010, &mut got).unwrap();
        if size == 2 {
            assert_eq!(got, lift::lower::XBOX_CS.to_le_bytes());
        } else {
            let (limit, base) = lift::lower::XBOX_GDT;
            assert_eq!(got[..2], limit.to_le_bytes());
            assert_eq!(got[2..], base.to_le_bytes());
        }
    }
    // jmp 0008:00401234 is a direct jump to 0x401234.
    let ir = lower_one(&[0xEA, 0x34, 0x12, 0x40, 0x00, 0x08, 0x00], &Env::default(), Site::Plain);
    assert!(matches!(ir.control, ir::Control::Jump { target: ir::Target::Direct(0x401234), .. }));
}

#[test]
fn jmp_through_a_kernel_slot_keeps_slot_and_ordinal() {
    // jmp [0x4115c4] (ff 25 c4 15 41 00)
    let env = Env { kernel_slots: [(0x4115c4, 255)].into(), ..Default::default() };
    let ir = lower_one(&[0xFF, 0x25, 0xC4, 0x15, 0x41, 0x00], &env, Site::Tail);
    assert!(matches!(
        ir.control,
        ir::Control::Jump {
            target: ir::Target::Dynamic(_),
            tail: true,
            kind: ir::CallKind::Kernel { slot: 0x4115c4, ordinal: 255 }
        }
    ));
}

#[test]
fn switch_records_its_shape_and_whether_the_table_is_read_only() {
    use analyze::{JumpTable, SwitchKind};
    // jmp [eax*4 + 0x2000] (ff 24 85 00 20 00 00)
    let code = [0xFF, 0x24, 0x85, 0x00, 0x20, 0x00, 0x00];
    let bounded = JumpTable {
        site: CODE,
        table: 0x2000,
        kind: SwitchKind::Bounded,
        count: 2,
        bound: Some(2),
        index_table: None,
        targets: vec![0x1100, 0x1200],
        holes: vec![],
    };
    let ro = Env { readonly: vec![(0x2000, 0x3000)], ..Default::default() };
    let ir = lower_one(&code, &ro, Site::Switch(&bounded));
    let ir::Control::Switch { shape, immutable, cases, .. } = ir.control else { panic!() };
    assert_eq!(shape, ir::SwitchShape::Bounded { entries: 2 });
    assert!(immutable);
    assert_eq!(cases, [ir::Dest::local(0x1100), ir::Dest::local(0x1200)]);
    // Writable, or an unbounded heuristic: not immutable.
    let ir = lower_one(&code, &Env::default(), Site::Switch(&bounded));
    assert!(matches!(ir.control, ir::Control::Switch { immutable: false, .. }));
    let unbounded = JumpTable { kind: SwitchKind::Unbounded, bound: None, ..bounded };
    let ir = lower_one(&code, &ro, Site::Switch(&unbounded));
    assert!(matches!(ir.control, ir::Control::Switch { shape: ir::SwitchShape::Unbounded, immutable: false, .. }));
}

#[test]
fn popfd_restoring_saved_flags_runs() {
    // pushfd; popfd (9c 9d): the usual save/restore round trip.
    let (c, _) = run(&[0x9C, 0x9D], |c, _| set_flags(c, &[(Cf, true), (Zf, true), (Df, true)]));
    flags(&c, &[(Cf, true), (Zf, true), (Df, true)]);
    assert_eq!(c.reg(Esp), STACK);
}

#[test]
fn popfd_setting_tf_stops_instead_of_silently_tracing() {
    // push 0x100; popfd (68 00 01 00 00 9d): TF would be set with no trap.
    let mut c = cpu();
    let e = run_with(&[0x68, 0x00, 0x01, 0x00, 0x00, 0x9D], &mut c, &mut mem()).unwrap_err();
    assert_eq!(e, Exit::Stop { pc: CODE + 5, reason: ir::StopReason::Unmodelled });
    assert_eq!(c.reg(Esp), STACK - 4, "nothing popped");
    assert_eq!(c.eflags & 0x100, 0);
}

/// Memory whose writes all fault: shows whether a store happened.
struct ReadOnly(SparseMemory);

impl interp::Memory for ReadOnly {
    fn read(&mut self, addr: u32, buf: &mut [u8]) -> Result<(), interp::MemFault> {
        self.0.read(addr, buf)
    }
    fn write(&mut self, addr: u32, _: &[u8]) -> Result<(), interp::MemFault> {
        Err(interp::MemFault { addr, write: true })
    }
}

#[test]
fn shift_by_cl_of_zero_keeps_value_and_every_flag() {
    // shl dword [0x4000], cl (d3 25 00 40 00 00)
    let code = [0xD3, 0x25, 0x00, 0x40, 0x00, 0x00];
    let ir = lower_one(&code, &Env::default(), Site::Plain);
    let mut c = cpu();
    c.set_reg(Ecx, 0x20); // masks to 0
    set_flags(&mut c, &[(Af, true), (Of, true), (Cf, true)]);
    let before = c.clone();
    let mut m = mem();
    m.load(DATA, &0x8000_0001u32.to_le_bytes());
    interp::step(&mut c, &mut m, &ir).unwrap();
    c.eip = before.eip;
    assert_eq!(c, before, "AF included: hardware leaves it for a zero count");
    assert_eq!(m.read_u32(DATA), Some(0x8000_0001));
}

#[test]
fn zero_count_memory_shifts_still_check_access() {
    // A zero count is still a read-modify-write of memory: hardware faults
    // on an unmapped or read-only operand whatever the count.
    // shl dword [eax], cl (d3 20); shl dword [eax], 0 (c1 20 00);
    // shld [eax], edx, cl (0f a5 10); shrd [eax], edx, 0 (0f ac 10 00).
    for code in [&[0xD3, 0x20][..], &[0xC1, 0x20, 0x00], &[0x0F, 0xA5, 0x10], &[0x0F, 0xAC, 0x10, 0x00]] {
        let ir = lower_one(code, &Env::default(), Site::Plain);
        let mut c = cpu();
        c.set_reg(Ecx, 0);
        c.set_reg(Eax, 0x0090_0000); // unmapped
        let e = interp::step(&mut c, &mut mem(), &ir).unwrap_err();
        assert!(matches!(e, Exit::Memory { fault: interp::MemFault { addr: 0x0090_0000, write: false }, .. }), "{code:x?}: {e:?}");
        c.set_reg(Eax, DATA);
        let before = c.clone();
        let e = interp::step(&mut c, &mut ReadOnly(mem()), &ir).unwrap_err();
        assert!(matches!(e, Exit::Memory { fault: interp::MemFault { addr: DATA, write: true }, .. }), "{code:x?}: {e:?}");
        assert_eq!(c, before, "{code:x?}: nothing changes on the fault");
    }
}

#[test]
fn sixteen_bit_double_shifts() {
    // shld ax, dx, 4 (66 0f a4 d0 04): ax = 0x1234, dx = 0xabcd -> 0x234a,
    // CF = last bit out of ax (bit 12 of 0x1234 = 1).
    let (c, _) = run(&[0x66, 0x0F, 0xA4, 0xD0, 0x04], |c, _| {
        c.set_reg(Eax, 0xFFFF_1234);
        c.set_reg(Edx, 0xABCD);
    });
    assert_eq!(c.reg(Eax), 0xFFFF_234A);
    flags(&c, &[(Cf, true), (Zf, false), (Sf, false), (Pf, false)]);
    // shrd ax, dx, cl (66 0f ad d0), cl = 1: 0x1234 >> 1 with dx's bit 0
    // shifted in at the top: 0x891a; CF = 0; OF = sign changed = 1.
    let (c, _) = run(&[0x66, 0x0F, 0xAD, 0xD0], |c, _| {
        c.set_reg(Eax, 0x1234);
        c.set_reg(Edx, 0x0001);
        c.set_reg(Ecx, 1);
    });
    assert_eq!(c.reg(Eax), 0x891A);
    flags(&c, &[(Cf, false), (Of, true), (Sf, true)]);
    // A count above 16 leaves every status flag undefined.
    let (c, _) = run(&[0x66, 0x0F, 0xAD, 0xD0], |c, _| {
        c.set_reg(Eax, 0x1234);
        c.set_reg(Ecx, 20);
    });
    check_undefined(&c, &[Cf, Of, Sf, Zf, Af, Pf]);
    let (c, _) = run(&[0x66, 0x0F, 0xA4, 0xD0, 0x11], |_, _| {});
    check_undefined(&c, &[Cf, Of, Sf, Zf, Af, Pf]);
}

#[test]
fn sixteen_bit_indirect_jmp_and_call_are_unsupported() {
    // jmp word [eax] (66 ff 20), call word [eax] (66 ff 10), and the
    // register forms (66 ff e0, 66 ff d0) load a 16-bit IP.
    for code in [&[0x66, 0xFF, 0x20][..], &[0x66, 0xFF, 0x10], &[0x66, 0xFF, 0xE0], &[0x66, 0xFF, 0xD0]] {
        let ir = lower_one(code, &Env::default(), Site::Plain);
        assert!(!ir.is_supported(), "{code:x?}:\n{ir}");
    }
    assert!(lower_one(&[0xFF, 0x20], &Env::default(), Site::Plain).is_supported());
}

#[test]
fn ldmxcsr_with_daz_is_a_general_protection_fault() {
    // ldmxcsr [0x4000] (0f ae 15 00 40 00 00) with DAZ (bit 6), which the
    // Pentium III lacks: #GP, MXCSR unchanged.
    let mut c = cpu();
    let mut m = mem();
    m.load(DATA, &0x1FC0u32.to_le_bytes());
    let e = run_with(&[0x0F, 0xAE, 0x15, 0x00, 0x40, 0x00, 0x00], &mut c, &mut m).unwrap_err();
    assert_eq!(e, Exit::Fault { pc: CODE, fault: Fault::GeneralProtection, next: None });
    assert_eq!(c.mxcsr, interp::MXCSR_DEFAULT);
}

#[test]
fn faulting_rep_compare_reports_the_flags_it_started_with() {
    // repe cmpsb (f3 a6), ESI = EDI = the last mapped byte, ECX = 2: the
    // first iteration compares equal (ZF = 1), the second faults. EFLAGS
    // must be the pre-instruction 0x3 (CF set), with the first
    // iteration's pointer and count progress kept. Both directions.
    for (df, step) in [(false, 1u32), (true, 1u32.wrapping_neg())] {
        let mut m = interp::FlatMemory::new(DATA, 0x100);
        let last = if df { DATA } else { DATA + 0xFF };
        let mut c = cpu();
        c.set_reg(Esi, last);
        c.set_reg(Edi, last);
        c.set_reg(Ecx, 2);
        c.eflags = 0x3 | if df { 0x400 } else { 0 };
        let flags_before = (c.eflags, c.flags_valid);
        let ir = lower_one(&[0xF3, 0xA6], &Env::default(), Site::Plain);
        let e = interp::step(&mut c, &mut m, &ir).unwrap_err();
        assert!(matches!(e, Exit::Memory { pc: CODE, .. }), "{e:?}");
        assert_eq!((c.eflags, c.flags_valid), flags_before, "df {df}");
        assert_eq!((c.reg(Ecx), c.reg(Esi), c.reg(Edi)), (1, last.wrapping_add(step), last.wrapping_add(step)));
    }
}

#[test]
fn atomic_forms_carry_their_operation() {
    use ir::{Atomic, AtomicOp};
    let one = |code: &[u8]| lower_one(code, &Env::default(), Site::Plain);
    // lock xadd [eax], ecx (f0 0f c1 08)
    let x = one(&[0xF0, 0x0F, 0xC1, 0x08]);
    assert!(matches!(x.atomic, Some(Atomic { op: AtomicOp::Xadd, operand: Some(_), old: Some((Ecx, ir::Slice::Full)) })));
    // lock add dword [eax], 1 (f0 83 00 01): constant operand.
    let a = one(&[0xF0, 0x83, 0x00, 0x01]);
    assert_eq!(a.atomic, Some(Atomic { op: AtomicOp::Add, operand: Some(ir::Value::u32(1)), old: None }));
    // lock dec word [eax] (66 f0 ff 08): no operand.
    let d = one(&[0x66, 0xF0, 0xFF, 0x08]);
    assert_eq!(d.atomic, Some(Atomic { op: AtomicOp::Dec, operand: None, old: None }));
    // lock cmpxchg [eax], dl (f0 0f b0 10); xchg [eax], bx (66 87 18).
    assert!(matches!(one(&[0xF0, 0x0F, 0xB0, 0x10]).atomic, Some(Atomic { op: AtomicOp::Cmpxchg, old: None, .. })));
    assert!(matches!(one(&[0x66, 0x87, 0x18]).atomic, Some(Atomic { op: AtomicOp::Xchg, old: Some((Ebx, ir::Slice::Low16)), .. })));
    // lock bts [eax], ecx (f0 0f ab 08)
    assert!(matches!(one(&[0xF0, 0x0F, 0xAB, 0x08]).atomic, Some(Atomic { op: AtomicOp::Bts, .. })));
    // Not lowered: lock adc and lock sbb in every operand form (dword,
    // byte and word registers, imm8 and imm32), and LOCK where #UD (lock
    // mov [eax], ecx: f0 89 08).
    let refused: [&[u8]; 13] = [
        &[0xF0, 0x11, 0x08],                         // lock adc [eax], ecx
        &[0xF0, 0x10, 0x08],                         // lock adc [eax], cl
        &[0x66, 0xF0, 0x11, 0x08],                   // lock adc [eax], cx
        &[0xF0, 0x83, 0x10, 0x05],                   // lock adc dword [eax], 5
        &[0xF0, 0x81, 0x10, 0x78, 0x56, 0x34, 0x12], // lock adc dword [eax], 0x12345678
        &[0xF0, 0x19, 0x08],                         // lock sbb [eax], ecx
        &[0xF0, 0x18, 0x08],                         // lock sbb [eax], cl
        &[0x66, 0xF0, 0x19, 0x08],                   // lock sbb [eax], cx
        &[0xF0, 0x83, 0x18, 0x05],                   // lock sbb dword [eax], 5
        &[0xF0, 0x80, 0x18, 0x05],                   // lock sbb byte [eax], 5
        &[0xF0, 0x81, 0x18, 0x78, 0x56, 0x34, 0x12], // lock sbb dword [eax], 0x12345678
        &[0xF0, 0x89, 0x08],                         // lock mov [eax], ecx (#UD)
        &[0xF0, 0x01, 0xC8],                         // lock add eax, ecx (#UD)
    ];
    for code in refused {
        let i = one(code);
        assert!(!i.is_supported() && i.atomic.is_none(), "{code:x?}");
    }
    // Plain forms are not atomic.
    assert_eq!(one(&[0x0F, 0xC1, 0x08]).atomic, None);
}

#[test]
fn shift_by_cl_defines_of_only_for_a_count_of_one() {
    // shl eax, cl (d3 e0)
    let (c, _) = run(&[0xD3, 0xE0], |c, _| {
        c.set_reg(Eax, 0x4000_0000);
        c.set_reg(Ecx, 1);
    });
    flags(&c, &[(Of, true), (Cf, false)]);
    let (c, _) = run(&[0xD3, 0xE0], |c, _| {
        c.set_reg(Eax, 0x4000_0000);
        c.set_reg(Ecx, 2);
    });
    check_undefined(&c, &[Of, Af]);
    flags(&c, &[(Cf, true), (Zf, true)]);
    // shl al, cl with CL = 9 (d2 e0): CF undefined once the count passes
    // an 8-bit operand's width.
    let (c, _) = run(&[0xD2, 0xE0], |c, _| c.set_reg(Ecx, 9));
    check_undefined(&c, &[Cf, Of]);
}

#[test]
fn rotate_through_carry_by_a_multiple_of_nine_keeps_cf_and_of() {
    // rcl al, 9 (c0 d0 09) and rcl al, cl with CL = 9 (d2 d0).
    for code in [&[0xC0, 0xD0, 0x09][..], &[0xD2, 0xD0][..]] {
        let (c, _) = run(code, |c, _| {
            c.set_reg(Eax, 0x81);
            c.set_reg(Ecx, 9);
            set_flags(c, &[(Cf, true), (Of, true)]);
        });
        assert_eq!(c.reg(Eax), 0x81);
        flags(&c, &[(Cf, true), (Of, true)]);
    }
}

#[test]
fn int_3_is_a_breakpoint_trap() {
    // int 3 (cd 03): #BP like int3, saved EIP after the instruction.
    let e = run_with(&[0xCD, 0x03], &mut cpu(), &mut mem()).unwrap_err();
    assert_eq!(e, Exit::Fault { pc: CODE, fault: Fault::Breakpoint, next: Some(CODE + 2) });
}

#[test]
fn repne_on_movs_repeats_like_rep() {
    // repne movsb (f2 a4), ECX = 3.
    let (c, m) = run(&[0xF2, 0xA4], |c, m| {
        m.load(DATA, &[1, 2, 3, 4]);
        c.set_reg(Esi, DATA);
        c.set_reg(Edi, DATA + 0x10);
        c.set_reg(Ecx, 3);
    });
    assert_eq!(c.reg(Ecx), 0);
    assert_eq!((0..4).map(|i| m.read_u8(DATA + 0x10 + i)).collect::<Vec<_>>(), [Some(1), Some(2), Some(3), Some(0)]);
}

#[test]
fn rep_fault_mid_iteration_keeps_completed_iterations() {
    // rep stosd (f3 ab) with 16 mapped bytes and EDI 8 bytes in: two
    // iterations commit, the third faults with the instruction restartable.
    let mut m = interp::FlatMemory::new(DATA, 0x10);
    let mut c = cpu();
    c.set_reg(Eax, 0xAABB_CCDD);
    c.set_reg(Edi, DATA + 8);
    c.set_reg(Ecx, 4);
    let ir = lower_one(&[0xF3, 0xAB], &Env::default(), Site::Plain);
    let e = interp::step(&mut c, &mut m, &ir).unwrap_err();
    assert_eq!(e, Exit::Memory { pc: CODE, fault: interp::MemFault { addr: DATA + 0x10, write: true } });
    assert_eq!((c.reg(Ecx), c.reg(Edi), c.eip), (2, DATA + 0x10, CODE));
    assert_eq!(m.read_u32(DATA + 8), Some(0xAABB_CCDD));
    assert_eq!(m.read_u32(DATA + 12), Some(0xAABB_CCDD));
}

#[test]
fn fp_control_goes_through_intrinsics() {
    // fnstcw [0x4000] (d9 3d 00 40 00 00); stmxcsr [0x4004]
    // (0f ae 1d 04 40 00 00). fninit is a helper call: tests/x87.rs.
    let code = [0xD9, 0x3D, 0x00, 0x40, 0x00, 0x00, 0x0F, 0xAE, 0x1D, 0x04, 0x40, 0x00, 0x00];
    let (_, m) = run(&code, |c, _| {
        c.x87.control = 0x027F;
        c.mxcsr = 0x1F81;
    });
    assert_eq!(m.read_u32(DATA).map(|v| v as u16), Some(0x027F));
    assert_eq!(m.read_u32(DATA + 4), Some(0x1F81));
    // ldmxcsr [0x4000] (0f ae 15 00 40 00 00) with a reserved bit: #GP,
    // MXCSR unchanged.
    let mut c = cpu();
    let mut m = mem();
    m.load(DATA, &0x1_0000u32.to_le_bytes());
    let e = run_with(&[0x0F, 0xAE, 0x15, 0x00, 0x40, 0x00, 0x00], &mut c, &mut m).unwrap_err();
    assert_eq!(e, Exit::Fault { pc: CODE, fault: Fault::GeneralProtection, next: None });
    assert_eq!(c.mxcsr, interp::MXCSR_DEFAULT);
}

// ---- SSE floating point and MMX --------------------------------------------

fn f32x4(l: [f32; 4]) -> u128 {
    l.iter().enumerate().fold(0, |a, (i, v)| a | (v.to_bits() as u128) << (32 * i))
}

#[test]
fn scalar_sse_keeps_the_upper_lanes_and_packed_computes_all_four() {
    // addss xmm0, xmm1 (f3 0f 58 c1); mulps xmm2, xmm3 (0f 59 d3)
    let (c, _) = run(&[0xF3, 0x0F, 0x58, 0xC1, 0x0F, 0x59, 0xD3], |c, _| {
        c.xmm[0] = f32x4([1.5, 7.0, 8.0, 9.0]);
        c.xmm[1] = f32x4([2.25, -1.0, -1.0, -1.0]);
        c.xmm[2] = f32x4([1.0, 2.0, 3.0, 4.0]);
        c.xmm[3] = f32x4([0.5, 0.5, -2.0, 0.0]);
    });
    assert_eq!(c.xmm[0], f32x4([3.75, 7.0, 8.0, 9.0]));
    assert_eq!(c.xmm[2], f32x4([0.5, 1.0, -6.0, 0.0]));
}

#[test]
fn sse_nans_follow_the_x86_rules() {
    // addss xmm0, xmm1 with an SNaN in xmm1: quieted. subss xmm2, xmm3 with
    // QNaNs in both: the first operand's wins. divss xmm4, xmm5, 0/0: the
    // default NaN 0xffc00000.
    let code = [0xF3, 0x0F, 0x58, 0xC1, 0xF3, 0x0F, 0x5C, 0xD3, 0xF3, 0x0F, 0x5E, 0xE5];
    let (c, _) = run(&code, |c, _| {
        c.xmm[1] = 0x7f80_0001;
        c.xmm[2] = 0x7fc0_1234;
        c.xmm[3] = 0xffc0_5678;
    });
    assert_eq!(c.xmm[0] as u32, 0x7fc0_0001);
    assert_eq!(c.xmm[2] as u32, 0x7fc0_1234);
    assert_eq!(c.xmm[4] as u32, 0xffc0_0000);
}

#[test]
fn comiss_sets_zf_pf_cf_and_clears_the_rest() {
    // comiss xmm0, xmm1 (0f 2f c1)
    let cmp = |a: f32, b: u32| {
        run(&[0x0F, 0x2F, 0xC1], |c, _| {
            c.xmm[0] = a.to_bits() as u128;
            c.xmm[1] = b as u128;
            set_flags(c, &[(Of, true), (Sf, true), (Af, true)]);
        })
        .0
    };
    for (a, b, zf, pf, cf) in [
        (1.0, 2.0f32.to_bits(), false, false, true),
        (2.0, 2.0f32.to_bits(), true, false, false),
        (3.0, 2.0f32.to_bits(), false, false, false),
        (1.0, 0x7fc0_0000, true, true, true), // unordered
    ] {
        let c = cmp(a, b);
        flags(&c, &[(Zf, zf), (Pf, pf), (Cf, cf), (Of, false), (Sf, false), (Af, false)]);
    }
}

#[test]
fn conversions_truncate_or_round_and_overflow_to_the_indefinite_integer() {
    // cvttss2si eax, xmm0 (f3 0f 2c c0); cvtss2si ecx, xmm1 (f3 0f 2d c9);
    // cvtss2si edx, xmm2 (f3 0f 2d d2); cvtsi2ss xmm3, ebx (f3 0f 2a db)
    let code = [0xF3, 0x0F, 0x2C, 0xC0, 0xF3, 0x0F, 0x2D, 0xC9, 0xF3, 0x0F, 0x2D, 0xD2, 0xF3, 0x0F, 0x2A, 0xDB];
    let (c, _) = run(&code, |c, _| {
        c.xmm[0] = (-2.75f32).to_bits() as u128;
        c.xmm[1] = 2.5f32.to_bits() as u128; // ties to even
        c.xmm[2] = 3e9f32.to_bits() as u128;
        c.xmm[3] = f32x4([0.0, 5.0, 6.0, 7.0]);
        c.set_reg(Ebx, -7i32 as u32);
    });
    assert_eq!(c.reg(Eax), -2i32 as u32);
    assert_eq!(c.reg(Ecx), 2);
    assert_eq!(c.reg(Edx), 0x8000_0000);
    assert_eq!(c.xmm[3], f32x4([-7.0, 5.0, 6.0, 7.0]));
}

#[test]
fn an_aligned_packed_operand_must_be_aligned() {
    // addps xmm0, [0x4004] (0f 58 05 04 40 00 00): #GP, xmm0 unchanged.
    let (mut c, mut m) = (cpu(), mem());
    c.xmm[0] = 5;
    let e = run_with(&[0x0F, 0x58, 0x05, 0x04, 0x40, 0x00, 0x00], &mut c, &mut m).unwrap_err();
    assert_eq!(e, Exit::Fault { pc: CODE, fault: Fault::GeneralProtection, next: None });
    assert_eq!(c.xmm[0], 5);
}

#[test]
fn mmx_aliases_the_x87_registers_and_makes_the_transition() {
    // movq mm1, [0x4000] (0f 6f 0d 00 40 00 00); paddusb mm1, mm2 (0f dc ca)
    let code = [0x0F, 0x6F, 0x0D, 0x00, 0x40, 0x00, 0x00, 0x0F, 0xDC, 0xCA];
    let (c, _) = run(&code, |c, m| {
        m.load(DATA, &0x01ff_8070_0000_0010u64.to_le_bytes());
        c.x87.st[2][..8].copy_from_slice(&0x0101_0190_7f00_00f0u64.to_le_bytes());
        c.x87.status = 0x3800 | 0x41; // TOP = 7
        c.x87.tag = 0xffff;
    });
    // Unsigned saturation per byte: 0x10 + 0xf0 = 0xff (saturated),
    // 0x70 + 0x90 = 0xff, 0x80 + 0x01 = 0x81, 0xff + 0x01 = 0xff.
    let mm1 = u64::from_le_bytes(c.x87.st[1][..8].try_into().unwrap());
    assert_eq!(mm1, 0x02ff_81ff_7f00_00ff);
    assert_eq!(c.x87.st[1][8..], [0xff, 0xff], "the exponent of a written MMX register is all ones");
    assert_eq!((c.x87.status, c.x87.tag), (0x41, 0), "TOP = 0, all tags valid, other status bits kept");
    // emms (0f 77): all tags empty.
    let (c, _) = run(&[0x0F, 0x77], |c, _| c.x87.tag = 0);
    assert_eq!(c.x87.tag, 0xffff);
}

#[test]
fn cvtpi2ps_from_memory_still_makes_the_mmx_transition() {
    // cvtpi2ps xmm0, [0x4000] (0f 2a 05 00 40 00 00)
    let (c, _) = run(&[0x0F, 0x2A, 0x05, 0x00, 0x40, 0x00, 0x00], |c, m| {
        m.load(DATA, &[3, 0, 0, 0, 0xfe, 0xff, 0xff, 0xff]);
        c.xmm[0] = f32x4([0.0, 0.0, 9.0, 10.0]);
        c.x87.tag = 0xffff;
        c.x87.status = 0x2000;
    });
    assert_eq!(c.xmm[0], f32x4([3.0, -2.0, 9.0, 10.0]));
    assert_eq!((c.x87.status, c.x87.tag), (0, 0));
    assert!(c.x87.st.iter().all(|r| r[8..] == [0, 0]), "no MMX register written");
}

#[test]
fn mmx_shift_counts_of_the_lane_width_or_more_clear_or_fill() {
    // psrlw mm0, mm1 (0f d1 c1) with a count of 2^32 (the count is 64-bit,
    // so not 0); psraw mm2, 99 (0f 71 e2 63); psllq mm3, 4 (0f 73 f3 04)
    let code = [0x0F, 0xD1, 0xC1, 0x0F, 0x71, 0xE2, 0x63, 0x0F, 0x73, 0xF3, 0x04];
    let (c, _) = run(&code, |c, _| {
        c.x87.st[0][..8].copy_from_slice(&u64::MAX.to_le_bytes());
        c.x87.st[1][..8].copy_from_slice(&(1u64 << 32).to_le_bytes());
        c.x87.st[2][..8].copy_from_slice(&0x8000_7fff_ffff_0001u64.to_le_bytes());
        c.x87.st[3][..8].copy_from_slice(&0xf000_0000_0000_0001u64.to_le_bytes());
    });
    let mm = |i: usize| u64::from_le_bytes(c.x87.st[i][..8].try_into().unwrap());
    assert_eq!(mm(0), 0);
    assert_eq!(mm(2), 0xffff_0000_ffff_0000);
    assert_eq!(mm(3), 0x10);
}

// ---- processor model -------------------------------------------------------

#[test]
fn rdtsc_splits_the_counter_into_edx_eax() {
    // rdtsc (0f 31) twice: the interpreter's counter returns and counts up.
    // EDX:EAX is the whole 64 bits; flags and other registers are kept.
    let (c, _) = run(&[0x0F, 0x31, 0x89, 0xC1, 0x0F, 0x31], |c, _| {
        c.tsc = 0x1234_5678_FFFF_FFFF;
        c.set_flag(Cf, true);
    });
    // mov ecx, eax (89 c1) kept the first low half.
    assert_eq!(c.reg(Ecx), 0xFFFF_FFFF);
    assert_eq!((c.reg(Edx), c.reg(Eax)), (0x1234_5679, 0));
    assert_eq!(c.tsc, 0x1234_5679_0000_0001);
    flags(&c, &[(Cf, true)]);
}

#[test]
fn cpuid_describes_the_xbox_pentium_iii() {
    // cpuid (0f a2) for each leaf in EAX; ECX is ignored.
    let leaf = |n: u32| {
        let (c, _) = run(&[0x0F, 0xA2], |c, _| {
            c.set_reg(Eax, n);
            c.set_reg(Ecx, 0xDEAD);
        });
        [c.reg(Eax), c.reg(Ebx), c.reg(Ecx), c.reg(Edx)]
    };
    let vendor = leaf(0);
    assert_eq!(vendor[0], 2, "highest basic leaf");
    let name: Vec<u8> = [vendor[1], vendor[3], vendor[2]].iter().flat_map(|r| r.to_le_bytes()).collect();
    assert_eq!(name, b"GenuineIntel");
    // What 0x34881d unpacks: stepping, model, family (bits 0-3, 4-7, 8-11),
    // then MMX, SSE and SSE2 (EDX bits 23, 25, 26).
    let [sig, _, _, features] = leaf(1);
    assert_eq!((sig & 0xF, sig >> 4 & 0xF, sig >> 8 & 0xF), (10, 8, 6));
    assert_eq!((features >> 23 & 1, features >> 25 & 1, features >> 26 & 1), (1, 1, 0));
    // TSC and CMOV, and the CX8 the kernel's interlocked helpers assume.
    assert_eq!((features >> 4 & 1, features >> 15 & 1, features >> 8 & 1), (1, 1, 1));
    assert_eq!(leaf(2), lift::lower::XBOX_CPUID[2]);
    // Leaf 2 decoded with the descriptors from Intel's SDM (vol. 2A, CPUID
    // table 3-12): one round (AL = 1), every register valid (bit 31 clear).
    let regs = leaf(2);
    assert_eq!(regs[0] & 0xFF, 1, "one round");
    assert!(regs.iter().all(|r| r >> 31 == 0));
    let mut caches: Vec<&str> = regs
        .iter()
        .flat_map(|r| r.to_le_bytes())
        .skip(1)
        .filter(|d| *d != 0)
        .map(|d| match d {
            0x01 => "ITLB 4K 4-way 32",
            0x02 => "ITLB 4M full 2",
            0x03 => "DTLB 4K 4-way 64",
            0x04 => "DTLB 4M 4-way 8",
            0x08 => "L1I 16K 4-way 32B",
            0x0C => "L1D 16K 4-way 32B",
            0x41 => "L2 128K 4-way 32B",
            0x42 => "L2 256K 4-way 32B",
            0x79 => "L2 128K 8-way 64B sectored",
            0x82 => "L2 256K 8-way 32B",
            d => panic!("descriptor {d:#04x} is not a P6 one"),
        })
        .collect();
    caches.sort_unstable();
    assert_eq!(
        caches,
        ["DTLB 4K 4-way 64", "DTLB 4M 4-way 8", "ITLB 4K 4-way 32", "ITLB 4M full 2", "L1D 16K 4-way 32B", "L1I 16K 4-way 32B", "L2 128K 4-way 32B"],
        "the Xbox's 128 KB L2 with 32-byte lines; 4-way is the nearest descriptor to its 8-way"
    );
    // Past the highest leaf, extended leaves included: leaf 2 again.
    assert_eq!(leaf(3), leaf(2));
    assert_eq!(leaf(0x8000_0000), leaf(2));
    assert!(leaf(0x8000_0000)[0] < 0x8000_0000, "no extended leaves");
}

#[test]
fn cli_sti_and_wbinvd_change_nothing() {
    // cli (fa), sti (fb), wbinvd (0f 09).
    let before = cpu();
    let (mut c, _) = run(&[0xFA, 0xFB, 0x0F, 0x09], |_, _| {});
    c.eip = before.eip;
    assert_eq!(c, Cpu { eip: c.eip, ..before });
}
