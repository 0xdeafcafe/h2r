//! The rules IR: one blob a compiler writes and a verifier and VM read.
//!
//! The binary form is canonical: [`Ir::decode`] accepts exactly the bytes
//! [`Ir::encode`] writes, so the SHA-256 of the encoding ([`Ir::id`]) names
//! a script everywhere. See `docs/game/rules-engine.md`, "The IR and the VM".

use content::Sha256;

use crate::ty::{Sizes, Ty};

/// The blob's first four bytes.
pub const MAGIC: [u8; 4] = *b"H2RL";
/// The IR version this build reads. It changes when ops change.
pub const IR_VERSION: u16 = 1;
/// The most bytes a blob may have.
pub const MAX_IR_BYTES: usize = 64 * 1024;
/// Capabilities a header may declare.
pub const MAX_REQUIRES: usize = 32;
/// Registers one handler may use.
pub const MAX_REGS: usize = 256;
/// Ops in one handler.
pub const MAX_OPS: usize = 8192;
/// Option bits a variant record carries for a script.
pub const OPTION_BITS: u32 = 32;
/// Bytes of a label, and of a message.
pub const MAX_LABEL_BYTES: usize = 32;
pub const MAX_TEXT_BYTES: usize = 128;
pub const MAX_TAG_BYTES: usize = 256;
/// Per-scope slot budgets.
pub const MAX_GLOBAL_SLOTS: usize = 64;
pub const MAX_PLAYER_SLOTS: usize = 16;
pub const MAX_TEAM_SLOTS: usize = 8;
pub const MAX_OBJECT_SLOTS: usize = 8;
/// Rows and columns of a constant table.
pub const MAX_TABLE_ROWS: usize = 256;
pub const MAX_TABLE_COLS: usize = 16;
pub const MAX_PALETTE_TAGS: usize = 64;

/// A register index within a handler's frame.
pub type Reg = u8;

/// The whole blob.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ir {
    pub header: Header,
    /// Names, option labels, class names and messages.
    pub strings: Vec<String>,
    pub options: Vec<OptionDef>,
    pub enums: Vec<EnumDef>,
    pub slots: Vec<Slot>,
    /// Forge labels the script names.
    pub labels: Vec<String>,
    /// Tag paths the script names, checked against the map at load.
    pub tags: Vec<String>,
    pub palettes: Vec<Palette>,
    pub tables: Vec<Table>,
    pub handlers: Vec<Handler>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub ir_version: u16,
    pub api_version: u16,
    pub name: String,
    /// The script's own version number (`rules "Name" 1`).
    pub version: u16,
    /// The retail engine index the game rides.
    pub carrier: u8,
    /// Teams the script needs; 0 is any.
    pub teams: u8,
    /// Capability names, the most the script can ever do.
    pub requires: Vec<String>,
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
}

impl Event {
    pub const ALL: [Event; 20] = [
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
            Event::Spawn | Event::Death | Event::Join | Event::Quit => &[Ty::Player],
            Event::Pickup | Event::Drop | Event::Press | Event::Edit | Event::Bounce => {
                &[Ty::Player, Ty::Object]
            }
            Event::Enter | Event::Leave => &[Ty::Object, Ty::Object],
            Event::SquadDead => &[Ty::Squad],
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
            | Event::Edit
            | Event::Join
            | Event::Quit
            | Event::ActorKill
            | Event::ActorAssist => 16,
            Event::Enter | Event::Leave => 64,
            Event::SquadDead => 16,
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
    /// Objects with the label-table entry's label, by placement id.
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

    /// The most members the set can have.
    pub fn bound(self) -> u64 {
        match self {
            SetBase::Players => crate::ty::MAX_PLAYERS as u64,
            SetBase::Teams => crate::ty::MAX_TEAMS as u64,
            SetBase::Labelled(_) => crate::ty::MAX_LABELLED as u64,
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
        w.u8(h.carrier);
        w.u8(h.teams);
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
        w.strs(&self.labels);
        w.strs(&self.tags);
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
        let carrier = r.u8("the header")?;
        let teams = r.u8("the header")?;
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
            carrier,
            teams,
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
        let labels = r.strs("the label table")?;
        let tags = r.strs("the tag table")?;
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
            enums,
            slots,
            labels,
            tags,
            palettes,
            tables,
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
                carrier: 2,
                teams: 0,
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
            enums: vec![],
            slots: vec![Slot {
                name: 0,
                scope: Scope::Player,
                ty: Ty::Bool,
                init: 0,
            }],
            labels: vec!["ff_drop".into()],
            tags: vec!["objects/weapons/x".into()],
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
