//! A pass-through plus patches, not a rebuild. Preserve all bytes, including unknown fields, resources and padding. Only
//! explicitly queued edits and the resulting length/name-offset/checksum fields
//! change. Variable-length tag names are appended; opaque metadata/raw pointers
//! keep their original addresses. This is not a general tag-graph relocator.
use crate::{Error, Map, Result, TagId, checksum::XorChecksum};
use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom, Write};

const CHUNK: u64 = 1024 * 1024;
const MAX_TABLE: usize = 64 * 1024 * 1024;

impl<R: Read + Seek> Map<R> {
    /// Write the entire original bounded input at the destination's current
    /// position, including physical trailing padding. Does not repair checksums.
    /// The destination must not alias the source. No files are opened for writing.
    pub fn write_to(&mut self, out: &mut impl Write) -> Result<u64> {
        let mut at = 0;
        while at < self.storage_length() {
            let n = (self.storage_length() - at).min(CHUNK);
            out.write_all(&self.read_storage_range(at, n)?)?;
            at += n;
        }
        Ok(at)
    }
    /// Begin a bounded edit transaction. Dropping it leaves the source unchanged.
    pub fn edit(&mut self) -> MapEditor<'_, R> {
        MapEditor {
            map: self,
            patches: BTreeMap::new(),
            names: BTreeMap::new(),
        }
    }
}

/// Edits refer to full salted tag IDs and offsets relative to each tag payload.
/// Writes stream from the source; they do not mutate it. A transaction rejects
/// overlapping patches. Callers own field semantics (especially pointers).
pub struct MapEditor<'a, R> {
    map: &'a mut Map<R>,
    patches: BTreeMap<u64, Vec<u8>>,
    names: BTreeMap<usize, String>,
}
impl<R: Read + Seek> MapEditor<'_, R> {
    /// Inspect the immutable source map (queued edits are not reflected).
    pub fn source(&self) -> &Map<R> {
        self.map
    }

    /// Read a bounded source tag range; queued patches are not reflected.
    pub fn read_tag_range(&mut self, id: TagId, offset: u32, size: u32) -> Result<Vec<u8>> {
        let at = self.tag_range(id, offset, size)?;
        self.map.read_file_range(at, size.into())
    }
    fn tag_range(&self, id: TagId, offset: u32, size: u32) -> Result<u64> {
        let loc = self.map.tag(id)?.location.ok_or(Error::NoTagData)?;
        if offset > loc.size || size > loc.size - offset {
            return Err(Error::Bounds {
                what: "tag edit",
                offset: offset.into(),
                size: size.into(),
                limit: loc.size.into(),
            });
        }
        let at = loc.file_offset()? + u64::from(offset);
        if at < 0x800 {
            return Err(Error::Invalid("tag edit overlaps header"));
        }
        Ok(at)
    }
    /// Queue a fixed-length replacement inside a tag. The tag index, sizes and
    /// metadata pointers stay unchanged. Unknown fields are untouched.
    pub fn patch_tag(&mut self, id: TagId, offset: u32, bytes: &[u8]) -> Result<()> {
        if bytes.len() > MAX_TABLE {
            return Err(Error::Limit);
        }
        let size = u32::try_from(bytes.len()).map_err(|_| Error::Limit)?;
        let at = self.tag_range(id, offset, size)?;
        if bytes.is_empty() {
            return Ok(());
        }
        // Patches must not corrupt structures the reader re-parses.
        let end = at + u64::from(size);
        if self
            .protected_ranges()
            .iter()
            .any(|&(lo, hi)| at < hi && lo < end)
        {
            return Err(Error::Invalid("tag edit overlaps index or string table"));
        }
        if let Some((&start, data)) = self.patches.range(..at + u64::from(size)).next_back()
            && start + data.len() as u64 > at
        {
            return Err(Error::Invalid("overlapping tag edits"));
        }
        self.patches.insert(at, bytes.to_vec());
        Ok(())
    }
    /// Byte ranges (start, end) of the tag index and the string/name tables.
    fn protected_ranges(&self) -> Vec<(u64, u64)> {
        let h = self.map.header();
        let mut r = vec![(
            u64::from(h.index_offset),
            u64::from(h.index_offset) + u64::from(h.index_size),
        )];
        for t in [h.strings, h.tag_names] {
            if t.count != 0 {
                let i = u64::from(t.index_offset);
                let d = u64::from(t.data_offset);
                r.push((i, i + u64::from(t.count) * 4));
                r.push((d, d + u64::from(t.data_size)));
            }
        }
        r
    }
    /// Rename a live tag. Rebuilds the complete name blob and offset array at
    /// aligned EOF on write. Null-slot names, empty/missing names and duplicate
    /// names are preserved. String IDs are a separate table and are unchanged.
    pub fn rename_tag(&mut self, id: TagId, name: &str) -> Result<()> {
        self.map.tag(id)?;
        if name.as_bytes().contains(&0) {
            return Err(Error::Invalid("NUL in tag name"));
        }
        if name.len() >= MAX_TABLE {
            return Err(Error::Limit);
        }
        self.names.insert(id.index(), name.to_owned());
        Ok(())
    }
    /// Write at the destination's current position and refresh +0x2d0 if edited.
    /// Output must be empty at/after that position, and must not alias the source.
    /// The caller handles truncation/atomic replacement and flush/sync policy.
    /// Returns the byte length written, leaving the cursor at the end.
    pub fn write_to(mut self, out: &mut (impl Write + Seek)) -> Result<u64> {
        if self.patches.is_empty() && self.names.is_empty() {
            return self.map.write_to(out);
        }
        // Header validator 0x122870 / per-cache limits at 0x214ed0.
        let limit = match self.map.header().cache_type {
            crate::CacheType::Campaign => 0x1180_0000,
            crate::CacheType::Multiplayer | crate::CacheType::MainMenu => 0x0500_0000,
            crate::CacheType::Shared => 0x0b40_0000,
            crate::CacheType::SinglePlayerShared => 0x2080_0000,
            crate::CacheType::Unknown(_) => {
                return Err(Error::Invalid("unknown cache type for editing"));
            }
        };
        if self.map.storage_length() > limit {
            return Err(Error::Invalid("source exceeds engine cache size limit"));
        }
        // The header is rewritten (checksum, and name-table fields), so no table
        // the reader parses may live inside it.
        let h = self.map.header();
        for t in [h.strings, h.tag_names] {
            if t.count != 0 {
                let i = u64::from(t.index_offset);
                let d = u64::from(t.data_offset);
                if i < 0x800 || (d < 0x800 && t.data_size != 0) {
                    return Err(Error::Invalid("string or name table inside header"));
                }
            }
        }
        let origin = out.stream_position()?;
        let mut header = self.map.read_file_range(0, 0x800)?;
        let append = self.rebuild_names(&mut header)?;
        let length = self.map.storage_length() + append.len() as u64;
        let end = origin.checked_add(length).ok_or(Error::Limit)?;
        if length > limit {
            return Err(Error::Invalid("edited map exceeds engine cache size limit"));
        }
        out.write_all(&header)?;
        let mut checksum = XorChecksum::default();
        let mut at = 0x800;
        while at < self.map.storage_length() {
            let n = (self.map.storage_length() - at).min(CHUNK);
            let mut bytes = self.map.read_storage_range(at, n)?;
            for (&start, patch) in self.patches.range(..at + n) {
                let end = start + patch.len() as u64;
                if end <= at {
                    continue;
                }
                let lo = start.max(at);
                let hi = end.min(at + n);
                bytes[(lo - at) as usize..(hi - at) as usize]
                    .copy_from_slice(&patch[(lo - start) as usize..(hi - start) as usize]);
            }
            checksum.update(&bytes);
            out.write_all(&bytes)?;
            at += n;
        }
        checksum.update(&append);
        out.write_all(&append)?;
        out.seek(SeekFrom::Start(
            origin.checked_add(0x2d0).ok_or(Error::Limit)?,
        ))?;
        out.write_all(&checksum.finish().to_le_bytes())?;
        out.seek(SeekFrom::Start(end))?;
        Ok(length)
    }
    fn rebuild_names(&mut self, header: &mut [u8]) -> Result<Vec<u8>> {
        if self.names.is_empty() {
            return Ok(Vec::new());
        }
        let table = self.map.header().tag_names;
        let index = self
            .map
            .read_file_range(table.index_offset.into(), u64::from(table.count) * 4)?;
        let blob = self
            .map
            .read_file_range(table.data_offset.into(), table.data_size.into())?;
        let mut offsets = Vec::new();
        let mut names = Vec::new();
        for slot in 0..self.map.index_slots() as usize {
            let name = if let Some(name) = self.names.get(&slot) {
                Some(name.as_bytes())
            } else if slot < table.count as usize {
                let p = u32::from_le_bytes(index[slot * 4..slot * 4 + 4].try_into().unwrap());
                if p == u32::MAX {
                    None
                } else {
                    let tail = blob
                        .get(p as usize..)
                        .ok_or(Error::Invalid("tag-name offset"))?;
                    let n = tail
                        .iter()
                        .position(|&b| b == 0)
                        .ok_or(Error::Invalid("tag-name terminator"))?;
                    Some(&tail[..n])
                }
            } else {
                None
            };
            let offset = if let Some(name) = name {
                if names
                    .len()
                    .checked_add(name.len() + 1)
                    .is_none_or(|n| n > MAX_TABLE)
                {
                    return Err(Error::Limit);
                }
                let offset = names.len() as u32;
                names.extend_from_slice(name);
                names.push(0);
                offset
            } else {
                u32::MAX
            };
            offsets.extend_from_slice(&offset.to_le_bytes());
        }
        let base = self.map.storage_length();
        let padding = (512 - base % 512) % 512;
        let data_offset = base + padding;
        let index_offset = (data_offset + names.len() as u64 + 511) & !511;
        let length = (index_offset + offsets.len() as u64 + 511) & !511;
        let length32 = u32::try_from(length).map_err(|_| Error::Limit)?;
        let mut append = vec![0; usize::try_from(length - base).map_err(|_| Error::Limit)?];
        append[padding as usize..padding as usize + names.len()].copy_from_slice(&names);
        let at = (index_offset - base) as usize;
        append[at..at + offsets.len()].copy_from_slice(&offsets);
        for (at, value) in [
            (8, length32),
            (0x2c0, self.map.index_slots()),
            (0x2c4, data_offset as u32),
            (0x2c8, names.len() as u32),
            (0x2cc, index_offset as u32),
        ] {
            header[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        Ok(append)
    }
}
