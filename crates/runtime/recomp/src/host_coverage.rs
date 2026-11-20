//! Publishable coverage is a sorted list of owner/selector address pairs.

use std::collections::BTreeSet;
use std::fmt::Write;
use std::path::Path;

use anyhow::{Context, Result, ensure};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Coverage {
    pub selectors: BTreeSet<(u32, u32)>,
}

impl Coverage {
    pub fn read(path: &Path) -> Result<Self> {
        let input =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&input).with_context(|| format!("invalid host coverage {}", path.display()))
    }

    fn parse(input: &str) -> Result<Self> {
        fn address(word: &str) -> Option<u32> {
            let hex = word.strip_prefix("0x")?;
            if hex.len() != 8 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            u32::from_str_radix(hex, 16).ok().filter(|&addr| addr != 0)
        }

        let mut list = Self::default();
        for (line, row) in input.lines().enumerate() {
            let fields: Vec<_> = row.split_whitespace().collect();
            let pair = match fields.as_slice() {
                [owner, selector] => address(owner).zip(address(selector)),
                _ => None,
            };
            list.selectors.insert(pair.with_context(|| {
                format!(
                    "line {} needs two nonzero 0x-prefixed 32-bit addresses",
                    line + 1
                )
            })?);
        }
        ensure!(!list.selectors.is_empty(), "no owner/selector pairs");
        Ok(list)
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        ensure!(!self.selectors.is_empty(), "no owner/selector pairs");
        ensure!(
            self.selectors
                .iter()
                .all(|&(owner, selector)| owner != 0 && selector != 0),
            "coverage contains a zero address"
        );
        std::fs::write(path, self.canonical())
            .with_context(|| format!("writing {}", path.display()))
    }

    fn canonical(&self) -> String {
        let mut output = String::new();
        for &(owner, selector) in &self.selectors {
            writeln!(output, "{owner:#010x} {selector:#010x}").unwrap();
        }
        output
    }

    pub fn fingerprint(&self) -> u64 {
        crate::fallback::fnv64(self.canonical().as_bytes())
    }

    pub fn owners(&self, plan: &[emit::Owner]) -> Result<BTreeSet<u32>> {
        ensure!(!self.selectors.is_empty(), "no owner/selector pairs");
        let mut owners = BTreeSet::new();
        for &(owner, selector) in &self.selectors {
            let valid = plan
                .binary_search_by_key(&owner, |o| o.func.entry)
                .is_ok_and(|i| plan[i].selectors.contains(&selector));
            ensure!(
                valid,
                "host coverage has unknown owner/selector {owner:#010x} {selector:#010x}"
            );
            owners.insert(owner);
        }
        Ok(owners)
    }
}

#[cfg(test)]
mod tests {
    use super::Coverage;

    #[test]
    fn coverage_round_trips_with_only_sorted_addresses() {
        let list = Coverage::parse(
            "0x00002000 0x0000200F\n0x00001000 0x00001000\n0x00002000 0x0000200f\n",
        )
        .unwrap();
        let output = "0x00001000 0x00001000\n0x00002000 0x0000200f\n";
        assert_eq!(list.canonical(), output);
        assert_eq!(Coverage::parse(output).unwrap(), list);
        for bad in [
            "",
            "0x00000000 0x00001000",
            "0x1000 0x00001000",
            "0x00001000 0x00001000 game-data",
            "0x+0001000 0x00001000",
        ] {
            assert!(Coverage::parse(bad).is_err(), "{bad:?}");
        }
    }
}
