//! The sound manager's outside entries as Rust bodies over the table
//! (docs/engine/sound-instances.md, "Entries", "The sound phase",
//! "Counters").
//!
//! [`ENTRIES`] is the doc's table: one [`Entry`] per guest function a
//! caller outside the sound code reaches, each with its [`Body`]. Under
//! `impl.sim.sound-events.table` ([`on`]) [`SoundTable`] installs every
//! body, and nothing behind them is reached: the starts read the request
//! record ([`super::request::read`]) and admit into [`Sounds`], the stop
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
use engine::{GuestMemory, MemFault};
use hooks::{BuildError, Builder, Feature, Flow, Manifest, SnapshotPolicy};
use sim_sound::chooser;
use sim_sound::events::TableEvent;
use sim_sound::instances::{
    Handle, Levels, Permutation, PitchRange, Rejected, Rng, SoundTag, SoundTags, Sounds, SourceParams, TagIndex,
    TagTable,
};
use sim_sound::owners::Reason;
use sim_sound::schedule::duration_ticks;

use super::owner_host::{GuestUnits, OwnerCounters, UnitCall};
use super::request::{self, RequestError};

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
/// `[0x4ed28c] + 0x29`: dialogue is on (`0x18c8c0`'s gate).
pub const DIALOGUE: u32 = 0x004e_d28c;
pub const DIALOGUE_ON: u32 = 0x29;
/// `0xbadc0(eax = object, 3)`: the object is a unit.
pub const OBJECT_IS: u32 = 0x000b_adc0;
/// `0xd0dc0(eax = unit, tag)`: the unit's line ended.
pub const UNIT_LINE_ENDED: u32 = 0x000d_0dc0;
/// `0x126c30(eax = record, tag, &channel, _) -> al`: the start gate. It
/// reads the class state and no pool (`h2 disasm 0x126c30`).
pub const START_GATE: u32 = 0x0012_6c30;
/// `0x126ec0(ecx = tag, &rule) -> eax`: the promotion rule, 0 to start.
pub const START_RULE: u32 = 0x0012_6ec0;
/// `0x3314b0`: the system time the pause records at sound globals `+0x80`.
pub const SYSTEM_TIME: u32 = 0x0033_14b0;
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
/// `[0x51ebd4]`: the sound gestalt. Ranges at `+0x24` (12 bytes: first
/// permutation `+8`, count `+0xa`), permutations at `+0x2c` (16 bytes:
/// skip `+2`), promotion banks at `+0x34` (`0x34` bytes).
pub const GESTALT: u32 = 0x0051_ebd4;

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
    /// `0x21d110`: the gate and rule, then [`request::read`] and `admit`.
    StartRecord,
    StartsUpdate,
    StartLayerNew,
    StartLayerPlay,
    StartLayerRun,
    /// `0x126360`: `stopping`, then the 300 ms fade.
    Stop,
    /// `0x126400`: every sound finished with [`Reason::Cleared`].
    StopAll,
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
    LoopUpdate,
    LoopSource,
    /// `0x219070`: [`Chooser::choose`](sim_sound::chooser::Chooser::choose) over the gestalt's rows.
    Choose,
    /// `0x219110`: [`chooser::pick`] over the gestalt's rows.
    Pick,
    HistoryLast,
    /// `0x2194a0`: the tag's promotion bank row.
    PromotionBank,
}

impl Body {
    /// Why a body refuses every call, naming the state the table lacks.
    pub fn refusal(self) -> Option<&'static str> {
        match self {
            Body::StartsUpdate | Body::StartLayerNew | Body::StartLayerPlay | Body::StartLayerRun => {
                Some("the start layer's records ([0x51ebe0]) are not in the table")
            }
            Body::Ambience => Some("the background ambience (sound globals +0x1ac) is not in the table"),
            Body::LoopUpdate | Body::LoopSource => Some("the looping-sound entries' arguments are not read"),
            Body::HistoryLast => Some("the range word +6 it returns is not read"),
            _ => None,
        }
    }

    /// The body calls the guest function it replaces (pool-free work).
    fn calls_original(self) -> bool {
        matches!(self, Body::Create | Body::Drop | Body::Listeners)
    }
}

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

/// Levels before the guest port lands: every start is refused by the
/// function the levels come from.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnportedLevels;

impl Levels for UnportedLevels {
    fn level(&self, _: TagIndex, _: &SourceParams) -> f32 {
        f32::NAN
    }
    fn refused(&self) -> Option<&'static str> {
        Some("0x1251e0")
    }
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
    /// The admission refused past the slot (`NoScale`, `NoPromotion`,
    /// `Choose`), or a chooser entry could not choose.
    pub refused_choose: u64,
    /// Named refusals ([`Body::refusal`]) by entry address.
    pub refused_entry: BTreeMap<u32, u64>,
    /// Guest calls the bodies make: the original create, dispose and
    /// listener math, the start gate and rule, the pause time.
    pub entry_guest_calls: u64,
    /// Mouth calls queued by the step and not made: the owner payload's
    /// layout is not read.
    pub mouth_unported: u64,
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
    tags: Arc<dyn SoundTags + Send + Sync>,
    levels: Arc<dyn Levels + Send + Sync>,
    pub ledger: SoundLedger,
    outbox: Vec<TableEvent>,
    last_phase: Option<u32>,
}

pub type Shared = Arc<Mutex<Table>>;

impl Table {
    pub fn new(tags: Arc<dyn SoundTags + Send + Sync>, levels: Arc<dyn Levels + Send + Sync>) -> Self {
        Self { sounds: Sounds::new(), tags, levels, ledger: SoundLedger::default(), outbox: Vec::new(), last_phase: None }
    }

    /// No tag data and no levels: every start refuses by name until the
    /// map's tags and the guest levels are wired.
    pub fn headless() -> Self {
        Self::new(Arc::new(TagTable::default()), Arc::new(UnportedLevels))
    }

    fn reset(&mut self) {
        self.sounds = Sounds::new();
        self.outbox.clear();
        self.last_phase = None;
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
            ("sound.owner_guest_calls", l.owner.guest_calls),
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
            ("sound.mouth_unported", l.mouth_unported),
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
        out
    }
}

/// The process's table: the one the hooks and the tick's phase share.
pub static SHARED: LazyLock<Shared> = LazyLock::new(|| Arc::new(Mutex::new(Table::headless())));

pub fn lock(t: &Mutex<Table>) -> MutexGuard<'_, Table> {
    t.lock().unwrap_or_else(|e| e.into_inner())
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

/// Make the unit calls the step queued: a line's end through `0x18c8c0`'s
/// gate and unit test; a mouth is counted and not made.
async fn unit_calls(cx: &Cx, table: &Shared, calls: Vec<UnitCall>) -> Result<(), engine::entry::EntryError> {
    for call in calls {
        match call {
            UnitCall::LineEnded { object, tag } => {
                let on = cx.read(|m| m.u32(DIALOGUE).and_then(|d| m.u8(d.wrapping_add(DIALOGUE_ON)))).unwrap_or(0) != 0;
                if !on {
                    continue;
                }
                if cx.call(GuestCall::new(OBJECT_IS).eax(object).args([3])).await?.eax != 0 {
                    cx.call(GuestCall::new(UNIT_LINE_ENDED).eax(object).args([tag.0])).await?;
                }
            }
            UnitCall::Mouth { .. } => {
                lock(table).ledger.mouth_unported += 1;
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
        t.ledger.owner.guest_calls += counters.guest_calls;
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
    let mut t = lock(table);
    t.ledger.phase_runs += 1;
    t.ledger.phase_ns += started.elapsed().as_nanos() as u64;
    Ok(Ret::new(0, 0))
}

impl Table {
    /// `step_loops`, then `step`, with the guest's seed word as the RNG.
    fn step_phase(&mut self, host: &mut GuestUnits<'_>, tick: u32) {
        let seed = seed_at(host.memory()).and_then(|at| Ok((at, host.memory().u32(at)?)));
        let Ok((at, before)) = seed else {
            self.ledger.refused_unreadable += 1;
            return;
        };
        let mut rng = Rng::new(before);
        let (tags, levels) = (Arc::clone(&self.tags), Arc::clone(&self.levels));
        if levels.refused().is_none() || self.sounds.loops().is_empty() {
            self.sounds.step_loops(&mut rng, &*tags, &*levels, tick);
        }
        let events = self.sounds.step(tick, host);
        self.outbox.extend(events);
        if rng.state() != before {
            self.ledger.draws += draws_between(before, rng.state());
            let _ = host.memory().set_u32(at, rng.state());
        }
    }

    /// Admit a read request on `tick` with the guest's seed.
    fn admit(&mut self, m: &mut dyn GuestMemory, request: Result<sim_sound::instances::Request, RequestError>, tick: u32) -> u32 {
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
        if self.levels.refused().is_some() {
            self.ledger.refused_no_level += 1;
            return NONE;
        }
        request.level = self.levels.level(request.tag, &request.source);
        let Ok(at) = seed_at(m) else {
            self.ledger.refused_unreadable += 1;
            return NONE;
        };
        let Ok(before) = m.u32(at) else {
            self.ledger.refused_unreadable += 1;
            return NONE;
        };
        let mut rng = Rng::new(before);
        let tags = Arc::clone(&self.tags);
        let result = self.sounds.admit(&mut rng, &*tags, request, tick);
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

/// A gestalt pitch range and its permutations, read for one chooser call.
struct Rows {
    absolute: u16,
    range: PitchRange,
    permutations: BTreeMap<u16, Permutation>,
}

impl SoundTags for Rows {
    fn sound(&self, _: TagIndex) -> Option<&SoundTag> {
        None
    }
    fn range(&self, index: u16) -> Option<&PitchRange> {
        (index == self.absolute).then_some(&self.range)
    }
    fn permutation(&self, index: u16) -> Option<&Permutation> {
        self.permutations.get(&index)
    }
    fn scale(&self, _: i8) -> Option<&sim_sound::instances::Scale> {
        None
    }
    fn promotion(&self, _: u8, _: u16) -> Option<u16> {
        None
    }
}

/// The rows of `absolute` from the gestalt, the reads `0x219110` makes.
fn rows(m: &dyn GuestMemory, absolute: i32) -> Result<Rows, MemFault> {
    let gestalt = m.u32(GESTALT)?;
    let at = m.u32(gestalt.wrapping_add(0x24))?.wrapping_add(12u32.wrapping_mul(absolute as u32));
    let first = m.u16(at.wrapping_add(8))? as i16;
    let count = m.u16(at.wrapping_add(0xa))? as i16;
    let perms = m.u32(gestalt.wrapping_add(0x2c))?;
    let mut permutations = BTreeMap::new();
    for i in 0..count.max(0) {
        let index = (i32::from(first) + i32::from(i)) as u16;
        let skip = m.u16(perms.wrapping_add(16 * u32::from(index)).wrapping_add(2))?;
        permutations.insert(index, Permutation { skip, length_ms: None });
    }
    Ok(Rows {
        absolute: absolute as u16,
        range: PitchRange { first_permutation: first, count, ..PitchRange::default() },
        permutations,
    })
}

/// The tag's class flags, as `0x219430` reads them: the sound globals
/// tag's class rows (`0x5c` bytes, flags `+0xa`), the class at tag `+2`.
fn class_flags(m: &dyn GuestMemory, tag: u32) -> Result<u8, MemFault> {
    let globals = m.u32(points::GLOBALS_TAG.base)?;
    let block = if m.u32(globals.wrapping_add(0xc0))? != 0 { m.u32(globals.wrapping_add(0xc4))? } else { 0 };
    let datum = m.u32(block.wrapping_add(4))?;
    let entries = m.u32(points::TAG_ENTRIES.base)?;
    let data = m.u32(entries.wrapping_add((datum & 0xffff) * 16 + 8))?;
    let classes = m.u32(data.wrapping_add(4))?;
    let class = m.u8(tag.wrapping_add(2))? as i8 as i32;
    m.u8(classes.wrapping_add((class * 0x5c) as u32).wrapping_add(0xa))
}

async fn choose(cx: &Cx, table: &Shared) -> EntryResult {
    let entry = cx.entry();
    let tag = entry.edi;
    let range = cx.arg(0)? as u16 as i16;
    let r = cx.write(|m| -> Result<Result<i32, ()>, MemFault> {
        let sound = SoundTag {
            first_range: m.u16(tag.wrapping_add(8))? as i16,
            range_count: m.u8(tag.wrapping_add(0xa))? as i8,
            class_flags: class_flags(m, tag)?,
            ..SoundTag::default()
        };
        let rows = rows(m, i32::from(sound.first_range) + i32::from(range))?;
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
        let rows = rows(m, absolute)?;
        let used = m.u32(used_at)?;
        let before = m.u32(seed)?;
        let mut rng = Rng::new(before);
        let picked = chooser::pick(&mut rng, &rows, &rows.range, absolute, i32::from(last as u16 as i16), used, invert as u8 != 0);
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

/// `0x21d110`: the gate and the rule (guest, pool-free), then the record
/// read and admitted. The start layer's record is not made.
async fn start_record(cx: &Cx, table: &Shared) -> EntryResult {
    let entry = cx.entry();
    let (record, tag) = (entry.eax, cx.arg(0)?);
    if !cx.read(sound_on) {
        return Ok(Ret::new(NONE, 4));
    }
    // Two scratch words below the entry's stack: the gate's channel, the
    // rule's out.
    let slot = entry.esp.wrapping_sub(8);
    let gate = cx.call(GuestCall::new(START_GATE).eax(record).args([tag, slot, 0]).reserve(8)).await?.eax;
    lock(table).ledger.entry_guest_calls += 1;
    if gate & 0xff == 0 {
        return Ok(Ret::new(NONE, 4));
    }
    let refused = cx.call(GuestCall::new(START_RULE).ecx(tag).args([slot.wrapping_add(4)]).reserve(8)).await?.eax;
    lock(table).ledger.entry_guest_calls += 1;
    if refused != 0 {
        return Ok(Ret::new(NONE, 4));
    }
    let handle = cx.write(|m| {
        let channel = m.u32(slot).unwrap_or(0);
        let rule = m.u32(slot.wrapping_add(4)).unwrap_or(NONE);
        let request = request::read(&*m, [tag, channel, record, rule]);
        let tick = tick_of(m).unwrap_or(0);
        lock(table).admit(m, request, tick)
    });
    Ok(Ret::new(handle, 4))
}

async fn stop(cx: &Cx, table: &Shared) -> EntryResult {
    let handle = cx.entry().ebx;
    if handle != NONE {
        let fade = duration_ticks(STOP_FADE_MS);
        with_units(cx, table, |t, host, tick| t.sounds.stop(Handle::from_raw(handle), tick, fade, host))?;
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

/// `0x125a90`: the paused byte and, on a pause, the time it began. The
/// channel calls are the host's and are not made.
async fn paused(cx: &Cx, table: &Shared) -> EntryResult {
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
        let now = cx.call(GuestCall::new(SYSTEM_TIME)).await?.eax;
        lock(table).ledger.entry_guest_calls += 1;
        cx.write(|m| m.set_u32(g.wrapping_add(sg::PAUSE_TIME), now))?;
    }
    cx.write(|m| m.set_u8(g.wrapping_add(sg::BUSY), 0))?;
    Ok(Ret::new(0, 0))
}

/// `0x2194a0`: tag `+0xd` is the bank, `0xff` for none.
fn promotion_bank(m: &dyn GuestMemory, tag: u32) -> Result<u32, MemFault> {
    let bank = m.u8(tag.wrapping_add(0xd))?;
    if bank == 0xff {
        return Ok(0);
    }
    let banks = m.u32(m.u32(GESTALT)?.wrapping_add(0x34))?;
    Ok(banks.wrapping_add((bank as i8 as i32 * 0x34) as u32))
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
/// stack arguments passed on as they came.
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
    let r = cx.call(GuestCall::new(entry.point.base).args(args)).await;
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
        Body::Service => {
            let mut t = lock(&table);
            let events = std::mem::take(&mut t.outbox);
            t.ledger.service_events += events.len() as u64;
            Ok(Ret::new(0, 0))
        }
        Body::Prefetch | Body::LoadPump => Ok(Ret::new(0, 0)),
        Body::StartRecord => start_record(&cx, &table).await,
        Body::Stop => stop(&cx, &table).await,
        Body::StopAll => stop_all(&cx, &table).await,
        Body::Environment => {
            let ecx = cx.entry().ecx;
            cx.write(|m| environment(m, ecx))?;
            Ok(Ret::new(0, 0))
        }
        Body::FirstListener => Ok(Ret::new(cx.read(first_listener)?, 0)),
        Body::Paused => paused(&cx, &table).await,
        Body::Choose => choose(&cx, &table).await,
        Body::Pick => pick(&cx, &table).await,
        Body::PromotionBank => {
            let tag = cx.entry().eax;
            Ok(Ret::new(cx.read(|m| promotion_bank(m, tag))?, 0))
        }
        Body::StartsUpdate
        | Body::StartLayerNew
        | Body::StartLayerPlay
        | Body::StartLayerRun
        | Body::Ambience
        | Body::LoopUpdate
        | Body::LoopSource
        | Body::HistoryLast => unreachable!("refused above"),
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
