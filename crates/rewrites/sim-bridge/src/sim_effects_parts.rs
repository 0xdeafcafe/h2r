//! `impl.sim.effects-parts`: the effect part split of docs/engine/effects-parts.md.
//!
//! A `before` hook on the part creation switch (`0x17a8a0`, `EFFECT_PART_CREATE`)
//! decodes the part and hands it to [`sim_effects::EffectsSim`]. Damage parts
//! extend the damage chain the sim hash covers; decals, lights and lens flares
//! queue a [`sim_effects::VisualEvent`] for the renderer. The parts retail
//! creates nothing for (`lens`, `MGS2`) return without the guest call; every
//! other part runs retail's creation, so the shared random stream and the
//! sim state keep retail order. The hook makes no guest call.

use std::sync::{Arc, Mutex, MutexGuard};

use engine::points;
use hooks::{BuildError, Builder, Feature, Flow, Manifest, SnapshotPolicy};
use sim_effects::{Disposition, EffectsSim, Part};

/// The switch (`impl.sim.effects-parts`, default off until `h2 sim diff` has
/// the answer for it).
pub const PARTS: &str = "sim.effects-parts";
const ID: &str = "impl.sim.effects-parts";

fn lock(m: &Mutex<EffectsSim>) -> MutexGuard<'_, EffectsSim> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The part switch's arguments: the effect datum (`[esp+4]`) and the part
/// record (`[esp+8]`); `ret 0x14`.
fn decode(g: &runtime::Guest, esp: u32) -> Result<(Part, u32), runtime::GuestFault> {
    let effect = g.u32(esp.wrapping_add(4))?;
    let part = g.u32(esp.wrapping_add(8))?;
    let tick = g.u32(g.u32(points::GAME_TIME.base)?.wrapping_add(8))?;
    Ok((
        Part {
            tick,
            effect: g.u32(effect.wrapping_add(4))?,
            group: g.u32(part.wrapping_add(8))?,
            tag: g.u32(part.wrapping_add(0x10))?,
            location: g.u16(part.wrapping_add(4))? as i16,
            flags: g.u16(part.wrapping_add(6))?,
        },
        part,
    ))
}

pub struct SimEffectsParts {
    sim: Arc<Mutex<EffectsSim>>,
}

impl SimEffectsParts {
    pub fn new() -> Self {
        SimEffectsParts { sim: Arc::new(Mutex::new(EffectsSim::new())) }
    }

    /// The split, for the renderer to drain visual events from and for the
    /// sim report to read the hash of.
    pub fn sim(&self) -> Arc<Mutex<EffectsSim>> {
        Arc::clone(&self.sim)
    }
}

impl Default for SimEffectsParts {
    fn default() -> Self {
        Self::new()
    }
}

impl Feature for SimEffectsParts {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "Effect parts: damage as sim state, visual events for the renderer")
            .doc("docs/engine/effects-parts.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        let save = Arc::clone(&self.sim);
        let load = Arc::clone(&self.sim);
        b.snapshot(SnapshotPolicy::section(
            move || lock(&save).save(),
            move |bytes, _| lock(&load).load(bytes),
        ));
        if !b.switches().implementation(PARTS, false) {
            return Ok(());
        }
        let sim = Arc::clone(&self.sim);
        b.raw().before(&points::EFFECT_PART_CREATE, "effect part split", move |c| {
            let decoded = {
                let mem = c.ctx.runtime().memory();
                decode(&mem, c.cpu.esp)
            };
            // A part the record cannot be read for is left to retail.
            let Ok((part, _)) = decoded else { return Ok(Flow::Continue) };
            match lock(&sim).part(part) {
                Disposition::RunOriginal => Ok(Flow::Continue),
                Disposition::Skip => {
                    // Retail loads the group into EAX before it returns.
                    c.cpu.eax = part.group;
                    c.ret(0x14).map(Flow::Return)
                }
            }
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hooks::{Base, Registry, Switches};

    #[test]
    fn the_switch_is_off_by_default_and_installs_the_split_when_on() {
        for (spec, on) in [("", false), ("impl.sim.effects-parts", true), ("-impl.sim.effects-parts", false)] {
            let mut registry = Registry::new(Switches::parse(spec).unwrap());
            registry.add(&SimEffectsParts::new()).unwrap();
            let plan = registry.build(&Base).unwrap();
            assert!(!plan.save_sections().is_empty(), "the damage chain is saved");
            assert_eq!(plan.chains.contains_key(&points::EFFECT_PART_CREATE.base), on, "{spec:?}");
        }
    }
}
