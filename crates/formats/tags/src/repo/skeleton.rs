//! A map's skeleton and extent table: the map file kept in the store so
//! that it can be given back byte for byte (docs/engine/asset-store.md,
//! "The store").
//!
//! ```text
//! <repo>/maps/<map>.skel      the skeleton: the bytes of the map that are not
//!                             raw files, at their offsets (deflate)
//! <repo>/maps/<map>.extents   the extent table: every other range of the file
//!                             as (offset, length, raw hash)
//! ```
//!
//! The extractor splits each map file ([`partition`]): the header, the BSP
//! regions, the string tables and the tag index with the metadata are the
//! skeleton; each local raw resource a tag names is an extent of its own
//! file in `raw/`; a range nothing names (padding, a resource no layout
//! reaches, bytes past the declared size) is stored in `raw/` by hash as an
//! extent of its own. Together the skeleton and the extents tile the file,
//! and [`StoreFile`] serves it: `read_at` answers a range from the skeleton
//! or from `raw/<hh>/<sha256>`. The extractor reads every map back through
//! it and checks the file's SHA-256 before it records the map
//! ([`super::TagRepo::check_reassembly`]).
use super::store::write_atomic;
use super::{Error, Result, TagRepo, hex};
use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Extension of a map's skeleton file.
pub const SKEL_EXT: &str = "skel";
/// Extension of a map's extent table.
pub const EXTENTS_EXT: &str = "extents";
/// First bytes of a skeleton file (then a deflate stream).
const SKEL_MAGIC: &[u8; 8] = b"h2skel\0\x01";
/// First line of an extent table.
const EXTENTS_MAGIC: &str = "h2 extents 1";
/// Largest unnamed range stored as one raw file; a longer one is split.
pub const UNNAMED_PIECE: u64 = 16 << 20;
/// Largest read of the map file at once.
const READ_PIECE: u64 = 16 << 20;

/// One range of a map file and where its bytes are kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    /// In the skeleton.
    Skeleton { offset: u64, len: u64 },
    /// A raw resource a tag names, in `raw/` by this hash.
    Named { offset: u64, len: u64, hash: String },
    /// Bytes nothing names: stored in `raw/` as a file of their own.
    Unnamed { offset: u64, len: u64 },
}

impl Part {
    /// (offset, length).
    pub fn range(&self) -> (u64, u64) {
        match *self {
            Part::Skeleton { offset, len }
            | Part::Named { offset, len, .. }
            | Part::Unnamed { offset, len } => (offset, len),
        }
    }
}

/// Split a file of `len` bytes: `structural` ranges (offset, length) go to
/// the skeleton, merged; each `named` raw resource (offset, length, hash)
/// that overlaps neither the skeleton nor a named resource taken before it
/// (by offset, the longest first at one offset) is an extent of its own;
/// every byte left is [`Part::Unnamed`], in pieces of at most
/// [`UNNAMED_PIECE`]. Ranges past `len` are clipped (structural) or left
/// out (named). The parts tile `[0, len)` in order.
pub fn partition(len: u64, structural: &[(u64, u64)], named: &[(u64, u64, String)]) -> Vec<Part> {
    let mut s: Vec<(u64, u64)> = structural
        .iter()
        .filter_map(|&(o, l)| {
            let e = o.saturating_add(l).min(len);
            (o < e).then_some((o, e))
        })
        .collect();
    s.sort_unstable();
    let mut skel: Vec<(u64, u64)> = Vec::with_capacity(s.len());
    for (o, e) in s {
        match skel.last_mut() {
            Some(last) if o <= last.1 => last.1 = last.1.max(e),
            _ => skel.push((o, e)),
        }
    }
    let mut n: Vec<(u64, u64, &str)> = named
        .iter()
        .filter(|(o, l, _)| *l > 0 && o.checked_add(*l).is_some_and(|e| e <= len))
        .map(|(o, l, h)| (*o, o + l, h.as_str()))
        .collect();
    n.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)).then(a.2.cmp(b.2)));
    let in_skel = |o: u64, e: u64| {
        let i = skel.partition_point(|r| r.1 <= o);
        i < skel.len() && skel[i].0 < e
    };
    let mut chosen: Vec<(u64, u64, &str)> = Vec::new();
    let mut cursor = 0;
    for (o, e, h) in n {
        if o < cursor || in_skel(o, e) {
            continue;
        }
        chosen.push((o, e, h));
        cursor = e;
    }
    let mut parts = Vec::with_capacity(skel.len() + chosen.len() * 2 + 1);
    let mut at = 0u64;
    let fill = |parts: &mut Vec<Part>, from: u64, to: u64| {
        let mut o = from;
        while o < to {
            let l = (to - o).min(UNNAMED_PIECE);
            parts.push(Part::Unnamed { offset: o, len: l });
            o += l;
        }
    };
    let (mut i, mut j) = (0, 0);
    while i < skel.len() || j < chosen.len() {
        let take_skel = match (skel.get(i), chosen.get(j)) {
            (Some(s), Some(c)) => s.0 < c.0,
            (Some(_), None) => true,
            _ => false,
        };
        let (o, e, part) = if take_skel {
            let (o, e) = skel[i];
            i += 1;
            (
                o,
                e,
                Part::Skeleton {
                    offset: o,
                    len: e - o,
                },
            )
        } else {
            let (o, e, h) = chosen[j];
            j += 1;
            (
                o,
                e,
                Part::Named {
                    offset: o,
                    len: e - o,
                    hash: h.to_owned(),
                },
            )
        };
        fill(&mut parts, at, o);
        parts.push(part);
        at = e;
    }
    fill(&mut parts, at, len);
    parts
}

/// The bytes of a map that are not raw files, at their offsets.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Skeleton {
    /// The map file's length.
    pub len: u64,
    /// (offset, length) of each range, sorted; their bytes are [`Self::bytes`]
    /// in order.
    pub ranges: Vec<(u64, u64)>,
    /// Every range's bytes, concatenated.
    pub bytes: Vec<u8>,
}

fn le_u64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

impl Skeleton {
    /// The file form: [`SKEL_MAGIC`], then a deflate stream of the length,
    /// the range count, each range's offset and length, and the bytes.
    pub fn encode(&self) -> Result<Vec<u8>> {
        use flate2::{Compression, write::DeflateEncoder};
        let mut out = SKEL_MAGIC.to_vec();
        let mut z = DeflateEncoder::new(&mut out, Compression::new(6));
        z.write_all(&self.len.to_le_bytes())?;
        z.write_all(&(self.ranges.len() as u64).to_le_bytes())?;
        for (o, l) in &self.ranges {
            z.write_all(&o.to_le_bytes())?;
            z.write_all(&l.to_le_bytes())?;
        }
        z.write_all(&self.bytes)?;
        z.finish()?;
        Ok(out)
    }

    /// Parse [`Skeleton::encode`].
    pub fn decode(file: &[u8]) -> Result<Self> {
        use flate2::read::DeflateDecoder;
        let bad = |why: &str| Error::Format(format!("skeleton: {why}"));
        let body = file
            .strip_prefix(SKEL_MAGIC.as_slice())
            .ok_or_else(|| bad("not a skeleton file"))?;
        let mut all = Vec::new();
        DeflateDecoder::new(body)
            .read_to_end(&mut all)
            .map_err(|e| bad(&e.to_string()))?;
        let len = le_u64(&all, 0).ok_or_else(|| bad("short"))?;
        let count = le_u64(&all, 8).ok_or_else(|| bad("short"))?;
        let table = usize::try_from(count)
            .ok()
            .and_then(|c| c.checked_mul(16))
            .and_then(|t| t.checked_add(16))
            .filter(|t| *t <= all.len())
            .ok_or_else(|| bad("range table past the end"))?;
        let mut ranges = Vec::with_capacity(count as usize);
        let mut total = 0u64;
        for i in 0..count as usize {
            let o = le_u64(&all, 16 + i * 16).expect("in table");
            let l = le_u64(&all, 24 + i * 16).expect("in table");
            total = total.checked_add(l).ok_or_else(|| bad("length overflow"))?;
            ranges.push((o, l));
        }
        let bytes = all.split_off(table);
        if bytes.len() as u64 != total {
            return Err(bad(&format!(
                "{} bytes of ranges, the table says {total}",
                bytes.len()
            )));
        }
        Ok(Skeleton { len, ranges, bytes })
    }
}

/// One extent: a range of the map file held by a raw file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Extent {
    /// File offset.
    pub offset: u64,
    /// Length, which is the raw file's.
    pub len: u64,
    /// The raw file's SHA-256 (hex).
    pub hash: String,
}

/// A map's extent table.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Extents {
    /// SHA-256 of the whole map file (hex): the disc's identity.
    pub file: String,
    /// The map file's length.
    pub len: u64,
    /// Every extent, by offset.
    pub extents: Vec<Extent>,
}

impl Extents {
    /// Text form: a first line, `file` and `length`, then one
    /// `offset<TAB>length<TAB>hash` line per extent (decimal).
    pub fn to_text(&self) -> String {
        let mut s = format!(
            "{EXTENTS_MAGIC}\nfile\t{}\nlength\t{}\n",
            self.file, self.len
        );
        for e in &self.extents {
            s.push_str(&format!("{}\t{}\t{}\n", e.offset, e.len, e.hash));
        }
        s
    }

    /// Parse [`Extents::to_text`].
    pub fn parse(text: &str) -> Result<Self> {
        let mut lines = text.lines();
        if lines.next() != Some(EXTENTS_MAGIC) {
            return Err(Error::Format("extent table: not one".into()));
        }
        let mut x = Extents::default();
        for (n, line) in lines.enumerate() {
            let bad = || Error::Format(format!("extent table line {}: {line:?}", n + 2));
            let f: Vec<&str> = line.split('\t').collect();
            match f.as_slice() {
                ["file", h] => x.file = (*h).to_owned(),
                ["length", l] => x.len = l.parse().map_err(|_| bad())?,
                [o, l, h] => {
                    let hash = (*h).to_owned();
                    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
                        return Err(bad());
                    }
                    x.extents.push(Extent {
                        offset: o.parse().map_err(|_| bad())?,
                        len: l.parse().map_err(|_| bad())?,
                        hash,
                    });
                }
                _ => return Err(bad()),
            }
        }
        Ok(x)
    }

    /// Bytes the extents cover.
    pub fn raw_bytes(&self) -> u64 {
        self.extents.iter().map(|e| e.len).sum()
    }
}

/// What writing a map's skeleton and extents did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SkeletonReport {
    /// The map file's length.
    pub len: u64,
    /// Bytes in the skeleton, and the skeleton file's size (compressed).
    pub skeleton: (u64, u64),
    /// Extents naming a tag's raw resource: count and bytes.
    pub named: (usize, u64),
    /// Extents of ranges nothing names: count and bytes.
    pub unnamed: (usize, u64),
}

fn skel_path(root: &Path, map: &str) -> PathBuf {
    root.join("maps").join(format!("{map}.{SKEL_EXT}"))
}

fn extents_path(root: &Path, map: &str) -> PathBuf {
    root.join("maps").join(format!("{map}.{EXTENTS_EXT}"))
}

impl TagRepo {
    /// Whether a map has both its skeleton and its extent table.
    pub fn has_skeleton(&self, map: &str) -> bool {
        StoreFile::exists(self.root(), map)
    }

    /// Write a map's skeleton and extent table from its `parts`
    /// ([`partition`]): `read` gives the file's bytes (offset, length);
    /// unnamed ranges are stored in `raw/`. `file` is the map file's
    /// SHA-256. The skeleton is written first, the extent table last.
    pub fn put_skeleton(
        &mut self,
        map: &str,
        file: &str,
        len: u64,
        parts: &[Part],
        read: &mut dyn FnMut(u64, u64) -> Result<Vec<u8>>,
    ) -> Result<SkeletonReport> {
        let mut skel = Skeleton {
            len,
            ..Default::default()
        };
        let mut ext = Extents {
            file: file.to_owned(),
            len,
            extents: Vec::new(),
        };
        let mut report = SkeletonReport {
            len,
            ..Default::default()
        };
        let mut at = 0u64;
        for p in parts {
            let (offset, plen) = p.range();
            if offset != at {
                return Err(Error::Invalid(format!(
                    "{map}: the parts leave {at:#x}..{offset:#x} out or overlap"
                )));
            }
            at = offset + plen;
            match p {
                Part::Skeleton { .. } => {
                    let mut o = offset;
                    while o < at {
                        let l = (at - o).min(READ_PIECE);
                        let b = read(o, l)?;
                        if b.len() as u64 != l {
                            return Err(Error::Invalid(format!("{map}: short read at {o:#x}")));
                        }
                        skel.bytes.extend_from_slice(&b);
                        o += l;
                    }
                    skel.ranges.push((offset, plen));
                    report.skeleton.0 += plen;
                }
                Part::Named { hash, .. } => {
                    ext.extents.push(Extent {
                        offset,
                        len: plen,
                        hash: hash.clone(),
                    });
                    report.named.0 += 1;
                    report.named.1 += plen;
                }
                Part::Unnamed { .. } => {
                    let b = read(offset, plen)?;
                    if b.len() as u64 != plen {
                        return Err(Error::Invalid(format!("{map}: short read at {offset:#x}")));
                    }
                    let hash = self.put_raw(&b)?;
                    ext.extents.push(Extent {
                        offset,
                        len: plen,
                        hash,
                    });
                    report.unnamed.0 += 1;
                    report.unnamed.1 += plen;
                }
            }
        }
        if at != len {
            return Err(Error::Invalid(format!(
                "{map}: the parts end at {at:#x}, the file at {len:#x}"
            )));
        }
        let encoded = skel.encode()?;
        report.skeleton.1 = encoded.len() as u64;
        write_atomic(&skel_path(self.root(), map), &encoded)?;
        write_atomic(&extents_path(self.root(), map), ext.to_text().as_bytes())?;
        Ok(report)
    }

    /// The map file served from the store.
    pub fn store_file(&self, map: &str) -> Result<StoreFile> {
        StoreFile::open(self.root(), map)
    }

    /// Write the map file to `out` from its skeleton and extents; returns
    /// its SHA-256 (hex).
    pub fn reassemble(&self, map: &str, out: &mut dyn Write) -> Result<String> {
        use sha2::Digest;
        let mut f = self.store_file(map)?;
        let mut h = sha2::Sha256::new();
        let mut buf = vec![0u8; 1 << 20];
        let mut at = 0u64;
        loop {
            let n = f.read_at(at, &mut buf)?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
            out.write_all(&buf[..n])?;
            at += n as u64;
        }
        Ok(hex(&h.finalize()))
    }

    /// Reassemble `map` and check it against `expected`, the SHA-256 of the
    /// file it was extracted from (and the one its extent table records).
    pub fn check_reassembly(&self, map: &str, expected: &str) -> Result<()> {
        let got = self.reassemble(map, &mut io::sink())?;
        let recorded = self.store_file(map)?.file_hash().to_owned();
        if got != expected || recorded != expected {
            return Err(Error::Invalid(format!(
                "store map {map}: reassembles to {got} (table says {recorded}), the map file is {expected}"
            )));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
enum Src {
    /// At this position of the skeleton's bytes.
    Skeleton(usize),
    /// The raw file of this hash.
    Raw(String),
}

#[derive(Clone, Debug)]
struct Piece {
    offset: u64,
    len: u64,
    src: Src,
}

/// A map file served from the store: its skeleton in memory, every other
/// range read from the raw file its extent names. Reads answer exactly the
/// bytes of the file the map was extracted from.
pub struct StoreFile {
    map: String,
    len: u64,
    file: String,
    skeleton: Vec<u8>,
    pieces: Vec<Piece>,
    raw_root: PathBuf,
    /// The raw file read last: (piece index, file).
    open: Option<(usize, fs::File)>,
    /// Position for [`Read`] and [`Seek`].
    pos: u64,
}

impl std::fmt::Debug for StoreFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoreFile")
            .field("map", &self.map)
            .field("len", &self.len)
            .field("pieces", &self.pieces.len())
            .finish()
    }
}

impl StoreFile {
    /// Whether the store at `root` has `map`'s skeleton and extent table.
    pub fn exists(root: &Path, map: &str) -> bool {
        skel_path(root, map).is_file() && extents_path(root, map).is_file()
    }

    /// The SHA-256 of `map`'s extent table file, if it has one: what
    /// changes when the map's extents change.
    pub fn table_hash(root: &Path, map: &str) -> Option<String> {
        fs::read(extents_path(root, map))
            .ok()
            .map(|b| hex(&super::sha256(&b)))
    }

    /// The extent table of `map` alone (no skeleton read).
    pub fn extents(root: &Path, map: &str) -> Result<Extents> {
        let p = extents_path(root, map);
        let text = fs::read_to_string(&p)
            .map_err(|e| Error::Invalid(format!("store map {map}: {}: {e}", p.display())))?;
        Extents::parse(&text)
    }

    /// Open `map` from the store at `root`.
    pub fn open(root: &Path, map: &str) -> Result<Self> {
        let sp = skel_path(root, map);
        let skel = fs::read(&sp)
            .map_err(|e| Error::Invalid(format!("store map {map}: {}: {e}", sp.display())))?;
        let skel = Skeleton::decode(&skel)?;
        let ext = Self::extents(root, map)?;
        Self::from_parts(map, skel, ext, root.join("raw"))
    }

    /// A store file from a skeleton and an extent table, reading raw files
    /// under `raw_root` (`<repo>/raw`). Refuses parts that do not tile the
    /// file exactly.
    pub fn from_parts(map: &str, skel: Skeleton, ext: Extents, raw_root: PathBuf) -> Result<Self> {
        let bad = |why: String| Error::Invalid(format!("store map {map}: {why}"));
        if skel.len != ext.len {
            return Err(bad(format!(
                "the skeleton is of a {}-byte file, the extent table of a {}-byte one",
                skel.len, ext.len
            )));
        }
        let mut pieces = Vec::with_capacity(skel.ranges.len() + ext.extents.len());
        let mut pos = 0usize;
        for &(offset, len) in &skel.ranges {
            pieces.push(Piece {
                offset,
                len,
                src: Src::Skeleton(pos),
            });
            pos += len as usize;
        }
        for e in &ext.extents {
            pieces.push(Piece {
                offset: e.offset,
                len: e.len,
                src: Src::Raw(e.hash.clone()),
            });
        }
        pieces.sort_by_key(|p| p.offset);
        let mut at = 0u64;
        for p in &pieces {
            if p.len == 0 {
                return Err(bad(format!("an empty range at {:#x}", p.offset)));
            }
            if p.offset != at {
                return Err(bad(format!(
                    "{} at {at:#x} (next range starts at {:#x})",
                    if p.offset > at {
                        "nothing"
                    } else {
                        "two ranges"
                    },
                    p.offset
                )));
            }
            at += p.len;
        }
        if at != skel.len {
            return Err(bad(format!(
                "the ranges end at {at:#x}, the file at {:#x}",
                skel.len
            )));
        }
        Ok(StoreFile {
            map: map.to_owned(),
            len: skel.len,
            file: ext.file,
            skeleton: skel.bytes,
            pieces,
            raw_root,
            open: None,
            pos: 0,
        })
    }

    /// The map file's length.
    pub fn len(&self) -> u64 {
        self.len
    }

    /// Whether the file is empty.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The map's name in the store.
    pub fn map(&self) -> &str {
        &self.map
    }

    /// The SHA-256 the extent table records for the whole file.
    pub fn file_hash(&self) -> &str {
        &self.file
    }

    /// Bytes held in the skeleton.
    pub fn skeleton_bytes(&self) -> u64 {
        self.skeleton.len() as u64
    }

    /// Number of extents (raw files) the file reads.
    pub fn extent_count(&self) -> usize {
        self.pieces
            .iter()
            .filter(|p| matches!(p.src, Src::Raw(_)))
            .count()
    }

    /// Read `[offset, offset + buf.len())`, short at the end of the file
    /// (0 at or past it). A raw file that is missing is
    /// [`Error::MissingRaw`]; one of the wrong length is an error naming it.
    pub fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        if offset >= self.len || buf.is_empty() {
            return Ok(0);
        }
        let n = (buf.len() as u64).min(self.len - offset) as usize;
        let mut done = 0usize;
        while done < n {
            let at = offset + done as u64;
            let i = self.pieces.partition_point(|p| p.offset + p.len <= at);
            let p = &self.pieces[i];
            let within = at - p.offset;
            let take = (p.len - within).min((n - done) as u64) as usize;
            let out = &mut buf[done..done + take];
            match &p.src {
                Src::Skeleton(pos) => {
                    let s = pos + within as usize;
                    out.copy_from_slice(&self.skeleton[s..s + take]);
                }
                Src::Raw(hash) => {
                    let reuse = matches!(&self.open, Some((j, _)) if *j == i);
                    if !reuse {
                        let path = self.raw_root.join(&hash[..2]).join(hash);
                        let f = match fs::File::open(&path) {
                            Ok(f) => f,
                            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                                return Err(Error::MissingRaw {
                                    map: self.map.clone(),
                                    hash: hash.clone(),
                                    offset: p.offset,
                                    len: p.len,
                                });
                            }
                            Err(e) => return Err(e.into()),
                        };
                        let have = f.metadata()?.len();
                        if have != p.len {
                            return Err(Error::Invalid(format!(
                                "store map {}: raw {hash} at {:#x} is {have} bytes, the extent table says {}",
                                self.map, p.offset, p.len
                            )));
                        }
                        self.open = Some((i, f));
                    }
                    let (_, f) = self.open.as_mut().expect("opened");
                    f.seek(SeekFrom::Start(within))?;
                    f.read_exact(out)?;
                }
            }
            done += take;
        }
        Ok(n)
    }
}

impl Read for StoreFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.read_at(self.pos, buf).map_err(|e| match e {
            Error::Io(e) => e,
            e => io::Error::other(e.to_string()),
        })?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for StoreFile {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let next = match to {
            SeekFrom::Start(o) => Some(o),
            SeekFrom::End(d) => self.len.checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        self.pos = next
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek before the start"))?;
        Ok(self.pos)
    }
}

/// One map of the store, as [`status`] reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapStatus {
    /// Map name (file stem).
    pub map: String,
    /// The map file's length.
    pub len: u64,
    /// Its SHA-256, as the extent table records it.
    pub file: String,
    /// Size of the skeleton file (compressed).
    pub skeleton_file: u64,
    /// Number of extents, and the bytes they cover.
    pub extents: (usize, u64),
}

/// What the store at a root holds for serving maps.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StoreStatus {
    /// Whether the root is a tag repository.
    pub exists: bool,
    /// Every map with a skeleton and an extent table, by name.
    pub maps: Vec<MapStatus>,
    /// Maps the extraction record names that have no skeleton or extent
    /// table (an extraction before AS1, or one interrupted): the plan takes
    /// them again.
    pub stale: Vec<String>,
    /// Maps with a table that does not read, with why.
    pub broken: Vec<(String, String)>,
}

/// The maps the store at `root` can serve, read from the extent tables
/// (no skeleton is decompressed).
pub fn status(root: &Path) -> StoreStatus {
    let mut out = StoreStatus::default();
    let Ok(repo) = TagRepo::open(root) else {
        return out;
    };
    out.exists = true;
    let mut names: Vec<String> = fs::read_dir(root.join("maps"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            e.file_name()
                .to_str()?
                .strip_suffix(&format!(".{EXTENTS_EXT}"))
                .map(str::to_owned)
        })
        .collect();
    names.sort();
    for map in names {
        let skel = fs::metadata(skel_path(root, &map)).map(|m| m.len());
        match (StoreFile::extents(root, &map), skel) {
            (Ok(x), Ok(s)) => out.maps.push(MapStatus {
                len: x.len,
                file: x.file.clone(),
                skeleton_file: s,
                extents: (x.extents.len(), x.raw_bytes()),
                map,
            }),
            (Err(e), _) => out.broken.push((map, e.to_string())),
            (_, Err(e)) => out.broken.push((map, format!("no skeleton: {e}"))),
        }
    }
    if let Ok(Some(rec)) = repo.extract_record() {
        out.stale = rec
            .maps
            .keys()
            .filter(|m| !repo.has_skeleton(m))
            .cloned()
            .collect();
    }
    out
}
