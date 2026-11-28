//! `impl.sim.lightmap-store`: the lightmap store answers the sampler
//! `0x000d1e10`, and the tick's render-state record reads find a sample
//! of their own (`engine::sim_lightmap`; docs/engine/sim-render-split.md,
//! "6. Render scratch and stamps", the effect tint).
//!
//! Two implements and two observers:
//!
//! - `0x000d1e10`, every caller: the Rust port over the store. A call the
//!   store cannot answer (no reader, no structure, a load that failed)
//!   runs the guest's body; the gap is named once and counted.
//! - `0x0003ddd0`, every caller: the Rust port
//!   (`engine::sim_lightmap::record_call`). Called from the tick's two
//!   reads (`0x001796e2`, `0x0017b61f`), the port of `0x000d1850`
//!   (`engine::sim_lightmap::tick_sample`) then samples the object into
//!   the tick's record (`engine::sim_lightmap::Fill`) before the call
//!   returns. Neither path runs guest code.
//! - map load and structure switch drop the store.
use guest_layout::points;
use engine::sim_lightmap::{self as lightmap, Fill, Site};
use hooks::{BuildError, Builder, Feature, Manifest, SnapshotPolicy};
use runtime::{CpuAbi, ExecCtx, GuestFault, TransferAbi};
use std::sync::Arc;

/// The switch. On: the tick's effect tint is the simulation's.
pub const LIGHTMAP_STORE: &str = "sim.lightmap-store";

pub struct LightmapStore;

impl Feature for LightmapStore {
    fn manifest(&self) -> Manifest {
        Manifest::new("sim-lightmap-store", "The simulation's lightmap sample, from a store of the map's lightmap")
            .doc("docs/engine/sim-render-split.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        if !b.switches().implementation(LIGHTMAP_STORE, true) {
            b.snapshot(SnapshotPolicy::Stateless);
            return Ok(());
        }
        install_sample(b);
        install_record(b);
        for (point, label) in INVALIDATE {
            b.raw().observe_before(point, label, |_| lightmap::invalidate());
        }
        // The store is derived from the map file and the tags: a loaded
        // snapshot only drops it.
        b.snapshot(SnapshotPolicy::section(Vec::new, |_, _| {
            lightmap::invalidate();
            Ok(())
        }));
        Ok(())
    }
}

const INVALIDATE: [(&points::Point, &str); 2] = [
    (&points::CACHE_TAGS_LOAD, "lightmap-invalidate-map"),
    (&points::BSP_SWITCH, "lightmap-invalidate-bsp"),
];

/// The original at `addr`, for calls the store declines.
fn original(cpu: &mut CpuAbi, ctx: &mut ExecCtx, addr: u32) -> Result<TransferAbi, GuestFault> {
    if let Some(original) = ctx.runtime().resolve_original(addr) {
        return Ok(original.invoke(cpu, ctx, addr));
    }
    let event = ctx.emit(runtime::Event::UnsupportedTarget {
        pc: cpu.eip,
        target: addr,
        kind: runtime::abi::JUMP,
        route: runtime::abi::ENTRY,
    });
    Ok(TransferAbi::fault(event))
}

fn install_sample(b: &mut Builder) {
    let point = &points::LIGHTMAP_SAMPLE;
    b.raw()
        .implement(point, LIGHTMAP_STORE, move |cpu, ctx, addr| {
            if addr == point.base {
                let rt = Arc::clone(ctx.runtime());
                let mut mem = rt.memory();
                match lightmap::ensure(&mem) {
                    Ok(store) => return lightmap::answer(cpu, &mut mem, &store),
                    Err(gap) => lightmap::gap(&gap),
                }
            }
            original(cpu, ctx, addr)
        })
        .context_only()
        .optional();
}

fn install_record(b: &mut Builder) {
    let point = &points::RENDER_STATE_RECORD;
    b.raw()
        .implement(point, LIGHTMAP_STORE, move |cpu, ctx, addr| {
            if addr != point.base {
                unimplemented!("lightmap: the record get entered at {addr:#010x}");
            }
            let rt = Arc::clone(ctx.runtime());
            let mut mem = rt.memory();
            let entry = cpu.esp;
            let ret = lightmap::record_call(cpu, &mut *mem)?;
            let Some(site) = Site::of(ret) else {
                return Ok(TransferAbi::ret(ret));
            };
            let object = site.object(cpu);
            if let Some(fill) = Fill::new(&*mem, object, cpu.eax)? {
                lightmap::tick_sample(cpu, &mut *mem, entry, fill)?;
            }
            Ok(TransferAbi::ret(ret))
        })
        .context_only()
        .optional();
}
