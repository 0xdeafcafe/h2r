//! Tag-write notices (docs/editor/realtime-edits.md, "Realtime coverage",
//! slice R8).
//!
//! The lifted code and the D3D path read a tag field where it lies, so a
//! written field shows there by itself. Host copies of tag bytes do not:
//! the native renderer's materials, the animation clip store, the HUD
//! kit's copy of the widget blocks, aim assist's remembered values, the
//! bots' data. After an `edit.set`, an `edit.batch` or an `edit.revert`
//! applies, the editor sends one [`TagWrite`] per tag whose bytes changed
//! to the sink [`Editor::with_tag_writes`] was given; the game's hot-edit
//! glue hands each to its subscribers.
//!
//! A notice is sent after the bytes are in guest memory, with the guest
//! still held, so a subscriber reads the new bytes and may write its own
//! copy at the same boundary.

use super::*;

/// One tag an applied edit changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagWrite {
    pub group: FourCC,
    /// The tag's datum.
    pub tag: u32,
    /// The tag's metadata allocation in guest memory, from the index.
    pub address: u32,
    pub size: u32,
    /// The guest addresses from the first changed byte to past the last,
    /// a moved block's root pair included.
    pub range: Range<u32>,
    /// Each field write's bytes, in the order applied: what the guest
    /// holds there now is the edited (or, for a revert, restored) value.
    /// A move's root pair is in [`Self::range`] but not here: it points
    /// into the edit arena, which no file holds.
    pub spans: Vec<Range<u32>>,
    /// Block pairs repointed by the edit, including the displaced storage.
    pub moved_blocks: Vec<BlockRepoint>,
    /// Whether the edit was a revert.
    pub revert: bool,
}

impl TagWrite {
    /// Whether `[start, end)` overlaps a written byte.
    pub fn touches(&self, start: u32, end: u32) -> bool {
        start < self.range.end && self.range.start < end
    }
}

/// A block pair written by a move or its revert.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockRepoint {
    pub root: u32,
    /// Count and address from before this pair was written.
    pub previous: (u32, u32),
}

/// Takes the notices of an applied edit, the guest still held.
pub type TagWriteSink = Box<dyn FnMut(&mut Guest, &TagWrite) + Send>;

/// The notices of one edit, one per tag in the order first changed.
#[derive(Default)]
pub(crate) struct Notices {
    revert: bool,
    tags: Vec<ChangedTag>,
}

struct ChangedTag {
    datum: u32,
    range: Range<u32>,
    spans: Vec<Range<u32>>,
    moved_blocks: Vec<BlockRepoint>,
}

impl Notices {
    pub(crate) fn new(revert: bool) -> Self {
        Notices { revert, tags: Vec::new() }
    }

    fn add(&mut self, tag: u32, at: u32, len: u32) -> &mut ChangedTag {
        let end = at.saturating_add(len);
        let i = match self.tags.iter().position(|entry| entry.datum == tag) {
            Some(i) => i,
            None => {
                self.tags.push(ChangedTag {
                    datum: tag,
                    range: at..end,
                    spans: Vec::new(),
                    moved_blocks: Vec::new(),
                });
                self.tags.len() - 1
            }
        };
        let entry = &mut self.tags[i];
        entry.range.start = entry.range.start.min(at);
        entry.range.end = entry.range.end.max(end);
        entry
    }

    /// A field write of `len` bytes at `at` in tag `tag`.
    pub(crate) fn field(&mut self, tag: u32, at: u32, len: u32) {
        if len != 0 {
            let entry = self.add(tag, at, len);
            entry.spans.push(at..at.saturating_add(len));
        }
    }

    /// A moved block's root pair at `root` in tag `tag`.
    pub(crate) fn root(&mut self, tag: u32, root: u32, previous: (u32, u32)) {
        self.add(tag, root, 8).moved_blocks.push(BlockRepoint { root, previous });
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.tags.is_empty()
    }
}

impl Editor {
    /// Applied edits send one [`TagWrite`] per changed tag to `sink`.
    pub fn with_tag_writes(mut self, sink: TagWriteSink) -> Editor {
        self.tag_writes = Some(sink);
        self
    }

    /// Sends `notices`, each tag's group and allocation read from the
    /// index now. A tag no longer in the index (its map went) is not sent.
    pub(crate) fn send_notices(&mut self, g: &mut Guest, notices: Notices) {
        if notices.is_empty() || self.tag_writes.is_none() {
            return;
        }
        let Ok(idx) = self.index(g) else { return };
        let revert = notices.revert;
        for change in notices.tags {
            let Ok(e) = index::by_datum(g, &idx, change.datum) else { continue };
            let notice = TagWrite {
                group: e.group,
                tag: change.datum,
                address: e.address,
                size: e.size,
                range: change.range,
                spans: change.spans,
                moved_blocks: change.moved_blocks,
                revert,
            };
            if let Some(sink) = self.tag_writes.as_mut() {
                sink(&mut *g, &notice);
            }
        }
    }
}

#[cfg(test)]
mod tests;
