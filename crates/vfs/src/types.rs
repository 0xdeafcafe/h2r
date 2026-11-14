use crate::{Error, Result};
use std::time::{SystemTime, UNIX_EPOCH};

pub const FATX_MAX_NAME: usize = 42;
pub const FATX_MAX_PATH: usize = 250;
pub const FATX_MAX_FILE_SIZE: u64 = u32::MAX as u64;
pub const FATX_MAX_DIRECTORY_ENTRIES: usize = 4096;
pub const FATX_CLUSTER_SIZE: u64 = 16 * 1024;
pub const ATTR_READONLY: u32 = 0x01;
pub const ATTR_HIDDEN: u32 = 0x02;
pub const ATTR_SYSTEM: u32 = 0x04;
pub const ATTR_DIRECTORY: u32 = 0x10;
pub const ATTR_ARCHIVE: u32 = 0x20;
pub const ATTR_NORMAL: u32 = 0x80;
pub const FATX_ATTRIBUTES: u32 =
    ATTR_READONLY | ATTR_HIDDEN | ATTR_SYSTEM | ATTR_DIRECTORY | ATTR_ARCHIVE;

/// NT file time: 100 ns ticks since 1601-01-01, UTC.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Timestamp(pub u64);
impl Timestamp {
    const UNIX_BIAS: u64 = 116_444_736_000_000_000;
    pub fn from_system(time: SystemTime) -> Self {
        let ticks = match time.duration_since(UNIX_EPOCH) {
            Ok(d) => {
                Self::UNIX_BIAS.saturating_add((d.as_nanos() / 100).min(u64::MAX as u128) as u64)
            }
            Err(e) => Self::UNIX_BIAS
                .saturating_sub((e.duration().as_nanos() / 100).min(u64::MAX as u128) as u64),
        };
        Self(ticks)
    }
    pub fn now() -> Self {
        Self::from_system(SystemTime::now()).fatx()
    }
    /// FATX uses a 2000 epoch and two-second resolution; use UTC deterministically.
    pub fn fatx(self) -> Self {
        const MIN: u64 = 125_911_584_000_000_000; // 2000-01-01
        const MAX: u64 = 166_304_447_980_000_000; // 2127-12-31 23:59:58
        Self(self.0.clamp(MIN, MAX) / 20_000_000 * 20_000_000)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BasicInfo {
    pub attributes: u32,
    pub creation_time: Timestamp,
    pub last_access_time: Timestamp,
    pub last_write_time: Timestamp,
    pub change_time: Timestamp,
}
impl BasicInfo {
    pub fn new(attributes: u32) -> Self {
        let now = Timestamp::now();
        Self {
            attributes,
            creation_time: now,
            last_access_time: now,
            last_write_time: now,
            change_time: now,
        }
    }
    pub fn is_directory(self) -> bool {
        self.attributes & ATTR_DIRECTORY != 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Metadata {
    pub basic: BasicInfo,
    pub end_of_file: u64,
    pub allocation_size: u64,
}
impl Metadata {
    pub fn is_directory(&self) -> bool {
        self.basic.is_directory()
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryEntry {
    pub name: String,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeInfo {
    pub label: String,
    pub filesystem: &'static str,
    pub serial: u32,
    pub read_only: bool,
    pub bytes_per_sector: u32,
    pub sectors_per_allocation_unit: u32,
    pub total_allocation_units: u64,
    pub available_allocation_units: u64,
    pub maximum_component_name_length: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Access(pub u32);
impl Access {
    pub const READ: Self = Self(0x1); // also FILE_LIST_DIRECTORY
    pub const WRITE: Self = Self(0x2);
    pub const APPEND: Self = Self(0x4);
    pub const EXECUTE: Self = Self(0x20);
    pub const READ_ATTRIBUTES: Self = Self(0x80);
    pub const WRITE_ATTRIBUTES: Self = Self(0x100);
    pub const DELETE: Self = Self(0x10000);
    pub const ALL: Self = Self(0x001F_01FF);
    pub fn from_nt(mask: u32) -> Result<Self> {
        // Expand generic rights here; do not silently grant MAXIMUM_ALLOWED.
        if mask & 0x0200_0000 != 0 {
            return Err(Error::NotSupported);
        }
        let mut rights = mask & 0x0FFF_FFFF;
        if mask & 0x8000_0000 != 0 {
            rights |= 0x0012_0089;
        }
        if mask & 0x4000_0000 != 0 {
            rights |= 0x0012_0116;
        }
        if mask & 0x2000_0000 != 0 {
            rights |= 0x0012_00A0;
        }
        if mask & 0x1000_0000 != 0 {
            rights |= Self::ALL.0;
        }
        if rights & !Self::ALL.0 != 0 {
            return Err(Error::InvalidParameter);
        }
        Ok(Self(rights))
    }
    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
    pub(crate) fn sharing(self) -> u32 {
        u32::from(self.0 & (Self::READ.0 | Self::EXECUTE.0) != 0)
            | (u32::from(self.0 & (Self::WRITE.0 | Self::APPEND.0) != 0) << 1)
            | (u32::from(self.contains(Self::DELETE)) << 2)
    }
}
impl std::ops::BitOr for Access {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ShareAccess(pub u32);
impl ShareAccess {
    pub const NONE: Self = Self(0);
    pub const READ: Self = Self(1);
    pub const WRITE: Self = Self(2);
    pub const DELETE: Self = Self(4);
    pub const ALL: Self = Self(7);
}
impl std::ops::BitOr for ShareAccess {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum CreateDisposition {
    Supersede = 0,
    Open = 1,
    Create = 2,
    OpenIf = 3,
    Overwrite = 4,
    OverwriteIf = 5,
}
impl TryFrom<u32> for CreateDisposition {
    type Error = Error;
    fn try_from(value: u32) -> Result<Self> {
        match value {
            0 => Ok(Self::Supersede),
            1 => Ok(Self::Open),
            2 => Ok(Self::Create),
            3 => Ok(Self::OpenIf),
            4 => Ok(Self::Overwrite),
            5 => Ok(Self::OverwriteIf),
            _ => Err(Error::InvalidParameter),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum CreateInformation {
    Superseded = 0,
    Opened = 1,
    Created = 2,
    Overwritten = 3,
}

#[derive(Debug, Clone, Copy)]
pub struct OpenOptions {
    pub access: Access,
    pub share: ShareAccess,
    pub disposition: CreateDisposition,
    /// None accepts either kind; Some(true) is FILE_DIRECTORY_FILE.
    pub directory: Option<bool>,
    pub attributes: u32,
    pub delete_on_close: bool,
}
impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            access: Access::READ | Access::READ_ATTRIBUTES,
            share: ShareAccess::ALL,
            disposition: CreateDisposition::Open,
            directory: None,
            attributes: 0,
            delete_on_close: false,
        }
    }
}
/// Owned by VFS, never a guest HANDLE. Kernel must put this in its own object table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Handle(pub(crate) u64);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenResult {
    pub handle: Handle,
    pub information: CreateInformation,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Offset {
    Current,
    Absolute(u64),
    EndOfFile,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileInformation {
    pub metadata: Metadata,
    pub position: u64,
    pub access: Access,
    pub delete_pending: bool,
    pub device_path: String,
}

impl OpenOptions {
    /// Decode the subset of Xbox NtCreateFile flags implemented by this synchronous VFS.
    /// Write-through and unbuffered flags are accepted as hints; guest alignment/completion belongs to the kernel.
    pub fn from_nt(
        access: u32,
        share: u32,
        disposition: u32,
        create_options: u32,
        attributes: u32,
    ) -> Result<Self> {
        const DIRECTORY: u32 = 0x1;
        const NON_DIRECTORY: u32 = 0x40;
        const DELETE_ON_CLOSE: u32 = 0x1000;
        // WRITE_THROUGH, NO_INTERMEDIATE_BUFFERING, SEQUENTIAL_ONLY, SYNCHRONOUS_IO_*, RANDOM_ACCESS,
        // NO_EA_KNOWLEDGE, OPEN_FOR_BACKUP_INTENT and NO_COMPRESSION are harmless hints here.
        const SUPPORTED: u32 =
            0x1 | 0x2 | 0x4 | 0x8 | 0x10 | 0x20 | 0x40 | 0x200 | 0x800 | 0x1000 | 0x4000 | 0x8000;
        if create_options & !SUPPORTED != 0 {
            return Err(Error::NotSupported);
        }
        if (create_options & DIRECTORY != 0 && create_options & NON_DIRECTORY != 0)
            || create_options & 0x30 == 0x30
            || share & !7 != 0
            || attributes & !(FATX_ATTRIBUTES | ATTR_NORMAL) != 0
        {
            return Err(Error::InvalidParameter);
        }
        let access = Access::from_nt(access)?;
        if create_options & 0x30 != 0 && access.0 & 0x100000 == 0 {
            return Err(Error::InvalidParameter);
        }
        if create_options & DELETE_ON_CLOSE != 0 && !access.contains(Access::DELETE) {
            return Err(Error::AccessDenied);
        }
        Ok(Self {
            access,
            share: ShareAccess(share),
            disposition: disposition.try_into()?,
            directory: if create_options & DIRECTORY != 0 {
                Some(true)
            } else if create_options & NON_DIRECTORY != 0 {
                Some(false)
            } else {
                None
            },
            attributes,
            delete_on_close: create_options & DELETE_ON_CLOSE != 0,
        })
    }
}
