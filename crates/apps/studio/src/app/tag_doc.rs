//! The tag editor's document, with no UI toolkit in sight: the open map's
//! tags and ordinary metadata, the field edits over them, their undo
//! stack, block element add / remove / duplicate, and what may not be
//! written and why.
//!
//! Edits are an overlay on the source's metadata bytes. The [`App`] hands
//! every changed run to the scenario session ([`Session::set_tag_patches`]),
//! so saving a map file or a package writes them through the same
//! lossless `MapEditor` path as the scenario's own edits, and the map's
//! dirty mark counts them.
//!
//! Layouts come from [`tags::registry`]: every byte of every root and
//! block element as a labelled field.
//!
//! A block grows past the elements it had in the source only while
//! growth is allowed (the session's switch, [`TagDoc::allow_growth`]):
//! its elements are copied to a new place past the end of the metadata
//! (provisional addresses the document alone uses) and its pair pointed
//! there. On save each such block is appended to the metadata
//! (`MapEditor::append_metadata`) and its pair pointed at the real
//! address ([`TagDoc::grown`], [`scenario_edit::TagGrown`]). Its old
//! elements stay as dead bytes; its elements' own child blocks keep
//! pointing where they did.
//!
//! [`App`]: crate::App
//! [`Session::set_tag_patches`]: scenario_edit::Session::set_tag_patches
use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Seek};
use std::sync::Arc;

use map::{FourCC, Map, TagId};
use tags::registry::{Kind, Layout, Registry};

/// Why a tag's bytes can be read but not written.
pub const SCENARIO_OWNED: &str = "the scenario tag's bytes belong to the Scenario tool (edit \
     placements there); shown as in the source";
/// Why a tag has no fields here.
pub const NO_DATA: &str = "no data in this map: the tag lives in a shared map";
pub const IN_BSP: &str = "lives in a structure BSP region, which the tag editor does not read yet";
pub const NO_LAYOUT: &str = "no layout for this group in the registry";
pub const TOO_SHORT: &str = "shorter than its group's root layout";
/// Why a block may not grow past the elements it had in the source.
pub const GROWTH_REFUSAL: &str = "the block is full: it has as many elements as in the source. \
     Growing it appends a copy to the map's metadata, and the engine's headroom for more tag \
     memory is not established (docs/editor/scenario-editor.md §6): turn on Edit > Allow \
     growing full blocks to do it";
/// Why a block inside a grown block's elements does not grow.
pub const NESTED_GROWTH: &str = "the block lies in elements that were grown and not yet saved: \
     save, reopen the map, then grow it";
/// Why an element with child arrays is not duplicated.
pub const SHARED_CHILDREN: &str = "the element has nonempty child blocks or data: a copy would \
     share those arrays with it";
/// Why a structural field is shown only.
pub const STRUCTURAL: &str = "a count, address or resource pointer: changing it by hand would \
     corrupt the map (use the block's add / remove / duplicate)";

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
    /// For a tag with no data here: the shared map that has a tag of the
    /// same group and name with data (`shared`, `single_player_shared`,
    /// `mainmenu`), when one was found ([`TagDoc::set_elsewhere`]).
    pub elsewhere: Option<String>,
}

/// One write of a field's bytes (one undo step, or merged into the last by
/// its key: every frame of one drag).
#[derive(Clone, Debug, PartialEq)]
pub struct TagWrite {
    pub tag: TagId,
    /// Ordinary metadata virtual address.
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
    /// End the current merge run.
    Seal,
}

#[derive(Clone, Debug, Default)]
struct Step {
    label: String,
    /// `(address, before, after)`.
    writes: Vec<(u32, Vec<u8>, Vec<u8>)>,
    merge: Option<u64>,
    /// String IDs this step added (the last of the document's).
    strings: Vec<String>,
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
    /// Changed bytes by address (only those that differ from `meta`).
    overlay: BTreeMap<u32, u8>,
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
    /// The source's byte at `a` (zero in the grown room).
    fn src(&self, a: u32) -> Option<u8> {
        if a >= self.ext_base {
            return self.ext.get((a - self.ext_base) as usize).copied();
        }
        self.meta.get(a.checked_sub(self.base)? as usize).copied()
    }
    fn src_range(&self, address: u32, len: u32) -> Option<&[u8]> {
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
    pub(super) homes_cache: std::cell::RefCell<(u64, HashMap<TagId, Option<super::shared_refs::Homes>>)>,
}

impl Default for TagDoc {
    fn default() -> Self {
        Self::new(Registry::builtin())
    }
}

fn u32_at(b: &[u8]) -> u32 {
    u32::from_le_bytes(b[..4].try_into().unwrap())
}

impl TagDoc {
    pub fn new(registry: &'static Registry) -> Self {
        Self {
            registry,
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
        }
    }

    pub fn registry(&self) -> &'static Registry {
        self.registry
    }

    /// Read `map`'s tags and ordinary metadata; drops every edit.
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
        let mut tags = Vec::new();
        for t in map.tags() {
            let (address, size, access) = match t.location {
                None => (0, 0, Access::None(NO_DATA)),
                Some(l) if l.size == 0 => (l.address, 0, Access::None(NO_DATA)),
                Some(l) if Some(l.space) != space => (l.address, l.size, Access::None(IN_BSP)),
                Some(l) => {
                    let access = match self.registry.group(t.group) {
                        None => Access::Read(NO_LAYOUT),
                        Some(r) if l.size < r.size => Access::Read(TOO_SHORT),
                        _ if Some(t.id) == map.scenario().map(|s| s.id) => {
                            Access::Read(SCENARIO_OWNED)
                        }
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
                elsewhere: None,
            });
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
    pub fn tags(&self) -> &[TagEntry] {
        self.loaded.as_ref().map_or(&[], |l| &l.tags)
    }
    pub fn tag(&self, id: TagId) -> Option<&TagEntry> {
        let l = self.loaded.as_ref()?;
        l.tags.get(*l.by_id.get(&id)?)
    }
    /// The root layout of a tag's group.
    pub fn layout(&self, id: TagId) -> Option<&Arc<Layout>> {
        self.registry.group(self.tag(id)?.group)
    }

    /// `len` bytes at `address`, edits applied; `None` outside the
    /// metadata.
    pub fn read(&self, address: u32, len: u32) -> Option<Vec<u8>> {
        let l = self.loaded.as_ref()?;
        let mut out = l.src_range(address, len)?.to_vec();
        for (a, b) in l.overlay.range(address..address + len) {
            out[(a - address) as usize] = *b;
        }
        Some(out)
    }
    /// The source's bytes, no edits (zeros in grown room).
    pub fn read_source(&self, address: u32, len: u32) -> Option<&[u8]> {
        self.loaded.as_ref()?.src_range(address, len)
    }
    /// Whether `address` is a grown block's provisional place.
    pub fn is_grown_room(&self, address: u32) -> bool {
        self.loaded.as_ref().is_some_and(|l| address >= l.ext_base)
    }
    fn read_u32(&self, address: u32) -> Option<u32> {
        self.read(address, 4).map(|b| u32_at(&b))
    }

    /// A block or data reference's `(count, address)` at `pair`.
    pub fn pair(&self, pair: u32) -> Option<(u32, u32)> {
        Some((self.read_u32(pair)?, self.read_u32(pair + 4)?))
    }

    /// The elements of the block at `pair` if they lie in the metadata.
    pub fn block(&self, pair: u32, layout: &Layout) -> Option<(u32, u32)> {
        let (n, a) = self.pair(pair)?;
        if n == 0 {
            return Some((0, a));
        }
        let len = n.checked_mul(layout.size)?;
        self.read_source(a, len).map(|_| (n, a))
    }

    /// Elements the block at `pair` may hold without growing: as many as
    /// it had in the source, when it still points where it did.
    pub fn capacity(&self, pair: u32) -> u32 {
        let (Some((n, a)), Some(src)) = (self.pair(pair), self.read_source(pair, 8)) else {
            return 0;
        };
        if let Some(cap) = self.loaded.as_ref().and_then(|l| l.allocs.get(&a)) {
            return (*cap).max(n);
        }
        let (sn, sa) = (u32_at(&src[..4]), u32_at(&src[4..]));
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
            let (n, a) = self.block(at + field, &child)?;
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

    fn check(&self, tag: TagId, address: u32, len: u32) -> Result<(), String> {
        if let Some(why) = self.refusal(tag) {
            return Err(why.into());
        }
        if self.tag(tag).is_none() {
            return Err("no such tag".into());
        }
        let l = self.loaded.as_ref().ok_or("no map is open")?;
        if self.read_source(address, len).is_none() {
            return Err(format!("{address:#x} is outside the map's metadata"));
        }
        if let Some((lo, hi)) = l.scenario
            && address < hi
            && lo < address + len
        {
            return Err(SCENARIO_OWNED.into());
        }
        Ok(())
    }

    fn put(&mut self, address: u32, bytes: &[u8]) {
        let Some(l) = self.loaded.as_mut() else {
            return;
        };
        for (k, b) in bytes.iter().enumerate() {
            let a = address + k as u32;
            if l.src(a) == Some(*b) {
                l.overlay.remove(&a);
            } else {
                l.overlay.insert(a, *b);
            }
        }
        self.rev += 1;
    }

    /// Show `bytes` at `address` without an undo step: for a tool that
    /// keeps its own edits and history (the HUD tool) and draws them with
    /// [`crate::fields::panel`]. Outside the metadata, nothing.
    pub fn mirror(&mut self, address: u32, bytes: &[u8]) {
        let len = bytes.len() as u32;
        if self.read(address, len).is_some_and(|now| now != bytes) {
            self.put(address, bytes);
        }
    }

    /// Write a field, as one undo step (or merged into the last by key).
    pub fn write(&mut self, w: TagWrite) -> Result<(), String> {
        let len = u32::try_from(w.bytes.len()).map_err(|_| "too long")?;
        self.check(w.tag, w.address, len)?;
        let before = self.read(w.address, len).expect("checked");
        if before == w.bytes {
            return Ok(());
        }
        self.put(w.address, &w.bytes);
        self.redo.clear();
        if let Some(last) = self.undo.last_mut()
            && !self.sealed
            && w.merge.is_some()
            && last.merge == w.merge
        {
            match last
                .writes
                .iter_mut()
                .find(|(a, b, _)| *a == w.address && b.len() == w.bytes.len())
            {
                Some((_, _, after)) => *after = w.bytes,
                None => last.writes.push((w.address, before, w.bytes)),
            }
        } else {
            self.undo.push(Step {
                label: format!("set {}", w.label),
                writes: vec![(w.address, before, w.bytes)],
                merge: w.merge,
                ..Step::default()
            });
        }
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
        let (n, a) = self.pair(pair).ok_or("no block here")?;
        let size = layout.size;
        if matches!(op, BlockOp::Add | BlockOp::Duplicate(_)) && n >= self.capacity(pair) {
            return self.grow(tag, pair, layout, op, n, a);
        }
        let elem = |i: u32| a + i * size;
        let mut step = Step {
            label: match op {
                BlockOp::Add => format!("add {}", layout.name),
                BlockOp::Duplicate(i) => format!("duplicate {} {i}", layout.name),
                BlockOp::Remove(i) => format!("remove {} {i}", layout.name),
            },
            ..Step::default()
        };
        let (from, bytes, count) = match *op {
            BlockOp::Add => (n, layout.blank(), n + 1),
            BlockOp::Duplicate(i) => {
                // Elements i..n move up one; i is copied into i + 1.
                let mut b = self
                    .read(elem(i), (n - i) * size)
                    .ok_or("block unreadable")?;
                let copy = b[..size as usize].to_vec();
                b.splice(0..0, copy);
                (i, b, n + 1)
            }
            BlockOp::Remove(i) => {
                // Elements i+1..n move down one; the last slot keeps its
                // bytes (dead until re-added).
                let b = self
                    .read(elem(i + 1), (n - i - 1) * size)
                    .ok_or("block unreadable")?;
                (i, b, n - 1)
            }
        };
        let mut writes = Vec::new();
        if !bytes.is_empty() {
            writes.push((elem(from), bytes));
        }
        writes.push((pair, count.to_le_bytes().to_vec()));
        for (at, after) in writes {
            self.check(tag, at, after.len() as u32)?;
            let before = self.read(at, after.len() as u32).expect("checked");
            step.writes.push((at, before, after));
        }
        for (at, _, after) in &step.writes {
            let after = after.clone();
            self.put(*at, &after);
        }
        self.undo.push(step);
        self.redo.clear();
        self.sealed = true;
        self.shape_rev += 1;
        Ok(())
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
            self.read(a, n * size).ok_or("block unreadable")?
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
        let mut step = Step {
            label,
            ..Step::default()
        };
        for (to, after) in [(at, bytes), (pair, pair_bytes)] {
            self.check(tag, to, after.len() as u32)?;
            let before = self.read(to, after.len() as u32).expect("checked");
            step.writes.push((to, before, after));
        }
        for (to, _, after) in &step.writes {
            let after = after.clone();
            self.put(*to, &after);
        }
        self.undo.push(step);
        self.redo.clear();
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
        let Some((n, _)) = self.block(pair, layout) else {
            return Some("the block's elements are outside the map's metadata");
        };
        match *op {
            BlockOp::Add | BlockOp::Duplicate(_) if n >= self.capacity(pair) => {
                if !self.allow_growth {
                    Some(GROWTH_REFUSAL)
                } else if self.is_grown_room(pair) {
                    Some(NESTED_GROWTH)
                } else {
                    match *op {
                        BlockOp::Duplicate(i) if i >= n => Some("no such element"),
                        BlockOp::Duplicate(i) if self.has_children(pair, layout, i) => {
                            Some(SHARED_CHILDREN)
                        }
                        _ => None,
                    }
                }
            }
            BlockOp::Duplicate(i) | BlockOp::Remove(i) if i >= n => Some("no such element"),
            BlockOp::Duplicate(i) if self.has_children(pair, layout, i) => Some(SHARED_CHILDREN),
            _ => None,
        }
    }

    fn has_children(&self, pair: u32, layout: &Layout, i: u32) -> bool {
        let Some((_, a)) = self.pair(pair) else {
            return true;
        };
        let at = a + i * layout.size;
        layout.fields.iter().any(|f| {
            matches!(f.kind, Kind::Block(_) | Kind::Data)
                && self.read_u32(at + f.offset).is_none_or(|n| n != 0)
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
        let step = self.undo.pop()?;
        for (at, before, _) in step.writes.iter().rev() {
            self.put(*at, before);
        }
        if let Some(l) = self.loaded.as_mut() {
            let keep = l.new_strings.len().saturating_sub(step.strings.len());
            l.new_strings.truncate(keep);
        }
        let label = step.label.clone();
        self.redo.push(step);
        self.sealed = true;
        self.shape_rev += 1;
        Some(label)
    }
    pub fn redo(&mut self) -> Option<String> {
        let step = self.redo.pop()?;
        for (at, _, after) in &step.writes {
            self.put(*at, after);
        }
        if let Some(l) = self.loaded.as_mut() {
            l.new_strings.extend(step.strings.iter().cloned());
        }
        let label = step.label.clone();
        self.undo.push(step);
        self.sealed = true;
        self.shape_rev += 1;
        Some(label)
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
        for s in steps {
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

    /// Every changed run in the source's metadata: `(address, bytes)`,
    /// ascending. A grown block's elements and its pair are not here: see
    /// [`Self::grown`]. A grown block emptied again by removals has its
    /// pair written as `(0, 0)`.
    pub fn patches(&self) -> Vec<(u32, Vec<u8>)> {
        let mut out: Vec<(u32, Vec<u8>)> = Vec::new();
        let Some(l) = self.loaded.as_ref() else {
            return out;
        };
        // Pairs pointing into grown room: written at save, not patched.
        let mut skip: BTreeMap<u32, Option<[u8; 8]>> = BTreeMap::new();
        for &p in l.grown.keys() {
            if let Some((n, a)) = self.pair(p)
                && a >= l.ext_base
            {
                skip.insert(p, (n == 0).then_some([0; 8]));
            }
        }
        let mut bytes = l.overlay.clone();
        for (p, empty) in &skip {
            for k in 0..8u32 {
                bytes.remove(&(p + k));
                if let Some(e) = empty
                    && l.src(p + k) != Some(e[k as usize])
                {
                    bytes.insert(p + k, e[k as usize]);
                }
            }
        }
        for (a, b) in bytes.range(..l.ext_base) {
            match out.last_mut() {
                Some((s, run)) if *s + run.len() as u32 == *a => run.push(*b),
                _ => out.push((*a, vec![*b])),
            }
        }
        out
    }
    /// The blocks grown past their source room, with every element as it
    /// stands, for the save to append and repoint.
    pub fn grown(&self) -> Vec<scenario_edit::TagGrown> {
        let Some(l) = self.loaded.as_ref() else {
            return Vec::new();
        };
        l.grown
            .iter()
            .filter_map(|(&root, &size)| {
                let (count, a) = self.pair(root)?;
                if a < l.ext_base || count == 0 {
                    return None;
                }
                Some(scenario_edit::TagGrown {
                    root,
                    count,
                    bytes: self.read(a, count * size)?,
                })
            })
            .collect()
    }

    /// Any byte in the tag's own payload differs from the source.
    pub fn is_changed(&self, id: TagId) -> bool {
        let (Some(l), Some(t)) = (self.loaded.as_ref(), self.tag(id)) else {
            return false;
        };
        t.size > 0
            && l.overlay
                .range(t.address..t.address + t.size)
                .next()
                .is_some()
    }
    /// Whether anything differs from the source.
    pub fn has_edits(&self) -> bool {
        self.loaded
            .as_ref()
            .is_some_and(|l| !l.overlay.is_empty() || !l.new_strings.is_empty())
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
        self.check(tag, address, 4)?;
        let after = value.to_le_bytes().to_vec();
        let before = self.read(address, 4).expect("checked");
        if before == after && !new {
            return Ok(());
        }
        self.put(address, &after);
        let mut step = Step {
            label: format!("set {label}"),
            writes: vec![(address, before, after)],
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
        self.undo.push(step);
        self.redo.clear();
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
