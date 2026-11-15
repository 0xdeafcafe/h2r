//! Shared by the emitter's integration tests: build a set of owners into a
//! host dylib and load it.

#![allow(dead_code)]

use std::io::Write;
use std::path::{Path, PathBuf};

use emit::compile::{self, Arch};
use emit::host::{BRIDGE, Generated};
use emit::Owner;

/// Report a skipped test where `cargo test` shows it: straight to the
/// stream, since the print macros of passing tests are captured.
pub fn skip(test: &str, why: &str) {
    let _ = writeln!(std::io::stderr(), "SKIPPED {test}: {why}");
}

/// The runtime's ABI header: `H2ABI_HEADER`, or the workspace's copy.
/// It is only ever read, and copied into the test's output directory.
pub fn abi_header() -> Option<PathBuf> {
    let p = std::env::var_os("H2ABI_HEADER")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../runtime/include/h2abi.h"));
    p.is_file().then_some(p)
}

/// The retail XBE: `H2_XBE`, or `disc/default.xbe`. Never in the repo.
pub fn xbe_path() -> Option<PathBuf> {
    let p = std::env::var_os("H2_XBE")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../disc/default.xbe"));
    p.is_file().then_some(p)
}

/// Emit, compile (host architecture) and load `owners` as a module named
/// `name`, under Cargo's per-test scratch directory in `target/`.
pub fn build(owners: &[Owner], header: &Path, name: &str) -> Generated {
    build_with(owners, header, name, emit::Codegen::default())
}

/// [`build`] with the given code generation choices.
#[allow(dead_code)]
pub fn build_with(owners: &[Owner], header: &Path, name: &str, codegen: emit::Codegen) -> Generated {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let src = dir.join("src");
    emit::write_module_with(owners, &src, codegen).unwrap();
    compile::install_header(&src, header).unwrap();
    let jobs = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let b = compile::build(&src, &dir, Arch::host(), jobs).unwrap_or_else(|e| panic!("{e}"));
    let dylib = dir.join(format!("lib{name}.dylib"));
    compile::host_dylib(&b, &src, &[("bridge.c", BRIDGE)], &dylib).unwrap_or_else(|e| panic!("{e}"));
    // SAFETY: built just now from this crate's output for this host.
    unsafe { Generated::load(&dylib) }.unwrap()
}

/// Assemble `body` at `base` and split it into blocks at every label the
/// code branches to and after every control transfer.
#[allow(dead_code)]
pub fn function(base: u32, body: impl FnOnce(&mut iced_x86::code_asm::CodeAssembler) -> Result<(), iced_x86::IcedError>) -> ir::IrFunction {
    let mut a = iced_x86::code_asm::CodeAssembler::new(32).unwrap();
    body(&mut a).unwrap();
    let code = a.assemble(base as u64).unwrap();
    let env = lift::Env::default();
    let insns: Vec<_> = iced_x86::Decoder::with_ip(32, &code, base as u64, iced_x86::DecoderOptions::NONE)
        .into_iter()
        .map(|i| lift::lower(&i, &env, lift::Site::Plain))
        .collect();
    let mut starts = std::collections::BTreeSet::from([base]);
    for i in &insns {
        match &i.control {
            ir::Control::Next => {}
            ir::Control::Branch { taken, fallthrough, .. } => {
                starts.extend([taken.pc, fallthrough.pc]);
            }
            ir::Control::Jump { target: ir::Target::Direct(t), .. } => {
                starts.extend([*t, i.next_pc()]);
            }
            _ => {
                starts.insert(i.next_pc());
            }
        }
    }
    let mut blocks: std::collections::BTreeMap<u32, ir::IrBlock> = std::collections::BTreeMap::new();
    let mut current = base;
    for i in insns {
        if starts.contains(&i.pc) {
            current = i.pc;
        }
        blocks.entry(current).or_insert_with(|| ir::IrBlock { start: current, insns: vec![] }).insns.push(i);
    }
    let f = ir::IrFunction { entry: base, blocks, interior_entries: vec![], errors: vec![] };
    f.validate().unwrap_or_else(|e| panic!("{e}\n{f}"));
    f
}
