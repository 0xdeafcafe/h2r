//! What a seat runs per tick (docs/engine/sim-rewrite.md, "The tick
//! driver" and "Seats and rules (phase 7)").
//!
//! **`impl.sim.present`** (on): the tick driver leaves out phases 26
//! (first person), 28 (HUD) and 29 (`0x16f200`, the countdown requests)
//! on a tick where no local user is mapped: a phantom or host seat has no
//! view. With a local user they run, and each draws from the
//! presentation's own copy of the two shared random seeds
//! (`engine::sim_tick::PRESENT_SEEDS`), so the sim's draws, and the sim
//! hash with them, do not depend on the view count. `sim.ledger` shows
//! the skipped phases at 0 calls and 0 ns.
//!
//! **The seat kernels** answer three guest functions, always, with no
//! switch and no guest arm. A call a kernel cannot answer yet is a named,
//! counted error (its `Decline`, logged the first time each map, counted in
//! `sim.arms`) that the lifted body runs:
//!
//! | Point | Kernel | Unported paths |
//! |---|---|---|
//! | `AIM_ASSIST_PARAMS` `1a50a0` | `engine::sim_seats::aim::arm` over `sim_objects::aim_assist` | `float`, `triggers`, `fault` |
//! | `PLAYER_RESPAWN_READY` `15dd40` | `engine::sim_seats::respawn` (the gate, its callees, the countdown with `a7840` and `19eb90`) | `post` (a host's event message), `announce` (a local player's HUD text and sound), `every-player`, `fault` |
//! | `PLAYER_PICKUP_SCAN` `1508e0` | `engine::sim_seats::player_objects` (the query `bb050` through `engine::sim_collision::plan_visit_at`, the link walk, `150e30`'s early exits) | `visit`, `candidate` (a type 2 or 3 handle: the Halo 1 prompt hook observes `150e30`), `pickup`, `fault` |
//!
//! Their counters are `seats.aim`, `seats.respawn` and
//! `seats.player-objects` in `sim.arms`.

use engine::points;
use engine::sim_seats::aim::arm;
use engine::sim_seats::{player_objects, respawn};
use engine::sim_tick::Present;
use hooks::Builder;
use runtime::TransferAbi;

/// The switch, without its `impl.` prefix.
pub const PRESENT: &str = "sim.present";

pub fn mode(switches: &hooks::Switches) -> Present {
    if switches.implementation(PRESENT, true) { Present::LocalOnly } else { Present::Always }
}

/// The labels the kernels install under.
pub const AIM: &str = "sim.seats.aim";
pub const RESPAWN: &str = "sim.seats.respawn";
pub const PLAYER_OBJECTS: &str = "sim.seats.player-objects";

/// The map-load observer that clears the three kernels' counters.
const RESET: &str = "seats: reset the aim, respawn and pickup counters";

/// The three kernels at their points, and their reset at map load.
/// [`crate::sim_native::SimNative`] calls it; the points are not seats, so
/// each plan holds one implementation there, beside the `aim-assist`
/// feature's `after` at the cone.
pub(crate) fn install(b: &mut Builder) {
    b.raw()
        .implement(&points::AIM_ASSIST_PARAMS, AIM, move |cpu, ctx, pc| {
            if pc == points::AIM_ASSIST_PARAMS.base {
                let step = arm::enter(cpu, &mut ctx.memory());
                if let arm::Step::Answered { target } = step {
                    return Ok(TransferAbi::ret(target));
                }
            }
            crate::sim_objects::original(cpu, ctx, pc, false)
        })
        .context_only()
        .optional();
    b.raw()
        .implement(&points::PLAYER_RESPAWN_READY, RESPAWN, move |cpu, ctx, pc| {
            if pc == points::PLAYER_RESPAWN_READY.base {
                let step = respawn::enter(cpu, &mut ctx.memory());
                if let respawn::Step::Answered { target } = step {
                    return Ok(TransferAbi::ret(target));
                }
            }
            crate::sim_objects::original(cpu, ctx, pc, false)
        })
        .context_only()
        .optional();
    b.raw()
        .implement(&points::PLAYER_PICKUP_SCAN, PLAYER_OBJECTS, move |cpu, ctx, pc| {
            if pc == points::PLAYER_PICKUP_SCAN.base {
                let step = player_objects::enter(cpu, &mut ctx.memory(), &engine::sim_collision::WORLD);
                if let player_objects::Step::Answered { target } = step {
                    return Ok(TransferAbi::ret(target));
                }
            }
            crate::sim_objects::original(cpu, ctx, pc, false)
        })
        .context_only()
        .optional();
    b.raw().observe_before(&points::CACHE_TAGS_LOAD, RESET, |_| {
        arm::reset();
        respawn::reset();
        player_objects::reset();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::entry::{self, Regs};
    use engine::sim_tick::{self, PHASES};
    use engine::testing::FakeGuest;
    use hooks::Switches;
    use std::sync::Mutex;

    const STACK: u32 = 0x0030_0000;
    const TIME: u32 = 0x0052_0000;
    const CLOCK: u32 = 0x0052_0100;
    const USERS: u32 = 0x0053_0000;

    fn run(mode: Present, mapped: &[u32]) -> Vec<u32> {
        let mut g = FakeGuest::new();
        g.map(STACK - 0x8000, 0x10000).map(0x0051_0000, 0x1000).map(TIME, 0x1000).map(0x004e_8000, 0x1000).map(USERS, 0x1000);
        g.put_u32(engine::points::GAME_TIME.base, TIME).put_u32(TIME + 8, 1).put_f32(TIME + 4, 0.25);
        g.put_u32(engine::points::TICK_CLOCK.base, CLOCK).put_f32(CLOCK + 4, 1.0).put_u32(CLOCK + 0x14, 5);
        g.put_u32(sim_tick::LOCAL_USERS, USERS);
        for u in 0..4 {
            g.put_u32(USERS + 0xc + 4 * u, if mapped.contains(&u) { u } else { u32::MAX });
        }
        let ledger: &'static Mutex<sim_tick::Ledger> = Box::leak(Box::new(Mutex::new(sim_tick::Ledger::new())));
        let f = move |cx: engine::entry::Cx| -> hooks::EntryFuture { Box::pin(sim_tick::run_with(cx, ledger, mode, sim_tick::Bodies::NONE)) };
        let callee = |c: &engine::entry::GuestCall, m: &mut dyn engine::memory::GuestMemory| {
            if c.target == sim_tick::RECORD_BEFORE || c.target == sim_tick::RECORD_AFTER {
                m.set_u8(STACK - sim_tick::RECORD_SIZE + sim_tick::RUN_FLAG, 1).unwrap();
            }
            Regs::default()
        };
        let r = entry::testing::run(&f, g, Regs { esp: STACK, ..Default::default() }, callee);
        r.calls.iter().map(|c| c.target).filter(|t| PHASES.iter().any(|p| p.entry == *t)).collect()
    }

    #[test]
    fn the_switch_is_on_by_default() {
        assert_eq!(mode(&Switches::defaults()), Present::LocalOnly);
        assert_eq!(mode(&Switches::defaults().with("impl.sim.present", false)), Present::Always);
    }

    #[test]
    fn phantom_seat_runs_no_camera() {
        let calls = run(Present::LocalOnly, &[]);
        assert_eq!(calls.len(), PHASES.len() - 3);
        for e in [0x0016_5dc1, 0x0019_17b2, 0x0016_f200] {
            assert!(!calls.contains(&e), "{e:#x}");
        }
    }

    /// The sim's state after one tick with `mapped` local users: every
    /// phase draws once from the shared seeds (the step the render uses)
    /// and the sim phases fold their draw into a sim word, so the words
    /// and the seeds hold every draw the sim saw.
    fn sim_state(mode: Present, mapped: &[u32]) -> [u32; 4] {
        const SEEDS: u32 = 0x0054_0000;
        const SIM: u32 = 0x0054_0100;
        let mut g = FakeGuest::new();
        g.map(STACK - 0x8000, 0x10000).map(0x0051_0000, 0x1000).map(TIME, 0x1000).map(0x004e_8000, 0x1000).map(USERS, 0x1000).map(SEEDS, 0x1000).map(0x004e_7000, 0x1000);
        g.put_u32(engine::points::GAME_TIME.base, TIME).put_u32(TIME + 8, 1).put_f32(TIME + 4, 0.25);
        g.put_u32(engine::points::TICK_CLOCK.base, CLOCK).put_f32(CLOCK + 4, 1.0).put_u32(CLOCK + 0x14, 5);
        g.put_u32(sim_tick::LOCAL_USERS, USERS);
        g.put_u32(engine::points::RANDOM_SEEDS.base, SEEDS).put_u32(SEEDS, 7).put_u32(SEEDS + 4, 11);
        for u in 0..4 {
            g.put_u32(USERS + 0xc + 4 * u, if mapped.contains(&u) { u } else { u32::MAX });
        }
        let ledger: &'static Mutex<sim_tick::Ledger> = Box::leak(Box::new(Mutex::new(sim_tick::Ledger::new())));
        let f = move |cx: engine::entry::Cx| -> hooks::EntryFuture { Box::pin(sim_tick::run_with(cx, ledger, mode, sim_tick::Bodies::NONE)) };
        let callee = |c: &engine::entry::GuestCall, m: &mut dyn engine::memory::GuestMemory| {
            if c.target == sim_tick::RECORD_BEFORE || c.target == sim_tick::RECORD_AFTER {
                m.set_u8(STACK - sim_tick::RECORD_SIZE + sim_tick::RUN_FLAG, 1).unwrap();
            }
            if let Some(i) = PHASES.iter().position(|p| p.entry == c.target) {
                for at in [SEEDS, SEEDS + 4] {
                    let seed = m.u32(at).unwrap().wrapping_mul(0x0019_660d).wrapping_add(0x3c6e_f35f);
                    m.set_u32(at, seed).unwrap();
                }
                if !sim_tick::is_presentation(i) {
                    let s = m.u32(SEEDS).unwrap();
                    let w = m.u32(SIM).unwrap();
                    m.set_u32(SIM, w.rotate_left(5) ^ s ^ i as u32).unwrap();
                }
            }
            Regs::default()
        };
        let r = entry::testing::run(&f, g, Regs { esp: STACK, ..Default::default() }, callee);
        use engine::memory::GuestMemory;
        let g = r.guest;
        [g.u32(SEEDS).unwrap(), g.u32(SEEDS + 4).unwrap(), g.u32(SIM).unwrap(), g.u32(TIME + 8).unwrap()]
    }

    #[test]
    fn hash_equal_with_0_and_4_views() {
        let _one = SEEDS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let none = sim_state(Present::LocalOnly, &[]);
        assert_eq!(sim_state(Present::LocalOnly, &[0, 1, 2, 3]), none);
        assert_eq!(sim_state(Present::LocalOnly, &[1]), none);
        // Retail's order: the views' draws reach the sim, so the state
        // depends on them, and the test sees it.
        assert_ne!(sim_state(Present::Always, &[0, 1, 2, 3]), none);
        // The presentation stream moved: the views drew from it.
        assert!(sim_tick::PRESENT_SEEDS.get().is_some());
    }

    /// The presentation stream is process-wide: the tests that drive it
    /// run one at a time.
    static SEEDS_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn the_kernels_own_their_points_with_no_switch() {
        use crate::sim_native::SimNative;
        use hooks::{Base, Registry};
        let mut registry = Registry::new(Switches::defaults());
        registry.add(&SimNative::for_tests()).unwrap();
        let plan = registry.build(&Base).unwrap();
        for (point, label) in [
            (&points::AIM_ASSIST_PARAMS, AIM),
            (&points::PLAYER_RESPAWN_READY, RESPAWN),
            (&points::PLAYER_PICKUP_SCAN, PLAYER_OBJECTS),
        ] {
            let implement = plan.chains[&point.base].implement.as_ref().unwrap();
            assert_eq!(implement.label, label, "{}", point.name);
        }
        assert!(plan.chains[&points::CACHE_TAGS_LOAD.base].observe_before.iter().any(|h| h.label == RESET));
    }

    #[test]
    fn a_local_seat_and_the_off_switch_keep_every_phase() {
        assert_eq!(run(Present::LocalOnly, &[2]).len(), PHASES.len());
        assert_eq!(run(Present::Always, &[]).len(), PHASES.len());
    }
}
