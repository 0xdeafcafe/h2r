//! The second cache volume, N:, mounted by `0x002d5d0a` (called from game
//! bootstrap at `0x00012003`, result ignored there). stdcall, no arguments,
//! plain `ret`, BOOL in EAX. Retail behaviour, from the instructions:
//!
//! 1. `[0x461d80]` (Z:'s row) == -1: XapiSetLastNTError(STATUS_UNSUCCESSFUL),
//!    return FALSE; N:'s row `[0x461d84]` is not written (`0x002d5d21`).
//! 2. Read the 512-byte cache database at raw-disk byte 0x800 (NtOpenFile 202,
//!    NtReadFile 219). Bad signature/version/marker: STATUS_INTERNAL_DB_CORRUPTION.
//! 3. Z:'s partition index is its row's index field. The chosen partition is
//!    the highest index below `min(HalDiskCachePartitionCount, 41)` that is
//!    not Z:'s and that no valid row holds (`0x002d5e01..0x002d5e2a`).
//! 4. N:'s row is `count - 1`, or `count - 2` if that is Z:'s row; it is
//!    stored at `0x002d5e38`. If nothing was free, the partition is the one
//!    that row names; otherwise the row is given the chosen index. The row's
//!    title becomes this title and its valid flag 0. The database is written
//!    back (NtWriteFile 236, status ignored).
//! 5. Format `\Device\Harddisk0\Partition<n>` (formatter `0x002d6c04`, which
//!    sets the last error itself on failure), verify its cluster size, and
//!    IoCreateSymbolicLink(`\??\N:`) at `0x002d5f2a`.
//! 6. Any failure from step 2 on: `[0x461d84] = -1`, last error set, FALSE.
//!
//! Native policy: [`Storage`] provides the database and the volume, "format"
//! empties the partition's host directories, and the cluster-size check is
//! skipped (the host volume reports 16 KiB clusters; retail formats N: with
//! the XBE's utility cluster size). Where retail would index outside the
//! table (a count below 2 or above 41, a Z: row outside it) or would pick
//! Z:'s own partition from a stale row, this fails with ERROR_GEN_FAILURE
//! instead. Nothing here writes Z:'s row or touches its volume.
use crate::{
    Error, GuestMemory, Result, Storage, Xapi, config::CachePartition, memory,
    storage::CACHE_DATABASE_SIZE,
};

pub const CACHE_DB_SIGNATURE: u32 = 0x9731_5286;
pub const CACHE_DB_VERSION: u32 = 2;
pub const CACHE_DB_MARKER_OFFSET: usize = 0x1f8;
pub const CACHE_DB_MARKER: u32 = 0xaa55_0000;
pub const CACHE_DB_ROWS: usize = 41;
/// Row index 0 is Partition3.
pub const FIRST_CACHE_PARTITION: u8 = 3;

/// RtlNtStatusToDosError(STATUS_UNSUCCESSFUL).
pub const ERROR_GEN_FAILURE: u32 = 31;
/// RtlNtStatusToDosError(STATUS_INTERNAL_DB_CORRUPTION).
pub const ERROR_INTERNAL_DB_CORRUPTION: u32 = 1358;

/// One 12-byte database row at 8 + 12 * i.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CacheRow {
    pub title_id: u32,
    /// Partition number minus 3.
    pub index: u32,
    pub valid: u32,
}

fn word(sector: &[u8; CACHE_DATABASE_SIZE], at: usize) -> u32 {
    u32::from_le_bytes(sector[at..at + 4].try_into().expect("4 bytes"))
}

/// Row `i` of a database sector.
pub fn row(sector: &[u8; CACHE_DATABASE_SIZE], i: usize) -> CacheRow {
    let at = 8 + 12 * i;
    CacheRow {
        title_id: word(sector, at),
        index: word(sector, at + 4),
        valid: word(sector, at + 8),
    }
}

fn set_row(sector: &mut [u8; CACHE_DATABASE_SIZE], i: usize, r: CacheRow) {
    let at = 8 + 12 * i;
    sector[at..at + 4].copy_from_slice(&r.title_id.to_le_bytes());
    sector[at + 4..at + 8].copy_from_slice(&r.index.to_le_bytes());
    sector[at + 8..at + 12].copy_from_slice(&r.valid.to_le_bytes());
}

/// The checks at `0x002d5dc0..0x002d5de4`.
pub fn database_is_valid(sector: &[u8; CACHE_DATABASE_SIZE]) -> bool {
    word(sector, 0) == CACHE_DB_SIGNATURE
        && word(sector, 4) == CACHE_DB_VERSION
        && word(sector, CACHE_DB_MARKER_OFFSET) == CACHE_DB_MARKER
}

/// What [`Xapi::mount_cache_drive`] decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CacheSelection {
    /// N:'s database row, published at `CacheGlobals::cache_row`.
    pub row: u32,
    /// Partition number backing N:.
    pub partition: u8,
}

/// Steps 3 and 4 over a valid sector: N:'s row and partition, and the row
/// update. Pure, so the selection rule is testable without a guest.
pub fn select(
    sector: &mut [u8; CACHE_DATABASE_SIZE],
    count: u32,
    utility_row: u32,
    title_id: u32,
    policy: CachePartition,
    utility_partition: u8,
) -> Result<CacheSelection> {
    let fail = Error::Win32(ERROR_GEN_FAILURE);
    let rows = (count as usize).min(CACHE_DB_ROWS);
    let z = utility_row as usize;
    if z >= rows {
        return Err(fail);
    }
    let mut n = count.checked_sub(1).ok_or(fail)?;
    if n == utility_row {
        n = n.checked_sub(1).ok_or(fail)?;
    }
    if n as usize >= rows {
        return Err(fail);
    }
    let z_index = row(sector, z).index;
    let index = match policy {
        CachePartition::Fixed(partition) => u32::from(
            partition
                .checked_sub(FIRST_CACHE_PARTITION)
                .ok_or(Error::INVALID_PARAMETER)?,
        ),
        CachePartition::Select => (0..rows as u32)
            .rev()
            .find(|&p| {
                p != z_index
                    && !(0..rows).any(|i| {
                        let r = row(sector, i);
                        r.valid != 0 && r.index == p
                    })
            })
            .unwrap_or(row(sector, n as usize).index),
    };
    let partition = u32::from(FIRST_CACHE_PARTITION) + index;
    if index == z_index || index as usize >= rows || partition == u32::from(utility_partition) {
        return Err(fail);
    }
    set_row(
        sector,
        n as usize,
        CacheRow {
            title_id,
            index,
            valid: 0,
        },
    );
    Ok(CacheSelection {
        row: n,
        partition: partition as u8,
    })
}

impl<StorageBackend: Storage> Xapi<StorageBackend> {
    /// `0x002d5d0a`: mount the second cache volume, N:, and publish its row.
    /// Win32 errors are the guest's FALSE + last error; guest memory faults
    /// are returned as such, with N:'s row unchanged.
    pub fn mount_cache_drive(&mut self, memory: &mut impl GuestMemory) -> Result<CacheSelection> {
        let globals = self.config.cache_drive.globals;
        memory.check(globals.cache_row, 4, true)?;
        let utility_row = memory::u32_at(memory, globals.utility_row)?;
        if utility_row == u32::MAX {
            return Err(Error::Win32(ERROR_GEN_FAILURE));
        }
        let certificate = memory::u32_at(memory, globals.certificate_pointer)?;
        let title_id = memory::u32_at(memory, memory::offset(certificate, 8)?)?;
        let count_address = memory::u32_at(memory, globals.partition_count_import)?;
        let count = memory::u32_at(memory, count_address)?;
        let result = self.select_and_mount(utility_row, title_id, count);
        let published = match result {
            Ok(selection) => selection.row,
            Err(Error::Win32(_)) => u32::MAX,
            Err(fault) => return Err(fault),
        };
        memory.write(globals.cache_row, &published.to_le_bytes())?;
        result
    }

    fn select_and_mount(
        &mut self,
        utility_row: u32,
        title_id: u32,
        count: u32,
    ) -> Result<CacheSelection> {
        let mut sector = self.storage.read_cache_database()?;
        if !database_is_valid(&sector) {
            return Err(Error::Win32(ERROR_INTERNAL_DB_CORRUPTION));
        }
        let selection = select(
            &mut sector,
            count,
            utility_row,
            title_id,
            self.config.cache_drive.partition,
            self.config.utility_partition,
        )?;
        // Retail ignores the NtWriteFile status (0x002d5e96).
        let _ = self.storage.write_cache_database(&sector);
        self.storage.mount_cache(selection.partition)?;
        Ok(selection)
    }
}
