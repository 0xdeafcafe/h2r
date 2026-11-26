//! Byte reader and writer for the canonical save: little-endian, no padding.

/// Names the layout a save was written under. A save from any other stamp
/// is refused by `World::restore`, with the stamp it carried in the error.
pub const BUILD_STAMP: &str = concat!("sim-world ", env!("CARGO_PKG_VERSION"), " layout 1");

const MAGIC: &[u8; 4] = b"H2WS";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RestoreError {
    /// Not a world save.
    BadMagic,
    /// Written by another build: `found` is the stamp it carries.
    OtherLayout { found: String },
    /// The save ends inside a field.
    Truncated,
    /// Sections are not in strictly ascending kind order.
    SectionOrder { kind: u16 },
    /// A section kind this build has no store for.
    UnknownKind { kind: u16 },
    /// A store every world has is absent.
    MissingSection { kind: u16 },
    /// A section's version is not the one this build writes.
    SectionVersion { kind: u16, version: u16 },
    /// Bytes left over after a section's records.
    Trailing { kind: u16 },
    /// A section is internally inconsistent (counts, salts, indexes).
    Corrupt { kind: u16, what: &'static str },
}

impl std::fmt::Display for RestoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadMagic => write!(f, "not a world save"),
            Self::OtherLayout { found } => {
                write!(f, "save is from {found:?}, this build is {BUILD_STAMP:?}")
            }
            Self::Truncated => write!(f, "save ends inside a field"),
            Self::SectionOrder { kind } => write!(f, "section {kind} is out of order"),
            Self::UnknownKind { kind } => write!(f, "no store for section kind {kind}"),
            Self::MissingSection { kind } => write!(f, "section {kind} is missing"),
            Self::SectionVersion { kind, version } => {
                write!(f, "section {kind} has version {version}")
            }
            Self::Trailing { kind } => write!(f, "section {kind} has trailing bytes"),
            Self::Corrupt { kind, what } => write!(f, "section {kind}: {what}"),
        }
    }
}

impl std::error::Error for RestoreError {}

#[derive(Default)]
pub struct Writer(Vec<u8>);

impl Writer {
    pub fn new() -> Self {
        Self(Vec::new())
    }
    pub fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    pub fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub fn bytes(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
    pub fn finish(self) -> Vec<u8> {
        self.0
    }
}

pub struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self(bytes)
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn take(&mut self, n: usize) -> Result<&'a [u8], RestoreError> {
        if self.0.len() < n {
            return Err(RestoreError::Truncated);
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    pub fn u8(&mut self) -> Result<u8, RestoreError> {
        Ok(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16, RestoreError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    pub fn u32(&mut self) -> Result<u32, RestoreError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    pub fn u64(&mut self) -> Result<u64, RestoreError> {
        let b = self.take(8)?;
        let mut a = [0; 8];
        a.copy_from_slice(b);
        Ok(u64::from_le_bytes(a))
    }
    /// The section body must be fully consumed.
    pub fn end(&self, kind: u16) -> Result<(), RestoreError> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(RestoreError::Trailing { kind })
        }
    }
}

pub(crate) fn write_header(w: &mut Writer) {
    w.bytes(MAGIC);
    w.u32(BUILD_STAMP.len() as u32);
    w.bytes(BUILD_STAMP.as_bytes());
}

pub(crate) fn read_header(r: &mut Reader<'_>) -> Result<(), RestoreError> {
    if r.take(4)? != MAGIC {
        return Err(RestoreError::BadMagic);
    }
    let len = r.u32()? as usize;
    let stamp = r.take(len)?;
    if stamp != BUILD_STAMP.as_bytes() {
        return Err(RestoreError::OtherLayout {
            found: String::from_utf8_lossy(stamp).into_owned(),
        });
    }
    Ok(())
}
