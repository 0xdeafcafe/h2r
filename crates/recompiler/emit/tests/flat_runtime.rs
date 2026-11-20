//! The flat memory model in the real runtime
//! (docs/runtime/contract-thin-transfers-flat-ram.md, slices F3 and F5), on
//! synthetic functions (no game data): a module built with `H2_FLAT_RAM`,
//! registered with a `Runtime` and run by its runner, once with the
//! context's fast memory on (the runtime's window) and once off (the null
//! window, where every access takes its fixup to the checked helpers). The
//! two must agree on everything but the fixups taken.
#![cfg(any(target_os = "macos", all(target_os = "linux", target_arch = "x86_64")))]

mod common;

use iced_x86::code_asm::*;
use runtime::executor::SecondExecutor;
use runtime::guest::{FaultReason, Permissions};
use runtime::{CpuAbi, Dispatch, Event, ExecCtx, Guest, Runner, Runtime, TransferAbi, abi, exec::HARNESS_EXIT};
use std::path::Path;
use std::sync::Arc;

const CODE: u32 = 0x0040_0000;
/// 16 KiB aligned: one host page on either host page size.
const DATA: u32 = 0x0060_0000;
/// The stack's page is 60 KiB above DATA, in another host page, so arming
/// DATA's page never guards it.
const STACK: u32 = 0x0061_0000;
/// The NV2A aperture: never RAM, never in the window.
const NV2A: u32 = 0xfd00_0000;

struct Module {
    table: *const runtime::generated::H2GenModule,
    version: runtime::ffi::GeneratedAbiVersion,
    _lib: libloading::Library,
    /// Installs the x87 helpers; kept loaded with the library.
    _host: emit::host::Generated,
}

/// `body` at [`CODE`], built for the flat model whatever the environment
/// says, with its bridge pointed at the production runtime (as
/// direct.rs does), or `None` (reported) where the test cannot run.
fn module(name: &str, body: impl FnOnce(&mut CodeAssembler) -> Result<(), IcedError>) -> Option<Module> {
    let Some(header) = common::abi_header() else {
        common::skip(name, "needs h2abi.h (H2ABI_HEADER)");
        return None;
    };
    if !runtime::window::supported() {
        common::skip(name, "no flat RAM window on this host");
        return None;
    }
    if std::env::var("H2_FASTMEM").as_deref() == Ok("0") {
        common::skip(name, "H2_FASTMEM=0 builds no inline accesses");
        return None;
    }
    let owners = emit::plan(vec![common::function(CODE, body)]);
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("flat-runtime-{name}"));
    let src = dir.join("src");
    emit::write_module_split(&owners, &src, emit::Codegen::accesses(true), emit::c::Split::DEFAULT).unwrap();
    let fastmem = src.join("h2fastmem.h");
    let text = std::fs::read_to_string(&fastmem).unwrap();
    std::fs::write(&fastmem, format!("#undef H2_FLAT_RAM\n#define H2_FLAT_RAM 1\n{text}")).unwrap();
    emit::compile::install_header(&src, &header).unwrap();
    let b = emit::compile::build(&src, &dir, emit::compile::Arch::host(), 2).unwrap_or_else(|e| panic!("{e}"));
    let dylib = dir.join(format!("lib{name}.dylib"));
    emit::compile::host_dylib(&b, &src, &[("bridge.c", emit::host::BRIDGE)], &dylib).unwrap_or_else(|e| panic!("{e}"));
    // SAFETY: built just now for this host.
    let host = unsafe { emit::host::Generated::load(&dylib) }.unwrap();
    assert!(host.flat, "the module carries feature 16");
    // SAFETY: as above.
    let lib = unsafe { libloading::Library::new(&dylib) }.unwrap();
    // SAFETY: the callbacks implement h2abi.h; the table is the module's.
    let (table, version) = unsafe {
        let install: libloading::Symbol<unsafe extern "C" fn(*const *const (), usize) -> u32> =
            lib.get(b"h2bridge_install_v3").unwrap();
        let callbacks = [
            runtime::ffi::h2abi_v1_read as *const (),
            runtime::ffi::h2abi_v1_write as *const (),
            runtime::generated::h2shim_v1_event as *const (),
            runtime::generated::h2shim_v1_unsupported as *const (),
            runtime::generated::h2shim_v1_rdtsc as *const (),
            runtime::ffi::h2abi_v1_atomic_rmw as *const (),
            runtime::ffi::h2abi_v1_xchg as *const (),
            runtime::ffi::h2abi_v1_cmpxchg as *const (),
            runtime::ffi::h2abi_v1_profile as *const (),
            runtime::ffi::h2abi_v1_fast_acquire as *const (),
            runtime::ffi::h2abi_v1_fast_release as *const (),
            runtime::ffi::h2abi_v1_read_counted as *const (),
            runtime::ffi::h2abi_v1_write_counted as *const (),
            runtime::ffi::h2abi_v1_charge as *const (),
            runtime::ffi::h2abi_v1_budget as *const (),
            runtime::ffi::h2abi_v1_exit as *const (),
        ];
        assert_eq!(install(callbacks.as_ptr(), size_of_val(&callbacks)), 1);
        let table: libloading::Symbol<*const runtime::generated::H2GenModule> = lib.get(b"h2gen_module").unwrap();
        let version = *lib.get::<runtime::ffi::GeneratedAbiVersion>(b"h2gen_module_abi_version").unwrap();
        (*table, version)
    };
    Some(Module { table, version, _lib: lib, _host: host })
}

fn dispatch(m: &Module) -> Dispatch {
    let mut d = Dispatch::default();
    // SAFETY: the module's own table and probe, kept loaded by `m`.
    unsafe { d.register_generated_module(&*m.table, m.version) }.unwrap();
    assert!(d.flat_ram());
    d
}

/// A guest with CODE, DATA (16 KiB) and the stack mapped, a return to the
/// harness on the stack, and `setup` applied.
fn guest(setup: impl FnOnce(&mut Guest)) -> Guest {
    let mut g = Guest::new();
    g.map_zeroed(CODE, 0x1000, Permissions::RX).unwrap();
    g.map_zeroed(DATA, 0x4000, Permissions::RW).unwrap();
    g.map_zeroed(STACK - 0x1000, 0x1000, Permissions::RW).unwrap();
    g.set_u32(STACK - 4, HARNESS_EXIT).unwrap();
    setup(&mut g);
    g
}

struct Run {
    rt: Arc<Runtime>,
    ctx: ExecCtx,
    t: TransferAbi,
    cpu: CpuAbi,
    /// Fixups the runtime's window took (the null window's are not counted).
    fixups: u64,
}

/// Run `m` from CODE with `eax` on a fresh runtime over `guest`, in the
/// window or (`window` false) the null window.
fn run(m: &Module, window: bool, eax: u32, guest: Guest) -> Run {
    let rt = Runtime::new(guest, dispatch(m));
    assert!(rt.flat_ram());
    let mut ctx = ExecCtx::new(rt.clone(), 1);
    ctx.enable_fast_memory(window);
    let mut cpu = CpuAbi { eax, esp: STACK - 4, ..CpuAbi::default() };
    let before = rt.window_stats().fixups;
    let t = Runner::default().run(&mut cpu, &mut ctx, CODE);
    let fixups = rt.window_stats().fixups - before;
    Run { rt, ctx, t, cpu, fixups }
}

/// F3: a page the access trap has armed is a guard in the window, so every
/// access to it takes its fixup into the checked helper, which logs it
/// exactly as with the window off: the same hits, and the sticky page's
/// line record counts all three accesses.
#[test]
fn armed_trap_page_logs_every_access() {
    const NAME: &str = "armed_trap_page_logs_every_access";
    let Some(m) = module(NAME, |a| {
        a.mov(ecx, dword_ptr(eax))?;
        a.mov(dword_ptr(eax + 4), ecx)?;
        a.mov(edx, dword_ptr(eax + 0x40))?;
        a.ret()
    }) else {
        return;
    };
    let armed = || {
        guest(|g| {
            g.set_u32(DATA, 0x1234_5678).unwrap();
            g.install_trap(64);
            g.set_sticky_pages(&[DATA >> 12]);
        })
    };
    let runs = [run(&m, true, DATA, armed()), run(&m, false, DATA, armed())];
    for r in &runs {
        assert_eq!((r.t.kind, r.cpu.eip), (abi::STOP, HARNESS_EXIT), "{:?} {:?}", r.t, r.ctx.events());
    }
    assert_eq!(runs[0].cpu, runs[1].cpu, "registers");
    assert_eq!(runs[0].fixups, 3, "each access to the armed page took its fixup, nothing else did");
    let logged = runs[0].rt.memory().take_trap_hits();
    let expected = runs[1].rt.memory().take_trap_hits();
    assert_eq!(logged, expected, "the window logs what the helper path logs");
    let writes: Vec<bool> = logged.hits.iter().map(|h| h.write).collect();
    assert_eq!(writes, [false, true], "the first guest read and write: {:x?}", logged.hits);
    assert!(logged.hits.iter().all(|h| (CODE..CODE + 0x10).contains(&h.pc)), "{:x?}", logged.hits);
    let lines: Vec<_> = logged.lines.iter().filter(|l| l.page == DATA >> 12).collect();
    assert_eq!(lines.len(), 1, "{:?}", logged.lines);
    assert_eq!((lines[0].accesses, lines[0].read, lines[0].write), (3, 0b11, 0b1));
    assert_eq!(runs[0].rt.memory().u32(DATA + 4).unwrap(), 0x1234_5678);
}

/// F3: a load or store at the NV2A aperture, which the window never holds,
/// takes its fixup into the checked helper and faults as the paged path
/// does: the same event, an unsupported device at the instruction.
#[test]
fn mmio_access_faults_as_before() {
    for (store, name) in [(false, "mmio-load"), (true, "mmio-store")] {
        let Some(m) = module(name, |a| {
            if store {
                a.mov(dword_ptr(eax), ecx)?;
            } else {
                a.mov(ecx, dword_ptr(eax))?;
            }
            a.ret()
        }) else {
            return;
        };
        let runs = [run(&m, true, NV2A, guest(|_| {})), run(&m, false, NV2A, guest(|_| {}))];
        let events: Vec<_> = runs.iter().map(|r| r.ctx.event(r.t.event_id).cloned()).collect();
        assert_eq!(events[0], events[1], "{name}: the same fault with the window on and off");
        assert!(
            matches!(&events[0], Some(Event::MemoryFault { pc: CODE, fault })
                if fault.addr == NV2A && fault.reason == FaultReason::UnsupportedDevice),
            "{name}: {:?}",
            events[0]
        );
        assert_eq!(runs[0].cpu, runs[1].cpu, "{name}: registers");
        assert!(runs[0].fixups >= 1, "{name}: the window access faulted and took its fixup");
    }
}

/// F5: a second executor's view, refreshed from a flat-RAM runtime, runs
/// its generated code natively in its own window (no fixups), on the
/// snapshot's bytes, without touching the live RAM; a second snapshot
/// re-aliases the new copy.
#[test]
fn executor_view_runs_native_in_its_window() {
    const NAME: &str = "executor_view_runs_native_in_its_window";
    let Some(m) = module(NAME, |a| {
        a.mov(ecx, dword_ptr(eax))?;
        a.add(ecx, 1)?;
        a.mov(dword_ptr(eax + 4), ecx)?;
        a.ret()
    }) else {
        return;
    };
    let live = Runtime::new(guest(|g| g.set_u32(DATA, 41).unwrap()), dispatch(&m));
    let mut exec = SecondExecutor::new(&live, |_, _| false);
    assert!(exec.runtime().flat_ram());
    for round in 0..2u32 {
        exec.snapshot(&live);
        let before = exec.runtime().window_stats().fixups;
        let mut cpu = CpuAbi { eax: DATA, esp: STACK - 0x100, ..CpuAbi::default() };
        exec.call(&mut cpu, CODE, &[]).unwrap_or_else(|e| panic!("round {round}: {e}"));
        assert_eq!(exec.runtime().window_stats().fixups - before, 0, "round {round}: every access native");
        assert_eq!(exec.runtime().memory().u32(DATA + 4).unwrap(), 42 + round, "round {round}");
        assert_eq!(live.memory().u32(DATA + 4).unwrap(), 0, "round {round}: the live RAM is untouched");
        live.memory().set_u32(DATA, 42 + round).unwrap();
    }
}
