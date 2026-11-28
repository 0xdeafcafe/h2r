//! The sound manager's outside entries as Rust bodies over the table
//! (docs/engine/sound-instances.md, "Entries", "The sound phase",
//! "Counters").
//!
//! [`ENTRIES`] is the doc's table: one [`Entry`] per guest function a
//! caller outside the sound code reaches, each with its [`Body`]. Under
//! `impl.sim.sound-events.table` ([`on`]) [`SoundTable`] installs every
//! body, and nothing behind them is reached: the starts read the request
//! record ([`super::request::read`]) and admit into [`Sounds`], the start
//! layer runs its rows ([`start_layer`]), the stop
//! and the stop-all walk end sounds in the table, the chooser entries run
//! [`Chooser`](sim_sound::chooser::Chooser) and [`chooser::pick`], and the create body leaves the pool
//! and channel globals 0, so a guest function that still reads the pool
//! faults with its address.
//!
//! A body whose guest state is not in the table yet refuses by name
//! ([`Body::refusal`]): it returns its empty answer and counts
//! `sound.refused_entry.<address>`. The tick's 30th phase is [`phase`],
//! run by `crate::tick` after the 29.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::Instant;

use engine::entry::{Cx, EntryResult, GuestCall, Ret};
use engine::points::{self, Point};
use engine::sim_sound::clock::system_time;
use engine::sim_sound::units;
use engine::{GuestMemory, MemFault};
use hooks::{BuildError, Builder, Feature, Flow, Manifest, SnapshotPolicy};
use sim_sound::chooser;
use sim_sound::events::TableEvent;
use sim_sound::feed::{NullTableHost, Scene, TableHost};
use sim_sound::impulse::Selection;
use sim_sound::instances::{Handle, Levels, Rejected, Request, Rng, SoundTag, SoundTags, Sounds, SourceParams, TagIndex};
use sim_sound::looping::{LoopError, seconds_to_ticks};
use sim_sound::owners::{NullOwners, Reason};
use sim_sound::schedule::duration_ticks;

use super::guest_tags::{self, Snapshot};
use super::levels::GuestLevels;
use super::owner_host::{GuestUnits, OwnerCounters, UnitCall};
use super::request::RequestError;

pub mod start_layer;

/// The table's sub-switch, without its `impl.` prefix. Off until `h2 sim
/// diff` parity; it runs only under `impl.sim.sound-events`.
pub const TABLE: &str = "sim.sound-events.table";
const ID: &str = "impl.sim.sound-events.table";

/// The table path is on: the events arm's switch and the sub-switch.
pub fn on(switches: &hooks::Switches) -> bool {
    switches.implementation(super::EVENTS, true) && switches.implementation(TABLE, false)
}

const NONE: u32 = u32::MAX;
/// `[0x4e6378]`: the channel array the create body leaves 0.
pub const CHANNELS: u32 = 0x004e_6378;
/// `0x18c6a0(owner, payload, tag, range, permutation, seconds)`: the unit's
/// mouth (`ret 0x18`). It tests the unit itself and reads no payload.
pub const UNIT_MOUTH: u32 = 0x0018_c6a0;
/// `0x126c30(eax = record, tag, &channel, _) -> al`: the start gate. It
/// reads the class state and no pool (`h2 disasm 0x126c30`).
pub const START_GATE: u32 = 0x0012_6c30;
/// `0x126ec0(ecx = tag, &rule) -> eax`: the promotion rule, 0 to start.
pub const START_RULE: u32 = 0x0012_6ec0;
/// The stop's fade (`0x1263e5`: 300 ms at instance `+0xb4`).
pub const STOP_FADE_MS: u32 = 300;
/// Sound globals: sound on (`+0x78`, `+0x79`, `+0x7a`), paused (`+0x7b`),
/// busy (`+0x7d`), the pause time (`+0x80`), listeners (`+0x88`, `0x48`
/// a user, active byte `+6`), the environment (`+0x1f8` to `+0x204`).
mod sg {
    pub const ON: [u32; 3] = [0x78, 0x79, 0x7a];
    pub const PAUSED: u32 = 0x7b;
    pub const BUSY: u32 = 0x7d;
    pub const PAUSE_TIME: u32 = 0x80;
    pub const LISTENERS: u32 = 0x88;
    pub const LISTENER: u32 = 0x48;
    pub const LISTENER_ACTIVE: u32 = 6;
    pub const ENV: u32 = 0x1f8;
    pub const ENV_PREVIOUS: u32 = 0x1fc;
    pub const ENV_NEXT: u32 = 0x200;
    pub const ENV_BLEND: u32 = 0x204;
}
use guest_tags::GESTALT;

/// What one body does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Body {
    /// `0x1253d0`: the guest's create, then the pool and channel globals 0
    /// and a fresh table.
    Create,
    /// `0x125600`: the table dropped, then the guest's dispose.
    Drop,
    /// `0x125b40`: the outbox handed to the host; no sim work.
    Service,
    /// `0x125f10`: nothing in the sim; the clip source decodes on demand.
    Prefetch,
    /// `0x21d110`: a start row for the sound's effect, the gate and rule,
    /// [`super::request::read`] and `admit`, then the row's `Mode::Start`
    /// ([`start_layer::start_record`]).
    StartRecord,
    /// `0x21d390`: the start rows' walk ([`start_layer::starts_update`]).
    StartsUpdate,
    /// `0x21d2c0`: a tag's start ([`start_layer::tag_start`]).
    StartLayerNew,
    /// `0x21d360`: a start's stop ([`start_layer::stop_start`]).
    StartLayerPlay,
    /// `0x21d630`: a start's components in a mode ([`start_layer::run_start`]).
    StartLayerRun,
    /// `0x126360`: `stopping`, then the 300 ms fade.
    Stop,
    /// `0x126400`: every sound finished with [`Reason::Cleared`].
    StopAll,
    /// `0x126660`: the guest's blend of the two background ambience slots
    /// (sound globals `+0x1a8`, `0x1c` bytes each) toward the candidates;
    /// it reads no pool or channel global, and the listener math reads the
    /// slots.
    Ambience,
    /// `0x1272c0`: the environment words of the sound globals.
    Environment,
    /// `0x125360`: the first mapped user's listener.
    FirstListener,
    /// `0x125a90`: the pause flag; the channels are the host's.
    Paused,
    /// `0x125d60`: nothing; the host mixes on its own thread.
    LoadPump,
    /// `0x127400`: the guest's camera math, which reads no pool.
    Listeners,
    /// `0x21a250`: find or start the loop of a tag on an owner, give it the
    /// source and the sequence, or stop it ([`LoopCall`]).
    LoopUpdate,
    /// `0x219c30`: nothing; the owner's listener distances it caches are
    /// read by [`GuestLevels`] at each admission.
    LoopSource,
    /// `0x219070`: [`Chooser::choose`](sim_sound::chooser::Chooser::choose) over the gestalt's rows.
    Choose,
    /// `0x219110`: [`chooser::pick`] over the gestalt's rows.
    Pick,
    /// `0x219430`: the range row's word `+6` when the tag's class sets
    /// flag bit 5, else -1: tag data, no history.
    HistoryLast,
    /// `0x2194a0`: the tag's promotion bank row.
    PromotionBank,
}

impl Body {
    /// Why a body refuses every call, naming the state the table lacks
    /// ([`REFUSING`]).
    pub fn refusal(self) -> Option<&'static str> {
        REFUSING.iter().find(|(b, _)| *b == self).map(|(_, why)| *why)
    }

    /// The body calls the guest function it replaces (pool-free work).
    fn calls_original(self) -> bool {
        matches!(self, Body::Create | Body::Drop | Body::Listeners | Body::Ambience)
    }
}

/// The bodies that refuse every call, with the state the table lacks: none
/// today.
pub static REFUSING: &[(Body, &str)] = &[];

/// One outside entry.
pub struct Entry {
    pub point: &'static Point,
    pub body: Body,
}

const fn e(point: &'static Point, body: Body) -> Entry {
    Entry { point, body }
}

/// The doc's "Entries" table, row by row.
pub static ENTRIES: &[Entry] = &[
    e(&points::SOUND_MANAGER_CREATE, Body::Create),
    e(&points::SOUND_MANAGER_DISPOSE, Body::Drop),
    e(&points::SOUND_UPDATE, Body::Service),
    e(&points::SOUND_PREFETCH, Body::Prefetch),
    e(&points::SOUND_START_RECORD, Body::StartRecord),
    e(&points::SOUND_STARTS_UPDATE, Body::StartsUpdate),
    e(&points::SOUND_START_LAYER_RUN, Body::StartLayerRun),
    e(&points::SOUND_START_LAYER_NEW, Body::StartLayerNew),
    e(&points::SOUND_START_LAYER_PLAY, Body::StartLayerPlay),
    e(&points::SOUND_STOP_INSTANCE, Body::Stop),
    e(&points::SOUND_STOP_ALL, Body::StopAll),
    e(&points::SOUND_AMBIENCE_SELECT, Body::Ambience),
    e(&points::SOUND_ENVIRONMENT_SET, Body::Environment),
    e(&points::SOUND_FIRST_LISTENER, Body::FirstListener),
    e(&points::SOUND_SET_PAUSED, Body::Paused),
    e(&points::SOUND_LOAD_PUMP, Body::LoadPump),
    e(&points::SOUND_LISTENERS_UPDATE, Body::Listeners),
    e(&points::SOUND_LOOP_UPDATE, Body::LoopUpdate),
    e(&points::SOUND_LOOP_SOURCE, Body::LoopSource),
    e(&points::SOUND_CHOOSE, Body::Choose),
    e(&points::SOUND_PICK, Body::Pick),
    e(&points::SOUND_HISTORY_LAST, Body::HistoryLast),
    e(&points::SOUND_PROMOTION_BANK, Body::PromotionBank),
];

/// The body of an address, if the table has one.
pub fn body(addr: u32) -> Option<Body> {
    ENTRIES.iter().find(|e| e.point.base == addr).map(|e| e.body)
}

/// The doc's counters (`sound.*`), read by `sim.ledger`. Not saved.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SoundLedger {
    /// RNG draws the table took (`sound.draws`).
    pub draws: u64,
    pub owner: OwnerCounters,
    /// Our reads of the guest pool: none, the table holds every instance.
    pub pool_reads: u64,
    /// An entry reached with no body: a hooked address [`body`] does not
    /// know (never, while the hooks come from [`ENTRIES`]).
    pub unported_entries: u64,
    pub refused_unknown_owner: u64,
    pub refused_unknown_tag: u64,
    pub refused_unreadable: u64,
    /// A start refused because [`Levels::refused`] names its source.
    pub refused_no_level: u64,
    /// Levels a guest read fault left at the floor (`GuestLevels::faults`).
    pub level_faults: u64,
    /// The admission refused past the slot (`NoScale`, `NoPromotion`,
    /// `Choose`), or a chooser entry could not choose.
    pub refused_choose: u64,
    /// Named refusals ([`Body::refusal`]) by entry address.
    pub refused_entry: BTreeMap<u32, u64>,
    /// Guest calls the bodies make: the original create, dispose and
    /// listener math, the start gate and rule, the pause time.
    pub entry_guest_calls: u64,
    /// Loop starts the full loop pool refused (`sound.loops_full`).
    pub loops_full: u64,
    /// Mouth calls made (`0x18c6a0`), one a tick per running owned sound.
    pub mouth_calls: u64,
    /// Events handed to the host by the service.
    pub service_events: u64,
    /// Runs of the sound phase, and their time on the monotonic clock.
    pub phase_runs: u64,
    pub phase_ns: u64,
}

/// The table, its tag data and levels, its counters and the events
/// waiting for the host.
pub struct Table {
    pub sounds: Sounds,
    /// Fixed tag data and levels (tests); `None` reads them from the map
    /// in guest memory at each call ([`Snapshot`], [`GuestLevels`]).
    tags: Option<Arc<dyn SoundTags + Send + Sync>>,
    levels: Option<Arc<dyn Levels + Send + Sync>>,
    pub ledger: SoundLedger,
    outbox: Vec<TableEvent>,
    last_phase: Option<u32>,
    /// Hears the outbox and the scene once a frame: [`NullTableHost`]
    /// until boot gives it the mixer ([`set_host`]).
    host: Box<dyn TableHost>,
}

pub type Shared = Arc<Mutex<Table>>;

impl Table {
    pub fn new(tags: Arc<dyn SoundTags + Send + Sync>, levels: Arc<dyn Levels + Send + Sync>) -> Self {
        Self { tags: Some(tags), levels: Some(levels), ..Self::guest() }
    }

    /// The map's tags and the guest levels, read from guest memory: the
    /// table boot installs.
    pub fn guest() -> Self {
        Self {
            sounds: Sounds::new(),
            tags: None,
            levels: None,
            ledger: SoundLedger::default(),
            outbox: Vec::new(),
            last_phase: None,
            host: Box::new(NullTableHost),
        }
    }

    /// The host that hears the table: the mixer feed at boot.
    pub fn set_host(&mut self, host: Box<dyn TableHost>) {
        self.host = host;
    }

    fn reset(&mut self) {
        self.sounds = Sounds::new();
        self.outbox.clear();
        self.last_phase = None;
        self.host.clear();
    }

    /// `0x125b40`'s body: the events since the last frame to the host in
    /// the table's order, then the scene. Nothing here changes the table.
    pub fn service(&mut self, tick: u32, ear: Option<sim_sound::feed::Ear>) {
        let events = std::mem::take(&mut self.outbox);
        self.ledger.service_events += events.len() as u64;
        for e in &events {
            self.host.event(e);
        }
        let scene = Scene { tick, ear, sources: self.sounds.sources() };
        self.host.frame(&scene);
    }

    fn refuse(&mut self, addr: u32) {
        *self.ledger.refused_entry.entry(addr).or_default() += 1;
    }

    /// Every counter by its `sound.*` name.
    pub fn counters(&self) -> BTreeMap<String, u64> {
        let c = self.sounds.counters();
        let l = &self.ledger;
        let mut out: BTreeMap<String, u64> = [
            ("sound.admitted", c.admitted),
            ("sound.rejected_silent", c.rejected_silent),
            ("sound.rejected_full", c.rejected_full),
            ("sound.refused_no_length", c.refused_no_length),
            ("sound.draws", l.draws),
            ("sound.owner_guest_writes", l.owner.guest_writes),
            ("sound.pool_reads", l.pool_reads),
            ("sound.unported_entries", l.unported_entries),
            ("sound.refused_unknown_owner", l.refused_unknown_owner),
            ("sound.refused_unknown_tag", l.refused_unknown_tag),
            ("sound.refused_unreadable", l.refused_unreadable),
            ("sound.refused_no_level", l.refused_no_level),
            ("sound.refused_choose", l.refused_choose),
            ("sound.refused_entry", l.refused_entry.values().sum()),
            ("sound.entry_guest_calls", l.entry_guest_calls),
            ("sound.mouth_calls", l.mouth_calls),
            ("sound.loops_full", l.loops_full),
            ("sound.level_faults", l.level_faults),
            ("sound.service_events", l.service_events),
            ("sound.phase_runs", l.phase_runs),
            ("sound.phase_ns", l.phase_ns),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
        for reason in Reason::ALL {
            out.insert(format!("sound.ended.{}", reason.name()), c.ended(reason));
        }
        for (addr, n) in &l.refused_entry {
            out.insert(format!("sound.refused_entry.{addr:#x}"), *n);
        }
        for (name, n) in self.host.counters() {
            out.insert(name.to_owned(), n);
        }
        out
    }
}

/// The process's table: the one the hooks and the tick's phase share.
pub static SHARED: LazyLock<Shared> = LazyLock::new(|| Arc::new(Mutex::new(Table::guest())));

pub fn lock(t: &Mutex<Table>) -> MutexGuard<'_, Table> {
    t.lock().unwrap_or_else(|e| e.into_inner())
}

/// Give the process's table its host: boot's mixer feed, once the audio
/// is up.
pub fn set_host(host: Box<dyn TableHost>) {
    lock(&SHARED).set_host(host);
}

/// The process table's counters, for `sim.ledger`. Empty with the table
/// off and no run.
pub fn counters() -> BTreeMap<String, u64> {
    let t = lock(&SHARED);
    if t.ledger == SoundLedger::default() && *t.sounds.counters() == Default::default() {
        return BTreeMap::new();
    }
    t.counters()
}

fn tick_of(m: &dyn GuestMemory) -> Result<u32, MemFault> {
    m.u32(m.u32(points::GAME_TIME.base)?.wrapping_add(8))
}

/// The game's seed word: `[RANDOM_SEEDS] + 4`.
fn seed_at(m: &dyn GuestMemory) -> Result<u32, MemFault> {
    Ok(m.u32(points::RANDOM_SEEDS.base)?.wrapping_add(4))
}

/// Draws from `before` to `after` (one step each), at most 1024.
fn draws_between(before: u32, after: u32) -> u64 {
    let mut rng = Rng::new(before);
    for n in 0..1024u64 {
        if rng.state() == after {
            return n;
        }
        rng.next_u32();
    }
    1024
}

fn sound_on(m: &dyn GuestMemory) -> bool {
    m.u32(points::SOUND_GLOBALS.base)
        .ok()
        .filter(|&g| g != 0)
        .is_some_and(|g| sg::ON.iter().all(|&o| m.u8(g.wrapping_add(o)).is_ok_and(|b| b != 0)))
}

/// An object's datum no longer names a live object (`[OBJECTS]`: count
/// `+0x38`, size `+0x24`, data `+0x44`, the salt at `+0`).
fn object_gone(m: &dyn GuestMemory, datum: u32) -> bool {
    let live = || -> Result<bool, MemFault> {
        let array = m.u32(points::OBJECTS.base)?;
        let index = datum & 0xffff;
        if index >= m.u32(array.wrapping_add(0x38))? {
            return Ok(false);
        }
        let at = m.u32(array.wrapping_add(0x24))?.wrapping_mul(index).wrapping_add(m.u32(array.wrapping_add(0x44))?);
        let salt = m.u16(at)?;
        Ok(salt != 0 && u32::from(salt) == datum >> 16)
    };
    !live().unwrap_or(false)
}

/// The permutation row a selection names (`0x18c6a0`'s fifth argument).
fn permutation_at(m: &dyn GuestMemory, tag: TagIndex, selection: Selection) -> Result<u32, MemFault> {
    let def = guest_tags::definition(m, tag.0)?;
    let gestalt = m.u32(GESTALT)?;
    let range = i32::from(m.u16(def.wrapping_add(8))? as i16) + i32::from(selection.range);
    let row = m.u32(gestalt.wrapping_add(0x24))?.wrapping_add(12u32.wrapping_mul(range as u32));
    let permutation = i32::from(m.u16(row.wrapping_add(8))? as i16) + i32::from(selection.permutation);
    Ok(m.u32(gestalt.wrapping_add(0x2c))?.wrapping_add(16u32.wrapping_mul(permutation as u32)))
}

/// Make the unit calls the step queued, in its order: a line's end
/// through `0x18c8c0`'s gate and unit test ([`units::line_end`]), a mouth
/// through `0x18c6a0`.
async fn unit_calls(cx: &Cx, table: &Shared, calls: Vec<UnitCall>) -> Result<(), engine::entry::EntryError> {
    for call in calls {
        match call {
            UnitCall::LineEnded { object, tag } => {
                if cx.write(|m| units::line_end(m, object, tag.0)).is_err() {
                    lock(table).ledger.refused_unreadable += 1;
                }
            }
            UnitCall::Mouth { object, tag, selection, ticks } => {
                let Ok(permutation) = cx.read(|m| permutation_at(m, tag, selection)) else {
                    lock(table).ledger.refused_unreadable += 1;
                    continue;
                };
                let seconds = super::owners::seconds(ticks).to_bits();
                let range = i32::from(selection.range) as u32;
                cx.call(GuestCall::new(UNIT_MOUTH).args([object, 0, tag.0, range, permutation, seconds])).await?;
                lock(table).ledger.mouth_calls += 1;
            }
        }
    }
    Ok(())
}

/// The finishes of a step or walk inside guest memory: the host reads the
/// gone objects, the table steps, and the queued unit calls come back.
fn with_units<T>(
    cx: &Cx,
    table: &Shared,
    f: impl FnOnce(&mut Table, &mut GuestUnits<'_>, u32) -> T,
) -> Result<(T, Vec<UnitCall>), MemFault> {
    cx.write(|m| {
        let tick = tick_of(m)?;
        let mut t = lock(table);
        let reads: &dyn GuestMemory = &*m;
        let gone = t
            .sounds
            .instances()
            .iter()
            .filter_map(|(_, i)| i.owner.unit())
            .filter(|&o| object_gone(reads, o))
            .collect();
        let mut host = GuestUnits::new(m);
        host.gone = gone;
        let out = f(&mut t, &mut host, tick);
        let (calls, counters) = (std::mem::take(&mut host.calls), host.counters);
        t.ledger.owner.guest_writes += counters.guest_writes;
        Ok((out, calls))
    })
}

/// The sound phase, the tick's 30th: once per tick number, `step_loops`
/// then `step` in slot order, the events to the outbox, then the unit
/// calls the step queued.
pub async fn phase(cx: &Cx, table: &Shared) -> EntryResult {
    let started = Instant::now();
    let tick = cx.read(|m| tick_of(m))?;
    {
        let mut t = lock(table);
        if t.last_phase == Some(tick) {
            return Ok(Ret::new(0, 0));
        }
        t.last_phase = Some(tick);
    }
    let (_, calls) = with_units(cx, table, |t, host, tick| t.step_phase(host, tick))?;
    unit_calls(cx, table, calls).await?;
    start_layer::drain_stops(cx, table).await?;
    let mut t = lock(table);
    t.ledger.phase_runs += 1;
    t.ledger.phase_ns += started.elapsed().as_nanos() as u64;
    Ok(Ret::new(0, 0))
}

impl Table {
    /// `step_loops`, then `step`, with the guest's seed word as the RNG.
    fn step_phase(&mut self, host: &mut GuestUnits<'_>, tick: u32) {
        let mem: &dyn GuestMemory = host.memory();
        let Ok((at, before)) = seed_at(mem).and_then(|at| Ok((at, mem.u32(at)?))) else {
            self.ledger.refused_unreadable += 1;
            return;
        };
        let mut rng = Rng::new(before);
        // A guest table's tags: every live loop's tag and its sounds.
        let mut loops = Snapshot::default();
        if self.tags.is_none() {
            let playing: Vec<TagIndex> = self.sounds.loops().iter().map(|(_, l)| l.tag).collect();
            for tag in playing {
                if loops.read_loop(mem, tag).is_err() {
                    self.ledger.refused_unreadable += 1;
                }
            }
        }
        let (fixed_tags, fixed_levels) = (self.tags.clone(), self.levels.clone());
        let tags: &dyn SoundTags = match &fixed_tags {
            Some(t) => &**t,
            None => &loops,
        };
        let guest_levels = GuestLevels::new(mem);
        let levels: &dyn Levels = match &fixed_levels {
            Some(l) => &**l,
            None => &guest_levels,
        };
        if levels.refused().is_none() {
            self.sounds.free_unrefreshed_loops();
            self.sounds.step_loops(&mut rng, tags, levels, tick);
        }
        self.ledger.level_faults += guest_levels.faults.get();
        let events = self.sounds.step(tick, host);
        self.outbox.extend(events);
        if rng.state() != before {
            self.ledger.draws += draws_between(before, rng.state());
            let _ = host.memory().set_u32(at, rng.state());
        }
    }

    /// Admit a read request on `tick` with the guest's seed.
    fn admit(&mut self, m: &mut dyn GuestMemory, request: Result<Request, RequestError>, tick: u32) -> u32 {
        let mut request = match request {
            Ok(r) => r,
            Err(RequestError::UnknownOwner { .. }) => {
                self.ledger.refused_unknown_owner += 1;
                return NONE;
            }
            Err(RequestError::Unreadable(_)) => {
                self.ledger.refused_unreadable += 1;
                return NONE;
            }
        };
        let snapshot = match &self.tags {
            Some(_) => None,
            None => match Snapshot::of_tag(&*m, request.tag, request.rule) {
                Ok(s) => Some(s),
                Err(_) => {
                    self.ledger.refused_unreadable += 1;
                    return NONE;
                }
            },
        };
        let fixed = self.tags.clone();
        let tags: &dyn SoundTags = match (&fixed, &snapshot) {
            (Some(t), _) => &**t,
            (None, Some(s)) => s,
            (None, None) => unreachable!("read above"),
        };
        if tags.sound(request.tag).is_none() {
            self.ledger.refused_unknown_tag += 1;
            return NONE;
        }
        request.level = match &self.levels {
            Some(l) if l.refused().is_some() => {
                self.ledger.refused_no_level += 1;
                return NONE;
            }
            Some(l) => l.level(request.tag, &request.source),
            None => {
                let l = GuestLevels::new(&*m);
                let level = l.level(request.tag, &request.source);
                self.ledger.level_faults += l.faults.get();
                level
            }
        };
        let Ok(at) = seed_at(m) else {
            self.ledger.refused_unreadable += 1;
            return NONE;
        };
        let Ok(before) = m.u32(at) else {
            self.ledger.refused_unreadable += 1;
            return NONE;
        };
        let mut rng = Rng::new(before);
        let result = self.sounds.admit(&mut rng, tags, request, tick);
        self.ledger.draws += draws_between(before, rng.state());
        let _ = m.set_u32(at, rng.state());
        match result {
            Ok(h) => h.raw(),
            Err(Rejected::Silent | Rejected::Full) => NONE,
            Err(Rejected::UnknownTag(_)) => {
                self.ledger.refused_unknown_tag += 1;
                NONE
            }
            Err(Rejected::NoScale(_) | Rejected::NoPromotion { .. } | Rejected::Choose(_)) => {
                self.ledger.refused_choose += 1;
                NONE
            }
        }
    }
}

/// `0x21a250`'s arguments: the tag, the owner key, the source record, the
/// mode (0 start, 1 keep, 2 stop), the flags (bit 0 the alternate) and the
/// stop's fade. Arguments 1 and 3 (record `+2`, the owner row's key) key
/// retail's per-owner rows, which the table does not keep.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoopCall {
    pub tag: TagIndex,
    pub owner: u32,
    pub source: SourceParams,
    pub mode: u32,
    pub flags: u32,
    /// Seconds; 0 stops with the tracks' outs.
    pub fade: f32,
}

impl LoopCall {
    /// The call's eight stack words, the source read from its record.
    pub fn read(m: &dyn GuestMemory, args: [u32; 8]) -> Result<Self, MemFault> {
        let mut source = SourceParams::default();
        if args[0] != NONE {
            for (i, word) in source.0.iter_mut().enumerate() {
                *word = m.u32(args[4].wrapping_add(4 * i as u32))?;
            }
        }
        Ok(Self { tag: TagIndex(args[0]), owner: args[2], source, mode: args[5], flags: args[6], fade: f32::from_bits(args[7]) })
    }

    /// The source's spatial type (word 0's byte 3, low half): a shared loop
    /// keys on its tag only with one.
    fn spatial(&self) -> bool {
        (self.source.0[0] >> 24) & 0xf != 0
    }

    /// Word 0 bit 1: the loop keeps the source it has.
    fn keeps_source(&self) -> bool {
        self.source.0[0] & 2 != 0
    }
}

impl Table {
    /// `0x21a250` over the table: 1 when the caller is done with the loop
    /// (no tag, or a stop with nothing left to stop), else 0.
    ///
    /// | Mode | Found | Does |
    /// |---|---|---|
    /// | 0, 1 | no | `loop_start` on the tag with flag bit 0 as the alternate |
    /// | 0, 1 | playing | the source (unless the source keeps its own), then `loop_set_alternate` |
    /// | 2 | no | 1 |
    /// | 2 | holds no sound | freed at once, 1 |
    /// | 2 | playing | `loop_stop` with a fade of 0, else `loop_fade_out` over it |
    /// | 2 | stopping | nothing |
    ///
    /// A loop of a tag with flag bit 7 and a source with a spatial type is
    /// keyed `tag | 0x6000`, one for every owner.
    pub fn loop_update(&mut self, m: &mut dyn GuestMemory, call: LoopCall, tick: u32) -> u32 {
        if call.tag.0 == NONE {
            return 1;
        }
        let Ok(at) = seed_at(m) else {
            self.ledger.refused_unreadable += 1;
            return 0;
        };
        let Ok(before) = m.u32(at) else {
            self.ledger.refused_unreadable += 1;
            return 0;
        };
        let snapshot = match &self.tags {
            Some(_) => None,
            None => match Snapshot::of_loop(&*m, call.tag) {
                Ok(s) => Some(s),
                Err(_) => {
                    self.ledger.refused_unreadable += 1;
                    return 0;
                }
            },
        };
        let shared = match guest_tags::loop_shared(&*m, call.tag) {
            Ok(shared) => shared && call.spatial(),
            Err(_) if self.tags.is_some() => false,
            Err(_) => {
                self.ledger.refused_unreadable += 1;
                return 0;
            }
        };
        let owner = if shared { call.tag.0 | 0x6000 } else { call.owner };
        let mut rng = Rng::new(before);
        let done = {
            let reads: &dyn GuestMemory = &*m;
            let (fixed_tags, fixed_levels) = (self.tags.clone(), self.levels.clone());
            let tags: &dyn SoundTags = match (&fixed_tags, &snapshot) {
                (Some(t), _) => &**t,
                (None, Some(s)) => s,
                (None, None) => unreachable!("read above"),
            };
            let guest_levels = GuestLevels::new(reads);
            let levels: &dyn Levels = match &fixed_levels {
                Some(l) => &**l,
                None => &guest_levels,
            };
            let done = self.loop_call(&mut rng, tags, levels, call, owner, tick);
            self.ledger.level_faults += guest_levels.faults.get();
            done
        };
        if rng.state() != before {
            self.ledger.draws += draws_between(before, rng.state());
            let _ = m.set_u32(at, rng.state());
        }
        done
    }

    fn loop_call(&mut self, rng: &mut Rng, tags: &dyn SoundTags, levels: &dyn Levels, call: LoopCall, owner: u32, tick: u32) -> u32 {
        let found = self.sounds.loop_find(call.tag, owner);
        let stopping = found.and_then(|h| self.sounds.loops().get(h)).is_some_and(|l| l.stopping);
        // Loop sounds are the table's own (`Owner::Loop`): no host is told.
        let owners = &mut NullOwners;
        match (call.mode, found) {
            (2.., None) => 1,
            (2, Some(_)) if stopping => 0,
            (2, Some(h)) => {
                if self.sounds.loop_free_silent(h) {
                    return 1;
                }
                if call.fade == 0.0 {
                    self.sounds.loop_stop(rng, tags, levels, h, tick, owners);
                } else {
                    self.sounds.loop_fade_out(h, tick, seconds_to_ticks(call.fade), owners);
                }
                0
            }
            (_, Some(h)) if !stopping => {
                self.sounds.loop_touch(h, call.flags & 2 != 0);
                if !call.keeps_source() {
                    self.sounds.loop_set_source(h, call.source);
                }
                self.sounds.loop_set_alternate(rng, tags, levels, h, call.flags & 1 != 0, tick, owners);
                0
            }
            (_, _) => {
                if levels.refused().is_some() {
                    self.ledger.refused_no_level += 1;
                    return 0;
                }
                match self.sounds.loop_start(rng, tags, levels, call.tag, owner, call.source, call.flags & 1 != 0, tick) {
                    Ok(h) => self.sounds.loop_touch(h, call.flags & 2 != 0),
                    Err(LoopError::UnknownTag(_)) => self.ledger.refused_unknown_tag += 1,
                    Err(LoopError::TooManyTracks(_)) => self.ledger.refused_unreadable += 1,
                    Err(LoopError::Full) => self.ledger.loops_full += 1,
                }
                0
            }
        }
    }
}

async fn choose(cx: &Cx, table: &Shared) -> EntryResult {
    let entry = cx.entry();
    let tag = entry.edi;
    let range = cx.arg(0)? as u16 as i16;
    let r = cx.write(|m| -> Result<Result<i32, ()>, MemFault> {
        let sound = SoundTag {
            first_range: m.u16(tag.wrapping_add(8))? as i16,
            range_count: m.u8(tag.wrapping_add(0xa))? as i8,
            class_flags: guest_tags::class_flags(m, tag)?,
            ..SoundTag::default()
        };
        let mut rows = Snapshot::default();
        rows.read_range(m, i32::from(sound.first_range) + i32::from(range))?;
        let at = seed_at(m)?;
        let before = m.u32(at)?;
        let mut rng = Rng::new(before);
        let mut t = lock(table);
        let chosen = t.sounds.chooser_mut().choose(&mut rng, &rows, &sound, range);
        t.ledger.draws += draws_between(before, rng.state());
        m.set_u32(at, rng.state())?;
        Ok(chosen.map_err(|_| ()))
    });
    let permutation = match r {
        Ok(Ok(p)) => p as u32 & 0xffff,
        _ => {
            lock(table).ledger.refused_choose += 1;
            0xffff
        }
    };
    Ok(Ret::new(permutation, 4))
}

async fn pick(cx: &Cx, table: &Shared) -> EntryResult {
    let entry = cx.entry();
    let (tag, range, used_at) = (entry.eax, entry.ecx as u16 as i16, entry.esi);
    let [last, seed, exhausted, invert] = [cx.arg(0)?, cx.arg(1)?, cx.arg(2)?, cx.arg(3)?];
    let r = cx.write(|m| -> Result<Result<chooser::Pick, ()>, MemFault> {
        let absolute = i32::from(m.u16(tag.wrapping_add(8))? as i16) + i32::from(range);
        let mut rows = Snapshot::default();
        rows.read_range(m, absolute)?;
        let row = rows.ranges[&(absolute as u16)];
        let used = m.u32(used_at)?;
        let before = m.u32(seed)?;
        let mut rng = Rng::new(before);
        let picked = chooser::pick(&mut rng, &rows, &row, absolute, i32::from(last as u16 as i16), used, invert as u8 != 0);
        lock(table).ledger.draws += draws_between(before, rng.state());
        m.set_u32(seed, rng.state())?;
        if let Ok(p) = picked {
            m.set_u32(used_at, p.used)?;
            if exhausted != 0 {
                m.set_u8(exhausted, u8::from(p.exhausted))?;
            }
        }
        Ok(picked.map_err(|_| ()))
    });
    let index = match r {
        Ok(Ok(p)) => p.index as u32,
        _ => {
            lock(table).ledger.refused_choose += 1;
            NONE
        }
    };
    Ok(Ret::new(index, 16))
}

async fn stop(cx: &Cx, table: &Shared) -> EntryResult {
    let handle = cx.entry().ebx;
    if handle != NONE {
        let fade = duration_ticks(STOP_FADE_MS);
        with_units(cx, table, |t, host, tick| t.sounds.stop(Handle::from_raw(handle), tick, fade, host))?;
        start_layer::drain_stops(cx, table).await?;
    }
    Ok(Ret::new(0, 0))
}

async fn stop_all(cx: &Cx, table: &Shared) -> EntryResult {
    let on = cx.read(|m| m.u32(points::SOUND_GLOBALS.base).and_then(|g| m.u8(g.wrapping_add(sg::ON[0])))).unwrap_or(0);
    if on != 0 {
        let (events, calls) = with_units(cx, table, |t, host, tick| t.sounds.finish_all(Reason::Cleared, tick, host))?;
        lock(table).outbox.extend(events);
        unit_calls(cx, table, calls).await?;
    }
    Ok(Ret::new(0, 0))
}

/// `0x1272c0`: a new environment becomes current; the one it replaces is
/// kept as previous with the blend at 0. Asking for the queued one makes
/// it current at once.
fn environment(m: &mut dyn GuestMemory, environment: u32) -> Result<(), MemFault> {
    let g = m.u32(points::SOUND_GLOBALS.base)?;
    let current = m.u32(g.wrapping_add(sg::ENV))?;
    if environment == current {
        return Ok(());
    }
    if environment != NONE && environment == m.u32(g.wrapping_add(sg::ENV_NEXT))? {
        let blend = m.u32(g.wrapping_add(sg::ENV_BLEND))?;
        m.set_u32(g.wrapping_add(sg::ENV), environment)?;
        m.set_u32(g.wrapping_add(sg::ENV_PREVIOUS), blend)?;
        m.set_f32(g.wrapping_add(sg::ENV_BLEND), 0.0)?;
        return Ok(());
    }
    m.set_u32(g.wrapping_add(sg::ENV_NEXT), current)?;
    m.set_u32(g.wrapping_add(sg::ENV), environment)?;
    m.set_f32(g.wrapping_add(sg::ENV_PREVIOUS), 0.0)?;
    m.set_f32(g.wrapping_add(sg::ENV_BLEND), 0.0)
}

/// `0x125360`: the listener of the first mapped local user (`[0x4e8c20] +
/// 0xc`, four slots), or 0 when none is mapped or it is inactive.
fn first_listener(m: &dyn GuestMemory) -> Result<u32, MemFault> {
    let users = m.u32(points::PLAYER_GLOBALS.base)?.wrapping_add(0xc);
    for user in 0..4u32 {
        if m.u32(users.wrapping_add(4 * user))? != NONE {
            let g = m.u32(points::SOUND_GLOBALS.base)?;
            let listener = g.wrapping_add(sg::LISTENERS).wrapping_add(sg::LISTENER * user);
            return Ok(if m.u8(listener.wrapping_add(sg::LISTENER_ACTIVE))? != 0 { listener } else { 0 });
        }
    }
    Ok(0)
}

/// `0x125a90`: the paused byte and, on a pause, the time it began
/// (`0x3314b0`, [`system_time`]). The channel calls are the host's and are
/// not made.
fn paused(cx: &Cx) -> EntryResult {
    let paused = cx.entry().eax;
    let g = cx.read(|m| m.u32(points::SOUND_GLOBALS.base))?;
    let flag = u8::from(paused == 0);
    if cx.read(|m| m.u8(g.wrapping_add(sg::PAUSED)))? == flag {
        return Ok(Ret::new(0, 0));
    }
    cx.write(|m| {
        m.set_u8(g.wrapping_add(sg::BUSY), 1)?;
        m.set_u8(g.wrapping_add(sg::PAUSED), flag)
    })?;
    if paused == 1 {
        cx.write(|m| {
            let now = system_time(&*m)?;
            m.set_u32(g.wrapping_add(sg::PAUSE_TIME), now)
        })?;
    }
    cx.write(|m| m.set_u8(g.wrapping_add(sg::BUSY), 0))?;
    Ok(Ret::new(0, 0))
}

/// `0x219430`: with the tag's class flag bit 5, the range row's `u16` at
/// `+6` (`0xffff` as -1); else -1.
fn range_word(m: &dyn GuestMemory, range: u32, tag: u32) -> Result<u32, MemFault> {
    if guest_tags::class_flags(m, tag)? & (1 << 5) == 0 {
        return Ok(NONE);
    }
    Ok(match m.u16(range.wrapping_add(6))? {
        0xffff => NONE,
        w => u32::from(w),
    })
}

/// The entry's empty answer: its return value and argument bytes.
fn empty(point: &Point) -> Ret {
    let (ret, pop) = match point.sig {
        points::Sig::Fn { ret, pop, .. } => (ret, pop),
        _ => (None, 0),
    };
    Ret::new(if ret.is_some() { if matches!(ret, Some(points::Loc::Al)) { 0 } else { NONE } } else { 0 }, pop)
}

/// The guest function a body replaces, run with the hook let through: its
/// registers and stack arguments passed on as they came.
async fn original(cx: &Cx, table: &Shared, entry: &Entry, busy: &AtomicBool) -> EntryResult {
    let pop = match entry.point.sig {
        points::Sig::Fn { pop, .. } => u32::from(pop),
        _ => 0,
    };
    let mut args = Vec::new();
    for n in 0..pop / 4 {
        args.push(cx.arg(n)?);
    }
    busy.store(true, Ordering::SeqCst);
    let e = cx.entry();
    let call = GuestCall::new(entry.point.base).eax(e.eax).ecx(e.ecx).edx(e.edx).ebx(e.ebx).esi(e.esi).edi(e.edi);
    let r = cx.call(call.args(args)).await;
    busy.store(false, Ordering::SeqCst);
    lock(table).ledger.entry_guest_calls += 1;
    r.map(|regs| Ret::new(regs.eax, 0))
}

/// One entry's body.
pub async fn run(cx: Cx, table: Shared, entry: &'static Entry, busy: Arc<AtomicBool>) -> EntryResult {
    if entry.body.refusal().is_some() {
        lock(&table).refuse(entry.point.base);
        return Ok(empty(entry.point));
    }
    match entry.body {
        Body::Create => {
            original(&cx, &table, entry, &busy).await?;
            cx.write(|m| {
                m.set_u32(points::SOUND_INSTANCES.base, 0)?;
                m.set_u32(CHANNELS, 0)
            })?;
            lock(&table).reset();
            Ok(Ret::new(0, 0))
        }
        Body::Drop => {
            lock(&table).reset();
            original(&cx, &table, entry, &busy).await?;
            Ok(Ret::new(0, 0))
        }
        Body::Listeners => {
            original(&cx, &table, entry, &busy).await?;
            Ok(Ret::new(0, 4))
        }
        Body::Ambience => {
            original(&cx, &table, entry, &busy).await?;
            Ok(Ret::new(0, 0))
        }
        Body::Service => {
            let (tick, ear) = cx.read(|m| (tick_of(m).unwrap_or(0), super::scene::ear(m).unwrap_or(None)));
            lock(&table).service(tick, ear);
            Ok(Ret::new(0, 0))
        }
        Body::Prefetch | Body::LoadPump => Ok(Ret::new(0, 0)),
        Body::StartRecord => Ok(Ret::new(start_layer::start_record(&cx, &table).await?, 4)),
        Body::StartLayerNew => Ok(Ret::new(start_layer::tag_start(&cx, &table).await?, 4)),
        Body::StartLayerPlay => {
            let start = Handle::from_raw(cx.entry().ecx);
            start_layer::stop_start(&cx, &table, start).await?;
            Ok(Ret::new(0, 0))
        }
        Body::StartLayerRun => {
            let (start, mode) = (Handle::from_raw(cx.arg(0)?), cx.arg(1)?);
            start_layer::run_entry(&cx, &table, start, mode).await?;
            Ok(Ret::new(0, 8))
        }
        Body::StartsUpdate => {
            start_layer::starts_update(&cx, &table).await?;
            Ok(Ret::new(0, 0))
        }
        Body::Stop => stop(&cx, &table).await,
        Body::StopAll => stop_all(&cx, &table).await,
        Body::Environment => {
            let ecx = cx.entry().ecx;
            cx.write(|m| environment(m, ecx))?;
            Ok(Ret::new(0, 0))
        }
        Body::FirstListener => Ok(Ret::new(cx.read(first_listener)?, 0)),
        Body::Paused => paused(&cx),
        Body::Choose => choose(&cx, &table).await,
        Body::Pick => pick(&cx, &table).await,
        Body::PromotionBank => {
            let tag = cx.entry().eax;
            Ok(Ret::new(cx.read(|m| engine::sim_sound::playback::bank_row(m, tag))?, 0))
        }
        Body::LoopUpdate => {
            let args = [cx.arg(0)?, cx.arg(1)?, cx.arg(2)?, cx.arg(3)?, cx.arg(4)?, cx.arg(5)?, cx.arg(6)?, cx.arg(7)?];
            let done = cx.write(|m| {
                let tick = tick_of(m).unwrap_or(0);
                let mut t = lock(&table);
                match LoopCall::read(&*m, args) {
                    Ok(call) => t.loop_update(m, call, tick),
                    Err(_) => {
                        t.ledger.refused_unreadable += 1;
                        0
                    }
                }
            });
            Ok(Ret::new(done, 32))
        }
        Body::LoopSource => Ok(Ret::new(0, 4)),
        Body::HistoryLast => {
            let (range, tag) = (cx.arg(0)?, cx.arg(1)?);
            Ok(Ret::new(cx.read(|m| range_word(m, range, tag))?, 8))
        }
    }
}

/// The table's hooks (`impl.sim.sound-events.table`): every entry in
/// [`ENTRIES`]. A body that calls its original is a `before` that answers
/// and lets the inner call through; the rest are implementations. The
/// tick's phase is `crate::tick`'s.
pub struct SoundTable {
    table: Shared,
}

impl SoundTable {
    pub fn new(table: Shared) -> Self {
        Self { table }
    }

    /// The process's table ([`SHARED`]), the one the tick's phase steps.
    pub fn process() -> Self {
        Self::new(Arc::clone(&SHARED))
    }
}

impl Feature for SoundTable {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "Sound table: the sound entries over the Rust table").doc("docs/engine/sound-instances.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        // The table is not saved yet: a snapshot restores no sound.
        b.snapshot(SnapshotPolicy::Stateless);
        if !on(b.switches()) {
            return Ok(());
        }
        for entry in ENTRIES {
            let table = Arc::clone(&self.table);
            let busy = Arc::new(AtomicBool::new(false));
            if entry.body.calls_original() {
                let hook = b.raw().before(entry.point, "sound table", move |call| {
                    if busy.load(Ordering::SeqCst) {
                        return Ok(Flow::Continue);
                    }
                    let (table, busy) = (Arc::clone(&table), Arc::clone(&busy));
                    call.answer_async(move |cx| Box::pin(run(cx, table, entry, busy)))
                });
                // First on the listeners: the bots' map swap runs around
                // the inner call, which reaches the camera math.
                if entry.body == Body::Listeners {
                    hook.first();
                }
            } else {
                b.raw().implement_async(entry.point, "sound table", move |cx| {
                    Box::pin(run(cx, Arc::clone(&table), entry, Arc::clone(&busy)))
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
