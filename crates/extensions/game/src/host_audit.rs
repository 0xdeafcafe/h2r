//! Evidence for light-host pruning. The collector is read-only with respect
//! to the title. Its timing totals include instrumentation overhead.

use anyhow::Result;
use engine::points;
use hooks::{BuildError, Builder, Feature, Manifest, SnapshotPolicy};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{Arc, Mutex},
};
use timeline::host::{Subsystem, thread_cpu_ns};

// The report and the module list are `host_report`'s; `h2 sim` and `h2
// build` name them here.
pub use crate::host_report::{Cost, ENV, MODULE_ENV, Report, TagRead, module_from_env, module_owners};

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
pub struct Audit {
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
            vfs::sim_store::record::flush();
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

    pub fn installed(&self, installed: &hooks::Installed) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.report.missing_hooks.extend(installed.notes.iter()
            .filter(|s| s.contains("host-audit")).cloned());
    }

    pub fn finish(&self, rt: &runtime::Runtime) -> Result<()> {
        vfs::sim_store::record::flush();
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
            // The AI and script updates are Rust tick phases
            // (engine::ai_update, engine::hs_update): their times are `ai`
            // and `script_scheduler` in sim.ledger.
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
