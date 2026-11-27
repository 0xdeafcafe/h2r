//! [`Base`]: the map a build reads, in memory or read by offset from a
//! source it never copies whole (a map the VFS streams:
//! docs/runtime/storage.md, "Filesystem implementations").
//!
//! A build opens several readers of its base at once (the decomposed map,
//! the relocator, the asset hashes), so a source is shared through
//! [`Shared`]; each [`BaseReader`] keeps its own position.
use std::cell::RefCell;
use std::io::{self, Cursor, Read, Seek, SeekFrom};

use content::Sha256;
use content::patch::Source;

/// The base map's bytes.
#[derive(Clone, Copy)]
pub enum Base<'a> {
    /// The whole file in memory.
    Bytes(&'a [u8]),
    /// A file read by offset ([`Shared`]).
    Shared(&'a dyn ReadShared),
}

/// A file read by offset through a shared reference.
pub trait ReadShared {
    fn len(&self) -> u64;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize>;
    fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()>;
}

impl<'a> From<&'a [u8]> for Base<'a> {
    fn from(bytes: &'a [u8]) -> Self {
        Self::Bytes(bytes)
    }
}

impl<'a> From<&'a Vec<u8>> for Base<'a> {
    fn from(bytes: &'a Vec<u8>) -> Self {
        Self::Bytes(bytes)
    }
}

impl<'a, 's> From<&'a Shared<'s>> for Base<'a> {
    fn from(shared: &'a Shared<'s>) -> Self {
        Self::Shared(shared)
    }
}

/// How much [`Base::sha256`] reads at once.
const CHUNK: usize = 1 << 20;

impl<'a> Base<'a> {
    /// The file's length.
    pub fn len(self) -> u64 {
        match self {
            Self::Bytes(b) => b.len() as u64,
            Self::Shared(s) => s.len(),
        }
    }

    pub fn is_empty(self) -> bool {
        self.len() == 0
    }

    /// A reader of the file, from its start.
    pub fn reader(self) -> BaseReader<'a> {
        match self {
            Self::Bytes(b) => BaseReader::Bytes(Cursor::new(b)),
            Self::Shared(s) => BaseReader::Shared { shared: s, pos: 0 },
        }
    }

    /// The file's SHA-256, read a megabyte at a time.
    pub fn sha256(self) -> io::Result<Sha256> {
        match self {
            Self::Bytes(b) => Ok(Sha256::of(b)),
            Self::Shared(s) => {
                let mut h = content::Hasher::default();
                let mut buf = vec![0u8; CHUNK.min(s.len() as usize)];
                let mut at = 0;
                while at < s.len() {
                    let n = (s.len() - at).min(CHUNK as u64) as usize;
                    s.read_exact_at(at, &mut buf[..n])?;
                    h.update(&buf[..n]);
                    at += n as u64;
                }
                Ok(h.finish())
            }
        }
    }
}

impl Source for Base<'_> {
    fn size(&self) -> u64 {
        self.len()
    }
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        match *self {
            Self::Bytes(b) => {
                let mut b = b;
                b.read_at(offset, buf)
            }
            Self::Shared(s) => s.read_at(offset, buf),
        }
    }
}

/// A source shared by every reader of one build. Reads borrow it one at a
/// time; a build is single-threaded.
pub struct Shared<'a> {
    source: RefCell<&'a mut dyn Source>,
    len: u64,
}

impl<'a> Shared<'a> {
    pub fn new(source: &'a mut dyn Source) -> Self {
        let len = source.size();
        Self {
            source: RefCell::new(source),
            len,
        }
    }
}

impl ReadShared for Shared<'_> {
    fn len(&self) -> u64 {
        self.len
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        self.source.borrow_mut().read_at(offset, buf)
    }
    fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        self.source.borrow_mut().read_exact_at(offset, buf)
    }
}

/// A reader of a [`Base`], with its own position.
pub enum BaseReader<'a> {
    Bytes(Cursor<&'a [u8]>),
    Shared { shared: &'a dyn ReadShared, pos: u64 },
}

impl Read for BaseReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Bytes(c) => c.read(buf),
            Self::Shared { shared, pos } => {
                let n = shared.len().saturating_sub(*pos).min(buf.len() as u64) as usize;
                if n == 0 {
                    return Ok(0);
                }
                let got = shared.read_at(*pos, &mut buf[..n])?;
                *pos += got as u64;
                Ok(got)
            }
        }
    }
}

impl Seek for BaseReader<'_> {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        match self {
            Self::Bytes(c) => c.seek(to),
            Self::Shared { shared, pos } => {
                let new = match to {
                    SeekFrom::Start(p) => Some(p),
                    SeekFrom::End(d) => shared.len().checked_add_signed(d),
                    SeekFrom::Current(d) => pos.checked_add_signed(d),
                };
                *pos = new.ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
                Ok(*pos)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readers_of_a_shared_source_keep_their_own_places() {
        let bytes: Vec<u8> = (0..=255).cycle().take(3 << 20).map(|b: u32| b as u8).collect();
        let mut source: &[u8] = &bytes;
        let shared = Shared::new(&mut source);
        let base = Base::from(&shared);
        let (mut a, mut b) = (base.reader(), base.reader());
        a.seek(SeekFrom::Start(300)).unwrap();
        let mut x = [0; 4];
        b.read_exact(&mut x).unwrap();
        assert_eq!(x, [0, 1, 2, 3]);
        a.read_exact(&mut x).unwrap();
        assert_eq!(x, [44, 45, 46, 47]);
        assert_eq!(b.seek(SeekFrom::End(-1)).unwrap(), bytes.len() as u64 - 1);
        assert_eq!(base.sha256().unwrap(), Base::from(&bytes).sha256().unwrap());
    }
}
