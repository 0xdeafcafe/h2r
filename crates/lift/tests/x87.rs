//! x87 lowering: decoded instructions to `Intrinsic::X87` shapes.
//!
//! Three groups:
//!
//! - **Shapes** (always run): every x87 mnemonic in the retail image's
//!   census, from hand-written encodings, lowers without an `Unsupported`,
//!   validates, and has the statement shape docs/FLOAT-ABI.md asks for
//!   (wait before a memory read, three-phase stores, FIP/FOP marks, the
//!   `fcomi` flag merge, environment images).
//! - **Semantics**: short programs through the interpreter, whose x87 ops
//!   run on `crates/x87`'s helpers.
//! - **Retail hot paths** (need `H2_XBE`): the functions holding the frame
//!   delta, tick budget, script sleep and `_ftol2` lower completely.

use std::io::Write;
use std::path::PathBuf;

use iced_x86::{Decoder, DecoderOptions, Instruction};
use ir::interp::{self, Cpu, Exit, SparseMemory};
use ir::x87::{X87Arith, X87CmpKind, X87Env, X87Format, X87Op, X87Src};
use ir::{Control, Fault, Flag, Gpr, Intrinsic, IrInsn, Slice, Stmt, Width};
use lift::{Env, Site, lower};

const CODE: u32 = 0x1000;
const DATA: u32 = 0x4000;
const STACK: u32 = 0x8000;

fn decode(code: &[u8]) -> Instruction {
    let i = Decoder::with_ip(32, code, CODE as u64, DecoderOptions::NONE).decode();
    assert_eq!(i.len(), code.len(), "{code:02x?} decodes to one instruction");
    i
}

fn lowered(code: &[u8]) -> IrInsn {
    let ir = lower(&decode(code), &Env::default(), Site::Plain);
    ir.validate().unwrap_or_else(|e| panic!("{code:02x?}: {e}\n{ir}"));
    ir
}

/// The x87 ops (and waits) of an instruction, nested statements included,
/// in order.
fn ops(ir: &IrInsn) -> Vec<Intrinsic> {
    fn walk(stmts: &[Stmt], out: &mut Vec<Intrinsic>) {
        for s in stmts {
            match s {
                Stmt::Intrinsic { op, .. } => out.push(*op),
                Stmt::If { then, otherwise, .. } => {
                    walk(then, out);
                    walk(otherwise, out);
                }
                _ => {}
            }
        }
    }
    let mut v = Vec::new();
    walk(&ir.stmts, &mut v);
    v
}

fn x87(op: X87Op) -> Intrinsic {
    Intrinsic::X87(op)
}

fn mark(ir: &IrInsn) -> Option<(u16, bool)> {
    ops(ir).into_iter().find_map(|o| match o {
        Intrinsic::X87(X87Op::Mark { fop, mem }) => Some((fop, mem)),
        _ => None,
    })
}

/// Every x87 mnemonic the retail census has (and the rest of the P6 set),
/// memory forms on `[eax]`: (encoding, control instruction).
const ALL: &[(&[u8], bool)] = &[
    // loads
    (&[0xD9, 0x00], false), // fld m32
    (&[0xDD, 0x00], false), // fld m64
    (&[0xDB, 0x28], false), // fld m80
    (&[0xD9, 0xC1], false), // fld st(1)
    (&[0xDF, 0x00], false), // fild m16
    (&[0xDB, 0x00], false), // fild m32
    (&[0xDF, 0x28], false), // fild m64
    (&[0xDF, 0x20], false), // fbld
    (&[0xD9, 0xE8], false), // fld1
    (&[0xD9, 0xE9], false), // fldl2t
    (&[0xD9, 0xEA], false), // fldl2e
    (&[0xD9, 0xEB], false), // fldpi
    (&[0xD9, 0xEC], false), // fldlg2
    (&[0xD9, 0xED], false), // fldln2
    (&[0xD9, 0xEE], false), // fldz
    // stores
    (&[0xD9, 0x10], false), // fst m32
    (&[0xDD, 0x10], false), // fst m64
    (&[0xDD, 0xD1], false), // fst st(1)
    (&[0xD9, 0x18], false), // fstp m32
    (&[0xDD, 0x18], false), // fstp m64
    (&[0xDB, 0x38], false), // fstp m80
    (&[0xDD, 0xD9], false), // fstp st(1)
    (&[0xD9, 0xD9], false), // fstp st(1), the D9 alias
    (&[0xDF, 0x10], false), // fist m16
    (&[0xDB, 0x10], false), // fist m32
    (&[0xDF, 0x18], false), // fistp m16
    (&[0xDB, 0x18], false), // fistp m32
    (&[0xDF, 0x38], false), // fistp m64
    (&[0xDF, 0x30], false), // fbstp
    // arithmetic
    (&[0xD8, 0x00], false), // fadd m32
    (&[0xDC, 0x00], false), // fadd m64
    (&[0xD8, 0xC1], false), // fadd st, st(1)
    (&[0xDC, 0xC1], false), // fadd st(1), st
    (&[0xDE, 0xC1], false), // faddp
    (&[0xDA, 0x00], false), // fiadd m32
    (&[0xDE, 0x00], false), // fiadd m16
    (&[0xD8, 0x20], false), // fsub m32
    (&[0xDC, 0xE9], false), // fsub st(1), st
    (&[0xDE, 0xE9], false), // fsubp
    (&[0xDA, 0x20], false), // fisub m32
    (&[0xD8, 0x28], false), // fsubr m32
    (&[0xDE, 0xE1], false), // fsubrp
    (&[0xDA, 0x28], false), // fisubr m32
    (&[0xD8, 0x08], false), // fmul m32
    (&[0xDE, 0xC9], false), // fmulp
    (&[0xDA, 0x08], false), // fimul m32
    (&[0xD8, 0x30], false), // fdiv m32
    (&[0xDE, 0xF9], false), // fdivp
    (&[0xDA, 0x30], false), // fidiv m32
    (&[0xD8, 0x38], false), // fdivr m32
    (&[0xDE, 0xF1], false), // fdivrp
    (&[0xDA, 0x38], false), // fidivr m32
    (&[0xD9, 0xFA], false), // fsqrt
    (&[0xD9, 0xE1], false), // fabs
    (&[0xD9, 0xE0], false), // fchs
    (&[0xD9, 0xFC], false), // frndint
    (&[0xD9, 0xFE], false), // fsin
    (&[0xD9, 0xFF], false), // fcos
    (&[0xD9, 0xF2], false), // fptan
    (&[0xD9, 0xF3], false), // fpatan
    (&[0xD9, 0xF1], false), // fyl2x
    (&[0xD9, 0xF9], false), // fyl2xp1
    (&[0xD9, 0xF0], false), // f2xm1
    (&[0xD9, 0xFD], false), // fscale
    (&[0xD9, 0xF4], false), // fxtract
    (&[0xD9, 0xFB], false), // fsincos
    (&[0xD9, 0xF8], false), // fprem
    (&[0xD9, 0xF5], false), // fprem1
    // compares
    (&[0xD8, 0x10], false), // fcom m32
    (&[0xD8, 0xD1], false), // fcom st(1)
    (&[0xDC, 0x18], false), // fcomp m64
    (&[0xD8, 0xD9], false), // fcomp st(1)
    (&[0xDE, 0xD9], false), // fcompp
    (&[0xDD, 0xE1], false), // fucom st(1)
    (&[0xDD, 0xE9], false), // fucomp st(1)
    (&[0xDA, 0xE9], false), // fucompp
    (&[0xDA, 0x10], false), // ficom m32
    (&[0xDE, 0x18], false), // ficomp m16
    (&[0xDB, 0xF1], false), // fcomi
    (&[0xDF, 0xF1], false), // fcomip
    (&[0xDB, 0xE9], false), // fucomi
    (&[0xDF, 0xE9], false), // fucomip
    (&[0xD9, 0xE4], false), // ftst
    (&[0xD9, 0xE5], false), // fxam
    // stack
    (&[0xD9, 0xC9], false), // fxch
    (&[0xDD, 0xC1], false), // ffree st(1)
    (&[0xDF, 0xC1], false), // ffreep st(1)
    (&[0xD9, 0xF7], false), // fincstp
    (&[0xD9, 0xF6], false), // fdecstp
    (&[0xDA, 0xC1], false), // fcmovb
    (&[0xDA, 0xC9], false), // fcmove
    (&[0xDA, 0xD1], false), // fcmovbe
    (&[0xDA, 0xD9], false), // fcmovu
    (&[0xDB, 0xC1], false), // fcmovnb
    (&[0xDB, 0xC9], false), // fcmovne
    (&[0xDB, 0xD1], false), // fcmovnbe
    (&[0xDB, 0xD9], false), // fcmovnu
    (&[0xD9, 0xD0], false), // fnop
    // control
    (&[0xD9, 0x28], true), // fldcw
    (&[0xD9, 0x38], true), // fnstcw
    (&[0xDF, 0xE0], true), // fnstsw ax
    (&[0xDD, 0x38], true), // fnstsw m16
    (&[0xDB, 0xE2], true), // fnclex
    (&[0xDB, 0xE3], true), // fninit
    (&[0xD9, 0x30], true), // fnstenv
    (&[0xD9, 0x20], true), // fldenv
    (&[0xDD, 0x30], true), // fnsave
    (&[0xDD, 0x20], true), // frstor
    (&[0x9B], true),       // fwait
    (&[0xDB, 0xE0], true), // fneni
    (&[0xDB, 0xE1], true), // fndisi
    (&[0xDB, 0xE4], true), // fnsetpm
];

#[test]
fn every_x87_form_lowers_and_marks_unless_control() {
    for &(code, control) in ALL {
        let ir = lowered(code);
        assert!(ir.is_supported(), "{code:02x?} unsupported:\n{ir}");
        assert_eq!(ir.control, Control::Next, "{code:02x?}");
        assert_eq!(mark(&ir).is_none(), control, "{code:02x?}: FIP/FOP mark\n{ir}");
    }
}

#[test]
fn a_memory_load_waits_reads_converts_then_marks() {
    // fld dword [ebp-8] (d9 45 f8)
    let ir = lowered(&[0xD9, 0x45, 0xF8]);
    let kinds: Vec<&str> = ir
        .stmts
        .iter()
        .filter_map(|s| match s {
            Stmt::Intrinsic { op: Intrinsic::X87Wait, .. } => Some("wait"),
            Stmt::Load { dst, .. } if dst.width == Width::W32 => Some("load"),
            Stmt::Intrinsic { op: Intrinsic::X87(X87Op::Load(X87Format::F32)), .. } => Some("ld_f32"),
            Stmt::Intrinsic { op: Intrinsic::X87(X87Op::Mark { .. }), .. } => Some("mark"),
            _ => None,
        })
        .collect();
    assert_eq!(kinds, ["wait", "load", "ld_f32", "mark"], "\n{ir}");
    // FOP: D9 & 7, then ModR/M 45.
    assert_eq!(mark(&ir), Some((0x145, true)));
}

#[test]
fn a_store_converts_writes_if_told_then_commits_and_pops() {
    // fstp dword [eax] (d9 18): no wait (the convert step checks ES itself).
    let ir = lowered(&[0xD9, 0x18]);
    assert_eq!(
        ops(&ir),
        [
            x87(X87Op::StoreConvert { to: X87Format::F32, src: 0 }),
            x87(X87Op::StoreCommit { pop: true }),
            x87(X87Op::Mark { fop: 0x118, mem: true }),
        ]
    );
    // The write sits inside `if write`.
    let store_in_if = ir.stmts.iter().any(|s| match s {
        Stmt::If { then, otherwise, .. } => {
            otherwise.is_empty()
                && matches!(then.as_slice(), [Stmt::Store { value, .. }] if value.width() == Width::W32)
        }
        _ => false,
    });
    assert!(store_in_if, "\n{ir}");
    // fist dword [eax] (db 10) keeps the value.
    assert!(ops(&lowered(&[0xDB, 0x10])).contains(&x87(X87Op::StoreCommit { pop: false })));
    // fstp tbyte [eax] (db 38) writes 80 bits.
    assert!(ops(&lowered(&[0xDB, 0x38])).contains(&x87(X87Op::StoreConvert { to: X87Format::F80, src: 0 })));
}

#[test]
fn register_arithmetic_names_the_destination_first() {
    let arith = |code: &[u8]| {
        ops(&lowered(code)).into_iter().find_map(|o| match o {
            Intrinsic::X87(X87Op::Arith { op, dst, src, pop }) => Some((op, dst, src, pop)),
            _ => None,
        })
    };
    // fsub st(1), st (dc e9): ST(1) = ST(1) - ST(0).
    assert_eq!(arith(&[0xDC, 0xE9]), Some((X87Arith::Sub, 1, X87Src::St(0), false)));
    // fsubrp st(1), st (de e1): ST(1) = ST(0) - ST(1), pop.
    assert_eq!(arith(&[0xDE, 0xE1]), Some((X87Arith::SubR, 1, X87Src::St(0), true)));
    // fdiv st, st(3) (d8 f3).
    assert_eq!(arith(&[0xD8, 0xF3]), Some((X87Arith::Div, 0, X87Src::St(3), false)));
    // fimul word [eax] (de 08).
    assert_eq!(arith(&[0xDE, 0x08]), Some((X87Arith::Mul, 0, X87Src::I16, false)));
    // fadd qword [eax] (dc 00).
    assert_eq!(arith(&[0xDC, 0x00]), Some((X87Arith::Add, 0, X87Src::F64, false)));
}

#[test]
fn compares_pop_and_ftst_compares_with_zero() {
    let cmp = |code: &[u8]| {
        lowered(code).stmts.iter().find_map(|s| match s {
            Stmt::Intrinsic { op: Intrinsic::X87(X87Op::Cmp { kind, src, pop, to_flags }), args, .. } => {
                Some((*kind, *src, *pop, *to_flags, args.iter().map(|a| a.as_const()).collect::<Vec<_>>()))
            }
            _ => None,
        })
    };
    assert_eq!(cmp(&[0xDE, 0xD9]), Some((X87CmpKind::Com, X87Src::St(1), 2, false, vec![])), "fcompp");
    assert_eq!(cmp(&[0xDA, 0xE9]), Some((X87CmpKind::Ucom, X87Src::St(1), 2, false, vec![])), "fucompp");
    assert_eq!(cmp(&[0xDE, 0x18]).map(|c| (c.0, c.1, c.2)), Some((X87CmpKind::Icom, X87Src::I16, 1)), "ficomp m16");
    assert_eq!(cmp(&[0xD9, 0xE4]), Some((X87CmpKind::Com, X87Src::F32, 0, false, vec![Some(0)])), "ftst");
    assert_eq!(cmp(&[0xDF, 0xF1]), Some((X87CmpKind::Com, X87Src::St(1), 1, true, vec![])), "fcomip");
    assert_eq!(cmp(&[0xDB, 0xE9]), Some((X87CmpKind::Ucom, X87Src::St(1), 0, true, vec![])), "fucomi");
}

#[test]
fn fcomi_defines_all_six_status_flags() {
    for code in [[0xDB, 0xF1], [0xDF, 0xF1], [0xDB, 0xE9], [0xDF, 0xE9]] {
        let fx = lowered(&code).flag_effects();
        for f in Flag::STATUS {
            assert!(fx.defined.contains(f), "{code:02x?}: {f:?}");
        }
        assert!(!fx.defined.contains(Flag::Df));
    }
    // The other compares leave EFLAGS alone.
    assert_eq!(lowered(&[0xDE, 0xD9]).flag_effects().defined.0, 0);
}

#[test]
fn fnstsw_ax_is_a_field_read_into_ax() {
    let ir = lowered(&[0xDF, 0xE0]);
    assert!(matches!(ir.stmts[0], Stmt::Intrinsic { op: Intrinsic::X87ReadStatus, .. }));
    assert!(matches!(ir.stmts[1], Stmt::WriteReg { reg: Gpr::Eax, slice: Slice::Low16, .. }));
    assert_eq!(ir.stmts.len(), 2);
}

/// The waiting control forms (`9B` + the `fn` form) decode as `wait` and
/// the non-waiting instruction (FLOAT-ABI.md §2.2): `wait` is one
/// `X87Wait`, the `fn` half has none.
#[test]
fn waiting_control_forms_are_a_wait_then_the_no_wait_form() {
    use iced_x86::Mnemonic as M;
    assert_eq!(ops(&lowered(&[0x9B])), [Intrinsic::X87Wait]);
    let forms: &[(&[u8], M)] = &[
        (&[0x9B, 0xD9, 0x38], M::Fnstcw),  // fstcw [eax]
        (&[0x9B, 0xDD, 0x38], M::Fnstsw),  // fstsw [eax]
        (&[0x9B, 0xDF, 0xE0], M::Fnstsw),  // fstsw ax
        (&[0x9B, 0xDB, 0xE2], M::Fnclex),  // fclex
        (&[0x9B, 0xDB, 0xE3], M::Fninit),  // finit
        (&[0x9B, 0xD9, 0x30], M::Fnstenv), // fstenv [eax]
        (&[0x9B, 0xDD, 0x30], M::Fnsave),  // fsave [eax]
    ];
    for &(code, m) in forms {
        let mut d = Decoder::with_ip(32, code, CODE as u64, DecoderOptions::NONE);
        let wait = d.decode();
        let rest = d.decode();
        assert_eq!((wait.mnemonic(), wait.len(), rest.mnemonic()), (M::Wait, 1, m), "{code:02x?}");
        let w = lower(&wait, &Env::default(), Site::Plain);
        assert_eq!(ops(&w), [Intrinsic::X87Wait], "{code:02x?}");
        let r = lower(&rest, &Env::default(), Site::Plain);
        r.validate().unwrap_or_else(|e| panic!("{code:02x?}: {e}\n{r}"));
        assert!(!ops(&r).contains(&Intrinsic::X87Wait), "{code:02x?}: no second wait\n{r}");
    }
}

#[test]
fn fisttp_is_invalid_and_the_8087_leftovers_do_nothing() {
    // fisttp word [eax] (df 08): SSE3, #UD on the Pentium III.
    assert_eq!(lowered(&[0xDF, 0x08]).control, Control::Fault(Fault::InvalidOpcode));
    for code in [[0xDB, 0xE0], [0xDB, 0xE1], [0xDB, 0xE4]] {
        assert!(lowered(&code).stmts.is_empty(), "{code:02x?}");
    }
    // fnop is a waiting instruction.
    assert_eq!(ops(&lowered(&[0xD9, 0xD0]))[0], Intrinsic::X87Wait);
}

#[test]
fn environment_images_are_written_and_read_in_chunks() {
    // fnsave [eax] (dd 30): image, 108 bytes of stores, commit; no wait.
    let ir = lowered(&[0xDD, 0x30]);
    assert_eq!(ops(&ir), [x87(X87Op::SaveImage(X87Env::Save)), x87(X87Op::SaveCommit(X87Env::Save))]);
    let stored: u32 = ir
        .stmts
        .iter()
        .filter_map(|s| if let Stmt::Store { value, .. } = s { Some(value.width().bytes()) } else { None })
        .sum();
    assert_eq!(stored, 108);
    // frstor [eax] (dd 20): wait first, 108 bytes of loads, then the load.
    let ir = lowered(&[0xDD, 0x20]);
    assert_eq!(ops(&ir), [Intrinsic::X87Wait, x87(X87Op::LoadEnv(X87Env::Rstor))]);
    let loaded: u32 = ir
        .stmts
        .iter()
        .filter_map(|s| if let Stmt::Load { dst, .. } = s { Some(dst.width.bytes()) } else { None })
        .sum();
    assert_eq!(loaded, 108);
    // fnstenv [eax] (d9 30): 28 bytes.
    let ir = lowered(&[0xD9, 0x30]);
    let stored: u32 = ir
        .stmts
        .iter()
        .filter_map(|s| if let Stmt::Store { value, .. } = s { Some(value.width().bytes()) } else { None })
        .sum();
    assert_eq!(stored, 28);
    // The 16-bit layouts (66 prefix) are refused.
    assert!(!lowered(&[0x66, 0xD9, 0x30]).is_supported());
}

#[test]
fn fop_rebuilds_modrm_for_every_addressing_form() {
    let fop = |code: &[u8]| mark(&lowered(code)).unwrap().0;
    assert_eq!(fop(&[0xD9, 0x44, 0x24, 0x10]), 0x144, "fld [esp+10h]: SIB, disp8");
    assert_eq!(fop(&[0xD9, 0x05, 0xD8, 0xDB, 0x45, 0x00]), 0x105, "fld [disp32]");
    assert_eq!(fop(&[0xD9, 0x04, 0x88]), 0x104, "fld [eax+ecx*4]");
    assert_eq!(fop(&[0xD9, 0x80, 0, 1, 0, 0]), 0x180, "fld [eax+disp32]");
    assert_eq!(fop(&[0xDD, 0x1C, 0x24]), 0x51c, "fstp qword [esp]");
    assert_eq!(fop(&[0xDE, 0xC9]), 0x6c9, "fmulp st(1)");
    assert_eq!(fop(&[0xD9, 0xE8]), 0x1e8, "fld1");
    // Redundant SIB bytes (no index) keep the encoded rm = 4.
    assert_eq!(fop(&[0xD9, 0x04, 0x20]), 0x104, "fld [eax] through a SIB");
    assert_eq!(fop(&[0xD9, 0x44, 0x20, 0x08]), 0x144, "fld [eax+8] through a SIB");
    assert_eq!(fop(&[0xD9, 0x04, 0x25, 0, 0x40, 0, 0]), 0x104, "fld [disp32] through a SIB");
    // An unexplained extra byte (a redundant REP prefix) is refused.
    assert!(!lowered(&[0xF3, 0xD9, 0x00]).is_supported());
}

#[test]
fn fs_relative_x87_operands_are_refused() {
    // fld dword fs:[eax] (64 d9 00)
    assert!(!lowered(&[0x64, 0xD9, 0x00]).is_supported());
}

#[test]
fn ftol2_replacement_is_one_helper_call() {
    let f = lift::crt::ftol2(lift::crt::FTOL2);
    f.validate().unwrap();
    let insn = f.insns().next().unwrap();
    assert_eq!(ops(insn), [x87(X87Op::Ftol2)]);
}

// ---- semantics: need crates/x87 in the interpreter ------------------------

fn run(code: &[u8], cpu: &mut Cpu, mem: &mut SparseMemory) -> Result<(), Exit> {
    let insns: Vec<_> = Decoder::with_ip(32, code, CODE as u64, DecoderOptions::NONE).into_iter().collect();
    let env = Env::default();
    let mut pc = CODE;
    while let Some(insn) = insns.iter().find(|i| i.ip32() == pc) {
        let ir = lower(insn, &env, Site::Plain);
        ir.validate().unwrap();
        pc = interp::step(cpu, mem, &ir)?.target();
    }
    Ok(())
}

fn setup() -> (Cpu, SparseMemory) {
    let mut c = Cpu::default();
    c.set_reg(Gpr::Esp, STACK);
    c.x87.control = 0x027F;
    let mut m = SparseMemory::new();
    m.map(DATA, 0x100);
    m.map(STACK - 0x100, 0x200);
    (c, m)
}

#[test]
fn one_plus_one_is_two_and_the_stack_is_empty_again() {
    // fld1; fld1; faddp; fstp dword [0x4000]
    let code = [0xD9, 0xE8, 0xD9, 0xE8, 0xDE, 0xC1, 0xD9, 0x1D, 0x00, 0x40, 0x00, 0x00];
    let (mut c, mut m) = setup();
    run(&code, &mut c, &mut m).unwrap();
    assert_eq!(m.read_u32(DATA), Some(0x4000_0000));
    assert_eq!((c.x87.tag, c.x87.status & 0x3800), (0xFFFF, 0));
    assert_eq!(c.x87.instruction_pointer, CODE + 6, "FIP: the fstp");
    assert_eq!(c.x87.data_pointer, DATA, "FDP: its operand");
}

#[test]
fn fcomip_merges_zf_pf_cf_and_clears_the_rest() {
    // fld1; fldz; fcomip st, st(1): 0 < 1, so CF=1, ZF=PF=0; OF/SF/AF = 0.
    let (mut c, mut m) = setup();
    c.eflags |= 0x0890; // OF, SF, AF set beforehand
    run(&[0xD9, 0xE8, 0xD9, 0xEE, 0xDF, 0xF1], &mut c, &mut m).unwrap();
    for (f, v) in [
        (Flag::Cf, true),
        (Flag::Zf, false),
        (Flag::Pf, false),
        (Flag::Of, false),
        (Flag::Sf, false),
        (Flag::Af, false),
    ] {
        assert!(c.flag_defined(f), "{f:?} defined");
        assert_eq!(c.flag(f), v, "{f:?}");
    }
}

#[test]
fn fnstsw_test_ah_jcc_branches_on_the_compare() {
    // fld1; fldz; fcompp (0 vs 1: C0 = 1); fnstsw ax; test ah, 0x41;
    // jz +5; mov al, 1 -> taken path skipped, al = 1 (lt, so not "gt").
    let code = [0xD9, 0xE8, 0xD9, 0xEE, 0xDE, 0xD9, 0xDF, 0xE0, 0xF6, 0xC4, 0x41, 0x74, 0x02, 0xB0, 0x01];
    let (mut c, mut m) = setup();
    run(&code, &mut c, &mut m).unwrap();
    assert_eq!(c.reg(Gpr::Eax) & 0xFF, 1);
    assert_eq!(c.x87.status & 0x4500, 0x0100, "C0 only");
}

#[test]
fn fldcw_normalises_and_an_unmasked_stack_fault_raises_mf_at_fwait() {
    // fldcw [0x4000] with 0x023E (IE unmasked; bit 6 clear): reads back
    // 0x027E. Then fld st(0) with an empty stack: IE+SF pending, ES set,
    // nothing pushed; the fwait after it raises #MF at its own PC.
    let code = [0xD9, 0x2D, 0x00, 0x40, 0x00, 0x00, 0xD9, 0xC0, 0x9B];
    let (mut c, mut m) = setup();
    m.load(DATA, &0x023Eu16.to_le_bytes());
    let e = run(&code, &mut c, &mut m).unwrap_err();
    assert_eq!(e, Exit::Fault { pc: CODE + 8, fault: Fault::FpError, next: None });
    assert_eq!(c.x87.control, 0x027E);
    assert_eq!(c.x87.status & 0x00C1, 0x00C1, "IE, SF, ES");
    assert_eq!(c.x87.instruction_pointer, CODE + 6, "FIP: the raiser, not the fwait");
}

#[test]
fn ftol2_truncates_and_returns() {
    let f = lift::crt::ftol2(lift::crt::FTOL2);
    let insn = f.insns().next().unwrap().clone();
    for (setup_code, want) in [
        // fld dword [0x4000] = -2.5
        (0xC020_0000u32, -2i64 as u64),
        // NaN
        (0x7FC0_0000, 0x8000_0000_0000_0000),
    ] {
        let (mut c, mut m) = setup();
        m.load(DATA, &setup_code.to_le_bytes());
        m.load(STACK, &0x1234u32.to_le_bytes());
        run(&[0xD9, 0x05, 0x00, 0x40, 0x00, 0x00], &mut c, &mut m).unwrap();
        let flow = interp::step(&mut c, &mut m, &insn).unwrap();
        assert_eq!(flow.target(), 0x1234);
        assert_eq!(((c.reg(Gpr::Edx) as u64) << 32) | c.reg(Gpr::Eax) as u64, want);
        assert_eq!(c.reg(Gpr::Esp), STACK + 4);
        assert_eq!(c.x87.tag, 0xFFFF, "popped");
    }
}

// ---- retail hot paths ------------------------------------------------------

fn xbe_path() -> Option<PathBuf> {
    let p = std::env::var_os("H2_XBE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../disc/default.xbe"));
    p.is_file().then_some(p)
}

/// Frame delta, tick budget (two), script sleep, and `_ftol2`.
const HOT: [u32; 5] = [0x0012_b229, 0x0014_67a1, 0x0014_69ad, 0x0020_8954, 0x0032_2b0c];

#[test]
fn retail_hot_paths_lower_completely() {
    let Some(path) = xbe_path() else {
        let _ = writeln!(std::io::stderr(), "SKIPPED retail_hot_paths_lower_completely: no XBE (set H2_XBE)");
        return;
    };
    let data = std::fs::read(path).unwrap();
    let x = xbe::Xbe::parse(&data).unwrap();
    assert_eq!(x.entry, 0x2d0aee, "these addresses are for the retail Halo 2 XBE");
    let analysis = analyze::analyze(&x).unwrap();
    let lifter = lift::Lifter::new(&analysis, &x).unwrap();
    for pc in HOT {
        // The function holding `pc`: the nearest entries at or below it.
        let owner = analysis
            .functions
            .range(..=pc)
            .rev()
            .take(16)
            .filter_map(|(&e, _)| lifter.lift(e))
            .find(|f| f.insns().any(|i| i.pc == pc))
            .unwrap_or_else(|| panic!("no function holds {pc:#x}"));
        owner.validate().unwrap_or_else(|e| panic!("{pc:#x} in {:#x}: {e}", owner.entry));
        if let Some(bad) = owner.insns().find(|i| !i.is_supported()) {
            panic!("{pc:#x} in {:#x}: unsupported at {:#x}:\n{bad}", owner.entry, bad.pc);
        }
        let at = owner.insns().find(|i| i.pc == pc).unwrap();
        assert!(ops(at).iter().any(|o| matches!(o, Intrinsic::X87(_))), "{pc:#x} is an x87 instruction:\n{at}");
    }
    // _ftol2 is the replacement, not the routine's own code.
    let f = lifter.lift(lift::crt::FTOL2).unwrap();
    assert_eq!(f.insns().count(), 1);
}
