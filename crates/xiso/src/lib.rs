//! Reader for the Xbox DVD filesystem (XDVDFS).
//!
//! Handles both "XISO" images (game partition only) and full Redump-style
//! images, where the game partition starts at a fixed offset after the
//! video partition.

use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom};

pub const SECTOR: u64 = 2048;
const MAGIC: &[u8; 20] = b"MICROSOFT*XBOX*MEDIA";
/// Volume descriptor lives at sector 32 of the game partition.
const VOLUME_DESCRIPTOR: u64 = 32 * SECTOR;
/// Known starts of the game partition: XISO, Redump XGD1, XGD2, XGD3.
pub const PARTITION_OFFSETS: [u64; 4] = [0, 0x1830_0000, 0x0FD9_0000, 0x0208_0000];

const ATTR_DIRECTORY: u8 = 0x10;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("no XDVDFS volume descriptor found")]
    NotXdvdfs,
    #[error("corrupt directory at sector {0}")]
    CorruptDirectory(u32),
    #[error("file not found: {0}")]
    NotFound(String),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Path from the root, `/`-separated, starting with `/`.
    pub path: String,
    pub sector: u32,
    pub size: u32,
    pub is_dir: bool,
}

pub struct Xiso<R> {
    reader: R,
    partition: u64,
    root_sector: u32,
    root_size: u32,
}

impl<R: Read + Seek> Xiso<R> {
    pub fn open(mut reader: R) -> Result<Self> {
        for partition in PARTITION_OFFSETS {
            let mut desc = [0u8; 28];
            if reader.seek(SeekFrom::Start(partition + VOLUME_DESCRIPTOR)).is_err()
                || reader.read_exact(&mut desc).is_err()
            {
                continue;
            }
            if &desc[..20] == MAGIC {
                let root_sector = u32::from_le_bytes(desc[20..24].try_into().unwrap());
                let root_size = u32::from_le_bytes(desc[24..28].try_into().unwrap());
                return Ok(Self { reader, partition, root_sector, root_size });
            }
        }
        Err(Error::NotXdvdfs)
    }

    /// Byte offset of the game partition within the image.
    pub fn partition_offset(&self) -> u64 {
        self.partition
    }

    /// Every file and directory, depth first, in on-disc tree order.
    pub fn entries(&mut self) -> Result<Vec<Entry>> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        self.walk(self.root_sector, self.root_size, "", &mut out, &mut seen)?;
        Ok(out)
    }

    pub fn find(&mut self, path: &str) -> Result<Entry> {
        let want = path.trim_start_matches('/');
        self.entries()?
            .into_iter()
            .find(|e| e.path[1..].eq_ignore_ascii_case(want))
            .ok_or_else(|| Error::NotFound(path.to_owned()))
    }

    pub fn read(&mut self, entry: &Entry) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; entry.size as usize];
        self.reader.seek(SeekFrom::Start(self.offset(entry.sector)))?;
        self.reader.read_exact(&mut buf)?;
        Ok(buf)
    }

    /// Stream a file to a writer without holding it all in memory.
    pub fn copy_to(&mut self, entry: &Entry, out: &mut impl std::io::Write) -> Result<u64> {
        self.reader.seek(SeekFrom::Start(self.offset(entry.sector)))?;
        Ok(std::io::copy(&mut (&mut self.reader).take(entry.size as u64), out)?)
    }

    fn offset(&self, sector: u32) -> u64 {
        self.partition + sector as u64 * SECTOR
    }

    fn walk(
        &mut self,
        sector: u32,
        size: u32,
        prefix: &str,
        out: &mut Vec<Entry>,
        seen: &mut HashSet<u32>,
    ) -> Result<()> {
        if size == 0 {
            return Ok(());
        }
        if !seen.insert(sector) {
            return Err(Error::CorruptDirectory(sector));
        }
        let mut table = vec![0u8; size as usize];
        self.reader.seek(SeekFrom::Start(self.offset(sector)))?;
        self.reader.read_exact(&mut table)?;

        // The directory is a binary tree; offsets are in 4-byte units.
        let mut stack = vec![(0usize, false)];
        let mut visited = HashSet::new();
        while let Some((node, expanded)) = stack.pop() {
            let at = node * 4;
            let Some(raw) = table.get(at..at + 14) else {
                return Err(Error::CorruptDirectory(sector));
            };
            let left = u16::from_le_bytes([raw[0], raw[1]]);
            let right = u16::from_le_bytes([raw[2], raw[3]]);
            if left == 0xFFFF {
                continue; // sector padding
            }
            if !expanded {
                if !visited.insert(node) {
                    return Err(Error::CorruptDirectory(sector));
                }
                // In-order: left, self, right.
                if right != 0 {
                    stack.push((right as usize, false));
                }
                stack.push((node, true));
                if left != 0 {
                    stack.push((left as usize, false));
                }
                continue;
            }
            let start = u32::from_le_bytes(raw[4..8].try_into().unwrap());
            let len = u32::from_le_bytes(raw[8..12].try_into().unwrap());
            let attr = raw[12];
            let name_len = raw[13] as usize;
            if name_len == 0 {
                // extract-xiso writes an empty directory as one zero-filled
                // sector: a single record with no name. It names nothing.
                continue;
            }
            let name = table
                .get(at + 14..at + 14 + name_len)
                .ok_or(Error::CorruptDirectory(sector))?;
            let path = format!("{prefix}/{}", String::from_utf8_lossy(name));
            let is_dir = attr & ATTR_DIRECTORY != 0;
            out.push(Entry { path: path.clone(), sector: start, size: len, is_dir });
            if is_dir {
                self.walk(start, len, &path, out, seen)?;
            }
        }
        Ok(())
    }
}


/// Whether `path` (an [`Entry::path`], `/`-separated from the root) is
/// picked by `patterns`: a pattern names a file (`maps/shared.map`) or,
/// ending in `/`, everything under a directory (`movie/`). A directory is
/// picked when it is on the way to a picked file, so the subset keeps its
/// tree. Case does not matter, as on the disc. Used to cut a trimmed copy
/// of the disc (docs/runtime/tvos.md).
pub fn subset_matches(path: &str, is_dir: bool, patterns: &[&str]) -> bool {
    let path = path.trim_start_matches('/').to_ascii_lowercase();
    patterns.iter().any(|p| {
        let p = p.trim_start_matches('/').to_ascii_lowercase();
        if let Some(dir) = p.strip_suffix('/') {
            // Inside the directory, the directory itself, or a parent of it.
            path.starts_with(&p) || path == dir || (is_dir && p.starts_with(&format!("{path}/")))
        } else {
            path == p || (is_dir && p.starts_with(&format!("{path}/")))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn dirent(left: u16, right: u16, sector: u32, size: u32, attr: u8, name: &str) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend(left.to_le_bytes());
        v.extend(right.to_le_bytes());
        v.extend(sector.to_le_bytes());
        v.extend(size.to_le_bytes());
        v.push(attr);
        v.push(name.len() as u8);
        v.extend(name.as_bytes());
        while v.len() % 4 != 0 {
            v.push(0xFF);
        }
        v
    }

    /// Image: root at sector 33 holding `b` (root node) with left child `a`
    /// and a directory `c` on the right holding `d`.
    fn image() -> Vec<u8> {
        let mut img = vec![0u8; 40 * SECTOR as usize];
        let vd = VOLUME_DESCRIPTOR as usize;
        img[vd..vd + 20].copy_from_slice(MAGIC);
        img[vd + 20..vd + 24].copy_from_slice(&33u32.to_le_bytes());
        img[vd + 24..vd + 28].copy_from_slice(&(SECTOR as u32).to_le_bytes());

        let b = dirent(0, 0, 0, 0, 0, "b"); // patched below
        let a = dirent(0, 0, 36, 3, 0, "a");
        let c = dirent(0, 0, 34, SECTOR as u32, ATTR_DIRECTORY, "c");
        let a_off = b.len() / 4;
        let c_off = (b.len() + a.len()) / 4;
        let b = dirent(a_off as u16, c_off as u16, 37, 2, 0, "b");
        let mut root: Vec<u8> = [b, a, c].concat();
        root.resize(SECTOR as usize, 0xFF);
        img[33 * 2048..34 * 2048].copy_from_slice(&root);

        let mut sub = dirent(0, 0, 38, 1, 0, "d");
        sub.resize(SECTOR as usize, 0xFF);
        img[34 * 2048..35 * 2048].copy_from_slice(&sub);
        img[36 * 2048..36 * 2048 + 3].copy_from_slice(b"aaa");
        img[37 * 2048..37 * 2048 + 2].copy_from_slice(b"bb");
        img[38 * 2048] = b'd';
        img
    }

    #[test]
    fn walks_tree_in_order() {
        let mut x = Xiso::open(Cursor::new(image())).unwrap();
        let paths: Vec<_> = x.entries().unwrap().into_iter().map(|e| e.path).collect();
        assert_eq!(paths, ["/a", "/b", "/c", "/c/d"]);
    }

    #[test]
    fn reads_files_case_insensitively() {
        let mut x = Xiso::open(Cursor::new(image())).unwrap();
        let e = x.find("/C/D").unwrap();
        assert_eq!(x.read(&e).unwrap(), b"d");
        let e = x.find("a").unwrap();
        assert_eq!(x.read(&e).unwrap(), b"aaa");
    }

    /// Presents `inner` as if it started `pad` zero bytes into the stream,
    /// so the Redump offset can be tested without a 400 MB buffer.
    struct Padded {
        pad: u64,
        inner: Cursor<Vec<u8>>,
        pos: u64,
    }

    impl Read for Padded {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.pos < self.pad {
                let n = buf.len().min((self.pad - self.pos) as usize);
                buf[..n].fill(0);
                self.pos += n as u64;
                return Ok(n);
            }
            self.inner.set_position(self.pos - self.pad);
            let n = self.inner.read(buf)?;
            self.pos += n as u64;
            Ok(n)
        }
    }

    impl Seek for Padded {
        fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
            let SeekFrom::Start(p) = to else { unimplemented!() };
            self.pos = p;
            Ok(p)
        }
    }

    #[test]
    fn finds_redump_partition() {
        let pad = 0x1830_0000;
        let mut x = Xiso::open(Padded { pad, inner: Cursor::new(image()), pos: 0 }).unwrap();
        assert_eq!(x.partition_offset(), pad);
        let e = x.find("/b").unwrap();
        assert_eq!(x.read(&e).unwrap(), b"bb");
    }

    /// extract-xiso writes an empty directory as a zero-filled sector. Its
    /// one record has no name and must not show up as an entry (it used to
    /// list as `/e/`, which `h2 extract` then refused as an unsafe path).
    #[test]
    fn zero_filled_directory_is_empty() {
        let mut img = image();
        let e = dirent(0, 0, 39, SECTOR as u32, ATTR_DIRECTORY, "e");
        // Hang `e` off `c` as its right child; sector 39 stays all zeros.
        let c_rec = 33 * 2048 + (dirent(0, 0, 0, 0, 0, "b").len() + dirent(0, 0, 0, 0, 0, "a").len());
        let e_off = 1500usize;
        img[c_rec + 2..c_rec + 4].copy_from_slice(&((e_off / 4) as u16).to_le_bytes());
        img[33 * 2048 + e_off..33 * 2048 + e_off + e.len()].copy_from_slice(&e);
        let mut x = Xiso::open(Cursor::new(img)).unwrap();
        let entries = x.entries().unwrap();
        let paths: Vec<_> = entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, ["/a", "/b", "/c", "/c/d", "/e"]);
        assert!(entries[4].is_dir);
    }

    #[test]
    fn rejects_non_xdvdfs() {
        assert!(matches!(Xiso::open(Cursor::new(vec![0u8; 0x20000])), Err(Error::NotXdvdfs)));
    }

    #[test]
    fn subset_picks_files_directories_and_their_parents() {
        let pats = ["default.xbe", "maps/shared.map", "fonts/", "Maps/MainMenu.map"];
        assert!(subset_matches("/default.xbe", false, &pats));
        assert!(subset_matches("/maps", true, &pats), "parent of a picked file");
        assert!(subset_matches("/maps/shared.map", false, &pats));
        assert!(subset_matches("/MAPS/mainmenu.map", false, &pats), "case-insensitive");
        assert!(!subset_matches("/maps/zanzibar.map", false, &pats));
        assert!(subset_matches("/fonts", true, &pats), "a picked directory");
        assert!(subset_matches("/fonts/x/y.bin", false, &pats), "under a picked directory");
        assert!(!subset_matches("/fontsx", true, &pats));
        assert!(!subset_matches("/movie", true, &pats));
        assert!(!subset_matches("/maps", false, &pats), "a file named like a parent dir");
    }
}
