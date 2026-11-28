//! The bounce search in sim mode (docs/runtime/bounce-search.md): the
//! glue between the game and the pure job ([`sim_bench::bounces::Job`]).
//!
//! Two halves:
//!
//! - [`tick`], a `before` on the game tick (`sim: bounces`, last in the
//!   chain). It reads every bound seat as the seam drops do
//!   (`sim_drops`), hands the job the last fork's outcome, and carries
//!   out each seat's act: the drops' (the pair word, the stop, the level,
//!   the pad, the teleport) and the search's two more, the body's whole
//!   velocity and crouch on the pad. When the job asks for a fork it
//!   keeps the ask; the tick observer `sim: bounces fork` marks it due
//!   once the tick has returned ([`fork_due`]) and ends the slice
//!   (`Runtime::request_preempt`). Inside the tick the game thread is in
//!   the tick chain's after-hooks continuation, which no save can carry,
//!   so a slice that ends there leaves the ask waiting.
//! - [`between_slices`], called by the boot loop with the machine at
//!   hand, a few transfers after that tick. It saves the whole machine
//!   into a ring entry, or puts one back (docs/runtime/bounce-search.md,
//!   "The fork"): guest RAM by the ring's page pool over the write
//!   watch (`sim_bounces::ring`), every other part by its snapshot
//!   section, in the fork's scope ([`fork_scope`]).
//!
//! At the end the run's provable sites go into its copy of the map's
//! verified file (`bots::spots::store_verified`), and the records into
//! the report's `bounces` block.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use agent::sim::calls::{self, Call, PendingCall, Shared};
use guest_layout::GuestMemory;
use live_telemetry::havok_agents::{self as hk, LevelPairs};
use sim_bench::bounces::{BouncesRun, ForkAsk, ForkDone, Job, MAX_VARIANTS};
use sim_bench::drops::{PairSense, SeatSense, Sense};

use crate::sim_drops::{Body, Seat, body, level, pad, stop};
use crate::snapshot_glue::{BootMachine, check_hash};
use ring::Ring;

mod ring;

thread_local! {
    static FORK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// A fork's save or load is running on this thread ([`fork_scope`]).
/// Frame-only snapshot policies serialise state that only a whole-machine
/// fork may carry; an ordinary agent snapshot has none of it.
pub fn in_fork() -> bool {
    FORK.get()
}

/// Runs `work` as a fork's save or load: the parts serialise what only a
/// whole-machine fork may ([`in_fork`]).
pub(crate) fn fork_scope<T>(work: impl FnOnce() -> T) -> T {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            FORK.set(self.0);
        }
    }
    let _guard = Restore(FORK.replace(true));
    work()
}

/// The most the ring keeps: a whole RAM for every variant and one more
/// for the capture in hand, and room for their sections. Pages two
/// entries share are kept once, so a run holds far less.
const RING_CAP: usize = (MAX_VARIANTS + 1) * runtime::guest::RAM_SIZE + (64 << 20);

/// A saved machine: every part's section but RAM, which is the ring's.
type Sections = Arc<Box<[u8]>>;

struct State {
    map: String,
    job: Job,
    calls: Option<Shared>,
    tickets: BTreeMap<u8, PendingCall>,
    /// Teleports refused when asked, answered the next tick.
    refused: BTreeMap<u8, String>,
    last_tick: Option<u64>,
    saved: bool,
    /// The fork the job asked for this tick, for [`between_slices`].
    fork: Fork,
    /// What came of it, for the next tick.
    done: Option<ForkDone>,
    /// `None` while [`between_slices`] works on it.
    ring: Option<Ring<Sections>>,
}

/// The fork the job asked for, and whether its tick has returned.
#[derive(Default)]
struct Fork {
    ask: Option<ForkAsk>,
    /// The tick that asked has returned ([`fork_due`]): until then the
    /// game thread is inside the tick (in its chain's after-hooks
    /// continuation), and a slice that ends there for any reason is no
    /// place to save.
    due: bool,
}

impl Fork {
    /// The job's ask this tick, before the tick runs.
    fn asked(&mut self, ask: Option<ForkAsk>) {
        *self = Fork { ask, due: false };
    }
    /// The tick returned: true when an ask waits.
    fn tick_returned(&mut self) -> bool {
        self.due = self.ask.is_some();
        self.due
    }
    fn pending(&self) -> bool {
        self.due && self.ask.is_some()
    }
    /// The ask, once its tick has returned.
    fn take(&mut self) -> Option<ForkAsk> {
        if !self.due {
            return None;
        }
        self.due = false;
        self.ask.take()
    }
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn lock() -> MutexGuard<'static, Option<State>> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The run's job (`Sim::add_bots`).
pub fn set(map: &str, job: Job) {
    *lock() = Some(State {
        map: map.to_string(),
        job,
        calls: None,
        tickets: BTreeMap::new(),
        refused: BTreeMap::new(),
        last_tick: None,
        saved: false,
        fork: Fork::default(),
        done: None,
        ring: Some(Ring::new(RING_CAP)),
    });
}

/// The calls queue the teleports go through (`Sim::install_with_film`).
pub fn set_calls(q: Shared) {
    if let Some(s) = lock().as_mut() {
        s.calls = Some(q);
    }
}

/// Whether this run searches.
pub fn on() -> bool {
    lock().is_some()
}

/// The report block.
pub fn report() -> Option<BouncesRun> {
    lock().as_ref().map(|s| s.job.report())
}

/// `match.status`'s `bounces`: trials planned and done, where the job is
/// (its phase, batch and variant), the forks so far and the error, if
/// any.
pub fn status() -> Option<serde_json::Value> {
    lock().as_ref().map(|s| {
        let (phase, batch, variant) = s.job.at();
        let f = s.job.stats();
        serde_json::json!({
            "planned": s.job.planned(),
            "done": s.job.done(),
            "sites": s.job.sites(),
            "batches": s.job.batches(),
            "phase": phase,
            "batch": batch,
            "variant": variant,
            "captures": f.captures,
            "restores": f.restores,
            "waves": f.waves,
            "busy": f.busy,
            "last_busy": f.last_busy,
            "error": s.job.error(),
        })
    })
}

/// The named error that stopped the run (`fork: ...`).
pub fn error() -> Option<String> {
    lock()
        .as_ref()
        .and_then(|s| s.job.error().map(str::to_string))
}

/// Writes the object's whole velocity and each of its Havok motions'.
fn velocity(m: &mut dyn GuestMemory, b: &Body, v: [f32; 3]) {
    use live_telemetry::layout::{BODY_VELOCITY, MOTION_VELOCITY, RIGID_MOTION};
    let mut bytes = [0u8; 12];
    for (i, c) in v.iter().enumerate() {
        bytes[i * 4..i * 4 + 4].copy_from_slice(&c.to_bits().to_le_bytes());
    }
    let _ = m.write(b.base.wrapping_add(BODY_VELOCITY), &bytes);
    let Ok(entities) = hk::object_entities(&*m, b.unit) else {
        return;
    };
    for e in entities {
        if let Ok(motion) = m.u32(e + RIGID_MOTION)
            && motion != 0
        {
            let _ = m.write(motion + MOTION_VELOCITY, &bytes);
        }
    }
}

/// What [`tick`] came to: whether the job is over (the match should
/// end). A fork it asked for waits in [`fork_pending`].
pub struct Ticked {
    pub done: bool,
}

/// One tick for `seats`, before the tick runs.
pub fn tick(g: &mut runtime::Guest, seats: &[Seat]) -> Ticked {
    let mut guard = lock();
    let Some(st) = guard.as_mut() else {
        return Ticked { done: false };
    };
    let quiet = |st: &State| Ticked {
        done: st.job.finished(),
    };
    let Ok(tick) = crate::telemetry::game_tick(g) else {
        return quiet(st);
    };
    // A tick's chain runs again after the agent's guest calls: once a tick.
    if st.last_tick == Some(tick) {
        return quiet(st);
    }
    st.last_tick = Some(tick);
    let m: &mut dyn GuestMemory = g;
    let bodies: Vec<Option<Body>> = seats
        .iter()
        .map(|s| body(&*m, u32::from(s.player)))
        .collect();
    let units: Vec<u32> = bodies
        .iter()
        .map(|b| b.as_ref().map_or(0, |b| b.unit))
        .collect();
    let pairs: Vec<LevelPairs> = hk::level_pairs(&*m, &units)
        .unwrap_or_else(|_| vec![LevelPairs::default(); units.len()]);
    let mut senses = Vec::with_capacity(seats.len());
    for (i, s) in seats.iter().enumerate() {
        let teleported = match st.refused.remove(&s.seat) {
            Some(e) => Some(Err(e)),
            None => match st.tickets.get(&s.seat).and_then(|t| t.outcome()) {
                Some(out) => {
                    st.tickets.remove(&s.seat);
                    Some(out.map(|_| ()).map_err(|e| e.message))
                }
                None => None,
            },
        };
        let b = bodies[i].as_ref();
        senses.push(SeatSense {
            seat: s.seat,
            sense: Sense {
                pos: b.and_then(|b| b.pos),
                vel: b.and_then(|b| b.vel),
                airborne: b.is_some_and(|b| b.airborne),
                pairs: pairs[i]
                    .pairs
                    .iter()
                    .map(|p| PairSense {
                        agent: p.agent,
                        word: p.word,
                    })
                    .collect(),
                asleep: pairs[i].asleep,
                teleported,
            },
        });
    }
    let step = st.job.tick(tick, &senses, st.done.take());
    let phantoms = bots::phantom::global();
    for (i, s) in seats.iter().enumerate() {
        let act = step
            .acts
            .iter()
            .find(|a| a.seat == s.seat)
            .map(|a| a.act.clone())
            .unwrap_or_default();
        let b = bodies[i].as_ref();
        if let Some(w) = act.base.word
            && !pairs[i].pairs.is_empty()
        {
            let _ = hk::set_level_words(m, &pairs[i].pairs, w);
        }
        if act.base.stop
            && let Some(b) = b
        {
            stop(m, b);
        }
        if let Some(xy) = act.base.level
            && let Some(b) = b
        {
            level(m, b, xy);
        }
        if let Some(v) = act.velocity
            && let Some(b) = b
        {
            velocity(m, b, v);
        }
        if let Some(h) = s.handle {
            let mut held = pad(act.base.walk, b.and_then(|b| b.yaw));
            held.buttons.set(bots::mind::CROUCH, act.crouch);
            if !phantoms.hold_pad(h, Some(held)) {
                st.job.note(format!(
                    "seat {}: its pad could not be held; its brain steered it",
                    s.seat
                ));
            }
        }
        if let Some((to, facing)) = act.base.teleport {
            let player = agent::sim::state::player_handle(&*m, u32::from(s.player)).ok();
            let submitted = match (&st.calls, player) {
                (Some(q), Some(player)) => calls::submit(
                    q,
                    Call::Teleport {
                        player,
                        pos: Some(to),
                        facing: Some([facing[0], facing[1], 0.0]),
                    },
                ),
                (None, _) => Err("no guest calls queue (agent-calls is off)".into()),
                (_, None) => Err(format!("no player.{}", s.player)),
            };
            match submitted {
                Ok(t) => {
                    st.tickets.insert(s.seat, t);
                }
                Err(e) => {
                    st.refused.insert(s.seat, e);
                }
            }
        }
    }
    st.fork.asked(step.fork);
    quiet(st)
}

/// After the tick returned (the tick observer `sim: bounces fork`): the
/// fork this tick asked for, if any, may be carried out from here on.
/// True when one waits: the caller ends the slice.
pub fn fork_due() -> bool {
    lock().as_mut().is_some_and(|s| s.fork.tick_returned())
}

/// Whether a fork waits for the boot loop ([`between_slices`]): asked,
/// and its tick has returned.
pub fn fork_pending() -> bool {
    lock().as_ref().is_some_and(|s| s.fork.pending())
}

/// What [`between_slices`] did to the machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Forked {
    Nothing,
    Captured,
    /// The machine was put back: the boot loop restores what belongs to
    /// it (the presentation pages, its idle count).
    Restored,
    /// A restore failed part way: the machine is not to run on.
    Broken,
}

/// Why `machine` can't be saved (or loaded) now: its parts' refusals and
/// blockers. The checks with no part (`BootMachine::checks`) are left
/// out: in a fork's scope they refuse a signed-in Live HLE, which a sim
/// always has and a fork can leave alone (no service call is in flight
/// between two ticks of a running match, and nothing outside the machine
/// is rewound).
fn blockers(machine: &BootMachine<'_>, load: bool) -> Option<String> {
    if let Some(why) = machine.parts.iter().find_map(|p| p.refusal()) {
        return Some(why);
    }
    let all: Vec<String> = machine
        .parts
        .iter()
        .flat_map(|p| {
            let name = p.name();
            let reasons = if load {
                p.film_load_blockers()
            } else {
                p.blockers()
            };
            reasons.into_iter().map(move |w| format!("{name}: {w}"))
        })
        .collect();
    (!all.is_empty()).then(|| all.join("; "))
}

fn capture(ring: &mut Ring<Sections>, machine: &mut BootMachine<'_>, key: u64) -> ForkDone {
    if let Some(why) = blockers(machine, false) {
        return ForkDone::Busy(why);
    }
    let Some(rt) = machine.rt else {
        return ForkDone::Failed("this machine has no running guest".into());
    };
    let out = machine.film_snapshot_writer(key);
    let sections = match machine.save_ring_sections(out) {
        Ok(s) => s.into_boxed_slice(),
        Err(e) => return ForkDone::Failed(e),
    };
    // A background thread may have gone live while saving.
    if let Some(why) = blockers(machine, false) {
        return ForkDone::Busy(why);
    }
    let bytes = sections.len();
    match ring.capture(key, &mut rt.memory(), Arc::new(sections), bytes) {
        Ok(true) => ForkDone::Captured { key },
        Ok(false) => ForkDone::Failed("the ring has no room for the entry".into()),
        Err(e) => ForkDone::Failed(format!("{e:#}")),
    }
}

/// `Err`: the machine is part loaded.
fn restore(
    ring: &mut Ring<Sections>,
    machine: &mut BootMachine<'_>,
    key: u64,
    check: bool,
) -> Result<ForkDone, String> {
    let Some(sections) = ring.get(key).cloned() else {
        return Ok(ForkDone::Failed(format!("no entry {key}")));
    };
    if let Some(why) = blockers(machine, true) {
        return Ok(ForkDone::Busy(why));
    }
    let snap = match snapshot::Snapshot::open(&sections, machine.shelf.build()) {
        Ok(s) => s,
        Err(e) => return Ok(ForkDone::Failed(e.to_string())),
    };
    let end_hash = check.then(|| (machine.hash)().total);
    machine.load_ring_sections(&snap, |guest, layout| {
        ring.restore_ram(key, guest, layout)
            .map_err(|e| format!("{e:#}"))
    })?;
    let ram_equal = check.then(|| {
        let h = check_hash(&snap, &(machine.hash)());
        h.checked && h.equal
    });
    Ok(ForkDone::Restored {
        key,
        end_hash,
        ram_equal,
    })
}

/// Between two slices, on the game's thread, with nothing of the game's
/// locked: carries out the fork [`tick`] asked for. The outcome is the
/// next tick's.
pub fn between_slices(machine: &mut BootMachine<'_>) -> Forked {
    let (ask, mut ring) = {
        let mut guard = lock();
        let Some(st) = guard.as_mut() else {
            return Forked::Nothing;
        };
        // Nothing until the tick that asked has returned.
        let Some(ask) = st.fork.take() else {
            return Forked::Nothing;
        };
        let Some(ring) = st.ring.take() else {
            st.done = Some(ForkDone::Failed("the ring is gone".into()));
            return Forked::Nothing;
        };
        (ask, ring)
    };
    // The parts' feature sections call back into the game's features:
    // the state lock is not held.
    let (done, forked) = fork_scope(|| match ask {
        ForkAsk::Capture { key } => {
            let done = capture(&mut ring, machine, key);
            let forked = if matches!(done, ForkDone::Captured { .. }) {
                Forked::Captured
            } else {
                Forked::Nothing
            };
            (done, forked)
        }
        ForkAsk::Restore { key, check } => match restore(&mut ring, machine, key, check) {
            Ok(done) => {
                let forked = if matches!(done, ForkDone::Restored { .. }) {
                    Forked::Restored
                } else {
                    Forked::Nothing
                };
                (done, forked)
            }
            Err(e) => (ForkDone::Failed(e), Forked::Broken),
        },
    });
    let mut guard = lock();
    if let Some(st) = guard.as_mut() {
        st.ring = Some(ring);
        if forked == Forked::Restored {
            // The game's tick is the fork's again, and every teleport of
            // the wave has been answered.
            st.last_tick = None;
            st.tickets.clear();
            st.refused.clear();
        }
        if forked == Forked::Broken
            && let ForkDone::Failed(why) = &done
        {
            // The boot loop stops here: no tick is left to hand it to.
            st.job.stop(format!("fork: restore: {why}"));
        }
        st.done = Some(done);
    }
    forked
}

/// Writes the run's provable sites into its copy of the map's verified
/// file, once. A note for the report, if any.
pub fn save() -> Option<String> {
    let mut guard = lock();
    let st = guard.as_mut()?;
    if st.saved {
        return None;
    }
    st.saved = true;
    let run = st.job.report();
    let report = sim_bench::bounces::summarize(std::slice::from_ref(&run));
    let proven = sim_bench::bounces::proven(&report);
    if proven.is_empty() {
        return None;
    }
    let entries: Vec<superbounce::verified::Entry> = proven
        .iter()
        .map(|p| superbounce::verified::Entry {
            kind: superbounce::Kind::Bounce,
            status: superbounce::Status::Proven,
            at: p.stand,
            offset: p.offset,
            yaw: None,
            height: Some(p.height),
            trials: p.trials,
            ok: p.ok,
            best: p.best,
        })
        .collect();
    let n = entries.len();
    Some(match bots::spots::store_verified(&st.map, &entries) {
        Ok(Some(path)) => format!("bounces: {n} provable sites in {}", path.display()),
        Ok(None) => "bounces: provable sites not saved: no bots data directory".into(),
        Err(e) => format!("bounces: provable sites not saved: {e}"),
    })
}

#[cfg(test)]
mod tests {
    use sim_bench::bounces as b;
    use sim_bench::drops::Proven;

    #[test]
    fn the_verified_text_is_the_games_verified_file() {
        let proven = [
            Proven {
                stand: [-10.54, 4.0, -3.19],
                offset: [0.0, -0.03],
                height: 3.65,
                trials: 1010,
                ok: 400,
                best: 12.3,
                ..Proven::default()
            },
            Proven {
                stand: [1.5, -2.25, 0.01],
                offset: [0.02, 0.0],
                height: 2.4,
                trials: 12,
                ok: 12,
                best: 7.75,
                ..Proven::default()
            },
        ];
        let text = b::verified_text(&proven);
        let entries = superbounce::verified::parse(&text).expect("the game reads it");
        assert_eq!(entries.len(), 2);
        for (e, p) in entries.iter().zip(&proven) {
            assert_eq!(e.kind, superbounce::Kind::Bounce);
            assert_eq!(e.status, superbounce::Status::Proven);
            for a in 0..3 {
                assert!((e.at[a] - p.stand[a]).abs() < 1e-3);
            }
            assert!((e.offset[0] - p.offset[0]).abs() < 1e-3);
            assert!((e.offset[1] - p.offset[1]).abs() < 1e-3);
            assert!((e.height.unwrap() - p.height).abs() < 1e-3);
            assert_eq!((e.trials, e.ok), (p.trials, p.ok));
        }
        assert!(superbounce::verified::parse(&b::verified_text(&[])).unwrap().is_empty());
    }

    /// A slice that ends inside the tick that asked (its time ran out
    /// before the tick returned) leaves the ask waiting; the boot loop
    /// carries it out only after the tick observer has seen the tick
    /// return, and once.
    #[test]
    fn a_fork_waits_for_its_tick_to_return() {
        let ask = b::ForkAsk::Capture { key: 3 };
        let mut f = super::Fork::default();
        assert!(!f.tick_returned());
        f.asked(Some(ask));
        assert!(!f.pending());
        assert_eq!(f.take(), None, "inside the tick");
        assert!(f.tick_returned());
        assert!(f.pending());
        assert_eq!(f.take(), Some(ask));
        assert!(!f.pending());
        assert_eq!(f.take(), None);
        // The next tick's ask starts not due, whatever came before.
        f.asked(Some(ask));
        assert!(f.tick_returned());
        f.asked(None);
        assert!(!f.tick_returned());
        assert_eq!(f.take(), None);
    }

    #[test]
    fn the_ring_holds_every_variant() {
        // An entry's cost is checked against the cap with a whole RAM.
        assert!(super::RING_CAP >= (b::MAX_VARIANTS + 1) * runtime::guest::RAM_SIZE);
    }
}
