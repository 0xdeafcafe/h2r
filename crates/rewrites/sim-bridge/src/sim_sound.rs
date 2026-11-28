//! Install the sound pass for playback and clock-only sessions.
//!
//! Two arms. `impl.sim.native.sound` ([`SimSound`]) is the exact port: the
//! guest records carry all persistent state across mixer callbacks and
//! checkpoints. `impl.sim.sound-events` ([`SimSoundEvents`]) is the sound
//! simulation of docs/engine/sound-sim.md: the engine's own admission makes
//! each instance, the sim decides when it ends, and the engine's finish
//! runs on that tick. The host mixer is fed from the sim's events.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use engine::entry::{Cx, EntryResult, GuestCall, Ret};
use engine::{GuestMemory, MemFault, points};
use hooks::{BuildError, Builder, Feature, Flow, Manifest, SnapshotPolicy};
use sim_sound::events::{SoundDef, SoundEvent, SoundId, Source};
use sim_sound::feed::{Host, NullHost};
use sim_sound::schedule::Schedule;

pub mod levels;
pub mod entries;
pub mod guest_tags;
pub mod lifecycle;
pub mod mixer_feed;
pub mod owner_host;
pub mod owners;
pub mod request;
pub mod scene;
pub mod voice_arm;

pub const SOUND: &str = "sim.native.sound";
/// The sound simulation's switch (`impl.sim.sound-events`, default on; `h2 sim diff` parity in
/// docs/engine/notes/2026-11-27-sound-events-parity.md).
pub const EVENTS: &str = "sim.sound-events";
const ID: &str = "impl.sim.native.sound";
const EVENTS_ID: &str = "impl.sim.sound-events";

/// What the sim reads of a sound tag when it is started without an
/// instance (tests, and the headless default).
pub trait Durations: Send + Sync {
    /// The definition of the sound tag `tag`.
    fn def(&self, tag: u32) -> SoundDef;
}

/// No tag knowledge: every sound ends on the tick it starts.
pub struct NoDurations;

impl Durations for NoDurations {
    fn def(&self, tag: u32) -> SoundDef {
        SoundDef { tag, duration_ms: None, looping: false }
    }
}

/// Guest calls and runs of the sound simulation. `guest_calls` is the number
/// of guest calls its hooks made (the finishes, the ramp polls and the
/// owners' lip-sync updates): a sim that starts no instance makes none
/// (`sound_phase_ledger_is_zero`). It is not a phase of the tick driver's
/// ledger ([`crate::tick::LEDGER`]): the 29 phases hold no sound phase
/// (docs/engine/sound-sim.md, "Where the sound runs").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SoundLedger {
    pub guest_calls: u64,
    /// Runs of the main-loop service `0x125b40`.
    pub service_runs: u64,
    /// Ticks the schedule was stepped.
    pub tick_steps: u64,
    pub starts: u64,
    pub events: u64,
    /// Finishes of instances the sim made, by end tick or by a ramp.
    pub finishes: u64,
    /// Finishes skipped because the guest had freed the instance already.
    pub stale: u64,
    /// Refusals, each a sound the sim could not account for from its tag.
    /// An admission returned a handle whose record or tick could not be
    /// read: the sound is not entered, and the guest keeps it.
    pub refused_unreadable: u64,
    /// The permutation's length could not be read from the tag: the sound is
    /// entered and ends on the tick it starts.
    pub refused_no_duration: u64,
}

/// A running sound: the engine's instance and the tick it started on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Live {
    pub instance: u32,
    pub start: u32,
}

/// What is due at the start of a tick.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Due {
    /// Sounds that ended since the last delivery: `(id, instance, reason)`.
    pub finishes: Vec<(SoundId, u32, u32)>,
    /// Sounds still running, in start order.
    pub running: Vec<(SoundId, Live)>,
}

/// The sound simulation: the schedule, the events waiting for the host, the
/// running instances and the host.
pub struct SoundSim {
    schedule: Schedule,
    outbox: Vec<SoundEvent>,
    host: Box<dyn Host + Send>,
    durations: Arc<dyn Durations>,
    ledger: SoundLedger,
    last_step: Option<u32>,
    live: BTreeMap<SoundId, Live>,
    finishes: Vec<(SoundId, u32, u32)>,
    /// The tick the instances were last delivered for.
    last_delivery: Option<u32>,
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
            live: BTreeMap::new(),
            finishes: Vec::new(),
            last_delivery: None,
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

    /// A start the engine admitted: `instance` is its record.
    pub fn start_instance(&mut self, def: SoundDef, source: Source, tick: u32, instance: u32) -> SoundId {
        let id = self.start_def(def, source, tick);
        self.live.insert(id, Live { instance, start: tick });
        id
    }

    pub fn stop(&mut self, id: SoundId, tick: u32) -> bool {
        self.schedule.stop(id, tick)
    }

    /// A stop whose ramp finished: the sound leaves the schedule and the
    /// running set; the caller finishes the instance it returns.
    pub fn end_ramp(&mut self, id: SoundId, tick: u32) -> Option<u32> {
        let live = self.live.remove(&id)?;
        self.schedule.stop(id, tick);
        Some(live.instance)
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
        for e in &events {
            if let SoundEvent::Ended { id, .. } = e
                && let Some(live) = self.live.remove(id)
            {
                self.finishes.push((*id, live.instance, owners::ENDED_REASON));
            }
        }
        self.outbox.extend(events);
    }

    /// What is due at the start of `tick`: once per tick number, so the tick
    /// entered again after the calls delivers once. None when nothing runs
    /// and nothing ended.
    pub fn due(&mut self, tick: u32) -> Option<Due> {
        if self.last_delivery == Some(tick) {
            return None;
        }
        self.last_delivery = Some(tick);
        let due = Due {
            finishes: std::mem::take(&mut self.finishes),
            running: self.live.iter().map(|(id, l)| (*id, *l)).collect(),
        };
        (!due.finishes.is_empty() || !due.running.is_empty()).then_some(due)
    }

    /// The running sounds, in start order.
    pub fn running(&self) -> Vec<(SoundId, Live)> {
        self.live.iter().map(|(id, l)| (*id, *l)).collect()
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

    pub fn ledger_mut(&mut self) -> &mut SoundLedger {
        &mut self.ledger
    }

    pub fn durations(&self) -> &Arc<dyn Durations> {
        &self.durations
    }

    /// The schedule (length-prefixed), then the running sounds, the ended
    /// sounds whose finish is due, and the tick the instances were last
    /// delivered for.
    fn save(&self) -> Vec<u8> {
        let schedule = self.schedule.save();
        let mut out = Vec::with_capacity(schedule.len() + 16);
        out.extend((schedule.len() as u32).to_le_bytes());
        out.extend(schedule);
        out.extend((self.live.len() as u32).to_le_bytes());
        for (id, l) in &self.live {
            for w in [id.0, l.instance, l.start] {
                out.extend(w.to_le_bytes());
            }
        }
        out.extend((self.finishes.len() as u32).to_le_bytes());
        for (id, instance, reason) in &self.finishes {
            for w in [id.0, *instance, *reason] {
                out.extend(w.to_le_bytes());
            }
        }
        out.extend(self.last_delivery.unwrap_or(u32::MAX).to_le_bytes());
        out
    }

    fn load(&mut self, bytes: &[u8]) -> Result<(), String> {
        let mut at = 0usize;
        let mut take = |n: usize| -> Result<&[u8], String> {
            let end = at.checked_add(n).filter(|&e| e <= bytes.len()).ok_or("sound save: short")?;
            let r = &bytes[at..end];
            at = end;
            Ok(r)
        };
        let word = |take: &mut dyn FnMut(usize) -> Result<Vec<u8>, String>| -> Result<u32, String> {
            Ok(u32::from_le_bytes(take(4)?.try_into().expect("4 bytes")))
        };
        let mut owned = |n: usize| take(n).map(<[u8]>::to_vec);
        let len = word(&mut owned)? as usize;
        let schedule = Schedule::restore(&owned(len)?).map_err(|e| format!("sound schedule: {e:?}"))?;
        let mut live = BTreeMap::new();
        for _ in 0..word(&mut owned)? {
            let id = SoundId(word(&mut owned)?);
            live.insert(id, Live { instance: word(&mut owned)?, start: word(&mut owned)? });
        }
        let mut finishes = Vec::new();
        for _ in 0..word(&mut owned)? {
            finishes.push((SoundId(word(&mut owned)?), word(&mut owned)?, word(&mut owned)?));
        }
        let last = word(&mut owned)?;
        if at != bytes.len() {
            return Err("sound save: trailing bytes".into());
        }
        self.schedule = schedule;
        self.live = live;
        self.finishes = finishes;
        self.last_delivery = (last != u32::MAX).then_some(last);
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

fn tick_of(m: &dyn GuestMemory) -> Result<u32, MemFault> {
    m.u32(m.u32(points::GAME_TIME.base)?.wrapping_add(8))
}

/// The mixer's clock for a tick, in milliseconds: the sim's copy of what the
/// engine advances from the wall clock (`0x126a34`).
pub fn clock_ms(tick: u32) -> u32 {
    (u64::from(tick) * 1000 / 30) as u32
}

/// The engine's admission (`SOUND_IMPULSE_ALLOCATE`) run as the engine
/// does, then the instance it returned entered in the schedule.
pub async fn admit(cx: Cx, sim: Arc<Mutex<SoundSim>>, busy: Arc<AtomicBool>, args: [u32; 4]) -> EntryResult {
    busy.store(true, Ordering::SeqCst);
    let called = cx.call(GuestCall::new(points::SOUND_IMPULSE_ALLOCATE.base).args(args)).await;
    busy.store(false, Ordering::SeqCst);
    let instance = called?.eax;
    if instance != owners::NONE {
        let read = cx.read(|m| Ok::<_, MemFault>((tick_of(m)?, owners::read_admitted(m, instance))));
        match read {
            Ok((tick, Some(a))) => {
                let def = SoundDef { tag: a.tag, duration_ms: a.duration_ms, looping: false };
                let mut s = lock(&sim);
                if a.duration_ms.is_none() {
                    s.ledger_mut().refused_no_duration += 1;
                }
                s.start_instance(def, Source(a.owner), tick, instance);
            }
            _ => lock(&sim).ledger_mut().refused_unreadable += 1,
        }
    }
    Ok(Ret::new(instance, 16))
}

/// The start of a tick: the mixer clock, the stops whose ramp is done, the
/// finishes of what ended, then the tick itself (the hook's chain runs again
/// for it, finds nothing due, and continues into the body).
pub async fn deliver(cx: Cx, sim: Arc<Mutex<SoundSim>>, tick: u32, due: Due) -> EntryResult {
    cx.write(|m| {
        if let Ok(mixer) = m.u32(owners::MIXER) {
            let _ = m.set_u32(mixer.wrapping_add(owners::MIXER_CLOCK), clock_ms(tick));
        }
    });
    let mut finishes = due.finishes;
    let mut calls = 0u64;
    for (id, live) in &due.running {
        if !cx.read(|m| owners::ramping(m, live.instance)) {
            continue;
        }
        let ramp = cx.call(GuestCall::new(owners::RAMP).eax(live.instance)).await?.eax;
        calls += 1;
        if cx.read(|m| owners::ramp_done(m, live.instance, ramp))
            && let Some(instance) = lock(&sim).end_ramp(*id, tick)
        {
            finishes.push((*id, instance, owners::FADE_REASON));
        }
    }
    for (_, instance, reason) in finishes {
        if !cx.read(|m| owners::record(m, instance).is_some()) {
            lock(&sim).ledger_mut().stale += 1;
            continue;
        }
        cx.call(GuestCall::new(owners::FINISH).eax(instance).args([reason])).await?;
        calls += 1;
        lock(&sim).ledger_mut().finishes += 1;
    }
    cx.call(GuestCall::new(points::GAME_TICK.base)).await?;
    lock(&sim).ledger_mut().guest_calls += calls;
    Ok(Ret::new(0, 0))
}

/// The main loop's update: the events to the host, then the owners'
/// lip-sync updates the mixer's loop makes (`0x129593`), one for each
/// running sound whose owner has one, with the seconds since its start.
pub async fn service(cx: Cx, sim: Arc<Mutex<SoundSim>>) -> EntryResult {
    let running = {
        let mut s = lock(&sim);
        s.service();
        s.running()
    };
    let calls: Vec<_> = cx.read(|m| {
        let tick = tick_of(m).unwrap_or(0);
        running
            .iter()
            .filter_map(|(_, l)| owners::update_call(m, l.instance, owners::seconds(tick.saturating_sub(l.start))))
            .collect()
    });
    lock(&sim).ledger_mut().guest_calls += calls.len() as u64;
    for (function, args) in calls {
        cx.call(GuestCall::new(function).args(args)).await?;
    }
    Ok(Ret::new(0, 0))
}

/// The sound simulation as hooks (`impl.sim.sound-events`).
///
/// - `GAME_TICK`, after the body: steps the schedule at the tick that ran
///   (an observer).
/// - `GAME_TICK`, before the body, first in the chain: [`deliver`].
/// - `SOUND_IMPULSE_ALLOCATE`: [`admit`] runs the engine's admission and
///   enters the instance it returns.
/// - `SOUND_UPDATE` (`0x125b40`, the main-loop service): [`service`].
pub struct SimSoundEvents {
    sim: Arc<Mutex<SoundSim>>,
}

impl SimSoundEvents {
    pub fn new(sim: SoundSim) -> Self {
        SimSoundEvents { sim: Arc::new(Mutex::new(sim)) }
    }

    /// No tag knowledge and no host: the headless arm.
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
        // The table (`impl.sim.sound-events.table`, entries.rs) owns the
        // sound entries when it is on, and this arm stands down.
        if !b.switches().implementation(EVENTS, true) || entries::on(b.switches()) {
            return Ok(());
        }
        let sim = Arc::clone(&self.sim);
        b.raw().observe_after(&points::GAME_TICK, "sound step", move |o| {
            if let Ok(tick) = game_tick(&o.guest()) {
                lock(&sim).step(tick);
            }
        });
        let sim = Arc::clone(&self.sim);
        b.raw().implement_async(&points::SOUND_UPDATE, "sound service", move |cx| {
            Box::pin(service(cx, Arc::clone(&sim)))
        });
        // The engine's admission, called from the hook: the inner call finds
        // `busy` set and runs the original.
        let sim = Arc::clone(&self.sim);
        let busy = Arc::new(AtomicBool::new(false));
        b.raw()
            .before(&points::SOUND_IMPULSE_ALLOCATE, "sound admission", move |call| {
            if busy.load(Ordering::SeqCst) {
                return Ok(Flow::Continue);
            }
            let esp = call.cpu.esp;
            let g = call.ctx.runtime().memory();
            let args = [g.u32(esp.wrapping_add(4))?, g.u32(esp.wrapping_add(8))?, g.u32(esp.wrapping_add(12))?, g.u32(esp.wrapping_add(16))?];
            drop(g);
            let (sim, busy) = (Arc::clone(&sim), Arc::clone(&busy));
            call.answer_async(move |cx| Box::pin(admit(cx, sim, busy, args)))
            })
            // After the Halo 1 promotion has rewritten the sound it admits.
            .after_feature("h1-sound-promotion");
        // The mixer clock, the ramps and the finishes, before the tick.
        let sim = Arc::clone(&self.sim);
        b.raw()
            .before(&points::GAME_TICK, "sound instances", move |call| {
                let rt = Arc::clone(call.ctx.runtime());
                let tick = game_tick(&rt.memory())?;
                let Some(due) = lock(&sim).due(tick) else {
                    return Ok(Flow::Continue);
                };
                let sim = Arc::clone(&sim);
                call.answer_async(move |cx| Box::pin(deliver(cx, sim, tick, due)))
            })
            .first()
            // Innermost of the tick's first hooks: the others run as they
            // would without it, and it answers the tick last.
            .after_feature("map-select")
            .after_feature("hud-kit")
            .after_feature("actor-net")
            .after_feature("agent-calls")
            .after_feature("host-lobby")
            .after_feature("skulls-custom");
        Ok(())
    }
}

pub struct SimSound;

impl Feature for SimSound {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "Native sound simulation").doc("docs/engine/sim-rewrite.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        if !b.switches().implementation(SOUND, false) {
            b.snapshot(SnapshotPolicy::Stateless);
            return Ok(());
        }
        crate::sim_objects::install_table(
            b,
            &engine::sim_sound::TABLE,
            SOUND,
            engine::sim_sound::generated_sites,
        );
        // The per-tick voice update, shadowing the guest's (voice_arm.rs).
        b.snapshot(SnapshotPolicy::section(voice_arm::save, |bytes, _| voice_arm::load(bytes)));
        voice_arm::install(b);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hooks::{Base, Registry, Switches};
    use sim_sound::events::SoundDef;
    use std::sync::atomic::AtomicU64;

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
        let starts: Vec<_> = log.iter().filter(|e| matches!(e, SoundEvent::Started { .. })).map(SoundEvent::id).collect();
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
        assert_eq!((ledger.refused_unreadable, ledger.refused_no_duration), (0, 0));
        assert_eq!(ledger.starts, 300);
        assert_eq!(ledger.tick_steps, 300);
        assert_eq!(ledger.service_runs, 300);
        assert!(sim.due(1).is_none(), "a sound with no instance has nothing to finish");

        // The hooks: with the switch on, the service is implemented here and
        // the admission and the tick's start are `before` hooks; the marker
        // start and the stop are the engine's own.
        // The table's switch is a child of the events switch, so the events
        // arm under test names it off: with the table on, this arm stands down.
        for (spec, on) in [
            ("impl.sim.sound-events=off", false),
            ("impl.sim.sound-events,impl.sim.sound-events.table=off", true),
        ] {
            let mut registry = Registry::new(Switches::parse(spec).unwrap());
            registry.add(&SimSoundEvents::headless()).unwrap();
            let plan = registry.build(&Base).unwrap();
            assert_eq!(!plan.save_sections().is_empty(), on, "the schedule is saved with the switch on");
            let chain = |p: &engine::points::Point| plan.chains.get(&p.base);
            assert_eq!(chain(&points::SOUND_UPDATE).is_some_and(|c| c.implement.is_some()), on);
            let labels = |p: &engine::points::Point| {
                chain(p).map(|c| c.before.iter().map(|h| h.label).collect::<Vec<_>>()).unwrap_or_default()
            };
            assert_eq!(labels(&points::SOUND_IMPULSE_ALLOCATE), if on { vec!["sound admission"] } else { vec![] });
            assert_eq!(labels(&points::GAME_TICK), if on { vec!["sound instances"] } else { vec![] });
            assert!(chain(&points::SOUND_IMPULSE_ALLOCATE).is_none_or(|c| c.implement.is_none()), "the engine's admission runs");
            assert!(chain(&points::SOUND_START_AT_MARKER).is_none(), "the marker start is the engine's");
            assert!(chain(&points::SOUND_STOP_INSTANCE).is_none(), "the stop is the engine's");
        }
    }

    #[test]
    fn the_switch_is_on_by_default() {
        let on = |switches: Switches| {
            let mut registry = Registry::new(switches);
            registry.add(&SimSoundEvents::headless()).unwrap();
            let plan = registry.build(&Base).unwrap();
            plan.chains.get(&points::SOUND_UPDATE.base).is_some_and(|c| c.implement.is_some())
        };
        assert!(on(Switches::defaults()));
        assert!(!on(Switches::defaults().with("impl.sim.sound-events", false)));
    }

    fn instance(n: u32) -> u32 {
        0x1234_0000 | n
    }

    #[test]
    fn an_ended_sounds_finish_is_due_on_the_tick_after_its_end_event() {
        let mut sim = SoundSim::new(Arc::new(Table), Box::new(NullHost));
        let a = sim.start_instance(SoundDef { tag: 1000, duration_ms: Some(1000), looping: false }, Source(1), 0, instance(2));
        let due = sim.due(1).unwrap();
        assert_eq!((due.finishes.len(), due.running.len()), (0, 1));
        assert_eq!(due.running[0], (a, Live { instance: instance(2), start: 0 }));
        assert!(sim.due(1).is_none(), "a tick delivers once");
        for t in 1..30 {
            sim.step(t);
            assert_eq!(sim.due(t + 1).unwrap().running.len(), 1);
        }
        sim.step(30);
        let due = sim.due(31).unwrap();
        assert_eq!(due.finishes, vec![(a, instance(2), owners::ENDED_REASON)]);
        assert!(due.running.is_empty());
        assert!(sim.due(32).is_none(), "nothing left to deliver");
        // A ramp's end takes the sound out of the schedule at once and gives
        // the instance back.
        let b = sim.start_instance(SoundDef { tag: 1, duration_ms: None, looping: true }, Source::NONE, 40, instance(3));
        assert_eq!(sim.end_ramp(b, 45), Some(instance(3)));
        assert_eq!(sim.end_ramp(b, 45), None);
        sim.step(45);
        assert!(sim.due(46).is_none(), "its end is not a second finish");
        assert!(sim.wait_done(b, 45));
    }

    #[test]
    fn running_sounds_survive_a_save() {
        let mut a = SoundSim::new(Arc::new(Table), Box::new(NullHost));
        let id = a.start_instance(SoundDef { tag: 900, duration_ms: Some(900), looping: false }, Source(1), 0, instance(1));
        a.start_instance(SoundDef { tag: 1, duration_ms: Some(1), looping: false }, Source(1), 0, instance(2));
        a.step(1);
        a.due(2);
        let mut b = SoundSim::new(Arc::new(Table), Box::new(NullHost));
        b.load(&a.save()).unwrap();
        assert_eq!(a.hash(), b.hash());
        assert_eq!(a.due(3), b.due(3));
        let due = b.due(4).unwrap();
        assert_eq!(due.running.iter().map(|r| r.0).collect::<Vec<_>>(), vec![id]);
        assert!(b.load(&[1, 2, 3]).is_err());
    }

    #[test]
    fn the_clock_is_the_ticks_in_milliseconds() {
        assert_eq!([clock_ms(0), clock_ms(1), clock_ms(3), clock_ms(30)], [0, 33, 100, 1000]);
    }

    mod fixture {
        pub use crate::sim_sound::owners::fixture::*;
    }

    /// The fixture's guest with the clock and the tick the sim reads.
    fn world() -> engine::testing::FakeGuest {
        let mut g = fixture::guest();
        g.put_u32(points::GAME_TIME.base, fixture::TIME).put_u32(fixture::TIME + 8, 40);
        g.put_u32(owners::MIXER, fixture::MIXER_AT);
        g
    }

    fn calls_of(r: &engine::entry::testing::Run) -> Vec<(u32, Option<u32>, Vec<u32>)> {
        r.calls.iter().map(|c| (c.target, c.eax, c.args.clone())).collect()
    }

    #[test]
    fn the_admission_is_the_engines_and_its_instance_is_entered() {
        use engine::entry::{self, Regs};
        for (returned, entered) in [(fixture::HANDLE, true), (u32::MAX, false)] {
            let sim = Arc::new(Mutex::new(SoundSim::new(Arc::new(NoDurations), Box::new(NullHost))));
            let busy = Arc::new(AtomicBool::new(false));
            let args = [fixture::TAG, 1, 0x0060_0000, 3];
            let (s, bz) = (Arc::clone(&sim), Arc::clone(&busy));
            let f = move |cx: Cx| -> hooks::EntryFuture { Box::pin(admit(cx, Arc::clone(&s), Arc::clone(&bz), args)) };
            let r = entry::testing::run(&f, world(), Regs { esp: 0x0030_0000, ..Default::default() }, move |c, _| {
                assert_eq!(c.target, points::SOUND_IMPULSE_ALLOCATE.base);
                Regs { eax: returned, ..Default::default() }
            });
            assert_eq!(r.result, Ok(Ret::new(returned, 16)));
            assert_eq!(calls_of(&r), vec![(points::SOUND_IMPULSE_ALLOCATE.base, None, args.to_vec())]);
            assert!(!busy.load(Ordering::SeqCst), "the guard is released");
            let s = lock(&sim);
            assert_eq!(s.running().len(), usize::from(entered));
            if entered {
                // The 1741 ms permutation of the fixture ends 52 ticks on.
                let (id, l) = s.running()[0];
                assert_eq!((l.instance, l.start), (fixture::HANDLE, 40));
                assert_eq!(s.schedule().ends_at(id), Some(40 + 52));
            }
        }
    }

    #[test]
    fn a_tick_start_sets_the_clock_polls_ramps_and_finishes_then_runs_the_tick() {
        use engine::entry::{self, Regs};
        let mut g = world();
        // The instance is ramping and its ramp has reached the floor.
        g.put_u8(fixture::REC + 4, 1 << 7).put_u32(fixture::REC + 0xb4, 400).put_u32(fixture::REC + 0xb8, 100);
        g.put_u32(fixture::FLOOR, (-64.0f32).to_bits()).put_u32(fixture::REC + 0x20, 0);
        let sim = Arc::new(Mutex::new(SoundSim::new(Arc::new(NoDurations), Box::new(NullHost))));
        let ramping = lock(&sim).start_instance(SoundDef { tag: 1, duration_ms: Some(60_000), looping: false }, Source(1), 0, fixture::HANDLE);
        let due = lock(&sim).due(40).unwrap();
        let s = Arc::clone(&sim);
        let f = move |cx: Cx| -> hooks::EntryFuture { Box::pin(deliver(cx, Arc::clone(&s), 40, due.clone())) };
        let r = entry::testing::run(&f, g, Regs { esp: 0x0030_0000, ..Default::default() }, |c, _| {
            Regs { eax: if c.target == owners::RAMP { (-64.0f32).to_bits() } else { 0 }, ..Default::default() }
        });
        assert_eq!(r.result, Ok(Ret::new(0, 0)));
        assert_eq!(
            calls_of(&r),
            vec![
                (owners::RAMP, Some(fixture::HANDLE), vec![]),
                (owners::FINISH, Some(fixture::HANDLE), vec![owners::FADE_REASON]),
                (points::GAME_TICK.base, None, vec![]),
            ]
        );
        // The clock: tick 40 is 1333 ms.
        assert_eq!(r.guest.u32(fixture::MIXER_AT + owners::MIXER_CLOCK), Ok(1333));
        let s = lock(&sim);
        assert!(s.running().is_empty());
        assert_eq!(s.ledger().finishes, 1);
        assert_eq!(s.ledger().guest_calls, 2);
        assert!(s.wait_done(ramping, 40));
    }

    #[test]
    fn an_instance_the_guest_freed_is_not_finished_twice() {
        use engine::entry::{self, Regs};
        let mut g = world();
        g.put_u16(fixture::REC, 0);
        let sim = Arc::new(Mutex::new(SoundSim::new(Arc::new(NoDurations), Box::new(NullHost))));
        let due = Due { finishes: vec![(SoundId(1), fixture::HANDLE, owners::ENDED_REASON)], running: vec![] };
        let s = Arc::clone(&sim);
        let f = move |cx: Cx| -> hooks::EntryFuture { Box::pin(deliver(cx, Arc::clone(&s), 40, due.clone())) };
        let r = entry::testing::run(&f, g, Regs { esp: 0x0030_0000, ..Default::default() }, |_, _| Regs::default());
        assert_eq!(calls_of(&r), vec![(points::GAME_TICK.base, None, vec![])]);
        assert_eq!(lock(&sim).ledger().stale, 1);
    }

    #[test]
    fn the_service_makes_the_owners_update_with_the_seconds_since_the_start() {
        use engine::entry::{self, Regs};
        let sim = Arc::new(Mutex::new(SoundSim::new(Arc::new(NoDurations), Box::new(NullHost))));
        lock(&sim).start_instance(SoundDef { tag: 1, duration_ms: Some(60_000), looping: false }, Source(1), 10, fixture::HANDLE);
        let s = Arc::clone(&sim);
        let f = move |cx: Cx| -> hooks::EntryFuture { Box::pin(service(cx, Arc::clone(&s))) };
        let r = entry::testing::run(&f, world(), Regs { esp: 0x0030_0000, ..Default::default() }, |_, _| Regs::default());
        assert_eq!(r.result, Ok(Ret::new(0, 0)));
        // Tick 40, started on 10: one second.
        let want = vec![(owners::UPDATE_LIP_SYNC, None, vec![0xaaaa_0001, fixture::REC + 0x5c, fixture::TAG, 1, fixture::PERMS + 7 * 16, 1.0f32.to_bits()])];
        assert_eq!(calls_of(&r), want);
        let s = lock(&sim);
        assert_eq!((s.ledger().service_runs, s.ledger().guest_calls), (1, 1));
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
            assert_eq!(!plan.save_sections().is_empty(), enabled, "the voices are saved with the switch on");
            let table = &*engine::sim_sound::TABLE;
            // The voice arm's points, beside the bodies.
            let arm = [
                &points::SOUND_IMPULSE_ALLOCATE,
                &points::SOUND_STOP_INSTANCE,
                &points::SOUND_FINISH_INSTANCE,
                &points::GAME_TICK,
                &points::SOUND_UPDATE,
            ];
            let bases: std::collections::BTreeSet<u32> =
                table.bodies().iter().map(|b| b.point.base).chain(arm.iter().map(|p| p.base)).collect();
            assert_eq!(plan.chains.len(), if enabled { bases.len() } else { 0 });
            if enabled {
                for body in table.bodies() {
                    let hook = plan.chains[&body.point.base].implement.as_ref().unwrap();
                    assert_eq!(hook.label, SOUND);
                    assert_eq!(hook.continuation_sites(), (body.sites, engine::sim_sound::generated_sites(body.point.base)));
                }
                let chain = |p: &engine::points::Point| &plan.chains[&p.base];
                let labels = |hs: &[hooks::Hook]| hs.iter().map(|h| h.label).collect::<Vec<_>>();
                assert_eq!(labels(&chain(&points::SOUND_IMPULSE_ALLOCATE).observe_after), ["voice start"]);
                assert_eq!(labels(&chain(&points::SOUND_STOP_INSTANCE).observe_before), ["voice stop"]);
                assert_eq!(labels(&chain(&points::SOUND_FINISH_INSTANCE).observe_before), ["voice finish"]);
                assert_eq!(labels(&chain(&points::GAME_TICK).observe_after), ["voice tick"]);
                assert_eq!(labels(&chain(&points::SOUND_UPDATE).before), ["voice play-outs"]);
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
