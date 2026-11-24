//! Data references in the scenario that change length: the session's form
//! of the tag editor's `TagDoc::resize_data`
//! (docs/editor/rich-editors.md, "Data that changes length").
//!
//! A data reference is an 8-byte `(size, address)` pair. Each edited one
//! keeps its whole current payload here, by the pair's virtual address;
//! [`Scenario::changes`] turns it into writes. A payload no longer than
//! the source's, whose source lies in the scenario's payload, is written
//! over the source bytes and the size patched (the tail stays as dead
//! bytes). A longer one, or any change to data outside the payload, is a
//! [`TagGrown`] at the pair: appended to the metadata on save, the pair
//! pointed at the copy, the old bytes left unreferenced.
use super::{Scenario, TagGrown, u32_at};
use crate::layout;
use crate::volumes::VolumeKind;
use crate::{Error, Result};

/// Why a data reference outside the scenario's payload is not resized here.
pub const DATA_OUTSIDE: &str = "the data reference is outside the scenario tag's payload";
/// Why a data reference the placement or volume blocks cover is not.
pub const DATA_OWNED: &str =
    "the data reference lies in a block the Scenario tool edits element by element";
/// Why a data reference whose bytes run past the payload's end is not.
pub const DATA_RUNS_PAST: &str = "the data starts in the scenario's payload and runs past its end";

impl Scenario {
    /// The `(size, address)` pair at `pair` as the source has it, if the
    /// pair lies in the scenario's payload.
    fn source_pair(&self, pair: u32) -> Option<(u32, u32)> {
        let at = pair.checked_sub(self.address)? as usize;
        (at + 8 <= self.payload.len())
            .then(|| (u32_at(&self.payload, at), u32_at(&self.payload, at + 4)))
    }

    /// The source bytes of the data at `pair`, when they lie in the
    /// payload (empty for an empty reference).
    fn source_data(&self, pair: u32) -> Option<&[u8]> {
        let (n, a) = self.source_pair(pair)?;
        if n == 0 {
            return Some(&[]);
        }
        let at = a.checked_sub(self.address)? as usize;
        self.payload.get(at..at.checked_add(n as usize)?)
    }

    /// The byte ranges the element-by-element blocks write: each block's
    /// source elements and its root pair.
    fn owned_spans(&self) -> Vec<(u64, u64)> {
        let span = |a: u32, len: u64| (u64::from(a), u64::from(a) + len);
        let mut out = Vec::new();
        for b in &self.blocks {
            out.push(span(self.address + b.def.offset, 8));
            out.push(span(
                b.address,
                b.capacity() as u64 * u64::from(b.def.stride),
            ));
        }
        out.push(span(self.address + layout::OBJECT_NAMES, 8));
        out.push(span(
            self.names.address,
            self.names.original.len() as u64 * u64::from(layout::OBJECT_NAME_STRIDE),
        ));
        for (kind, b) in VolumeKind::ALL.iter().zip(&self.volume_blocks) {
            let (offset, stride) = kind.block();
            out.push(span(self.address + offset, 8));
            out.push(span(b.address, b.capacity() as u64 * u64::from(stride)));
        }
        out.retain(|(a, e)| e > a);
        out
    }

    /// Why the data reference at `pair` may not be resized here, if it may
    /// not (growth is the session's to allow; see
    /// [`crate::Session::data_refusal`]).
    pub fn data_pair_refusal(&self, pair: u32) -> Option<&'static str> {
        let Some((n, a)) = self.source_pair(pair) else {
            return Some(DATA_OUTSIDE);
        };
        let hit = |lo: u64, hi: u64| {
            self.owned_spans()
                .into_iter()
                .any(|(a, e)| lo < e && a < hi)
        };
        let p = u64::from(pair);
        if hit(p, p + 8) {
            return Some(DATA_OWNED);
        }
        let (lo, hi) = (
            u64::from(self.address),
            u64::from(self.address) + self.payload.len() as u64,
        );
        let (da, de) = (u64::from(a), u64::from(a) + u64::from(n));
        if n > 0 && da >= lo && da < hi {
            if de > hi {
                return Some(DATA_RUNS_PAST);
            }
            if hit(da, de) {
                return Some(DATA_OWNED);
            }
        }
        None
    }

    /// How many bytes the data at `pair` can hold in place: its source
    /// length when its source lies in the payload, else none (every change
    /// to it is a new copy).
    pub fn data_capacity(&self, pair: u32) -> u32 {
        self.source_data(pair).map_or(0, |b| b.len() as u32)
    }

    /// The current payload of the data at `pair`: the edited bytes, or the
    /// source's when they lie in the payload. `None` for unedited data
    /// outside it.
    pub fn data(&self, pair: u32) -> Option<Vec<u8>> {
        match self.data.get(&pair) {
            Some(b) => Some(b.clone()),
            None => self.source_data(pair).map(<[u8]>::to_vec),
        }
    }

    /// Whether the data at `pair` would need new room for `len` bytes.
    pub fn data_grows(&self, pair: u32, len: u32) -> bool {
        len > self.data_capacity(pair)
    }

    /// Give the data reference at `pair` the payload `bytes`. Returns the
    /// edit it replaced (`None`: the source's), for an undo. Refused by
    /// [`Self::data_pair_refusal`]'s reasons; growth is not checked here.
    pub fn resize_data(&mut self, pair: u32, bytes: Vec<u8>) -> Result<Option<Vec<u8>>> {
        if let Some(why) = self.data_pair_refusal(pair) {
            return Err(Error::Refused(why));
        }
        if u32::try_from(bytes.len()).is_err() {
            return Err(Error::Refused("the data is too long"));
        }
        Ok(self.data.insert(pair, bytes))
    }

    /// Put back an edit [`Self::resize_data`] replaced, for an undo.
    pub(crate) fn write_data(&mut self, pair: u32, before: Option<Vec<u8>>) {
        match before {
            Some(b) => self.data.insert(pair, b),
            None => self.data.remove(&pair),
        };
    }

    /// Whether any data reference differs from its source.
    pub(crate) fn data_modified(&self) -> bool {
        self.data
            .iter()
            .any(|(&pair, b)| self.source_data(pair) != Some(b.as_slice()))
    }

    /// The writes the edited data references make, in place or grown.
    pub(crate) fn data_changes(
        &self,
        patches: &mut Vec<(u32, Vec<u8>)>,
        grown: &mut Vec<TagGrown>,
    ) {
        for (&pair, bytes) in &self.data {
            let source = self.source_data(pair);
            if source == Some(bytes.as_slice()) {
                continue;
            }
            let len = bytes.len() as u32;
            let Some((n, a)) = self.source_pair(pair) else {
                continue;
            };
            if source.is_some() && len <= n {
                if len > 0 {
                    patches.push((a, bytes.clone()));
                }
                if len != n {
                    patches.push((pair, len.to_le_bytes().to_vec()));
                }
            } else if len == 0 {
                patches.push((pair, 0u32.to_le_bytes().to_vec()));
            } else {
                grown.push(TagGrown {
                    root: pair,
                    count: len,
                    bytes: bytes.clone(),
                });
            }
        }
    }
}
