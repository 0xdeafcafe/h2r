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
    /// Virtual address of element 0 in the source (0 when it was empty).
    pub address: u32,
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
    /// Elements the source block holds: up to this many fit in place.
    pub fn capacity(&self) -> usize {
        self.original.len()
    }
}

/// What a save writes for the current scenario.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// Fixed-size replacements `(ordinary metadata virtual address, bytes)`,
    /// one per contiguous run of changed bytes: element bytes in place, and
    /// the root count of a block that shrank or grew back within its source
    /// size.
    pub patches: Vec<(u32, Vec<u8>)>,
    /// Blocks that outgrew their source: written as a new copy appended to
    /// the metadata, then the root's `(count, address)` pointed at it.
    pub grown: Vec<Grown>,
}
impl Changes {
    pub fn is_empty(&self) -> bool {
        self.patches.is_empty() && self.grown.is_empty()
    }
}

/// A block written to a new place because it has more elements than its
/// source had.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grown {
    pub kind: BlockKind,
    /// Virtual address of the block's `(count, address)` pair in the root.
    pub root: u32,
    pub count: u32,
    /// Every element, in order.
    pub bytes: Vec<u8>,
}

/// A removal, recorded so it can be undone exactly.
#[derive(Clone, Debug, PartialEq)]
pub struct Removed {
    pub kind: BlockKind,
    pub index: usize,
    /// The removed element's bytes.
    pub entry: Vec<u8>,
    /// Object names repointed by the removal: (name index, old placement
    /// index).
    pub renamed: Vec<(usize, i16)>,
}

/// The scenario object names block, kept so a removal can repoint names.
#[derive(Clone, Debug, Default)]
pub struct Names {
    pub address: u32,
    entries: Vec<Vec<u8>>,
    original: Vec<Vec<u8>>,
}
impl Names {
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    /// (object type, placement index) of name `i`.
    pub fn target(&self, i: usize) -> Option<(i16, i16)> {
        let e = self.entries.get(i)?;
        let at = |o: usize| i16::from_le_bytes([e[o], e[o + 1]]);
        Some((
            at(layout::OBJECT_NAME_TYPE),
            at(layout::OBJECT_NAME_PLACEMENT),
        ))
    }
    fn set_placement(&mut self, i: usize, placement: i16) {
        let o = layout::OBJECT_NAME_PLACEMENT;
        self.entries[i][o..o + 2].copy_from_slice(&placement.to_le_bytes());
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
    /// Scenario object names (edited only by removals).
    pub names: Names,
    pub tags: HashMap<TagId, TagInfo>,
}

pub(crate) fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_bits(u32_at(b, o))
}

impl Scenario {
    /// Read the map's scenario. Every block must fit, with its declared
    /// stride, either the scenario's own payload or (a block this editor
    /// grew) the ordinary metadata past every tag; otherwise nothing is
    /// returned.
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
        // Outside its own payload, a block may only live past every tag:
        // where `MapEditor::append_metadata` puts a grown block.
        let space = map.metadata_space();
        let tags_end = map
            .tags()
            .iter()
            .filter_map(|t| t.location)
            .filter(|l| Some(l.space) == space)
            .map(|l| u64::from(l.address) + u64::from(l.size))
            .max()
            .unwrap_or(0);
        let mut fetch = |address: u32, size: u32| -> Result<Vec<u8>> {
            if u64::from(address) < tags_end {
                return Err(Error::Invalid("block inside another tag"));
            }
            let at = map.file_offset(address, size)?;
            Ok(map.read_file_range(at, size.into())?)
        };
        Self::decode(id, loc.address, &bytes, tags, &mut fetch)
    }

    /// Decode from a payload whose first byte is at virtual `address`.
    /// `fetch(address, size)` reads blocks that lie outside the payload.
    pub fn decode(
        tag: TagId,
        address: u32,
        bytes: &[u8],
        tags: HashMap<TagId, TagInfo>,
        fetch: &mut dyn FnMut(u32, u32) -> Result<Vec<u8>>,
    ) -> Result<Self> {
        if bytes.len() < ROOT as usize {
            return Err(Error::Invalid("scenario root is too short"));
        }
        let size = u32::try_from(bytes.len()).map_err(|_| Error::Invalid("scenario too large"))?;
        let mut elements = |field: u32, stride: u32| -> Result<(u32, Vec<Vec<u8>>)> {
            let count = u32_at(bytes, field as usize);
            let pointer = u32_at(bytes, field as usize + 4);
            if count == 0 {
                return Ok((0, Vec::new()));
            }
            let len = u64::from(count) * u64::from(stride);
            let len32 =
                u32::try_from(len).map_err(|_| Error::Invalid("scenario block too large"))?;
            if pointer < address.saturating_add(ROOT) && address < pointer.saturating_add(len32) {
                return Err(Error::Invalid("scenario block overlaps the root"));
            }
            let inside = pointer
                .checked_sub(address)
                .filter(|&s| u64::from(s) + len <= u64::from(size));
            let data = match inside {
                Some(s) => bytes[s as usize..s as usize + len as usize].to_vec(),
                None if pointer >= address && pointer < address + size => {
                    return Err(Error::Invalid(
                        "scenario block runs past the scenario payload",
                    ));
                }
                None => fetch(pointer, len32).map_err(|_| {
                    Error::Invalid("scenario block outside the scenario and the metadata")
                })?,
            };
            let out = data
                .chunks_exact(stride as usize)
                .map(<[u8]>::to_vec)
                .collect();
            Ok((pointer, out))
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
                address: start,
                original: entries.clone(),
                entries,
                palette,
            });
        }
        let (names_at, names_entries) = elements(layout::OBJECT_NAMES, layout::OBJECT_NAME_STRIDE)?;
        let names = Names {
            address: names_at,
            original: names_entries.clone(),
            entries: names_entries,
        };
        // Blocks this editor writes must not overlap each other, or one
        // patch would change another block's element.
        let mut spans: Vec<(u64, u64)> = blocks
            .iter()
            .filter(|b| !b.is_empty())
            .map(|b| (b.address, b.len(), b.def.stride))
            .chain((!names.is_empty()).then_some((
                names.address,
                names.len(),
                layout::OBJECT_NAME_STRIDE,
            )))
            .map(|(address, n, stride)| {
                let a = u64::from(address);
                (a, a + n as u64 * u64::from(stride))
            })
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
            names,
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
            || self.names.entries != self.names.original
    }

    /// What a save must write: the minimal fixed-size patches for blocks
    /// that fit their source space (one per contiguous run of changed
    /// bytes, plus the root count when it changed), and a full copy of each
    /// block that outgrew it. Unchanged bytes are never written; a removal
    /// leaves the old tail element's bytes in place as dead data.
    pub fn changes(&self) -> Changes {
        let mut out = Changes::default();
        for b in &self.blocks {
            let root = self.address + b.def.offset;
            if b.len() > b.capacity() {
                out.grown.push(Grown {
                    kind: b.def.kind,
                    root,
                    count: b.len() as u32,
                    bytes: b.entries.concat(),
                });
                continue;
            }
            diff_into(
                &mut out.patches,
                b.address,
                b.def.stride,
                &b.entries,
                &b.original,
            );
            if b.len() != b.capacity() {
                out.patches
                    .push((root, (b.len() as u32).to_le_bytes().to_vec()));
            }
        }
        diff_into(
            &mut out.patches,
            self.names.address,
            layout::OBJECT_NAME_STRIDE,
            &self.names.entries,
            &self.names.original,
        );
        out.patches.sort();
        out
    }

    /// Why entries of a block may not be removed, if they may not.
    pub fn remove_refusal(&self, kind: BlockKind) -> Option<&'static str> {
        layout::block(kind).remove_refusal
    }

    /// Swap-remove an element: the last element moves into its slot and
    /// the count drops by one. Returns what [`Self::unremove`] needs.
    pub fn remove(&mut self, kind: BlockKind, index: usize) -> Result<Removed> {
        if let Some(why) = self.remove_refusal(kind) {
            return Err(Error::Refused(why));
        }
        let block = self.block_mut(kind);
        if index >= block.len() {
            return Err(Error::NoEntry(kind, index));
        }
        let last = block.len() - 1;
        let ty = block.def.object_type;
        let entry = block.entries.swap_remove(index);
        // Names point at placements by (type, index): the removed one's
        // name now names nothing, the moved one's follows it.
        let mut renamed = Vec::new();
        if let Some(ty) = ty {
            for n in 0..self.names.len() {
                let Some((t, p)) = self.names.target(n) else {
                    continue;
                };
                if t != ty || p < 0 {
                    continue;
                }
                let to = if p as usize == index {
                    -1
                } else if p as usize == last {
                    index as i16
                } else {
                    continue;
                };
                renamed.push((n, p));
                self.names.set_placement(n, to);
            }
        }
        Ok(Removed {
            kind,
            index,
            entry,
            renamed,
        })
    }

    /// Scenario object names that name entry `index` of a placement block.
    pub fn names_of(&self, kind: BlockKind, index: usize) -> Vec<usize> {
        let Some(ty) = layout::block(kind).object_type else {
            return Vec::new();
        };
        (0..self.names.len())
            .filter(|&n| self.names.target(n) == Some((ty, index as i16)))
            .collect()
    }

    /// Undo [`Self::remove`]: the moved element returns to the end and the
    /// removed one to its slot, byte for byte.
    pub fn unremove(&mut self, removed: &Removed) -> Result<()> {
        let block = self.block_mut(removed.kind);
        if removed.index > block.len() {
            return Err(Error::NoEntry(removed.kind, removed.index));
        }
        if removed.index == block.len() {
            block.entries.push(removed.entry.clone());
        } else {
            let moved = std::mem::replace(&mut block.entries[removed.index], removed.entry.clone());
            block.entries.push(moved);
        }
        for &(n, p) in removed.renamed.iter().rev() {
            self.names.set_placement(n, p);
        }
        Ok(())
    }

    /// Why entries may not be added to a block, if they may not.
    pub fn add_refusal(&self, kind: BlockKind) -> Option<&'static str> {
        let def = layout::block(kind);
        if let Some(why) = def.add_refusal {
            return Some(why);
        }
        let b = self.block(kind);
        if def.max_count.is_some_and(|m| b.len() >= m) {
            return Some("the block is at the engine's entry limit");
        }
        None
    }

    /// Append an element (exactly one stride of bytes). Returns its index.
    /// A block that grows past its source size is written as a new copy at
    /// the end of the metadata on save.
    pub fn add(&mut self, kind: BlockKind, entry: Vec<u8>) -> Result<usize> {
        if let Some(why) = self.add_refusal(kind) {
            return Err(Error::Refused(why));
        }
        let block = self.block_mut(kind);
        if entry.len() != block.def.stride as usize {
            return Err(Error::Invalid("entry is not one element long"));
        }
        block.entries.push(entry);
        Ok(block.len() - 1)
    }

    /// Undo [`Self::add`]: drop the last element, which must be `entry`.
    pub fn unadd(&mut self, kind: BlockKind, entry: &[u8]) -> Result<()> {
        let block = self.block_mut(kind);
        if block.entries.last().map(Vec::as_slice) != Some(entry) {
            return Err(Error::Invalid("the added entry is no longer last"));
        }
        block.entries.pop();
        Ok(())
    }

    /// A new element for a block: a copy of element `like` if given, else a
    /// zeroed element whose palette index is the palette's first entry
    /// (when it has one) and whose name index is -1. The position, when
    /// given, is written to the position field.
    pub fn new_entry(
        &self,
        kind: BlockKind,
        like: Option<usize>,
        position: Option<[f32; 3]>,
    ) -> Result<Vec<u8>> {
        let block = self.block(kind);
        let mut entry = match like {
            Some(i) => block.entry(i).ok_or(Error::NoEntry(kind, i))?.to_vec(),
            None => {
                let mut e = vec![0; block.def.stride as usize];
                if let Some(f) = block.def.role(Role::PaletteIndex) {
                    let first: i16 = if block.palette.is_empty() { -1 } else { 0 };
                    let o = f.offset as usize;
                    e[o..o + 2].copy_from_slice(&first.to_le_bytes());
                }
                if let Some(f) = block.def.field("name index") {
                    let o = f.offset as usize;
                    e[o..o + 2].copy_from_slice(&(-1i16).to_le_bytes());
                }
                e
            }
        };
        if let (Some(p), Some(f)) = (position, block.def.role(Role::Position)) {
            if !p.iter().all(|x| x.is_finite()) {
                return Err(Error::Value(f.name, "must be finite"));
            }
            let o = f.offset as usize;
            for (k, x) in p.iter().enumerate() {
                entry[o + 4 * k..o + 4 * k + 4].copy_from_slice(&x.to_le_bytes());
            }
        }
        Ok(entry)
    }

    /// The tag an element places: its palette entry, or its item reference
    /// (netgame and starting equipment).
    pub fn object_tag(&self, kind: BlockKind, index: usize) -> Option<TagRef> {
        let block = self.block(kind);
        if let Some(f) = block.def.role(Role::PaletteIndex) {
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
        }
    }

    /// A short label for an element: the base name of its palette tag or
    /// item reference (from the map's own tag names), else its index.
    pub fn label(&self, kind: BlockKind, index: usize) -> String {
        let tag = self.object_tag(kind, index);
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

/// Push one patch per contiguous run of bytes that differ between
/// `now[i]` and `was[i]`, for the elements both have.
fn diff_into(
    out: &mut Vec<(u32, Vec<u8>)>,
    address: u32,
    stride: u32,
    now: &[Vec<u8>],
    was: &[Vec<u8>],
) {
    for (i, (now, was)) in now.iter().zip(was).enumerate() {
        let base = address + i as u32 * stride;
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
