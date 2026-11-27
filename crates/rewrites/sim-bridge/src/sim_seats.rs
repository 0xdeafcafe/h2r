//! `impl.sim.native.seats`: seats and rules, phase 7 of the simulation
//! rewrite (docs/engine/sim-rewrite.md, section 4 "Seats and rules").
//! Default off.
//!
//! Every entry in `engine::sim_seats::SEATS` gets its point and its
//! return sites. At either, the body runs one segment; a decline (or a
//! transfer anywhere else) falls back to the original at the entry, or
//! its resume at a site, exactly as the per-function ports do. It retires
//! `sim.native_observer_camera`: the camera is one of the entries. The
//! whole prediction walk is another, which takes over from
//! `sim.observer-resources` (`crate::sim_observer`) while the seats are
//! on; that trim stays the default until the seats are promoted.
//!
//! **Parity arm, then authority.** The seats install as the shadow's
//! guest-authoritative arm (docs/engine/shadow-exec.md): while the agent
//! API's `shadow.enable` has checks on for a seat — directly or by the
//! `sim.seats` group — the original answers on the live state and the
//! body is checked against it on a copy. Turning the checks off makes
//! the bodies authoritative. No switch separates the two; the shadow is
//! the switch.
//!
//! [`crate::sim_native::SimNative`] calls [`install`]: the bodies hold no
//! state, so the feature's snapshot policy does not change. The bound
//! controller's device read runs through its kernel arm
//! (`engine::sim_seats::device::arm`): the guest answers until the
//! kernel's predictions have matched its clean count, then the kernel,
//! with one read in 64 still the guest's and compared. The arm's proof
//! resets at map load ([`DEVICE_RESET`]). The phantom seats' action
//! builder stays in the generated code (the bodies decline it), so hosts
//! and phantom rosters are unchanged with the switch on.
//!
//! The seats a host runs for phantoms keep working because the unbound
//! path — the entry that zeroes the action and collects the script
//! input — is the body itself, and every other path it does not take it
//! declines to the original.

use hooks::Builder;
use engine::sim_seats;
use runtime::{CpuAbi, GuestFault};

/// The switch, without its `impl.` prefix.
pub const SEATS: &str = "sim.native.seats";
/// The shadow registry's group for the seats.
pub const GROUP: &str = "sim.seats";
/// The device arm's reset at map load.
pub const DEVICE_RESET: &str = "seats: reset the device arm";

pub fn on(switches: &hooks::Switches) -> bool {
    switches.implementation(SEATS, false)
}

/// Whether the per-function port at `addr` stands down: the switch is on
/// and a seat owns the point.
pub fn owns(switches: &hooks::Switches, addr: u32) -> bool {
    on(switches) && sim_seats::SEATS.iter().any(|seat| seat.point.base == addr)
}

pub(crate) fn install(b: &mut Builder) {
    if on(b.switches()) {
        b.raw().observe_before(&engine::points::CACHE_TAGS_LOAD, DEVICE_RESET, |_| {
            sim_seats::device::arm::reset()
        });
    }
    for seat in sim_seats::SEATS {
        let installed = on(b.switches());
        crate::shadow::register(seat.name, GROUP, seat.point, installed);
        if !installed {
            continue;
        }
        let point = seat.point;
        let sites = seat.sites;
        let name = seat.name;
        let run = seat.run;
        b.raw()
            .implement(point, SEATS, move |cpu, ctx, pc| {
                let site = pc != point.base && sites.contains(&pc);
                if pc == point.base || site {
                    let mut mem = ctx.memory();
                    // The parity arm: with this seat's checks on, the
                    // original answers and the body runs on a copy.
                    let checked = crate::shadow::SHADOW.check_before_original(
                        name,
                        &*cpu,
                        &mem,
                        pc,
                        crate::shadow::game_tick,
                        &mut |cpu: &mut CpuAbi, mem: &mut runtime::Guest| {
                            match run(cpu, mem, pc) {
                                Ok(Some(transfer)) => Ok(transfer),
                                // A decline is not comparable, and neither
                                // is a fault: either way this call is
                                // skipped, and the original answers.
                                Ok(None) => Err(declined(pc)),
                                Err(fault) => Err(fault),
                            }
                        },
                    );
                    if !checked {
                        if let Some(transfer) = run(cpu, &mut mem, pc)? {
                            return Ok(transfer);
                        }
                    }
                }
                crate::sim_objects::original(cpu, ctx, pc, site)
            })
            .resumes(sites)
            .context_only()
            // The light-host trim hooks the prediction point too; it
            // stands down while the seats are on, and this orders the two
            // should a plan ever hold both.
            .after_feature("sim-observer-resources");
    }
}

fn declined(pc: u32) -> GuestFault {
    GuestFault {
        addr: pc,
        len: 0,
        access: runtime::guest::Access::Execute,
        reason: runtime::guest::FaultReason::InvalidOperation,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim_native::SimNative;
    use hooks::{Base, Registry, Switches};

    #[test]
    fn the_switch_is_off_by_default_and_installs_every_seat_when_on() {
        assert!(!on(&Switches::defaults()));
        assert!(on(&Switches::defaults().with(&format!("impl.{SEATS}"), true)));
        // `-impl.sim.native` turns every subsystem off.
        assert!(!on(&Switches::defaults().with("impl.sim.native", false)));
        let mut registry = Registry::new(Switches::defaults());
        registry.add(&SimNative::for_tests()).unwrap();
        let plan = registry.build(&Base).unwrap();
        for seat in sim_seats::SEATS {
            assert!(
                !plan.chains.contains_key(&seat.point.base),
                "{}: off installs nothing",
                seat.name
            );
        }

        let mut registry = Registry::new(Switches::defaults().with(&format!("impl.{SEATS}"), true));
        registry.add(&SimNative::for_tests()).unwrap();
        let plan = registry.build(&Base).unwrap();
        for seat in sim_seats::SEATS {
            let implement = plan.chains[&seat.point.base].implement.as_ref().unwrap();
            assert_eq!(implement.label, SEATS, "{}", seat.name);
            assert_eq!(implement.continuation_sites().0, seat.sites, "{}", seat.name);
        }
    }

    #[test]
    fn the_device_arm_resets_at_map_load_only_while_the_seats_are_on() {
        let resets = |switches: Switches| {
            let mut registry = Registry::new(switches);
            registry.add(&SimNative::for_tests()).unwrap();
            let plan = registry.build(&Base).unwrap();
            let load = &plan.chains[&engine::points::CACHE_TAGS_LOAD.base];
            load.observe_before.iter().any(|h| h.label == DEVICE_RESET)
        };
        assert!(!resets(Switches::defaults()));
        assert!(resets(Switches::defaults().with(&format!("impl.{SEATS}"), true)));
    }

    #[test]
    fn every_seat_name_is_registered_with_the_shadow_group() {
        let mut registry = Registry::new(Switches::defaults());
        registry.add(&SimNative::for_tests()).unwrap();
        registry.build(&Base).unwrap();
        let status = crate::shadow::SHADOW.status();
        for seat in sim_seats::SEATS {
            let port = status.ports.iter().find(|p| p.name == seat.name);
            assert!(port.is_some(), "{} is not registered", seat.name);
            assert_eq!(port.unwrap().group, GROUP, "{}", seat.name);
        }
    }

    #[test]
    fn the_seats_own_their_points_only_while_the_switch_is_on() {
        let on = Switches::defaults().with(&format!("impl.{SEATS}"), true);
        let base = sim_seats::SEATS[0].point.base;
        assert!(owns(&on, base));
        assert!(!owns(&Switches::defaults(), base));
        assert!(!owns(&on, base + 4));
    }

    #[test]
    fn the_retired_observer_camera_switch_is_gone_and_the_trim_stays() {
        assert!(hooks::switches::table::get("impl.sim.native_observer_camera").is_none());
        assert!(hooks::switches::table::get("impl.sim.observer-resources").is_some());
        assert!(hooks::switches::table::get("impl.sim.native.seats").is_some());
    }
}
