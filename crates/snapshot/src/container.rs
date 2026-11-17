//! The snapshot file: a versioned header keyed by build, then tagged,
//! versioned, checksummed sections.
//!
//! ```text
//! magic        8 bytes  "H2SNAP\r\n"
//! format       u32      FORMAT_VERSION
//! header       u32 len + bytes (Header)
//! header crc   u32      CRC-32 of the header bytes
//! sections     u32 count, then each:
//!   tag        8 bytes  ASCII, NUL padded
//!   version    u32      the section's own layout version
//!   codec      u8       0 stored, 1 zstd
//!   raw len    u64      bytes after decoding
//!   stored     u64 len + bytes
//!   crc        u32      CRC-32 of the decoded bytes
//! file crc     u32      CRC-32 of every byte before it
//! ```
//!
//! All integers are little-endian. The `\r\n` in the magic catches a file
//! that went through a text-mode copy. The file CRC covers the section
//! table, so a damaged tag or version is refused at open; each section's
//! own CRC covers its decoded bytes, so a decoder fault is caught too. A file loads only into the build
//! that wrote it ([`BuildKey`]); the format version guards the container,
//! each section's version guards its own payload.
use crate::Error;
use crate::codec::{Reader, Writer};
use std::collections::BTreeSet;

pub const MAGIC: [u8; 8] = *b"H2SNAP\r\n";
pub const FORMAT_VERSION: u32 = 1;
/// No section decodes to more than this; the RAM image is 64 MB.
pub const MAX_SECTION: u64 = 1 << 30;

/// What a snapshot can only be loaded into: the image's build stamp
/// (`boot::build_stamp`: XBE hash, generator and ABI versions, flags) and
/// the feature switches, which decide what hooks and HLE state exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildKey {
    pub stamp: String,
    pub switches: String,
}

impl BuildKey {
    /// 64-bit FNV-1a over both fields, as 16 hex digits: the store's
    /// directory for this build.
    pub fn id(&self) -> String {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in self.stamp.bytes().chain([0]).chain(self.switches.bytes()) {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        format!("{h:016x}")
    }
}

/// Everything `state.list` shows without decoding a section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub build: BuildKey,
    pub name: String,
    /// Lockstep's tick sequence number when the snapshot was taken.
    pub tick: u64,
    /// Presents (frames shown) at that moment.
    pub presents: u64,
    /// Free-form, informational: map, phase, who saved it.
    pub notes: Vec<(String, String)>,
}

impl Header {
    fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.str(&self.build.stamp)
            .str(&self.build.switches)
            .str(&self.name)
            .u64(self.tick)
            .u64(self.presents)
            .list(&self.notes, |w, (k, v)| {
                w.str(k).str(v);
            });
        w.into_bytes()
    }
    fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Reader::new(bytes, "header");
        let header = Self {
            build: BuildKey {
                stamp: r.str()?.to_owned(),
                switches: r.str()?.to_owned(),
            },
            name: r.str()?.to_owned(),
            tick: r.u64()?,
            presents: r.u64()?,
            notes: r.list(8, |r| Ok((r.str()?.to_owned(), r.str()?.to_owned())))?,
        };
        r.finish()?;
        Ok(header)
    }
}

/// A section's name: up to 8 ASCII bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Tag([u8; 8]);

impl Tag {
    /// Panics (at compile time in a const) unless `name` is 1 to 8 ASCII
    /// graphic bytes.
    pub const fn new(name: &str) -> Self {
        let b = name.as_bytes();
        assert!(!b.is_empty() && b.len() <= 8, "tag must be 1 to 8 bytes");
        let mut out = [0u8; 8];
        let mut i = 0;
        while i < b.len() {
            assert!(b[i].is_ascii_graphic(), "tag must be ASCII");
            out[i] = b[i];
            i += 1;
        }
        Self(out)
    }
    pub fn as_str(&self) -> &str {
        let n = self.0.iter().position(|&b| b == 0).unwrap_or(8);
        std::str::from_utf8(&self.0[..n]).unwrap_or("?")
    }
    fn from_bytes(b: [u8; 8]) -> Result<Self, Error> {
        let n = b.iter().position(|&c| c == 0).unwrap_or(8);
        if n == 0 || !b[..n].iter().all(u8::is_ascii_graphic) || b[n..].iter().any(|&c| c != 0) {
            return Err(Error::Corrupt("section table", format!("bad tag {b:?}")));
        }
        Ok(Self(b))
    }
}

impl std::fmt::Display for Tag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How a section's bytes are stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    Stored,
    /// zstd at level 1: fast, and plenty for state that is mostly small
    /// tables. The RAM section compresses its own pages and is stored.
    Zstd,
}

/// Builds a snapshot file in memory.
pub struct SnapshotWriter {
    header: Header,
    sections: Vec<(Tag, u32, Codec, Vec<u8>)>,
}

impl SnapshotWriter {
    pub fn new(header: Header) -> Self {
        Self {
            header,
            sections: Vec::new(),
        }
    }
    /// Add a section. Each tag appears once.
    pub fn section(
        &mut self,
        tag: Tag,
        version: u32,
        codec: Codec,
        bytes: Vec<u8>,
    ) -> Result<&mut Self, Error> {
        if self.sections.iter().any(|s| s.0 == tag) {
            return Err(Error::DuplicateSection(tag));
        }
        if bytes.len() as u64 > MAX_SECTION {
            return Err(Error::Corrupt("section table", format!("{tag} too large")));
        }
        self.sections.push((tag, version, codec, bytes));
        Ok(self)
    }
    pub fn finish(self) -> Result<Vec<u8>, Error> {
        let mut w = Writer::new();
        w.raw(&MAGIC).u32(FORMAT_VERSION);
        let header = self.header.encode();
        w.blob(&header).u32(crc32fast::hash(&header));
        w.u32(self.sections.len() as u32);
        for (tag, version, codec, raw) in &self.sections {
            let (code, stored) = match codec {
                Codec::Stored => (0u8, None),
                Codec::Zstd => (1, Some(zstd::bulk::compress(raw, 1).map_err(Error::Io)?)),
            };
            let stored = stored.as_deref().unwrap_or(raw);
            w.raw(&tag.0)
                .u32(*version)
                .u8(code)
                .u64(raw.len() as u64)
                .u64(stored.len() as u64)
                .raw(stored)
                .u32(crc32fast::hash(raw));
        }
        let crc = crc32fast::hash(&w.bytes);
        w.u32(crc);
        Ok(w.into_bytes())
    }
}

struct Entry {
    tag: Tag,
    version: u32,
    codec: Codec,
    raw_len: u64,
    stored: std::ops::Range<usize>,
    crc: u32,
}

/// A parsed snapshot file. Sections are decoded and checked on demand.
pub struct Snapshot<'a> {
    pub header: Header,
    bytes: &'a [u8],
    entries: Vec<Entry>,
}

/// The magic and format version, then the header: enough to list a
/// snapshot or refuse it, whatever build is asking.
pub fn read_header(bytes: &[u8]) -> Result<Header, Error> {
    let mut r = Reader::new(bytes, "file header");
    if r.array::<8>()? != MAGIC {
        return Err(Error::NotASnapshot);
    }
    let version = r.u32()?;
    if version != FORMAT_VERSION {
        return Err(Error::FormatVersion(version));
    }
    let header = r.blob()?;
    if r.u32()? != crc32fast::hash(header) {
        return Err(Error::Checksum("header".into()));
    }
    Header::decode(header)
}

impl<'a> Snapshot<'a> {
    /// Parse a whole file and check that it belongs to `build`. Section
    /// payloads are only located here, not decoded.
    pub fn open(bytes: &'a [u8], build: &BuildKey) -> Result<Self, Error> {
        let header = read_header(bytes)?;
        let Some(body_len) = bytes.len().checked_sub(4) else {
            return Err(Error::Truncated("file"));
        };
        let (body, crc) = bytes.split_at(body_len);
        if crc32fast::hash(body) != u32::from_le_bytes(crc.try_into().expect("4 bytes")) {
            return Err(Error::Checksum("file".into()));
        }
        if header.build != *build {
            return Err(Error::WrongBuild {
                saved: Box::new(header.build),
                running: Box::new(build.clone()),
            });
        }
        let mut r = Reader::new(body, "section table");
        // Skip what read_header checked.
        r.raw(12)?;
        let n = r.u32()? as usize;
        r.raw(n + 4)?;
        let count = r.u32()?;
        let mut entries = Vec::new();
        let mut seen = BTreeSet::new();
        for _ in 0..count {
            let tag = Tag::from_bytes(r.array()?)?;
            if !seen.insert(tag) {
                return Err(Error::DuplicateSection(tag));
            }
            let version = r.u32()?;
            let codec = match r.u8()? {
                0 => Codec::Stored,
                1 => Codec::Zstd,
                c => return Err(Error::Corrupt("section table", format!("{tag}: codec {c}"))),
            };
            let raw_len = r.u64()?;
            let stored_len = r.u64()?;
            if raw_len > MAX_SECTION
                || stored_len > r.remaining() as u64
                || (codec == Codec::Stored && stored_len != raw_len)
            {
                return Err(Error::Corrupt(
                    "section table",
                    format!("{tag}: lengths {raw_len}/{stored_len}"),
                ));
            }
            let start = body.len() - r.remaining();
            r.raw(stored_len as usize)?;
            let crc = r.u32()?;
            entries.push(Entry {
                tag,
                version,
                codec,
                raw_len,
                stored: start..start + stored_len as usize,
                crc,
            });
        }
        r.finish()?;
        Ok(Self {
            header,
            bytes,
            entries,
        })
    }

    /// The tags present, in file order.
    pub fn tags(&self) -> Vec<Tag> {
        self.entries.iter().map(|e| e.tag).collect()
    }

    /// A section's version and decoded bytes, checksum verified; `None` if
    /// the file has no such section.
    pub fn section(&self, tag: Tag) -> Result<Option<(u32, Vec<u8>)>, Error> {
        let Some(e) = self.entries.iter().find(|e| e.tag == tag) else {
            return Ok(None);
        };
        let stored = &self.bytes[e.stored.clone()];
        let raw = match e.codec {
            Codec::Stored => stored.to_vec(),
            Codec::Zstd => zstd::bulk::decompress(stored, e.raw_len as usize)
                .map_err(|_| Error::Checksum(tag.to_string()))?,
        };
        if raw.len() as u64 != e.raw_len || crc32fast::hash(&raw) != e.crc {
            return Err(Error::Checksum(tag.to_string()));
        }
        Ok(Some((e.version, raw)))
    }

    /// [`Snapshot::section`] for a section the loader cannot do without,
    /// in a version it understands.
    pub fn require(
        &self,
        tag: Tag,
        versions: std::ops::RangeInclusive<u32>,
    ) -> Result<Vec<u8>, Error> {
        let (version, bytes) = self.section(tag)?.ok_or(Error::MissingSection(tag))?;
        if !versions.contains(&version) {
            return Err(Error::SectionVersion(tag, version));
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> Header {
        Header {
            build: BuildKey {
                stamp: "xbe 0123 generator abcd".into(),
                switches: "agent,-movies".into(),
            },
            name: "test.fight".into(),
            tick: 1234,
            presents: 2468,
            notes: vec![("map".into(), "synthetic".into())],
        }
    }

    const A: Tag = Tag::new("alpha");
    const B: Tag = Tag::new("beta");

    fn file() -> Vec<u8> {
        let mut w = SnapshotWriter::new(header());
        w.section(A, 3, Codec::Stored, b"plain bytes".to_vec())
            .unwrap()
            .section(B, 1, Codec::Zstd, vec![7; 10_000])
            .unwrap();
        w.finish().unwrap()
    }

    #[test]
    fn sections_round_trip() {
        let bytes = file();
        assert!(bytes.len() < 10_000, "zstd shrank the repetitive section");
        let s = Snapshot::open(&bytes, &header().build).unwrap();
        assert_eq!(s.header, header());
        assert_eq!(s.tags(), vec![A, B]);
        assert_eq!(s.section(A).unwrap(), Some((3, b"plain bytes".to_vec())));
        assert_eq!(s.require(B, 1..=1).unwrap(), vec![7; 10_000]);
        assert_eq!(s.section(Tag::new("gamma")).unwrap(), None);
        assert_eq!(
            s.require(Tag::new("gamma"), 1..=1),
            Err(Error::MissingSection(Tag::new("gamma")))
        );
        assert_eq!(s.require(A, 1..=2), Err(Error::SectionVersion(A, 3)));
        assert_eq!(read_header(&bytes).unwrap(), header());
    }

    #[test]
    fn another_build_is_refused_but_can_still_list_it() {
        let bytes = file();
        let mut other = header().build;
        other.switches = "agent".into();
        assert!(matches!(
            Snapshot::open(&bytes, &other),
            Err(Error::WrongBuild { .. })
        ));
        assert_eq!(read_header(&bytes).unwrap().name, "test.fight");
        assert_ne!(other.id(), header().build.id());
        assert_eq!(header().build.id().len(), 16);
    }

    #[test]
    fn damage_is_detected() {
        let good = file();
        let build = header().build;
        assert!(matches!(
            Snapshot::open(b"not a snapshot at all", &build),
            Err(Error::NotASnapshot)
        ));
        let mut v2 = good.clone();
        v2[8] = 2;
        assert_eq!(read_header(&v2), Err(Error::FormatVersion(2)));
        // Every truncation fails cleanly.
        for n in 0..good.len() {
            assert!(Snapshot::open(&good[..n], &build).is_err(), "prefix {n}");
        }
        // A flipped bit anywhere is refused at open.
        for i in 0..good.len() {
            let mut bad = good.clone();
            bad[i] ^= 0x10;
            assert!(
                Snapshot::open(&bad, &build).is_err(),
                "flip at {i} went unnoticed"
            );
        }
        let mut extra = good.clone();
        extra.push(0);
        assert!(Snapshot::open(&extra, &build).is_err());
    }

    #[test]
    fn a_tag_is_written_once() {
        let mut w = SnapshotWriter::new(header());
        w.section(A, 1, Codec::Stored, vec![]).unwrap();
        assert!(matches!(
            w.section(A, 1, Codec::Stored, vec![]),
            Err(Error::DuplicateSection(_))
        ));
    }
}
