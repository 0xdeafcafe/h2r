//! Seats and rules, phase 7 of the simulation rewrite
//! (docs/engine/sim-rewrite.md, section 4 "Seats and rules"). Always
//! installed: there is no switch and no guest arm.
//!
//! Every entry in `engine::sim_seats::SEATS` gets its point and its
//! return sites. At either, the body runs one segment. A body that
//! declines is on a path not ported yet (a float environment it does not
//! assume, an aliased or unmapped operand, a resource type an edited
//! image can name): that is a named, counted error ([`stats`],
//! `seats.<name>` in `sim.arms`, logged the first time each map), and the
//! lifted body runs that one segment. A transfer anywhere else is the
//! original's by construction (a site the body does not own). The camera
//! is one of the entries; the whole prediction walk is another, which
//! owns the point the light host's resource trim used to hook.
//!
//! [`crate::sim_native::SimNative`] calls [`install`]: the bodies hold no
//! state, so the feature's snapshot policy does not change. The bound
//! controller's device read runs through its kernel arm
//! (`engine::sim_seats::device::arm`), and the bodies' leaf callees
//! through theirs (`engine::sim_seats::leaf`); both reset at map load
//! ([`DEVICE_RESET`]). The phantom seats' action builder stays in the
//! generated code (the bodies decline it); the phantom executor runs it
//! in Rust.

use hooks::Builder;
use engine::sim_seats;
use std::sync::atomic::{AtomicU64, Ordering};

/// The label every seat installs under.
pub const SEATS: &str = "sim.native.seats";
/// The device and leaf arms' reset at map load, with the seats' counters.
pub const DEVICE_RESET: &str = "seats: reset the device and leaf arms";

/// Whether a seat owns the point at `addr`: the per-function port there
/// stands down.
pub fn owns(addr: u32) -> bool {
    sim_seats::SEATS.iter().any(|seat| seat.point.base == addr)
}

const N: usize = sim_seats::SEATS.len();
static ANSWERED: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];
static DECLINED: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];

/// One seat's counters since the map loaded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stats {
    pub name: &'static str,
    /// Segments the body ran.
    pub answered: u64,
    /// Segments on an unported path, which the lifted body ran.
    pub declined: u64,
}

pub fn stats() -> Vec<Stats> {
    sim_seats::SEATS
        .iter()
        .enumerate()
        .map(|(i, seat)| Stats {
            name: seat.name,
            answered: ANSWERED[i].load(Ordering::Relaxed),
            declined: DECLINED[i].load(Ordering::Relaxed),
        })
        .collect()
}

fn reset_stats() {
    for c in ANSWERED.iter().chain(&DECLINED) {
        c.store(0, Ordering::Relaxed);
    }
}

/// Count seat `i`'s unported path, and say so the first time each map.
fn declined(i: usize, pc: u32) {
    if DECLINED[i].fetch_add(1, Ordering::Relaxed) == 0 {
        eprintln!(
            "sim: seat {}: unported path at {pc:#x}; the lifted body runs it (counted in sim.arms)",
            sim_seats::SEATS[i].name
        );
    }
}

pub(crate) fn install(b: &mut Builder) {
    b.raw().observe_before(&engine::points::CACHE_TAGS_LOAD, DEVICE_RESET, |_| {
        sim_seats::device::arm::reset();
        sim_seats::leaf::reset();
        reset_stats();
    });
    for (i, seat) in sim_seats::SEATS.iter().enumerate() {
        let point = seat.point;
        let sites = seat.sites;
        let run = seat.run;
        b.raw()
            .implement(point, SEATS, move |cpu, ctx, pc| {
                let site = pc != point.base && sites.contains(&pc);
                if pc == point.base || site {
                    let mut mem = ctx.memory();
                    if let Some(transfer) = run(cpu, &mut mem, pc)? {
                        ANSWERED[i].fetch_add(1, Ordering::Relaxed);
                        return Ok(transfer);
                    }
                    declined(i, pc);
                }
                crate::sim_objects::original(cpu, ctx, pc, site)
            })
            .resumes(sites)
            .context_only();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim_native::SimNative;
    use hooks::{Base, Registry, Switches};

    #[test]
    fn every_seat_installs_with_no_switch() {
        // `-impl.sim.native` turns the other subsystems off, not the seats.
        for switches in [Switches::defaults(), Switches::defaults().with("impl.sim.native", false)] {
            let mut registry = Registry::new(switches);
            registry.add(&SimNative::for_tests()).unwrap();
            let plan = registry.build(&Base).unwrap();
            for seat in sim_seats::SEATS {
                let implement = plan.chains[&seat.point.base].implement.as_ref().unwrap();
                assert_eq!(implement.label, SEATS, "{}", seat.name);
                assert_eq!(implement.continuation_sites().0, seat.sites, "{}", seat.name);
            }
            let load = &plan.chains[&engine::points::CACHE_TAGS_LOAD.base];
            assert!(load.observe_before.iter().any(|h| h.label == DEVICE_RESET));
        }
    }

    #[test]
    fn the_seat_switches_are_gone() {
        for gone in [
            "impl.sim.native.seats",
            "impl.sim.aim-assist",
            "impl.sim.respawn-ready",
            "impl.sim.player-objects",
            "impl.sim.observer-resources",
            "impl.sim.native_observer_camera",
        ] {
            assert!(hooks::switches::table::get(gone).is_none(), "{gone}");
        }
    }

    #[test]
    fn stats_name_every_seat() {
        let names: Vec<_> = stats().iter().map(|s| s.name).collect();
        assert_eq!(names, sim_seats::SEATS.iter().map(|s| s.name).collect::<Vec<_>>());
    }

    /// The phantom action builder's executor (`bot_glue::retail_input`,
    /// `Builder::on`) keeps an override through `prefer_generated` only
    /// where its placed chain is context-only (`Chain::render_thread`,
    /// which `hooks::install::place` marks from). The target update and
    /// search hold the seat alone, so a phantom runs them in Rust. The
    /// control update also holds the aim-assist feature's open and close,
    /// which keep per-player host state and run after the body, so it
    /// stays the lifted original there.
    #[test]
    fn a_phantom_executor_keeps_the_target_seats_and_not_the_control_update() {
        let mut registry = Registry::new(Switches::defaults());
        registry.add(&SimNative::for_tests()).unwrap();
        registry
            .add(&hooks::from_fn("aim-assist-shape", |b| {
                b.raw().before(&engine::points::PLAYER_CONTROL_UPDATE, "open", |_| {
                    Ok(hooks::Flow::Continue)
                });
                b.raw().after(&engine::points::PLAYER_CONTROL_UPDATE, "close", |_| Ok(()));
                Ok(())
            }))
            .unwrap();
        let plan = registry.build(&Base).unwrap();
        for point in [&engine::points::AIM_TARGET_UPDATE, &engine::points::AIM_TARGET_SEARCH] {
            assert!(plan.chains[&point.base].render_thread().is_ok(), "{}", point.name);
        }
        assert!(plan.chains[&engine::points::PLAYER_CONTROL_UPDATE.base].render_thread().is_err());
    }

    #[test]
    fn the_seats_own_their_points() {
        let base = sim_seats::SEATS[0].point.base;
        assert!(owns(base));
        assert!(!owns(base + 4));
    }
}
