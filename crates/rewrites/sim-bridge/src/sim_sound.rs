//! Install the sound pass for playback and clock-only sessions.
//!
//! Two arms. `impl.sim.native.sound` ([`SimSound`]) is the exact port: the
//! guest records carry all persistent state across mixer callbacks and
//! checkpoints. `impl.sim.sound-events` ([`SimSoundEvents`]) is the sound
//! simulation of docs/engine/sound-sim.md: a sound started in the sim ends
//! on a tick computed from its tag, there is no voice state, and the host
//! mixer is fed from the sim's events. It makes no guest call.

use std::sync::{Arc, Mutex, MutexGuard};

use engine::points;
use hooks::{BuildError, Builder, Feature, Manifest, SnapshotPolicy};
use sim_sound::events::{SoundDef, SoundEvent, SoundId, Source};
use sim_sound::feed::{Host, NullHost};
use sim_sound::schedule::Schedule;

pub mod mixer_feed;
pub mod tag_durations;

pub use tag_durations::GuestDurations;

pub const SOUND: &str = "sim.native.sound";
/// The sound simulation's switch (`impl.sim.sound-events`, default off until
/// `h2 sim diff` has the answer for it).
pub const EVENTS: &str = "sim.sound-events";
const ID: &str = "impl.sim.native.sound";
const EVENTS_ID: &str = "impl.sim.sound-events";

/// What the sim hands out in place of a guest sound instance: the high bits
/// mark it, the low bits are the [`SoundId`].
pub const SIM_HANDLE: u32 = 0x7000_0000;

pub fn handle(id: SoundId) -> u32 {
    SIM_HANDLE | (id.0 & 0x0fff_ffff)
}

/// The id behind a handle the sim gave out.
pub fn from_handle(h: u32) -> Option<SoundId> {
    (h & 0xf000_0000 == SIM_HANDLE).then_some(SoundId(h & 0x0fff_ffff))
}

/// What the sim reads of a sound tag. The tag's duration and loop flag are
/// the whole of what decides when the sound ends.
pub trait Durations: Send + Sync {
    /// The definition of the sound tag `tag`.
    fn def(&self, tag: u32) -> SoundDef;
    /// The definition of `tag` read from guest memory at a start. The default
    /// is [`Durations::def`]; [`GuestDurations`] reads the loaded tag.
    fn guest_def(&self, _g: &runtime::Guest, tag: u32) -> SoundDef {
        self.def(tag)
    }
    /// The tag of a marker start, from its parameter block in guest memory.
    /// `None` when the block's layout is not known: the start then has tag 0
    /// and ends on the tick it starts.
    fn marker_tag(&self, _g: &runtime::Guest, _params: u32) -> Option<u32> {
        None
    }
}

/// No tag knowledge: every sound ends on the tick it starts.
pub struct NoDurations;

impl Durations for NoDurations {
    fn def(&self, tag: u32) -> SoundDef {
        SoundDef { tag, duration_ms: None, looping: false }
    }
}

/// Guest calls and runs of the sound simulation. `guest_calls` is the number
/// of guest calls its hooks made: none of its entry points holds a guest-call
/// capability, so it stays 0 (`sound_phase_ledger_is_zero`). It is not a
/// phase of the tick driver's ledger ([`crate::tick::LEDGER`]): the 29 phases
/// hold no sound phase (docs/engine/sound-sim.md, "Where the sound runs").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SoundLedger {
    pub guest_calls: u64,
    /// Runs of the main-loop service `0x125b40`.
    pub service_runs: u64,
    /// Ticks the schedule was stepped.
    pub tick_steps: u64,
    pub starts: u64,
    pub events: u64,
}

/// The sound simulation: the schedule, the events waiting for the host, and
/// the host. Nothing in it reads guest memory.
pub struct SoundSim {
    schedule: Schedule,
    outbox: Vec<SoundEvent>,
    host: Box<dyn Host + Send>,
    durations: Arc<dyn Durations>,
    ledger: SoundLedger,
    last_step: Option<u32>,
}

impl SoundSim {
    pub fn new(durations: Arc<dyn Durations>, host: Box<dyn Host + Send>) -> Self {
        SoundSim {
            schedule: Schedule::new(),
            outbox: Vec::new(),
            host,
            durations,
            ledger: SoundLedger::default(),
            last_step: None,
        }
    }

    /// A sound started on `tick` from a tag.
    pub fn start(&mut self, tag: u32, source: Source, tick: u32) -> SoundId {
        let def = self.durations.def(tag);
        self.start_def(def, source, tick)
    }

    pub fn start_def(&mut self, def: SoundDef, source: Source, tick: u32) -> SoundId {
        self.ledger.starts += 1;
        self.schedule.start(def, source, tick)
    }

    pub fn stop(&mut self, id: SoundId, tick: u32) -> bool {
        self.schedule.stop(id, tick)
    }

    /// The tick's sound step, from the game thread inside the tick. A tick
    /// the sim steps twice (a rerun of the same tick) steps once.
    pub fn step(&mut self, tick: u32) {
        if self.last_step == Some(tick) {
            return;
        }
        self.last_step = Some(tick);
        self.ledger.tick_steps += 1;
        let events = self.schedule.step(tick);
        self.ledger.events += events.len() as u64;
        self.outbox.extend(events);
    }

    /// The main-loop service: hands what the ticks since the last run made to
    /// the host. It changes nothing the schedule holds.
    pub fn service(&mut self) {
        self.ledger.service_runs += 1;
        let events = std::mem::take(&mut self.outbox);
        sim_sound::feed::deliver(&mut *self.host, &events);
    }

    /// A script's wait on a sound: done on the computed end tick.
    pub fn wait_done(&self, id: SoundId, tick: u32) -> bool {
        self.schedule.finished(id, tick)
    }

    pub fn ticks_left(&self, id: SoundId, tick: u32) -> Option<u32> {
        self.schedule.ticks_left(id, tick)
    }

    pub fn schedule(&self) -> &Schedule {
        &self.schedule
    }

    /// The sound state the sim hash covers.
    pub fn hash(&self) -> u64 {
        self.schedule.hash()
    }

    pub fn ledger(&self) -> SoundLedger {
        self.ledger
    }

    pub fn durations(&self) -> &Arc<dyn Durations> {
        &self.durations
    }

    fn save(&self) -> Vec<u8> {
        self.schedule.save()
    }

    fn load(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.schedule = Schedule::restore(bytes).map_err(|e| format!("sound schedule: {e:?}"))?;
        self.outbox.clear();
        self.last_step = None;
        Ok(())
    }
}

fn lock(m: &Mutex<SoundSim>) -> MutexGuard<'_, SoundSim> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn game_tick(g: &runtime::Guest) -> Result<u32, runtime::GuestFault> {
    g.u32(g.u32(points::GAME_TIME.base)?.wrapping_add(8))
}

/// The sound simulation as hooks (`impl.sim.sound-events`).
///
/// - `GAME_TICK`, after the body: steps the schedule at the tick that ran
///   (an observer; no guest call).
/// - `SOUND_UPDATE` (`0x125b40`, the main-loop service): implemented here;
///   it hands the events to the host and returns, with no guest call.
/// - `SOUND_IMPULSE_ALLOCATE` and `SOUND_START_AT_MARKER`: implemented
///   here; the start goes to the schedule and the caller gets a sim handle
///   ([`handle`]) in place of a guest instance.
pub struct SimSoundEvents {
    sim: Arc<Mutex<SoundSim>>,
}

impl SimSoundEvents {
    pub fn new(sim: SoundSim) -> Self {
        SimSoundEvents { sim: Arc::new(Mutex::new(sim)) }
    }

    /// The guest's tags and no host: the headless arm.
    pub fn headless() -> Self {
        Self::new(SoundSim::new(Arc::new(NoDurations), Box::new(NullHost)))
    }

    pub fn sim(&self) -> Arc<Mutex<SoundSim>> {
        Arc::clone(&self.sim)
    }
}

impl Feature for SimSoundEvents {
    fn manifest(&self) -> Manifest {
        Manifest::new(EVENTS_ID, "Sound simulation: events and computed end ticks")
            .doc("docs/engine/sound-sim.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        let save = Arc::clone(&self.sim);
        let load = Arc::clone(&self.sim);
        b.snapshot(SnapshotPolicy::section(
            move || lock(&save).save(),
            move |bytes, _| lock(&load).load(bytes),
        ));
        if !b.switches().implementation(EVENTS, false) {
            return Ok(());
        }
        let sim = Arc::clone(&self.sim);
        b.raw().observe_after(&points::GAME_TICK, "sound step", move |o| {
            if let Ok(tick) = game_tick(&o.guest()) {
                lock(&sim).step(tick);
            }
        });
        let sim = Arc::clone(&self.sim);
        b.raw().implement(&points::SOUND_UPDATE, "sound service", move |cpu, ctx, _| {
            lock(&sim).service();
            ctx.memory().ret(cpu, 0)
        });
        let sim = Arc::clone(&self.sim);
        b.raw().implement(&points::SOUND_IMPULSE_ALLOCATE, "sound impulse", move |cpu, ctx, _| {
            let mut mem = ctx.memory();
            let tag = mem.u32(cpu.esp.wrapping_add(4))?;
            let source = mem.u32(cpu.esp.wrapping_add(12))?;
            let tick = game_tick(&mem)?;
            let durations = Arc::clone(lock(&sim).durations());
            let def = durations.guest_def(&mem, tag);
            cpu.eax = handle(lock(&sim).start_def(def, Source(source), tick));
            mem.ret(cpu, 16)
        });
        let sim = Arc::clone(&self.sim);
        b.raw().implement(&points::SOUND_START_AT_MARKER, "sound at marker", move |cpu, ctx, _| {
            let mut mem = ctx.memory();
            let params = mem.u32(cpu.esp.wrapping_add(4))?;
            let tick = game_tick(&mem)?;
            let durations = Arc::clone(lock(&sim).durations());
            let tag = durations.marker_tag(&mem, params).unwrap_or(0);
            let def = durations.guest_def(&mem, tag);
            cpu.eax = handle(lock(&sim).start_def(def, Source(cpu.esi), tick));
            mem.ret(cpu, 4)
        });
        Ok(())
    }
}

pub struct SimSound;

impl Feature for SimSound {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "Native sound simulation").doc("docs/engine/sim-rewrite.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        b.snapshot(SnapshotPolicy::Stateless);
        if b.switches().implementation(SOUND, false) {
            crate::sim_objects::install_table(
                b,
                &engine::sim_sound::TABLE,
                SOUND,
                engine::sim_sound::generated_sites,
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hooks::{Base, Registry, Switches};
    use sim_sound::events::SoundDef;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Ticks per second the end ticks are computed at.
    const SECOND: u32 = 30;

    struct Table;
    impl Durations for Table {
        fn def(&self, tag: u32) -> SoundDef {
            SoundDef { tag, duration_ms: Some(tag), looping: false }
        }
    }

    /// A host that records what it hears, standing in for the audible mixer.
    #[derive(Clone, Default)]
    struct Audible(Arc<Mutex<Vec<SoundEvent>>>, Arc<AtomicU64>);
    impl Host for Audible {
        fn event(&mut self, e: &SoundEvent) {
            self.0.lock().unwrap().push(*e);
            self.1.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// One scripted run: the same starts and stops on the same ticks, the
    /// service at an arbitrary frame cadence. Returns the sim hash per tick.
    fn run(mut sim: SoundSim, frames_per_tick: u32) -> Vec<u64> {
        let mut hashes = Vec::new();
        let mut live = Vec::new();
        for tick in 0..120u32 {
            if tick % 7 == 0 {
                live.push(sim.start(1000 + tick * 10, Source(tick), tick));
            }
            if tick == 50 {
                let id = live[3];
                sim.stop(id, tick);
            }
            sim.step(tick);
            for _ in 0..frames_per_tick {
                sim.service();
            }
            hashes.push(sim.hash());
        }
        hashes
    }

    #[test]
    fn null_sink_host_hash_equals_audible() {
        let null = run(SoundSim::new(Arc::new(Table), Box::new(NullHost)), 1);
        let audible = Audible::default();
        let heard = run(SoundSim::new(Arc::new(Table), Box::new(audible.clone())), 1);
        assert_eq!(null, heard);
        assert!(audible.1.load(Ordering::Relaxed) > 0, "the audible host heard the events");
        // How often the main loop serves the host does not reach the hash.
        let often = run(SoundSim::new(Arc::new(Table), Box::new(Audible::default())), 3);
        let never = run(SoundSim::new(Arc::new(Table), Box::new(NullHost)), 0);
        assert_eq!(null, often);
        assert_eq!(null, never);
        // The host hears starts in start order and every sound's end once.
        let log = audible.0.lock().unwrap();
        let starts: Vec<_> = log.iter().filter_map(|e| matches!(e, SoundEvent::Started { .. }).then(|| e.id())).collect();
        assert!(starts.windows(2).all(|w| w[0] < w[1]));
        let mut ends = log.iter().filter(|e| !matches!(e, SoundEvent::Started { .. })).map(SoundEvent::id).collect::<Vec<_>>();
        ends.sort();
        ends.dedup();
        assert_eq!(ends.len(), log.iter().filter(|e| !matches!(e, SoundEvent::Started { .. })).count());
    }

    #[test]
    fn script_wait_ends_on_computed_tick() {
        let mut sim = SoundSim::new(Arc::new(Table), Box::new(NullHost));
        // A 2 s line started on tick 10 ends on tick 70, whatever the
        // frames between do and whether or not a host listens.
        let id = sim.start(2000, Source(1), 10);
        assert_eq!(sim.schedule().ends_at(id), Some(10 + 2 * SECOND));
        for tick in 10..70 {
            sim.step(tick);
            sim.service();
            assert!(!sim.wait_done(id, tick), "tick {tick}");
            assert_eq!(sim.ticks_left(id, tick), Some(70 - tick));
        }
        assert!(sim.wait_done(id, 70));
        sim.step(70);
        assert!(sim.wait_done(id, 70));
        assert_eq!(sim.ticks_left(id, 70), Some(0));
        // A loop is never done; stopping it is.
        let looped = sim.start_def(SoundDef { tag: 1, duration_ms: None, looping: true }, Source::NONE, 70);
        sim.step(71);
        assert!(!sim.wait_done(looped, 100_000));
        assert!(sim.stop(looped, 72));
        assert!(sim.wait_done(looped, 72));
        // The same tick stepped twice steps once.
        let before = sim.ledger().tick_steps;
        sim.step(71);
        assert_eq!(sim.ledger().tick_steps, before);
    }

    #[test]
    fn sound_phase_ledger_is_zero() {
        let mut sim = SoundSim::new(Arc::new(Table), Box::new(NullHost));
        for tick in 0..300 {
            sim.start(500, Source(tick), tick);
            sim.step(tick);
            sim.service();
        }
        let ledger = sim.ledger();
        assert_eq!(ledger.guest_calls, 0);
        assert_eq!(ledger.starts, 300);
        assert_eq!(ledger.tick_steps, 300);
        assert_eq!(ledger.service_runs, 300);

        // The hooks: with the switch on, the service and both starts are
        // implemented in Rust; no table body (a guest continuation) and no
        // other entry is installed.
        for (spec, on) in [("", false), ("impl.sim.sound-events", true)] {
            let mut registry = Registry::new(Switches::parse(spec).unwrap());
            registry.add(&SimSoundEvents::headless()).unwrap();
            let plan = registry.build(&Base).unwrap();
            assert!(!plan.save_sections().is_empty(), "the schedule is saved");
            for point in [&points::SOUND_UPDATE, &points::SOUND_IMPULSE_ALLOCATE, &points::SOUND_START_AT_MARKER] {
                let chain = plan.chains.get(&point.base);
                assert_eq!(chain.is_some_and(|c| c.implement.is_some()), on, "{}", point.name);
                if let Some(hook) = chain.and_then(|c| c.implement.as_ref()) {
                    assert!(hook.continuation_sites().0.is_empty(), "{}: no guest continuation", point.name);
                }
            }
            assert_eq!(plan.chains.contains_key(&points::GAME_TICK.base), on);
        }
    }

    #[test]
    fn handles_round_trip() {
        for n in [1, 2, 0x0fff_ffff] {
            assert_eq!(from_handle(handle(SoundId(n))), Some(SoundId(n)));
        }
        assert_eq!(from_handle(0x0012_3456), None);
    }

    #[test]
    fn saved_schedule_restores_into_a_fresh_sim() {
        let mut a = SoundSim::new(Arc::new(Table), Box::new(NullHost));
        a.start(5000, Source(1), 0);
        a.start(100, Source(2), 1);
        a.step(2);
        let mut b = SoundSim::new(Arc::new(Table), Box::new(NullHost));
        b.load(&a.save()).unwrap();
        assert_eq!(a.hash(), b.hash());
        a.step(40);
        b.step(40);
        assert_eq!(a.hash(), b.hash());
    }

    #[test]
    fn switch_installs_each_sound_entry_and_its_callback_returns() {
        for (spec, enabled) in [
            ("", false),
            ("-impl.sim.native", false),
            ("impl.sim.native", true),
            ("impl.sim.native,-impl.sim.native.sound", false),
            ("-impl.sim.native,impl.sim.native.sound", true),
        ] {
            let mut registry = Registry::new(Switches::parse(spec).unwrap());
            registry.add(&SimSound).unwrap();
            let plan = registry.build(&Base).unwrap();
            assert!(plan.save_sections().is_empty());
            let table = &*engine::sim_sound::TABLE;
            assert_eq!(plan.chains.len(), if enabled { table.bodies().len() } else { 0 });
            if enabled {
                for body in table.bodies() {
                    let hook = plan.chains[&body.point.base].implement.as_ref().unwrap();
                    assert_eq!(hook.label, SOUND);
                    assert_eq!(hook.continuation_sites(), (body.sites, engine::sim_sound::generated_sites(body.point.base)));
                }
            }
        }
    }

    #[test]
    fn callbacks_must_return_to_a_supported_float_environment() {
        let table = &*engine::sim_sound::TABLE;
        let mut cpu = runtime::CpuAbi::default();
        cpu.mxcsr = 0x1f80;
        cpu.x87.control = 0x37f;
        cpu.x87.status = 0;
        assert!(table.supports(&cpu));
        for rc in [0x400, 0x800, 0xc00] {
            cpu.x87.control = 0x37f | rc;
            assert!(!table.supports(&cpu));
        }
    }
}
