//! Value trees of tags read through a definition, and the ways in and out:
//! loose tags ([`decode`], [`encode`]), builder trees ([`to_built`],
//! [`from_built`]), fresh tags ([`default_record`]) and comparison
//! ([`diff`]).

use super::{Coverage, FieldDef, Kind, StructDef};
use crate::repo::{Body, LooseTag, Node, RawRes, Region, TagKey, Tree, Value};
use map::build::{Block, FixupKind, RawRef};
use map::{FourCC, TagId};
use std::collections::{BTreeMap, HashMap};

/// What can go wrong between a definition and a tag.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{path}: {why}")]
pub struct DefError {
    /// Field path (`object.functions[2].output`).
    pub path: String,
    /// What is wrong.
    pub why: String,
}

fn fail<T>(path: &str, why: impl Into<String>) -> Result<T, DefError> {
    Err(DefError {
        path: path.to_owned(),
        why: why.into(),
    })
}

/// The tag a reference or datum names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// None (`0xffffffff`).
    Null,
    /// By group and path (loose tags).
    Key(TagKey),
    /// By datum in a tag set (builder trees, converters).
    Id(TagId),
}

/// A string ID's value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Str {
    /// Zero: no string.
    Null,
    /// By text.
    Text(String),
    /// A raw ID word of a pinned map's table (builder trees that keep a
    /// retail map's bytes).
    Index(u32),
}

/// A raw-resource pointer's value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RawVal {
    /// No resource: the word as stored (`0` or `0xffffffff`), or a pinned
    /// map's pointer in a builder tree.
    Word(u32),
    /// A resource of a loose tag.
    Res(RawRes),
    /// A resource of a builder tree.
    Built(RawRef),
}

/// One field's value.
#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    /// Integer (any width or sign).
    Int(i64),
    /// Float.
    Float(f32),
    /// Several integers or floats in a row.
    List(Vec<Val>),
    /// Enumeration value.
    Enum(u32),
    /// Flag bits.
    Flags(u32),
    /// String ID.
    String(Str),
    /// Tag reference: the group word as stored, and the tag.
    Ref { group: u32, target: Target },
    /// Bare datum.
    Datum(Target),
    /// Raw-resource pointer.
    Raw(RawVal),
    /// Plain word.
    Word(u32),
    /// BSP descriptor: whether the builder fills it, and its bytes.
    Bsp { fixup: bool, bytes: [u8; 12] },
    /// Block elements.
    Block(Vec<Record>),
    /// A block or data reference with no child array: a count with a null
    /// address (retail has some), or a pinned map's pointer kept in a
    /// builder tree.
    Unresolved { count: u32, address: u32 },
    /// Data-reference bytes.
    Data(Vec<u8>),
    /// Embedded struct.
    Struct(Record),
    /// An unknown span or padding, as bytes.
    Bytes(Vec<u8>),
}

/// One element: its fields' values, in the definition's order and names.
#[derive(Clone, Debug, Default)]
pub struct Record {
    /// (name, value) per field of the definition.
    pub fields: Vec<(String, Val)>,
    /// On a block's first element: the loose tag's node the block came
    /// from. Blocks with the same `share` encode as one array, as the
    /// cache compiler laid them out; blocks without one share by content.
    /// Layout, not a value: equality and [`diff`] ignore it.
    pub share: Option<u32>,
}

impl PartialEq for Record {
    fn eq(&self, o: &Self) -> bool {
        self.fields == o.fields
    }
}

impl Record {
    /// A record of these fields.
    pub fn new(fields: Vec<(String, Val)>) -> Self {
        Record {
            fields,
            share: None,
        }
    }
}

impl Record {
    /// The value of field `name`.
    pub fn get(&self, name: &str) -> Option<&Val> {
        self.fields.iter().find(|f| f.0 == name).map(|f| &f.1)
    }

    /// The value of field `name`, mutably.
    pub fn get_mut(&mut self, name: &str) -> Option<&mut Val> {
        self.fields
            .iter_mut()
            .find(|f| f.0 == name)
            .map(|f| &mut f.1)
    }

    /// The value at `path`: names joined by `.`, block elements as
    /// `name[i]` (`object.functions[0].output`).
    pub fn at(&self, path: &str) -> Option<&Val> {
        let (first, rest) = split_path(path);
        let (name, index) = parse_seg(first)?;
        let v = self.get(name)?;
        match (index, rest) {
            (Some(i), Some(rest)) => match v {
                Val::Block(e) => e.get(i)?.at(rest),
                _ => None,
            },
            (Some(_), None) => None,
            (None, Some(rest)) => match v {
                Val::Struct(r) => r.at(rest),
                _ => None,
            },
            (None, None) => Some(v),
        }
    }

    /// The value at `path`, mutably.
    pub fn at_mut(&mut self, path: &str) -> Option<&mut Val> {
        let (first, rest) = split_path(path);
        let (name, index) = parse_seg(first)?;
        let v = self.get_mut(name)?;
        match (index, rest) {
            (Some(i), Some(rest)) => match v {
                Val::Block(e) => e.get_mut(i)?.at_mut(rest),
                _ => None,
            },
            (Some(_), None) => None,
            (None, Some(rest)) => match v {
                Val::Struct(r) => r.at_mut(rest),
                _ => None,
            },
            (None, None) => Some(v),
        }
    }

    /// Sets the value at `path`, which must exist and keep its kind.
    pub fn set(&mut self, path: &str, value: Val) -> Result<(), DefError> {
        let Some(v) = self.at_mut(path) else {
            return fail(path, "no such field");
        };
        if std::mem::discriminant(v) != std::mem::discriminant(&value)
            && !matches!(
                (&*v, &value),
                (Val::Block(_), Val::Unresolved { .. }) | (Val::Unresolved { .. }, Val::Block(_))
            )
        {
            return fail(path, format!("{} where {} is", short(&value), short(v)));
        }
        *v = value;
        Ok(())
    }
}

fn split_path(path: &str) -> (&str, Option<&str>) {
    match path.split_once('.') {
        Some((a, b)) => (a, Some(b)),
        None => (path, None),
    }
}

fn parse_seg(seg: &str) -> Option<(&str, Option<usize>)> {
    match seg.split_once('[') {
        Some((n, i)) => Some((n, Some(i.strip_suffix(']')?.parse().ok()?))),
        None => Some((seg, None)),
    }
}

impl Val {
    /// A tag reference to `id` of `group` (a builder tree's datum).
    pub fn tag(group: FourCC, id: TagId) -> Val {
        if id == TagId::NULL {
            Val::Ref {
                group: u32::MAX,
                target: Target::Null,
            }
        } else {
            Val::Ref {
                group: group.as_u32(),
                target: Target::Id(id),
            }
        }
    }

    /// A string ID by text (empty: none).
    pub fn text(s: &str) -> Val {
        Val::String(if s.is_empty() {
            Str::Null
        } else {
            Str::Text(s.to_owned())
        })
    }

    /// A list of floats.
    pub fn floats(v: &[f32]) -> Val {
        Val::List(v.iter().map(|x| Val::Float(*x)).collect())
    }
}

// ------------------------------------------------------------------ decode

/// Reads a loose tag through its group's definition.
pub fn decode(def: &StructDef, tag: &LooseTag) -> Result<Record, DefError> {
    let Body::Tree(tree) = &tag.body else {
        return fail("", "an alias has no bytes of its own");
    };
    let root = tree.nodes.first().ok_or_else(|| DefError {
        path: String::new(),
        why: "no root".into(),
    })?;
    if root.count != 1 || root.size != def.size {
        return fail(
            "",
            format!(
                "root is {} x {:#x}, the definition {:#x}",
                root.count, root.size, def.size
            ),
        );
    }
    let mut d = Decoder {
        tree,
        fields: tree
            .nodes
            .iter()
            .map(|n| n.fields.iter().map(|(o, v)| (*o, v)).collect())
            .collect(),
    };
    d.fields(def, 0, 0, "")
}

struct Decoder<'t> {
    tree: &'t Tree,
    fields: Vec<BTreeMap<u32, &'t Value>>,
}

fn int_at(b: &[u8], at: usize, bytes: u8, signed: bool) -> i64 {
    match (bytes, signed) {
        (1, false) => i64::from(b[at]),
        (1, true) => i64::from(b[at] as i8),
        (2, false) => i64::from(u16::from_le_bytes([b[at], b[at + 1]])),
        (2, true) => i64::from(i16::from_le_bytes([b[at], b[at + 1]])),
        (4, false) => i64::from(u32_at(b, at)),
        _ => i64::from(u32_at(b, at) as i32),
    }
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().expect("4 bytes"))
}

fn join(path: &str, name: &str) -> String {
    if path.is_empty() {
        name.to_owned()
    } else {
        format!("{path}.{name}")
    }
}

/// Plain values: everything with no fixup.
fn plain(kind: &Kind, b: &[u8], at: usize) -> Option<Val> {
    Some(match kind {
        Kind::Int {
            bytes,
            signed,
            count,
        } => {
            let one = |k: u32| {
                Val::Int(int_at(
                    b,
                    at + (k * u32::from(*bytes)) as usize,
                    *bytes,
                    *signed,
                ))
            };
            if *count == 1 {
                one(0)
            } else {
                Val::List((0..*count).map(one).collect())
            }
        }
        Kind::Float { count } => {
            let one = |k: u32| Val::Float(f32::from_bits(u32_at(b, at + 4 * k as usize)));
            if *count == 1 {
                one(0)
            } else {
                Val::List((0..*count).map(one).collect())
            }
        }
        Kind::Enum { bytes, .. } => Val::Enum(int_at(b, at, *bytes, false) as u32),
        Kind::Flags { bytes, .. } => Val::Flags(int_at(b, at, *bytes, false) as u32),
        Kind::RawSize { .. } => Val::Int(i64::from(u32_at(b, at))),
        Kind::Word => Val::Word(u32_at(b, at)),
        Kind::Unknown { len, .. } | Kind::Pad { len } => {
            Val::Bytes(b[at..at + *len as usize].to_vec())
        }
        _ => return None,
    })
}

impl Decoder<'_> {
    fn fields(
        &mut self,
        def: &StructDef,
        node: usize,
        at: u32,
        path: &str,
    ) -> Result<Record, DefError> {
        let mut out = Vec::with_capacity(def.fields.len());
        for f in &def.fields {
            let p = join(path, &f.name);
            let v = self.field(f, node, at + f.offset, &p)?;
            out.push((f.name.clone(), v));
        }
        Ok(Record::new(out))
    }

    fn field(&mut self, f: &FieldDef, node: usize, at: u32, path: &str) -> Result<Val, DefError> {
        let n = &self.tree.nodes[node];
        let b = &n.bytes;
        let w = f.kind.width();
        if (at + w) as usize > b.len() {
            return fail(path, "past the end of the element");
        }
        let fx = &self.fields[node];
        // Fixups inside the field, other than where this kind has its own.
        let own = match &f.kind {
            Kind::TagRef { .. } => Some(at + 4),
            Kind::Datum { .. }
            | Kind::StringId
            | Kind::Raw
            | Kind::Bsp
            | Kind::Block(_)
            | Kind::Data { .. } => Some(at),
            Kind::Struct(_) => None,
            _ => None,
        };
        if !matches!(f.kind, Kind::Struct(_))
            && let Some((o, v)) = fx.range(at..at + w).find(|(o, _)| Some(**o) != own)
        {
            return fail(
                path,
                format!("the tag has a {v:?} at {o:#x}, inside a field of another type"),
            );
        }
        if let Some(v) = plain(&f.kind, b, at as usize) {
            return Ok(v);
        }
        let mine = own.and_then(|o| fx.get(&o).copied());
        let a = at as usize;
        Ok(match &f.kind {
            Kind::StringId => match mine {
                Some(Value::String(t)) => Val::String(Str::Text(t.clone())),
                None if u32_at(b, a) == 0 => Val::String(Str::Null),
                None => Val::String(Str::Index(u32_at(b, a))),
                Some(v) => return fail(path, format!("string ID holds {v:?}")),
            },
            Kind::TagRef { .. } => Val::Ref {
                group: u32_at(b, a),
                target: self.target(mine, u32_at(b, a + 4), path)?,
            },
            Kind::Datum { .. } => Val::Datum(self.target(mine, u32_at(b, a), path)?),
            Kind::Raw => match mine {
                Some(Value::Raw(i)) => Val::Raw(RawVal::Res(
                    self.tree
                        .raws
                        .get(*i as usize)
                        .cloned()
                        .ok_or_else(|| DefError {
                            path: path.into(),
                            why: "raw index past the list".into(),
                        })?,
                )),
                None => Val::Raw(RawVal::Word(u32_at(b, a))),
                Some(v) => return fail(path, format!("raw pointer holds {v:?}")),
            },
            Kind::Bsp => Val::Bsp {
                fixup: matches!(mine, Some(Value::Bsp)),
                bytes: b[a..a + 12].try_into().expect("12"),
            },
            Kind::Block(child) => match mine {
                Some(Value::Block(c)) => {
                    let c = *c as usize;
                    let cn = &self.tree.nodes[c];
                    if cn.size != child.size {
                        return fail(
                            path,
                            format!(
                                "elements of {:#x}, the definition {:#x}",
                                cn.size, child.size
                            ),
                        );
                    }
                    let mut elems = Vec::with_capacity(cn.count as usize);
                    for i in 0..cn.count {
                        elems.push(self.fields(
                            child,
                            c,
                            i * child.size,
                            &format!("{path}[{i}]"),
                        )?);
                    }
                    if let Some(e) = elems.first_mut() {
                        e.share = Some(c as u32);
                    }
                    Val::Block(elems)
                }
                None => match (u32_at(b, a), u32_at(b, a + 4)) {
                    (0, 0) => Val::Block(Vec::new()),
                    (count, address) => Val::Unresolved { count, address },
                },
                Some(v) => return fail(path, format!("block holds {v:?}")),
            },
            Kind::Data { .. } => match mine {
                Some(Value::Data(c)) => Val::Data(self.tree.nodes[*c as usize].bytes.clone()),
                None => match (u32_at(b, a), u32_at(b, a + 4)) {
                    (0, 0) => Val::Data(Vec::new()),
                    (count, address) => Val::Unresolved { count, address },
                },
                Some(v) => return fail(path, format!("data reference holds {v:?}")),
            },
            Kind::Struct(s) => Val::Struct(self.fields(s, node, at, path)?),
            _ => unreachable!("plain kinds handled above"),
        })
    }

    fn target(&self, v: Option<&Value>, word: u32, path: &str) -> Result<Target, DefError> {
        match v {
            Some(Value::Ref(k)) => Ok(Target::Key(k.clone())),
            None if word == u32::MAX => Ok(Target::Null),
            None => Ok(Target::Id(TagId(word))),
            Some(v) => fail(path, format!("reference holds {v:?}")),
        }
    }
}

// ------------------------------------------------------------------ encode

/// A node before numbering: children owned, not yet shared.
struct ONode {
    bytes: Vec<u8>,
    count: u32,
    size: u32,
    align: u32,
    fields: Vec<(u32, OField)>,
    digest: [u8; 32],
    share: Option<u32>,
}

enum OField {
    Block(Box<ONode>),
    Data(Box<ONode>),
    Ref(TagKey),
    Str(String),
    Raw(RawRes),
    Bsp,
}

/// Writes a record as a loose tag. Arrays with the same contents become
/// one node, as the cache compiler shares them.
pub fn encode(
    def: &StructDef,
    key: TagKey,
    rec: &Record,
    region: Region,
) -> Result<LooseTag, DefError> {
    let mut root = ONode {
        bytes: vec![0; def.size as usize],
        count: 1,
        size: def.size,
        align: def.align.max(1),
        fields: Vec::new(),
        digest: [0; 32],
        share: None,
    };
    write_fields(def, rec, &mut root, 0, "")?;
    digest(&mut root);
    let mut tree = Tree {
        region,
        ..Default::default()
    };
    let mut seen = HashMap::new();
    let mut raws = HashMap::new();
    flatten(&root, &mut tree, &mut seen, &mut raws);
    Ok(LooseTag {
        key,
        body: Body::Tree(tree),
    })
}

fn put_int(b: &mut [u8], at: usize, bytes: u8, v: i64) {
    let le = v.to_le_bytes();
    b[at..at + bytes as usize].copy_from_slice(&le[..bytes as usize]);
}

fn put_u32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// Writes a plain value; `false` when the kind is not plain.
fn write_plain(
    kind: &Kind,
    v: &Val,
    b: &mut [u8],
    at: usize,
    path: &str,
) -> Result<bool, DefError> {
    let mismatch = || fail(path, format!("{} for a {kind:?}", short(v)));
    match (kind, v) {
        (
            Kind::Int {
                bytes, count: 1, ..
            },
            Val::Int(x),
        ) => put_int(b, at, *bytes, *x),
        (Kind::Int { bytes, count, .. }, Val::List(l)) if l.len() == *count as usize => {
            for (k, x) in l.iter().enumerate() {
                let Val::Int(x) = x else { return mismatch() };
                put_int(b, at + k * *bytes as usize, *bytes, *x);
            }
        }
        (Kind::Float { count: 1 }, Val::Float(x)) => put_u32(b, at, x.to_bits()),
        (Kind::Float { count }, Val::List(l)) if l.len() == *count as usize => {
            for (k, x) in l.iter().enumerate() {
                let Val::Float(x) = x else { return mismatch() };
                put_u32(b, at + 4 * k, x.to_bits());
            }
        }
        (Kind::Enum { bytes, .. }, Val::Enum(x)) | (Kind::Flags { bytes, .. }, Val::Flags(x)) => {
            put_int(b, at, *bytes, i64::from(*x))
        }
        (Kind::RawSize { .. }, Val::Int(x)) => put_u32(b, at, *x as u32),
        (Kind::Word, Val::Word(x)) => put_u32(b, at, *x),
        (Kind::Unknown { len, .. } | Kind::Pad { len }, Val::Bytes(x))
            if x.len() == *len as usize =>
        {
            b[at..at + x.len()].copy_from_slice(x)
        }
        (
            Kind::Int { .. }
            | Kind::Float { .. }
            | Kind::Enum { .. }
            | Kind::Flags { .. }
            | Kind::RawSize { .. }
            | Kind::Word
            | Kind::Unknown { .. }
            | Kind::Pad { .. },
            _,
        ) => return mismatch(),
        _ => return Ok(false),
    }
    Ok(true)
}

fn check_names(def: &StructDef, rec: &Record, path: &str) -> Result<(), DefError> {
    if rec.fields.len() != def.fields.len()
        || rec
            .fields
            .iter()
            .zip(&def.fields)
            .any(|(r, d)| r.0 != d.name)
    {
        return fail(path, format!("the record's fields are not {}'s", def.name));
    }
    Ok(())
}

fn write_fields(
    def: &StructDef,
    rec: &Record,
    n: &mut ONode,
    at: u32,
    path: &str,
) -> Result<(), DefError> {
    check_names(def, rec, path)?;
    for (f, (_, v)) in def.fields.iter().zip(&rec.fields) {
        let p = join(path, &f.name);
        let a = (at + f.offset) as usize;
        if write_plain(&f.kind, v, &mut n.bytes, a, &p)? {
            continue;
        }
        let o = a as u32;
        match (&f.kind, v) {
            (Kind::StringId, Val::String(s)) => match s {
                Str::Null => put_u32(&mut n.bytes, a, 0),
                Str::Text(t) => n.fields.push((o, OField::Str(t.clone()))),
                Str::Index(i) => put_u32(&mut n.bytes, a, *i),
            },
            (Kind::TagRef { .. }, Val::Ref { group, target }) => {
                put_u32(&mut n.bytes, a, *group);
                write_target(n, a + 4, target, &p)?;
            }
            (Kind::Datum { .. }, Val::Datum(target)) => write_target(n, a, target, &p)?,
            (Kind::Raw, Val::Raw(r)) => match r {
                RawVal::Word(w) => put_u32(&mut n.bytes, a, *w),
                RawVal::Res(r) => n.fields.push((o, OField::Raw(r.clone()))),
                RawVal::Built(_) => return fail(&p, "a builder raw in a loose tag"),
            },
            (Kind::Bsp, Val::Bsp { fixup, bytes }) => {
                n.bytes[a..a + 12].copy_from_slice(bytes);
                if *fixup {
                    n.fields.push((o, OField::Bsp));
                }
            }
            (Kind::Block(child), Val::Block(elems)) => {
                put_u32(&mut n.bytes, a, elems.len() as u32);
                if elems.is_empty() {
                    continue;
                }
                let mut c = ONode {
                    bytes: vec![0; (child.size as usize) * elems.len()],
                    count: elems.len() as u32,
                    size: child.size,
                    align: child.align.max(1),
                    fields: Vec::new(),
                    digest: [0; 32],
                    share: elems[0].share,
                };
                for (i, e) in elems.iter().enumerate() {
                    write_fields(
                        child,
                        e,
                        &mut c,
                        i as u32 * child.size,
                        &format!("{p}[{i}]"),
                    )?;
                }
                n.fields.push((o, OField::Block(Box::new(c))));
            }
            (Kind::Data { align }, Val::Data(d)) => {
                put_u32(&mut n.bytes, a, d.len() as u32);
                if d.is_empty() {
                    continue;
                }
                n.fields.push((
                    o,
                    OField::Data(Box::new(ONode {
                        bytes: d.clone(),
                        count: d.len() as u32,
                        size: 1,
                        align: (*align).max(1),
                        fields: Vec::new(),
                        digest: [0; 32],
                        share: None,
                    })),
                ));
            }
            (Kind::Block(_) | Kind::Data { .. }, Val::Unresolved { count, address }) => {
                put_u32(&mut n.bytes, a, *count);
                put_u32(&mut n.bytes, a + 4, *address);
            }
            (Kind::Struct(s), Val::Struct(r)) => write_fields(s, r, n, o, &p)?,
            (k, v) => return fail(&p, format!("{} for a {k:?}", short(v))),
        }
    }
    Ok(())
}

fn write_target(n: &mut ONode, a: usize, t: &Target, path: &str) -> Result<(), DefError> {
    match t {
        Target::Null => put_u32(&mut n.bytes, a, u32::MAX),
        Target::Key(k) => n.fields.push((a as u32, OField::Ref(k.clone()))),
        Target::Id(_) => return fail(path, "a datum in a loose tag (name the tag by key)"),
    }
    Ok(())
}

fn digest(n: &mut ONode) {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    h.update(n.count.to_le_bytes());
    h.update(n.size.to_le_bytes());
    h.update(n.align.to_le_bytes());
    h.update((n.bytes.len() as u64).to_le_bytes());
    h.update(&n.bytes);
    n.fields.sort_by_key(|f| f.0);
    for (o, f) in &mut n.fields {
        h.update(o.to_le_bytes());
        match f {
            OField::Block(c) | OField::Data(c) => {
                digest(c);
                h.update([if matches!(f, OField::Block(_)) { 0 } else { 1 }]);
                let c = match f {
                    OField::Block(c) | OField::Data(c) => c,
                    _ => unreachable!(),
                };
                h.update(c.digest);
            }
            OField::Ref(k) => {
                h.update([2]);
                h.update(k.group.0);
                h.update((k.path.len() as u64).to_le_bytes());
                h.update(k.path.as_bytes());
            }
            OField::Str(s) => {
                h.update([3]);
                h.update((s.len() as u64).to_le_bytes());
                h.update(s.as_bytes());
            }
            OField::Raw(r) => {
                h.update([4]);
                h.update(r.hash);
                h.update(r.len.to_le_bytes());
                match r.origin {
                    crate::repo::Origin::Local => h.update([0]),
                    crate::repo::Origin::External { file, offset } => {
                        h.update([1, file as u8]);
                        h.update(offset.to_le_bytes());
                    }
                }
            }
            OField::Bsp => h.update([5]),
        }
    }
    n.digest = h.finalize().into();
}

/// Numbers nodes depth first in field order (as extraction does), one
/// node per distinct content.
fn flatten(
    n: &ONode,
    tree: &mut Tree,
    seen: &mut HashMap<([u8; 32], Option<u32>), u32>,
    raws: &mut HashMap<RawRes, u32>,
) -> u32 {
    // Retail sharing where the record knows it (same source node and
    // content), content sharing otherwise.
    let key = (n.digest, n.share);
    if let Some(&i) = seen.get(&key) {
        return i;
    }
    let index = tree.nodes.len() as u32;
    seen.insert(key, index);
    tree.nodes.push(Node {
        bytes: n.bytes.clone(),
        count: n.count,
        size: n.size,
        align: n.align,
        fields: Vec::new(),
    });
    let mut fields = Vec::with_capacity(n.fields.len());
    for (o, f) in &n.fields {
        let v = match f {
            OField::Block(c) => Value::Block(flatten(c, tree, seen, raws)),
            OField::Data(c) => Value::Data(flatten(c, tree, seen, raws)),
            OField::Ref(k) => Value::Ref(k.clone()),
            OField::Str(s) => Value::String(s.clone()),
            OField::Raw(r) => {
                let next = tree.raws.len() as u32;
                let i = *raws.entry(r.clone()).or_insert_with(|| {
                    tree.raws.push(r.clone());
                    next
                });
                Value::Raw(i)
            }
            OField::Bsp => Value::Bsp,
        };
        fields.push((*o, v));
    }
    tree.nodes[index as usize].fields = fields;
    index
}

/// A loose tag with every array written out where it is reached (no
/// sharing), as one byte string: what "byte-identical" compares. Elements
/// in visit order, then each field's value. Array alignment is left out:
/// it is placement, not bytes (the schema keeps the weakest alignment seen
/// per group, a definition one per element type).
pub fn canonical(tag: &LooseTag) -> Vec<u8> {
    let mut out = Vec::new();
    match &tag.body {
        Body::Alias { owner, offset } => {
            out.extend_from_slice(b"alias");
            out.extend_from_slice(owner.to_string().as_bytes());
            out.extend_from_slice(&offset.to_le_bytes());
        }
        Body::Tree(t) => {
            out.push(t.region as u8);
            if !t.nodes.is_empty() {
                canon_node(t, 0, &mut out);
            }
        }
    }
    out
}

fn canon_node(t: &Tree, i: usize, out: &mut Vec<u8>) {
    let n = &t.nodes[i];
    for v in [n.count, n.size, n.bytes.len() as u32, n.fields.len() as u32] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&n.bytes);
    for (o, v) in &n.fields {
        out.extend_from_slice(&o.to_le_bytes());
        match v {
            Value::Block(c) | Value::Data(c) => {
                out.push(if matches!(v, Value::Block(_)) { 0 } else { 1 });
                canon_node(t, *c as usize, out);
            }
            Value::Ref(k) => {
                out.push(2);
                out.extend_from_slice(k.to_string().as_bytes());
                out.push(0);
            }
            Value::String(s) => {
                out.push(3);
                out.extend_from_slice(s.as_bytes());
                out.push(0);
            }
            Value::Raw(r) => {
                out.push(4);
                let r = &t.raws[*r as usize];
                out.extend_from_slice(&r.hash);
                out.extend_from_slice(&r.len.to_le_bytes());
                out.extend_from_slice(format!("{:?}", r.origin).as_bytes());
            }
            Value::Bsp => out.push(5),
        }
    }
}

// ------------------------------------------------------------------ default

/// A fresh element: every field at its definition's default (zero, null,
/// empty, or the file's `default=`).
pub fn default_record(def: &StructDef) -> Record {
    Record::new(
        def.fields
            .iter()
            .map(|f| (f.name.clone(), default_val(f)))
            .collect(),
    )
}

fn default_val(f: &FieldDef) -> Val {
    let d = f.default.as_deref();
    let num = |s: &str| -> Option<i64> {
        match s.strip_prefix("-0x") {
            Some(h) => i64::from_str_radix(h, 16).ok().map(|v| -v),
            None => match s.strip_prefix("0x") {
                Some(h) => i64::from_str_radix(h, 16).ok(),
                None => s.parse().ok(),
            },
        }
    };
    match &f.kind {
        Kind::Int { count, .. } => {
            let v = d.and_then(num).unwrap_or(0);
            if *count == 1 {
                Val::Int(v)
            } else {
                Val::List(vec![Val::Int(v); *count as usize])
            }
        }
        Kind::Float { count } => {
            let vals: Vec<f32> = d
                .map(|s| s.split(',').filter_map(|x| x.parse().ok()).collect())
                .unwrap_or_default();
            let at = |k: usize| vals.get(k).or(vals.first()).copied().unwrap_or(0.0);
            if *count == 1 {
                Val::Float(at(0))
            } else {
                Val::List((0..*count as usize).map(|k| Val::Float(at(k))).collect())
            }
        }
        Kind::Enum { values, .. } => Val::Enum(
            d.and_then(|s| {
                values
                    .iter()
                    .position(|v| v == s)
                    .map(|p| p as u32)
                    .or_else(|| num(s).map(|v| v as u32))
            })
            .unwrap_or(0),
        ),
        Kind::Flags { bits, .. } => Val::Flags(
            d.map(|s| {
                s.split('|').fold(0u32, |acc, b| {
                    acc | bits
                        .iter()
                        .position(|x| x == b)
                        .map(|p| 1 << p)
                        .or_else(|| num(b).map(|v| v as u32))
                        .unwrap_or(0)
                })
            })
            .unwrap_or(0),
        ),
        Kind::StringId => Val::String(Str::Null),
        Kind::TagRef { .. } => Val::Ref {
            group: u32::MAX,
            target: Target::Null,
        },
        Kind::Datum { .. } => Val::Datum(Target::Null),
        Kind::Raw => Val::Raw(RawVal::Word(u32::MAX)),
        Kind::RawSize { .. } => Val::Int(0),
        Kind::Word => Val::Word(d.and_then(num).unwrap_or(0) as u32),
        Kind::Bsp => Val::Bsp {
            fixup: false,
            bytes: [0; 12],
        },
        Kind::Block(_) => Val::Block(Vec::new()),
        Kind::Data { .. } => Val::Data(Vec::new()),
        Kind::Struct(s) => Val::Struct(default_record(s)),
        Kind::Unknown { len, .. } | Kind::Pad { len } => {
            let mut b = vec![0u8; *len as usize];
            if let Some(h) = d.filter(|h| h.len() == 2 * b.len()) {
                for (k, x) in b.iter_mut().enumerate() {
                    *x = u8::from_str_radix(&h[2 * k..2 * k + 2], 16).unwrap_or(0);
                }
            }
            Val::Bytes(b)
        }
    }
}

// ------------------------------------------------------------------ builder trees

/// Writes a record as a builder tree. Tag references by key are resolved
/// with `resolve` (unresolved keys are an error); loose raw resources are
/// not supported (convert them to [`RawVal::Built`] first).
pub fn to_built(
    def: &StructDef,
    rec: &Record,
    resolve: &mut dyn FnMut(&TagKey) -> Option<TagId>,
) -> Result<Block, DefError> {
    let mut b = Block::zeroed(def.size as usize).aligned(def.align.max(1));
    built_fields(def, rec, &mut b, 0, "", resolve)?;
    Ok(b)
}

fn built_fields(
    def: &StructDef,
    rec: &Record,
    b: &mut Block,
    at: u32,
    path: &str,
    resolve: &mut dyn FnMut(&TagKey) -> Option<TagId>,
) -> Result<(), DefError> {
    check_names(def, rec, path)?;
    for (f, (_, v)) in def.fields.iter().zip(&rec.fields) {
        let p = join(path, &f.name);
        let a = (at + f.offset) as usize;
        if write_plain(&f.kind, v, &mut b.bytes, a, &p)? {
            continue;
        }
        let o = a as u32;
        let mut id_of = |t: &Target, p: &str| -> Result<Option<TagId>, DefError> {
            match t {
                Target::Null => Ok(None),
                Target::Id(id) => Ok(Some(*id)),
                Target::Key(k) => resolve(k).map(Some).ok_or_else(|| DefError {
                    path: p.into(),
                    why: format!("{k} is not in the set"),
                }),
            }
        };
        match (&f.kind, v) {
            (Kind::StringId, Val::String(s)) => match s {
                Str::Null => put_u32(&mut b.bytes, a, 0),
                Str::Text(t) => {
                    b.string_id(o, t);
                }
                Str::Index(i) => put_u32(&mut b.bytes, a, *i),
            },
            (Kind::TagRef { .. }, Val::Ref { group, target }) => {
                put_u32(&mut b.bytes, a, *group);
                match id_of(target, &p)? {
                    // Null keeps its group word: no fixup.
                    None => put_u32(&mut b.bytes, a + 4, u32::MAX),
                    Some(id) => {
                        b.tag_ref(o, id);
                    }
                }
            }
            (Kind::Datum { .. }, Val::Datum(target)) => match id_of(target, &p)? {
                None => put_u32(&mut b.bytes, a, u32::MAX),
                Some(id) => {
                    b.tag_id(o, id);
                }
            },
            (Kind::Raw, Val::Raw(r)) => match r {
                RawVal::Word(w) => put_u32(&mut b.bytes, a, *w),
                RawVal::Built(r) => {
                    b.raw(o, *r);
                }
                RawVal::Res(_) => return fail(&p, "a loose raw in a builder tree"),
            },
            (Kind::Bsp, Val::Bsp { bytes, .. }) => b.bytes[a..a + 12].copy_from_slice(bytes),
            (Kind::Block(child), Val::Block(elems)) => {
                if elems.is_empty() {
                    put_u32(&mut b.bytes, a, 0);
                    put_u32(&mut b.bytes, a + 4, 0);
                    continue;
                }
                let mut c =
                    Block::zeroed(child.size as usize * elems.len()).aligned(child.align.max(1));
                for (i, e) in elems.iter().enumerate() {
                    built_fields(
                        child,
                        e,
                        &mut c,
                        i as u32 * child.size,
                        &format!("{p}[{i}]"),
                        resolve,
                    )?;
                }
                b.block(o, elems.len() as u32, c);
            }
            (Kind::Data { align }, Val::Data(d)) => {
                if d.is_empty() {
                    put_u32(&mut b.bytes, a, 0);
                    put_u32(&mut b.bytes, a + 4, 0);
                    continue;
                }
                b.data(o, Block::new(d.clone()).aligned((*align).max(1)));
            }
            (Kind::Block(_) | Kind::Data { .. }, Val::Unresolved { count, address }) => {
                put_u32(&mut b.bytes, a, *count);
                put_u32(&mut b.bytes, a + 4, *address);
            }
            (Kind::Struct(s), Val::Struct(r)) => built_fields(s, r, b, o, &p, resolve)?,
            (k, v) => return fail(&p, format!("{} for a {k:?}", short(v))),
        }
    }
    Ok(())
}

/// Reads a builder tree through a definition. Fields with no fixup keep
/// their bytes: a pinned map's block pointers become
/// [`Val::Unresolved`], its datums [`Target::Id`] and its string IDs
/// [`Str::Index`].
pub fn from_built(def: &StructDef, b: &Block) -> Result<Record, DefError> {
    if b.bytes.len() < def.size as usize {
        return fail(
            "",
            format!("{:#x} bytes, the definition {:#x}", b.bytes.len(), def.size),
        );
    }
    built_read(def, b, 0, "")
}

fn built_read(def: &StructDef, b: &Block, at: u32, path: &str) -> Result<Record, DefError> {
    let mut out = Vec::with_capacity(def.fields.len());
    for f in &def.fields {
        let p = join(path, &f.name);
        let o = at + f.offset;
        let a = o as usize;
        let fx = b.fixups.iter().find(|x| x.offset == o).map(|x| &x.kind);
        let v = if let Some(v) = plain(&f.kind, &b.bytes, a) {
            if let Some(k) = fx {
                return fail(&p, format!("a {k:?} fixup on a plain field"));
            }
            v
        } else {
            match (&f.kind, fx) {
                (Kind::StringId, Some(FixupKind::StringId(s))) => Val::text(s),
                (Kind::StringId, None) => match u32_at(&b.bytes, a) {
                    0 => Val::String(Str::Null),
                    w => Val::String(Str::Index(w)),
                },
                (Kind::TagRef { .. }, Some(FixupKind::TagRef(id))) => Val::Ref {
                    group: u32_at(&b.bytes, a),
                    target: if *id == TagId::NULL {
                        Target::Null
                    } else {
                        Target::Id(*id)
                    },
                },
                (Kind::TagRef { .. }, None) => Val::Ref {
                    group: u32_at(&b.bytes, a),
                    target: match u32_at(&b.bytes, a + 4) {
                        u32::MAX => Target::Null,
                        w => Target::Id(TagId(w)),
                    },
                },
                (Kind::Datum { .. }, Some(FixupKind::TagId(id))) => {
                    Val::Datum(if *id == TagId::NULL {
                        Target::Null
                    } else {
                        Target::Id(*id)
                    })
                }
                (Kind::Datum { .. }, None) => Val::Datum(match u32_at(&b.bytes, a) {
                    u32::MAX => Target::Null,
                    w => Target::Id(TagId(w)),
                }),
                (Kind::Raw, Some(FixupKind::Raw(r))) => Val::Raw(RawVal::Built(*r)),
                (Kind::Raw, None) => Val::Raw(RawVal::Word(u32_at(&b.bytes, a))),
                (Kind::Bsp, _) => Val::Bsp {
                    fixup: fx.is_some(),
                    bytes: b.bytes[a..a + 12].try_into().expect("12"),
                },
                (Kind::Block(child), Some(FixupKind::Block { count, child: c })) => {
                    let mut elems = Vec::with_capacity(*count as usize);
                    for i in 0..*count {
                        elems.push(built_read(child, c, i * child.size, &format!("{p}[{i}]"))?);
                    }
                    Val::Block(elems)
                }
                (Kind::Data { .. }, Some(FixupKind::Data(c))) => Val::Data(c.bytes.clone()),
                (Kind::Block(_) | Kind::Data { .. }, None) => {
                    match (u32_at(&b.bytes, a), u32_at(&b.bytes, a + 4)) {
                        (0, _) => {
                            if matches!(f.kind, Kind::Block(_)) {
                                Val::Block(Vec::new())
                            } else {
                                Val::Data(Vec::new())
                            }
                        }
                        (count, address) => Val::Unresolved { count, address },
                    }
                }
                (Kind::Struct(s), _) => Val::Struct(built_read(s, b, o, &p)?),
                (_, k) => return fail(&p, format!("unexpected fixup {k:?}")),
            }
        };
        out.push((f.name.clone(), v));
    }
    Ok(Record::new(out))
}

// ------------------------------------------------------------------ diff

/// One field that differs between two records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// Field path.
    pub path: String,
    /// Whether the field is an `unknown` span or padding.
    pub unknown: bool,
    /// The first record's value, readably.
    pub a: String,
    /// The second's.
    pub b: String,
}

/// Every field of `a` and `b` (both of `def`) that differs, depth first.
/// Floats compare by bits. Blocks of different lengths report their
/// lengths, then compare the elements both have.
pub fn diff(def: &StructDef, a: &Record, b: &Record) -> Vec<Change> {
    let mut out = Vec::new();
    diff_into(def, a, b, "", &mut out);
    out
}

fn diff_into(def: &StructDef, a: &Record, b: &Record, path: &str, out: &mut Vec<Change>) {
    for f in &def.fields {
        let p = join(path, &f.name);
        let (Some(x), Some(y)) = (a.get(&f.name), b.get(&f.name)) else {
            out.push(Change {
                path: p,
                unknown: false,
                a: a.get(&f.name).map_or("missing".into(), show),
                b: b.get(&f.name).map_or("missing".into(), show),
            });
            continue;
        };
        match (&f.kind, x, y) {
            (Kind::Struct(s), Val::Struct(x), Val::Struct(y)) => diff_into(s, x, y, &p, out),
            (Kind::Block(s), Val::Block(x), Val::Block(y)) => {
                if x.len() != y.len() {
                    out.push(Change {
                        path: format!("{p}.count"),
                        unknown: false,
                        a: x.len().to_string(),
                        b: y.len().to_string(),
                    });
                }
                for (i, (ex, ey)) in x.iter().zip(y).enumerate() {
                    diff_into(s, ex, ey, &format!("{p}[{i}]"), out);
                }
            }
            _ => {
                if !same(x, y) {
                    out.push(Change {
                        path: p,
                        unknown: matches!(f.kind, Kind::Unknown { .. } | Kind::Pad { .. }),
                        a: show(x),
                        b: show(y),
                    });
                }
            }
        }
    }
}

/// Equal, floats by bits.
pub fn same(a: &Val, b: &Val) -> bool {
    match (a, b) {
        (Val::Float(x), Val::Float(y)) => x.to_bits() == y.to_bits(),
        (Val::List(x), Val::List(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| same(a, b))
        }
        _ => a == b,
    }
}

fn short(v: &Val) -> &'static str {
    match v {
        Val::Int(_) => "an integer",
        Val::Float(_) => "a float",
        Val::List(_) => "a list",
        Val::Enum(_) => "an enum",
        Val::Flags(_) => "flags",
        Val::String(_) => "a string ID",
        Val::Ref { .. } => "a tag reference",
        Val::Datum(_) => "a datum",
        Val::Raw(_) => "a raw pointer",
        Val::Word(_) => "a word",
        Val::Bsp { .. } => "a BSP descriptor",
        Val::Block(_) => "a block",
        Val::Unresolved { .. } => "an unresolved pointer",
        Val::Data(_) => "data",
        Val::Struct(_) => "a struct",
        Val::Bytes(_) => "bytes",
    }
}

/// A value, readably and briefly (blocks and data by length).
pub fn show(v: &Val) -> String {
    match v {
        Val::Int(x) => x.to_string(),
        Val::Float(x) => format!("{x}"),
        Val::List(l) => format!("[{}]", l.iter().map(show).collect::<Vec<_>>().join(", ")),
        Val::Enum(x) => x.to_string(),
        Val::Flags(x) => format!("{x:#x}"),
        Val::String(Str::Null) => "none".into(),
        Val::String(Str::Text(t)) => format!("\"{t}\""),
        Val::String(Str::Index(i)) => format!("string #{:#x}", i & 0xff_ffff),
        Val::Ref { group, target } => {
            let g = super::fourcc_text(FourCC::from_u32(*group));
            match target {
                Target::Null if *group == u32::MAX => "null".into(),
                Target::Null => format!("{g} null"),
                Target::Key(k) => k.to_string(),
                Target::Id(id) => format!("{g} {:#010x}", id.0),
            }
        }
        Val::Datum(t) => match t {
            Target::Null => "null".into(),
            Target::Key(k) => k.to_string(),
            Target::Id(id) => format!("{:#010x}", id.0),
        },
        Val::Raw(RawVal::Word(w)) => format!("{w:#x}"),
        Val::Raw(RawVal::Res(r)) => format!("raw {} bytes {}", r.len, &r.hex()[..12]),
        Val::Raw(RawVal::Built(r)) => format!("{r:?}"),
        Val::Word(w) => format!("{w:#x}"),
        Val::Bsp { fixup, .. } => format!("bsp (filled: {fixup})"),
        Val::Block(e) => format!("{} elements", e.len()),
        Val::Unresolved { count, address } => format!("{count} at {address:#x} (pinned)"),
        Val::Data(d) => format!("{} bytes", d.len()),
        Val::Struct(_) => "struct".into(),
        Val::Bytes(b) => {
            if b.iter().all(|x| *x == 0) {
                "zero".into()
            } else {
                b.iter().map(|x| format!("{x:02x}")).collect()
            }
        }
    }
}

// ------------------------------------------------------------------ coverage

/// Bytes of a decoded tag by what its definition says they are, every
/// element counted.
pub fn coverage(def: &StructDef, rec: &Record) -> Coverage {
    let mut c = def.coverage();
    for (f, (_, v)) in def.fields.iter().zip(&rec.fields) {
        match (&f.kind, v) {
            (Kind::Block(s), Val::Block(e)) => {
                for r in e {
                    c.add(&coverage(s, r));
                }
            }
            (Kind::Data { .. }, Val::Data(d)) => c.data += d.len() as u64,
            (Kind::Struct(s), Val::Struct(r)) => {
                // The struct's own bytes are in `def.coverage()`; add only
                // what its blocks reach.
                let mut inner = coverage(s, r);
                let own = s.coverage();
                inner.named -= own.named;
                inner.typed -= own.typed;
                inner.pad -= own.pad;
                inner.unknown -= own.unknown;
                c.add(&inner);
            }
            _ => {}
        }
    }
    c
}

/// Every unknown span's bytes in a decoded tag, by definition path
/// (`group/field/field[]/...`), for value statistics.
pub fn unknowns<'r>(
    def: &StructDef,
    rec: &'r Record,
    path: &str,
    out: &mut Vec<(String, &'r [u8])>,
) {
    for (f, (_, v)) in def.fields.iter().zip(&rec.fields) {
        let p = format!("{path}/{}", f.name);
        match (&f.kind, v) {
            (Kind::Unknown { .. } | Kind::Pad { .. }, Val::Bytes(b)) => out.push((p, b)),
            (Kind::Block(s), Val::Block(e)) => {
                for r in e {
                    unknowns(s, r, &format!("{p}[]"), out);
                }
            }
            (Kind::Struct(s), Val::Struct(r)) => unknowns(s, r, &p, out),
            _ => {}
        }
    }
}
