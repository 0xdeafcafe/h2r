//! What a seat runs per tick (docs/engine/sim-rewrite.md, "The tick
//! driver" and "Seats and rules (phase 7)").
//!
//! **`impl.sim.present`** (on): the tick driver leaves out phases 26
//! (first person), 28 (HUD) and 29 (`0x16f200`, the camera neighbourhood)
//! on a tick where no local user is mapped: a phantom or host seat has no
//! view. With a local user they run, and each draws from the
//! presentation's own copy of the two shared random seeds
//! (`engine::sim_tick::PRESENT_SEEDS`), so the sim's draws, and the sim
//! hash with them, do not depend on the view count. `sim.ledger` shows
//! the skipped phases at 0 calls and 0 ns.
//!
//! **`impl.sim.aim-assist`** (off): the aim-assist cone `1a50a0` is the
//! idiomatic kernel `sim_objects::aim_assist`, run as a shadow arm by
//! `engine::sim_seats::aim::arm`: the guest answers and the kernel's
//! prediction is compared at the return until 4096 calls have matched,
//! then the kernel answers and one call in 64 is still compared; a
//! mismatch gives every call back to the guest until the next map. Its
//! counters are `seats.aim` in `sim.arms`.
//!
//! **`impl.sim.respawn-ready`** (off): the respawn gate `15dd40`
//! (`PLAYER_RESPAWN_READY`) and its callees `158a50`, `15db30` and `15b2f0`
//! as the kernel `engine::sim_seats::respawn`, run as the same shadow arm.
//! A respawn timer above zero is declined by name (`countdown`): the
//! entity mark `a7840` and the death event `19eb90` are guest. It is not a
//! replacement of the guest function. Its counters are `seats.respawn` in
//! `sim.arms`.

use engine::points;
use engine::sim_seats::aim::arm::{self, Step};
use engine::sim_seats::respawn;
use engine::sim_tick::Present;
use hooks::Builder;
use runtime::TransferAbi;

/// The switch, without its `impl.` prefix.
pub const PRESENT: &str = "sim.present";
/// The aim-assist arm's switch, without its `impl.` prefix.
pub const AIM: &str = "sim.aim-assist";

pub fn mode(switches: &hooks::Switches) -> Present {
    if switches.implementation(PRESENT, true) { Present::LocalOnly } else { Present::Always }
}

/// The respawn gate arm's switch, without its `impl.` prefix.
pub const RESPAWN: &str = "sim.respawn-ready";

pub fn respawn_on(switches: &hooks::Switches) -> bool {
    switches.implementation(RESPAWN, false)
}

pub fn aim_on(switches: &hooks::Switches) -> bool {
    switches.implementation(AIM, false)
}

/// The map-load observer that clears the aim arm's proof.
const AIM_RESET: &str = "seats: reset the aim-assist arm";

/// The aim-assist arm at `AIM_ASSIST_PARAMS`, and its reset at map load.
/// [`crate::sim_native::SimNative`] calls it; the point is the arm's
/// alone (it is not one of the seats), so the plan holds one
/// implementation there, beside the `aim-assist` feature's `after`.
pub(crate) fn install(b: &mut Builder) {
    install_aim(b);
    install_respawn(b);
}

/// The respawn gate's arm at `PLAYER_RESPAWN_READY`, and its reset.
fn install_respawn(b: &mut Builder) {
    if !respawn_on(b.switches()) {
        return;
    }
    let point = &points::PLAYER_RESPAWN_READY;
    b.raw()
        .implement(point, RESPAWN, move |cpu, ctx, pc| {
            if pc == point.base {
                let step = {
                    let mem = ctx.memory();
                    respawn::enter(cpu, &mem)
                };
                match step {
                    respawn::Step::Answered { target } => return Ok(TransferAbi::ret(target)),
                    respawn::Step::Audit(prediction) => {
                        let watched = ctx.observe_guest_return(cpu, respawn::ARGS, move |c, _| {
                            respawn::compare(&prediction, c)
                        });
                        if !watched {
                            respawn::dropped();
                        }
                    }
                    respawn::Step::Guest => {}
                }
            }
            crate::sim_objects::original(cpu, ctx, pc, false)
        })
        .context_only()
        .optional();
    b.raw().observe_before(&points::CACHE_TAGS_LOAD, RESPAWN_RESET, |_| respawn::reset());
}

/// The map-load observer that clears the respawn arm's proof.
const RESPAWN_RESET: &str = "seats: reset the respawn-ready arm";

fn install_aim(b: &mut Builder) {
    if !aim_on(b.switches()) {
        return;
    }
    let point = &points::AIM_ASSIST_PARAMS;
    b.raw()
        .implement(point, AIM, move |cpu, ctx, pc| {
            if pc == point.base {
                let step = {
                    let mut mem = ctx.memory();
                    arm::enter(cpu, &mut mem)
                };
                match step {
                    Step::Answered { target } => return Ok(TransferAbi::ret(target)),
                    Step::Audit(prediction) => {
                        let watched = ctx.observe_guest_return(cpu, arm::ARGS, move |c, ctx| {
                            arm::compare(&prediction, c, &ctx.runtime().memory())
                        });
                        if !watched {
                            arm::dropped();
                        }
                    }
                    Step::Guest => {}
                }
            }
            crate::sim_objects::original(cpu, ctx, pc, false)
        })
        .context_only()
        .optional();
    b.raw().observe_before(&points::CACHE_TAGS_LOAD, AIM_RESET, |_| arm::reset());
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
    fn the_aim_arm_owns_its_point_only_when_on() {
        use crate::sim_native::SimNative;
        use hooks::{Base, Registry};
        assert!(!aim_on(&Switches::defaults()));
        let mut registry = Registry::new(Switches::defaults());
        registry.add(&SimNative::for_tests()).unwrap();
        let plan = registry.build(&Base).unwrap();
        assert!(plan.chains.get(&points::AIM_ASSIST_PARAMS.base).and_then(|c| c.implement.as_ref()).is_none());
        // On, with the seats on too: one implementation, the arm's.
        let on = Switches::defaults().with("impl.sim.aim-assist", true).with("impl.sim.native.seats", true);
        let mut registry = Registry::new(on);
        registry.add(&SimNative::for_tests()).unwrap();
        let plan = registry.build(&Base).unwrap();
        let implement = plan.chains[&points::AIM_ASSIST_PARAMS.base].implement.as_ref().unwrap();
        assert_eq!(implement.label, AIM);
    }

    #[test]
    fn a_local_seat_and_the_off_switch_keep_every_phase() {
        assert_eq!(run(Present::LocalOnly, &[2]).len(), PHASES.len());
        assert_eq!(run(Present::Always, &[]).len(), PHASES.len());
    }
}
