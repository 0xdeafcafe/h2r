//! The runtime ABI of docs/lifter/FLOAT-ABI.md §1-§4 over the runtime's `X87Abi`
//! (h2abi.h v1): C layout, the stack model (TOP in the status word, tags,
//! stack faults), the helper return statuses, the #MF-at-next-instruction
//! contract, two-phase stores and environment operations, and control-word
//! normalisation. Expectations follow the SDM's x87 state description
//! (Vol. 1 §8.1, §8.6 "x87 FPU exception synchronization") and each
//! instruction's flag table.
#![cfg(feature = "abi")]

use core::mem::{align_of, offset_of, size_of};
use x87::F80;
use x87::abi::ffi::*;
use x87::abi::*;

const J: u64 = 1 << 63;
const ONE: F80 = F80::new(0x3fff, J);
const TWO: F80 = F80::new(0x4000, J);
const IND: F80 = F80::new(0xffff, 0xc000_0000_0000_0000);
const IE: u16 = H2_FP_IE;
const SF: u16 = H2_FP_SF;
const CC: u16 = SW_C3 | SW_C2 | SW_C1 | SW_C0;

fn fresh() -> X87Abi {
    X87Abi::default()
}

/// The control word with every exception masked except `unmask`.
fn cw_unmasking(unmask: u16) -> u16 {
    0x037f & !unmask
}

/// The three phases of a store as generated code runs them, with a guest
/// write that always succeeds.
fn store<T: Copy>(
    s: &mut X87Abi,
    pop: bool,
    phase1: impl FnOnce(&X87Abi) -> (u32, Store<T>),
    dest: &mut T,
) -> u32 {
    let (r, o) = phase1(s);
    match r {
        status::OK => {
            *dest = o.value;
            store_commit(s, o.status, pop);
        }
        status::SUPPRESS => store_commit(s, o.status, false),
        _ => {}
    }
    r
}

#[test]
fn c_layout_matches_h2abi() {
    // h2abi.h v1: _Static_assert(sizeof(X87Abi) == 100), align 4, and the
    // field offsets below (crates/runtime/src/cpu.rs is the same struct).
    assert_eq!(size_of::<X87Abi>(), 100);
    assert_eq!(align_of::<X87Abi>(), 4);
    assert_eq!(offset_of!(X87Abi, st), 0);
    assert_eq!(offset_of!(X87Abi, control), 80);
    assert_eq!(offset_of!(X87Abi, status), 82);
    assert_eq!(offset_of!(X87Abi, tag), 84);
    assert_eq!(offset_of!(X87Abi, opcode), 86);
    assert_eq!(offset_of!(X87Abi, instruction_pointer), 88);
    assert_eq!(offset_of!(X87Abi, data_pointer), 92);
    assert_eq!(offset_of!(X87Abi, instruction_selector), 96);
    assert_eq!(offset_of!(X87Abi, data_selector), 98);
    // Registers are guest-memory byte images: significand LE, then se LE.
    let mut s = fresh();
    ld_const(&mut s, 2);
    assert_eq!(
        s.st[7],
        [0x35, 0xc2, 0x68, 0x21, 0xa2, 0xda, 0x0f, 0xc9, 0x00, 0x40]
    );
}

#[test]
fn init_state() {
    // FNINIT: CW 0x037f, SW 0 (so TOP 0), all tags empty.
    let s = fresh();
    assert_eq!(
        (s.control, s.status, s.tag, s.top()),
        (0x037f, 0, 0xffff, 0)
    );
}

#[test]
fn top_lives_in_the_status_word() {
    let mut s = fresh();
    // MMX entry (h2gen.h) clears TOP in the status word; the helpers see it.
    s.status = 5 << 11;
    assert_eq!(s.top(), 5);
    ld_const(&mut s, 0);
    assert_eq!((s.status >> 11) & 7, 4);
    assert_eq!(s.phys(0), 4);
    assert_eq!(s.st[4], ONE.to_bytes());
    // Writing other status bits never disturbs TOP.
    clex(&mut s);
    assert_eq!(s.top(), 4);
}

#[test]
fn push_pop_wraparound_and_tags() {
    let mut s = fresh();
    // Push 8 values: TOP goes 0 -> 7 -> ... -> 0, filling every register.
    let vals = [
        ONE,
        F80::ZERO,
        F80::INFINITY,
        TWO,
        F80::new(0, 1),
        IND,
        F80::PI,
        F80::LN2,
    ];
    for (n, v) in vals.iter().enumerate() {
        assert_eq!(ld_f80(&mut s, v.to_bytes()), status::OK);
        assert_eq!(s.top() as usize, (8 - (n + 1)) & 7);
    }
    assert_eq!(s.top(), 0);
    // Tags: valid 00, zero 01, special 10 (inf, denormal, NaN), empty 11.
    let expect = |v: F80| match v.classify() {
        x87::Class::Zero => TAG_ZERO,
        x87::Class::Normal => TAG_VALID,
        _ => TAG_SPECIAL,
    };
    for (n, v) in vals.iter().enumerate() {
        let p = (8 - (n + 1)) & 7;
        assert_eq!(s.tag_of(p), expect(*v), "reg {p}");
        assert_eq!(s.reg(p), *v);
    }
    assert_eq!(s.st(0), Some(F80::LN2));
    assert_eq!(s.st(7), Some(ONE));
    // Pop all eight with FSTP m80.
    for n in 0..8 {
        let mut out = [0u8; 10];
        assert_eq!(store(&mut s, true, |s| st_f80(s, 0), &mut out), 0);
        assert_eq!(F80::from_bytes(out), vals[7 - n]);
    }
    assert_eq!((s.top(), s.tag), (0, 0xffff));
    // FDECSTP/FINCSTP move TOP without touching tags.
    adj_top(&mut s, -1);
    assert_eq!((s.top(), s.tag), (7, 0xffff));
    adj_top(&mut s, 2);
    assert_eq!(s.top(), 1);
    // FFREE marks a register empty without moving TOP.
    let mut s = fresh();
    ld_f80(&mut s, ONE.to_bytes());
    ld_f80(&mut s, TWO.to_bytes());
    assert_eq!(free(&mut s, 1, false), 0);
    assert_eq!((s.top(), s.st(0), s.st(1)), (6, Some(TWO), None));
}

#[test]
fn stack_overflow_masked() {
    // SDM Vol. 1 §8.5.1.1: pushing onto a full stack is #IS with C1 = 1;
    // masked, the indefinite is loaded.
    let mut s = fresh();
    for _ in 0..8 {
        ld_const(&mut s, 0);
    }
    let top = s.top();
    assert_eq!(ld_const(&mut s, 2), status::OK);
    assert_eq!(s.status & (IE | SF | SW_C1), IE | SF | SW_C1);
    assert_eq!(s.status & SW_ES, 0);
    assert_eq!(s.top(), (top + 7) & 7);
    assert_eq!(s.st(0), Some(IND));
    assert_eq!(s.st[s.phys(0)], [0, 0, 0, 0, 0, 0, 0, 0xc0, 0xff, 0xff]);
    assert_eq!(s.tag_of(s.phys(0)), TAG_SPECIAL);
}

#[test]
fn stack_underflow_masked() {
    // Reading an empty register is #IS with C1 = 0; masked, the
    // destination gets the indefinite.
    let mut s = fresh();
    s.status |= SW_C1;
    let mut out = 0u32;
    assert_eq!(store(&mut s, false, |s| st_f32(s, 0), &mut out), 0);
    assert_eq!(out, 0xffc0_0000);
    assert_eq!(s.status & (IE | SF | SW_C1), IE | SF);
    // Arithmetic with an empty source: ST(0) becomes the indefinite.
    let mut s = fresh();
    ld_const(&mut s, 0);
    assert_eq!(arith(&mut s, arith_op::ADD, 0, src_kind::ST, 1), 0);
    assert_eq!(s.st(0), Some(IND));
    assert_eq!(s.status & (IE | SF), IE | SF);
    // FCOM against an empty register: unordered.
    let mut s = fresh();
    ld_const(&mut s, 0);
    let r = cmp(&mut s, 0, src_kind::ST, 3, 0, false);
    assert_eq!(r, (SW_C3 | SW_C2 | SW_C0) as u32);
    assert_eq!(s.status & CC, SW_C3 | SW_C2 | SW_C0);
}

#[test]
fn unmasked_stack_fault_records_and_faults_at_next_instruction() {
    // Stack overflow with IE unmasked: the instruction completes (OK) with
    // only the status word changed: IE, SF, C1, ES and B.
    let mut s = fresh();
    ldcw(&mut s, cw_unmasking(IE));
    for _ in 0..8 {
        assert_eq!(ld_const(&mut s, 0), 0);
    }
    let before = s;
    assert_eq!(ld_const(&mut s, 2), status::OK);
    assert_eq!(
        s.status & (IE | SF | SW_C1 | SW_ES | SW_B),
        IE | SF | SW_C1 | SW_ES | SW_B
    );
    assert_eq!(
        (s.top(), s.tag, s.st),
        (before.top(), before.tag, before.st)
    );
    // The NEXT waiting instruction reports #MF (event 14) and does
    // nothing; arg is the status word.
    let pending = s;
    assert_eq!(ld_const(&mut s, 0), status::MF);
    assert_eq!(arith(&mut s, arith_op::ADD, 0, src_kind::ST, 1), status::MF);
    assert_eq!(wait(&s), status::MF);
    assert_eq!(st_f32(&s, 0).0, status::MF);
    assert_eq!(ldcw(&mut s, 0x037f), status::MF);
    assert_eq!(s, pending);
    assert_eq!(H2SHIM_FP_ERROR, 14);
    // Non-waiting forms still run: FNSTSW reads the word, FNSTENV images it.
    assert_eq!(
        save_image(&s, env_op::FNSTENV)[4..6],
        s.status.to_le_bytes()
    );
    // FNCLEX clears it; then everything runs again.
    clex(&mut s);
    assert_eq!(s.status & 0x80ff, 0);
    assert_eq!(wait(&s), status::OK);
}

#[test]
fn unmasked_zero_divide_does_not_commit() {
    let mut s = fresh();
    ldcw(&mut s, cw_unmasking(H2_FP_ZE));
    ld_const(&mut s, 0); // 1
    ld_const(&mut s, 1); // 0
    let before = s;
    // fdivp st(1), st: ST(1) = 1 / 0.
    let r = arith(&mut s, arith_op::DIV | ARITH_POP, 1, src_kind::ST, 0);
    assert_eq!(r, status::OK);
    assert_eq!(
        (s.top(), s.tag, s.st),
        (before.top(), before.tag, before.st)
    );
    assert_eq!(
        s.status & (H2_FP_ZE | SW_ES | SW_B),
        H2_FP_ZE | SW_ES | SW_B
    );
    assert_eq!(ld_const(&mut s, 0), status::MF);
}

#[test]
fn unmasked_post_computation_exceptions_are_unmodelled() {
    // PE/OE/UE unmasked: hardware stores a rounded or bias-adjusted result
    // before faulting. Not modelled: the helper changes nothing and asks
    // for stop 12.
    let mut s = fresh();
    ldcw(&mut s, cw_unmasking(H2_FP_PE));
    ld_const(&mut s, 2); // pi: exact load, fine
    let before = s;
    assert_eq!(st_f32(&s, 0).0, status::UNMODELLED);
    assert_eq!(
        arith(&mut s, arith_op::DIV, 0, src_kind::I32, 3),
        status::UNMODELLED
    );
    assert_eq!(s, before);
    assert_eq!(H2SHIM_UNMODELLED, 12);
    // Overflow unmasked.
    let mut s = fresh();
    ldcw(&mut s, cw_unmasking(H2_FP_OE));
    ld_f80(&mut s, F80::new(0x7ffe, J).to_bytes());
    let before = s;
    assert_eq!(
        arith(&mut s, arith_op::ADD, 0, src_kind::ST, 0),
        status::UNMODELLED
    );
    assert_eq!(s, before);
    // A stack fault with IE unmasked is a pre-computation fault: SUPPRESS.
    let mut s = fresh();
    ldcw(&mut s, cw_unmasking(IE));
    assert_eq!(st_f32(&s, 0).0, status::SUPPRESS);
}

#[test]
fn two_phase_store_changes_nothing_before_commit() {
    let mut s = fresh();
    ld_const(&mut s, 2); // pi
    let before = s;
    let (r, o) = st_f32(&s, 0);
    assert_eq!(r, status::OK);
    assert_eq!(o.value, 0x4049_0fdb);
    // pi rounds up to binary32: PE and C1 in the status to commit.
    assert_eq!(o.status & (H2_FP_PE | SW_C1), H2_FP_PE | SW_C1);
    // A faulting guest write: nothing is committed, state untouched.
    assert_eq!(s, before);
    // A successful one: commit with the pop.
    store_commit(&mut s, o.status, true);
    assert_eq!((s.top(), s.tag), (0, 0xffff));
    assert_eq!(s.status & (H2_FP_PE | SW_C1), H2_FP_PE | SW_C1);

    // Unmasked invalid on FISTP: SUPPRESS, destination not written, no
    // pop, ES pending.
    let mut s = fresh();
    ldcw(&mut s, cw_unmasking(IE));
    ld_f64(&mut s, 3e10f64.to_bits());
    let mut out = 0x1234_5678i32;
    assert_eq!(
        store(&mut s, true, |s| st_i32(s, 0), &mut out),
        status::SUPPRESS
    );
    assert_eq!((out, s.top()), (0x1234_5678, 7));
    assert_eq!(s.status & (IE | SW_ES | SW_B), IE | SW_ES | SW_B);
}

#[test]
fn control_word_normalisation() {
    // The CRT translator writes 0x023F (bit 6 clear); a P6 reads back 0x027F.
    let mut s = fresh();
    ldcw(&mut s, 0x023f);
    assert_eq!(s.control, 0x027f);
    // Reserved bits 13-15 read as zero.
    ldcw(&mut s, 0xe37f);
    assert_eq!(s.control, 0x037f);
    assert_eq!(normalise_cw(0), 0x0040);
    // FLDENV normalises the same way.
    let mut img = save_image(&s, env_op::FNSTENV);
    img[0..2].copy_from_slice(&0xf23fu16.to_le_bytes());
    load_env(&mut s, env_op::FLDENV, &img);
    assert_eq!(s.control, 0x127f);
}

#[test]
fn ldcw_unmasking_a_pending_flag_sets_es() {
    let mut s = fresh();
    ld_const(&mut s, 2);
    let mut out = 0u32;
    store(&mut s, false, |s| st_f32(s, 0), &mut out); // PE, masked
    assert_eq!(s.status & (H2_FP_PE | SW_ES), H2_FP_PE);
    ldcw(&mut s, cw_unmasking(H2_FP_PE));
    assert_eq!(s.status & (SW_ES | SW_B), SW_ES | SW_B);
    // FLDCW is a waiting instruction: with ES set it raises #MF first, so
    // the handler's FNCLEX must come before re-masking.
    assert_eq!(ldcw(&mut s, 0x037f), status::MF);
    clex(&mut s);
    assert_eq!(ldcw(&mut s, 0x037f), status::OK);
    assert_eq!(s.status & (SW_ES | SW_B), 0);
}

#[test]
fn arithmetic_and_condition_codes() {
    // fld1; fld1; faddp st(1), st -> 2 with one register in use.
    let mut s = fresh();
    ld_const(&mut s, 0);
    ld_const(&mut s, 0);
    assert_eq!(
        arith(&mut s, arith_op::ADD | ARITH_POP, 1, src_kind::ST, 0),
        0
    );
    assert_eq!((s.top(), s.st(0), s.st(1)), (7, Some(TWO), None));
    // fsubr m32 1.0: ST(0) = 1 - 2 = -1.
    assert_eq!(
        arith(&mut s, arith_op::SUBR, 0, src_kind::F32, 0x3f80_0000),
        0
    );
    assert_eq!(s.st(0), Some(F80::new(0xbfff, J)));
    // fidiv m16 -4: -1 / -4 = 0.25.
    assert_eq!(arith(&mut s, arith_op::DIV, 0, src_kind::I16, 0xfffc), 0);
    assert_eq!(s.st(0), Some(F80::new(0x3ffd, J)));
    // A binary32 denormal memory operand sets DE.
    s.status &= SW_TOP;
    assert_eq!(arith(&mut s, arith_op::MUL, 0, src_kind::F32, 1), 0);
    assert_eq!(s.status & 0x3f, H2_FP_DE);
    // PC=53 from the control word: 1 + 2^-63 - 1 is still exact.
    let mut s = fresh();
    ldcw(&mut s, 0x027f);
    ld_f80(&mut s, F80::new(0x3fff, J | 1).to_bytes());
    arith(&mut s, arith_op::SUB, 0, src_kind::F32, 0x3f80_0000);
    assert_eq!(s.st(0), Some(F80::new(0x3fc0, J)));
    // C1 reports a round-up: 2/3 at RN rounds up.
    let mut s = fresh();
    ld_const(&mut s, 0);
    arith(&mut s, arith_op::ADD, 0, src_kind::F32, 0x3f80_0000);
    arith(&mut s, arith_op::DIV, 0, src_kind::I32, 3);
    assert_eq!(s.status & (H2_FP_PE | SW_C1), H2_FP_PE | SW_C1);
}

#[test]
fn compare_payloads() {
    let mut s = fresh();
    ld_const(&mut s, 0); // 1
    // fcom m32 2.0: less -> C0, returned and in the status word.
    let r = cmp(&mut s, 0, src_kind::F32, 0x4000_0000, 0, false);
    assert_eq!(r, SW_C0 as u32);
    assert_eq!(s.status & CC, SW_C0);
    // ftst = fcom against F32 +0.0: 1 > 0 -> nothing.
    assert_eq!(cmp(&mut s, 0, src_kind::F32, 0, 0, false), 0);
    // fcomi st, st(0): equal -> ZF in the payload; C1 cleared only.
    s.status |= SW_C1 | SW_C0;
    let r = cmp(&mut s, 0, src_kind::ST, 0, 0, true);
    assert_eq!(r, EFL_ZF);
    assert_eq!(s.status & CC, SW_C0);
    // fucomip with a QNaN: unordered, no IE; one pop.
    ld_f80(&mut s, F80::new(0x7fff, 0xc000_0000_0000_0001).to_bytes());
    s.status &= SW_TOP;
    let r = cmp(&mut s, 1, src_kind::ST, 1, 1, true);
    assert_eq!(r, EFL_ZF | EFL_PF | EFL_CF);
    assert_eq!(s.status & IE, 0);
    assert_eq!(s.top(), 7);
    // fcompp (0 against 1): less, then two pops empty the stack.
    ld_const(&mut s, 1);
    let r = cmp(&mut s, 0, src_kind::ST, 1, 2, false);
    assert_eq!(r, SW_C0 as u32);
    assert_eq!((s.top(), s.tag), (0, 0xffff));
    // ficom m16 against 1: equal.
    ld_const(&mut s, 0);
    assert_eq!(cmp(&mut s, 2, src_kind::I16, 1, 0, false), SW_C3 as u32);
    // fcomi with an unmasked invalid (QNaN): SUPPRESS with payload 0, no
    // pop, ES.
    let mut s = fresh();
    ldcw(&mut s, cw_unmasking(IE));
    ld_f80(&mut s, F80::new(0x7fff, 0xc000_0000_0000_0001).to_bytes());
    assert_eq!(
        cmp(&mut s, 0, src_kind::F32, 0x3f80_0000, 1, true),
        status::SUPPRESS
    );
    assert_eq!(s.top(), 7);
    assert!(s.pending());
}

#[test]
fn xam_classes() {
    let cases: [(Option<F80>, u16); 7] = [
        (None, SW_C3 | SW_C0),
        (Some(F80::new(0x7fff, 0x4000_0000_0000_0000)), 0),
        (Some(F80::new(0x7fff, 0xc000_0000_0000_0000)), SW_C0),
        (Some(ONE), SW_C2),
        (Some(F80::INFINITY), SW_C2 | SW_C0),
        (Some(F80::ZERO), SW_C3),
        (Some(F80::new(0x8000, 1)), SW_C3 | SW_C2 | SW_C1),
    ];
    for (v, want) in cases {
        let mut s = fresh();
        if let Some(v) = v {
            ld_f80(&mut s, v.to_bytes());
        }
        xam(&mut s);
        assert_eq!(s.status & CC, want, "{v:x?}");
    }
}

#[test]
fn unary_ops_through_the_stack() {
    // fptan pushes 1.0; fsincos leaves cos in ST(0), sin in ST(1).
    let mut s = fresh();
    ld_const(&mut s, 1);
    assert_eq!(unary(&mut s, unary_op::TAN), 0);
    assert_eq!((s.st(0), s.st(1), s.top()), (Some(ONE), Some(F80::ZERO), 6));
    let mut s = fresh();
    ld_const(&mut s, 1);
    unary(&mut s, unary_op::SINCOS);
    assert_eq!((s.st(0), s.st(1)), (Some(ONE), Some(F80::ZERO)));
    // Out of range: C2 set, operand unchanged, nothing pushed.
    let mut s = fresh();
    let big = F80::new(0x403e, J);
    ld_f80(&mut s, big.to_bytes());
    unary(&mut s, unary_op::SIN);
    assert_eq!((s.st(0), s.status & SW_C2, s.top()), (Some(big), SW_C2, 7));
    unary(&mut s, unary_op::TAN);
    assert_eq!((s.st(0), s.top()), (Some(big), 7));
    // fpatan: atan2(1, 1) = pi/4, pop.
    let mut s = fresh();
    ld_const(&mut s, 0);
    ld_const(&mut s, 0);
    unary(&mut s, unary_op::PATAN);
    assert_eq!(
        (s.st(0), s.top()),
        (Some(F80::new(0x3ffe, 0xc90f_daa2_2168_c235)), 7)
    );
    // fxtract: ST(0) = significand, ST(1) = exponent.
    let mut s = fresh();
    ld_int(&mut s, 12);
    unary(&mut s, unary_op::XTRACT);
    assert_eq!(
        (s.st(0), s.st(1)),
        (
            Some(F80::new(0x3fff, 0xc000_0000_0000_0000)),
            Some(F80::new(0x4000, 0xc000_0000_0000_0000))
        )
    );
    // fprem: 5.5 rem 2 = 1.5, Q = 2: C0 = Q2 = 0, C3 = Q1 = 1, C1 = Q0 = 0.
    let mut s = fresh();
    ld_int(&mut s, 2);
    ld_f80(&mut s, F80::new(0x4001, 0xb000_0000_0000_0000).to_bytes());
    unary(&mut s, unary_op::PREM);
    assert_eq!(s.st(0), Some(F80::new(0x3fff, 0xc000_0000_0000_0000)));
    assert_eq!(s.status & CC, SW_C3);
    // A large exponent difference sets C2 until the loop finishes.
    let mut s = fresh();
    ld_int(&mut s, 3);
    ld_f80(&mut s, F80::new(0x3fff + 100, J).to_bytes());
    let mut rounds = 0;
    loop {
        unary(&mut s, unary_op::PREM);
        rounds += 1;
        if s.status & SW_C2 == 0 {
            break;
        }
    }
    assert!(rounds >= 2);
    assert_eq!(s.st(0), Some(ONE));
    assert_eq!(s.status & (SW_C0 | SW_C1 | SW_C3), SW_C0 | SW_C1);
    // fsqrt of an empty stack: underflow.
    let mut s = fresh();
    unary(&mut s, unary_op::SQRT);
    assert_eq!(s.status & (IE | SF | SW_C1), IE | SF);
}

#[test]
fn stores_and_integer_indefinite() {
    let mut s = fresh();
    ld_f64(&mut s, 3e10f64.to_bits());
    let mut o32 = 0i32;
    assert_eq!(store(&mut s, false, |s| st_i32(s, 0), &mut o32), 0);
    assert_eq!(o32, i32::MIN);
    assert!(s.status & IE != 0);
    let mut o64 = 0i64;
    store(&mut s, false, |s| st_i64(s, 0), &mut o64);
    assert_eq!(o64, 30_000_000_000);
    // RC=up vs truncation of 2.5 (the tick-budget/script-sleep fistp sites
    // make RC observable).
    let mut s = fresh();
    ldcw(&mut s, 0x0b7f);
    ld_f64(&mut s, 2.5f64.to_bits());
    store(&mut s, false, |s| st_i32(s, 0), &mut o32);
    assert_eq!(o32, 3);
    store(&mut s, false, |s| st_i32_trunc(s, 0), &mut o32);
    assert_eq!(o32, 2);
    let mut o16 = 0i16;
    store(&mut s, true, |s| st_i16(s, 0), &mut o16);
    assert_eq!((o16, s.top()), (3, 0));
    // FBSTP.
    let mut s = fresh();
    ld_int(&mut s, -1234);
    let mut b = [0u8; 10];
    store(&mut s, true, |s| st_bcd(s, 0), &mut b);
    assert_eq!(b, [0x34, 0x12, 0, 0, 0, 0, 0, 0, 0, 0x80]);
    // FLD m32 SNaN: IE and the quieted NaN.
    let mut s = fresh();
    ld_f32(&mut s, 0x7f80_0001);
    assert_eq!(s.st(0), Some(F80::new(0x7fff, 0xc000_0100_0000_0000)));
    assert_eq!(s.status & IE, IE);
    // FST ST(i) copies exactly; FSTP ST(0) is a pop.
    let mut s = fresh();
    ld_const(&mut s, 2);
    ld_const(&mut s, 0);
    st_st(&mut s, 1, false);
    assert_eq!(s.st(1), Some(ONE));
    st_st(&mut s, 0, true);
    assert_eq!((s.top(), s.st(0)), (7, Some(ONE)));
}

#[test]
fn ftol2_contract() {
    // _ftol2 (0x322b0c): int64 in EDX:EAX, truncation whatever RC, one pop.
    let cases: [(u64, u64, u16); 7] = [
        (2.75f64.to_bits(), 2, H2_FP_PE),
        ((-2.75f64).to_bits(), (-2i64) as u64, H2_FP_PE),
        (1e18f64.to_bits(), 1_000_000_000_000_000_000, 0),
        // |x| >= 2^63 and NaN: 0x8000000000000000 with IE.
        (9.3e18f64.to_bits(), 0x8000_0000_0000_0000, IE),
        ((-9.3e18f64).to_bits(), 0x8000_0000_0000_0000, IE),
        (0x7ff8_0000_0000_0000, 0x8000_0000_0000_0000, IE),
        // -2^63 itself is representable.
        (0xc3e0_0000_0000_0000, 0x8000_0000_0000_0000, 0),
    ];
    for (x, want, fl) in cases {
        let mut s = fresh();
        ldcw(&mut s, 0x027f); // PC=53, RN: RC must not matter
        ld_f64(&mut s, x);
        let mut out = 0u64;
        assert_eq!(ftol2(&mut s, &mut out), status::OK);
        assert_eq!(out, want, "{x:#x}");
        assert_eq!(s.status & 0x3f, fl, "{x:#x}");
        assert_eq!((s.top(), s.tag), (0, 0xffff));
    }
    // Empty stack: stack fault, indefinite.
    let mut s = fresh();
    let mut out = 0u64;
    ftol2(&mut s, &mut out);
    assert_eq!(out, 0x8000_0000_0000_0000);
    assert_eq!(s.status & (IE | SF), IE | SF);
}

#[test]
fn fxch_and_cmov() {
    let mut s = fresh();
    ld_const(&mut s, 0);
    ld_const(&mut s, 2);
    xch(&mut s, 1);
    assert_eq!((s.st(0), s.st(1)), (Some(ONE), Some(F80::PI)));
    // FXCH with an empty ST(2): underflow, masked -> indefinite swapped in.
    xch(&mut s, 2);
    assert_eq!((s.st(0), s.st(2)), (Some(IND), Some(ONE)));
    assert_eq!(s.status & (IE | SF), IE | SF);
    let mut s = fresh();
    ld_const(&mut s, 0);
    ld_const(&mut s, 2);
    cmov(&mut s, 1, false);
    assert_eq!(s.st(0), Some(F80::PI));
    cmov(&mut s, 1, true);
    assert_eq!(s.st(0), Some(ONE));
}

#[test]
fn env_image_then_commit() {
    let mut s = fresh();
    ldcw(&mut s, 0x027f);
    ld_const(&mut s, 2);
    ld_const(&mut s, 1);
    ld_const(&mut s, 0);
    s.instruction_pointer = 0x1234_5678;
    s.opcode = 0x7d9;
    let saved = s;
    // FNSAVE phase 1: the image, no state change.
    let img = save_image(&s, env_op::FNSAVE);
    assert_eq!(s, saved);
    assert_eq!(&img[0..2], &0x027fu16.to_le_bytes());
    assert_eq!(&img[4..6], &saved.status.to_le_bytes());
    assert_eq!(&img[12..16], &0x1234_5678u32.to_le_bytes());
    assert_eq!(&img[18..20], &0x07d9u16.to_le_bytes());
    assert_eq!(&img[28..38], &ONE.to_bytes()); // ST(0) first
    assert_eq!(&img[48..58], &F80::PI.to_bytes());
    // Phase 3 after a successful write: FNINIT.
    save_commit(&mut s, env_op::FNSAVE);
    assert_eq!((s.control, s.tag, s.top()), (0x037f, 0xffff, 0));
    // FRSTOR.
    assert_eq!(load_env(&mut s, env_op::FRSTOR, &img), 0);
    assert_eq!(s, saved);
    // FNSTENV: image, then mask all.
    ldcw(&mut s, 0x0200);
    let e = save_image(&s, env_op::FNSTENV);
    assert_eq!(&e[0..2], &0x0240u16.to_le_bytes());
    save_commit(&mut s, env_op::FNSTENV);
    assert_eq!(s.control, 0x027f);
    assert_eq!(env_len(env_op::FNSTENV), 28);
    assert_eq!(env_len(env_op::FRSTOR), 108);
}

#[test]
fn fldenv_recomputes_es_and_tags() {
    // An image with PE set and PE unmasked: loading it raises ES and B.
    let mut s = fresh();
    ld_const(&mut s, 1); // ST(0) = 0 in physical 7
    let mut img = save_image(&s, env_op::FNSTENV);
    img[0..2].copy_from_slice(&cw_unmasking(H2_FP_PE).to_le_bytes());
    img[4..6].copy_from_slice(&(H2_FP_PE | (7 << 11)).to_le_bytes());
    // Tag word claims "valid" for physical 7 (which holds +0): recomputed.
    img[8..10].copy_from_slice(&0x3fffu16.to_le_bytes());
    load_env(&mut s, env_op::FLDENV, &img);
    assert_eq!(s.status & (SW_ES | SW_B), SW_ES | SW_B);
    assert_eq!(s.tag_of(7), TAG_ZERO);
    assert_eq!(wait(&s), status::MF);
    // ... and an image with ES set but everything masked clears it.
    let mut img2 = img;
    img2[0..2].copy_from_slice(&0x037fu16.to_le_bytes());
    img2[4..6].copy_from_slice(&(H2_FP_PE | SW_ES | SW_B | (7 << 11)).to_le_bytes());
    clex(&mut s);
    load_env(&mut s, env_op::FLDENV, &img2);
    assert_eq!(s.status & (SW_ES | SW_B), 0);
}

#[test]
fn ffi_entry_points() {
    let mut s = fresh();
    let p: *mut X87Abi = &mut s;
    // SAFETY: `p` points at a live X87Abi and the buffers are large enough.
    unsafe {
        assert_eq!(h2x87_v1_ld_const(p, 2), 0);
        assert_eq!(h2x87_v1_ld_i32(p, 2), 0);
        assert_eq!(h2x87_v1_arith(p, arith_op::MUL | ARITH_POP, 1, 0, 0), 0);
        let mut out = [0u8; 10];
        let mut sw = 0u16;
        assert_eq!(h2x87_v1_st_f80(p, 0, &mut out, &mut sw), 0);
        assert_eq!(h2x87_v1_store_commit(p, sw, 0), 0);
        assert_eq!(
            F80::from_bytes(out),
            F80::new(0x4001, 0xc90f_daa2_2168_c235)
        );
        assert_eq!(h2x87_v1_cmp(p, 0, 0, 0, 1, 1), EFL_ZF);
        let mut img = [0u8; 108];
        assert_eq!(h2x87_v1_save_image(p, env_op::FNSAVE, img.as_mut_ptr()), 0);
        assert_eq!(h2x87_v1_save_commit(p, env_op::FNSAVE), 0);
        assert_eq!(h2x87_v1_load_env(p, env_op::FRSTOR, img.as_ptr()), 0);
        let mut v = 0u64;
        assert_eq!(h2x87_v1_ld_f64(p, 2.5f64.to_bits()), 0);
        assert_eq!(h2x87_v1_ftol2(p, &mut v), 0);
        assert_eq!(v, 2);
        assert_eq!(h2x87_v1_wait(p), 0);
        assert_eq!(h2x87_v1_init(p), 0);
    }
    assert_eq!(s.control, 0x037f);
}

#[test]
#[should_panic(expected = "bad load-constant selector")]
fn bad_selector_panics() {
    let mut s = fresh();
    ld_const(&mut s, 7);
}

// A null `X87Abi*` through the C ABI panics inside an `extern "C"` function,
// which aborts the process (it cannot unwind), so it is not testable with
// `should_panic`; the checked path is the same `expect` as above.

#[test]
fn transcendentals_under_directed_rc_are_unmodelled() {
    // The crate rounds transcendentals to nearest only. Under RC=up the
    // helper stops instead of returning a possibly wrong rounding; the
    // RC-exact ops (sqrt, rndint, scale, prem) still run.
    let mut s = fresh();
    ldcw(&mut s, 0x0b7f);
    ld_const(&mut s, 0);
    let before = s;
    for op in [
        unary_op::SIN,
        unary_op::COS,
        unary_op::TAN,
        unary_op::SINCOS,
        unary_op::X2M1,
    ] {
        assert_eq!(unary(&mut s, op), status::UNMODELLED, "op {op}");
    }
    assert_eq!(s, before);
    assert_eq!(unary(&mut s, unary_op::SQRT), status::OK);
    assert_eq!(unary(&mut s, unary_op::RNDINT), status::OK);
}

// ------------------------------------------------- re-review regressions
// Each of these failed before the fix it names (docs/lifter/FLOAT-ABI.md §2.3).

const DE: u16 = H2_FP_DE;
const ZE: u16 = H2_FP_ZE;
const UE: u16 = H2_FP_UE;
const PE: u16 = H2_FP_PE;
const MIN_NORMAL: F80 = F80::new(0x0001, J);

#[test]
fn unmasked_underflow_stops_on_an_exact_tiny_result() {
    // UM clear: UE is signalled on tininess alone (SDM Vol. 1 §4.9.1.5), so
    // 2^-16382 * 0.5 = 2^-16383, exact, still raises it. Hardware would
    // deliver the bias-adjusted result (§8.5.5): not modelled, so stop and
    // change nothing.
    let mut s = fresh();
    ldcw(&mut s, cw_unmasking(UE));
    ld_f80(&mut s, MIN_NORMAL.to_bytes());
    let before = s;
    let half = 0x3f00_0000;
    assert_eq!(
        arith(&mut s, arith_op::MUL, 0, src_kind::F32, half),
        status::UNMODELLED
    );
    assert_eq!(s, before);
    // Masked, the same exact denormal result raises nothing.
    ldcw(&mut s, 0x037f);
    assert_eq!(
        arith(&mut s, arith_op::MUL, 0, src_kind::F32, half),
        status::OK
    );
    assert_eq!(s.st(0), Some(F80::new(0, 0x4000_0000_0000_0000)));
    assert_eq!(s.status & 0x3f, 0);
}

#[test]
fn unmasked_underflow_stops_on_an_exact_tiny_store() {
    // 2^-149 is exactly the smallest binary32 denormal: tiny, not inexact.
    let mut s = fresh();
    ldcw(&mut s, cw_unmasking(UE));
    ld_f80(&mut s, F80::new(0x3fff - 149, J).to_bytes());
    assert_eq!(st_f32(&s, 0).0, status::UNMODELLED);
    // An exact store of a value normal in binary32 is fine.
    ld_const(&mut s, 0);
    assert_eq!(
        st_f32(&s, 0),
        (
            status::OK,
            Store {
                value: 0x3f80_0000,
                status: s.status
            }
        )
    );
    // Masked: exact, so no UE.
    let mut s = fresh();
    ld_f80(&mut s, F80::new(0x3fff - 149, J).to_bytes());
    let (r, o) = st_f32(&s, 0);
    assert_eq!((r, o.value, o.status & 0x3f), (status::OK, 1, 0));
}

#[test]
fn zero_divide_outranks_a_denormal_operand() {
    // SDM Vol. 1 §4.9.2: ZE is detected before DE. Denormal / +0 with only
    // DE unmasked is a masked ZE: +inf, ZE alone, nothing pending.
    let mut s = fresh();
    ldcw(&mut s, cw_unmasking(DE));
    ld_f80(&mut s, F80::new(0, 1).to_bytes());
    assert_eq!(
        arith(&mut s, arith_op::DIV, 0, src_kind::F32, 0),
        status::OK
    );
    assert_eq!(s.st(0), Some(F80::INFINITY));
    assert_eq!(s.status & 0xff, ZE);
    // The same for a binary32-denormal memory operand divided by a zero
    // register (FDIVR m32: ST0 = m32 / ST0).
    let mut s = fresh();
    ldcw(&mut s, cw_unmasking(DE));
    ld_const(&mut s, 1);
    assert_eq!(
        arith(&mut s, arith_op::DIVR, 0, src_kind::F32, 1),
        status::OK
    );
    assert_eq!(s.st(0), Some(F80::INFINITY));
    assert_eq!(s.status & 0xff, ZE);
}

#[test]
fn a_suppressing_denormal_records_no_precision_flag() {
    // FADD m32 (the smallest binary32 denormal) to 1.0 with DE unmasked:
    // the add does not happen, so the PE it would raise is not recorded.
    let mut s = fresh();
    ldcw(&mut s, cw_unmasking(DE));
    ld_const(&mut s, 0);
    let before = s;
    assert_eq!(
        arith(&mut s, arith_op::ADD, 0, src_kind::F32, 1),
        status::OK
    );
    assert_eq!(s.status & 0x3f, DE);
    assert!(s.pending());
    assert_eq!((s.st, s.tag), (before.st, before.tag));
    // The same through a transcendental: fsqrt of an 80-bit denormal.
    let mut s = fresh();
    ldcw(&mut s, cw_unmasking(DE));
    ld_f80(&mut s, F80::new(0, 3).to_bytes());
    assert_eq!(unary(&mut s, unary_op::SQRT), status::OK);
    assert_eq!(s.status & 0x3f, DE);
    let mut s = fresh();
    ldcw(&mut s, cw_unmasking(DE));
    ld_const(&mut s, 0);
    ld_f80(&mut s, F80::new(0, 3).to_bytes());
    assert_eq!(unary(&mut s, unary_op::PATAN), status::OK);
    assert_eq!(s.status & 0x3f, DE);
}

#[test]
fn fnstenv_discards_the_pending_exception() {
    // FSQRT on an empty stack with IE unmasked leaves #MF pending. FNSTENV
    // images ES and B, then masks everything, which clears them live.
    let mut s = fresh();
    ldcw(&mut s, cw_unmasking(IE));
    assert_eq!(unary(&mut s, unary_op::SQRT), status::OK);
    assert!(s.pending());
    let img = save_image(&s, env_op::FNSTENV);
    let saved_sw = u16::from_le_bytes([img[4], img[5]]);
    assert_eq!(saved_sw & (SW_ES | SW_B), SW_ES | SW_B);
    save_commit(&mut s, env_op::FNSTENV);
    assert_eq!(s.control & 0x3f, 0x3f);
    assert_eq!(s.status & (SW_ES | SW_B), 0);
    assert_eq!(s.status & (IE | SF), IE | SF, "the flags stay");
    assert_eq!(wait(&s), status::OK);
    // FLDENV of the saved image brings the fault back.
    let mut full = [0u8; 108];
    full[..28].copy_from_slice(&img[..28]);
    assert_eq!(load_env(&mut s, env_op::FLDENV, &full), status::OK);
    assert_eq!(wait(&s), status::MF);
}

#[test]
fn a_suppressed_compare_keeps_the_old_condition_codes() {
    // FCOM m32 1.0 against a QNaN with IE unmasked, previous C3 set: the
    // compare does not happen. The payload is SUPPRESS with the unchanged
    // C3/C2/C0, so a fused fnstsw/test ah,0x41/jz takes the branch fnstsw
    // would.
    let mut s = fresh();
    ldcw(&mut s, cw_unmasking(IE));
    ld_f80(&mut s, F80::INDEFINITE.to_bytes());
    s.status |= SW_C3;
    let before = s;
    let r = cmp(&mut s, 0, src_kind::F32, 0x3f80_0000, 1, false);
    assert_eq!(r & status::MASK, status::SUPPRESS);
    assert_eq!(r & !status::MASK, SW_C3 as u32);
    assert_eq!(
        r as u16 & (SW_C3 | SW_C2 | SW_C0),
        s.status & (SW_C3 | SW_C2 | SW_C0)
    );
    let fused_jz = (r >> 8) & 0x41 == 0;
    let real_jz = (s.status >> 8) & 0x41 == 0;
    assert_eq!(fused_jz, real_jz);
    assert_eq!((s.top(), s.tag), (before.top(), before.tag), "no pop");
    assert!(s.pending());
    // FCOMI: SUPPRESS, payload 0; the caller must keep ZF/PF/CF.
    let mut s = fresh();
    ldcw(&mut s, cw_unmasking(IE));
    ld_f80(&mut s, F80::INDEFINITE.to_bytes());
    ld_f80(&mut s, F80::INDEFINITE.to_bytes());
    assert_eq!(cmp(&mut s, 0, src_kind::ST, 1, 1, true), status::SUPPRESS);
    // An unmasked stack underflow (FCOM on an empty stack) also suppresses;
    // C1 = 0 (underflow) is the one condition code it writes.
    let mut s = fresh();
    ldcw(&mut s, cw_unmasking(IE));
    s.status |= SW_C0 | SW_C1;
    let r = cmp(&mut s, 0, src_kind::F32, 0, 0, false);
    assert_eq!(r, status::SUPPRESS | SW_C0 as u32);
    assert_eq!(s.status & (SW_C1 | IE | SF), IE | SF);
    // A masked QNaN compare is still an ordinary unordered result.
    let mut s = fresh();
    ld_f80(&mut s, F80::INDEFINITE.to_bytes());
    assert_eq!(
        cmp(&mut s, 0, src_kind::F32, 0x3f80_0000, 0, false),
        (SW_C3 | SW_C2 | SW_C0) as u32
    );
}

#[test]
fn fscale_by_zero_normalises_a_pseudo_denormal() {
    // A pseudo-denormal is a DE operand; the arithmetic result is its normal
    // encoding (biased exponent 1), tagged valid.
    let mut s = fresh();
    ld_f80(&mut s, F80::ZERO.to_bytes());
    ld_f80(&mut s, F80::new(0, 0x8000_0000_0000_0001).to_bytes());
    assert_eq!(unary(&mut s, unary_op::SCALE), status::OK);
    assert_eq!(s.st(0), Some(F80::new(1, 0x8000_0000_0000_0001)));
    assert_eq!(s.tag_of(s.phys(0)), TAG_VALID);
    assert_eq!(s.status & 0x3f, DE);
}

#[test]
fn save_image_reads_through_a_const_pointer() {
    // h2x87_v1_save_image takes `const X87Abi *`: shared, immutable storage
    // is a valid input.
    static FROZEN: X87Abi = X87Abi {
        st: [[0; 10]; 8],
        control: 0x037f,
        status: 0x3800 | PE,
        tag: 0xffff,
        opcode: 0x1d9,
        instruction_pointer: 0x1234,
        data_pointer: 0x5678,
        instruction_selector: 0x1b,
        data_selector: 0x23,
    };
    let mut img = [0u8; 108];
    // SAFETY: FROZEN is a live X87Abi, only read; img has 108 bytes.
    let r = unsafe { h2x87_v1_save_image(&FROZEN, env_op::FNSAVE, img.as_mut_ptr()) };
    assert_eq!(r, status::OK);
    assert_eq!(img, save_image(&FROZEN, env_op::FNSAVE));
    assert_eq!(u16::from_le_bytes([img[4], img[5]]), 0x3800 | PE);
}

#[test]
fn remainder_shortcuts_report_tininess_and_normalise() {
    // FPREM/FPREM1 with |ST0| far below |ST1| (quotient 0) or ST1 = inf
    // return ST0 as the remainder. A true denormal is an exact tiny result:
    // with UE unmasked, both stop and change nothing.
    for divisor in [ONE, F80::INFINITY] {
        for op in [unary_op::PREM, unary_op::PREM1] {
            let mut s = fresh();
            ldcw(&mut s, cw_unmasking(UE));
            ld_f80(&mut s, divisor.to_bytes());
            ld_f80(&mut s, F80::new(0, 1).to_bytes());
            let before = s;
            assert_eq!(
                unary(&mut s, op),
                status::UNMODELLED,
                "op {op} by {divisor:?}"
            );
            assert_eq!(s, before);
            // Masked, the pseudo-denormal comes back normalised and valid,
            // with DE alone and the remainder complete (C2 = 0).
            let mut s = fresh();
            s.status |= SW_C2;
            ld_f80(&mut s, divisor.to_bytes());
            ld_f80(&mut s, F80::new(0, 0x8000_0000_0000_0001).to_bytes());
            assert_eq!(unary(&mut s, op), status::OK);
            assert_eq!(s.st(0), Some(F80::new(1, 0x8000_0000_0000_0001)));
            assert_eq!(s.tag_of(s.phys(0)), TAG_VALID);
            assert_eq!((s.status & 0x3f, s.status & SW_C2), (DE, 0));
        }
    }
}
