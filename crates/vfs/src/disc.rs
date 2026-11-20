use crate::{
    ATTR_DIRECTORY, ATTR_READONLY, BasicInfo, DirectoryEntry, Error, HostFile, Metadata, Result,
    Timestamp, VolumeInfo, backend::Backend, path::components,
};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

/// Indexes only XDVDFS directory records. Reads seek into the original image; no extraction or full-file buffering.
pub struct DiscBackend<R> {
    reader: R,
    partition_offset: u64,
    entries: BTreeMap<String, xiso::Entry>,
    volume_bytes: u64,
    /// The image file, when opened by path ([`Backend::host_file`]).
    image: Option<std::path::PathBuf>,
    mapping: Option<MappedDisc>,
}
impl DiscBackend<File> {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_mapping(path, false)
    }

    pub(crate) fn open_with_mapping(path: impl AsRef<Path>, mmap: bool) -> Result<Self> {
        let file = File::open(path.as_ref())?;
        let mapping = if mmap { MappedDisc::new(&file) } else { None };
        let mut disc = Self::new(file)?;
        disc.mapping = mapping;
        disc.image = Some(path.as_ref().to_path_buf());
        Ok(disc)
    }
}
impl<R: Read + Seek> DiscBackend<R> {
    pub fn new(mut reader: R) -> Result<Self> {
        let mut iso = xiso::Xiso::open(&mut reader)?;
        let partition_offset = iso.partition_offset();
        let mut entries = BTreeMap::new();
        let mut volume_bytes = 0;
        for entry in iso.entries()? {
            let path = entry.path.trim_start_matches('/').replace('/', "\\");
            if components(&path).is_err() {
                continue;
            }
            if let Some((parent, _)) = path.rsplit_once('\\')
                && !entries
                    .get(&parent.to_ascii_lowercase())
                    .is_some_and(|e: &xiso::Entry| e.is_dir)
            {
                continue;
            }
            volume_bytes = volume_bytes.max(entry.sector as u64 * xiso::SECTOR + entry.size as u64);
            if entries.insert(path.to_ascii_lowercase(), entry).is_some() {
                continue;
            }
        }
        Ok(Self {
            reader,
            partition_offset,
            entries,
            volume_bytes,
            image: None,
            mapping: None,
        })
    }
    fn entry(&self, path: &str) -> Result<&xiso::Entry> {
        components(path)?;
        if let Some(entry) = self.entries.get(&path.to_ascii_lowercase()) {
            return Ok(entry);
        }
        if let Some((parent, _)) = path.rsplit_once('\\') {
            match self.entries.get(&parent.to_ascii_lowercase()) {
                Some(entry) if !entry.is_dir => return Err(Error::ObjectPathNotFound),
                None => return Err(Error::ObjectPathNotFound),
                _ => {}
            }
        }
        Err(Error::ObjectNameNotFound)
    }
    fn metadata(entry: Option<&xiso::Entry>) -> Metadata {
        let directory = entry.is_none_or(|e| e.is_dir);
        let size = if directory {
            0
        } else {
            entry.unwrap().size as u64
        };
        Metadata {
            basic: BasicInfo {
                attributes: ATTR_READONLY | if directory { ATTR_DIRECTORY } else { 0 },
                // The existing xiso reader exposes no timestamps. Zero means unavailable.
                creation_time: Timestamp(0),
                last_access_time: Timestamp(0),
                last_write_time: Timestamp(0),
                change_time: Timestamp(0),
            },
            end_of_file: size,
            allocation_size: size.div_ceil(xiso::SECTOR) * xiso::SECTOR,
        }
    }
}
impl<R: Read + Seek + Send> Backend for DiscBackend<R> {
    fn read_only(&self) -> bool {
        true
    }
    fn host_file(&mut self, path: &str) -> Option<HostFile> {
        let image = self.image.clone()?;
        let entry = self.entry(path).ok().filter(|e| !e.is_dir)?;
        Some(HostFile {
            path: image,
            offset: self.partition_offset + entry.sector as u64 * xiso::SECTOR,
            len: entry.size as u64,
        })
    }
    fn stat(&mut self, path: &str) -> Result<Metadata> {
        Ok(Self::metadata(if path.is_empty() {
            None
        } else {
            Some(self.entry(path)?)
        }))
    }
    fn read_directory(&mut self, path: &str) -> Result<Vec<DirectoryEntry>> {
        if !self.stat(path)?.is_directory() {
            return Err(Error::InvalidParameter);
        }
        let base = if path.is_empty() {
            String::new()
        } else {
            format!("{}\\", path.to_ascii_lowercase())
        };
        Ok(self
            .entries
            .iter()
            .filter_map(|(key, entry)| {
                let rest = key.strip_prefix(&base)?;
                if rest.contains('\\') {
                    return None;
                }
                Some(DirectoryEntry {
                    name: entry.path.rsplit('/').next().unwrap().to_owned(),
                    metadata: Self::metadata(Some(entry)),
                })
            })
            .collect())
    }
    fn read_at(&mut self, path: &str, offset: u64, buffer: &mut [u8]) -> Result<usize> {
        if path.is_empty() {
            return Err(Error::FileIsADirectory);
        }
        let entry = self.entry(path)?;
        if entry.is_dir {
            return Err(Error::FileIsADirectory);
        }
        let len = (entry.size as u64)
            .saturating_sub(offset)
            .min(buffer.len() as u64) as usize;
        let start = self.partition_offset + entry.sector as u64 * xiso::SECTOR;
        if len == 0 {
            return Ok(0);
        }
        if let Some(map) = &self.mapping {
            map.read(start + offset, &mut buffer[..len])?;
        } else {
            self.reader.seek(SeekFrom::Start(start + offset))?;
            self.reader.read_exact(&mut buffer[..len])?;
        }
        Ok(len)
    }
    fn volume_info(&mut self) -> Result<VolumeInfo> {
        Ok(VolumeInfo {
            label: "Xbox DVD".into(),
            filesystem: "XDVDFS",
            serial: 0,
            read_only: true,
            bytes_per_sector: 2048,
            sectors_per_allocation_unit: 1,
            total_allocation_units: self.volume_bytes.div_ceil(2048),
            available_allocation_units: 0,
            maximum_component_name_length: 255,
        })
    }
}

/// Map an immutable host input (a disc, a verified map or a sim store).
#[allow(unsafe_code)]
pub(crate) fn map_immutable(file: &File) -> std::io::Result<memmap2::Mmap> {
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other("expected a regular file"));
    }
    // SAFETY: these inputs remain intact for the mapping's lifetime.
    // The store builder replaces files by rename, including on a rebuild.
    unsafe { memmap2::MmapOptions::new().map(file) }
}

pub(crate) fn map_read_only(file: &File) -> Option<memmap2::Mmap> {
    map_immutable(file).ok()
}

/// A whole-file read-only mapping whose bulk reads drop their pages after
/// the copy out: every byte the title takes from the disc lands in guest
/// RAM or in its cache-partition copy, so the mapped pages need not stay
/// resident (a container host is charged for them; docs/live/dedicated-host.md,
/// "Light host"). Small reads keep their pages.
pub(crate) struct MappedDisc {
    map: memmap2::Mmap,
    /// A descriptor of the mapped file, for Linux page-cache advice.
    #[cfg(target_os = "linux")]
    file: Option<File>,
}

/// Bulk reads drop their pages. The guest's file I/O moves at most one
/// kernel I/O chunk (64 KiB, `kernel::limits::IO_CHUNK`) per call, so every
/// whole-map transfer drops as it goes; the small metadata reads of boot
/// (headers, directory records) stay cached.
pub(crate) const BULK_READ: usize = 64 * 1024;

impl MappedDisc {
    pub(crate) fn new(file: &File) -> Option<Self> {
        map_read_only(file).map(|map| Self {
            map,
            #[cfg(target_os = "linux")]
            file: file.try_clone().ok(),
        })
    }

    /// Copy `[offset, offset + buffer.len())` out of the mapping, then drop
    /// those pages again for a bulk read. A later read faults them back in,
    /// so the bytes are the same either way.
    pub(crate) fn read(&self, offset: u64, buffer: &mut [u8]) -> Result<()> {
        copy_mapped(&self.map, offset, buffer)?;
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        if buffer.len() >= BULK_READ
            && let Some((start, len)) =
                page_span(offset, buffer.len() as u64, rustix::param::page_size() as u64)
        {
            self.drop_mapped_pages(start, len);
        }
        Ok(())
    }
}

/// The whole pages covering `[offset, offset + len)`: `(start, len)`,
/// page-aligned and rounded outward, so a fault's readahead around the
/// bytes asked for is dropped with them. `None` when the range is empty.
fn page_span(offset: u64, len: u64, page: u64) -> Option<(u64, u64)> {
    if len == 0 {
        return None;
    }
    let start = offset / page * page;
    let end = (offset + len).div_ceil(page) * page;
    Some((start, end - start))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
impl MappedDisc {
    #[allow(unsafe_code)]
    fn drop_mapped_pages(&self, start: u64, len: u64) {
        // The last page may extend beyond EOF. Give the kernel only the
        // mapped bytes and let it round the length to whole pages.
        let Some(bytes) = self.map.get(start as usize..) else {
            return;
        };
        let len = bytes.len().min(len as usize);
        let at = bytes.as_ptr().cast_mut().cast();
        // SAFETY: the range starts on a host page boundary inside our
        // immutable mapping. Discarding clean pages preserves their bytes.
        #[cfg(target_os = "macos")]
        unsafe {
            libc::msync(at, len, libc::MS_KILLPAGES);
        }
        #[cfg(target_os = "linux")]
        unsafe {
            libc::madvise(at, len, libc::MADV_DONTNEED);
            if let Some(file) = &self.file {
                use std::os::fd::AsRawFd;
                libc::posix_fadvise(
                    file.as_raw_fd(),
                    start as libc::off_t,
                    len as libc::off_t,
                    libc::POSIX_FADV_DONTNEED,
                );
            }
        }
    }
}

pub(crate) fn copy_mapped(map: &[u8], offset: u64, buffer: &mut [u8]) -> Result<()> {
    let bytes = usize::try_from(offset)
        .ok()
        .and_then(|start| map.get(start..)?.get(..buffer.len()))
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::UnexpectedEof))?;
    buffer.copy_from_slice(bytes);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bulk_read_through_the_mapping_reads_the_same_bytes_twice() {
        // A file large enough for one bulk read and some tail: the second
        // read must see the first's bytes after its pages were dropped.
        let dir = std::env::temp_dir().join(format!("h2-mapped-disc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("data.bin");
        let bytes: Vec<u8> = (0..BULK_READ + 8192).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &bytes).unwrap();
        let file = File::open(&path).unwrap();
        let map = MappedDisc::new(&file).expect("a regular file maps");
        let mut first = vec![0u8; BULK_READ];
        let mut second = vec![0u8; BULK_READ];
        map.read(3, &mut first).unwrap();
        map.read(3, &mut second).unwrap();
        assert_eq!(first, &bytes[3..3 + BULK_READ]);
        assert_eq!(second, first);
        let mut tail = [0u8; 8];
        map.read((BULK_READ) as u64, &mut tail).unwrap();
        assert_eq!(&tail[..], &bytes[BULK_READ..BULK_READ + tail.len()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn page_spans_round_outward_and_skip_empty_ranges() {
        let page = 16384u64;
        assert_eq!(page_span(0, 1, page), Some((0, page)));
        assert_eq!(page_span(1, page - 1, page), Some((0, page)));
        assert_eq!(page_span(1, page, page), Some((0, 2 * page)));
        assert_eq!(page_span(page, page, page), Some((page, page)));
        assert_eq!(page_span(5, 0, page), None);
    }
}
