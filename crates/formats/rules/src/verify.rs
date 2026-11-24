//! The verifier: reads IR, never source, and refuses a blob whole.
//!
//! It checks every op, dead branches included: register and slot indices
//! and types, forward jumps inside their region, set bounds, label, tag,
//! string and table indices, API entry numbers and their capabilities
//! against the header's `requires`. It also reports the worst-tick
//! estimate. See `docs/game/rules-engine.md`, "Compiler and verifier" and
//! "Budget".

use crate::caps::{self, Kind, VARIANT_FIELDS};
use crate::ir::*;
use crate::settings;
use crate::ty::{MAX_ENUM_VARIANTS, MAX_ENUMS, Sizes, Ty};
use crate::types::{self, Type};

/// Why a blob was refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error(transparent)]
    Decode(#[from] DecodeError),
    #[error("IR version {0} is unsupported (this build reads {IR_VERSION})")]
    IrVersion(u16),
    #[error("API version {0} is unsupported (this build has {api})", api = caps::API_VERSION)]
    ApiVersion(u16),
    #[error("uses names {}, which is no built-in game", .0.name())]
    Uses(Type),
    #[error("the script needs {0} teams; at most 8 exist")]
    Teams(u8),
    #[error("{what} exceeds its limit of {max}")]
    Limit { what: &'static str, max: usize },
    #[error("unknown capability {0}")]
    UnknownCapability(String),
    #[error("capability {0} is declared twice")]
    DuplicateCapability(String),
    #[error("{table} entry {index} is longer than {max} bytes or empty")]
    Text {
        table: &'static str,
        index: usize,
        max: usize,
    },
    #[error("label {index}: {reason}")]
    Label { index: usize, reason: &'static str },
    #[error("{what} index {index} is out of range")]
    Index { what: &'static str, index: u32 },
    #[error("option {0} has an invalid type, range or default")]
    Option(usize),
    #[error("the options need {0} bits; a record carries {OPTION_BITS}")]
    OptionBits(u32),
    #[error("slot {0} has an invalid type, scope or initial value")]
    Slot(usize),
    #[error("table {table} cell {cell} is not a value of its column")]
    Cell { table: usize, cell: usize },
    #[error("handler {handler}: {reason}")]
    Handler {
        handler: usize,
        reason: &'static str,
    },
    #[error("two handlers answer {0}")]
    DuplicateHandler(String),
    #[error("the description is {0} UTF-16 units; at most {MAX_ABOUT_UNITS}")]
    About(usize),
    #[error("setting {index}: {reason}")]
    Setting { index: u16, reason: &'static str },
    #[error("quick mark {index}: {reason}")]
    Quick { index: usize, reason: &'static str },
    #[error("the script uses a built-in game's rules, so it may have no {0}")]
    UsesHolds(&'static str),
    #[error("handler {handler}, op {pc}: {fault}")]
    Op {
        handler: usize,
        pc: usize,
        fault: Fault,
    },
}

/// What is wrong with one op.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Fault {
    #[error("register {0} does not exist")]
    Register(Reg),
    #[error("register {reg} is {got:?}, not {want}")]
    Type {
        reg: Reg,
        want: &'static str,
        got: Ty,
    },
    #[error("{0} is not a value of the register's type")]
    Const(i32),
    #[error("slot {0} does not exist")]
    Slot(u16),
    #[error("slot {0} is read or written without its owner")]
    Owner(u16),
    #[error("option {0} does not exist")]
    Option(u8),
    #[error("variant field {0} does not exist")]
    VariantField(u8),
    #[error("table {0} does not exist")]
    Table(u8),
    #[error("column {0} does not exist")]
    Column(u8),
    #[error("jump to {0} is not forward")]
    Backward(u16),
    #[error("jump to {0} leaves its region")]
    JumpOut(u16),
    #[error("a body of {0} ops leaves its region")]
    BodyOut(u16),
    #[error("label {0} does not exist")]
    Label(u16),
    #[error("label {0} names starting locations, which a loop cannot visit")]
    SpawnSet(u16),
    #[error("API entry {0} does not exist in this API version")]
    Entry(u16),
    #[error("{entry} takes {want} arguments, given {got}")]
    Arity {
        entry: &'static str,
        want: usize,
        got: usize,
    },
    #[error("{entry} returns {want:?}, written to {got:?}")]
    Result {
        entry: &'static str,
        want: Option<Ty>,
        got: Option<Ty>,
    },
    #[error("{entry} needs capability {cap}, which the header does not declare")]
    Undeclared {
        entry: &'static str,
        cap: &'static str,
    },
    #[error("capability index {0} is not in requires")]
    CapIndex(u8),
    #[error("capability {0} cannot be granted")]
    NotGrantable(String),
    #[error("palette {0} does not exist")]
    Palette(u8),
    #[error("remainder of fixed values")]
    FixedRem,
    #[error("{0} is not followed at once by a message")]
    Unshown(&'static str),
    #[error("a message shows at most {max} values, given {0}", max = caps::MAX_HOLES)]
    Holes(usize),
    #[error("the values of a message do not follow at once the constant that writes its text")]
    Template,
    #[error("a jump or a loop lands inside a message's values")]
    IntoHoles,
    #[error("{0} names its tag and its field as constants written directly before the call")]
    FieldRead(&'static str),
    #[error("{entry} names no valid field: {why}")]
    FieldPath { entry: &'static str, why: String },
    #[error("string {0} is no message template")]
    NotTemplate(u32),
    #[error("the message's text has holes {want:?}, given values {got:?}")]
    HoleTypes {
        want: Vec<caps::Hole>,
        got: Vec<caps::Hole>,
    },
}

/// A passing blob's budget figures.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    /// Each handler's worst-case op count, in handler order.
    pub handler_ops: Vec<u64>,
    /// Each handler's worst case times the events the estimate assumes a
    /// tick holds (`Event::worst_per_tick`), summed. `on enter` and
    /// `on leave` share their 64 runs.
    pub worst_tick: u64,
    /// The handler that costs the most in the estimate.
    pub costliest: Option<usize>,
}

impl Report {
    /// Whether the estimate fits the VM's per-tick budget.
    pub fn fits(&self) -> bool {
        self.worst_tick <= crate::vm::TICK_BUDGET as u64
    }
}

/// Decodes and verifies a blob.
pub fn load(bytes: &[u8]) -> Result<(Ir, Report), Refusal> {
    let ir = Ir::decode(bytes)?;
    let report = verify(&ir)?;
    Ok((ir, report))
}

/// Verifies a decoded blob.
pub fn verify(ir: &Ir) -> Result<Report, Refusal> {
    let h = &ir.header;
    if h.ir_version != IR_VERSION {
        return Err(Refusal::IrVersion(h.ir_version));
    }
    if h.api_version != caps::API_VERSION {
        return Err(Refusal::ApiVersion(h.api_version));
    }
    if let Some(u) = h.uses
        && !u.is_retail()
    {
        return Err(Refusal::Uses(u));
    }
    if h.teams as usize > crate::ty::MAX_TEAMS {
        return Err(Refusal::Teams(h.teams));
    }
    if h.name.is_empty() || h.name.len() > MAX_TEXT_BYTES {
        return Err(Refusal::Text {
            table: "the name",
            index: 0,
            max: MAX_TEXT_BYTES,
        });
    }
    if h.points.is_empty() || h.points.len() > types::MAX_POINTS_BYTES {
        return Err(Refusal::Text {
            table: "the points label",
            index: 0,
            max: types::MAX_POINTS_BYTES,
        });
    }
    if h.requires.len() > MAX_REQUIRES {
        return Err(Refusal::Limit {
            what: "requires",
            max: MAX_REQUIRES,
        });
    }
    for (i, name) in h.requires.iter().enumerate() {
        if caps::capability(name, h.api_version).is_none() {
            return Err(Refusal::UnknownCapability(name.clone()));
        }
        if h.requires[..i].contains(name) {
            return Err(Refusal::DuplicateCapability(name.clone()));
        }
    }
    text_table(&ir.strings, "the string table", MAX_TEXT_BYTES)?;
    labels(&ir.labels)?;
    let paths: Vec<String> = ir.tags.iter().map(|t| t.path.clone()).collect();
    text_table(&paths, "the tag table", MAX_TAG_BYTES)?;
    let sizes = ir.sizes();
    let string = |index: u16| -> Result<(), Refusal> {
        if (index as usize) < ir.strings.len() {
            Ok(())
        } else {
            Err(Refusal::Index {
                what: "string",
                index: index as u32,
            })
        }
    };

    if ir.enums.len() > MAX_ENUMS {
        return Err(Refusal::Limit {
            what: "enums",
            max: MAX_ENUMS,
        });
    }
    for e in &ir.enums {
        string(e.name)?;
        if e.variants.is_empty() || e.variants.len() > MAX_ENUM_VARIANTS {
            return Err(Refusal::Limit {
                what: "enum variants",
                max: MAX_ENUM_VARIANTS,
            });
        }
        for &v in &e.variants {
            string(v)?;
        }
    }

    let mut bits = 0u32;
    for (i, o) in ir.options.iter().enumerate() {
        string(o.name)?;
        string(o.label)?;
        if let Some(d) = o.description {
            string(d)?;
        }
        let typed = match o.ty {
            Ty::Int => true,
            Ty::Bool => o.lo == 0 && o.hi == 1,
            Ty::Enum(e) => {
                o.lo == 0 && sizes.enum_variants.get(e as usize).is_some_and(|&n| o.hi as i64 == n as i64 - 1)
            }
            _ => false,
        };
        if !typed || o.lo > o.hi || o.default < o.lo || o.default > o.hi {
            return Err(Refusal::Option(i));
        }
        bits = bits.saturating_add(o.bits());
    }
    if bits > OPTION_BITS {
        return Err(Refusal::OptionBits(bits));
    }
    settings_and_marks(ir)?;

    let mut per_scope = [0usize; 4];
    for (i, s) in ir.slots.iter().enumerate() {
        string(s.name)?;
        per_scope[s.scope as usize] += 1;
        let timer_ok = s.ty != Ty::Timer || s.scope == Scope::Global;
        if !timer_ok || !s.ty.valid(s.init, &sizes) {
            return Err(Refusal::Slot(i));
        }
    }
    let budgets = [
        ("global slots", MAX_GLOBAL_SLOTS),
        ("player slots", MAX_PLAYER_SLOTS),
        ("team slots", MAX_TEAM_SLOTS),
        ("object slots", MAX_OBJECT_SLOTS),
    ];
    for (n, (what, max)) in per_scope.iter().zip(budgets) {
        if *n > max {
            return Err(Refusal::Limit { what, max });
        }
    }

    if ir.palettes.len() > 255 {
        return Err(Refusal::Limit {
            what: "palettes",
            max: 255,
        });
    }
    for p in &ir.palettes {
        string(p.name)?;
        if p.tags.is_empty() || p.tags.len() > MAX_PALETTE_TAGS {
            return Err(Refusal::Limit {
                what: "palette tags",
                max: MAX_PALETTE_TAGS,
            });
        }
        for &t in &p.tags {
            if t as usize >= ir.tags.len() {
                return Err(Refusal::Index {
                    what: "tag",
                    index: t as u32,
                });
            }
        }
    }

    if ir.tables.len() > 255 {
        return Err(Refusal::Limit {
            what: "tables",
            max: 255,
        });
    }
    for (ti, t) in ir.tables.iter().enumerate() {
        string(t.name)?;
        if t.columns.is_empty() || t.columns.len() > MAX_TABLE_COLS {
            return Err(Refusal::Limit {
                what: "table columns",
                max: MAX_TABLE_COLS,
            });
        }
        if t.cells.len() % t.columns.len() != 0 || t.rows() > MAX_TABLE_ROWS {
            return Err(Refusal::Limit {
                what: "table rows",
                max: MAX_TABLE_ROWS,
            });
        }
        for c in &t.columns {
            string(c.name)?;
            if c.ty == Ty::Timer {
                return Err(Refusal::Cell { table: ti, cell: 0 });
            }
        }
        for (ci, &v) in t.cells.iter().enumerate() {
            if !t.columns[ci % t.columns.len()].ty.valid(v, &sizes) {
                return Err(Refusal::Cell {
                    table: ti,
                    cell: ci,
                });
            }
        }
    }

    let mut handler_ops = Vec::with_capacity(ir.handlers.len());
    for (hi, hd) in ir.handlers.iter().enumerate() {
        for other in &ir.handlers[..hi] {
            if other.event == hd.event && other.timer == hd.timer {
                return Err(Refusal::DuplicateHandler(hd.event.name().to_string()));
            }
        }
        let bad = |reason| Refusal::Handler {
            handler: hi,
            reason,
        };
        match (hd.event, hd.timer) {
            (Event::Timer, Some(slot)) => {
                let ok = ir
                    .slots
                    .get(slot as usize)
                    .is_some_and(|s| s.ty == Ty::Timer && s.scope == Scope::Global);
                if !ok {
                    return Err(bad("on timer names no global timer slot"));
                }
            }
            (Event::Timer, None) => return Err(bad("on timer names no slot")),
            (_, Some(_)) => return Err(bad("only on timer names a slot")),
            (_, None) => {}
        }
        if hd.regs.len() > MAX_REGS {
            return Err(Refusal::Limit {
                what: "registers",
                max: MAX_REGS,
            });
        }
        if hd.code.len() > MAX_OPS {
            return Err(Refusal::Limit {
                what: "ops",
                max: MAX_OPS,
            });
        }
        let args = hd.event.args();
        if hd.regs.len() < args.len() || &hd.regs[..args.len()] != args {
            return Err(bad("argument registers do not match the event"));
        }
        if hd.regs.iter().any(|&t| t == Ty::Timer || !enum_known(t, &sizes)) {
            return Err(bad("a register has an invalid type"));
        }
        let cx = Cx { ir, hd, sizes: &sizes };
        let mut ends: Vec<usize> = Vec::new();
        for (pc, op) in hd.code.iter().enumerate() {
            while ends.last() == Some(&pc) {
                ends.pop();
            }
            let region = ends.last().copied().unwrap_or(hd.code.len());
            cx.op(pc, op, region, &mut ends).map_err(|fault| Refusal::Op {
                handler: hi,
                pc,
                fault,
            })?;
        }
        message_runs(ir, hd).map_err(|(pc, fault)| Refusal::Op {
            handler: hi,
            pc,
            fault,
        })?;
        field_reads(ir, hd).map_err(|(pc, fault)| Refusal::Op {
            handler: hi,
            pc,
            fault,
        })?;
        handler_ops.push(cost(&ir.labels, &hd.code, 0, hd.code.len()));
    }

    let mut worst_tick = 0u64;
    let mut zone = 0u64;
    let mut costliest: Option<(usize, u64)> = None;
    for (i, (hd, &ops)) in ir.handlers.iter().zip(&handler_ops).enumerate() {
        let total = match hd.event {
            // Enter and leave share 64 runs a tick; the dearer one bounds it.
            Event::Enter | Event::Leave => {
                zone = zone.max(ops);
                ops.saturating_mul(hd.event.worst_per_tick())
            }
            e => {
                let t = ops.saturating_mul(e.worst_per_tick());
                worst_tick = worst_tick.saturating_add(t);
                t
            }
        };
        if costliest.is_none_or(|(_, c)| total > c) {
            costliest = Some((i, total));
        }
    }
    worst_tick = worst_tick.saturating_add(zone.saturating_mul(64));
    Ok(Report {
        handler_ops,
        worst_tick,
        costliest: costliest.map(|(i, _)| i),
    })
}

/// The hole a `fmt_*` call queues a value for.
fn fmt_hole(op: &Op) -> Option<caps::Hole> {
    match *op {
        Op::Call { entry, .. } => caps::Hole::of_entry(entry),
        _ => None,
    }
}

/// The text register of a `message` call: its last argument.
fn message_text(op: &Op) -> Option<Reg> {
    match op {
        Op::Call { entry, args, .. } if caps::is_message(*entry) => args.last().copied(),
        _ => None,
    }
}

/// The message rule ("Messages" in `docs/game/rules-language-types.md`).
/// Each run of `fmt_*` calls directly follows the `Const` that writes the
/// text of the `message` call directly after it, nothing jumps or loops
/// into the run or onto the message, and the text is a template whose
/// holes match the run in count, order and type. A message with no run
/// whose text is written by the op before it shows a template with no
/// holes. Runs after the per-op checks, so every index here is valid.
fn message_runs(ir: &Ir, hd: &Handler) -> Result<(), (usize, Fault)> {
    let code = &hd.code;
    // The ops reached other than from the op before them: jump targets,
    // and the op after a loop body (reached when the set is empty).
    let mut entered = vec![false; code.len() + 1];
    for (pc, op) in code.iter().enumerate() {
        let to = match *op {
            Op::Jump { to } | Op::JumpIf { to, .. } | Op::JumpIfNot { to, .. } => to as usize,
            Op::ForEach { body, .. } => pc + 1 + body as usize,
            _ => continue,
        };
        if let Some(e) = entered.get_mut(to) {
            *e = true;
        }
    }
    let template = |text: Reg, before: Option<usize>| -> Option<Result<Vec<caps::Hole>, u32>> {
        let i = before?;
        match code[i] {
            Op::Const { dst, value } if dst == text => {
                let s = ir.strings.get(value as usize)?;
                Some(caps::holes(s).ok_or(value as u32))
            }
            _ => None,
        }
    };
    let mut pc = 0;
    while pc < code.len() {
        let start = pc;
        let mut got = Vec::new();
        while let Some(h) = code.get(pc).and_then(fmt_hole) {
            got.push(h);
            pc += 1;
        }
        if got.is_empty() {
            // A message with no values.
            if let Some(text) = message_text(&code[pc])
                && !entered[pc]
            {
                match template(text, pc.checked_sub(1)) {
                    Some(Ok(want)) if !want.is_empty() => return Err((pc, Fault::HoleTypes { want, got })),
                    Some(Err(s)) => return Err((pc, Fault::NotTemplate(s))),
                    _ => {}
                }
            }
            pc += 1;
            continue;
        }
        if got.len() > caps::MAX_HOLES {
            return Err((start, Fault::Holes(got.len())));
        }
        let Some(text) = code.get(pc).and_then(message_text) else {
            let name = match code[start] {
                Op::Call { entry, .. } => caps::ENTRIES[entry as usize].name,
                _ => "a message value",
            };
            return Err((start, Fault::Unshown(name)));
        };
        if let Some(at) = (start..=pc).find(|&i| entered[i]) {
            return Err((at, Fault::IntoHoles));
        }
        match template(text, start.checked_sub(1)) {
            None => return Err((start, Fault::Template)),
            Some(Err(s)) => return Err((pc, Fault::NotTemplate(s))),
            Some(Ok(want)) if want != got => return Err((pc, Fault::HoleTypes { want, got })),
            Some(Ok(_)) => {}
        }
        pc += 1;
    }
    Ok(())
}

/// The tag read rule ("Advanced data" in `docs/game/rules-world-api.md`).
/// A `tag_int` or `tag_fixed` call is directly after the two constants that
/// write its tag and its field, nothing jumps or loops into them or onto
/// the call, and the field is a valid path ([`crate::fields::valid_path`]),
/// so a host resolves every read at load. Runs after the per-op checks, so
/// every index here is valid.
fn field_reads(ir: &Ir, hd: &Handler) -> Result<(), (usize, Fault)> {
    let code = &hd.code;
    let mut entered = vec![false; code.len() + 1];
    for (pc, op) in code.iter().enumerate() {
        let to = match *op {
            Op::Jump { to } | Op::JumpIf { to, .. } | Op::JumpIfNot { to, .. } => to as usize,
            Op::ForEach { body, .. } => pc + 1 + body as usize,
            _ => continue,
        };
        if let Some(e) = entered.get_mut(to) {
            *e = true;
        }
    }
    for (pc, op) in code.iter().enumerate() {
        let Op::Call { entry, .. } = *op else { continue };
        if !crate::fields::is_read(entry) {
            continue;
        }
        let name = caps::ENTRIES[entry as usize].name;
        let Some((_, field)) = crate::fields::constants(code, pc) else {
            return Err((pc, Fault::FieldRead(name)));
        };
        if pc < 2 || (pc - 1..=pc).any(|i| entered[i]) {
            return Err((pc, Fault::FieldRead(name)));
        }
        let path = usize::try_from(field).ok().and_then(|i| ir.strings.get(i));
        if let Err(why) = crate::fields::valid_path(path.map_or("", String::as_str)) {
            return Err((pc, Fault::FieldPath { entry: name, why }));
        }
    }
    Ok(())
}

fn enum_known(t: Ty, sizes: &Sizes) -> bool {
    match t {
        Ty::Enum(e) => (e as usize) < sizes.enum_variants.len(),
        _ => true,
    }
}

/// The label table: at most [`MAX_LABELS`] entries, each with text of 1 to
/// 32 [`label_char`]s that no other entry has, and requirements a map can
/// meet.
/// The description, the settings, the quick marks and the scorer: what the
/// compiler holds a `settings` block, a `quick` line and a `uses` line
/// to (`docs/game/rules-variants.md`, "The IR").
fn settings_and_marks(ir: &Ir) -> Result<(), Refusal> {
    let h = &ir.header;
    let units = h.about.encode_utf16().count();
    if units > MAX_ABOUT_UNITS {
        return Err(Refusal::About(units));
    }
    for (i, s) in ir.settings.iter().enumerate() {
        let bad = |reason| Err(Refusal::Setting { index: s.index, reason });
        if i > 0 && ir.settings[i - 1].index >= s.index {
            return bad("is out of order or named twice");
        }
        let Some(row) = settings::row_at(s.index) else {
            return bad("is not in the settings table");
        };
        if !row.applies(h.engine()) {
            return bad("does not apply to the header's game");
        }
        if !row.in_range(s.value) {
            return bad("has a value outside its range");
        }
    }
    if ir.quick.len() > MAX_QUICK {
        return Err(Refusal::Limit {
            what: "quick marks",
            max: MAX_QUICK,
        });
    }
    for (index, q) in ir.quick.iter().enumerate() {
        let bad = |reason| Err(Refusal::Quick { index, reason });
        if ir.quick[..index].contains(q) {
            return bad("is marked twice");
        }
        match *q {
            Quick::Setting(s) => match settings::row_at(s) {
                Some(row) if row.is_record() && row.applies(h.engine()) => {}
                Some(row) if row.is_record() => return bad("names a setting of another game"),
                Some(_) => return bad("names no record setting"),
                None => return bad("names no setting"),
            },
            Quick::Option(o) => {
                if o as usize >= ir.options.len() {
                    return bad("names no option");
                }
            }
        }
    }
    if h.uses.is_some() {
        if !ir.handlers.is_empty() {
            return Err(Refusal::UsesHolds("handlers"));
        }
        if !ir.slots.is_empty() {
            return Err(Refusal::UsesHolds("variables"));
        }
        if !ir.options.is_empty() {
            return Err(Refusal::UsesHolds("options"));
        }
    }
    Ok(())
}

fn labels(list: &[LabelDef]) -> Result<(), Refusal> {
    use crate::ir::label_flags::*;
    if list.len() > MAX_LABELS {
        return Err(Refusal::Limit {
            what: "labels",
            max: MAX_LABELS,
        });
    }
    let texts: Vec<String> = list.iter().map(|l| l.text.clone()).collect();
    text_table(&texts, "the label table", MAX_LABEL_BYTES)?;
    for (index, l) in list.iter().enumerate() {
        let bad = |reason| Err(Refusal::Label { index, reason });
        if !l.text.chars().all(label_char) {
            return bad("its text holds a character a Forge label cannot");
        }
        if list[..index].iter().any(|o| o.text == l.text) {
            return bad("another label has its text");
        }
        if l.flags & !ALL != 0 {
            return bad("it has an unknown flag");
        }
        if l.min > l.max || l.max as usize > crate::ty::MAX_LABELLED {
            return bad("its member count is not min <= max <= 256");
        }
        if l.single() && (l.max != 1 || l.flags != SINGLE) {
            return bad("a single label has at most 1 member and no other requirement");
        }
        if l.kind == LabelKind::Spawn && (l.single() || l.per_team()) {
            return bad("starting locations are a set with no per team");
        }
        match l.range() {
            Some(_) if !l.numbered() => return bad("a number range needs numbered"),
            Some(_) if l.lo > l.hi => return bad("its number range is not ordered"),
            Some(_) => {
                let span = l.hi as i64 - l.lo as i64 + 1;
                if span > crate::ty::MAX_LABELLED as i64 {
                    return bad("its number range holds more than 256 numbers");
                }
                if span > l.max as i64 {
                    return bad("its number range needs more members than its max");
                }
            }
            None if l.lo != 0 || l.hi != 0 => return bad("it has a number range without the flag"),
            None => {}
        }
    }
    Ok(())
}

fn text_table(list: &[String], table: &'static str, max: usize) -> Result<(), Refusal> {
    if list.len() > u16::MAX as usize / 2 {
        return Err(Refusal::Limit {
            what: table,
            max: u16::MAX as usize / 2,
        });
    }
    for (index, s) in list.iter().enumerate() {
        if s.is_empty() && table != "the string table" || s.len() > max {
            return Err(Refusal::Text { table, index, max });
        }
    }
    Ok(())
}

/// The worst-case op count of `code[start..end]`: every op once, and a
/// `ForEach` body once per member of its set's bound, a labelled set's
/// being its label's `max`.
pub fn cost(labels: &[LabelDef], code: &[Op], start: usize, end: usize) -> u64 {
    let mut total = 0u64;
    let mut pc = start;
    while pc < end && pc < code.len() {
        total = total.saturating_add(1);
        if let Op::ForEach { set, body, .. } = code[pc] {
            let body_end = (pc + 1 + body as usize).min(end);
            let inner = cost(labels, code, pc + 1, body_end);
            total = total.saturating_add(set.base.bound(labels).saturating_mul(inner));
            pc = body_end;
        } else {
            pc += 1;
        }
    }
    total
}

struct Cx<'a> {
    ir: &'a Ir,
    hd: &'a Handler,
    sizes: &'a Sizes,
}

impl Cx<'_> {
    fn reg(&self, r: Reg) -> Result<Ty, Fault> {
        self.hd.regs.get(r as usize).copied().ok_or(Fault::Register(r))
    }

    fn want(&self, r: Reg, want: Ty) -> Result<(), Fault> {
        let got = self.reg(r)?;
        if got == want {
            Ok(())
        } else {
            Err(Fault::Type {
                reg: r,
                want: want.name(),
                got,
            })
        }
    }

    fn num(&self, r: Reg, ty: NumTy) -> Result<(), Fault> {
        self.want(r, ty.ty())
    }

    fn jump(&self, pc: usize, to: u16, region: usize) -> Result<(), Fault> {
        if (to as usize) <= pc {
            return Err(Fault::Backward(to));
        }
        if to as usize > region {
            return Err(Fault::JumpOut(to));
        }
        Ok(())
    }

    fn cap(&self, cap: u8) -> Result<&str, Fault> {
        self.ir
            .header
            .requires
            .get(cap as usize)
            .map(String::as_str)
            .ok_or(Fault::CapIndex(cap))
    }

    fn slot(&self, slot: u16, owner: Option<Reg>) -> Result<&Slot, Fault> {
        let s = self.ir.slots.get(slot as usize).ok_or(Fault::Slot(slot))?;
        match (s.scope.owner(), owner) {
            (None, None) => {}
            (Some(ty), Some(r)) => self.want(r, ty)?,
            _ => return Err(Fault::Owner(slot)),
        }
        Ok(s)
    }

    fn op(&self, pc: usize, op: &Op, region: usize, ends: &mut Vec<usize>) -> Result<(), Fault> {
        match *op {
            Op::Const { dst, value } => {
                if !self.reg(dst)?.valid(value, self.sizes) {
                    return Err(Fault::Const(value));
                }
            }
            Op::Move { dst, src } => {
                let t = self.reg(src)?;
                self.want(dst, t)?;
            }
            Op::Arith { op, ty, dst, a, b } => {
                if op == ArithOp::Rem && ty == NumTy::Fixed {
                    return Err(Fault::FixedRem);
                }
                self.num(dst, ty)?;
                self.num(a, ty)?;
                self.num(b, ty)?;
            }
            Op::Neg { ty, dst, a } => {
                self.num(dst, ty)?;
                self.num(a, ty)?;
            }
            Op::Cmp { op, dst, a, b } => {
                self.want(dst, Ty::Bool)?;
                let t = self.reg(a)?;
                self.want(b, t)?;
                let ordered = matches!(t, Ty::Int | Ty::Fixed);
                if !matches!(op, CmpOp::Eq | CmpOp::Ne) && !ordered {
                    return Err(Fault::Type {
                        reg: a,
                        want: "int or fixed",
                        got: t,
                    });
                }
            }
            Op::Not { dst, a } => {
                self.want(dst, Ty::Bool)?;
                self.want(a, Ty::Bool)?;
            }
            Op::Load { dst, slot, owner } => {
                let s = self.slot(slot, owner)?;
                self.want(dst, s.ty.register())?;
            }
            Op::Store { slot, owner, src } => {
                let s = self.slot(slot, owner)?;
                self.want(src, s.ty.register())?;
            }
            Op::Opt { dst, index } => {
                let o = self.ir.options.get(index as usize).ok_or(Fault::Option(index))?;
                self.want(dst, o.ty)?;
            }
            Op::Variant { dst, field } => {
                let &(_, ty) = VARIANT_FIELDS
                    .get(field as usize)
                    .ok_or(Fault::VariantField(field))?;
                self.want(dst, ty)?;
            }
            Op::TableRead {
                dst,
                table,
                row,
                col,
            } => {
                let t = self.ir.tables.get(table as usize).ok_or(Fault::Table(table))?;
                let c = t.columns.get(col as usize).ok_or(Fault::Column(col))?;
                self.want(row, Ty::Int)?;
                self.want(dst, c.ty)?;
            }
            Op::TableFind { dst, table, key } => {
                let t = self.ir.tables.get(table as usize).ok_or(Fault::Table(table))?;
                self.want(dst, Ty::Int)?;
                self.want(key, t.columns[0].ty)?;
            }
            Op::Jump { to } => self.jump(pc, to, region)?,
            Op::JumpIf { cond, to } | Op::JumpIfNot { cond, to } => {
                self.want(cond, Ty::Bool)?;
                self.jump(pc, to, region)?;
            }
            Op::ForEach { set, dst, body } => {
                if let SetBase::Labelled(l) = set.base {
                    let def = self.ir.labels.get(l as usize).ok_or(Fault::Label(l))?;
                    if def.kind == LabelKind::Spawn {
                        return Err(Fault::SpawnSet(l));
                    }
                }
                if let Some(n) = set.pick {
                    self.want(n, Ty::Int)?;
                }
                self.want(dst, set.base.elem())?;
                let end = pc + 1 + body as usize;
                if end > region {
                    return Err(Fault::BodyOut(body));
                }
                ends.push(end);
            }
            Op::Call {
                entry,
                ref args,
                dst,
            } => {
                let e = caps::entry(entry, self.ir.header.api_version).ok_or(Fault::Entry(entry))?;
                if args.len() != e.args.len() {
                    return Err(Fault::Arity {
                        entry: caps::spelled(e),
                        want: e.args.len(),
                        got: args.len(),
                    });
                }
                for (&r, &ty) in args.iter().zip(e.args) {
                    if e.id == caps::id::FMT_ENUM {
                        // Any enum: the VM names the variant from the
                        // register's type.
                        let got = self.reg(r)?;
                        if !matches!(got, Ty::Enum(_)) {
                            return Err(Fault::Type { reg: r, want: "an enum", got });
                        }
                    } else {
                        self.want(r, ty)?;
                    }
                }
                let got = match dst {
                    Some(r) => Some(self.reg(r)?),
                    None => None,
                };
                // A read's result may be dropped; an action has none.
                let fits = match (e.result, got) {
                    (Some(w), Some(g)) => w == g,
                    (Some(_), None) => e.kind != Kind::Action,
                    (None, None) => true,
                    (None, Some(_)) => false,
                };
                if !fits {
                    return Err(Fault::Result {
                        entry: caps::spelled(e),
                        want: e.result,
                        got,
                    });
                }
                if let Some(cap) = e.cap
                    && !self.ir.header.requires.iter().any(|r| r == cap)
                {
                    return Err(Fault::Undeclared { entry: caps::spelled(e), cap });
                }
            }
            Op::Grant {
                player,
                cap,
                uses: _,
                palette,
            } => {
                self.want(player, Ty::Player)?;
                self.grantable(cap)?;
                if let Some(p) = palette
                    && p as usize >= self.ir.palettes.len()
                {
                    return Err(Fault::Palette(p));
                }
            }
            Op::Revoke { player, cap } => {
                self.want(player, Ty::Player)?;
                self.grantable(cap)?;
            }
            Op::Granted { dst, player, cap } => {
                self.want(dst, Ty::Bool)?;
                self.want(player, Ty::Player)?;
                self.grantable(cap)?;
            }
        }
        Ok(())
    }

    fn grantable(&self, cap: u8) -> Result<(), Fault> {
        let name = self.cap(cap)?;
        let known = caps::capability(name, self.ir.header.api_version);
        if !known.is_some_and(|c| c.grantable) {
            return Err(Fault::NotGrantable(name.to_string()));
        }
        Ok(())
    }
}

/// The first entry a blob calls that a host cannot make on the engine the
/// blob's record is on ([`Header::engine`], [`caps::Entry::hostable_on`]). A host refuses such a script by
/// that entry's name.
pub fn unhostable(ir: &Ir) -> Option<&'static caps::Entry> {
    for hd in &ir.handlers {
        for op in &hd.code {
            if let Op::Call { entry, .. } = *op
                && let Some(e) = caps::entry(entry, ir.header.api_version)
                && !e.hostable_on(ir.header.engine())
            {
                return Some(e);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::tests::sample;

    #[test]
    fn the_sample_verifies() {
        let report = verify(&sample()).unwrap();
        // 6 ops, the empty ForEach body costs nothing.
        assert_eq!(report.handler_ops, vec![6]);
        assert_eq!(report.worst_tick, 6 * 16);
        assert!(report.fits());
    }

    #[test]
    fn header_refusals_name_what_failed() {
        let mut ir = sample();
        ir.header.requires.push("net.socket".into());
        assert_eq!(verify(&ir), Err(Refusal::UnknownCapability("net.socket".into())));

        let mut ir = sample();
        ir.header.requires.push("forge.move".into());
        assert_eq!(verify(&ir), Err(Refusal::DuplicateCapability("forge.move".into())));

        let mut ir = sample();
        ir.header.ir_version = 2;
        assert_eq!(verify(&ir), Err(Refusal::IrVersion(2)));

        let mut ir = sample();
        ir.header.api_version = 9;
        assert_eq!(verify(&ir), Err(Refusal::ApiVersion(9)));

        let mut ir = sample();
        ir.header.uses = Some(Type::Party);
        ir.handlers.clear();
        ir.slots.clear();
        ir.options.clear();
        ir.quick.retain(|q| matches!(q, Quick::Setting(_)));
        assert_eq!(verify(&ir), Err(Refusal::Uses(Type::Party)));
        ir.header.uses = Some(Type::Slayer);
        assert!(verify(&ir).is_ok());

        for points in [String::new(), "x".repeat(types::MAX_POINTS_BYTES + 1)] {
            let mut ir = sample();
            ir.header.points = points;
            assert_eq!(
                verify(&ir),
                Err(Refusal::Text {
                    table: "the points label",
                    index: 0,
                    max: types::MAX_POINTS_BYTES
                })
            );
        }
    }

    /// The table index of a setting, by name.
    fn row(name: &str) -> u16 {
        settings::named(name).unwrap().0
    }

    #[test]
    fn a_setting_of_another_game_is_refused() {
        // The sample rides Slayer; Juggernaut's movement is Juggernaut's.
        let mut ir = sample();
        ir.settings.push(Setting {
            index: row("juggernaut_movement"),
            value: 2,
        });
        ir.settings.sort_by_key(|s| s.index);
        let index = row("juggernaut_movement");
        let refusal = Refusal::Setting {
            index,
            reason: "does not apply to the header's game",
        };
        assert_eq!(load(&ir.encode()).unwrap_err(), refusal);
        ir.header.game_type = Type::Juggernaut;
        assert!(load(&ir.encode()).is_ok(), "{:?}", load(&ir.encode()));
        // A quick mark of another game's setting is refused too.
        let mut ir = sample();
        ir.quick.push(Quick::Setting(index));
        assert_eq!(
            load(&ir.encode()).unwrap_err(),
            Refusal::Quick {
                index: 2,
                reason: "names a setting of another game"
            }
        );
    }

    #[test]
    fn the_verifier_refuses_unknown_settings_values_and_disorder() {
        let mut ir = sample();
        ir.settings.push(Setting {
            index: settings::TABLE.len() as u16,
            value: 0,
        });
        assert!(matches!(
            load(&ir.encode()).unwrap_err(),
            Refusal::Setting {
                reason: "is not in the settings table",
                ..
            }
        ));
        let mut ir = sample();
        ir.settings[1].value = 3; // shields is 0 to 2.
        assert!(matches!(
            verify(&ir).unwrap_err(),
            Refusal::Setting {
                reason: "has a value outside its range",
                ..
            }
        ));
        // Disorder never decodes; in memory the verifier refuses it too.
        let mut ir = sample();
        ir.settings.reverse();
        assert!(matches!(load(&ir.encode()).unwrap_err(), Refusal::Decode(_)));
        assert!(matches!(
            verify(&ir).unwrap_err(),
            Refusal::Setting {
                reason: "is out of order or named twice",
                ..
            }
        ));
        let mut ir = sample();
        ir.header.about = "x".repeat(MAX_ABOUT_UNITS + 1);
        assert_eq!(verify(&ir), Err(Refusal::About(MAX_ABOUT_UNITS + 1)));
    }

    #[test]
    fn a_sixth_quick_mark_is_refused() {
        let mut ir = sample();
        ir.quick = ["score_limit", "time_limit", "lives", "respawn_time", "shields"]
            .map(|n| Quick::Setting(row(n)))
            .to_vec();
        assert!(load(&ir.encode()).is_ok());
        ir.quick.push(Quick::Option(0));
        assert!(matches!(load(&ir.encode()).unwrap_err(), Refusal::Decode(_)));
        assert_eq!(
            verify(&ir),
            Err(Refusal::Limit {
                what: "quick marks",
                max: MAX_QUICK
            })
        );
    }

    #[test]
    fn a_quick_mark_names_a_setting_or_an_option() {
        let refused = |q: Quick| {
            let mut ir = sample();
            ir.quick = vec![q];
            verify(&ir).err()
        };
        assert_eq!(refused(Quick::Setting(row("lives"))), None);
        assert_eq!(refused(Quick::Option(0)), None);
        assert_eq!(
            refused(Quick::Option(1)),
            Some(Refusal::Quick {
                index: 0,
                reason: "names no option"
            })
        );
        assert_eq!(
            refused(Quick::Setting(settings::TABLE.len() as u16)),
            Some(Refusal::Quick {
                index: 0,
                reason: "names no setting"
            })
        );
        assert_eq!(
            refused(Quick::Setting(row("gravity"))),
            Some(Refusal::Quick {
                index: 0,
                reason: "names no record setting"
            })
        );
        let mut ir = sample();
        ir.quick = vec![Quick::Option(0), Quick::Option(0)];
        assert_eq!(
            verify(&ir),
            Err(Refusal::Quick {
                index: 1,
                reason: "is marked twice"
            })
        );
    }

    #[test]
    fn a_uses_script_takes_no_handlers() {
        let mut ir = sample();
        ir.header.uses = Some(Type::Slayer);
        assert_eq!(load(&ir.encode()).unwrap_err(), Refusal::UsesHolds("handlers"));
        ir.handlers.clear();
        assert_eq!(verify(&ir), Err(Refusal::UsesHolds("variables")));
        ir.slots.clear();
        // The sample's quick line marks its option.
        ir.quick.retain(|q| matches!(q, Quick::Setting(_)));
        assert_eq!(verify(&ir), Err(Refusal::UsesHolds("options")));
        ir.options.clear();
        let report = load(&ir.encode()).unwrap().1;
        assert_eq!(report.worst_tick, 0);
    }

    #[test]
    fn op_refusals_name_the_op() {
        fn refuse(f: impl Fn(&mut Ir)) -> Fault {
            let mut ir = sample();
            f(&mut ir);
            match verify(&ir) {
                Err(Refusal::Op { fault, .. }) => fault,
                other => panic!("expected an op refusal, got {other:?}"),
            }
        }
        // A backward jump.
        assert_eq!(
            refuse(|ir| ir.handlers[0].code[2] = Op::JumpIfNot { cond: 3, to: 1 }),
            Fault::Backward(1)
        );
        // A jump past the end.
        assert_eq!(
            refuse(|ir| ir.handlers[0].code[2] = Op::Jump { to: 99 }),
            Fault::JumpOut(99)
        );
        // A register that does not exist.
        assert_eq!(
            refuse(|ir| ir.handlers[0].code[0] = Op::Const { dst: 9, value: 0 }),
            Fault::Register(9)
        );
        // A wrong type.
        assert!(matches!(
            refuse(|ir| ir.handlers[0].code[0] = Op::Const { dst: 0, value: 99 }),
            Fault::Const(99)
        ));
        // A slot that does not exist, and a scoped slot without its owner.
        assert_eq!(
            refuse(|ir| ir.handlers[0].code[0] = Op::Load { dst: 3, slot: 7, owner: None }),
            Fault::Slot(7)
        );
        assert_eq!(
            refuse(|ir| ir.handlers[0].code[0] = Op::Load { dst: 3, slot: 0, owner: None }),
            Fault::Owner(0)
        );
        // An unknown entry, and one whose capability is not declared.
        assert_eq!(
            refuse(|ir| ir.handlers[0].code[3] = Op::Call { entry: 999, args: vec![], dst: None }),
            Fault::Entry(999)
        );
        assert_eq!(
            refuse(|ir| {
                ir.handlers[0].code[3] = Op::Call {
                    entry: caps::id::SET_TEAM,
                    args: vec![0, 0],
                    dst: None,
                }
            }),
            Fault::Type {
                reg: 0,
                want: "team",
                got: Ty::Player
            }
        );
        assert_eq!(
            refuse(|ir| {
                ir.handlers[0].code[3] = Op::Call {
                    entry: caps::id::RESPAWN,
                    args: vec![0, 0],
                    dst: None,
                }
            }),
            Fault::Arity {
                entry: "respawn",
                want: 1,
                got: 2
            }
        );
        assert_eq!(
            refuse(|ir| {
                ir.handlers[0].code[3] = Op::Call {
                    entry: caps::id::DELETE,
                    args: vec![4],
                    dst: None,
                }
            }),
            Fault::Undeclared {
                entry: "delete",
                cap: "objects.delete"
            }
        );
        // An action's result written to a register.
        assert!(matches!(
            refuse(|ir| {
                ir.handlers[0].code[3] = Op::Call {
                    entry: caps::id::SCORE,
                    args: vec![0, 2],
                    dst: Some(2),
                }
            }),
            Fault::Result { .. }
        ));
        // A label, a palette and a ForEach body out of range.
        assert_eq!(
            refuse(|ir| {
                ir.handlers[0].code[4] = Op::ForEach {
                    set: Set {
                        base: SetBase::Labelled(5),
                        pick: None,
                    },
                    dst: 4,
                    body: 0,
                }
            }),
            Fault::Label(5)
        );
        assert_eq!(
            refuse(|ir| {
                ir.handlers[0].code[4] = Op::ForEach {
                    set: Set {
                        base: SetBase::Players,
                        pick: None,
                    },
                    dst: 0,
                    body: 9,
                }
            }),
            Fault::BodyOut(9)
        );
        assert_eq!(
            refuse(|ir| {
                ir.handlers[0].code[5] = Op::Grant {
                    player: 0,
                    cap: 0,
                    uses: 1,
                    palette: Some(3),
                }
            }),
            Fault::Palette(3)
        );
        // A table that does not exist.
        assert_eq!(
            refuse(|ir| {
                ir.handlers[0].code[0] = Op::TableRead {
                    dst: 2,
                    table: 4,
                    row: 2,
                    col: 0,
                }
            }),
            Fault::Table(4)
        );
    }

    #[test]
    fn a_jump_cannot_leave_a_loop_body() {
        let mut ir = sample();
        let hd = &mut ir.handlers[0];
        hd.code = vec![
            Op::ForEach {
                set: Set {
                    base: SetBase::Players,
                    pick: None,
                },
                dst: 0,
                body: 1,
            },
            Op::Jump { to: 3 },
            Op::Const { dst: 2, value: 0 },
        ];
        assert_eq!(
            verify(&ir),
            Err(Refusal::Op {
                handler: 0,
                pc: 1,
                fault: Fault::JumpOut(3)
            })
        );
        ir.handlers[0].code[1] = Op::Jump { to: 2 };
        let report = verify(&ir).unwrap();
        // The loop op, 16 bodies of one op, and the const.
        assert_eq!(report.handler_ops, vec![1 + 16 + 1]);
    }

    #[test]
    fn grants_need_a_declared_grantable_capability() {
        let mut ir = sample();
        ir.header.requires = vec!["hud.message".into()];
        assert_eq!(
            verify(&ir),
            Err(Refusal::Op {
                handler: 0,
                pc: 5,
                fault: Fault::NotGrantable("hud.message".into())
            })
        );
        ir.header.requires.clear();
        assert_eq!(
            verify(&ir),
            Err(Refusal::Op {
                handler: 0,
                pc: 5,
                fault: Fault::CapIndex(0)
            })
        );
    }

    #[test]
    fn table_option_and_slot_refusals() {
        let mut ir = sample();
        ir.tables[0].columns[0].ty = Ty::Bool;
        assert_eq!(verify(&ir), Err(Refusal::Cell { table: 0, cell: 0 }));

        let mut ir = sample();
        ir.options[0].default = 9;
        assert_eq!(verify(&ir), Err(Refusal::Option(0)));

        let mut ir = sample();
        ir.options[0].hi = i32::MAX;
        ir.options[0].lo = i32::MIN;
        ir.options[0].default = 0;
        assert!(verify(&ir).is_ok(), "32 bits fit");
        let mut wide = ir.options[0].clone();
        wide.lo = 0;
        wide.hi = 1;
        wide.default = 0;
        ir.options.push(wide);
        assert_eq!(verify(&ir), Err(Refusal::OptionBits(33)));

        let mut ir = sample();
        ir.slots[0].ty = Ty::Timer;
        assert_eq!(verify(&ir), Err(Refusal::Slot(0)));

        let mut ir = sample();
        ir.labels[0].text = "x".repeat(33);
        assert_eq!(
            verify(&ir),
            Err(Refusal::Text {
                table: "the label table",
                index: 0,
                max: MAX_LABEL_BYTES
            })
        );
    }

    #[test]
    fn label_table_refusals() {
        use crate::ir::label_flags::*;
        fn refuse(f: impl Fn(&mut LabelDef)) -> Refusal {
            let mut ir = sample();
            f(&mut ir.labels[1]);
            verify(&ir).unwrap_err()
        }
        let label = |reason| Refusal::Label { index: 1, reason };
        assert_eq!(
            refuse(|l| l.text = "Zone".into()),
            label("its text holds a character a Forge label cannot")
        );
        assert_eq!(refuse(|l| l.text = "ff_drop".into()), label("another label has its text"));
        assert_eq!(refuse(|l| l.flags |= 0x10), label("it has an unknown flag"));
        assert_eq!(refuse(|l| l.min = 5), label("its member count is not min <= max <= 256"));
        assert_eq!(refuse(|l| l.max = 257), label("its member count is not min <= max <= 256"));
        assert_eq!(
            refuse(|l| {
                l.flags = SINGLE;
                l.lo = 0;
                l.hi = 0;
                l.min = 1;
            }),
            label("a single label has at most 1 member and no other requirement")
        );
        assert_eq!(
            refuse(|l| {
                l.flags = SINGLE | NUMBERED;
                l.lo = 0;
                l.hi = 0;
                l.min = 1;
                l.max = 1;
            }),
            label("a single label has at most 1 member and no other requirement")
        );
        assert_eq!(
            refuse(|l| {
                l.kind = LabelKind::Spawn;
                l.flags |= PER_TEAM;
            }),
            label("starting locations are a set with no per team")
        );
        assert_eq!(refuse(|l| l.flags = RANGE), label("a number range needs numbered"));
        assert_eq!(
            refuse(|l| {
                l.lo = 2;
                l.hi = 1;
            }),
            label("its number range is not ordered")
        );
        assert_eq!(
            refuse(|l| {
                l.max = 256;
                l.lo = 0;
                l.hi = 256;
            }),
            label("its number range holds more than 256 numbers")
        );
        assert_eq!(
            refuse(|l| l.hi = 4),
            label("its number range needs more members than its max")
        );
        assert_eq!(
            refuse(|l| l.flags = NUMBERED),
            label("it has a number range without the flag")
        );

        let mut ir = sample();
        ir.labels = (0..=MAX_LABELS).map(|i| LabelDef::objects(format!("l{i}"))).collect();
        assert_eq!(
            verify(&ir),
            Err(Refusal::Limit {
                what: "labels",
                max: MAX_LABELS
            })
        );

        // A single optional label, a numbered set and spawns all pass.
        let mut ir = sample();
        ir.labels.push(LabelDef {
            text: "bonus".into(),
            kind: LabelKind::Object,
            min: 0,
            max: 1,
            flags: SINGLE,
            lo: 0,
            hi: 0,
        });
        ir.labels.push(LabelDef {
            kind: LabelKind::Spawn,
            flags: NUMBERED,
            ..LabelDef::objects("attack")
        });
        verify(&ir).unwrap();
    }

    #[test]
    fn a_loop_cannot_visit_starting_locations() {
        let mut ir = sample();
        ir.labels[0].kind = LabelKind::Spawn;
        assert_eq!(
            verify(&ir),
            Err(Refusal::Op {
                handler: 0,
                pc: 4,
                fault: Fault::SpawnSet(0)
            })
        );
    }

    #[test]
    fn a_labelled_loop_costs_its_label_max() {
        let mut ir = sample();
        ir.handlers[0].code = vec![
            Op::ForEach {
                set: Set {
                    base: SetBase::Labelled(1),
                    pick: None,
                },
                dst: 4,
                body: 1,
            },
            Op::Const { dst: 2, value: 0 },
        ];
        // The loop op and 4 bodies of one op: label 1's max is 4.
        assert_eq!(verify(&ir).unwrap().handler_ops, vec![1 + 4]);
        ir.labels[1].max = 2;
        ir.labels[1].min = 2;
        assert_eq!(verify(&ir).unwrap().handler_ops, vec![1 + 2]);
        // An unbounded label costs 256.
        if let Op::ForEach { set, .. } = &mut ir.handlers[0].code[0] {
            set.base = SetBase::Labelled(0);
        }
        assert_eq!(verify(&ir).unwrap().handler_ops, vec![1 + 256]);
    }

    #[test]
    fn the_estimate_shares_zone_events_and_names_the_costliest() {
        let mut ir = sample();
        let mut enter = ir.handlers[0].clone();
        enter.event = Event::Enter;
        enter.regs[0] = Ty::Object;
        enter.regs[1] = Ty::Object;
        enter.code = vec![Op::Const { dst: 2, value: 1 }; 3];
        let mut leave = enter.clone();
        leave.event = Event::Leave;
        leave.code.truncate(1);
        ir.handlers.push(enter);
        ir.handlers.push(leave);
        let report = verify(&ir).unwrap();
        assert_eq!(report.handler_ops, vec![6, 3, 1]);
        assert_eq!(report.worst_tick, 6 * 16 + 3 * 64);
        assert_eq!(report.costliest, Some(1));
    }

    #[test]
    fn unhostable_names_the_first_untraced_entry() {
        let mut ir = sample();
        assert_eq!(unhostable(&ir), None);
        ir.header.requires.push("player.team".into());
        ir.handlers[0].regs.push(Ty::Team);
        ir.handlers[0].code[3] = Op::Call {
            entry: caps::id::SET_TEAM,
            args: vec![0, 5],
            dst: None,
        };
        verify(&ir).unwrap();
        assert_eq!(unhostable(&ir).map(|e| e.name), Some("set_team"));
    }

    #[test]
    fn any_script_can_give_its_players_role_traits() {
        let src = |ty: &str| {
            format!("rules \"Crown\" 1\ntype {ty}\n\non kill(victim, killer) {{ role_traits(victim, false) }}\n")
        };
        let on_juggernaut = crate::compile(&src("juggernaut")).unwrap();
        assert_eq!(unhostable(&on_juggernaut), None);
        let on_slayer = crate::compile(&src("slayer")).unwrap();
        assert_eq!(unhostable(&on_slayer), None);
    }

    /// The sample with one `on kill` handler of `code`: registers victim,
    /// killer, an int, a bool, a text and an enum; string 2 is
    /// `{player} scored, {int} to go`, string 3 `{object}`.
    fn message_ir(code: Vec<Op>) -> Ir {
        let mut ir = sample();
        ir.header.requires.push("hud.message".into());
        ir.strings.push("{player} scored, {int} to go".into());
        ir.strings.push("{object}".into());
        ir.enums.push(EnumDef { name: 0, variants: vec![1] });
        let hd = &mut ir.handlers[0];
        hd.regs = vec![Ty::Player, Ty::Player, Ty::Int, Ty::Bool, Ty::Text, Ty::Enum(0)];
        hd.code = code;
        ir
    }

    fn call(entry: u16, args: &[Reg]) -> Op {
        Op::Call {
            entry,
            args: args.to_vec(),
            dst: None,
        }
    }

    fn refused(code: Vec<Op>) -> (usize, Fault) {
        match verify(&message_ir(code)) {
            Err(Refusal::Op { pc, fault, .. }) => (pc, fault),
            other => panic!("expected an op refusal, got {other:?}"),
        }
    }

    #[test]
    fn a_message_run_verifies_and_every_break_of_it_is_refused() {
        use crate::caps::{Hole, id};
        let text = Op::Const { dst: 4, value: 2 };
        let n = Op::Const { dst: 2, value: 3 };
        let good = vec![
            n.clone(),
            text.clone(),
            call(id::FMT_PLAYER, &[1]),
            call(id::FMT_INT, &[2]),
            call(id::MESSAGE, &[0, 4]),
        ];
        verify(&message_ir(good)).unwrap();

        // A run with no message after it.
        assert_eq!(
            refused(vec![n.clone(), text.clone(), call(id::FMT_PLAYER, &[1]), call(id::FMT_INT, &[2])]),
            (2, Fault::Unshown("fmt_player"))
        );
        assert_eq!(
            refused(vec![n.clone(), text.clone(), call(id::FMT_PLAYER, &[1]), call(id::SCORE, &[0, 2])]),
            (2, Fault::Unshown("fmt_player"))
        );
        // The values out of order, too few, or none.
        assert_eq!(
            refused(vec![
                n.clone(),
                text.clone(),
                call(id::FMT_INT, &[2]),
                call(id::FMT_PLAYER, &[1]),
                call(id::MESSAGE, &[0, 4]),
            ]),
            (
                4,
                Fault::HoleTypes {
                    want: vec![Hole::Player, Hole::Int],
                    got: vec![Hole::Int, Hole::Player],
                }
            )
        );
        assert_eq!(
            refused(vec![text.clone(), call(id::FMT_PLAYER, &[1]), call(id::MESSAGE, &[0, 4])]),
            (
                2,
                Fault::HoleTypes {
                    want: vec![Hole::Player, Hole::Int],
                    got: vec![Hole::Player],
                }
            )
        );
        assert_eq!(
            refused(vec![text.clone(), call(id::MESSAGE, &[0, 4])]),
            (
                1,
                Fault::HoleTypes {
                    want: vec![Hole::Player, Hole::Int],
                    got: vec![],
                }
            )
        );
        // A value of the wrong register type.
        assert!(matches!(
            refused(vec![text.clone(), call(id::FMT_PLAYER, &[2]), call(id::MESSAGE, &[0, 4])]),
            (1, Fault::Type { reg: 2, .. })
        ));
        assert_eq!(
            refused(vec![text.clone(), call(id::FMT_ENUM, &[2]), call(id::MESSAGE, &[0, 4])]),
            (
                1,
                Fault::Type {
                    reg: 2,
                    want: "an enum",
                    got: Ty::Int,
                }
            )
        );
        // The text not written just before the run.
        assert_eq!(
            refused(vec![
                text.clone(),
                n.clone(),
                call(id::FMT_PLAYER, &[1]),
                call(id::FMT_INT, &[2]),
                call(id::MESSAGE, &[0, 4]),
            ]),
            (2, Fault::Template)
        );
        // A jump into the run.
        assert_eq!(
            refused(vec![
                n.clone(),
                Op::Cmp {
                    op: CmpOp::Eq,
                    dst: 3,
                    a: 0,
                    b: 1,
                },
                Op::JumpIfNot { cond: 3, to: 5 },
                text.clone(),
                call(id::FMT_PLAYER, &[1]),
                call(id::FMT_INT, &[2]),
                call(id::MESSAGE, &[0, 4]),
            ]),
            (5, Fault::IntoHoles)
        );
        // More values than a message shows.
        let mut five = vec![n.clone(), text.clone()];
        five.extend((0..5).map(|_| call(id::FMT_INT, &[2])));
        five.push(call(id::MESSAGE, &[0, 4]));
        assert_eq!(refused(five), (2, Fault::Holes(5)));
        // A text that is no template.
        assert_eq!(
            refused(vec![Op::Const { dst: 4, value: 3 }, call(id::MESSAGE, &[0, 4])]),
            (1, Fault::NotTemplate(3))
        );
        // An enum value of any enum type is a value.
        let mut ir = message_ir(vec![]);
        ir.strings.push("{enum}".into());
        ir.handlers[0].code = vec![
            Op::Const { dst: 4, value: 4 },
            call(id::FMT_ENUM, &[5]),
            call(id::MESSAGE_ALL, &[4]),
        ];
        verify(&ir).unwrap();
    }
}
