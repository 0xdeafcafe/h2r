//! Evidence for light-host pruning. The collector is read-only with respect
//! to the title. Its timing totals include instrumentation overhead.

use anyhow::{Context, Result, ensure};
use engine::points;
use feature::{BuildError, Builder, Feature, Manifest, SnapshotPolicy};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use timeline::host::{Subsystem, thread_cpu_ns};

pub const ENV: &str = "H2_HOST_AUDIT";
pub const MODULE_ENV: &str = "H2_HOST_MODULE";

pub(crate) fn module_from_env() -> Option<PathBuf> {
    std::env::var_os(MODULE_ENV).filter(|v| !v.is_empty()).map(PathBuf::from)
}

pub(crate) fn module_owners(path: &Path, expected: &str) -> Result<BTreeSet<u32>> {
    let input = std::fs::File::open(path).with_context(|| format!("reading {}", path.display()))?;
    let report: Report = serde_json::from_reader(std::io::BufReader::new(input))?;
    ensure!(report.image == expected, "host module audit must come from the matching full clang module");
    report.check_coverage()?;
    Ok(report.functions)
}

#[derive(Clone, Copy, Debug, Ord, PartialOrd, Eq, PartialEq, Serialize, Deserialize)]
pub struct TagRead {
    datum: u32,
    group: u32,
    /// The intersection of an observed 64-byte line with the tag span,
    /// relative to this tag's start.
    start: u32,
    end: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Cost {
    calls: u64,
    cpu_ns: u64,
    max_ns: u64,
}

impl Cost {
    fn add(&mut self, other: &Self) {
        self.calls = self.calls.saturating_add(other.calls);
        self.cpu_ns = self.cpu_ns.saturating_add(other.cpu_ns);
        self.max_ns = self.max_ns.max(other.max_ns);
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Report {
    format: u32,
    image: String,
    light: bool,
    pub runs: u64,
    pub missing_runs: BTreeSet<String>,
    pub ticks: u64,
    pub functions: BTreeSet<u32>,
    pub registered_functions: BTreeSet<u32>,
    pub selectors: BTreeSet<(u32, u32)>,
    unmapped_entries: u64,
    pub tag_reads: BTreeMap<String, BTreeSet<TagRead>>,
    tag_lookups: BTreeMap<String, BTreeSet<u32>>,
    /// (usage, definition datum) by cache name.
    pub equipment_used: BTreeMap<String, BTreeSet<(String, u32)>>,
    /// (fourcc, definition datum) of weapons, vehicles and equipment visible in the tag index.
    pub equipment_index: BTreeMap<String, BTreeSet<(u32, u32)>>,
    pub timing: BTreeMap<String, Cost>,
    missing_hooks: BTreeSet<String>,
    tag_epochs: u64,
    unidentified_epochs: u64,
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

#[derive(Clone, Copy)]
struct TagSpan {
    datum: u32,
    group: u32,
    start: u32,
    end: u32,
}

fn tag_spans(g: &runtime::Guest) -> Vec<TagSpan> {
    use engine::tags::{TAG_ENTRIES_GLOBAL, TAG_INDEX_GLOBAL, entry_layout, index_layout};
    let read = |at| g.u32(at).unwrap_or(0);
    let index = read(TAG_INDEX_GLOBAL);
    let entries = read(TAG_ENTRIES_GLOBAL);
    if index == 0 || entries == 0 {
        return Vec::new();
    }
    let n = index.checked_add(index_layout::TAG_COUNT).map_or(0, read);
    if n > 65536 {
        return Vec::new();
    }
    (0..n).filter_map(|i| {
        let entry = entries.checked_add(i * entry_layout::STRIDE)?;
        let datum = read(entry.checked_add(entry_layout::DATUM)?);
        let group = read(entry.checked_add(entry_layout::GROUP)?);
        let start = read(entry.checked_add(entry_layout::ADDRESS)?);
        let size = read(entry.checked_add(entry_layout::SIZE)?);
        if datum == u32::MAX || datum & 0xffff != i || start == 0 || size == 0 {
            return None;
        }
        g.check(start, size as usize, runtime::guest::Access::Read, 1).ok()?;
        Some(TagSpan { datum, group, start, end: start.checked_add(size)? })
    }).collect()
}

#[derive(Default)]
struct State {
    map: Option<String>,
    tags: Vec<TagSpan>,
    active: BTreeMap<(&'static str, u32), Vec<u64>>,
    report: Report,
}

impl State {
    fn collect(&mut self, g: &mut runtime::Guest) {
        let log = g.take_trap_hits();
        let Some(map) = &self.map else { return };
        let mut pages = BTreeMap::<u32, u64>::new();
        for lines in log.lines.into_iter().filter(|l| l.read != 0) {
            *pages.entry(lines.page).or_default() |= lines.read;
        }
        let reads = self.report.tag_reads.entry(map.clone()).or_default();
        for t in &self.tags {
            for (&page, &line_mask) in pages.range((t.start >> 12)..=((t.end - 1) >> 12)) {
                let mut lines = line_mask;
                while lines != 0 {
                    let start = (u64::from(page) << 12) + u64::from(lines.trailing_zeros() * 64);
                    let first = start.max(u64::from(t.start));
                    let last = (start + 64).min(u64::from(t.end));
                    if first < last {
                        reads.insert(TagRead {
                            datum: t.datum,
                            group: t.group,
                            start: (first - u64::from(t.start)) as u32,
                            end: (last - u64::from(t.start)) as u32,
                        });
                    }
                    lines &= lines - 1;
                }
            }
        }
    }

    fn used(&mut self, kind: &str, datum: u32) {
        if let Some(map) = &self.map {
            self.report.equipment_used.entry(map.clone())
                .or_default().insert((kind.to_string(), datum));
        }
    }

    fn arm(&mut self, g: &mut runtime::Guest) {
        self.collect(g);
        g.disarm_all();
        self.tags = tag_spans(g);
        self.map = crate::telemetry::loaded_map_name(g).ok().filter(|name| !name.is_empty());
        self.report.tag_epochs += 1;
        if self.map.is_none() || self.tags.is_empty() {
            self.report.unidentified_epochs += 1;
            return;
        }
        if let Some(map) = &self.map {
            let list = self.report.equipment_index.entry(map.clone()).or_default();
            for t in &self.tags {
                if matches!(&t.group.to_be_bytes(), b"weap" | b"vehc" | b"eqip") {
                    list.insert((t.group, t.datum));
                }
            }
        }
        let pages: BTreeSet<_> = self.tags.iter()
            .flat_map(|t| (t.start >> 12)..=((t.end - 1) >> 12))
            .collect();
        g.watch_read_lines(pages);
    }
}

#[derive(Clone)]
pub(crate) struct Audit {
    path: PathBuf,
    state: Arc<Mutex<State>>,
}

impl Audit {
    pub fn from_env(image: impl FnOnce() -> String, light: bool) -> Option<Self> {
        let path = std::env::var_os(ENV).filter(|p| !p.is_empty())?;
        timeline::host::set_subsystem_audit(true);
        Some(Self {
            path: path.into(),
            state: Arc::new(Mutex::new(State {
                report: Report {
                    format: 1, image: image(), light, runs: 1, ..Default::default()
                },
                ..Default::default()
            })),
        })
    }

    fn report(&self, rt: &runtime::Runtime) -> Report {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.collect(&mut rt.memory());
        let mut report = state.report.clone();
        if let Some(coverage) = rt.guest_coverage() {
            report.functions = coverage.functions;
            report.registered_functions = coverage.registered;
            report.selectors = coverage.selectors;
            report.unmapped_entries = coverage.unmapped;
        }
        for (name, system) in [
            ("bots", Subsystem::Bots),
            ("telemetry", Subsystem::Telemetry),
            ("telemetry_worker", Subsystem::TelemetryWorker),
        ] {
            let sample = system.reading();
            report.timing.insert(name.into(), Cost {
                calls: sample.n, cpu_ns: sample.sum_ns, max_ns: sample.max_ns,
            });
        }
        report
    }

    pub fn add_method(&self, agent: &agent::Agent, rt: Arc<runtime::Runtime>) {
        let this = self.clone();
        agent.add_method(agent_proto::method::HOST_AUDIT, agent::Access::Read, move |params| {
            if !(params.is_null() || params.as_object().is_some_and(|v| v.is_empty())) {
                return Err(agent_proto::RpcError::new(agent_proto::code::INVALID_PARAMS, "host.audit takes no parameters"));
            }
            let r = this.report(&rt);
            r.write(&this.path)
                .map_err(|e| agent_proto::RpcError::new(agent_proto::code::UNAVAILABLE, e.to_string()))?;
            Ok(serde_json::json!({
                "path": this.path,
                "ticks": r.ticks,
                "functions": r.functions.len(),
                "registered_functions": r.registered_functions.len(),
                "tag_ranges": r.tag_reads.values().map(BTreeSet::len).sum::<usize>(),
                "timing": r.timing,
            }))
        });
    }

    pub fn installed(&self, installed: &feature::Installed) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.report.missing_hooks.extend(installed.notes.iter()
            .filter(|s| s.contains("host-audit")).cloned());
    }

    pub fn finish(&self, rt: &runtime::Runtime) -> Result<()> {
        self.report(rt).write(&self.path)?;
        rt.memory().remove_trap();
        timeline::host::set_subsystem_audit(false);
        Ok(())
    }
}

impl Feature for Audit {
    fn manifest(&self) -> Manifest {
        Manifest::new("host-audit", "Light host coverage, tag reads and tick attribution")
            .doc("docs/runtime/sim.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        b.snapshot(SnapshotPolicy::Stateless);
        for point in [&points::CACHE_TAGS_LOAD, &points::CACHE_BSP_FIXUP] {
            let state = Arc::clone(&self.state);
            b.raw().observe_after(point, "arm tag reads", move |o| {
                state.lock().unwrap_or_else(|e| e.into_inner()).arm(&mut o.guest());
            }).optional();
        }
        let state = Arc::clone(&self.state);
        b.raw().observe_before(&points::TAG_LOOKUP, "record tag lookup", move |o| {
            // ECX is the requested salted datum. Record requests even when
            // the guest rejects the group or the datum later in this call.
            let datum = o.cpu.ecx;
            let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
            if datum != u32::MAX
                && let Some(name) = state.map.clone()
            {
                state.report.tag_lookups.entry(name).or_default().insert(datum);
            }
        }).optional();

        let state = Arc::clone(&self.state);
        b.frame().on_tick("count timed ticks and ridden vehicles", move |o| {
            let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
            state.report.ticks += 1;
            if state.map.as_deref().is_none_or(|map| map == "mainmenu") {
                return;
            }
            let g = o.guest();
            for player in crate::telemetry::players(&g).unwrap_or_default() {
                if let Ok(Some(unit)) = crate::telemetry::object(&g, player.unit)
                    && let Some(definition) = unit.vehicle_definition
                {
                    state.used("vehicle_ridden", definition);
                }
            }
        });
        let state = Arc::clone(&self.state);
        b.raw().observe_before(&points::WEAPON_FIRE, "record fired weapon", move |o| {
            if let Some(weapon) = o.arg_u32("weapon")
                && let Ok(Some(definition)) = crate::telemetry::object_definition(&o.guest(), weapon)
            {
                state.lock().unwrap_or_else(|e| e.into_inner()).used("weapon_fired", definition);
            }
        }).optional();
        let state = Arc::clone(&self.state);
        b.raw().observe_before(&points::POWERUP_TAKEN, "record powerup", move |o| {
            if let Some(datum) = o.arg_u32("definition") {
                state.lock().unwrap_or_else(|e| e.into_inner()).used("powerup_taken", datum);
            }
        }).optional();

        for (name, point) in [
            ("tick", &points::GAME_TICK),
            ("objects_pre_physics", &points::OBJECTS_PRE_PHYSICS),
            ("physics", &points::WORLD_PHYSICS),
            ("objects_post_physics", &points::OBJECTS_POST_PHYSICS),
            ("objects_finalize", &points::OBJECTS_FINALIZE),
            ("ai", &points::AI_UPDATE),
            ("scripts", &points::SCRIPT_UPDATE),
            ("effect_parts", &points::EFFECT_PART_CREATE),
            ("particles", &points::PARTICLE_SYSTEM_UPDATE),
            ("director", &points::CAMERA_DIRECTOR),
            ("observer", &points::CAMERA_OBSERVER),
            ("first_person", &points::CAMERA_FIRST_PERSON),
            ("sound", &points::SOUND_UPDATE),
            ("ui_state", &points::UI_UPDATE),
            ("ui_draw", &points::UI_RENDER_LAYERS),
            ("network_pre", &points::NETWORK_PRE_UPDATE),
            ("network_post", &points::NETWORK_POST_UPDATE),
        ] {
            let state = Arc::clone(&self.state);
            b.raw().observe_before(point, name, move |o| {
                if let Some(at) = thread_cpu_ns() {
                    state.lock().unwrap_or_else(|e| e.into_inner())
                        .active.entry((name, o.thread_id)).or_default().push(at);
                }
            }).optional();
            let state = Arc::clone(&self.state);
            b.raw().observe_after(point, name, move |o| {
                if let Some(end) = thread_cpu_ns() {
                    let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(start) = state.active.get_mut(&(name, o.thread_id)).and_then(Vec::pop) {
                        let ns = end.saturating_sub(start);
                        state.report.timing.entry(name.into()).or_default().add(&Cost {
                            calls: 1, cpu_ns: ns, max_ns: ns,
                        });
                    }
                }
            }).optional();
        }
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
