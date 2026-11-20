//! The flat memory model end to end (docs/runtime/contract-thin-transfers-flat-ram.md,
//! section 4.1, slice F3), on synthetic functions (no game data): a module
//! built with `H2_FLAT_RAM` runs in the test host's window and again
//! against its null window, where every access takes its fixup to the
//! checked helpers, and the two must agree on registers, memory, charges
//! and faults.
#![cfg(all(unix, any(target_arch = "x86_64", target_arch = "aarch64")))]

mod common;

use std::path::Path;

use emit::compile::{self, Arch};
use emit::host::{BRIDGE, CpuAbi, Ctx, ENTRY, FAULT, Generated, HARNESS_EXIT, HostEvent, JUMP, RETURN, TransferAbi};
use iced_x86::code_asm::*;
use ir::interp::{FlatMemory, MemFault};

const A: u32 = 0x0040_0000;
/// Host-page aligned on 4 KiB and 16 KiB hosts, so the window maps it all.
const MEM: u32 = 0x0060_0000;
const MEM_LEN: usize = 0x1_0000;
const STACK: u32 = MEM + 0xff00;
/// A 16 KiB boundary inside the memory: on either host page size, both
/// sides are mapped.
const CROSS: u32 = MEM + 0x8000;

/// [`common::build_with`] with the flat model selected in the module's
/// own headers, whatever the environment says.
fn build_flat(owners: &[emit::Owner], header: &Path, name: &str, codegen: emit::Codegen) -> Generated {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let src = dir.join("src");
    emit::write_module_split(owners, &src, codegen, emit::c::Split::DEFAULT).unwrap();
    let fastmem = src.join("h2fastmem.h");
    let text = std::fs::read_to_string(&fastmem).unwrap();
    std::fs::write(&fastmem, format!("#undef H2_FLAT_RAM\n#define H2_FLAT_RAM 1\n{text}")).unwrap();
    compile::install_header(&src, header).unwrap();
    let jobs = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let b = compile::build(&src, &dir, Arch::host(), jobs).unwrap_or_else(|e| panic!("{e}"));
    let dylib = dir.join(format!("lib{name}.dylib"));
    compile::host_dylib(&b, &src, &[("bridge.c", BRIDGE)], &dylib).unwrap_or_else(|e| panic!("{e}"));
    // SAFETY: built just now from this crate's output for this host.
    unsafe { Generated::load(&dylib) }.unwrap()
}

/// The module, or `None` (reported) where the test cannot run.
fn module(test: &str, codegen: emit::Codegen, body: impl FnOnce(&mut CodeAssembler) -> Result<(), IcedError>) -> Option<Generated> {
    let Some(header) = common::abi_header() else {
        common::skip(test, "needs h2abi.h (H2ABI_HEADER)");
        return None;
    };
    if std::env::var("H2_FASTMEM").as_deref() == Ok("0") {
        common::skip(test, "H2_FASTMEM=0 builds no inline accesses");
        return None;
    }
    let f = common::function(A, body);
    let name = format!("flat-{test}-{}", u8::from(codegen.accounting));
    let g = build_flat(&emit::plan(vec![f]), &header, &name, codegen);
    assert!(g.flat, "the module carries feature 16");
    Some(g)
}

struct Run {
    t: TransferAbi,
    cpu: CpuAbi,
    mem: Vec<u8>,
    events: Vec<HostEvent>,
    charged: u64,
    fixups: u64,
}

/// Run `g` from A with `gpr` (esp set to a return to the harness), in the
/// window or the null window.
fn run(g: &Generated, gpr: [u32; 8], init: &[(u32, &[u8])], null: bool) -> Run {
    let mut mem = FlatMemory::new(MEM, MEM_LEN);
    for (at, bytes) in init {
        assert!(mem.load(*at, bytes));
    }
    mem.load(STACK - 4, &HARNESS_EXIT.to_le_bytes());
    let mut cpu = CpuAbi { gpr, ..CpuAbi::default() };
    cpu.gpr[4] = STACK - 4;
    let mut ctx = Ctx::new(mem);
    ctx.null_window = null;
    let (t, _) = g.run(&mut cpu, &mut ctx, TransferAbi::new(JUMP, ENTRY, A), 16);
    Run { t, cpu, mem: ctx.mem.bytes.clone(), events: ctx.events.clone(), charged: ctx.charged, fixups: ctx.fixups }
}

/// Both runs, checked equal in all but the fixups they took.
fn both(g: &Generated, gpr: [u32; 8], init: &[(u32, &[u8])]) -> (Run, Run) {
    let w = run(g, gpr, init, false);
    let n = run(g, gpr, init, true);
    assert_eq!((w.t.kind, w.t.target_pc), (n.t.kind, n.t.target_pc), "transfer");
    assert_eq!(w.cpu, n.cpu, "registers");
    assert!(w.mem == n.mem, "memory");
    assert_eq!(w.events, n.events, "events");
    assert_eq!(w.charged, n.charged, "accesses charged");
    assert!(n.fixups > 0, "the null window takes every access through its fixup");
    (w, n)
}

/// Loads then stores of 1, 2, 4, 8 and 16 bytes from `[eax]`.
fn widths(a: &mut CodeAssembler) -> Result<(), IcedError> {
    a.mov(ecx, dword_ptr(eax))?;
    a.movzx(edx, word_ptr(eax + 4))?;
    a.movzx(ebx, byte_ptr(eax + 7))?;
    a.movq(xmm0, qword_ptr(eax + 8))?;
    a.movdqu(xmm1, xmmword_ptr(eax + 16))?;
    a.add(ecx, 1)?;
    a.mov(dword_ptr(eax + 32), ecx)?;
    a.mov(word_ptr(eax + 36), dx)?;
    a.mov(byte_ptr(eax + 38), bl)?;
    a.movq(qword_ptr(eax + 40), xmm0)?;
    a.movdqu(xmmword_ptr(eax + 48), xmm1)?;
    a.ret()
}

fn pattern() -> Vec<u8> {
    (0..64u8).map(|i| i.wrapping_mul(37).wrapping_add(11)).collect()
}

#[test]
fn window_matches_null_window_at_every_width_and_edge() {
    for accounting in [false, true] {
        let codegen = emit::Codegen::accesses(accounting);
        let Some(g) = module("widths", codegen, widths) else { return };
        let bytes = pattern();
        // Inside one page, then straddling every width across CROSS.
        for at in [MEM + 0x1000, CROSS - 3, CROSS - 7, CROSS - 13, CROSS - 33, CROSS - 47] {
            let (w, _) = both(&g, [at, 0, 0, 0, 0, 0, 0, 0], &[(at, &bytes)]);
            assert_eq!((w.t.kind, w.t.target_pc), (RETURN, HARNESS_EXIT), "at {at:#x}");
            assert_eq!(w.fixups, 0, "every access at {at:#x} is native in the window");
        }
        // The last bytes of the memory: the loads fit, the stores run off
        // the end and fault, in both the same way.
        let at = MEM + MEM_LEN as u32 - 40;
        let (w, _) = both(&g, [at, 0, 0, 0, 0, 0, 0, 0], &[(at, &bytes[..40])]);
        assert_eq!(w.t.kind, FAULT);
        assert!(w.fixups > 0, "the access past the mapped pages took its fixup");
    }
}

#[test]
fn page_crossing_access_is_native() {
    let Some(g) = module("crossing", emit::Codegen::default(), |a| {
        a.mov(ecx, dword_ptr(eax))?;
        a.mov(dword_ptr(eax + 4), ecx)?;
        a.ret()
    }) else {
        return;
    };
    for at in [CROSS - 1, CROSS - 2, CROSS - 3, CROSS - 5] {
        let (w, _) = both(&g, [at, 0, 0, 0, 0, 0, 0, 0], &[(at, &[1, 2, 3, 4])]);
        assert_eq!(w.cpu.gpr[1], 0x0403_0201);
        assert_eq!(w.fixups, 0, "{at:#x}: one instruction across the boundary, no fixup");
    }
}

#[test]
fn access_past_4gib_takes_the_fixup() {
    let Some(g) = module("past4g", emit::Codegen::default(), |a| {
        a.mov(ecx, dword_ptr(eax))?;
        a.ret()
    }) else {
        return;
    };
    // base + 0xffff_fffe + 4 runs into the guard page above the window.
    let (w, _) = both(&g, [0xffff_fffe, 0, 0, 0, 0, 0, 0, 0], &[]);
    assert_eq!(w.t.kind, FAULT);
    assert!(w.fixups >= 1, "the window access faulted and took its fixup");
    assert!(
        matches!(w.events.first(), Some(&HostEvent::Memory { pc: A, fault: MemFault { write: false, .. } })),
        "{:?}",
        w.events
    );
}

#[test]
fn unmapped_access_faults_as_before() {
    let Some(g) = module("unmapped", emit::Codegen::default(), |a| {
        a.mov(dword_ptr(eax), ecx)?;
        a.ret()
    }) else {
        return;
    };
    let (w, _) = both(&g, [0x0090_0000, 7, 0, 0, 0, 0, 0, 0], &[]);
    assert_eq!(w.t.kind, FAULT);
    assert!(w.fixups >= 1, "the window access faulted and took its fixup");
    assert_eq!(
        w.events.first(),
        Some(&HostEvent::Memory { pc: A, fault: MemFault { addr: 0x0090_0000, write: true } })
    );
}

#[test]
fn bulk_copy_matches_in_both_windows() {
    let Some(g) = module("movs", emit::Codegen::default(), |a| {
        a.rep().movsd()?;
        a.ret()
    }) else {
        return;
    };
    let bytes = pattern();
    // Forward, within a page and across CROSS, then overlapping by one
    // element (x86 propagates, not memmove).
    for (src, dst, n) in [(MEM + 0x100, MEM + 0x2000, 16), (CROSS - 24, CROSS + 0x100, 16), (MEM + 0x300, MEM + 0x304, 15)] {
        let (w, _) = both(&g, [0, n, 0, 0, 0, 0, src, dst], &[(src, &bytes)]);
        assert_eq!((w.t.kind, w.t.target_pc), (RETURN, HARNESS_EXIT));
        assert_eq!((w.cpu.gpr[1], w.cpu.gpr[6], w.cpu.gpr[7]), (0, src + 4 * n, dst + 4 * n));
    }
}
