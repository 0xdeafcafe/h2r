//! Register homes (`emit::c::homes`): a handler keeps in C locals only the
//! registers it writes, and reads the others from `CpuAbi`. These tests
//! pin which registers each kind of statement keeps, the C that results,
//! and that a handler which leaves EFLAGS alone still exits with it
//! canonical. Behaviour against the interpreter is in `differential.rs`
//! (the `homes-*` fixtures and every other one); across direct calls, in
//! `direct.rs` (`a_callee_writes_registers_its_caller_only_reads`).

mod common;

use emit::c::{HOME_FLAGS, OwnerShape, dirty_at_exits, homes};
use emit::host::{CpuAbi, Ctx, ENTRY, HARNESS_EXIT, JUMP, RETURN, TransferAbi};
use iced_x86::code_asm::*;
use ir::interp::FlatMemory;

const BASE: u32 = 0x0040_0000;
const MEM: u32 = 0x0010_0000;
const MEM_LEN: usize = 0x1000;

const EAX: u32 = 1 << 0;
const ECX: u32 = 1 << 1;
const EDX: u32 = 1 << 2;
const EBX: u32 = 1 << 3;
const ESP: u32 = 1 << 4;
const ESI: u32 = 1 << 6;
const EDI: u32 = 1 << 7;

fn homes_of(body: impl FnOnce(&mut CodeAssembler) -> Result<(), IcedError>) -> u32 {
    homes(&common::function(BASE, body))
}

/// The C of one function, planned as the image's are.
fn c_of(body: impl FnOnce(&mut CodeAssembler) -> Result<(), IcedError>) -> String {
    let owners = emit::plan(vec![common::function(BASE, body)]);
    let o = &owners[0];
    let shape = OwnerShape {
        selectors: &o.selectors,
        loops: &o.loops,
        accounting: false,
        block_clock: false,
        functions: None,
    };
    emit::c::function(&o.func, &shape)
}

#[test]
fn only_written_registers_are_homed() {
    // `ret` pops: ESP. Nothing else is touched.
    assert_eq!(homes_of(|a| a.ret()), ESP);
    // Reads alone (an address, a store's value) keep nothing.
    assert_eq!(
        homes_of(|a| {
            a.mov(dword_ptr(ebx + esi * 4), edi)?;
            a.ret()
        }),
        ESP
    );
    assert_eq!(
        homes_of(|a| {
            a.mov(eax, dword_ptr(ebx))?;
            a.ret()
        }),
        EAX | ESP
    );
}

#[test]
fn sse_run_resamples_after_memory_and_control_flow() {
    let text = c_of(|a| {
        let mut join = a.create_label();
        a.addps(xmm0, xmm1)?;
        a.mulps(xmm0, xmm2)?;
        a.movups(xmm0, xmmword_ptr(esi))?;
        a.addps(xmm0, xmm3)?;
        a.jz(join)?;
        a.addss(xmm0, xmm4)?;
        a.set_label(&mut join)?;
        a.subps(xmm0, xmm5)?;
        a.ret()
    });
    let mut samples = Vec::new();
    let mut n = 0;
    for line in text.lines() {
        if line.contains("h2fp_ = H2_FP_SAMPLE()") {
            n += 1;
        }
        if line.contains("H2_SSE_RUN(") {
            samples.push(n);
        }
    }
    assert_eq!(samples, [1, 1, 2, 3, 4], "{text}");
}

#[test]
fn x87_agreeing_paths_specialize_but_different_depths_join_dynamically() {
    let text = c_of(|a| {
        let mut join = a.create_label();
        a.fld(dword_ptr(esi))?;
        a.fadd(dword_ptr(esi + 4))?;
        a.jz(join)?;
        a.fld1()?;
        a.set_label(&mut join)?;
        a.fadd(dword_ptr(esi + 8))?;
        a.ret()
    });
    assert!(text.contains("H2X87_AT(0u, ld_f32"), "{text}");
    assert!(text.contains("H2X87_AT(7u, arith"), "{text}");
    assert!(text.contains("h2x87n_arith(&cpu->x87"), "{text}");
}

#[test]
fn partial_writes_home_the_whole_register() {
    // AL, AH and CX writes merge into the rest of the register, which must
    // be current in the local: EAX, ECX homed, BL's EBX not.
    assert_eq!(
        homes_of(|a| {
            a.mov(al, bl)?;
            a.mov(ah, 1)?;
            a.mov(cx, word_ptr(esi))?;
            a.ret()
        }),
        EAX | ECX | ESP
    );
}

#[test]
fn any_flag_access_homes_eflags() {
    // A flag write.
    assert_eq!(
        homes_of(|a| {
            a.add(eax, 1)?;
            a.ret()
        }),
        EAX | ESP | HOME_FLAGS
    );
    // A flag read only (a branch on the caller's flags), no write.
    assert_eq!(
        homes_of(|a| {
            let mut l = a.create_label();
            a.jc(l)?;
            a.mov(edx, 1)?;
            a.set_label(&mut l)?;
            a.ret()
        }),
        EDX | ESP | HOME_FLAGS
    );
    // pushfd reads all of EFLAGS.
    assert_eq!(
        homes_of(|a| {
            a.pushfd()?;
            a.ret()
        }),
        ESP | HOME_FLAGS
    );
}

#[test]
fn rep_and_atomics_home_what_their_c_touches() {
    // The REP loop counts ECX in C and the bulk copy moves ESI/EDI; it
    // saves and restores EFLAGS.
    assert_eq!(
        homes_of(|a| {
            a.rep().movsb()?;
            a.ret()
        }),
        ECX | ESP | ESI | EDI | HOME_FLAGS
    );
    // xadd writes the old value into its source register; the runtime
    // writes the flags.
    assert_eq!(
        homes_of(|a| {
            a.lock().xadd(dword_ptr(esi), ecx)?;
            a.ret()
        }),
        ECX | ESP | HOME_FLAGS
    );
    // cmpxchg: the runtime writes EAX in CpuAbi, and the instruction's IR
    // (which the C replaces with the call) writes it too, so it is homed
    // and reloaded after the call.
    assert_eq!(
        homes_of(|a| {
            a.lock().cmpxchg(dword_ptr(esi), edx)?;
            a.mov(dword_ptr(edi), eax)?;
            a.ret()
        }),
        EAX | ESP | HOME_FLAGS
    );
}

#[test]
fn the_c_reads_unhomed_registers_from_cpuabi() {
    let body = |a: &mut CodeAssembler| {
        a.mov(eax, dword_ptr(ebx + 4))?;
        a.mov(cl, bh)?;
        a.ret()
    };
    let text = c_of(body);
    assert!(
        text.contains(&format!("H2_LOCALS_HOMED({:#x}u);", EAX | ECX | ESP)),
        "{text}"
    );
    assert!(text.contains("cpu->ebx"), "{text}");
    // A homed register is read from its local.
    assert!(
        !text.contains("cpu->eax") && !text.contains("cpu->ecx"),
        "{text}"
    );
}

/// The call or tail-jump instructions' PCs of `f`, in order.
fn exits(f: &ir::IrFunction) -> Vec<u32> {
    f.insns()
        .filter(|i| {
            matches!(
                i.control,
                ir::Control::Call { .. } | ir::Control::Jump { tail: true, .. }
            )
        })
        .map(|i| i.pc)
        .collect()
}

#[test]
fn a_direct_call_spills_only_what_changed_since_the_last_sync() {
    // EBX is set before the first call only: the second call has nothing
    // to spill but the return address push (ESP); EDI is never written.
    let f = common::function(BASE, |a| {
        a.mov(ebx, 1)?;
        a.call(0x0050_0000u64)?;
        a.call(0x0050_1000u64)?;
        a.mov(edi, ebx)?;
        a.ret()
    });
    let home = homes(&f);
    assert_eq!(home, EBX | ESP | EDI);
    let d = dirty_at_exits(&f, home);
    let pcs = exits(&f);
    assert_eq!(pcs.len(), 2);
    assert_eq!(d[&pcs[0]], EBX | ESP);
    assert_eq!(d[&pcs[1]], ESP);
}

#[test]
fn dirt_flows_round_loops_and_through_joins() {
    // ESI is written after the call, on the way back round the loop; EDX
    // on one side of a branch only. Both reach the call.
    let f = common::function(BASE, |a| {
        let mut top = a.create_label();
        let mut skip = a.create_label();
        a.mov(ecx, 3)?;
        a.set_label(&mut top)?;
        a.test(ecx, 1)?;
        a.jz(skip)?;
        a.mov(edx, ecx)?;
        a.set_label(&mut skip)?;
        a.call(0x0050_0000u64)?;
        a.lea(esi, ptr(ecx + 0x100))?;
        a.dec(ecx)?;
        a.jnz(top)?;
        a.ret()
    });
    let home = homes(&f);
    let d = dirty_at_exits(&f, home);
    let pcs = exits(&f);
    assert_eq!(d[&pcs[0]], ECX | EDX | ESP | ESI | HOME_FLAGS);
}

#[test]
fn an_atomic_resyncs_everything_but_its_old_value() {
    let f = common::function(BASE, |a| {
        a.mov(ebx, 1)?;
        a.lock().xadd(dword_ptr(esi), ecx)?;
        a.call(0x0050_0000u64)?;
        a.ret()
    });
    let home = homes(&f);
    let d = dirty_at_exits(&f, home);
    assert_eq!(d[&exits(&f)[0]], ECX | ESP);
}

/// Builds and runs `entry` from `cpu` on the emitter's host.
fn run(name: &str, f: ir::IrFunction, cpu: &mut CpuAbi) -> Option<TransferAbi> {
    let Some(header) = common::abi_header() else {
        common::skip(name, "needs h2abi.h (H2ABI_HEADER)");
        return None;
    };
    let owners = emit::plan(vec![f]);
    let g = common::build(&owners, &header, name);
    let mut ctx = Ctx::new(FlatMemory::new(MEM, MEM_LEN));
    ctx.mem.load(MEM + 0x800, &HARNESS_EXIT.to_le_bytes());
    Some(
        g.run(cpu, &mut ctx, TransferAbi::new(JUMP, ENTRY, BASE), 16)
            .0,
    )
}

/// A handler that never touches EFLAGS leaves the value it was entered
/// with, but still with bit 1 set, as every exit must (RUNTIME-CONTRACT.md
/// §4, "Exit"); the defined mask is untouched.
#[test]
fn eflags_left_alone_still_exits_canonical() {
    const NAME: &str = "eflags_left_alone_still_exits_canonical";
    let f = common::function(BASE, |a| {
        a.mov(eax, dword_ptr(esp))?;
        a.mov(edx, ebx)?;
        a.ret()
    });
    assert_eq!(homes(&f) & HOME_FLAGS, 0);
    let mut cpu = CpuAbi {
        eflags: 0x0cd5,
        eflags_defined: 0x0c01,
        ..CpuAbi::default()
    };
    cpu.gpr[3] = 0x1234_5678;
    cpu.gpr[4] = MEM + 0x800;
    let Some(t) = run(NAME, f, &mut cpu) else {
        return;
    };
    assert_eq!((t.kind, t.target_pc), (RETURN, HARNESS_EXIT), "{t:x?}");
    assert_eq!((cpu.eflags, cpu.eflags_defined), (0x0cd7, 0x0c01));
    assert_eq!(
        (cpu.gpr[0], cpu.gpr[2], cpu.gpr[3]),
        (HARNESS_EXIT, 0x1234_5678, 0x1234_5678)
    );
}
