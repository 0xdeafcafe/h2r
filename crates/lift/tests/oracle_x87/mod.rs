//! What Unicorn cannot answer about x87 state, and how the oracle harness
//! steps around it: masks by mnemonic, as docs/FLOAT.md §2.9 and
//! docs/FLOAT-ABI.md §6.3 set out. `crates/x87` is the authority for these
//! bits, and they are taken from our side:
//!
//! - the sticky exception flags but ZE (IE/DE/OE/UE/PE/SF), which Unicorn
//!   mostly does not set, from the first x87 operation until something
//!   defines them again (`fninit`, `fnclex`, `fnsave`; `fldenv`/`frstor`
//!   load whatever was masked in the image they read);
//! - C1, and C0/C2/C3 after an instruction that leaves them undefined
//!   (loads, stores, arithmetic, stack ops, `fldcw`), until one defines
//!   them (the compares, `ftst`, `fxam`, the initialising ones);
//! - control-word bit 6 after `fldcw`/`fldenv`/`frstor` (a P6 forces it to
//!   1, Unicorn stores the raw word; `needs-hardware`);
//! - FOP, FCS and FDS, which Unicorn does not keep. FIP and FDP it keeps
//!   for the data instructions, and they are compared, but it also moves
//!   FIP on the control instructions (`fnstsw`, `fwait`, ...), not on
//!   `fnop`, and `frstor`/`fldenv` do not load them: after one of those
//!   they are masked until the next data instruction;
//! - the reserved upper halves of the `fnstenv`/`fnsave` control, status,
//!   tag and FDS words: the crate writes 0xffff as a P6 does, Unicorn 0
//!   (`needs-hardware`);
//! - the sign of an `fbstp` whose rounded result is zero: a P6 stores a
//!   BCD -0, Unicorn drops the sign;
//! - OF/SF/AF after `fcomi` and its forms, which Unicorn leaves alone
//!   where the SDM clears them;
//! - the tag of the destination of `fst`/`fstp st(i)`, which Unicorn does
//!   not update.
//!
//! A mask follows the value it covers and ends when that value is
//! overwritten: status bits read into AX by `fnstsw ax` or into memory
//! (`fnstsw m16`, `fnstcw`, the `fnstenv`/`fnsave` image) stay masked until
//! an instruction writes those register bits or bytes outright or fully
//! determines them (`mov`, `and` with 0 bits, `or` with 1 bits); flags
//! computed from masked bits (`sahf`, `test ah`, `and ax, imm`, ...) stay
//! masked until something writes the flags; a tag stays masked until the
//! register is pushed onto, popped, freed or initialised.
//!
//! **Stack faults.** Unicorn has no tag checks, so it runs an overflowing
//! or underflowing operation as an ordinary one. Whether an instruction
//! stack-faults is decided here, independently of the side under test, by
//! a model of TOP and the empty registers ([`Stack`]) from the fixture's
//! initial state. From a predicted fault on, the x87 registers, tags and
//! status (but TOP), compare flags and x87 stores are taken from our
//! side; everything else is still compared. Our interpreter must fault at
//! exactly the predicted instructions: an unexpected or missing stack
//! fault is a mismatch.
//!
//! Skipped by class instead: transcendentals (host libm in Unicorn), an
//! `fld` of a signalling NaN (Unicorn does not quiet it), an `fbstp` whose
//! rounded result does not fit 18 digits (a P6 stores the BCD indefinite,
//! Unicorn garbage), and a stack fault after an instruction the model does
//! not follow.

use iced_x86::{Instruction, InstructionInfoFactory, MemorySize, Mnemonic as M, OpAccess, OpKind, Register};
use ir::Gpr;
use ir::interp::{self, Cpu, Memory};

const SW_STICKY: u16 = 0x007b; // IE DE OE UE PE SF
const SW_SF: u16 = 0x0040;
const SW_C1: u16 = 0x0200;
const SW_C023: u16 = 0x4500;
const SW_TOP: u16 = 0x3800;
const EFL_COMPARE: u32 = 0x08d5; // OF SF ZF AF PF CF

/// TOP and which physical registers are empty, from the fixture's initial
/// state and a per-mnemonic table of reads, pushes and pops: what decides
/// a stack fault, independently of the interpreter.
#[derive(Clone, Copy)]
struct Stack {
    top: u8,
    empty: [bool; 8],
}

/// One instruction's stack use: the ST(i) it reads, then whether it
/// pushes, how many it pops and the ST(i) it writes (`fst st(i)`); or one
/// of the instructions with a rule of its own.
enum Use {
    Plain { reads: Vec<u8>, push: bool, pops: u8, writes: Option<u8> },
    Free(u8),
    Inc,
    Dec,
    Init,
    Save,
    Restore,
    None,
}

fn st_operand(insn: &Instruction) -> Option<u8> {
    (0..insn.op_count())
        .filter(|&k| insn.op_kind(k) == OpKind::Register)
        .map(|k| insn.op_register(k))
        .filter(|r| (Register::ST0..=Register::ST7).contains(r))
        .map(|r| r as u8 - Register::ST0 as u8)
        .max()
}

/// `None`: the model does not follow `insn`.
fn stack_use(insn: &Instruction) -> Option<Use> {
    let mem = (0..insn.op_count()).any(|k| insn.op_kind(k) == OpKind::Memory);
    let i = st_operand(insn).unwrap_or(1);
    let plain = |reads: &[u8], push: bool, pops: u8| Use::Plain { reads: reads.to_vec(), push, pops, writes: None };
    Some(match insn.mnemonic() {
        M::Fld if mem => plain(&[], true, 0),
        M::Fld => plain(&[i], true, 0),
        M::Fild | M::Fbld | M::Fld1 | M::Fldl2t | M::Fldl2e | M::Fldpi | M::Fldlg2 | M::Fldln2 | M::Fldz => {
            plain(&[], true, 0)
        }
        M::Fxtract => plain(&[0], true, 0),
        M::Fst | M::Fist if mem => plain(&[0], false, 0),
        M::Fstp | M::Fistp | M::Fbstp if mem => plain(&[0], false, 1),
        M::Fst => Use::Plain { reads: vec![0], push: false, pops: 0, writes: Some(i) },
        M::Fstp => Use::Plain { reads: vec![0], push: false, pops: 1, writes: Some(i) },
        M::Fadd | M::Fsub | M::Fsubr | M::Fmul | M::Fdiv | M::Fdivr | M::Fcom | M::Fucom | M::Fcomi | M::Fucomi
            if !mem =>
        {
            plain(&[0, i], false, 0)
        }
        M::Faddp
        | M::Fsubp
        | M::Fsubrp
        | M::Fmulp
        | M::Fdivp
        | M::Fdivrp
        | M::Fcomp
        | M::Fucomp
        | M::Fcomip
        | M::Fucomip
            if !mem =>
        {
            plain(&[0, i], false, 1)
        }
        M::Fadd
        | M::Fsub
        | M::Fsubr
        | M::Fmul
        | M::Fdiv
        | M::Fdivr
        | M::Fcom
        | M::Fiadd
        | M::Fisub
        | M::Fisubr
        | M::Fimul
        | M::Fidiv
        | M::Fidivr
        | M::Ficom => plain(&[0], false, 0),
        M::Fcomp | M::Ficomp => plain(&[0], false, 1),
        M::Fcompp | M::Fucompp => plain(&[0, 1], false, 2),
        M::Fsqrt | M::Fabs | M::Fchs | M::Frndint | M::Ftst => plain(&[0], false, 0),
        M::Fscale => plain(&[0, 1], false, 0),
        M::Fxch => plain(&[0, i], false, 0),
        M::Fcmovb | M::Fcmove | M::Fcmovbe | M::Fcmovu | M::Fcmovnb | M::Fcmovne | M::Fcmovnbe | M::Fcmovnu => {
            plain(&[0, i], false, 0)
        }
        M::Ffree => Use::Free(i),
        M::Fincstp => Use::Inc,
        M::Fdecstp => Use::Dec,
        M::Fninit => Use::Init,
        M::Fnsave => Use::Save,
        M::Fldenv | M::Frstor => Use::Restore,
        M::Fxam | M::Fnop | M::Wait | M::Fnstsw | M::Fnstcw | M::Fldcw | M::Fnclex | M::Fnstenv => Use::None,
        _ => return None,
    })
}

impl Stack {
    fn of(x: &interp::X87) -> Stack {
        Stack { top: ((x.status >> 11) & 7) as u8, empty: std::array::from_fn(|r| (x.tag >> (2 * r)) & 3 == 3) }
    }

    fn phys(&self, i: u8) -> usize {
        ((self.top + i) & 7) as usize
    }

    /// Run `u`: whether it stack-faults, and the physical registers whose
    /// emptiness it sets (`None` with a fault: the state after it is ours).
    fn step(&mut self, u: &Use, saved: &mut Option<Stack>) -> Result<Vec<usize>, ()> {
        let mut touched = Vec::new();
        match u {
            Use::Plain { reads, push, pops, writes } => {
                if reads.iter().any(|&i| self.empty[self.phys(i)]) {
                    return Err(());
                }
                if *push && !self.empty[self.phys(7)] {
                    return Err(());
                }
                if let Some(i) = writes {
                    let r = self.phys(*i);
                    self.empty[r] = false;
                }
                if *push {
                    self.top = (self.top + 7) & 7;
                    self.empty[self.top as usize] = false;
                    touched.push(self.top as usize);
                }
                for _ in 0..*pops {
                    self.empty[self.top as usize] = true;
                    touched.push(self.top as usize);
                    self.top = (self.top + 1) & 7;
                }
            }
            Use::Free(i) => {
                let r = self.phys(*i);
                self.empty[r] = true;
                touched.push(r);
            }
            Use::Inc => self.top = (self.top + 1) & 7,
            Use::Dec => self.top = (self.top + 7) & 7,
            Use::Init | Use::Save => {
                if matches!(u, Use::Save) {
                    *saved = Some(*self);
                }
                *self = Stack { top: 0, empty: [true; 8] };
                touched.extend(0..8);
            }
            Use::Restore => {
                *self = saved.ok_or(())?;
                touched.extend(0..8);
            }
            Use::None => {}
        }
        Ok(touched)
    }
}

/// The rounded integer `fbstp` stores from `r` under `rc` (control word
/// bits 10-11): `None` for NaN or infinity, otherwise (negative, magnitude),
/// the magnitude saturating at `u128::MAX`.
fn bcd_rounded(r: [u8; 10], rc: u16) -> Option<(bool, u128)> {
    let neg = r[9] & 0x80 != 0;
    let exp = (u16::from_le_bytes([r[8], r[9]]) & 0x7fff) as i32;
    let mant = u64::from_le_bytes(r[..8].try_into().unwrap());
    if exp == 0x7fff {
        return None;
    }
    if mant == 0 {
        return Some((neg, 0));
    }
    // value = mant * 2^shift
    let shift = exp.max(1) - 16383 - 63;
    if shift >= 0 {
        return Some((neg, if shift >= 64 { u128::MAX } else { (mant as u128) << shift }));
    }
    let s = (-shift) as u32;
    let (q, rem, half) = if s >= 128 {
        (0u128, 1u128, 2u128) // nonzero remainder, below a half
    } else {
        let m = mant as u128;
        let q = if s >= 64 { 0 } else { m >> s };
        let rem = if s >= 64 { m } else { m & ((1u128 << s) - 1) };
        (q, rem, 1u128 << (s - 1))
    };
    let up = match (rc >> 10) & 3 {
        0 => rem > half || (rem == half && q & 1 == 1),
        1 => neg && rem != 0, // toward -inf: magnitude up when negative
        2 => !neg && rem != 0,
        _ => false,
    };
    Some((neg, q + up as u128))
}

#[derive(Default)]
pub struct X87Check {
    /// Status-word bits not compared.
    status: u16,
    /// Control-word bit 6 not compared.
    cw6: bool,
    /// Any x87 instruction ran (FOP/FCS/FDS not compared).
    any: bool,
    /// FIP/FDP not compared (a control instruction ran last).
    fip: bool,
    /// Tag-word bits not compared.
    tag: u16,
    /// `status` when the last `fnstenv`/`fnsave` image was written: what
    /// `fldenv`/`frstor` load back from it.
    saved: u16,
    /// Bits of EAX that hold masked status bits (`fnstsw ax`).
    eax: u32,
    /// EFLAGS bits computed from masked state.
    flags: u32,
    /// Memory bytes holding masked bits: (address, bit mask).
    mem: Vec<(u32, u8)>,
    /// The stack model; `None` once it cannot follow.
    stack: Option<Stack>,
    saved_stack: Option<Stack>,
    /// A stack fault happened (predicted): registers, tags and the status
    /// but TOP are ours from here on.
    faulted: bool,
    /// The model's prediction for the instruction about to run, checked
    /// against ours afterwards.
    predicted: Option<bool>,
    /// Independent mismatches found while stepping.
    errors: Vec<String>,
}

impl X87Check {
    pub fn new(initial: &interp::X87) -> X87Check {
        X87Check { stack: Some(Stack::of(initial)), ..X87Check::default() }
    }

    /// Account for `insn`, about to run on `cpu`. `Err(class)` skips the
    /// fixture.
    pub fn before(&mut self, insn: &Instruction, cpu: &Cpu, memory: &mut dyn Memory) -> Result<(), String> {
        let m = insn.mnemonic();
        let ea_of = |base: Register, index: Register, scale: u32, disp: u32| {
            let r = |reg: Register| if reg == Register::None { 0 } else { cpu.reg(gpr_of(reg)) };
            r(base).wrapping_add(r(index).wrapping_mul(scale)).wrapping_add(disp)
        };
        let ea =
            || ea_of(insn.memory_base(), insn.memory_index(), insn.memory_index_scale(), insn.memory_displacement32());
        // Overwrites end masks: the flags it writes, the EAX bits and bytes
        // it writes outright or fully determines, the bytes it stores.
        let ah = (self.eax >> 8) & 0xff;
        let modified = eflags_of(insn.rflags_modified());
        self.flags &= !modified;
        // `and`/`or`/`test` with an immediate: the destination bits the
        // immediate decides (0 for and, 1 for or), and those the result
        // still takes from the destination.
        let imm = (insn.op_count() == 2
            && matches!(
                insn.op1_kind(),
                OpKind::Immediate8
                    | OpKind::Immediate16
                    | OpKind::Immediate32
                    | OpKind::Immediate8to16
                    | OpKind::Immediate8to32
            ))
        .then(|| insn.immediate(1) as u32);
        let (decided, kept) = match (m, imm) {
            (M::And, Some(i)) => (!i, i),
            (M::Or, Some(i)) => (i, !i),
            (M::Test, Some(i)) => (0, i),
            _ => (0, !0),
        };
        // What the destination's masked bits contribute to the result, for
        // the flags computed from it.
        let mut read_masked = false;
        // The destination's masked bits that reach the result, as bits of
        // the operand, when only the destination brings any.
        let mut result_masked = 0u32;
        let mut source_masked = false;
        // `xor r, r` / `sub r, r`: zero whatever r held.
        let zeroing = matches!(m, M::Xor | M::Sub)
            && insn.op0_kind() == OpKind::Register
            && insn.op1_kind() == OpKind::Register
            && insn.op0_register() == insn.op1_register();
        let info = InstructionInfoFactory::new().info(insn).clone();
        for u in info.used_registers() {
            let (bits, shift) = match u.register() {
                Register::EAX => (!0u32, 0),
                Register::AX => (0xffff, 0),
                Register::AL => (0xff, 0),
                Register::AH => (0xff00, 8),
                _ => continue,
            };
            let dest = insn.op0_kind() == OpKind::Register && insn.op0_register() == u.register();
            let (decided, kept) = if dest { ((decided << shift) & bits, (kept << shift) & bits) } else { (0, bits) };
            if !zeroing
                && matches!(u.access(), OpAccess::Read | OpAccess::ReadWrite | OpAccess::ReadCondWrite)
                && self.eax & kept != 0
            {
                read_masked = true;
                if dest {
                    result_masked |= (self.eax & kept) >> shift;
                } else {
                    source_masked = true;
                }
            }
            if zeroing && dest {
                self.eax &= !bits;
                continue;
            }
            match u.access() {
                OpAccess::Write => self.eax &= !bits,
                OpAccess::ReadWrite if dest && imm.is_some() => self.eax &= !decided,
                // Masked bits flow on into whatever the operation makes of
                // the register.
                OpAccess::ReadWrite | OpAccess::CondWrite | OpAccess::ReadCondWrite if self.eax & bits != 0 => {
                    self.eax |= bits
                }
                _ => {}
            }
        }
        for u in info.used_memory() {
            let a = ea_of(u.base(), u.index(), u.scale(), u.displacement() as u32);
            let n = u.memory_size().size() as u32;
            match u.access() {
                OpAccess::Write => self.mem.retain(|&(b, _)| b.wrapping_sub(a) >= n),
                OpAccess::ReadWrite | OpAccess::Read if imm.is_some() && insn.op0_kind() == OpKind::Memory => {
                    for (b, bits) in self.mem.iter_mut() {
                        let k = b.wrapping_sub(a);
                        if k < n {
                            let through = *bits & (kept >> (8 * k)) as u8;
                            if through != 0 {
                                read_masked = true;
                                result_masked |= (through as u32) << (8 * k);
                            }
                            if u.access() == OpAccess::ReadWrite {
                                *bits &= !((decided >> (8 * k)) as u8);
                            }
                        }
                    }
                    self.mem.retain(|&(_, bits)| bits != 0);
                }
                _ => {}
            }
        }
        // Flags computed from masked bits: `sahf` from AH bit for bit; a
        // logic op on its destination flag by flag, from where the masked
        // bits land in the result (ZF anywhere, SF the top bit, PF the low
        // byte; OF/CF are cleared); anything else wholesale.
        let logic = matches!(m, M::And | M::Or | M::Xor | M::Test);
        if m == M::Sahf {
            let map = [(0x01, 0x001), (0x04, 0x004), (0x10, 0x010), (0x40, 0x040), (0x80, 0x080)];
            self.flags |= map.iter().filter(|(b, _)| ah & b != 0).map(|(_, f)| f).sum::<u32>();
        } else if logic && read_masked && !source_masked {
            let width = match insn.op0_kind() {
                OpKind::Register => insn.op0_register().size(),
                _ => insn.memory_size().size(),
            } as u32
                * 8;
            let top = 1u32 << (width - 1);
            let f = [(result_masked != 0, 0x40), (result_masked & top != 0, 0x80), (result_masked & 0xff != 0, 0x04)];
            self.flags |= modified & f.iter().filter(|(on, _)| *on).map(|(_, b)| b).sum::<u32>();
        } else if read_masked {
            self.flags |= if logic { modified & 0x00c4 } else { modified }; // SF ZF PF
        }
        self.predicted = None;
        if !lift_x87(m) {
            return Ok(());
        }
        let name = format!("{m:?}").to_ascii_lowercase();
        if matches!(
            m,
            M::Fsin
                | M::Fcos
                | M::Fsincos
                | M::Fptan
                | M::Fpatan
                | M::F2xm1
                | M::Fyl2x
                | M::Fyl2xp1
                | M::Fprem
                | M::Fprem1
        ) {
            return Err(format!("x87: {name} is host libm in Unicorn (needs-hardware)"));
        }
        self.any = true;

        // The stack model first: a fault changes what the rest compares.
        let known = match stack_use(insn) {
            Some(Use::Restore) if self.saved_stack.is_none() => None,
            u => u,
        };
        let fault = match (self.stack.as_mut(), known) {
            (Some(s), Some(u)) => match s.step(&u, &mut self.saved_stack) {
                Ok(touched) => {
                    for r in touched {
                        self.tag &= !(3 << (2 * r));
                    }
                    false
                }
                Err(()) => true,
            },
            _ => {
                self.stack = None;
                false
            }
        };
        if self.stack.is_some() {
            self.predicted = Some(fault);
        }
        if fault {
            self.faulted = true;
            self.stack = None;
        }

        let mem = insn.op_count() > 0 && insn.op0_kind() == OpKind::Memory;
        if self.faulted && mem && matches!(m, M::Fst | M::Fstp | M::Fist | M::Fistp | M::Fbstp) {
            let a = ea();
            self.mem.extend((0..insn.memory_size().size() as u32).map(|o| (a + o, 0xff)));
        }
        if self.faulted && matches!(m, M::Fcomi | M::Fcomip | M::Fucomi | M::Fucomip) {
            self.flags |= EFL_COMPARE;
        }
        let status = if self.faulted { !SW_TOP } else { self.status };
        let lo = status as u8;
        let hi = (status >> 8) as u8;
        let cw = if self.cw6 { 0x40 } else { 0 };
        match m {
            M::Fnstsw if mem => self.mem.extend([(ea(), lo), (ea() + 1, hi)]),
            M::Fnstsw => self.eax |= status as u32,
            M::Fnstcw => self.mem.push((ea(), cw)),
            M::Fnstenv | M::Fnsave => {
                self.saved = status;
                let a = ea();
                self.mem.extend([(a, cw), (a + 4, lo), (a + 5, hi)]);
                self.mem.extend([2, 3, 6, 7, 10, 11, 16, 17, 18, 19, 24, 25, 26, 27].map(|o| (a + o, 0xff)));
                if self.fip {
                    self.mem.extend([12, 13, 14, 15, 20, 21, 22, 23].map(|o| (a + o, 0xff)));
                }
                if self.faulted {
                    // The tag word and the registers.
                    self.mem.extend([8, 9].map(|o| (a + o, 0xff)));
                    if m == M::Fnsave {
                        self.mem.extend((28..108).map(|o| (a + o, 0xff)));
                    }
                }
            }
            M::Fld if mem && matches!(insn.memory_size(), MemorySize::Float32 | MemorySize::Float64) => {
                let n = insn.memory_size().size();
                let mut b = [0u8; 8];
                if memory.read(ea(), &mut b[..n]).is_ok() {
                    let v = u64::from_le_bytes(b);
                    let snan = if n == 4 {
                        (v >> 23) & 0xff == 0xff && v & 0x7f_ffff != 0 && v & 0x40_0000 == 0
                    } else {
                        (v >> 52) & 0x7ff == 0x7ff && v & ((1 << 52) - 1) != 0 && v & (1 << 51) == 0
                    };
                    if snan {
                        return Err("x87: fld of a signalling NaN, which Unicorn does not quiet".into());
                    }
                }
            }
            M::Fbstp if !self.faulted => {
                let top = ((cpu.x87.status >> 11) & 7) as usize;
                match bcd_rounded(cpu.x87.st[top], cpu.x87.control) {
                    Some((_, n)) if n < 1_000_000_000_000_000_000 => {
                        if n == 0 {
                            // A rounded zero keeps its sign on a P6.
                            self.mem.push((ea() + 9, 0x80));
                        }
                    }
                    _ => return Err("x87: fbstp out of range, where Unicorn stores garbage digits".into()),
                }
            }
            M::Fst | M::Fstp if insn.op0_kind() == OpKind::Register => {
                let top = (cpu.x87.status >> 11) & 7;
                let i = insn.op0_register() as u16 - Register::ST0 as u16;
                self.tag |= 3 << (2 * ((top + i) & 7));
            }
            _ => {}
        }
        if matches!(m, M::Fcomi | M::Fcomip | M::Fucomi | M::Fucomip) {
            self.flags |= 0x890; // OF SF AF
        }
        self.fip = matches!(
            m,
            M::Fnstsw | M::Fnstcw | M::Fnstenv | M::Wait | M::Fnop | M::Fldcw | M::Fnclex | M::Fldenv | M::Frstor
        ) || (self.fip && matches!(m, M::Fninit | M::Fnsave));
        match m {
            // Status reads and stores, waits: nothing changes.
            M::Fnstsw | M::Fnstcw | M::Fnstenv | M::Wait | M::Fnop => {}
            // The whole status word is defined again.
            M::Fninit | M::Fnsave => {
                self.status = 0;
                self.faulted = false;
                self.tag = 0;
                if self.stack.is_none() && m == M::Fninit {
                    self.stack = Some(Stack { top: 0, empty: [true; 8] });
                }
            }
            M::Fldenv | M::Frstor => self.status = self.saved,
            M::Fnclex => self.status = (self.status & !SW_STICKY) | SW_C1 | SW_C023,
            // C0..C3 defined (C1 = 0 or the sign).
            M::Fcom
            | M::Fcomp
            | M::Fcompp
            | M::Fucom
            | M::Fucomp
            | M::Fucompp
            | M::Ficom
            | M::Ficomp
            | M::Ftst
            | M::Fxam => {
                self.status = (self.status & !(SW_C023 | SW_C1)) | SW_STICKY;
            }
            // The EFLAGS compares leave C0/C2/C3 alone.
            M::Fcomi | M::Fcomip | M::Fucomi | M::Fucomip => self.status |= SW_STICKY | SW_C1,
            _ => self.status |= SW_STICKY | SW_C1 | SW_C023,
        }
        if matches!(m, M::Fldcw | M::Fldenv | M::Frstor) {
            self.cw6 = true;
        }
        if m == M::Fninit || m == M::Fnsave {
            self.cw6 = false;
        }
        Ok(())
    }

    /// After `insn` ran: our stack fault (a new SF) must be the model's.
    /// `Err(class)` skips the fixture when the model could not say.
    pub fn after(&mut self, insn: &Instruction, before: &interp::X87, after: &interp::X87) -> Result<(), String> {
        if !lift_x87(insn.mnemonic()) || before.status & SW_SF != 0 {
            return Ok(());
        }
        let ours = after.status & SW_SF != 0;
        match self.predicted {
            Some(p) if p != ours => self.errors.push(format!(
                "stack fault at {:#x} ({}): model {p}, ours {ours}",
                insn.ip32(),
                format!("{:?}", insn.mnemonic()).to_ascii_lowercase()
            )),
            None if ours => return Err("x87: stack fault after an instruction the stack model does not follow".into()),
            _ => {}
        }
        Ok(())
    }

    /// Mismatches found while stepping.
    pub fn errors(&self) -> &[String] {
        &self.errors
    }

    /// Memory bytes holding masked bits: (address, bit mask).
    pub fn mem(&self) -> &[(u32, u8)] {
        &self.mem
    }

    /// Take the masked bits of the expected state from ours.
    pub fn apply(&self, ours: &Cpu, want: &mut interp::X87, want_eax: &mut u32, want_flags: &mut u32) {
        *want_eax = (*want_eax & !self.eax) | (ours.gpr[0] & self.eax);
        *want_flags = (*want_flags & !self.flags) | (ours.eflags & self.flags);
        if !self.any {
            return;
        }
        let o = &ours.x87;
        let status = if self.faulted { !SW_TOP } else { self.status };
        want.status = (want.status & !status) | (o.status & status);
        if self.cw6 {
            want.control = (want.control & !0x40) | (o.control & 0x40);
        }
        want.opcode = o.opcode;
        want.instruction_selector = o.instruction_selector;
        want.data_selector = o.data_selector;
        if self.fip {
            want.instruction_pointer = o.instruction_pointer;
            want.data_pointer = o.data_pointer;
        }
        want.tag = (want.tag & !self.tag) | (o.tag & self.tag);
        if self.faulted {
            want.st = o.st;
            want.tag = o.tag;
        }
    }
}

/// iced's flag bits as EFLAGS bits.
fn eflags_of(r: u32) -> u32 {
    use iced_x86::RflagsBits as F;
    [(F::OF, 0x800), (F::SF, 0x80), (F::ZF, 0x40), (F::AF, 0x10), (F::CF, 0x01), (F::PF, 0x04)]
        .iter()
        .filter(|(f, _)| r & f != 0)
        .map(|(_, e)| e)
        .sum()
}

/// Whether the lifter lowers `m` as x87 (`lift`'s own list is private).
fn lift_x87(m: M) -> bool {
    let n = format!("{m:?}");
    n.starts_with('F') || n == "Wait"
}

fn gpr_of(r: Register) -> Gpr {
    match r {
        Register::EAX => Gpr::Eax,
        Register::ECX => Gpr::Ecx,
        Register::EDX => Gpr::Edx,
        Register::EBX => Gpr::Ebx,
        Register::ESP => Gpr::Esp,
        Register::EBP => Gpr::Ebp,
        Register::ESI => Gpr::Esi,
        Register::EDI => Gpr::Edi,
        r => panic!("operand addressed through {r:?}"),
    }
}

mod tests {
    use super::*;

    fn f80(neg: bool, exp: u16, mant: u64) -> [u8; 10] {
        let mut r = [0u8; 10];
        r[..8].copy_from_slice(&mant.to_le_bytes());
        r[8..].copy_from_slice(&(exp | (neg as u16) << 15).to_le_bytes());
        r
    }

    #[test]
    fn bcd_rounding_finds_the_stored_integer() {
        const ONE: u64 = 1 << 63;
        // 2^59 fits 18 digits; 10^18 does not.
        assert_eq!(bcd_rounded(f80(false, 16383 + 59, ONE), 0), Some((false, 1 << 59)));
        let e18 = 1_000_000_000_000_000_000u128;
        let lz = (e18 as u64).leading_zeros();
        let r = f80(false, 16383 + 63 - lz as u16, (e18 as u64) << lz);
        assert_eq!(bcd_rounded(r, 0), Some((false, e18)));
        // -0.75: -1 to nearest, -0 chopped; -0.25 to nearest is -0.
        let m075 = f80(true, 16382, 0xc000_0000_0000_0000);
        assert_eq!(bcd_rounded(m075, 0x000), Some((true, 1)));
        assert_eq!(bcd_rounded(m075, 0xc00), Some((true, 0)));
        assert_eq!(bcd_rounded(f80(true, 16381, ONE), 0x000), Some((true, 0)));
        // Ties to even, and directed rounding of negatives.
        assert_eq!(bcd_rounded(f80(false, 16383 + 1, 0xa000_0000_0000_0000), 0), Some((false, 2))); // 2.5
        assert_eq!(bcd_rounded(f80(true, 16382, ONE), 0x400), Some((true, 1))); // -0.5 down
        assert_eq!(bcd_rounded(f80(true, 16382, ONE), 0x800), Some((true, 0))); // -0.5 up
        assert_eq!(bcd_rounded(f80(false, 0x7fff, ONE), 0), None);
    }
}
