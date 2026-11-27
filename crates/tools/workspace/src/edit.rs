//! The `edits` builder, and the record edits the `tag` builder shares
//! (docs/editor/tag-workspace.md, "Edits are data").
//!
//! An edit file is a YAML list, one entry per tag that exists:
//!
//! ```yaml
//! - tag: 'jpt!:globals\damage\sample'   # <group>:<path>
//!   was: <SHA-256 of the tag the edit was made on>   # optional
//!   block:                              # applied first, so `set` can reach what they add
//!     - {path: player_responses, op: add, like: 0}
//!     - {path: player_responses, op: remove, index: 1}
//!   set:
//!     - {path: damage_lower_bound, value: 12.5}
//!     - {path: "player_responses[0].duration", value: 0.5}
//! ```
//!
//! Paths are `tags::defs` field paths (`a.b[2].c`), the ones `tag.set` takes.
//! The tag is read through its group's definition
//! ([`tags::defs::from_built`]), changed as a [`Record`] and written back
//! ([`tags::defs::to_built`]); a tag the base map pinned is laid out anew
//! first ([`Unpin`](crate::Unpin)). A line that does not resolve stops the
//! build with its tag and the entry's line; nothing is dropped quietly.
//!
//! A line's `from` is the value it replaces, checked against one field
//! (docs/engine/asset-store.md, "Mods and packages"): two mods that set
//! different fields of one asset both apply, and a line applies to each
//! map's variant of the asset in which the field has that value. The check
//! is against the tag as the build first found it, so a line of a later
//! package still applies over an earlier package's line on the same field
//! (the later value wins and is listed). A `from` or `was` that does not
//! match, a key the map lacks and a patch of an asset an earlier package
//! overrides whole without naming its hash are refusals
//! ([`crate::Cx::refuse`]): here they stop the build; the game's build leaves the
//! package off for the map with the reason.
use content::Sha256;
use map::FourCC;
use serde::{Deserialize, Serialize};
use serde_yaml_ng::Value;
use tags::defs::{
    Defs, FieldDef, Kind as Fk, Record, Str, StructDef, Target, Val, default_record, from_built,
    parse_fourcc, to_built,
};
use tags::repo::TagKey;

use crate::build::{Cx, Failure};
use crate::builders::Job;

/// One `set` line.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetLine {
    pub path: String,
    /// The value the line replaces, when it names one: the line is for
    /// other content if the field does not hold it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<Value>,
    pub value: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Op {
    Add,
    Remove,
}

/// One `block` line: add an element (a copy of element `like`, or the
/// definition's defaults) or remove element `index`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockLine {
    pub path: String,
    pub op: Op,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub like: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
}

/// One entry of an edit file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TagEdit {
    pub tag: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub was: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub block: Vec<BlockLine>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub set: Vec<SetLine>,
}

impl SetLine {
    /// A `set` line from the JSON value `tag.set` takes for `path`.
    pub fn from_json(path: &str, value: &serde_json::Value) -> Result<Self, String> {
        let value = serde_yaml_ng::to_value(value).map_err(|e| format!("{path}: {e}"))?;
        Ok(Self {
            path: path.to_owned(),
            from: None,
            value,
        })
    }
}

/// An edit file's text for `edits`: the YAML list [`parse_edits`] reads
/// back to the same entries. No entries is the empty text.
pub fn edits_text(edits: &[TagEdit]) -> Result<String, String> {
    if edits.is_empty() {
        return Ok(String::new());
    }
    serde_yaml_ng::to_string(edits).map_err(|e| e.to_string())
}

pub(crate) fn text_of(source: &[u8]) -> Result<&str, Failure> {
    std::str::from_utf8(source).map_err(|e| Failure::from(format!("not UTF-8 text: {e}")))
}

pub(crate) fn yaml_failure(e: serde_yaml_ng::Error) -> Failure {
    Failure {
        why: e.to_string(),
        line: e.location().map(|l| l.line()),
    }
}

/// An edit file's entries, each with the line its `-` is on.
pub fn parse_edits(text: &str) -> Result<Vec<(TagEdit, usize)>, Failure> {
    let edits: Vec<TagEdit> = serde_yaml_ng::from_str(text).map_err(yaml_failure)?;
    let starts: Vec<usize> = text
        .lines()
        .enumerate()
        .filter(|(_, l)| l.starts_with("- ") || *l == "-")
        .map(|(i, _)| i + 1)
        .collect();
    Ok(edits
        .into_iter()
        .enumerate()
        .map(|(i, e)| (e, starts.get(i).copied().unwrap_or(1)))
        .collect())
}

/// `group:path`.
pub fn parse_group_ref(text: &str) -> Result<(FourCC, String), String> {
    let (g, path) = text
        .split_once(':')
        .ok_or_else(|| format!("{text:?} is not <group>:<path>"))?;
    let group = parse_fourcc(g).ok_or_else(|| format!("{g:?} is not a tag group"))?;
    if path.is_empty() {
        return Err(format!("{text:?} has no path"));
    }
    Ok((group, path.replace('/', "\\")))
}

/// The definition of `group`'s tags.
pub fn def_of(group: FourCC) -> Result<&'static StructDef, String> {
    Defs::builtin().group(group).ok_or_else(|| {
        format!(
            "no definition for group {}: it cannot be edited as data",
            tags::defs::fourcc_text(group)
        )
    })
}

/// The field a path ends at.
pub fn field_of<'d>(def: &'d StructDef, path: &str) -> Result<&'d FieldDef, String> {
    let segs: Vec<&str> = path.split('.').collect();
    let mut cur = def;
    for (i, seg) in segs.iter().enumerate() {
        let name = seg.split('[').next().unwrap_or(seg);
        let f = cur
            .field(name)
            .ok_or_else(|| format!("no field {name:?} in {path:?}"))?;
        if i + 1 == segs.len() {
            return Ok(f);
        }
        cur = match &f.kind {
            Fk::Block(s) | Fk::Struct(s) => s,
            _ => return Err(format!("{name:?} in {path:?} is not a block or struct")),
        };
    }
    Err("an empty path".into())
}

fn plain(path: &str) -> String {
    path.split('.')
        .map(|s| s.split('[').next().unwrap_or(s))
        .collect::<Vec<_>>()
        .join(".")
}

/// Refuse what can be seen without a map: bad YAML, a group with no
/// definition, a path the definition does not have.
pub fn check(job: &Job<'_>) -> Result<(), Failure> {
    let text = text_of(job.source)?;
    for (e, line) in parse_edits(text)? {
        let at = |why: String| Failure::at(line, why);
        let (group, _) = parse_group_ref(&e.tag).map_err(at)?;
        let def = def_of(group).map_err(at)?;
        if let Some(was) = &e.was {
            was.parse::<Sha256>().map_err(|e| at(e.to_string()))?;
        }
        for b in &e.block {
            check_block_line(def, b).map_err(at)?;
        }
        for s in &e.set {
            field_of(def, &s.path).map_err(at)?;
        }
    }
    Ok(())
}

pub(crate) fn check_block_line(def: &StructDef, b: &BlockLine) -> Result<(), String> {
    def.child(&plain(&b.path))
        .filter(|_| matches!(field_of(def, &b.path).map(|f| &f.kind), Ok(Fk::Block(_))))
        .ok_or_else(|| format!("{:?} is not a block", b.path))?;
    match b.op {
        Op::Add if b.index.is_some() => Err("an add line takes `like`, not `index`".into()),
        Op::Remove if b.index.is_none() => Err("a remove line needs `index`".into()),
        Op::Remove if b.like.is_some() => Err("a remove line takes `index`, not `like`".into()),
        _ => Ok(()),
    }
}

pub fn apply(cx: &mut Cx<'_>, job: &Job<'_>) -> Result<(), Failure> {
    let text = text_of(job.source)?;
    for (e, line) in parse_edits(text)? {
        edit_one(cx, &e).map_err(|why| Failure::at(line, why))?;
    }
    Ok(())
}

fn edit_one(cx: &mut Cx<'_>, e: &TagEdit) -> Result<(), String> {
    let (group, path) = parse_group_ref(&e.tag)?;
    let key = Cx::key(group, &path);
    // A key the map lacks, a stale `was` and a patch of an asset an earlier
    // package overrides whole are refusals (module docs).
    let id = cx
        .find(&key)
        .ok_or_else(|| cx.refuse(format!("the map has no tag {key}")))?;
    cx.touch(id)?;
    let over_override = cx.check_base(&key, id, e.was.as_deref())?;
    if e.block.is_empty() && e.set.is_empty() {
        return Ok(());
    }
    let def = def_of(group)?;
    let block = cx.read_block(id)?;
    // Nothing has changed the tag since the build found it, as a rule:
    // these are the bytes a later package's `from` lines check.
    if !over_override {
        cx.keep_base(id, &block);
    }
    let mut rec = from_built(def, &block).map_err(|x| format!("reading tag {key}: {x}"))?;
    check_froms(cx, def, &key, id, &rec, &e.set, over_override)?;
    let sets: Vec<(&str, &Value)> = e.set.iter().map(|s| (s.path.as_str(), &s.value)).collect();
    change(def, &mut rec, &e.block, &sets).map_err(|why| format!("tag {key}: {why}"))?;
    for s in &e.set {
        cx.note_set(&key, &s.path);
    }
    let built = write_back(cx, def, &rec, &key)?;
    cx.write_block(id, &key, built)
}

/// The `from` of each `set` line of `entry` against the tag it replaces:
/// `rec` as it is now, when the patch is over an earlier package's whole
/// override of the asset; else the tag as the build first found it
/// ([`crate::Cx::base_block`]). A line whose field does not hold its `from` is a
/// refusal; a line whose path does not resolve is the line's own error.
fn check_froms(
    cx: &mut Cx<'_>,
    def: &StructDef,
    key: &TagKey,
    id: map::TagId,
    rec: &Record,
    lines: &[SetLine],
    over_override: bool,
) -> Result<(), String> {
    if lines.iter().all(|s| s.from.is_none()) {
        return Ok(());
    }
    let first = if over_override {
        None
    } else {
        Some(cx.base_block(id).and_then(|block| {
            from_built(def, &block).map_err(|x| format!("reading tag {key}: {x}"))
        })?)
    };
    for s in lines.iter().filter(|s| s.from.is_some()) {
        let base = first.as_ref().unwrap_or(rec);
        if let Some(why) = from_mismatch(def, base, s)? {
            return Err(cx.refuse(format!("tag {key}: `from` {}: {why}", s.path)));
        }
    }
    Ok(())
}

/// A line's `from` against `base`: `Err` when its path does not resolve (as
/// its `value` would not), `Ok(Some(why))` when the field does not hold the
/// value it names, `Ok(None)` when it does or names none.
pub(crate) fn from_mismatch(
    def: &StructDef,
    base: &Record,
    s: &SetLine,
) -> Result<Option<String>, String> {
    let Some(from) = &s.from else {
        return Ok(None);
    };
    let field = field_of(def, &s.path).map_err(|why| format!("set {:?}: {why}", s.path))?;
    let cur = base
        .at(&s.path)
        .ok_or_else(|| format!("set {:?}: no such element", s.path))?;
    let want = match to_val(&field.kind, cur, from) {
        Ok(want) => want,
        Err(why) => return Ok(Some(format!("is not a value the field takes ({why})"))),
    };
    if same(&want, cur) {
        return Ok(None);
    }
    Ok(Some(format!(
        "is {}, but the field is {}",
        yaml(from),
        shown(cur)
    )))
}

/// Whether `want` (a `from` as the field's kind) is the value `cur` holds:
/// floats by their bits.
fn same(want: &Val, cur: &Val) -> bool {
    match (want, cur) {
        (Val::Float(a), Val::Float(b)) => a.to_bits() == b.to_bits(),
        (Val::List(a), Val::List(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same(a, b))
        }
        _ => want == cur,
    }
}

fn yaml(v: &Value) -> String {
    serde_yaml_ng::to_string(v)
        .unwrap_or_default()
        .trim_end()
        .to_owned()
}

/// A field's value for a line of text.
fn shown(v: &Val) -> String {
    match v {
        Val::Float(f) => f.to_string(),
        Val::Int(n) => n.to_string(),
        Val::Enum(n) | Val::Flags(n) | Val::Word(n) => n.to_string(),
        Val::String(Str::Text(s)) => s.clone(),
        Val::String(_) => "null".into(),
        Val::Ref { target, .. } | Val::Datum(target) => match target {
            Target::Key(k) => k.to_string(),
            Target::Id(id) => id.to_string(),
            Target::Null => "null".into(),
        },
        Val::List(items) => format!(
            "[{}]",
            items.iter().map(shown).collect::<Vec<_>>().join(", ")
        ),
        other => format!("{other:?}"),
    }
}

/// A record as a block tree, its tag references resolved in the map.
pub(crate) fn write_back(
    cx: &Cx<'_>,
    def: &StructDef,
    rec: &Record,
    key: &TagKey,
) -> Result<map::build::Block, String> {
    let set = &*cx.set;
    to_built(def, rec, &mut |k: &TagKey| set.find(k.group, &k.path))
        .map_err(|x| format!("writing tag {key}: {x}"))
}

/// Apply `block` lines, then `set` lines, to a record.
pub(crate) fn change(
    def: &StructDef,
    rec: &mut Record,
    blocks: &[BlockLine],
    sets: &[(&str, &Value)],
) -> Result<(), String> {
    for b in blocks {
        block_op(def, rec, b).map_err(|why| format!("block {:?}: {why}", b.path))?;
    }
    for (path, value) in sets {
        let field = field_of(def, path).map_err(|why| format!("set {path:?}: {why}"))?;
        let cur = rec
            .at(path)
            .ok_or_else(|| format!("set {path:?}: no such element"))?;
        let new = to_val(&field.kind, cur, value).map_err(|why| format!("set {path:?}: {why}"))?;
        *rec.at_mut(path).expect("found above") = new;
    }
    Ok(())
}

fn block_op(def: &StructDef, rec: &mut Record, b: &BlockLine) -> Result<(), String> {
    check_block_line(def, b)?;
    let child = def.child(&plain(&b.path)).expect("checked");
    let Some(val) = rec.at_mut(&b.path) else {
        return Err("no such element".into());
    };
    let Val::Block(elems) = val else {
        return Err("the tag keeps this block's elements in bytes it does not lay out".into());
    };
    match b.op {
        Op::Add => {
            let new = match b.like {
                Some(i) => elems
                    .get(i)
                    .cloned()
                    .ok_or_else(|| format!("`like` {i}: the block has {} elements", elems.len()))?,
                None => default_record(child),
            };
            elems.push(new);
        }
        Op::Remove => {
            let i = b.index.expect("checked");
            if i >= elems.len() {
                return Err(format!("index {i}: the block has {} elements", elems.len()));
            }
            elems.remove(i);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- values

/// `value` as the same kind of [`Val`] as `cur`.
fn to_val(kind: &Fk, cur: &Val, value: &Value) -> Result<Val, String> {
    match (kind, cur) {
        (Fk::Int { bytes, signed, .. }, Val::Int(_)) => {
            Ok(Val::Int(int_in(value, *bytes, *signed)?))
        }
        (Fk::Int { bytes, signed, .. }, Val::List(items)) => list(items.len(), value, |v| {
            int_in(v, *bytes, *signed).map(Val::Int)
        }),
        (Fk::Float { .. }, Val::Float(_)) => float(value).map(Val::Float),
        (Fk::Float { .. }, Val::List(items)) => {
            list(items.len(), value, |v| float(v).map(Val::Float))
        }
        (Fk::Enum { bytes, values }, Val::Enum(_)) => {
            let n = match value {
                Value::String(s) => match values.iter().position(|v| v == s) {
                    Some(p) => p as u64,
                    None => {
                        return Err(format!("{s:?} is not one of {}", values.join(", ")));
                    }
                },
                v => int_in(v, *bytes, false)? as u64,
            };
            Ok(Val::Enum(small(n, *bytes)?))
        }
        (Fk::Flags { bytes, bits }, Val::Flags(_)) => {
            let one = |v: &Value| -> Result<u32, String> {
                match v {
                    Value::String(s) => bits
                        .iter()
                        .position(|b| b == s)
                        .map(|p| 1u32 << p)
                        .ok_or_else(|| format!("{s:?} is not a flag: {}", bits.join(", "))),
                    v => Ok(int_in(v, *bytes, false)? as u32),
                }
            };
            let n = match value {
                Value::Sequence(items) => {
                    let mut n = 0;
                    for v in items {
                        n |= one(v)?;
                    }
                    n
                }
                v => one(v)?,
            };
            Ok(Val::Flags(small(u64::from(n), *bytes)?))
        }
        (Fk::StringId, Val::String(_)) => match value {
            Value::Null => Ok(Val::String(Str::Null)),
            Value::String(s) if s.is_empty() => Ok(Val::String(Str::Null)),
            Value::String(s) => Ok(Val::String(Str::Text(s.clone()))),
            _ => Err("a string ID takes text".into()),
        },
        (Fk::TagRef { groups }, Val::Ref { .. }) => match value {
            Value::Null => Ok(Val::Ref {
                group: u32::MAX,
                target: Target::Null,
            }),
            v => {
                let key = tag_ref(groups, v)?;
                Ok(Val::Ref {
                    group: key.group.as_u32(),
                    target: Target::Key(key),
                })
            }
        },
        (Fk::Datum { groups }, Val::Datum(_)) => match value {
            Value::Null => Ok(Val::Datum(Target::Null)),
            v => Ok(Val::Datum(Target::Key(tag_ref(groups, v)?))),
        },
        (Fk::Word, Val::Word(_)) => Ok(Val::Word(int_in(value, 4, false)? as u32)),
        (Fk::Block(_), _) => Err("a block is changed with `block` lines".into()),
        (Fk::Data { .. } | Fk::Raw | Fk::RawSize { .. } | Fk::Bsp, _) => {
            Err("this field's bytes are not set from a value".into())
        }
        (Fk::Unknown { .. } | Fk::Pad { .. }, _) => {
            Err("the definition does not know this field yet".into())
        }
        _ => Err("this field cannot be set from this kind of value".into()),
    }
}

fn list(
    len: usize,
    value: &Value,
    one: impl Fn(&Value) -> Result<Val, String>,
) -> Result<Val, String> {
    let Value::Sequence(items) = value else {
        return Err(format!("a list of {len} values"));
    };
    if items.len() != len {
        return Err(format!("{} values where the field has {len}", items.len()));
    }
    let vals: Result<Vec<Val>, String> = items.iter().map(one).collect();
    vals.map(Val::List)
}

fn int_in(value: &Value, bytes: u8, signed: bool) -> Result<i64, String> {
    let n = match value {
        Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_u64().and_then(|u| i64::try_from(u).ok()))
            .ok_or_else(|| format!("{n} is not a whole number in range"))?,
        Value::String(s) => parse_int(s).ok_or_else(|| format!("{s:?} is not a whole number"))?,
        _ => return Err("a whole number".into()),
    };
    let bits = u32::from(bytes) * 8;
    let (lo, hi) = if signed {
        (-(1i64 << (bits - 1)), (1i64 << (bits - 1)) - 1)
    } else {
        (0, (1i64 << bits) - 1)
    };
    if n < lo || n > hi {
        return Err(format!("{n} does not fit {lo}..={hi}"));
    }
    Ok(n)
}

fn parse_int(s: &str) -> Option<i64> {
    let (neg, s) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s),
    };
    let n = match s.strip_prefix("0x") {
        Some(h) => i64::from_str_radix(h, 16).ok()?,
        None => s.parse().ok()?,
    };
    Some(if neg { -n } else { n })
}

fn float(value: &Value) -> Result<f32, String> {
    match value {
        Value::Number(n) => n
            .as_f64()
            .map(|f| f as f32)
            .ok_or_else(|| format!("{n} is not a number")),
        _ => Err("a number".into()),
    }
}

fn small(n: u64, bytes: u8) -> Result<u32, String> {
    let max = (1u64 << (u32::from(bytes) * 8)) - 1;
    if n > max {
        return Err(format!("{n} does not fit {bytes} bytes"));
    }
    Ok(n as u32)
}

/// `group:path`, or a bare path when the field takes one group.
fn tag_ref(groups: &[FourCC], value: &Value) -> Result<TagKey, String> {
    let Value::String(s) = value else {
        return Err("a tag as <group>:<path>".into());
    };
    let (group, path) = match s.split_once(':') {
        Some(_) => parse_group_ref(s)?,
        None => match groups {
            [only] => (*only, s.replace('/', "\\")),
            _ => return Err(format!("{s:?}: name the group as <group>:<path>")),
        },
    };
    if !groups.is_empty() && !groups.contains(&group) {
        return Err(format!(
            "a {} tag where the field takes another group",
            tags::defs::fourcc_text(group)
        ));
    }
    Ok(TagKey::new(group, path))
}
