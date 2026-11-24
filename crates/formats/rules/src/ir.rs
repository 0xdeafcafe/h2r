//! The rules IR: one blob a compiler writes and a verifier and VM read.
//!
//! The binary form is canonical: [`Ir::decode`] accepts exactly the bytes
//! [`Ir::encode`] writes, so the SHA-256 of the encoding ([`Ir::id`]) names
//! a script everywhere. See `docs/game/rules-engine.md`, "The IR and the VM".

use content::Sha256;

use crate::ty::{Sizes, Ty};
use crate::types::Type;

/// The blob's first four bytes.
pub const MAGIC: [u8; 4] = *b"H2RL";
/// The IR version this build reads. It changes when ops change.
pub const IR_VERSION: u16 = 1;
/// The most bytes a blob may have.
pub const MAX_IR_BYTES: usize = 256 * 1024;
/// Capabilities a header may declare.
pub const MAX_REQUIRES: usize = 32;
/// Registers one handler may use.
pub const MAX_REGS: usize = 256;
/// Ops in one handler.
pub const MAX_OPS: usize = 32_768;
/// Option bits a variant record carries for a script.
pub const OPTION_BITS: u32 = 32;
/// Bytes of a label, and of a message.
pub const MAX_LABEL_BYTES: usize = 32;
pub const MAX_TEXT_BYTES: usize = 128;
pub const MAX_TAG_BYTES: usize = 256;
/// Per-scope slot budgets.
pub const MAX_GLOBAL_SLOTS: usize = 256;
pub const MAX_PLAYER_SLOTS: usize = 64;
pub const MAX_TEAM_SLOTS: usize = 32;
pub const MAX_OBJECT_SLOTS: usize = 32;
/// Rows and columns of a constant table.
pub const MAX_TABLE_ROWS: usize = 256;
pub const MAX_TABLE_COLS: usize = 16;
pub const MAX_PALETTE_TAGS: usize = 64;
/// Labels a script declares at most.
pub const MAX_LABELS: usize = 64;
/// Quick Options marks a script names at most: the pregame panel's rows
/// (`lobby_options::PANEL_ROWS`).
pub const MAX_QUICK: usize = 5;
/// UTF-16 units of a script's `about` text at most: a variant's
/// description (`variants::portable::MAX_DESCRIPTION_UNITS`).
pub const MAX_ABOUT_UNITS: usize = 150;

/// A register index within a handler's frame.
pub type Reg = u8;

/// The whole blob.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ir {
    pub header: Header,
    /// Names, option labels, class names and messages.
    pub strings: Vec<String>,
    pub options: Vec<OptionDef>,
    /// The record settings the script's `settings` block names, in
    /// ascending table index order ([`crate::settings::TABLE`]).
    pub settings: Vec<Setting>,
    /// The Quick Options marks, in the script's order; at most
    /// [`MAX_QUICK`].
    pub quick: Vec<Quick>,
    pub enums: Vec<EnumDef>,
    pub slots: Vec<Slot>,
    /// Forge labels the script declares, and what a map must hold of
    /// each.
    pub labels: Vec<LabelDef>,
    /// Tag paths the script names, each with the group a map's tag at that
    /// path must be.
    pub tags: Vec<TagDef>,
    pub palettes: Vec<Palette>,
    pub tables: Vec<Table>,
    /// The HUD widgets the script declares (`hud`), at most
    /// [`MAX_WIDGETS`]: a widget's number is its index here.
    pub widgets: Vec<WidgetDef>,
    /// The waypoints the script declares (`waypoint`), at most
    /// [`MAX_WAYPOINTS`]: a waypoint's number is its index here.
    pub waypoints: Vec<WaypointDef>,
    pub handlers: Vec<Handler>,
}

/// HUD widgets a script declares at most (the show state's widget table,
/// `crate::show::MAX_WIDGETS`).
pub const MAX_WIDGETS: usize = 16;
/// Waypoints a script declares at most (`crate::show::MAX_WAYPOINTS`).
pub const MAX_WAYPOINTS: usize = 8;

/// What a widget draws as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WidgetKind {
    /// A line of text, or a number the HUD words.
    Text,
    /// The timed centred line.
    Banner,
    /// A bar with a fill.
    Meter,
}

impl WidgetKind {
    pub const ALL: [WidgetKind; 3] = [WidgetKind::Text, WidgetKind::Banner, WidgetKind::Meter];

    pub fn code(self) -> u8 {
        self as u8
    }

    pub fn from_code(c: u8) -> Option<WidgetKind> {
        Self::ALL.into_iter().find(|k| k.code() == c)
    }

    /// The word a `hud` declaration spells it with.
    pub fn word(self) -> &'static str {
        match self {
            WidgetKind::Text => "text",
            WidgetKind::Banner => "banner",
            WidgetKind::Meter => "meter",
        }
    }
}

/// A `hud <name>: <kind> at <slot>` declaration: a place in a HUD of the
/// kit, named by the element's id (`koth.hill`), that the script words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WidgetDef {
    /// The declared name, an index into the string table.
    pub name: u16,
    pub kind: WidgetKind,
    /// The slot, an index into the string table.
    pub slot: u16,
}

/// A `waypoint <name> [at <slot>]` declaration: a mark over a thing, drawn
/// as the navpoint element `slot` names (none: the script's own list).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WaypointDef {
    pub name: u16,
    pub slot: Option<u16>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub ir_version: u16,
    pub api_version: u16,
    pub name: String,
    /// The script's own version number (`rules "Name" 1`).
    pub version: u16,
    /// The game type (`type slayer`): the game type list's row, the
    /// announcer's line at the start and the default of `points`.
    pub game_type: Type,
    /// The built-in game whose rules the script plays under its own record
    /// (`uses slayer`): the script holds settings only. One of the retail
    /// types. See `docs/game/game-types-without-carriers.md`, "The header".
    pub uses: Option<Type>,
    /// Teams the script needs; 0 is any.
    pub teams: u8,
    /// What a point is (`points "Captures"`), for the scoreboard and the
    /// post-game: the type's default when the script names none. At most
    /// [`crate::types::MAX_POINTS_BYTES`].
    pub points: String,
    /// The description (`about "..."`), at most [`MAX_ABOUT_UNITS`]
    /// UTF-16 units; empty when the script has none.
    pub about: String,
    /// Capability names, the most the script can ever do.
    pub requires: Vec<String>,
}

impl Header {
    /// The retail engine index the script's record is on: the `uses`
    /// game's, else the type's ([`Type::engine`]). Every script record
    /// moves to index 2 in GT2 of
    /// `docs/game/game-types-without-carriers.md`.
    pub fn engine(&self) -> u8 {
        self.uses.unwrap_or(self.game_type).engine()
    }
}

/// The header byte of a script with no `uses` line.
const NO_USES: u8 = 0xff;

/// One setting of the `settings` block: a [`crate::settings::TABLE`]
/// index and the value the record stores.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Setting {
    pub index: u16,
    pub value: i32,
}

/// One Quick Options mark: a record setting of the table, or one of the
/// script's options.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quick {
    /// A [`crate::settings::TABLE`] index.
    Setting(u16),
    /// An index into [`Ir::options`].
    Option(u16),
}

impl Quick {
    fn code(self) -> (u8, u16) {
        match self {
            Quick::Setting(i) => (0, i),
            Quick::Option(i) => (1, i),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Global,
    Player,
    Team,
    Object,
}

impl Scope {
    fn code(self) -> u8 {
        self as u8
    }

    fn from_code(c: u8) -> Option<Scope> {
        Some(match c {
            0 => Scope::Global,
            1 => Scope::Player,
            2 => Scope::Team,
            3 => Scope::Object,
            _ => return None,
        })
    }

    /// The register type that owns a slot of this scope.
    pub fn owner(self) -> Option<Ty> {
        match self {
            Scope::Global => None,
            Scope::Player => Some(Ty::Player),
            Scope::Team => Some(Ty::Team),
            Scope::Object => Some(Ty::Object),
        }
    }
}

/// A script option: one row of a variant editor, packed into the record's
/// 32 option bits in declaration order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OptionDef {
    pub name: u16,
    pub label: u16,
    pub description: Option<u16>,
    /// `Int`, `Bool` or an `Enum`.
    pub ty: Ty,
    pub lo: i32,
    pub hi: i32,
    pub default: i32,
}

impl OptionDef {
    /// Bits the option takes in the record.
    pub fn bits(&self) -> u32 {
        let span = (self.hi as i64 - self.lo as i64).max(0) as u64;
        64 - span.leading_zeros()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnumDef {
    pub name: u16,
    pub variants: Vec<u16>,
}

/// A variable slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slot {
    pub name: u16,
    pub scope: Scope,
    pub ty: Ty,
    pub init: i32,
}

/// The characters a label's text may hold: those of
/// `forge_doc::label_char` (lowercase ASCII letters, digits, `_`, `-` and
/// `.`), so a script's label compares byte-exact with a map's.
pub fn label_char(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-' | '.')
}

/// What a label's members are, by the placement classes that satisfy it
/// (`labels::Class`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LabelKind {
    /// Any placement but a starting location.
    Object,
    /// A placement with a volume: a Forge trigger volume or a netgame flag.
    Zone,
    /// Starting locations: a set passed to `spawn_areas`, never visited.
    Spawn,
    /// The points of a flight path: placements numbered from 0 with no
    /// gap, at least two. Passed to `fly`, never visited
    /// (docs/game/rules-world-api.md, "Vehicles and drop ships").
    Path,
}

impl LabelKind {
    pub fn code(self) -> u8 {
        match self {
            LabelKind::Object => 1,
            LabelKind::Zone => 2,
            LabelKind::Spawn => 3,
            LabelKind::Path => 4,
        }
    }

    pub fn from_code(c: u8) -> Option<LabelKind> {
        Some(match c {
            1 => LabelKind::Object,
            2 => LabelKind::Zone,
            3 => LabelKind::Spawn,
            4 => LabelKind::Path,
            _ => return None,
        })
    }
}

/// [`LabelDef::flags`] bits.
pub mod label_flags {
    /// One value, not a set (`label goal: zone`, `label bonus: object?`):
    /// `max` is 1, and `min` 0 for an optional one.
    pub const SINGLE: u8 = 1;
    /// Every member has a number.
    pub const NUMBERED: u8 = 2;
    /// Every member's number is in `lo..=hi` and each number there has a
    /// member. Needs [`NUMBERED`].
    pub const RANGE: u8 = 4;
    /// Each team in play has a member whose team it is.
    pub const PER_TEAM: u8 = 8;
    pub const ALL: u8 = SINGLE | NUMBERED | RANGE | PER_TEAM;
}

/// One label-table entry: the map text and what a map must hold of it.
/// The start gate (`labels::check`) reads it; the verifier and the VM bound
/// a set over it by `max` whatever the map holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LabelDef {
    /// The map's label text.
    pub text: String,
    pub kind: LabelKind,
    /// Members the map must have at least.
    pub min: u16,
    /// Members the map may have at most, and the most a set visits; at
    /// most [`crate::ty::MAX_LABELLED`]. [`crate::ty::MAX_LABELLED`] states
    /// no limit: a map with more is not blocked, and a set visits the first
    /// 256.
    pub max: u16,
    /// [`label_flags`] bits.
    pub flags: u8,
    /// The number range, inclusive, with [`label_flags::RANGE`]; both 0
    /// without it.
    pub lo: i32,
    pub hi: i32,
}

impl LabelDef {
    /// A set of objects that requires nothing of the map: what a label
    /// with no requirements is.
    pub fn objects(text: impl Into<String>) -> LabelDef {
        LabelDef {
            text: text.into(),
            kind: LabelKind::Object,
            min: 0,
            max: crate::ty::MAX_LABELLED as u16,
            flags: 0,
            lo: 0,
            hi: 0,
        }
    }

    pub fn single(&self) -> bool {
        self.flags & label_flags::SINGLE != 0
    }

    pub fn numbered(&self) -> bool {
        self.flags & label_flags::NUMBERED != 0
    }

    pub fn per_team(&self) -> bool {
        self.flags & label_flags::PER_TEAM != 0
    }

    /// The number range, with [`label_flags::RANGE`].
    pub fn range(&self) -> Option<std::ops::RangeInclusive<i32>> {
        (self.flags & label_flags::RANGE != 0).then_some(self.lo..=self.hi)
    }

    /// Whether `max` limits the map; [`crate::ty::MAX_LABELLED`] does not.
    pub fn bounded(&self) -> bool {
        (self.max as usize) < crate::ty::MAX_LABELLED
    }
}

/// The group a tag path must be on the map.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum TagGroup {
    /// Any object definition.
    #[default]
    Any,
    Weapon,
    /// A sound (`snd!`): the map must hold it, which no object type says.
    Sound,
}

impl TagGroup {
    pub fn code(self) -> u8 {
        match self {
            TagGroup::Any => 0,
            TagGroup::Weapon => 1,
            TagGroup::Sound => 2,
        }
    }

    pub fn from_code(c: u8) -> Option<TagGroup> {
        Some(match c {
            0 => TagGroup::Any,
            1 => TagGroup::Weapon,
            2 => TagGroup::Sound,
            _ => return None,
        })
    }
}

/// One tag-table entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagDef {
    pub path: String,
    pub group: TagGroup,
}

impl TagDef {
    /// A path of any object definition.
    pub fn any(path: impl Into<String>) -> TagDef {
        TagDef {
            path: path.into(),
            group: TagGroup::Any,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Palette {
    pub name: u16,
    pub tags: Vec<u16>,
}

/// Constant rows of typed columns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Table {
    pub name: u16,
    pub columns: Vec<Column>,
    /// Row-major, `rows * columns.len()` values.
    pub cells: Vec<i32>,
}

impl Table {
    pub fn rows(&self) -> usize {
        if self.columns.is_empty() {
            0
        } else {
            self.cells.len() / self.columns.len()
        }
    }

    pub fn cell(&self, row: usize, col: usize) -> Option<i32> {
        if row >= self.rows() || col >= self.columns.len() {
            return None;
        }
        self.cells.get(row * self.columns.len() + col).copied()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Column {
    pub name: u16,
    pub ty: Ty,
}

/// The events a handler answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Event {
    Init,
    RoundStart,
    Tick,
    Kill,
    Spawn,
    Death,
    Pickup,
    Drop,
    Enter,
    Leave,
    Press,
    Timer,
    SquadDead,
    ActorKill,
    Bounce,
    Edit,
    Join,
    Quit,
    RoundEnd,
    /// A second player who damaged a killed actor in its last five
    /// seconds (`firefight::director::Kill::assist`).
    ActorAssist,
    /// A fall out of the map during a bounce: the superbounce medal chain's
    /// Bounced (`superbounce_chain::Event::Bounced`).
    Bounced,
    /// A mover reached the end of its `move_to` (`on arrive(o)`): the VM's
    /// own event, fired on the tick the mover ends.
    Arrive,
    /// An object took damage: the object, the player it is credited to and
    /// the amount as a fraction of full vitality (`SimEvent::Damage`).
    Damage,
    /// A player's biped touched down: the speed it fell at on the tick
    /// before and its vertical speed now, both `fixed` world units a
    /// second, `down` positive for a fall and `up` positive for rising.
    Touchdown,
}

impl Event {
    pub const ALL: [Event; 24] = [
        Event::Init,
        Event::RoundStart,
        Event::Tick,
        Event::Kill,
        Event::Spawn,
        Event::Death,
        Event::Pickup,
        Event::Drop,
        Event::Enter,
        Event::Leave,
        Event::Press,
        Event::Timer,
        Event::SquadDead,
        Event::ActorKill,
        Event::Bounce,
        Event::Edit,
        Event::Join,
        Event::Quit,
        Event::RoundEnd,
        Event::ActorAssist,
        Event::Bounced,
        Event::Arrive,
        Event::Damage,
        Event::Touchdown,
    ];

    pub fn code(self) -> u8 {
        self as u8
    }

    pub fn from_code(c: u8) -> Option<Event> {
        Event::ALL.get(c as usize).copied()
    }

    /// The language's name for the event (`on kill`).
    pub fn name(self) -> &'static str {
        match self {
            Event::Init => "init",
            Event::RoundStart => "round_start",
            Event::Tick => "tick",
            Event::Kill => "kill",
            Event::Spawn => "spawn",
            Event::Death => "death",
            Event::Pickup => "pickup",
            Event::Drop => "drop",
            Event::Enter => "enter",
            Event::Leave => "leave",
            Event::Press => "press",
            Event::Timer => "timer",
            Event::SquadDead => "squad_dead",
            Event::ActorKill => "actor_kill",
            Event::Bounce => "bounce",
            Event::Edit => "edit",
            Event::Join => "join",
            Event::Quit => "quit",
            Event::RoundEnd => "round_end",
            Event::ActorAssist => "actor_assist",
            Event::Bounced => "bounced",
            Event::Arrive => "arrive",
            Event::Damage => "damage",
            Event::Touchdown => "touchdown",
        }
    }

    pub fn named(name: &str) -> Option<Event> {
        Event::ALL.iter().copied().find(|e| e.name() == name)
    }

    /// The types of the handler's arguments, in registers 0.. of its frame.
    /// `on timer` names a timer slot instead.
    pub fn args(self) -> &'static [Ty] {
        match self {
            Event::Kill => &[Ty::Player, Ty::Player],
            Event::Spawn | Event::Death | Event::Join | Event::Quit | Event::Bounced => {
                &[Ty::Player]
            }
            Event::Pickup | Event::Drop | Event::Press | Event::Edit => &[Ty::Player, Ty::Object],
            // The chain's count (1 for a single bounce, 2 for a double) and
            // whether it was a Monster Bounce.
            Event::Bounce => &[Ty::Player, Ty::Object, Ty::Int, Ty::Bool],
            Event::Enter | Event::Leave => &[Ty::Object, Ty::Object],
            Event::Arrive => &[Ty::Object],
            Event::SquadDead => &[Ty::Squad],
            Event::Damage => &[Ty::Object, Ty::Player, Ty::Fixed],
            Event::Touchdown => &[Ty::Player, Ty::Fixed, Ty::Fixed],
            Event::ActorKill => &[Ty::Player, Ty::Class],
            Event::ActorAssist => &[Ty::Player, Ty::Player, Ty::Class],
            Event::Init | Event::RoundStart | Event::Tick | Event::Timer | Event::RoundEnd => &[],
        }
    }

    /// How many times the worst-tick estimate assumes the event fires in a
    /// tick: one per player for per-player events, 64 zone events, 16
    /// squads, one otherwise. The assumption of one kill, death or spawn
    /// per player a tick is not verified.
    pub fn worst_per_tick(self) -> u64 {
        match self {
            Event::Kill
            | Event::Spawn
            | Event::Death
            | Event::Pickup
            | Event::Drop
            | Event::Press
            | Event::Bounce
            | Event::Bounced
            | Event::Edit
            | Event::Join
            | Event::Quit
            | Event::ActorKill
            | Event::ActorAssist
            | Event::Damage
            | Event::Touchdown => 16,
            Event::Enter | Event::Leave => 64,
            Event::SquadDead => 16,
            // One a mover, and a script has at most 16 movers.
            Event::Arrive => 16,
            _ => 1,
        }
    }
}

/// One handler: the event it answers, its register types and its code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Handler {
    pub event: Event,
    /// For `on timer`, the global timer slot it answers.
    pub timer: Option<u16>,
    /// The type of every register; the event's arguments come first.
    pub regs: Vec<Ty>,
    pub code: Vec<Op>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumTy {
    Int,
    Fixed,
}

impl NumTy {
    pub fn ty(self) -> Ty {
        match self {
            NumTy::Int => Ty::Int,
            NumTy::Fixed => Ty::Fixed,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArithOp {
    Add,
    Sub,
    Mul,
    /// Division by zero gives 0 and a trace warning.
    Div,
    /// Int only; remainder by zero gives 0.
    Rem,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// The base of a finite set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetBase {
    /// Present players, by roster slot.
    Players,
    /// Teams in play, by index.
    Teams,
    /// Objects with the label-table entry's label, by placement id, at
    /// most the entry's `max`.
    Labelled(u16),
    /// The objective registry's objects (`E+0xc1c`, at most 8), by object
    /// handle.
    Objectives,
}

impl SetBase {
    pub fn elem(self) -> Ty {
        match self {
            SetBase::Players => Ty::Player,
            SetBase::Teams => Ty::Team,
            SetBase::Labelled(_) | SetBase::Objectives => Ty::Object,
        }
    }

    /// The most members the set can have, given the blob's label table: a
    /// labelled set's is its label's `max`.
    pub fn bound(self, labels: &[LabelDef]) -> u64 {
        match self {
            SetBase::Players => crate::ty::MAX_PLAYERS as u64,
            SetBase::Teams => crate::ty::MAX_TEAMS as u64,
            SetBase::Labelled(l) => labels
                .get(l as usize)
                .map_or(crate::ty::MAX_LABELLED, |d| (d.max as usize).min(crate::ty::MAX_LABELLED))
                as u64,
            SetBase::Objectives => crate::ty::MAX_OBJECTIVES as u64,
        }
    }
}

/// A set a `ForEach` visits: a base, or `count` members of it drawn with the
/// VM's generator (`pick`), in draw order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Set {
    pub base: SetBase,
    pub pick: Option<Reg>,
}

/// A register op. Jump targets are op indices in the handler's code and
/// always forward.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    Const { dst: Reg, value: i32 },
    Move { dst: Reg, src: Reg },
    Arith { op: ArithOp, ty: NumTy, dst: Reg, a: Reg, b: Reg },
    Neg { ty: NumTy, dst: Reg, a: Reg },
    /// `Eq` and `Ne` take any two registers of one type; the others ints or
    /// fixed. The result is a bool.
    Cmp { op: CmpOp, dst: Reg, a: Reg, b: Reg },
    Not { dst: Reg, a: Reg },
    /// A slot read. `owner` is the player, team or object register for a
    /// scoped slot; through `none` it reads the slot's initial value.
    Load { dst: Reg, slot: u16, owner: Option<Reg> },
    /// A slot write; through `none` it does nothing.
    Store { slot: u16, owner: Option<Reg>, src: Reg },
    /// A script option's value from the record's option bits.
    Opt { dst: Reg, index: u8 },
    /// A retail record field ([`crate::caps::VARIANT_FIELDS`]).
    Variant { dst: Reg, field: u8 },
    /// A cell of a constant table. A row out of range is a script error,
    /// except -1 (a key [`Op::TableFind`] did not find), which reads the
    /// column's zero value.
    TableRead { dst: Reg, table: u8, row: Reg, col: u8 },
    /// The first row whose first column equals `key`, or -1.
    TableFind { dst: Reg, table: u8, key: Reg },
    Jump { to: u16 },
    JumpIf { cond: Reg, to: u16 },
    JumpIfNot { cond: Reg, to: u16 },
    /// Runs the next `body` ops once per member of `set`, with the member
    /// in `dst`, then continues after them.
    ForEach { set: Set, dst: Reg, body: u16 },
    /// A numbered API entry.
    Call { entry: u16, args: Vec<Reg>, dst: Option<Reg> },
    /// Grants the header's `requires[cap]` to a player.
    Grant { player: Reg, cap: u8, uses: u8, palette: Option<u8> },
    Revoke { player: Reg, cap: u8 },
    Granted { dst: Reg, player: Reg, cap: u8 },
}

/// A blob that does not decode.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    #[error("the blob is {0} bytes, over the {MAX_IR_BYTES}-byte cap")]
    TooLarge(usize),
    #[error("the blob does not start with the rules magic")]
    Magic,
    #[error("the blob ends inside {0}")]
    Truncated(&'static str),
    #[error("{0} bytes follow the blob's code")]
    Trailing(usize),
    #[error("{what} has the invalid value {value}")]
    Invalid { what: &'static str, value: u32 },
    #[error("a string is not UTF-8")]
    Utf8,
}

impl Ir {
    /// The canonical encoding.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer(Vec::new());
        w.bytes(&MAGIC);
        let h = &self.header;
        w.u16(h.ir_version);
        w.u16(h.api_version);
        w.str(&h.name);
        w.u16(h.version);
        w.u8(h.game_type.code());
        w.u8(h.uses.map_or(NO_USES, Type::code));
        w.u8(h.teams);
        w.str(&h.points);
        w.str(&h.about);
        w.len(h.requires.len());
        for r in &h.requires {
            w.str(r);
        }
        w.strs(&self.strings);
        w.len(self.options.len());
        for o in &self.options {
            w.u16(o.name);
            w.u16(o.label);
            w.opt_u16(o.description);
            w.u8(o.ty.code());
            w.i32(o.lo);
            w.i32(o.hi);
            w.i32(o.default);
        }
        w.len(self.settings.len());
        for s in &self.settings {
            w.u16(s.index);
            w.i32(s.value);
        }
        w.len(self.quick.len());
        for q in &self.quick {
            let (kind, index) = q.code();
            w.u8(kind);
            w.u16(index);
        }
        w.len(self.enums.len());
        for e in &self.enums {
            w.u16(e.name);
            w.len(e.variants.len());
            for &v in &e.variants {
                w.u16(v);
            }
        }
        w.len(self.slots.len());
        for s in &self.slots {
            w.u16(s.name);
            w.u8(s.scope.code());
            w.u8(s.ty.code());
            w.i32(s.init);
        }
        w.len(self.labels.len());
        for l in &self.labels {
            w.str(&l.text);
            w.u8(l.kind.code());
            w.u16(l.min);
            w.u16(l.max);
            w.u8(l.flags);
            w.i32(l.lo);
            w.i32(l.hi);
        }
        w.len(self.tags.len());
        for t in &self.tags {
            w.str(&t.path);
            w.u8(t.group.code());
        }
        w.len(self.palettes.len());
        for p in &self.palettes {
            w.u16(p.name);
            w.len(p.tags.len());
            for &t in &p.tags {
                w.u16(t);
            }
        }
        w.len(self.tables.len());
        for t in &self.tables {
            w.u16(t.name);
            w.len(t.columns.len());
            for c in &t.columns {
                w.u16(c.name);
                w.u8(c.ty.code());
            }
            w.len(t.rows());
            for &v in &t.cells {
                w.i32(v);
            }
        }
        w.len(self.widgets.len());
        for d in &self.widgets {
            w.u16(d.name);
            w.u8(d.kind.code());
            w.u16(d.slot);
        }
        w.len(self.waypoints.len());
        for d in &self.waypoints {
            w.u16(d.name);
            w.opt_u16(d.slot);
        }
        w.len(self.handlers.len());
        for hd in &self.handlers {
            w.u8(hd.event.code());
            w.opt_u16(hd.timer);
            w.len(hd.regs.len());
            for r in &hd.regs {
                w.u8(r.code());
            }
            w.len(hd.code.len());
            for op in &hd.code {
                w.op(op);
            }
        }
        w.0
    }

    /// Decodes a blob, refusing anything but the canonical form. A decoded
    /// blob still needs [`crate::verify`] before it runs.
    pub fn decode(bytes: &[u8]) -> Result<Ir, DecodeError> {
        if bytes.len() > MAX_IR_BYTES {
            return Err(DecodeError::TooLarge(bytes.len()));
        }
        let mut r = Reader { bytes, at: 0 };
        if r.take(4, "the magic")? != MAGIC {
            return Err(DecodeError::Magic);
        }
        let ir_version = r.u16("the header")?;
        let api_version = r.u16("the header")?;
        let name = r.str("the header")?;
        let version = r.u16("the header")?;
        let ty = r.u8("the header")?;
        let game_type = Type::from_code(ty).ok_or(DecodeError::Invalid {
            what: "the header's type",
            value: ty as u32,
        })?;
        let uses = match r.u8("the header")? {
            NO_USES => None,
            c => Some(Type::from_code(c).ok_or(DecodeError::Invalid {
                what: "the header's uses",
                value: c as u32,
            })?),
        };
        let teams = r.u8("the header")?;
        let points = r.str("the header")?;
        let about = r.str("the header")?;
        let n = r.len("the requires list")?;
        let mut requires = Vec::with_capacity(n.min(64));
        for _ in 0..n {
            requires.push(r.str("the requires list")?);
        }
        let header = Header {
            ir_version,
            api_version,
            name,
            version,
            game_type,
            uses,
            teams,
            points,
            about,
            requires,
        };
        let strings = r.strs("the string table")?;
        let n = r.len("the option table")?;
        let mut options = Vec::with_capacity(n.min(64));
        for _ in 0..n {
            options.push(OptionDef {
                name: r.u16("the option table")?,
                label: r.u16("the option table")?,
                description: r.opt_u16("the option table")?,
                ty: r.ty("an option type")?,
                lo: r.i32("the option table")?,
                hi: r.i32("the option table")?,
                default: r.i32("the option table")?,
            });
        }
        let n = r.len("the settings")?;
        let mut settings: Vec<Setting> = Vec::with_capacity(n.min(64));
        for _ in 0..n {
            let index = r.u16("the settings")?;
            let value = r.i32("the settings")?;
            // Ascending, so a setting is named once and the order is one.
            if settings.last().is_some_and(|s| s.index >= index) {
                return Err(DecodeError::Invalid {
                    what: "the settings' order",
                    value: index as u32,
                });
            }
            settings.push(Setting { index, value });
        }
        let n = r.len("the quick marks")?;
        if n > MAX_QUICK {
            return Err(DecodeError::Invalid {
                what: "the quick marks' count",
                value: n as u32,
            });
        }
        let mut quick = Vec::with_capacity(n);
        for _ in 0..n {
            let kind = r.u8("the quick marks")?;
            let index = r.u16("the quick marks")?;
            quick.push(match kind {
                0 => Quick::Setting(index),
                1 => Quick::Option(index),
                _ => {
                    return Err(DecodeError::Invalid {
                        what: "a quick mark's kind",
                        value: kind as u32,
                    });
                }
            });
        }
        let n = r.len("the enum table")?;
        let mut enums = Vec::with_capacity(n.min(64));
        for _ in 0..n {
            let name = r.u16("the enum table")?;
            let count = r.len("the enum table")?;
            let mut variants = Vec::with_capacity(count.min(64));
            for _ in 0..count {
                variants.push(r.u16("the enum table")?);
            }
            enums.push(EnumDef { name, variants });
        }
        let n = r.len("the slot table")?;
        let mut slots = Vec::with_capacity(n.min(256));
        for _ in 0..n {
            let name = r.u16("the slot table")?;
            let scope = r.u8("the slot table")?;
            let scope = Scope::from_code(scope).ok_or(DecodeError::Invalid {
                what: "a slot scope",
                value: scope as u32,
            })?;
            slots.push(Slot {
                name,
                scope,
                ty: r.ty("a slot type")?,
                init: r.i32("the slot table")?,
            });
        }
        let n = r.len("the label table")?;
        let mut labels = Vec::with_capacity(n.min(MAX_LABELS));
        for _ in 0..n {
            let text = r.str("the label table")?;
            let kind = r.u8("the label table")?;
            let kind = LabelKind::from_code(kind).ok_or(DecodeError::Invalid {
                what: "a label kind",
                value: kind as u32,
            })?;
            labels.push(LabelDef {
                text,
                kind,
                min: r.u16("the label table")?,
                max: r.u16("the label table")?,
                flags: r.u8("the label table")?,
                lo: r.i32("the label table")?,
                hi: r.i32("the label table")?,
            });
        }
        let n = r.len("the tag table")?;
        let mut tags = Vec::with_capacity(n.min(1024));
        for _ in 0..n {
            let path = r.str("the tag table")?;
            let group = r.u8("the tag table")?;
            let group = TagGroup::from_code(group).ok_or(DecodeError::Invalid {
                what: "a tag group",
                value: group as u32,
            })?;
            tags.push(TagDef { path, group });
        }
        let n = r.len("the palette table")?;
        let mut palettes = Vec::with_capacity(n.min(64));
        for _ in 0..n {
            let name = r.u16("the palette table")?;
            let count = r.len("the palette table")?;
            let mut tags = Vec::with_capacity(count.min(64));
            for _ in 0..count {
                tags.push(r.u16("the palette table")?);
            }
            palettes.push(Palette { name, tags });
        }
        let n = r.len("the constant tables")?;
        let mut tables = Vec::with_capacity(n.min(64));
        for _ in 0..n {
            let name = r.u16("the constant tables")?;
            let cols = r.len("the constant tables")?;
            let mut columns = Vec::with_capacity(cols.min(64));
            for _ in 0..cols {
                columns.push(Column {
                    name: r.u16("the constant tables")?,
                    ty: r.ty("a column type")?,
                });
            }
            let rows = r.len("the constant tables")?;
            let count = rows.checked_mul(cols).ok_or(DecodeError::Truncated("a table"))?;
            if count > r.remaining() / 4 {
                return Err(DecodeError::Truncated("a table"));
            }
            let mut cells = Vec::with_capacity(count);
            for _ in 0..count {
                cells.push(r.i32("a table")?);
            }
            if cols == 0 && rows != 0 {
                return Err(DecodeError::Invalid {
                    what: "a table with no columns",
                    value: rows as u32,
                });
            }
            tables.push(Table {
                name,
                columns,
                cells,
            });
        }
        let n = r.len("the widget table")?;
        if n > MAX_WIDGETS {
            return Err(DecodeError::Invalid {
                what: "the widget count",
                value: n as u32,
            });
        }
        let mut widgets = Vec::with_capacity(n);
        for _ in 0..n {
            let name = r.u16("the widget table")?;
            let kind = r.u8("the widget table")?;
            let kind = WidgetKind::from_code(kind).ok_or(DecodeError::Invalid {
                what: "a widget kind",
                value: kind as u32,
            })?;
            widgets.push(WidgetDef {
                name,
                kind,
                slot: r.u16("the widget table")?,
            });
        }
        let n = r.len("the waypoint table")?;
        if n > MAX_WAYPOINTS {
            return Err(DecodeError::Invalid {
                what: "the waypoint count",
                value: n as u32,
            });
        }
        let mut waypoints = Vec::with_capacity(n);
        for _ in 0..n {
            waypoints.push(WaypointDef {
                name: r.u16("the waypoint table")?,
                slot: r.opt_u16("the waypoint table")?,
            });
        }
        let n = r.len("the handler table")?;
        let mut handlers = Vec::with_capacity(n.min(64));
        for _ in 0..n {
            let event = r.u8("the handler table")?;
            let event = Event::from_code(event).ok_or(DecodeError::Invalid {
                what: "an event",
                value: event as u32,
            })?;
            let timer = r.opt_u16("the handler table")?;
            let count = r.len("a register table")?;
            let mut regs = Vec::with_capacity(count.min(MAX_REGS));
            for _ in 0..count {
                regs.push(r.ty("a register type")?);
            }
            let count = r.len("a handler's code")?;
            let mut code = Vec::with_capacity(count.min(MAX_OPS));
            for _ in 0..count {
                code.push(r.op()?);
            }
            handlers.push(Handler {
                event,
                timer,
                regs,
                code,
            });
        }
        if r.remaining() != 0 {
            return Err(DecodeError::Trailing(r.remaining()));
        }
        Ok(Ir {
            header,
            strings,
            options,
            settings,
            quick,
            enums,
            slots,
            labels,
            tags,
            palettes,
            tables,
            widgets,
            waypoints,
            handlers,
        })
    }

    /// The script's identity: the SHA-256 of its encoding.
    pub fn id(&self) -> Sha256 {
        Sha256::of(&self.encode())
    }

    /// The table sizes value validity depends on.
    pub fn sizes(&self) -> Sizes {
        Sizes {
            strings: self.strings.len(),
            tags: self.tags.len(),
            labels: self.labels.len(),
            enum_variants: self.enums.iter().map(|e| e.variants.len()).collect(),
        }
    }

    /// The bit offset of each option in the record's 32 option bits.
    pub fn option_offsets(&self) -> Vec<u32> {
        let mut at = 0;
        self.options
            .iter()
            .map(|o| {
                let here = at;
                at += o.bits();
                here
            })
            .collect()
    }

    /// The option bits that hold every option's default.
    pub fn default_options(&self) -> u32 {
        let mut bits = 0u64;
        for (o, off) in self.options.iter().zip(self.option_offsets()) {
            let raw = (o.default as i64 - o.lo as i64).max(0) as u64;
            if off < 64 {
                bits |= raw << off;
            }
        }
        bits as u32
    }

    /// The value of the option named `name` in the record's option bits
    /// `bits`, as the VM reads it (`lo` plus its raw field, at most `hi`);
    /// `None` when the script declares no such option. A bool is 0 or 1,
    /// an enum the index of its value ([`Ir::option_values`]). For a host
    /// that reads a script's options without running it.
    pub fn option_value(&self, bits: u32, name: &str) -> Option<i32> {
        let index = (0..self.options.len()).find(|&i| self.option_name(i) == name)?;
        let o = &self.options[index];
        let off = self.option_offsets()[index];
        let width = o.bits();
        if width == 0 || off >= 64 {
            return Some(o.lo);
        }
        let raw = (u64::from(bits) >> off) & ((1u64 << width) - 1);
        Some((i64::from(o.lo) + raw as i64).min(i64::from(o.hi)) as i32)
    }

    /// An option's name, from the string table.
    pub fn option_name(&self, index: usize) -> &str {
        self.options
            .get(index)
            .and_then(|o| self.strings.get(o.name as usize))
            .map_or("", String::as_str)
    }

    /// An enum option's value names, by value from `lo`; empty for an int
    /// or a bool.
    pub fn option_values(&self, index: usize) -> Vec<&str> {
        match self.options.get(index).map(|o| o.ty) {
            Some(Ty::Enum(e)) => self
                .enums
                .get(e as usize)
                .map(|d| {
                    d.variants
                        .iter()
                        .map(|&s| self.strings.get(s as usize).map_or("", String::as_str))
                        .collect()
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    /// Packs option values given by name into the record's 32 option bits,
    /// in declaration order ("Carrying a script"). An option left out takes
    /// its default. Live's variant routes and `h2 variants port --option`
    /// both pack through here.
    pub fn pack_options(&self, given: &[(String, OptionValue)]) -> Result<u32, OptionError> {
        for (i, (name, _)) in given.iter().enumerate() {
            if !(0..self.options.len()).any(|o| self.option_name(o) == name) {
                return Err(OptionError::Unknown(name.clone()));
            }
            if given[..i].iter().any(|(n, _)| n == name) {
                return Err(OptionError::Twice(name.clone()));
            }
        }
        let mut bits = 0u64;
        for (index, (o, off)) in self.options.iter().zip(self.option_offsets()).enumerate() {
            let name = self.option_name(index);
            let range = || OptionError::Range {
                name: name.to_string(),
                lo: o.lo,
                hi: o.hi,
            };
            let value = match given.iter().find(|(n, _)| n == name).map(|(_, v)| v) {
                None => o.default,
                Some(OptionValue::Bool(b)) => i32::from(*b),
                Some(OptionValue::Int(n)) => i32::try_from(*n).map_err(|_| range())?,
                Some(OptionValue::Name(s)) => self
                    .option_values(index)
                    .iter()
                    .position(|v| *v == s.as_str())
                    .map(|p| o.lo + p as i32)
                    .ok_or_else(|| OptionError::NoValue {
                        name: name.to_string(),
                        value: s.clone(),
                    })?,
            };
            if !(o.lo..=o.hi).contains(&value) {
                return Err(range());
            }
            if off < 64 {
                bits |= ((i64::from(value) - i64::from(o.lo)) as u64) << off;
            }
        }
        Ok(bits as u32)
    }
}

/// An option value given by name ([`Ir::pack_options`]): a number, a bool
/// or an enum value's name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OptionValue {
    Int(i64),
    Bool(bool),
    Name(String),
}

impl OptionValue {
    /// Reads command-line text: `true` or `false`, an integer, or else an
    /// enum value's name.
    pub fn parse(text: &str) -> OptionValue {
        match text {
            "true" => OptionValue::Bool(true),
            "false" => OptionValue::Bool(false),
            t => t
                .parse::<i64>()
                .map_or_else(|_| OptionValue::Name(t.to_string()), OptionValue::Int),
        }
    }
}

/// Why option values do not pack.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum OptionError {
    #[error("the script has no option {0:?}")]
    Unknown(String),
    #[error("option {0:?} is given twice")]
    Twice(String),
    #[error("option {name:?} takes {lo}..={hi}")]
    Range { name: String, lo: i32, hi: i32 },
    #[error("option {name:?} has no value {value:?}")]
    NoValue { name: String, value: String },
}

struct Writer(Vec<u8>);

impl Writer {
    fn bytes(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.bytes(&v.to_le_bytes());
    }
    fn i32(&mut self, v: i32) {
        self.bytes(&v.to_le_bytes());
    }
    /// Lengths are u16; the 64 KiB cap keeps every count below it.
    fn len(&mut self, n: usize) {
        self.u16(n.min(u16::MAX as usize) as u16);
    }
    fn str(&mut self, s: &str) {
        self.len(s.len());
        self.bytes(s.as_bytes());
    }
    fn strs(&mut self, list: &[String]) {
        self.len(list.len());
        for s in list {
            self.str(s);
        }
    }
    fn opt_u16(&mut self, v: Option<u16>) {
        match v {
            None => self.u8(0),
            Some(v) => {
                self.u8(1);
                self.u16(v);
            }
        }
    }
    fn opt_u8(&mut self, v: Option<u8>) {
        match v {
            None => self.u8(0),
            Some(v) => {
                self.u8(1);
                self.u8(v);
            }
        }
    }
    fn op(&mut self, op: &Op) {
        match *op {
            Op::Const { dst, value } => {
                self.u8(0);
                self.u8(dst);
                self.i32(value);
            }
            Op::Move { dst, src } => {
                self.u8(1);
                self.u8(dst);
                self.u8(src);
            }
            Op::Arith { op, ty, dst, a, b } => {
                self.u8(2);
                self.u8(op as u8);
                self.u8(ty as u8);
                self.u8(dst);
                self.u8(a);
                self.u8(b);
            }
            Op::Neg { ty, dst, a } => {
                self.u8(3);
                self.u8(ty as u8);
                self.u8(dst);
                self.u8(a);
            }
            Op::Cmp { op, dst, a, b } => {
                self.u8(4);
                self.u8(op as u8);
                self.u8(dst);
                self.u8(a);
                self.u8(b);
            }
            Op::Not { dst, a } => {
                self.u8(5);
                self.u8(dst);
                self.u8(a);
            }
            Op::Load { dst, slot, owner } => {
                self.u8(6);
                self.u8(dst);
                self.u16(slot);
                self.opt_u8(owner);
            }
            Op::Store { slot, owner, src } => {
                self.u8(7);
                self.u16(slot);
                self.opt_u8(owner);
                self.u8(src);
            }
            Op::Opt { dst, index } => {
                self.u8(8);
                self.u8(dst);
                self.u8(index);
            }
            Op::Variant { dst, field } => {
                self.u8(9);
                self.u8(dst);
                self.u8(field);
            }
            Op::TableRead {
                dst,
                table,
                row,
                col,
            } => {
                self.u8(10);
                self.u8(dst);
                self.u8(table);
                self.u8(row);
                self.u8(col);
            }
            Op::TableFind { dst, table, key } => {
                self.u8(11);
                self.u8(dst);
                self.u8(table);
                self.u8(key);
            }
            Op::Jump { to } => {
                self.u8(12);
                self.u16(to);
            }
            Op::JumpIf { cond, to } => {
                self.u8(13);
                self.u8(cond);
                self.u16(to);
            }
            Op::JumpIfNot { cond, to } => {
                self.u8(14);
                self.u8(cond);
                self.u16(to);
            }
            Op::ForEach { set, dst, body } => {
                self.u8(15);
                match set.base {
                    SetBase::Players => self.u8(0),
                    SetBase::Teams => self.u8(1),
                    SetBase::Labelled(l) => {
                        self.u8(2);
                        self.u16(l);
                    }
                    SetBase::Objectives => self.u8(3),
                }
                self.opt_u8(set.pick);
                self.u8(dst);
                self.u16(body);
            }
            Op::Call {
                entry,
                ref args,
                dst,
            } => {
                self.u8(16);
                self.u16(entry);
                self.u8(args.len().min(255) as u8);
                for &a in args.iter().take(255) {
                    self.u8(a);
                }
                self.opt_u8(dst);
            }
            Op::Grant {
                player,
                cap,
                uses,
                palette,
            } => {
                self.u8(17);
                self.u8(player);
                self.u8(cap);
                self.u8(uses);
                self.opt_u8(palette);
            }
            Op::Revoke { player, cap } => {
                self.u8(18);
                self.u8(player);
                self.u8(cap);
            }
            Op::Granted { dst, player, cap } => {
                self.u8(19);
                self.u8(dst);
                self.u8(player);
                self.u8(cap);
            }
        }
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn remaining(&self) -> usize {
        self.bytes.len() - self.at
    }
    fn take(&mut self, n: usize, what: &'static str) -> Result<&'a [u8], DecodeError> {
        if self.remaining() < n {
            return Err(DecodeError::Truncated(what));
        }
        let out = &self.bytes[self.at..self.at + n];
        self.at += n;
        Ok(out)
    }
    fn u8(&mut self, what: &'static str) -> Result<u8, DecodeError> {
        Ok(self.take(1, what)?[0])
    }
    fn u16(&mut self, what: &'static str) -> Result<u16, DecodeError> {
        let b = self.take(2, what)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn i32(&mut self, what: &'static str) -> Result<i32, DecodeError> {
        let b = self.take(4, what)?;
        Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn len(&mut self, what: &'static str) -> Result<usize, DecodeError> {
        Ok(self.u16(what)? as usize)
    }
    fn str(&mut self, what: &'static str) -> Result<String, DecodeError> {
        let n = self.len(what)?;
        let b = self.take(n, what)?;
        String::from_utf8(b.to_vec()).map_err(|_| DecodeError::Utf8)
    }
    fn strs(&mut self, what: &'static str) -> Result<Vec<String>, DecodeError> {
        let n = self.len(what)?;
        let mut out = Vec::with_capacity(n.min(1024));
        for _ in 0..n {
            out.push(self.str(what)?);
        }
        Ok(out)
    }
    fn flag(&mut self, what: &'static str) -> Result<bool, DecodeError> {
        match self.u8(what)? {
            0 => Ok(false),
            1 => Ok(true),
            v => Err(DecodeError::Invalid {
                what: "an option flag",
                value: v as u32,
            }),
        }
    }
    fn opt_u16(&mut self, what: &'static str) -> Result<Option<u16>, DecodeError> {
        Ok(if self.flag(what)? {
            Some(self.u16(what)?)
        } else {
            None
        })
    }
    fn opt_u8(&mut self, what: &'static str) -> Result<Option<u8>, DecodeError> {
        Ok(if self.flag(what)? {
            Some(self.u8(what)?)
        } else {
            None
        })
    }
    fn ty(&mut self, what: &'static str) -> Result<Ty, DecodeError> {
        let c = self.u8(what)?;
        Ty::from_code(c).ok_or(DecodeError::Invalid {
            what,
            value: c as u32,
        })
    }
    fn num_ty(&mut self) -> Result<NumTy, DecodeError> {
        match self.u8("an op")? {
            0 => Ok(NumTy::Int),
            1 => Ok(NumTy::Fixed),
            v => Err(DecodeError::Invalid {
                what: "a number type",
                value: v as u32,
            }),
        }
    }
    fn op(&mut self) -> Result<Op, DecodeError> {
        const W: &str = "an op";
        let tag = self.u8(W)?;
        Ok(match tag {
            0 => Op::Const {
                dst: self.u8(W)?,
                value: self.i32(W)?,
            },
            1 => Op::Move {
                dst: self.u8(W)?,
                src: self.u8(W)?,
            },
            2 => {
                let op = match self.u8(W)? {
                    0 => ArithOp::Add,
                    1 => ArithOp::Sub,
                    2 => ArithOp::Mul,
                    3 => ArithOp::Div,
                    4 => ArithOp::Rem,
                    v => {
                        return Err(DecodeError::Invalid {
                            what: "an arithmetic op",
                            value: v as u32,
                        });
                    }
                };
                Op::Arith {
                    op,
                    ty: self.num_ty()?,
                    dst: self.u8(W)?,
                    a: self.u8(W)?,
                    b: self.u8(W)?,
                }
            }
            3 => Op::Neg {
                ty: self.num_ty()?,
                dst: self.u8(W)?,
                a: self.u8(W)?,
            },
            4 => {
                let op = match self.u8(W)? {
                    0 => CmpOp::Eq,
                    1 => CmpOp::Ne,
                    2 => CmpOp::Lt,
                    3 => CmpOp::Le,
                    4 => CmpOp::Gt,
                    5 => CmpOp::Ge,
                    v => {
                        return Err(DecodeError::Invalid {
                            what: "a comparison",
                            value: v as u32,
                        });
                    }
                };
                Op::Cmp {
                    op,
                    dst: self.u8(W)?,
                    a: self.u8(W)?,
                    b: self.u8(W)?,
                }
            }
            5 => Op::Not {
                dst: self.u8(W)?,
                a: self.u8(W)?,
            },
            6 => Op::Load {
                dst: self.u8(W)?,
                slot: self.u16(W)?,
                owner: self.opt_u8(W)?,
            },
            7 => Op::Store {
                slot: self.u16(W)?,
                owner: self.opt_u8(W)?,
                src: self.u8(W)?,
            },
            8 => Op::Opt {
                dst: self.u8(W)?,
                index: self.u8(W)?,
            },
            9 => Op::Variant {
                dst: self.u8(W)?,
                field: self.u8(W)?,
            },
            10 => Op::TableRead {
                dst: self.u8(W)?,
                table: self.u8(W)?,
                row: self.u8(W)?,
                col: self.u8(W)?,
            },
            11 => Op::TableFind {
                dst: self.u8(W)?,
                table: self.u8(W)?,
                key: self.u8(W)?,
            },
            12 => Op::Jump { to: self.u16(W)? },
            13 => Op::JumpIf {
                cond: self.u8(W)?,
                to: self.u16(W)?,
            },
            14 => Op::JumpIfNot {
                cond: self.u8(W)?,
                to: self.u16(W)?,
            },
            15 => {
                let base = match self.u8(W)? {
                    0 => SetBase::Players,
                    1 => SetBase::Teams,
                    2 => SetBase::Labelled(self.u16(W)?),
                    3 => SetBase::Objectives,
                    v => {
                        return Err(DecodeError::Invalid {
                            what: "a set",
                            value: v as u32,
                        });
                    }
                };
                let pick = self.opt_u8(W)?;
                Op::ForEach {
                    set: Set { base, pick },
                    dst: self.u8(W)?,
                    body: self.u16(W)?,
                }
            }
            16 => {
                let entry = self.u16(W)?;
                let n = self.u8(W)?;
                let mut args = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    args.push(self.u8(W)?);
                }
                Op::Call {
                    entry,
                    args,
                    dst: self.opt_u8(W)?,
                }
            }
            17 => Op::Grant {
                player: self.u8(W)?,
                cap: self.u8(W)?,
                uses: self.u8(W)?,
                palette: self.opt_u8(W)?,
            },
            18 => Op::Revoke {
                player: self.u8(W)?,
                cap: self.u8(W)?,
            },
            19 => Op::Granted {
                dst: self.u8(W)?,
                player: self.u8(W)?,
                cap: self.u8(W)?,
            },
            v => {
                return Err(DecodeError::Invalid {
                    what: "an op code",
                    value: v as u32,
                });
            }
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn sample() -> Ir {
        Ir {
            header: Header {
                ir_version: IR_VERSION,
                api_version: crate::caps::API_VERSION,
                name: "Sample".into(),
                version: 1,
                game_type: Type::Research,
                uses: None,
                teams: 0,
                points: "Points".into(),
                about: "A sample.".into(),
                requires: vec!["forge.move".into()],
            },
            strings: vec!["kill_points".into(), "elite".into()],
            options: vec![OptionDef {
                name: 0,
                label: 0,
                description: None,
                ty: Ty::Int,
                lo: 1,
                hi: 5,
                default: 1,
            }],
            // score_limit 25, shields none.
            settings: vec![Setting { index: 1, value: 25 }, Setting { index: 8, value: 1 }],
            quick: vec![Quick::Setting(1), Quick::Option(0)],
            enums: vec![],
            slots: vec![Slot {
                name: 0,
                scope: Scope::Player,
                ty: Ty::Bool,
                init: 0,
            }],
            labels: vec![
                LabelDef::objects("ff_drop"),
                LabelDef {
                    text: "zone".into(),
                    kind: LabelKind::Zone,
                    min: 2,
                    max: 4,
                    flags: label_flags::NUMBERED | label_flags::RANGE,
                    lo: 0,
                    hi: 1,
                },
            ],
            tags: vec![TagDef {
                path: "objects/weapons/x".into(),
                group: TagGroup::Weapon,
            }],
            palettes: vec![Palette {
                name: 1,
                tags: vec![0],
            }],
            tables: vec![Table {
                name: 1,
                columns: vec![Column {
                    name: 1,
                    ty: Ty::Int,
                }],
                cells: vec![3, -4],
            }],
            widgets: vec![WidgetDef {
                name: 0,
                kind: WidgetKind::Text,
                slot: 1,
            }],
            waypoints: vec![WaypointDef { name: 1, slot: Some(0) }],
            handlers: vec![Handler {
                event: Event::Kill,
                timer: None,
                regs: vec![Ty::Player, Ty::Player, Ty::Int, Ty::Bool, Ty::Object],
                code: vec![
                    Op::Const { dst: 2, value: -1 },
                    Op::Cmp {
                        op: CmpOp::Eq,
                        dst: 3,
                        a: 0,
                        b: 1,
                    },
                    Op::JumpIfNot { cond: 3, to: 4 },
                    Op::Call {
                        entry: crate::caps::id::SCORE,
                        args: vec![0, 2],
                        dst: None,
                    },
                    Op::ForEach {
                        set: Set {
                            base: SetBase::Labelled(0),
                            pick: Some(2),
                        },
                        dst: 4,
                        body: 0,
                    },
                    Op::Grant {
                        player: 0,
                        cap: 0,
                        uses: 3,
                        palette: Some(0),
                    },
                ],
            }],
        }
    }

    #[test]
    fn codec_round_trips() {
        let ir = sample();
        let bytes = ir.encode();
        assert_eq!(Ir::decode(&bytes), Ok(ir.clone()));
        assert_eq!(Ir::decode(&bytes).unwrap().encode(), bytes);
    }

    #[test]
    fn the_type_and_uses_round_trip_and_an_unknown_one_is_refused() {
        let mut ir = sample();
        for row in crate::types::TABLE {
            ir.header.game_type = row.ty;
            ir.header.uses = Some(row.ty);
            let back = Ir::decode(&ir.encode()).unwrap();
            assert_eq!((back.header.game_type, back.header.uses), (row.ty, Some(row.ty)));
        }
        let mut other = sample();
        other.header.game_type = Type::Party;
        assert_ne!(sample().id(), other.id(), "the type is part of the hash");
        let mut other = sample();
        other.header.uses = Some(Type::Slayer);
        assert_ne!(sample().id(), other.id(), "uses is part of the hash");
        let mut other = sample();
        other.header.points = "Laps".into();
        assert_ne!(sample().id(), other.id(), "the points label is part of the hash");
        // The type byte follows the name and version, then the uses byte.
        let at = 4 + 2 + 2 + 2 + "Sample".len() + 2;
        let mut bytes = sample().encode();
        assert_eq!((bytes[at], bytes[at + 1]), (Type::Research.code(), NO_USES));
        bytes[at] = 11;
        assert_eq!(
            Ir::decode(&bytes),
            Err(DecodeError::Invalid {
                what: "the header's type",
                value: 11
            })
        );
        let mut bytes = sample().encode();
        bytes[at + 1] = 11;
        assert_eq!(
            Ir::decode(&bytes),
            Err(DecodeError::Invalid {
                what: "the header's uses",
                value: 11
            })
        );
    }

    #[test]
    fn the_engine_is_the_uses_games_else_the_types() {
        let mut h = sample().header;
        assert_eq!(h.engine(), 2);
        h.game_type = Type::Ctf;
        assert_eq!(h.engine(), 1);
        h.game_type = Type::Party;
        h.uses = Some(Type::Juggernaut);
        assert_eq!(h.engine(), 7);
    }

    #[test]
    fn settings_round_trip_and_decode_refuses_disorder() {
        let ir = sample();
        assert_eq!(crate::settings::TABLE[1].name, "score_limit");
        assert_eq!(crate::settings::TABLE[8].name, "shields");
        let back = Ir::decode(&ir.encode()).unwrap();
        assert_eq!(back.settings, ir.settings);
        assert_eq!(back.quick, ir.quick);
        assert_eq!((back.header.uses, back.header.about.as_str()), (None, "A sample."));

        // Out of order, and one index twice: the encoding is not canonical.
        let mut disorder = sample();
        disorder.settings.reverse();
        assert_eq!(
            Ir::decode(&disorder.encode()),
            Err(DecodeError::Invalid {
                what: "the settings' order",
                value: 1
            })
        );
        let mut twice = sample();
        twice.settings[1].index = 1;
        assert_eq!(
            Ir::decode(&twice.encode()),
            Err(DecodeError::Invalid {
                what: "the settings' order",
                value: 1
            })
        );
        // Six marks.
        let mut many = sample();
        many.quick = (0..6).map(Quick::Setting).collect();
        assert_eq!(
            Ir::decode(&many.encode()),
            Err(DecodeError::Invalid {
                what: "the quick marks' count",
                value: 6
            })
        );
    }

    #[test]
    fn the_hash_covers_settings_marks_and_about() {
        let base = sample().id();
        let changes: [fn(&mut Ir); 6] = [
            |ir| ir.settings[0].value = 26,
            |ir| ir.settings.truncate(1),
            |ir| ir.quick.reverse(),
            |ir| ir.quick.clear(),
            |ir| ir.header.about = "Another.".into(),
            |ir| ir.header.uses = Some(Type::Slayer),
        ];
        for (i, change) in changes.iter().enumerate() {
            let mut ir = sample();
            change(&mut ir);
            assert_ne!(ir.id(), base, "change {i}");
        }
    }

    #[test]
    fn id_is_stable_and_changes_with_content() {
        let a = sample();
        assert_eq!(a.id(), sample().id());
        let mut b = sample();
        b.tables[0].cells[0] = 4;
        assert_ne!(a.id(), b.id());
    }

    #[test]
    fn decode_refuses_trailing_truncated_and_oversized() {
        let mut bytes = sample().encode();
        bytes.push(0);
        assert_eq!(Ir::decode(&bytes), Err(DecodeError::Trailing(1)));
        let bytes = sample().encode();
        for cut in 0..bytes.len() {
            assert!(Ir::decode(&bytes[..cut]).is_err(), "cut at {cut}");
        }
        assert_eq!(
            Ir::decode(&vec![0; MAX_IR_BYTES + 1]),
            Err(DecodeError::TooLarge(MAX_IR_BYTES + 1))
        );
        let mut bytes = sample().encode();
        bytes[0] = b'X';
        assert_eq!(Ir::decode(&bytes), Err(DecodeError::Magic));
    }

    #[test]
    fn decode_never_panics_on_corruption() {
        let bytes = sample().encode();
        let mut state = 0x1234_5678_u64;
        for _ in 0..2000 {
            let mut b = bytes.clone();
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let at = (state >> 33) as usize % b.len();
            b[at] ^= (state >> 13) as u8 | 1;
            if let Ok(ir) = Ir::decode(&b) {
                assert_eq!(ir.encode(), b);
                let _ = crate::verify::verify(&ir);
            }
        }
    }

    #[test]
    fn the_label_and_tag_tables_round_trip_and_refuse_unknown_codes() {
        let ir = sample();
        let back = Ir::decode(&ir.encode()).unwrap();
        assert_eq!(back.labels, ir.labels);
        assert_eq!(back.tags, ir.tags);
        assert_eq!(back.labels[1].range(), Some(0..=1));
        assert!(back.labels[1].numbered() && !back.labels[1].single());
        assert_eq!(back.labels[0].range(), None);
        assert!(!back.labels[0].bounded() && back.labels[1].bounded());

        // Each requirement is part of the hash.
        let mut other = sample();
        other.labels[1].min = 3;
        assert_ne!(other.id(), ir.id());
        let mut other = sample();
        other.tags[0].group = TagGroup::Any;
        assert_ne!(other.id(), ir.id());

        fn find(bytes: &[u8], text: &str) -> usize {
            let mut want = (text.len() as u16).to_le_bytes().to_vec();
            want.extend_from_slice(text.as_bytes());
            bytes
                .windows(want.len())
                .position(|w| w == want.as_slice())
                .map(|at| at + want.len())
                .unwrap()
        }
        // The kind byte follows a label's text, the group a tag's path.
        let mut bytes = ir.encode();
        let at = find(&bytes, "zone");
        assert_eq!(bytes[at], LabelKind::Zone.code());
        bytes[at] = 9;
        assert_eq!(
            Ir::decode(&bytes),
            Err(DecodeError::Invalid {
                what: "a label kind",
                value: 9
            })
        );
        let mut bytes = ir.encode();
        let at = find(&bytes, "objects/weapons/x");
        assert_eq!(bytes[at], TagGroup::Weapon.code());
        bytes[at] = 7;
        assert_eq!(
            Ir::decode(&bytes),
            Err(DecodeError::Invalid {
                what: "a tag group",
                value: 7
            })
        );
    }

    #[test]
    fn a_labelled_set_is_bounded_by_its_label() {
        let ir = sample();
        assert_eq!(SetBase::Labelled(0).bound(&ir.labels), crate::ty::MAX_LABELLED as u64);
        assert_eq!(SetBase::Labelled(1).bound(&ir.labels), 4);
        assert_eq!(SetBase::Players.bound(&ir.labels), crate::ty::MAX_PLAYERS as u64);
    }

    #[test]
    fn options_pack_in_declaration_order() {
        let mut ir = sample();
        ir.options.push(OptionDef {
            name: 0,
            label: 0,
            description: None,
            ty: Ty::Bool,
            lo: 0,
            hi: 1,
            default: 1,
        });
        assert_eq!(ir.options[0].bits(), 3);
        assert_eq!(ir.option_offsets(), vec![0, 3]);
        assert_eq!(ir.default_options(), 0b1000);
    }
}
