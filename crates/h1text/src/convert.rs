//! Converting Halo 1 string lists into Halo 2 localized text.
//!
//! Halo 2 keeps localized text in one place per map: nine locale tables in
//! the `matg` globals, each a global index of `(string ID, blob offset)`
//! rows over a NUL-terminated UTF-8-like blob ([`map::build`] places the
//! index and blob as raw resources). A `unic` tag just selects a row range
//! in its language's index, so a converted Halo 1 string list becomes a
//! `unic` whose English range is the run of rows minted for it, and the
//! string IDs it needs are interned into the set so other tags can name
//! them ([`tags::localization`] resolves the text).
//!
//! Halo 1 has only English text and references its lists by tag and index;
//! the converter mints one string ID per entry, named
//! `<Halo 1 tag path>\<index>` (`ustr`) or `<Halo 1 tag path>\<message
//! name>` (`hmt `), so a converted scenario can find the same text. Glyph
//! substitutions in `hmt ` messages are dropped (the Halo 2 side has its
//! own button glyphs).

use h1models::Converted;
use map::{FourCC, Map};
use map::build::{Block, TagPayload, TagSet};
use std::io::{Read, Seek};
use tags::StringId;

use crate::h1::{Hmt, Ustr};
use crate::{Error, Result};

/// The `unic` tag's size: 16 observed-zero build bytes, then nine
/// `(start, count)` language ranges.
pub const UNIC_SIZE: usize = 0x34;
/// Where the language ranges start in a `unic`.
const RANGES: usize = 0x10;
/// The `matg` locale descriptors: nine, from `+0x188`, `0x1c` each.
const MATG_DESCRIPTORS: usize = 0x188;
const SECTOR: usize = 0x200;

/// A map's English locale table under construction: the global row index
/// and blob every converted string list feeds into.
#[derive(Debug, Default)]
pub struct Locales {
    rows: Vec<(u32, u32)>,
    blob: Vec<u8>,
}

impl Locales {
    /// An empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// A table that extends an existing English table (the base map's
    /// `(string ID, blob offset)` index and its blob), so a converted
    /// level keeps every base string and its rows: new rows go after them.
    pub fn extending(index: &[u8], blob: Vec<u8>) -> Result<Self> {
        if !index.len().is_multiple_of(8) {
            return Err(Error::Invalid("locale index is not whole rows".into()));
        }
        let rows: Vec<(u32, u32)> = index
            .as_chunks::<8>()
            .0
            .iter()
            .map(|r| {
                (
                    u32::from_le_bytes(r[0..4].try_into().expect("4")),
                    u32::from_le_bytes(r[4..8].try_into().expect("4")),
                )
            })
            .collect();
        if rows.iter().any(|r| r.1 as usize >= blob.len().max(1)) {
            return Err(Error::Invalid("locale row past its blob".into()));
        }
        Ok(Self { rows, blob })
    }

    /// Reads the existing English rows and blob so that all of the base's
    /// `unic` ranges, including those with repeated keys, keep their rows.
    pub fn from_map<R: Read + Seek>(map: &mut Map<R>) -> Result<Self> {
        let globals = map.globals().ok_or_else(|| Error::Invalid("no globals".into()))?;
        let location = globals.location.ok_or_else(|| Error::Invalid("no globals data".into()))?;
        if (location.size as usize) < MATG_DESCRIPTORS + 0x1c {
            return Err(Error::Invalid("globals shorter than their English locale".into()));
        }
        let desc = map.read_file_range(location.file_offset()? + MATG_DESCRIPTORS as u64, 0x1c)?;
        let field = |at: usize| u32::from_le_bytes(desc[at..at + 4].try_into().expect("word"));
        let index_size = u64::from(field(8)) * 8;
        let blob_size = u64::from(field(0xc));
        let index = if index_size > 0 {
            map.read_file_range(u64::from(field(0x10)), index_size)?
        } else {
            Vec::new()
        };
        let blob = if blob_size > 0 {
            map.read_file_range(u64::from(field(0x14)), blob_size)?
        } else {
            Vec::new()
        };
        Self::extending(&index, blob)
    }

    /// Places the rebuilt English resources in the pinned file prefix. The
    /// globals are still pinned while the scenario is written. Scratch and
    /// prune builds relocate the resulting map's globals and resources.
    pub fn install_pinned(&self, set: &mut TagSet) -> Result<()> {
        let globals = set.globals;
        let Some(TagPayload::Pinned { bytes, .. }) = set.tag(globals).map(|tag| &tag.payload) else {
            return Err(Error::Invalid("expected pinned globals".into()));
        };
        if bytes.len() < MATG_DESCRIPTORS + 0x1c {
            return Err(Error::Invalid("globals shorter than their English locale".into()));
        }
        let pin = set.pinned.as_mut().ok_or_else(|| Error::Invalid("expected pinned map".into()))?;
        // The prefix starts immediately after the sector-aligned header.
        let header_len = pin.header.len();
        let index_at = append_locale(&mut pin.file, header_len, &self.index())?;
        let blob_at = append_locale(&mut pin.file, header_len, self.blob())?;
        let Some(TagPayload::Pinned { bytes, .. }) = set.tag_mut(globals).map(|tag| &mut tag.payload)
        else {
            unreachable!("checked globals before appending the resources");
        };
        let descriptor = &mut bytes[MATG_DESCRIPTORS..MATG_DESCRIPTORS + 0x1c];
        descriptor[..8].fill(0);
        descriptor[0x18] = 0;
        for (at, word) in [
            (8, self.rows.len() as u32),
            (0xc, self.blob.len() as u32),
            (0x10, index_at),
            (0x14, blob_at),
        ] {
            descriptor[at..at + 4].copy_from_slice(&word.to_le_bytes());
        }
        Ok(())
    }

    /// Mints one row: `name` interned into `set` as the string ID, `text`
    /// appended NUL-terminated to the blob. Returns the packed ID.
    pub fn add(&mut self, set: &mut TagSet, name: &str, text: &str) -> Result<StringId> {
        if text.chars().any(|c| c as u32 > 0xffff) {
            // The engine's blob converter narrows four-byte sequences to
            // their low 16 bits, so such text would not read back.
            return Err(Error::Unsupported(
                "text has a character outside the Basic Multilingual Plane".into(),
            ));
        }
        let key = set.intern(name)?;
        self.rows.push((key, self.blob.len() as u32));
        self.blob.extend_from_slice(text.as_bytes());
        self.blob.push(0);
        Ok(StringId(key))
    }

    /// Adds every string of `ustr`, named `<tag path>\<index>`. Returns the
    /// converted `unic` tag and the string IDs, in list order.
    pub fn add_ustr(
        &mut self,
        set: &mut TagSet,
        tag_path: &str,
        ustr: &Ustr,
    ) -> Result<(Converted, Vec<StringId>)> {
        let start = self.rows.len();
        let mut ids = Vec::with_capacity(ustr.strings.len());
        for (i, text) in ustr.strings.iter().enumerate() {
            ids.push(self.add(set, &format!("{tag_path}\\{i}"), text)?);
        }
        Ok((self.unic(start, &ids)?, ids))
    }

    /// Adds every message of `hmt`, named `<tag path>\<message name>`, its
    /// glyphs dropped. Returns the converted `unic` tag and the string IDs,
    /// in message order.
    pub fn add_hmt(
        &mut self,
        set: &mut TagSet,
        tag_path: &str,
        hmt: &Hmt,
    ) -> Result<(Converted, Vec<StringId>)> {
        for (i, message) in hmt.messages.iter().enumerate() {
            if hmt.messages[..i].iter().any(|m| m.name == message.name) {
                return Err(Error::Invalid(format!(
                    "duplicate message name {:?}: one unic range cannot hold two rows with the same ID",
                    message.name
                )));
            }
        }
        let start = self.rows.len();
        let mut ids = Vec::with_capacity(hmt.messages.len());
        for message in &hmt.messages {
            ids.push(self.add(
                set,
                &format!("{tag_path}\\{}", message.name),
                &message.plain_text(),
            )?);
        }
        Ok((self.unic(start, &ids)?, ids))
    }

    /// The `unic` for a run of rows: 16 zero build bytes, then the English
    /// range `[start, start + ids.len())` and eight empty ranges.
    fn unic(&self, start: usize, ids: &[StringId]) -> Result<Converted> {
        let start = i16::try_from(start)
            .map_err(|_| Error::Invalid("too many locale rows for a unic range".into()))?;
        let count = i16::try_from(ids.len())
            .map_err(|_| Error::Invalid("too many strings for a unic range".into()))?;
        let mut root = Block::zeroed(UNIC_SIZE);
        root.put_u16(RANGES, start as u16);
        root.put_u16(RANGES + 2, count as u16);
        Ok(Converted {
            group: FourCC(*b"unic"),
            root,
            raw: Vec::new(),
        })
    }

    /// The global row index: `(string ID, blob offset)` pairs, 8 bytes each.
    pub fn index(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.rows.len() * 8);
        for (key, offset) in &self.rows {
            out.extend_from_slice(&key.to_le_bytes());
            out.extend_from_slice(&offset.to_le_bytes());
        }
        out
    }

    /// The blob: every string NUL-terminated.
    pub fn blob(&self) -> &[u8] {
        &self.blob
    }

    /// The number of rows.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether no row was added.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Writes the English locale descriptor into a `matg` block: row count
    /// and blob size, then the file offsets of the index and blob raw
    /// resources from a first [`TagSet::build`] (the other eight languages
    /// stay zero: no rows, no blob). Building again with these values in
    /// place yields the same layout, since only field values changed.
    pub fn patch_matg(&self, matg: &mut Block, index_offset: u32, blob_offset: u32) {
        let at = MATG_DESCRIPTORS;
        matg.put_u32(at + 0x08, self.rows.len() as u32);
        matg.put_u32(at + 0x0c, self.blob.len() as u32);
        matg.put_u32(at + 0x10, index_offset);
        matg.put_u32(at + 0x14, blob_offset);
    }
}

fn append_locale(prefix: &mut Vec<u8>, header_len: usize, bytes: &[u8]) -> Result<u32> {
    let start = prefix.len().next_multiple_of(SECTOR);
    let offset = start + header_len;
    let end = (start + bytes.len()).next_multiple_of(SECTOR);
    if end + header_len >= 1 << 30 {
        return Err(Error::Invalid("locale exceeds the map's raw address space".into()));
    }
    prefix.resize(start, 0);
    prefix.extend_from_slice(bytes);
    prefix.resize(end, 0);
    Ok(offset as u32)
}
