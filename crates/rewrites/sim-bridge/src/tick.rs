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
//! Particles age as if seen (`engine::particle_seen`,
//! docs/engine/sim-render-split.md, "6. Render scratch and stamps"): at
//! each particle location's per-frame move the driver's observer sets the
//! stamp the move reads, so no system moves by what a view drew. It is on
//! wherever the tick is, rendered or not.
//!
//! [`LEDGER`] counts and times each guest phase the body calls. `sim.ledger` reads it
//! and `tick-ledger.toml` is its ratchet.

use std::sync::{Arc, Mutex};

use guest_layout::points;
use engine::sim_tick::{self, Ledger};
use hooks::{BuildError, Builder, EntryFuture, Feature, Manifest, SnapshotPolicy};

pub const ID: &str = "sim-tick";

/// The process's ledger, over every tick the driver runs.
pub static LEDGER: Mutex<Ledger> = Mutex::new(Ledger::new());

/// A copy of the ledger now.
pub fn ledger() -> Ledger {
    *LEDGER.lock().unwrap_or_else(|e| e.into_inner())
}

pub struct TickDriver;

/// The label of the particle locations' as-seen observer.
pub const AS_SEEN: &str = "particles age as if seen";

/// The tick: the 29 phases, then the node matrices of every object left
/// with a dirty pose ([`engine::tick_poses`], docs/engine/sim-render-split.md,
/// "2. Node matrices"), so the frame step reads matrices and never builds
/// them. Then the sound phase
/// (`crate::sim_sound::entries::phase`, docs/engine/sound-instances.md,
/// "The sound phase"). It runs once per tick number, so a tick whose
/// run flag held the count does not step the sounds again.
pub async fn run(
    cx: guest_layout::entry::Cx,
    present: sim_tick::Present,
    bodies: sim_tick::Bodies,
    sound: crate::sim_sound::entries::Shared,
) -> guest_layout::entry::EntryResult {
    // The World holds the tick's live objects and players: read in at the entry and
    // again at the exit. The guest's phases still write the arena
    // (creates, deletes, bodies) between the two; a scatter of the entry's
    // table at the exit would undo them, so the exit imports.
    cx.read(|m| crate::world::import_objects(m, &crate::world::OBJECT_TABLE));
    cx.read(|m| crate::world::import_players(m, &crate::world::PLAYERS_STATE));
    let ret = sim_tick::run_with(cx.clone(), &LEDGER, present, bodies).await?;
    cx.read(|m| crate::world::import_objects(m, &crate::world::OBJECT_TABLE));
    cx.read(|m| crate::world::import_players(m, &crate::world::PLAYERS_STATE));
    // The Havok heap as the step left it: the guest's Havok code still
    // writes it, so the World's physics store is read in after the tick.
    cx.read(|m| crate::world::import_heap(m, &crate::world::HAVOK_HEAP));
    engine::tick_poses::run(&cx, sim_tick::RECORD_SIZE).await?;
    crate::sim_sound::entries::phase(&cx, &sound).await?;
    Ok(ret)
}

/// The Rust phase bodies: every one (AI, the scheduler, effects, post_game, the
/// countdown requests). The
/// lifted form ([`sim_tick::Bodies::NONE`]) is the tests' reference only.
pub const BODIES: sim_tick::Bodies = sim_tick::Bodies::ALL;

impl Feature for TickDriver {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "The game tick, run from Rust").doc("docs/engine/sim-rewrite.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        // The body's state is its future, parked while a phase runs; the
        // ledger is a counter of this process, not of the game.
        b.snapshot(SnapshotPolicy::Stateless);
        let present = crate::present::mode(b.switches());
        let bodies = BODIES;
        let sound = Arc::clone(&crate::sim_sound::entries::SHARED);
        b.raw().implement_async(&points::GAME_TICK, "tick driver", move |cx| -> EntryFuture {
            Box::pin(run(cx, present, bodies, sound.clone()))
        });
        b.raw()
            .observe_before(&points::PARTICLE_LOCATION_MOVE, AS_SEEN, |o| {
                let location = o.cpu.ebx;
                let _ = engine::particle_seen::as_seen(&mut *o.guest(), location);
            })
            .optional();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::entry::{self, Regs};
    use engine::sim_tick::{Owner, SYSTEMS};

    #[test]
    fn the_rust_bodies_always_run() {
        assert!(BODIES.rust);
        let rust: Vec<_> = SYSTEMS.iter().filter(|p| p.owner == Owner::Rust).map(|p| p.name).collect();
        assert_eq!(rust, ["ai", "script_scheduler", "effects", "post_game", "countdown_requests"]);
    }
    use guest_layout::memory::GuestMemory;
    use guest_layout::testing::FakeGuest;
    use hooks::Flow;

    const STACK: u32 = 0x0030_0000;
    const TIME: u32 = 0x0052_0000;
    /// The tick clock, where `TICK_CLOCK` points.
    const CLOCK: u32 = 0x0052_0100;
    const RECORD: u32 = STACK - sim_tick::RECORD_SIZE;

    fn guest(tick: u32) -> FakeGuest {
        let mut g = FakeGuest::new();
        g.map(STACK - 0x8000, 0x10000).map(0x0051_0000, 0x1000).map(TIME, 0x1000);
        g.put_u32(points::GAME_TIME.base, TIME).put_u32(TIME + 8, tick).put_f32(TIME + 4, 0.25);
        g.put_u32(points::TICK_CLOCK.base, CLOCK).put_f32(CLOCK + 4, 1.0).put_u32(CLOCK + 0x14, 5);
        g
    }

    /// One tick over a fake guest with a fresh ledger. The record's before
    /// step sets the run flag to `flag`; the close sets it to `after`.
    fn tick_with(from: u32, flag: u8, after: u8) -> (entry::testing::Run, &'static Mutex<Ledger>) {
        let ledger: &'static Mutex<Ledger> = Box::leak(Box::new(Mutex::new(Ledger::new())));
        let f = move |cx: guest_layout::entry::Cx| -> EntryFuture { Box::pin(sim_tick::run(cx, ledger)) };
        let regs = Regs { esp: STACK, ..Default::default() };
        let callee = move |c: &guest_layout::entry::GuestCall, m: &mut dyn GuestMemory| {
            let at = RECORD + sim_tick::RUN_FLAG;
            if c.target == sim_tick::RECORD_BEFORE {
                m.set_u8(at, flag).unwrap();
            } else if c.target == sim_tick::RECORD_AFTER {
                m.set_u8(at, after).unwrap();
            }
            Regs::default()
        };
        (entry::testing::run(&f, guest(from), regs, callee), ledger)
    }

    fn tick(from: u32) -> (entry::testing::Run, &'static Mutex<Ledger>) {
        tick_with(from, 1, 1)
    }

    #[test]
    fn ledger_counts_guest_phases() {
        let (run, ledger) = tick(5);
        assert_eq!(run.result, Ok(hooks::Ret::new(0, 0)));
        let targets: Vec<u32> = run.calls.iter().map(|c| c.target).collect();
        let mut want = vec![sim_tick::CONTROL_WORD_FN, sim_tick::RECORD_BUILD, sim_tick::RECORD_BEFORE];
        want.extend(SYSTEMS.iter().map(|p| p.entry));
        want.push(sim_tick::RECORD_AFTER);
        assert_eq!(targets, want);
        // `sim_tick::run` is the lifted form: every phase a guest call.
        let l = ledger.lock().unwrap();
        for (i, p) in SYSTEMS.iter().enumerate() {
            assert_eq!(l.calls(i), 1, "{}", p.name);
        }
        assert_eq!((l.ticks(), l.tick()), (1, 6));
        // The phases' times are spans inside the tick: they sum to at most
        // the tick's, in the total and in the last tick.
        let sum: u64 = (0..SYSTEMS.len()).map(|i| l.phase_ns(i)).sum();
        let last: u64 = (0..SYSTEMS.len()).map(|i| l.phase_last_ns(i)).sum();
        assert!(sum <= l.tick_ns() && last <= l.tick_last_ns(), "{sum} {last} {}", l.tick_ns());
        assert_eq!((sum, l.tick_ns()), (last, l.tick_last_ns()), "one tick: total is last");
        assert_eq!(l.loops(), 0, "one entry has no span yet");
        assert_eq!(run.guest.u32(TIME + 8), Ok(6), "the tick count moved by one");
        // The record sits below the entry stack and the body's calls below it.
        let build = &run.calls[1];
        assert_eq!(build.eax, Some(RECORD));
        assert!(run.calls.iter().all(|c| c.reserve == sim_tick::RECORD_SIZE));
        // The control word's mask is retail's.
        assert_eq!(run.calls[0].args, [sim_tick::CONTROL_WORD_NEW, 0x000f_ffff]);
        // Phase 1 takes the record on the stack and in ECX; the rest EBX.
        let phases = &run.calls[3..3 + SYSTEMS.len()];
        assert_eq!((phases[0].args.clone(), phases[0].ecx, phases[0].ebx), (vec![RECORD], Some(RECORD), None));
        for c in &phases[1..] {
            assert_eq!((c.args.len(), c.ecx, c.ebx), (0, None, Some(RECORD)), "{:#x}", c.target);
        }
        // The clock step: the counter fell by one, the clock gained 0.25.
        assert_eq!(run.guest.u32(CLOCK + 0x14), Ok(4));
        assert_eq!(run.guest.f32(CLOCK + 4), Ok(1.25));
    }

    /// A clear run flag skips the phases and the clock; the close still runs
    /// and the tick does not move.
    #[test]
    fn a_clear_run_flag_skips_the_phases() {
        let (run, ledger) = tick_with(5, 0, 0);
        let targets: Vec<u32> = run.calls.iter().map(|c| c.target).collect();
        assert_eq!(
            targets,
            [
                sim_tick::CONTROL_WORD_FN,
                sim_tick::RECORD_BUILD,
                sim_tick::RECORD_BEFORE,
                sim_tick::RECORD_AFTER
            ]
        );
        assert_eq!(run.guest.u32(TIME + 8), Ok(5));
        assert_eq!((run.guest.u32(CLOCK + 0x14), run.guest.f32(CLOCK + 4)), (Ok(5), Ok(1.0)));
        assert_eq!(ledger.lock().unwrap().tick(), 5);
    }

    /// A close that clears the flag leaves the tick where it was, though the
    /// phases and the clock ran.
    #[test]
    fn a_close_that_clears_the_flag_holds_the_tick() {
        let (run, _) = tick_with(5, 1, 0);
        assert_eq!(run.calls.len(), 4 + SYSTEMS.len());
        assert_eq!(run.guest.u32(TIME + 8), Ok(5));
        assert_eq!(run.guest.u32(CLOCK + 0x14), Ok(4));
    }

    /// After the record's close, the tick builds the node matrices of each
    /// object still carrying a dirty pose (`engine::tick_poses`): one
    /// `0x000bd090` call per object, in index order, and none for a clean
    /// or held one.
    #[test]
    fn the_tick_ends_with_the_dirty_poses_built() {
        use engine::tick_poses::{BODY_FLAGS, DIRTY, HOLD, PARENT};
        let (pool, slots) = (0x0060_0000, 0x0060_1000);
        let mut g = guest(5);
        g.put_u32(points::OBJECTS.base, pool).put_u32(pool + 0x38, 3).put_u32(pool + 0x44, slots);
        for (i, flags) in [DIRTY, DIRTY | HOLD, DIRTY].into_iter().enumerate() {
            let (slot, body) = (slots + 12 * i as u32, 0x0061_0000 + 0x100 * i as u32);
            g.put_u16(slot, 7).put_u32(slot + 8, body);
            g.put_u32(body + BODY_FLAGS, flags).put_u32(body + PARENT, u32::MAX);
        }
        let callee = |c: &guest_layout::entry::GuestCall, m: &mut dyn GuestMemory| {
            let at = RECORD + sim_tick::RUN_FLAG;
            match c.target {
                sim_tick::RECORD_BEFORE | sim_tick::RECORD_AFTER => m.set_u8(at, 1).unwrap(),
                t if t == points::OBJECT_NODE_MATRICES.base => {
                    let flags = 0x0061_0000 + 0x100 * (c.args[0] & 0xffff) + BODY_FLAGS;
                    let v = m.u32(flags).unwrap();
                    m.set_u32(flags, v & !DIRTY).unwrap();
                }
                _ => {}
            }
            Regs::default()
        };
        let table = Arc::new(Mutex::new(crate::sim_sound::entries::Table::guest()));
        let f = move |cx: guest_layout::entry::Cx| -> EntryFuture {
            Box::pin(run(cx, sim_tick::Present::Always, sim_tick::Bodies::NONE, Arc::clone(&table)))
        };
        let r = entry::testing::run(&f, g, Regs { esp: STACK, ..Default::default() }, callee);
        assert_eq!(r.result, Ok(hooks::Ret::new(0, 0)));
        let after = r.calls.iter().position(|c| c.target == sim_tick::RECORD_AFTER).unwrap();
        let tail: Vec<_> = r.calls[after + 1..].iter().map(|c| (c.target, c.args.clone())).collect();
        let bd090 = points::OBJECT_NODE_MATRICES.base;
        assert_eq!(tail, [(bd090, vec![7 << 16]), (bd090, vec![7 << 16 | 2])]);
        assert_eq!(engine::tick_poses::plan(&r.guest), Some(vec![]));
    }

    /// The tick steps the sound phase after the 29 and
    /// once per tick number: a tick whose close held the count does not
    /// step it again.
    #[test]
    fn the_sound_phase_runs_after_the_phases_once_per_tick_number() {
        use crate::sim_sound::entries::{Table, lock};
        let table = Arc::new(Mutex::new(Table::guest()));
        let regs = Regs { esp: STACK, ..Default::default() };
        let mut g = guest(5);
        g.map(0x004e_0000, 0x1_0000).put_u32(points::RANDOM_SEEDS.base, TIME + 0x200);
        let callee = |c: &guest_layout::entry::GuestCall, m: &mut dyn GuestMemory| {
            let at = RECORD + sim_tick::RUN_FLAG;
            if c.target == sim_tick::RECORD_BEFORE {
                m.set_u8(at, 1).unwrap();
            } else if c.target == sim_tick::RECORD_AFTER {
                m.set_u8(at, 0).unwrap();
            }
            Regs::default()
        };
        for _ in 0..2 {
            let t = Arc::clone(&table);
            let f = move |cx: guest_layout::entry::Cx| -> EntryFuture {
                Box::pin(run(cx, sim_tick::Present::Always, sim_tick::Bodies::NONE, Arc::clone(&t)))
            };
            let r = entry::testing::run(&f, g.clone(), regs, callee);
            assert_eq!(r.result, Ok(hooks::Ret::new(0, 0)));
            assert_eq!(r.calls.last().map(|c| c.target), Some(sim_tick::RECORD_AFTER), "the phase makes no guest call");
        }
        assert_eq!(lock(&table).ledger.phase_runs, 1, "tick 5 twice steps once");
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

    #[test]
    fn every_particle_location_moves_as_if_seen() {
        let mut r = hooks::Registry::new(hooks::Switches::parse("").unwrap());
        r.add(&TickDriver).unwrap();
        let text = r.build(&hooks::Base).unwrap().text();
        assert!(
            text.contains(&format!("0x00248750 PARTICLE_LOCATION_MOVE: observe_before [sim-tick:{AS_SEEN}]")),
            "{text}"
        );
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
