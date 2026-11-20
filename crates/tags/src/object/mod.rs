//! The object tag family: the shared `obje` header, its `unit`/`item`/`devi`
//! layers, the concrete placed types, `hlmt` object models, and the
//! `itmc`/`vehc` collections.
//!
//! Self-contained (table-driven decoder, no `schema.rs` involvement) so it
//! merges independently of the render/collision/physics/animation decoders.
//! Layouts and their per-field provenance are in [`layouts`]; the evidence is
//! written up in `docs/formats/objects.md`.
//!
//! Blocks in cache metadata may point into another tag's data (shared,
//! aliased blocks), so block spans are checked against the whole ordinary
//! metadata region, never assumed to lie in the owning tag. Every decode is
//! bounded by an element [`Budget`].
use crate::wire::{StringId, TagBlock, TagReference};
use crate::{Error, Result};
use map::{FourCC, Map, TagId};
use std::collections::HashMap;
use std::io::{Read, Seek};

pub mod direct_drive;
pub mod h1_flight;
pub mod layouts;
mod resolve;
pub use resolve::*;

/// How a field's offset, type and (where given) role were established.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Evidence {
    /// Engine code reads it this way (address in the field note).
    Engine,
    /// Structure checked on every disc instance (type/stride/class).
    Disc,
    /// An interpretation or name chosen here.
    Design,
}
impl Evidence {
    pub fn label(self) -> &'static str {
        match self {
            Self::Engine => "E",
            Self::Disc => "V",
            Self::Design => "D",
        }
    }
}

/// Wire type of one reviewed field.
#[derive(Clone, Copy, Debug)]
pub enum Kind {
    U8,
    U16,
    I16,
    U32,
    F32,
    Vec3,
    StringId,
    /// Eight-byte reference; allowed classes (empty: any class).
    TagRef(&'static [&'static str]),
    /// A bare four-byte salted datum.
    TagDatum,
    /// Block with a reviewed stride and element layout.
    Block(u32, &'static Layout),
    /// Block whose element size is not reviewed: count and address only.
    OpaqueBlock,
    Bytes(usize),
}
impl Kind {
    pub fn size(self) -> u32 {
        match self {
            Self::U8 => 1,
            Self::U16 | Self::I16 => 2,
            Self::U32 | Self::F32 | Self::StringId | Self::TagDatum => 4,
            Self::Vec3 => 12,
            Self::TagRef(_) | Self::Block(..) | Self::OpaqueBlock => 8,
            Self::Bytes(n) => n as u32,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Field {
    /// Offset from the start of the record (for root layers, of the tag).
    pub offset: u32,
    pub name: &'static str,
    pub kind: Kind,
    pub evidence: Evidence,
    pub note: &'static str,
}
/// One layer (`obje`, `unit`, `bipd`, ...) or block element. `start..end` is
/// the byte range the layer owns; fields lie inside it.
#[derive(Debug)]
pub struct Layout {
    pub name: &'static str,
    pub start: u32,
    pub end: u32,
    pub fields: &'static [Field],
}

/// Engine object type, the `obje +0` value. Order is the type table at
/// `0x00468630` (E); the placement columns are the descriptor's scenario
/// placement-block offset, palette-block offset and placement record size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectType {
    pub index: u16,
    pub group: &'static str,
    /// Runtime object body size from descriptor `+8` (not the tag size).
    pub body_size: u16,
    /// `scnr` offsets of the placement and palette blocks and the placement
    /// stride, from descriptor `+0xa/+0xc/+0xe`; `None` for unplaceable types.
    pub placement: Option<(u16, u16, u16)>,
}
pub const OBJECT_TYPES: [ObjectType; 13] = [
    ObjectType {
        index: 0,
        group: "bipd",
        body_size: 0x464,
        placement: Some((0x60, 0x68, 0x54)),
    },
    ObjectType {
        index: 1,
        group: "vehi",
        body_size: 0x428,
        placement: Some((0x70, 0x78, 0x54)),
    },
    ObjectType {
        index: 2,
        group: "weap",
        body_size: 0x25c,
        placement: Some((0x90, 0x98, 0x54)),
    },
    ObjectType {
        index: 3,
        group: "eqip",
        body_size: 0x184,
        placement: Some((0x80, 0x88, 0x38)),
    },
    ObjectType {
        index: 4,
        group: "garb",
        body_size: 0x170,
        placement: None,
    },
    ObjectType {
        index: 5,
        group: "proj",
        body_size: 0x1ac,
        placement: None,
    },
    ObjectType {
        index: 6,
        group: "scen",
        body_size: 0x13c,
        placement: Some((0x50, 0x58, 0x5c)),
    },
    ObjectType {
        index: 7,
        group: "mach",
        body_size: 0x1e0,
        placement: Some((0xa8, 0xb0, 0x48)),
    },
    ObjectType {
        index: 8,
        group: "ctrl",
        body_size: 0x1d4,
        placement: Some((0xb8, 0xc0, 0x44)),
    },
    ObjectType {
        index: 9,
        group: "lifi",
        body_size: 0x1e4,
        placement: Some((0xc8, 0xd0, 0x54)),
    },
    ObjectType {
        index: 10,
        group: "ssce",
        body_size: 0x148,
        placement: Some((0xd8, 0xe0, 0x50)),
    },
    ObjectType {
        index: 11,
        group: "bloc",
        body_size: 0x130,
        placement: Some((0x328, 0x330, 0x4c)),
    },
    ObjectType {
        index: 12,
        group: "crea",
        body_size: 0x208,
        placement: Some((0x368, 0x370, 0x34)),
    },
];

/// A decodable group: its layer chain and fixed root size (V: the first child
/// block of every disc instance starts exactly at `root_size`).
#[derive(Debug)]
pub struct GroupDef {
    pub group: &'static str,
    pub root_size: u32,
    pub layers: &'static [&'static Layout],
    /// Engine object type, for placeable object groups.
    pub object_type: Option<u16>,
}
use layouts as l;
pub static GROUPS: [GroupDef; 16] = [
    GroupDef {
        group: "bipd",
        root_size: 0x314,
        layers: &[&l::OBJE, &l::UNIT, &l::BIPD],
        object_type: Some(0),
    },
    GroupDef {
        group: "vehi",
        root_size: 0x300,
        layers: &[&l::OBJE, &l::UNIT, &l::VEHI],
        object_type: Some(1),
    },
    GroupDef {
        group: "weap",
        root_size: 0x31c,
        layers: &[&l::OBJE, &l::ITEM, &l::WEAP],
        object_type: Some(2),
    },
    GroupDef {
        group: "eqip",
        root_size: 0x13c,
        layers: &[&l::OBJE, &l::ITEM, &l::EQIP],
        object_type: Some(3),
    },
    GroupDef {
        group: "garb",
        root_size: 0x1d4,
        layers: &[&l::OBJE, &l::ITEM, &l::GARB],
        object_type: Some(4),
    },
    GroupDef {
        group: "proj",
        root_size: 0x1a4,
        layers: &[&l::OBJE, &l::PROJ],
        object_type: Some(5),
    },
    GroupDef {
        group: "scen",
        root_size: 0xc4,
        layers: &[&l::OBJE, &l::SCEN],
        object_type: Some(6),
    },
    GroupDef {
        group: "mach",
        root_size: 0x134,
        layers: &[&l::OBJE, &l::DEVI, &l::MACH],
        object_type: Some(7),
    },
    GroupDef {
        group: "ctrl",
        root_size: 0x140,
        layers: &[&l::OBJE, &l::DEVI, &l::CTRL],
        object_type: Some(8),
    },
    GroupDef {
        group: "ssce",
        root_size: 0xcc,
        layers: &[&l::OBJE, &l::SSCE],
        object_type: Some(10),
    },
    GroupDef {
        group: "bloc",
        root_size: 0xc0,
        layers: &[&l::OBJE, &l::BLOC],
        object_type: Some(11),
    },
    GroupDef {
        group: "crea",
        root_size: 0x180,
        layers: &[&l::OBJE, &l::CREA],
        object_type: Some(12),
    },
    GroupDef {
        group: "hlmt",
        root_size: 0xfc,
        layers: &[&l::HLMT],
        object_type: None,
    },
    GroupDef {
        group: "itmc",
        root_size: 0xc,
        layers: &[&l::COLLECTION],
        object_type: None,
    },
    GroupDef {
        group: "vehc",
        root_size: 0xc,
        layers: &[&l::COLLECTION],
        object_type: None,
    },
    // No `lifi` tags exist on the retail disc; its layout is not reviewed.
    GroupDef {
        group: "obje",
        root_size: 0xbc,
        layers: &[&l::OBJE],
        object_type: None,
    },
];
impl GroupDef {
    pub fn find(group: FourCC) -> Option<&'static GroupDef> {
        GROUPS
            .iter()
            .find(|g| g.group.as_bytes() == group.0 && g.group != "obje")
    }
    /// Ancestor classes implied by the layer chain, nearest first
    /// (`bipd` -> `unit`, `obje`). Validated against each map's group table.
    pub fn ancestry(&self) -> Vec<&'static str> {
        self.layers.iter().rev().skip(1).map(|l| l.name).collect()
    }
}

/// Upper bounds on decoding work for one tag, counted over every decoded
/// block element including repeats through aliased blocks. Real tags stay far
/// below (largest observed: 72 nodes, ~100 predicted resources per tag).
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    pub elements: u32,
    pub per_block: u32,
    pub depth: u8,
}
impl Default for Budget {
    fn default() -> Self {
        Self {
            elements: 1 << 16,
            per_block: 4096,
            depth: 6,
        }
    }
}

/// Tag index facts needed to validate references and string IDs.
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    pub tags: HashMap<u32, CatalogTag>,
    /// class -> (parent, grandparent), from the map's group table.
    pub groups: HashMap<FourCC, (Option<FourCC>, Option<FourCC>)>,
    pub strings: Vec<Option<String>>,
}
#[derive(Clone, Debug)]
pub struct CatalogTag {
    pub id: TagId,
    pub group: FourCC,
    pub name: Option<String>,
    /// Ordinary-metadata address and size; `None` when the tag has no data.
    pub location: Option<(u32, u32)>,
}
impl Catalog {
    pub fn from_map<R: Read + Seek>(map: &Map<R>) -> Self {
        Self {
            tags: map
                .tags()
                .iter()
                .map(|t| {
                    let location = t.location.map(|l| (l.address, l.size));
                    (
                        t.id.0,
                        CatalogTag {
                            id: t.id,
                            group: t.group,
                            name: t.name.clone(),
                            location,
                        },
                    )
                })
                .collect(),
            groups: map
                .groups()
                .iter()
                .map(|g| (g.class, (g.parent, g.grandparent)))
                .collect(),
            strings: map.strings().to_vec(),
        }
    }
    pub fn get(&self, id: TagId) -> Option<&CatalogTag> {
        self.tags.get(&id.0)
    }
    /// True when `class` is `want` or one of its ancestors is.
    pub fn is_a(&self, class: FourCC, want: FourCC) -> bool {
        class == want
            || self
                .groups
                .get(&class)
                .is_some_and(|(p, g)| *p == Some(want) || *g == Some(want))
    }
}

/// The map's ordinary metadata region, read once. Object tags never live in
/// BSP regions, so this single address space covers every block they use.
#[derive(Clone, Debug)]
pub struct Metadata {
    pub base: u32,
    pub bytes: Vec<u8>,
}
impl Metadata {
    pub fn read<R: Read + Seek>(map: &mut Map<R>) -> Result<Self> {
        let s = map
            .metadata_space()
            .ok_or(Error::Invalid("map has no ordinary metadata"))?;
        let bytes = map.read_file_range(s.file_offset, u64::from(s.size))?;
        Ok(Self {
            base: s.virtual_base,
            bytes,
        })
    }
    pub fn slice(&self, address: u32, size: u64) -> Result<&[u8]> {
        let bad = || Error::Bounds { address, size };
        let start = address.checked_sub(self.base).ok_or_else(bad)? as usize;
        let end = start
            .checked_add(usize::try_from(size).map_err(|_| bad())?)
            .ok_or_else(bad)?;
        self.bytes.get(start..end).ok_or_else(bad)
    }
}

/// Everything a decode needs, independent of any I/O.
#[derive(Clone, Debug)]
pub struct Context {
    pub catalog: Catalog,
    pub meta: Metadata,
}
impl Context {
    pub fn read<R: Read + Seek>(map: &mut Map<R>) -> Result<Self> {
        Ok(Self {
            catalog: Catalog::from_map(map),
            meta: Metadata::read(map)?,
        })
    }
}

/// A decoded field value. Unknown bytes outside fields are kept in
/// [`Record::opaque`].
#[derive(Clone, Debug)]
pub enum Value {
    U8(u8),
    U16(u16),
    I16(i16),
    U32(u32),
    F32(f32),
    Vec3([f32; 3]),
    StringId(StringId, Option<String>),
    TagRef(TagReference),
    TagDatum(TagId),
    /// Reviewed block; `aliased` when its data lies outside the owning tag.
    Block {
        block: TagBlock,
        aliased: bool,
        elements: Vec<Record>,
    },
    OpaqueBlock {
        block: TagBlock,
        aliased: bool,
    },
    Bytes(Vec<u8>),
}
#[derive(Clone, Debug)]
pub struct Record {
    pub layout: &'static str,
    pub address: u32,
    pub fields: Vec<(&'static Field, Value)>,
    /// Uninterpreted spans: (offset, raw bytes).
    pub opaque: Vec<(u32, Vec<u8>)>,
}
impl Record {
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.fields
            .iter()
            .find(|(f, _)| f.name == name)
            .map(|(_, v)| v)
    }
    pub fn sid(&self, name: &str) -> Option<(StringId, Option<&str>)> {
        match self.get(name)? {
            Value::StringId(id, s) => Some((*id, s.as_deref())),
            _ => None,
        }
    }
    pub fn tag_ref(&self, name: &str) -> Option<TagReference> {
        match self.get(name)? {
            Value::TagRef(r) => Some(*r),
            _ => None,
        }
    }
    pub fn elements(&self, name: &str) -> &[Record] {
        match self.get(name) {
            Some(Value::Block { elements, .. }) => elements,
            _ => &[],
        }
    }
    pub fn block(&self, name: &str) -> Option<TagBlock> {
        match self.get(name)? {
            Value::Block { block, .. } | Value::OpaqueBlock { block, .. } => Some(*block),
            _ => None,
        }
    }
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().expect("4 bytes"))
}
fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_bits(u32_at(b, o))
}
fn ctx_err(record: &'static str, field: &'static str, source: Error) -> Error {
    Error::Field {
        record,
        field,
        source: Box::new(source),
    }
}

/// One decode in progress: shared state for the tag being read.
struct Decoder<'a> {
    ctx: &'a Context,
    /// Owning tag's metadata range, to flag aliased blocks.
    own: (u32, u32),
    budget: Budget,
    elements: u32,
}
impl Decoder<'_> {
    fn reference(&self, b: &[u8], allowed: &[&str]) -> Result<TagReference> {
        let group = u32_at(b, 0);
        let id = TagId(u32_at(b, 4));
        if group == u32::MAX {
            if id != TagId::NULL {
                return Err(Error::Invalid("reference without a class has a datum"));
            }
            return Ok(TagReference {
                group: FourCC::from_u32(group),
                id,
            });
        }
        let class = FourCC::from_u32(group);
        // Null references keep whatever class the authoring default had, even
        // classes absent from the cache (V: `antr`, `shdr`); only live ones
        // are held to the field's classes.
        if id != TagId::NULL {
            if !allowed.is_empty() && !allowed.iter().any(|a| a.as_bytes() == class.0) {
                return Err(Error::Invalid("reference class not allowed for this field"));
            }
            let tag = self
                .ctx
                .catalog
                .get(id)
                .ok_or(Error::Invalid("reference datum is not a tag"))?;
            if !self.ctx.catalog.is_a(tag.group, class) {
                return Err(Error::Invalid("referenced tag has another class"));
            }
        }
        Ok(TagReference { group: class, id })
    }
    fn datum(&self, v: u32) -> Result<TagId> {
        let id = TagId(v);
        if id != TagId::NULL && self.ctx.catalog.get(id).is_none() {
            return Err(Error::Invalid("datum is not a tag"));
        }
        Ok(id)
    }
    fn string_id(&self, v: u32) -> Result<(StringId, Option<String>)> {
        let id = StringId(v);
        if v == 0 {
            return Ok((id, None));
        }
        Ok((
            id,
            id.resolve(&self.ctx.catalog.strings)?.map(str::to_owned),
        ))
    }
    fn block_header(&self, b: &[u8], stride: Option<u32>) -> Result<(TagBlock, bool)> {
        let block = TagBlock {
            count: u32_at(b, 0),
            address: u32_at(b, 4),
        };
        if block.count == 0 {
            return Ok((block, false));
        }
        if block.count > self.budget.per_block {
            return Err(Error::Invalid("block count exceeds the per-block budget"));
        }
        if !block.address.is_multiple_of(4) {
            return Err(Error::Invalid("block address is not four-byte aligned"));
        }
        let size = u64::from(block.count) * u64::from(stride.unwrap_or(1));
        self.ctx.meta.slice(block.address, size)?;
        let (s, n) = self.own;
        let end = u64::from(block.address) + size;
        let aliased = block.address < s || end > u64::from(s) + u64::from(n);
        Ok((block, aliased))
    }
    fn record(&mut self, layout: &'static Layout, address: u32, depth: u8) -> Result<Record> {
        if depth > self.budget.depth {
            return Err(Error::Invalid("block nesting exceeds the depth budget"));
        }
        let len = layout.end - layout.start;
        let bytes = self
            .ctx
            .meta
            .slice(address + layout.start, u64::from(len))?;
        let mut fields = Vec::with_capacity(layout.fields.len());
        let mut opaque = Vec::new();
        let mut cursor = layout.start;
        for field in layout.fields {
            let at = (field.offset - layout.start) as usize;
            if field.offset > cursor {
                opaque.push((cursor, bytes[(cursor - layout.start) as usize..at].to_vec()));
            }
            let wire = &bytes[at..at + field.kind.size() as usize];
            let value = self
                .value(field, wire, depth)
                .map_err(|e| ctx_err(layout.name, field.name, e))?;
            fields.push((field, value));
            cursor = field.offset + field.kind.size();
        }
        if cursor < layout.end {
            opaque.push((cursor, bytes[(cursor - layout.start) as usize..].to_vec()));
        }
        Ok(Record {
            layout: layout.name,
            address,
            fields,
            opaque,
        })
    }
    fn value(&mut self, field: &Field, b: &[u8], depth: u8) -> Result<Value> {
        Ok(match field.kind {
            Kind::U8 => Value::U8(b[0]),
            Kind::U16 => Value::U16(u16_at(b, 0)),
            Kind::I16 => Value::I16(u16_at(b, 0) as i16),
            Kind::U32 => Value::U32(u32_at(b, 0)),
            Kind::F32 => Value::F32(f32_at(b, 0)),
            Kind::Vec3 => Value::Vec3([f32_at(b, 0), f32_at(b, 4), f32_at(b, 8)]),
            Kind::StringId => {
                let (id, s) = self.string_id(u32_at(b, 0))?;
                Value::StringId(id, s)
            }
            Kind::TagRef(allowed) => Value::TagRef(self.reference(b, allowed)?),
            Kind::TagDatum => Value::TagDatum(self.datum(u32_at(b, 0))?),
            Kind::Bytes(_) => Value::Bytes(b.to_vec()),
            Kind::OpaqueBlock => {
                let (block, aliased) = self.block_header(b, None)?;
                Value::OpaqueBlock { block, aliased }
            }
            Kind::Block(stride, layout) => {
                debug_assert_eq!(layout.end - layout.start, stride);
                let (block, aliased) = self.block_header(b, Some(stride))?;
                self.elements = self
                    .elements
                    .checked_add(block.count)
                    .filter(|n| *n <= self.budget.elements)
                    .ok_or(Error::Invalid("tag exceeds the element budget"))?;
                let elements = (0..block.count)
                    .map(|i| self.record(layout, block.address + i * stride, depth + 1))
                    .collect::<Result<_>>()?;
                Value::Block {
                    block,
                    aliased,
                    elements,
                }
            }
        })
    }
}

/// A decoded tag of one of the [`GROUPS`]: one record per layer.
#[derive(Clone, Debug)]
pub struct DecodedTag {
    pub id: TagId,
    pub group: FourCC,
    pub name: Option<String>,
    pub def: &'static GroupDef,
    /// Root-level layers in chain order (`obje`, `unit`, `bipd`).
    pub layers: Vec<Record>,
    /// Payload length, which also covers this tag's own blocks.
    pub size: u32,
}
impl DecodedTag {
    pub fn layer(&self, name: &str) -> Option<&Record> {
        self.layers.iter().find(|r| r.layout == name)
    }
}

impl Context {
    /// Decode any tag of a supported group with the default [`Budget`].
    pub fn decode(&self, id: TagId) -> Result<DecodedTag> {
        self.decode_with(id, Budget::default())
    }
    pub fn decode_with(&self, id: TagId, budget: Budget) -> Result<DecodedTag> {
        let tag = self.catalog.get(id).ok_or(Error::Invalid("no such tag"))?;
        let def = GroupDef::find(tag.group).ok_or(Error::UnsupportedGroup(tag.group))?;
        let (address, size) = tag.location.ok_or(map::Error::NoTagData)?;
        if size < def.root_size {
            return Err(Error::RecordSize {
                actual: size as usize,
                expected: def.root_size as usize,
            });
        }
        let mut d = Decoder {
            ctx: self,
            own: (address, size),
            budget,
            elements: 0,
        };
        let layers = def
            .layers
            .iter()
            .map(|l| d.record(l, address, 0))
            .collect::<Result<Vec<_>>>()?;
        let out = DecodedTag {
            id,
            group: tag.group,
            name: tag.name.clone(),
            def,
            layers,
            size,
        };
        if let Some(want) = def.object_type {
            match out.layers[0].get("object_type") {
                Some(Value::U16(t)) if *t == want => {}
                _ => {
                    return Err(Error::Invalid(
                        "obje +0 object type does not match the tag class",
                    ));
                }
            }
        }
        Ok(out)
    }
}

/// The shared `obje` header, typed for the fields with engine evidence.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectHeader {
    pub object_type: u16,
    pub flags: u16,
    pub bounding_radius: f32,
    pub bounding_offset: [f32; 3],
    pub default_variant: StringId,
    pub model: TagReference,
    pub creation_effect: TagReference,
    pub attachments: Vec<Attachment>,
    pub widgets: Vec<TagReference>,
    pub functions: Vec<Function>,
    pub change_colors: u32,
    pub predicted_resources: Vec<TagId>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Attachment {
    pub kind: TagReference,
    pub marker: Option<String>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Function {
    pub output: Option<String>,
}
impl ObjectHeader {
    pub fn from_record(r: &Record) -> Result<Self> {
        let missing = || Error::Invalid("not an obje record");
        let u16f = |n| match r.get(n) {
            Some(Value::U16(v)) => Ok(*v),
            _ => Err(missing()),
        };
        let f32f = |n| match r.get(n) {
            Some(Value::F32(v)) => Ok(*v),
            _ => Err(missing()),
        };
        Ok(Self {
            object_type: u16f("object_type")?,
            flags: u16f("flags")?,
            bounding_radius: f32f("bounding_radius")?,
            bounding_offset: match r.get("bounding_offset") {
                Some(Value::Vec3(v)) => *v,
                _ => return Err(missing()),
            },
            default_variant: r.sid("default_variant").ok_or_else(missing)?.0,
            model: r.tag_ref("model").ok_or_else(missing)?,
            creation_effect: r.tag_ref("creation_effect").ok_or_else(missing)?,
            attachments: r
                .elements("attachments")
                .iter()
                .map(|e| Attachment {
                    kind: e.tag_ref("type").expect("layout"),
                    marker: e.sid("marker").and_then(|s| s.1.map(str::to_owned)),
                })
                .collect(),
            widgets: r
                .elements("widgets")
                .iter()
                .filter_map(|e| e.tag_ref("type"))
                .collect(),
            functions: r
                .elements("functions")
                .iter()
                .map(|e| Function {
                    output: e.sid("output").and_then(|s| s.1.map(str::to_owned)),
                })
                .collect(),
            change_colors: r.block("change_colors").map_or(0, |b| b.count),
            predicted_resources: r
                .elements("predicted_resources")
                .iter()
                .filter_map(|e| match e.get("tag") {
                    Some(Value::TagDatum(t)) => Some(*t),
                    _ => None,
                })
                .collect(),
        })
    }
}

/// True for the groups whose tags are placed in the world (`obje` subtypes).
pub fn is_object_group(group: FourCC) -> bool {
    GroupDef::find(group).is_some_and(|d| d.object_type.is_some())
}

#[cfg(test)]
mod tests;
