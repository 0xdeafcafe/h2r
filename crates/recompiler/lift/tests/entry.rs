//! The LIFTER.md §9 milestone on the real image: interpret from the entry
//! point to the first kernel call and check the state there.
//!
//! Needs the user's XBE, which is never in the repo: set `H2_XBE` to a
//! `default.xbe`, or put one at `disc/default.xbe`. Without it the test
//! prints a note and passes, so `cargo test` works on a fresh checkout.

use std::io::Write;
use std::path::PathBuf;

use ir::Gpr;
use lift::harness::{Stop, run_to_kernel};

fn xbe_path() -> Option<PathBuf> {
    let p = std::env::var_os("H2_XBE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../disc/default.xbe"));
    p.is_file().then_some(p)
}

#[test]
fn entry_runs_to_ps_create_system_thread_ex() {
    let Some(path) = xbe_path() else {
        // Straight to the stream, not eprintln!: the test harness captures
        // the print macros of passing tests, and a skip should be seen.
        let _ = writeln!(std::io::stderr(), "SKIPPED entry_runs_to_ps_create_system_thread_ex: no XBE (set H2_XBE)");
        return;
    };
    let data = std::fs::read(path).unwrap();
    let x = xbe::Xbe::parse(&data).unwrap();
    assert_eq!(x.entry, 0x2d0aee, "these numbers are for the retail Halo 2 XBE");
    let analysis = analyze::analyze(&x).unwrap();
    let lifter = lift::Lifter::new(&analysis, &x).unwrap();
    let run = run_to_kernel(&x, &lifter, 100_000).unwrap();

    // Stopped at the CreateThread wrapper's call through slot 0x4115c4,
    // ordinal 255 (PsCreateSystemThreadEx), with the return pushed.
    assert_eq!(
        run.stop,
        Stop::KernelCall { pc: 0x2d21ff, slot: 0x4115c4, ordinal: 255, target: 0x8000_00FF, continuation: 0x2d2205 }
    );
    let esp = run.cpu.reg(Gpr::Esp);
    let arg = |i: u32| run.mem.read_u32(esp + 4 * i).unwrap();
    assert_eq!(arg(0), 0x2d2205);

    // The ten stdcall arguments.
    let ebp = run.cpu.reg(Gpr::Ebp);
    assert_eq!(arg(1), ebp + 0xc, "ThreadHandle points into CreateThread's frame");
    assert_eq!(arg(2), 0, "ThreadExtensionSize");
    assert_eq!(arg(3), 0x40000, "KernelStackSize");
    assert_eq!(arg(4), 0x94, "TlsDataSize");
    assert_eq!(arg(5), 0, "ThreadId");
    assert_eq!(arg(6), 0x2d0a7a, "StartRoutine");
    assert_eq!(arg(7), 0, "StartContext");
    assert_eq!(arg(8), 0, "CreateSuspended");
    assert_eq!(arg(9), 0, "DebuggerThread");
    assert_eq!(arg(10), 0x2d212e, "SystemRoutine");

    // The startup arithmetic's stores: TLS size, and the TLS index from
    // the signed division 0x94 / -4.
    assert_eq!(run.mem.read_u32(0x55f138), Some(0x94));
    assert_eq!(run.mem.read_u32(0x480100), Some(0xffff_ffdb));
}
