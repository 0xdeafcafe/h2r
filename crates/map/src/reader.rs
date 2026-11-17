use crate::*;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const HEADER_SIZE: u64 = 0x800;
// Retail tables are much smaller. Bound attacker-controlled eager allocations.
const MAX_TABLE_BYTES: u64 = 64 * 1024 * 1024;

/// An open retail Xbox map. Indexes are owned; payload bytes stay in the stream.
/// Parsing supports the retail `02.09.27.09809` build, not Vista's different v8.
/// An individual eagerly loaded table is limited to 64 MiB.
pub struct Map<R = File> {
    reader: R,
    base: u64,
    length: u64,
    storage_length: u64,
    header: Header,
    groups: Vec<TagGroup>,
    strings: Vec<Option<String>>,
    tags: Vec<Tag>,
    by_id: HashMap<TagId, usize>,
    index_slots: u32,
    scenario: TagId,
    globals: TagId,
    index_space: AddressSpace,
    metadata_space: Option<AddressSpace>,
}

impl Map<File> {
    /// Open a `.map` file without loading raw resource data into memory.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::from_reader(File::open(path)?)
    }
}

impl<R: Read + Seek> Map<R> {
    /// Parse a seekable stream whose offset zero is the map header.
    /// The stream's current position is ignored.
    pub fn from_reader(mut reader: R) -> Result<Self> {
        let length = reader.seek(SeekFrom::End(0))?;
        Self::from_region(reader, 0, length)
    }

    /// Parse a bounded map inside another file (e.g. an XDVDFS entry).
    /// `base` and `length` describe the container region; exposed offsets stay
    /// map-relative. Payload reads cannot escape the declared file size. Lossless
    /// writing and checksumming include trailing bytes within the supplied region.
    pub fn from_region(mut reader: R, base: u64, length: u64) -> Result<Self> {
        let actual = reader.seek(SeekFrom::End(0))?;
        bounds("container", base, length, actual)?;
        let h = read_at(&mut reader, base, length, 0, HEADER_SIZE)?;
        if &h[..4] != b"daeh" || &h[0x7fc..] != b"toof" {
            return Err(Error::Invalid("header/footer signature"));
        }
        let version = word(&h, 4);
        if version != 8 {
            return Err(Error::Unsupported(format!("version {version}")));
        }
        let build = fixed_string(&h[0x120..0x140], "build")?;
        if build != "02.09.27.09809" {
            return Err(Error::Unsupported(format!(
                "v8 build {build:?}; expected Xbox retail"
            )));
        }
        let header = Header {
            version,
            build,
            file_size: word(&h, 8),
            index_offset: word(&h, 0x10),
            index_size: word(&h, 0x14),
            tag_data_size: word(&h, 0x18),
            meta_size: word(&h, 0x1c),
            cache_type: word(&h, 0x140).into(),
            name: fixed_string(&h[0x198..0x1b8], "map name")?,
            scenario_path: fixed_string(&h[0x1bc..0x2bc], "scenario path")?,
            strings: StringTableLocation {
                count: word(&h, 0x164),
                index_offset: word(&h, 0x16c),
                data_offset: word(&h, 0x170),
                data_size: word(&h, 0x168),
            },
            tag_names: StringTableLocation {
                count: word(&h, 0x2c0),
                index_offset: word(&h, 0x2cc),
                data_offset: word(&h, 0x2c4),
                data_size: word(&h, 0x2c8),
            },
            fixed_strings_offset: word(&h, 0x160),
            low_detail_offset: word(&h, 0x158),
            low_detail_size: word(&h, 0x15c),
            checksum: word(&h, 0x2d0),
        };
        bounds(
            "declared file",
            HEADER_SIZE,
            u64::from(header.file_size).saturating_sub(HEADER_SIZE),
            length,
        )?;
        if u64::from(header.file_size) < HEADER_SIZE {
            return Err(Error::Invalid("declared file smaller than header"));
        }
        let storage_length = length;
        let length = u64::from(header.file_size);
        let index = u64::from(header.index_offset);
        if index < HEADER_SIZE || header.index_size < 32 {
            return Err(Error::Invalid("index overlaps header or is too small"));
        }
        bounds("index", index, u64::from(header.index_size), length)?;
        let meta_offset = index + u64::from(header.index_size);
        bounds(
            "metadata",
            meta_offset,
            u64::from(header.tag_data_size),
            length,
        )?;
        let ih = read_at(&mut reader, base, length, index, 32)?;
        if &ih[28..32] != b"sgat" {
            return Err(Error::Invalid("tag-index signature"));
        }
        let index_base = word(&ih, 0)
            .checked_sub(32)
            .ok_or(Error::Invalid("group table address"))?;
        let index_space = address_space(index_base, index, header.index_size)?;
        let group_count = word(&ih, 4);
        let group_bytes = table_size(group_count, 12)?;
        let group_offset = index_space.translate(word(&ih, 0), group_bytes)?;
        let group_data = read_at(&mut reader, base, length, group_offset, group_bytes.into())?;
        let groups: Vec<_> = group_data
            .as_chunks::<12>()
            .0
            .iter()
            .map(|g| TagGroup {
                class: FourCC::from_u32(word(g, 0)),
                parent: optional_group(word(g, 4)),
                grandparent: optional_group(word(g, 8)),
            })
            .collect();
        let index_slots = word(&ih, 24);
        if index_slots > 65536 {
            return Err(Error::Invalid("tag count exceeds datum slot space"));
        }
        let tag_bytes = table_size(index_slots, 16)?;
        // The entry table normally follows the groups; a map whose entries
        // outgrew its index region keeps them at the end of the ordinary
        // metadata (base: index base + header `+0x1c` - `+0x18`).
        let tag_offset = match index_space.translate(word(&ih, 8), tag_bytes) {
            Ok(o) => o,
            Err(e) => {
                let base = index_base
                    .checked_add(header.meta_size)
                    .and_then(|b| b.checked_sub(header.tag_data_size))
                    .ok_or(e)?;
                address_space(base, meta_offset, header.tag_data_size)?
                    .translate(word(&ih, 8), tag_bytes)?
            }
        };
        if tag_offset < group_offset + u64::from(group_bytes) {
            return Err(Error::Invalid("tag table overlaps groups"));
        }
        let tag_data = read_at(&mut reader, base, length, tag_offset, tag_bytes.into())?;
        // The first slot anchors the ordinary metadata region on Xbox.
        // Do not guess a base from an arbitrary later tag if it is missing.
        let metadata_space = if index_slots == 0 {
            None
        } else {
            let first = word(&tag_data, 8);
            if first == 0 || first == u32::MAX {
                return Err(Error::Invalid("missing first-tag metadata anchor"));
            }
            Some(address_space(first, meta_offset, header.tag_data_size)?)
        };
        if header.tag_names.count != 0 && header.tag_names.count != index_slots {
            return Err(Error::Invalid("tag-name count differs from index slots"));
        }
        let strings = read_strings(&mut reader, base, length, header.strings, "string-ID table")?;
        let names = read_strings(
            &mut reader,
            base,
            length,
            header.tag_names,
            "tag-name table",
        )?;
        let mut tags = Vec::new();
        let mut by_id = HashMap::new();
        for (slot, t) in tag_data.as_chunks::<16>().0.iter().enumerate() {
            let group = word(t, 0);
            let id = TagId(word(t, 4));
            if group == u32::MAX && id == TagId::NULL {
                continue;
            }
            if group == u32::MAX || id == TagId::NULL || id.index() != slot {
                return Err(Error::Invalid("invalid tag datum or slot"));
            }
            let group = FourCC::from_u32(group);
            if !groups.iter().any(|g| g.class == group) {
                return Err(Error::Invalid("tag uses undeclared class"));
            }
            let address = word(t, 8);
            let size = word(t, 12);
            let location = if address == 0 || address == u32::MAX {
                None
            } else {
                let space = metadata_space.ok_or(Error::Invalid("missing metadata region"))?;
                space.translate(address, size)?;
                Some(TagLocation {
                    address,
                    size,
                    space,
                })
            };
            by_id.insert(id, tags.len());
            tags.push(Tag {
                id,
                group,
                name: names.get(slot).cloned().flatten(),
                index_address: address,
                index_size: size,
                location,
            });
        }
        let mut map = Self {
            reader,
            base,
            length,
            storage_length,
            header,
            groups,
            strings,
            tags,
            by_id,
            index_slots,
            scenario: TagId(word(&ih, 12)),
            globals: TagId(word(&ih, 16)),
            index_space,
            metadata_space,
        };
        map.check_root(map.scenario, FourCC(*b"scnr"))?;
        map.check_root(map.globals, FourCC(*b"matg"))?;
        map.resolve_bsps()?;
        Ok(map)
    }

    /// Parsed map header.
    pub fn header(&self) -> &Header {
        &self.header
    }
    /// Live tags in index order (including tags with no payload).
    pub fn tags(&self) -> &[Tag] {
        &self.tags
    }
    /// Number of original slots, including null entries.
    pub fn index_slots(&self) -> u32 {
        self.index_slots
    }
    /// Declared class hierarchy.
    pub fn groups(&self) -> &[TagGroup] {
        &self.groups
    }
    /// String-ID entries, preserving missing offsets as `None`.
    /// This is distinct from tag paths and from localized UI strings.
    pub fn strings(&self) -> &[Option<String>] {
        &self.strings
    }
    /// Translate index-header/group/tag-table addresses (primary magic).
    pub fn index_space(&self) -> AddressSpace {
        self.index_space
    }
    /// Ordinary metadata translation (secondary magic); BSPs have their own
    /// [`TagLocation::space`]. An empty tag index has no metadata anchor.
    pub fn metadata_space(&self) -> Option<AddressSpace> {
        self.metadata_space
    }
    /// Scenario selected by the full datum in the index header.
    pub fn scenario(&self) -> Option<&Tag> {
        self.tag(self.scenario).ok()
    }
    /// Globals selected by the full datum in the index header.
    pub fn globals(&self) -> Option<&Tag> {
        self.tag(self.globals).ok()
    }
    /// Find a tag by full datum, unique path or path plus class.
    pub fn tag<'a>(&self, query: impl Into<TagQuery<'a>>) -> Result<&Tag> {
        match query.into() {
            TagQuery::Id(id) => self
                .by_id
                .get(&id)
                .map(|i| &self.tags[*i])
                .ok_or(Error::TagNotFound),
            q => {
                let (name, group) = match q {
                    TagQuery::Name(n) => (n, None),
                    TagQuery::NameInGroup(n, g) => (n, Some(g)),
                    _ => unreachable!(),
                };
                let mut matches = self.tags.iter().filter(|t| {
                    t.name.as_deref() == Some(name) && group.is_none_or(|g| t.group == g)
                });
                let found = matches.next().ok_or(Error::TagNotFound)?;
                if matches.next().is_some() {
                    return Err(Error::AmbiguousName);
                }
                Ok(found)
            }
        }
    }
    /// Translate a complete ordinary metadata range. Use a tag's location space
    /// for BSP/lightmap pointers, whose virtual ranges can overlap each other.
    pub fn file_offset(&self, address: u32, size: u32) -> Result<u64> {
        self.metadata_space
            .ok_or(Error::NoTagData)?
            .translate(address, size)
    }
    /// Read a tag's contiguous payload, resolving BSP/lightmap storage too.
    /// Embedded references and raw-resource data are not recursively followed.
    pub fn read_tag_bytes<'a>(&mut self, query: impl Into<TagQuery<'a>>) -> Result<Vec<u8>> {
        let loc = self.tag(query)?.location.ok_or(Error::NoTagData)?;
        self.read(loc.file_offset()?, u64::from(loc.size))
    }

    /// Read a map-relative file range (not a metadata or resource pointer).
    /// Bounded by the declared map size, even inside a larger disc image.
    /// Individual reads are capped at the same 64 MiB limit as index tables.
    pub fn read_file_range(&mut self, offset: u64, size: u64) -> Result<Vec<u8>> {
        if size > MAX_TABLE_BYTES {
            return Err(Error::Limit);
        }
        self.read(offset, size)
    }

    /// Entire bounded input length, including storage padding after the declared map.
    pub fn storage_length(&self) -> u64 {
        self.storage_length
    }

    // Only the lossless writer/checksummer may read beyond the declared map.
    pub(crate) fn read_storage_range(&mut self, offset: u64, size: u64) -> Result<Vec<u8>> {
        if size > MAX_TABLE_BYTES {
            return Err(Error::Limit);
        }
        read_at(
            &mut self.reader,
            self.base,
            self.storage_length,
            offset,
            size,
        )
    }

    fn read(&mut self, offset: u64, size: u64) -> Result<Vec<u8>> {
        read_at(&mut self.reader, self.base, self.length, offset, size)
    }
    fn check_root(&self, id: TagId, group: FourCC) -> Result<()> {
        if id != TagId::NULL && self.tag(id)?.group != group {
            return Err(Error::Invalid("root tag class"));
        }
        Ok(())
    }
    fn resolve_bsps(&mut self) -> Result<()> {
        let Some(scenario) = self.scenario() else {
            return Ok(());
        };
        let loc = scenario.location.ok_or(Error::NoTagData)?;
        if loc.size < 0x218 {
            return Err(Error::Invalid("scenario too short for BSP table"));
        }
        let block = self.read(loc.file_offset()? + 0x210, 8)?;
        let count = word(&block, 0);
        if count == 0 {
            return Ok(());
        }
        let size = table_size(count, 0x44)?;
        let table = self.read(loc.space.translate(word(&block, 4), size)?, size.into())?;
        for entry in table.as_chunks::<0x44>().0 {
            let offset = word(entry, 0);
            if offset == 0 {
                continue;
            } // Descriptor without local BSP storage.
            let space = address_space(word(entry, 8), offset.into(), word(entry, 4))?;
            bounds("BSP", space.file_offset, space.size.into(), self.length)?;
            if space.size < 16 {
                return Err(Error::Invalid("BSP region smaller than header"));
            }
            let bh = self.read(offset.into(), 16)?;
            if &bh[12..16] != b"psbs" {
                return Err(Error::Invalid("BSP signature"));
            }
            let used = word(&bh, 0);
            let bsp_address = word(&bh, 4);
            let light_address = word(&bh, 8);
            if used < 16
                || used > space.size
                || u64::from(bsp_address) != u64::from(space.virtual_base) + 16
            {
                return Err(Error::Invalid("BSP used size or root address"));
            }
            let light_id = TagId(word(entry, 0x1c));
            let bsp_size = if light_id == TagId::NULL {
                used - 16
            } else {
                light_address
                    .checked_sub(bsp_address)
                    .ok_or(Error::Invalid("BSP lightmap order"))?
            };
            let light_size = (used - 16)
                .checked_sub(bsp_size)
                .ok_or(Error::Invalid("BSP lightmap size"))?;
            self.set_bsp_location(
                TagId(word(entry, 0x14)),
                FourCC(*b"sbsp"),
                TagLocation {
                    address: bsp_address,
                    size: bsp_size,
                    space,
                },
            )?;
            if light_id != TagId::NULL {
                self.set_bsp_location(
                    light_id,
                    FourCC(*b"ltmp"),
                    TagLocation {
                        address: light_address,
                        size: light_size,
                        space,
                    },
                )?;
            }
        }
        Ok(())
    }
    fn set_bsp_location(&mut self, id: TagId, group: FourCC, location: TagLocation) -> Result<()> {
        location.file_offset()?;
        let i = *self.by_id.get(&id).ok_or(Error::TagNotFound)?;
        let tag = &mut self.tags[i];
        if tag.group != group || tag.location.is_some() {
            return Err(Error::Invalid("BSP reference class or duplicate location"));
        }
        tag.location = Some(location);
        Ok(())
    }
}

fn address_space(virtual_base: u32, file_offset: u64, size: u32) -> Result<AddressSpace> {
    if u64::from(virtual_base) + u64::from(size) > 1u64 << 32 {
        return Err(Error::Invalid("virtual region wraps"));
    }
    Ok(AddressSpace {
        virtual_base,
        file_offset,
        size,
    })
}
fn optional_group(n: u32) -> Option<FourCC> {
    (n != u32::MAX).then(|| FourCC::from_u32(n))
}
fn word(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(
        data[at..at + 4]
            .try_into()
            .expect("validated structure size"),
    )
}
fn bounds(what: &'static str, offset: u64, size: u64, limit: u64) -> Result<()> {
    if offset > limit || size > limit - offset {
        return Err(Error::Bounds {
            what,
            offset,
            size,
            limit,
        });
    }
    Ok(())
}
fn table_size(count: u32, stride: u32) -> Result<u32> {
    let bytes = u64::from(count) * u64::from(stride);
    if bytes > MAX_TABLE_BYTES {
        return Err(Error::Limit);
    }
    Ok(bytes as u32)
}
fn read_at(
    reader: &mut (impl Read + Seek),
    base: u64,
    length: u64,
    offset: u64,
    size: u64,
) -> Result<Vec<u8>> {
    bounds("file", offset, size, length)?;
    let size = usize::try_from(size).map_err(|_| Error::Limit)?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(size).map_err(|_| Error::Limit)?;
    bytes.resize(size, 0);
    reader.seek(SeekFrom::Start(
        base.checked_add(offset)
            .ok_or(Error::Invalid("container offset overflow"))?,
    ))?;
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}
fn fixed_string(bytes: &[u8], what: &'static str) -> Result<String> {
    let end = bytes
        .iter()
        .position(|b| *b == 0)
        .ok_or(Error::Invalid("unterminated string"))?;
    Ok(std::str::from_utf8(&bytes[..end])
        .map_err(|_| Error::Encoding(what))?
        .to_owned())
}
fn read_strings(
    reader: &mut (impl Read + Seek),
    base: u64,
    length: u64,
    table: StringTableLocation,
    what: &'static str,
) -> Result<Vec<Option<String>>> {
    if table.count == 0 {
        return Ok(Vec::new());
    }
    let index_size = table_size(table.count, 4)?;
    if u64::from(table.data_size) > MAX_TABLE_BYTES {
        return Err(Error::Limit);
    }
    let index = read_at(
        reader,
        base,
        length,
        table.index_offset.into(),
        index_size.into(),
    )?;
    let blob = read_at(
        reader,
        base,
        length,
        table.data_offset.into(),
        table.data_size.into(),
    )?;
    let mut result = Vec::new();
    result
        .try_reserve_exact(table.count as usize)
        .map_err(|_| Error::Limit)?;
    // Bound decoded allocation too: repeated offsets must not amplify a tiny
    // input into arbitrarily large strings.
    let mut total = 0u64;
    for entry in index.as_chunks::<4>().0 {
        let offset = word(entry, 0);
        if offset == u32::MAX {
            result.push(None);
            continue;
        }
        let tail = blob
            .get(offset as usize..)
            .ok_or(Error::Invalid("string offset outside blob"))?;
        let end = tail
            .iter()
            .position(|b| *b == 0)
            .ok_or(Error::Invalid("unterminated table string"))?;
        total += end as u64;
        if total > MAX_TABLE_BYTES {
            return Err(Error::Limit);
        }
        result.push(Some(
            std::str::from_utf8(&tail[..end])
                .map_err(|_| Error::Encoding(what))?
                .to_owned(),
        ));
    }
    Ok(result)
}
