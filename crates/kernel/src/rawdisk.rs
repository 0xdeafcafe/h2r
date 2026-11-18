//! `\Device\Harddisk0\Partition0`: the whole disk as one raw file.
//!
//! The kernel's disk driver owns this device object: `Kernel::set_vfs` mounts
//! it into the attached VFS unless the VFS already provides it. VFS partitions
//! are host directories, not FATX images, so the raw device is synthetic. It is zero except for the sectors XAPI reads:
//! the utility-cache database at byte 0x800, which `XapiSelectCachePartition`
//! (retail 0x2d592c) reads and rewrites with 512-byte NtReadFile/NtWriteFile.
//! Writes are kept for the session so a rewritten database reads back, up to
//! `limits::MAX_RAW_DISK_SECTORS` distinct sectors; a write that would retain
//! more fails with STATUS_DISK_FULL before any byte of it is stored. The
//! kernel keeps a handle on them ([`RawSectors`]) so a machine snapshot saves
//! and restores them (docs/runtime/snapshots.md).
//!
//! Cache database layout, derived from the code at 0x2d59af..0x2d5b5e:
//!
//! | Offset | Field |
//! | --- | --- |
//! | 0x000 | signature 0x97315286 |
//! | 0x004 | version 2 |
//! | 0x008 | up to 41 entries of 12 bytes: title ID, cache partition index (partition = index + 3), valid flag |
//! | 0x1f8 | marker 0xaa550000 |
//!
//! XAPI scans `min(HalDiskCachePartitionCount, 41)` entries for a valid entry
//! with its title ID; finding one selects that partition without formatting.
//! Otherwise it chooses an unused or least recently used partition and asks
//! for a format. A database failing the signature/version/marker checks is
//! treated as empty.
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};
use vfs::{Backend, BasicInfo, DirectoryEntry, Error, Metadata, Result, VolumeInfo};

pub const RAW_DISK_DEVICE: &str = "\\Device\\Harddisk0\\Partition0";
/// A 512 KiB config area, three 750 MiB caches, a 500 MiB system partition and
/// a 5 GiB data partition: the sizes VFS gives its host-backed partitions.
pub const DISK_SIZE: u64 = 0x8_0000 + 3 * 750 * 1024 * 1024 + 500 * 1024 * 1024 + (5 << 30);
pub const SECTOR_SIZE: u64 = 512;
pub const CACHE_DB_OFFSET: u64 = 0x800;
pub const CACHE_DB_SIGNATURE: u32 = 0x9731_5286;
pub const CACHE_DB_VERSION: u32 = 2;
pub const CACHE_DB_MARKER_OFFSET: usize = 0x1f8;
pub const CACHE_DB_MARKER: u32 = 0xaa55_0000;
pub const CACHE_DB_ENTRIES: usize = 41;
/// First cache partition: entry index 0 is Partition3.
pub const FIRST_CACHE_PARTITION: u8 = 3;

/// One title's cache-partition assignment in the database.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheEntry {
    pub title_id: u32,
    /// Partition number, 3..=5.
    pub partition: u8,
}

/// Build the 512-byte cache database sector, most recently used first.
pub fn cache_database(entries: &[CacheEntry]) -> Result<[u8; SECTOR_SIZE as usize]> {
    if entries.len() > CACHE_DB_ENTRIES
        || entries
            .iter()
            .any(|e| !(FIRST_CACHE_PARTITION..=5).contains(&e.partition))
    {
        return Err(Error::InvalidParameter);
    }
    let mut sector = [0; SECTOR_SIZE as usize];
    sector[0..4].copy_from_slice(&CACHE_DB_SIGNATURE.to_le_bytes());
    sector[4..8].copy_from_slice(&CACHE_DB_VERSION.to_le_bytes());
    for (i, entry) in entries.iter().enumerate() {
        let at = 8 + i * 12;
        sector[at..at + 4].copy_from_slice(&entry.title_id.to_le_bytes());
        let index = (entry.partition - FIRST_CACHE_PARTITION) as u32;
        sector[at + 4..at + 8].copy_from_slice(&index.to_le_bytes());
        sector[at + 8..at + 12].copy_from_slice(&1u32.to_le_bytes());
    }
    sector[CACHE_DB_MARKER_OFFSET..CACHE_DB_MARKER_OFFSET + 4]
        .copy_from_slice(&CACHE_DB_MARKER.to_le_bytes());
    Ok(sector)
}

/// The sectors a raw disk retains, by sector number.
pub type Sectors = BTreeMap<u64, [u8; SECTOR_SIZE as usize]>;

/// A raw disk's retained sectors, shared between the VFS backend and the
/// kernel. The lock is a leaf: nothing else is taken while it is held.
#[derive(Clone, Default)]
pub struct RawSectors(Arc<Mutex<Sectors>>);

impl RawSectors {
    pub fn lock(&self) -> MutexGuard<'_, Sectors> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The raw disk. Unwritten sectors outside the synthetic ones read as zero.
pub struct RawDisk {
    size: u64,
    sectors: RawSectors,
}
impl RawDisk {
    /// `size` is rounded down to whole sectors and must cover the database.
    pub fn new(size: u64, cache: &[CacheEntry]) -> Result<Self> {
        let size = size - size % SECTOR_SIZE;
        if size < CACHE_DB_OFFSET + SECTOR_SIZE {
            return Err(Error::InvalidParameter);
        }
        let mut sectors = BTreeMap::new();
        sectors.insert(CACHE_DB_OFFSET / SECTOR_SIZE, cache_database(cache)?);
        Ok(Self {
            size,
            sectors: RawSectors(Arc::new(Mutex::new(sectors))),
        })
    }
    /// A handle on the sectors, for machine snapshots.
    pub fn sectors(&self) -> RawSectors {
        self.sectors.clone()
    }
    fn check(path: &str) -> Result<()> {
        if path.is_empty() {
            Ok(())
        } else {
            Err(Error::ObjectNameNotFound)
        }
    }
}
impl Backend for RawDisk {
    fn read_only(&self) -> bool {
        false
    }
    fn stat(&mut self, path: &str) -> Result<Metadata> {
        Self::check(path)?;
        Ok(Metadata {
            basic: BasicInfo::default(),
            end_of_file: self.size,
            allocation_size: self.size,
        })
    }
    fn read_directory(&mut self, path: &str) -> Result<Vec<DirectoryEntry>> {
        Self::check(path)?;
        Err(Error::NotADirectory)
    }
    fn read_at(&mut self, path: &str, offset: u64, buffer: &mut [u8]) -> Result<usize> {
        Self::check(path)?;
        if offset >= self.size {
            return Ok(0);
        }
        let n = buffer.len().min((self.size - offset) as usize);
        let sectors = self.sectors.lock();
        let mut done = 0;
        while done < n {
            let at = offset + done as u64;
            let (sector, within) = (at / SECTOR_SIZE, (at % SECTOR_SIZE) as usize);
            let count = (n - done).min(SECTOR_SIZE as usize - within);
            let out = &mut buffer[done..done + count];
            match sectors.get(&sector) {
                Some(bytes) => out.copy_from_slice(&bytes[within..within + count]),
                None => out.fill(0),
            }
            done += count;
        }
        Ok(n)
    }
    fn write_at(&mut self, path: &str, offset: u64, buffer: &[u8]) -> Result<usize> {
        Self::check(path)?;
        let end = offset
            .checked_add(buffer.len() as u64)
            .ok_or(Error::InvalidParameter)?;
        if end > self.size {
            return Err(Error::DiskFull);
        }
        let mut sectors = self.sectors.lock();
        if !buffer.is_empty() {
            let (first, last) = (offset / SECTOR_SIZE, (end - 1) / SECTOR_SIZE);
            let span = last - first + 1;
            let free = crate::limits::MAX_RAW_DISK_SECTORS.saturating_sub(sectors.len()) as u64;
            // Count only when the span could exceed the remaining budget, so
            // ordinary small writes never walk the map.
            if span > free {
                let retained = sectors.range(first..=last).count() as u64;
                if span - retained > free {
                    return Err(Error::DiskFull);
                }
            }
        }
        let mut done = 0;
        while done < buffer.len() {
            let at = offset + done as u64;
            let (sector, within) = (at / SECTOR_SIZE, (at % SECTOR_SIZE) as usize);
            let count = (buffer.len() - done).min(SECTOR_SIZE as usize - within);
            sectors
                .entry(sector)
                .or_insert([0; SECTOR_SIZE as usize])[within..within + count]
                .copy_from_slice(&buffer[done..done + count]);
            done += count;
        }
        Ok(buffer.len())
    }
    fn volume_info(&mut self) -> Result<VolumeInfo> {
        Ok(VolumeInfo {
            label: String::new(),
            filesystem: "RAW",
            serial: 0,
            read_only: false,
            bytes_per_sector: SECTOR_SIZE as u32,
            sectors_per_allocation_unit: 1,
            total_allocation_units: self.size / SECTOR_SIZE,
            available_allocation_units: 0,
            maximum_component_name_length: 0,
        })
    }
}
