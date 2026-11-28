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

use crate::bodies::Body;
use crate::caps::{self, Entry, Kind, Order, VARIANT_FIELDS};
use crate::ir::*;
use crate::policy;
use crate::ty::{
    MAX_LABELLED, MAX_OBJECTIVES, MAX_PLAYERS, MAX_SQUADS, MAX_TEAMS, NONE, Ty, fixed, object_id, object_value,
};
use crate::verify::{self, Refusal};

/// Ops one tick may execute across all its handlers.
pub const TICK_BUDGET: u32 = 100_000;
/// `on enter` and `on leave` runs a tick; the rest carry over.
pub const ZONE_EVENTS_PER_TICK: u8 = 64;
/// The most bytes [`RulesExt::encode`] may write.
pub const MAX_STATE_BYTES: usize = 64 * 1024;
/// Queued calls waiting for the next tick boundary.
pub const MAX_QUEUE: usize = 128;
/// Objects that carry object variables at once.
pub const MAX_OBJECT_RECORDS: usize = MAX_LABELLED;
/// Zones `ticks_held` counts for at once.
pub const MAX_HELD_ZONES: usize = 32;
/// Areas `show_area` keeps drawn at once.
pub const MAX_SHOWN_AREAS: usize = 8;
/// Movers (`move_to`) running at once.
pub const MAX_MOVERS: usize = 16;
/// Barriers (`set_open`) open at once.
pub const MAX_GATES: usize = 16;
/// How far below its closed place an open barrier stands, in 16.16 world
/// units: far enough that nothing it blocks is on it.
pub const GATE_DROP: i32 = 64 << 16;
/// Pickers (`offer_pick`) a script offers at once, numbered from 0.
pub const MAX_PICKS: usize = 2;
/// Rows (`pick_row`) a picker holds.
pub const MAX_PICK_ROWS: usize = 6;
/// Pairs of objects the sight table (`sees`) keeps at once.
pub const MAX_SIGHT_PAIRS: usize = 32;
/// A sight pair nobody asked for in this many ticks is dropped.
pub const SIGHT_KEEP: u32 = 30;

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
/// over `guest_layout::entry::Cx`; tests implement it over a fake.
pub trait Game {
    /// The variant's type announces a round, including one with no handler.
    fn start_line(&mut self, _ty: crate::types::Type) {}
    /// What a point is called on the scoreboard and in the post-game: the
    /// header's `points`, told at each round start.
    fn points_label(&mut self, _label: &str) {}
    /// Roster slots of the players present. Order does not matter.
    fn players(&self) -> Vec<u8>;
    /// Indices of the teams in play. Order does not matter.
    fn teams(&self) -> Vec<u8>;
    /// The roster slots seated on `team`, for a waypoint's audience
    /// (`mark(wp, at, team)`), resolved from the roster every machine
    /// holds, so all of them keep the same mask. A game that does not say
    /// answers none, and the team's mark is drawn to nobody.
    fn players_of_team(&self, _team: u8) -> Vec<u8> {
        Vec::new()
    }
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
    /// A mover's step for this tick: `object` set to `at` (16.16 world
    /// units) and moving at `velocity` (16.16 world units a second; all
    /// zero on the tick it arrives). The VM makes one for each running
    /// mover every tick ([`Vm::update`]).
    fn step_object(&mut self, _object: u32, _at: [i32; 3], _velocity: [i32; 3]) {}
    /// The sight test behind `sees`: whether nothing solid is between the
    /// two objects. The VM asks once a tick for each pair in its sight
    /// table at the tick's end ([`Vm::update`]). A host that traces at
    /// the next boundary returns `None` to keep the previous result until
    /// it can call [`Vm::record_sight`]. A game with no trace answers false.
    fn trace_sight(&mut self, _from: u32, _to: u32) -> Option<bool> {
        Some(false)
    }
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
    /// A superbounce by `player` off the strip `spot`, the `chain`th of its
    /// chain (1 for a single bounce), and whether it was a Monster Bounce.
    Bounce {
        player: u8,
        spot: u32,
        chain: u32,
        monster: bool,
    },
    Edit(u8, u32),
    Join(u8),
    Quit(u8),
    /// A fall out of the map during a bounce: the Bounced medal.
    Bounced(u8),
    /// `object` took damage credited to `by`: `amount` as a `fixed` raw
    /// value, a fraction of full vitality.
    Damage {
        object: u32,
        by: Option<u8>,
        amount: i32,
    },
    /// A player's biped touched down: the speed it fell at on the tick
    /// before and its vertical speed now, `fixed` world units a second.
    Touchdown { player: u8, down: i32, up: i32 },
    /// A landing the host judged a superbounce, when its launch window
    /// closed: the strip (`superbounce_chain::spot_id`, `None` off every
    /// marked strip) and the launch, a `fixed` raw value in world units a
    /// second, rounded up so a `>` against a constant is the host's own
    /// comparison.
    Launch { player: u8, spot: Option<u32>, speed: i32 },
    /// The player's run of bounces with no ground between them ended.
    Grounded(u8),
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
            Input::Bounce { .. } => Event::Bounce,
            Input::Edit(..) => Event::Edit,
            Input::Join(_) => Event::Join,
            Input::Quit(_) => Event::Quit,
            Input::Bounced(_) => Event::Bounced,
            Input::Damage { .. } => Event::Damage,
            Input::Touchdown { .. } => Event::Touchdown,
            Input::Launch { .. } => Event::Launch,
            Input::Grounded(_) => Event::Grounded,
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
    /// `set_body` named a tag that is no body of `crate::bodies::LIST`.
    Body,
    /// More than [`MAX_SHOWN_AREAS`] areas shown at once (`show_area`).
    Areas,
    /// A `hud_*`, `mark` or `unmark` call named a widget or waypoint the
    /// script does not declare.
    Hud,
    /// More than [`MAX_MOVERS`] movers running at once (`move_to`).
    Movers,
    /// More than [`MAX_SIGHT_PAIRS`] pairs asked to `sees` within
    /// [`SIGHT_KEEP`] ticks.
    Sight,
    /// More than [`MAX_GATES`] barriers open at once (`set_open`).
    Gates,
    /// A picker (`offer_pick`, `pick_row`) numbered past [`MAX_PICKS`], a
    /// row for a picker never offered, or more than [`MAX_PICK_ROWS`] rows.
    Picks,
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
            6 => ErrorKind::Body,
            7 => ErrorKind::Areas,
            8 => ErrorKind::Hud,
            9 => ErrorKind::Movers,
            10 => ErrorKind::Sight,
            11 => ErrorKind::Gates,
            12 => ErrorKind::Picks,
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
    /// Players holding the script role, one bit per player slot.
    pub role_mask: u16,
    pub queue: Vec<Queued>,
    /// `ticks_held`'s counters: per zone object a script has named, the
    /// ticks each team held it alone, by team index. Counting starts the
    /// tick after a script first names the zone and ends at the round's
    /// end.
    pub held: BTreeMap<u32, [u32; MAX_TEAMS]>,
    /// Each team's spawn rule (`spawn_areas`, `spawn_near`), by team
    /// index: which starting locations its players' spawns may use. The
    /// host's spawn hook reads it at each spawn; it ends at the round's
    /// end.
    pub spawns: [Option<Spawns>; MAX_TEAMS],
    /// Each team's body (`set_body`), by team index: the body its players
    /// spawn in; `None`, their own. The host's spawn hook reads it at each
    /// spawn; it ends at the round's end.
    pub bodies: [Option<Body>; MAX_TEAMS],
    /// Each team's objective HUD state (`objective`, `objective_mark`,
    /// `meter`, `countdown`), by team index. The objective HUD reads it on
    /// every machine that runs the VM; it ends at the round's end.
    pub objectives: [Objective; MAX_TEAMS],
    /// The areas a script shows in play (`show_area`), in the order it
    /// first showed them, at most [`MAX_SHOWN_AREAS`]: a zone object and
    /// the team whose colour its wall takes (`None` for neutral). Every
    /// machine, and a film, draws the same walls from them.
    pub shown: Vec<(u32, Option<u8>)>,
    /// The script's HUD widgets and waypoints (`hud_*`, `mark`): the show
    /// state every machine that runs the VM draws from
    /// ([`crate::show::view`]) and a film's keyframe restores.
    pub show: crate::show::State,
    /// The movers a script runs (`move_to`), in the order started, at most
    /// [`MAX_MOVERS`]. The VM steps each every tick ([`Vm::update`]), so a
    /// checkpoint, a film's keyframe and the sim hash carry them and every
    /// machine that runs the VM moves the same objects the same way.
    pub movers: Vec<Mover>,
    /// The sight table (`sees`): each pair a script asked for within the
    /// last [`SIGHT_KEEP`] ticks, in the order first asked, with the
    /// answer of its latest trace. The VM traces each at every tick's end
    /// ([`Vm::update`]), so a read answers what the tick before the
    /// asking ended with; the state carries it so every machine, a
    /// checkpoint and a film's keyframe read the same.
    pub sight: Vec<Sight>,
    /// The barriers a script holds open (`set_open`), in the order opened,
    /// each with the place it closes back to, at most [`MAX_GATES`].
    pub gates: Vec<Gate>,
    /// The pickers the script offers (`offer_pick`), in slot order.
    pub picks: Vec<PickOffer>,
    /// What each player picked in them, at most one row for each player and
    /// picker, in the order made. The host reports a pick
    /// ([`Vm::choose`]); `picked` reads it.
    pub choices: Vec<Choice>,
    /// Each player's loadout window ([`Window`]): the rows of a picker the
    /// player may pick from now, and the row last picked. The script sets
    /// the window (`offer`); the host reports the pick
    /// ([`Vm::choose_loadout`]); `choice` reads the row. The row outlives a
    /// round, the window does not.
    pub loadout: [Window; MAX_PLAYERS],
}

/// One player's loadout window: the picker whose rows the player picks
/// from, how many of its first rows are open (0 = none offered) and the
/// row last picked (0 until one is). A row at or past `count` is refused.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Window {
    pub picker: u8,
    pub count: u8,
    pub row: u8,
}

/// One picker a script offers: its slot, and its title and rows as indexes
/// of the script's string table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickOffer {
    pub slot: u8,
    pub title: u16,
    pub rows: Vec<u16>,
}

/// One player's pick: the row of picker `slot`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Choice {
    pub player: u8,
    pub slot: u8,
    pub row: u8,
}

/// One open barrier: the object and where it stands when closed, 16.16
/// world units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gate {
    pub object: u32,
    pub home: [i32; 3],
}

impl Gate {
    /// Where the barrier stands while open.
    pub fn open_at(&self) -> [i32; 3] {
        [self.home[0], self.home[1], self.home[2].saturating_sub(GATE_DROP)]
    }
}

/// One pair of the sight table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sight {
    pub from: u32,
    pub to: u32,
    /// The last tick a script asked for the pair.
    pub asked: u32,
    /// Whether the latest trace found nothing solid between them; false
    /// until the first trace.
    pub clear: bool,
}

/// An object moving in a straight line between two points in a stated
/// number of ticks (`move_to(o, at, t)`): the record the VM steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mover {
    pub object: u32,
    /// Where the object was when `move_to` was called, 16.16 world units.
    pub from: [i32; 3],
    pub to: [i32; 3],
    /// The tick `move_to` was called on.
    pub start: u32,
    /// Ticks from `start` to arriving, at least 1.
    pub duration: u32,
}

impl Mover {
    /// The tick the mover arrives on, and `on arrive` runs.
    pub fn end(&self) -> u32 {
        self.start.saturating_add(self.duration)
    }

    /// The position on `tick`: the straight line, in 64-bit integer steps
    /// truncated toward `from`, and exactly `to` from the end tick on.
    pub fn at(&self, tick: u32) -> [i32; 3] {
        let k = i64::from(tick.saturating_sub(self.start).min(self.duration));
        let d = i64::from(self.duration);
        let mut out = [0; 3];
        for i in 0..3 {
            let delta = i64::from(self.to[i]) - i64::from(self.from[i]);
            out[i] = (i64::from(self.from[i]) + delta * k / d).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
        }
        out
    }

    /// The velocity while moving, in 16.16 world units a second.
    pub fn velocity(&self) -> [i32; 3] {
        let d = i64::from(self.duration);
        let mut out = [0; 3];
        for i in 0..3 {
            let delta = i64::from(self.to[i]) - i64::from(self.from[i]);
            out[i] = (delta * i64::from(caps::TICKS_PER_SECOND) / d).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
        }
        out
    }
}

/// A team's spawn rule: the starting locations its players' spawns may
/// use (docs/game/rules-engine.md, "The engine API").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spawns {
    /// `spawn_areas(team, label, n)`: the starting locations of the IR's
    /// label `label` numbered `number`.
    Areas { label: u16, number: i32 },
    /// `spawn_near(team, zone, n)`: the `count` starting locations nearest
    /// the zone object `zone`.
    Near { zone: u32, count: u8 },
}

/// The marks one team's objective HUD shows at most: two sites or the goal.
pub const MAX_OBJECTIVE_MARKS: usize = 3;

/// What a mark asks a player to do at its zone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkKind {
    Attack,
    Defend,
}

impl MarkKind {
    /// The kind a script's `objective_mark` number names: 1 attack, 2
    /// defend; 0 is no mark.
    pub fn from_code(code: i32) -> Option<MarkKind> {
        match code {
            1 => Some(MarkKind::Attack),
            2 => Some(MarkKind::Defend),
            _ => None,
        }
    }

    pub fn code(self) -> u8 {
        match self {
            MarkKind::Attack => 1,
            MarkKind::Defend => 2,
        }
    }
}

/// One team's objective HUD state (docs/game/hud-kit.md, "The scripted
/// objective HUD"): the line a script sets, the zones it marks, the gauge
/// and the clock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Objective {
    /// The objective line, an index of the script's string table.
    pub text: Option<u16>,
    /// The marked zone objects, in the order they were marked.
    pub marks: [Option<(u32, MarkKind)>; MAX_OBJECTIVE_MARKS],
    /// The gauge: its value and what it is of.
    pub meter: Option<(i32, i32)>,
    /// The tick the clock reaches zero at.
    pub ends: Option<u32>,
}

impl Objective {
    /// Whether the state asks the HUD to draw nothing.
    pub fn is_empty(&self) -> bool {
        *self == Objective::default()
    }

    /// Ticks left on the clock at `tick`; none when no clock runs.
    pub fn left(&self, tick: u32) -> Option<u32> {
        self.ends.map(|e| e.saturating_sub(tick))
    }

    /// The marked zones with their kinds.
    pub fn marked(&self) -> impl Iterator<Item = (u32, MarkKind)> + '_ {
        self.marks.iter().flatten().copied()
    }

    /// Marks `zone` as `kind`, or lifts its mark for no kind. Whether it
    /// fits: a fourth mark does not.
    fn mark(&mut self, zone: u32, kind: Option<MarkKind>) -> bool {
        let at = self.marks.iter().position(|m| m.is_some_and(|(z, _)| z == zone));
        match (at, kind) {
            (Some(i), Some(k)) => self.marks[i] = Some((zone, k)),
            (Some(i), None) => {
                self.marks[i] = None;
                self.marks.sort_by_key(|m| m.is_none());
            }
            (None, Some(k)) => match self.marks.iter().position(Option::is_none) {
                Some(i) => self.marks[i] = Some((zone, k)),
                None => return false,
            },
            (None, None) => {}
        }
        true
    }
}

/// The most starting locations `spawn_near` keeps.
pub const MAX_SPAWN_NEAR: i32 = u8::MAX as i32;

/// A team's spawn rule with its label resolved, as a host reads it
/// ([`Vm::spawn_rule`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpawnRule<'a> {
    /// The starting locations labelled `label` and numbered `number`.
    Areas { label: &'a str, number: i32 },
    /// The `count` starting locations nearest the zone object `zone`.
    Near { zone: u32, count: u8 },
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
/// 2 added the `ticks_held` counters; 3 the teams' spawn rules and
/// bodies; 4 the script role mask and the shown areas; 5 the show state;
/// 6 the movers; 7 the sight table; 8 the open barriers; 9 the pickers and
/// what each player picked; 10 the loadout windows.
const STATE_VERSION: u8 = 10;

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
        w.extend_from_slice(&self.role_mask.to_le_bytes());
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
        w.extend_from_slice(&(self.globals.len() as u16).to_le_bytes());
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
        for rule in &self.spawns {
            match *rule {
                None => w.push(0),
                Some(Spawns::Areas { label, number }) => {
                    w.push(1);
                    w.extend_from_slice(&label.to_le_bytes());
                    w.extend_from_slice(&number.to_le_bytes());
                }
                Some(Spawns::Near { zone, count }) => {
                    w.push(2);
                    w.extend_from_slice(&zone.to_le_bytes());
                    w.push(count);
                }
            }
        }
        for body in &self.bodies {
            w.push(body.map_or(0, Body::code));
        }
        w.push(self.held.len() as u8);
        for (zone, counts) in &self.held {
            w.extend_from_slice(&zone.to_le_bytes());
            for c in counts {
                w.extend_from_slice(&c.to_le_bytes());
            }
        }
        w.push(self.shown.len() as u8);
        for (zone, team) in &self.shown {
            w.extend_from_slice(&zone.to_le_bytes());
            w.push(team.unwrap_or(NO_TEAM));
        }
        let show = self.show.encode();
        w.push(show.len() as u8);
        w.extend_from_slice(&show);
        w.push(self.movers.len() as u8);
        for m in &self.movers {
            w.extend_from_slice(&m.object.to_le_bytes());
            i32s(&mut w, &m.from);
            i32s(&mut w, &m.to);
            w.extend_from_slice(&m.start.to_le_bytes());
            w.extend_from_slice(&m.duration.to_le_bytes());
        }
        w.push(self.sight.len() as u8);
        for t in &self.sight {
            w.extend_from_slice(&t.from.to_le_bytes());
            w.extend_from_slice(&t.to.to_le_bytes());
            w.extend_from_slice(&t.asked.to_le_bytes());
            w.push(t.clear as u8);
        }
        w.push(self.gates.len() as u8);
        for g in &self.gates {
            w.extend_from_slice(&g.object.to_le_bytes());
            i32s(&mut w, &g.home);
        }
        w.push(self.picks.len() as u8);
        for o in &self.picks {
            w.push(o.slot);
            w.extend_from_slice(&o.title.to_le_bytes());
            w.push(o.rows.len() as u8);
            for row in &o.rows {
                w.extend_from_slice(&row.to_le_bytes());
            }
        }
        w.push(self.choices.len() as u8);
        for c in &self.choices {
            w.extend_from_slice(&[c.player, c.slot, c.row]);
        }
        for l in &self.loadout {
            w.extend_from_slice(&[l.picker, l.count, l.row]);
        }
        for o in &self.objectives {
            w.extend_from_slice(&o.text.unwrap_or(u16::MAX).to_le_bytes());
            for m in &o.marks {
                match m {
                    None => w.push(0),
                    Some((zone, kind)) => {
                        w.push(kind.code());
                        w.extend_from_slice(&zone.to_le_bytes());
                    }
                }
            }
            match o.meter {
                None => w.push(0),
                Some((value, of)) => {
                    w.push(1);
                    w.extend_from_slice(&value.to_le_bytes());
                    w.extend_from_slice(&of.to_le_bytes());
                }
            }
            match o.ends {
                None => w.push(0),
                Some(t) => {
                    w.push(1);
                    w.extend_from_slice(&t.to_le_bytes());
                }
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
        let role_mask = r.u16()?;
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
        let n = r.u16()? as usize;
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
        let mut spawns = [None; MAX_TEAMS];
        for rule in &mut spawns {
            let at = r.at;
            *rule = match r.u8()? {
                0 => None,
                1 => Some(Spawns::Areas {
                    label: r.u16()?,
                    number: r.i32()?,
                }),
                2 => {
                    let zone = r.u32()?;
                    let count = r.u8()?;
                    if count == 0 {
                        return Err(StateError::Malformed(at));
                    }
                    Some(Spawns::Near { zone, count })
                }
                _ => return Err(StateError::Malformed(at)),
            };
        }
        let mut bodies = [None; MAX_TEAMS];
        for body in &mut bodies {
            let at = r.at;
            *body = match r.u8()? {
                0 => None,
                c => Some(Body::from_code(c).ok_or(StateError::Malformed(at))?),
            };
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
        let at = r.at;
        let n = r.u8()? as usize;
        if n > MAX_SHOWN_AREAS {
            return Err(StateError::Malformed(at));
        }
        let mut shown: Vec<(u32, Option<u8>)> = Vec::with_capacity(n);
        for _ in 0..n {
            let at = r.at;
            let zone = r.u32()?;
            let team = match r.u8()? {
                NO_TEAM => None,
                t if (t as usize) < MAX_TEAMS => Some(t),
                _ => return Err(StateError::Malformed(at)),
            };
            if shown.iter().any(|(z, _)| *z == zone) {
                return Err(StateError::Malformed(at));
            }
            shown.push((zone, team));
        }
        let at = r.at;
        let n = r.u8()? as usize;
        let show = crate::show::State::decode(r.take(n)?).map_err(|_| StateError::Malformed(at))?;
        let at = r.at;
        let n = r.u8()? as usize;
        if n > MAX_MOVERS {
            return Err(StateError::Malformed(at));
        }
        let mut movers: Vec<Mover> = Vec::with_capacity(n);
        for _ in 0..n {
            let at = r.at;
            let object = r.u32()?;
            let from = [r.i32()?, r.i32()?, r.i32()?];
            let to = [r.i32()?, r.i32()?, r.i32()?];
            let start = r.u32()?;
            let duration = r.u32()?;
            if object_id(object as i32).is_none() || duration == 0 || movers.iter().any(|m| m.object == object) {
                return Err(StateError::Malformed(at));
            }
            movers.push(Mover {
                object,
                from,
                to,
                start,
                duration,
            });
        }
        let at = r.at;
        let n = r.u8()? as usize;
        if n > MAX_SIGHT_PAIRS {
            return Err(StateError::Malformed(at));
        }
        let mut sight: Vec<Sight> = Vec::with_capacity(n);
        for _ in 0..n {
            let at = r.at;
            let (from, to, asked) = (r.u32()?, r.u32()?, r.u32()?);
            let clear = match r.u8()? {
                0 => false,
                1 => true,
                _ => return Err(StateError::Malformed(at)),
            };
            let known = |o: u32| object_id(o as i32).is_some();
            if !known(from) || !known(to) || sight.iter().any(|t| t.from == from && t.to == to) {
                return Err(StateError::Malformed(at));
            }
            sight.push(Sight { from, to, asked, clear });
        }
        let at = r.at;
        let n = r.u8()? as usize;
        if n > MAX_GATES {
            return Err(StateError::Malformed(at));
        }
        let mut gates: Vec<Gate> = Vec::with_capacity(n);
        for _ in 0..n {
            let at = r.at;
            let object = r.u32()?;
            let home = [r.i32()?, r.i32()?, r.i32()?];
            if object_id(object as i32).is_none() || gates.iter().any(|g| g.object == object) {
                return Err(StateError::Malformed(at));
            }
            gates.push(Gate { object, home });
        }
        let at = r.at;
        let n = r.u8()? as usize;
        if n > MAX_PICKS {
            return Err(StateError::Malformed(at));
        }
        let mut picks: Vec<PickOffer> = Vec::with_capacity(n);
        for _ in 0..n {
            let at = r.at;
            let slot = r.u8()?;
            let title = r.u16()?;
            let rows = r.u8()? as usize;
            if slot as usize >= MAX_PICKS || rows > MAX_PICK_ROWS || picks.iter().any(|o| o.slot >= slot) {
                return Err(StateError::Malformed(at));
            }
            let mut list = Vec::with_capacity(rows);
            for _ in 0..rows {
                list.push(r.u16()?);
            }
            picks.push(PickOffer { slot, title, rows: list });
        }
        let at = r.at;
        let n = r.u8()? as usize;
        if n > MAX_PLAYERS * MAX_PICKS {
            return Err(StateError::Malformed(at));
        }
        let mut choices: Vec<Choice> = Vec::with_capacity(n);
        for _ in 0..n {
            let at = r.at;
            let c = Choice { player: r.u8()?, slot: r.u8()?, row: r.u8()? };
            let offered = picks.iter().find(|o| o.slot == c.slot).is_some_and(|o| (c.row as usize) < o.rows.len());
            if c.player as usize >= MAX_PLAYERS
                || !offered
                || choices.iter().any(|d| d.player == c.player && d.slot == c.slot)
            {
                return Err(StateError::Malformed(at));
            }
            choices.push(c);
        }
        let mut loadout = [Window::default(); MAX_PLAYERS];
        for l in &mut loadout {
            let at = r.at;
            *l = Window { picker: r.u8()?, count: r.u8()?, row: r.u8()? };
            if l.picker as usize >= MAX_PICKS
                || l.count as usize > MAX_PICK_ROWS
                || (l.count > 0 && l.row >= l.count)
                || (l.count == 0 && l.row as usize >= MAX_PICK_ROWS)
            {
                return Err(StateError::Malformed(at));
            }
        }
        let mut objectives = [Objective::default(); MAX_TEAMS];
        for o in &mut objectives {
            let at = r.at;
            o.text = match r.u16()? {
                u16::MAX => None,
                t => Some(t),
            };
            for m in &mut o.marks {
                let at = r.at;
                *m = match r.u8()? {
                    0 => None,
                    c => {
                        let kind = [MarkKind::Attack, MarkKind::Defend]
                            .into_iter()
                            .find(|k| k.code() == c)
                            .ok_or(StateError::Malformed(at))?;
                        Some((r.u32()?, kind))
                    }
                };
            }
            o.meter = match r.u8()? {
                0 => None,
                1 => Some((r.i32()?, r.i32()?)),
                _ => return Err(StateError::Malformed(at)),
            };
            o.ends = match r.u8()? {
                0 => None,
                1 => Some(r.u32()?),
                _ => return Err(StateError::Malformed(at)),
            };
        }
        if r.at != bytes.len() {
            return Err(StateError::Malformed(r.at));
        }
        Ok(RulesExt {
            rng,
            tick,
            ops,
            zone_events,
            role_mask,
            stopped,
            globals,
            players,
            teams,
            objects,
            grants,
            queue,
            held,
            spawns,
            bodies,
            objectives,
            shown,
            show,
            movers,
            sight,
            gates,
            picks,
            choices,
            loadout,
        })
    }
}

/// A shown area's neutral team, in the state's bytes.
const NO_TEAM: u8 = 0xff;

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
    fn i32(&mut self) -> Result<i32, StateError> {
        Ok(self.u32()? as i32)
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
    role_traits: [bool; 3],
    options: u32,
    ext: RulesExt,
    /// Each slot's position within its scope.
    pos: Vec<u16>,
    /// Initial values per scope (global, player, team, object).
    inits: [Vec<i32>; 4],
    warnings: Vec<Warning>,
    report: verify::Report,
}

/// The three booleans the game queries for a role holder. A script may
/// expose any of them as ordinary variant options.
fn role_traits(ir: &Ir, options: u32) -> [bool; 3] {
    [("role_active_camo", false), ("role_extra_damage", true), ("role_damage_resistance", false)]
        .map(|(name, default)| {
            if ir.options.iter().enumerate().any(|(i, o)| o.ty == Ty::Bool && ir.option_name(i) == name) {
                ir.option_value(options, name).is_some_and(|v| v != 0)
            } else {
                default
            }
        })
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
            role_mask: 0,
            grants: Vec::new(),
            queue: Vec::new(),
            held: BTreeMap::new(),
            spawns: [None; MAX_TEAMS],
            bodies: [None; MAX_TEAMS],
            objectives: [Objective::default(); MAX_TEAMS],
            shown: Vec::new(),
            show: crate::show::State::default(),
            movers: Vec::new(),
            sight: Vec::new(),
            gates: Vec::new(),
            picks: Vec::new(),
            choices: Vec::new(),
            loadout: [Window::default(); MAX_PLAYERS],
        };
        Ok(Vm {
            role_traits: role_traits(&ir, options),
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
            && ext.shown.len() <= MAX_SHOWN_AREAS
            && ext.show.widgets.iter().skip(ir.widgets.len()).all(Option::is_none)
            && ext.show.waypoints.iter().skip(ir.waypoints.len()).all(Option::is_none)
            && ext
                .show
                .widgets
                .iter()
                .flatten()
                .all(|w| w.text.is_none_or(|t| (t as usize) < ir.strings.len()))
            && ext.movers.len() <= MAX_MOVERS
            && ext.sight.len() <= MAX_SIGHT_PAIRS
            && ext.gates.len() <= MAX_GATES
            && ext
                .picks
                .iter()
                .all(|o| (o.title as usize) < ir.strings.len() && o.rows.iter().all(|&t| (t as usize) < ir.strings.len()))
            && ext.objectives.iter().all(|o| {
                o.text.is_none_or(|t| (t as usize) < ir.strings.len())
                    && o.marks.iter().skip_while(|m| m.is_some()).all(|m| m.is_none())
            })
            && ext.spawns.iter().all(|r| match r {
                Some(Spawns::Areas { label, .. }) => (*label as usize) < ir.labels.len(),
                _ => true,
            })
            && ext.grants.iter().all(|g| {
                (g.cap as usize) < ir.header.requires.len()
                    && g.palette.is_none_or(|p| (p as usize) < ir.palettes.len())
            });
        if !fits {
            return Err(StateError::Shape);
        }
        Ok(Vm {
            role_traits: role_traits(&ir, options),
            ir,
            options,
            ext,
            pos,
            inits,
            warnings: Vec::new(),
            report,
        })
    }

    /// Swaps in a new program between ticks, keeping every slot value, the
    /// generator, grants and queued calls (`crate::reload`). `ir` must
    /// verify and keep the state shape ([`crate::reload::compatible`]);
    /// otherwise nothing changes. A stopped script runs again.
    pub fn swap_program(&mut self, ir: Ir) -> Result<(), crate::reload::SwapError> {
        use crate::reload::{SwapError, carry, compatible};
        let report = verify::verify(&ir)?;
        compatible(&self.ir, &ir).map_err(SwapError::Incompatible)?;
        carry(&self.ir, &ir, &mut self.ext);
        let (pos, inits) = layout(&ir);
        self.ext.stopped = None;
        self.role_traits = role_traits(&ir, self.options);
        self.ir = ir;
        self.pos = pos;
        self.inits = inits;
        self.report = report;
        Ok(())
    }

    pub fn ir(&self) -> &Ir {
        &self.ir
    }

    pub fn ext(&self) -> &RulesExt {
        &self.ext
    }

    /// End the role grants when the host observes a round boundary.
    pub fn clear_roles(&mut self) {
        self.ext.role_mask = 0;
    }

    pub fn has_role(&self, slot: u8) -> bool {
        slot < MAX_PLAYERS as u8 && self.ext.role_mask & (1 << slot) != 0
    }

    /// The role holder's active camo, extra damage or damage resistance.
    pub fn role_trait(&self, trait_id: u32) -> Option<bool> {
        self.role_traits.get(trait_id.checked_sub(1)? as usize).copied()
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
        if input == Input::RoundStart {
            return self.round_start_as(game, self.ir.header.game_type);
        }
        self.fire_handler(game, input)
    }

    /// Starts a round of the variant's declared type. A settings-only
    /// variant can use a different type's built-in for its handlers.
    pub fn round_start_as(&mut self, game: &mut dyn Game, ty: crate::types::Type) -> Fired {
        if self.ext.stopped.is_none() {
            game.points_label(&self.ir.header.points);
            if ty.row().start != crate::types::StartLine::None {
                game.start_line(ty);
            }
        }
        self.fire_handler(game, Input::RoundStart)
    }

    fn fire_handler(&mut self, game: &mut dyn Game, input: Input) -> Fired {
        if self.ext.stopped.is_some() {
            return Fired::Stopped;
        }
        let event = input.event();
        let Some(hi) = self.handler(event, None) else {
            self.after(game, &input);
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
        self.after(game, &input);
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
        if self.step_movers(game) == Fired::Stopped {
            return Fired::Stopped;
        }
        let fired = match self.handler(Event::Tick, None) {
            Some(hi) => self.run(game, hi, &[]),
            None => Fired::NoHandler,
        };
        if fired != Fired::Stopped {
            self.trace_sight(game);
        }
        fired
    }

    /// The tick's end: each pair of the sight table is traced again, so a
    /// read on the next tick answers from this one, and a pair nobody
    /// asked for in [`SIGHT_KEEP`] ticks leaves the table.
    fn trace_sight(&mut self, game: &mut dyn Game) {
        let tick = self.ext.tick;
        self.ext.sight.retain(|t| tick.saturating_sub(t.asked) < SIGHT_KEEP);
        for t in &mut self.ext.sight {
            if let Some(clear) = game.trace_sight(t.from, t.to) {
                t.clear = clear;
            }
        }
    }

    /// Steps each running mover for this tick, in the order started: the
    /// game is told the object's place and velocity, and a mover whose end
    /// tick this is is told its end, leaves the list, and then `on arrive`
    /// runs for each such object in that order (a handler may start its
    /// object moving again). A mover takes no step on the tick it was
    /// started on.
    fn step_movers(&mut self, game: &mut dyn Game) -> Fired {
        let tick = self.ext.tick;
        let mut arrived = Vec::new();
        let mut i = 0;
        while i < self.ext.movers.len() {
            let m = self.ext.movers[i];
            if tick <= m.start {
                i += 1;
            } else if tick >= m.end() {
                game.step_object(m.object, m.to, [0; 3]);
                arrived.push(m.object);
                self.ext.movers.remove(i);
            } else {
                game.step_object(m.object, m.at(tick), m.velocity());
                i += 1;
            }
        }
        for object in arrived {
            if let Some(hi) = self.handler(Event::Arrive, None)
                && self.run(game, hi, &[object_value(object)]) == Fired::Stopped
            {
                return Fired::Stopped;
            }
        }
        Fired::Ran
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
                        .any(|&t| self.ir.tags.get(t as usize).is_some_and(|tag| tag.path == p.tag))
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

    /// Reads an object variable by name, including its default before the
    /// first write to the object.
    pub fn object_var(&self, object: u32, name: &str) -> Option<i32> {
        let slot = self.slot_named(name, Scope::Object)?;
        Some(self.ext.objects.get(&object).map_or(self.ir.slots[slot].init, |row| {
            row[self.pos[slot] as usize]
        }))
    }

    /// Writes a global by name, between ticks, for a host that holds the
    /// mode's state in the script (Firefight's pools). `false` when the
    /// script declares no global of that name.
    pub fn set_global(&mut self, name: &str, value: i32) -> bool {
        let Some(slot) = self.slot_named(name, Scope::Global) else {
            return false;
        };
        let at = self.pos[slot] as usize;
        self.ext.globals[at] = value;
        true
    }

    /// Writes a player variable by name, between ticks. `false` when the
    /// script declares no player variable of that name or the slot is
    /// outside the roster.
    pub fn set_player_var(&mut self, player: u8, name: &str, value: i32) -> bool {
        let Some(slot) = self.slot_named(name, Scope::Player) else {
            return false;
        };
        let Some(row) = self.ext.players.get_mut(player as usize) else {
            return false;
        };
        row[self.pos[slot] as usize] = value;
        true
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
    /// `ticks_held` counters, the spawn rules, the bodies and the shown
    /// areas end at the round's end, and each open barrier closes.
    fn after(&mut self, game: &mut dyn Game, input: &Input) {
        match *input {
            Input::Quit(p) => {
                self.ext.grants.retain(|g| g.player != p);
                if p < MAX_PLAYERS as u8 {
                    self.ext.role_mask &= !(1 << p);
                }
            }
            Input::RoundEnd => {
                self.ext.grants.clear();
                self.clear_roles();
                self.ext.held.clear();
                self.ext.spawns = [None; MAX_TEAMS];
                self.ext.bodies = [None; MAX_TEAMS];
                self.ext.objectives = [Objective::default(); MAX_TEAMS];
                self.ext.shown.clear();
                self.ext.show = crate::show::State::default();
                self.ext.movers.clear();
                self.ext.sight.clear();
                self.ext.picks.clear();
                self.ext.choices.clear();
                for l in &mut self.ext.loadout {
                    l.count = 0;
                }
                // Every barrier still open stands where it closed.
                for gate in std::mem::take(&mut self.ext.gates) {
                    game.step_object(gate.object, gate.home, [0; 3]);
                }
            }
            _ => {}
        }
    }

    /// Team `team`'s spawn rule, its label resolved: what the host's spawn
    /// hook keeps the team's spawns to. `None`: the engine's own pick.
    pub fn spawn_rule(&self, team: u8) -> Option<SpawnRule<'_>> {
        match (*self.ext.spawns.get(team as usize)?)? {
            Spawns::Areas { label, number } => Some(SpawnRule::Areas {
                label: self.ir.labels.get(label as usize)?.text.as_str(),
                number,
            }),
            Spawns::Near { zone, count } => Some(SpawnRule::Near { zone, count }),
        }
    }

    /// The movers running (`move_to`), in the order started.
    pub fn movers(&self) -> &[Mover] {
        &self.ext.movers
    }

    /// The barriers held open (`set_open`), in the order opened.
    pub fn gates(&self) -> &[Gate] {
        &self.ext.gates
    }

    /// The host's put of `object` to `at` (world units) was refused, which
    /// the engine does for a place outside the world. When it was the put
    /// that opened a barrier ([`Gate::open_at`], read as the host reads a
    /// 16.16 value), the barrier never moved: it is closed again here, so
    /// the next `set_open(o, true)` tries again. Any other refused put
    /// changes nothing; a barrier's closing put is not one to take back,
    /// as the barrier is already forgotten. Returns whether a barrier was
    /// closed.
    pub fn put_refused(&mut self, object: u32, at: [f32; 3]) -> bool {
        let world = |v: i32| (f64::from(v) / 65536.0) as f32;
        let held = self
            .ext
            .gates
            .iter()
            .position(|g| g.object == object && g.open_at().map(world) == at);
        match held {
            Some(i) => {
                self.ext.gates.remove(i);
                true
            }
            None => false,
        }
    }

    /// The pickers the script offers (`offer_pick`), in slot order.
    pub fn picks(&self) -> &[PickOffer] {
        &self.ext.picks
    }

    /// What the players picked, in the order made.
    pub fn choices(&self) -> &[Choice] {
        &self.ext.choices
    }

    /// The pickers some player's loadout window reads its rows from: the
    /// host asks those players through [`Window`], not as a plain picker.
    pub fn loadout_pickers(&self) -> Vec<u8> {
        let mut out: Vec<u8> = self.ext.loadout.iter().filter(|l| l.count > 0).map(|l| l.picker).collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// The pickers that are plain ones: all but [`Vm::loadout_pickers`].
    pub fn plain_picks(&self) -> Vec<PickOffer> {
        let held = self.loadout_pickers();
        self.ext.picks.iter().filter(|o| !held.contains(&o.slot)).cloned().collect()
    }

    /// A player's loadout window.
    pub fn window(&self, player: u8) -> Option<Window> {
        self.ext.loadout.get(player as usize).copied()
    }

    /// The host reports that `player` picked `row` of the loadout window.
    /// A row the window does not open (at or past its count, or past the
    /// rows its picker holds), or a player outside the roster, is refused
    /// (`false`) and the last pick stands. A pick holds until the next.
    pub fn choose_loadout(&mut self, player: u8, row: u8) -> bool {
        let Some(l) = self.ext.loadout.get(player as usize).copied() else {
            return false;
        };
        let rows = self.ext.picks.iter().find(|o| o.slot == l.picker).map_or(0, |o| o.rows.len());
        if row >= l.count || row as usize >= rows {
            return false;
        }
        self.ext.loadout[player as usize].row = row;
        true
    }

    /// A string of the script's table, as an offer names its title and rows.
    pub fn string(&self, id: u16) -> Option<&str> {
        self.ir.strings.get(id as usize).map(String::as_str)
    }

    /// The host reports that `player` picked `row` of picker `slot`. A
    /// pick of a picker not offered, a row it does not hold or a player
    /// outside the roster is refused (`false`); a player's second pick of
    /// the picker replaces the first. A pick holds until the round's end.
    pub fn choose(&mut self, player: u8, slot: u8, row: u8) -> bool {
        let offered = self
            .ext
            .picks
            .iter()
            .find(|o| o.slot == slot)
            .is_some_and(|o| (row as usize) < o.rows.len());
        if !offered || player as usize >= MAX_PLAYERS {
            return false;
        }
        match self.ext.choices.iter_mut().find(|c| c.player == player && c.slot == slot) {
            Some(c) => c.row = row,
            None => self.ext.choices.push(Choice { player, slot, row }),
        }
        true
    }

    /// The sight table (`sees`), in the order its pairs were first asked.
    pub fn sight(&self) -> &[Sight] {
        &self.ext.sight
    }

    /// Records a boundary trace if the ordered pair is still in the table.
    pub fn record_sight(&mut self, from: u32, to: u32, clear: bool) {
        if let Some(pair) = self.ext.sight.iter_mut().find(|p| p.from == from && p.to == to) {
            pair.clear = clear;
        }
    }

    /// The areas shown in play (`show_area`): each zone object and the team
    /// whose colour its wall takes, `None` for neutral.
    pub fn shown_areas(&self) -> &[(u32, Option<u8>)] {
        &self.ext.shown
    }

    /// The script's HUD widgets and waypoints: what every machine that
    /// runs the VM draws ([`crate::show::view`]).
    pub fn show(&self) -> &crate::show::State {
        &self.ext.show
    }

    /// Team `team`'s body (`set_body`): what the host's spawn hook makes its
    /// players spawn as. `None`: their own.
    pub fn body(&self, team: u8) -> Option<Body> {
        *self.ext.bodies.get(team as usize)?
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
        let o = object_value;
        match input {
            Input::Init | Input::RoundStart | Input::RoundEnd => vec![],
            Input::Kill { victim, killer } => vec![p(*victim), op(*killer)],
            Input::Spawn(x)
            | Input::Death(x)
            | Input::Join(x)
            | Input::Quit(x)
            | Input::Bounced(x)
            | Input::Grounded(x) => vec![p(*x)],
            Input::Pickup(x, ob) | Input::Drop(x, ob) | Input::Press(x, ob) | Input::Edit(x, ob) => {
                vec![p(*x), o(*ob)]
            }
            Input::Bounce {
                player,
                spot,
                chain,
                monster,
            } => vec![
                p(*player),
                o(*spot),
                i32::try_from(*chain).unwrap_or(i32::MAX),
                i32::from(*monster),
            ],
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
            Input::Damage { object, by, amount } => vec![o(*object), op(*by), *amount],
            Input::Touchdown { player, down, up } => vec![p(*player), *down, *up],
            Input::Launch { player, spot, speed } => vec![p(*player), spot.map_or(NONE, o), *speed],
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
                holes: Vec::new(),
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
        Ty::Object => Arg::Object(object_id(v)),
        Ty::Squad => Arg::Squad(opt_u8(v).unwrap_or(0)),
        Ty::Class => Arg::Class(string(&ir.strings, v)),
        Ty::Tag => Arg::Tag(usize::try_from(v).ok().and_then(|i| ir.tags.get(i)).map(|t| t.path.clone())),
        Ty::Label => Arg::Label(
            usize::try_from(v)
                .ok()
                .and_then(|i| ir.labels.get(i))
                .map(|l| l.text.clone())
                .unwrap_or_default(),
        ),
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
    /// The values `fmt_*` calls queued for the next `message`, at most
    /// [`caps::MAX_HOLES`] (the verifier's message rule).
    holes: Vec<Arg>,
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
                    if let Some(hole) = caps::Hole::of_entry(entry) {
                        let r = args[0];
                        self.hole(hole, hd.regs[r as usize], regs[r as usize]);
                        continue;
                    }
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
                    Scope::Object => object_id(o).and_then(|i| self.ext.objects.get(&i)),
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
                if let Some(id) = object_id(o) {
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
                // The label's max bounds the set on any map, checked or not:
                // the verifier's estimate counts no more.
                let def = &self.ir.labels[l as usize];
                let max = (def.max as usize).min(MAX_LABELLED);
                let mut placed = game.labelled(&def.text);
                placed.sort();
                placed.dedup_by_key(|p| p.object);
                placed
                    .into_iter()
                    .map(|p| object_value(p.object))
                    .take(max)
                    .collect()
            }
            SetBase::Objectives => {
                let mut list = game.objectives();
                list.sort_unstable();
                list.dedup();
                list.into_iter()
                    .map(object_value)
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
                    caps::id::VEC_LENGTH => fixed::length(raw[0], raw[1], raw[2]),
                    // The table's answer: a pair asked for the first time
                    // reads false and is traced at this tick's end. A
                    // `none` or an object that is gone sees nothing.
                    caps::id::SEES => {
                        let (Some(from), Some(to)) = (object_id(raw[0]), object_id(raw[1])) else {
                            return Ok(Some(0));
                        };
                        let tick = self.ext.tick;
                        let full = self.ext.sight.len() >= MAX_SIGHT_PAIRS;
                        match self.ext.sight.iter_mut().find(|t| t.from == from && t.to == to) {
                            Some(t) => {
                                t.asked = tick;
                                t.clear as i32
                            }
                            None if full => return Err(ErrorKind::Sight),
                            None => {
                                self.ext.sight.push(Sight { from, to, asked: tick, clear: false });
                                0
                            }
                        }
                    }
                    // The player's row of the picker, or -1: none made, none
                    // offered, or a player or picker out of range.
                    caps::id::PICKED => match (u8::try_from(raw[0]), u8::try_from(raw[1])) {
                        (Ok(player), Ok(slot)) => self
                            .ext
                            .choices
                            .iter()
                            .find(|c| c.player == player && c.slot == slot)
                            .map_or(-1, |c| i32::from(c.row)),
                        _ => -1,
                    },
                    // The row the player last picked in the loadout window,
                    // 0 when none was; -1 for a player out of range.
                    caps::id::CHOICE => u8::try_from(raw[0])
                        .ok()
                        .and_then(|p| self.ext.loadout.get(p as usize))
                        .map_or(-1, |l| i32::from(l.row)),
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
                        let (Some(zone), Ok(team)) = (object_id(raw[0]), usize::try_from(raw[1])) else {
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
                        self.mover(game, e.id, &raw)?;
                        self.gate(game, e.id, &raw)?;
                        self.pick(e.id, &raw)?;
                        self.offer(e.id, &raw)?;
                        self.stand(game, e.id, &raw)?;
                        let mut call = self.resolve(e, &raw);
                        if caps::is_message(e.id) {
                            // The holes' values follow the entry's own
                            // arguments, in hole order.
                            call.args.append(&mut self.holes);
                        }
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

    /// `move_to` and `stop`: the movers in the VM's state. `move_to` starts
    /// the object moving from where the game reads it now, in a straight
    /// line, arriving `t` ticks on (at least 1); the object's earlier mover
    /// is replaced, with no `on arrive`. `stop` ends the object's mover,
    /// with none. `none` for the object does nothing; a seventeenth mover
    /// is a script error.
    fn mover(&mut self, game: &mut dyn Game, entry: u16, raw: &[i32]) -> Result<(), ErrorKind> {
        match (entry, raw) {
            (caps::id::MOVE_TO, &[object, x, y, z, ticks]) => {
                let Some(object) = object_id(object) else {
                    return Ok(());
                };
                let at = |game: &mut dyn Game, entry: u16, axis: i32| {
                    game.read(&Call {
                        entry: &caps::ENTRIES[entry as usize],
                        args: vec![Arg::Object(Some(object)), Arg::Int(axis)],
                    })
                };
                let from = [0, 1, 2].map(|axis| at(game, caps::id::OBJECT_POS, axis));
                let mover = Mover {
                    object,
                    from,
                    to: [x, y, z],
                    start: self.ext.tick,
                    duration: ticks.max(1) as u32,
                };
                let full = self.ext.movers.len() >= MAX_MOVERS;
                match self.ext.movers.iter_mut().find(|m| m.object == object) {
                    Some(m) => *m = mover,
                    None if full => return Err(ErrorKind::Movers),
                    None => self.ext.movers.push(mover),
                }
            }
            (caps::id::STOP, &[object]) => {
                if let Some(object) = object_id(object) {
                    self.ext.movers.retain(|m| m.object != object);
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// `set_open`: opening a closed barrier notes where it stands (as the
    /// game reads it now) and steps it [`GATE_DROP`] below; closing an open
    /// one steps it back to the noted place and forgets it. A barrier
    /// already in the state asked for, and `none`, do nothing; a
    /// seventeenth open barrier is a script error. The steps are the
    /// host's put at the tick boundary, as a `teleport`'s.
    fn gate(&mut self, game: &mut dyn Game, entry: u16, raw: &[i32]) -> Result<(), ErrorKind> {
        let (caps::id::SET_OPEN, &[object, open]) = (entry, raw) else {
            return Ok(());
        };
        let Some(object) = object_id(object) else {
            return Ok(());
        };
        let held = self.ext.gates.iter().position(|g| g.object == object);
        match (open != 0, held) {
            (true, None) => {
                if self.ext.gates.len() >= MAX_GATES {
                    return Err(ErrorKind::Gates);
                }
                let home = [0, 1, 2].map(|axis| {
                    game.read(&Call {
                        entry: &caps::ENTRIES[caps::id::OBJECT_POS as usize],
                        args: vec![Arg::Object(Some(object)), Arg::Int(axis)],
                    })
                });
                let gate = Gate { object, home };
                game.step_object(object, gate.open_at(), [0; 3]);
                self.ext.gates.push(gate);
            }
            (false, Some(i)) => {
                let gate = self.ext.gates.remove(i);
                game.step_object(object, gate.home, [0; 3]);
            }
            _ => {}
        }
        Ok(())
    }

    /// `offer(p, picker, count)`: opens the first `count` rows of `picker`
    /// to the player (1 up to [`MAX_PICK_ROWS`]). A row last picked that the
    /// window still holds stays the player's; else the player is back on
    /// row 0. A picker out of range, a count out of range, or a player out
    /// of the roster is a script error.
    fn offer(&mut self, entry: u16, raw: &[i32]) -> Result<(), ErrorKind> {
        let (caps::id::OFFER, &[player, picker, count]) = (entry, raw) else {
            return Ok(());
        };
        let player = usize::try_from(player).ok().filter(|&p| p < MAX_PLAYERS).ok_or(ErrorKind::Picks)?;
        let picker = u8::try_from(picker).ok().filter(|&s| (s as usize) < MAX_PICKS).ok_or(ErrorKind::Picks)?;
        let count = u8::try_from(count).ok().filter(|&n| n >= 1 && n as usize <= MAX_PICK_ROWS).ok_or(ErrorKind::Picks)?;
        let l = &mut self.ext.loadout[player];
        let row = if l.row < count { l.row } else { 0 };
        *l = Window { picker, count, row };
        Ok(())
    }

    /// `offer_pick` and `pick_row`: the pickers in the VM's state, which the
    /// host opens for the players. `offer_pick(n, title)` makes picker `n`
    /// (0 up to [`MAX_PICKS`]) with no rows, replacing one of that number
    /// and the picks made in it; `pick_row(n, label)` adds a row to it, at
    /// most [`MAX_PICK_ROWS`]. A picker out of range, or a row for a picker
    /// not offered, is a script error. The round's end takes them all.
    fn pick(&mut self, entry: u16, raw: &[i32]) -> Result<(), ErrorKind> {
        let text = |v: i32| u16::try_from(v).ok().filter(|&t| (t as usize) < self.ir.strings.len());
        match (entry, raw) {
            (caps::id::OFFER_PICK, &[slot, title]) => {
                let slot = u8::try_from(slot).ok().filter(|&s| (s as usize) < MAX_PICKS).ok_or(ErrorKind::Picks)?;
                let title = text(title).ok_or(ErrorKind::Picks)?;
                self.ext.choices.retain(|c| c.slot != slot);
                let offer = PickOffer { slot, title, rows: Vec::new() };
                match self.ext.picks.iter_mut().find(|o| o.slot == slot) {
                    Some(o) => *o = offer,
                    None => {
                        self.ext.picks.push(offer);
                        self.ext.picks.sort_by_key(|o| o.slot);
                    }
                }
            }
            (caps::id::PICK_ROW, &[slot, label]) => {
                let label = text(label).ok_or(ErrorKind::Picks)?;
                let offer = u8::try_from(slot)
                    .ok()
                    .and_then(|s| self.ext.picks.iter_mut().find(|o| o.slot == s))
                    .ok_or(ErrorKind::Picks)?;
                if offer.rows.len() >= MAX_PICK_ROWS {
                    return Err(ErrorKind::Picks);
                }
                offer.rows.push(label);
            }
            _ => {}
        }
        Ok(())
    }

    /// The standing rules an action sets in the VM's state, which the host
    /// reads when the engine asks: a team's spawn rule and its body. `none`
    /// for the team sets nothing; `spawn_near` with a count of 0 or less,
    /// or `none` for the zone, lifts the team's rule, and `set_body` with
    /// `none` gives the team its own bodies. A `set_body` tag that is no
    /// body is a script error.
    fn stand(&mut self, game: &dyn Game, entry: u16, raw: &[i32]) -> Result<(), ErrorKind> {
        if entry == caps::id::ROLE_TRAITS {
            if (0..MAX_PLAYERS as i32).contains(&raw[0]) {
                let bit = 1u16 << raw[0];
                if raw[1] != 0 {
                    self.ext.role_mask |= bit;
                } else {
                    self.ext.role_mask &= !bit;
                }
            }
            return Ok(());
        }
        // The script's HUD: a widget or a waypoint is a declaration's
        // number, and one the script does not declare is a script error.
        match entry {
            caps::id::HUD_VALUE
            | caps::id::HUD_FILL
            | caps::id::HUD_STYLE
            | caps::id::HUD_TEXT
            | caps::id::HUD_SHOW => {
                let w = usize::try_from(raw[0])
                    .ok()
                    .filter(|&w| w < self.ir.widgets.len())
                    .and_then(|w| self.ext.show.widget(w))
                    .ok_or(ErrorKind::Hud)?;
                let v = raw[1];
                match entry {
                    caps::id::HUD_VALUE => w.value = v,
                    caps::id::HUD_FILL => w.fill = crate::show::fill_byte(v),
                    caps::id::HUD_STYLE => w.style = v.clamp(0, i32::from(u8::MAX)) as u8,
                    caps::id::HUD_TEXT => w.text = u16::try_from(v).ok(),
                    _ => w.shown = v != 0,
                }
                return Ok(());
            }
            caps::id::HUD_VALUE_FOR => {
                // The player's own value of a widget: none or a seat past
                // the roster sets nothing; a second widget that reads per
                // viewer is a script error.
                let w = usize::try_from(raw[0])
                    .ok()
                    .filter(|&w| w < self.ir.widgets.len())
                    .ok_or(ErrorKind::Hud)?;
                if let Some(slot) = usize::try_from(raw[1]).ok().filter(|&s| s < MAX_PLAYERS)
                    && !self.ext.show.set_for(w, slot, raw[2])
                {
                    return Err(ErrorKind::Hud);
                }
                return Ok(());
            }
            caps::id::MARK | caps::id::MARK_PLAYER | caps::id::MARK_TEAM | caps::id::UNMARK => {
                let at = usize::try_from(raw[0])
                    .ok()
                    .filter(|&w| w < self.ir.waypoints.len())
                    .ok_or(ErrorKind::Hud)?;
                // The audience: `mark` is every viewer; the player form is
                // the slot's bit (a `none` player or one past the roster
                // is an empty mask); the team form is the seated players'
                // bits, from the roster every machine holds. An empty mask
                // draws to nobody.
                let audience = match entry {
                    caps::id::MARK | caps::id::UNMARK => crate::show::EVERYONE,
                    caps::id::MARK_PLAYER => raw
                        .get(2)
                        .and_then(|&p| u8::try_from(p).ok())
                        .filter(|&p| (p as usize) < crate::ty::MAX_PLAYERS)
                        .map(crate::show::player)
                        .unwrap_or(0),
                    _ => raw
                        .get(2)
                        .and_then(|&t| u8::try_from(t).ok())
                        .map(|t| {
                            game.players_of_team(t).iter().fold(0, |mask, &slot| {
                                mask | crate::show::player(slot)
                            })
                        })
                        .unwrap_or(0),
                };
                self.ext.show.waypoints[at] = match raw.get(1).and_then(|&o| object_id(o)) {
                    Some(object) if entry != caps::id::UNMARK => Some(crate::show::Waypoint {
                        at: object,
                        audience,
                    }),
                    _ => None,
                };
                return Ok(());
            }
            _ => {}
        }
        // The areas shown in play lead with a zone, not a team: `show_area`
        // shows it in a team's colour (neutral for `none`) or recolours it,
        // and `hide_area` takes it off. `none` for the zone does nothing; a
        // ninth area is a script error.
        match entry {
            caps::id::SHOW_AREA => {
                let Some(zone) = raw.first().and_then(|&z| object_id(z)) else {
                    return Ok(());
                };
                let team = raw
                    .get(1)
                    .and_then(|&t| u8::try_from(t).ok())
                    .filter(|&t| (t as usize) < MAX_TEAMS);
                let full = self.ext.shown.len() >= MAX_SHOWN_AREAS;
                match self.ext.shown.iter_mut().find(|(z, _)| *z == zone) {
                    Some(row) => row.1 = team,
                    None if full => return Err(ErrorKind::Areas),
                    None => self.ext.shown.push((zone, team)),
                }
                return Ok(());
            }
            caps::id::HIDE_AREA => {
                if let Some(zone) = raw.first().and_then(|&z| object_id(z)) {
                    self.ext.shown.retain(|(z, _)| *z != zone);
                }
                return Ok(());
            }
            _ => {}
        }
        let Some(team) = raw
            .first()
            .and_then(|&t| usize::try_from(t).ok())
            .filter(|&t| t < MAX_TEAMS)
        else {
            return Ok(());
        };
        match entry {
            caps::id::SPAWN_AREAS => {
                if let (Some(label), Some(&number)) = (raw.get(1).and_then(|&l| u16::try_from(l).ok()), raw.get(2)) {
                    self.ext.spawns[team] = Some(Spawns::Areas { label, number });
                }
            }
            caps::id::SPAWN_NEAR => {
                let zone = raw.get(1).and_then(|&z| object_id(z));
                let count = raw.get(2).copied().unwrap_or(0).min(MAX_SPAWN_NEAR);
                self.ext.spawns[team] = match zone {
                    Some(zone) if count > 0 => Some(Spawns::Near {
                        zone,
                        count: count as u8,
                    }),
                    _ => None,
                };
            }
            caps::id::OBJECTIVE => {
                self.ext.objectives[team].text = match raw.get(1) {
                    Some(&t) => Some(
                        u16::try_from(t)
                            .ok()
                            .filter(|&t| (t as usize) < self.ir.strings.len())
                            .ok_or(ErrorKind::Hud)?,
                    ),
                    None => None,
                };
            }
            caps::id::OBJECTIVE_MARK => {
                if let (Some(zone), Some(&kind)) = (raw.get(1).and_then(|&z| object_id(z)), raw.get(2)) {
                    let kind = match kind {
                        0 => None,
                        k => Some(MarkKind::from_code(k).ok_or(ErrorKind::Hud)?),
                    };
                    if !self.ext.objectives[team].mark(zone, kind) {
                        return Err(ErrorKind::Hud);
                    }
                }
            }
            caps::id::METER => {
                if let (Some(&value), Some(&of)) = (raw.get(1), raw.get(2)) {
                    self.ext.objectives[team].meter = (of > 0).then_some((value.clamp(0, of), of));
                }
            }
            caps::id::COUNTDOWN => {
                if let Some(&ticks) = raw.get(1) {
                    self.ext.objectives[team].ends =
                        (ticks > 0).then(|| self.ext.tick.saturating_add(ticks as u32));
                }
            }
            caps::id::SET_BODY => {
                let path = raw
                    .get(1)
                    .and_then(|&t| usize::try_from(t).ok())
                    .and_then(|i| self.ir.tags.get(i))
                    .map(|t| t.path.as_str());
                self.ext.bodies[team] = match path {
                    None => None,
                    Some(p) => Some(crate::bodies::of_path(p).ok_or(ErrorKind::Body)?),
                };
            }
            _ => {}
        }
        Ok(())
    }

    /// A `fmt_*` call: queues the value of register type `ty` for the next
    /// `message`. A player or team shows as itself, an int or a time as
    /// its number (the template's hole says which), and an enum as its
    /// variant's name.
    fn hole(&mut self, hole: caps::Hole, ty: Ty, v: i32) {
        let arg = match (hole, ty) {
            (caps::Hole::Enum, Ty::Enum(e)) => {
                let name = self
                    .ir
                    .enums
                    .get(e as usize)
                    .and_then(|d| usize::try_from(v).ok().and_then(|i| d.variants.get(i)))
                    .and_then(|&s| self.ir.strings.get(s as usize))
                    .cloned()
                    .unwrap_or_default();
                Arg::Text(name)
            }
            _ => to_arg(self.ir, ty, v),
        };
        // The verifier bounds a run at MAX_HOLES; the cap keeps the VM's
        // own promise.
        if self.holes.len() < caps::MAX_HOLES {
            self.holes.push(arg);
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
    fn a_labelled_set_visits_at_most_its_label_max() {
        let src = "rules \"Bound\" 1\ntype slayer\nlabel crate: objects\nglobal var n: int = 0\n\
                   on tick { for each crate o { n = n + 1 } }\n";
        let run = |max: Option<u16>, placed: u32| {
            let (mut ir, _) = crate::build(src).unwrap();
            assert_eq!(ir.labels[0].max as usize, MAX_LABELLED);
            if let Some(max) = max {
                ir.labels[0].max = max;
            }
            let mut vm = Vm::new(ir, 0, 0).unwrap();
            let mut g = crate::fake::FakeGame::with_players(0);
            for i in 0..placed {
                g.label("crate", 1000 + i, 0, i);
            }
            vm.begin_tick(1);
            vm.update(&mut g);
            vm.global("n")
        };
        // The map holds 10; the label allows 3, so the loop visits 3.
        assert_eq!(run(Some(3), 10), Some(3));
        assert_eq!(run(Some(3), 2), Some(2));
        // With no stated limit the set visits the first 256.
        assert_eq!(run(None, 300), Some(MAX_LABELLED as i32));
    }

    #[test]
    fn below_stays_in_range() {
        let mut s = 7u64;
        for n in 1..50u64 {
            assert!(below(&mut s, n) < n);
        }
    }

    #[test]
    fn host_writes_change_only_the_named_slots() {
        let src = "rules \"Write\" 1\ntype slayer\n\
                   global var pool: int = 7\nplayer var stock: int = 3\n\
                   on tick { pool = pool }\n";
        let (ir, _) = crate::build(src).unwrap();
        let mut vm = Vm::new(ir, 0, 0).unwrap();
        assert!(vm.set_global("pool", 4));
        assert_eq!(vm.global("pool"), Some(4));
        assert!(!vm.set_global("none_such", 1), "an unknown global");

        assert!(vm.set_player_var(2, "stock", 0));
        assert_eq!(vm.player_var(2, "stock"), Some(0));
        assert_eq!(vm.player_var(1, "stock"), Some(3), "another player's row");
        assert!(!vm.set_player_var(2, "none_such", 1), "an unknown variable");
        assert!(!vm.set_player_var(16, "stock", 1), "outside the roster");
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
            role_mask: 0x8002,
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
            spawns: [
                Some(Spawns::Areas { label: 0, number: -2 }),
                None,
                Some(Spawns::Near { zone: 0x4001_0003, count: 4 }),
                None,
                None,
                None,
                None,
                None,
            ],
            bodies: [Some(Body::Elite), Some(Body::Spartan), None, None, None, None, None, None],
            objectives: {
                let mut o = [Objective::default(); MAX_TEAMS];
                o[1] = Objective {
                    text: None,
                    marks: [Some((0x4001_0003, MarkKind::Attack)), Some((7, MarkKind::Defend)), None],
                    meter: Some((12, 30)),
                    ends: Some(900),
                };
                o
            },
            shown: vec![(0x4001_0003, Some(1)), (0x4001_0007, None)],
            show: {
                let mut show = crate::show::State::default();
                show.widgets[2] = Some(crate::show::Widget {
                    shown: true,
                    value: -4,
                    fill: 200,
                    style: 3,
                    text: Some(1),
                    audience: 0b101,
                });
                show.waypoints[1] = Some(crate::show::Waypoint { at: 0x4001_0003, audience: 3 });
                show
            },
            movers: vec![Mover {
                object: 0x4001_0009,
                from: [1, -2, 3],
                to: [-65_536, 65_536, 0],
                start: 7,
                duration: 30,
            }],
            sight: vec![
                Sight { from: 0x4001_0001, to: 0x4001_0002, asked: 40, clear: true },
                Sight { from: 0x4001_0002, to: 0x4001_0001, asked: 41, clear: false },
            ],
            gates: vec![Gate { object: 0x4001_0003, home: [65_536, -2, 3] }],
            picks: vec![
                PickOffer { slot: 0, title: 3, rows: vec![4, 5, 6] },
                PickOffer { slot: 1, title: 7, rows: vec![8] },
            ],
            choices: vec![
                Choice { player: 2, slot: 0, row: 2 },
                Choice { player: 0, slot: 1, row: 0 },
            ],
            loadout: std::array::from_fn(|i| Window { picker: (i % MAX_PICKS) as u8, count: 3, row: (i % 3) as u8 }),
        }
    }

    #[test]
    fn state_refuses_unsorted_zones() {
        let mut e = ext();
        e.held.clear();
        e.shown.clear();
        // Two zones, 12 before 5: rewrite the tail by hand.
        e.show = crate::show::State::default();
        let bytes = e.encode();
        let show = e.show.encode();
        // Cut the held count, the shown count and the show state.
        let mut bad = bytes[..bytes.len() - 3 - show.len()].to_vec();
        bad.push(2);
        for zone in [12u32, 5] {
            bad.extend_from_slice(&zone.to_le_bytes());
            bad.extend_from_slice(&[0; 4 * MAX_TEAMS]);
        }
        bad.push(0);
        bad.push(show.len() as u8);
        bad.extend_from_slice(&show);
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
    fn state_refuses_a_spawn_rule_it_does_not_know() {
        let bytes = ext().encode();
        // The spawn rules sit before the held counters: team 0's kind byte
        // is the first of them.
        // After the spawn rules and bodies come the held counters, the
        // shown zones, the show state and the movers (36 bytes each).
        let e = ext();
        let held = 1 + 2 * (4 + 4 * MAX_TEAMS)
            + (1 + 2 * (4 + 1))
            + (1 + e.show.encode().len())
            + (1 + 36 * e.movers.len())
            + (1 + 13 * e.sight.len())
            + (1 + 16 * e.gates.len())
            + (1 + e.picks.iter().map(|o| 4 + 2 * o.rows.len()).sum::<usize>())
            + (1 + 3 * e.choices.len())
            + 3 * MAX_PLAYERS
            + e.objectives
                .iter()
                .map(|o| {
                    2 + o.marks.iter().map(|m| if m.is_some() { 5 } else { 1 }).sum::<usize>()
                        + if o.meter.is_some() { 9 } else { 1 }
                        + if o.ends.is_some() { 5 } else { 1 }
                })
                .sum::<usize>();
        let rules = (1 + 2 + 4) + (MAX_TEAMS - 2) + (1 + 4 + 1);
        let first = bytes.len() - held - MAX_TEAMS - rules;
        assert_eq!(bytes[first], 1, "team 0 has an Areas rule");
        let mut bad = bytes.clone();
        bad[first] = 3;
        assert!(RulesExt::decode(&bad).is_err());
        // A body byte that is no body.
        let mut bad = bytes.clone();
        bad[bytes.len() - held - MAX_TEAMS] = 3;
        assert!(RulesExt::decode(&bad).is_err());
        // A Near rule keeps at least one location.
        let mut e = ext();
        e.spawns[2] = Some(Spawns::Near { zone: 1, count: 0 });
        assert!(RulesExt::decode(&e.encode()).is_err());
    }

    /// `spawn_near` and `spawn_areas` set the team's rule in the state the
    /// host's spawn hook reads; a count of 0 lifts it, and the round's end
    /// ends them all.
    #[test]
    fn spawn_rules_stand_in_the_state_until_the_round_ends() {
        let src = "rules \"Lines\" 1\ntype slayer\nteams 2\nrequires player.spawn\n\
                   label goal: zone\nlabel attack: spawns\nglobal var phase: int = 0\n\
                   on round_start { spawn_near(team 1, goal, 3)\n spawn_areas(team 0, attack, 2) }\n\
                   on tick { phase = phase + 1\n if phase == 2 { spawn_near(team 1, goal, 0) } }\n";
        let (ir, _) = crate::build(src).unwrap();
        let mut vm = Vm::new(ir, 0, 0).unwrap();
        let mut g = crate::fake::FakeGame::with_players(2);
        g.label("goal", 0x4001_0002, 0, 2);
        vm.begin_tick(0);
        vm.fire(&mut g, Input::RoundStart);
        assert_eq!(vm.spawn_rule(1), Some(SpawnRule::Near { zone: 0x4001_0002, count: 3 }));
        assert_eq!(vm.spawn_rule(0), Some(SpawnRule::Areas { label: "attack", number: 2 }));
        assert_eq!(vm.spawn_rule(2), None);
        assert_eq!(vm.spawn_rule(200), None);
        // The rule is in the record: a resume keeps it.
        let back = Vm::resume(vm.ir().clone(), 0, RulesExt::decode(&vm.ext().encode()).unwrap()).unwrap();
        assert_eq!(back.spawn_rule(1), vm.spawn_rule(1));
        vm.begin_tick(1);
        vm.update(&mut g);
        assert!(vm.spawn_rule(1).is_some());
        vm.begin_tick(2);
        vm.update(&mut g);
        assert_eq!(vm.spawn_rule(1), None, "a count of 0 lifts it");
        assert!(vm.spawn_rule(0).is_some());
        vm.fire(&mut g, Input::RoundEnd);
        assert_eq!(vm.spawn_rule(0), None);
    }

    /// The spawn rule is in the record: a checkpoint revert puts an earlier
    /// record back and the team's rule is the one it held then, a rule
    /// set since gone and a rule lifted since back.
    #[test]
    fn a_reverted_record_restores_the_spawn_rule() {
        let src = "rules \"Lines\" 1\ntype slayer\nteams 2\nrequires player.spawn\n\
                   label attack: spawns\nglobal var phase: int = 0\n\
                   on round_start { spawn_areas(team 0, attack, 0) }\n\
                   on tick { phase = phase + 1\n if phase == 2 { spawn_areas(team 0, attack, 1) } }\n";
        let (ir, _) = crate::build(src).unwrap();
        let mut vm = Vm::new(ir, 0, 0).unwrap();
        let mut g = crate::fake::FakeGame::with_players(2);
        vm.begin_tick(0);
        vm.fire(&mut g, Input::RoundStart);
        let checkpoint = vm.ext().encode();
        vm.begin_tick(1);
        vm.update(&mut g);
        vm.begin_tick(2);
        vm.update(&mut g);
        assert_eq!(vm.spawn_rule(0), Some(SpawnRule::Areas { label: "attack", number: 1 }));
        let back = Vm::resume(vm.ir().clone(), 0, RulesExt::decode(&checkpoint).unwrap()).unwrap();
        assert_eq!(back.spawn_rule(0), Some(SpawnRule::Areas { label: "attack", number: 0 }));
        assert_eq!(back.spawn_rule(1), None);
    }

    /// The objective entries stand in the state per team until the round
    /// ends: the line, a mark, the gauge and the clock's end tick; a fourth
    /// mark or an unknown kind stops the script.
    #[test]
    fn objective_entries_stand_in_the_state_until_the_round_ends() {
        let src = "rules \"Obj\" 1\ntype slayer\nteams 2\nrequires hud.objective\n\
                   label goal: zone\n\
                   on round_start { objective(team 1, \"Hold it\")\n objective_mark(team 1, goal, 2)\n\
                   meter(team 1, 5, 20)\n countdown(team 1, seconds(3)) }\n";
        let (ir, _) = crate::build(src).unwrap();
        let mut vm = Vm::new(ir, 0, 0).unwrap();
        let mut g = crate::fake::FakeGame::with_players(2);
        g.label("goal", 0x4001_0002, 0, 1);
        vm.begin_tick(4);
        vm.fire(&mut g, Input::RoundStart);
        assert_eq!(g.script_error, None);
        let o = vm.ext().objectives[1];
        assert_eq!(o.text.map(|t| vm.ir().strings[t as usize].as_str()), Some("Hold it"));
        assert_eq!(o.marked().collect::<Vec<_>>(), [(0x4001_0002, MarkKind::Defend)]);
        assert_eq!(o.meter, Some((5, 20)));
        assert_eq!(o.left(5), Some(89));
        assert!(vm.ext().objectives[0].is_empty());
        // The record keeps it.
        let back = RulesExt::decode(&vm.ext().encode()).unwrap();
        assert_eq!(back.objectives, vm.ext().objectives);
        vm.fire(&mut g, Input::RoundEnd);
        assert!(vm.ext().objectives.iter().all(Objective::is_empty));
    }

    #[test]
    fn a_mark_lifts_by_kind_zero_and_a_fourth_mark_is_refused() {
        let mut o = Objective::default();
        assert!(o.mark(1, Some(MarkKind::Attack)));
        assert!(o.mark(2, Some(MarkKind::Defend)));
        assert!(o.mark(1, Some(MarkKind::Defend)), "a zone marked again is changed");
        assert!(o.mark(3, Some(MarkKind::Attack)));
        assert!(!o.mark(4, Some(MarkKind::Attack)), "a fourth mark does not fit");
        assert!(o.mark(2, None));
        assert_eq!(
            o.marked().collect::<Vec<_>>(),
            [(1, MarkKind::Defend), (3, MarkKind::Attack)]
        );
        assert!(o.mark(4, Some(MarkKind::Attack)));
        assert!(o.mark(9, None), "lifting an unmarked zone does nothing");
    }

    /// `set_body` sets the team's body in the state, `none` gives the
    /// players their own, and a tag that is no body stops the script.
    #[test]
    fn a_teams_body_stands_in_the_state_and_a_wrong_tag_stops_the_script() {
        let src = "rules \"Bodies\" 1\ntype slayer\nteams 2\nrequires player.disguise\n\
                   table kinds: (side: team, body: tag) = [(team 0, \"objects/characters/elite/elite\")]\n\
                   global var phase: int = 0\n\
                   on round_start { set_body(team 1, \"objects/characters/elite/elite_mp\")\n\
                   set_body(team 0, \"Objects/Characters/Masterchief/Masterchief_MP\") }\n\
                   on tick { phase = phase + 1\n if phase == 1 { set_body(team 0, none) }\n\
                   if phase == 2 { set_body(team 1, kinds[team 0].body) } }\n";
        let (ir, _) = crate::build(src).unwrap();
        let mut vm = Vm::new(ir, 0, 0).unwrap();
        let mut g = crate::fake::FakeGame::with_players(2);
        vm.begin_tick(0);
        vm.fire(&mut g, Input::RoundStart);
        assert_eq!(vm.body(1), Some(Body::Elite));
        assert_eq!(vm.body(0), Some(Body::Spartan), "in any case");
        assert_eq!(vm.body(2), None);
        vm.begin_tick(1);
        vm.update(&mut g);
        assert_eq!(vm.body(0), None, "none: their own");
        assert_eq!(g.script_error, None);
        vm.begin_tick(2);
        vm.update(&mut g);
        assert_eq!(g.script_error.map(|e| e.kind), Some(ErrorKind::Body));
        assert_eq!(vm.body(1), Some(Body::Elite), "the refused call changed nothing");
    }

    /// `show_area` and `hide_area` keep at most eight areas in the state, in
    /// the order first shown: showing one again recolours it, `none` is
    /// neutral, the round's end clears them and a ninth stops the script.
    #[test]
    fn shown_areas_stand_in_the_state_until_the_round_ends() {
        let src = "rules \"Walls\" 1\ntype slayer\nteams 2\n\
                   label goal: zones numbered\nglobal var phase: int = 0\n\
                   on round_start { for each goal z { show_area(z, team 1) }\n\
                   if let first = goal[0] { show_area(first, none) } }\n\
                   on tick { phase = phase + 1\n if phase == 1 { if let second = goal[1] { hide_area(second) } }\n\
                   if phase == 2 { for each goal z { show_area(z, team 0) } } }\n";
        let (ir, _) = crate::build(src).unwrap();
        let mut vm = Vm::new(ir, 0, 0).unwrap();
        let mut g = crate::fake::FakeGame::with_players(2);
        for n in 0..3 {
            g.label("goal", 0x4001_0000 + n as u32, n, 2);
        }
        vm.begin_tick(0);
        vm.fire(&mut g, Input::RoundStart);
        assert_eq!(
            vm.shown_areas(),
            [(0x4001_0000, None), (0x4001_0001, Some(1)), (0x4001_0002, Some(1))],
            "the first recoloured to neutral, in the order first shown"
        );
        assert_eq!(g.shown, vm.shown_areas(), "the fake keeps the same");
        // The record carries them: a resume has the same walls.
        let back = Vm::resume(vm.ir().clone(), 0, RulesExt::decode(&vm.ext().encode()).unwrap()).unwrap();
        assert_eq!(back.shown_areas(), vm.shown_areas());
        vm.begin_tick(1);
        vm.update(&mut g);
        assert_eq!(vm.shown_areas(), [(0x4001_0000, None), (0x4001_0002, Some(1))]);
        vm.begin_tick(2);
        vm.update(&mut g);
        assert_eq!(
            vm.shown_areas(),
            [(0x4001_0000, Some(0)), (0x4001_0002, Some(0)), (0x4001_0001, Some(0))]
        );
        vm.fire(&mut g, Input::RoundEnd);
        g.after(&Input::RoundEnd);
        assert!(vm.shown_areas().is_empty());
        assert!(g.shown.is_empty());
    }

    #[test]
    fn a_ninth_shown_area_stops_the_script() {
        let src = "rules \"Walls\" 1\ntype slayer\nlabel goal: zones numbered\n\
                   on round_start { for each goal z { show_area(z, none) } }\n";
        let (ir, _) = crate::build(src).unwrap();
        let mut vm = Vm::new(ir, 0, 0).unwrap();
        let mut g = crate::fake::FakeGame::with_players(1);
        for n in 0..9 {
            g.label("goal", 0x4001_0000 + n as u32, n, 2);
        }
        vm.begin_tick(0);
        vm.fire(&mut g, Input::RoundStart);
        assert_eq!(g.script_error.map(|e| e.kind), Some(ErrorKind::Areas));
        assert_eq!(vm.shown_areas().len(), MAX_SHOWN_AREAS);
    }

    #[test]
    fn state_refuses_a_shown_area_twice_or_too_many() {
        let mut e = ext();
        e.shown = vec![(1, None), (1, Some(0))];
        assert!(RulesExt::decode(&e.encode()).is_err());
        let mut e = ext();
        e.shown = (0..MAX_SHOWN_AREAS as u32 + 1).map(|z| (z, None)).collect();
        assert!(RulesExt::decode(&e.encode()).is_err());
    }

    #[test]
    fn a_mover_walks_a_straight_line_and_ends_exactly_on_its_target() {
        let m = Mover {
            object: 1,
            from: [0, 65_536, -65_536],
            to: [10 * 65_536, 65_536, 65_536],
            start: 4,
            duration: 10,
        };
        assert_eq!(m.end(), 14);
        assert_eq!(m.at(4), m.from);
        assert_eq!(m.at(9), [5 * 65_536, 65_536, 0]);
        assert_eq!(m.at(14), m.to);
        assert_eq!(m.at(1_000), m.to, "past the end it stays on the target");
        // 10 units in 10 ticks at 30 ticks a second: 30 units a second.
        assert_eq!(m.velocity(), [30 * 65_536, 0, 6 * 65_536]);
        // A third of a unit is rounded toward the start, the same on every
        // machine.
        let third = Mover {
            object: 2,
            from: [0; 3],
            to: [10, -10, 0],
            start: 0,
            duration: 3,
        };
        assert_eq!(third.at(1), [3, -3, 0]);
        assert_eq!(third.at(2), [6, -6, 0]);
    }

    #[test]
    fn state_refuses_a_mover_with_no_time_none_for_an_object_twice_or_too_many() {
        let bad = |f: &dyn Fn(&mut Vec<Mover>)| {
            let mut e = ext();
            f(&mut e.movers);
            assert!(RulesExt::decode(&e.encode()).is_err());
        };
        bad(&|m| m[0].duration = 0);
        bad(&|m| m[0].object = u32::MAX);
        bad(&|m| {
            let again = m[0];
            m.push(again);
        });
        bad(&|m| {
            *m = (0..MAX_MOVERS as u32 + 1)
                .map(|o| Mover { object: o, ..m[0] })
                .collect()
        });
        let e = ext();
        assert_eq!(RulesExt::decode(&e.encode()).unwrap().movers, e.movers);
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
            role_mask: u16::MAX,
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
            spawns: [Some(Spawns::Near { zone: u32::MAX, count: u8::MAX }); MAX_TEAMS],
            bodies: [Some(Body::Elite); MAX_TEAMS],
            objectives: [Objective {
                text: Some(u16::MAX - 1),
                marks: [Some((u32::MAX, MarkKind::Defend)); MAX_OBJECTIVE_MARKS],
                meter: Some((i32::MIN, i32::MAX)),
                ends: Some(u32::MAX),
            }; MAX_TEAMS],
            shown: (0..MAX_SHOWN_AREAS as u32).map(|z| (z, Some(7))).collect(),
            show: crate::show::State {
                widgets: [Some(crate::show::Widget {
                    shown: true,
                    value: i32::MIN,
                    fill: 255,
                    style: 255,
                    text: Some(u16::MAX),
                    audience: u16::MAX,
                }); crate::show::MAX_WIDGETS],
                waypoints: [Some(crate::show::Waypoint { at: u32::MAX, audience: u16::MAX });
                    crate::show::MAX_WAYPOINTS],
                per_viewer: Some(crate::show::PerViewer {
                    widget: 15,
                    values: [u8::MAX; MAX_PLAYERS],
                }),
            },
            movers: (0..MAX_MOVERS as u32)
                .map(|o| Mover {
                    object: o,
                    from: [i32::MIN; 3],
                    to: [i32::MAX; 3],
                    start: u32::MAX,
                    duration: u32::MAX,
                })
                .collect(),
            sight: (0..MAX_SIGHT_PAIRS as u32)
                .map(|o| Sight { from: o, to: o + 1, asked: u32::MAX, clear: true })
                .collect(),
            gates: (0..MAX_GATES as u32).map(|o| Gate { object: o, home: [i32::MIN; 3] }).collect(),
            picks: (0..MAX_PICKS as u8)
                .map(|slot| PickOffer { slot, title: u16::MAX, rows: vec![u16::MAX; MAX_PICK_ROWS] })
                .collect(),
            choices: (0..MAX_PICKS as u8)
                .flat_map(|slot| (0..MAX_PLAYERS as u8).map(move |player| Choice { player, slot, row: MAX_PICK_ROWS as u8 - 1 }))
                .collect(),
            loadout: [Window { picker: MAX_PICKS as u8 - 1, count: MAX_PICK_ROWS as u8, row: MAX_PICK_ROWS as u8 - 1 }; MAX_PLAYERS],
        };
        let bytes = e.encode();
        assert!(bytes.len() <= MAX_STATE_BYTES, "{} bytes", bytes.len());
        assert_eq!(RulesExt::decode(&bytes), Ok(e));
    }

    #[test]
    fn a_bot_order_changes_no_rulesext_byte() {
        // One script, its kill handler ordering the killer to follow the
        // victim (docs/game/rules-world-api.md, "Bots"): after the order's
        // queue entry drains at the next tick's flush, the state is
        // byte-identical to the run whose killer was `none` and made no
        // order. The runs share one IR, so the rng (`seed`, from the IR's
        // hash) and the slot table match; only the order differs.
        let src = "rules \"Orders\" 1\ntype slayer\nrequires bots.orders\non kill(victim, killer) { if killer != none { bot_follow(killer, victim) } }\n";
        let run = |killer: Option<u8>| {
            let (ir, _) = crate::build(src).unwrap();
            let mut vm = Vm::new(ir, 0, 0).unwrap();
            let mut g = crate::fake::FakeGame::with_players(2);
            vm.begin_tick(1);
            vm.fire(&mut g, Input::Kill { victim: 0, killer });
            vm.begin_tick(2);
            vm.flush_queue(&mut g);
            (vm.ext().encode(), g)
        };
        let (with, g) = run(Some(1));
        assert_eq!(g.calls_to("bot_follow").len(), 1, "the order was made");
        let (without, g) = run(None);
        assert!(g.calls_to("bot_follow").is_empty(), "no killer, no order");
        assert_eq!(with, without);
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
