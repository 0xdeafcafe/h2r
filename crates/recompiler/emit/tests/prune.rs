//! A pruned lift (docs/lifter/lift-pruning.md, slice 3) on a synthetic XBE,
//! never retail bytes: the entry calls A, A calls the replaced B, and only
//! B calls C. `plan_image` with the keep set lifts the entry and A; the
//! module carries the pruned table on both tiers; on the real runtime a
//! call of B or C, and a return into B's body, stop with `Event::Pruned`
//! naming B.

mod common;

use std::collections::BTreeMap;
use std::path::Path;

use emit::pruned::{Range, Replaced, Root, Table};
use runtime::guest::Permissions;
use runtime::{CpuAbi, Dispatch, Event, ExecCtx, Guest, Runner, Runtime, TransferAbi, abi};

const BASE: u32 = 0x10000;
const TEXT: u32 = 0x11000;
const RDATA: u32 = 0x12000;
const ENTRY: u32 = TEXT;
const A: u32 = TEXT + 0x10;
const B: u32 = TEXT + 0x20;
const C: u32 = TEXT + 0x40;
/// B's `ret`, after its call of C.
const B_BODY: u32 = B + 5;
const STACK: u32 = 0x20_0000;

/// A minimal retail XBE: `.text` at 0x11000 holding `code`, and `.rdata` at
/// 0x12000 with a kernel thunk table importing KeBugCheck.
fn xbe_with(code: &[u8]) -> Vec<u8> {
    let mut d = vec![0u8; 0x1200];
    let put = |d: &mut Vec<u8>, o: usize, v: u32| d[o..o + 4].copy_from_slice(&v.to_le_bytes());
    d[..4].copy_from_slice(b"XBEH");
    put(&mut d, 0x104, BASE);
    put(&mut d, 0x108, 0x1000);
    put(&mut d, 0x10C, 0x3000);
    put(&mut d, 0x118, BASE + 0x400);
    put(&mut d, 0x11C, 2);
    put(&mut d, 0x120, BASE + 0x200);
    put(&mut d, 0x128, TEXT ^ 0xA8FC_57AB);
    put(&mut d, 0x158, RDATA ^ 0x5B6D_40B6);
    for (at, va, name, off) in [(0x200, TEXT, b".text\0".as_slice(), 0x1000), (0x238, RDATA, b".rdata\0", 0x1100)] {
        put(&mut d, at, if va == TEXT { 0x06 } else { 0x02 });
        put(&mut d, at + 4, va);
        put(&mut d, at + 8, 0x100);
        put(&mut d, at + 12, off);
        put(&mut d, at + 16, 0x100);
        let n = if va == TEXT { 0x280 } else { 0x290 };
        put(&mut d, at + 20, BASE + n as u32);
        d[n..n + name.len()].copy_from_slice(name);
    }
    d[0x1000..0x1100].fill(0xCC);
    d[0x1000..0x1000 + code.len()].copy_from_slice(code);
    put(&mut d, 0x1100, 0x8000_0000 | 95);
    d
}

/// entry: call A; ret. A: call B; ret. B: call C; ret. C: mov eax, 7; ret.
fn image() -> Vec<u8> {
    let mut c = vec![0xCCu8; 0x50];
    let mut at = |va: u32, bytes: &[u8]| c[(va - TEXT) as usize..][..bytes.len()].copy_from_slice(bytes);
    let call = |from: u32, to: u32| {
        let [r0, r1, r2, r3] = to.wrapping_sub(from + 5).to_le_bytes();
        [0xE8, r0, r1, r2, r3, 0xC3]
    };
    at(ENTRY, &call(ENTRY, A));
    at(A, &call(A, B));
    at(B, &call(B, C));
    at(C, &[0xB8, 7, 0, 0, 0, 0xC3]);
    xbe_with(&c)
}

fn replaced() -> Replaced {
    let root = Root { entry: B, point: "B_POINT".into(), feature: "b-port".into() };
    Replaced { entries: BTreeMap::from([(B, root)]), calls: Default::default() }
}

/// The pruned lift: its owners and its table.
fn lift() -> (Vec<emit::Owner>, Table) {
    let data = image();
    let x = xbe::Xbe::parse(&data).unwrap();
    let analysis = analyze::analyze(&x).unwrap();
    let r = replaced();
    let keep = r.keep(&x, &analysis);
    let owners =
        emit::plan_image(&analysis, &x, emit::opt::Passes::from_env().unwrap(), Some(&keep)).unwrap();
    let table = Table::new(&x, &analysis, &keep, &r, &owners);
    (owners, table)
}

#[test]
fn a_pruned_lift_keeps_the_caller_and_tables_what_the_replaced_entry_alone_reaches() {
    let (owners, table) = lift();
    let lifted: Vec<u32> = owners.iter().map(|o| o.func.entry).collect();
    assert_eq!(lifted, [ENTRY, A]);
    assert_eq!(table.roots, replaced().entries.into_values().collect::<Vec<_>>());
    assert_eq!(
        table.ranges,
        [Range { start: B, end: B + 6, root: B }, Range { start: C, end: C + 6, root: B }]
    );
    // Unpruned, everything is lifted.
    let data = image();
    let x = xbe::Xbe::parse(&data).unwrap();
    let analysis = analyze::analyze(&x).unwrap();
    let all = emit::plan_image(&analysis, &x, emit::opt::Passes::from_env().unwrap(), None).unwrap();
    assert_eq!(all.iter().map(|o| o.func.entry).collect::<Vec<_>>(), [ENTRY, A, B, C]);
}

/// The clang tier's module, its bridge pointed at the production runtime
/// (as direct.rs does), registered with a dispatch holding its pruned
/// table as boot's `load_module` reads it.
struct Module {
    _lib: libloading::Library,
    _host: emit::host::Generated,
    table: *const runtime::generated::H2GenModule,
    version: runtime::ffi::GeneratedAbiVersion,
    pruned: Table,
}

/// The exported table of a loaded module.
///
/// # Safety
/// `lib` is a module built by this crate.
unsafe fn exported_table(lib: &libloading::Library) -> Table {
    // SAFETY: the symbol is a NUL-terminated byte array (pruned.rs).
    let text = unsafe {
        let sym = lib.get::<*const std::ffi::c_char>(emit::pruned::SYMBOL.as_bytes()).unwrap();
        std::ffi::CStr::from_ptr(*sym).to_str().unwrap().to_owned()
    };
    Table::parse(&text).unwrap()
}

fn clang_module(name: &str) -> Option<Module> {
    let Some(header) = common::abi_header() else {
        common::skip(name, "needs h2abi.h (H2ABI_HEADER)");
        return None;
    };
    let (owners, table) = lift();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("prune-{name}"));
    let src = dir.join("src");
    emit::write_module_with(&owners, &src, emit::Codegen::accesses(true)).unwrap();
    emit::pruned::write_c(&src, Some(&table)).unwrap();
    for e in std::fs::read_dir(&src).unwrap().flatten() {
        let text = std::fs::read_to_string(e.path()).unwrap_or_default();
        for left_out in [B, C] {
            assert!(!text.contains(&emit::c::symbol(left_out)), "{} defines or names {left_out:#x}", e.path().display());
        }
    }
    emit::compile::install_header(&src, &header).unwrap();
    let b = emit::compile::build(&src, &dir, emit::compile::Arch::host(), 2).unwrap_or_else(|e| panic!("{e}"));
    let dylib = dir.join(emit::compile::Arch::host().module_file_name());
    emit::compile::host_dylib(&b, &src, &[("bridge.c", emit::host::BRIDGE)], &dylib).unwrap_or_else(|e| panic!("{e}"));
    // SAFETY: built just now for this host.
    let host = unsafe { emit::host::Generated::load(&dylib) }.unwrap();
    // SAFETY: as above; the callbacks implement h2abi.h.
    let lib = unsafe { libloading::Library::new(&dylib) }.unwrap();
    let (module, version) = unsafe {
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
    // SAFETY: built by this crate just now.
    let pruned = unsafe { exported_table(&lib) };
    assert_eq!(pruned, table, "the module carries the table it was built with");
    Some(Module { _lib: lib, _host: host, table: module, version, pruned })
}

/// A machine over the module: the text mapped executable, a return to the
/// harness on the stack.
fn machine(m: &Module) -> (ExecCtx, CpuAbi) {
    let mut d = Dispatch::default();
    // SAFETY: the module's own table and probe, kept loaded by `m`.
    unsafe { d.register_generated_module(&*m.table, m.version) }.unwrap();
    let ranges = m.pruned.ranges.iter().map(|r| runtime::pruned::Range { start: r.start, end: r.end, root: r.root });
    d.set_pruned(runtime::pruned::Pruned::new(
        ranges.collect(),
        m.pruned.roots.iter().map(|r| (r.entry, r.point.clone())),
    ));
    let mut g = Guest::new();
    g.map_zeroed(TEXT, 0x1000, Permissions::RX).unwrap();
    g.map_zeroed(STACK - 0x1000, 0x1000, Permissions::RW).unwrap();
    g.set_u32(STACK - 4, runtime::exec::HARNESS_EXIT).unwrap();
    let ctx = ExecCtx::new(Runtime::new(g, d), 1);
    (ctx, CpuAbi { esp: STACK - 4, ..CpuAbi::default() })
}

/// The stop's event: `(caller, target, root, point)`.
fn pruned_stop(ctx: &ExecCtx, t: TransferAbi) -> (u32, u32, u32, String) {
    assert_eq!(t.kind, abi::STOP, "{t:x?}");
    match ctx.event(t.event_id) {
        Some(Event::Pruned { caller, target, root, point }) => (*caller, *target, *root, point.to_string()),
        other => panic!("not a pruned stop: {other:x?}"),
    }
}

#[test]
fn calls_and_returns_into_left_out_code_stop_naming_the_replaced_entry() {
    const NAME: &str = "calls_and_returns_into_left_out_code_stop_naming_the_replaced_entry";
    let Some(m) = clang_module(NAME) else { return };
    let runner = Runner::default();
    let before = runtime::pruned::hits();

    // A's call of the replaced B: no hook answers here, so it stops, and
    // the caller is A's return address.
    let (mut ctx, mut cpu) = machine(&m);
    let t = runner.run(&mut cpu, &mut ctx, A);
    assert_eq!(pruned_stop(&ctx, t), (A + 5, B, B, "B_POINT".into()));
    let report = ctx.report(t.event_id).unwrap().summary;
    assert!(report.contains("B_POINT") && report.contains("H2_LIFT_PRUNE=0"), "{report}");

    // C, reached only from B: its stop names B.
    let (mut ctx, mut cpu) = machine(&m);
    let t = runner.run_from(&mut cpu, &mut ctx, TransferAbi::call(C, B_BODY));
    assert_eq!(pruned_stop(&ctx, t), (B_BODY, C, B, "B_POINT".into()));

    // A return into B's body.
    let (mut ctx, mut cpu) = machine(&m);
    cpu.eip = C + 5;
    let t = runner.run_from(&mut cpu, &mut ctx, TransferAbi::ret(B_BODY));
    assert_eq!(pruned_stop(&ctx, t), (C + 5, B_BODY, B, "B_POINT".into()));

    // The kept entry still runs, as far as the cut.
    let (mut ctx, mut cpu) = machine(&m);
    let t = runner.run(&mut cpu, &mut ctx, ENTRY);
    assert_eq!(pruned_stop(&ctx, t).1, B);
    assert!(runtime::pruned::hits() >= before + 4);
}

/// The fast tier's module carries the same table, and no handler for the
/// left-out functions.
#[cfg(unix)]
#[test]
fn the_fast_tier_module_carries_the_same_table() {
    let (owners, table) = lift();
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("prune-fast");
    let dylib = dir.join(emit::compile::Arch::host().module_file_name());
    clif::build_dylib_pruned(&owners, &dir, &dylib, 1, Some(&table)).unwrap_or_else(|e| panic!("{e}"));
    // SAFETY: a fast-tier module built just now; only data is read.
    let lib = unsafe { libloading::Library::new(&dylib) }.unwrap();
    assert_eq!(unsafe { exported_table(&lib) }, table);
    for (entry, defined) in [(A, true), (B, false), (C, false)] {
        // SAFETY: only the symbol's presence is checked.
        let found = unsafe { lib.get::<*const ()>(emit::c::symbol(entry).as_bytes()) }.is_ok();
        assert_eq!(found, defined, "{}", emit::c::symbol(entry));
    }
    // Unpruned, the module has no table.
    clif::build_dylib(&owners, &dir, &dylib.with_extension("unpruned"), 1).unwrap_or_else(|e| panic!("{e}"));
    let lib = unsafe { libloading::Library::new(dylib.with_extension("unpruned")) }.unwrap();
    assert!(unsafe { lib.get::<*const u8>(emit::pruned::SYMBOL.as_bytes()) }.is_err());
}
