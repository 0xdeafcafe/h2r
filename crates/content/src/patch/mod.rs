//! File patch formats and the byte runs they become.
//!
//! PPF and IPS are offset patches: they become an [`OffsetPatch`], sorted
//! non-overlapping byte runs, and several of them compose into an
//! [`Overlay`] that a reader lays over the base as it streams, without
//! copying the base. BPS moves data (copies from the source and from the
//! target), so it is applied to the whole base in memory ([`Bps::apply`])
//! and the result is materialised.
//!
//! Growth: an IPS or PPF record past the end of the base grows the file,
//! with zeros where nothing is written, as the tools that apply them do
//! (Lunar IPS; ApplyPPF seeks to the record's offset and writes, which
//! extends the file), up to [`MAX_LEN`]. IPS may also truncate (the common
//! Lunar IPS extension). BPS states its target size.
//!
//! [`write`] makes patches: PPF3 for a same-length edit, BPS otherwise.
use std::io;

pub(crate) mod bps;
pub(crate) mod ips;
mod overlay;
pub(crate) mod ppf;
pub mod write;

pub use bps::{Bps, crc32};
pub use ips::parse_ips;
pub use overlay::{MAX_LEN, OffsetPatch, Overlay, OverlayView, Run};
pub use ppf::{BlockCheck, Ppf};
pub use write::{Format, Written, diff};

#[derive(Debug, thiserror::Error)]
pub enum PatchError {
    #[error("{format}: truncated ({what})")]
    Truncated {
        format: &'static str,
        what: &'static str,
    },
    #[error("{format}: {why}")]
    Malformed { format: &'static str, why: String },
    #[error("{format}: writes bytes {start:#x}..{end:#x}, past the 4 GiB a map file can be")]
    TooLong {
        format: &'static str,
        start: u64,
        end: u64,
    },
    #[error("{format}: {what} does not match the base")]
    BaseMismatch { format: &'static str, what: String },
    #[error("unrecognised patch format{0}")]
    Unknown(String),
    #[error(transparent)]
    Io(#[from] io::Error),
}

pub(crate) fn truncated(format: &'static str, what: &'static str) -> PatchError {
    PatchError::Truncated { format, what }
}

/// Random-access bytes: the base map, a host file, a buffer.
pub trait Source {
    /// Its length in bytes.
    fn size(&self) -> u64;
    /// Read up to `buf.len()` bytes at `offset`; fewer only at the end.
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize>;
    /// Exactly `buf.len()` bytes at `offset`.
    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        let mut at = 0;
        while at < buf.len() {
            match self.read_at(offset + at as u64, &mut buf[at..])? {
                0 => return Err(io::ErrorKind::UnexpectedEof.into()),
                n => at += n,
            }
        }
        Ok(())
    }
    /// All of it.
    fn read_all(&mut self) -> io::Result<Vec<u8>> {
        let len = usize::try_from(self.size()).map_err(|_| io::Error::other("source too large"))?;
        let mut out = vec![0; len];
        self.read_exact_at(0, &mut out)?;
        Ok(out)
    }
}

impl Source for &[u8] {
    fn size(&self) -> u64 {
        <[u8]>::len(self) as u64
    }
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(<[u8]>::len(self));
        let n = (<[u8]>::len(self) - start).min(buf.len());
        buf[..n].copy_from_slice(&self[start..start + n]);
        Ok(n)
    }
}

/// A host file as a [`Source`], its length fixed when opened.
pub struct FileSource {
    file: std::fs::File,
    len: u64,
}
impl FileSource {
    pub fn new(file: std::fs::File) -> io::Result<Self> {
        let len = file.metadata()?.len();
        Ok(Self { file, len })
    }
    pub fn open(path: &std::path::Path) -> io::Result<Self> {
        Self::new(std::fs::File::open(path)?)
    }
}
impl Source for FileSource {
    fn size(&self) -> u64 {
        self.len
    }
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        use std::io::{Read, Seek, SeekFrom};
        let n = self.len.saturating_sub(offset).min(buf.len() as u64) as usize;
        if n == 0 {
            return Ok(0);
        }
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.read_exact(&mut buf[..n])?;
        Ok(n)
    }
}

/// A parsed file layer.
#[derive(Debug)]
pub enum Patch {
    Ppf(Ppf),
    Ips(OffsetPatch),
    Bps(Bps),
}

impl Patch {
    /// Parse a patch, recognising its format by its signature.
    pub fn parse(bytes: &[u8]) -> Result<Self, PatchError> {
        if bytes.starts_with(b"PPF") {
            return Ppf::parse(bytes).map(Self::Ppf);
        }
        if bytes.starts_with(b"PATCH") {
            return parse_ips(bytes).map(Self::Ips);
        }
        if bytes.starts_with(b"BPS1") {
            return Bps::parse(bytes).map(Self::Bps);
        }
        Err(PatchError::Unknown(String::new()))
    }

    /// Whether `bytes` start like a patch this module reads.
    pub fn recognises(bytes: &[u8]) -> bool {
        [&b"PPF"[..], b"PATCH", b"BPS1"]
            .iter()
            .any(|m| bytes.starts_with(m))
    }
}
