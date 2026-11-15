//! Utility-volume interface over the same VFS instance used by the kernel.
//! Ordinary file, save, enumeration and volume queries remain generated.

use crate::Result;

pub trait Storage {
    /// Mount Z: to partition 3..5 (XMountUtilityDrive, 0x002d5b77).
    /// Preserve content unless format=true; then clear only this utility volume.
    /// Never erase U:/T: or translate guest input into an arbitrary host path.
    fn mount_utility(&mut self, partition: u8, format: bool) -> Result<()>;

    /// Format the currently mounted Z: (XFormatUtilityDrive, 0x002d5c7c).
    /// Fail if no utility volume is mounted. Errors must be Win32, not NTSTATUS.
    fn format_utility(&mut self) -> Result<()>;
}
