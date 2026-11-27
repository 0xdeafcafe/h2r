//! The pruned table (docs/lifter/lift-pruning.md, "Boot safety"): what a
//! pruned lift left out, carried in the module so both tiers' boot can
//! stop by name on a transfer into it and refuse a plan that no longer
//! replaces a left-out root.
//!
//! The table is text, one row per line, exported as the NUL-terminated
//! `h2gen_pruned_v1`: from `h2gen_pruned.c` on the clang tier
//! ([`write_c`]), from a data object on the fast tier
//! (`clif::module::pruned`). A module without the symbol left nothing out.
//!
//! ```text
//! root   <entry>  <point>  <feature>     a replaced entry the lift cut
//! range  <start>  <end>    <root>        [start, end) left out, owned by root
//! ```
//!
//! Addresses are 8 hex digits; fields are tab-separated. A range's root is
//! 0 when no replaced entry reaches it.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::Path;

use analyze::prune::Keep;

/// The exported symbol both tiers define.
pub const SYMBOL: &str = "h2gen_pruned_v1";
/// The clang tier's source file for it.
pub const FILE: &str = "h2gen_pruned.c";

/// `H2_LIFT_PRUNE`: unset, empty or `1` leaves replaced code out; `0`
/// lifts everything.
pub fn enabled() -> Result<bool, String> {
    parse_switch(std::env::var("H2_LIFT_PRUNE").ok().as_deref())
}

/// [`enabled`] for a given value.
pub fn parse_switch(v: Option<&str>) -> Result<bool, String> {
    match v {
        None | Some("") | Some("1") => Ok(true),
        Some("0") => Ok(false),
        Some(v) => Err(format!("H2_LIFT_PRUNE={v}: expected 1 (the default) or 0")),
    }
}

/// A replaced entry: the hook point it is and the feature whose
/// implementation stands in for its body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    pub entry: u32,
    pub point: String,
    pub feature: String,
}

/// What the plan lets the lift leave out: the entries replaced in every
/// boot profile, and the guest functions features' Rust calls, which stay
/// (`boot::registration::lift_replaced`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Replaced {
    pub entries: BTreeMap<u32, Root>,
    pub calls: BTreeSet<u32>,
}

impl Replaced {
    /// The keep set ([`analyze::prune::keep`]).
    pub fn keep(&self, xbe: &xbe::Xbe, analysis: &analyze::Analysis) -> Keep {
        let replaced: BTreeSet<u32> = self.entries.keys().copied().collect();
        analyze::prune::keep(xbe, analysis, &replaced, &self.calls)
    }
}

/// `[start, end)` left out, owned by the replaced entry `root` (0: none
/// reaches it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Range {
    pub start: u32,
    pub end: u32,
    pub root: u32,
}

/// The table: the cut roots and the left-out ranges, sorted and disjoint.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Table {
    pub roots: Vec<Root>,
    pub ranges: Vec<Range>,
}

impl Table {
    /// The table for a lift that kept `owners` of `keep`: every dropped
    /// function's CFG blocks, less the bytes a kept owner's instructions
    /// cover (a tail shared with kept code stays runnable), each owned by
    /// the lowest replaced entry that reaches it.
    pub fn new(
        xbe: &xbe::Xbe,
        analysis: &analyze::Analysis,
        keep: &Keep,
        replaced: &Replaced,
        owners: &[crate::Owner],
    ) -> Table {
        let builder = analyze::cfg::CfgBuilder::new(analysis, xbe);
        let mut pieces = Vec::new();
        for (&entry, d) in &keep.dropped {
            let root = d.by.first().copied().unwrap_or(0);
            match builder.build(entry) {
                Some(cfg) if !cfg.blocks.is_empty() => {
                    pieces.extend(cfg.blocks.values().map(|b| Range { start: b.start, end: b.end, root }));
                }
                _ => pieces.push(Range { start: entry, end: entry.saturating_add(1), root }),
            }
        }
        let kept = merge(owners.iter().flat_map(|o| o.func.insns().map(|i| (i.pc, i.next_pc()))).collect());
        Table::from_pieces(
            replaced.entries.values().filter(|r| keep.dropped.contains_key(&r.entry)).cloned().collect(),
            pieces,
            &kept,
        )
    }

    /// `pieces` less `kept` (sorted, disjoint), made disjoint (an earlier
    /// piece keeps shared bytes) and joined where neighbours share a root.
    fn from_pieces(roots: Vec<Root>, mut pieces: Vec<Range>, kept: &[(u32, u32)]) -> Table {
        pieces.sort();
        let mut ranges: Vec<Range> = Vec::new();
        for p in pieces {
            for (start, end) in subtract((p.start, p.end), kept) {
                let start = ranges.last().map_or(start, |l| start.max(l.end));
                if start >= end {
                    continue;
                }
                let joins = ranges.last().is_some_and(|l| l.end == start && l.root == p.root);
                if joins {
                    ranges.last_mut().expect("joins a last range").end = end;
                } else {
                    ranges.push(Range { start, end, root: p.root });
                }
            }
        }
        Table { roots, ranges }
    }

    /// The table's text (the module docs' rows).
    pub fn encode(&self) -> String {
        let mut s = String::new();
        for r in &self.roots {
            s.push_str(&format!("root\t{:08x}\t{}\t{}\n", r.entry, r.point, r.feature));
        }
        for r in &self.ranges {
            s.push_str(&format!("range\t{:08x}\t{:08x}\t{:08x}\n", r.start, r.end, r.root));
        }
        s
    }

    /// Read [`Table::encode`]'s text.
    pub fn parse(text: &str) -> Result<Table, String> {
        let hex = |f: &str| u32::from_str_radix(f, 16).map_err(|e| format!("{f:?}: {e}"));
        let mut t = Table::default();
        for (n, line) in text.lines().enumerate().filter(|(_, l)| !l.is_empty()) {
            let f: Vec<&str> = line.split('\t').collect();
            let bad = || format!("pruned table line {}: {line:?}", n + 1);
            match f.as_slice() {
                ["root", entry, point, feature] => t.roots.push(Root {
                    entry: hex(entry).map_err(|e| format!("{}: {e}", bad()))?,
                    point: (*point).to_owned(),
                    feature: (*feature).to_owned(),
                }),
                ["range", start, end, root] => t.ranges.push(Range {
                    start: hex(start).map_err(|e| format!("{}: {e}", bad()))?,
                    end: hex(end).map_err(|e| format!("{}: {e}", bad()))?,
                    root: hex(root).map_err(|e| format!("{}: {e}", bad()))?,
                }),
                _ => return Err(bad()),
            }
        }
        Ok(t)
    }

    /// The exported symbol's bytes: the text and its NUL.
    pub fn bytes(&self) -> Vec<u8> {
        let mut b = self.encode().into_bytes();
        b.push(0);
        b
    }
}

/// Sort and join overlapping or touching intervals.
fn merge(mut v: Vec<(u32, u32)>) -> Vec<(u32, u32)> {
    v.sort();
    let mut out: Vec<(u32, u32)> = Vec::with_capacity(v.len());
    for (s, e) in v {
        if out.last().is_some_and(|l| s <= l.1) {
            let l = out.last_mut().expect("overlaps a last interval");
            l.1 = l.1.max(e);
        } else {
            out.push((s, e));
        }
    }
    out
}

/// `(s, e)` less the sorted, disjoint `cover`.
fn subtract((s, e): (u32, u32), cover: &[(u32, u32)]) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    let mut at = s;
    let mut i = cover.partition_point(|c| c.1 <= s);
    while i < cover.len() && cover[i].0 < e {
        if cover[i].0 > at {
            out.push((at, cover[i].0));
        }
        at = at.max(cover[i].1);
        i += 1;
    }
    if at < e {
        out.push((at, e));
    }
    out
}

/// Write the clang tier's `h2gen_pruned.c` into `dir`, or remove it when
/// nothing was left out.
pub fn write_c(dir: &Path, table: Option<&Table>) -> io::Result<()> {
    let path = dir.join(FILE);
    let Some(table) = table else {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    };
    let mut s = format!("{}#include \"h2gen.h\"\n\nH2_EXPORT const unsigned char {}[] = {{\n", crate::BANNER, SYMBOL);
    for chunk in table.bytes().chunks(16) {
        s.push_str("   ");
        for b in chunk {
            s.push_str(&format!(" {b:#04x},"));
        }
        s.push('\n');
    }
    s.push_str("};\n");
    if std::fs::read(&path).is_ok_and(|old| old == s.as_bytes()) {
        return Ok(());
    }
    crate::output::replace(&path, s.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_switch_is_on_unless_zero() {
        assert_eq!(parse_switch(None), Ok(true));
        assert_eq!(parse_switch(Some("")), Ok(true));
        assert_eq!(parse_switch(Some("1")), Ok(true));
        assert_eq!(parse_switch(Some("0")), Ok(false));
        assert!(parse_switch(Some("off")).is_err());
    }

    #[test]
    fn kept_bytes_are_cut_out_and_neighbours_join() {
        let roots = vec![Root { entry: 0x100, point: "B".into(), feature: "f".into() }];
        let pieces = vec![
            Range { start: 0x100, end: 0x110, root: 0x100 },
            Range { start: 0x110, end: 0x120, root: 0x100 },
            // Holds a tail B shares with kept code.
            Range { start: 0x120, end: 0x130, root: 0x100 },
            // Overlaps B's last block: the earlier range keeps those bytes.
            Range { start: 0x12c, end: 0x140, root: 0 },
        ];
        let t = Table::from_pieces(roots, pieces, &[(0x124, 0x128)]);
        assert_eq!(
            t.ranges,
            [
                Range { start: 0x100, end: 0x124, root: 0x100 },
                Range { start: 0x128, end: 0x130, root: 0x100 },
                Range { start: 0x130, end: 0x140, root: 0 },
            ]
        );
        assert_eq!(Table::parse(&t.encode()), Ok(t.clone()));
        assert_eq!(t.bytes().last(), Some(&0));
        assert!(Table::parse("range\t1\t2").is_err());
    }
}
