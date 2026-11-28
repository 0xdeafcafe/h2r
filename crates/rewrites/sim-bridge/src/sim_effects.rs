//! Effects, particles, contrails and decals, phase 6 of the simulation
//! rewrite (docs/engine/sim-effects.md; design docs/engine/sim-rewrite.md,
//! "Effects (phase 6)"). Always installed, with no switch.
//!
//! Every [`engine::sim_effects::Port`] gets its entry and return sites,
//! and the Rust answers each of them. A call outside what the port's
//! `supported` check describes (a float environment off the one the sim
//! runs in, an armed memory trap, a chain or emission loop past the
//! port's bound) is still the port's: it is counted by port name
//! ([`outside`]) and run. No call goes to the original. The ports keep no
//! state, so the feature adds no snapshot section.
//!
//! The parity arm is the shadow registry (docs/engine/shadow-exec.md):
//! each port registers under its point's name in the group
//! [`engine::sim_effects::GROUP`], and every call goes through
//! [`crate::shadow::run`]. `shadow.enable` over the agent API then checks
//! sampled calls against the lifted original while the port stays
//! authoritative. [`crate::sim_native::SimNative`] calls [`install`].
use engine::sim_effects::{GROUP, Port, ports};
use hooks::Builder;
use runtime::{CpuAbi, Guest};
use std::collections::BTreeMap;
use std::sync::Mutex;

/// The label of the ports' implementations.
pub const EFFECTS: &str = "sim.native.effects";

/// Calls each port ran outside its `supported` check, by point name.
static OUTSIDE: Mutex<BTreeMap<&'static str, u64>> = Mutex::new(BTreeMap::new());

/// Every port's count of calls run outside its `supported` check, in
/// name order.
pub fn outside() -> Vec<(&'static str, u64)> {
    OUTSIDE.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(&k, &v)| (k, v)).collect()
}

fn count_outside(name: &'static str) {
    *OUTSIDE.lock().unwrap_or_else(|e| e.into_inner()).entry(name).or_default() += 1;
}

/// Register and install every port.
pub(crate) fn install(b: &mut Builder) {
    for port in ports() {
        crate::shadow::register(port.point.name, GROUP, port.point, true);
        install_port(b, port);
    }
}

fn install_port(b: &mut Builder, port: &'static Port) {
    b.raw()
        .implement(port.point, EFFECTS, move |cpu, ctx, pc| {
            let site = pc != port.point.base && port.sites.contains(&pc);
            if pc != port.point.base && !site {
                return crate::sim_objects::original(cpu, ctx, pc, site);
            }
            let mut mem = ctx.memory();
            if !(port.supported)(cpu, &mem, pc) {
                count_outside(port.point.name);
            }
            crate::shadow::run(
                port.point.name,
                cpu,
                &mut mem,
                pc,
                &mut |cpu: &mut CpuAbi, mem: &mut Guest| port.invoke(cpu, mem, pc),
            )
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
    fn every_port_installs_with_its_sites() {
        let mut registry = Registry::new(Switches::defaults());
        registry.add(&SimNative::for_tests()).unwrap();
        let plan = registry.build(&Base).unwrap();
        for port in ports() {
            let implement = plan.chains[&port.point.base].implement.as_ref().unwrap();
            assert_eq!(implement.label, EFFECTS, "{}", port.point.name);
            assert_eq!(implement.continuation_sites().0, port.sites);
        }
        // The manager keeps the "sim-native" section; it holds no bytes.
        assert!(plan.save_sections().iter().all(|(_, bytes)| bytes.is_empty()));
        // The old switch no longer parses.
        assert!(Switches::parse("impl.sim.native.effects").is_err());
    }

    #[test]
    fn a_plan_registers_every_port_installed() {
        let mut registry = Registry::new(Switches::defaults());
        registry.add(&SimNative::for_tests()).unwrap();
        registry.build(&Base).unwrap();
        let status = crate::shadow::SHADOW.status();
        for port in ports() {
            let found = status.ports.iter().find(|p| p.name == port.point.name);
            assert_eq!(found.map(|p| p.group), Some(GROUP), "{}", port.point.name);
        }
    }

    #[test]
    fn a_call_outside_the_check_is_counted_and_answered_by_the_port() {
        let mut registry = Registry::new(Switches::defaults());
        registry.add(&SimNative::for_tests()).unwrap();
        let plan = registry.build(&Base).unwrap();
        // An unmasked MXCSR is outside the float checks; the empty guest
        // maps nothing, so the port faults rather than handing the call
        // to the original.
        let mut cpu = CpuAbi { mxcsr: 0, ..CpuAbi::default() };
        let empty = Guest::new();
        let port = ports().find(|p| !(p.supported)(&cpu, &empty, p.point.base)).unwrap();
        let rt = runtime::Runtime::new(Guest::new(), runtime::Dispatch::default());
        let mut ctx = runtime::ExecCtx::new(rt, 1);
        let before = outside().iter().find(|(n, _)| *n == port.point.name).map_or(0, |e| e.1);
        let hook = plan.chains[&port.point.base].implement.as_ref().unwrap();
        let hooks::Run::Implement(run) = &hook.run else { unreachable!() };
        let _ = run(&mut cpu, &mut ctx, port.point.base);
        let after = outside().iter().find(|(n, _)| *n == port.point.name).map_or(0, |e| e.1);
        assert_eq!(after, before + 1, "{}", port.point.name);
    }
}
