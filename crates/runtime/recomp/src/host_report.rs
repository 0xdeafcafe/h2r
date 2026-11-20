//! The host audit's report, and the light host's module list read from it
//! (docs/runtime/sim.md). Plain data: `host_audit` collects it in a run,
//! `h2 sim` merges runs, and the build reads which owners a light host
//! compiles natively (`H2_HOST_MODULE`).

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

pub const ENV: &str = "H2_HOST_AUDIT";
pub const MODULE_ENV: &str = "H2_HOST_MODULE";

pub fn module_from_env() -> Option<PathBuf> {
    std::env::var_os(MODULE_ENV).filter(|v| !v.is_empty()).map(PathBuf::from)
}

pub fn module_owners(path: &Path, expected: &str) -> Result<BTreeSet<u32>> {
    let input = std::fs::File::open(path).with_context(|| format!("reading {}", path.display()))?;
    let report: Report = serde_json::from_reader(std::io::BufReader::new(input))?;
    ensure!(report.image == expected, "host module audit must come from the matching full clang module");
    report.check_coverage()?;
    Ok(report.functions)
}

#[derive(Clone, Copy, Debug, Ord, PartialOrd, Eq, PartialEq, Serialize, Deserialize)]
pub struct TagRead {
    pub datum: u32,
    pub group: u32,
    /// The intersection of an observed 64-byte line with the tag span,
    /// relative to this tag's start.
    pub start: u32,
    pub end: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Cost {
    pub calls: u64,
    pub cpu_ns: u64,
    pub max_ns: u64,
}

impl Cost {
    pub fn add(&mut self, other: &Self) {
        self.calls = self.calls.saturating_add(other.calls);
        self.cpu_ns = self.cpu_ns.saturating_add(other.cpu_ns);
        self.max_ns = self.max_ns.max(other.max_ns);
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Report {
    pub format: u32,
    pub image: String,
    pub light: bool,
    pub runs: u64,
    pub missing_runs: BTreeSet<String>,
    pub ticks: u64,
    pub functions: BTreeSet<u32>,
    pub registered_functions: BTreeSet<u32>,
    pub selectors: BTreeSet<(u32, u32)>,
    pub unmapped_entries: u64,
    pub tag_reads: BTreeMap<String, BTreeSet<TagRead>>,
    pub tag_lookups: BTreeMap<String, BTreeSet<u32>>,
    /// (usage, definition datum) by cache name.
    pub equipment_used: BTreeMap<String, BTreeSet<(String, u32)>>,
    /// (fourcc, definition datum) of weapons, vehicles and equipment visible in the tag index.
    pub equipment_index: BTreeMap<String, BTreeSet<(u32, u32)>>,
    pub timing: BTreeMap<String, Cost>,
    pub missing_hooks: BTreeSet<String>,
    pub tag_epochs: u64,
    pub unidentified_epochs: u64,
}

impl Report {
    fn check_coverage(&self) -> Result<()> {
        ensure!(self.format == 1 && self.light && self.image.starts_with("xbe ") && self.image.lines().count() == 1,
            "host audit must use the light profile and a full clang module");
        ensure!(self.runs > 0 && self.ticks > 0 && self.unmapped_entries == 0 && !self.functions.is_empty(),
            "host audit has no complete generated-entry coverage");
        ensure!(self.missing_runs.is_empty() && self.missing_hooks.is_empty(),
            "host audit has missing runs or hooks");
        Ok(())
    }

    pub fn coverage(&self) -> Result<crate::host_coverage::Coverage> {
        self.check_coverage()?;
        let owners: BTreeSet<_> = self.selectors.iter().map(|&(owner, _)| owner).collect();
        ensure!(owners == self.functions && owners.is_subset(&self.registered_functions),
            "host audit selectors disagree with the generated registry");
        Ok(crate::host_coverage::Coverage { selectors: self.selectors.clone() })
    }

    pub fn merge(&mut self, other: Self) -> Result<()> {
        if self.runs == 0 {
            *self = other;
            return Ok(());
        }
        ensure!(
            (self.format, &self.image, self.light) == (other.format, &other.image, other.light)
                && self.registered_functions == other.registered_functions,
            "cannot merge audit reports from different images, registries or profiles"
        );
        self.runs += other.runs;
        self.missing_runs.extend(other.missing_runs);
        self.ticks += other.ticks;
        self.functions.extend(other.functions);
        self.selectors.extend(other.selectors);
        self.unmapped_entries += other.unmapped_entries;
        self.missing_hooks.extend(other.missing_hooks);
        self.tag_epochs += other.tag_epochs;
        self.unidentified_epochs += other.unidentified_epochs;
        for (key, reads) in other.tag_reads {
            self.tag_reads.entry(key).or_default().extend(reads);
        }
        for (key, lookups) in other.tag_lookups {
            self.tag_lookups.entry(key).or_default().extend(lookups);
        }
        for (key, used) in other.equipment_used {
            self.equipment_used.entry(key).or_default().extend(used);
        }
        for (key, index) in other.equipment_index {
            self.equipment_index.entry(key).or_default().extend(index);
        }
        for (key, cost) in other.timing {
            self.timing.entry(key).or_default().add(&cost);
        }
        Ok(())
    }

    /// Indexed equipment without an observed use of the corresponding kind.
    pub fn equipment_gaps(&self) -> BTreeMap<&str, BTreeSet<(u32, u32)>> {
        let mut gaps = BTreeMap::new();
        for (map, index) in &self.equipment_index {
            // The menu can index inherited tags but has no gameplay seats.
            if map == "mainmenu" {
                continue;
            }
            let used = self.equipment_used.get(map);
            let missing: BTreeSet<_> = index.iter().copied().filter(|&(group, datum)| {
                let kind = match &group.to_be_bytes() {
                    b"weap" => "weapon_fired",
                    b"vehc" => "vehicle_ridden",
                    b"eqip" => "powerup_taken",
                    _ => return false,
                };
                !used.is_some_and(|uses| uses.iter().any(|(k, d)| k == kind && *d == datum))
            }).collect();
            if !missing.is_empty() {
                gaps.insert(map.as_str(), missing);
            }
        }
        gaps
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let f = std::fs::File::create(path).with_context(|| format!("writing {}", path.display()))?;
        let mut writer = std::io::BufWriter::new(f);
        serde_json::to_writer(&mut writer, self)?;
        std::io::Write::flush(&mut writer)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_unions_observations_and_rejects_other_images() {
        let first = Report {
            runs: 1,
            format: 1,
            light: true,
            image: "xbe one".into(),
            ticks: 5,
            registered_functions: [1, 2, 3].into(),
            functions: [1, 2].into(),
            selectors: [(1, 1), (2, 2)].into(),
            ..Report::default()
        };
        let second = Report {
            functions: [2, 3].into(),
            selectors: [(2, 2), (2, 20), (3, 30)].into(),
            ..first.clone()
        };
        let mut merged = first.clone();
        merged.merge(second.clone()).unwrap();
        assert_eq!(merged.functions, [1, 2, 3].into());
        assert_eq!((merged.runs, merged.ticks), (2, 10));
        let coverage = merged.coverage().unwrap();
        assert_eq!(coverage.selectors, [(1, 1), (2, 2), (2, 20), (3, 30)].into());
        let mut reverse = second;
        reverse.merge(first).unwrap();
        assert_eq!(reverse.coverage().unwrap(), coverage);
        assert!(merged.merge(Report { image: "xbe two".into(), ..merged.clone() }).is_err());
        merged.missing_runs.insert("missing-match".into());
        assert!(merged.coverage().is_err());
    }
}
