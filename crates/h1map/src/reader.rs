//! Inflating a map and reading its index.

use std::collections::BTreeMap;
use std::io::Read;

use flate2::read::ZlibDecoder;

use crate::format::*;
use crate::{Error, Result};

/// Longest tag name read before giving up on finding its NUL.
const MAX_NAME: usize = 0x200;

/// A decompressed Halo 1 Xbox map, its header and its tag index.
#[derive(Debug, Clone)]
pub struct H1Map {
    data: Vec<u8>,
    header: Header,
    index: IndexHeader,
    base: u32,
    tags: Vec<Tag>,
}

/// Parses and checks the uncompressed `0x800`-byte header.
pub fn parse_header(h: &[u8]) -> Result<Header> {
    if h.len() < HEADER_LEN {
        return Err(Error::Invalid(format!(
            "header is {:#x} bytes, want {HEADER_LEN:#x}",
            h.len()
        )));
    }
    let u = |at| u32_at(h, at).unwrap();
    if u(0) != HEAD || u(FOOT_OFFSET) != FOOT {
        return Err(Error::Invalid(
            "no head/foot FourCCs: not a cache file".into(),
        ));
    }
    let version = u(4);
    if version != VERSION {
        return Err(Error::Unsupported(format!(
            "cache version {version}; this reader reads Halo 1 Xbox maps (version {VERSION})"
        )));
    }
    let header = Header {
        version,
        file_length: u(0x08),
        field_0c: u(0x0c),
        index_offset: u(0x10),
        metadata_size: u(0x14),
        name: fixed_str(&h[0x20..0x40]),
        build: fixed_str(&h[0x40..0x60]),
        cache_type: CacheType::from_raw(u16_at(h, 0x60).unwrap()),
        checksum: u(0x64),
    };
    if header.file_length < HEADER_LEN as u32 || header.file_length > MAX_FILE_LEN {
        return Err(Error::Invalid(format!(
            "decompressed length {:#x} is out of range",
            header.file_length
        )));
    }
    let end = header.index_offset as u64 + header.metadata_size as u64;
    if (header.index_offset as usize) < HEADER_LEN || end > header.file_length as u64 {
        return Err(Error::Bounds {
            what: "tag data",
            offset: header.index_offset as u64,
            size: header.metadata_size as u64,
            limit: header.file_length as u64,
        });
    }
    Ok(header)
}

impl H1Map {
    /// Reads a compressed map as stored on the disc: the header, then the
    /// zlib stream, which must inflate to exactly the header's decompressed
    /// length less the header. Bytes after the stream are ignored.
    pub fn from_reader(mut r: impl Read) -> Result<Self> {
        let mut head = vec![0u8; HEADER_LEN];
        r.read_exact(&mut head)?;
        let header = parse_header(&head)?;
        let want = header.file_length as usize - HEADER_LEN;
        let mut data = head;
        data.reserve_exact(want);
        // One byte over the limit tells "too long" from "exactly right".
        ZlibDecoder::new(r)
            .take(want as u64 + 1)
            .read_to_end(&mut data)?;
        let got = data.len() - HEADER_LEN;
        if got != want {
            return Err(Error::Invalid(format!(
                "zlib stream inflates to {got:#x} bytes; the header says {want:#x} ({:#x} with the header)",
                header.file_length
            )));
        }
        Self::parse(data, header)
    }

    /// Reads an already decompressed map: the header followed by the
    /// inflated body, `file_length` bytes in all.
    pub fn from_decompressed(data: Vec<u8>) -> Result<Self> {
        let header = parse_header(&data)?;
        if data.len() != header.file_length as usize {
            return Err(Error::Invalid(format!(
                "{:#x} bytes; the header says {:#x}",
                data.len(),
                header.file_length
            )));
        }
        Self::parse(data, header)
    }

    fn parse(data: Vec<u8>, header: Header) -> Result<Self> {
        let index = parse_index(&data, header.index_offset as usize)?;
        // The tag array follows the index header, so the index header's
        // address (the metadata base) is the array's address less its length.
        let base = index.tag_array_address.wrapping_sub(index.len);
        let mut map = Self {
            data,
            header,
            index,
            base,
            tags: Vec::new(),
        };
        if map.index.tag_count > MAX_TAGS {
            return Err(Error::Invalid(format!("{} tags", map.index.tag_count)));
        }
        let count = map.index.tag_count as usize;
        let array = map
            .read(map.index.tag_array_address, count * TAG_ENTRY_LEN)?
            .to_vec();
        let mut tags = Vec::with_capacity(count);
        for (i, e) in array.as_chunks::<TAG_ENTRY_LEN>().0.iter().enumerate() {
            let u = |at| u32_at(e.as_slice(), at).unwrap();
            let id = TagId(u(0x0c));
            if id.index() as usize != i {
                return Err(Error::Invalid(format!(
                    "tag {i} has ID {id}, whose index is {}",
                    id.index()
                )));
            }
            let name_address = u(0x10);
            tags.push(Tag {
                group: Group(u(0x00)),
                parent: Group(u(0x04)),
                grandparent: Group(u(0x08)),
                id,
                name_address,
                name: map.read_cstr(name_address)?,
                data_address: u(0x14),
                external: u(0x18) != 0,
                data: None,
            });
        }
        map.set_ranges(&mut tags);
        map.tags = tags;
        map.tag(map.index.scenario)
            .map_err(|_| Error::Invalid(format!("scenario {} is not a tag", map.index.scenario)))?;
        Ok(map)
    }

    /// Each local tag's data runs to the next tag's data, or the end of the
    /// tag data region.
    fn set_ranges(&self, tags: &mut [Tag]) {
        let mut starts: Vec<usize> = tags
            .iter()
            .filter(|t| !t.external)
            .filter_map(|t| self.offset(t.data_address, 0).ok())
            .collect();
        starts.sort_unstable();
        starts.dedup();
        let end = self.metadata_end();
        for t in tags.iter_mut().filter(|t| !t.external) {
            let Ok(start) = self.offset(t.data_address, 0) else {
                continue;
            };
            if start >= end {
                continue;
            }
            let next = starts.partition_point(|&s| s <= start);
            t.data = Some(start..starts.get(next).copied().unwrap_or(end));
        }
    }

    /// The file header.
    pub fn header(&self) -> &Header {
        &self.header
    }

    /// The tag index header.
    pub fn index(&self) -> &IndexHeader {
        &self.index
    }

    /// The address of the tag index header, derived from this file. Retail
    /// Xbox maps use [`XBOX_METADATA_BASE`].
    pub fn metadata_base(&self) -> u32 {
        self.base
    }

    /// The whole decompressed file, header included.
    pub fn bytes(&self) -> &[u8] {
        &self.data
    }

    /// Every tag, in tag array order.
    pub fn tags(&self) -> &[Tag] {
        &self.tags
    }

    /// The tag with this ID; the salt must match too.
    pub fn tag(&self, id: TagId) -> Result<&Tag> {
        self.tags
            .get(id.index() as usize)
            .filter(|t| t.id == id)
            .ok_or(Error::TagNotFound)
    }

    /// The tag with this group and name (case-insensitive, as the engine).
    pub fn find(&self, group: Group, name: &str) -> Option<&Tag> {
        self.tags
            .iter()
            .find(|t| t.group == group && t.name.eq_ignore_ascii_case(name))
    }

    /// The scenario tag the index header names.
    pub fn scenario(&self) -> &Tag {
        self.tag(self.index.scenario).expect("checked when opened")
    }

    /// Tag counts by group, sorted by group name.
    pub fn group_counts(&self) -> Vec<(Group, usize)> {
        let mut counts: BTreeMap<String, (Group, usize)> = BTreeMap::new();
        for t in &self.tags {
            counts.entry(t.group.to_string()).or_insert((t.group, 0)).1 += 1;
        }
        counts.into_values().collect()
    }

    /// A tag's bytes: [`Tag::data`].
    pub fn tag_data(&self, tag: &Tag) -> Result<&[u8]> {
        let r = tag.data.clone().ok_or(Error::NoTagData)?;
        Ok(&self.data[r])
    }

    /// `len` bytes at a tag data address.
    pub fn read(&self, address: u32, len: usize) -> Result<&[u8]> {
        let at = self.offset(address, len)?;
        Ok(&self.data[at..at + len])
    }

    /// The bytes of a tag block's elements, `elem_len` bytes each. An empty
    /// block is an empty slice whatever its address.
    pub fn block(&self, block: Block, elem_len: usize) -> Result<&[u8]> {
        if block.count == 0 {
            return Ok(&[]);
        }
        let len = (block.count as usize)
            .checked_mul(elem_len)
            .filter(|&n| n <= self.data.len())
            .ok_or(Error::Bounds {
                what: "tag block",
                offset: block.address as u64,
                size: u64::MAX,
                limit: self.data.len() as u64,
            })?;
        self.read(block.address, len)
    }

    /// A NUL-terminated string at a tag data address.
    pub fn read_cstr(&self, address: u32) -> Result<String> {
        let at = self.offset(address, 1)?;
        let window = &self.data[at..self.metadata_end().min(at + MAX_NAME)];
        let len = window
            .iter()
            .position(|&c| c == 0)
            .ok_or(Error::Invalid(format!(
                "no NUL within {MAX_NAME:#x} bytes of the string at {address:#x}"
            )))?;
        Ok(String::from_utf8_lossy(&window[..len]).into_owned())
    }

    /// The scenario's structure BSPs: where each one's region of the file
    /// is, and its `sbsp` tag.
    pub fn structure_bsps(&self) -> Result<Vec<BspRef>> {
        let scnr = self.scenario();
        if scnr.group != Group::SCENARIO {
            return Err(Error::Invalid(format!(
                "the scenario tag is a {}",
                scnr.group
            )));
        }
        let bytes = self.tag_data(scnr)?;
        let block = Block::read(bytes, SCNR_STRUCTURE_BSPS).ok_or(Error::Bounds {
            what: "scenario structure BSP block",
            offset: SCNR_STRUCTURE_BSPS as u64,
            size: BLOCK_LEN as u64,
            limit: bytes.len() as u64,
        })?;
        let elems = self.block(block, SCNR_BSP_LEN)?;
        let mut out = Vec::with_capacity(block.count as usize);
        for e in elems.as_chunks::<SCNR_BSP_LEN>().0 {
            let u = |at| u32_at(e.as_slice(), at).unwrap();
            let tag = TagRef::read(e.as_slice(), 0x10).unwrap();
            let target = self.tag(tag.id)?;
            if target.group != Group::STRUCTURE_BSP {
                return Err(Error::Invalid(format!(
                    "structure BSP reference names a {} tag",
                    target.group
                )));
            }
            let bsp = BspRef {
                file_offset: u(0x00),
                size: u(0x04),
                address: u(0x08),
                tag,
                name: target.name.clone(),
            };
            self.bsp_bytes(&bsp)?;
            out.push(bsp);
        }
        Ok(out)
    }

    /// A structure BSP's region of the decompressed file.
    pub fn bsp_bytes(&self, bsp: &BspRef) -> Result<&[u8]> {
        let (at, len) = (bsp.file_offset as usize, bsp.size as usize);
        self.data
            .get(at..at.saturating_add(len))
            .ok_or(Error::Bounds {
                what: "structure BSP",
                offset: at as u64,
                size: len as u64,
                limit: self.data.len() as u64,
            })
    }

    fn metadata_end(&self) -> usize {
        (self.header.index_offset + self.header.metadata_size) as usize
    }

    /// File offset of `len` bytes at a tag data address.
    fn offset(&self, address: u32, len: usize) -> Result<usize> {
        let size = self.header.metadata_size as u64;
        let rel = address.wrapping_sub(self.base) as u64;
        if address < self.base || rel + len as u64 > size {
            return Err(Error::Bounds {
                what: "tag data address",
                offset: address as u64,
                size: len as u64,
                limit: self.base as u64 + size,
            });
        }
        Ok(self.header.index_offset as usize + rel as usize)
    }
}

/// The tag index header at `at`: four fields, the model data fields, then
/// `tags` (at `0x20` on Xbox; `0x24` on PC).
fn parse_index(data: &[u8], at: usize) -> Result<IndexHeader> {
    let u = |off: usize| {
        u32_at(data, at + off).ok_or(Error::Bounds {
            what: "tag index header",
            offset: (at + off) as u64,
            size: 4,
            limit: data.len() as u64,
        })
    };
    let tags_at = [0x20, 0x24]
        .into_iter()
        .find(|&o| u(o).is_ok_and(|v| v == TAGS))
        .ok_or_else(|| Error::Invalid("no 'tags' FourCC closing the tag index header".into()))?;
    Ok(IndexHeader {
        tag_array_address: u(0x00)?,
        scenario: TagId(u(0x04)?),
        checksum: u(0x08)?,
        tag_count: u(0x0c)?,
        model_fields: (0x10..tags_at).step_by(4).map(&u).collect::<Result<_>>()?,
        len: tags_at as u32 + 4,
    })
}
