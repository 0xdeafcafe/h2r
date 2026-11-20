//! Clock-only submix processing. The game still smooths obstruction and
//! occlusion and submits both DirectSound calls. Only their inaudible filter
//! coefficients and room gains can be replaced. See docs/perf/sim-budget.md.

use std::sync::Arc;

use crate::{BuildError, Builder, Feature, Manifest, SnapshotPolicy, points};
use runtime::{CpuAbi, ExecCtx, GuestFault, TransferAbi};

const SOUND_GLOBALS: u32 = 0x0051_ebe4;
const SET_I3DL2_SOURCE: u32 = 0x0037_c6e5;
const SET_FILTER: u32 = 0x0037_b68b;

/// Registered by the light host when DirectSound runs on its silent clock.
pub struct SimSubmix;

impl Feature for SimSubmix {
    fn manifest(&self) -> Manifest {
        Manifest::new("sim-submix", "Clock-only sound submix effects")
            .doc("docs/perf/sim-budget.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        b.snapshot(SnapshotPolicy::Stateless);
        if b.switches().implementation("sim.submix-fx", true) {
            b.raw().implement(
                &points::SOUND_SUBMIX_FX,
                "omit silent submix effect math",
                submix,
            );
        }
        Ok(())
    }
}

fn submix(cpu: &mut CpuAbi, ctx: &mut ExecCtx, _: u32) -> Result<TransferAbi, GuestFault> {
    let entry_sp = cpu.esp;
    let saved_indices = (cpu.esi, cpu.edi);
    // Match the guest's stack frame (64 scratch bytes, saved ESI/EDI).
    // The setters get valid stack descriptors even when a stream callback
    // re-enters the sound service on the same guest thread.
    let frame = entry_sp.wrapping_sub(72);
    let rt = Arc::clone(ctx.runtime());
    let mut mem = rt.memory();
    let globals = mem.u32(SOUND_GLOBALS)?;
    let record = globals
        .wrapping_add(0x1a18)
        .wrapping_add(cpu.eax.wrapping_mul(0x3c));
    let handle_at = record.wrapping_add(0x34);
    let handle = mem.u32(handle_at)?;
    let room_desc = frame.wrapping_add(36);
    mem.write(room_desc, [0; 36])?;
    cpu.esi = globals;
    cpu.edi = record;
    cpu.esp = frame.wrapping_sub(12);
    mem.set_u32(cpu.esp, handle)?;
    mem.set_u32(cpu.esp.wrapping_add(4), room_desc)?;
    mem.set_u32(cpu.esp.wrapping_add(8), 1)?; // DS3D_DEFERRED
    ctx.call_guest_named(
        cpu,
        &mut mem,
        SET_I3DL2_SOURCE,
        "sim: submix room",
        move |cpu, ctx| {
            let rt = Arc::clone(ctx.runtime());
            let mut mem = rt.memory();
            // 0x22145a reloads the handle after the first setter. Any pending
            // DirectSound callback ran there and could have changed the record.
            let handle = mem.u32(handle_at)?;
            let filter_desc = frame.wrapping_add(12);
            mem.write(filter_desc, [0; 24])?;
            cpu.esp = frame.wrapping_sub(8);
            mem.set_u32(cpu.esp, handle)?;
            mem.set_u32(cpu.esp.wrapping_add(4), filter_desc)?;
            ctx.call_guest_named(
                cpu,
                &mut mem,
                SET_FILTER,
                "sim: submix filter",
                move |cpu, ctx| {
                    cpu.esp = entry_sp;
                    (cpu.esi, cpu.edi) = saved_indices;
                    // 0x221481: preserve the last setter's HRESULT, ret 4.
                    ctx.memory().ret(cpu, 4)
                },
            )
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Base, Plan, Registry, Switches};
    use runtime::exec::HARNESS_EXIT;
    use runtime::guest::Permissions;
    use runtime::{Dispatch, Event, Guest, Runner, Runtime};

    fn plan(switches: &str) -> Plan {
        let mut registry = Registry::new(Switches::parse(switches).unwrap());
        registry.add(&SimSubmix).unwrap();
        registry.build(&Base).unwrap()
    }

    #[test]
    fn switch_removes_only_the_submix_implementation() {
        assert!(plan("-impl.sim.submix-fx").chains.is_empty());
        for switches in ["", "impl.sim.submix-fx"] {
            let enabled = plan(switches);
            assert_eq!(enabled.chains.len(), 1);
            assert!(
                enabled.chains[&points::SOUND_SUBMIX_FX.base]
                    .implement
                    .is_some()
            );
        }
    }

    #[test]
    fn calls_both_setters_in_order_and_reloads_the_mix_in_after_callbacks() {
        const GLOBALS: u32 = 0x1000;
        const SLOT: u32 = 3;
        const HANDLE_AT: u32 = GLOBALS + 0x1a18 + SLOT * 0x3c + 0x34;
        const ESP: u32 = 0x5000;
        let mut mem = Guest::new();
        mem.map_zeroed(GLOBALS, 0x5000, Permissions::RW).unwrap();
        mem.map_zeroed(SOUND_GLOBALS & !0xfff, 0x1000, Permissions::RW)
            .unwrap();
        for addr in [points::SOUND_SUBMIX_FX.base, SET_I3DL2_SOURCE, SET_FILTER] {
            mem.map_zeroed(addr & !0xfff, 0x1000, Permissions::RX)
                .unwrap();
        }
        mem.write(ESP - 72, [0xff; 72]).unwrap();
        mem.set_u32(SOUND_GLOBALS, GLOBALS).unwrap();
        mem.set_u32(HANDLE_AT, 11).unwrap();
        mem.set_u32(ESP, HARNESS_EXIT).unwrap();
        let mut dispatch = Dispatch::default();
        dispatch.register_override(points::SOUND_SUBMIX_FX.base, "submix", submix);
        dispatch.register_override(SET_I3DL2_SOURCE, "room", |cpu, ctx, _| {
            let mut m = ctx.memory();
            assert_eq!(m.u32(cpu.esp + 4)?, 11);
            let desc = m.u32(cpu.esp + 8)?;
            assert_eq!(m.read::<36>(desc)?, [0; 36]);
            assert_eq!(m.u32(cpu.esp + 12)?, 1);
            m.set_u32(HANDLE_AT, 12)?;
            cpu.eax = 0;
            m.ret(cpu, 12)
        });
        dispatch.register_override(SET_FILTER, "filter", |cpu, ctx, _| {
            let m = ctx.memory();
            assert_eq!(m.u32(cpu.esp + 4)?, 12);
            let desc = m.u32(cpu.esp + 8)?;
            assert_eq!(m.read::<24>(desc)?, [0; 24]);
            cpu.eax = 42;
            m.ret(cpu, 8)
        });
        let mut ctx = ExecCtx::new(Runtime::new(mem, dispatch), 1);
        let mut cpu = CpuAbi {
            esp: ESP,
            eax: SLOT,
            edi: 91,
            esi: 92,
            ..Default::default()
        };
        let transfer = Runner::default().run(&mut cpu, &mut ctx, points::SOUND_SUBMIX_FX.base);
        assert!(matches!(
            ctx.event(transfer.event_id),
            Some(Event::HarnessExit)
        ));
        assert_eq!((cpu.eax, cpu.esp, cpu.edi, cpu.esi), (42, ESP + 8, 91, 92));
    }
}
