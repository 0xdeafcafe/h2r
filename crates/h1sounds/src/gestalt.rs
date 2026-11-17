//! The Halo 2 `ugh!` sound gestalt as an editable model.
//!
//! A map has one `ugh!`. Every `snd!` in the map indexes into it, so a map
//! that gains sounds must gain gestalt entries, and the gestalt must be
//! rewritten. [`SoundGestalt::decode`] reads all eleven blocks, including
//! the three with child blocks, and refuses a tag whose bytes it does not
//! account for exactly once (so no unknown pointer can be left behind).
//! [`SoundGestalt::to_block`] writes it back as a `map::build` tree, which
//! moves freely; [`SoundGestalt::install`] puts it in a [`TagSet`] in place
//! of the old one (same slot, so every reference to it stays valid).
//!
//! Values that depend on the map (string IDs in the import names, raw
//! pointers in chunks and extra infos, the extra infos' owner datum) are
//! kept as stored. They stay valid in a map built from the same
//! [`TagSet::from_map`] decomposition: string IDs keep their index, the
//! pinned file prefix holding the raw data does not move, and slots keep
//! their datums. New chunks point at raw resources added to the set.
//!
//! Layouts and evidence: `docs/formats/h1-sounds.md` ("The Halo 2 gestalt").

use map::FourCC;
use map::build::{Block, RawId, RawRef, TagPayload, TagSet};

use crate::{Error, Result};

/// `ugh!`.
pub const UGH: FourCC = FourCC(*b"ugh!");
/// Bytes in the gestalt's header: eleven `{count, address}` blocks.
pub const HEADER_LEN: usize = 0x58;
/// Name for a gestalt made from nothing (a map with no `ugh!`).
pub const DEFAULT_NAME: &str = "i've got a lovely bunch of coconuts";

/// Element strides of the eleven blocks, in header order.
pub const STRIDES: [usize; 11] = [0x38, 0x14, 4, 10, 12, 16, 0x34, 1, 12, 0x1c, 0x2c];

/// Block indices in the header.
pub mod blocks {
    /// Playback parameters (`0x38`), indexed by `snd!` `+0x06`.
    pub const PLAYBACKS: usize = 0;
    /// Scales (`0x14`), indexed by `snd!` `+0x0b`.
    pub const SCALES: usize = 1;
    /// Import names: string IDs, indexed by pitch ranges and permutations.
    pub const IMPORT_NAMES: usize = 2;
    /// Pitch range parameters (10 bytes).
    pub const PITCH_RANGE_PARAMETERS: usize = 3;
    /// Pitch ranges (12).
    pub const PITCH_RANGES: usize = 4;
    /// Permutations (16).
    pub const PERMUTATIONS: usize = 5;
    /// Custom playbacks (`0x34`), with children.
    pub const CUSTOM_PLAYBACKS: usize = 6;
    /// Runtime permutation flags: one byte each, eight permutations a byte.
    pub const RUNTIME_PERMUTATION_FLAGS: usize = 7;
    /// Chunks (12).
    pub const CHUNKS: usize = 8;
    /// Promotions (`0x1c`), with children.
    pub const PROMOTIONS: usize = 9;
    /// Extra infos (`0x2c`), with children.
    pub const EXTRA_INFOS: usize = 10;
}

/// The child blocks of a nested block's element: `(offset, stride)`.
pub fn child_blocks(block: usize) -> &'static [(usize, usize)] {
    match block {
        blocks::CUSTOM_PLAYBACKS => &[(0x14, 0x48), (0x1c, 0x30)],
        blocks::PROMOTIONS => &[(0, 0x10), (8, 4)],
        blocks::EXTRA_INFOS => &[(0, 0), (0x18, 0x10)],
        _ => &[],
    }
}

/// A chunk's raw pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkPointer {
    /// As stored (file selector in the top two bits).
    Stored(u32),
    /// A raw resource added to the set.
    Local(RawId),
}

/// A gestalt chunk (12 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chunk {
    /// `+0`.
    pub pointer: ChunkPointer,
    /// `+4`: size, with a flag in bit 31.
    pub size: u32,
    /// `+8`: runtime, `0xffffffff` on disc.
    pub runtime: u32,
}

/// A child block of one element of a nested block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Child {
    /// Element count.
    pub count: u32,
    /// Element bytes.
    pub bytes: Vec<u8>,
}

/// One element of a nested block: its bytes (child block fields zeroed)
/// and one child per [`child_blocks`] entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nested {
    /// The element's bytes.
    pub bytes: Vec<u8>,
    /// Its children, in [`child_blocks`] order.
    pub children: Vec<Child>,
}

/// The whole `ugh!` tag.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SoundGestalt {
    /// Element bytes of the flat blocks, by header index (`IMPORT_NAMES`,
    /// `CHUNKS` and the nested blocks stay empty here).
    pub flat: [Vec<u8>; 11],
    /// Chunks.
    pub chunks: Vec<Chunk>,
    /// Elements of the nested blocks, by header index.
    pub nested: [Vec<Nested>; 11],
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().expect("4 bytes"))
}

fn is_nested(i: usize) -> bool {
    !child_blocks(i).is_empty()
}

/// Byte ranges a decode has read, to prove the tag is covered exactly:
/// gaps are only the padding up to the next 4-byte boundary.
struct Coverage(Vec<(usize, usize)>);

impl Coverage {
    fn check(mut self, bytes: &[u8]) -> Result<()> {
        self.0.retain(|r| r.1 > 0);
        self.0.sort();
        let mut end = 0;
        for &(start, len) in &self.0 {
            if start < end {
                return Err(Error::Invalid(format!("ugh!: overlap at {start:#x}")));
            }
            // Only padding to the next 4-byte boundary. Retail maps don't
            // always zero it (two stray bytes after an odd count of 10-byte
            // pitch range parameters), but it is too short for a pointer.
            if start != end && (start - end >= 4 || start % 4 != 0) {
                return Err(Error::Invalid(format!(
                    "ugh!: bytes {end:#x}..{start:#x} belong to no known block"
                )));
            }
            end = start + len;
        }
        if bytes.len() < end || bytes.len() - end >= 4 || bytes[end..].iter().any(|&b| b != 0) {
            return Err(Error::Invalid(format!(
                "ugh!: bytes {end:#x}..{:#x} belong to no known block",
                bytes.len()
            )));
        }
        Ok(())
    }
}

impl SoundGestalt {
    /// Decodes an `ugh!` tag's bytes, loaded at `address`. Fails unless the
    /// known blocks cover the bytes exactly once (gaps only for 4-byte
    /// alignment, and zero).
    pub fn decode(bytes: &[u8], address: u32) -> Result<Self> {
        if bytes.len() < HEADER_LEN {
            return Err(Error::Invalid(format!("ugh!: {} bytes", bytes.len())));
        }
        let mut cov = Coverage(vec![(0, HEADER_LEN)]);
        let mut slice = |count: u32, at: u32, stride: usize| -> Result<Vec<u8>> {
            if count == 0 {
                return Ok(Vec::new());
            }
            if stride == 0 {
                return Err(Error::Unsupported(format!(
                    "ugh!: a child block of unknown layout has {count} elements"
                )));
            }
            let start = at.wrapping_sub(address) as usize;
            let len = count as usize * stride;
            let s = bytes
                .get(start..start.saturating_add(len))
                .ok_or_else(|| Error::Invalid(format!("ugh!: block at {at:#x} out of the tag")))?;
            cov.0.push((start, len));
            Ok(s.to_vec())
        };
        let mut g = SoundGestalt::default();
        for i in 0..11 {
            let (count, at) = (u32_at(bytes, i * 8), u32_at(bytes, i * 8 + 4));
            let elems = slice(count, at, STRIDES[i])?;
            if i == blocks::CHUNKS {
                g.chunks = elems
                    .chunks_exact(12)
                    .map(|c| Chunk {
                        pointer: ChunkPointer::Stored(u32_at(c, 0)),
                        size: u32_at(c, 4),
                        runtime: u32_at(c, 8),
                    })
                    .collect();
            } else if is_nested(i) {
                for e in elems.chunks_exact(STRIDES[i]) {
                    let mut n = Nested {
                        bytes: e.to_vec(),
                        children: Vec::new(),
                    };
                    for &(off, stride) in child_blocks(i) {
                        let (c, a) = (u32_at(e, off), u32_at(e, off + 4));
                        n.children.push(Child {
                            count: c,
                            bytes: slice(c, a, stride)?,
                        });
                        n.bytes[off..off + 8].fill(0);
                    }
                    g.nested[i].push(n);
                }
            } else {
                g.flat[i] = elems;
            }
        }
        cov.check(bytes)?;
        Ok(g)
    }

    /// Element count of block `i`.
    pub fn count(&self, i: usize) -> usize {
        if i == blocks::CHUNKS {
            self.chunks.len()
        } else if is_nested(i) {
            self.nested[i].len()
        } else {
            self.flat[i].len() / STRIDES[i]
        }
    }

    /// Element `n` of flat block `i`.
    pub fn element(&self, i: usize, n: usize) -> &[u8] {
        &self.flat[i][n * STRIDES[i]..(n + 1) * STRIDES[i]]
    }

    /// Appends an element to flat block `i`, or finds an identical one when
    /// `dedupe`; returns its index.
    pub fn push(&mut self, i: usize, element: &[u8], dedupe: bool) -> usize {
        assert_eq!(element.len(), STRIDES[i], "block {i} stride");
        if dedupe
            && let Some(n) = self.flat[i]
                .chunks_exact(STRIDES[i])
                .position(|e| e == element)
        {
            return n;
        }
        self.flat[i].extend_from_slice(element);
        self.count(i) - 1
    }

    /// The gestalt as a block tree.
    pub fn to_block(&self) -> Block {
        let mut root = Block::zeroed(HEADER_LEN);
        for i in 0..11 {
            let child = if i == blocks::CHUNKS {
                let mut b = Block::zeroed(self.chunks.len() * 12);
                for (n, c) in self.chunks.iter().enumerate() {
                    let at = n * 12;
                    match c.pointer {
                        ChunkPointer::Stored(p) => {
                            b.put_u32(at, p);
                        }
                        ChunkPointer::Local(r) => {
                            b.raw(at as u32, RawRef::Local(r));
                        }
                    }
                    b.put_u32(at + 4, c.size).put_u32(at + 8, c.runtime);
                }
                b
            } else if is_nested(i) {
                let stride = STRIDES[i];
                let mut b = Block::zeroed(self.nested[i].len() * stride);
                for (n, e) in self.nested[i].iter().enumerate() {
                    let at = n * stride;
                    b.bytes[at..at + stride].copy_from_slice(&e.bytes);
                    for (&(off, _), c) in child_blocks(i).iter().zip(&e.children) {
                        b.block((at + off) as u32, c.count, Block::new(c.bytes.clone()));
                    }
                }
                b
            } else {
                Block::new(self.flat[i].clone())
            };
            root.block((i * 8) as u32, self.count(i) as u32, child);
        }
        root
    }

    /// The `ugh!` slot of `set`, if any.
    pub fn slot(set: &TagSet) -> Option<usize> {
        set.slots
            .iter()
            .position(|s| s.tag.as_ref().is_some_and(|t| t.group == UGH))
    }

    /// Decodes the set's (pinned) `ugh!`; an empty gestalt when it has none.
    pub fn from_set(set: &TagSet) -> Result<Self> {
        let Some(slot) = Self::slot(set) else {
            return Ok(Self::default());
        };
        match &set.slots[slot].tag.as_ref().expect("a tag").payload {
            TagPayload::Pinned { address, bytes } => Self::decode(bytes, *address),
            _ => Err(Error::Invalid(
                "the set's ugh! is not pinned; keep the SoundGestalt that built it".into(),
            )),
        }
    }

    /// Writes the gestalt into `set`: in place of its `ugh!` (same slot and
    /// datum), or as a new `ugh!` tag named `name`.
    pub fn install(&self, set: &mut TagSet, name: &str) -> Result<map::TagId> {
        let payload = TagPayload::Built(self.to_block());
        if let Some(slot) = Self::slot(set) {
            let t = set.slots[slot].tag.as_mut().expect("a tag");
            t.payload = payload;
            return Ok(t.id);
        }
        set.add_group(UGH, None, None);
        Ok(set.add_tag(UGH, name, payload)?)
    }
}
