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
mod x87;

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::Path;

use ir::{Control, IrFunction, Target};

/// The support header, copied next to the generated files.
pub const PRELUDE: &str = include_str!("../c/h2gen.h");
/// The SSE reference semantics in C (`h2s_*`), included by [`PRELUDE`].
pub const SIMD_HEADER: &str = include_str!("../../simd/c/h2simd.h");
/// The x87 helper declarations (`h2x87_v1_*`), included by [`PRELUDE`].
/// The x87 helper declarations (`h2x87_v1_*`), from `crates/x87`.
pub const X87_HEADER: &str = include_str!("../../x87/include/h2x87.h");

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

/// Decide every owner's shape and registrations. `funcs` are validated
/// lifted functions, one per discovered entry.
pub fn plan(funcs: Vec<IrFunction>) -> Vec<Owner> {
    let mut owners: Vec<Owner> = funcs
        .into_iter()
        .map(|f| {
            let entry_exits = f.blocks.values().filter_map(|b| b.insns.last()).map(entry_exits).sum();
            let selectors = BTreeSet::from([f.entry]);
            let loops = backedge::find(&f);
            Owner { func: f, selectors, entries: Vec::new(), resumes: Vec::new(), entry_exits, loops }
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
    owners
}

/// Lift every discovered function and plan its owner.
pub fn plan_image(analysis: &analyze::Analysis, xbe: &xbe::Xbe) -> Result<Vec<Owner>, xbe::Error> {
    let lifter = lift::Lifter::new(analysis, xbe)?;
    let funcs = analysis.functions.keys().filter_map(|&e| lifter.lift(e)).collect();
    Ok(plan(funcs))
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
}

/// Write `content` unless the file already holds exactly that, so build
/// tools see unchanged shards as unchanged.
fn write_if_changed(path: &Path, content: &str) -> io::Result<bool> {
    if std::fs::read(path).is_ok_and(|old| old == content.as_bytes()) {
        return Ok(false);
    }
    std::fs::write(path, content)?;
    Ok(true)
}

const BANNER: &str = "/* Generated by crates/emit from a user-supplied game image.\n * Derived from the disc: never edit, never commit. */\n";

/// Emit every owner into `dir`.
pub fn write_module(owners: &[Owner], dir: &Path) -> io::Result<WriteReport> {
    std::fs::create_dir_all(dir)?;
    let mut report = WriteReport { owners: owners.len(), ..Default::default() };
    let mut files: BTreeMap<String, String> = BTreeMap::new();
    files.insert("h2gen.h".into(), PRELUDE.to_owned());
    files.insert("h2simd.h".into(), SIMD_HEADER.to_owned());
    files.insert("h2x87.h".into(), X87_HEADER.to_owned());

    // Which file (stem) defines each owner's handler.
    let mut home: BTreeMap<u32, String> = BTreeMap::new();
    for o in owners {
        let shape = c::OwnerShape { selectors: &o.selectors, loops: &o.loops };
        let text = c::function(&o.func, &shape);
        let stem = if text.len() > OVERSIZED {
            format!("fn_{:08x}", o.func.entry)
        } else {
            format!("shard_{:05x}", o.func.entry >> SHARD_SHIFT)
        };
        // Each object carries its own version probe, so the runtime can
        // check every handler's object (register_generated_object).
        let file = files.entry(format!("{stem}.c")).or_insert_with(|| {
            format!("{BANNER}#include \"h2gen.h\"\n\nH2_DEFINE_GENERATED_ABI_NAMED(h2gen_abi_version_{stem})\n\n")
        });
        file.push_str(&text);
        file.push('\n');
        home.insert(o.func.entry, stem);
        report.entries += o.entries.len();
        report.resumes += o.resumes.len();
        report.entry_exits += o.entry_exits;
        report.back_edges += o.loops.back_edges.len();
    }
    files.insert("h2gen_registry.c".into(), registry(owners, &home));

    // Drop generated sources from a previous run that no longer exist.
    for e in std::fs::read_dir(dir)?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let generated = name.ends_with(".c") && (name.starts_with("shard_") || name.starts_with("fn_"));
        if generated && !files.contains_key(&name) {
            std::fs::remove_file(e.path())?;
        }
    }
    let mut manifest = String::new();
    for (name, text) in &files {
        if write_if_changed(&dir.join(name), text)? {
            report.written += 1;
        }
        let h = fnv64(text.as_bytes());
        manifest.push_str(&format!("{name}\t{h:016x}\n"));
        report.bytes += text.len();
        report.files.insert(name.clone(), h);
    }
    write_if_changed(&dir.join("manifest.tsv"), &manifest)?;
    Ok(report)
}

/// The registration object: ABI version export and the entry/resume tables.
fn registry(owners: &[Owner], home: &BTreeMap<u32, String>) -> String {
    let mut s = format!("{BANNER}#include \"h2gen.h\"\n\nH2_DEFINE_GENERATED_ABI_NAMED(h2gen_module_abi_version)\n\n");
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
        "const H2GenModule h2gen_module = {{H2_ABI_VERSION, {}u, {}u, 0u, entries, resumes}};\n",
        entries.len(),
        resumes.len()
    ));
    s
}
