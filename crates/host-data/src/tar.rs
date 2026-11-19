//! Read and write the bundle's regular-file ustar entries. Header fields
//! and checksums follow POSIX ustar. The reader rejects links and extensions
//! and validates each relative path before extraction.

use std::io::{self, Read, Write};

use sha2::{Digest, Sha256};

/// One tar block, in bytes.
pub(crate) const BLOCK: usize = 512;
/// The ustar size field holds 11 octal digits; a file may not be larger.
pub(crate) const MAX_FILE_BYTES: u64 = 8_589_934_591; // 8 GiB - 1

/// A path that is safe to store in, and extract from, a bundle: relative,
/// `/`-separated, one plain name per component.
///
/// This rejects absolute paths, `..` and `.` components, empty components,
/// backslashes, drive colons, NUL, non-ASCII characters, and anything
/// longer than the ustar name field. It is the single check both the
/// writer and the extractor run; the extractor additionally joins only
/// onto a directory it created itself.
pub(crate) fn validate_path(path: &str) -> Result<(), String> {
    let bad = |why: &str| format!("path {path:?}: {why}");
    if path.is_empty() {
        return Err(bad("is empty"));
    }
    if path.len() > 99 {
        return Err(bad("is longer than the ustar name field"));
    }
    if path.starts_with('/') {
        return Err(bad("is absolute"));
    }
    if path.ends_with('/') {
        return Err(bad("has a trailing separator"));
    }
    for b in path.bytes() {
        if !matches!(b, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'.' | b'_' | b'-' | b'/') {
            return Err(bad("has a character outside [A-Za-z0-9._-/]"));
        }
    }
    if path.contains("//") {
        return Err(bad("has an empty component"));
    }
    for component in path.split('/') {
        if component.ends_with('.') {
            return Err(bad("has a component ending in a dot"));
        }
        let stem = component.split('.').next().unwrap().to_ascii_uppercase();
        let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.as_bytes()[3].is_ascii_digit());
        if device {
            return Err(bad("has a reserved Windows device name"));
        }
    }
    Ok(())
}

/// Write a ustar header block for `name`/`size`.
///
/// Fixed, deterministic fields: mode 0644, uid/gid 0, mtime 0, typeflag
/// `0` (regular file), no prefix. The checksum is the byte sum of the
/// block with the checksum field itself read as eight spaces.
fn write_header(out: &mut impl Write, name: &str, size: u64) -> io::Result<()> {
    debug_assert!(validate_path(name).is_ok());
    assert!(size <= MAX_FILE_BYTES, "file larger than ustar can encode");
    let mut h = [0u8; BLOCK];
    h[..name.len()].copy_from_slice(name.as_bytes());
    h[100..108].copy_from_slice(b"0000644\0"); // mode
    h[108..116].copy_from_slice(b"0000000\0"); // uid
    h[116..124].copy_from_slice(b"0000000\0"); // gid
    h[124..136].copy_from_slice(format!("{size:011o}\0").as_bytes()); // size
    h[136..148].copy_from_slice(b"00000000000\0"); // mtime
    h[156] = b'0'; // typeflag: regular file
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    let sum: u32 = h.iter().map(|&b| u32::from(b)).sum::<u32>() + 8 * u32::from(b' ');
    h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes()); // checksum
    out.write_all(&h)
}

/// A ustar archive under construction. Entries are appended in order and
/// streamed; nothing is held in memory beyond one buffer.
pub(crate) struct TarWriter<W> {
    out: W,
}

impl<W: Write> TarWriter<W> {
    pub(crate) fn new(out: W) -> Self {
        Self { out }
    }

    /// Append one regular file: exactly `size` bytes read from `src`
    /// (a short source is an error), zero-padded to a block boundary.
    /// Returns the sha256 of the file's bytes, as lowercase hex.
    pub(crate) fn append(&mut self, path: &str, size: u64, src: &mut impl Read) -> io::Result<String> {
        validate_path(path).map_err(io::Error::other)?;
        if size > MAX_FILE_BYTES {
            return Err(io::Error::other(format!("{path} is larger than ustar can encode")));
        }
        write_header(&mut self.out, path, size)?;
        let mut hasher = Sha256::new();
        let mut remaining = size;
        let mut buf = vec![0u8; 256 * 1024];
        while remaining > 0 {
            let want = buf.len().min(usize::try_from(remaining).unwrap_or(buf.len()));
            src.read_exact(&mut buf[..want])?;
            self.out.write_all(&buf[..want])?;
            hasher.update(&buf[..want]);
            remaining -= want as u64;
        }
        let pad = (BLOCK - (size % BLOCK as u64) as usize) % BLOCK;
        if pad > 0 {
            self.out.write_all(&vec![0u8; pad])?;
        }
        Ok(hex(&hasher.finalize()))
    }

    /// Write the two zero end blocks and return the underlying writer.
    pub(crate) fn finish(mut self) -> io::Result<W> {
        self.out.write_all(&[0u8; BLOCK * 2])?;
        self.out.flush()?;
        Ok(self.out)
    }
}

/// A streaming ustar reader over regular-file entries.
pub(crate) struct TarReader<R> {
    src: R,
    /// Bytes left in the current entry's data.
    remaining: u64,
    padding: usize,
    finished: bool,
}

/// A malformed or unexpected archive. [`crate::Error::Corrupt`] carries
/// these out of extraction.
#[derive(Debug, thiserror::Error)]
pub(crate) enum TarError {
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("corrupt archive: {0}")]
    Corrupt(&'static str),
    #[error("unsafe entry path: {0}")]
    UnsafePath(String),
}

impl<R: Read> TarReader<R> {
    pub(crate) fn new(src: R) -> Self {
        Self { src, remaining: 0, padding: 0, finished: false }
    }

    #[cfg(test)]
    pub(crate) fn remaining(&self) -> u64 {
        self.remaining
    }

    /// The next entry as `(path, size)`, or `None` at the end-of-archive
    /// marker. Data of an entry left unread is skipped, so a caller that
    /// only wants names never has to copy payloads.
    pub(crate) fn next(&mut self) -> Result<Option<(String, u64)>, TarError> {
        if self.finished {
            return Ok(None);
        }
        self.skip_current()?;
        let mut h = [0u8; BLOCK];
        self.src.read_exact(&mut h[..self.padding])?;
        if h[..self.padding].iter().any(|&b| b != 0) {
            return Err(TarError::Corrupt("nonzero entry padding"));
        }
        read_block(&mut self.src, &mut h)?;
        if h.iter().all(|&b| b == 0) {
            read_block(&mut self.src, &mut h)?;
            if h.iter().any(|&b| b != 0) {
                return Err(TarError::Corrupt("missing second end-of-archive block"));
            }
            require_zero_tail(&mut self.src)?;
            self.finished = true;
            return Ok(None);
        }
        if &h[257..263] != b"ustar\0" || &h[263..265] != b"00" {
            return Err(TarError::Corrupt("not a ustar archive"));
        }
        let stored =
            parse_octal(&h[148..156]).ok_or(TarError::Corrupt("unreadable checksum field"))?;
        let filled: u32 = h[..148].iter().chain(h[156..].iter()).map(|&b| u32::from(b)).sum();
        if u64::from(filled + 8 * u32::from(b' ')) != stored {
            return Err(TarError::Corrupt("header checksum mismatch"));
        }
        if h[156] != b'0' {
            // Directories, links, devices and pax entries are all refused:
            // the bundle holds regular files only.
            return Err(TarError::Corrupt("only regular-file entries are allowed"));
        }
        if h[157..257].iter().any(|&b| b != 0) {
            return Err(TarError::Corrupt("link name in use"));
        }
        if h[345..500].iter().any(|&b| b != 0) {
            return Err(TarError::Corrupt("ustar prefix field in use"));
        }
        let name_len = h[..100]
            .iter()
            .position(|&b| b == 0)
            .ok_or(TarError::Corrupt("unterminated entry name"))?;
        let name = std::str::from_utf8(&h[..name_len])
            .map_err(|_| TarError::Corrupt("entry name is not UTF-8"))?
            .to_owned();
        validate_path(&name).map_err(TarError::UnsafePath)?;
        let size =
            parse_octal(&h[124..136]).ok_or(TarError::Corrupt("unreadable size field"))?;
        if size > MAX_FILE_BYTES {
            return Err(TarError::Corrupt("entry size beyond the ustar range"));
        }
        self.remaining = size;
        self.padding = (BLOCK - (size % BLOCK as u64) as usize) % BLOCK;
        Ok(Some((name, size)))
    }

    /// Discard the current entry's unread data.
    fn skip_current(&mut self) -> Result<(), TarError> {
        let mut scratch = [0u8; 16 * 1024];
        while self.remaining > 0 {
            let want = scratch.len().min(usize::try_from(self.remaining).unwrap_or(scratch.len()));
            self.src
                .read_exact(&mut scratch[..want])
                .map_err(|_| TarError::Corrupt("entry data truncated"))?;
            self.remaining -= want as u64;
        }
        Ok(())
    }
}

/// Reading a [`TarReader`] yields the current entry's bytes, ending
/// cleanly at the entry boundary; reads never cross into the next header.
impl<R: Read> Read for TarReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.remaining == 0 {
            return Ok(0);
        }
        let want = buf.len().min(usize::try_from(self.remaining).unwrap_or(buf.len()));
        let n = self.src.read(&mut buf[..want])?;
        self.remaining -= n as u64;
        Ok(n)
    }
}

/// One block, fully present or the archive is corrupt.
fn read_block<R: Read>(src: &mut R, into: &mut [u8; BLOCK]) -> Result<(), TarError> {
    src.read_exact(into)
        .map_err(|_| TarError::Corrupt("archive ends inside a header"))
}

/// Allow zero padding after the two end blocks.
fn require_zero_tail<R: Read>(src: &mut R) -> Result<(), TarError> {
    let mut buf = [0u8; 16 * 1024];
    loop {
        let n = src.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        if buf[..n].iter().any(|&b| b != 0) {
            return Err(TarError::Corrupt("data after the end-of-archive marker"));
        }
    }
}

/// A ustar octal field: leading octal digits until the first non-octal
/// byte (the NUL or space terminator). `None` when no digits or overflow.
fn parse_octal(field: &[u8]) -> Option<u64> {
    let mut v: u64 = 0;
    let mut digits = 0;
    for &b in field {
        if !(b'0'..=b'7').contains(&b) {
            break;
        }
        v = v.checked_mul(8)?.checked_add(u64::from(b - b'0'))?;
        digits += 1;
    }
    if digits == 0 || field[digits..].iter().any(|&b| b != 0 && b != b' ') {
        None
    } else {
        Some(v)
    }
}

/// Lowercase hex, used for every digest the crate writes or compares.
pub(crate) fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn round_trip(entries: Vec<(&'static str, Vec<u8>)>) -> Vec<(String, u64, Vec<u8>)> {
        let mut buf = Vec::new();
        {
            let mut w = TarWriter::new(&mut buf);
            for (name, data) in &entries {
                let mut src = Cursor::new(data.clone());
                w.append(name, data.len() as u64, &mut src).unwrap();
            }
            w.finish().unwrap();
        }
        let mut r = TarReader::new(Cursor::new(buf));
        let mut out = Vec::new();
        while let Some((name, size)) = r.next().unwrap() {
            let mut data = Vec::new();
            r.read_to_end(&mut data).unwrap();
            assert_eq!(r.remaining(), 0);
            out.push((name, size, data));
        }
        out
    }

    #[test]
    fn entries_round_trip_across_block_boundaries() {
        let entries = vec![
            ("default.xbe", b"XBEH".to_vec()),
            ("maps/a.map", vec![]),
            ("maps/b.map", vec![7u8; 513]),
            ("maps/c.map", vec![9u8; BLOCK - 1]),
        ];
        let out = round_trip(entries.clone());
        assert_eq!(out.len(), entries.len());
        for ((name, data), (rname, rsize, rdata)) in entries.iter().zip(&out) {
            assert_eq!(name, rname);
            assert_eq!(*rsize, data.len() as u64);
            assert_eq!(&rdata, data);
        }
    }

    #[test]
    fn append_returns_the_sha256_of_the_file_bytes() {
        let data = b"payload-bytes".to_vec();
        let mut w = TarWriter::new(Vec::new());
        let digest = w.append("maps/x.map", data.len() as u64, &mut Cursor::new(data.clone())).unwrap();
        w.finish().unwrap();
        let want = hex(&Sha256::digest(&data));
        assert_eq!(digest, want);
    }

    #[test]
    fn a_short_source_is_an_error() {
        let mut w = TarWriter::new(Vec::new());
        let err = w.append("maps/x.map", 100, &mut Cursor::new(vec![0u8; 10])).unwrap_err();
        assert!(err.to_string().contains("unexpected end of file"), "{err}");
    }

    #[test]
    fn the_writer_refuses_unsafe_names() {
        for bad in ["../evil", "/etc/passwd", "maps/../../x", "a\\b", "a//b", "", "."] {
            let mut w = TarWriter::new(Vec::new());
            assert!(w.append(bad, 0, &mut Cursor::new(vec![])).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_reader_rejects_a_tampered_checksum() {
        let mut buf = Vec::new();
        {
            let mut w = TarWriter::new(&mut buf);
            w.append("maps/a.map", 3, &mut Cursor::new(vec![1, 2, 3])).unwrap();
            w.finish().unwrap();
        }
        buf[0] ^= 0x20; // a byte of the name, after the checksum was fixed
        let mut r = TarReader::new(Cursor::new(buf));
        assert!(matches!(r.next(), Err(TarError::Corrupt("header checksum mismatch"))));
    }

    #[test]
    fn the_reader_rejects_data_after_the_end_marker() {
        let mut buf = Vec::new();
        {
            let mut w = TarWriter::new(&mut buf);
            w.append("a", 0, &mut Cursor::new(vec![])).unwrap();
            w.finish().unwrap();
        }
        buf.extend_from_slice(b"trailing");
        let mut r = TarReader::new(Cursor::new(buf));
        assert!(r.next().unwrap().is_some());
        assert!(matches!(r.next(), Err(TarError::Corrupt(_))));
    }

    #[test]
    fn the_reader_rejects_a_truncated_archive() {
        let mut buf = Vec::new();
        {
            let mut w = TarWriter::new(&mut buf);
            w.append("a", 10, &mut Cursor::new(vec![0u8; 10])).unwrap();
            w.finish().unwrap();
        }
        buf.truncate(buf.len() - BLOCK); // one end block short
        let mut r = TarReader::new(Cursor::new(buf));
        assert!(r.next().unwrap().is_some());
        assert!(matches!(r.next(), Err(TarError::Corrupt(_))));
    }

    /// Hand-built headers for entries our writer refuses to make; the
    /// reader must refuse them too.
    fn raw_entry(name: &str, typeflag: u8, data: &[u8]) -> Vec<u8> {
        let mut h = [0u8; BLOCK];
        let n = name.len().min(99);
        h[..n].copy_from_slice(&name.as_bytes()[..n]);
        h[124..136].copy_from_slice(format!("{:011o}\0", data.len()).as_bytes());
        h[156] = typeflag;
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00");
        let sum: u32 = h.iter().map(|&b| u32::from(b)).sum::<u32>() + 8 * u32::from(b' ');
        h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        let mut out = h.to_vec();
        out.extend_from_slice(data);
        let pad = (BLOCK - data.len() % BLOCK) % BLOCK;
        out.extend(std::iter::repeat_n(0u8, pad));
        out
    }

    fn archive_of(entries: &[Vec<u8>]) -> Vec<u8> {
        let mut buf = entries.concat();
        buf.extend_from_slice(&[0u8; BLOCK * 2]);
        buf
    }

    #[test]
    fn the_reader_rejects_traversal_and_absolute_names() {
        for name in ["../evil", "/etc/passwd", "maps/../../escape"] {
            let buf = archive_of(&[raw_entry(name, b'0', b"x")]);
            let mut r = TarReader::new(Cursor::new(buf));
            let err = r.next().unwrap_err();
            assert!(matches!(err, TarError::UnsafePath(_)), "{name}: {err}");
        }
    }

    #[test]
    fn the_reader_rejects_non_regular_entries() {
        for typeflag in [b'2', b'5', b'1', b'x'] {
            // '2' symlink, '5' directory, '1' hard link,
            // 'x' pax extended header.
            let e = raw_entry("maps/evil", typeflag, b"");
            let buf = archive_of(&[e]);
            let mut r = TarReader::new(Cursor::new(buf));
            assert!(
                matches!(r.next(), Err(TarError::Corrupt(_))),
                "typeflag {}",
                typeflag as char
            );
        }
    }

    #[test]
    fn unread_entry_data_is_skipped_before_the_next_header() {
        let mut buf = Vec::new();
        {
            let mut w = TarWriter::new(&mut buf);
            w.append("a", 512, &mut Cursor::new(vec![1u8; 512])).unwrap();
            w.append("b", 4, &mut Cursor::new(vec![2, 2, 2, 2])).unwrap();
            w.finish().unwrap();
        }
        let mut r = TarReader::new(Cursor::new(buf));
        assert_eq!(r.next().unwrap().unwrap().0, "a");
        assert_eq!(r.next().unwrap().unwrap().0, "b"); // skipped a's data
        let mut data = Vec::new();
        r.read_to_end(&mut data).unwrap();
        assert_eq!(data, vec![2, 2, 2, 2]);
        assert!(r.next().unwrap().is_none());
    }

    #[test]
    fn reads_stop_at_the_entry_boundary() {
        let mut buf = Vec::new();
        {
            let mut w = TarWriter::new(&mut buf);
            w.append("a", 4, &mut Cursor::new(vec![1, 2, 3, 4])).unwrap();
            w.finish().unwrap();
        }
        let mut r = TarReader::new(Cursor::new(buf));
        r.next().unwrap();
        let mut big = [0u8; 4096];
        assert_eq!(r.read(&mut big).unwrap(), 4);
        assert_eq!(r.read(&mut big).unwrap(), 0);
    }
}
