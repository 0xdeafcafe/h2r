//! `impl.sim.present`: seats that present nothing run no presentation
//! phase (docs/engine/sim-rewrite.md, "The tick driver"). Default off.
//!
//! With the switch on, the tick driver leaves out phases 26 (first person),
//! 28 (HUD) and 29 (`0x16f200`, the camera neighbourhood) on a tick where
//! no local user is mapped: a phantom or host seat has no view. With a
//! local user, every phase runs as retail does, because first person draws
//! the sim RNG. The sim hash therefore cannot depend on the view count.
//! `sim.ledger` shows the skipped phases at 0 calls and 0 ns.
//!
//! Aim assist is not part of this switch: it stays a sim kernel under
//! `impl.sim.native.seats` (`engine::sim_seats::aim`).

use engine::sim_tick::Present;

/// The switch, without its `impl.` prefix.
pub const PRESENT: &str = "sim.present";

pub fn mode(switches: &hooks::Switches) -> Present {
    if switches.implementation(PRESENT, false) { Present::LocalOnly } else { Present::Always }
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
        let f = move |cx: engine::entry::Cx| -> hooks::EntryFuture { Box::pin(sim_tick::run_with(cx, ledger, mode)) };
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

    #[test]
    fn a_local_seat_and_the_off_switch_keep_every_phase() {
        assert_eq!(run(Present::LocalOnly, &[2]).len(), PHASES.len());
        assert_eq!(run(Present::Always, &[]).len(), PHASES.len());
    }
}
