//! The Cranelift fast tier: planned IR (`emit::Owner`) straight to host
//! objects, linked into the same kind of module clang's build makes
//! (docs/lifter/cranelift-tier.md).
//!
//! A module is the shard objects from [`compile_shard`] (one handler
//! `h2fn_<entry>` per owner, the C signature, and each shard's ABI probe),
//! the registry from [`registry_c`] and the helper library
//! [`HELPERS_C`], both small C files, plus the host's bridge. Everything
//! derived from the disc stays in the caller's output directory.

mod abi;
pub mod lower;
mod x87;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use cranelift_codegen::ir::{AbiParam, InstBuilder, types};
use cranelift_codegen::isa::OwnedTargetIsa;
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::Context;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{Linkage, Module, default_libcall_names};
use cranelift_object::{ObjectBuilder, ObjectModule};
use emit::{EntryKind, Owner};

pub use lower::Unsupported;

/// Out-of-line SSE helpers the handlers call (`c/h2clif_helpers.c`).
pub const HELPERS_C: &str = include_str!("../c/h2clif_helpers.c");
/// File names the module's C parts are written under.
pub const HELPERS_NAME: &str = "h2clif_helpers.c";
pub const REGISTRY_NAME: &str = "h2clif_registry.c";

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

/// The registration object, as `emit`'s registry but with no module
/// features: the fast tier counts no accesses and makes no direct calls.
pub fn registry_c(owners: &[Owner]) -> String {
    let mut s = String::from(
        "/* Generated by crates/clif from a user-supplied game image.\n * Derived from the disc: never edit, never commit. */\n#include \"h2gen.h\"\n\nH2_EXPORT H2_DEFINE_GENERATED_ABI_NAMED(h2gen_module_abi_version)\n\n",
    );
    let stems: std::collections::BTreeSet<String> = owners.iter().map(|o| stem(o.func.entry)).collect();
    for st in &stems {
        s.push_str(&format!("uint32_t h2gen_abi_version_{st}(void);\n"));
    }
    for o in owners {
        s.push_str(&format!("TransferAbi {}(CpuAbi *, ExecCtxOpaque *, uint32_t);\n", symbol(o.func.entry)));
    }
    let mut entries: Vec<(u32, EntryKind, u32)> =
        owners.iter().flat_map(|o| o.entries.iter().map(move |(a, k)| (*a, *k, o.func.entry))).collect();
    entries.sort();
    s.push_str("\nstatic const H2GenEntry entries[] = {\n");
    for (a, k, owner) in &entries {
        s.push_str(&format!(
            "    {{{a:#010x}u, {}u, {}, h2gen_abi_version_{}}},\n",
            *k as u32,
            symbol(*owner),
            stem(*owner)
        ));
    }
    s.push_str("    {0u, 0u, 0, 0},\n};\n\nstatic const H2GenResume resumes[] = {\n");
    let mut resumes: Vec<(u32, u32)> =
        owners.iter().flat_map(|o| o.resumes.iter().map(move |pc| (*pc, o.func.entry))).collect();
    resumes.sort();
    for (pc, owner) in &resumes {
        s.push_str(&format!("    {{{pc:#010x}u, {owner:#010x}u, {pc:#010x}u}},\n"));
    }
    s.push_str("    {0u, 0u, 0u},\n};\n\n");
    s.push_str(&format!(
        "H2_EXPORT const H2GenModule h2gen_module = {{H2_ABI_VERSION, {}u, {}u, 0u, entries, resumes}};\n",
        entries.len(),
        resumes.len()
    ));
    s
}

/// What [`build_dylib`] did, with its stage times in seconds.
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub owners: usize,
    pub shards: usize,
    pub object_bytes: u64,
    /// Cranelift, all shards.
    pub compile_secs: f64,
    /// Writing objects and the C parts.
    pub write_secs: f64,
    /// Compiling the C parts (registry, helpers, bridge) and linking.
    pub link_secs: f64,
}

/// Compile `owners` with Cranelift and link them, the registry, the helper
/// library and `extra` C files (the host's bridge) into the dylib `out`.
/// `dir` holds the objects and C sources; `h2abi` is the runtime's ABI
/// header.
pub fn build_dylib(
    owners: &[Owner],
    dir: &std::path::Path,
    h2abi: &std::path::Path,
    extra: &[(&str, &str)],
    out: &std::path::Path,
    jobs: usize,
) -> Result<Report, String> {
    use emit::compile;
    let t = std::time::Instant::now();
    let isa = isa()?;
    let objects = compile_all(&isa, owners, jobs).map_err(|e| e.to_string())?;
    let compile_secs = t.elapsed().as_secs_f64();

    let t = std::time::Instant::now();
    let src = dir.join("src");
    let obj = dir.join("obj");
    std::fs::create_dir_all(&src).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&obj).map_err(|e| e.to_string())?;
    for (name, text) in headers() {
        emit::output::replace(&src.join(name), text.as_bytes()).map_err(|e| e.to_string())?;
    }
    compile::install_header(&src, h2abi).map_err(|e| e.to_string())?;
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
    let registry = registry_c(owners);
    let mut c: Vec<(&str, &str)> = vec![(HELPERS_NAME, HELPERS_C), (REGISTRY_NAME, &registry)];
    c.extend_from_slice(extra);
    let build = compile::Build {
        arch: compile::Arch::host(),
        objects: paths,
        cache_objects: Vec::new(),
        compiled: objects.len(),
        reused: 0,
        cold: 0,
        compile_secs,
        cache_secs: 0.0,
        archive_secs: 0.0,
        object_bytes,
        library: out.to_path_buf(),
        library_bytes: 0,
    };
    compile::host_dylib(&build, &src, &c, out).map_err(|e| e.to_string())?;
    Ok(Report {
        owners: owners.len(),
        shards: objects.len(),
        object_bytes,
        compile_secs,
        write_secs,
        link_secs: t.elapsed().as_secs_f64(),
    })
}

/// The support headers the C parts include, as `(name, text)`: written
/// next to them, with the runtime's `h2abi.h` installed by the caller.
pub fn headers() -> [(&'static str, &'static str); 6] {
    [
        ("h2gen.h", emit::PRELUDE),
        ("h2fastmem.h", emit::FAST_MEMORY_HEADER),
        ("h2simd.h", emit::SIMD_HEADER),
        ("h2x87.h", emit::X87_HEADER),
        ("h2sse_native.h", emit::SSE_NATIVE_HEADER),
        ("h2x87_native.h", emit::X87_NATIVE_HEADER),
    ]
}
