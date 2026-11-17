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
use map::FourCC;
use map::build::{Block, TagSet};
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
