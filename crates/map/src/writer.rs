//! A pass-through plus patches, not a rebuild. Preserve all bytes, including unknown fields, resources and padding. Only
//! explicitly queued edits and the resulting length/name-offset/checksum fields
//! change. Variable-length tag names are appended; opaque metadata/raw pointers
//! keep their original addresses. New metadata bytes may be appended after the
//! last tag (growing the ordinary metadata region in place, see
//! [`MapEditor::append_metadata`]); nothing that exists moves. This is not a
//! general tag-graph relocator.
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
            appended: Vec::new(),
            strings: Vec::new(),
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
    /// Bytes appended after the ordinary metadata region, unpadded.
    appended: Vec<u8>,
    /// New string IDs, after the source's.
    strings: Vec<String>,
}

/// Growth granularity of the ordinary metadata region: every size the
/// engine reads for it (`+0x18`, `+0x1c`) and the file length stay
/// multiples of this when they were before (all retail maps are).
pub const METADATA_GROWTH_ALIGN: u32 = 0x1000;
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
        self.queue(at, bytes)
    }
    /// Queue a fixed-length replacement at an ordinary metadata virtual
    /// address (any tag's bytes, or bytes between tags). The range must lie
    /// inside the source's ordinary metadata region; bytes queued with
    /// [`Self::append_metadata`] are not patchable (pass their final value).
    pub fn patch_metadata(&mut self, address: u32, bytes: &[u8]) -> Result<()> {
        if bytes.len() > MAX_TABLE {
            return Err(Error::Limit);
        }
        let size = u32::try_from(bytes.len()).map_err(|_| Error::Limit)?;
        let space = self
            .map
            .metadata_space()
            .ok_or(Error::Invalid("map has no metadata region"))?;
        let at = space.translate(address, size)?;
        if at < 0x800 {
            return Err(Error::Invalid("tag edit overlaps header"));
        }
        self.queue(at, bytes)
    }
    fn queue(&mut self, at: u64, bytes: &[u8]) -> Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let size = bytes.len() as u64;
        // Patches must not corrupt structures the reader re-parses.
        let end = at + size;
        if self
            .protected_ranges()
            .iter()
            .any(|&(lo, hi)| at < hi && lo < end)
        {
            return Err(Error::Invalid("tag edit overlaps index or string table"));
        }
        if let Some((&start, data)) = self.patches.range(..end).next_back()
            && start + data.len() as u64 > at
        {
            return Err(Error::Invalid("overlapping tag edits"));
        }
        self.patches.insert(at, bytes.to_vec());
        Ok(())
    }

    /// Append `bytes` to the ordinary metadata region, after every existing
    /// tag, and return the virtual address of their first byte (aligned to
    /// `align`, a power of two up to 0x1000). The caller then points a block
    /// at them with [`Self::patch_metadata`]; no tag owns the new bytes.
    ///
    /// The region is the last thing in a retail cache file, and its virtual
    /// base is the first tag's address, so growing it moves nothing: on
    /// write the region size (`+0x18`), the total metadata size (`+0x1c`)
    /// and the file length (`+8`) grow by the appended bytes rounded up to
    /// [`METADATA_GROWTH_ALIGN`] (zero padded). Refused when the region does
    /// not end the file, when the file has trailing bytes past its declared
    /// length, or when the grown sizes exceed the engine's limits
    /// ([`max_metadata_size`] for `+0x1c`, and the per-type cache size).
    pub fn append_metadata(&mut self, bytes: &[u8], align: u32) -> Result<u32> {
        if !align.is_power_of_two() || align > METADATA_GROWTH_ALIGN {
            return Err(Error::Invalid("append alignment"));
        }
        let h = self.map.header();
        let space = self
            .map
            .metadata_space()
            .ok_or(Error::Invalid("map has no metadata region"))?;
        let region_end = u64::from(h.index_offset)
            + u64::from(h.index_size)
            + u64::from(self.map.metadata_size());
        if region_end != u64::from(h.file_size)
            || self.map.storage_length() != u64::from(h.file_size)
        {
            return Err(Error::Invalid(
                "metadata region does not end the file; cannot grow it",
            ));
        }
        let pad = (align as usize - self.appended.len() % align as usize) % align as usize;
        let start = self.appended.len() + pad;
        let total = start.checked_add(bytes.len()).ok_or(Error::Limit)?;
        let (growth, ..) = self.grown_sizes(total)?;
        // Nothing else may already use the grown addresses (a BSP's own
        // space; on retail maps those sit below the ordinary metadata).
        let lo = u64::from(space.virtual_base) + u64::from(space.size);
        let hi = lo + u64::from(growth);
        if self.map.tags().iter().filter_map(|t| t.location).any(|l| {
            let a = u64::from(l.space.virtual_base);
            l.space != space && a < hi && lo < a + u64::from(l.space.size)
        }) {
            return Err(Error::Invalid("grown metadata would overlap a BSP"));
        }
        let address = space
            .virtual_base
            .checked_add(space.size)
            .and_then(|a| a.checked_add(u32::try_from(start).ok()?))
            .ok_or(Error::Limit)?;
        self.appended.resize(start, 0);
        self.appended.extend_from_slice(bytes);
        Ok(address)
    }
    /// The bytes [`Self::append_metadata`] has queued, before padding.
    pub fn appended_len(&self) -> usize {
        self.appended.len()
    }
    /// Header sizes after appending `n` bytes: (growth, new `+0x18`, new
    /// `+0x1c`, new file length), checked against the engine's limits.
    fn grown_sizes(&self, n: usize) -> Result<(u32, u32, u32, u32)> {
        let h = self.map.header();
        let a = METADATA_GROWTH_ALIGN as usize;
        let growth = n.div_ceil(a).checked_mul(a).ok_or(Error::Limit)?;
        let growth = u32::try_from(growth).map_err(|_| Error::Limit)?;
        let data = self
            .map
            .metadata_size()
            .checked_add(growth)
            .ok_or(Error::Limit)?;
        let total = h.meta_size.checked_add(growth).ok_or(Error::Limit)?;
        let file = h.file_size.checked_add(growth).ok_or(Error::Limit)?;
        if growth != 0 && total > max_metadata_size(h.cache_type)? {
            return Err(Error::Invalid(
                "grown metadata exceeds the engine's tag memory",
            ));
        }
        if growth != 0 && u64::from(file) > cache_size_limit(h.cache_type)? {
            return Err(Error::Invalid("edited map exceeds engine cache size limit"));
        }
        Ok((growth, data, total, file))
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
    /// Add a string ID after the map's own (and those added before) and
    /// return its value (index in the low 24 bits, byte length in the top
    /// 8). On write the three string tables (the fixed 0x80-byte slots,
    /// the offset index and the blob) are rewritten whole at the end of
    /// the file, the source's entries first and unchanged, and the header's
    /// `+0x160..+0x174` pointed at them; the old tables stay as dead bytes.
    /// A string the map already has is refused: use its value.
    pub fn add_string(&mut self, value: &str) -> Result<u32> {
        if value.is_empty() || value.as_bytes().contains(&0) {
            return Err(Error::Invalid("string ID empty or with a NUL"));
        }
        if value.len() >= crate::build::FIXED_STRING_SLOT {
            return Err(Error::Invalid("string ID too long for its fixed slot"));
        }
        let have = |s: &Option<String>| s.as_deref() == Some(value);
        if self.map.strings().iter().any(have) || self.strings.iter().any(|s| s == value) {
            return Err(Error::Invalid("the map already has that string ID"));
        }
        let index = self.map.strings().len() + self.strings.len();
        let id = crate::build::string_id(index, value)?;
        self.strings.push(value.to_owned());
        Ok(id)
    }
    /// Write at the destination's current position and refresh +0x2d0 if edited.
    /// Output must be empty at/after that position, and must not alias the source.
    /// The caller handles truncation/atomic replacement and flush/sync policy.
    /// Returns the byte length written, leaving the cursor at the end.
    pub fn write_to(mut self, out: &mut (impl Write + Seek)) -> Result<u64> {
        if self.patches.is_empty()
            && self.names.is_empty()
            && self.appended.is_empty()
            && self.strings.is_empty()
        {
            return self.map.write_to(out);
        }
        let limit = cache_size_limit(self.map.header().cache_type)?;
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
        let mut grown = Vec::new();
        if !self.appended.is_empty() {
            let (growth, data, total, file) = self.grown_sizes(self.appended.len())?;
            grown = std::mem::take(&mut self.appended);
            grown.resize(growth as usize, 0);
            for (at, value) in [(8, file), (0x18, data), (0x1c, total)] {
                header[at..at + 4].copy_from_slice(&value.to_le_bytes());
            }
        }
        let names_base = self.map.storage_length() + grown.len() as u64;
        let mut append = grown;
        append.extend(self.rebuild_names(&mut header, names_base)?);
        let strings_base = self.map.storage_length() + append.len() as u64;
        append.extend(self.rebuild_strings(&mut header, strings_base)?);
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
    /// The string-ID tables with the added strings, placed from file offset
    /// `base` (the end of everything written before them), each sector
    /// aligned as the name tables are.
    fn rebuild_strings(&mut self, header: &mut [u8], base: u64) -> Result<Vec<u8>> {
        if self.strings.is_empty() {
            return Ok(Vec::new());
        }
        const SLOT: usize = crate::build::FIXED_STRING_SLOT;
        let table = self.map.header().strings;
        let fixed_offset = self.map.header().fixed_strings_offset;
        let count = table.count as usize;
        let mut index = self
            .map
            .read_file_range(table.index_offset.into(), table.count as u64 * 4)?;
        let mut blob = self
            .map
            .read_file_range(table.data_offset.into(), table.data_size.into())?;
        // The source's fixed slots as stored; a map without them gets them
        // from its strings.
        let mut fixed = if fixed_offset != 0 {
            self.map
                .read_file_range(fixed_offset.into(), (count * SLOT) as u64)?
        } else {
            let mut f = vec![0; count * SLOT];
            for (i, s) in self.map.strings().iter().enumerate() {
                if let Some(s) = s.as_deref().filter(|s| s.len() < SLOT) {
                    f[i * SLOT..][..s.len()].copy_from_slice(s.as_bytes());
                }
            }
            f
        };
        for s in &self.strings {
            let offset = u32::try_from(blob.len()).map_err(|_| Error::Limit)?;
            index.extend_from_slice(&offset.to_le_bytes());
            blob.extend_from_slice(s.as_bytes());
            blob.push(0);
            let mut slot = [0u8; SLOT];
            slot[..s.len()].copy_from_slice(s.as_bytes());
            fixed.extend_from_slice(&slot);
        }
        if blob.len() > MAX_TABLE || fixed.len() > MAX_TABLE {
            return Err(Error::Limit);
        }
        let sector = |n: u64| (n + 511) & !511;
        let fixed_at = sector(base);
        let index_at = sector(fixed_at + fixed.len() as u64);
        let blob_at = sector(index_at + index.len() as u64);
        let end = sector(blob_at + blob.len() as u64);
        let mut out = vec![0; usize::try_from(end - base).map_err(|_| Error::Limit)?];
        for (at, bytes) in [(fixed_at, &fixed), (index_at, &index), (blob_at, &blob)] {
            let at = (at - base) as usize;
            out[at..at + bytes.len()].copy_from_slice(bytes);
        }
        let word = |v: u64| u32::try_from(v).map_err(|_| Error::Limit);
        for (at, value) in [
            (8, word(end)?),
            (0x160, word(fixed_at)?),
            (0x164, word((count + self.strings.len()) as u64)?),
            (0x168, word(blob.len() as u64)?),
            (0x16c, word(index_at)?),
            (0x170, word(blob_at)?),
        ] {
            header[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        Ok(out)
    }
    /// The rebuilt name tables, placed from file offset `base` (the end of
    /// everything written before them).
    fn rebuild_names(&mut self, header: &mut [u8], base: u64) -> Result<Vec<u8>> {
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

/// Native campaign slots can stream a complete converted level up to the absolute
/// v8 header limit at 0x12289b. The retail editing limits below still apply to retail maps.
pub const NATIVE_CAMPAIGN_FILE_LIMIT: u32 = 0x2080_0000;

/// Header validator `0x122870` / per-cache size limits at `0x214ed0`.
pub fn cache_size_limit(cache_type: crate::CacheType) -> Result<u64> {
    Ok(match cache_type {
        crate::CacheType::Campaign => 0x1180_0000,
        crate::CacheType::Multiplayer | crate::CacheType::MainMenu => 0x0500_0000,
        crate::CacheType::Shared => 0x0b40_0000,
        crate::CacheType::SinglePlayerShared => 0x2080_0000,
        crate::CacheType::Unknown(_) => {
            return Err(Error::Invalid("unknown cache type for editing"));
        }
    })
}

/// The largest total metadata size (header `+0x1c`: index, BSP space and
/// ordinary metadata) an append may produce for a cache type.
///
/// The loader (`0x1228f0`) takes `+0x1c` rounded up to a page from the low
/// end of the physical arena that `0x12b400` reserves (`0x30e4000` bytes at
/// `0x61000`), and fails only when it would cross the arena's high cursor
/// (`0x12296c`); no constant bounds it. After load, `0x12c1e0` gives every
/// page left between the cursors to one pool, so growth is taken from that
/// pool. How much headroom the high side leaves at load time is not
/// measured, so this bound is empirical: the largest total of any retail
/// map of the same type on the owner's disc (V), which that pool is known
/// to tolerate. See docs/formats/map-format.md, "Growing the metadata".
pub fn max_metadata_size(cache_type: crate::CacheType) -> Result<u32> {
    Ok(match cache_type {
        crate::CacheType::Campaign => 0x00c2_4000,
        crate::CacheType::Multiplayer => 0x0062_c000,
        crate::CacheType::MainMenu => 0x001d_3000,
        crate::CacheType::Shared => 0x0044_5000,
        crate::CacheType::SinglePlayerShared => 0x0069_e000,
        crate::CacheType::Unknown(_) => {
            return Err(Error::Invalid("unknown cache type for editing"));
        }
    })
}
