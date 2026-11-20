//! Compare our function starts with another tool's (Ghidra's export from
//! scripts/ghidra/ExportFunctions.java). Neither side is ground truth; the
//! disagreements are where to look.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::{Analysis, Source};

#[derive(Debug, Clone)]
pub struct ExternalFunction {
    pub entry: u32,
    pub size: u32,
    /// DEFAULT for auto-named, IMPORTED / USER_DEFINED / ANALYSIS otherwise.
    pub source: String,
    pub name: String,
}

/// Parse the TSV written by ExportFunctions.java. Addresses that aren't
/// plain hex (external or overlay spaces) are skipped.
pub fn parse_ghidra_tsv(text: &str) -> Vec<ExternalFunction> {
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let mut c = line.split('\t');
            let entry = u32::from_str_radix(c.next()?.trim_start_matches("0x"), 16).ok()?;
            let size = c.next()?.parse().ok()?;
            let (_min, _max) = (c.next()?, c.next()?);
            let source = c.next()?.to_owned();
            let name = c.next()?.to_owned();
            Some(ExternalFunction { entry, size, source, name })
        })
        .collect()
}

pub struct Comparison {
    pub both: usize,
    /// Their starts we don't have, by what we have there:
    /// 'I' an instruction inside another function, 'i' mid-instruction,
    /// 'd' data, '?' undecoded.
    pub only_theirs_by_claim: BTreeMap<char, Vec<u32>>,
    pub only_ours: BTreeMap<Source, Vec<u32>>,
    pub only_theirs: Vec<u32>,
    /// Our conflict addresses that the other tool treats as a function start.
    pub conflicts_at_their_starts: usize,
    /// Functions the other tool has a real name for, per our section.
    pub named: BTreeMap<String, usize>,
    pub named_examples: Vec<(u32, String)>,
}

pub fn compare(ours: &Analysis, theirs: &[ExternalFunction]) -> Comparison {
    let in_code = |va: u32| ours.section_of(va).is_some();
    let their_starts: BTreeSet<u32> = theirs.iter().map(|f| f.entry).filter(|v| in_code(*v)).collect();
    let mut c = Comparison {
        both: 0,
        only_theirs_by_claim: BTreeMap::new(),
        only_ours: BTreeMap::new(),
        only_theirs: Vec::new(),
        conflicts_at_their_starts: ours.conflicts.iter().filter(|c| their_starts.contains(&c.va)).count(),
        named: BTreeMap::new(),
        named_examples: Vec::new(),
    };
    for (va, f) in &ours.functions {
        if their_starts.contains(va) {
            c.both += 1;
        } else {
            c.only_ours.entry(f.source).or_default().push(*va);
        }
    }
    c.only_theirs = their_starts.iter().copied().filter(|v| !ours.functions.contains_key(v)).collect();
    for &v in &c.only_theirs {
        let claim = ours.claim(v).map_or(' ', |(ch, _)| ch);
        c.only_theirs_by_claim.entry(claim).or_default().push(v);
    }
    for f in theirs {
        if f.source != "DEFAULT"
            && let Some(sec) = ours.section_of(f.entry)
        {
            *c.named.entry(sec.to_owned()).or_default() += 1;
            if c.named_examples.len() < 15 && sec != ".text" {
                c.named_examples.push((f.entry, f.name.clone()));
            }
        }
    }
    c
}

impl fmt::Display for Comparison {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "function starts in both: {}", self.both)?;
        writeln!(f, "only ours:")?;
        for (src, v) in &self.only_ours {
            let sample: Vec<String> = v.iter().take(5).map(|a| format!("{a:#x}")).collect();
            writeln!(f, "  {src:?}: {} (e.g. {})", v.len(), sample.join(" "))?;
        }
        let sample: Vec<String> = self.only_theirs.iter().take(8).map(|a| format!("{a:#x}")).collect();
        writeln!(f, "only theirs: {} (e.g. {})", self.only_theirs.len(), sample.join(" "))?;
        for (claim, v) in &self.only_theirs_by_claim {
            let what = match claim {
                'I' => "inside another of our functions",
                'i' => "mid-instruction for us",
                'd' => "data for us",
                '?' => "undecoded for us",
                _ => "other",
            };
            let sample: Vec<String> = v.iter().take(6).map(|a| format!("{a:#x}")).collect();
            writeln!(f, "  {what}: {} (e.g. {})", v.len(), sample.join(" "))?;
        }
        writeln!(f, "our conflicts at their function starts: {}", self.conflicts_at_their_starts)?;
        writeln!(f, "named by them, per section: {:?}", self.named)?;
        for (va, name) in &self.named_examples {
            writeln!(f, "  {va:#010x} {name}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_export() {
        let tsv = "entry\tsize\tmin\tmax\tsource\tname\n\
                   00012000\t16\t00012000\t0001200f\tDEFAULT\tFUN_00012000\n\
                   EXTERNAL:00000001\t1\tx\tx\tIMPORTED\tKeBugCheck\n\
                   003f5240\t40\t003f5240\t003f5267\tIMPORTED\tD3DDevice_Clear\n";
        let f = parse_ghidra_tsv(tsv);
        assert_eq!(f.len(), 2);
        assert_eq!(f[1].entry, 0x3f5240);
        assert_eq!(f[1].name, "D3DDevice_Clear");
    }
}
