//! [`Storage`] over the kernel's one shared `vfs::Vfs` (never a second
//! instance over the same roots).
//!
//! The overrides call it with the guest memory and XAPI guards held, so it
//! takes only the VFS lock (order: guest memory, XAPI, VFS). Capture the
//! kernel's VFS `Arc` once at setup with no guard held; never call into
//! kernel state from here.
//!
//! - Z: and N: are links to `\Device\Harddisk0\Partition<n>`. Mounting
//!   replaces an existing link of the same letter.
//! - "Format" is `Vfs::format_device`: the partition's host directories are
//!   emptied, and it fails while any file is open on that volume.
//! - Neither letter can be put on, or format, the volume the other one
//!   resolves to (through any alias, including links into a subdirectory),
//!   so N:'s format can never empty Z:'s cache (or the reverse).
//! - The cache database is the kernel's raw disk, `\Device\Harddisk0\Partition0`
//!   at byte 0x800, mounted into the same VFS by `Kernel::set_vfs`.
use crate::{Error, Result, Storage, storage::CACHE_DATABASE_SIZE};
use std::sync::{Arc, Mutex, MutexGuard};
use vfs::{Access, OpenOptions, ShareAccess, Vfs};

pub const RAW_DISK_DEVICE: &str = "\\Device\\Harddisk0\\Partition0";
pub const CACHE_DATABASE_OFFSET: u64 = 0x800;
pub const UTILITY_LINK: &str = "\\??\\Z:";
pub const CACHE_LINK: &str = "\\??\\N:";

/// ERROR_SHARING_VIOLATION: the other letter already uses the volume.
const ERROR_SHARING_VIOLATION: u32 = 32;

pub fn partition_device(partition: u8) -> String {
    format!("\\Device\\Harddisk0\\Partition{partition}")
}

/// Win32 code for a VFS status, as the kernel's RtlNtStatusToDosError
/// (`crates/runtime/kernel/src/dos.rs`) maps it.
pub fn win32(error: vfs::Error) -> Error {
    use vfs::Error::*;
    Error::Win32(match error {
        InvalidHandle => 6,
        InvalidParameter => 87,
        NoSuchFile | ObjectNameNotFound => 2,
        EndOfFile => 38,
        AccessDenied | DeletePending | FileIsADirectory => 5,
        ObjectNameInvalid => 123,
        ObjectNameCollision => 183,
        ObjectPathNotFound => 3,
        SharingViolation => 32,
        DiskFull => 112,
        MediaWriteProtected => 19,
        NotSupported => 50,
        DirectoryNotEmpty => 145,
        NotADirectory => 267,
        ReparsePointNotResolved => 1921,
        FileTooLarge => 223,
        FileCorrupt => 1392,
        IoDeviceError => 1117,
        // STATUS_DEVICE_DATA_ERROR: a sim store read outside its ranges.
        NotInStore => 23,
        NoMoreFiles => 18,
    })
}

pub struct VfsStorage {
    vfs: Arc<Mutex<Vfs>>,
}

impl VfsStorage {
    pub fn new(vfs: Arc<Mutex<Vfs>>) -> Self {
        Self { vfs }
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, Vfs> {
        self.vfs.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// True if `link` resolves, through every alias and including links into
    /// a subdirectory, onto the mounted volume `device`. A letter that does
    /// not resolve uses no volume.
    fn uses(vfs: &Vfs, link: &str, device: &str) -> bool {
        vfs.device_of(link)
            .is_ok_and(|mounted| mounted.eq_ignore_ascii_case(device))
    }

    /// Format (optionally) and link `link` to `partition`, refusing the
    /// volume `other` names.
    fn mount(&mut self, link: &str, other: &str, partition: u8, format: bool) -> Result<()> {
        if partition < 3 {
            return Err(Error::INVALID_PARAMETER);
        }
        let device = partition_device(partition);
        let mut vfs = self.lock();
        // The device must exist before anything changes.
        vfs.query_path(&device).map_err(win32)?;
        if Self::uses(&vfs, other, &device) {
            return Err(Error::Win32(ERROR_SHARING_VIOLATION));
        }
        if format {
            vfs.format_device(&device).map_err(win32)?;
        }
        match vfs.namespace.delete_symbolic_link(link) {
            Ok(()) | Err(vfs::Error::ObjectNameNotFound) => {}
            Err(e) => return Err(win32(e)),
        }
        vfs.namespace
            .create_symbolic_link(link, &device)
            .map_err(win32)
    }

    fn database(
        &mut self,
        write: Option<&[u8; CACHE_DATABASE_SIZE]>,
    ) -> Result<[u8; CACHE_DATABASE_SIZE]> {
        let mut vfs = self.lock();
        let access = if write.is_some() {
            Access::READ | Access::WRITE
        } else {
            Access::READ
        };
        let handle = vfs
            .open(
                RAW_DISK_DEVICE,
                OpenOptions {
                    access,
                    share: ShareAccess::READ | ShareAccess::WRITE,
                    directory: Some(false),
                    ..OpenOptions::default()
                },
            )
            .map_err(win32)?
            .handle;
        let offset = vfs::Offset::Absolute(CACHE_DATABASE_OFFSET);
        let mut sector = write.copied().unwrap_or([0; CACHE_DATABASE_SIZE]);
        let result = match write {
            Some(bytes) => vfs.write(handle, offset, bytes),
            None => vfs.read(handle, offset, &mut sector),
        };
        let closed = vfs.close(handle);
        let n = result.map_err(win32)?;
        closed.map_err(win32)?;
        if n != CACHE_DATABASE_SIZE {
            return Err(win32(vfs::Error::EndOfFile));
        }
        Ok(sector)
    }
}

impl Storage for VfsStorage {
    fn mount_utility(&mut self, partition: u8, format: bool) -> Result<()> {
        if !(3..=5).contains(&partition) {
            return Err(Error::INVALID_PARAMETER);
        }
        self.mount(UTILITY_LINK, CACHE_LINK, partition, format)
    }

    fn format_utility(&mut self) -> Result<()> {
        let mut vfs = self.lock();
        // The volume Z: really resolves to, not its immediate link target.
        let device = vfs
            .device_of(UTILITY_LINK)
            .map_err(|_| Error::Win32(3))?
            .to_owned();
        if Self::uses(&vfs, CACHE_LINK, &device) {
            return Err(Error::Win32(ERROR_SHARING_VIOLATION));
        }
        vfs.format_device(&device).map_err(win32)
    }

    fn read_cache_database(&mut self) -> Result<[u8; CACHE_DATABASE_SIZE]> {
        self.database(None)
    }

    fn write_cache_database(&mut self, sector: &[u8; CACHE_DATABASE_SIZE]) -> Result<()> {
        self.database(Some(sector)).map(drop)
    }

    fn mount_cache(&mut self, partition: u8) -> Result<()> {
        self.mount(CACHE_LINK, UTILITY_LINK, partition, true)
    }

    fn saves(&mut self) -> Option<&mut dyn crate::SaveStore> {
        Some(self)
    }
}
