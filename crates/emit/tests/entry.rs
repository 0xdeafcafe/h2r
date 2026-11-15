//! LIFTER.md §9 through generated C: run the retail image's entry to its
//! first kernel call in compiled handlers and check it ends exactly where
//! the IR interpreter does, with the same CPU state and memory.
//!
//! Needs the user's XBE (`H2_XBE` or `disc/default.xbe`) and the runtime's
//! `h2abi.h` (`H2ABI_HEADER` or the workspace copy); without them it says
//! so and passes. By default the module holds the functions the
//! interpreter's run lifted (seconds to build); `H2_EMIT_FULL=1` builds the
//! whole image instead (minutes, cached afterwards).

mod common;

use emit::host::{self, CALL, Ctx, ENTRY, JUMP, TransferAbi};
use lift::harness::{self, Stop};

#[test]
fn entry_reaches_the_same_kernel_call_through_generated_c() {
    const NAME: &str = "entry_reaches_the_same_kernel_call_through_generated_c";
    let (Some(xbe_path), Some(header)) = (common::xbe_path(), common::abi_header()) else {
        common::skip(NAME, "needs the XBE (H2_XBE) and h2abi.h (H2ABI_HEADER)");
        return;
    };
    let data = std::fs::read(xbe_path).unwrap();
    let x = xbe::Xbe::parse(&data).unwrap();
    let analysis = analyze::analyze(&x).unwrap();
    let lifter = lift::Lifter::new(&analysis, &x).unwrap();

    // The interpreter's run is the reference.
    let reference = harness::run_to_kernel(&x, &lifter, 1_000_000).unwrap();
    let Stop::KernelCall { pc, target, continuation, .. } = reference.stop else {
        panic!("interpreter stopped with {:?}", reference.stop)
    };
    assert_eq!((pc, target, continuation), (0x2d21ff, 0x8000_00ff, 0x2d2205), "the retail XBE's first kernel call");

    let full = std::env::var_os("H2_EMIT_FULL").is_some();
    let owners = if full {
        emit::plan_image(&analysis, &x, emit::opt::Passes::from_env().unwrap()).unwrap()
    } else {
        emit::plan(reference.functions.iter().filter_map(|&e| lifter.lift(e)).collect())
    };
    let g = common::build(&owners, &header, if full { "entry_full" } else { "entry_path" });

    let (mut cpu, mut ctx): (_, Ctx) = host::entry_state(&x).unwrap();
    let (t, calls) = g.run(&mut cpu, &mut ctx, TransferAbi::new(JUMP, ENTRY, x.entry), 100_000);

    // The handler leaves with the call through the import slot, the
    // continuation pushed, exactly as the interpreter stopped.
    assert_eq!((t.kind, t.route, t.target_pc, t.resume_pc), (CALL, ENTRY, target, continuation), "{t:x?}");
    assert!(ctx.events.is_empty(), "{:?}", ctx.events);
    assert!(calls > 1, "the path crosses handlers");
    assert_eq!(cpu.to_interp(), reference.cpu, "CPU state at the kernel call");
    let diff = ctx.mem.bytes.iter().zip(&reference.mem.bytes).position(|(a, b)| a != b);
    assert_eq!(diff, None, "first differing byte of guest memory");
    // The two stores the startup path makes (LIFTER.md §9).
    assert_eq!(ctx.mem.read_u32(0x55f138), Some(0x94));
    assert_eq!(ctx.mem.read_u32(0x480100), Some(0xffff_ffdb));
}
