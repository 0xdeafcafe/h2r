//! A test map as a package (docs/editor/studio-redesign.md, "Each content
//! path as a package"): a `map` item (`onto`) and an `edits` item that
//! changes the base's scenario.
//!
//! [`scenario_edits`] is the scenario of one map file against another's, as
//! an `edits` file: `block` lines that add or remove placements at the end
//! of a block, and `set` lines for every field that differs. It stops with
//! the field named when one cannot be said as a value (bytes the definition
//! does not lay out, a tag the map does not name), and it checks its own
//! output: the lines, applied to the first map's scenario, give the
//! second's. [`test_map_package`] wraps the file in the package that
//! builds the map.
use std::collections::BTreeMap;
use std::io::Cursor;

use content::Sha256;
use content::package::{Lock, Manifest, Package};
use map::build::{TagPayload, TagSet};
use map::{FourCC, Map, TagId};
use serde_yaml_ng::{Mapping, Value};
use tags::defs::{
    Kind as Fk, Record, StructDef, Target, Val, default_record, fourcc_text, from_built, same,
};
use tags::repo::TagKey;

use crate::build::{Loaded, Unpin};
use crate::edit::{BlockLine, Op, SetLine, change, def_of};
use crate::unpin::MapUnpin;

const SCNR: FourCC = FourCC(*b"scnr");

/// A test map's package: what names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestMap<'a> {
    /// The package's id.
    pub id: &'a str,
    /// The map's name and package directory.
    pub name: &'a str,
    /// The disc map it is made on.
    pub base: &'a str,
    pub title: &'a str,
    pub purpose: &'a str,
}

/// The package of a test map: an `onto` map over `map.base` that takes the
/// `edits` file `edits` (what [`scenario_edits`] wrote).
pub fn test_map_package(map: &TestMap<'_>, edits: &str) -> Result<Loaded, String> {
    let spec = format!(
        "mode = \"onto\"\nbase = {:?}\nname = {:?}\ntitle = {:?}\ndescription = {:?}\n",
        map.base,
        map.name,
        format!("Test: {}", map.title),
        map.purpose
    );
    let manifest = format!(
        "[package]\nformat = 1\nid = {id:?}\nname = {title:?}\ndescription = {purpose:?}\n\
         [[item]]\nkind = \"map\"\nsource = \"map.toml\"\nmaps = \"all\"\n\
         [[item]]\nkind = \"edits\"\nsource = \"scenario.edits.yaml\"\nmaps = [{name:?}]\n",
        id = map.id,
        title = format!("Test: {}", map.title),
        purpose = map.purpose,
        name = map.name,
    );
    let manifest = Manifest::from_toml(&manifest).map_err(|e| format!("the manifest: {e}"))?;
    let files: BTreeMap<String, Vec<u8>> = [
        ("map.toml".to_owned(), spec.into_bytes()),
        ("scenario.edits.yaml".to_owned(), edits.as_bytes().to_vec()),
    ]
    .into();
    let lock = Lock::new(
        files
            .iter()
            .map(|(n, b)| (n.clone(), Sha256::of(b)))
            .collect(),
    );
    let package = Package::new(manifest, lock).map_err(|e| format!("the package: {e}"))?;
    Ok(Loaded { package, files })
}

/// A map's scenario as a record, its tag references named by key.
fn scenario_of(bytes: &[u8]) -> Result<(TagKey, Record), String> {
    let mut map = Map::from_reader(Cursor::new(bytes)).map_err(|e| format!("not a map: {e}"))?;
    let unpin = MapUnpin::new(&mut map)?;
    let set = TagSet::from_map(&mut map).map_err(|e| format!("decomposing the map: {e}"))?;
    let (id, name) = set
        .slots
        .iter()
        .find_map(|s| {
            let t = s.tag.as_ref()?;
            (t.group == SCNR).then(|| (t.id, s.name.clone()))
        })
        .ok_or("the map has no scenario")?;
    let name = name.ok_or("the scenario has no name")?;
    let entry = set.tag(id).ok_or("the map has no scenario")?;
    let block = match &entry.payload {
        TagPayload::Built(b) => b.clone(),
        TagPayload::Pinned { .. } => unpin.tree(id)?,
        _ => return Err("the scenario has no data".into()),
    };
    let def = def_of(SCNR)?;
    let mut rec = from_built(def, &block).map_err(|e| format!("reading the scenario: {e}"))?;
    let names = |t: TagId| -> Option<TagKey> {
        let s = set.slots.get(t.index())?;
        let e = s.tag.as_ref().filter(|e| e.id == t)?;
        Some(TagKey::new(e.group, s.name.clone()?))
    };
    keyed(&mut rec, &names);
    Ok((TagKey::new(SCNR, name), rec))
}

/// `rec` with every datum a tag reference names given as its key.
fn keyed(rec: &mut Record, names: &dyn Fn(TagId) -> Option<TagKey>) {
    for (_, v) in &mut rec.fields {
        keyed_val(v, names);
    }
}

fn keyed_val(v: &mut Val, names: &dyn Fn(TagId) -> Option<TagKey>) {
    match v {
        Val::Ref { target, .. } | Val::Datum(target) => {
            if let Target::Id(id) = target
                && let Some(k) = names(*id)
            {
                *target = Target::Key(k);
            }
        }
        Val::List(items) => items.iter_mut().for_each(|i| keyed_val(i, names)),
        Val::Block(elems) => elems.iter_mut().for_each(|e| keyed(e, names)),
        Val::Struct(r) => keyed(r, names),
        _ => {}
    }
}

fn join(path: &str, name: &str) -> String {
    if path.is_empty() {
        name.to_owned()
    } else {
        format!("{path}.{name}")
    }
}

fn number(n: impl Into<serde_yaml_ng::Number>) -> Value {
    Value::Number(n.into())
}

fn key_text(k: &TagKey) -> Value {
    Value::String(format!("{}:{}", fourcc_text(k.group), k.path))
}

/// `v` as the value a `set` line gives it.
fn yaml_of(v: &Val) -> Result<Value, String> {
    Ok(match v {
        Val::Int(i) => number(*i),
        Val::Float(f) if f.is_finite() => number(f64::from(*f)),
        Val::Float(_) => return Err("a float that is not finite".into()),
        Val::List(items) => Value::Sequence(items.iter().map(yaml_of).collect::<Result<_, _>>()?),
        Val::Enum(n) | Val::Flags(n) | Val::Word(n) => number(i64::from(*n)),
        Val::String(tags::defs::Str::Null) => Value::Null,
        Val::String(tags::defs::Str::Text(t)) => Value::String(t.clone()),
        Val::String(tags::defs::Str::Index(_)) => {
            return Err("a string ID the table does not name".into());
        }
        Val::Ref { group, target } => match target {
            Target::Null if *group == u32::MAX => Value::Null,
            Target::Key(k) if k.group.as_u32() == *group => key_text(k),
            _ => return Err("a tag reference the map does not name".into()),
        },
        Val::Datum(target) => match target {
            Target::Null => Value::Null,
            Target::Key(k) => key_text(k),
            Target::Id(_) => return Err("a datum the map does not name".into()),
        },
        _ => return Err("bytes the definition does not lay out as a value".into()),
    })
}

#[derive(Default)]
struct Lines {
    blocks: Vec<BlockLine>,
    sets: Vec<SetLine>,
}

impl Lines {
    fn len(&self) -> (usize, usize) {
        (self.blocks.len(), self.sets.len())
    }

    fn truncate(&mut self, at: (usize, usize)) {
        self.blocks.truncate(at.0);
        self.sets.truncate(at.1);
    }
}

fn walk(def: &StructDef, a: &Record, b: &Record, path: &str, out: &mut Lines) -> Result<(), String> {
    for f in &def.fields {
        let p = join(path, &f.name);
        let (Some(x), Some(y)) = (a.get(&f.name), b.get(&f.name)) else {
            return Err(format!("{p}: the field is missing"));
        };
        match (&f.kind, x, y) {
            (Fk::Struct(s), Val::Struct(x), Val::Struct(y)) => walk(s, x, y, &p, out)?,
            (Fk::Block(s), Val::Block(x), Val::Block(y)) => walk_block(s, x, y, &p, out)?,
            _ => {
                if !same(x, y) {
                    let value = yaml_of(y).map_err(|why| format!("{p}: {why}"))?;
                    out.sets.push(SetLine { path: p, value });
                }
            }
        }
    }
    Ok(())
}

/// A block's elements `x` to `y`: the tail removed or added, the elements
/// both have walked.
fn walk_block(
    def: &StructDef,
    x: &[Record],
    y: &[Record],
    path: &str,
    out: &mut Lines,
) -> Result<(), String> {
    for i in (y.len()..x.len()).rev() {
        out.blocks.push(BlockLine {
            path: path.to_owned(),
            op: Op::Remove,
            like: None,
            index: Some(i),
        });
    }
    for (i, (ex, ey)) in x.iter().zip(y).enumerate() {
        walk(def, ex, ey, &format!("{path}[{i}]"), out)?;
    }
    if y.len() > x.len() {
        let default = default_record(def);
        for (i, ey) in y.iter().enumerate().skip(x.len()) {
            // A copy of the block's last element keeps what the new one
            // shares with it; a new element of zeros says the rest.
            let copies = x.len().checked_sub(1);
            let mut last = String::new();
            for like in [copies, None] {
                let at = out.len();
                let template = like.map_or(&default, |l| &x[l]);
                out.blocks.push(BlockLine {
                    path: path.to_owned(),
                    op: Op::Add,
                    like,
                    index: None,
                });
                match walk(def, template, ey, &format!("{path}[{i}]"), out) {
                    Ok(()) => {
                        last.clear();
                        break;
                    }
                    Err(why) => {
                        out.truncate(at);
                        last = why;
                    }
                }
            }
            if !last.is_empty() {
                return Err(last);
            }
        }
    }
    Ok(())
}

/// Whether `lines`, applied to `before`, give `after`.
fn reproduces(def: &StructDef, before: &Record, after: &Record, lines: &Lines) -> Result<(), String> {
    let mut rec = before.clone();
    let sets: Vec<(&str, &Value)> = lines
        .sets
        .iter()
        .map(|s| (s.path.as_str(), &s.value))
        .collect();
    change(def, &mut rec, &lines.blocks, &sets)?;
    if rec == *after {
        return Ok(());
    }
    let first = tags::defs::diff(def, &rec, after)
        .into_iter()
        .next()
        .map_or("a layout detail".to_owned(), |c| {
            format!("{}: {} where the map has {}", c.path, c.a, c.b)
        });
    Err(format!("the edits do not give the map's scenario ({first})"))
}

/// The edit-file entry that turns scenario record `before` into `after`,
/// or `None` when they are the same.
fn entry_between(
    key: &TagKey,
    def: &StructDef,
    before: &Record,
    after: &Record,
) -> Result<Option<Value>, String> {
    let mut lines = Lines::default();
    walk(def, before, after, "", &mut lines)?;
    if lines.blocks.is_empty() && lines.sets.is_empty() {
        return Ok(None);
    }
    reproduces(def, before, after, &lines)?;
    let mut entry = Mapping::new();
    entry.insert(
        Value::String("tag".into()),
        Value::String(format!("{}:{}", fourcc_text(key.group), key.path)),
    );
    let blocks: Vec<Value> = lines
        .blocks
        .iter()
        .map(|b| {
            let mut m = Mapping::new();
            m.insert("path".into(), Value::String(b.path.clone()));
            let (op, n) = match b.op {
                Op::Add => ("add", b.like),
                Op::Remove => ("remove", b.index),
            };
            m.insert("op".into(), Value::String(op.into()));
            if let Some(n) = n {
                let word = if b.op == Op::Add { "like" } else { "index" };
                m.insert(word.into(), number(n as u64));
            }
            Value::Mapping(m)
        })
        .collect();
    if !blocks.is_empty() {
        entry.insert("block".into(), Value::Sequence(blocks));
    }
    let sets: Vec<Value> = lines
        .sets
        .iter()
        .map(|s| {
            let mut m = Mapping::new();
            m.insert("path".into(), Value::String(s.path.clone()));
            m.insert("value".into(), s.value.clone());
            Value::Mapping(m)
        })
        .collect();
    if !sets.is_empty() {
        entry.insert("set".into(), Value::Sequence(sets));
    }
    Ok(Some(Value::Mapping(entry)))
}

/// An `edits` file that changes the scenario of map file `before` to
/// `after`'s, or `None` when it is the same. Both are one map's bytes, the
/// second the first with its placements edited.
pub fn scenario_edits(before: &[u8], after: &[u8]) -> Result<Option<String>, String> {
    let (key, a) = scenario_of(before).map_err(|e| format!("the base map: {e}"))?;
    let (other, b) = scenario_of(after).map_err(|e| format!("the edited map: {e}"))?;
    if key != other {
        return Err(format!("the scenario is {key} in one map and {other} in the other"));
    }
    let def = def_of(SCNR)?;
    let Some(entry) = entry_between(&key, def, &a, &b)? else {
        return Ok(None);
    };
    serde_yaml_ng::to_string(&Value::Sequence(vec![entry]))
        .map(Some)
        .map_err(|e| format!("writing the edits: {e}"))
}

/// [`scenario_edits`] of two scenario records, for the tests and for a
/// caller that read them already.
pub fn records_edits(
    key: &TagKey,
    before: &Record,
    after: &Record,
) -> Result<Option<String>, String> {
    let def = def_of(key.group)?;
    let Some(entry) = entry_between(key, def, before, after)? else {
        return Ok(None);
    };
    serde_yaml_ng::to_string(&Value::Sequence(vec![entry]))
        .map(Some)
        .map_err(|e| format!("writing the edits: {e}"))
}
