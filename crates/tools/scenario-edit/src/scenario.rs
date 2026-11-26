//! The scenario's placement blocks, read once from a map and kept as raw
//! element bytes. Typed access goes through [`crate::layout`]; edits change
//! only the bytes of the field they name, and [`Scenario::patches`] turns
//! the difference from the source into fixed-size tag patches.
pub(crate) use tags::bytes::u32_at;
use tags::bytes::f32_at;
use crate::layout::{self, BlockDef, BlockKind, FieldDef, FieldType, Role};
use crate::volumes::{self, Frame, VolumeBlock, VolumeKind, Volumes};
use crate::{Error, Result};
use map::{FourCC, Map, TagId};
use std::collections::HashMap;
use std::io::{Read, Seek};
use std::sync::atomic::{AtomicU64, Ordering};

pub mod data;
pub mod ground;

/// The `scnr` root is at least this long: the furthest block pair this crate
/// reads is crates' palette at `+0x330`.
const ROOT: u32 = 0x338;

/// The compiled script syntax nodes' block pair. Script atoms name
/// trigger volumes by index; the editor does not read the atoms' types, so
/// a map with any node keeps its source volumes in their slots.
const SCRIPT_NODES: usize = 0x238;

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

// Layout identities also distinguish divergent edits to cloned scenarios.
fn next_layout_rev() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LayoutChange {
    before: u64,
    after: u64,
}

/// Identity of an entry within its document and block. A swap-removal changes
/// the last entry's index without changing which placement it represents.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PlacementId {
    Source(usize),
    /// The layout revision created by the append. Redo reuses the same revision.
    Added(u64),
}

/// One placement block.
#[derive(Clone, Debug)]
pub struct Block {
    pub def: &'static BlockDef,
    /// Virtual address of element 0 in the source (0 when it was empty).
    pub address: u32,
    entries: Vec<Vec<u8>>,
    original: Vec<Vec<u8>>,
    ids: Vec<PlacementId>,
    layout_rev: u64,
    // unadd takes bytes, so it needs the parent of each consecutive append.
    // A removal carries this trail in its undo receipt.
    before_appends: Vec<u64>,
    /// The palette's tag references, when the block has a palette.
    pub palette: Vec<TagRef>,
}
impl Block {
    /// Identity of the block's ordered placement slots. Zero names the source
    /// layout. Field edits preserve it and undo restores the prior identity.
    pub fn layout_rev(&self) -> u64 {
        self.layout_rev
    }
    fn change_layout(&mut self, replay: Option<LayoutChange>) -> LayoutChange {
        let before = self.layout_rev;
        // Redo can reuse an identity only when it starts from its parent.
        let after = replay
            .filter(|change| change.before == before)
            .map_or_else(next_layout_rev, |change| change.after);
        self.layout_rev = after;
        LayoutChange { before, after }
    }
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
    pub fn placement_id(&self, index: usize) -> Option<PlacementId> {
        self.ids.get(index).copied()
    }
    pub fn source_entries(&self) -> &[Vec<u8>] {
        &self.original
    }
    /// Decode a field supplied by the game's conditional write reply.
    pub fn field_value(&self, index: usize, field: &FieldDef, bytes: &[u8]) -> Option<Value> {
        let mut entry = self.entry(index)?.to_vec();
        if bytes.len() != field.ty.size() as usize {
            return None;
        }
        let at = field.offset as usize;
        entry.get_mut(at..at + bytes.len())?.copy_from_slice(bytes);
        Some(read_field(&entry, field))
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
    /// Fixed-size replacements in structure BSP regions.
    pub bsp_patches: Vec<BspPatch>,
    /// Blocks that outgrew their source: written as a new copy appended to
    /// the metadata, then the root's `(count, address)` pointed at it.
    pub grown: Vec<Grown>,
    /// Other tags' blocks that outgrew their source (the tag editor's),
    /// and the scenario's volume blocks that did, written the same way.
    pub tag_grown: Vec<TagGrown>,
    /// String IDs to add after the map's own, in order (the tag editor's).
    pub strings: Vec<String>,
    /// Fixed-size replacements inside local raw blocks (the Modeling
    /// tool's in-place geometry writes).
    pub raw_patches: Vec<RawPatch>,
    /// New raw blocks (the Modeling tool's grown sections), each with the
    /// tag word that takes its raw pointer.
    pub raw_appends: Vec<RawAppend>,
    /// The Modeling tool's grown tag blocks (a structure's level collision
    /// with a solid added), each appended to the metadata with its pair,
    /// in the ordinary metadata or a BSP region, pointed at the copy.
    pub geometry_grown: Vec<GeometryGrown>,
    /// Tags written whole to a new place (the Textures tool's bitmaps that
    /// gained images or sequences), each with the raw blocks its words
    /// point at.
    pub tag_moves: Vec<TagMove>,
    /// Why the Modeling tool's edits cannot be saved, when they cannot: a
    /// save is then refused whole, before anything is written.
    pub geometry_refusal: Option<String>,
    /// The same for the Textures tool's edits.
    pub bitmap_refusal: Option<String>,
    /// The same for the Sound window's mouth curve edits.
    pub sound_refusal: Option<String>,
    /// The areas record (`game_markers::record`) the map would carry,
    /// when the areas differ from the ones it was opened with (empty when
    /// they were all taken off); none otherwise. A save appends it to the map's tail
    /// ([`Scenario::area_record`]).
    pub areas: Option<Vec<u8>>,
}
impl Changes {
    pub fn is_empty(&self) -> bool {
        self.patches.is_empty()
            && self.bsp_patches.is_empty()
            && self.grown.is_empty()
            && self.tag_grown.is_empty()
            && self.strings.is_empty()
            && self.raw_patches.is_empty()
            && self.raw_appends.is_empty()
            && self.geometry_grown.is_empty()
            && self.tag_moves.is_empty()
            && self.geometry_refusal.is_none()
            && self.bitmap_refusal.is_none()
            && self.sound_refusal.is_none()
            && self.areas.is_none()
    }
}

/// A fixed-size replacement inside a raw data block of the map
/// (`map::MapEditor::patch_raw`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawPatch {
    pub block: map::RawPointer,
    pub block_size: u32,
    /// Offset of the first byte in the block.
    pub offset: u32,
    pub bytes: Vec<u8>,
}

/// New raw data for the map (`map::MapEditor::append_raw`) and the tag
/// word that takes its raw pointer: a grown section's block, whose block
/// info `+0` is the word.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawAppend {
    pub bytes: Vec<u8>,
    /// Alignment in the file, a power of two up to 0x1000.
    pub align: u32,
    /// Where the pointer word is: the ordinary metadata (`None`) or a BSP
    /// region.
    pub pointer_space: Option<map::AddressSpace>,
    pub pointer_address: u32,
}

/// A tag written whole to the end of the metadata, its index entry
/// pointed at the copy (`map::MapEditor::move_tag`); the old bytes stay as
/// dead data. A bitmap that gained images or sequences
/// (`tags::bitmap_grow`, docs/editor/modeling.md, "Adding images").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagMove {
    pub tag: map::TagId,
    /// The payload laid out from offset 0.
    pub bytes: Vec<u8>,
    /// Offsets of the words in `bytes` that hold an offset from the start
    /// (its block pointers): the copy's address is added to each.
    pub rebase: Vec<u32>,
    /// New raw blocks, each with the offset in `bytes` of the word that
    /// takes its raw pointer (an image's LOD pointer).
    pub raw: Vec<MovedRaw>,
}

/// A new raw block for a moved tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MovedRaw {
    pub bytes: Vec<u8>,
    /// Alignment in the file, a power of two up to 0x1000.
    pub align: u32,
    /// Offset in [`TagMove::bytes`] of the pointer word.
    pub pointer: u32,
}

/// What the Modeling tool hands the save (docs/editor/modeling-targets.md,
/// "The session and the save"): tag patches in the ordinary metadata and
/// BSP regions, raw block patches, new raw blocks, or the reason it cannot
/// be written.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GeometryChanges {
    pub patches: Vec<(u32, Vec<u8>)>,
    pub bsp_patches: Vec<BspPatch>,
    pub raw_patches: Vec<RawPatch>,
    pub raw_appends: Vec<RawAppend>,
    pub grown: Vec<GeometryGrown>,
    /// Tags written whole to a new place (the Textures tool's grown
    /// bitmaps).
    pub moved: Vec<TagMove>,
    pub refusal: Option<String>,
}

/// A Modeling tool block with more elements than its source had
/// (`geometry_edit::level_collision::GrownBlock`): appended to the
/// ordinary metadata (`map::MapEditor::append_metadata`), then its
/// `(count, address)` pair, which may lie in a BSP region, pointed there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeometryGrown {
    /// Where the pair is: the ordinary metadata (`None`) or a BSP region.
    pub pair_space: Option<map::AddressSpace>,
    pub pair: u32,
    pub count: u32,
    /// Every element, in order.
    pub bytes: Vec<u8>,
    /// Alignment in the metadata, a power of two up to 0x1000.
    pub align: u32,
}

/// A fixed-size replacement inside a structure BSP region.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BspPatch {
    /// The source region, whose file offset distinguishes BSPs at the same virtual address.
    pub space: map::AddressSpace,
    /// Starting virtual address in the region.
    pub address: u32,
    /// Replacement bytes, in file order.
    pub bytes: Vec<u8>,
}

/// A block of a tag other than the scenario with more elements than its
/// source had: appended to the metadata, then its `(count, address)` pair
/// pointed at the copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagGrown {
    /// Virtual address of the block's `(count, address)` pair.
    pub root: u32,
    pub count: u32,
    /// Every element, in order.
    pub bytes: Vec<u8>,
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
    pub(crate) layout: LayoutChange,
    id: PlacementId,
    before_appends: Vec<u64>,
}

/// A volume block removal, recorded so it can be undone exactly.
#[derive(Clone, Debug, PartialEq)]
pub struct VolumeRemoved {
    pub kind: VolumeKind,
    pub index: usize,
    /// The removed element's bytes.
    pub entry: Vec<u8>,
    /// Kill triggers and BSP switches that named the moved last volume and
    /// now name its new slot: (kind, entry index, old volume index).
    pub repointed: Vec<(VolumeKind, usize, i16)>,
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
    /// The text of name `i` (its 32 bytes up to the first NUL), or `None`
    /// past the block or for bytes that are not UTF-8.
    pub fn name(&self, i: usize) -> Option<&str> {
        let e = self.entries.get(i)?;
        let text = &e[..e.len().min(32)];
        let end = text.iter().position(|&b| b == 0).unwrap_or(text.len());
        std::str::from_utf8(&text[..end]).ok()
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
    /// Trigger volumes and what names them, decoded from
    /// [`Self::volume_block`] after every edit.
    pub volumes: Volumes,
    /// The trigger volume, kill trigger and BSP switch blocks' bytes, by
    /// [`VolumeKind`].
    volume_blocks: [VolumeBlock; 3],
    /// Each trigger volume name's text, by string ID, from the map's table.
    volume_names: HashMap<u32, String>,
    /// Why the volume blocks may not be edited, when they may not.
    volume_lock: Option<String>,
    /// The scenario has compiled script nodes (see [`SCRIPT_NODES`]).
    scripted: bool,
    pub tags: HashMap<TagId, TagInfo>,
    /// Patches to other tags' bytes (the tag editor's field edits, kept
    /// outside this tag): `(ordinary metadata virtual address, bytes)`,
    /// saved with the scenario's own. See [`Self::set_tag_patches`].
    tag_patches: Vec<(u32, Vec<u8>)>,
    bsp_patches: Vec<BspPatch>,
    /// Other tags' grown blocks. See [`Self::set_tag_grown`].
    tag_grown: Vec<TagGrown>,
    /// New string IDs. See [`Self::set_tag_strings`].
    tag_strings: Vec<String>,
    /// The recordings block's writes. See [`Self::set_recording_changes`].
    recording_patches: Vec<(u32, Vec<u8>)>,
    recording_grown: Vec<TagGrown>,
    /// The script tables' writes. See [`Self::set_script_changes`].
    script_patches: Vec<(u32, Vec<u8>)>,
    script_grown: Vec<TagGrown>,
    /// The payload as read, for the source of a resized data reference.
    payload: std::sync::Arc<[u8]>,
    /// Resized data references: each one's whole current payload, by the
    /// virtual address of its `(size, address)` pair. See [`data`].
    data: std::collections::BTreeMap<u32, Vec<u8>>,
    /// The Modeling tool's writes. See [`Self::set_geometry_changes`].
    geometry: GeometryChanges,
    /// The Textures tool's writes, in the same shape. See
    /// [`Self::set_bitmap_changes`].
    bitmap: GeometryChanges,
    /// The Sound window's mouth curve writes (raw patches only). See
    /// [`Self::set_sound_changes`].
    sound: GeometryChanges,
    /// The area of each netgame flag that has one, by placement, so a
    /// swap-removal or an append moves nothing
    /// (docs/game/gametype-markers.md, "In the document"). Edited through
    /// [`Self::set_area`]; an entry whose flag is gone waits for its undo.
    areas: std::collections::BTreeMap<PlacementId, game_markers::Area>,
    /// The areas record the map was opened with, as it encodes now (empty
    /// when it had none): a save writes the areas again, and
    /// [`Changes::areas`] holds the record only when it differs from this.
    areas_loaded: Vec<u8>,
    /// The ground label of each netgame flag that has one, by placement
    /// ([`ground`]).
    labels: std::collections::BTreeMap<PlacementId, ground::FlagLabel>,
}

impl Scenario {
    /// Read the map's scenario. Every block must fit, with its declared
    /// stride, either the scenario's own payload or (a block this editor
    /// grew) ordinary metadata no tag owns; otherwise nothing is
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
        // Outside its own payload, a block may only live in ordinary
        // metadata no tag owns: where `MapEditor::append_metadata` puts a
        // grown block, which tags added by a later rebuild (`map::build`)
        // may follow.
        let space = map.metadata_space();
        let tag_ranges: Vec<(u64, u64)> = map
            .tags()
            .iter()
            .filter_map(|t| t.location)
            .filter(|l| Some(l.space) == space)
            .map(|l| {
                (
                    u64::from(l.address),
                    u64::from(l.address) + u64::from(l.size),
                )
            })
            .collect();
        let mut fetch = |address: u32, size: u32| -> Result<Vec<u8>> {
            let (lo, hi) = (u64::from(address), u64::from(address) + u64::from(size));
            if tag_ranges.iter().any(|&(a, b)| lo < b && a < hi) {
                return Err(Error::Invalid("block inside another tag"));
            }
            let at = map.file_offset(address, size)?;
            Ok(map.read_file_range(at, size.into())?)
        };
        let mut scenario = Self::decode(id, loc.address, &bytes, tags, &mut fetch)?;
        scenario.volumes.resolve_names(map.strings());
        scenario.volume_names = scenario
            .volumes
            .triggers
            .iter()
            .filter_map(|v| Some((v.name_id, v.name.clone()?)))
            .collect();
        Ok(scenario)
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
                ids: (0..entries.len()).map(PlacementId::Source).collect(),
                entries,
                layout_rev: 0,
                before_appends: Vec::new(),
                palette,
            });
        }
        let (names_at, names_entries) = elements(layout::OBJECT_NAMES, layout::OBJECT_NAME_STRIDE)?;
        let names = Names {
            address: names_at,
            original: names_entries.clone(),
            entries: names_entries,
        };
        // A volume block that does not read leaves the volumes empty and
        // locked with the reason, and the placements still open.
        let read = VolumeKind::ALL.map(|kind| {
            let (offset, stride) = kind.block();
            elements(offset, stride)
        });
        let (volumes, volume_blocks, mut volume_lock) = match read {
            [Ok(t), Ok(k), Ok(w)] => {
                let volumes = Volumes::from_rows(&t.1, &k.1, &w.1);
                let blocks = [t, k, w].map(|(at, rows)| VolumeBlock::new(at, rows));
                (volumes, blocks, None)
            }
            [Err(e), ..] | [_, Err(e), _] | [.., Err(e)] => {
                let why = e.to_string();
                let lock = format!("the volume blocks do not read: {why}");
                (Volumes::failed(why), Default::default(), Some(lock))
            }
        };
        let scripted = u32_at(bytes, SCRIPT_NODES) != 0;
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
        // The volume blocks are written the same way: one that overlaps
        // another block is drawn but not edited.
        for (kind, block) in VolumeKind::ALL.iter().zip(&volume_blocks) {
            if block.is_empty() {
                continue;
            }
            let a = u64::from(block.address);
            spans.push((a, a + block.len() as u64 * u64::from(kind.block().1)));
        }
        spans.sort();
        if volume_lock.is_none() && spans.windows(2).any(|w| w[0].1 > w[1].0) {
            volume_lock = Some("a volume block overlaps another block".into());
        }
        Ok(Self {
            tag,
            address,
            size,
            blocks,
            names,
            volumes,
            volume_blocks,
            volume_names: HashMap::new(),
            volume_lock,
            scripted,
            tags,
            tag_patches: Vec::new(),
            bsp_patches: Vec::new(),
            tag_grown: Vec::new(),
            tag_strings: Vec::new(),
            recording_patches: Vec::new(),
            recording_grown: Vec::new(),
            script_patches: Vec::new(),
            script_grown: Vec::new(),
            payload: bytes.into(),
            data: std::collections::BTreeMap::new(),
            geometry: GeometryChanges::default(),
            bitmap: GeometryChanges::default(),
            sound: GeometryChanges::default(),
            areas: std::collections::BTreeMap::new(),
            areas_loaded: Vec::new(),
            labels: std::collections::BTreeMap::new(),
        })
    }

    /// Every netgame flag's type, identifier and position, by index: what
    /// an areas record's rows find their flags by
    /// ([`game_markers::record::place`]).
    fn record_flags(&self) -> Vec<game_markers::record::Flag> {
        let flags = BlockKind::NetgameFlags;
        (0..self.block(flags).len())
            .map(|i| game_markers::record::Flag {
                flag_type: self
                    .get(flags, i, "type")
                    .ok()
                    .and_then(Value::as_int)
                    .and_then(|v| i16::try_from(v).ok())
                    .unwrap_or(-1),
                identifier: self
                    .get(flags, i, "identifier")
                    .ok()
                    .and_then(Value::as_int)
                    .and_then(|v| i16::try_from(v).ok())
                    .unwrap_or(-1),
                at: self
                    .get(flags, i, "position")
                    .ok()
                    .and_then(Value::as_vec3)
                    .unwrap_or([0.0; 3]),
            })
            .collect()
    }

    /// The rows of the areas record a save writes: one for each flag that
    /// has an area, in flag order.
    pub fn area_rows(&self) -> Vec<game_markers::record::Row> {
        let flags = self.record_flags();
        self.flag_areas()
            .into_iter()
            .map(|(i, area)| game_markers::record::Row {
                index: i as u32,
                flag_type: flags[i].flag_type,
                identifier: flags[i].identifier,
                at: flags[i].at,
                area: Some(area.clone()),
                // Studio has no show control yet: a Studio map's flags are
                // the base map's, which shows none.
                show: false,
            })
            .collect()
    }

    /// The areas record a save appends to the map
    /// (`game_markers::record`), or none when no flag has an area.
    pub fn area_record(&self) -> Option<Vec<u8>> {
        let rows = self.area_rows();
        (!rows.is_empty()).then(|| game_markers::record::encode(&rows))
    }

    /// Takes the areas a map was saved with: each row goes to the flag it
    /// finds ([`game_markers::record::place`]) and is not an edit. Returns
    /// how many rows found no flag (the flag was moved or changed type
    /// since); those are dropped.
    pub fn load_areas(&mut self, rows: &[game_markers::record::Row]) -> usize {
        let (placed, lost) = game_markers::record::place(rows, &self.record_flags());
        for (index, area) in placed {
            // Stored as read: `Area::validate` passed in the decoder.
            let _ = self.write_area(index, Some(area));
        }
        self.areas_loaded = self.area_record().unwrap_or_default();
        lost.len()
    }

    /// The area of netgame flag `index`, if it has one.
    pub fn area(&self, index: usize) -> Option<&game_markers::Area> {
        let id = self.block(BlockKind::NetgameFlags).placement_id(index)?;
        self.areas.get(&id)
    }

    /// Every netgame flag's stored area, by flag index.
    pub fn flag_areas(&self) -> Vec<(usize, &game_markers::Area)> {
        (0..self.block(BlockKind::NetgameFlags).len())
            .filter_map(|i| Some((i, self.area(i)?)))
            .collect()
    }

    /// Whether flag `index` is one of three or more corners of one hill:
    /// the same hill type and identifier (docs/game/gametype-markers.md,
    /// "A hill is one flag or its corners").
    pub fn is_hill_corner(&self, index: usize) -> bool {
        let flags = BlockKind::NetgameFlags;
        let key = |i: usize| {
            let ty = self.get(flags, i, "type").ok()?.as_int()?;
            let id = self.get(flags, i, "identifier").ok()?.as_int()?;
            (11..=18).contains(&ty).then_some((ty, id))
        };
        let Some(mine) = key(index) else {
            return false;
        };
        (0..self.block(flags).len()).filter(|&i| key(i) == Some(mine)).count() >= 3
    }

    /// Sets or clears (`None`) netgame flag `index`'s area, and returns
    /// the one it replaced. Refused, with `Area::validate`'s sentence, for
    /// an area the tools may not store, and for one on a hill's corner.
    pub fn set_area(
        &mut self,
        index: usize,
        area: Option<game_markers::Area>,
    ) -> Result<Option<game_markers::Area>> {
        if let Some(a) = &area {
            a.validate().map_err(|e| Error::Declined(e.to_string()))?;
            if self.is_hill_corner(index) {
                return Err(Error::Declined("a hill with corners takes no area".into()));
            }
        }
        self.write_area(index, area)
    }

    /// [`Self::set_area`] without the checks, for an undo.
    pub(crate) fn write_area(
        &mut self,
        index: usize,
        area: Option<game_markers::Area>,
    ) -> Result<Option<game_markers::Area>> {
        let flags = BlockKind::NetgameFlags;
        let id = self
            .block(flags)
            .placement_id(index)
            .ok_or(Error::NoEntry(flags, index))?;
        Ok(match area {
            Some(a) => self.areas.insert(id, a),
            None => self.areas.remove(&id),
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
            || self.volume_blocks.iter().any(VolumeBlock::is_modified)
            || self.data_modified()
    }

    /// What a save must write: the minimal fixed-size patches for blocks
    /// that fit their source space (one per contiguous run of changed
    /// bytes, plus the root count when it changed), and a full copy of each
    /// block that outgrew it. Unchanged bytes are never written; a removal
    /// leaves the old tail element's bytes in place as dead data.
    pub fn changes(&self) -> Changes {
        let mut out = Changes {
            patches: self.tag_patches.clone(),
            bsp_patches: self.bsp_patches.clone(),
            tag_grown: self.tag_grown.clone(),
            strings: self.tag_strings.clone(),
            raw_patches: self.geometry.raw_patches.clone(),
            raw_appends: self.geometry.raw_appends.clone(),
            geometry_grown: self.geometry.grown.clone(),
            geometry_refusal: self.geometry.refusal.clone(),
            ..Changes::default()
        };
        // The areas count as a change when they are not the ones the map
        // was opened with; a map opened with areas and left alone has none.
        let record = self.area_record().unwrap_or_default();
        if record != self.areas_loaded {
            out.areas = Some(record);
        }
        out.patches.extend(self.recording_patches.iter().cloned());
        out.tag_grown.extend(self.recording_grown.iter().cloned());
        out.patches.extend(self.script_patches.iter().cloned());
        out.tag_grown.extend(self.script_grown.iter().cloned());
        out.patches.extend(self.geometry.patches.iter().cloned());
        out.bsp_patches
            .extend(self.geometry.bsp_patches.iter().cloned());
        // The Textures tool's writes go out the same way, after the
        // Modeling tool's (a raw block patch never shares a block with a
        // grown one: bitmap pixels and mesh sections are other blocks).
        out.patches.extend(self.bitmap.patches.iter().cloned());
        out.raw_patches.extend(self.bitmap.raw_patches.iter().cloned());
        out.raw_appends.extend(self.bitmap.raw_appends.iter().cloned());
        out.geometry_grown.extend(self.bitmap.grown.iter().cloned());
        out.tag_moves.extend(self.geometry.moved.iter().cloned());
        out.tag_moves.extend(self.bitmap.moved.iter().cloned());
        out.bitmap_refusal = self.bitmap.refusal.clone();
        // The Sound window's mouth curves: raw patches in the extra-info
        // resource, a block no other tool patches.
        out.raw_patches.extend(self.sound.raw_patches.iter().cloned());
        out.sound_refusal = self.sound.refusal.clone();
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
        // The volume blocks, by the same rules; one that outgrew its
        // source is written as the tag editor's grown blocks are.
        for (kind, b) in VolumeKind::ALL.iter().zip(&self.volume_blocks) {
            let (offset, stride) = kind.block();
            let root = self.address + offset;
            if b.len() > b.capacity() {
                out.tag_grown.push(TagGrown {
                    root,
                    count: b.len() as u32,
                    bytes: b.entries.concat(),
                });
                continue;
            }
            diff_into(&mut out.patches, b.address, stride, &b.entries, &b.original);
            if b.len() != b.capacity() {
                out.patches
                    .push((root, (b.len() as u32).to_le_bytes().to_vec()));
            }
        }
        self.data_changes(&mut out.patches, &mut out.tag_grown);
        out.patches.sort();
        out
    }

    /// Replace the patches to other tags' bytes (every changed run, not a
    /// delta). Refused when one touches this scenario's payload, which
    /// the placement blocks own.
    pub fn set_tag_patches(&mut self, patches: Vec<(u32, Vec<u8>)>) -> Result<()> {
        let (lo, hi) = (
            u64::from(self.address),
            u64::from(self.address) + u64::from(self.size),
        );
        if patches.iter().any(|(a, b)| {
            let (a, e) = (u64::from(*a), u64::from(*a) + b.len() as u64);
            a < hi && lo < e
        }) {
            return Err(Error::Refused(
                "the scenario tag's bytes belong to the Scenario tool",
            ));
        }
        self.tag_patches = patches;
        Ok(())
    }
    /// The patches to other tags' bytes.
    pub fn tag_patches(&self) -> &[(u32, Vec<u8>)] {
        &self.tag_patches
    }

    /// Replace the BSP region patches. Save checks the ranges against the source map.
    pub fn set_bsp_patches(&mut self, patches: Vec<BspPatch>) {
        self.bsp_patches = patches;
    }

    /// Replace the Modeling tool's writes (all of them, not a delta).
    /// Refused when a tag patch touches this scenario's payload. A patch
    /// that overlaps the tag editor's is refused by the save, which names
    /// both as overlapping edits.
    pub fn set_geometry_changes(&mut self, changes: GeometryChanges) -> Result<()> {
        let (lo, hi) = (
            u64::from(self.address),
            u64::from(self.address) + u64::from(self.size),
        );
        if changes.patches.iter().any(|(a, b)| {
            let (a, e) = (u64::from(*a), u64::from(*a) + b.len() as u64);
            a < hi && lo < e
        }) {
            return Err(Error::Refused(
                "the scenario tag's bytes belong to the Scenario tool",
            ));
        }
        for g in &changes.grown {
            let (a, e) = (u64::from(g.pair), u64::from(g.pair) + 8);
            if g.pair_space.is_none() && a < hi && lo < e {
                return Err(Error::Refused(
                    "the scenario tag's bytes belong to the Scenario tool",
                ));
            }
            if g.bytes.is_empty() || g.count == 0 {
                return Err(Error::Refused("a grown block has no elements"));
            }
        }
        self.geometry = changes;
        Ok(())
    }
    /// The Modeling tool's writes.
    pub fn geometry_changes(&self) -> &GeometryChanges {
        &self.geometry
    }

    /// Replace the Textures tool's writes (all of them, not a delta):
    /// metadata patches for the images' records, raw block patches for
    /// pixels in this map, new raw blocks for pixels that grew or came from
    /// a shared cache (`assets::write`). Refused, like
    /// [`Self::set_geometry_changes`], when a patch touches this scenario's
    /// payload. The `refusal` makes the save fail before it writes.
    pub fn set_bitmap_changes(&mut self, changes: GeometryChanges) -> Result<()> {
        let (lo, hi) = (
            u64::from(self.address),
            u64::from(self.address) + u64::from(self.size),
        );
        if changes.patches.iter().any(|(a, b)| {
            let (a, e) = (u64::from(*a), u64::from(*a) + b.len() as u64);
            a < hi && lo < e
        }) {
            return Err(Error::Refused(
                "the scenario tag's bytes belong to the Scenario tool",
            ));
        }
        if changes.moved.iter().any(|m| m.tag == self.tag) {
            return Err(Error::Refused(
                "the scenario tag's bytes belong to the Scenario tool",
            ));
        }
        self.bitmap = changes;
        Ok(())
    }
    /// The Textures tool's writes.
    pub fn bitmap_changes(&self) -> &GeometryChanges {
        &self.bitmap
    }

    /// Replace the Sound window's writes (all of them, not a delta): raw
    /// block patches inside a sound's extra-info resource
    /// (docs/editor/rich-editors.md, "Sounds and facial animation"). Only
    /// `raw_patches` and `refusal` are taken; any other write is refused,
    /// because a mouth curve edit never changes a length or a tag. The
    /// `refusal` makes the save fail before it writes.
    pub fn set_sound_changes(&mut self, changes: GeometryChanges) -> Result<()> {
        if !(changes.patches.is_empty()
            && changes.bsp_patches.is_empty()
            && changes.raw_appends.is_empty()
            && changes.grown.is_empty()
            && changes.moved.is_empty())
        {
            return Err(Error::Refused(
                "a mouth curve edit writes raw bytes in place and nothing else",
            ));
        }
        self.sound = changes;
        Ok(())
    }
    /// The Sound window's writes.
    pub fn sound_changes(&self) -> &GeometryChanges {
        &self.sound
    }

    /// Replace the recordings block's writes (all of them, not a delta):
    /// patches to the scenario's own `recorded_animation` elements, their
    /// count and the streams written in place, and the streams that
    /// outgrew their source, appended at save with the element's
    /// `(size, address)` pair pointed at the copy
    /// (`hs::recording_edit::changes`). They are the scenario's bytes but
    /// no placement block's, so they go out beside the blocks' own.
    /// Refused when a grown stream's pair lies outside the scenario's
    /// payload or a patch covers it.
    pub fn set_recording_changes(
        &mut self,
        patches: Vec<(u32, Vec<u8>)>,
        grown: Vec<TagGrown>,
    ) -> Result<()> {
        let (lo, hi) = (self.address, self.address.saturating_add(self.size));
        for g in &grown {
            let end = g.root.saturating_add(8);
            if g.root < lo || end > hi {
                return Err(Error::Refused(
                    "a grown recording's pair is outside the scenario's payload",
                ));
            }
            if patches
                .iter()
                .any(|(a, b)| *a < end && g.root < a.saturating_add(b.len() as u32))
            {
                return Err(Error::Refused("a grown recording's pair is also patched"));
            }
            if g.bytes.is_empty() || g.count == 0 {
                return Err(Error::Refused("a grown recording has no bytes"));
            }
        }
        self.recording_patches = patches;
        self.recording_grown = grown;
        Ok(())
    }
    /// The recordings block's in-place writes and grown streams.
    pub fn recording_changes(&self) -> (&[(u32, Vec<u8>)], &[TagGrown]) {
        (&self.recording_patches, &self.recording_grown)
    }

    /// Replace the script tables' writes (all of them, not a delta): the
    /// patches to the nodes, string data, scripts, globals and references
    /// the scenario's header describes, written in place, and the tables
    /// that outgrew their source, appended at save with the header's
    /// `(count, address)` pair pointed at the copy (`hs::table_writes`).
    /// They are the scenario's bytes but no placement block's, so they go
    /// out beside the blocks' own. Refused when a grown table's pair lies
    /// outside the scenario's payload or a patch covers it.
    pub fn set_script_changes(
        &mut self,
        patches: Vec<(u32, Vec<u8>)>,
        grown: Vec<TagGrown>,
    ) -> Result<()> {
        let (lo, hi) = (self.address, self.address.saturating_add(self.size));
        for g in &grown {
            let end = g.root.saturating_add(8);
            if g.root < lo || end > hi {
                return Err(Error::Refused(
                    "a grown script table's pair is outside the scenario's payload",
                ));
            }
            if patches
                .iter()
                .any(|(a, b)| *a < end && g.root < a.saturating_add(b.len() as u32))
            {
                return Err(Error::Refused("a grown script table's pair is also patched"));
            }
            if g.bytes.is_empty() || g.count == 0 {
                return Err(Error::Refused("a grown script table has no bytes"));
            }
        }
        self.script_patches = patches;
        self.script_grown = grown;
        Ok(())
    }
    /// The script tables' in-place writes and grown tables.
    pub fn script_changes(&self) -> (&[(u32, Vec<u8>)], &[TagGrown]) {
        (&self.script_patches, &self.script_grown)
    }

    /// Replace the other tags' grown blocks (all of them, not a delta).
    /// Refused when a block's pair lies in this scenario's payload, or a
    /// patch from [`Self::set_tag_patches`] covers it (the pair is written
    /// at save, once the copy has its address).
    pub fn set_tag_grown(&mut self, grown: Vec<TagGrown>) -> Result<()> {
        let (lo, hi) = (self.address, self.address.saturating_add(self.size));
        for g in &grown {
            let end = g.root.saturating_add(8);
            if g.root < hi && lo < end {
                return Err(Error::Refused(
                    "the scenario tag's bytes belong to the Scenario tool",
                ));
            }
            if self
                .tag_patches
                .iter()
                .any(|(a, b)| *a < end && g.root < a.saturating_add(b.len() as u32))
            {
                return Err(Error::Refused("a grown block's pair is also patched"));
            }
            if g.bytes.is_empty() || g.count == 0 {
                return Err(Error::Refused("a grown block has no elements"));
            }
        }
        self.tag_grown = grown;
        Ok(())
    }
    /// The other tags' grown blocks.
    pub fn tag_grown(&self) -> &[TagGrown] {
        &self.tag_grown
    }

    /// Replace the string IDs to add (all of them, in order: the first
    /// gets the index after the map's last). Their values are already in
    /// the patches that use them; the save checks each gets the index it
    /// was given.
    pub fn set_tag_strings(&mut self, strings: Vec<String>) {
        self.tag_strings = strings;
    }
    pub fn tag_strings(&self) -> &[String] {
        &self.tag_strings
    }

    /// A volume block's bytes.
    pub fn volume_block(&self, kind: VolumeKind) -> &VolumeBlock {
        &self.volume_blocks[kind.slot()]
    }

    /// Whether the map has compiled script nodes, which can name trigger
    /// volumes by index.
    pub fn is_scripted(&self) -> bool {
        self.scripted
    }

    /// Why entries of a volume block may not be changed or added, if they
    /// may not. BSP switches are read only: what else names a switch is
    /// not surveyed.
    pub fn volume_refusal(&self, kind: VolumeKind) -> Option<String> {
        if let Some(why) = &self.volume_lock {
            return Some(why.clone());
        }
        (kind == VolumeKind::BspSwitch).then(|| "BSP switches are read only".to_owned())
    }

    /// Why entry `index` of a volume block may not be removed, if it may
    /// not: a trigger volume a kill trigger or BSP switch names (each is
    /// named), or a source volume of a scripted map, whose scripts may
    /// name it or the last volume by index.
    pub fn volume_remove_refusal(&self, kind: VolumeKind, index: usize) -> Option<String> {
        if let Some(why) = self.volume_refusal(kind) {
            return Some(why);
        }
        let block = self.volume_block(kind);
        if index >= block.len() {
            return Some(format!(
                "no {} {index} (the map has {})",
                kind.noun(),
                block.len()
            ));
        }
        if kind != VolumeKind::Trigger {
            return None;
        }
        let holders = self.volumes.holders(index);
        if !holders.is_empty() {
            let names: Vec<String> = holders.iter().map(|h| h.describe()).collect();
            return Some(format!(
                "{} is named by {}",
                self.volumes.label(index),
                names.join(" and ")
            ));
        }
        (self.scripted && index < block.capacity()).then(|| {
            format!(
                "{} came with the map, whose scripts can name trigger volumes by index; \
                 only a volume added in this editor can be removed",
                self.volumes.label(index)
            )
        })
    }

    fn check_volume_entry(&self, kind: VolumeKind, entry: &[u8], count: usize) -> Result<()> {
        if entry.len() != kind.block().1 as usize {
            return Err(Error::Invalid("entry is not one element long"));
        }
        match kind {
            VolumeKind::Trigger if volumes::entry_frame(entry).axes().is_none() => {
                Err(Error::Value(
                    "frame",
                    "needs a finite origin, a positive size and two directions that are not parallel",
                ))
            }
            VolumeKind::Kill | VolumeKind::BspSwitch => {
                let v = volumes::named_volume(entry);
                if usize::try_from(v).ok().is_some_and(|v| v < count) {
                    Ok(())
                } else {
                    Err(Error::Value("trigger_volume", "names no trigger volume"))
                }
            }
            VolumeKind::Trigger => Ok(()),
        }
    }

    fn refresh_volumes(&mut self) {
        let [t, k, w] = &self.volume_blocks;
        let mut volumes = Volumes::from_rows(&t.entries, &k.entries, &w.entries);
        volumes.name_with(|id| self.volume_names.get(&id).cloned());
        self.volumes = volumes;
    }

    /// Replace entry `index` of a volume block after validating it.
    /// Returns the old bytes.
    pub fn set_volume(
        &mut self,
        kind: VolumeKind,
        index: usize,
        entry: Vec<u8>,
    ) -> Result<Vec<u8>> {
        if let Some(why) = self.volume_refusal(kind) {
            return Err(Error::Declined(why));
        }
        let triggers = self.volume_block(VolumeKind::Trigger).len();
        self.check_volume_entry(kind, &entry, triggers)?;
        self.write_volume(kind, index, entry)
    }

    /// Replace a volume entry without validation (undo).
    pub(crate) fn write_volume(
        &mut self,
        kind: VolumeKind,
        index: usize,
        entry: Vec<u8>,
    ) -> Result<Vec<u8>> {
        let block = &mut self.volume_blocks[kind.slot()];
        let slot = block
            .entries
            .get_mut(index)
            .ok_or_else(|| Error::Declined(format!("no {} {index}", kind.noun())))?;
        if slot.len() != entry.len() {
            return Err(Error::Invalid("entry is not one element long"));
        }
        let old = std::mem::replace(slot, entry);
        self.refresh_volumes();
        Ok(old)
    }

    /// Set trigger volume `index`'s frame (squared, every other byte
    /// kept). Returns the old entry.
    pub fn set_volume_frame(&mut self, index: usize, frame: &Frame) -> Result<Vec<u8>> {
        let like = self
            .volume_block(VolumeKind::Trigger)
            .entry(index)
            .ok_or_else(|| Error::Declined(format!("no trigger volume {index}")))?;
        let frame = frame.squared().unwrap_or(*frame);
        let entry = volumes::trigger_entry(Some(like), &frame);
        self.set_volume(VolumeKind::Trigger, index, entry)
    }

    /// Append an entry to a volume block. Returns its index. A block that
    /// grows past its source size is written as a new copy on save.
    pub fn add_volume(&mut self, kind: VolumeKind, entry: Vec<u8>) -> Result<usize> {
        if let Some(why) = self.volume_refusal(kind) {
            return Err(Error::Declined(why));
        }
        let triggers = self.volume_block(VolumeKind::Trigger).len();
        self.check_volume_entry(kind, &entry, triggers)?;
        let block = &mut self.volume_blocks[kind.slot()];
        block.entries.push(entry);
        let index = block.len() - 1;
        self.refresh_volumes();
        Ok(index)
    }

    /// Undo [`Self::add_volume`]: drop the last entry, which must be `entry`.
    pub fn unadd_volume(&mut self, kind: VolumeKind, entry: &[u8]) -> Result<()> {
        let block = &mut self.volume_blocks[kind.slot()];
        if block.entries.last().map(Vec::as_slice) != Some(entry) {
            return Err(Error::Invalid("the added entry is no longer last"));
        }
        block.entries.pop();
        self.refresh_volumes();
        Ok(())
    }

    /// Swap-remove entry `index` of a volume block: the last entry moves
    /// into its slot, and the kill triggers and BSP switches that named a
    /// moved trigger volume follow it. Refused as
    /// [`Self::volume_remove_refusal`] says.
    pub fn remove_volume(&mut self, kind: VolumeKind, index: usize) -> Result<VolumeRemoved> {
        if let Some(why) = self.volume_remove_refusal(kind, index) {
            return Err(Error::Declined(why));
        }
        let block = &mut self.volume_blocks[kind.slot()];
        let last = block.len() - 1;
        let entry = block.entries.swap_remove(index);
        let mut repointed = Vec::new();
        if kind == VolumeKind::Trigger && index != last {
            for holder in [VolumeKind::Kill, VolumeKind::BspSwitch] {
                let block = &mut self.volume_blocks[holder.slot()];
                for (i, e) in block.entries.iter_mut().enumerate() {
                    let v = volumes::named_volume(e);
                    if usize::try_from(v).ok() == Some(last) {
                        repointed.push((holder, i, v));
                        volumes::set_named_volume(e, index as i16);
                    }
                }
            }
        }
        self.refresh_volumes();
        Ok(VolumeRemoved {
            kind,
            index,
            entry,
            repointed,
        })
    }

    /// Undo [`Self::remove_volume`], byte for byte.
    pub fn unremove_volume(&mut self, removed: &VolumeRemoved) -> Result<()> {
        let block = &mut self.volume_blocks[removed.kind.slot()];
        if removed.index > block.len() {
            return Err(Error::Declined(format!(
                "no {} {}",
                removed.kind.noun(),
                removed.index
            )));
        }
        if removed.index == block.len() {
            block.entries.push(removed.entry.clone());
        } else {
            let moved = std::mem::replace(&mut block.entries[removed.index], removed.entry.clone());
            block.entries.push(moved);
        }
        for &(holder, i, v) in removed.repointed.iter().rev() {
            if let Some(e) = self.volume_blocks[holder.slot()].entries.get_mut(i) {
                volumes::set_named_volume(e, v);
            }
        }
        self.refresh_volumes();
        Ok(())
    }

    /// Why entries of a block may not be removed, if they may not.
    pub fn remove_refusal(&self, kind: BlockKind) -> Option<&'static str> {
        layout::block(kind).remove_refusal
    }

    /// Swap-remove an element: the last element moves into its slot and
    /// the count drops by one. Returns what [`Self::unremove`] needs.
    pub fn remove(&mut self, kind: BlockKind, index: usize) -> Result<Removed> {
        self.remove_with_rev(kind, index, None)
    }

    pub(crate) fn remove_with_rev(
        &mut self,
        kind: BlockKind,
        index: usize,
        replay: Option<LayoutChange>,
    ) -> Result<Removed> {
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
        let id = block.ids.swap_remove(index);
        let layout = block.change_layout(replay);
        let before_appends = std::mem::take(&mut block.before_appends);
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
            id,
            renamed,
            layout,
            before_appends,
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
        // unremove can also be applied to a different layout outside History.
        let id = if block.layout_rev != removed.layout.after && block.ids.contains(&removed.id) {
            PlacementId::Added(next_layout_rev())
        } else {
            removed.id
        };
        if removed.index == block.len() {
            block.ids.push(id);
            block.entries.push(removed.entry.clone());
        } else {
            let last_id = std::mem::replace(&mut block.ids[removed.index], id);
            block.ids.push(last_id);
            let moved = std::mem::replace(&mut block.entries[removed.index], removed.entry.clone());
            block.entries.push(moved);
        }
        if block.layout_rev == removed.layout.after {
            block.layout_rev = removed.layout.before;
            block.before_appends.clone_from(&removed.before_appends);
        } else {
            // Applied to another layout, this insertion creates new slots.
            block.layout_rev = next_layout_rev();
            block.before_appends.clear();
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
        self.add_with_rev(kind, entry, None).map(|(index, _)| index)
    }

    pub(crate) fn add_with_rev(
        &mut self,
        kind: BlockKind,
        entry: Vec<u8>,
        replay: Option<LayoutChange>,
    ) -> Result<(usize, LayoutChange)> {
        if let Some(why) = self.add_refusal(kind) {
            return Err(Error::Refused(why));
        }
        let block = self.block_mut(kind);
        if entry.len() != block.def.stride as usize {
            return Err(Error::Invalid("entry is not one element long"));
        }
        block.entries.push(entry);
        let layout = block.change_layout(replay);
        block.ids.push(PlacementId::Added(layout.after));
        block.before_appends.push(layout.before);
        Ok((block.len() - 1, layout))
    }

    /// Undo [`Self::add`]: drop the last element, which must be `entry`.
    pub fn unadd(&mut self, kind: BlockKind, entry: &[u8]) -> Result<()> {
        let block = self.block_mut(kind);
        if block.entries.last().map(Vec::as_slice) != Some(entry) {
            return Err(Error::Invalid("the added entry is no longer last"));
        }
        block.entries.pop();
        block.ids.pop();
        block.layout_rev = block.before_appends.pop().unwrap_or_else(next_layout_rev);
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
