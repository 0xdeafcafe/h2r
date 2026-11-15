//! Utility-volume interface over the same VFS instance used by the kernel.
//! Ordinary file, save, enumeration and volume queries remain generated.
//! [`crate::VfsStorage`] implements all of it over a shared `vfs::Vfs`.

use crate::{Error, Result};

/// Size of the utility-cache database sector at raw-disk byte 0x800.
pub const CACHE_DATABASE_SIZE: usize = 512;

/// Errors must be Win32 codes, not NTSTATUS.
pub trait Storage {
    /// Mount Z: to partition 3..5 (XMountUtilityDrive, 0x002d5b77).
    /// Preserve content unless format=true; then clear only this utility volume.
    /// Never erase U:/T: or translate guest input into an arbitrary host path.
    fn mount_utility(&mut self, partition: u8, format: bool) -> Result<()>;

    /// Format the currently mounted Z: (XFormatUtilityDrive, 0x002d5c7c).
    /// Fail if no utility volume is mounted.
    fn format_utility(&mut self) -> Result<()>;

    /// Read the cache database: 512 bytes at byte 0x800 of
    /// `\Device\Harddisk0\Partition0` (the kernel's raw disk). The default
    /// has no raw disk and fails, so N: is not mounted.
    fn read_cache_database(&mut self) -> Result<[u8; CACHE_DATABASE_SIZE]> {
        Err(Error::NOT_SUPPORTED)
    }

    /// Write the cache database back. `0x002d5d0a` ignores the result.
    fn write_cache_database(&mut self, _sector: &[u8; CACHE_DATABASE_SIZE]) -> Result<()> {
        Err(Error::NOT_SUPPORTED)
    }

    /// Empty cache partition `partition` (3..) and link `\??\N:` to it
    /// (`0x002d5d0a`). Must refuse the volume Z: is on, and must not touch
    /// any other volume.
    fn mount_cache(&mut self, _partition: u8) -> Result<()> {
        Err(Error::NOT_SUPPORTED)
    }
}
