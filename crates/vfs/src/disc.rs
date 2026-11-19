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
    mapping: Option<memmap2::Mmap>,
}
impl DiscBackend<File> {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_mapping(path, false)
    }

    pub(crate) fn open_with_mapping(path: impl AsRef<Path>, mmap: bool) -> Result<Self> {
        let file = File::open(path.as_ref())?;
        let mapping = if mmap { map_read_only(&file) } else { None };
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
            copy_mapped(map, start + offset, &mut buffer[..len])?;
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

/// Only the DVD input in the immutable-disc profile is mapped. The title
/// may replace or truncate its writable cache files and installed maps.
// The crate denies unsafe code; this read-only map is its one exception.
#[allow(unsafe_code)]
pub(crate) fn map_read_only(file: &File) -> Option<memmap2::Mmap> {
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    // SAFETY: The host-light input is an immutable disc image or immutable
    // DVD files. Active files must remain intact until the run ends.
    unsafe { memmap2::MmapOptions::new().map(file) }.ok()
}

pub(crate) fn copy_mapped(map: &[u8], offset: u64, buffer: &mut [u8]) -> Result<()> {
    let bytes = usize::try_from(offset).ok()
        .and_then(|start| map.get(start..)?.get(..buffer.len()))
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::UnexpectedEof))?;
    buffer.copy_from_slice(bytes);
    Ok(())
}
