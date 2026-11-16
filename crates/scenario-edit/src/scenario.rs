//! The scenario's placement blocks, read once from a map and kept as raw
//! element bytes. Typed access goes through [`crate::layout`]; edits change
//! only the bytes of the field they name, and [`Scenario::patches`] turns
//! the difference from the source into fixed-size tag patches.
use crate::layout::{self, BlockDef, BlockKind, FieldDef, FieldType, Role};
use crate::{Error, Result};
use map::{FourCC, Map, TagId};
use std::collections::HashMap;
use std::io::{Read, Seek};

/// The `scnr` root is at least this long: the furthest block pair this crate
/// reads is crates' palette at `+0x330`.
const ROOT: u32 = 0x338;

/// An 8-byte tag reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TagRef {
    pub group: FourCC,
    pub id: TagId,
}
impl TagRef {
    pub const NONE: Self = Self {
        group: FourCC([0xff; 4]),
        id: TagId::NULL,
    };
    pub fn is_none(self) -> bool {
        self.id == TagId::NULL
    }
    fn decode(b: &[u8]) -> Self {
        Self {
            group: FourCC::from_u32(u32_at(b, 0)),
            id: TagId(u32_at(b, 4)),
        }
    }
    fn encode(self) -> [u8; 8] {
        let mut out = [0; 8];
        out[..4].copy_from_slice(&self.group.as_u32().to_le_bytes());
        out[4..].copy_from_slice(&self.id.0.to_le_bytes());
        out
    }
}

/// A typed field value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value {
    Int(i64),
    Float(f32),
    Vec3([f32; 3]),
    Tag(TagRef),
}
impl Value {
    pub fn as_vec3(self) -> Option<[f32; 3]> {
        match self {
            Self::Vec3(v) => Some(v),
            _ => None,
        }
    }
    pub fn as_f32(self) -> Option<f32> {
        match self {
            Self::Float(v) => Some(v),
            _ => None,
        }
    }
    pub fn as_int(self) -> Option<i64> {
        match self {
            Self::Int(v) => Some(v),
            _ => None,
        }
    }
}

/// One tag in the map's index, for labels and reference validation.
#[derive(Clone, Debug)]
pub struct TagInfo {
    pub group: FourCC,
    pub name: Option<String>,
}

/// One placement block.
#[derive(Clone, Debug)]
pub struct Block {
    pub def: &'static BlockDef,
    /// Payload offset of element 0 (unused when the block is empty).
    pub start: u32,
    entries: Vec<Vec<u8>>,
    original: Vec<Vec<u8>>,
    /// The palette's tag references, when the block has a palette.
    pub palette: Vec<TagRef>,
}
impl Block {
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    /// Current bytes of an element (including every unknown byte).
    pub fn entry(&self, index: usize) -> Option<&[u8]> {
        self.entries.get(index).map(Vec::as_slice)
    }
}

/// The scenario tag's placement blocks.
#[derive(Clone, Debug)]
pub struct Scenario {
    pub tag: TagId,
    /// Payload virtual address and size.
    pub address: u32,
    pub size: u32,
    pub blocks: Vec<Block>,
    pub tags: HashMap<TagId, TagInfo>,
}

pub(crate) fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_bits(u32_at(b, o))
}

impl Scenario {
    /// Read the map's scenario. Every block must fit the scenario's own
    /// payload with its declared stride; otherwise nothing is returned.
    pub fn read<R: Read + Seek>(map: &mut Map<R>) -> Result<Self> {
        let tag = map
            .scenario()
            .ok_or(Error::Invalid("map has no scenario"))?;
        let id = tag.id;
        let loc = tag
            .location
            .ok_or(Error::Invalid("scenario has no payload"))?;
        let bytes = map.read_tag_bytes(id)?;
        let tags = map
            .tags()
            .iter()
            .map(|t| {
                (
                    t.id,
                    TagInfo {
                        group: t.group,
                        name: t.name.clone(),
                    },
                )
            })
            .collect();
        Self::decode(id, loc.address, &bytes, tags)
    }

    /// Decode from a payload whose first byte is at virtual `address`.
    pub fn decode(
        tag: TagId,
        address: u32,
        bytes: &[u8],
        tags: HashMap<TagId, TagInfo>,
    ) -> Result<Self> {
        if bytes.len() < ROOT as usize {
            return Err(Error::Invalid("scenario root is too short"));
        }
        let size = u32::try_from(bytes.len()).map_err(|_| Error::Invalid("scenario too large"))?;
        let elements = |field: u32, stride: u32| -> Result<(u32, Vec<Vec<u8>>)> {
            let count = u32_at(bytes, field as usize);
            let pointer = u32_at(bytes, field as usize + 4);
            if count == 0 {
                return Ok((0, Vec::new()));
            }
            let len = u64::from(count) * u64::from(stride);
            let start = pointer
                .checked_sub(address)
                .filter(|&s| u64::from(s) + len <= u64::from(size))
                .ok_or(Error::Invalid(
                    "scenario block outside the scenario payload",
                ))?;
            if start < ROOT {
                return Err(Error::Invalid("scenario block overlaps the root"));
            }
            let at = start as usize;
            let out = bytes[at..at + len as usize]
                .chunks_exact(stride as usize)
                .map(<[u8]>::to_vec)
                .collect();
            Ok((start, out))
        };
        let mut blocks = Vec::new();
        for def in layout::BLOCKS {
            let (start, entries) = elements(def.offset, def.stride)?;
            let palette = match &def.palette {
                Some(p) => elements(p.offset, p.stride)?
                    .1
                    .iter()
                    .map(|e| TagRef::decode(e))
                    .collect(),
                None => Vec::new(),
            };
            blocks.push(Block {
                def,
                start,
                original: entries.clone(),
                entries,
                palette,
            });
        }
        // Blocks this editor writes must not overlap each other, or one
        // patch would change another block's element.
        let mut spans: Vec<(u32, u32)> = blocks
            .iter()
            .filter(|b| !b.is_empty())
            .map(|b| (b.start, b.start + b.len() as u32 * b.def.stride))
            .collect();
        spans.sort();
        if spans.windows(2).any(|w| w[0].1 > w[1].0) {
            return Err(Error::Invalid("scenario placement blocks overlap"));
        }
        Ok(Self {
            tag,
            address,
            size,
            blocks,
            tags,
        })
    }

    pub fn block(&self, kind: BlockKind) -> &Block {
        self.blocks
            .iter()
            .find(|b| b.def.kind == kind)
            .expect("every kind is read")
    }
    fn block_mut(&mut self, kind: BlockKind) -> &mut Block {
        self.blocks
            .iter_mut()
            .find(|b| b.def.kind == kind)
            .expect("every kind is read")
    }

    fn locate(
        &self,
        kind: BlockKind,
        index: usize,
        field: &str,
    ) -> Result<(&[u8], &'static FieldDef)> {
        let block = self.block(kind);
        let def = block
            .def
            .field(field)
            .ok_or(Error::NoField(field.to_owned()))?;
        let entry = block.entry(index).ok_or(Error::NoEntry(kind, index))?;
        Ok((entry, def))
    }

    /// A field's current value.
    pub fn get(&self, kind: BlockKind, index: usize, field: &str) -> Result<Value> {
        let (entry, def) = self.locate(kind, index, field)?;
        Ok(read_field(entry, def))
    }

    /// Set a field after validating the value. Returns the previous value.
    /// Read-only fields, wrong types, out-of-range integers, nonfinite
    /// floats, palette indices outside the palette and references to tags
    /// that are absent or of a group the field does not accept are refused.
    pub fn set(
        &mut self,
        kind: BlockKind,
        index: usize,
        field: &str,
        value: Value,
    ) -> Result<Value> {
        let (_, def) = self.locate(kind, index, field)?;
        if !def.editable {
            return Err(Error::ReadOnly(def.name));
        }
        self.validate(kind, def, value)?;
        self.write(kind, index, field, value)
    }

    /// Write a field without validation: undo uses it to restore source
    /// values exactly, even ones the validator would refuse as new input.
    pub(crate) fn write(
        &mut self,
        kind: BlockKind,
        index: usize,
        field: &str,
        value: Value,
    ) -> Result<Value> {
        let (entry, def) = self.locate(kind, index, field)?;
        let old = read_field(entry, def);
        let bytes = encode_field(def, value)?;
        let at = def.offset as usize;
        self.block_mut(kind).entries[index][at..at + bytes.len()].copy_from_slice(&bytes);
        Ok(old)
    }

    fn validate(&self, kind: BlockKind, def: &FieldDef, value: Value) -> Result<()> {
        match (def.ty, value) {
            (FieldType::Vec3, Value::Vec3(v)) if v.iter().all(|x| x.is_finite()) => {}
            (FieldType::F32, Value::Float(v)) if v.is_finite() => {
                if def.role == Role::Scale && v < 0.0 {
                    return Err(Error::Value(def.name, "scale must not be negative"));
                }
            }
            (FieldType::Vec3 | FieldType::F32, Value::Vec3(_) | Value::Float(_)) => {
                return Err(Error::Value(def.name, "must be finite"));
            }
            (FieldType::U8 | FieldType::I16 | FieldType::U16 | FieldType::U32, Value::Int(v)) => {
                let (lo, hi) = match def.ty {
                    FieldType::U8 => (0, 0xff),
                    FieldType::I16 => (-0x8000, 0x7fff),
                    FieldType::U16 => (0, 0xffff),
                    _ => (0, 0xffff_ffff),
                };
                if !(lo..=hi).contains(&v) {
                    return Err(Error::Value(def.name, "out of range for its type"));
                }
                if def.role == Role::PaletteIndex {
                    let n = self.block(kind).palette.len() as i64;
                    if v != -1 && !(0..n).contains(&v) {
                        return Err(Error::Value(def.name, "not an entry of the palette"));
                    }
                }
            }
            (FieldType::TagRef, Value::Tag(r)) => {
                if !r.is_none() {
                    let info = self
                        .tags
                        .get(&r.id)
                        .ok_or(Error::Value(def.name, "tag is not in this map"))?;
                    if info.group != r.group {
                        return Err(Error::Value(def.name, "group does not match the tag"));
                    }
                    if !layout::ITEM_GROUPS.contains(&r.group.0) {
                        return Err(Error::Value(def.name, "group is not accepted here"));
                    }
                }
            }
            _ => return Err(Error::Value(def.name, "wrong value type")),
        }
        Ok(())
    }

    /// Whether any element differs from the source.
    pub fn is_modified(&self) -> bool {
        self.blocks.iter().any(|b| b.entries != b.original)
    }

    /// The minimal fixed-size patches (payload offset, bytes) that turn the
    /// source scenario into the current one: one per contiguous run of
    /// changed bytes inside an element. Unchanged bytes are never written.
    pub fn patches(&self) -> Vec<(u32, Vec<u8>)> {
        let mut out = Vec::new();
        for b in &self.blocks {
            for (i, (now, was)) in b.entries.iter().zip(&b.original).enumerate() {
                let base = b.start + i as u32 * b.def.stride;
                let mut k = 0;
                while k < now.len() {
                    if now[k] == was[k] {
                        k += 1;
                        continue;
                    }
                    let s = k;
                    while k < now.len() && now[k] != was[k] {
                        k += 1;
                    }
                    out.push((base + s as u32, now[s..k].to_vec()));
                }
            }
        }
        out
    }

    /// A short label for an element: the base name of its palette tag or
    /// item reference (from the map's own tag names), else its index.
    pub fn label(&self, kind: BlockKind, index: usize) -> String {
        let block = self.block(kind);
        let tag = if let Some(f) = block.def.role(Role::PaletteIndex) {
            self.get(kind, index, f.name)
                .ok()
                .and_then(Value::as_int)
                .and_then(|i| usize::try_from(i).ok())
                .and_then(|i| block.palette.get(i).copied())
        } else {
            block
                .def
                .fields
                .iter()
                .find(|f| f.ty == FieldType::TagRef)
                .and_then(|f| match self.get(kind, index, f.name) {
                    Ok(Value::Tag(r)) => Some(r),
                    _ => None,
                })
        };
        match tag.and_then(|r| self.tag_name(r.id)) {
            Some(name) => format!("{index}: {name}"),
            None => format!("{index}"),
        }
    }

    /// The base name (after the last backslash) of a tag, if the map has names.
    pub fn tag_name(&self, id: TagId) -> Option<&str> {
        let name = self.tags.get(&id)?.name.as_deref()?;
        Some(name.rsplit('\\').next().unwrap_or(name))
    }
}

pub(crate) fn read_field(entry: &[u8], def: &FieldDef) -> Value {
    let o = def.offset as usize;
    match def.ty {
        FieldType::U8 => Value::Int(entry[o].into()),
        FieldType::I16 => Value::Int(i16::from_le_bytes([entry[o], entry[o + 1]]).into()),
        FieldType::U16 => Value::Int(u16::from_le_bytes([entry[o], entry[o + 1]]).into()),
        FieldType::U32 => Value::Int(u32_at(entry, o).into()),
        FieldType::F32 => Value::Float(f32_at(entry, o)),
        FieldType::Vec3 => {
            Value::Vec3([f32_at(entry, o), f32_at(entry, o + 4), f32_at(entry, o + 8)])
        }
        FieldType::TagRef => Value::Tag(TagRef::decode(&entry[o..o + 8])),
    }
}

fn encode_field(def: &FieldDef, value: Value) -> Result<Vec<u8>> {
    Ok(match (def.ty, value) {
        (FieldType::U8, Value::Int(v)) => vec![v as u8],
        (FieldType::I16 | FieldType::U16, Value::Int(v)) => (v as u16).to_le_bytes().to_vec(),
        (FieldType::U32, Value::Int(v)) => (v as u32).to_le_bytes().to_vec(),
        (FieldType::F32, Value::Float(v)) => v.to_le_bytes().to_vec(),
        (FieldType::Vec3, Value::Vec3(v)) => v.iter().flat_map(|x| x.to_le_bytes()).collect(),
        (FieldType::TagRef, Value::Tag(r)) => r.encode().to_vec(),
        _ => return Err(Error::Value(def.name, "wrong value type")),
    })
}
