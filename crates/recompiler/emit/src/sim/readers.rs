//! Who reads a block of guest memory: the loads of a [`census`] matched to
//! named blocks ([`Watch`]).
//!
//! `docs/engine/sim-render-split.md` slice 2 asks this of the tick: does
//! any code `game_tick` reaches read the flare array or what the
//! push-buffer callback `0x00024970` stores? A load matches a block when
//! its address is a constant inside it, an index off a base inside it, or
//! any offset from a pointer read from one of the block's pointer globals
//! ([`Watch::through`]). Loads through a pointer the walk cannot name
//! (`Kind::Unknown`, or a root that is an argument or entry register) are
//! not matched: the static answer covers named addresses, and the run-time
//! census (`render_census`) covers the rest.
//!
//! [`census`]: super::census

use std::collections::BTreeMap;

use super::census::{Kind, Root, Site};

/// A named block of guest memory.
#[derive(Debug, Clone, Copy)]
pub struct Watch {
    pub name: &'static str,
    /// The block's addresses, `start..end`.
    pub start: u32,
    pub end: u32,
    /// Globals holding a pointer to the block (or to a record of it): a
    /// load at any offset from their value is a read of the block.
    pub through: &'static [u32],
}

impl Watch {
    /// The load `site` can read the block.
    pub fn matches(&self, site: &Site) -> bool {
        match site.kind {
            Kind::Absolute(a) => a < self.end && a.saturating_add(site.bytes.max(1)) > self.start,
            Kind::Indexed(base) => (self.start..self.end).contains(&base),
            Kind::Field(Root::Global(g), _) | Kind::Reachable(Root::Global(g)) => self.through.contains(&g),
            _ => false,
        }
    }
}

/// The loads of `loads` that can read `watch`, by function.
pub fn readers<'a>(loads: &'a [Site], watch: &Watch) -> BTreeMap<u32, Vec<&'a Site>> {
    let mut by: BTreeMap<u32, Vec<&Site>> = BTreeMap::new();
    for s in loads.iter().filter(|s| watch.matches(s)) {
        by.entry(s.function).or_default().push(s);
    }
    by
}

/// `readers` as markdown: one row per function, with its load PCs.
pub fn markdown(watch: &Watch, by: &BTreeMap<u32, Vec<&Site>>) -> String {
    let mut s = format!("### {} ({:#010x}..{:#010x})\n\n", watch.name, watch.start, watch.end);
    if by.is_empty() {
        s.push_str("No load of the closure reads it.\n");
        return s;
    }
    s.push_str("| function | loads | pcs |\n|---|---|---|\n");
    for (f, v) in by {
        let pcs: Vec<String> = v.iter().take(6).map(|l| format!("{:#010x}", l.pc)).collect();
        s.push_str(&format!("| {f:#010x} | {} | {} |\n", v.len(), pcs.join(" ")));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOCK: Watch = Watch { name: "block", start: 0x0051_f40c, end: 0x0052_340c, through: &[0x0048_5890] };

    fn load(kind: Kind, bytes: u32) -> Site {
        Site { function: 0x1000, pc: 0x1004, kind, bytes, repeat: false }
    }

    #[test]
    fn constants_match_by_overlap() {
        assert!(BLOCK.matches(&load(Kind::Absolute(0x0051_f40c), 4)));
        assert!(BLOCK.matches(&load(Kind::Absolute(0x0052_3408), 4)));
        // A dword ending on the block's first byte overlaps it.
        assert!(BLOCK.matches(&load(Kind::Absolute(0x0051_f409), 4)));
        assert!(!BLOCK.matches(&load(Kind::Absolute(0x0051_f408), 4)));
        assert!(!BLOCK.matches(&load(Kind::Absolute(0x0052_340c), 1)));
    }

    #[test]
    fn an_index_matches_by_its_base() {
        assert!(BLOCK.matches(&load(Kind::Indexed(0x0051_f40c), 4)));
        assert!(!BLOCK.matches(&load(Kind::Indexed(0x0051_f400), 4)));
    }

    #[test]
    fn a_pointer_global_matches_at_any_depth() {
        let g = Root::Global(0x0048_5890);
        assert!(BLOCK.matches(&load(Kind::Field(g, Some(0x10)), 4)));
        assert!(BLOCK.matches(&load(Kind::Field(g, None), 4)));
        assert!(BLOCK.matches(&load(Kind::Reachable(g), 4)));
        assert!(!BLOCK.matches(&load(Kind::Field(Root::Global(0x0048_5898), Some(0)), 4)));
        assert!(!BLOCK.matches(&load(Kind::Field(Root::Arg(4), Some(0)), 4)));
        assert!(!BLOCK.matches(&load(Kind::Unknown, 4)));
        assert!(!BLOCK.matches(&load(Kind::Stack, 4)));
    }

    #[test]
    fn readers_group_by_function() {
        let loads = [load(Kind::Absolute(0x0051_f40c), 4), Site { function: 0x2000, ..load(Kind::Indexed(0x0051_f40c), 1) }, load(Kind::Stack, 4)];
        let by = readers(&loads, &BLOCK);
        assert_eq!(by.keys().copied().collect::<Vec<_>>(), vec![0x1000, 0x2000]);
        assert!(markdown(&BLOCK, &by).contains("| 0x00001000 | 1 | 0x00001004 |"));
        assert!(markdown(&BLOCK, &BTreeMap::new()).contains("No load"));
    }
}
