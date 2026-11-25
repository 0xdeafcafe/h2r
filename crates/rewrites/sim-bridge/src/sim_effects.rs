//! `impl.sim.native.effects`: effects, particles, contrails and decals,
//! phase 6 of the simulation rewrite (docs/engine/sim-effects.md; design
//! docs/engine/sim-rewrite.md, "Effects (phase 6)"). Default off.
//!
//! Every [`engine::sim_effects::Port`] gets its entry and return sites.
//! At either, with the float environment the port assumes, the port runs
//! on the live guest; anywhere else the original runs (the entry, or the
//! resume at the site). The ports keep no state, so the feature adds no
//! snapshot section.
//!
//! The parity arm is the shadow registry (docs/engine/shadow-exec.md):
//! each port registers under its point's name in the group
//! [`engine::sim_effects::GROUP`], installed or not, and every installed
//! call goes through [`crate::shadow::run`]. `shadow.enable` over the agent
//! API then checks sampled calls against the lifted original while the
//! port stays authoritative, with no switch or variable of its own.
//! [`crate::sim_native::SimNative`] calls [`install`].
use engine::sim_effects::{GROUP, Port, ports};
use hooks::Builder;
use runtime::{CpuAbi, Guest};

/// The switch, without its `impl.` prefix, and the label of the ports'
/// implementations.
pub const EFFECTS: &str = "sim.native.effects";

pub fn on(switches: &hooks::Switches) -> bool {
    switches.implementation(EFFECTS, false)
}

/// Register every port with the shadow registry and, when the switch is
/// on, install it.
pub(crate) fn install(b: &mut Builder) {
    let installed = on(b.switches());
    for port in ports() {
        crate::shadow::register(port.point.name, GROUP, port.point, installed);
        if installed {
            install_port(b, port);
        }
    }
}

fn install_port(b: &mut Builder, port: &'static Port) {
    b.raw()
        .implement(port.point, EFFECTS, move |cpu, ctx, pc| {
            let site = pc != port.point.base && port.sites.contains(&pc);
            if pc == port.point.base || site {
                let mut mem = ctx.memory();
                if (port.supported)(cpu, &mem, pc) {
                    return crate::shadow::run(
                        port.point.name,
                        cpu,
                        &mut mem,
                        pc,
                        &mut |cpu: &mut CpuAbi, mem: &mut Guest| port.invoke(cpu, mem, pc),
                    );
                }
            }
            crate::sim_objects::original(cpu, ctx, pc, site)
        })
        .resumes(port.sites)
        .context_only();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim_native::SimNative;
    use hooks::{Base, Registry, Switches};

    #[test]
    fn the_switch_is_off_by_default_and_installs_every_port_when_on() {
        assert!(!on(&Switches::defaults()));
        assert!(on(&Switches::defaults().with(&format!("impl.{EFFECTS}"), true)));
        // `-impl.sim.native` turns every subsystem off.
        assert!(!on(&Switches::defaults().with("impl.sim.native", false)));

        let mut registry = Registry::new(Switches::defaults());
        registry.add(&SimNative::for_tests()).unwrap();
        let plan = registry.build(&Base).unwrap();
        assert!(plan.chains.is_empty(), "off: nothing installs");

        let mut registry = Registry::new(Switches::defaults().with(&format!("impl.{EFFECTS}"), true));
        registry.add(&SimNative::for_tests()).unwrap();
        let plan = registry.build(&Base).unwrap();
        for port in ports() {
            let implement = plan.chains[&port.point.base].implement.as_ref().unwrap();
            assert_eq!(implement.label, EFFECTS, "{}", port.point.name);
            assert_eq!(implement.continuation_sites().0, port.sites);
        }
        assert!(plan.save_sections().is_empty());
    }

    #[test]
    fn a_plan_registers_every_port_installed_or_not() {
        let mut registry = Registry::new(Switches::defaults());
        registry.add(&SimNative::for_tests()).unwrap();
        registry.build(&Base).unwrap();
        let status = crate::shadow::SHADOW.status();
        for port in ports() {
            let found = status.ports.iter().find(|p| p.name == port.point.name);
            assert_eq!(found.map(|p| p.group), Some(GROUP), "{}", port.point.name);
        }
    }
}
