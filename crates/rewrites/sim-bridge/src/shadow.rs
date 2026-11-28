//! Shadow checks for the native ports (docs/engine/shadow-exec.md): the
//! glue between the port installers ([`crate::sim_native`],
//! [`crate::sim_physics`]) and [`lifted::shadow`].
//!
//! Every port is registered with the process's registry at plan build,
//! installed or not, under its name and group. Authoritative bodies use
//! [`run`]. Guest-authoritative arms use [`compare_only`] before running
//! generated code. Both check the registry's atomic flag before doing work.

pub use lifted::shadow::{Enabled, Error, Mismatch, Port, PortFn, SHADOW, Shadow, Status};

use guest_layout::points;
use runtime::{CpuAbi, Guest, GuestFault, TransferAbi};

/// The group of [`crate::sim_native`]'s entries.
pub const SIM_NATIVE: &str = "sim.native";
/// The group of [`crate::sim_physics`]'s ports.
pub const SIM_PHYSICS: &str = "sim.physics";

/// The game's tick: `[GAME_TIME] + 8`, when that reads.
pub fn game_tick(g: &Guest) -> Option<u64> {
    let read = |a: u32| {
        let mut b = [0; 4];
        g.read_bytes(a, &mut b, 1)
            .ok()
            .map(|()| u32::from_le_bytes(b))
    };
    let time = read(points::GAME_TIME.base).filter(|&p| p != 0)?;
    read(time.wrapping_add(8)).map(u64::from)
}

/// Register a port with the process's registry.
pub fn register(
    name: &'static str,
    group: &'static str,
    point: &'static points::Point,
    installed: bool,
) {
    SHADOW.register(name, group, point.name, point.base, installed);
}

/// Run an installed port body through the process's registry.
pub fn run(
    name: &'static str,
    cpu: &mut CpuAbi,
    guest: &mut Guest,
    pc: u32,
    port: &mut PortFn<'_>,
) -> Result<TransferAbi, GuestFault> {
    SHADOW.run(name, cpu, guest, pc, game_tick, port)
}

/// Check a candidate on private state before dispatching the live guest.
pub fn compare_only(
    name: &'static str,
    cpu: &CpuAbi,
    guest: &Guest,
    pc: u32,
    port: &mut PortFn<'_>,
) {
    SHADOW.compare_only(name, cpu, guest, pc, game_tick, port);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every_port() -> impl Iterator<Item = &'static str> {
        let native = crate::sim_native::ENTRIES.iter().map(|&(_, name, _)| name);
        native
            .chain(engine::sim_physics::PORTS.iter().map(|p| p.name))
            .chain(crate::sim_objects::SHADOW_POINTS.iter().map(|p| p.name))
    }

    #[test]
    fn a_plan_registers_every_port_installed_or_not() {
        let mut registry = hooks::Registry::new(hooks::Switches::defaults());
        registry
            .add(&crate::sim_native::SimNative::for_tests())
            .unwrap();
        registry.add(&crate::sim_physics::NativePhysics).unwrap();
        registry.build(&hooks::Base).unwrap();
        let status = SHADOW.status();
        for name in every_port() {
            let port = status.ports.iter().find(|p| p.name == name);
            assert!(port.is_some(), "{name} is not registered");
        }
        let pose = status
            .ports
            .iter()
            .find(|p| p.name == "sim.port.unit_pose")
            .unwrap();
        assert_eq!(pose.group, SIM_NATIVE);
    }

    /// The disc gate accepts each port by its registered name.
    #[test]
    fn the_disc_gate_names_every_port() {
        let script = include_str!("../../../../scripts/shadow-gate.sh");
        for name in every_port() {
            assert!(
                script.contains(&format!(" {name} ")) || script.contains(&format!(" {name}\n")),
                "scripts/shadow-gate.sh does not list {name}"
            );
        }
    }
}
