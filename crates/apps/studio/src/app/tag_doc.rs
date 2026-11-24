//! The tag editor's document, with no UI toolkit in sight: the open map's
//! tags, ordinary metadata and BSP regions, the field edits, their undo
//! stack, block element add / remove / duplicate, and what may not be
//! written and why.
//!
//! Edits are an overlay on the source's metadata and BSP region bytes.
//! The [`App`] hands each changed run to the scenario session for the dirty
//! mark and the lossless `MapEditor` save path shared by maps and packages.
//!
//! Layouts come from [`tags::registry`]: every byte of every root and
//! block element as a labelled field.
//!
//! A block in ordinary metadata grows past its source capacity while
//! growth is allowed (the session's switch, [`TagDoc::allow_growth`]):
//! its elements are copied to a new place past the end of the metadata
//! (provisional addresses the document alone uses) and its pair pointed
//! there. On save each such block is appended to the metadata
//! (`MapEditor::append_metadata`) and its pair pointed at the real
//! address ([`TagDoc::grown`], [`scenario_edit::TagGrown`]). Its old
//! elements stay as dead bytes; its elements' own child blocks keep
//! pointing where they did.
//!
//! A data reference changes length the same way ([`TagDoc::resize_data`]):
//! a shorter payload is written in place with its size patched (the tail
//! stays as dead bytes); a longer one is copied past the end of the
//! metadata, grown like a block of one-byte elements, and saved with
//! `MapEditor::append_metadata`. See docs/editor/rich-editors.md, "Data
//! that changes length".
//!
//! Every write, block change, resize, string add, undo and redo reports a
//! [`Change`] with its class (Field, or Shape where the layout or the
//! strings change), which the fan-out takes each frame
//! ([`TagDoc::take_changes`], docs/editor/live-editors.md, "One write,
//! three deliveries").
//!
//! [`App`]: crate::App
use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Seek};
use std::sync::Arc;

use map::{AddressSpace, FourCC, Map, TagId};
use scenario_edit::{
    BspPatch,
    layout::{BLOCKS, BlockDef},
};
use tags::registry::{Kind, Layout, Registry};

use super::live::{Change, Class};

/// Why a tag's bytes can be read but not written.
pub const SCENARIO_OWNED: &str = "the scenario tag's bytes belong to the Scenario tool (edit \
     placements there); shown as in the source";
/// Why a tag has no fields here.
pub const NO_DATA: &str = "no data in this map: the tag lives in a shared map";
pub const NO_LAYOUT: &str = "no layout for this group in the registry";
pub const TOO_SHORT: &str = "shorter than its group's root layout";
/// Why a block may not grow past the elements it had in the source.
pub const GROWTH_REFUSAL: &str = "the block is full: it has as many elements as in the source. \
     Growing it appends a copy to the map's metadata, and the engine's headroom for more tag \
     memory is not established (docs/editor/scenario-editor.md §6): turn on Edit > Allow \
     growing full blocks to do it";
/// Why a data reference may not grow past the length it had in the source.
pub const DATA_GROWTH_REFUSAL: &str = "the data is longer than it was in the source. Growing it \
     appends a copy to the map's metadata, and the engine's headroom for more tag memory is not \
     established (docs/editor/scenario-editor.md §6): turn on Edit > Allow growing full blocks \
     to do it";
/// Why a data reference in a BSP region does not change length.
pub const DATA_BSP_REFUSAL: &str =
    "a data reference in a structure BSP region cannot change length";
/// Why a data reference inside a grown block's elements does not grow.
pub const DATA_NESTED_GROWTH: &str = "the data lies in elements that were grown and not yet \
     saved: save, reopen the map, then grow it";
/// Why a BSP block may not grow past its original allocation.
pub const BSP_GROWTH_REFUSAL: &str =
    "the block is full: blocks in a structure BSP region cannot grow past their source capacity";
/// Why a block inside a grown block's elements does not grow.
pub const NESTED_GROWTH: &str = "the block lies in elements that were grown and not yet saved: \
     save, reopen the map, then grow it";
/// Why an element with child arrays is not duplicated.
pub const SHARED_CHILDREN: &str = "the element has nonempty child blocks or data: a copy would \
     share those arrays with it";
/// Why a structural field is shown only: a count, address or resource
/// pointer, which by hand would corrupt the map.
pub const STRUCTURAL: &str = "Set by the editor. Add or remove list items to change it.";

/// What may be done with a tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Access {
    /// Fields shown and written.
    Edit,
    /// Fields shown, not written: why.
    Read(&'static str),
    /// Nothing to show: why.
    None(&'static str),
}

/// One tag of the open map.
#[derive(Clone, Debug)]
pub struct TagEntry {
    pub id: TagId,
    pub group: FourCC,
    /// Its path, or `#<index>` when the map has none.
    pub name: String,
    /// Payload virtual address and size (zero without data).
    pub address: u32,
    pub size: u32,
    pub access: Access,
    /// The separate region that owns this BSP or lightmap payload.
    pub bsp: Option<BspTag>,
    /// For a tag with no data here: the shared map that has a tag of the
    /// same group and name with data (`shared`, `single_player_shared`,
    /// `mainmenu`), when one was found ([`TagDoc::set_elsewhere`]).
    pub elsewhere: Option<String>,
}

#[derive(Clone, Copy, Debug)]
pub struct BspTag {
    /// Index in the scenario's structure BSP block, when available.
    pub index: Option<usize>,
    pub space: AddressSpace,
}

/// One write of a field's bytes (one undo step, or merged into the last by
/// its key: every frame of one drag).
#[derive(Clone, Debug, PartialEq)]
pub struct TagWrite {
    pub tag: TagId,
    /// Virtual address in the tag's own region.
    pub address: u32,
    pub bytes: Vec<u8>,
    /// For the undo step: the field's name.
    pub label: String,
    pub merge: Option<u64>,
}

/// A change to a block's elements.
#[derive(Clone, Debug, PartialEq)]
pub enum BlockOp {
    /// Append a blank element.
    Add,
    /// Insert a copy of element `i` after it.
    Duplicate(u32),
    /// Remove element `i`; later elements move down one.
    Remove(u32),
}

/// The tag editor's commands (through [`crate::Action::Tag`]).
#[derive(Clone, Debug, PartialEq)]
pub enum TagAction {
    /// Show a tag's fields.
    Select(TagId),
    Write(TagWrite),
    /// Change the elements of the block whose `(count, address)` pair is
    /// at `pair`, with `layout` its element.
    Block {
        tag: TagId,
        pair: u32,
        layout: Arc<Layout>,
        op: BlockOp,
    },
    /// Set the string ID field at `address` to `text`, adding the string
    /// to the map when it has none (see [`TagDoc::set_string`]).
    SetString {
        tag: TagId,
        address: u32,
        text: String,
        label: String,
    },
    /// Give the data reference at field `field` of the element at
    /// `element` (laid out as `layout`) the payload `bytes`, which may
    /// have another length (see [`TagDoc::resize_data`]).
    ResizeData {
        tag: TagId,
        element: u32,
        layout: Arc<Layout>,
        field: u32,
        bytes: Vec<u8>,
        label: String,
    },
    /// End the current merge run.
    Seal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Region {
    Metadata,
    Bsp(AddressSpace),
}

impl TagEntry {
    fn region(&self) -> Region {
        self.bsp
            .map_or(Region::Metadata, |bsp| Region::Bsp(bsp.space))
    }
}

/// A block a block operation changed, and whether a running game can be
/// given the result by an `edit.batch` move (docs/editor/realtime-edits.md,
/// "The editor"): the tag is a live one, the pair lies in the
/// loaded metadata, and the operation shifts no element another block may
/// index (an append, or the removal or copy of the last element).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockEdit {
    pub tag: TagId,
    /// The `(count, address)` pair's address.
    pub pair: u32,
    /// Bytes per element.
    pub size: u32,
    pub live: bool,
}

#[derive(Clone, Debug, Default)]
struct Step {
    label: String,
    /// `((region, address), before, after)`.
    writes: Vec<((Region, u32), Vec<u8>, Vec<u8>)>,
    merge: Option<u64>,
    reload: Vec<(String, Vec<(u32, u32)>)>,
    /// String IDs this step added (the last of the document's).
    strings: Vec<String>,
    /// The step changed a block's count or a data reference's length.
    shape: bool,
    /// The tag the step edits: its document's undo stack
    /// ([`TagDoc::undo_tag`]). `None` for a step over several tags (an
    /// import squashed into one).
    tag: Option<TagId>,
    /// The block a block operation changed ([`TagDoc::live_blocks`]).
    blocks: Vec<BlockEdit>,
}

impl Step {
    /// The fan-out's class of the step: Shape when it changed a layout or
    /// added strings, else Field.
    fn class(&self) -> Class {
        if self.shape || !self.reload.is_empty() || !self.strings.is_empty() {
            Class::Shape
        } else {
            Class::Field
        }
    }

    fn ranges(&self) -> Vec<(u32, u32)> {
        self.writes
            .iter()
            .map(|((_, a), _, after)| (*a, after.len() as u32))
            .collect()
    }
}

/// Two steps that may not be undone or redone out of order: one writes
/// bytes the other writes, or both add strings (string IDs are numbered
/// in the order they were added).
fn conflicts(a: &Step, b: &Step) -> bool {
    if !a.strings.is_empty() && !b.strings.is_empty() {
        return true;
    }
    a.writes.iter().any(|((ra, aa), _, wa)| {
        b.writes.iter().any(|((rb, ab), _, wb)| {
            let (a0, a1) = (u64::from(*aa), u64::from(*aa) + wa.len() as u64);
            let (b0, b1) = (u64::from(*ab), u64::from(*ab) + wb.len() as u64);
            ra == rb && a0 < b1 && b0 < a1
        })
    })
}

/// The open map's tags and the edits over them.
struct Loaded {
    tags: Vec<TagEntry>,
    by_id: HashMap<TagId, usize>,
    /// Group → (parent, grandparent), for reference filters.
    parents: HashMap<FourCC, [Option<FourCC>; 2]>,
    strings: Vec<Option<String>>,
    string_ids: HashMap<String, u32>,
    /// String IDs added, after the map's own.
    new_strings: Vec<String>,
    /// Virtual address of `meta[0]`.
    base: u32,
    /// The source's ordinary metadata.
    meta: Vec<u8>,
    /// Source BSP regions, keyed by file offset. Virtual addresses can
    /// repeat in different regions.
    bsp_regions: HashMap<u64, Vec<u8>>,
    /// Changed bytes by region and address.
    overlay: BTreeMap<Region, BTreeMap<u32, u8>>,
    /// The scenario payload `[lo, hi)`.
    scenario: Option<(u32, u32)>,
    /// Provisional address of grown elements (past the metadata's end,
    /// aligned); `ext` zero bytes of room there so far.
    ext_base: u32,
    ext: Vec<u8>,
    /// Pairs of the source that were grown at some point: pair → element
    /// size. Exported while they point into `ext`.
    grown: BTreeMap<u32, u32>,
    /// Grown allocations: provisional address → elements they hold.
    allocs: BTreeMap<u32, u32>,
}

impl Loaded {
    fn src_range(&self, region: Region, address: u32, len: u32) -> Option<&[u8]> {
        if let Region::Bsp(space) = region {
            let offset = space.translate(address, len).ok()? - space.file_offset;
            let start = offset as usize;
            return self
                .bsp_regions
                .get(&space.file_offset)?
                .get(start..start + len as usize);
        }
        let (buf, start) = if address >= self.ext_base {
            (&self.ext, self.ext_base)
        } else {
            (&self.meta, self.base)
        };
        let at = address.checked_sub(start)? as usize;
        buf.get(at..at.checked_add(len as usize)?)
    }
}

/// The tag editor's document.
pub struct TagDoc {
    registry: &'static Registry,
    /// Datums resident in the checksum-matched game (including shared tags).
    pub(crate) live_datums: Option<Arc<std::collections::HashSet<TagId>>>,
    loaded: Option<Loaded>,
    /// The tag shown.
    pub selected: Option<TagId>,
    undo: Vec<Step>,
    redo: Vec<Step>,
    sealed: bool,
    /// Bumped on every change of the bytes.
    rev: u64,
    /// The session's map revision this was loaded for.
    pub(crate) for_rev: Option<u64>,
    /// Why the last load failed.
    pub error: Option<String>,
    /// Whether a full block may grow (the session's switch).
    pub allow_growth: bool,
    /// Bumped whenever block counts or addresses may have changed (load,
    /// block changes, undo, redo): what [`Self::homes_cache`] is keyed by.
    shape_rev: u64,
    /// Each tag's raw pointers by map, as counted for `shape_rev`
    /// (`shared_refs::TagDoc::homes`).
    pub(super) homes_cache:
        std::cell::RefCell<(u64, HashMap<TagId, Option<super::shared_refs::Homes>>)>,
    /// The reverse reference index over the map's tags, as built for
    /// `rev` ([`Self::reverse_index`], read by `tag_api::uses`).
    pub(super) uses_cache: std::cell::RefCell<(u64, std::sync::Arc<super::tag_api::UsesIndex>)>,
    /// Writes not yet taken by the fan-out ([`Self::take_changes`]).
    changes: Vec<Change>,
}

impl Default for TagDoc {
    fn default() -> Self {
        Self::new(Registry::builtin())
    }
}

fn u32_at(b: &[u8]) -> u32 {
    u32::from_le_bytes(b[..4].try_into().unwrap())
}

fn changed_runs<'a>(bytes: impl IntoIterator<Item = (&'a u32, &'a u8)>) -> Vec<(u32, Vec<u8>)> {
    let mut out: Vec<(u32, Vec<u8>)> = Vec::new();
    for (&address, &byte) in bytes {
        match out.last_mut() {
            Some((start, run)) if address - *start == run.len() as u32 => run.push(byte),
            _ => out.push((address, vec![byte])),
        }
    }
    out
}

impl TagDoc {
    pub fn new(registry: &'static Registry) -> Self {
        Self {
            registry,
            live_datums: None,
            loaded: None,
            selected: None,
            undo: Vec::new(),
            redo: Vec::new(),
            sealed: true,
            rev: 0,
            for_rev: None,
            error: None,
            allow_growth: false,
            shape_rev: 0,
            homes_cache: Default::default(),
            uses_cache: Default::default(),
            changes: Vec::new(),
        }
    }

    /// The writes since the last call, oldest first, each with its class.
    pub fn take_changes(&mut self) -> Vec<Change> {
        std::mem::take(&mut self.changes)
    }

    /// Reports a write of `ranges` to `tag`. Consecutive writes of one
    /// tag, class and label (every frame of a drag) are one change, and
    /// a document nobody drains keeps the last [`super::live::KEEP`].
    fn changed(
        &mut self,
        class: Class,
        tag: Option<TagId>,
        ranges: Vec<(u32, u32)>,
        label: &str,
    ) {
        if let Some(last) = self.changes.last_mut()
            && last.class == class
            && last.tag == tag
            && last.label == label
        {
            for r in ranges {
                if !last.ranges.contains(&r) {
                    last.ranges.push(r);
                }
            }
            return;
        }
        if self.changes.len() >= super::live::KEEP {
            self.changes.remove(0);
        }
        self.changes.push(Change::new(class, tag, ranges, label));
    }

    /// Reports an undone or redone step's writes, in its class. The tag is
    /// the one owning its last write (a block's pair, in its tag's root
    /// or elements; a grown block's copy lies past the metadata).
    fn changed_step(&mut self, step: &Step) {
        let tag = step.writes.iter().rev().find_map(|(at, _, _)| self.owner(*at));
        self.changed(step.class(), tag, step.ranges(), &step.label);
    }

    /// Reports a new step of `tag`'s, in its class.
    fn changed_tag_step(&mut self, tag: TagId, step: &Step) {
        self.changed(step.class(), Some(tag), step.ranges(), &step.label);
    }

    /// The loaded tag whose payload holds `address` in `region`.
    fn owner(&self, (region, address): (Region, u32)) -> Option<TagId> {
        self.tags()
            .iter()
            .find(|t| {
                t.region() == region && address.checked_sub(t.address).is_some_and(|o| o < t.size)
            })
            .map(|t| t.id)
    }

    pub fn registry(&self) -> &'static Registry {
        self.registry
    }

    /// Read `map`'s tags, ordinary metadata and BSP regions. Drops every edit.
    pub fn load<R: Read + Seek>(&mut self, map: &mut Map<R>) -> map::Result<()> {
        self.close();
        let space = map.metadata_space();
        let meta = match space {
            Some(s) => map.read_file_range(s.file_offset, u64::from(s.size))?,
            None => Vec::new(),
        };
        let scenario = map
            .scenario()
            .and_then(|t| t.location)
            .map(|l| (l.address, l.address.saturating_add(l.size)));
        let bsp_indices: HashMap<TagId, usize> =
            scenario_edit::geometry::scenario_bsp_lightmaps(map)
                .unwrap_or_default()
                .into_iter()
                .enumerate()
                .flat_map(|(i, (bsp, lightmap))| {
                    std::iter::once((bsp, i)).chain(lightmap.map(|id| (id, i)))
                })
                .collect();
        let mut bsp_sizes = HashMap::<u64, u32>::new();
        let mut tags = Vec::new();
        for t in map.tags() {
            let bsp = t.location.filter(|l| Some(l.space) != space).map(|l| {
                bsp_sizes
                    .entry(l.space.file_offset)
                    .and_modify(|size| *size = (*size).max(l.space.size))
                    .or_insert(l.space.size);
                BspTag {
                    index: bsp_indices.get(&t.id).copied(),
                    space: l.space,
                }
            });
            let (address, size, access) = match t.location {
                None => (0, 0, Access::None(NO_DATA)),
                Some(l) if l.size == 0 => (l.address, 0, Access::None(NO_DATA)),
                Some(l) if t.group.0 == *b"scnr" => {
                    (l.address, l.size, Access::Read(SCENARIO_OWNED))
                }
                Some(l) => {
                    let access = match self.registry.group(t.group) {
                        None => Access::Read(NO_LAYOUT),
                        Some(r) if l.size < r.size => Access::Read(TOO_SHORT),
                        _ => Access::Edit,
                    };
                    (l.address, l.size, access)
                }
            };
            tags.push(TagEntry {
                id: t.id,
                group: t.group,
                name: t
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("#{}", t.id.index())),
                address,
                size,
                access,
                bsp,
                elsewhere: None,
            });
        }
        let mut bsp_regions = HashMap::new();
        for (offset, size) in bsp_sizes {
            bsp_regions.insert(offset, map.read_file_range(offset, u64::from(size))?);
        }
        let by_id = tags.iter().enumerate().map(|(i, t)| (t.id, i)).collect();
        let parents = map
            .groups()
            .iter()
            .map(|g| (g.class, [g.parent, g.grandparent]))
            .collect();
        let strings = map.strings().to_vec();
        let string_ids = strings
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                let s = s.as_ref()?;
                let len = u32::try_from(s.len()).ok().filter(|l| *l < 256)?;
                Some((s.clone(), i as u32 | len << 24))
            })
            .collect();
        let base = space.map_or(0, |s| s.virtual_base);
        let ext_base = (base as u64 + meta.len() as u64).next_multiple_of(0x1000);
        let ext_base = u32::try_from(ext_base).unwrap_or(u32::MAX);
        self.loaded = Some(Loaded {
            tags,
            by_id,
            parents,
            strings,
            string_ids,
            new_strings: Vec::new(),
            base,
            meta,
            bsp_regions,
            overlay: BTreeMap::new(),
            scenario,
            ext_base,
            ext: Vec::new(),
            grown: BTreeMap::new(),
            allocs: BTreeMap::new(),
        });
        self.rev += 1;
        self.shape_rev += 1;
        Ok(())
    }

    /// Forget the map and every edit.
    pub fn close(&mut self) {
        self.live_datums = None;
        self.loaded = None;
        self.selected = None;
        self.undo.clear();
        self.redo.clear();
        self.sealed = true;
        self.error = None;
        self.rev += 1;
        self.shape_rev += 1;
    }

    pub fn is_open(&self) -> bool {
        self.loaded.is_some()
    }
    pub fn rev(&self) -> u64 {
        self.rev
    }
    /// Changes when block counts or addresses may have (not on field
    /// writes).
    pub fn shape_rev(&self) -> u64 {
        self.shape_rev
    }
    /// The reverse reference index over the map's tags, cached by the
    /// document's revision: what `tag_api::uses` walks up from a tag.
    /// `skip` leaves a tag out of it (the scenario's, whose references
    /// are the placements').
    pub(crate) fn reverse_index(
        &self,
        skip: Option<TagId>,
    ) -> std::sync::Arc<super::tag_api::UsesIndex> {
        let rev = self.rev();
        {
            let cache = self.uses_cache.borrow();
            if cache.0 == rev {
                return cache.1.clone();
            }
        }
        let index = std::sync::Arc::new(super::tag_api::build_uses_index(self, skip));
        *self.uses_cache.borrow_mut() = (rev, index.clone());
        index
    }
    pub fn tags(&self) -> &[TagEntry] {
        self.loaded.as_ref().map_or(&[], |l| &l.tags)
    }
    pub fn tag(&self, id: TagId) -> Option<&TagEntry> {
        let l = self.loaded.as_ref()?;
        l.tags.get(*l.by_id.get(&id)?)
    }
    pub(crate) fn resident(&self, id: TagId) -> bool {
        self.live_datums.as_ref().map_or_else(
            || self.tag(id).is_some_and(|t| t.size != 0),
            |loaded| loaded.contains(&id),
        )
    }

    /// The root layout of a tag's group.
    pub fn layout(&self, id: TagId) -> Option<&Arc<Layout>> {
        let tag = self.tag(id)?;
        let layout = self.registry.group(tag.group)?;
        if (tag.bsp.is_some() || tag.group.0 == *b"scnr") && tag.size < layout.size {
            return None;
        }
        Some(layout)
    }

    /// The Scene block that owns this scenario root pair. A palette jumps
    /// to the block whose placements use it.
    pub(crate) fn scene_block(&self, tag: TagId, pair: u32) -> Option<&'static BlockDef> {
        let entry = self.tag(tag)?;
        if entry.access != Access::Read(SCENARIO_OWNED) {
            return None;
        }
        let offset = pair.checked_sub(entry.address)?;
        BLOCKS.iter().find(|def| {
            def.offset == offset || def.palette.as_ref().is_some_and(|p| p.offset == offset)
        })
    }

    /// Read bytes in a tag's region, with edits applied.
    pub fn read(&self, tag: TagId, address: u32, len: u32) -> Option<Vec<u8>> {
        self.read_in(self.tag(tag)?.region(), address, len)
    }

    pub(crate) fn read_metadata(&self, address: u32, len: u32) -> Option<Vec<u8>> {
        self.read_in(Region::Metadata, address, len)
    }

    fn read_in(&self, region: Region, address: u32, len: u32) -> Option<Vec<u8>> {
        let l = self.loaded.as_ref()?;
        let mut out = l.src_range(region, address, len)?.to_vec();
        if let Some(bytes) = l.overlay.get(&region) {
            for (&a, &b) in bytes
                .range(address..)
                .take_while(|&(&a, _)| a - address < len)
            {
                out[(a - address) as usize] = b;
            }
        }
        Some(out)
    }

    /// Read source bytes in a tag's region (zeros in grown room).
    pub fn read_source(&self, tag: TagId, address: u32, len: u32) -> Option<&[u8]> {
        self.loaded
            .as_ref()?
            .src_range(self.tag(tag)?.region(), address, len)
    }

    pub(crate) fn read_metadata_source(&self, address: u32, len: u32) -> Option<&[u8]> {
        self.loaded
            .as_ref()?
            .src_range(Region::Metadata, address, len)
    }

    /// Whether `address` is a grown block's provisional place in metadata.
    pub fn is_grown_room(&self, address: u32) -> bool {
        self.loaded.as_ref().is_some_and(|l| address >= l.ext_base)
    }

    fn pair_in(&self, region: Region, pair: u32) -> Option<(u32, u32)> {
        let bytes = self.read_in(region, pair, 8)?;
        Some((u32_at(&bytes), u32_at(&bytes[4..])))
    }

    /// A block or data reference's `(count, address)` at `pair`.
    pub fn pair(&self, tag: TagId, pair: u32) -> Option<(u32, u32)> {
        self.pair_in(self.tag(tag)?.region(), pair)
    }

    /// The elements of the block at `pair` if they lie in the tag's region.
    pub fn block(&self, tag: TagId, pair: u32, layout: &Layout) -> Option<(u32, u32)> {
        let (n, a) = self.pair(tag, pair)?;
        if n == 0 {
            return Some((0, a));
        }
        let len = n.checked_mul(layout.size)?;
        self.read_source(tag, a, len).map(|_| (n, a))
    }

    /// Elements the block at `pair` may hold without growing: as many as
    /// it had in the source, when it still points where it did.
    pub fn capacity(&self, tag: TagId, pair: u32) -> u32 {
        let (Some((n, a)), Some(src)) = (self.pair(tag, pair), self.read_source(tag, pair, 8))
        else {
            return 0;
        };
        if self.tag(tag).is_some_and(|t| t.bsp.is_none())
            && let Some(cap) = self.loaded.as_ref().and_then(|l| l.allocs.get(&a))
        {
            return (*cap).max(n);
        }
        let (sn, sa) = (u32_at(src), u32_at(&src[4..]));
        if sa == a { sn.max(n) } else { n }
    }

    /// The element at `path` from the tag's root: per step, the block
    /// field's offset and the element index. Returns its address and
    /// layout.
    pub fn resolve(&self, tag: TagId, path: &[(u32, u32)]) -> Option<(u32, Arc<Layout>)> {
        let mut at = self.tag(tag)?.address;
        let mut layout = self.layout(tag)?.clone();
        for &(field, index) in path {
            let Kind::Block(child) = &layout.field_at(field)?.kind else {
                return None;
            };
            let child = child.clone();
            let (n, a) = self.block(tag, at.checked_add(field)?, &child)?;
            if index >= n {
                return None;
            }
            at = a + index * child.size;
            layout = child;
        }
        Some((at, layout))
    }

    /// Why `tag` may not be written, if it may not.
    pub fn refusal(&self, tag: TagId) -> Option<&'static str> {
        match &self.tag(tag)?.access {
            Access::Edit => None,
            Access::Read(why) | Access::None(why) => Some(why),
        }
    }

    fn check(&self, tag: TagId, address: u32, len: u32) -> Result<Region, String> {
        if let Some(why) = self.refusal(tag) {
            return Err(why.into());
        }
        let region = self.tag(tag).ok_or("no such tag")?.region();
        let l = self.loaded.as_ref().ok_or("no map is open")?;
        if l.src_range(region, address, len).is_none() {
            return Err(format!("{address:#x} is outside the tag's address space"));
        }
        match region {
            Region::Metadata => {
                if let Some((lo, hi)) = l.scenario
                    && address < hi
                    && u64::from(lo) < u64::from(address) + u64::from(len)
                {
                    return Err(SCENARIO_OWNED.into());
                }
            }
            Region::Bsp(space) if address - space.virtual_base < 16 => {
                return Err("the BSP region header is read-only".into());
            }
            Region::Bsp(_) => {}
        }
        Ok(region)
    }

    fn put(&mut self, (region, address): (Region, u32), bytes: &[u8]) {
        let l = self.loaded.as_mut().expect("map open");
        let mut overlay = l.overlay.remove(&region).unwrap_or_default();
        let source = l
            .src_range(region, address, bytes.len() as u32)
            .expect("checked");
        for (k, (&byte, &original)) in bytes.iter().zip(source).enumerate() {
            let a = address + k as u32;
            if byte == original {
                overlay.remove(&a);
            } else {
                overlay.insert(a, byte);
            }
        }
        if !overlay.is_empty() {
            l.overlay.insert(region, overlay);
        }
        self.rev += 1;
    }

    /// Show `bytes` at `address` without an undo step: for a tool that
    /// keeps its own edits and history (the HUD tool) and draws them with
    /// [`crate::fields::panel`]. Outside the metadata, nothing.
    pub fn mirror(&mut self, address: u32, bytes: &[u8]) {
        let len = bytes.len() as u32;
        if self
            .read_metadata(address, len)
            .is_some_and(|now| now != bytes)
        {
            self.put((Region::Metadata, address), bytes);
        }
    }

    /// Write a field, as one undo step (or merged into the last by key).
    pub fn write(&mut self, w: TagWrite) -> Result<(), String> {
        let len = u32::try_from(w.bytes.len()).map_err(|_| "too long")?;
        let region = self.check(w.tag, w.address, len)?;
        let at = (region, w.address);
        let before = self.read_in(region, w.address, len).expect("checked");
        if before == w.bytes {
            return Ok(());
        }
        self.put(at, &w.bytes);
        let label = format!("set {}", w.label);
        self.changed(Class::Field, Some(w.tag), vec![(w.address, len)], &label);
        if let Some(last) = self.undo.last_mut()
            && !self.sealed
            && w.merge.is_some()
            && last.merge == w.merge
            && last.tag == Some(w.tag)
            && last
                .writes
                .first()
                .is_some_and(|(dest, _, _)| dest.0 == region)
        {
            match last
                .writes
                .iter_mut()
                .find(|(a, b, _)| *a == at && b.len() == w.bytes.len())
            {
                Some((_, _, after)) => *after = w.bytes,
                None => last.writes.push((at, before, w.bytes)),
            }
        } else {
            self.undo.push(Step {
                label: format!("set {}", w.label),
                writes: vec![(at, before, w.bytes)],
                merge: w.merge,
                tag: Some(w.tag),
                ..Step::default()
            });
        }
        self.forget_redo_against_last();
        self.sealed = w.merge.is_none();
        Ok(())
    }

    /// Change a block's elements, as one undo step.
    pub fn block_op(
        &mut self,
        tag: TagId,
        pair: u32,
        layout: &Layout,
        op: &BlockOp,
    ) -> Result<(), String> {
        if let Some(why) = self.block_refusal(tag, pair, layout, op) {
            return Err(why.into());
        }
        let (n, a) = self.pair(tag, pair).ok_or("no block here")?;
        let size = layout.size;
        if matches!(op, BlockOp::Add | BlockOp::Duplicate(_)) && n >= self.capacity(tag, pair) {
            return self.grow(tag, pair, layout, op, n, a);
        }
        let elem = |i: u32| a + i * size;
        let mut step = Step {
            label: match op {
                BlockOp::Add => format!("add {}", layout.name),
                BlockOp::Duplicate(i) => format!("duplicate {} {i}", layout.name),
                BlockOp::Remove(i) => format!("remove {} {i}", layout.name),
            },
            shape: true,
            ..Step::default()
        };
        let (from, bytes, count) = match *op {
            BlockOp::Add => (n, layout.blank(), n + 1),
            BlockOp::Duplicate(i) => {
                // Elements i..n move up one; i is copied into i + 1.
                let mut b = self
                    .read(tag, elem(i), (n - i) * size)
                    .ok_or("block unreadable")?;
                let copy = b[..size as usize].to_vec();
                b.splice(0..0, copy);
                (i, b, n + 1)
            }
            BlockOp::Remove(i) => {
                // Elements i+1..n move down one; the last slot keeps its
                // bytes (dead until re-added).
                let b = if i + 1 == n {
                    Vec::new()
                } else {
                    self.read(tag, elem(i + 1), (n - i - 1) * size)
                        .ok_or("block unreadable")?
                };
                (i, b, n - 1)
            }
        };
        let mut writes = Vec::new();
        if !bytes.is_empty() {
            writes.push((elem(from), bytes));
        }
        writes.push((pair, count.to_le_bytes().to_vec()));
        for (at, after) in writes {
            let region = self.check(tag, at, after.len() as u32)?;
            let before = self
                .read_in(region, at, after.len() as u32)
                .expect("checked");
            step.writes.push(((region, at), before, after));
        }
        for (at, _, after) in &step.writes {
            self.put(*at, after);
        }
        step.blocks.push(BlockEdit {
            tag,
            pair,
            size,
            live: self.block_live(tag, pair, op, n),
        });
        if self.tag(tag).is_some_and(|t| t.bsp.is_none()) && !step.blocks[0].live {
            step.reload.push((
                step.label.clone(),
                step.writes
                    .iter()
                    .map(|((_, a), _, b)| (*a, b.len() as u32))
                    .collect(),
            ));
        }
        self.changed_tag_step(tag, &step);
        self.push_step(tag, step);
        self.sealed = true;
        self.shape_rev += 1;
        Ok(())
    }

    /// Whether a running game can be given `op`'s result as a block move:
    /// a live tag's block in the loaded metadata, and an operation that
    /// leaves every element's index where it was (the definitions do not
    /// say which blocks other blocks index, so only an append and the
    /// removal or copy of the last element qualify).
    fn block_live(&self, tag: TagId, pair: u32, op: &BlockOp, n: u32) -> bool {
        self.tag(tag).is_some_and(|t| {
            t.bsp.is_none() && !matches!(&t.group.0, b"sbsp" | b"ltmp" | b"coll" | b"phmo")
        }) && !self.is_grown_room(pair)
            && match *op {
                BlockOp::Add => true,
                BlockOp::Duplicate(i) | BlockOp::Remove(i) => i + 1 == n,
            }
    }

    /// The blocks the undo stack's block operations changed, once each; a
    /// block is live when every operation on it is.
    pub fn live_blocks(&self) -> Vec<BlockEdit> {
        let mut blocks: BTreeMap<(u32, u32), BlockEdit> = BTreeMap::new();
        for b in self.undo.iter().flat_map(|s| &s.blocks) {
            blocks
                .entry((b.tag.0, b.pair))
                .and_modify(|held| held.live &= b.live)
                .or_insert(*b);
        }
        blocks.into_values().collect()
    }

    /// Add or duplicate past the block's room: copy its elements, with the
    /// new one, to new room in `ext` (twice the elements, so the next adds
    /// fit) and point the pair there, as one undo step.
    fn grow(
        &mut self,
        tag: TagId,
        pair: u32,
        layout: &Layout,
        op: &BlockOp,
        n: u32,
        a: u32,
    ) -> Result<(), String> {
        let size = layout.size;
        let mut bytes = if n == 0 {
            Vec::new()
        } else {
            self.read(tag, a, n * size).ok_or("block unreadable")?
        };
        let label = match *op {
            BlockOp::Add => {
                bytes.extend(layout.blank());
                format!("add {} (grown)", layout.name)
            }
            BlockOp::Duplicate(i) => {
                let at = (i * size) as usize;
                let copy = bytes[at..at + size as usize].to_vec();
                bytes.splice(at..at, copy);
                format!("duplicate {} {i} (grown)", layout.name)
            }
            BlockOp::Remove(_) => unreachable!("removal never grows"),
        };
        let cap = (n + 1).max(n * 2).max(4);
        let room = cap.checked_mul(size).ok_or("block too large")?;
        let l = self.loaded.as_mut().ok_or("no map is open")?;
        let at = (l.ext_base as u64 + l.ext.len() as u64).next_multiple_of(16);
        let end = at + u64::from(room);
        let at = u32::try_from(at)
            .ok()
            .filter(|_| end <= u64::from(u32::MAX))
            .ok_or("no address room left to grow into")?;
        l.ext.resize((end - u64::from(l.ext_base)) as usize, 0);
        l.allocs.insert(at, cap);
        l.grown.insert(pair, size);
        let mut pair_bytes = (n + 1).to_le_bytes().to_vec();
        pair_bytes.extend(at.to_le_bytes());
        let live = self.block_live(tag, pair, op, n);
        let mut step = Step {
            label,
            shape: true,
            blocks: vec![BlockEdit {
                tag,
                pair,
                size,
                live,
            }],
            ..Step::default()
        };
        for (to, after) in [(at, bytes), (pair, pair_bytes)] {
            let region = self.check(tag, to, after.len() as u32)?;
            let before = self
                .read_in(region, to, after.len() as u32)
                .expect("checked");
            step.writes.push(((region, to), before, after));
        }
        for (to, _, after) in &step.writes {
            self.put(*to, after);
        }
        if !live {
            step.reload.push((
                step.label.clone(),
                step.writes
                    .iter()
                    .map(|((_, a), _, b)| (*a, b.len() as u32))
                    .collect(),
            ));
        }
        self.changed_tag_step(tag, &step);
        self.push_step(tag, step);
        self.sealed = true;
        self.shape_rev += 1;
        Ok(())
    }

    /// Why `op` may not be done to the block at `pair`, if it may not.
    pub fn block_refusal(
        &self,
        tag: TagId,
        pair: u32,
        layout: &Layout,
        op: &BlockOp,
    ) -> Option<&'static str> {
        if let Some(why) = self.refusal(tag) {
            return Some(why);
        }
        let Some((n, _)) = self.block(tag, pair, layout) else {
            return Some("the block's elements are outside the tag's address space");
        };
        match *op {
            BlockOp::Add | BlockOp::Duplicate(_) if n >= self.capacity(tag, pair) => {
                if self.tag(tag).is_some_and(|t| t.bsp.is_some()) {
                    Some(BSP_GROWTH_REFUSAL)
                } else if !self.allow_growth {
                    Some(GROWTH_REFUSAL)
                } else if self.is_grown_room(pair) {
                    Some(NESTED_GROWTH)
                } else {
                    match *op {
                        BlockOp::Duplicate(i) if i >= n => Some("no such element"),
                        BlockOp::Duplicate(i) if self.has_children(tag, pair, layout, i) => {
                            Some(SHARED_CHILDREN)
                        }
                        _ => None,
                    }
                }
            }
            BlockOp::Duplicate(i) | BlockOp::Remove(i) if i >= n => Some("no such element"),
            BlockOp::Duplicate(i) if self.has_children(tag, pair, layout, i) => {
                Some(SHARED_CHILDREN)
            }
            _ => None,
        }
    }

    /// Why the data reference at `pair` may not be given `len` bytes, if it
    /// may not.
    pub fn data_refusal(&self, tag: TagId, pair: u32, len: u32) -> Option<&'static str> {
        if let Some(why) = self.refusal(tag) {
            return Some(why);
        }
        if self.tag(tag).is_some_and(|t| t.bsp.is_some()) {
            return Some(DATA_BSP_REFUSAL);
        }
        let Some((n, a)) = self.pair(tag, pair) else {
            return Some("the data reference is outside the tag's address space");
        };
        if n > 0 && self.read_source(tag, a, n).is_none() {
            return Some("the data is outside the tag's address space");
        }
        if len > self.capacity(tag, pair) {
            if !self.allow_growth {
                return Some(DATA_GROWTH_REFUSAL);
            }
            if self.is_grown_room(pair) {
                return Some(DATA_NESTED_GROWTH);
            }
        }
        None
    }

    /// Give the data reference at `field` of the element at `element`
    /// (laid out as `layout`) the payload `bytes`, as one undo step.
    ///
    /// A payload no longer than the source's (or than the room an earlier
    /// growth made) is written in place and the size patched; the tail
    /// stays as dead bytes. A longer one is copied to new room past the
    /// metadata's end and the pair pointed there; the save appends it
    /// ([`Self::grown`]). Growth is refused while [`Self::allow_growth`]
    /// is off. That the metadata ends the file, and the cache's size
    /// limit, are checked by the save (`MapEditor::append_metadata`).
    pub fn resize_data(
        &mut self,
        tag: TagId,
        element: u32,
        layout: &Layout,
        field: u32,
        bytes: &[u8],
        label: &str,
    ) -> Result<(), String> {
        let f = layout
            .field_at(field)
            .filter(|f| f.offset == field && matches!(f.kind, Kind::Data))
            .ok_or("no data reference at that field")?;
        let name = f.name.clone();
        let pair = element.checked_add(field).ok_or("address overflow")?;
        let len = u32::try_from(bytes.len()).map_err(|_| "the data is too long")?;
        if let Some(why) = self.data_refusal(tag, pair, len) {
            return Err(why.into());
        }
        let (n, a) = self.pair(tag, pair).ok_or("no data here")?;
        if n == len && (len == 0 || self.read(tag, a, len).as_deref() == Some(bytes)) {
            return Ok(());
        }
        let mut step = Step {
            label: format!(
                "resize {}",
                if label.is_empty() {
                    name.as_str()
                } else {
                    label
                }
            ),
            shape: true,
            ..Step::default()
        };
        let mut writes: Vec<(u32, Vec<u8>)> = Vec::new();
        if len <= self.capacity(tag, pair) {
            if len > 0 {
                writes.push((a, bytes.to_vec()));
            }
            writes.push((pair, len.to_le_bytes().to_vec()));
        } else {
            let cap = len
                .checked_next_multiple_of(16)
                .ok_or("the data is too long")?;
            let l = self.loaded.as_mut().ok_or("no map is open")?;
            let at = (l.ext_base as u64 + l.ext.len() as u64).next_multiple_of(16);
            let end = at + u64::from(cap);
            let at = u32::try_from(at)
                .ok()
                .filter(|_| end <= u64::from(u32::MAX))
                .ok_or("no address room left to grow into")?;
            l.ext.resize((end - u64::from(l.ext_base)) as usize, 0);
            l.allocs.insert(at, cap);
            l.grown.insert(pair, 1);
            step.label.push_str(" (grown)");
            let mut pair_bytes = len.to_le_bytes().to_vec();
            pair_bytes.extend(at.to_le_bytes());
            writes.push((at, bytes.to_vec()));
            writes.push((pair, pair_bytes));
        }
        for (to, after) in writes {
            let region = self.check(tag, to, after.len() as u32)?;
            let before = self
                .read_in(region, to, after.len() as u32)
                .expect("checked");
            step.writes.push(((region, to), before, after));
        }
        for (to, _, after) in &step.writes {
            self.put(*to, after);
        }
        step.reload.push((
            step.label.clone(),
            step.writes
                .iter()
                .map(|((_, a), _, b)| (*a, b.len() as u32))
                .collect(),
        ));
        self.changed_tag_step(tag, &step);
        self.push_step(tag, step);
        self.sealed = true;
        self.shape_rev += 1;
        Ok(())
    }

    fn has_children(&self, tag: TagId, pair: u32, layout: &Layout, i: u32) -> bool {
        let Some((_, a)) = self.pair(tag, pair) else {
            return true;
        };
        let at = a + i * layout.size;
        layout.fields.iter().any(|f| {
            matches!(f.kind, Kind::Block(_) | Kind::Data)
                && self
                    .read(tag, at + f.offset, 4)
                    .is_none_or(|b| u32_at(&b) != 0)
        })
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    /// Undo the last step; returns its label.
    pub fn undo(&mut self) -> Option<String> {
        let last = self.undo.len().checked_sub(1)?;
        Some(self.undo_at(last))
    }

    /// Undo step `i` of the undo stack: the last, or one whose later
    /// steps write none of its bytes ([`Self::undo_tag`]).
    fn undo_at(&mut self, i: usize) -> String {
        let step = self.undo.remove(i);
        for (at, before, _) in step.writes.iter().rev() {
            self.put(*at, before);
        }
        self.changed_step(&step);
        if let Some(l) = self.loaded.as_mut() {
            let keep = l.new_strings.len().saturating_sub(step.strings.len());
            l.new_strings.truncate(keep);
        }
        let label = step.label.clone();
        self.redo.push(step);
        self.sealed = true;
        self.shape_rev += 1;
        label
    }
    pub fn redo(&mut self) -> Option<String> {
        let last = self.redo.len().checked_sub(1)?;
        Some(self.redo_at(last))
    }

    /// Redo step `i` of the redo stack: the last, or one that no step
    /// undone after it writes the same bytes as ([`Self::redo_tag`]).
    fn redo_at(&mut self, i: usize) -> String {
        let step = self.redo.remove(i);
        for (at, _, after) in &step.writes {
            self.put(*at, after);
        }
        self.changed_step(&step);
        if let Some(l) = self.loaded.as_mut() {
            l.new_strings.extend(step.strings.iter().cloned());
        }
        let label = step.label.clone();
        self.undo.push(step);
        self.sealed = true;
        self.shape_rev += 1;
        label
    }

    /// `tag`'s own undo stack holds a step: its document tab can undo.
    pub fn can_undo_tag(&self, tag: TagId) -> bool {
        self.undo.iter().any(|s| s.tag == Some(tag))
    }
    pub fn can_redo_tag(&self, tag: TagId) -> bool {
        self.redo.iter().any(|s| s.tag == Some(tag))
    }

    /// Undo `tag`'s last step, leaving every other tag's edits as they
    /// are: each tag's document has its own undo stack over the map's
    /// one overlay (docs/editor/studio-redesign.md, "The shell"). Refused
    /// while a later step of another tag writes the same bytes, or adds a
    /// string after one this step added: that one is undone first.
    /// `Ok(None)` when the tag has nothing to undo.
    pub fn undo_tag(&mut self, tag: TagId) -> Result<Option<String>, String> {
        let Some(i) = self.undo.iter().rposition(|s| s.tag == Some(tag)) else {
            return Ok(None);
        };
        if let Some(later) = self.undo[i + 1..].iter().find(|l| conflicts(l, &self.undo[i])) {
            return Err(format!(
                "a later edit ({}) writes the same bytes: undo it first",
                later.label
            ));
        }
        Ok(Some(self.undo_at(i)))
    }

    /// Redo `tag`'s last undone step. Refused while a step undone after it
    /// (made before it) writes the same bytes and is still undone.
    pub fn redo_tag(&mut self, tag: TagId) -> Result<Option<String>, String> {
        let Some(i) = self.redo.iter().rposition(|s| s.tag == Some(tag)) else {
            return Ok(None);
        };
        if let Some(earlier) = self.redo[i + 1..].iter().find(|e| conflicts(e, &self.redo[i])) {
            return Err(format!(
                "an earlier edit ({}) writes the same bytes and is undone: redo it first",
                earlier.label
            ));
        }
        Ok(Some(self.redo_at(i)))
    }

    /// Push a new step of `tag`'s and drop the redo steps it makes wrong.
    fn push_step(&mut self, tag: TagId, mut step: Step) {
        step.tag = Some(tag);
        self.undo.push(step);
        self.forget_redo_against_last();
    }

    /// A new edit drops the redo steps of its own tag, and any other's it
    /// conflicts with (the same bytes, or both adding strings); the rest
    /// keep their redo. A step over several tags drops every redo step.
    fn forget_redo_against_last(&mut self) {
        let Some(new) = self.undo.last() else {
            return;
        };
        self.redo.retain(|r| {
            new.tag.is_some() && r.tag.is_some() && r.tag != new.tag && !conflicts(r, new)
        });
    }

    pub fn seal(&mut self) {
        self.sealed = true;
    }

    /// Undo steps so far: a mark for [`Self::rollback`] and
    /// [`Self::squash`].
    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }
    /// Undo every step past `mark` and forget them (an import that failed
    /// part way leaves the document as it was).
    pub fn rollback(&mut self, mark: usize) {
        let undone = self.undo.len() > mark;
        while self.undo.len() > mark {
            self.undo();
        }
        if undone {
            self.redo.clear();
        }
    }
    /// Make the steps past `mark` one undo step named `label` (an import
    /// is one step). Undoing it reverts their writes newest first and
    /// takes back the strings they added.
    pub fn squash(&mut self, mark: usize, label: String) {
        if self.undo.len() <= mark {
            return;
        }
        let steps: Vec<Step> = self.undo.drain(mark..).collect();
        let mut one = Step {
            label,
            ..Step::default()
        };
        one.tag = steps.first().and_then(|s| s.tag);
        for s in steps {
            if s.tag != one.tag {
                one.tag = None;
            }
            one.shape |= s.shape;
            one.reload.extend(s.reload);
            one.writes.extend(s.writes);
            one.strings.extend(s.strings);
        }
        self.undo.push(one);
        self.sealed = true;
    }

    /// Show every tag read-only, saying `why` (a map browsed in the Tags
    /// tool that the scenario session does not hold, so nothing saves its
    /// edits).
    pub fn read_only(&mut self, why: &'static str) {
        if let Some(l) = self.loaded.as_mut() {
            for t in &mut l.tags {
                if t.access == Access::Edit {
                    t.access = Access::Read(why);
                }
            }
        }
        self.rev += 1;
    }
    /// Record that tag `id`, which has no data here, has data in shared
    /// map `map`.
    pub fn set_elsewhere(&mut self, id: TagId, map: &str) {
        let Some(l) = self.loaded.as_mut() else {
            return;
        };
        if let Some(&i) = l.by_id.get(&id) {
            l.tags[i].elsewhere = Some(map.to_owned());
        }
    }

    pub(crate) fn structural_edits(&self) -> impl Iterator<Item = &(String, Vec<(u32, u32)>)> {
        self.undo.iter().flat_map(|s| &s.reload)
    }

    /// Every changed run in the source's metadata: `(address, bytes)`,
    /// ascending. A grown block's elements and its pair are not here: see
    /// [`Self::grown`]. A grown block emptied again by removals has its
    /// pair written as `(0, 0)`.
    pub fn patches(&self) -> Vec<(u32, Vec<u8>)> {
        let Some(l) = self.loaded.as_ref() else {
            return Vec::new();
        };
        // Pairs pointing into grown room: written at save, not patched.
        let mut skip: BTreeMap<u32, Option<[u8; 8]>> = BTreeMap::new();
        for &p in l.grown.keys() {
            if let Some((n, a)) = self.pair_in(Region::Metadata, p)
                && a >= l.ext_base
            {
                skip.insert(p, (n == 0).then_some([0; 8]));
            }
        }
        let mut bytes = l
            .overlay
            .get(&Region::Metadata)
            .cloned()
            .unwrap_or_default();
        for (p, empty) in &skip {
            for k in 0..8u32 {
                bytes.remove(&(p + k));
                if let Some(e) = empty
                    && l.src_range(Region::Metadata, p + k, 1)
                        .and_then(|b| b.first())
                        != Some(&e[k as usize])
                {
                    bytes.insert(p + k, e[k as usize]);
                }
            }
        }
        changed_runs(bytes.range(..l.ext_base))
    }

    /// Fixed-length edits in structure BSP regions, ordered by region and address.
    pub fn bsp_patches(&self) -> Vec<BspPatch> {
        let Some(l) = self.loaded.as_ref() else {
            return Vec::new();
        };
        let mut patches = Vec::new();
        for (&region, bytes) in &l.overlay {
            if let Region::Bsp(space) = region {
                patches.extend(
                    changed_runs(bytes)
                        .into_iter()
                        .map(|(address, bytes)| BspPatch {
                            space,
                            address,
                            bytes,
                        }),
                );
            }
        }
        patches
    }

    /// The blocks and data references grown past their source room, with
    /// every element (for data, every byte, and `count` the length) as it
    /// stands, for the save to append and repoint.
    pub fn grown(&self) -> Vec<scenario_edit::TagGrown> {
        let Some(l) = self.loaded.as_ref() else {
            return Vec::new();
        };
        l.grown
            .iter()
            .filter_map(|(&root, &size)| {
                let (count, a) = self.pair_in(Region::Metadata, root)?;
                if a < l.ext_base || count == 0 {
                    return None;
                }
                Some(scenario_edit::TagGrown {
                    root,
                    count,
                    bytes: self.read_metadata(a, count * size)?,
                })
            })
            .collect()
    }

    /// Any byte in the tag's own payload differs from the source.
    pub fn is_changed(&self, id: TagId) -> bool {
        let (Some(l), Some(t)) = (self.loaded.as_ref(), self.tag(id)) else {
            return false;
        };
        l.overlay.get(&t.region()).is_some_and(|bytes| {
            bytes
                .range(t.address..)
                .next()
                .is_some_and(|(&a, _)| a - t.address < t.size)
        })
    }
    /// Whether anything differs from the source.
    pub fn has_edits(&self) -> bool {
        self.loaded
            .as_ref()
            .is_some_and(|l| !l.overlay.is_empty() || !l.new_strings.is_empty())
    }

    pub(crate) fn source_string(&self, id: u32) -> bool {
        self.loaded.as_ref().is_some_and(|l| {
            tags::StringId(id)
                .resolve(&l.strings)
                .ok()
                .flatten()
                .is_some()
        })
    }

    /// The string of a string ID's value, if the map has it (or it was
    /// added).
    pub fn string(&self, value: u32) -> Option<&str> {
        let l = self.loaded.as_ref()?;
        if let Some(s) = tags::StringId(value).resolve(&l.strings).ok().flatten() {
            return Some(s);
        }
        let index = (value & 0x00ff_ffff) as usize;
        let s = l.new_strings.get(index.checked_sub(l.strings.len())?)?;
        (s.len() == (value >> 24) as usize).then_some(s.as_str())
    }
    /// The value of the string ID for `text`, if the map has the string
    /// (or it was added).
    pub fn string_id(&self, text: &str) -> Option<u32> {
        let l = self.loaded.as_ref()?;
        if let Some(v) = l.string_ids.get(text) {
            return Some(*v);
        }
        let k = l.new_strings.iter().position(|s| s == text)?;
        Some(((l.strings.len() + k) as u32) | (text.len() as u32) << 24)
    }
    /// The string IDs added, in order, after the map's own.
    pub fn new_strings(&self) -> &[String] {
        self.loaded.as_ref().map_or(&[], |l| &l.new_strings)
    }

    /// Set the string ID field at `address` to `text`, as one undo step.
    /// A string the map lacks is added after its own (saved with the map:
    /// `MapEditor::add_string`); undoing the step takes it back.
    pub fn set_string(
        &mut self,
        tag: TagId,
        address: u32,
        text: &str,
        label: &str,
    ) -> Result<(), String> {
        let (value, new) = match self.string_id(text) {
            Some(v) => (v, false),
            None => {
                if text.is_empty() || text.contains('\0') {
                    return Err("a new string ID needs text, and no NUL".into());
                }
                if text.len() >= map::build::FIXED_STRING_SLOT {
                    return Err(format!(
                        "a string ID is under {} bytes",
                        map::build::FIXED_STRING_SLOT
                    ));
                }
                let l = self.loaded.as_ref().ok_or("no map is open")?;
                let index = l.strings.len() + l.new_strings.len();
                let v = map::build::string_id(index, text).map_err(|e| e.to_string())?;
                (v, true)
            }
        };
        let region = self.check(tag, address, 4)?;
        let at = (region, address);
        let after = value.to_le_bytes().to_vec();
        let before = self.read_in(region, address, 4).expect("checked");
        if before == after && !new {
            return Ok(());
        }
        self.put(at, &after);
        let mut step = Step {
            label: format!("set {label}"),
            writes: vec![(at, before, after)],
            ..Step::default()
        };
        if new {
            step.label = format!("set {label} (new string {text:?})");
            step.strings.push(text.to_owned());
            if let Some(l) = self.loaded.as_mut() {
                l.new_strings.push(text.to_owned());
            }
            self.rev += 1;
        }
        self.changed_tag_step(tag, &step);
        self.push_step(tag, step);
        self.sealed = true;
        Ok(())
    }

    /// Whether a tag of `group` fits a reference allowing `groups` (empty:
    /// any): its group or a parent of it.
    pub fn fits(&self, group: FourCC, groups: &[FourCC]) -> bool {
        if groups.is_empty() || groups.contains(&group) {
            return true;
        }
        let Some(l) = self.loaded.as_ref() else {
            return false;
        };
        l.parents
            .get(&group)
            .is_some_and(|p| p.iter().flatten().any(|g| groups.contains(g)))
    }

    /// `group: name` of a datum, `none` for null.
    pub fn tag_label(&self, id: u32) -> String {
        if id == u32::MAX {
            return "none".into();
        }
        match self.tag(TagId(id)) {
            Some(t) => format!("{}: {}", tags::defs::fourcc_text(t.group), t.name),
            None => format!("missing {id:#010x}"),
        }
    }
}

#[cfg(test)]
mod order_tests {
    use super::*;

    fn step(tag: u32, at: u32, len: usize) -> Step {
        Step {
            label: format!("step of {tag}"),
            writes: vec![((Region::Metadata, at), vec![0; len], vec![1; len])],
            tag: Some(TagId(tag)),
            ..Step::default()
        }
    }

    /// Steps of two tags may be undone out of order only when neither
    /// writes the other's bytes and they do not both add strings.
    #[test]
    fn steps_conflict_on_shared_bytes_or_two_new_strings() {
        assert!(conflicts(&step(1, 0x10, 4), &step(2, 0x12, 4)));
        assert!(!conflicts(&step(1, 0x10, 4), &step(2, 0x14, 4)));
        let space = AddressSpace {
            virtual_base: 0,
            file_offset: 0x100,
            size: 0x100,
        };
        let bsp = Step {
            writes: vec![((Region::Bsp(space), 0x10), vec![0; 4], vec![1; 4])],
            ..step(2, 0x10, 4)
        };
        assert!(!conflicts(&step(1, 0x10, 4), &bsp), "another region");
        let mut a = step(1, 0x10, 4);
        a.strings.push("a".into());
        let mut b = step(2, 0x40, 4);
        b.strings.push("b".into());
        assert!(conflicts(&a, &b));
        assert!(!conflicts(&a, &step(2, 0x40, 4)));
    }

    /// A tag's undo under a later step that writes its bytes is refused,
    /// naming that step; so is a redo over an earlier step still undone.
    #[test]
    fn an_out_of_order_step_over_the_same_bytes_is_refused() {
        let mut doc = TagDoc::default();
        assert_eq!(doc.undo_tag(TagId(1)), Ok(None));
        doc.undo.push(step(1, 0x10, 4));
        doc.undo.push(step(2, 0x12, 4));
        assert!(doc.can_undo_tag(TagId(1)) && doc.can_undo_tag(TagId(2)));
        let e = doc.undo_tag(TagId(1)).unwrap_err();
        assert!(e.contains("step of 2"), "{e}");
        assert_eq!(doc.undo.len(), 2, "nothing undone");

        let mut doc = TagDoc::default();
        // Undone in order: the later step first.
        doc.redo.push(step(2, 0x12, 4));
        doc.redo.push(step(1, 0x10, 4));
        let e = doc.redo_tag(TagId(2)).unwrap_err();
        assert!(e.contains("step of 1"), "{e}");
        assert!(doc.can_redo_tag(TagId(2)) && !doc.can_undo_tag(TagId(2)));
    }
}
