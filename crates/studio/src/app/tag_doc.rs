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
     Growing it means appending to the map's metadata and repointing it, which the tag editor \
     does not do yet (and the engine's headroom for more tag memory is not established, \
     docs/editor/scenario-editor.md §6)";
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
    /// End the current merge run.
    Seal,
}

#[derive(Clone, Debug, Default)]
struct Step {
    label: String,
    /// `(address, before, after)`.
    writes: Vec<(u32, Vec<u8>, Vec<u8>)>,
    merge: Option<u64>,
}

/// The open map's tags and the edits over them.
struct Loaded {
    tags: Vec<TagEntry>,
    by_id: HashMap<TagId, usize>,
    /// Group → (parent, grandparent), for reference filters.
    parents: HashMap<FourCC, [Option<FourCC>; 2]>,
    strings: Vec<Option<String>>,
    string_ids: HashMap<String, u32>,
    /// Virtual address of `meta[0]`.
    base: u32,
    /// The source's ordinary metadata.
    meta: Vec<u8>,
    /// Changed bytes by address (only those that differ from `meta`).
    overlay: BTreeMap<u32, u8>,
    /// The scenario payload `[lo, hi)`.
    scenario: Option<(u32, u32)>,
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
        self.loaded = Some(Loaded {
            tags,
            by_id,
            parents,
            strings,
            string_ids,
            base: space.map_or(0, |s| s.virtual_base),
            meta,
            overlay: BTreeMap::new(),
            scenario,
        });
        self.rev += 1;
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
    }

    pub fn is_open(&self) -> bool {
        self.loaded.is_some()
    }
    pub fn rev(&self) -> u64 {
        self.rev
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
        let at = address.checked_sub(l.base)? as usize;
        let mut out = l.meta.get(at..at.checked_add(len as usize)?)?.to_vec();
        for (a, b) in l.overlay.range(address..address + len) {
            out[(a - address) as usize] = *b;
        }
        Some(out)
    }
    /// The source's bytes, no edits.
    pub fn read_source(&self, address: u32, len: u32) -> Option<&[u8]> {
        let l = self.loaded.as_ref()?;
        let at = address.checked_sub(l.base)? as usize;
        l.meta.get(at..at.checked_add(len as usize)?)
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
            if l.meta[(a - l.base) as usize] == *b {
                l.overlay.remove(&a);
            } else {
                l.overlay.insert(a, *b);
            }
        }
        self.rev += 1;
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
                Some(GROWTH_REFUSAL)
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
        let label = step.label.clone();
        self.redo.push(step);
        self.sealed = true;
        Some(label)
    }
    pub fn redo(&mut self) -> Option<String> {
        let step = self.redo.pop()?;
        for (at, _, after) in &step.writes {
            self.put(*at, after);
        }
        let label = step.label.clone();
        self.undo.push(step);
        self.sealed = true;
        Some(label)
    }
    pub fn seal(&mut self) {
        self.sealed = true;
    }

    /// Every changed run: `(address, bytes)`, ascending.
    pub fn patches(&self) -> Vec<(u32, Vec<u8>)> {
        let mut out: Vec<(u32, Vec<u8>)> = Vec::new();
        let Some(l) = self.loaded.as_ref() else {
            return out;
        };
        for (a, b) in &l.overlay {
            match out.last_mut() {
                Some((s, run)) if *s + run.len() as u32 == *a => run.push(*b),
                _ => out.push((*a, vec![*b])),
            }
        }
        out
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
        self.loaded.as_ref().is_some_and(|l| !l.overlay.is_empty())
    }

    /// The string of a string ID's value, if the map has it.
    pub fn string(&self, value: u32) -> Option<&str> {
        let l = self.loaded.as_ref()?;
        tags::StringId(value).resolve(&l.strings).ok().flatten()
    }
    /// The value of the string ID for `text`, if the map has the string.
    pub fn string_id(&self, text: &str) -> Option<u32> {
        self.loaded.as_ref()?.string_ids.get(text).copied()
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
