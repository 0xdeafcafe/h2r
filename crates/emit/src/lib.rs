//! C11 backend: semantic IR (`crates/ir`) to disposable C against the
//! runtime's versioned ABI (`crates/runtime/include/h2abi.h`).
//!
//! Pipeline (LIFTER.md §1, §2, §7):
//!
//! 1. [`plan`] decides each owner's shape: which blocks its handler holds,
//!    which start selectors it accepts, and which continuation PCs and
//!    interior entries it registers.
//! 2. [`c::function`] writes one handler per owner.
//! 3. [`write_module`] groups handlers into 4 KiB entry-VA shards
//!    (`entry >> 14`), isolates oversized owners in their own file, adds a
//!    registration object, and rewrites only files whose content changed.
//! 4. [`compile`] builds the shards with clang into per-architecture static
//!    libraries (and, for tests, a host dylib), caching objects by content
//!    hash.
//!
//! # Code shared between owners
//!
//! The CFG never absorbs another function's entry: any branch, jump,
//! fall-through or switch case that lands on one is a tail transfer in the
//! IR ([`ir::Dest::tail`]), emitted as an entry-routed `JUMP`, so it goes
//! through dispatch and an override of the inner function is honoured even
//! when the outer one runs into it. On the retail image that covers the
//! 125 nested entries discovery's reachability CFGs used to contain.
//!
//! Code shared without an entry (two functions jumping into a common
//! tail) is still duplicated: each owner's handler holds its own copy, used
//! only by its own internal control flow. Every address the runtime can
//! enter from outside, call continuations and interior entries, is
//! registered once, against its canonical owner ([`ir::canonical_owner`]:
//! the containing function with the greatest entry at or below the
//! address, else the lowest), and only that owner accepts it as a start
//! selector. All copies are the same lowering of the same instructions, so
//! which copy runs is unobservable. A call continuation that is itself
//! another function's entry (A calls, B starts right after the call) is
//! not a block of A at all; it is registered as a resume of B's handler,
//! so the return resumes B's code directly rather than going through entry
//! dispatch, where an override of B would run in place of A's
//! continuation.
//!
//! What the runtime must provide for generated modules (imports, the
//! atomic contract, a registration adapter) is in `RUNTIME-CONTRACT.md`.

pub mod backedge;
pub mod c;
pub mod compile;
pub mod host;
mod lazy;
pub mod output;
pub mod par;
mod x87;

/// The IR passes [`plan`] runs, re-exported for callers choosing them.
pub use opt;

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::Path;

use ir::{Control, IrFunction, Target};

/// The support header, copied next to the generated files.
pub const PRELUDE: &str = include_str!("../c/h2gen.h");
pub const FAST_MEMORY_HEADER: &str = include_str!("../c/h2fastmem.h");
/// Guarded native SSE arithmetic; enabled unless `H2_SSE_NATIVE=0`.
pub const SSE_NATIVE_HEADER: &str = include_str!("../c/h2sse_native.h");
/// The SSE reference semantics in C (`h2s_*`), included by [`PRELUDE`].
pub const SIMD_HEADER: &str = include_str!("../../simd/c/h2simd.h");
/// The x87 helper declarations (`h2x87_v1_*`), included by [`PRELUDE`].
/// The x87 helper declarations (`h2x87_v1_*`), from `crates/x87`.
pub const X87_HEADER: &str = include_str!("../../x87/include/h2x87.h");
/// Guarded native x87 operations; enabled unless `H2_X87_NATIVE=0`.
pub const X87_NATIVE_HEADER: &str = include_str!("../c/h2x87_native.h");

/// Entry-VA bucket size for shards (LIFTER.md §1): 4 KiB buckets. The
/// bucket is a pure function of the entry address, so editing, adding or
/// removing a function never moves another into a different shard, and a
/// change rebuilds one ~0.5 MB shard (about 1.5 s of clang) instead of a
/// ~2 MB one (6 s). A compile has no per-file cost worth avoiding: the
/// support headers parse in 40 ms, so a precompiled header buys nothing.
pub const SHARD_SHIFT: u32 = 12;
/// An owner whose C is larger than this gets a file of its own, so one
/// huge function doesn't serialise a whole shard's compile.
pub const OVERSIZED: usize = 1 << 20;

/// How an owner registers an address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EntryKind {
    Function = 0,
    /// An address-taken label inside the owner, entered with its own
    /// selector.
    Interior = 1,
}

/// One generated handler and everything it registers.
pub struct Owner {
    pub func: IrFunction,
    /// Accepted start selectors.
    pub selectors: BTreeSet<u32>,
    /// Addresses registered as generated entries that resolve to this
    /// handler (with selector = address).
    pub entries: Vec<(u32, EntryKind)>,
    /// Continuation PCs registered as resumes of this handler.
    pub resumes: Vec<u32>,
    /// Direct exits to other functions' entries that are not `jmp`s
    /// (branch sides, fall-throughs, switch cases): code the CFG keeps out
    /// of this handler so it runs, once, in the target's.
    pub entry_exits: usize,
    /// Loop back-edges, each a preemption safe point, and their headers,
    /// which are registered as resumes ([`backedge`]).
    pub loops: backedge::Loops,
    /// What the IR passes did to `func` ([`opt::optimize`]).
    pub opt: opt::Report,
}

/// Branch sides, switch cases and fall-throughs of a block's last
/// instruction that leave for another entry.
fn entry_exits(insn: &ir::IrInsn) -> usize {
    match &insn.control {
        Control::Branch { taken, fallthrough, .. } => taken.tail as usize + fallthrough.tail as usize,
        Control::Switch { cases, .. } => cases.iter().filter(|c| c.tail).count(),
        Control::Jump { target: Target::Direct(t), tail: true, .. } if *t == insn.next_pc() => 1,
        _ => 0,
    }
}

/// Decide every owner's shape and registrations, and run the IR passes
/// `H2_PASSES` selects (all of them by default; see [`opt::Passes`]).
/// `funcs` are validated lifted functions, one per discovered entry.
///
/// # Panics
///
/// If `H2_PASSES` is set to something [`opt::Passes::parse`] rejects.
pub fn plan(funcs: Vec<IrFunction>) -> Vec<Owner> {
    plan_with(funcs, opt::Passes::from_env().unwrap_or_else(|e| panic!("{e}")))
}

/// [`plan`] with an explicit pass selection.
pub fn plan_with(funcs: Vec<IrFunction>, passes: opt::Passes) -> Vec<Owner> {
    let mut owners: Vec<Owner> = funcs
        .into_iter()
        .map(|f| {
            let entry_exits = f.blocks.values().filter_map(|b| b.insns.last()).map(entry_exits).sum();
            let selectors = BTreeSet::from([f.entry]);
            let loops = backedge::find(&f);
            let opt = opt::Report::default();
            Owner { func: f, selectors, entries: Vec::new(), resumes: Vec::new(), entry_exits, loops, opt }
        })
        .collect();
    owners.sort_by_key(|o| o.func.entry);

    // Every externally enterable address, with the owners that hold it.
    let mut interior: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    let mut continuations: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for o in &owners {
        let f = &o.func;
        for &l in &f.interior_entries {
            interior.entry(l).or_default().push(f.entry);
        }
        for b in f.blocks.values() {
            if let Some(Control::Call { continuation, .. }) = b.insns.last().map(|i| &i.control)
                && f.blocks.contains_key(continuation)
            {
                continuations.entry(*continuation).or_default().push(f.entry);
            }
        }
    }
    let index: BTreeMap<u32, usize> = owners.iter().enumerate().map(|(i, o)| (o.func.entry, i)).collect();
    // A continuation that is another function's entry is not a block of
    // the caller (the CFG never absorbs an entry), but a return still lands
    // there: register it as a resume of the function that starts there.
    // The return then resumes that handler directly, like any return,
    // instead of being dispatched (and possibly overridden) as an entry;
    // calls and jumps to the entry keep entry dispatch.
    for o in &owners {
        for b in o.func.blocks.values() {
            if let Some(Control::Call { continuation, .. }) = b.insns.last().map(|i| &i.control)
                && !o.func.blocks.contains_key(continuation)
                && index.contains_key(continuation)
            {
                continuations.entry(*continuation).or_default().push(*continuation);
            }
        }
    }
    // A REP string instruction that faults part-way leaves EIP on itself
    // with its progress in ESI/EDI/ECX; restarting there continues it. Each
    // one is a resume of its owner so the runtime can re-enter at that PC
    // (RUNTIME-CONTRACT.md, "Restarting after a fault").
    for o in &owners {
        for b in o.func.blocks.values() {
            for i in &b.insns {
                if i.stmts.iter().any(|s| matches!(s, ir::Stmt::Repeat { .. })) {
                    let holders = continuations.entry(i.pc).or_default();
                    if !holders.contains(&o.func.entry) {
                        holders.push(o.func.entry);
                    }
                }
            }
        }
    }
    // A loop header is where a preempted loop resumes (`backedge`): a
    // resume of every owner that holds the loop, registered once against
    // the canonical one like any continuation.
    for o in &owners {
        for &h in &o.loops.headers {
            let holders = continuations.entry(h).or_default();
            if !holders.contains(&o.func.entry) {
                holders.push(o.func.entry);
            }
        }
    }
    for o in &mut owners {
        o.entries.push((o.func.entry, EntryKind::Function));
    }
    for (pc, holders) in interior {
        // A function entry is registered as itself, never as an interior.
        if index.contains_key(&pc) {
            continue;
        }
        let o = &mut owners[index[&ir::canonical_owner(pc, holders).unwrap()]];
        o.entries.push((pc, EntryKind::Interior));
        o.selectors.insert(pc);
    }
    for (pc, holders) in continuations {
        let o = &mut owners[index[&ir::canonical_owner(pc, holders).unwrap()]];
        o.resumes.push(pc);
        o.selectors.insert(pc);
    }
    // The passes need the final shape: every selector (an entry point
    // whose state comes from CpuAbi) and every polled header (an exit).
    // Each owner on its own: spread over the cores.
    par::for_each_mut(&mut owners, |o| {
        let shape = opt::Shape { selectors: &o.selectors, polled: &o.loops.headers };
        o.opt = opt::optimize(&mut o.func, &shape, passes);
    });
    owners
}

/// Lift every discovered function and plan its owner, running `passes`.
pub fn plan_image(
    analysis: &analyze::Analysis,
    xbe: &xbe::Xbe,
    passes: opt::Passes,
) -> Result<Vec<Owner>, xbe::Error> {
    let lifter = lift::Lifter::new(analysis, xbe)?;
    let entries: Vec<u32> = analysis.functions.keys().copied().collect();
    let funcs = par::map(&entries, |&e| lifter.lift(e)).into_iter().flatten().collect();
    Ok(plan_with(funcs, passes))
}

/// FNV-1a, 64-bit: stable across Rust versions, which a cache key needs.
pub fn fnv64(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x100_0000_01b3))
}

/// What [`write_module`] did.
#[derive(Debug, Default, Clone)]
pub struct WriteReport {
    /// Generated files, with their content hash.
    pub files: BTreeMap<String, u64>,
    /// Files whose content changed (and were rewritten).
    pub written: usize,
    pub bytes: usize,
    pub owners: usize,
    pub entries: usize,
    pub resumes: usize,
    /// Branch sides, fall-throughs and switch cases emitted as entry-routed
    /// transfers because they land on another function's entry.
    pub entry_exits: usize,
    /// Loop back-edges, each with a preemption check ([`backedge`]).
    pub back_edges: usize,
    /// What the IR passes did, summed over owners.
    pub opt: opt::Report,
}

/// Write `content` unless the file already holds exactly that, so build
/// tools see unchanged shards as unchanged.
fn write_if_changed(path: &Path, content: &str) -> io::Result<bool> {
    if std::fs::read(path).is_ok_and(|old| old == content.as_bytes()) {
        return Ok(false);
    }
    output::replace(path, content.as_bytes())?;
    Ok(true)
}

const BANNER: &str = "/* Generated by crates/emit from a user-supplied game image.\n * Derived from the disc: never edit, never commit. */\n";

/// Code generation choices that change the C but not what it means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Codegen {
    /// Access accounting (module feature `H2GEN_FEATURE_ACCOUNTING`; on in
    /// [`Codegen::from_env`] unless `H2_ACCESS_COUNT=0`, off in
    /// `Codegen::default()`): handlers count their guest memory accesses in
    /// a local and charge them in bulk, instead of the runtime charging each
    /// accessor call. See h2gen.h, "Access accounting".
    pub accounting: bool,
    /// Keep only the registers a handler writes in C locals and read the
    /// others from `CpuAbi` ([`c::homes`]); on unless `H2_REG_HOMES=0`,
    /// which keeps every register in a local (the older shape, for
    /// comparison and bisection).
    pub reg_homes: bool,
}

impl Default for Codegen {
    /// No accounting, register homes on.
    fn default() -> Self {
        Codegen { accounting: false, reg_homes: true }
    }
}

impl Codegen {
    /// Accounting is on unless `H2_ACCESS_COUNT=0`; register homes unless
    /// `H2_REG_HOMES=0`.
    pub fn from_env() -> Result<Codegen, String> {
        let mut c = Codegen::parse(std::env::var("H2_ACCESS_COUNT").ok().as_deref())?;
        c.reg_homes = switch("H2_REG_HOMES", std::env::var("H2_REG_HOMES").ok().as_deref())?;
        Ok(c)
    }

    /// [`Codegen::from_env`] for a given `H2_ACCESS_COUNT` value, with
    /// register homes on.
    pub fn parse(access_count: Option<&str>) -> Result<Codegen, String> {
        Ok(Codegen { accounting: switch("H2_ACCESS_COUNT", access_count)?, reg_homes: true })
    }
}

/// An on-by-default `0`/`1` switch.
fn switch(name: &str, value: Option<&str>) -> Result<bool, String> {
    match value {
        Some("0") => Ok(false),
        None | Some("") | Some("1") => Ok(true),
        Some(v) => Err(format!("{name}={v}: expected 0 or 1")),
    }
}

/// Emit every owner into `dir`, with [`Codegen::from_env`].
pub fn write_module(owners: &[Owner], dir: &Path) -> io::Result<WriteReport> {
    let codegen = Codegen::from_env().map_err(io::Error::other)?;
    write_module_with(owners, dir, codegen)
}

/// Emit every owner into `dir`.
pub fn write_module_with(owners: &[Owner], dir: &Path, codegen: Codegen) -> io::Result<WriteReport> {
    write_module_with_hot(owners, dir, codegen, None)
}

/// Keep all entry and resume records. Owners outside an audited allowlist
/// are replaced by a faulting ABI body rather than leaving stale pointers.
pub fn write_module_with_hot(
    owners: &[Owner],
    dir: &Path,
    codegen: Codegen,
    hot: Option<&BTreeSet<u32>>,
) -> io::Result<WriteReport> {
    std::fs::create_dir_all(dir)?;
    // Defined ahead of the header in every generated file, so all of a
    // module's objects agree.
    let banner = if codegen.accounting {
        format!("{BANNER}#define H2_ACCOUNTING 1\n")
    } else {
        BANNER.to_owned()
    };
    let mut report = WriteReport { owners: owners.len(), ..Default::default() };
    let mut files: BTreeMap<String, String> = BTreeMap::new();
    files.insert("h2gen.h".into(), PRELUDE.to_owned());
    files.insert("h2fastmem.h".into(), FAST_MEMORY_HEADER.to_owned());
    files.insert("h2simd.h".into(), SIMD_HEADER.to_owned());
    files.insert("h2x87.h".into(), X87_HEADER.to_owned());
    files.insert("h2sse_native.h".into(), SSE_NATIVE_HEADER.to_owned());
    files.insert("h2x87_native.h".into(), X87_NATIVE_HEADER.to_owned());

    // Which file (stem) defines each owner's handler.
    let mut home: BTreeMap<u32, String> = BTreeMap::new();
    // Each owner's C is independent of the others': generate it on every
    // core, then lay it out in owner order as before.
    let functions: BTreeSet<u32> = owners.iter().map(|o| o.func.entry).collect();
    let texts = par::map(owners, |o| {
        if hot.is_some_and(|hot| !hot.contains(&o.func.entry)) {
            return cold_owner(o.func.entry);
        }
        let shape = c::OwnerShape {
            selectors: &o.selectors,
            loops: &o.loops,
            accounting: codegen.accounting,
            functions: Some(&functions),
            reg_homes: codegen.reg_homes,
        };
        c::function(&o.func, &shape)
    });
    for (o, text) in owners.iter().zip(texts) {
        let stem = if text.len() > OVERSIZED {
            format!("fn_{:08x}", o.func.entry)
        } else {
            format!("shard_{:05x}", o.func.entry >> SHARD_SHIFT)
        };
        // Each object carries its own version probe, so the runtime can
        // check every handler's object (register_generated_object).
        let file = files.entry(format!("{stem}.c")).or_insert_with(|| {
            format!("{banner}#include \"h2gen.h\"\n\nH2_DEFINE_GENERATED_ABI_NAMED(h2gen_abi_version_{stem})\n\n")
        });
        file.push_str(&text);
        file.push('\n');
        home.insert(o.func.entry, stem);
        report.entries += o.entries.len();
        report.resumes += o.resumes.len();
        report.entry_exits += o.entry_exits;
        report.back_edges += o.loops.back_edges.len();
        report.opt += o.opt;
    }
    files.insert("h2gen_registry.c".into(), registry(owners, &home, &banner, codegen));

    // Drop generated sources from a previous run that no longer exist.
    for e in std::fs::read_dir(dir)?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let generated = name.ends_with(".c") && (name.starts_with("shard_") || name.starts_with("fn_"));
        if generated && !files.contains_key(&name) {
            std::fs::remove_file(e.path())?;
        }
    }
    let mut manifest = String::new();
    let files_list: Vec<(&String, &String)> = files.iter().collect();
    let written = par::map(&files_list, |(name, text)| {
        write_if_changed(&dir.join(name), text).map(|w| (w, fnv64(text.as_bytes())))
    });
    for ((name, text), w) in files_list.into_iter().zip(written) {
        let (changed, h) = w?;
        if changed {
            report.written += 1;
        }
        manifest.push_str(&format!("{name}\t{h:016x}\n"));
        report.bytes += text.len();
        report.files.insert(name.clone(), h);
    }
    write_if_changed(&dir.join("manifest.tsv"), &manifest)?;
    Ok(report)
}

/// Code 15 is diagnosed by boot's shim. It is a fault, so the interpreter's
/// STOP recovery cannot turn a coverage miss into a plausible match.
fn cold_owner(owner: u32) -> String {
    format!(
        "TransferAbi {}(CpuAbi *cpu, ExecCtxOpaque *ctx, uint32_t start) {{\n\
             cpu->eip = start;\n\
             uint32_t event = h2shim_v1_event(ctx, start, start, 15u, {owner:#x}u);\n\
             return (TransferAbi){{H2_ABI_VERSION, sizeof(TransferAbi), H2_FAULT, H2_ENTRY, 0u, 0u, event}};\n\
         }}\n",
        c::symbol(owner),
    )
}

/// The registration object: ABI version export and the entry/resume tables.
fn registry(owners: &[Owner], home: &BTreeMap<u32, String>, banner: &str, codegen: Codegen) -> String {
    let mut s = format!("{banner}#define H2_FLAT_STATS_REGISTRY 1\n#include \"h2gen.h\"\n\nH2_EXPORT H2_DEFINE_GENERATED_ABI_NAMED(h2gen_module_abi_version)\n\n");
    let stems: BTreeSet<&String> = home.values().collect();
    for stem in stems {
        s.push_str(&format!("uint32_t h2gen_abi_version_{stem}(void);\n"));
    }
    for o in owners {
        s.push_str(&format!("TransferAbi {}(CpuAbi *, ExecCtxOpaque *, uint32_t);\n", c::symbol(o.func.entry)));
    }
    let mut entries: Vec<(u32, EntryKind, u32)> =
        owners.iter().flat_map(|o| o.entries.iter().map(move |(a, k)| (*a, *k, o.func.entry))).collect();
    entries.sort();
    s.push_str("\nstatic const H2GenEntry entries[] = {\n");
    for (a, k, owner) in &entries {
        let probe = format!("h2gen_abi_version_{}", home[owner]);
        s.push_str(&format!("    {{{a:#010x}u, {}u, {}, {probe}}},\n", *k as u32, c::symbol(*owner)));
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
        "H2_EXPORT const H2GenModule h2gen_module = {{H2_ABI_VERSION, {}u, {}u, {}, entries, resumes}};\n",
        entries.len(),
        resumes.len(),
        if codegen.accounting { "H2GEN_FEATURE_ACCOUNTING | H2GEN_FEATURE_DIRECT" } else { "H2GEN_FEATURE_DIRECT" }
    ));
    s
}

#[cfg(test)]
mod tests {
    use super::Codegen;

    #[test]
    fn access_accounting_defaults_on_and_zero_turns_it_off() {
        assert_eq!(Codegen::parse(None), Ok(Codegen { accounting: true, ..Codegen::default() }));
        assert_eq!(Codegen::parse(Some("")), Ok(Codegen { accounting: true, ..Codegen::default() }));
        assert_eq!(Codegen::parse(Some("1")), Ok(Codegen { accounting: true, ..Codegen::default() }));
        assert_eq!(Codegen::parse(Some("0")), Ok(Codegen { accounting: false, ..Codegen::default() }));
        assert!(Codegen::parse(Some("on")).is_err());
        let on = std::env::var("H2_ACCESS_COUNT").as_deref() != Ok("0");
        let homes = std::env::var("H2_REG_HOMES").as_deref() != Ok("0");
        assert_eq!(Codegen::from_env(), Ok(Codegen { accounting: on, reg_homes: homes }));
    }
}
