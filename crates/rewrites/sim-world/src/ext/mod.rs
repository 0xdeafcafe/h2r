//! `SimExt`: the side arena for simulation state with no retail home
//! (docs/engine/sim-rewrite.md section 2, the **Extension** kind).
//!
//! A container of named, versioned records. Each feature registers its own
//! [`Kind`] (`RulesExt` is [`RULES`], `ForgeExt` is [`FORGE`]) and keeps its
//! record as bytes it encodes itself. The container has one canonical byte
//! form ([`SimExt::save`]), so two machines with the same records have the
//! same bytes, the same CRC and the same [`SimExt::digest`]:
//!
//! ```text
//! empty container    zero bytes
//! otherwise          "SEXT" | u16 FORMAT | u16 count | record * count | u32 CRC-32
//! record             u8 name length | name | u16 version | u32 length | bytes
//! ```
//!
//! Records are in name order, every integer is little-endian, and the
//! CRC-32 (IEEE) covers everything before it. [`SimExt::restore`] refuses a
//! blob of another format, a record of an unregistered kind or of another
//! version, a record over its kind's cap, a container over [`MAX_BYTES`],
//! and any blob that is not canonical, and changes nothing when it refuses.
//!
//! [`checkpoint::State`] keeps the copies taken at checkpoint saves.
//! [`store::ExtStore`] holds the process's state as the world's `kind::EXT`
//! section, so `World::save`, `World::restore_into` and `World::sim_hash`
//! cover it and film keyframes carry it. `sim_bridge::sim_ext` moves the
//! slot copies with the guest's checkpoints, and `lockstep::hash_sim` folds
//! [`digest`] into the arena hash when the container holds a record.

pub mod checkpoint;
pub mod store;
pub use checkpoint::{SLOTS, State, digest, global};

use std::collections::BTreeMap;
use std::fmt;
use std::hash::Hasher;

/// The first bytes of a non-empty container.
pub const MAGIC: [u8; 4] = *b"SEXT";
/// The container's layout version. A blob of any other is refused.
pub const FORMAT: u16 = 1;
/// The largest container, all records and framing included.
pub const MAX_BYTES: usize = 256 * 1024;
/// The longest record name.
pub const MAX_NAME: usize = 32;

const HEAD: usize = 4 + 2 + 2;
const RECORD_HEAD: usize = 1 + 2 + 4;
const CRC: usize = 4;

/// A record kind: its name, the layout version of its bytes, and its cap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Kind {
    /// Lowercase ASCII letters, digits, `-`, `_` and `.`; at most
    /// [`MAX_NAME`] bytes.
    pub name: &'static str,
    /// The owner's layout version. A saved record of another version is
    /// refused, not migrated.
    pub version: u16,
    /// The largest record of this kind.
    pub max_len: usize,
}

impl Kind {
    pub const fn new(name: &'static str, version: u16, max_len: usize) -> Self {
        Self {
            name,
            version,
            max_len,
        }
    }
}

/// `RulesExt`: the rules VM's variables, timers and generator state
/// (docs/game/rules-engine.md, "Carrying a script": VM state 16 KiB):
/// `rules::RulesExt::encode`, set by the rules host runtime
/// (`game::rules::runtime`) at a script game's start and removed at its
/// end.
pub const RULES: Kind = Kind::new("rules", 1, 64 * 1024);
/// `ForgeExt`: emitter timers, logic node state, volume occupancy
/// (docs/game/forge-design.md, slice F7). Opaque until F7 lands.
pub const FORGE: Kind = Kind::new("forge", 1, 64 * 1024);
/// The kinds every container knows from the start.
pub const BUILT_IN: [Kind; 2] = [FORGE, RULES];

/// Why a record or a blob was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// A name that is empty, too long or has a character outside the set.
    Name(String),
    /// A kind registered under this name with another version or cap.
    Conflict(String),
    /// A record of a kind nobody registered.
    Unregistered(String),
    /// A record over its kind's cap.
    RecordTooLarge {
        name: String,
        len: usize,
        max: usize,
    },
    /// A container over [`MAX_BYTES`].
    TooLarge { len: usize },
    /// The bytes end inside a field.
    Truncated,
    /// The first four bytes are not [`MAGIC`].
    Magic,
    /// A container layout other than [`FORMAT`].
    Format { found: u16 },
    /// The stored CRC-32 does not match the bytes.
    Crc { stored: u32, computed: u32 },
    /// A record saved by another layout version of its kind.
    RecordVersion {
        name: String,
        found: u16,
        expected: u16,
    },
    /// Records out of name order, a repeated name, or a container that
    /// says it holds no records (the empty container is zero bytes).
    NotCanonical(String),
    /// Bytes after the last record.
    Trailing,
    /// A checkpoint slot outside `0..SLOTS`.
    Slot(u16),
    /// A revert to a slot no save was seen for; the records were cleared.
    NoSlot(u16),
    /// A snapshot section that is not this build's layout.
    Section(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Name(n) => write!(f, "record name {n:?} is not a valid kind name"),
            Error::Conflict(n) => {
                write!(
                    f,
                    "record kind {n:?} is already registered with another version or cap"
                )
            }
            Error::Unregistered(n) => write!(f, "record kind {n:?} is not registered"),
            Error::RecordTooLarge { name, len, max } => {
                write!(f, "record {name:?} is {len} bytes, over its {max}-byte cap")
            }
            Error::TooLarge { len } => {
                write!(
                    f,
                    "extension state is {len} bytes, over the {MAX_BYTES}-byte cap"
                )
            }
            Error::Truncated => f.write_str("extension state ends inside a field"),
            Error::Magic => f.write_str("not extension state (bad magic)"),
            Error::Format { found } => {
                write!(
                    f,
                    "extension state format {found}; this build reads {FORMAT}"
                )
            }
            Error::Crc { stored, computed } => write!(
                f,
                "extension state CRC {stored:08x} does not match its bytes ({computed:08x})"
            ),
            Error::RecordVersion {
                name,
                found,
                expected,
            } => write!(
                f,
                "record {name:?} is version {found}; this build reads version {expected}"
            ),
            Error::NotCanonical(why) => write!(f, "extension state is not canonical: {why}"),
            Error::Trailing => f.write_str("extension state has bytes after its last record"),
            Error::Slot(s) => write!(f, "checkpoint slot {s} is out of range"),
            Error::NoSlot(s) => write!(
                f,
                "no extension state was saved with checkpoint slot {s}; records cleared"
            ),
            Error::Section(why) => write!(f, "extension snapshot section: {why}"),
        }
    }
}

impl std::error::Error for Error {}

fn check_name(name: &str) -> Result<(), Error> {
    let ok = !name.is_empty()
        && name.len() <= MAX_NAME
        && name
            .bytes()
            .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.'));
    if ok {
        Ok(())
    } else {
        Err(Error::Name(name.into()))
    }
}

/// The records of one machine's extension state, and the kinds it accepts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SimExt {
    kinds: BTreeMap<&'static str, Kind>,
    records: BTreeMap<&'static str, Vec<u8>>,
}

impl Default for SimExt {
    fn default() -> Self {
        Self::new()
    }
}

impl SimExt {
    /// No records; the [`BUILT_IN`] kinds registered.
    pub fn new() -> Self {
        let mut ext = Self::bare();
        for kind in BUILT_IN {
            ext.register(kind).expect("the built-in kinds are valid");
        }
        ext
    }

    /// No records and no kinds.
    pub fn bare() -> Self {
        Self {
            kinds: BTreeMap::new(),
            records: BTreeMap::new(),
        }
    }

    /// Accept records of `kind`. Registering the same kind again is a
    /// no-op; the same name with another version or cap is refused.
    pub fn register(&mut self, kind: Kind) -> Result<(), Error> {
        check_name(kind.name)?;
        match self.kinds.get(kind.name) {
            Some(k) if *k == kind => Ok(()),
            Some(_) => Err(Error::Conflict(kind.name.into())),
            None => {
                self.kinds.insert(kind.name, kind);
                Ok(())
            }
        }
    }

    /// The registered kinds, in name order.
    pub fn kinds(&self) -> impl Iterator<Item = &Kind> + '_ {
        self.kinds.values()
    }

    fn registered(&self, kind: &Kind) -> Result<&'static str, Error> {
        match self.kinds.get(kind.name) {
            Some(k) if k == kind => Ok(k.name),
            Some(_) => Err(Error::Conflict(kind.name.into())),
            None => Err(Error::Unregistered(kind.name.into())),
        }
    }

    /// Replace `kind`'s record. Refused, with nothing changed, for an
    /// unregistered kind, a record over its cap, or a container that would
    /// go over [`MAX_BYTES`].
    pub fn set(&mut self, kind: &Kind, bytes: Vec<u8>) -> Result<(), Error> {
        let name = self.registered(kind)?;
        if bytes.len() > kind.max_len {
            return Err(Error::RecordTooLarge {
                name: name.into(),
                len: bytes.len(),
                max: kind.max_len,
            });
        }
        let others: usize = self
            .records
            .iter()
            .filter(|(n, _)| **n != name)
            .map(|(n, d)| RECORD_HEAD + n.len() + d.len())
            .sum();
        let len = HEAD + CRC + others + RECORD_HEAD + name.len() + bytes.len();
        if len > MAX_BYTES {
            return Err(Error::TooLarge { len });
        }
        self.records.insert(name, bytes);
        Ok(())
    }

    /// `kind`'s record, if there is one.
    pub fn get(&self, kind: &Kind) -> Option<&[u8]> {
        let name = self.registered(kind).ok()?;
        self.records.get(name).map(Vec::as_slice)
    }

    /// Drop `kind`'s record.
    pub fn remove(&mut self, kind: &Kind) -> Option<Vec<u8>> {
        let name = self.registered(kind).ok()?;
        self.records.remove(name)
    }

    /// Drop every record and keep the kinds. Returns how many there were.
    pub fn clear(&mut self) -> usize {
        let n = self.records.len();
        self.records.clear();
        n
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// How many records it holds.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// The names of the records it holds, in order.
    pub fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.records.keys().copied()
    }

    /// The length of [`SimExt::save`]'s bytes.
    pub fn encoded_len(&self) -> usize {
        if self.records.is_empty() {
            return 0;
        }
        HEAD + CRC
            + self
                .records
                .iter()
                .map(|(n, d)| RECORD_HEAD + n.len() + d.len())
                .sum::<usize>()
    }

    /// The canonical bytes: zero bytes with no records.
    pub fn save(&self) -> Vec<u8> {
        if self.records.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(self.encoded_len());
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&FORMAT.to_le_bytes());
        out.extend_from_slice(&(self.records.len() as u16).to_le_bytes());
        for (name, data) in &self.records {
            let version = self.kinds.get(name).map_or(0, |k| k.version);
            out.push(name.len() as u8);
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&version.to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(data);
        }
        let crc = crc32(&out);
        out.extend_from_slice(&crc.to_le_bytes());
        out
    }

    /// Replace every record with `bytes`'s ([`SimExt::save`]'s form; zero
    /// bytes clears). A refusal changes nothing.
    pub fn restore(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.records = self.decode(bytes)?;
        Ok(())
    }

    /// Check `bytes` against the registered kinds without keeping them.
    pub fn check(&self, bytes: &[u8]) -> Result<(), Error> {
        self.decode(bytes).map(drop)
    }

    fn decode(&self, bytes: &[u8]) -> Result<BTreeMap<&'static str, Vec<u8>>, Error> {
        let mut records = BTreeMap::new();
        if bytes.is_empty() {
            return Ok(records);
        }
        if bytes.len() > MAX_BYTES {
            return Err(Error::TooLarge { len: bytes.len() });
        }
        if bytes.len() < HEAD + CRC {
            return Err(Error::Truncated);
        }
        if bytes[..4] != MAGIC {
            return Err(Error::Magic);
        }
        let found = u16::from_le_bytes([bytes[4], bytes[5]]);
        if found != FORMAT {
            return Err(Error::Format { found });
        }
        let (body, tail) = bytes.split_at(bytes.len() - CRC);
        let stored = u32::from_le_bytes([tail[0], tail[1], tail[2], tail[3]]);
        let computed = crc32(body);
        if stored != computed {
            return Err(Error::Crc { stored, computed });
        }
        let count = u16::from_le_bytes([body[6], body[7]]);
        if count == 0 {
            return Err(Error::NotCanonical("a container with no records".into()));
        }
        let mut r = Reader::new(body, HEAD);
        let mut last: Option<&'static str> = None;
        for _ in 0..count {
            let n = r.u8()? as usize;
            let raw = r.take(n)?;
            let name = std::str::from_utf8(raw)
                .map_err(|_| Error::Name(String::from_utf8_lossy(raw).into_owned()))?;
            check_name(name)?;
            let kind = self
                .kinds
                .get(name)
                .ok_or_else(|| Error::Unregistered(name.into()))?;
            let version = r.u16()?;
            if version != kind.version {
                return Err(Error::RecordVersion {
                    name: name.into(),
                    found: version,
                    expected: kind.version,
                });
            }
            let len = r.u32()? as usize;
            if len > kind.max_len {
                return Err(Error::RecordTooLarge {
                    name: name.into(),
                    len,
                    max: kind.max_len,
                });
            }
            let data = r.take(len)?.to_vec();
            if last.is_some_and(|l| l >= kind.name) {
                return Err(Error::NotCanonical(format!(
                    "record {name:?} is out of name order or repeated"
                )));
            }
            last = Some(kind.name);
            records.insert(kind.name, data);
        }
        if !r.done() {
            return Err(Error::Trailing);
        }
        Ok(records)
    }

    /// Feed the canonical bytes to `h`.
    pub fn hash_into<H: Hasher>(&self, h: &mut H) {
        h.write(&self.save());
    }

    /// The FNV-1a 64 digest of the canonical bytes; `None` with no
    /// records, so an unused container leaves every hash it is folded into
    /// as it was.
    pub fn digest(&self) -> Option<u64> {
        if self.records.is_empty() {
            return None;
        }
        let mut h = Fnv64::new();
        self.hash_into(&mut h);
        Some(h.finish())
    }
}

/// Bounds-checked little-endian reads.
pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(bytes: &'a [u8], at: usize) -> Self {
        Self { bytes, at }
    }
    pub(crate) fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let end = self.at.checked_add(n).ok_or(Error::Truncated)?;
        let out = self.bytes.get(self.at..end).ok_or(Error::Truncated)?;
        self.at = end;
        Ok(out)
    }
    pub(crate) fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }
    pub(crate) fn u16(&mut self) -> Result<u16, Error> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    pub(crate) fn u32(&mut self) -> Result<u32, Error> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    pub(crate) fn done(&self) -> bool {
        self.at == self.bytes.len()
    }
}

/// CRC-32 (IEEE 802.3, reflected, polynomial `0xedb88320`).
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in bytes {
        crc ^= u32::from(b);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

/// FNV-1a, 64-bit: the same on every host and Rust release, unlike
/// `DefaultHasher`.
#[derive(Clone, Copy, Debug)]
pub struct Fnv64(u64);

impl Fnv64 {
    pub const fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl Default for Fnv64 {
    fn default() -> Self {
        Self::new()
    }
}

impl Hasher for Fnv64 {
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    fn finish(&self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests;
