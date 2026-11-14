//! Halo 2 retail Xbox localization, derived from cache bytes and the executable.
//!
//! Provenance for each table and the conversion algorithm: `docs/localization.md`.
//! Cache blobs contain NUL-terminated UTF-8-like bytes, not UTF-16. The engine
//! converts these to 16-bit units. No game strings or external layouts are built in.
use crate::{Error, Result, records::LocaleDescriptor, wire::BinaryRecord};
use map::{FourCC, Map, TagId};
use std::io::{Read, Seek};

pub use crate::wire::{StringId, StringIdParts};

/// Slots in `matg` and `unic`, not the Xbox dashboard's one-based language IDs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Language {
    English,
    Japanese,
    German,
    French,
    Spanish,
    Italian,
    Korean,
    Chinese,
    Portuguese,
}
impl Language {
    pub const ALL: [Self; 9] = [
        Self::English,
        Self::Japanese,
        Self::German,
        Self::French,
        Self::Spanish,
        Self::Italian,
        Self::Korean,
        Self::Chinese,
        Self::Portuguese,
    ];
    pub const fn code(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::Japanese => "ja",
            Self::German => "de",
            Self::French => "fr",
            Self::Spanish => "es",
            Self::Italian => "it",
            Self::Korean => "ko",
            Self::Chinese => "zh",
            Self::Portuguese => "pt",
        }
    }
}
impl std::str::FromStr for Language {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|l| l.code().eq_ignore_ascii_case(s))
            .ok_or(Error::Invalid(
                "language must be en, ja, de, fr, es, it, ko, zh or pt",
            ))
    }
}

/// Signed shorts consumed by the engine at `unic + 0x10 + language * 4`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LanguageRange {
    pub start: i16,
    pub count: i16,
}
impl LanguageRange {
    fn checked(self, length: usize) -> Result<std::ops::Range<usize>> {
        // Empty spans do not dereference their start (which can be -1).
        if self.count == 0 {
            return Ok(0..0);
        }
        if self.start < 0 || self.count < 0 {
            return Err(Error::Invalid("negative unic language range"));
        }
        let start = self.start as usize;
        let end = start + self.count as usize;
        if end > length {
            return Err(Error::Invalid("unic language range outside locale index"));
        }
        Ok(start..end)
    }
}

/// Compiled Xbox multilingual Unicode string list. Each language selects a
/// contiguous span in the corresponding global locale index; no text is inline.
#[derive(Clone, Debug)]
pub struct Unic {
    pub ranges: [LanguageRange; 9],
}
impl Unic {
    pub const SIZE: usize = 0x34;
    /// Decode the retail cache form. Its first 16 bytes are observed-zero build
    /// fields; nonzero forms are unsupported, not interpreted as editor layouts.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        crate::wire::exact(bytes.len(), Self::SIZE)?;
        if bytes[..16].iter().any(|b| *b != 0) {
            return Err(Error::Invalid("unsupported nonzero unic build fields"));
        }
        let mut ranges = [LanguageRange::default(); 9];
        for (range, b) in ranges.iter_mut().zip(bytes[16..].as_chunks::<4>().0.iter()) {
            *range = LanguageRange {
                start: i16::from_le_bytes([b[0], b[1]]),
                count: i16::from_le_bytes([b[2], b[3]]),
            };
            range.checked(usize::MAX)?;
        }
        Ok(Self { ranges })
    }
    pub fn read<R: Read + Seek>(map: &mut Map<R>, id: TagId) -> Result<Self> {
        let tag = map.tag(id)?;
        if tag.group != FourCC(*b"unic") {
            return Err(Error::UnsupportedGroup(tag.group));
        }
        let location = tag.location.ok_or(map::Error::NoTagData)?;
        crate::wire::exact(location.size as usize, Self::SIZE)?;
        Self::decode(&map.read_tag_bytes(id)?)
    }
}

/// A full packed key and byte range in one locale's blob. Keys can repeat across
/// `unic` ranges and may refer to different text. The byte length excludes NUL.
#[derive(Clone, Copy, Debug)]
pub struct LocaleEntry {
    pub id: StringId,
    pub offset: u32,
    length: u32,
}
impl LocaleEntry {
    pub fn byte_length(self) -> u32 {
        self.length
    }
}

/// Owned table bytes; entries keep offsets so repeated offsets cannot amplify
/// allocations into repeated strings. Construction validates every terminator.
#[derive(Clone, Debug)]
pub struct LocaleTable {
    entries: Vec<LocaleEntry>,
    blob: Vec<u8>,
    non_utf8: usize,
}
const MAX_BYTES: u64 = 64 * 1024 * 1024;
impl LocaleTable {
    pub fn decode(index: &[u8], blob: Vec<u8>) -> Result<Self> {
        if !index.len().is_multiple_of(8) {
            return Err(Error::Invalid("locale index stride"));
        }
        if index.len() as u64 + blob.len() as u64 > MAX_BYTES {
            return Err(map::Error::Limit.into());
        }
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(index.len() / 8)
            .map_err(|_| map::Error::Limit)?;
        let mut non_utf8 = 0;
        let mut scanned = 0u64;
        for row in index.as_chunks::<8>().0 {
            let id = StringId(u32::from_le_bytes(row[..4].try_into().unwrap()));
            let offset = u32::from_le_bytes(row[4..].try_into().unwrap());
            let tail = blob
                .get(offset as usize..)
                .ok_or(Error::Invalid("locale string offset"))?;
            // Bound work as well as allocation for hostile repeated offsets.
            let budget = (MAX_BYTES - scanned) as usize;
            let length =
                tail.iter()
                    .take(budget)
                    .position(|b| *b == 0)
                    .ok_or(if tail.len() > budget {
                        Error::Invalid("locale string scan limit exceeded")
                    } else {
                        Error::Invalid("unterminated locale string")
                    })?;
            scanned += length as u64 + 1;
            non_utf8 += usize::from(std::str::from_utf8(&tail[..length]).is_err());
            entries.push(LocaleEntry {
                id,
                offset,
                length: length as u32,
            });
        }
        Ok(Self {
            entries,
            blob,
            non_utf8,
        })
    }
    pub fn entries(&self) -> &[LocaleEntry] {
        &self.entries
    }
    /// Retail truncation is preserved and reported, not rejected or replaced.
    pub fn non_utf8_entries(&self) -> usize {
        self.non_utf8
    }
    pub fn bytes(&self, index: usize) -> Option<&[u8]> {
        let e = self.entries.get(index)?;
        self.blob
            .get(e.offset as usize..e.offset as usize + e.length as usize)
    }
    /// Full text, without the UI wrapper's 255-unit output truncation or NUL.
    pub fn utf16_at(&self, index: usize) -> Option<Vec<u16>> {
        Some(engine_wide(self.bytes(index)?))
    }
    /// Rust strings cannot represent isolated UTF-16 surrogates; those become
    /// U+FFFD. Use `utf16_at` to preserve exact engine code units for modded data.
    pub fn text_at(&self, index: usize) -> Option<String> {
        Some(String::from_utf16_lossy(&self.utf16_at(index)?))
    }
    fn find(&self, id: StringId, range: std::ops::Range<usize>) -> Option<usize> {
        self.entries
            .get(range.clone())?
            .iter()
            .position(|e| e.id == id)
            .map(|i| range.start + i)
    }
}

/// All nine locales for one map, loaded with map-relative file offsets.
#[derive(Clone, Debug)]
pub struct Localization {
    /// Serialized descriptors, not live host pointers. Only the low byte of
    /// each `loaded` word is the engine flag; the upper three bytes are opaque.
    pub descriptors: [LocaleDescriptor; 9],
    tables: [LocaleTable; 9],
}
impl Localization {
    /// Load locale structure and bytes without requiring debug identifier names.
    /// Use `StringId::resolve` or CLI `strings --all` to validate names separately.
    pub fn read<R: Read + Seek>(map: &mut Map<R>) -> Result<Self> {
        let globals = map
            .globals()
            .ok_or(Error::Invalid("map has no globals tag"))?;
        let location = globals.location.ok_or(map::Error::NoTagData)?;
        if location.size < 0x284 {
            return Err(Error::Invalid("short matg locales"));
        }
        // Only the embedded descriptors are needed, not matg's child payloads.
        let descriptor_bytes = map.read_file_range(location.file_offset()? + 0x188, 9 * 0x1c)?;
        let mut descriptors = Vec::new();
        let mut tables = Vec::new();
        let mut total = 0u64;
        for b in descriptor_bytes.as_chunks::<{ LocaleDescriptor::SIZE }>().0 {
            let d = LocaleDescriptor::decode(b)?;
            let index_size = u64::from(d.count) * 8;
            total += index_size + u64::from(d.data_size);
            if total > MAX_BYTES {
                return Err(map::Error::Limit.into());
            }
            let index = if index_size == 0 {
                Vec::new()
            } else {
                map.read_file_range(d.index_file_offset.into(), index_size)?
            };
            let blob = if d.data_size == 0 {
                Vec::new()
            } else {
                map.read_file_range(d.data_file_offset.into(), d.data_size.into())?
            };
            let table = LocaleTable::decode(&index, blob)?;
            // Identifier names are debug metadata, not required by the engine's
            // full-key locale lookup. The CLI's --all pass checks them separately.
            drop(index);
            descriptors.push(d);
            tables.push(table);
        }
        Ok(Self {
            descriptors: descriptors.try_into().expect("nine descriptor slots"),
            tables: tables.try_into().expect("nine locale tables"),
        })
    }
    pub fn table(&self, language: Language) -> &LocaleTable {
        &self.tables[language as usize]
    }
    /// Inspection only: returns the first full-key match in the whole language
    /// table. Conflicting duplicate IDs occur on disc, so this can return the
    /// wrong text for a widget. Use `unic` and `text_in` for widget lookups.
    /// No implicit English fallback or UI truncation; a miss returns None.
    pub fn first_text(&self, id: StringId, language: Language) -> Option<String> {
        let table = self.table(language);
        table.text_at(table.find(id, 0..table.entries.len())?)
    }
    /// Inspection only: returns the first full-key match in the whole language
    /// table. Conflicting duplicate IDs occur on disc, so this can return the
    /// wrong text for a widget. Use `unic` and `utf16_in` for widget lookups.
    /// No implicit English fallback or UI truncation; a miss returns None.
    pub fn first_utf16(&self, id: StringId, language: Language) -> Option<Vec<u16>> {
        let table = self.table(language);
        table.utf16_at(table.find(id, 0..table.entries.len())?)
    }
    /// Read a compiled unic and validate all nine ranges against this locale.
    /// Supply the same map used to load this Localization.
    pub fn unic<R: Read + Seek>(&self, map: &mut Map<R>, id: TagId) -> Result<Unic> {
        let unic = Unic::read(map, id)?;
        self.validate_unic(&unic)?;
        Ok(unic)
    }
    /// Validate all language ranges against their respective index counts.
    pub fn validate_unic(&self, unic: &Unic) -> Result<()> {
        for language in Language::ALL {
            unic.ranges[language as usize].checked(self.table(language).entries.len())?;
        }
        Ok(())
    }
    pub fn entries_for(&self, unic: &Unic, language: Language) -> Result<&[LocaleEntry]> {
        let table = self.table(language);
        let range = unic.ranges[language as usize].checked(table.entries.len())?;
        Ok(&table.entries[range])
    }
    /// Scoped lookup: Ok(None) is a missing key; Err means the selected range
    /// is corrupt. An existing empty translation is Ok(Some(String::new())).
    pub fn text_in(&self, unic: &Unic, id: StringId, language: Language) -> Result<Option<String>> {
        Ok(self
            .utf16_in(unic, id, language)?
            .map(|units| String::from_utf16_lossy(&units)))
    }
    /// Scoped code-unit lookup: Ok(None) is a missing key; Err means the
    /// selected range is corrupt. No UI truncation or terminating NUL is added.
    pub fn utf16_in(
        &self,
        unic: &Unic,
        id: StringId,
        language: Language,
    ) -> Result<Option<Vec<u16>>> {
        let table = self.table(language);
        let range = unic.ranges[language as usize].checked(table.entries.len())?;
        Ok(table.find(id, range).and_then(|row| table.utf16_at(row)))
    }
}

// Derived from 0x140440, not a standards-compliant UTF-8 decoder. Invalid input
// advances one byte without output. Accepted multibyte sequences store low 16
// bits, including overlongs/surrogates; four-byte scalars do not form a pair.
// Bounds checks replace the original converter's unbounded continuation reads.
fn engine_wide(bytes: &[u8]) -> Vec<u16> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() && bytes[i] != 0 {
        let first = bytes[i];
        let (width, mut value) = match first {
            0..=0x7f => (1, u32::from(first)),
            0xc0..=0xdf => (2, u32::from(first & 0x1f)),
            0xe0..=0xef => (3, u32::from(first & 0x0f)),
            0xf0..=0xf7 => (4, u32::from(first & 7)),
            _ => {
                i += 1;
                continue;
            }
        };
        if let Some(seq) = bytes.get(i..i + width)
            && seq[1..].iter().all(|b| b & 0xc0 == 0x80)
        {
            for b in &seq[1..] {
                value = value << 6 | u32::from(b & 0x3f);
            }
            out.push(value as u16);
            i += width;
        } else {
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests;
