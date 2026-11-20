//! View-scoped counts of Direct3D entry invocations, before resolution or
//! any native admission. The agent exposes only completed present windows.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::Mutex;

use feature::native_residual::{ViewScope, probe_on, view};
use serde_json::{Value, json};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Caller {
    view: ViewScope,
    entry: &'static str,
    pc: u32,
}

#[derive(Default)]
struct Count {
    calls: u64,
    backend_draws: u64,
}

type Counts = BTreeMap<Caller, Count>;

thread_local! {
    static ACTIVE: Cell<Option<(u64, Caller)>> = const { Cell::new(None) };
}

struct Restore(Option<(u64, Caller)>);

impl Drop for Restore {
    fn drop(&mut self) {
        ACTIVE.set(self.0);
    }
}

static CENSUS: Mutex<BTreeMap<u64, Counts>> = Mutex::new(BTreeMap::new());

pub(crate) fn install_census(rt: &runtime::Runtime, entries: &[(u32, &'static str)]) {
    let frames = probe_on(&feature::Switches::from_env().unwrap_or_default());
    if frames.is_empty() {
        return;
    }
    rt.edit_dispatch(|d| {
        for &(addr, entry) in entries {
            let Some(previous) = d.override_handler(addr) else {
                continue;
            };
            let context_only = d.is_context_only(addr);
            let name = d.override_name(addr).unwrap_or(entry).to_owned();
            d.register_override(addr, name, move |cpu, ctx, selector| {
                let frame = crate::d3d_diag::presents() + 1;
                let caller = frames.contains(&frame).then(|| Caller {
                    view: view(),
                    entry,
                    pc: ctx.memory().u32(cpu.esp).unwrap_or(0),
                });
                if let Some(caller) = caller {
                    let mut census = CENSUS.lock().unwrap_or_else(|e| e.into_inner());
                    census
                        .entry(frame)
                        .or_default()
                        .entry(caller)
                        .or_default()
                        .calls += 1;
                }
                let _restore = Restore(ACTIVE.replace(caller.map(|caller| (frame, caller))));
                Ok(previous.invoke(cpu, ctx, selector))
            });
            if context_only {
                d.mark_context_only(addr);
            }
        }
    });
}

pub(crate) fn observing_draw() -> bool {
    ACTIVE.get().is_some()
}

/// Counts only calls that reach the backend draw path. A pushed packet
/// flushed by a later ABI entry has no original return PC at this point.
pub(crate) fn backend_draw(entry: &'static str) {
    let Some((frame, caller)) = ACTIVE.get() else {
        return;
    };
    let caller = if caller.entry == entry {
        caller
    } else {
        Caller {
            entry,
            pc: 0,
            view: caller.view,
        }
    };
    let mut census = CENSUS.lock().unwrap_or_else(|e| e.into_inner());
    census
        .entry(frame)
        .or_default()
        .entry(caller)
        .or_default()
        .backend_draws += 1;
}

pub(crate) fn census() -> Option<Value> {
    let requested = probe_on(&feature::Switches::from_env().unwrap_or_default());
    if requested.is_empty() {
        return None;
    }
    let presented = crate::d3d_diag::presents();
    let census = CENSUS.lock().unwrap_or_else(|e| e.into_inner());
    Some(json!({
        "requested": requested,
        "frames": requested.iter().filter(|&&frame| frame <= presented).map(|&frame| {
            summarize(frame, census.get(&frame))
        }).collect::<Vec<_>>(),
    }))
}

fn summarize(frame: u64, counts: Option<&Counts>) -> Value {
    let mut by_view: BTreeMap<ViewScope, Vec<Value>> = BTreeMap::new();
    let mut totals: BTreeMap<ViewScope, Count> = BTreeMap::new();
    for (caller, count) in counts.into_iter().flatten() {
        let total = totals.entry(caller.view).or_default();
        total.calls += count.calls;
        total.backend_draws += count.backend_draws;
        by_view.entry(caller.view).or_default().push(json!({
            "entry": caller.entry,
            "return_pc": (caller.pc != 0).then(|| format!("{:#010x}", caller.pc)),
            "calls": count.calls,
            "backend_draw_attempts": count.backend_draws,
        }));
    }
    json!({
        "present": frame,
        "views": by_view.into_iter().map(|(view, callers)| json!({
            "view": format!("{view:?}"),
            "calls": totals[&view].calls,
            "backend_draw_attempts": totals[&view].backend_draws,
            "distinct_callers": callers.iter().filter(|row| row["calls"].as_u64().is_some_and(|n| n > 0)).count(),
            "callers": callers,
        })).collect::<Vec<_>>(),
    })
}

/// Use the same resolved indexed input and two-copy reservation as the
/// opaque layers. No sky model state, sort order or layer handler is moved.
pub(crate) fn install<B: d3d::Backend + Send + 'static>(
    rt: &runtime::Runtime,
    device: &d3d::abi::SharedDevice<B>,
    entries: &[(u32, &'static str)],
) {
    install_census(rt, entries);
    let enabled = feature::Switches::from_env().is_ok_and(|s| feature::native_residual::on(&s));
    if enabled && let Ok(mut device) = device.lock() {
        device.set_native_residual(true);
        device.add_draw_admission(std::sync::Arc::new(ResidualDraws::default()));
    }
}

#[derive(Default)]
struct ResidualDraws {
    geometry: std::sync::Arc<native_render::world_layers_gpu::Cache>,
}

impl d3d::DrawAdmission for ResidualDraws {
    fn admit(
        &self,
        snapshot: &d3d::DrawSnapshot<'_>,
    ) -> Option<(d3d::ExternalPlan, Box<dyn d3d::ExternalPass>)> {
        use native_render::world_layers::{Owner, Site};

        let call = crate::native_world_layers_glue::indexed_draw()?;
        let draw = snapshot.draw;
        if draw.entry != "D3DDevice_DrawIndexedVertices"
            || draw.indices.as_ref()?.len() != call.index_count? as usize
        {
            return None;
        }
        let stage = match (call.layer.site, call.layer.number) {
            (Site::Sky, 15) => timeline::native::Stage::World,
            (Site::Scene | Site::AlternateView, 2) if call.layer.list != 0 => {
                match call.geometry?.owner {
                    Owner::Cluster(_) | Owner::Placement(_) => timeline::native::Stage::World,
                    Owner::Object { .. } => timeline::native::Stage::Objects,
                }
            }
            _ => return None,
        };
        let pass = native_render::world_layers_gpu::IndexedPass::new(
            draw,
            std::sync::Arc::clone(&self.geometry),
            call.frame,
            None,
        )?;
        Some((
            d3d::ExternalPlan {
                stage,
                view: call.layer.view,
                ranges: 1,
                slots: Vec::new(),
            },
            Box::new(pass),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callers_and_invocations_are_counted_by_scene_kind() {
        let counts = Counts::from([
            (
                Caller {
                    view: ViewScope::Scene(1),
                    entry: "clear",
                    pc: 100,
                },
                Count {
                    calls: 4,
                    backend_draws: 3,
                },
            ),
            (
                Caller {
                    view: ViewScope::Scene(1),
                    entry: "clear",
                    pc: 200,
                },
                Count {
                    calls: 1,
                    backend_draws: 0,
                },
            ),
            (
                Caller {
                    view: ViewScope::Alternate,
                    entry: "clear",
                    pc: 100,
                },
                Count {
                    calls: 2,
                    backend_draws: 0,
                },
            ),
        ]);
        let value = summarize(10, Some(&counts));
        assert_eq!(value["present"], 10);
        assert_eq!(value["views"][0]["view"], "Alternate");
        assert_eq!(value["views"][0]["distinct_callers"], 1);
        assert_eq!(value["views"][1]["view"], "Scene(1)");
        assert_eq!(value["views"][1]["backend_draw_attempts"], 3);
        assert_eq!(value["views"][1]["calls"], 5);
        assert_eq!(value["views"][1]["distinct_callers"], 2);
    }
}
