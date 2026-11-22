//! The VM: runs a verified blob's handlers against a [`Game`].
//!
//! All state a run needs between handlers is in [`RulesExt`], which has a
//! canonical byte form so it can live in `SimExt` and be hashed. The VM
//! uses integers and 16.16 fixed point only, draws from its own SplitMix64
//! generator, and visits players by slot, teams by index and objects by
//! placement id, so the same blob, options, seed and inputs give the same
//! calls on every machine. See `docs/game/rules-engine.md`, "The IR and the
//! VM".

use std::collections::BTreeMap;

use crate::caps::{self, Entry, Kind, Order, VARIANT_FIELDS};
use crate::ir::*;
use crate::policy;
use crate::ty::{MAX_LABELLED, MAX_OBJECTIVES, MAX_PLAYERS, MAX_SQUADS, MAX_TEAMS, NONE, Ty, fixed};
use crate::verify::{self, Refusal};

/// Ops one tick may execute across all its handlers.
pub const TICK_BUDGET: u32 = 20_000;
/// `on enter` and `on leave` runs a tick; the rest carry over.
pub const ZONE_EVENTS_PER_TICK: u8 = 64;
/// The most bytes [`RulesExt::encode`] may write.
pub const MAX_STATE_BYTES: usize = 16 * 1024;
/// Queued calls waiting for the next tick boundary.
pub const MAX_QUEUE: usize = 128;
/// Objects that carry object variables at once.
pub const MAX_OBJECT_RECORDS: usize = MAX_LABELLED;
/// Zones `ticks_held` counts for at once.
pub const MAX_HELD_ZONES: usize = 32;

/// A value handed to the game, resolved from the blob's tables.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Arg {
    Int(i32),
    Fixed(i32),
    Bool(bool),
    Player(Option<u8>),
    Team(Option<u8>),
    Object(Option<u32>),
    Squad(u8),
    /// A kit class name; `None` for a class the script never names.
    Class(Option<String>),
    Tag(Option<String>),
    Label(String),
    Text(String),
    Enum(u8),
}

/// One call of an API entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    pub entry: &'static Entry,
    pub args: Vec<Arg>,
}

/// A labelled object a set visits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Placed {
    pub placement: u32,
    pub object: u32,
}

/// What the VM reads from and does to the game. The host implements it
/// over `engine::entry::Cx`; tests implement it over a fake.
pub trait Game {
    /// Roster slots of the players present. Order does not matter.
    fn players(&self) -> Vec<u8>;
    /// Indices of the teams in play. Order does not matter.
    fn teams(&self) -> Vec<u8>;
    /// The objects with a Forge label. Order does not matter.
    fn labelled(&self, label: &str) -> Vec<Placed>;
    /// The objects of the objective registry (`E+0xc1c`). Order does not
    /// matter.
    fn objectives(&self) -> Vec<u32> {
        Vec::new()
    }
    /// A retail record field by its [`VARIANT_FIELDS`] name.
    fn variant(&self, field: &str) -> i32;
    /// The zone test behind `ticks_held`: the one team whose living players
    /// alone are inside the zone object this tick; `None` when it is empty
    /// or contested. A game with no zone test answers `None`.
    fn zone_holder(&self, _zone: u32) -> Option<u8> {
        None
    }
    /// A read entry. The VM maps a value outside the result type to its
    /// zero value.
    fn read(&mut self, call: &Call) -> i32;
    /// An action entry, made now (an `Order::Now` entry, or a queued one
    /// from [`Vm::flush_queue`]).
    fn act(&mut self, call: &Call);
    /// The VM stopped on a script error: end the round with the outcome
    /// "script error" through `GAME_END`.
    fn script_error(&mut self, error: &ScriptError);
}

/// An event the host reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Input {
    Init,
    RoundStart,
    RoundEnd,
    Kill { victim: u8, killer: Option<u8> },
    Spawn(u8),
    Death(u8),
    Pickup(u8, u32),
    Drop(u8, u32),
    Enter { zone: u32, object: u32 },
    Leave { zone: u32, object: u32 },
    Press(u8, u32),
    SquadDead(u8),
    ActorKill { killer: Option<u8>, class: String },
    /// The assist of a kill, reported after its `ActorKill`.
    ActorAssist {
        killer: Option<u8>,
        assist: Option<u8>,
        class: String,
    },
    Bounce(u8, u32),
    Edit(u8, u32),
    Join(u8),
    Quit(u8),
}

impl Input {
    pub fn event(&self) -> Event {
        match self {
            Input::Init => Event::Init,
            Input::RoundStart => Event::RoundStart,
            Input::RoundEnd => Event::RoundEnd,
            Input::Kill { .. } => Event::Kill,
            Input::Spawn(_) => Event::Spawn,
            Input::Death(_) => Event::Death,
            Input::Pickup(..) => Event::Pickup,
            Input::Drop(..) => Event::Drop,
            Input::Enter { .. } => Event::Enter,
            Input::Leave { .. } => Event::Leave,
            Input::Press(..) => Event::Press,
            Input::SquadDead(_) => Event::SquadDead,
            Input::ActorKill { .. } => Event::ActorKill,
            Input::ActorAssist { .. } => Event::ActorAssist,
            Input::Bounce(..) => Event::Bounce,
            Input::Edit(..) => Event::Edit,
            Input::Join(_) => Event::Join,
            Input::Quit(_) => Event::Quit,
        }
    }
}

/// What [`Vm::fire`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fired {
    Ran,
    /// The script has no handler for the event.
    NoHandler,
    /// The tick's 64 zone events are used; report it again next tick.
    Carried,
    /// The VM is stopped on a script error.
    Stopped,
}

/// Why the VM stopped. It happens at the same op on every run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScriptError {
    pub tick: u32,
    pub handler: u16,
    pub pc: u16,
    pub kind: ErrorKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// The tick's [`TICK_BUDGET`] ran out.
    Budget,
    /// A table row out of range.
    TableRow,
    /// More than [`MAX_OBJECT_RECORDS`] objects with object variables.
    Objects,
    /// More than [`MAX_QUEUE`] queued calls.
    Queue,
    /// `squad(n)` outside the kit's slots.
    Squad,
    /// More than [`MAX_HELD_ZONES`] zones named to `ticks_held`.
    Zones,
}

impl ErrorKind {
    fn code(self) -> u8 {
        self as u8
    }

    fn from_code(c: u8) -> Option<ErrorKind> {
        Some(match c {
            0 => ErrorKind::Budget,
            1 => ErrorKind::TableRow,
            2 => ErrorKind::Objects,
            3 => ErrorKind::Queue,
            4 => ErrorKind::Squad,
            5 => ErrorKind::Zones,
            _ => return None,
        })
    }
}

/// A trace warning; not state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Warning {
    DivideByZero { handler: u16, pc: u16 },
}

/// A player's power: a capability the script granted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Grant {
    pub player: u8,
    /// Index into the header's `requires`.
    pub cap: u8,
    pub uses: u8,
    /// Index into the blob's palettes.
    pub palette: Option<u8>,
}

/// A call queued for the next tick boundary, with its raw values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Queued {
    pub entry: u16,
    pub args: Vec<i32>,
}

/// The VM's state: what `SimExt` carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RulesExt {
    pub rng: u64,
    pub tick: u32,
    /// Ops used this tick.
    pub ops: u32,
    /// Zone events run this tick.
    pub zone_events: u8,
    pub stopped: Option<ScriptError>,
    pub globals: Vec<i32>,
    /// Per-player slots, [`MAX_PLAYERS`] rows.
    pub players: Vec<Vec<i32>>,
    /// Per-team slots, [`MAX_TEAMS`] rows.
    pub teams: Vec<Vec<i32>>,
    /// Object slots by object handle, for objects written to.
    pub objects: BTreeMap<u32, Vec<i32>>,
    /// Sorted by player, then capability.
    pub grants: Vec<Grant>,
    pub queue: Vec<Queued>,
    /// `ticks_held`'s counters: per zone object a script has named, the
    /// ticks each team held it alone, by team index. Counting starts the
    /// tick after a script first names the zone and ends at the round's
    /// end.
    pub held: BTreeMap<u32, [u32; MAX_TEAMS]>,
}

/// State bytes that do not decode, or do not fit the blob.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum StateError {
    #[error("rules state is {0} bytes, over the {MAX_STATE_BYTES}-byte cap")]
    TooLarge(usize),
    #[error("rules state is malformed at byte {0}")]
    Malformed(usize),
    #[error("rules state does not fit the script's slot table")]
    Shape,
}

const STATE_MAGIC: [u8; 4] = *b"H2RX";
/// 2 added the `ticks_held` counters.
const STATE_VERSION: u8 = 2;

impl RulesExt {
    /// Whether `player` holds `cap` (an index into `requires`) with uses
    /// left.
    pub fn granted(&self, player: u8, cap: u8) -> bool {
        self.grant_of(player, cap).is_some_and(|g| g.uses > 0)
    }

    pub fn grant_of(&self, player: u8, cap: u8) -> Option<&Grant> {
        self.grants.iter().find(|g| g.player == player && g.cap == cap)
    }

    /// Grants or replaces a power. Uses are at most 255 by type.
    pub fn grant(&mut self, grant: Grant) {
        if grant.player as usize >= MAX_PLAYERS {
            return;
        }
        let key = (grant.player, grant.cap);
        match self.grants.binary_search_by(|g| (g.player, g.cap).cmp(&key)) {
            Ok(i) => self.grants[i] = grant,
            Err(i) => self.grants.insert(i, grant),
        }
    }

    pub fn revoke(&mut self, player: u8, cap: u8) {
        self.grants.retain(|g| !(g.player == player && g.cap == cap));
    }

    /// The canonical bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Vec::new();
        w.extend_from_slice(&STATE_MAGIC);
        w.push(STATE_VERSION);
        w.extend_from_slice(&self.rng.to_le_bytes());
        w.extend_from_slice(&self.tick.to_le_bytes());
        w.extend_from_slice(&self.ops.to_le_bytes());
        w.push(self.zone_events);
        match self.stopped {
            None => w.push(0),
            Some(e) => {
                w.push(1);
                w.extend_from_slice(&e.tick.to_le_bytes());
                w.extend_from_slice(&e.handler.to_le_bytes());
                w.extend_from_slice(&e.pc.to_le_bytes());
                w.push(e.kind.code());
            }
        }
        let i32s = |w: &mut Vec<u8>, vs: &[i32]| {
            for v in vs {
                w.extend_from_slice(&v.to_le_bytes());
            }
        };
        w.push(self.globals.len() as u8);
        i32s(&mut w, &self.globals);
        let width = |rows: &[Vec<i32>]| rows.first().map_or(0, |r| r.len());
        w.push(width(&self.players) as u8);
        for row in &self.players {
            i32s(&mut w, row);
        }
        w.push(width(&self.teams) as u8);
        for row in &self.teams {
            i32s(&mut w, row);
        }
        let ow = self.objects.values().next().map_or(0, |r| r.len());
        w.push(ow as u8);
        w.extend_from_slice(&(self.objects.len() as u16).to_le_bytes());
        for (id, row) in &self.objects {
            w.extend_from_slice(&id.to_le_bytes());
            i32s(&mut w, row);
        }
        w.push(self.grants.len() as u8);
        for g in &self.grants {
            w.push(g.player);
            w.push(g.cap);
            w.push(g.uses);
            match g.palette {
                None => w.push(0),
                Some(p) => {
                    w.push(1);
                    w.push(p);
                }
            }
        }
        w.extend_from_slice(&(self.queue.len() as u16).to_le_bytes());
        for q in &self.queue {
            w.extend_from_slice(&q.entry.to_le_bytes());
            w.push(q.args.len() as u8);
            i32s(&mut w, &q.args);
        }
        w.push(self.held.len() as u8);
        for (zone, counts) in &self.held {
            w.extend_from_slice(&zone.to_le_bytes());
            for c in counts {
                w.extend_from_slice(&c.to_le_bytes());
            }
        }
        w
    }

    /// Decodes the canonical bytes; anything else is refused.
    pub fn decode(bytes: &[u8]) -> Result<RulesExt, StateError> {
        if bytes.len() > MAX_STATE_BYTES {
            return Err(StateError::TooLarge(bytes.len()));
        }
        let mut r = StateReader { bytes, at: 0 };
        if r.take(4)? != STATE_MAGIC || r.u8()? != STATE_VERSION {
            return Err(StateError::Malformed(0));
        }
        let rng = r.u64()?;
        let tick = r.u32()?;
        let ops = r.u32()?;
        let zone_events = r.u8()?;
        let stopped = match r.u8()? {
            0 => None,
            1 => {
                let tick = r.u32()?;
                let handler = r.u16()?;
                let pc = r.u16()?;
                let at = r.at;
                let kind = ErrorKind::from_code(r.u8()?).ok_or(StateError::Malformed(at))?;
                Some(ScriptError {
                    tick,
                    handler,
                    pc,
                    kind,
                })
            }
            _ => return Err(StateError::Malformed(r.at - 1)),
        };
        let n = r.u8()? as usize;
        let globals = r.i32s(n)?;
        let pw = r.u8()? as usize;
        let mut players = Vec::with_capacity(MAX_PLAYERS);
        for _ in 0..MAX_PLAYERS {
            players.push(r.i32s(pw)?);
        }
        let tw = r.u8()? as usize;
        let mut teams = Vec::with_capacity(MAX_TEAMS);
        for _ in 0..MAX_TEAMS {
            teams.push(r.i32s(tw)?);
        }
        let ow = r.u8()? as usize;
        let n = r.u16()? as usize;
        let mut objects = BTreeMap::new();
        let mut last: Option<u32> = None;
        for _ in 0..n {
            let at = r.at;
            let id = r.u32()?;
            if last.is_some_and(|l| l >= id) || ow == 0 {
                return Err(StateError::Malformed(at));
            }
            last = Some(id);
            objects.insert(id, r.i32s(ow)?);
        }
        let n = r.u8()? as usize;
        let mut grants: Vec<Grant> = Vec::with_capacity(n);
        for _ in 0..n {
            let at = r.at;
            let player = r.u8()?;
            let cap = r.u8()?;
            let uses = r.u8()?;
            let palette = match r.u8()? {
                0 => None,
                1 => Some(r.u8()?),
                _ => return Err(StateError::Malformed(at)),
            };
            let g = Grant {
                player,
                cap,
                uses,
                palette,
            };
            if player as usize >= MAX_PLAYERS
                || grants.last().is_some_and(|l| (l.player, l.cap) >= (player, cap))
            {
                return Err(StateError::Malformed(at));
            }
            grants.push(g);
        }
        let n = r.u16()? as usize;
        let mut queue = Vec::with_capacity(n.min(MAX_QUEUE));
        for _ in 0..n {
            let entry = r.u16()?;
            let argc = r.u8()? as usize;
            queue.push(Queued {
                entry,
                args: r.i32s(argc)?,
            });
        }
        let n = r.u8()? as usize;
        let mut held = BTreeMap::new();
        let mut last: Option<u32> = None;
        for _ in 0..n {
            let at = r.at;
            let zone = r.u32()?;
            if last.is_some_and(|l| l >= zone) || n > MAX_HELD_ZONES {
                return Err(StateError::Malformed(at));
            }
            last = Some(zone);
            let mut counts = [0u32; MAX_TEAMS];
            for c in &mut counts {
                *c = r.u32()?;
            }
            held.insert(zone, counts);
        }
        if r.at != bytes.len() {
            return Err(StateError::Malformed(r.at));
        }
        Ok(RulesExt {
            rng,
            tick,
            ops,
            zone_events,
            stopped,
            globals,
            players,
            teams,
            objects,
            grants,
            queue,
            held,
        })
    }
}

struct StateReader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> StateReader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], StateError> {
        if self.bytes.len() - self.at < n {
            return Err(StateError::Malformed(self.at));
        }
        let out = &self.bytes[self.at..self.at + n];
        self.at += n;
        Ok(out)
    }
    fn u8(&mut self) -> Result<u8, StateError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, StateError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Result<u32, StateError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn u64(&mut self) -> Result<u64, StateError> {
        let b = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Ok(u64::from_le_bytes(a))
    }
    fn i32s(&mut self, n: usize) -> Result<Vec<i32>, StateError> {
        let b = self.take(n * 4)?;
        Ok(b.chunks_exact(4)
            .map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect())
    }
}

/// SplitMix64 step.
pub fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// A draw in `0..n` (`n > 0`).
fn below(state: &mut u64, n: u64) -> u64 {
    ((splitmix64(state) as u128 * n as u128) >> 64) as u64
}

/// The generator's seed: the match seed mixed with the blob's hash.
pub fn seed(match_seed: u64, ir: &Ir) -> u64 {
    let id = ir.id();
    let mut b = [0u8; 8];
    b.copy_from_slice(&id.0[..8]);
    match_seed ^ u64::from_le_bytes(b)
}

/// A Forge proposal from a player using a granted power.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Proposal<'a> {
    pub player: u8,
    /// The capability name, `forge.move` and so on.
    pub cap: &'a str,
    /// The tag placed, or the tag of the object touched.
    pub tag: &'a str,
    /// The target was made by this match's edits.
    pub match_made: bool,
    /// The target is a biped, a held object or a registered objective.
    pub protected: bool,
}

/// Why a host refuses a proposal before stamping it.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ProposalRefusal {
    #[error("edits are never counted in a ranked game")]
    Ranked,
    #[error("Theater replays a film and takes no edits")]
    Theater,
    #[error("the script is stopped")]
    Stopped,
    #[error("the script does not declare {0}")]
    Undeclared(String),
    #[error("the player holds no grant for {0}")]
    NoGrant(String),
    #[error("the grant for {0} has no uses left")]
    NoUses(String),
    #[error("tag {0} is outside the grant's palette")]
    OutsidePalette(String),
    #[error("with no palette a grant reaches only objects this match made")]
    NotMatchMade,
    #[error("no grant reaches a biped, a held object or an objective")]
    Protected,
}

/// A running script: the verified blob, its option bits and its state.
#[derive(Clone, Debug)]
pub struct Vm {
    ir: Ir,
    options: u32,
    ext: RulesExt,
    /// Each slot's position within its scope.
    pos: Vec<u16>,
    /// Initial values per scope (global, player, team, object).
    inits: [Vec<i32>; 4],
    warnings: Vec<Warning>,
    report: verify::Report,
}

impl Vm {
    /// Verifies `ir` and starts it with fresh state.
    pub fn new(ir: Ir, options: u32, match_seed: u64) -> Result<Vm, Refusal> {
        let report = verify::verify(&ir)?;
        let (pos, inits) = layout(&ir);
        let ext = RulesExt {
            rng: seed(match_seed, &ir),
            tick: 0,
            ops: 0,
            zone_events: 0,
            stopped: None,
            globals: inits[0].clone(),
            players: vec![inits[1].clone(); MAX_PLAYERS],
            teams: vec![inits[2].clone(); MAX_TEAMS],
            objects: BTreeMap::new(),
            grants: Vec::new(),
            queue: Vec::new(),
            held: BTreeMap::new(),
        };
        Ok(Vm {
            ir,
            options,
            ext,
            pos,
            inits,
            warnings: Vec::new(),
            report,
        })
    }

    /// Verifies `ir` and resumes it from saved state.
    pub fn resume(ir: Ir, options: u32, ext: RulesExt) -> Result<Vm, StateError> {
        let report = verify::verify(&ir).map_err(|_| StateError::Shape)?;
        let (pos, inits) = layout(&ir);
        let fits = ext.globals.len() == inits[0].len()
            && ext.players.len() == MAX_PLAYERS
            && ext.players.iter().all(|r| r.len() == inits[1].len())
            && ext.teams.len() == MAX_TEAMS
            && ext.teams.iter().all(|r| r.len() == inits[2].len())
            && ext.objects.values().all(|r| r.len() == inits[3].len())
            && ext.objects.len() <= MAX_OBJECT_RECORDS
            && ext.queue.len() <= MAX_QUEUE
            && ext.held.len() <= MAX_HELD_ZONES
            && ext.grants.iter().all(|g| {
                (g.cap as usize) < ir.header.requires.len()
                    && g.palette.is_none_or(|p| (p as usize) < ir.palettes.len())
            });
        if !fits {
            return Err(StateError::Shape);
        }
        Ok(Vm {
            ir,
            options,
            ext,
            pos,
            inits,
            warnings: Vec::new(),
            report,
        })
    }

    pub fn ir(&self) -> &Ir {
        &self.ir
    }

    pub fn ext(&self) -> &RulesExt {
        &self.ext
    }

    pub fn report(&self) -> &verify::Report {
        &self.report
    }

    /// Trace warnings since the last call.
    pub fn take_warnings(&mut self) -> Vec<Warning> {
        std::mem::take(&mut self.warnings)
    }

    /// Starts a tick: the op budget and the zone-event count reset.
    pub fn begin_tick(&mut self, tick: u32) {
        self.ext.tick = tick;
        self.ext.ops = 0;
        self.ext.zone_events = 0;
    }

    /// Runs the handler for an event the host reported.
    pub fn fire(&mut self, game: &mut dyn Game, input: Input) -> Fired {
        if self.ext.stopped.is_some() {
            return Fired::Stopped;
        }
        let event = input.event();
        let Some(hi) = self.handler(event, None) else {
            self.after(&input);
            return Fired::NoHandler;
        };
        if matches!(event, Event::Enter | Event::Leave) {
            if self.ext.zone_events >= ZONE_EVENTS_PER_TICK {
                return Fired::Carried;
            }
            self.ext.zone_events += 1;
        }
        let args = self.args(&input);
        let fired = self.run(game, hi, &args);
        self.after(&input);
        fired
    }

    /// The per-tick group: each zone `ticks_held` counts adds a tick to the
    /// team holding it alone, timers count down and fire in slot order,
    /// then `on tick` runs.
    pub fn update(&mut self, game: &mut dyn Game) -> Fired {
        if self.ext.stopped.is_some() {
            return Fired::Stopped;
        }
        for (&zone, counts) in self.ext.held.iter_mut() {
            if let Some(t) = game.zone_holder(zone)
                && let Some(c) = counts.get_mut(t as usize)
            {
                *c = c.saturating_add(1);
            }
        }
        let mut fired = Vec::new();
        for (slot, s) in self.ir.slots.iter().enumerate() {
            if s.ty != Ty::Timer {
                continue;
            }
            let at = self.pos[slot] as usize;
            let v = &mut self.ext.globals[at];
            if *v > 0 {
                *v -= 1;
                if *v == 0 {
                    fired.push(slot as u16);
                }
            }
        }
        for slot in fired {
            if let Some(hi) = self.handler(Event::Timer, Some(slot))
                && self.run(game, hi, &[]) == Fired::Stopped
            {
                return Fired::Stopped;
            }
        }
        match self.handler(Event::Tick, None) {
            Some(hi) => self.run(game, hi, &[]),
            None => Fired::NoHandler,
        }
    }

    /// Makes the queued calls in program order. The host calls it from its
    /// `before` on the next `GAME_TICK`.
    pub fn flush_queue(&mut self, game: &mut dyn Game) {
        let queue = std::mem::take(&mut self.ext.queue);
        for q in queue {
            if let Some(entry) = caps::entry(q.entry, self.ir.header.api_version) {
                let args = entry
                    .args
                    .iter()
                    .zip(&q.args)
                    .map(|(&ty, &v)| to_arg(&self.ir, ty, v))
                    .collect();
                game.act(&Call { entry, args });
            }
        }
    }

    /// Checks a granted power's proposal before the host stamps it: never
    /// in a ranked game or in Theater ([`policy::edits_allowed`]). The host
    /// also runs Forge's own validation.
    pub fn check_proposal(&self, context: policy::Context<'_>, p: &Proposal<'_>) -> Result<(), ProposalRefusal> {
        match context {
            policy::Context::Ranked => return Err(ProposalRefusal::Ranked),
            policy::Context::Theater => return Err(ProposalRefusal::Theater),
            policy::Context::Custom | policy::Context::Hopper { .. } => {}
        }
        if self.ext.stopped.is_some() {
            return Err(ProposalRefusal::Stopped);
        }
        let Some(cap) = self.cap_index(p.cap) else {
            return Err(ProposalRefusal::Undeclared(p.cap.to_string()));
        };
        let Some(g) = self.ext.grant_of(p.player, cap) else {
            return Err(ProposalRefusal::NoGrant(p.cap.to_string()));
        };
        if g.uses == 0 {
            return Err(ProposalRefusal::NoUses(p.cap.to_string()));
        }
        if p.protected {
            return Err(ProposalRefusal::Protected);
        }
        match g.palette {
            Some(palette) => {
                let inside = self.ir.palettes.get(palette as usize).is_some_and(|pal| {
                    pal.tags
                        .iter()
                        .any(|&t| self.ir.tags.get(t as usize).is_some_and(|tag| tag == p.tag))
                });
                if !inside {
                    return Err(ProposalRefusal::OutsidePalette(p.tag.to_string()));
                }
            }
            // Placing needs a palette: it is what the player can make.
            None if p.cap == "forge.place" => {
                return Err(ProposalRefusal::OutsidePalette(p.tag.to_string()));
            }
            None if !p.match_made => return Err(ProposalRefusal::NotMatchMade),
            None => {}
        }
        Ok(())
    }

    /// An accepted proposal costs one use at its apply tick. Returns the
    /// uses left.
    pub fn spend(&mut self, player: u8, cap: &str) -> Result<u8, ProposalRefusal> {
        let Some(c) = self.cap_index(cap) else {
            return Err(ProposalRefusal::Undeclared(cap.to_string()));
        };
        let Some(g) = self
            .ext
            .grants
            .iter_mut()
            .find(|g| g.player == player && g.cap == c)
        else {
            return Err(ProposalRefusal::NoGrant(cap.to_string()));
        };
        if g.uses == 0 {
            return Err(ProposalRefusal::NoUses(cap.to_string()));
        }
        g.uses -= 1;
        Ok(g.uses)
    }

    /// The index of a declared capability in `requires`.
    pub fn cap_index(&self, name: &str) -> Option<u8> {
        self.ir
            .header
            .requires
            .iter()
            .position(|r| r == name)
            .map(|i| i as u8)
    }

    /// Reads a global by name, for tests and the agent API.
    pub fn global(&self, name: &str) -> Option<i32> {
        let slot = self.slot_named(name, Scope::Global)?;
        Some(self.ext.globals[self.pos[slot] as usize])
    }

    /// Reads a player variable by name.
    pub fn player_var(&self, player: u8, name: &str) -> Option<i32> {
        let slot = self.slot_named(name, Scope::Player)?;
        let row = self.ext.players.get(player as usize)?;
        Some(row[self.pos[slot] as usize])
    }

    fn slot_named(&self, name: &str, scope: Scope) -> Option<usize> {
        self.ir.slots.iter().position(|s| {
            s.scope == scope && self.ir.strings.get(s.name as usize).is_some_and(|n| n == name)
        })
    }

    fn handler(&self, event: Event, timer: Option<u16>) -> Option<usize> {
        self.ir
            .handlers
            .iter()
            .position(|h| h.event == event && h.timer == timer)
    }

    /// Grants end at the player's quit and at the round's end; the
    /// `ticks_held` counters end at the round's end.
    fn after(&mut self, input: &Input) {
        match *input {
            Input::Quit(p) => self.ext.grants.retain(|g| g.player != p),
            Input::RoundEnd => {
                self.ext.grants.clear();
                self.ext.held.clear();
            }
            _ => {}
        }
    }

    /// The ticks `team` has held `zone` alone, as `ticks_held` reads it,
    /// for tests and the agent API.
    pub fn ticks_held(&self, zone: u32, team: u8) -> u32 {
        self.ext
            .held
            .get(&zone)
            .and_then(|c| c.get(team as usize))
            .copied()
            .unwrap_or(0)
    }

    fn args(&self, input: &Input) -> Vec<i32> {
        let p = |s: u8| if (s as usize) < MAX_PLAYERS { s as i32 } else { NONE };
        let op = |s: Option<u8>| s.map_or(NONE, p);
        let o = |id: u32| i32::try_from(id).unwrap_or(NONE);
        match input {
            Input::Init | Input::RoundStart | Input::RoundEnd => vec![],
            Input::Kill { victim, killer } => vec![p(*victim), op(*killer)],
            Input::Spawn(x) | Input::Death(x) | Input::Join(x) | Input::Quit(x) => vec![p(*x)],
            Input::Pickup(x, ob)
            | Input::Drop(x, ob)
            | Input::Press(x, ob)
            | Input::Bounce(x, ob)
            | Input::Edit(x, ob) => vec![p(*x), o(*ob)],
            Input::Enter { zone, object } | Input::Leave { zone, object } => {
                vec![o(*zone), o(*object)]
            }
            Input::SquadDead(s) => vec![if (*s as usize) < MAX_SQUADS { *s as i32 } else { 0 }],
            Input::ActorKill { killer, class } => vec![op(*killer), self.class(class)],
            Input::ActorAssist {
                killer,
                assist,
                class,
            } => vec![op(*killer), op(*assist), self.class(class)],
        }
    }

    /// A class name as a string-table index; `none` for a class the script
    /// never names.
    fn class(&self, class: &str) -> i32 {
        self.ir
            .strings
            .iter()
            .position(|s| s == class)
            .map_or(NONE, |i| i as i32)
    }

    fn run(&mut self, game: &mut dyn Game, handler: usize, args: &[i32]) -> Fired {
        let result = {
            let mut run = Run {
                ir: &self.ir,
                ext: &mut self.ext,
                pos: &self.pos,
                inits: &self.inits,
                options: self.options,
                warnings: &mut self.warnings,
                handler: handler as u16,
            };
            run.exec(game, args)
        };
        match result {
            Ok(()) => Fired::Ran,
            Err((pc, kind)) => {
                let error = ScriptError {
                    tick: self.ext.tick,
                    handler: handler as u16,
                    pc,
                    kind,
                };
                self.ext.stopped = Some(error);
                self.ext.queue.clear();
                game.script_error(&error);
                Fired::Stopped
            }
        }
    }
}

fn layout(ir: &Ir) -> (Vec<u16>, [Vec<i32>; 4]) {
    let mut inits: [Vec<i32>; 4] = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    let mut pos = Vec::with_capacity(ir.slots.len());
    for s in &ir.slots {
        let list = &mut inits[s.scope as usize];
        pos.push(list.len() as u16);
        list.push(s.init);
    }
    (pos, inits)
}

/// Resolves a raw value to what the game sees.
fn to_arg(ir: &Ir, ty: Ty, v: i32) -> Arg {
    let opt_u8 = |v: i32| u8::try_from(v).ok();
    let string = |list: &[String], v: i32| usize::try_from(v).ok().and_then(|i| list.get(i).cloned());
    match ty {
        Ty::Int | Ty::Timer => Arg::Int(v),
        Ty::Fixed => Arg::Fixed(v),
        Ty::Bool => Arg::Bool(v != 0),
        Ty::Player => Arg::Player(opt_u8(v)),
        Ty::Team => Arg::Team(opt_u8(v)),
        Ty::Object => Arg::Object(u32::try_from(v).ok()),
        Ty::Squad => Arg::Squad(opt_u8(v).unwrap_or(0)),
        Ty::Class => Arg::Class(string(&ir.strings, v)),
        Ty::Tag => Arg::Tag(string(&ir.tags, v)),
        Ty::Label => Arg::Label(string(&ir.labels, v).unwrap_or_default()),
        Ty::Text => Arg::Text(string(&ir.strings, v).unwrap_or_default()),
        Ty::Enum(_) => Arg::Enum(opt_u8(v).unwrap_or(0)),
    }
}

/// One loop in progress.
struct Frame {
    members: Vec<i32>,
    next: usize,
    dst: Reg,
    start: usize,
    end: usize,
}

struct Run<'a> {
    ir: &'a Ir,
    ext: &'a mut RulesExt,
    pos: &'a [u16],
    inits: &'a [Vec<i32>; 4],
    options: u32,
    warnings: &'a mut Vec<Warning>,
    handler: u16,
}

type Fault = (u16, ErrorKind);

impl<'a> Run<'a> {
    fn exec(&mut self, game: &mut dyn Game, args: &[i32]) -> Result<(), Fault> {
        let ir: &'a Ir = self.ir;
        let hd = &ir.handlers[self.handler as usize];
        let mut regs: Vec<i32> = hd.regs.iter().map(|t| t.zero()).collect();
        for (r, &a) in regs.iter_mut().zip(args) {
            *r = a;
        }
        let code = &hd.code;
        let mut loops: Vec<Frame> = Vec::new();
        let mut pc = 0usize;
        loop {
            if let Some(top) = loops.last_mut()
                && pc == top.end
            {
                if top.next < top.members.len() {
                    regs[top.dst as usize] = top.members[top.next];
                    top.next += 1;
                    pc = top.start;
                } else {
                    loops.pop();
                }
                continue;
            }
            if pc >= code.len() {
                return Ok(());
            }
            let here = pc as u16;
            if self.ext.ops >= TICK_BUDGET {
                return Err((here, ErrorKind::Budget));
            }
            self.ext.ops += 1;
            pc += 1;
            match code[here as usize] {
                Op::Const { dst, value } => regs[dst as usize] = value,
                Op::Move { dst, src } => regs[dst as usize] = regs[src as usize],
                Op::Arith { op, ty, dst, a, b } => {
                    let (a, b) = (regs[a as usize], regs[b as usize]);
                    if b == 0 && matches!(op, ArithOp::Div | ArithOp::Rem) {
                        self.warnings.push(Warning::DivideByZero {
                            handler: self.handler,
                            pc: here,
                        });
                    }
                    regs[dst as usize] = match (ty, op) {
                        (_, ArithOp::Add) => a.saturating_add(b),
                        (_, ArithOp::Sub) => a.saturating_sub(b),
                        (NumTy::Int, ArithOp::Mul) => a.saturating_mul(b),
                        (NumTy::Fixed, ArithOp::Mul) => fixed::mul(a, b),
                        (NumTy::Int, ArithOp::Div) => {
                            if b == 0 {
                                0
                            } else {
                                a.saturating_div(b)
                            }
                        }
                        (NumTy::Fixed, ArithOp::Div) => fixed::div(a, b),
                        (_, ArithOp::Rem) => a.checked_rem(b).unwrap_or(0),
                    };
                }
                Op::Neg { dst, a, .. } => regs[dst as usize] = regs[a as usize].saturating_neg(),
                Op::Cmp { op, dst, a, b } => {
                    let (a, b) = (regs[a as usize], regs[b as usize]);
                    let holds = match op {
                        CmpOp::Eq => a == b,
                        CmpOp::Ne => a != b,
                        CmpOp::Lt => a < b,
                        CmpOp::Le => a <= b,
                        CmpOp::Gt => a > b,
                        CmpOp::Ge => a >= b,
                    };
                    regs[dst as usize] = holds as i32;
                }
                Op::Not { dst, a } => regs[dst as usize] = (regs[a as usize] == 0) as i32,
                Op::Load { dst, slot, owner } => {
                    let owner = owner.map(|r| regs[r as usize]);
                    regs[dst as usize] = self.load(slot, owner);
                }
                Op::Store { slot, owner, src } => {
                    let owner = owner.map(|r| regs[r as usize]);
                    let v = regs[src as usize];
                    self.store(slot, owner, v).map_err(|k| (here, k))?;
                }
                Op::Opt { dst, index } => regs[dst as usize] = self.option(index as usize),
                Op::Variant { dst, field } => {
                    let (name, ty) = VARIANT_FIELDS[field as usize];
                    let v = game.variant(name);
                    regs[dst as usize] = if ty == Ty::Bool { (v != 0) as i32 } else { v };
                }
                Op::TableRead {
                    dst,
                    table,
                    row,
                    col,
                } => {
                    let t = &ir.tables[table as usize];
                    let row = regs[row as usize];
                    regs[dst as usize] = if row == NONE {
                        t.columns[col as usize].ty.zero()
                    } else {
                        usize::try_from(row)
                            .ok()
                            .and_then(|r| t.cell(r, col as usize))
                            .ok_or((here, ErrorKind::TableRow))?
                    };
                }
                Op::TableFind { dst, table, key } => {
                    let t = &ir.tables[table as usize];
                    let key = regs[key as usize];
                    regs[dst as usize] = (0..t.rows())
                        .find(|&r| t.cell(r, 0) == Some(key))
                        .map_or(NONE, |r| r as i32);
                }
                Op::Jump { to } => pc = to as usize,
                Op::JumpIf { cond, to } => {
                    if regs[cond as usize] != 0 {
                        pc = to as usize;
                    }
                }
                Op::JumpIfNot { cond, to } => {
                    if regs[cond as usize] == 0 {
                        pc = to as usize;
                    }
                }
                Op::ForEach { set, dst, body } => {
                    let mut members = self.members(game, set.base);
                    if let Some(n) = set.pick {
                        let n = regs[n as usize].clamp(0, members.len() as i32) as usize;
                        for i in 0..n {
                            let j = i + below(&mut self.ext.rng, (members.len() - i) as u64) as usize;
                            members.swap(i, j);
                        }
                        members.truncate(n);
                    }
                    let end = pc + body as usize;
                    if let Some(&first) = members.first() {
                        regs[dst as usize] = first;
                        loops.push(Frame {
                            members,
                            next: 1,
                            dst,
                            start: pc,
                            end,
                        });
                    } else {
                        pc = end;
                    }
                }
                Op::Call {
                    entry,
                    ref args,
                    dst,
                } => {
                    let e = &caps::ENTRIES[entry as usize];
                    let raw: Vec<i32> = args.iter().map(|&r| regs[r as usize]).collect();
                    let result = self.call(game, e, raw).map_err(|k| (here, k))?;
                    if let (Some(dst), Some(v)) = (dst, result) {
                        regs[dst as usize] = v;
                    }
                }
                Op::Grant {
                    player,
                    cap,
                    uses,
                    palette,
                } => {
                    if let Ok(p) = u8::try_from(regs[player as usize]) {
                        self.ext.grant(Grant {
                            player: p,
                            cap,
                            uses,
                            palette,
                        });
                    }
                }
                Op::Revoke { player, cap } => {
                    if let Ok(p) = u8::try_from(regs[player as usize]) {
                        self.ext.revoke(p, cap);
                    }
                }
                Op::Granted { dst, player, cap } => {
                    let held = u8::try_from(regs[player as usize]).is_ok_and(|p| self.ext.granted(p, cap));
                    regs[dst as usize] = held as i32;
                }
            }
        }
    }

    fn option(&self, index: usize) -> i32 {
        let o = &self.ir.options[index];
        let off: u32 = self.ir.options[..index].iter().map(|o| o.bits()).sum();
        let bits = o.bits();
        if bits == 0 {
            return o.lo;
        }
        let raw = (self.options as u64 >> off) & ((1u64 << bits) - 1);
        (o.lo as i64 + raw as i64).min(o.hi as i64) as i32
    }

    fn load(&self, slot: u16, owner: Option<i32>) -> i32 {
        let s = &self.ir.slots[slot as usize];
        let at = self.pos[slot as usize] as usize;
        match owner {
            None => self.ext.globals[at],
            Some(o) => {
                let row = match s.scope {
                    Scope::Player => usize::try_from(o).ok().and_then(|i| self.ext.players.get(i)),
                    Scope::Team => usize::try_from(o).ok().and_then(|i| self.ext.teams.get(i)),
                    Scope::Object => u32::try_from(o).ok().and_then(|i| self.ext.objects.get(&i)),
                    Scope::Global => None,
                };
                row.map_or(s.init, |r| r[at])
            }
        }
    }

    fn store(&mut self, slot: u16, owner: Option<i32>, v: i32) -> Result<(), ErrorKind> {
        let s = &self.ir.slots[slot as usize];
        let at = self.pos[slot as usize] as usize;
        let v = if s.ty == Ty::Timer { v.max(0) } else { v };
        match (s.scope, owner) {
            (Scope::Global, _) => self.ext.globals[at] = v,
            (Scope::Player, Some(o)) => {
                if let Some(row) = usize::try_from(o).ok().and_then(|i| self.ext.players.get_mut(i)) {
                    row[at] = v;
                }
            }
            (Scope::Team, Some(o)) => {
                if let Some(row) = usize::try_from(o).ok().and_then(|i| self.ext.teams.get_mut(i)) {
                    row[at] = v;
                }
            }
            (Scope::Object, Some(o)) => {
                if let Ok(id) = u32::try_from(o) {
                    if !self.ext.objects.contains_key(&id) {
                        if self.ext.objects.len() >= MAX_OBJECT_RECORDS {
                            return Err(ErrorKind::Objects);
                        }
                        self.ext.objects.insert(id, self.inits[3].clone());
                    }
                    if let Some(row) = self.ext.objects.get_mut(&id) {
                        row[at] = v;
                    }
                }
            }
            (_, None) => {}
        }
        Ok(())
    }

    fn members(&self, game: &mut dyn Game, base: SetBase) -> Vec<i32> {
        match base {
            SetBase::Players => sorted_small(game.players(), MAX_PLAYERS),
            SetBase::Teams => sorted_small(game.teams(), MAX_TEAMS),
            SetBase::Labelled(l) => {
                let mut placed = game.labelled(&self.ir.labels[l as usize]);
                placed.sort();
                placed.dedup_by_key(|p| p.object);
                placed
                    .into_iter()
                    .filter_map(|p| i32::try_from(p.object).ok())
                    .take(MAX_LABELLED)
                    .collect()
            }
            SetBase::Objectives => {
                let mut list = game.objectives();
                list.sort_unstable();
                list.dedup();
                list.into_iter()
                    .filter_map(|o| i32::try_from(o).ok())
                    .take(MAX_OBJECTIVES)
                    .collect()
            }
        }
    }

    fn call(&mut self, game: &mut dyn Game, e: &'static Entry, raw: Vec<i32>) -> Result<Option<i32>, ErrorKind> {
        match e.kind {
            Kind::VmOnly => {
                let v = match e.id {
                    caps::id::CLAMP => raw[0].max(raw[1]).min(raw[2]),
                    caps::id::MIN => raw[0].min(raw[1]),
                    caps::id::MAX => raw[0].max(raw[1]),
                    caps::id::SQUAD => {
                        if (0..MAX_SQUADS as i32).contains(&raw[0]) {
                            raw[0]
                        } else {
                            return Err(ErrorKind::Squad);
                        }
                    }
                    caps::id::NOW => i32::try_from(self.ext.tick).unwrap_or(i32::MAX),
                    caps::id::RANDOM => {
                        if raw[0] > 0 {
                            (splitmix64(&mut self.ext.rng) % raw[0] as u64) as i32
                        } else {
                            0
                        }
                    }
                    // A zone named for the first time is counted from the
                    // next update; `none` and a team out of range read 0.
                    caps::id::TICKS_HELD => {
                        let (Ok(zone), Ok(team)) = (u32::try_from(raw[0]), usize::try_from(raw[1])) else {
                            return Ok(Some(0));
                        };
                        if team >= MAX_TEAMS {
                            return Ok(Some(0));
                        }
                        if !self.ext.held.contains_key(&zone) {
                            if self.ext.held.len() >= MAX_HELD_ZONES {
                                return Err(ErrorKind::Zones);
                            }
                            self.ext.held.insert(zone, [0; MAX_TEAMS]);
                        }
                        let n = self.ext.held.get(&zone).map_or(0, |c| c[team]);
                        i32::try_from(n).unwrap_or(i32::MAX)
                    }
                    _ => 0,
                };
                Ok(Some(v))
            }
            Kind::Read => {
                let call = self.resolve(e, &raw);
                let v = game.read(&call);
                let ty = e.result.unwrap_or(Ty::Int);
                let sizes = self.ir.sizes();
                Ok(Some(if ty.valid(v, &sizes) { v } else { ty.zero() }))
            }
            Kind::Action => {
                match e.order {
                    Order::Now => {
                        let call = self.resolve(e, &raw);
                        game.act(&call);
                    }
                    Order::Queued => {
                        if self.ext.queue.len() >= MAX_QUEUE {
                            return Err(ErrorKind::Queue);
                        }
                        self.ext.queue.push(Queued {
                            entry: e.id,
                            args: raw,
                        });
                    }
                }
                Ok(None)
            }
        }
    }

    fn resolve(&self, e: &'static Entry, raw: &[i32]) -> Call {
        Call {
            entry: e,
            args: e
                .args
                .iter()
                .zip(raw)
                .map(|(&ty, &v)| to_arg(self.ir, ty, v))
                .collect(),
        }
    }
}

fn sorted_small(mut list: Vec<u8>, max: usize) -> Vec<i32> {
    list.retain(|&v| (v as usize) < max);
    list.sort_unstable();
    list.dedup();
    list.into_iter().map(i32::from).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitmix64_matches_the_reference() {
        // Reference outputs for seed 0 (Vigna's splitmix64.c).
        let mut s = 0u64;
        assert_eq!(splitmix64(&mut s), 0xe220_a839_7b1d_cdaf);
        assert_eq!(splitmix64(&mut s), 0x6e78_9e6a_a1b9_65f4);
        assert_eq!(splitmix64(&mut s), 0x06c4_5d18_8009_454f);
    }

    #[test]
    fn below_stays_in_range() {
        let mut s = 7u64;
        for n in 1..50u64 {
            assert!(below(&mut s, n) < n);
        }
    }

    fn ext() -> RulesExt {
        RulesExt {
            rng: 5,
            tick: 9,
            ops: 3,
            zone_events: 1,
            stopped: Some(ScriptError {
                tick: 9,
                handler: 1,
                pc: 4,
                kind: ErrorKind::Budget,
            }),
            globals: vec![1, -2],
            players: vec![vec![0]; MAX_PLAYERS],
            teams: vec![vec![]; MAX_TEAMS],
            objects: [(3u32, vec![7, 8]), (9, vec![1, 2])].into_iter().collect(),
            grants: vec![
                Grant {
                    player: 0,
                    cap: 1,
                    uses: 20,
                    palette: Some(0),
                },
                Grant {
                    player: 2,
                    cap: 0,
                    uses: 3,
                    palette: None,
                },
            ],
            queue: vec![Queued {
                entry: caps::id::HUNT,
                args: vec![4],
            }],
            held: [(5u32, [0, 30, 0, 0, 0, 0, 0, 0]), (12, [7; MAX_TEAMS])].into_iter().collect(),
        }
    }

    #[test]
    fn state_refuses_unsorted_zones() {
        let mut e = ext();
        e.held.clear();
        let bytes = e.encode();
        // Two zones, 12 before 5: rewrite the tail by hand.
        let mut bad = bytes[..bytes.len() - 1].to_vec();
        bad.push(2);
        for zone in [12u32, 5] {
            bad.extend_from_slice(&zone.to_le_bytes());
            bad.extend_from_slice(&[0; 4 * MAX_TEAMS]);
        }
        assert!(RulesExt::decode(&bad).is_err());
    }

    #[test]
    fn state_round_trips_canonically() {
        let e = ext();
        let bytes = e.encode();
        assert_eq!(RulesExt::decode(&bytes), Ok(e));
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(RulesExt::decode(&longer).is_err());
        for cut in 0..bytes.len() {
            assert!(RulesExt::decode(&bytes[..cut]).is_err(), "cut at {cut}");
        }
    }

    #[test]
    fn state_refuses_unsorted_grants() {
        let mut e = ext();
        e.grants.swap(0, 1);
        assert!(RulesExt::decode(&e.encode()).is_err());
    }

    #[test]
    fn the_largest_state_fits_the_cap() {
        let e = RulesExt {
            rng: 0,
            tick: 0,
            ops: 0,
            zone_events: 0,
            stopped: None,
            globals: vec![0; MAX_GLOBAL_SLOTS],
            players: vec![vec![0; MAX_PLAYER_SLOTS]; MAX_PLAYERS],
            teams: vec![vec![0; MAX_TEAM_SLOTS]; MAX_TEAMS],
            objects: (0..MAX_OBJECT_RECORDS as u32)
                .map(|i| (i, vec![0; MAX_OBJECT_SLOTS]))
                .collect(),
            grants: (0..MAX_PLAYERS as u8)
                .flat_map(|p| {
                    (0..4u8).map(move |c| Grant {
                        player: p,
                        cap: c,
                        uses: 255,
                        palette: Some(0),
                    })
                })
                .collect(),
            queue: vec![
                Queued {
                    entry: caps::id::PLACE_SQUAD,
                    args: vec![0; 4],
                };
                MAX_QUEUE
            ],
            held: (0..MAX_HELD_ZONES as u32).map(|z| (z, [u32::MAX; MAX_TEAMS])).collect(),
        };
        let bytes = e.encode();
        assert!(bytes.len() <= MAX_STATE_BYTES, "{} bytes", bytes.len());
        assert_eq!(RulesExt::decode(&bytes), Ok(e));
    }

    #[test]
    fn grants_replace_and_revoke() {
        let mut e = ext();
        e.grants.clear();
        e.grant(Grant {
            player: 3,
            cap: 0,
            uses: 2,
            palette: None,
        });
        e.grant(Grant {
            player: 1,
            cap: 0,
            uses: 5,
            palette: None,
        });
        e.grant(Grant {
            player: 3,
            cap: 0,
            uses: 9,
            palette: None,
        });
        assert_eq!(e.grants.len(), 2);
        assert_eq!(e.grants[0].player, 1);
        assert_eq!(e.grant_of(3, 0).unwrap().uses, 9);
        assert!(e.granted(3, 0));
        e.revoke(3, 0);
        assert!(!e.granted(3, 0));
        e.grant(Grant {
            player: 16,
            cap: 0,
            uses: 1,
            palette: None,
        });
        assert_eq!(e.grants.len(), 1);
    }
}
