//! Applies edit files to a map (docs/game/ui-in-maps.md, "Retail edits as
//! data").
//!
//! The map is decomposed (`map::build::TagSet::from_map`, every tag pinned
//! where it was). Each screen a file edits is laid out anew as a block
//! tree with the checked-in tag schema (`tags::schema::Relocator`, which
//! gives every block and data reference of the `wgit` a fixup), so a block
//! array that grows grows inside the tag's own payload
//! (`TagPayload::Built`), as `inject::tag::add_screens` grows a `wgtz`.
//! Arrays another tag points into stay where they were: when any do, the
//! screen's old bytes are kept in place (`PinnedImage::unowned`).
//!
//! The edits are made on that tree, and the tag takes it only when every
//! edit of the file succeeds: a file is applied whole or not at all, and a
//! file that fails leaves its screen pinned where it was. A string name is
//! a string ID fixup (`FixupKind::StringId`), which the builder interns in
//! the map's string table (existing strings keep their index), so the
//! game, and our code, find the block's string ID by name.

use std::collections::{BTreeSet, HashSet};
use std::io::Cursor;

use anyhow::{Context, Result, anyhow};
use map::build::{Block, Fixup, FixupKind, TagPayload, TagSet};
use map::{Map, TagId};
use serde::{Deserialize, Serialize};
use tags::schema::{Relocator, Schema};

use super::{BlockKind, BlockName, Edit, EditFile, PARKED, Place, Rect};

/// A `wgit` root's panes block (count, address) and header string ID.
const ROOT_PANES: u32 = 0x20;
const ROOT_HEADER: usize = 0x2c;
/// One pane's size.
const PANE_SIZE: usize = 0x4c;

/// What one file did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Applied {
    /// The file's name (its stem).
    pub file: String,
    pub screen: String,
    /// One line per edit.
    pub lines: Vec<String>,
}

/// What [`apply_all`] made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edited {
    /// The map; the input's bytes when no file applied.
    pub map: Vec<u8>,
    pub applied: Vec<Applied>,
    /// Each file that did not apply, and why. Its screen is as it was.
    pub failed: Vec<(String, String)>,
}

/// `original` (a map) with `files` applied in order. An error when the
/// map cannot be read or built; a file that cannot be applied is named in
/// [`Edited::failed`] and leaves its screen as it was.
pub fn apply_all(original: &[u8], files: &[(String, EditFile)]) -> Result<Edited> {
    let mut map = Map::from_reader(Cursor::new(original)).context("not a map")?;
    let relocator = Relocator::new(Schema::builtin(), &mut map)
        .map_err(|e| anyhow!("reading the map's tags: {e}"))?;
    let screens: Vec<(TagId, String)> = map
        .tags()
        .iter()
        .filter(|t| t.group == tags::ui::SCREEN)
        .map(|t| (t.id, t.name.clone().unwrap_or_default()))
        .collect();
    let mut set = TagSet::from_map(&mut map).context("decomposing the map")?;
    // A screen laid out anew may land past the metadata's old end (its
    // old range is too small once a block grew); the tag-memory bound is
    // empirical, as `inject::tag::add_screens` says, and lifted the same
    // way for the same few bytes.
    set.check_tag_memory = false;

    let mut unpinned = BTreeSet::new();
    let mut applied = Vec::new();
    let mut failed = Vec::new();
    for (name, file) in files {
        match apply_file(&mut set, &relocator, &screens, &mut unpinned, file) {
            Ok(lines) => applied.push(Applied {
                file: name.clone(),
                screen: file.screen.clone(),
                lines,
            }),
            Err(why) => failed.push((name.clone(), why)),
        }
    }
    if unpinned.is_empty() {
        return Ok(Edited {
            map: original.to_vec(),
            applied,
            failed,
        });
    }
    let (map, _) = set.build().context("building the map")?;
    Ok(Edited {
        map,
        applied,
        failed,
    })
}

fn apply_file(
    set: &mut TagSet,
    relocator: &Relocator<'_>,
    screens: &[(TagId, String)],
    unpinned: &mut BTreeSet<TagId>,
    file: &EditFile,
) -> Result<Vec<String>, String> {
    let id = screens
        .iter()
        .find(|(_, n)| n.eq_ignore_ascii_case(&file.screen))
        .map(|(id, _)| *id)
        .ok_or_else(|| format!("the map has no screen tag {}", file.screen))?;
    // The tree as it is now: laid out anew from the map, or as an earlier
    // file left it.
    let mut work = if unpinned.contains(&id) {
        match set.tag(id).map(|t| &t.payload) {
            Some(TagPayload::Built(root)) => root.clone(),
            _ => return Err("the screen tag was not laid out anew".into()),
        }
    } else {
        tree_of(relocator, id)?
    };
    let mut lines = Vec::with_capacity(file.edits.len());
    for (i, e) in file.edits.iter().enumerate() {
        let line = edit(&mut work, file.pane, e)
            .map_err(|why| format!("edits[{i}] ({} {}): {why}", e.op(), e.block()))?;
        lines.push(line);
    }
    if !unpinned.contains(&id) {
        unpin(set, relocator, id)?;
        unpinned.insert(id);
    }
    set.tag_mut(id)
        .ok_or("the screen tag left the set")?
        .payload = TagPayload::Built(work);
    Ok(lines)
}

/// Screen tag `id` as a block tree, every array inside it a child block
/// (arrays of other tags stay where they are).
fn tree_of(relocator: &Relocator<'_>, id: TagId) -> Result<Block, String> {
    if let Err(d) = relocator.check(id) {
        return Err(format!(
            "the tag schema cannot lay the screen tag out anew: {} (block path {:x?}, offset {:#x}, value {:#x})",
            d.refusal, d.path, d.offset, d.value
        ));
    }
    relocator
        .to_built_among(id, &HashSet::from([id]))
        .map_err(|r| format!("the tag schema cannot lay the screen tag out anew: {r}"))
}

/// Screen tag `id` made a block tree in `set` (module docs): the caller
/// gives it the edited tree.
fn unpin(set: &mut TagSet, relocator: &Relocator<'_>, id: TagId) -> Result<(), String> {
    let old = match set.tag(id).map(|t| &t.payload) {
        Some(TagPayload::Pinned { address, bytes }) => (*address, bytes.clone()),
        _ => return Err("the screen tag is not where the map had it".into()),
    };
    let others = relocator.pointed_into(id);
    // Also points the tags that share its bytes (index size 0) at it.
    relocator
        .relocate(set, &[id], false)
        .map_err(|e| format!("laying the screen tag out anew: {e}"))?;
    if !others.is_empty() {
        // Another tag's array is in its old bytes: they stay where they
        // are, owned by no tag, so that pointer still reads them.
        set.pinned
            .as_mut()
            .ok_or("a decomposed map has its image")?
            .unowned
            .push(old);
    }
    Ok(())
}

/// The child block at `offset` of `parent`, and its count.
fn child_mut(parent: &mut Block, offset: u32) -> Option<(&mut u32, &mut Block)> {
    parent.fixups.iter_mut().find_map(|f| {
        let at = f.offset;
        match &mut f.kind {
            FixupKind::Block { count, child } if at == offset => Some((count, child)),
            _ => None,
        }
    })
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn i16_at(b: &[u8], at: usize) -> i16 {
    i16::from_le_bytes([b[at], b[at + 1]])
}

fn put_i16(b: &mut [u8], at: usize, v: i16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

/// Pane `pane`'s block of `kind` in the screen's tree, and its count.
fn pane_block(root: &mut Block, pane: usize, kind: BlockKind) -> Result<(&mut u32, &mut Block), String> {
    let (panes, block) =
        child_mut(root, ROOT_PANES).ok_or("the screen's panes are not in its own bytes")?;
    if pane >= *panes as usize {
        return Err(format!("the screen has {} panes, no pane {pane}", *panes));
    }
    // Edited, so placed for this tag alone.
    block.share = None;
    let at = pane * PANE_SIZE + kind.pane_field() as usize;
    let stored = u32_at(&block.bytes, at);
    match child_mut(block, at as u32) {
        Some((count, child)) => {
            child.share = None;
            Ok((count, child))
        }
        None => Err(format!(
            "pane {pane}'s {} ({stored} stored) are another tag's or have no address",
            kind.field_name()
        )),
    }
}

/// The place of element `e` of `kind` as a rect (a point gives its
/// top-left twice).
pub(crate) fn place_of(kind: BlockKind, e: &[u8]) -> Rect {
    match kind.place() {
        Place::Rect(at) => [
            i16_at(e, at),
            i16_at(e, at + 2),
            i16_at(e, at + 4),
            i16_at(e, at + 6),
        ],
        Place::Point(at) => {
            let (x, y) = (i16_at(e, at), i16_at(e, at + 2));
            [y, x, y, x]
        }
    }
}

/// Writes `rect` as element `e`'s place (a point takes the top-left).
fn put_place(kind: BlockKind, e: &mut [u8], rect: Rect) {
    match kind.place() {
        Place::Rect(at) => {
            for (k, v) in rect.into_iter().enumerate() {
                put_i16(e, at + 2 * k, v);
            }
        }
        Place::Point(at) => {
            put_i16(e, at, rect[1]);
            put_i16(e, at + 2, rect[0]);
        }
    }
}

/// `rect` parked off screen: a rect keeps its left and right, a point
/// goes to (PARKED, PARKED).
pub(crate) fn parked(kind: BlockKind, rect: Rect) -> Rect {
    match kind.place() {
        Place::Rect(_) => [PARKED, rect[1], PARKED, rect[3]],
        Place::Point(_) => [PARKED; 4],
    }
}

/// Whether element `e` of `kind` is parked ([`parked`]).
pub(crate) fn is_parked(kind: BlockKind, e: &[u8]) -> bool {
    let r = place_of(kind, e);
    r[0] == PARKED && r[2] == PARKED && (matches!(kind.place(), Place::Rect(_)) || r[1] == PARKED)
}

/// Drops `block`'s fixups at `offset`: the bytes there are written.
fn clear_fixup(block: &mut Block, offset: usize) {
    block.fixups.retain(|f| f.offset as usize != offset);
}

/// Makes edit `e` on pane `pane` of the screen tree `root`; a line saying
/// what it did.
fn edit(root: &mut Block, pane: usize, e: &Edit) -> Result<String, String> {
    let (kind, index) = match (e, e.block()) {
        (Edit::SetString { name, .. }, BlockName::Title) => {
            clear_fixup(root, ROOT_HEADER);
            root.string_id(ROOT_HEADER as u32, name);
            return Ok(format!("title reads {name}"));
        }
        (_, BlockName::Title) => return Err("the title has only a string ID".into()),
        (_, BlockName::Block(k, i)) => (k, i),
    };
    let (count, block) = pane_block(root, pane, kind)?;
    let n = *count as usize;
    let size = kind.size();
    if block.bytes.len() != n * size {
        return Err(format!(
            "pane {pane}'s {} hold {} bytes for {n} blocks",
            kind.field_name(),
            block.bytes.len()
        ));
    }
    if index >= n {
        return Err(format!("pane {pane} has {n} {kind} blocks, no {kind}.{index}"));
    }
    let start = index * size;
    match e {
        Edit::SetRect { rect, .. } => {
            put_place(kind, &mut block.bytes[start..start + size], *rect);
            Ok(format!("{kind}.{index} at {rect:?}"))
        }
        Edit::Hide { .. } => {
            if kind == BlockKind::Button {
                return Err("a button keeps its place in the focus order; it is not hidden by a rect".into());
            }
            let el = &mut block.bytes[start..start + size];
            let rect = parked(kind, place_of(kind, el));
            put_place(kind, el, rect);
            Ok(format!("{kind}.{index} parked at {rect:?}"))
        }
        Edit::Duplicate { rect, copy, .. } => {
            if *copy != BlockName::Block(kind, n) {
                return Err(format!(
                    "the copy would be {kind}.{n} (the pane has {n} {kind} blocks), and the file says {copy}"
                ));
            }
            let bytes = block.bytes[start..start + size].to_vec();
            let moved: Vec<Fixup> = block
                .fixups
                .iter()
                .filter(|f| (start..start + size).contains(&(f.offset as usize)))
                .map(|f| Fixup {
                    offset: f.offset - start as u32 + (n * size) as u32,
                    kind: f.kind.clone(),
                })
                .collect();
            block.bytes.extend_from_slice(&bytes);
            block.fixups.extend(moved);
            *count += 1;
            put_place(kind, &mut block.bytes[n * size..], *rect);
            Ok(format!("{kind}.{index} copied to {copy} at {rect:?}"))
        }
        Edit::SetString { name, .. } => {
            let at = kind
                .string_at()
                .ok_or_else(|| format!("a {kind} block has no string ID"))?;
            clear_fixup(block, start + at);
            block.string_id((start + at) as u32, name);
            Ok(format!("{kind}.{index} reads {name}"))
        }
    }
}
