//! The edits that turn one pane into another: the editor's
//! `ui.export_edits` (docs/game/ui-in-maps.md, "Retail edits as data").
//!
//! The editor holds a retail screen's edits as writes in the map's tag
//! document. Its pane's text, button and bitmap blocks and the screen's
//! header string ID, read once from the loaded map and once as edited
//! ([`PaneBytes`]), give the edits back: a block whose place differs is
//! a `set_rect` (a `hide` when it is parked), a string ID that differs a
//! `set_string`, a block past the loaded count a `duplicate` of the block
//! it is a copy of. What the format cannot say (another field changed, a
//! block removed, a string ID with no name) is listed, not dropped.

use super::apply::{is_parked, place_of};
use super::{BlockKind, BlockName, Edit, EditFile, Place, string_name_ok};

/// One pane's text, button and bitmap blocks as their elements' bytes,
/// and the screen's header string ID.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PaneBytes {
    pub header: u32,
    pub texts: Vec<u8>,
    pub buttons: Vec<u8>,
    pub bitmaps: Vec<u8>,
}

impl PaneBytes {
    /// The elements of `kind`.
    pub fn of(&self, kind: BlockKind) -> &[u8] {
        match kind {
            BlockKind::Text => &self.texts,
            BlockKind::Button => &self.buttons,
            BlockKind::Bitmap => &self.bitmaps,
        }
    }

    /// The elements of `kind`, to fill.
    pub fn of_mut(&mut self, kind: BlockKind) -> &mut Vec<u8> {
        match kind {
            BlockKind::Text => &mut self.texts,
            BlockKind::Button => &mut self.buttons,
            BlockKind::Bitmap => &mut self.bitmaps,
        }
    }
}

/// What [`diff_pane`] found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Diff {
    /// The edits, `None` when there are none.
    pub file: Option<EditFile>,
    /// Each change the format cannot hold, by block.
    pub skipped: Vec<String>,
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// The element's bytes with its place and string ID zeroed: equal for
/// two elements that differ in those alone.
fn rest(kind: BlockKind, e: &[u8]) -> Vec<u8> {
    let mut b = e.to_vec();
    match kind.place() {
        Place::Rect(at) => b[at..at + 8].fill(0),
        Place::Point(at) => b[at..at + 4].fill(0),
    }
    if let Some(at) = kind.string_at() {
        b[at..at + 4].fill(0);
    }
    b
}

fn string_of(kind: BlockKind, e: &[u8]) -> Option<u32> {
    kind.string_at().map(|at| u32_at(e, at))
}

/// The edits that make `now` of `source`, pane `pane` of screen tag
/// `screen` in map `map`; `name` gives a string ID's name.
pub fn diff_pane(
    screen: &str,
    map: &str,
    pane: usize,
    source: &PaneBytes,
    now: &PaneBytes,
    name: &dyn Fn(u32) -> Option<String>,
) -> Diff {
    let mut copies = Vec::new();
    let mut places = Vec::new();
    let mut strings = Vec::new();
    let mut skipped = Vec::new();
    let set_string = |block: BlockName, id: u32, strings: &mut Vec<Edit>, skipped: &mut Vec<String>| {
        match name(id) {
            Some(n) if string_name_ok(&n) => strings.push(Edit::SetString { block, name: n }),
            Some(n) => skipped.push(format!(
                "{block}: its string ID names {n:?}, which a file cannot name (lowercase letters, digits and _)"
            )),
            None => skipped.push(format!("{block}: its string ID {id:#010x} has no name in the map")),
        }
    };
    if now.header != source.header {
        set_string(BlockName::Title, now.header, &mut strings, &mut skipped);
    }
    for kind in BlockKind::ALL {
        let size = kind.size();
        let src: Vec<&[u8]> = source.of(kind).chunks_exact(size).collect();
        let cur: Vec<&[u8]> = now.of(kind).chunks_exact(size).collect();
        if cur.len() < src.len() {
            skipped.push(format!(
                "{kind}: {} of {} blocks removed; a file only adds blocks",
                src.len() - cur.len(),
                src.len()
            ));
        }
        for (i, e) in cur.iter().enumerate() {
            let block = BlockName::Block(kind, i);
            if let Some(s) = src.get(i) {
                if e == s {
                    continue;
                }
                if rest(kind, e) != rest(kind, s) {
                    skipped.push(format!(
                        "{block}: fields other than its place and string ID changed"
                    ));
                }
                if place_of(kind, e) != place_of(kind, s) {
                    if kind != BlockKind::Button && is_parked(kind, e) {
                        places.push(Edit::Hide { block });
                    } else {
                        places.push(Edit::SetRect {
                            block,
                            rect: place_of(kind, e),
                        });
                    }
                }
                if let (Some(a), Some(b)) = (string_of(kind, e), string_of(kind, s))
                    && a != b
                {
                    set_string(block, a, &mut strings, &mut skipped);
                }
                continue;
            }
            // Past the loaded count: a copy of the first loaded block it
            // matches outside its place and string ID.
            let mine = rest(kind, e);
            let Some(j) = src.iter().position(|s| rest(kind, s) == mine) else {
                skipped.push(format!(
                    "{block}: added, and no block the screen had is its original"
                ));
                continue;
            };
            copies.push(Edit::Duplicate {
                block: BlockName::Block(kind, j),
                rect: place_of(kind, e),
                copy: block,
            });
            if let (Some(a), Some(b)) = (string_of(kind, e), string_of(kind, src[j]))
                && a != b
            {
                set_string(block, a, &mut strings, &mut skipped);
            }
        }
    }
    let edits: Vec<Edit> = copies.into_iter().chain(places).chain(strings).collect();
    let file = (!edits.is_empty()).then(|| EditFile {
        screen: screen.to_owned(),
        map: map.to_owned(),
        pane,
        edits,
    });
    Diff { file, skipped }
}
