//! The per-effect bodies of the effects phase in Rust
//! (docs/engine/effects-parts.md, "The per-effect bodies"): the alive
//! test `1794e0`, the cull `179730` and the delete `1774f0` the effect
//! walk calls, counted by `engine::effect_update::arm`.
//!
//! Each body's entry and sites get an implementation that runs the Rust
//! segment and fuses into the next body (`1794e0` into `1774f0`)
//! ([`segment`]). No segment is the guest's: one entered outside the
//! bodies' float environment is counted (`float_env`) and still answered.
//!
//! The effect port of `1792b0` (`engine::sim_effects`, always installed)
//! keeps that point, so the update body is left out ([`bodies`]).
//!
//! Hooks this adds, and their order: the implementation of each body's
//! point (`EFFECT_ALIVE`, `EFFECT_CULLED`, `EFFECT_DELETE`), guarded when
//! another feature hooks it too; and, last, an `observe_before` on
//! `CACHE_TAGS_LOAD` that clears the counters.

use crate::sim_objects::original;
use engine::effect_update::{TABLE, arm};
use engine::points;
use engine::sim_objects::{self as objects, Body, Table};
use hooks::Builder;
use runtime::{CpuAbi, ExecCtx, GuestFault, TransferAbi};

/// The label of the bodies' implementations.
pub const LABEL: &str = "effect bodies";
/// The map-load observer that clears the counters.
pub const RESET: &str = "effect bodies: reset the counters";

/// The bodies to install: all but the update, which the effect port owns.
pub fn bodies() -> impl Iterator<Item = &'static Body> {
    TABLE.bodies().iter().copied().filter(|b| b.point.base != points::EFFECT_UPDATE.base)
}

pub(crate) fn install(b: &mut Builder) {
    let table: &'static Table = &TABLE;
    // No body calls the update, so leaving it out needs no guard.
    for body in bodies() {
        let point = body.point;
        b.raw()
            .implement(point, LABEL, move |cpu, ctx, pc| {
                let site = pc != point.base && body.sites.contains(&pc);
                if pc == point.base || site {
                    return segment(cpu, ctx, pc);
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
}

/// One segment of a body at `pc`, answered by the Rust and fused into
/// every body it reaches. A float environment off the bodies' own is
/// counted and still answered.
pub(crate) fn segment(cpu: &mut CpuAbi, ctx: &mut ExecCtx, pc: u32) -> Result<TransferAbi, GuestFault> {
    let table: &'static Table = &TABLE;
    if !table.supports(cpu) {
        arm::float_env(pc);
    }
    let mut mem = ctx.memory();
    objects::drive_admitting(cpu, &mut mem, table, pc, arm::admit, arm::answer_segment)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim_effects_parts::SimEffectsParts;
    use hooks::{Base, Registry, Switches};

    #[test]
    fn the_feature_installs_every_body_but_the_update_with_its_sites_and_the_reset() {
        let mut registry = Registry::new(Switches::defaults());
        registry.add(&SimEffectsParts).unwrap();
        let plan = registry.build(&Base).unwrap();
        for body in bodies() {
            let implement = plan.chains[&body.point.base].implement.as_ref().unwrap();
            assert_eq!(implement.label, LABEL, "{}", body.point.name);
            assert_eq!(implement.continuation_sites().0, body.sites);
        }
        let load = &plan.chains[&points::CACHE_TAGS_LOAD.base];
        assert!(load.observe_before.iter().any(|h| h.label == RESET));
    }

    #[test]
    fn the_effect_port_keeps_the_update() {
        let kept: Vec<u32> = bodies().map(|b| b.point.base).collect();
        assert_eq!(
            kept,
            [points::EFFECT_ALIVE.base, points::EFFECT_CULLED.base, points::EFFECT_DELETE.base]
        );
    }
}
