//! The per-effect bodies of the effects phase in Rust
//! (docs/engine/effects-parts.md, "The per-effect bodies"): the alive
//! test `1794e0`, the cull `179730`, the delete `1774f0` and the event
//! clock `1792b0` the effect walk calls, through their arm
//! (`engine::effect_update::arm`).
//!
//! Each body's entry and sites get an implementation that gives the
//! segment its turn ([`segment`]): the Rust body answers and fuses into
//! the next body whose turn is an answer too (`1794e0` into `1774f0`), or
//! the guest answers and the Rust segment, predicted on the state it
//! started from, is compared at the guest's transfer.
//!
//! `impl.sim.native.effects` ports `1792b0` too (`engine::sim_effects`,
//! default off); with it on, that port keeps the point and the update
//! body is left out ([`bodies`]).
//!
//! Hooks this adds, and their order: the implementation of each body's
//! point (`EFFECT_ALIVE`, `EFFECT_CULLED`, `EFFECT_DELETE`,
//! `EFFECT_UPDATE`), guarded when another feature hooks it too; and,
//! last, an `observe_before` on `CACHE_TAGS_LOAD` (the proof is the
//! map's) and on `GAME_STATE_REVERT` and `GAME_STATE_RESTORE` (the audit
//! slots start over).

use crate::sim_objects::original;
use engine::effect_update::{TABLE, arm};
use engine::points;
use engine::sim_objects::{self as objects, Body, Table};
use hooks::Builder;
use runtime::{CpuAbi, ExecCtx, GuestFault, TransferAbi};

/// The label of the bodies' implementations.
pub const LABEL: &str = "effect bodies";
/// The map-load observer that clears the arm's proof and fallback.
pub const RESET: &str = "effect bodies: reset the arm";
/// The revert and restore observers that start its audit slots over.
pub const NEW_RUN: [(&points::Point, &str); 2] = [
    (&points::GAME_STATE_REVERT, "effect bodies: slots at a revert"),
    (&points::GAME_STATE_RESTORE, "effect bodies: slots at a restore"),
];

/// The bodies to install: all four, less the update when
/// `impl.sim.native.effects` ports it.
pub fn bodies(switches: &hooks::Switches) -> impl Iterator<Item = &'static Body> {
    let native = crate::sim_effects::on(switches);
    TABLE.bodies().iter().copied().filter(move |b| !(native && b.point.base == points::EFFECT_UPDATE.base))
}

pub(crate) fn install(b: &mut Builder) {
    let table: &'static Table = &TABLE;
    // No body calls the update, so leaving it out needs no guard.
    let bodies: Vec<&'static Body> = bodies(b.switches()).collect();
    for body in bodies {
        let point = body.point;
        b.raw()
            .implement(point, LABEL, move |cpu, ctx, pc| {
                let site = pc != point.base && body.sites.contains(&pc);
                if pc == point.base || site {
                    return segment(cpu, ctx, pc, site);
                }
                original(cpu, ctx, pc, site)
            })
            .resumes(body.sites)
            .context_only()
            .when_shared(move |entry| {
                table.guard(entry);
            });
    }
    // Observers reset process counters only, so they sit last.
    b.raw().observe_before(&points::CACHE_TAGS_LOAD, RESET, |_| arm::reset()).last();
    for (point, label) in NEW_RUN {
        b.raw().observe_before(point, label, |_| arm::new_run()).last();
    }
}

/// One segment of a body at `pc`: the Rust body answers on its turn;
/// otherwise the guest answers and the prediction is compared. An
/// unsupported float environment is the guest's, counted as declined.
pub(crate) fn segment(cpu: &mut CpuAbi, ctx: &mut ExecCtx, pc: u32, site: bool) -> Result<TransferAbi, GuestFault> {
    let table: &'static Table = &TABLE;
    if !table.supports(cpu) {
        arm::declined(pc);
        return original(cpu, ctx, pc, site);
    }
    if arm::turn(pc) == Some(arm::Turn::Answer) {
        let mut mem = ctx.memory();
        return objects::drive_admitting(cpu, &mut mem, table, pc, arm::admit, arm::answer_segment);
    }
    let prediction = {
        let mut mem = ctx.memory();
        arm::predict(cpu, &mut mem, pc)
    };
    // A direct-call image can run a callee inside the caller's handler:
    // stop the original at its next transfer, as the prediction stops.
    let guest = ctx.with_routed_calls(|ctx| original(cpu, ctx, pc, site));
    if let Some(prediction) = prediction {
        match &guest {
            Ok(transfer) => prediction.compare(cpu, &ctx.memory(), *transfer),
            Err(_) => prediction.guest_error(),
        }
    }
    guest
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim_effects_parts::SimEffectsParts;
    use hooks::{Base, Registry, Switches};

    #[test]
    fn the_feature_installs_every_body_with_its_sites_and_the_resets() {
        let mut registry = Registry::new(Switches::defaults());
        registry.add(&SimEffectsParts).unwrap();
        let plan = registry.build(&Base).unwrap();
        for body in TABLE.bodies() {
            let implement = plan.chains[&body.point.base].implement.as_ref().unwrap();
            assert_eq!(implement.label, LABEL, "{}", body.point.name);
            assert_eq!(implement.continuation_sites().0, body.sites);
        }
        let load = &plan.chains[&points::CACHE_TAGS_LOAD.base];
        assert!(load.observe_before.iter().any(|h| h.label == RESET));
        for (point, label) in NEW_RUN {
            assert!(plan.chains[&point.base].observe_before.iter().any(|h| h.label == label), "{label}");
        }
    }

    #[test]
    fn the_native_effects_port_keeps_the_update() {
        let native = Switches::defaults().with(&format!("impl.{}", crate::sim_effects::EFFECTS), true);
        let kept: Vec<u32> = bodies(&native).map(|b| b.point.base).collect();
        assert_eq!(
            kept,
            [points::EFFECT_ALIVE.base, points::EFFECT_CULLED.base, points::EFFECT_DELETE.base]
        );
        assert_eq!(bodies(&Switches::defaults()).count(), 4);
    }
}
