//! Which guest functions a lift can leave out once Rust replaces some
//! entries (`docs/lifter/lift-pruning.md`, "Reachability").
//!
//! Nodes are owners. An edge to a replaced function's entry stops there
//! (the cut); an edge to one of its interior entries keeps the whole owner.
//! Every function discovery found by anything but a direct call or tail
//! call is a root, and so is every code address stored anywhere: an
//! immediate in code or an aligned word in a data section. A function is
//! dropped only when no root reaches it without passing a cut.
//!
//! [`Graph::build`] reads the analyzer's CFGs once; [`keep_in`] walks a
//! graph, so tests can hand it an invented one. The walk is breadth-first
//! and records how it first reached each kept function ([`Why`]), so
//! [`Keep::path`] answers "why is this kept" with a shortest chain back to
//! a root. All inputs are sorted maps
//! and sets: the result and its digest do not depend on hash order.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write as _;

use xbe::Xbe;

use crate::cfg::{CallTarget, CfgBuilder};
use crate::{Analysis, Source, is_code_section};

/// One function in the reachability graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub source: Source,
    /// Static addresses control leaves for: direct call targets, tail
    /// transfers, and branches or switch cases that land on another entry.
    pub targets: BTreeSet<u32>,
    /// Bytes of code in the function's CFG.
    pub bytes: u32,
}

/// The graph [`keep_in`] walks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Graph {
    /// Every discovered function, by entry.
    pub functions: BTreeMap<u32, Node>,
    /// Addresses inside a function that something can enter by: CFG
    /// interior entries, and stored values landing on an instruction start.
    /// Value: the owner.
    pub interior: BTreeMap<u32, u32>,
    /// Code addresses stored in the image: immediates and memory
    /// displacements in decoded code, aligned words in data sections.
    pub stored: BTreeSet<u32>,
}

/// Why a function is left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dropped {
    /// Replaced entries whose cut bodies reach this function through
    /// dropped code only. A replaced function's own body lists itself.
    /// Empty if nothing reaches it at all.
    pub by: BTreeSet<u32>,
    pub bytes: u32,
}

/// How the walk first reached a kept function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// A root: found by something other than a direct call or tail call
    /// (`Entry`, `CodeRef`, `DataPointer`, `GapScan`, ...).
    Found(Source),
    /// A root: a code address stored in the image (an immediate or
    /// displacement in decoded code, an aligned word in a data section:
    /// vtables, callback and handler tables). `at` is the stored value, an
    /// interior entry when it is not the owner's.
    Stored { at: u32 },
    /// A root the caller gave ([`keep`]'s `extra_roots`). `at` as above.
    Extra { at: u32 },
    /// An exiting edge of the kept function `caller`: a direct call, tail
    /// transfer, or branch or switch case landing on another entry. `at` is
    /// the edge's target, an interior entry when it is not the owner's.
    From { caller: u32, at: u32 },
}

impl Why {
    /// Whether this is a root rather than an edge.
    pub fn is_root(self) -> bool {
        !matches!(self, Why::From { .. })
    }
}

/// The result of [`keep`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Keep {
    pub kept: BTreeSet<u32>,
    /// How the walk first reached each kept owner; its keys are `kept`.
    pub why: BTreeMap<u32, Why>,
    pub dropped: BTreeMap<u32, Dropped>,
    /// FNV-1a over the replaced entries and the dropped set, in order.
    pub digest: u64,
}

impl Keep {
    /// Why `owner` is kept: its own step first, then each caller's, ending
    /// at a root. A shortest such chain (the walk is breadth-first). `None`
    /// when `owner` is not a kept owner (dropped, unknown, or an interior
    /// address: map it through [`Graph::interior`] first).
    pub fn path(&self, owner: u32) -> Option<Vec<(u32, Why)>> {
        let mut out = Vec::new();
        let mut cur = owner;
        loop {
            let why = *self.why.get(&cur)?;
            out.push((cur, why));
            match why {
                Why::From { caller, .. } => cur = caller,
                _ => return Some(out),
            }
        }
    }
}

impl Graph {
    /// The graph of every function `analysis` found in `xbe`.
    pub fn build(xbe: &Xbe, analysis: &Analysis) -> Self {
        let builder = CfgBuilder::new(analysis, xbe);
        let mut g = Graph::default();
        for (&entry, f) in &analysis.functions {
            let mut node = Node { source: f.source, targets: BTreeSet::new(), bytes: 0 };
            let Some(cfg) = builder.build(entry) else {
                g.functions.insert(entry, node);
                continue;
            };
            for block in cfg.blocks.values() {
                node.bytes += block.end - block.start;
                node.targets.extend(block.edges.iter().filter(|e| e.exits).filter_map(|e| e.target));
                for &pc in &block.insns {
                    if let Some(insn) = builder.code().decode(pc) {
                        g.stored.extend(crate::dispatch::references(&insn));
                    }
                }
            }
            node.targets.extend(cfg.calls.iter().filter_map(|c| match c.target {
                CallTarget::Direct(t) => Some(t),
                _ => None,
            }));
            for &i in &cfg.interior_entries {
                g.interior.insert(i, entry);
            }
            g.functions.insert(entry, node);
        }
        for s in xbe.sections.iter().filter(|s| !is_code_section(s)) {
            if let Ok(bytes) = xbe.section_bytes(s) {
                g.stored.extend(bytes.as_chunks::<4>().0.iter().map(|w| u32::from_le_bytes(*w)));
            }
        }
        // Keep only values that land on code, mapping instruction starts
        // inside a function to their owner.
        let stored = std::mem::take(&mut g.stored);
        for v in stored {
            if g.functions.contains_key(&v) || g.interior.contains_key(&v) {
                g.stored.insert(v);
            } else if let Some(('I', Some(owner))) = analysis.claim(v) {
                g.interior.insert(v, owner);
                g.stored.insert(v);
            }
        }
        g
    }
}

/// The functions a lift keeps when `replaced` entries are implemented in
/// Rust. `extra_roots` are addresses the plan needs whatever the guest
/// does: unreplaced chain points, guest functions Rust calls, hook resume
/// sites.
pub fn keep(xbe: &Xbe, analysis: &Analysis, replaced: &BTreeSet<u32>, extra_roots: &BTreeSet<u32>) -> Keep {
    keep_in(&Graph::build(xbe, analysis), replaced, extra_roots)
}

/// [`keep`] over a built graph.
pub fn keep_in(graph: &Graph, replaced: &BTreeSet<u32>, extra_roots: &BTreeSet<u32>) -> Keep {
    // The owner an address reaches, or `None` at a cut or outside code.
    // The XBE entry is never cut: nothing runs before it to stand in.
    let reach = |va: u32| -> Option<u32> {
        if let Some(node) = graph.functions.get(&va) {
            (!replaced.contains(&va) || node.source == Source::Entry).then_some(va)
        } else {
            graph.interior.get(&va).copied()
        }
    };
    // Roots in a fixed order: discovery sources, then stored addresses, then
    // the caller's. The first reason to reach an owner is the one kept.
    let roots = graph
        .functions
        .iter()
        .filter(|(_, n)| !matches!(n.source, Source::Call | Source::TailCall))
        .map(|(&va, n)| (va, Why::Found(n.source)))
        .chain(graph.stored.iter().map(|&at| (at, Why::Stored { at })))
        .chain(extra_roots.iter().map(|&at| (at, Why::Extra { at })));
    let mut why: BTreeMap<u32, Why> = BTreeMap::new();
    let mut work: VecDeque<(u32, Why)> = roots.filter_map(|(va, w)| Some((reach(va)?, w))).collect();
    while let Some((owner, w)) = work.pop_front() {
        if why.contains_key(&owner) {
            continue;
        }
        why.insert(owner, w);
        if let Some(node) = graph.functions.get(&owner) {
            for &at in &node.targets {
                if let Some(next) = reach(at).filter(|n| !why.contains_key(n)) {
                    work.push_back((next, Why::From { caller: owner, at }));
                }
            }
        }
    }
    let kept: BTreeSet<u32> = why.keys().copied().collect();

    let mut dropped: BTreeMap<u32, Dropped> = graph
        .functions
        .iter()
        .filter(|(va, _)| !kept.contains(*va))
        .map(|(&va, n)| (va, Dropped { by: BTreeSet::new(), bytes: n.bytes }))
        .collect();
    // Attribute: from each cut body, through dropped code, stopping at
    // other cuts (their bodies are theirs).
    for &r in replaced {
        if !dropped.contains_key(&r) {
            continue;
        }
        let mut seen = BTreeSet::new();
        let mut work = vec![r];
        while let Some(owner) = work.pop() {
            if !seen.insert(owner) {
                continue;
            }
            let Some(d) = dropped.get_mut(&owner) else { continue };
            d.by.insert(r);
            work.extend(graph.functions[&owner].targets.iter().copied().filter_map(reach));
        }
    }

    let mut h = Fnv::default();
    for &r in replaced {
        h.word(r);
    }
    h.word(u32::MAX);
    for &d in dropped.keys() {
        h.word(d);
    }
    Keep { kept, why, dropped, digest: h.0 }
}

struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl Fnv {
    fn word(&mut self, w: u32) {
        for b in w.to_le_bytes() {
            self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
    }
}

/// Parse a replaced list: one entry per line, hex with or without `0x`,
/// then an optional name; `#` starts a comment.
pub fn parse_replaced(text: &str) -> Result<BTreeMap<u32, String>, String> {
    let mut out = BTreeMap::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let (addr, name) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let digits = addr.strip_prefix("0x").or_else(|| addr.strip_prefix("0X")).unwrap_or(addr);
        let va = u32::from_str_radix(digits, 16).map_err(|_| format!("line {}: bad address {addr:?}", n + 1))?;
        out.insert(va, name.trim().to_owned());
    }
    Ok(out)
}

/// The `h2 analyze --prune-report` TSV. Shards are the emit crate's
/// entry-VA buckets (`entry >> shard_shift`); a shard is emptied when every
/// function in it is dropped.
pub fn report(graph: &Graph, keep: &Keep, replaced: &BTreeMap<u32, String>, shard_shift: u32) -> String {
    let shard = |va: u32| va >> shard_shift;
    let mut live: BTreeMap<u32, usize> = BTreeMap::new();
    for &va in &keep.kept {
        *live.entry(shard(va)).or_default() += 1;
    }
    let emptied: BTreeSet<u32> = keep.dropped.keys().map(|&va| shard(va)).filter(|s| !live.contains_key(s)).collect();
    let name = |va: u32| replaced.get(&va).filter(|n| !n.is_empty()).map_or("-", String::as_str);

    let mut out = String::new();
    let _ = writeln!(out, "# point\tentry\tname\tbody\talone\tshared\tbytes\tshards\temptied");
    for &r in replaced.keys() {
        let body = if !graph.functions.contains_key(&r) {
            "unknown"
        } else if keep.kept.contains(&r) {
            "kept"
        } else {
            "dropped"
        };
        let mine = || keep.dropped.iter().filter(|(_, d)| d.by.contains(&r));
        let alone: Vec<_> = mine().filter(|(_, d)| d.by.len() == 1).collect();
        let shared = mine().count() - alone.len();
        let bytes: u64 = alone.iter().map(|(_, d)| u64::from(d.bytes)).sum();
        let shards: BTreeSet<u32> = alone.iter().map(|(va, _)| shard(**va)).collect();
        // Emptied by this point alone: every dropped function there is its.
        let own = shards
            .iter()
            .filter(|&&s| {
                emptied.contains(&s)
                    && keep.dropped.iter().all(|(&va, d)| shard(va) != s || d.by == BTreeSet::from([r]))
            })
            .count();
        let _ = writeln!(
            out,
            "point\t{r:#010x}\t{}\t{body}\t{}\t{shared}\t{bytes}\t{}\t{own}",
            name(r),
            alone.len(),
            shards.len()
        );
    }
    let _ = writeln!(out, "# function\tentry\tsource\tbytes\tshard\tby");
    for (&va, d) in &keep.dropped {
        let by: Vec<String> = d.by.iter().map(|b| format!("{b:#010x}")).collect();
        let source = graph.functions.get(&va).map_or_else(|| "-".to_owned(), |n| format!("{:?}", n.source));
        let by = if by.is_empty() { "-".to_owned() } else { by.join(",") };
        let _ = writeln!(out, "function\t{va:#010x}\t{source}\t{}\t{:05x}\t{by}", d.bytes, shard(va));
    }
    let bytes: u64 = keep.dropped.values().map(|d| u64::from(d.bytes)).sum();
    let touched: BTreeSet<u32> = keep.dropped.keys().map(|&va| shard(va)).collect();
    let _ = writeln!(out, "# total\tfunctions\tkept\tdropped\tbytes\tshards\temptied\tdigest");
    let _ = writeln!(
        out,
        "total\t{}\t{}\t{}\t{bytes}\t{}\t{}\t{:016x}",
        graph.functions.len(),
        keep.kept.len(),
        keep.dropped.len(),
        touched.len(),
        emptied.len(),
        keep.digest
    );
    out
}

/// `h2 analyze --prune-report --why <addr>`: why `va` is kept or dropped,
/// tab-separated. `interior` maps an interior address to its owner; each
/// `why` row is one step from the function back to a root (kind `found`,
/// `stored`, `extra` or `edge`, then the address the step lands on and the
/// caller or `-`); a `dropped` row lists the replaced entries whose bodies
/// reach it; `unknown` is an address that is not code.
pub fn why(graph: &Graph, keep: &Keep, va: u32) -> String {
    let mut out = String::new();
    let owner = if graph.functions.contains_key(&va) {
        va
    } else if let Some(&owner) = graph.interior.get(&va) {
        let _ = writeln!(out, "interior\t{va:#010x}\t{owner:#010x}");
        owner
    } else {
        let _ = writeln!(out, "unknown\t{va:#010x}");
        return out;
    };
    if let Some(path) = keep.path(owner) {
        let _ = writeln!(out, "# why\tentry\tkind\tat\tcaller");
        for (f, w) in path {
            let (kind, at, caller) = match w {
                Why::Found(s) => (format!("found:{s:?}"), f, None),
                Why::Stored { at } => ("stored".to_owned(), at, None),
                Why::Extra { at } => ("extra".to_owned(), at, None),
                Why::From { caller, at } => ("edge".to_owned(), at, Some(caller)),
            };
            let caller = caller.map_or_else(|| "-".to_owned(), |c| format!("{c:#010x}"));
            let _ = writeln!(out, "why\t{f:#010x}\t{kind}\t{at:#010x}\t{caller}");
        }
    } else if let Some(d) = keep.dropped.get(&owner) {
        let by: Vec<String> = d.by.iter().map(|b| format!("{b:#010x}")).collect();
        let by = if by.is_empty() { "-".to_owned() } else { by.join(",") };
        let _ = writeln!(out, "dropped\t{owner:#010x}\t{by}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(source: Source, targets: &[u32]) -> Node {
        Node { source, targets: targets.iter().copied().collect(), bytes: 16 }
    }

    /// entry 0x1000 calls 0x2000 (replaced) and 0x3000. 0x2000 calls
    /// 0x2100, which calls 0x2200. 0x3000 calls 0x3100.
    fn graph() -> Graph {
        Graph {
            functions: BTreeMap::from([
                (0x1000, node(Source::Entry, &[0x2000, 0x3000])),
                (0x2000, node(Source::Call, &[0x2100])),
                (0x2100, node(Source::Call, &[0x2200])),
                (0x2200, node(Source::Call, &[])),
                (0x3000, node(Source::Call, &[0x3100])),
                (0x3100, node(Source::Call, &[])),
            ]),
            interior: BTreeMap::new(),
            stored: BTreeSet::new(),
        }
    }

    fn set(v: &[u32]) -> BTreeSet<u32> {
        v.iter().copied().collect()
    }

    #[test]
    fn nothing_replaced_keeps_everything() {
        let g = graph();
        let k = keep_in(&g, &set(&[]), &set(&[]));
        assert_eq!(k.kept, g.functions.keys().copied().collect());
        assert!(k.dropped.is_empty());
    }

    #[test]
    fn cut_drops_the_body_and_its_only_reached_callees() {
        let k = keep_in(&graph(), &set(&[0x2000]), &set(&[]));
        assert_eq!(k.kept, set(&[0x1000, 0x3000, 0x3100]));
        assert_eq!(k.dropped.keys().copied().collect::<BTreeSet<_>>(), set(&[0x2000, 0x2100, 0x2200]));
        assert!(k.dropped.values().all(|d| d.by == set(&[0x2000])));
    }

    #[test]
    fn callee_reached_from_two_sides_is_kept() {
        let mut g = graph();
        g.functions.get_mut(&0x3100).unwrap().targets.insert(0x2200);
        let k = keep_in(&g, &set(&[0x2000]), &set(&[]));
        assert!(k.kept.contains(&0x2200));
        assert_eq!(k.dropped.keys().copied().collect::<BTreeSet<_>>(), set(&[0x2000, 0x2100]));
    }

    #[test]
    fn address_taken_callee_is_kept() {
        let mut g = graph();
        g.stored.insert(0x2100);
        let k = keep_in(&g, &set(&[0x2000]), &set(&[]));
        assert!(k.kept.contains(&0x2100) && k.kept.contains(&0x2200));
        assert_eq!(k.dropped.keys().copied().collect::<BTreeSet<_>>(), set(&[0x2000]));
    }

    #[test]
    fn non_call_sources_are_roots() {
        for source in [Source::GapScan, Source::CodeRef, Source::DataPointer] {
            let mut g = graph();
            g.functions.get_mut(&0x2200).unwrap().source = source;
            let k = keep_in(&g, &set(&[0x2000]), &set(&[]));
            assert!(k.kept.contains(&0x2200), "{source:?}");
            assert!(!k.kept.contains(&0x2100), "{source:?}");
        }
    }

    #[test]
    fn interior_entry_keeps_its_owner_even_when_replaced() {
        let mut g = graph();
        g.interior.insert(0x2008, 0x2000);
        g.stored.insert(0x2008);
        let k = keep_in(&g, &set(&[0x2000]), &set(&[]));
        assert!(k.kept.contains(&0x2000) && k.kept.contains(&0x2200));
        assert!(k.dropped.is_empty());
    }

    #[test]
    fn extra_roots_are_kept() {
        let k = keep_in(&graph(), &set(&[0x2000]), &set(&[0x2100]));
        assert!(k.kept.contains(&0x2100) && k.kept.contains(&0x2200));
    }

    #[test]
    fn entry_point_is_always_kept() {
        let k = keep_in(&graph(), &set(&[0x1000]), &set(&[]));
        assert!(k.kept.contains(&0x1000));
        assert!(k.dropped.is_empty());
    }

    #[test]
    fn nested_cuts_attribute_each_body_to_its_own_point() {
        let k = keep_in(&graph(), &set(&[0x2000, 0x2100]), &set(&[]));
        assert_eq!(k.dropped[&0x2000].by, set(&[0x2000]));
        assert_eq!(k.dropped[&0x2100].by, set(&[0x2100]));
        assert_eq!(k.dropped[&0x2200].by, set(&[0x2100]));
    }

    #[test]
    fn deterministic_and_digest_follows_the_dropped_set() {
        let g = graph();
        let a = keep_in(&g, &set(&[0x2000]), &set(&[]));
        assert_eq!(a, keep_in(&g, &set(&[0x2000]), &set(&[])));
        assert_ne!(a.digest, keep_in(&g, &set(&[0x2100]), &set(&[])).digest);
        assert_ne!(a.digest, keep_in(&g, &set(&[]), &set(&[])).digest);
    }

    #[test]
    fn why_names_every_kept_function_and_only_those() {
        let k = keep_in(&graph(), &set(&[0x2000]), &set(&[]));
        assert_eq!(k.why.keys().copied().collect::<BTreeSet<_>>(), k.kept);
        assert!(k.path(0x2100).is_none(), "dropped functions have no path");
        assert!(k.path(0x9999).is_none());
    }

    #[test]
    fn path_runs_back_through_callers_to_the_entry() {
        let k = keep_in(&graph(), &set(&[0x2000]), &set(&[]));
        assert_eq!(
            k.path(0x3100).unwrap(),
            vec![
                (0x3100, Why::From { caller: 0x3000, at: 0x3100 }),
                (0x3000, Why::From { caller: 0x1000, at: 0x3000 }),
                (0x1000, Why::Found(Source::Entry)),
            ]
        );
    }

    #[test]
    fn path_is_a_shortest_chain() {
        // 0x2200 is reachable as entry → 0x3000 → 0x3100 → 0x2100 → 0x2200
        // and as entry → 0x3000 → 0x2200; the walk names the second.
        let mut g = graph();
        g.functions.get_mut(&0x3100).unwrap().targets.insert(0x2100);
        g.functions.get_mut(&0x3000).unwrap().targets.insert(0x2200);
        let k = keep_in(&g, &set(&[0x2000]), &set(&[]));
        let p = k.path(0x2200).unwrap();
        assert_eq!(p.len(), 3, "{p:?}");
        assert_eq!(p[1].0, 0x3000);
    }

    /// A function only an indirect call reaches: its address sits in a
    /// vtable word, so the stored-address root keeps it and the path says so.
    #[test]
    fn indirect_target_in_a_table_is_kept_with_a_stored_root() {
        let mut g = graph();
        // 0x4000 is called only through a table; it calls 0x4100.
        g.functions.insert(0x4000, node(Source::Call, &[0x4100]));
        g.functions.insert(0x4100, node(Source::Call, &[]));
        g.stored.insert(0x4000);
        let k = keep_in(&g, &set(&[0x2000]), &set(&[]));
        assert!(k.kept.contains(&0x4000) && k.kept.contains(&0x4100));
        assert_eq!(
            k.path(0x4100).unwrap(),
            vec![(0x4100, Why::From { caller: 0x4000, at: 0x4100 }), (0x4000, Why::Stored { at: 0x4000 })]
        );
        // Without the table word nothing reaches it.
        g.stored.clear();
        let k = keep_in(&g, &set(&[0x2000]), &set(&[]));
        assert!(k.dropped.contains_key(&0x4000) && k.dropped[&0x4000].by.is_empty());
    }

    #[test]
    fn replaced_owner_kept_by_an_interior_entry_says_which() {
        let mut g = graph();
        g.interior.insert(0x2008, 0x2000);
        g.stored.insert(0x2008);
        let k = keep_in(&g, &set(&[0x2000]), &set(&[]));
        assert_eq!(k.path(0x2000).unwrap(), vec![(0x2000, Why::Stored { at: 0x2008 })]);
        assert_eq!(k.why[&0x2100], Why::From { caller: 0x2000, at: 0x2100 });
    }

    #[test]
    fn extra_root_is_named() {
        let k = keep_in(&graph(), &set(&[0x2000]), &set(&[0x2100]));
        assert_eq!(k.why[&0x2100], Why::Extra { at: 0x2100 });
        assert!(k.why[&0x2100].is_root());
        assert_eq!(k.path(0x2200).unwrap().last().unwrap().1, Why::Extra { at: 0x2100 });
    }

    #[test]
    fn why_report_rows() {
        let mut g = graph();
        g.interior.insert(0x3104, 0x3100);
        let k = keep_in(&g, &set(&[0x2000]), &set(&[]));
        let r = why(&g, &k, 0x3104);
        assert_eq!(
            r,
            "interior\t0x00003104\t0x00003100\n\
             # why\tentry\tkind\tat\tcaller\n\
             why\t0x00003100\tedge\t0x00003100\t0x00003000\n\
             why\t0x00003000\tedge\t0x00003000\t0x00001000\n\
             why\t0x00001000\tfound:Entry\t0x00001000\t-\n"
        );
        assert_eq!(why(&g, &k, 0x2100), "dropped\t0x00002100\t0x00002000\n");
        assert_eq!(why(&g, &k, 0x5555), "unknown\t0x00005555\n");
    }

    #[test]
    fn report_rows_and_totals() {
        let g = graph();
        let replaced = BTreeMap::from([(0x2000, "native_thing".to_owned())]);
        let k = keep_in(&g, &replaced.keys().copied().collect(), &set(&[]));
        let r = report(&g, &k, &replaced, 12);
        assert!(r.contains("point\t0x00002000\tnative_thing\tdropped\t3\t0\t48\t1\t1\n"), "{r}");
        assert!(r.contains("function\t0x00002100\tCall\t16\t00002\t0x00002000\n"), "{r}");
        assert!(r.contains(&format!("total\t6\t3\t3\t48\t1\t1\t{:016x}\n", k.digest)), "{r}");
    }

    #[test]
    fn parses_replaced_lists() {
        let l = parse_replaced("# header\n0x2d0aee entry\n1234\n\n0XABC  two words # note\n").unwrap();
        assert_eq!(l[&0x2d0aee], "entry");
        assert_eq!(l[&0x1234], "");
        assert_eq!(l[&0xabc], "two words");
        assert!(parse_replaced("zz\n").is_err());
    }

    #[test]
    fn built_from_an_image() {
        // 0x11000: call 0x11010; call 0x11020; ret
        // 0x11010: call 0x11030; ret
        // 0x11020: ret        0x11030: ret
        let mut code = vec![0xcc; 0x40];
        code[..11].copy_from_slice(&[0xe8, 0x0b, 0, 0, 0, 0xe8, 0x16, 0, 0, 0, 0xc3]);
        code[0x10..0x16].copy_from_slice(&[0xe8, 0x1b, 0, 0, 0, 0xc3]);
        code[0x20] = 0xc3;
        code[0x30] = 0xc3;
        let data = crate::tests::xbe_with(&code);
        let xbe = Xbe::parse(&data).unwrap();
        let a = crate::analyze(&xbe).unwrap();
        let g = Graph::build(&xbe, &a);
        assert_eq!(g.functions[&0x11000].targets, set(&[0x11010, 0x11020]));
        assert_eq!(g.functions[&0x11010].bytes, 6);
        let k = keep(&xbe, &a, &set(&[0x11010]), &set(&[]));
        assert_eq!(k.kept, set(&[0x11000, 0x11020]));
        assert_eq!(k.dropped.keys().copied().collect::<BTreeSet<_>>(), set(&[0x11010, 0x11030]));
    }
}
