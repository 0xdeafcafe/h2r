//! Cross-check the lowering against an independent CPU oracle: JSON-lines
//! fixtures (one instruction sequence, initial state and memory, and the
//! machine's outcome each) produced by the oracle tool from synthetic
//! encodings, never from game data.
//!
//! The corpus test is feature-gated (`--features oracle-fixtures`) and
//! driven by `H2_ORACLE_FIXTURES`, a `:`-separated list of `.jsonl` files.
//! The harness itself is checked always, by the self-contained tests at the
//! end (hand-written fixtures, not machine observations). Each
//! fixture runs through `lift::lower` and the IR interpreter from the
//! fixture's full initial state (GPRs, EFLAGS, XMM, x87, MXCSR); registers,
//! EIP, XMM, x87, MXCSR, memory and the way the run stopped must match.
//! Status flags the oracle defines are compared whatever we claim, and
//! which flags are defined must itself agree: our defined mask may neither
//! hide a flag the oracle defines nor define one it leaves undefined.
//! Fixtures the oracle refused, or that lower to an unsupported
//! instruction, are counted and skipped, by class (printed at the end).

use std::collections::{BTreeMap, HashMap};

use iced_x86::{Decoder, DecoderOptions, MemorySize};
use ir::interp::{self, Cpu, Exit, MemFault, Memory};
use ir::{Fault, Gpr};
use serde_json::Value;

/// Where the oracle places the code; its page is mapped read-only.
const CODE_ADDRESS: u32 = 0x10_0000;
const STATUS_FLAGS: u32 = 0x0cd5;

/// 4 KiB pages: what the oracle maps, nothing else.
struct PageMem {
    pages: HashMap<u32, Vec<u8>>,
}

impl PageMem {
    fn mapped(&self, a: u32) -> bool {
        self.pages.contains_key(&(a & !0xfff))
    }

    fn byte(&mut self, a: u32) -> &mut u8 {
        &mut self.pages.get_mut(&(a & !0xfff)).unwrap()[(a & 0xfff) as usize]
    }
}

impl Memory for PageMem {
    fn read(&mut self, addr: u32, buf: &mut [u8]) -> Result<(), MemFault> {
        if let Some(a) = (0..buf.len() as u32).map(|i| addr.wrapping_add(i)).find(|a| !self.mapped(*a)) {
            return Err(MemFault { addr: a, write: false });
        }
        for (i, b) in buf.iter_mut().enumerate() {
            *b = *self.byte(addr.wrapping_add(i as u32));
        }
        Ok(())
    }

    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), MemFault> {
        let bad = |a: u32| !self.mapped(a) || (a & !0xfff) == CODE_ADDRESS;
        if let Some(a) = (0..data.len() as u32).map(|i| addr.wrapping_add(i)).find(|a| bad(*a)) {
            return Err(MemFault { addr: a, write: true });
        }
        for (i, b) in data.iter().enumerate() {
            *self.byte(addr.wrapping_add(i as u32)) = *b;
        }
        Ok(())
    }
}

fn u32_of(v: &Value) -> u32 {
    v.as_u64().expect("number") as u32
}

fn bytes_of(v: &Value) -> Vec<u8> {
    v.as_array().expect("byte array").iter().map(|b| u32_of(b) as u8).collect()
}

fn gprs(s: &Value) -> [u32; 8] {
    ["eax", "ecx", "edx", "ebx", "esp", "ebp", "esi", "edi"].map(|r| u32_of(&s[r]))
}

fn x87_of(s: &Value) -> interp::X87 {
    let x = &s["x87"];
    // The oracle lists logical ST(0)..ST(7); the interpreter keeps physical
    // registers (MMX register N is physical N). ST(i) is physical TOP + i.
    let top = (u32_of(&x["status"]) >> 11) & 7;
    let mut st = [[0u8; 10]; 8];
    for (i, r) in x["st"].as_array().unwrap().iter().enumerate() {
        st[(top as usize + i) & 7] = bytes_of(r).try_into().unwrap();
    }
    interp::X87 {
        st,
        control: u32_of(&x["control"]) as u16,
        status: u32_of(&x["status"]) as u16,
        tag: u32_of(&x["tag"]) as u16,
        opcode: u32_of(&x["opcode"]) as u16,
        instruction_pointer: u32_of(&x["instruction_pointer"]),
        data_pointer: u32_of(&x["data_pointer"]),
        instruction_selector: u32_of(&x["code_segment"]) as u16,
        data_selector: u32_of(&x["data_segment"]) as u16,
    }
}

/// `rcpss`/`rcpps`/`rsqrtss`/`rsqrtps` are approximations whose exact bits
/// are the CPU's own tables: neither the oracle (QEMU computes the exact
/// quotient) nor the reference semantics claim the Pentium III's. Their
/// result lanes are compared against the architectural guarantee instead,
/// relative error at most 1.5 * 2^-12 (SDM vol. 2, RCPSS/RSQRTSS), with
/// specials exact. A denormal input is treated as a zero of its sign (the
/// SDM's rule, which QEMU does not follow), so there the result must be
/// exactly what that zero gives. Lanes the instruction does not compute
/// are compared exactly.
struct Approx {
    reg: usize,
    rsqrt: bool,
    /// (lane, input bits) of the computed lanes.
    lanes: Vec<(u32, u32)>,
}

impl Approx {
    fn of(insn: &iced_x86::Instruction, cpu: &Cpu, mem: &mut PageMem) -> Option<Approx> {
        use iced_x86::{Mnemonic as M, OpKind, Register};
        let (rsqrt, scalar) = match insn.mnemonic() {
            M::Rcpss => (false, true),
            M::Rcpps => (false, false),
            M::Rsqrtss => (true, true),
            M::Rsqrtps => (true, false),
            _ => return None,
        };
        let reg = (insn.op_register(0) as usize).checked_sub(Register::XMM0 as usize)?;
        let n = if scalar { 1 } else { 4 };
        let src: Vec<u32> = if insn.op_kind(1) == OpKind::Memory {
            let mut a = insn.memory_displacement32();
            for r in [insn.memory_base(), insn.memory_index()] {
                if let Some((g, _)) = lift::lower::gpr(r) {
                    let scale = if r == insn.memory_index() { insn.memory_index_scale() } else { 1 };
                    a = a.wrapping_add(cpu.gpr[g as usize].wrapping_mul(scale));
                }
            }
            let mut buf = [0u8; 16];
            mem.read(a, &mut buf[..4 * n]).ok()?;
            (0..n).map(|i| u32::from_le_bytes(buf[4 * i..4 * i + 4].try_into().unwrap())).collect()
        } else {
            let x = cpu.xmm[(insn.op_register(1) as usize).checked_sub(Register::XMM0 as usize)?];
            (0..n).map(|i| (x >> (32 * i)) as u32).collect()
        };
        Some(Approx { reg, rsqrt, lanes: src.into_iter().enumerate().map(|(i, v)| (i as u32, v)).collect() })
    }

    fn lane_ok(&self, ours: u32, theirs: u32, input: u32) -> bool {
        if input & 0x7f80_0000 == 0 && input & 0x007f_ffff != 0 {
            // Denormal: as a zero of the input's sign, which gives an
            // infinity of that sign for both operations.
            return ours == (input & 0x8000_0000) | 0x7f80_0000;
        }
        if ours == theirs {
            return true;
        }
        // A tiny result is flushed to a zero of the input's sign (SDM: large
        // inputs "underflow to 0.0"); QEMU returns the denormal.
        if theirs & 0x7f80_0000 == 0 && ours == input & 0x8000_0000 && ours & 0x8000_0000 == theirs & 0x8000_0000 {
            return true;
        }
        let (a, b) = (f32::from_bits(ours), f32::from_bits(theirs));
        if !(a.is_finite() && b.is_finite()) || a == 0.0 || b == 0.0 || a.is_sign_negative() != b.is_sign_negative() {
            return false;
        }
        let _ = self.rsqrt;
        ((a as f64 - b as f64) / b as f64).abs() <= 1.5 * (2f64).powi(-12)
    }

    fn matches(&self, ours: u128, theirs: u128) -> bool {
        (0..4).all(|i| {
            let (o, t) = ((ours >> (32 * i)) as u32, (theirs >> (32 * i)) as u32);
            match self.lanes.iter().find(|(l, _)| *l == i) {
                Some((_, input)) => self.lane_ok(o, t, *input),
                None => o == t,
            }
        })
    }
}

/// Unicorn (QEMU) departs from the SDM on two MMX side effects, and the
/// expected state is adjusted for exactly these after an MMX transition:
///
/// - an MMX write leaves bits 64-79 of the register as they were; hardware
///   sets them to all ones (SDM vol. 1, 9.5.1);
/// - its tag word is computed from register contents (zero 01, special 10),
///   as `fnstenv` would report it, where the state here keeps the tags the
///   MMX transition writes: valid (00) for every register.
///
/// Anything else in the x87 state is still compared exactly, and an empty
/// tag (11) in the oracle's word is never accepted for an all-valid one.
fn unicorn_mmx(ours: &interp::X87, want: &mut interp::X87, initial: &interp::X87) {
    for i in 0..8 {
        if ours.st[i][8..] == [0xff, 0xff] && want.st[i][8..] == initial.st[i][8..] {
            want.st[i][8] = 0xff;
            want.st[i][9] = 0xff;
        }
    }
    if ours.tag == 0 && (0..8).all(|i| (want.tag >> (2 * i)) & 3 != 3) {
        want.tag = 0;
    }
}

/// What Unicorn cannot answer about x87 state, masked by mnemonic as
/// docs/FLOAT.md §2.9 and docs/FLOAT-ABI.md §6.3 set out; `crates/x87` is
/// the authority for these bits, and they are taken from our side:
///
/// - the sticky exception flags but ZE (IE/DE/OE/UE/PE/SF), which Unicorn
///   mostly does not set, from the first x87 operation until something
///   defines them again (`fninit`, `fnclex`, `fnsave`; `fldenv`/`frstor`
///   load whatever was masked in the image they read);
/// - C1, and C0/C2/C3 after an instruction that leaves them undefined
///   (loads, stores, arithmetic, stack ops, `fldcw`), until one defines
///   them (the compares, `ftst`, `fxam`, the initialising ones);
/// - control-word bit 6 after `fldcw`/`fldenv`/`frstor` (a P6 forces it to
///   1, Unicorn stores the raw word; `needs-hardware`);
/// - FOP, FCS and FDS, which Unicorn does not keep. FIP and FDP it keeps
///   for the data instructions, and they are compared, but it also moves
///   FIP on the control instructions (`fnstsw`, `fwait`, ...), not on
///   `fnop`, and `frstor`/`fldenv` do not load them: after one of those
///   they are masked until the next data instruction;
/// - the reserved upper halves of the `fnstenv`/`fnsave` control, status,
///   tag and FDS words: the crate writes 0xffff as a P6 does, Unicorn 0
///   (`needs-hardware`), and the sign of an `fbstp` result that may round
///   to a BCD -0, which Unicorn drops;
/// - OF/SF/AF after `fcomi` and its forms, which Unicorn leaves alone
///   where the SDM clears them;
/// - the tag of the destination of `fst`/`fstp st(i)`, which Unicorn does
///   not update.
///
/// Masked status bits also mask what reads them: `fnstsw ax` (and the
/// flags `sahf` or `test ah` then make), `fnstsw m16`, `fnstcw m16` and the
/// `fnstenv`/`fnsave` image. Fixtures with transcendentals (Unicorn uses
/// host libm), a stack fault (Unicorn has no tag checks), an `fld` of a
/// signalling NaN (Unicorn does not quiet it) or an out-of-range `fbstp`
/// (Unicorn stores garbage digits, not the BCD indefinite) are skipped by
/// class instead.
#[derive(Default)]
struct X87Masks {
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
    /// EFLAGS bits computed from masked status bits.
    flags: u32,
    /// Memory bytes holding masked bits: (address, bit mask).
    mem: Vec<(u32, u8)>,
}

const SW_STICKY: u16 = 0x007b; // IE DE OE UE PE SF
const SW_C1: u16 = 0x0200;
const SW_C023: u16 = 0x4500;

impl X87Masks {
    /// Account for `insn`, about to run on `cpu`. `Err(class)` skips the
    /// fixture.
    fn before(&mut self, insn: &iced_x86::Instruction, cpu: &Cpu, memory: &mut PageMem) -> Result<(), String> {
        use iced_x86::Mnemonic as M;
        let m = insn.mnemonic();
        // AH holding masked bits, read back into EFLAGS.
        let ah = (self.eax >> 8) & 0xff;
        match m {
            M::Sahf if ah != 0 => {
                let map = [(0x01, 0x001), (0x04, 0x004), (0x10, 0x010), (0x40, 0x040), (0x80, 0x080)];
                self.flags |= map.iter().filter(|(b, _)| ah & b != 0).map(|(_, f)| f).sum::<u32>();
            }
            M::Test
                if insn.op0_kind() == iced_x86::OpKind::Register
                    && insn.op0_register() == iced_x86::Register::AH
                    && insn.immediate8() as u32 & ah != 0 =>
            {
                self.flags |= 0x0c4; // ZF SF PF
            }
            _ => {}
        }
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
        let ea = || {
            let r = |reg: iced_x86::Register| {
                if reg == iced_x86::Register::None { 0 } else { cpu.reg(gpr_of(reg)) }
            };
            r(insn.memory_base())
                .wrapping_add(r(insn.memory_index()).wrapping_mul(insn.memory_index_scale()))
                .wrapping_add(insn.memory_displacement32())
        };
        let lo = self.status as u8;
        let hi = (self.status >> 8) as u8;
        let cw = if self.cw6 { 0x40 } else { 0 };
        let mem = insn.op_count() > 0 && insn.op0_kind() == iced_x86::OpKind::Memory;
        match m {
            M::Fnstsw if mem => self.mem.extend([(ea(), lo), (ea() + 1, hi)]),
            M::Fnstsw => self.eax |= self.status as u32,
            M::Fnstcw => self.mem.push((ea(), cw)),
            M::Fnstenv | M::Fnsave => {
                self.saved = self.status;
                let a = ea();
                self.mem.extend([(a, cw), (a + 4, lo), (a + 5, hi)]);
                self.mem.extend([2, 3, 6, 7, 10, 11, 16, 17, 18, 19, 24, 25, 26, 27].map(|o| (a + o, 0xff)));
                if self.fip {
                    self.mem.extend([12, 13, 14, 15, 20, 21, 22, 23].map(|o| (a + o, 0xff)));
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
            M::Fbstp => {
                // |ST(0)| >= 10^18 (exponent 2^59 and up is always out of
                // range; the check is conservative below it), NaN or
                // infinity: the BCD indefinite on a P6, garbage in Unicorn.
                let top = ((cpu.x87.status >> 11) & 7) as usize;
                let r = cpu.x87.st[top];
                let exp = u16::from_le_bytes([r[8], r[9]]) & 0x7fff;
                if exp >= 0x3fff + 59 {
                    return Err("x87: fbstp out of range, where Unicorn stores garbage digits".into());
                }
                // A negative value under 1 may round to a BCD -0, whose
                // sign Unicorn drops (needs-hardware).
                if r[9] & 0x80 != 0 && exp < 0x3fff {
                    self.mem.push((ea() + 9, 0x80));
                }
            }
            M::Fst | M::Fstp if insn.op0_kind() == iced_x86::OpKind::Register => {
                let top = (cpu.x87.status >> 11) & 7;
                let i = insn.op0_register().number() as u16;
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
            M::Fninit | M::Fnsave => self.status = 0,
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

    /// Take the masked bits of the expected state from ours.
    fn apply(&self, ours: &Cpu, want: &mut interp::X87, want_eax: &mut u32, want_flags: &mut u32) {
        if !self.any {
            return;
        }
        let o = &ours.x87;
        want.status = (want.status & !self.status) | (o.status & self.status);
        if self.cw6 {
            want.control = (want.control & !0x40) | (o.control & 0x40);
        }
        want.opcode = o.opcode;
        if self.fip {
            want.instruction_pointer = o.instruction_pointer;
            want.data_pointer = o.data_pointer;
        }
        want.tag = (want.tag & !self.tag) | (o.tag & self.tag);
        want.instruction_selector = o.instruction_selector;
        want.data_selector = o.data_selector;
        *want_eax = (*want_eax & !self.eax) | (ours.gpr[0] & self.eax);
        *want_flags = (*want_flags & !self.flags) | (ours.eflags & self.flags);
    }
}

/// Whether the lifter lowers `m` as x87 (`lift`'s own list is private).
fn lift_x87(m: iced_x86::Mnemonic) -> bool {
    let n = format!("{m:?}");
    n.starts_with('F') || n == "Wait"
}

fn gpr_of(r: iced_x86::Register) -> Gpr {
    use iced_x86::Register as R;
    match r {
        R::EAX => Gpr::Eax,
        R::ECX => Gpr::Ecx,
        R::EDX => Gpr::Edx,
        R::EBX => Gpr::Ebx,
        R::ESP => Gpr::Esp,
        R::EBP => Gpr::Ebp,
        R::ESI => Gpr::Esi,
        R::EDI => Gpr::Edi,
        r => panic!("x87 operand addressed through {r:?}"),
    }
}

/// Why a fixture was skipped, for the summary.
fn skip_class(insn: &iced_x86::Instruction, detail: &str) -> String {
    let m = format!("{:?}", insn.mnemonic()).to_ascii_lowercase();
    let w = if insn.op_count() > 0 && insn.op_kind(0) == iced_x86::OpKind::Register {
        insn.op_register(0).size() * 8
    } else {
        insn.memory_size().size() * 8
    };
    format!("{m}/{w}: {detail}")
}

fn seed_pages(memory: &Value) -> HashMap<u32, Vec<u8>> {
    let mut pages: HashMap<u32, Vec<u8>> = HashMap::new();
    for m in memory.as_array().unwrap() {
        let (addr, bytes) = (u32_of(&m[0]), bytes_of(&m[1]));
        for (i, b) in bytes.iter().enumerate() {
            let a = addr.wrapping_add(i as u32);
            pages.entry(a & !0xfff).or_insert_with(|| vec![0; 4096])[(a & 0xfff) as usize] = *b;
        }
    }
    pages
}

/// The oracle's stop, in the same words as ours; `None` if it refused.
fn oracle_stop(stop: &Value) -> Option<String> {
    if stop == "Completed" {
        return Some("completed".into());
    }
    let fault = stop.get("Fault")?;
    Some(match fault {
        Value::String(s) if s == "DivideError" => "de".into(),
        Value::String(s) if s == "InvalidOpcode" => "ud".into(),
        Value::Object(o) if o.contains_key("PageFault") => {
            let write = o["PageFault"]["access"] == "Write";
            format!("pf-{}", if write { "w" } else { "r" })
        }
        other => format!("{other}"),
    })
}

/// Run one fixture; `Ok(Err(class))` when it is skipped.
fn check(f: &Value) -> Result<Result<(), String>, String> {
    let Some(theirs) = oracle_stop(&f["outcome"]["stop"]) else {
        return Ok(Err(format!("oracle refused: {}", f["outcome"]["stop"])));
    };
    let code = bytes_of(&f["bytes"]);
    let s = &f["initial"];
    let mut cpu = Cpu {
        gpr: gprs(s),
        eflags: u32_of(&s["eflags"]),
        eip: u32_of(&s["eip"]),
        x87: x87_of(s),
        mxcsr: u32_of(&s["mxcsr"]),
        ..Cpu::default()
    };
    for (i, x) in s["xmm"].as_array().unwrap().iter().enumerate() {
        cpu.xmm[i] = u128::from_le_bytes(bytes_of(x).try_into().unwrap());
    }
    let mut mem = PageMem { pages: seed_pages(&f["memory"]) };
    let mut page = vec![0u8; 4096];
    page[..code.len()].copy_from_slice(&code);
    mem.pages.insert(CODE_ADDRESS, page);

    let initial_x87 = cpu.x87.clone();
    let start = cpu.eip;
    let end = start + code.len() as u32;
    let mut result = Ok(());
    let mut mmx = false;
    let mut sse_fp = false;
    let mut approx: Option<Approx> = None;
    let mut x87 = X87Masks::default();
    for _ in 0..64 {
        if cpu.eip < start || cpu.eip >= end {
            break;
        }
        let at = (cpu.eip - start) as usize;
        let insn = Decoder::with_ip(32, &code[at..], cpu.eip as u64, DecoderOptions::NONE).decode();
        let ir = lift::lower(&insn, &lift::Env::default(), lift::Site::Plain);
        ir.validate().map_err(|e| format!("invalid IR: {e}"))?;
        if let Some((_, detail)) = ir.unsupported() {
            return Ok(Err(skip_class(&insn, detail)));
        }
        // Whether the x87 state may show the MMX transition (see
        // unicorn_mmx; the oracle's tag word still has to agree).
        mmx |= ir.stmts.iter().any(|s| matches!(s, ir::Stmt::Intrinsic { op: ir::Intrinsic::MmxEnter, .. }));
        sse_fp |= ir.stmts.iter().any(|s| matches!(s, ir::Stmt::Intrinsic { op: ir::Intrinsic::Sse(_), .. }));
        if let Some(a) = Approx::of(&insn, &cpu, &mut mem) {
            approx = Some(a);
        }
        if let Err(class) = x87.before(&insn, &cpu, &mut mem) {
            return Ok(Err(class));
        }
        if let Err(e) = interp::step(&mut cpu, &mut mem, &ir) {
            result = Err(e);
            break;
        }
    }
    if x87.any && cpu.x87.status & 0x0040 != 0 {
        return Ok(Err("x87: stack fault, which Unicorn does not model (no tag checks)".into()));
    }
    let ours = match &result {
        Ok(()) => "completed".to_string(),
        Err(Exit::Fault { fault: Fault::DivideError, .. }) => "de".into(),
        Err(Exit::Fault { fault: Fault::InvalidOpcode, .. }) => "ud".into(),
        Err(Exit::Memory { fault, .. }) => format!("pf-{}", if fault.write { "w" } else { "r" }),
        Err(e) => format!("{e:?}"),
    };
    let mut errs = Vec::new();
    if ours != theirs {
        errs.push(format!("stop: ours {ours}, oracle {theirs}"));
    }
    let o = &f["outcome"];
    let e = &o["state"];
    let mut want = gprs(e);
    let mut want_x87 = x87_of(e);
    let mut want_eflags = u32_of(&e["eflags"]);
    x87.apply(&cpu, &mut want_x87, &mut want[0], &mut want_eflags);
    let masks: Vec<u32> = o["defined_gprs"].as_array().unwrap().iter().map(u32_of).collect();
    for (i, g) in Gpr::ALL.iter().enumerate() {
        if (cpu.gpr[i] ^ want[i]) & masks[i] != 0 {
            errs.push(format!("{g:?}: ours {:#x}, oracle {:#x}", cpu.gpr[i], want[i]));
        }
    }
    if cpu.eip != u32_of(&e["eip"]) {
        errs.push(format!("eip: ours {:#x}, oracle {:#x}", cpu.eip, u32_of(&e["eip"])));
    }
    // Values: every status flag the oracle defines, whatever we claim.
    let theirs_defined = u32_of(&o["defined_flags"]) & STATUS_FLAGS;
    if (cpu.eflags ^ want_eflags) & theirs_defined != 0 {
        errs.push(format!("eflags: ours {:#x}, oracle {want_eflags:#x}, compared {theirs_defined:#x}", cpu.eflags));
    }
    // Validity, independently: the same status flags defined on both sides.
    let ours_defined = cpu.flags_valid & STATUS_FLAGS;
    if ours_defined != theirs_defined {
        errs.push(format!("defined flags: ours {ours_defined:#x}, oracle {theirs_defined:#x}"));
    }
    // Unicorn never sets MXCSR's exception flags (bits 0-5): after SSE
    // arithmetic only the control bits are compared here. The flags are
    // checked against hardware by crates/simd's own oracle corpus.
    let flags_unmodelled = if sse_fp { 0x3f } else { 0 };
    if (cpu.mxcsr ^ u32_of(&e["mxcsr"])) & !flags_unmodelled != 0 {
        errs.push(format!("mxcsr: ours {:#x}, oracle {:#x}", cpu.mxcsr, u32_of(&e["mxcsr"])));
    }
    if mmx {
        unicorn_mmx(&cpu.x87, &mut want_x87, &initial_x87);
    }
    if cpu.x87 != want_x87 {
        errs.push(format!("x87: ours {:x?}, oracle {want_x87:x?}", cpu.x87));
    }
    for (i, x) in e["xmm"].as_array().unwrap().iter().enumerate() {
        let want = u128::from_le_bytes(bytes_of(x).try_into().unwrap());
        let ok = match &approx {
            Some(a) if a.reg == i => a.matches(cpu.xmm[i], want),
            _ => cpu.xmm[i] == want,
        };
        if !ok {
            errs.push(format!("xmm{i} differs: ours {:#034x}, oracle {want:#034x}", cpu.xmm[i]));
        }
    }
    let mut want_pages = seed_pages(&f["memory"]);
    for w in o["writes"].as_array().unwrap() {
        let addr = u32_of(&w["address"]);
        for (i, b) in bytes_of(&w["bytes"]).iter().enumerate() {
            let a = addr.wrapping_add(i as u32);
            if let Some(p) = want_pages.get_mut(&(a & !0xfff)) {
                p[(a & 0xfff) as usize] = *b;
            }
        }
    }
    for &(a, bits) in &x87.mem {
        if let Some(p) = want_pages.get_mut(&(a & !0xfff)) {
            let i = (a & 0xfff) as usize;
            p[i] = (p[i] & !bits) | (mem.pages[&(a & !0xfff)][i] & bits);
        }
    }
    for (base, page) in &want_pages {
        if let Some(i) = (0..4096).find(|&i| mem.pages[base][i] != page[i]) {
            errs.push(format!("mem[{:#x}] differs", base + i as u32));
        }
    }
    if errs.is_empty() { Ok(Ok(())) } else { Err(errs.join("; ")) }
}

/// Outcomes over a set of fixtures: matches, skips by class, and the
/// mismatching cases by encoding name.
#[derive(Default)]
struct Tally {
    matched: usize,
    skipped: usize,
    skips: BTreeMap<String, usize>,
    mismatches: BTreeMap<String, Vec<String>>,
}

impl Tally {
    fn add(&mut self, f: &Value) {
        match check(f) {
            Ok(Ok(())) => self.matched += 1,
            Ok(Err(class)) => {
                self.skipped += 1;
                *self.skips.entry(class).or_default() += 1;
            }
            Err(why) => {
                let name = f["name"].as_str().unwrap_or("?").to_owned();
                self.mismatches.entry(name).or_default().push(format!("{}: {why}", f["case"]));
            }
        }
    }
}

#[cfg(feature = "oracle-fixtures")]
#[test]
fn lowering_matches_the_oracle_fixtures() {
    use std::io::{BufRead, Write};
    let Some(paths) = std::env::var_os("H2_ORACLE_FIXTURES") else {
        let _ = writeln!(std::io::stderr(), "SKIPPED lowering_matches_the_oracle_fixtures: set H2_ORACLE_FIXTURES");
        return;
    };
    let mut t = Tally::default();
    for path in std::env::split_paths(&paths) {
        let file = std::fs::File::open(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        for line in std::io::BufReader::new(file).lines() {
            t.add(&serde_json::from_str(&line.unwrap()).unwrap());
        }
    }
    let Tally { matched, skipped, skips, mismatches } = t;
    let mut err = std::io::stderr();
    let _ = writeln!(err, "oracle: {matched} match, {skipped} skipped, {} encodings differ", mismatches.len());
    let mut by_count: Vec<_> = skips.iter().collect();
    by_count.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    for (class, n) in by_count {
        let _ = writeln!(err, "  skipped {n:6}  {class}");
    }
    let report: Vec<String> = mismatches.iter().map(|(k, v)| format!("{k} ({}): {}", v.len(), v[0])).collect();
    assert!(mismatches.is_empty(), "{}", report.join("\n"));
    assert!(matched > 0, "no fixture was checked");
}

// ---- the harness itself ----------------------------------------------------

mod harness {
    use serde_json::json;

    use super::*;

    const DATA: u32 = 0x20_0000;

    /// A machine state in the fixture schema: `gpr` EAX..EDI, EIP at the
    /// code, and non-default x87/MXCSR state (TOP = 3, odd tags and
    /// registers), so a harness that ignored any of it would notice.
    fn state(gpr: [u32; 8], eip: u32, eflags: u32) -> Value {
        let st: Vec<Vec<u8>> = (0..8u8).map(|i| (0..10u8).map(|j| i * 16 + j).collect()).collect();
        let xmm: Vec<Vec<u8>> = (0..8u8).map(|i| vec![i; 16]).collect();
        json!({
            "eax": gpr[0], "ecx": gpr[1], "edx": gpr[2], "ebx": gpr[3],
            "esp": gpr[4], "ebp": gpr[5], "esi": gpr[6], "edi": gpr[7],
            "eip": eip, "eflags": eflags,
            "x87": {
                "st": st, "control": 0x0e7f, "status": 0x1800 | 0x0021, "tag": 0x0f3c,
                "opcode": 0x01d9, "instruction_pointer": 0x1234, "data_pointer": 0x5678,
                "code_segment": 8, "data_segment": 16,
            },
            "xmm": xmm,
            "mxcsr": 0x9fc0 & 0xffbf,
        })
    }

    /// One completed instruction from `before` to `after`, flags defined
    /// per `defined_flags`.
    fn fixture(name: &str, bytes: &[u8], before: Value, after: Value, defined_flags: u32) -> Value {
        json!({
            "schema_version": 1, "name": name, "case": "harness", "seed": 0,
            "bytes": bytes,
            "initial": before,
            "memory": [[DATA, vec![0u8; 64]]],
            "outcome": {
                "stop": "Completed",
                "state": after,
                "defined_gprs": vec![u32::MAX; 8],
                "defined_flags": defined_flags,
                "writes": [],
            },
        })
    }

    const GPR: [u32; 8] = [0xf0, 0x3c, 3, 4, DATA + 0x40, 6, DATA, DATA + 8];

    /// `nop`: everything carried through unchanged.
    fn nop() -> Value {
        let a = state(GPR, CODE_ADDRESS, 0x2 | 0x0801);
        let b = state(GPR, CODE_ADDRESS + 1, 0x2 | 0x0801);
        fixture("nop", &[0x90], a, b, STATUS_FLAGS)
    }

    /// `and eax, ecx`: 0xf0 & 0x3c = 0x30, PF set, CF/OF/SF/ZF clear, AF
    /// undefined.
    fn and(eflags_after: u32, defined: u32) -> Value {
        let mut after = GPR;
        after[0] = 0x30;
        let a = state(GPR, CODE_ADDRESS, 0x2 | 0x0801);
        let b = state(after, CODE_ADDRESS + 2, eflags_after);
        fixture("and", &[0x21, 0xc8], a, b, defined)
    }

    const AF: u32 = 0x10;

    fn outcome(f: &Value) -> Result<Result<(), String>, String> {
        check(f)
    }

    #[test]
    fn non_default_fp_state_is_imported_and_carried_through() {
        assert_eq!(outcome(&nop()), Ok(Ok(())));
    }

    #[test]
    fn a_wrong_x87_or_mxcsr_expectation_is_caught() {
        let mut f = nop();
        f["outcome"]["state"]["x87"]["control"] = json!(0x037f);
        assert!(outcome(&f).unwrap_err().contains("x87"));
        let mut f = nop();
        f["outcome"]["state"]["x87"]["st"][2][0] = json!(0xee);
        assert!(outcome(&f).unwrap_err().contains("x87"));
        let mut f = nop();
        f["outcome"]["state"]["mxcsr"] = json!(0x1f80);
        assert!(outcome(&f).unwrap_err().contains("mxcsr"));
    }

    #[test]
    fn x87_registers_are_mapped_through_top() {
        // ST(i) is physical (TOP + i) & 7: with TOP = 3 the fixture's ST(0)
        // is physical 3. A consistent rotation on both sides still matches;
        // an expectation listing the registers physically does not.
        let mut f = nop();
        let st = f["initial"]["x87"]["st"].clone();
        let physical: Vec<Value> = (0..8).map(|p| st[(p + 8 - 3) % 8].clone()).collect();
        f["outcome"]["state"]["x87"]["st"] = json!(physical);
        assert!(outcome(&f).unwrap_err().contains("x87"));
    }

    #[test]
    fn undefined_flags_are_ignored_and_defined_ones_compared() {
        let all_but_af = STATUS_FLAGS & !AF;
        let right = 0x2 | 0x4;
        assert_eq!(outcome(&and(right, all_but_af)), Ok(Ok(())));
        // AF is undefined after AND: any value the oracle saw is accepted.
        assert_eq!(outcome(&and(right | AF, all_but_af)), Ok(Ok(())));
        // A defined flag with the wrong value.
        assert!(outcome(&and(right | 0x40, all_but_af)).unwrap_err().contains("eflags"));
        // The oracle defining AF where we do not.
        assert!(outcome(&and(right, STATUS_FLAGS)).unwrap_err().contains("defined flags"));
        // The oracle leaving ZF undefined where we define it.
        assert!(outcome(&and(right, all_but_af & !0x40)).unwrap_err().contains("defined flags"));
    }

    #[test]
    fn skips_are_counted_by_class() {
        let mut t = Tally::default();
        t.add(&nop());
        // aaa (37): not lowered.
        let mut aaa = nop();
        aaa["bytes"] = json!([0x37]);
        t.add(&aaa);
        t.add(&aaa);
        // The oracle refusing an instruction.
        let mut refused = nop();
        refused["outcome"]["stop"] = json!({"Unsupported": "Pause / [PAUSE]"});
        t.add(&refused);
        // A mismatch is neither a match nor a skip.
        let mut wrong = nop();
        wrong["outcome"]["state"]["eax"] = json!(1);
        t.add(&wrong);
        assert_eq!((t.matched, t.skipped), (1, 3));
        let skips: Vec<(&str, usize)> = t.skips.iter().map(|(k, v)| (k.as_str(), *v)).collect();
        assert_eq!(
            skips,
            [("aaa/0: integer: not implemented", 2), ("oracle refused: {\"Unsupported\":\"Pause / [PAUSE]\"}", 1)]
        );
        assert_eq!(t.mismatches.keys().collect::<Vec<_>>(), ["nop"]);
    }

    #[test]
    fn the_mmx_transition_is_compared_with_unicorns_departures_only() {
        // movq mm1, [esi] (0f 6f 0e): mm1 = 8 zero bytes, TOP = 0, all
        // tags valid. Unicorn reports bits 64-79 unchanged and the tag word
        // as classified contents.
        let a = state(GPR, CODE_ADDRESS, 0x2 | 0x0801);
        let mut b = state(GPR, CODE_ADDRESS + 3, 0x2 | 0x0801);
        let mut st = a["x87"]["st"].clone();
        // Logical order after the transition (TOP = 0) is physical.
        let mut physical: Vec<Value> = (0..8).map(|p| st[(p + 8 - 3) % 8].take()).collect();
        // mm1 = physical 1: eight zero bytes.
        for b in physical[1].as_array_mut().unwrap().iter_mut().take(8) {
            *b = json!(0);
        }
        b["x87"]["st"] = json!(physical);
        b["x87"]["status"] = json!(0x0021);
        b["x87"]["tag"] = json!(0x5554);
        let f = fixture("movq", &[0x0f, 0x6f, 0x0e], a, b, STATUS_FLAGS);
        assert_eq!(outcome(&f), Ok(Ok(())));
        // An empty tag where the transition makes every register valid is
        // still a mismatch, and so is a wrong significand.
        let mut g = f.clone();
        g["outcome"]["state"]["x87"]["tag"] = json!(0xc000);
        assert!(outcome(&g).unwrap_err().contains("x87"));
        let mut g = f.clone();
        g["outcome"]["state"]["x87"]["st"][1][3] = json!(9);
        assert!(outcome(&g).unwrap_err().contains("x87"));
    }

    #[test]
    fn approximations_are_held_to_the_architectural_bound() {
        // rcpss xmm1, xmm2 (f3 0f 53 ca) with xmm2 lane 0 = 3.0.
        let mut a = state(GPR, CODE_ADDRESS, 0x2);
        let three = 3.0f32.to_bits().to_le_bytes();
        let mut x2 = vec![0u8; 16];
        x2[..4].copy_from_slice(&three);
        a["xmm"][2] = json!(x2);
        let result = |bits: u32| {
            let mut b = a.clone();
            b["eip"] = json!(CODE_ADDRESS + 4);
            let mut x1 = vec![1u8; 16];
            x1[..4].copy_from_slice(&bits.to_le_bytes());
            b["xmm"][1] = json!(x1);
            fixture("rcpss", &[0xf3, 0x0f, 0x53, 0xca], a.clone(), b, STATUS_FLAGS)
        };
        let exact = (1.0f32 / 3.0).to_bits();
        assert_eq!(outcome(&result(exact)), Ok(Ok(())));
        // Within 1.5 * 2^-12 relative: accepted; outside: not.
        assert_eq!(outcome(&result(exact + 4)), Ok(Ok(())));
        assert!(outcome(&result(exact + 0x2000)).unwrap_err().contains("xmm1"));
    }

    #[test]
    fn after_sse_arithmetic_mxcsr_control_is_compared_but_not_its_flags() {
        // addss xmm1, xmm2 (f3 0f 58 ca): lanes 0x01010101 + 0x02020202,
        // inexact, so the reference sets PE where Unicorn reports none.
        let a = state(GPR, CODE_ADDRESS, 0x2);
        let mut b = a.clone();
        b["eip"] = json!(CODE_ADDRESS + 4);
        let sum = (f32::from_bits(0x0101_0101) + f32::from_bits(0x0202_0202)).to_bits();
        let mut x1 = vec![1u8; 16];
        x1[..4].copy_from_slice(&sum.to_le_bytes());
        b["xmm"][1] = json!(x1);
        let f = fixture("addss", &[0xf3, 0x0f, 0x58, 0xca], a, b, STATUS_FLAGS);
        assert_eq!(outcome(&f), Ok(Ok(())));
        let mut g = f.clone();
        g["outcome"]["state"]["mxcsr"] = json!(0x9f80 ^ 0x6000); // rounding control
        assert!(outcome(&g).unwrap_err().contains("mxcsr"));
        // Without SSE arithmetic, the flag bits are compared too.
        let mut n = nop();
        n["outcome"]["state"]["mxcsr"] = json!(0x9f80 | 0x20);
        assert!(outcome(&n).unwrap_err().contains("mxcsr"));
    }
}
