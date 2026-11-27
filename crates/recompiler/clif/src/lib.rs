//! The Cranelift fast tier: planned IR (`emit::Owner`) straight to host
//! objects, linked into the same kind of module clang's build makes
//! (docs/lifter/cranelift-tier.md).
//!
//! A module is the shard objects from [`compile_shard`] (one handler
//! `h2fn_<entry>` per owner, the C signature, and each shard's ABI probe),
//! the registry ([`module::registry`]) and the bridge
//! ([`module::bridge`]), all Cranelift objects, linked by the platform's
//! linker alone: no C is compiled. Everything derived from the disc stays
//! in the caller's output directory.

mod abi;
pub mod lower;
pub mod module;
mod x87;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::sync::atomic::{AtomicUsize, Ordering};

use cranelift_codegen::ir::{AbiParam, InstBuilder, types};
use cranelift_codegen::isa::OwnedTargetIsa;
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::Context;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{Linkage, Module, default_libcall_names};
use cranelift_object::{ObjectBuilder, ObjectModule};
use emit::Owner;

pub use abi::{BOOT_BRIDGE_SIZE, CLIF_COUNT, TEST_BRIDGE_SIZE};
pub use lower::Unsupported;

/// The registry's and the bridge's object stems, next to the shards'.
pub const REGISTRY_STEM: &str = "h2clif_registry";
pub const BRIDGE_STEM: &str = "h2clif_bridge";

/// The handler symbol, as the C tier names it.
pub fn symbol(entry: u32) -> String {
    emit::c::symbol(entry)
}

/// The object a handler goes in: the C tier's shard of its entry.
pub fn stem(entry: u32) -> String {
    format!("shard_{:05x}", entry >> emit::SHARD_SHIFT)
}

pub use emit::compile::clif_opt_level as opt_level;

/// The host target, set up for objects linked into a dylib.
pub fn isa() -> Result<OwnedTargetIsa, String> {
    let mut flags = settings::builder();
    let set = |f: &mut settings::Builder, k: &str, v: &str| f.set(k, v).map_err(|e| format!("{k}={v}: {e}"));
    set(&mut flags, "opt_level", opt_level()?)?;
    set(&mut flags, "is_pic", "true")?;
    // Sampling profilers and backtraces walk frame pointers.
    set(&mut flags, "preserve_frame_pointers", "true")?;
    // Release skips it (time); the tests turn it on with H2_CLIF_VERIFY=1.
    let verify = cfg!(debug_assertions) || std::env::var_os("H2_CLIF_VERIFY").is_some_and(|v| v == "1");
    set(&mut flags, "enable_verifier", if verify { "true" } else { "false" })?;
    let isa = cranelift_native::builder().map_err(str::to_owned)?;
    isa.finish(settings::Flags::new(flags)).map_err(|e| e.to_string())
}

/// What went wrong compiling a shard.
#[derive(Debug)]
pub struct Error {
    pub entry: Option<u32>,
    pub message: String,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.entry {
            Some(e) => write!(f, "{}: {}", symbol(e), self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for Error {}

fn error(entry: Option<u32>, message: impl std::fmt::Display) -> Error {
    Error { entry, message: message.to_string() }
}

/// One object holding `owners`' handlers and the probe
/// `h2gen_abi_version_<stem>` the registry names for them.
pub fn compile_shard(isa: &OwnedTargetIsa, stem: &str, owners: &[&Owner]) -> Result<Vec<u8>, Error> {
    let builder = ObjectBuilder::new(isa.clone(), stem, default_libcall_names()).map_err(|e| error(None, e))?;
    let mut m = ObjectModule::new(builder);
    let mut imports = lower::Imports::default();
    let cc = isa.default_call_conv();
    let fc = isa.frontend_config();
    let mut ctx = Context::new();
    let mut fbc = FunctionBuilderContext::new();

    // The shard's ABI probe: `uint32_t h2gen_abi_version_<stem>(void)`.
    let mut sig = m.make_signature();
    sig.returns.push(AbiParam::new(types::I32));
    let probe = m
        .declare_function(&format!("h2gen_abi_version_{stem}"), Linkage::Export, &sig)
        .map_err(|e| error(None, e))?;
    ctx.func.signature = sig;
    {
        let mut b = FunctionBuilder::new(&mut ctx.func, &mut fbc);
        let k = b.create_block();
        b.switch_to_block(k);
        let v = b.ins().iconst(types::I32, abi::ABI_VERSION as i64);
        b.ins().return_(&[v]);
        b.seal_all_blocks();
        b.finalize(fc);
    }
    m.define_function(probe, &mut ctx).map_err(|e| error(None, e))?;
    m.clear_context(&mut ctx);

    let sig = lower::handler_signature(cc);
    for o in owners {
        let entry = o.func.entry;
        let id = m.declare_function(&symbol(entry), Linkage::Export, &sig).map_err(|e| error(Some(entry), e))?;
        ctx.func.signature = sig.clone();
        let shape = lower::Shape { selectors: &o.selectors, headers: &o.loops.headers };
        let b = FunctionBuilder::new(&mut ctx.func, &mut fbc);
        lower::Lower::function(b, &mut m, &mut imports, &o.func, &shape, fc)
            .map_err(|u| error(Some(entry), u.0))?;
        m.define_function(id, &mut ctx).map_err(|e| error(Some(entry), format!("{e:?}")))?;
        m.clear_context(&mut ctx);
    }
    m.finish().emit().map_err(|e| error(None, e))
}

/// Every owner's shard ([`stem`]), in shard order.
pub fn shards(owners: &[Owner]) -> BTreeMap<String, Vec<&Owner>> {
    let mut s: BTreeMap<String, Vec<&Owner>> = BTreeMap::new();
    for o in owners {
        s.entry(stem(o.func.entry)).or_default().push(o);
    }
    s
}

/// Compile every shard on `jobs` threads: `(stem, object bytes)` in shard
/// order. The first error stops the build.
pub fn compile_all(isa: &OwnedTargetIsa, owners: &[Owner], jobs: usize) -> Result<Vec<(String, Vec<u8>)>, Error> {
    let shards: Vec<(String, Vec<&Owner>)> = shards(owners).into_iter().collect();
    // Largest first, so the long ones don't finish last.
    let mut order: Vec<usize> = (0..shards.len()).collect();
    let size = |i: usize| shards[i].1.iter().map(|o| o.func.blocks.values().map(|b| b.insns.len()).sum::<usize>()).sum::<usize>();
    order.sort_by_key(|&i| std::cmp::Reverse(size(i)));
    let next = AtomicUsize::new(0);
    let results: Vec<std::sync::Mutex<Option<Result<Vec<u8>, Error>>>> =
        shards.iter().map(|_| std::sync::Mutex::new(None)).collect();
    std::thread::scope(|s| {
        for _ in 0..jobs.max(1) {
            s.spawn(|| {
                loop {
                    let k = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&i) = order.get(k) else { break };
                    let (stem, os) = &shards[i];
                    let r = compile_shard(isa, stem, os);
                    let failed = r.is_err();
                    *results[i].lock().unwrap() = Some(r);
                    if failed {
                        // Stop handing out work.
                        next.store(order.len(), Ordering::Relaxed);
                    }
                }
            });
        }
    });
    let mut out = Vec::with_capacity(shards.len());
    for ((stem, _), r) in shards.into_iter().zip(results) {
        match r.into_inner().unwrap() {
            Some(Ok(bytes)) => out.push((stem, bytes)),
            Some(Err(e)) => return Err(e),
            None => return Err(error(None, format!("{stem}: not compiled (an earlier shard failed)"))),
        }
    }
    Ok(out)
}

/// What [`build_dylib`] did, with its stage times in seconds.
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub owners: usize,
    pub shards: usize,
    pub object_bytes: u64,
    /// Cranelift: all shards, the registry and the bridge.
    pub compile_secs: f64,
    /// Writing the objects.
    pub write_secs: f64,
    /// The linker.
    pub link_secs: f64,
}

/// Compile `owners` with Cranelift into shard objects, the registry and
/// the bridge, and link them into the dylib `out` with the platform's
/// linker (`emit::compile::link_objects`). No C is compiled; `dir/obj`
/// holds the objects.
pub fn build_dylib(owners: &[Owner], dir: &std::path::Path, out: &std::path::Path, jobs: usize) -> Result<Report, String> {
    build_dylib_with(owners, dir, out, jobs, None, None)
}

/// [`build_dylib`] for a pruned lift: the module carries `pruned`'s table
/// (docs/lifter/lift-pruning.md, "Boot safety").
pub fn build_dylib_pruned(
    owners: &[Owner],
    dir: &std::path::Path,
    out: &std::path::Path,
    jobs: usize,
    pruned: Option<&emit::pruned::Table>,
) -> Result<Report, String> {
    build_dylib_with(owners, dir, out, jobs, None, pruned)
}

/// [`build_dylib`], with the linker looked up on the `PATH`-style list
/// `tools` instead of `PATH` when it is given.
pub fn build_dylib_with(
    owners: &[Owner],
    dir: &std::path::Path,
    out: &std::path::Path,
    jobs: usize,
    tools: Option<OsString>,
    pruned: Option<&emit::pruned::Table>,
) -> Result<Report, String> {
    let t = std::time::Instant::now();
    let isa = isa()?;
    let mut objects = compile_all(&isa, owners, jobs).map_err(|e| e.to_string())?;
    let shards = objects.len();
    objects.push((REGISTRY_STEM.to_owned(), module::registry(&isa, owners).map_err(|e| e.to_string())?));
    objects.push((BRIDGE_STEM.to_owned(), module::bridge(&isa).map_err(|e| e.to_string())?));
    let mut exports = module::EXPORTS.to_vec();
    if let Some(table) = pruned {
        objects.push((module::PRUNED_STEM.to_owned(), module::pruned(&isa, table).map_err(|e| e.to_string())?));
        exports.push((emit::pruned::SYMBOL, true));
    }
    let compile_secs = t.elapsed().as_secs_f64();

    let t = std::time::Instant::now();
    let obj = dir.join("obj");
    std::fs::create_dir_all(&obj).map_err(|e| e.to_string())?;
    let mut paths = Vec::with_capacity(objects.len());
    let mut object_bytes = 0;
    for (stem, bytes) in &objects {
        let p = obj.join(format!("{stem}.o"));
        emit::output::replace(&p, bytes).map_err(|e| e.to_string())?;
        object_bytes += bytes.len() as u64;
        paths.push(p);
    }
    let write_secs = t.elapsed().as_secs_f64();

    let t = std::time::Instant::now();
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    emit::compile::link_objects(emit::compile::Arch::host(), &paths, &exports, out, tools)
        .map_err(|e| e.to_string())?;
    Ok(Report { owners: owners.len(), shards, object_bytes, compile_secs, write_secs, link_secs: t.elapsed().as_secs_f64() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_import_has_a_table_slot_of_its_own() {
        let mut names: Vec<&str> = abi::CORE.to_vec();
        names.extend(emit::host::X87_BRIDGE_SYMBOLS);
        names.push(abi::SSE);
        assert_eq!(names.len(), abi::SLOTS);
        for (i, n) in names.iter().enumerate() {
            assert_eq!(abi::slot(n), Some(i), "{n}");
        }
        assert_eq!(abi::X87_COUNT, emit::host::X87_BRIDGE_SYMBOLS.len());
        assert_eq!(abi::slot("h2abi_v1_cmpxchg8b"), None);
    }

    fn owner(entry: u32, imprecise_faults: bool) -> Owner {
        Owner {
            func: ir::IrFunction {
                entry,
                blocks: std::collections::BTreeMap::new(),
                interior_entries: Vec::new(),
                errors: Vec::new(),
            },
            selectors: std::collections::BTreeSet::from([entry]),
            entries: vec![(entry, emit::EntryKind::Function)],
            resumes: Vec::new(),
            entry_exits: 0,
            loops: Default::default(),
            opt: emit::opt::Report { imprecise_faults, ..Default::default() },
        }
    }

    /// The fast tier declares no clock, but says when its faults are
    /// imprecise, as clang's module does.
    #[test]
    fn the_registry_declares_imprecise_faults_only_when_planned_with_them() {
        assert_eq!(module::features(&[owner(0x1000, false), owner(0x2000, false)]), 0);
        assert_eq!(module::features(&[owner(0x1000, false), owner(0x2000, true)]), 128);
    }

    #[test]
    fn the_registry_and_bridge_build_with_cranelift_alone() {
        let isa = isa().unwrap();
        assert!(!module::registry(&isa, &[]).unwrap().is_empty());
        assert!(!module::bridge(&isa).unwrap().is_empty());
    }
}

