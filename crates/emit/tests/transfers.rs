//! Registration and transfer routing through generated C, on synthetic
//! functions (no game data): where returns land, and that atomics take
//! the runtime's operation-wide path.

mod common;

use std::collections::BTreeMap;

use emit::host::{CpuAbi, Ctx, ENTRY, HARNESS_EXIT, JUMP, RESUME, RETURN, TransferAbi};
use iced_x86::code_asm::*;
use iced_x86::{Decoder, DecoderOptions};
use ir::interp::FlatMemory;
use ir::{IrBlock, IrFunction};

const A: u32 = 0x0040_0000;
const B: u32 = A + 5; // right after A's `call`: A's continuation
const C: u32 = 0x0040_1000;
const STACK: u32 = 0x0010_8000;

/// One function of a single block assembled at `base`.
fn single_block(base: u32, body: impl FnOnce(&mut CodeAssembler) -> Result<(), IcedError>) -> IrFunction {
    let mut a = CodeAssembler::new(32).unwrap();
    body(&mut a).unwrap();
    let code = a.assemble(base as u64).unwrap();
    let env = lift::Env::default();
    let insns = Decoder::with_ip(32, &code, base as u64, DecoderOptions::NONE)
        .into_iter()
        .map(|i| lift::lower(&i, &env, lift::Site::Plain))
        .collect();
    let f = IrFunction {
        entry: base,
        blocks: BTreeMap::from([(base, IrBlock { start: base, insns })]),
        interior_entries: vec![],
        errors: vec![],
    };
    f.validate().unwrap_or_else(|e| panic!("{e}\n{f}"));
    f
}

/// A: `call C`, whose continuation is B's entry (A's CFG stops there).
/// B: `mov eax, 1; ret`. C: `ret`.
fn shared_continuation() -> Vec<IrFunction> {
    vec![
        single_block(A, |a| a.call(C as u64)),
        single_block(B, |a| {
            a.mov(eax, 1)?;
            a.ret()
        }),
        single_block(C, |a| a.ret()),
    ]
}

#[test]
fn a_continuation_that_is_another_entry_is_registered_as_its_resume() {
    let owners = emit::plan(shared_continuation());
    let by_entry: BTreeMap<u32, &emit::Owner> = owners.iter().map(|o| (o.func.entry, o)).collect();
    assert_eq!(by_entry[&B].resumes, vec![B], "the return into B resumes B's handler");
    assert!(by_entry[&A].resumes.is_empty());
    assert!(by_entry[&B].selectors.contains(&B));
}

#[test]
fn a_return_into_another_entry_resumes_it_through_c() {
    const NAME: &str = "a_return_into_another_entry_resumes_it_through_c";
    let Some(header) = common::abi_header() else {
        common::skip(NAME, "needs h2abi.h (H2ABI_HEADER)");
        return;
    };
    let owners = emit::plan(shared_continuation());
    let g = common::build(&owners, &header, "transfers-resume");
    let mut mem = FlatMemory::new(STACK - 0x1000, 0x1000);
    mem.load(STACK - 4, &HARNESS_EXIT.to_le_bytes());
    let mut cpu = CpuAbi::default();
    cpu.gpr[4] = STACK - 4;
    let mut ctx = Ctx::new(mem);
    // A calls C; C returns to B's entry address, which only the resume
    // table may resolve (the host never sends a return to entry dispatch).
    let (t, calls) = g.run(&mut cpu, &mut ctx, TransferAbi::new(JUMP, ENTRY, A), 16);
    assert_eq!((t.kind, t.target_pc), (RETURN, HARNESS_EXIT), "{t:x?} after {calls} calls");
    assert_eq!(cpu.gpr[0], 1, "B ran");
    assert_eq!(calls, 3, "A, C, then B resumed");
}

#[test]
fn atomics_go_through_the_runtime_accessors_and_leave_nothing_open_on_a_fault() {
    const NAME: &str = "atomics_go_through_the_runtime_accessors_and_leave_nothing_open_on_a_fault";
    let Some(header) = common::abi_header() else {
        common::skip(NAME, "needs h2abi.h (H2ABI_HEADER)");
        return;
    };
    // lock add dword [eax], 1; lock xadd [eax], ecx; xchg [eax], edx;
    // lock cmpxchg [eax], ebx; ret
    let f = single_block(A, |a| {
        a.lock().add(dword_ptr(eax), 1)?;
        a.lock().xadd(dword_ptr(eax), ecx)?;
        a.xchg(dword_ptr(eax), edx)?;
        a.lock().cmpxchg(dword_ptr(eax), ebx)?;
        a.ret()
    });
    let g = common::build(&emit::plan(vec![f]), &header, "transfers-atomic");
    let data = STACK - 0x800;
    let run = |acc: u32| {
        let mut mem = FlatMemory::new(STACK - 0x1000, 0x1000);
        mem.load(STACK - 4, &HARNESS_EXIT.to_le_bytes());
        mem.load(data, &10u32.to_le_bytes());
        let mut cpu = CpuAbi { gpr: [acc, 3, 7, 9, STACK - 4, 0, 0, 0], ..CpuAbi::default() };
        let mut ctx = Ctx::new(mem);
        let (t, _) = g.run(&mut cpu, &mut ctx, TransferAbi::new(JUMP, ENTRY, A), 4);
        (t, cpu, ctx)
    };
    let (t, cpu, ctx) = run(data);
    assert_eq!((t.kind, t.target_pc), (RETURN, HARNESS_EXIT));
    // 10 + 1 = 11; xadd: 14, ecx = 11; xchg: 7, edx = 14; cmpxchg: eax
    // (= data) != 7, so eax = 7 and memory keeps 7.
    assert_eq!(ctx.atomics, vec![1, 1, 15, 16], "ADD, ADD (xadd), xchg, cmpxchg");
    assert_eq!((cpu.gpr[1], cpu.gpr[2], cpu.gpr[0]), (11, 14, 7));
    assert_eq!(ctx.mem.read_u32(data), Some(7));
    // An unmapped operand: the first accessor fails, the handler faults at
    // that instruction with nothing changed, and no later accessor runs.
    let (t, cpu, ctx) = run(0x0090_0000);
    assert_eq!((t.kind, cpu.eip, cpu.gpr[0]), (emit::host::FAULT, A, 0x0090_0000));
    assert_eq!(ctx.atomics, vec![1]);
}

#[test]
fn a_faulting_rep_restarts_at_its_own_pc() {
    const NAME: &str = "a_faulting_rep_restarts_at_its_own_pc";
    // mov ecx, 8; rep stosd; mov eax, 1; ret. The rep is mid-block.
    let f = single_block(A, |a| {
        a.mov(ecx, 8)?;
        a.rep().stosd()?;
        a.mov(eax, 1)?;
        a.ret()
    });
    let rep = A + 5;
    let owners = emit::plan(vec![f]);
    assert_eq!(owners[0].resumes, vec![rep], "the REP is a restart point of its owner");
    assert!(owners[0].selectors.contains(&rep));
    let Some(header) = common::abi_header() else {
        common::skip(NAME, "needs h2abi.h (H2ABI_HEADER)");
        return;
    };
    let g = common::build(&owners, &header, "transfers-rep-restart");
    let mut mem = FlatMemory::new(STACK - 0x1000, 0x1000);
    let top = STACK - 0x800;
    mem.load(top, &HARNESS_EXIT.to_le_bytes());
    // EDI two dwords below the end of memory: the third store faults.
    let mut cpu = CpuAbi { gpr: [0, 0, 0, 0, top, 0, 0, STACK - 8], ..CpuAbi::default() };
    let mut ctx = Ctx::new(mem);
    let (t, _) = g.run(&mut cpu, &mut ctx, TransferAbi::new(JUMP, ENTRY, A), 4);
    assert_eq!((t.kind, cpu.eip), (emit::host::FAULT, rep), "{t:x?}");
    assert_eq!((cpu.gpr[1], cpu.gpr[7]), (6, STACK), "two iterations done");
    // A handler fixes EDI and continues at the faulting PC.
    cpu.gpr[7] = STACK - 0x400;
    let (t, calls) = g.run(&mut cpu, &mut ctx, TransferAbi::new(JUMP, RESUME, rep), 4);
    assert_eq!((t.kind, t.target_pc), (RETURN, HARNESS_EXIT), "{t:x?}");
    assert_eq!((calls, cpu.gpr[0], cpu.gpr[1], cpu.gpr[7]), (1, 1, 0, STACK - 0x400 + 24));
}
