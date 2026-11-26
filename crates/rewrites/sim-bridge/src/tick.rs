//! The game tick from Rust (`impl` of `GAME_TICK`, `0x00137fe0`;
//! docs/engine/sim-rewrite.md, "The tick driver"). [`engine::sim_tick::run`]
//! is the body: the control word, the update record, the 29 phases in retail
//! order and the tick count. There is no switch: this is the tick.
//!
//! The hooks on `GAME_TICK` stay where they were. Rust takes the original's
//! place in the chain, so each `observe_before`, `before`, `after` and
//! `observe_after` hook (lockstep's hash, telemetry, firefight, the agent
//! server and its queued calls, actor-net, cheats, skulls, co-op) runs at the
//! same point around the body. A `before` that returns skips the body as it
//! skipped the original.
//!
//! [`LEDGER`] counts each guest phase the body calls. `sim.ledger` reads it
//! and `tick-ledger.toml` is its ratchet.

use std::sync::Mutex;

use engine::points;
use engine::sim_tick::{self, Ledger};
use hooks::{BuildError, Builder, EntryFuture, Feature, Manifest, SnapshotPolicy};

pub const ID: &str = "sim-tick";

/// The process's ledger, over every tick the driver runs.
pub static LEDGER: Mutex<Ledger> = Mutex::new(Ledger::new());

/// A copy of the ledger now.
pub fn ledger() -> Ledger {
    LEDGER.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

pub struct TickDriver;

impl Feature for TickDriver {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "The game tick, run from Rust").doc("docs/engine/sim-rewrite.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        // The body's state is its future, parked while a phase runs; the
        // ledger is a counter of this process, not of the game.
        b.snapshot(SnapshotPolicy::Stateless);
        b.raw().implement_async(&points::GAME_TICK, "tick driver", |cx| -> EntryFuture {
            Box::pin(sim_tick::run(cx, &LEDGER))
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::entry::{self, Regs};
    use engine::sim_tick::{Owner, PHASES};
    use engine::memory::GuestMemory;
    use engine::testing::FakeGuest;
    use hooks::Flow;

    const STACK: u32 = 0x0030_0000;
    const TIME: u32 = 0x0052_0000;

    fn guest(tick: u32) -> FakeGuest {
        let mut g = FakeGuest::new();
        g.map(STACK - 0x8000, 0x10000).map(0x0051_0000, 0x1000).map(TIME, 0x1000);
        g.put_u32(points::GAME_TIME.base, TIME).put_u32(TIME + 8, tick);
        g
    }

    /// One tick over a fake guest with a fresh ledger.
    fn tick(from: u32) -> (entry::testing::Run, &'static Mutex<Ledger>) {
        let ledger: &'static Mutex<Ledger> = Box::leak(Box::new(Mutex::new(Ledger::new())));
        let f = move |cx: engine::entry::Cx| -> EntryFuture { Box::pin(sim_tick::run(cx, ledger)) };
        let regs = Regs { esp: STACK, ..Default::default() };
        (entry::testing::run(&f, guest(from), regs, |_, _| Regs::default()), ledger)
    }

    #[test]
    fn ledger_counts_guest_phases() {
        let (run, ledger) = tick(5);
        assert_eq!(run.result, Ok(hooks::Ret::new(0, 0)));
        let targets: Vec<u32> = run.calls.iter().map(|c| c.target).collect();
        let mut want = vec![sim_tick::CONTROL_WORD_FN, sim_tick::RECORD_BUILD, sim_tick::RECORD_BEFORE];
        want.extend(PHASES.iter().map(|p| p.entry));
        want.push(sim_tick::RECORD_AFTER);
        assert_eq!(targets, want);
        let l = ledger.lock().unwrap();
        for (i, p) in PHASES.iter().enumerate() {
            assert_eq!(l.calls(i), u32::from(p.owner == Owner::Guest), "{}", p.name);
        }
        assert_eq!((l.ticks(), l.tick()), (1, 6));
        assert_eq!(run.guest.u32(TIME + 8), Ok(6), "the tick count moved by one");
        // The record sits below the entry stack and the body's calls below it.
        let build = &run.calls[1];
        assert_eq!(build.eax, Some(STACK - sim_tick::RECORD_SIZE));
        assert!(run.calls.iter().all(|c| c.reserve == sim_tick::RECORD_SIZE));
    }

    /// The ledger's phase table is at or under `tick-ledger.toml` for
    /// every map kind: the ratchet only falls.
    #[test]
    fn ledger_ratchet_does_not_grow() {
        let text = include_str!("../tick-ledger.toml");
        let mut in_table = false;
        let mut kinds = Vec::new();
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            if line.starts_with('[') {
                in_table = line == "[guest_phases]";
            } else if in_table {
                let (kind, n) = line.split_once('=').expect("kind = number");
                kinds.push((kind.trim().to_owned(), n.trim().parse::<usize>().expect("a count")));
            }
        }
        assert_eq!(
            kinds.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
            ["menu", "multiplayer", "campaign"]
        );
        for (kind, allowed) in kinds {
            assert!(
                sim_tick::guest_phases() <= allowed,
                "{kind}: {} guest phases are called, tick-ledger.toml allows {allowed}",
                sim_tick::guest_phases()
            );
        }
    }

    /// Every hook kind on `GAME_TICK` stays in its place around the driver:
    /// before the body, after it, and the observers outside both.
    #[test]
    fn existing_tick_hooks_still_fire() {
        let hooked = hooks::from_fn("tick-hooks", |b: &mut Builder| {
            let p = &points::GAME_TICK;
            b.raw().observe_before(p, "obs-before", |_| {});
            b.raw().before(p, "before", |_| Ok(Flow::Continue));
            b.raw().after(p, "after", |_| Ok(()));
            b.raw().observe_after(p, "obs-after", |_| {});
            Ok(())
        });
        let mut r = hooks::Registry::new(hooks::Switches::parse("").unwrap());
        r.add(&hooked).unwrap();
        r.add(&TickDriver).unwrap();
        let plan = r.build(&hooks::Base).unwrap();
        let text = plan.text();
        assert!(
            text.contains(
                "0x00137fe0 GAME_TICK: observe_before [tick-hooks:obs-before] -> before [tick-hooks:before] \
                 -> implement sim-tick:tick driver -> after [tick-hooks:after] \
                 -> observe_after [tick-hooks:obs-after]\n"
            ),
            "{text}"
        );
    }
}
