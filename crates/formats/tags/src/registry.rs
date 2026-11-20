//! The field registry: every byte of every group's root and block elements
//! as a labelled, typed field, for editors (`h2-editor`'s tag editor).
//!
//! Two sources merge here:
//!
//! - [`crate::schema`] gives the structure of every group seen on the
//!   retail disc: element sizes, blocks and data references with their
//!   child layouts, tag references, datums, string IDs and raw pointers.
//!   It has no names.
//! - [`crate::defs`] gives typed, named definitions for the groups the
//!   converters use (integers, floats, enums, flags, embedded structs),
//!   with the evidence for each name.
//!
//! Where a definition describes the bytes, its fields are used (and its
//! names, unless the name is only its type and offset, `string_1c`). Where
//! the schema knows a layout-dependent field the definition does not, the
//! schema's type is used. Every byte nothing describes becomes an
//! unnamed field labelled by its offset (`+0x14`), in words where the
//! alignment allows, so an editor can still show and change it. Unnamed
//! fields are marked ([`Field::named`] false) so names can grow.
//!
//! No name here comes from a community tool or plugin: they come from our
//! own definitions, and those from the XBE's readers and the disc.
use crate::defs::{self, Defs, Kind as DefKind, StructDef};
use crate::schema::{self, Schema};
use map::FourCC;
use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

#[cfg(test)]
mod tests;

/// Every group's root layout.
#[derive(Clone, Debug, Default)]
pub struct Registry {
    pub groups: BTreeMap<FourCC, Arc<Layout>>,
    /// Definition fields dropped because the schema types the same bytes
    /// otherwise (`group+0xoffset: why`). Empty while the definitions
    /// agree with the schema (checked by a test).
    pub conflicts: Vec<String>,
}

/// One element layout: fields tiling `0..size` with no gap or overlap.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    /// The definition's name for the element, else the group or the
    /// block's field name.
    pub name: String,
    pub size: u32,
    pub align: u32,
    pub fields: Vec<Field>,
}

/// One field of a [`Layout`].
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    /// Byte offset in the element.
    pub offset: u32,
    /// The label: our name, or the field's type and offset when unnamed.
    pub name: String,
    /// The embedded structs the field sits in (`unit.obje`), empty at the
    /// element's top level.
    pub section: String,
    /// Whether `name` says what the field is (from a definition), not only
    /// where it is.
    pub named: bool,
    pub kind: Kind,
    /// The definition's note: what it means and the evidence.
    pub meaning: String,
}

/// A field's type, as an editor shows it.
#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// `count` integers of 1, 2 or 4 bytes.
    Int { bytes: u8, signed: bool, count: u32 },
    /// `count` IEEE singles, with how to show them.
    Float { count: u32, hint: Hint },
    /// Enumeration; `values[i]` names value `i`.
    Enum { bytes: u8, values: Vec<String> },
    /// Flag bits; `bits[i]` names bit `i` (`_` or missing: unnamed).
    Flags { bytes: u8, bits: Vec<String> },
    /// 4 bytes: a string ID.
    StringId,
    /// 8 bytes: group, then datum; `groups` the allowed groups (empty: any).
    TagRef { groups: Vec<FourCC> },
    /// 4 bytes: a bare datum of a tag of `groups` (empty: any).
    Datum { groups: Vec<FourCC> },
    /// 8 bytes: count and address of elements of the child layout.
    Block(Arc<Layout>),
    /// 8 bytes: byte length and address of bytes.
    Data,
    /// 4 bytes: a raw-resource pointer.
    Raw,
    /// 4 bytes: the byte length of the raw resource at `of`.
    RawSize { of: u32 },
    /// 4 bytes of packed data that can look like an address.
    Word,
    /// 12 bytes: a scenario BSP descriptor.
    Bsp,
    /// Padding: zero everywhere and read by nothing found.
    Pad { len: u32 },
    /// 1, 2 or 4 bytes nothing describes yet: an editor guesses how to
    /// show them from the value.
    Unknown { len: u32 },
}

/// How to show a float array, from its name and note.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hint {
    Plain,
    /// Radians, shown in degrees.
    Angle,
    /// Red, green, blue in `0..1`.
    Colour,
    /// A lower and an upper bound.
    Bounds,
}

impl Kind {
    /// Bytes the field occupies.
    pub fn width(&self) -> u32 {
        match self {
            Kind::Int { bytes, count, .. } => u32::from(*bytes) * count,
            Kind::Float { count, .. } => 4 * count,
            Kind::Enum { bytes, .. } | Kind::Flags { bytes, .. } => u32::from(*bytes),
            Kind::StringId | Kind::Datum { .. } | Kind::Raw | Kind::RawSize { .. } | Kind::Word => {
                4
            }
            Kind::TagRef { .. } | Kind::Block(_) | Kind::Data => 8,
            Kind::Bsp => 12,
            Kind::Pad { len } | Kind::Unknown { len } => *len,
        }
    }

    /// A short type label (`f32[3]`, `enum16`, `tagref`, ...).
    pub fn label(&self) -> String {
        let n = |c: u32| {
            if c == 1 {
                String::new()
            } else {
                format!("[{c}]")
            }
        };
        match self {
            Kind::Int {
                bytes,
                signed,
                count,
            } => format!(
                "{}{}{}",
                if *signed { "i" } else { "u" },
                bytes * 8,
                n(*count)
            ),
            Kind::Float { count, hint } => match hint {
                Hint::Plain => format!("f32{}", n(*count)),
                Hint::Angle => format!("angle{}", n(*count)),
                Hint::Colour => "colour".into(),
                Hint::Bounds => "bounds".into(),
            },
            Kind::Enum { bytes, .. } => format!("enum{}", bytes * 8),
            Kind::Flags { bytes, .. } => format!("flags{}", bytes * 8),
            Kind::StringId => "string id".into(),
            Kind::TagRef { .. } => "tagref".into(),
            Kind::Datum { .. } => "datum".into(),
            Kind::Block(l) => format!("block {:#x}", l.size),
            Kind::Data => "data".into(),
            Kind::Raw => "raw".into(),
            Kind::RawSize { .. } => "raw size".into(),
            Kind::Word => "word".into(),
            Kind::Bsp => "bsp".into(),
            Kind::Pad { len } => format!("pad {len:#x}"),
            Kind::Unknown { len } => format!("? {len}"),
        }
    }

    /// The field holds an address, a count of a block or a resource size:
    /// changing it by hand would corrupt the map, so editors show it only.
    pub fn is_structural(&self) -> bool {
        matches!(
            self,
            Kind::Block(_) | Kind::Data | Kind::Raw | Kind::RawSize { .. } | Kind::Bsp
        )
    }
}

/// Bytes of the registry by what labels them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Coverage {
    /// Fields with a name from a definition.
    pub named: u64,
    /// Fields whose type is known (a reference, string ID, block) but not
    /// their role.
    pub typed: u64,
    /// Padding.
    pub pad: u64,
    /// Bytes nothing describes.
    pub unknown: u64,
    /// Fields named, counted once per field.
    pub named_fields: u64,
    /// Fields not named, counted once per field.
    pub unnamed_fields: u64,
}

impl Coverage {
    pub fn add(&mut self, o: &Coverage) {
        self.named += o.named;
        self.typed += o.typed;
        self.pad += o.pad;
        self.unknown += o.unknown;
        self.named_fields += o.named_fields;
        self.unnamed_fields += o.unnamed_fields;
    }
    pub fn total(&self) -> u64 {
        self.named + self.typed + self.pad + self.unknown
    }
}

impl Layout {
    /// The field at `offset` exactly.
    pub fn field_at(&self, offset: u32) -> Option<&Field> {
        self.fields
            .binary_search_by_key(&offset, |f| f.offset)
            .ok()
            .map(|i| &self.fields[i])
    }

    /// The field covering byte `offset`.
    pub fn field_covering(&self, offset: u32) -> Option<&Field> {
        let i = self.fields.partition_point(|f| f.offset <= offset);
        let f = self.fields.get(i.checked_sub(1)?)?;
        (offset < f.offset + f.kind.width()).then_some(f)
    }

    /// Bytes by label, this element and every block's element once
    /// (a block counts its 8 bytes and its element's layout).
    pub fn coverage(&self) -> Coverage {
        let mut c = Coverage::default();
        for f in &self.fields {
            let w = u64::from(f.kind.width());
            match &f.kind {
                Kind::Pad { .. } => c.pad += w,
                Kind::Unknown { .. } => c.unknown += w,
                _ if f.named => c.named += w,
                _ => c.typed += w,
            }
            if f.named {
                c.named_fields += 1;
            } else {
                c.unnamed_fields += 1;
            }
            if let Kind::Block(child) = &f.kind {
                c.add(&child.coverage());
            }
        }
        c
    }

    /// A fresh element: zero, with every tag reference, datum and string
    /// ID null (`0xffffffff` datums; string ID 0 is the empty string).
    pub fn blank(&self) -> Vec<u8> {
        let mut out = vec![0u8; self.size as usize];
        for f in &self.fields {
            let at = f.offset as usize;
            match f.kind {
                Kind::TagRef { .. } => out[at..at + 8].fill(0xff),
                Kind::Datum { .. } => out[at..at + 4].fill(0xff),
                _ => {}
            }
        }
        out
    }
}

impl Registry {
    /// The built-in schema merged with the built-in definitions.
    pub fn builtin() -> &'static Registry {
        static R: OnceLock<Registry> = OnceLock::new();
        R.get_or_init(|| Registry::build(Schema::builtin(), Defs::builtin()))
    }

    /// Merge `schema` with `defs`. Every group of either gets a layout.
    pub fn build(schema: &Schema, defs: &Defs) -> Registry {
        let mut r = Registry::default();
        let mut groups: Vec<FourCC> = schema.groups.keys().copied().collect();
        groups.extend(defs.groups.keys().copied());
        groups.sort();
        groups.dedup();
        for g in groups {
            let name = defs::fourcc_text(g);
            let layout = merge(
                &name,
                defs.groups.get(&g),
                schema.groups.get(&g),
                &mut r.conflicts,
            );
            r.groups.insert(g, Arc::new(layout));
        }
        r
    }

    /// The root layout of `group`.
    pub fn group(&self, group: FourCC) -> Option<&Arc<Layout>> {
        self.groups.get(&group)
    }

    /// Every group's coverage, summed.
    pub fn coverage(&self) -> Coverage {
        let mut c = Coverage::default();
        for l in self.groups.values() {
            c.add(&l.coverage());
        }
        c
    }
}

/// One definition field with its offset from the element start and the
/// embedded structs it sits in.
struct Leaf<'a> {
    at: u32,
    section: String,
    def: &'a defs::FieldDef,
}

fn flatten<'a>(s: &'a StructDef, base: u32, section: &str, out: &mut Vec<Leaf<'a>>) {
    for f in &s.fields {
        if let DefKind::Struct(sub) = &f.kind {
            let sec = if section.is_empty() {
                f.name.clone()
            } else {
                format!("{section}.{}", f.name)
            };
            flatten(sub, base + f.offset, &sec, out);
        } else {
            out.push(Leaf {
                at: base + f.offset,
                section: section.to_owned(),
                def: f,
            });
        }
    }
}

/// Whether a definition field of `kind` can stand for the schema's `s`.
fn compatible(kind: &DefKind, s: &schema::Field) -> bool {
    use schema::Field as S;
    matches!(
        (kind, s),
        (DefKind::Block(_) | DefKind::Data { .. }, S::Block(_))
            | (DefKind::TagRef { .. }, S::TagRef)
            | (DefKind::Datum { .. }, S::TagId)
            | (DefKind::StringId, S::StringId)
            | (DefKind::Raw, S::Raw)
            | (DefKind::RawSize { .. }, S::RawSize(_))
            | (DefKind::Word, S::Word)
            | (DefKind::Bsp, S::Bsp)
    )
}

/// A schema block of 1-byte elements with no fields is a data reference.
fn is_data(s: &schema::Struct) -> bool {
    s.size == 1 && s.fields.is_empty()
}

fn hint(name: &str, meaning: &str, count: u32) -> Hint {
    let n = name.to_ascii_lowercase();
    let m = meaning.to_ascii_lowercase();
    if count == 3 && (n.contains("colour") || n.contains("color")) {
        Hint::Colour
    } else if n.contains("angle")
        || n.contains("field_of_view")
        || n.ends_with("_yaw")
        || n.ends_with("_pitch")
        || m.contains("radians")
    {
        Hint::Angle
    } else if count == 2 && (n.contains("bounds") || n.ends_with("_range")) {
        Hint::Bounds
    } else {
        Hint::Plain
    }
}

/// The element layout from a definition (when its size matches) and the
/// schema's structure.
fn merge(
    name: &str,
    def: Option<&StructDef>,
    schema: Option<&schema::Struct>,
    conflicts: &mut Vec<String>,
) -> Layout {
    let def = def.filter(|d| schema.is_none_or(|s| s.size == d.size));
    let size = schema.map(|s| s.size).or(def.map(|d| d.size)).unwrap_or(0);
    let align = schema
        .map(|s| s.align)
        .or(def.map(|d| d.align))
        .unwrap_or(4);
    let empty = BTreeMap::new();
    let sfields = schema.map_or(&empty, |s| &s.fields);
    let mut fields: Vec<Field> = Vec::new();

    if let Some(d) = def {
        let mut leaves = Vec::new();
        flatten(d, 0, "", &mut leaves);
        for leaf in leaves {
            let f = leaf.def;
            let (lo, hi) = (leaf.at, leaf.at + f.kind.width());
            if matches!(f.kind, DefKind::Unknown { .. }) {
                continue;
            }
            // A schema field inside these bytes must be this field.
            let clash = sfields
                .range(lo..hi)
                .next()
                .filter(|(o, s)| **o != lo || !compatible(&f.kind, s));
            // ... and this field, if layout-dependent, must be the schema's
            // (or a field the schema never saw set: definition-only).
            if let Some((o, s)) = clash {
                conflicts.push(format!(
                    "{name}+{lo:#x} {}: the schema has {s:?} at {o:#x}",
                    f.name
                ));
                continue;
            }
            let named = !f.typed_only() && !f.name.starts_with("unknown");
            let kind = match &f.kind {
                DefKind::Int {
                    bytes,
                    signed,
                    count,
                } => Kind::Int {
                    bytes: *bytes,
                    signed: *signed,
                    count: *count,
                },
                DefKind::Float { count } => Kind::Float {
                    count: *count,
                    hint: hint(&f.name, &f.meaning, *count),
                },
                DefKind::Enum { bytes, values } => Kind::Enum {
                    bytes: *bytes,
                    values: values.clone(),
                },
                DefKind::Flags { bytes, bits } => Kind::Flags {
                    bytes: *bytes,
                    bits: bits.clone(),
                },
                DefKind::StringId => Kind::StringId,
                DefKind::TagRef { groups } => Kind::TagRef {
                    groups: groups.clone(),
                },
                DefKind::Datum { groups } => Kind::Datum {
                    groups: groups.clone(),
                },
                DefKind::Raw => Kind::Raw,
                DefKind::RawSize { of } => Kind::RawSize { of: *of },
                DefKind::Word => Kind::Word,
                DefKind::Bsp => Kind::Bsp,
                DefKind::Data { .. } => Kind::Data,
                DefKind::Block(sub) => {
                    let child = match sfields.get(&lo) {
                        Some(schema::Field::Block(c)) => Some(&**c),
                        _ => None,
                    };
                    Kind::Block(Arc::new(merge(
                        &format!("{name}.{}", f.name),
                        Some(sub),
                        child,
                        conflicts,
                    )))
                }
                DefKind::Pad { len } => Kind::Pad { len: *len },
                DefKind::Struct(_) | DefKind::Unknown { .. } => unreachable!("flattened"),
            };
            let label = if named {
                f.name.clone()
            } else {
                unnamed_label(&kind, lo)
            };
            fields.push(Field {
                offset: lo,
                name: label,
                section: leaf.section,
                named,
                kind,
                meaning: f.meaning.clone(),
            });
        }
    }

    // Schema fields no definition field stands for.
    fields.sort_by_key(|f| f.offset);
    for (o, s) in sfields {
        let covered = covering(&fields, *o).is_some();
        if covered {
            continue;
        }
        let kind = match s {
            schema::Field::Block(c) if is_data(c) => Kind::Data,
            schema::Field::Block(c) => Kind::Block(Arc::new(merge(
                &format!("{name}+{o:#x}"),
                None,
                Some(c),
                conflicts,
            ))),
            schema::Field::TagRef => Kind::TagRef { groups: Vec::new() },
            schema::Field::TagId => Kind::Datum { groups: Vec::new() },
            schema::Field::StringId => Kind::StringId,
            schema::Field::Raw => Kind::Raw,
            schema::Field::Bsp => Kind::Bsp,
            schema::Field::Word => Kind::Word,
            schema::Field::RawSize(of) => Kind::RawSize { of: *of },
        };
        let at = fields.partition_point(|f| f.offset < *o);
        fields.insert(
            at,
            Field {
                offset: *o,
                name: unnamed_label(&kind, *o),
                section: String::new(),
                named: false,
                kind,
                meaning: String::new(),
            },
        );
    }

    // Gaps: unnamed words.
    let mut out = Vec::with_capacity(fields.len());
    let mut at = 0;
    for f in fields {
        if f.offset < at {
            // Overlap (a schema field inside a definition field was
            // refused above, so this is a definition bug): keep the first.
            conflicts.push(format!("{name}+{:#x} {}: overlaps", f.offset, f.name));
            continue;
        }
        fill(&mut out, at, f.offset);
        at = f.offset + f.kind.width();
        out.push(f);
    }
    if at > size {
        conflicts.push(format!("{name}: fields end at {at:#x}, element {size:#x}"));
        out.retain(|f| f.offset + f.kind.width() <= size);
        at = out.last().map_or(0, |f| f.offset + f.kind.width());
    }
    fill(&mut out, at, size);
    Layout {
        name: def.map_or_else(|| name.to_owned(), |d| d.name.clone()),
        size,
        align,
        fields: out,
    }
}

fn covering(fields: &[Field], offset: u32) -> Option<&Field> {
    fields
        .iter()
        .find(|f| f.offset <= offset && offset < f.offset + f.kind.width())
}

/// `[lo, hi)` as unknown pieces: bytes up to 2-alignment, a half word up
/// to 4-alignment, words, then the tail.
fn fill(out: &mut Vec<Field>, mut lo: u32, hi: u32) {
    while lo < hi {
        let len = if lo % 4 == 0 && hi - lo >= 4 {
            4
        } else if lo % 2 == 0 && hi - lo >= 2 {
            2
        } else {
            1
        };
        let kind = Kind::Unknown { len };
        out.push(Field {
            offset: lo,
            name: unnamed_label(&kind, lo),
            section: String::new(),
            named: false,
            kind,
            meaning: String::new(),
        });
        lo += len;
    }
}

/// The label of an unnamed field: its type and offset (`+0x14`,
/// `tagref +0x3c`).
fn unnamed_label(kind: &Kind, offset: u32) -> String {
    match kind {
        Kind::Unknown { .. } => format!("+{offset:#x}"),
        Kind::Block(_) => format!("block +{offset:#x}"),
        Kind::Data => format!("data +{offset:#x}"),
        Kind::TagRef { .. } => format!("tagref +{offset:#x}"),
        Kind::Datum { .. } => format!("datum +{offset:#x}"),
        Kind::StringId => format!("string +{offset:#x}"),
        Kind::Pad { .. } => format!("pad +{offset:#x}"),
        k => format!("{} +{offset:#x}", k.label()),
    }
}
