//! Differential test: synthetic functions run through the IR interpreter
//! and through generated, compiled C, from the same random states, must
//! end the same way with the same CPU state and memory.
//!
//! The fixtures are small x86 functions (assembled here, no game data)
//! covering what the lowering emits: ALU and flag formulas at every width,
//! shifts and rotates by immediates and by CL (including zero), multiply
//! and divide (and `#DE`), bit ops, atomics, string ops with REP (and a
//! fault mid-iteration), stack and EFLAGS forms, branches and loops, SSE
//! moves (and the alignment `#GP`), FP control intrinsics, traps, and
//! transfers that leave the function. Each runs from many random states
//! whose pointers land in, near and outside mapped memory, so loads and
//! stores fault some of the time. Needs `h2abi.h` (`H2ABI_HEADER` or the
//! workspace copy) and clang.

mod common;

use std::collections::{BTreeMap, BTreeSet, HashMap};

use emit::host::{CpuAbi, Ctx, ENTRY, FAULT, HARNESS_EXIT, HostEvent, JUMP, RETURN, STOP, TransferAbi};
use iced_x86::IcedError;
use iced_x86::code_asm::CodeAssembler;
use iced_x86::{Decoder, DecoderOptions, FlowControl};
use ir::interp::{self, Cpu, Exit, FlatMemory};
use ir::{Fault, IrBlock, IrFunction, StopReason};

const MEM_BASE: u32 = 0x0010_0000;
const MEM_LEN: usize = 0x1_0000;
const CODE_BASE: u32 = 0x0040_0000;
const STATES: usize = 48;

type Asm = fn(&mut CodeAssembler) -> Result<(), IcedError>;

/// (name, body). Every body ends in `ret`; nothing follows it.
fn fixtures() -> Vec<(&'static str, Asm)> {
    use iced_x86::code_asm::*;
    vec![
        ("alu-reg", |a| {
            a.add(eax, edx)?;
            a.adc(ecx, eax)?;
            a.sub(dl, cl)?;
            a.sbb(ax, dx)?;
            a.and(ebx, ecx)?;
            a.or(si, ax)?;
            a.xor(ch, dl)?;
            a.cmp(eax, ecx)?;
            a.setl(al)?;
            a.test(dx, cx)?;
            a.setbe(ah)?;
            a.cmovg(ecx, edx)?;
            a.cmovp(dx, ax)?;
            a.ret()
        }),
        ("alu-mem", |a| {
            a.add(dword_ptr(ebx), eax)?;
            a.sub(word_ptr(ebx + 2), dx)?;
            a.or(byte_ptr(esi), cl)?;
            a.adc(eax, dword_ptr(edi + 8))?;
            a.cmp(byte_ptr(esi + 1), 0x7f)?;
            a.sets(byte_ptr(edi + 3))?;
            a.xor(dword_ptr(ebp - 4), 1)?;
            a.ret()
        }),
        ("unary", |a| {
            a.inc(eax)?;
            a.dec(cl)?;
            a.neg(dx)?;
            a.not(ebp)?;
            a.inc(dword_ptr(esi))?;
            a.neg(byte_ptr(edi))?;
            a.dec(word_ptr(ebx + 4))?;
            a.ret()
        }),
        ("multiply", |a| {
            a.mul(edx)?;
            a.imul(ecx)?;
            a.imul_3(eax, edx, 7)?;
            a.imul_2(si, word_ptr(ebx))?;
            a.mul(byte_ptr(esi))?;
            a.imul(cl)?;
            a.ret()
        }),
        ("divide", |a| {
            a.and(edx, 0xff)?;
            a.div(ecx)?; // random divisors: zero and overflow both happen
            a.idiv(bl)?;
            a.cwd()?;
            a.idiv(word_ptr(esi))?;
            a.ret()
        }),
        ("shift-imm", |a| {
            a.shl(eax, 1)?;
            a.shr(edx, 5)?;
            a.sar(cl, 3)?;
            a.rol(ax, 9)?;
            a.ror(byte_ptr(esi), 1)?;
            a.rcl(edx, 1)?;
            a.rcr(al, 9)?; // count mod 9 == 0: value, CF and OF kept
            a.rcl(bx, 17)?;
            a.rcr(dword_ptr(ebx), 31)?;
            a.shl(dl, 8)?;
            a.shr(si, 17)?;
            a.shld(eax, edx, 4)?;
            a.shrd(dword_ptr(edi), ecx, 1)?;
            a.ret()
        }),
        ("shift-cl", |a| {
            a.and(ecx, 0x27)?; // small counts, zero among them, some >= 32
            a.shl(eax, cl)?;
            a.sar(dx, cl)?;
            a.shr(byte_ptr(esi), cl)?;
            a.rcl(bl, cl)?;
            a.rcr(si, cl)?;
            a.rol(dword_ptr(ebx), cl)?;
            a.ror(dh, cl)?;
            a.shld(eax, edx, cl)?;
            a.shrd(dword_ptr(edi), edx, cl)?;
            a.ret()
        }),
        ("bits", |a| {
            a.bt(eax, edx)?;
            a.bts(ecx, 5)?;
            a.btr(dx, ax)?;
            a.btc(dword_ptr(esi), 3)?;
            a.and(eax, 0x7f)?;
            a.bts(dword_ptr(ebx), eax)?;
            a.btr(word_ptr(edi), ax)?;
            a.bt(dword_ptr(ebx), edx)?; // random offset: usually faults
            a.bsf(ecx, edx)?;
            a.bsr(si, word_ptr(esi))?;
            a.bswap(eax)?;
            a.ret()
        }),
        ("atomics", |a| {
            a.cmpxchg(dword_ptr(ebx), ecx)?;
            a.lock().xadd(dword_ptr(esi), edx)?;
            a.lock().add(dword_ptr(edi), eax)?;
            a.xchg(dword_ptr(ebx + 4), ecx)?;
            a.and(eax, 0x1f)?;
            a.lock().bts(dword_ptr(ebx), eax)?;
            a.lock().cmpxchg(byte_ptr(esi + 1), dl)?;
            a.lock().dec(word_ptr(edi + 2))?;
            a.ret()
        }),
        ("atomics-all", |a| {
            // Every operation the runtime accessors take, at every width,
            // with random (sometimes unmapped) addresses: EAX is random.
            a.lock().or(dword_ptr(esi), edx)?;
            a.lock().and(word_ptr(edi + 2), 0x7ff0)?;
            a.lock().xor(byte_ptr(ebx + 1), cl)?;
            a.lock().sub(dword_ptr(ebx + 8), ecx)?;
            a.lock().inc(byte_ptr(esi + 3))?;
            a.lock().neg(dword_ptr(edi + 4))?;
            a.lock().not(word_ptr(esi + 6))?;
            a.mov(ecx, edx)?;
            a.and(ecx, 0x3ff)?; // bit offsets beyond the first word
            a.lock().btr(dword_ptr(edi), ecx)?;
            a.lock().btc(word_ptr(esi), cx)?;
            a.lock().bts(word_ptr(ebx), 13)?;
            a.lock().xadd(byte_ptr(edi + 9), dh)?;
            a.xchg(word_ptr(esi + 10), bx)?;
            a.lock().cmpxchg(word_ptr(ebx + 12), si)?;
            let mut keep = a.create_label();
            a.test(bl, 1)?;
            a.jnz(keep)?;
            a.mov(eax, esi)?; // half the time mapped, else random EAX
            a.set_label(&mut keep)?;
            a.lock().add(dword_ptr(eax), 1)?;
            a.ret()
        }),
        ("strings", |a| {
            a.and(ecx, 15)?;
            a.rep().movsd()?;
            a.mov(ecx, eax)?;
            a.and(ecx, 7)?;
            a.repe().cmpsb()?;
            a.mov(ecx, edx)?;
            a.and(ecx, 7)?;
            a.repne().scasb()?;
            a.stosw()?;
            a.lodsd()?;
            a.and(ecx, 3)?;
            a.repne().stosb()?; // F2 on stos repeats like F3
            a.ret()
        }),
        ("string-fault", |a| {
            // EDI walks off the end of mapped memory part way through.
            a.mov(edi, (MEM_BASE + MEM_LEN as u32 - 24) as i32)?;
            a.and(ecx, 31)?;
            a.cld()?;
            a.rep().stosd()?;
            a.ret()
        }),
        ("compare-fault", |a| {
            // REPE CMPSB from the last mapped bytes, either direction: a
            // fault after equal iterations reports the starting EFLAGS.
            let mut down = a.create_label();
            let mut go = a.create_label();
            a.and(ecx, 7)?;
            a.test(dl, 1)?;
            a.jnz(down)?;
            a.mov(esi, (MEM_BASE + MEM_LEN as u32 - 3) as i32)?;
            a.mov(edi, (MEM_BASE + MEM_LEN as u32 - 3) as i32)?;
            a.cld()?;
            a.jmp(go)?;
            a.set_label(&mut down)?;
            a.mov(esi, (MEM_BASE + 2) as i32)?;
            a.mov(edi, (MEM_BASE + 2) as i32)?;
            a.std()?;
            a.set_label(&mut go)?;
            a.repe().cmpsb()?;
            a.cld()?;
            a.ret()
        }),
        ("shift-zero-access", |a| {
            // Zero-count shifts of memory still read and write it; EAX is
            // random (so usually unmapped) half the time.
            let mut keep = a.create_label();
            a.test(bl, 1)?;
            a.jnz(keep)?;
            a.mov(eax, esi)?;
            a.set_label(&mut keep)?;
            a.and(ecx, 0x20)?; // CL masks to 0 either way
            a.shl(dword_ptr(eax), cl)?;
            a.rcr(word_ptr(eax), 0)?;
            a.shld(dword_ptr(eax), edx, cl)?;
            a.shrd(word_ptr(eax), dx, 0)?;
            a.ret()
        }),
        ("shift-16", |a| {
            a.and(ecx, 0x1f)?; // counts above 16 leave the flags undefined
            a.shld(ax, dx, cl)?;
            a.shrd(word_ptr(esi), bx, cl)?;
            a.shld(si, di, 3)?;
            a.shrd(bp, ax, 16)?;
            a.ret()
        }),
        ("stack-and-eflags", |a| {
            a.pushfd()?;
            a.pop(eax)?;
            a.and(eax, !0x7300)?; // leave TF/IF/IOPL/NT alone
            a.push(eax)?;
            a.popfd()?;
            a.lahf()?;
            a.xor(ah, 0x41)?;
            a.sahf()?;
            a.pushad()?;
            a.popad()?;
            a.push(dword_ptr(esi))?;
            a.pop(dword_ptr(edi))?;
            a.enter(16u32, 0u32)?;
            a.leave()?;
            a.ret_1(8)
        }),
        ("popfd-system", |a| {
            a.push(eax)?; // random: usually changes TF/IF/IOPL/NT, which stops
            a.popfd()?;
            a.ret()
        }),
        ("widen-and-move", |a| {
            a.movzx(eax, byte_ptr(esi))?;
            a.movsx(edx, word_ptr(edi))?;
            a.movsx(cx, bl)?;
            a.lea(ebx, dword_ptr(esi + ecx * 4 + 0x10))?;
            a.cbw()?;
            a.cwde()?;
            a.cdq()?;
            a.mov(ebx, MEM_BASE as i32 + 0x100)?;
            a.xlatb()?;
            a.mov(word_ptr(edi + 6), dx)?;
            a.xchg(eax, edx)?;
            a.ret()
        }),
        ("branches", |a| {
            let mut less = a.create_label();
            let mut join = a.create_label();
            let mut top = a.create_label();
            let mut skip = a.create_label();
            a.cmp(eax, edx)?;
            a.jl(less)?;
            a.add(eax, 1)?;
            a.jmp(join)?;
            a.set_label(&mut less)?;
            a.sub(eax, 1)?;
            a.set_label(&mut join)?;
            a.and(ecx, 7)?;
            a.jecxz(skip)?;
            a.set_label(&mut top)?;
            a.add(edx, eax)?;
            a.loop_(top)?;
            a.set_label(&mut skip)?;
            a.test(dl, 1)?;
            a.cmovnz(eax, edx)?;
            a.ret()
        }),
        ("sse", |a| {
            a.movaps(xmm0, xmmword_ptr(ebx))?; // alignment is random: #GP
            a.movups(xmm1, xmmword_ptr(esi))?;
            a.xorps(xmm0, xmm1)?;
            a.andnps(xmm2, xmm0)?;
            a.shufps(xmm2, xmm1, 0x1b)?;
            a.unpcklps(xmm3, xmm2)?;
            a.unpckhps(xmm4, xmm0)?;
            a.movss(dword_ptr(edi), xmm2)?;
            a.movss(xmm5, dword_ptr(esi + 4))?;
            a.movss(xmm6, xmm3)?;
            a.movhlps(xmm7, xmm1)?;
            a.movlhps(xmm1, xmm7)?;
            a.movhps(xmm0, qword_ptr(edi + 8))?;
            a.movlps(qword_ptr(esi + 16), xmm4)?;
            a.movmskps(eax, xmm0)?;
            a.ret()
        }),
        ("sse-fp", |a| {
            a.addss(xmm0, xmm1)?;
            a.subss(xmm1, dword_ptr(esi))?;
            a.mulss(xmm2, xmm0)?;
            a.divss(xmm3, xmm2)?;
            a.mulps(xmm4, xmm5)?;
            a.addps(xmm5, xmm6)?;
            a.subps(xmm6, xmmword_ptr(ebx))?; // random alignment: #GP
            a.divps(xmm7, xmm0)?;
            a.minss(xmm0, xmm3)?;
            a.maxps(xmm1, xmm4)?;
            a.minps(xmm2, xmm5)?;
            a.maxss(xmm3, dword_ptr(edi + 4))?;
            a.sqrtss(xmm4, xmm1)?;
            a.sqrtps(xmm5, xmm2)?;
            a.rcpss(xmm6, xmm3)?;
            a.rsqrtss(xmm7, dword_ptr(esi + 8))?;
            a.rsqrtps(xmm0, xmm7)?;
            a.rcpps(xmm1, xmm6)?;
            a.cmpss(xmm2, xmm1, 1)?;
            a.cmpps(xmm3, xmm4, 4)?;
            a.cmpps(xmm4, xmm5, 3)?;
            a.comiss(xmm5, xmm6)?;
            a.setb(al)?;
            a.setp(ah)?;
            a.ucomiss(xmm6, dword_ptr(esi + 12))?;
            a.cvtsi2ss(xmm7, ecx)?;
            a.cvtsi2ss(xmm6, dword_ptr(edi))?;
            a.cvtss2si(edx, xmm5)?;
            a.cvttss2si(ecx, dword_ptr(esi + 16))?;
            a.movntps(xmmword_ptr(ebx + 16), xmm0)?; // aligned when subps was
            a.prefetcht0(byte_ptr(eax))?;
            a.prefetchnta(byte_ptr(ecx))?;
            a.sfence()?;
            a.ret()
        }),
        ("sse-mmx", |a| {
            a.cvtpi2ps(xmm0, mm1)?;
            a.cvtpi2ps(xmm1, qword_ptr(esi))?;
            a.cvtps2pi(mm2, xmm3)?;
            a.cvttps2pi(mm3, qword_ptr(edi + 8))?;
            a.pshufw(mm4, mm2, 0x1b)?;
            a.pinsrw(mm5, ecx, 2)?;
            a.pinsrw(mm6, word_ptr(esi + 6), 7)?;
            a.pextrw(eax, mm5, 1)?;
            a.pmovmskb(edx, mm4)?;
            a.pmaxsw(mm0, mm1)?;
            a.pminsw(mm1, qword_ptr(esi + 16))?;
            a.pmaxub(mm2, mm3)?;
            a.pminub(mm3, mm4)?;
            a.pavgb(mm4, mm5)?;
            a.pavgw(mm5, mm6)?;
            a.psadbw(mm6, mm7)?;
            a.movntq(qword_ptr(edi + 24), mm6)?;
            a.ret()
        }),
        ("mmx", |a| {
            a.movq(mm0, qword_ptr(esi))?;
            a.movd(mm1, ecx)?;
            a.movd(mm2, dword_ptr(edi + 4))?;
            a.paddw(mm0, mm3)?;
            a.paddsw(mm1, mm4)?;
            a.paddusb(mm2, mm5)?;
            a.psubusw(mm3, qword_ptr(esi + 8))?;
            a.psubsb(mm4, mm6)?;
            a.psubd(mm5, mm7)?;
            a.paddd(mm6, mm0)?;
            a.paddb(mm7, mm1)?;
            a.pmullw(mm0, mm2)?;
            a.pmulhw(mm1, mm3)?;
            a.pmaddwd(mm2, mm4)?;
            a.pand(mm3, mm5)?;
            a.pandn(mm4, mm6)?;
            a.por(mm5, mm7)?;
            a.pxor(mm6, qword_ptr(edi + 16))?;
            a.psllw(mm7, 3)?;
            a.pslld(mm0, mm1)?; // 64-bit count: usually all zero
            a.psrlq(mm1, 17)?;
            a.psraw(mm2, 20)?;
            a.psrad(mm3, mm4)?;
            a.psrlw(mm4, qword_ptr(esi + 24))?;
            a.psllq(mm5, 63)?;
            a.psrld(mm6, 31)?;
            a.movd(eax, mm7)?;
            a.movd(dword_ptr(edi + 32), mm6)?;
            a.movq(qword_ptr(edi + 40), mm5)?;
            a.ret()
        }),
        ("mmx-pack", |a| {
            // Small counts from ECX so the register-count shifts do shift.
            a.and(ecx, 0x1f)?;
            a.movd(mm7, ecx)?;
            a.psllw(mm0, mm7)?;
            a.psrad(mm1, mm7)?;
            a.punpcklbw(mm0, mm1)?;
            a.punpcklwd(mm1, dword_ptr(esi))?;
            a.punpckldq(mm2, mm3)?;
            a.punpckhbw(mm3, mm4)?;
            a.punpckhwd(mm4, qword_ptr(edi))?;
            a.punpckhdq(mm5, mm6)?;
            a.packsswb(mm0, mm1)?;
            a.packssdw(mm1, mm2)?;
            a.packuswb(mm2, qword_ptr(esi + 8))?;
            a.pcmpeqb(mm3, mm3)?;
            a.pcmpeqw(mm4, mm5)?;
            a.pcmpeqd(mm5, mm6)?;
            a.pcmpgtb(mm6, mm0)?;
            a.pcmpgtw(mm0, mm1)?;
            a.pcmpgtd(mm1, mm2)?;
            a.pmulhuw(mm2, mm3)?;
            a.test(dl, 1)?;
            let mut skip = a.create_label();
            a.jz(skip)?;
            a.emms()?;
            a.set_label(&mut skip)?;
            a.ret()
        }),
        ("fp-control", |a| {
            a.fnstcw(word_ptr(esi))?;
            a.fninit()?;
            a.fldcw(word_ptr(edi))?;
            a.fnstsw(ax)?;
            a.fnclex()?;
            a.fnstsw(word_ptr(esi + 2))?;
            a.stmxcsr(dword_ptr(esi + 4))?;
            a.and(dword_ptr(edi + 8), 0xffff)?;
            a.ldmxcsr(dword_ptr(edi + 8))?;
            a.ldmxcsr(dword_ptr(ebx))?; // random: reserved bits give #GP
            a.wait()?;
            a.ret()
        }),
        ("traps", |a| {
            let mut first = a.create_label();
            let mut done = a.create_label();
            a.test(al, 3)?;
            a.jnz(first)?;
            a.int(3)?; // CD 03: #BP, saved EIP after it
            a.set_label(&mut first)?;
            a.test(al, 4)?;
            a.jz(done)?;
            a.int3()?;
            a.set_label(&mut done)?;
            a.ret()
        }),
        ("leaves", |a| {
            let mut far = a.create_label();
            a.test(dl, 1)?;
            a.jnz(far)?;
            a.test(dl, 2)?;
            a.jz(far)?;
            a.call(0x0080_0000u64)?; // pushes, then leaves for another function
            a.ret()?;
            a.set_label(&mut far)?;
            a.and(eax, 0xfff0)?;
            a.jmp(eax)
        }),
    ]
}

/// Assemble `body` at `base` and build its function: blocks split at every
/// branch target and after every transfer, lowered without CFG facts
/// (jumps are local, calls return).
fn function(base: u32, body: Asm) -> IrFunction {
    let mut a = CodeAssembler::new(32).unwrap();
    body(&mut a).unwrap();
    let code = a.assemble(base as u64).unwrap();
    let insns: Vec<_> = Decoder::with_ip(32, &code, base as u64, DecoderOptions::NONE).into_iter().collect();
    let mut leaders = BTreeSet::from([base]);
    for i in &insns {
        match i.flow_control() {
            FlowControl::ConditionalBranch | FlowControl::UnconditionalBranch if i.near_branch32() != 0 => {
                leaders.insert(i.near_branch32());
                leaders.insert(i.next_ip32());
            }
            FlowControl::Next => {}
            _ => {
                leaders.insert(i.next_ip32());
            }
        }
    }
    let env = lift::Env::default();
    let mut blocks: BTreeMap<u32, IrBlock> = BTreeMap::new();
    let mut start = base;
    for i in &insns {
        if leaders.contains(&i.ip32()) {
            start = i.ip32();
        }
        let ir = lift::lower(i, &env, lift::Site::Plain);
        blocks.entry(start).or_insert_with(|| IrBlock { start, insns: Vec::new() }).insns.push(ir);
    }
    let f = IrFunction { entry: base, blocks, interior_entries: Vec::new(), errors: Vec::new() };
    f.validate().unwrap_or_else(|e| panic!("{e}\n{f}"));
    f
}

/// How a run ended, in terms both backends can report.
#[derive(Debug, Clone, PartialEq, Eq)]
enum End {
    Returned,
    /// Left the function for an address it doesn't contain.
    Left(u32),
    Memory { pc: u32, addr: u32, write: bool },
    Fault { pc: u32, code: u32, saved_eip: u32 },
    Stop { pc: u32, code: u32 },
    Unsupported { pc: u32 },
}

fn shim_code(f: Fault) -> u32 {
    match f {
        Fault::DivideError => 1,
        Fault::Breakpoint => 2,
        Fault::InvalidOpcode => 3,
        Fault::Interrupt(_) => 4,
        Fault::Overflow => 5,
        Fault::BoundRange => 6,
        Fault::GeneralProtection => 7,
        Fault::SimdException => 13,
        Fault::FpError => 14,
    }
}

fn stop_code(r: StopReason) -> u32 {
    match r {
        StopReason::Halt => 8,
        StopReason::UnknownTarget => 10,
        StopReason::Unsupported => 10,
        StopReason::DescentError => 11,
        StopReason::Unmodelled => 12,
    }
}

fn run_interp(f: &IrFunction, cpu: &mut Cpu, mem: &mut FlatMemory) -> End {
    let insns: HashMap<u32, &ir::IrInsn> = f.insns().map(|i| (i.pc, i)).collect();
    let mut pc = f.entry;
    for _ in 0..100_000 {
        if pc == HARNESS_EXIT {
            return End::Returned;
        }
        let Some(insn) = insns.get(&pc) else { return End::Left(pc) };
        match interp::step(cpu, mem, insn) {
            Ok(flow) => pc = flow.target(),
            Err(Exit::Memory { pc, fault }) => return End::Memory { pc, addr: fault.addr, write: fault.write },
            Err(Exit::Fault { pc, fault, next }) => {
                return End::Fault { pc, code: shim_code(fault), saved_eip: next.unwrap_or(pc) };
            }
            Err(Exit::Stop { pc, reason }) => return End::Stop { pc, code: stop_code(reason) },
            Err(Exit::UnknownTarget { pc, .. }) => return End::Stop { pc, code: 10 },
            Err(Exit::Unsupported { pc, .. }) => return End::Unsupported { pc },
        }
    }
    panic!("interpreter did not finish")
}

fn run_c(g: &emit::host::Generated, entry: u32, cpu: &mut CpuAbi, ctx: &mut Ctx) -> End {
    let (t, _) = g.run(cpu, ctx, TransferAbi::new(JUMP, ENTRY, entry), 100_000);
    match t.kind {
        RETURN if t.target_pc == HARNESS_EXIT => End::Returned,
        FAULT | STOP => match ctx.event(t.event_id).cloned() {
            Some(HostEvent::Memory { pc, fault }) => End::Memory { pc, addr: fault.addr, write: fault.write },
            Some(HostEvent::Guest { pc, saved_eip, code, .. }) if t.kind == FAULT => {
                End::Fault { pc, code, saved_eip }
            }
            Some(HostEvent::Guest { pc, code, .. }) => End::Stop { pc, code },
            Some(HostEvent::Unsupported { pc, .. }) => End::Unsupported { pc },
            None => panic!("transfer without an event: {t:x?}"),
        },
        _ => End::Left(t.target_pc),
    }
}

/// xorshift64*: deterministic, so failures reproduce.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 32) as u32
    }

    /// A register value: an edge case a third of the time.
    fn value(&mut self) -> u32 {
        const EDGES: [u32; 10] = [0, 1, 2, 0x7f, 0x80, 0xff, 0xffff, 0x7fff_ffff, 0x8000_0000, 0xffff_ffff];
        let r = self.next();
        if r.is_multiple_of(3) { EDGES[(r / 3) as usize % EDGES.len()] } else { self.next() }
    }

    /// A pointer into mapped memory, mostly well inside it.
    fn pointer(&mut self) -> u32 {
        MEM_BASE + 0x100 + self.next() % (MEM_LEN as u32 - 0x400)
    }
}

fn random_state(rng: &mut Rng) -> (Cpu, FlatMemory) {
    let mut mem = FlatMemory::new(MEM_BASE, MEM_LEN);
    for b in mem.bytes.iter_mut() {
        *b = rng.next() as u8;
    }
    let mut cpu = Cpu::default();
    for r in cpu.gpr.iter_mut() {
        *r = rng.value();
    }
    // EBX, ESI, EDI, EBP point into memory (EBX dword-aligned half the
    // time, so aligned SSE loads sometimes succeed); ESP near the top with
    // the return address planted.
    for i in [3, 5, 6, 7] {
        cpu.gpr[i] = rng.pointer();
    }
    if rng.next().is_multiple_of(2) {
        cpu.gpr[3] &= !0xf;
    }
    let stack_top = MEM_BASE + MEM_LEN as u32 - 0x200;
    cpu.gpr[4] = stack_top;
    mem.load(stack_top, &HARNESS_EXIT.to_le_bytes());
    // Status flags and DF random; system flags clear as the model has them.
    cpu.eflags = 2 | (rng.next() & 0x0cd5);
    // XMM lanes: a quarter special values (zeros, infinities, NaNs,
    // denormals, conversion bounds), the rest random bits.
    const SPECIAL: [u32; 12] = [
        0, 0x8000_0000, 0x7f80_0000, 0xff80_0000, 0x7fc0_0000, 0x7f80_0001, 0xffc0_0000, 1, 0x3f80_0000,
        0xbf80_0000, 0x4f00_0000, 0xcf00_0000,
    ];
    for x in cpu.xmm.iter_mut() {
        *x = 0;
        for lane in 0..4 {
            let r = rng.next();
            let v = if r.is_multiple_of(4) { SPECIAL[(r >> 8) as usize % SPECIAL.len()] } else { rng.next() };
            *x |= (v as u128) << (32 * lane);
        }
    }
    cpu.x87.control = rng.next() as u16;
    cpu.x87.status = rng.next() as u16;
    // ES (a pending x87 exception) stops every MMX instruction with #MF:
    // set it in a quarter of the states, so both paths run.
    if !rng.next().is_multiple_of(4) {
        cpu.x87.status &= !0x0080;
    }
    cpu.x87.tag = rng.next() as u16;
    for st in cpu.x87.st.iter_mut() {
        for b in st.iter_mut() {
            *b = rng.next() as u8;
        }
    }
    cpu.mxcsr = rng.next() & ir::interp::MXCSR_MASK;
    (cpu, mem)
}

#[test]
fn generated_c_matches_the_interpreter() {
    const NAME: &str = "generated_c_matches_the_interpreter";
    let Some(header) = common::abi_header() else {
        common::skip(NAME, "needs h2abi.h (H2ABI_HEADER)");
        return;
    };
    let fixtures = fixtures();
    let funcs: Vec<(&str, IrFunction)> =
        fixtures.iter().enumerate().map(|(i, (n, body))| (*n, function(CODE_BASE + 0x1000 * i as u32, *body))).collect();
    let owners = emit::plan(funcs.iter().map(|(_, f)| f.clone()).collect());
    let g = common::build(&owners, &header, "differential");

    let mut ends: BTreeMap<&str, BTreeMap<String, usize>> = BTreeMap::new();
    let mut failures = Vec::new();
    for (k, (name, f)) in funcs.iter().enumerate() {
        let mut rng = Rng(0x4832_7265_636f_6d70 ^ (k as u64 + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15));
        for s in 0..STATES {
            let (mut cpu, mut mem) = random_state(&mut rng);
            let mut abi = CpuAbi::from_interp(&cpu);
            let mut ctx = Ctx::new(mem.clone());
            let want = run_interp(f, &mut cpu, &mut mem);
            let got = run_c(&g, f.entry, &mut abi, &mut ctx);
            let kind = format!("{want:?}").split([' ', '(', '{']).next().unwrap().to_owned();
            *ends.entry(name).or_default().entry(kind).or_default() += 1;
            let mut why = Vec::new();
            if got != want {
                why.push(format!("end: C {got:x?}, interpreter {want:x?}"));
            }
            let c = abi.to_interp();
            if c != cpu {
                why.push(format!("cpu:\n  C    {c:x?}\n  interp {cpu:x?}"));
            }
            if let Some(i) = ctx.mem.bytes.iter().zip(&mem.bytes).position(|(a, b)| a != b) {
                why.push(format!("memory differs first at {:#x}", MEM_BASE + i as u32));
            }
            if !why.is_empty() {
                failures.push(format!("{name} state {s}: {}", why.join("; ")));
            }
        }
    }
    // Each fixture should exercise more than one outcome where it can.
    eprintln!("outcomes: {ends:#?}");
    assert!(failures.is_empty(), "{} mismatches:\n{}", failures.len(), failures.join("\n"));
    let all: BTreeSet<&String> = ends.values().flat_map(|m| m.keys()).collect();
    for k in ["Returned", "Left", "Memory", "Fault", "Stop"] {
        assert!(all.contains(&k.to_string()), "no fixture ended with {k}: {ends:?}");
    }
}
