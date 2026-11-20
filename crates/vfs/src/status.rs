//! NTSTATUS values at the storage boundary. Guest structure packing belongs to the kernel.

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[repr(u32)]
pub enum Error {
    #[error("invalid handle")]
    InvalidHandle = 0xC000_0008,
    #[error("invalid parameter")]
    InvalidParameter = 0xC000_000D,
    #[error("no such file (initial directory search)")]
    NoSuchFile = 0xC000_000F,
    #[error("end of file")]
    EndOfFile = 0xC000_0011,
    #[error("access denied")]
    AccessDenied = 0xC000_0022,
    #[error("invalid object name")]
    ObjectNameInvalid = 0xC000_0033,
    #[error("object name not found")]
    ObjectNameNotFound = 0xC000_0034,
    #[error("object name collision")]
    ObjectNameCollision = 0xC000_0035,
    #[error("object path not found")]
    ObjectPathNotFound = 0xC000_003A,
    #[error("sharing violation")]
    SharingViolation = 0xC000_0043,
    #[error("delete pending")]
    DeletePending = 0xC000_0056,
    #[error("disk full")]
    DiskFull = 0xC000_007F,
    #[error("media is write protected")]
    MediaWriteProtected = 0xC000_00A2,
    #[error("file is a directory")]
    FileIsADirectory = 0xC000_00BA,
    #[error("not supported")]
    NotSupported = 0xC000_00BB,
    #[error("directory not empty")]
    DirectoryNotEmpty = 0xC000_0101,
    #[error("not a directory")]
    NotADirectory = 0xC000_0103,
    #[error("too many symbolic links")]
    ReparsePointNotResolved = 0xC000_0280,
    #[error("file too large")]
    FileTooLarge = 0xC000_0904,
    /// A sim store was asked for bytes no recording covered. The named
    /// cause is on stderr; the guest sees a device data error.
    #[error("bytes not in the sim store")]
    NotInStore = 0xC000_009C,
    #[error("filesystem corrupt or ambiguous")]
    FileCorrupt = 0xC000_0102,
    #[error("host I/O error")]
    IoDeviceError = 0xC000_0185,
    #[error("no more files")]
    NoMoreFiles = 0x8000_0006,
}

impl Error {
    pub const fn ntstatus(self) -> u32 {
        self as u32
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        use std::io::ErrorKind::*;
        match error.kind() {
            NotFound => Self::ObjectNameNotFound,
            PermissionDenied => Self::AccessDenied,
            AlreadyExists => Self::ObjectNameCollision,
            InvalidInput | InvalidData => Self::InvalidParameter,
            NotADirectory => Self::NotADirectory,
            IsADirectory => Self::FileIsADirectory,
            DirectoryNotEmpty => Self::DirectoryNotEmpty,
            StorageFull => Self::DiskFull,
            ReadOnlyFilesystem => Self::MediaWriteProtected,
            FileTooLarge => Self::FileTooLarge,
            UnexpectedEof => Self::FileCorrupt,
            _ => Self::IoDeviceError,
        }
    }
}

impl From<xiso::Error> for Error {
    fn from(error: xiso::Error) -> Self {
        match error {
            xiso::Error::Io(e) => e.into(),
            xiso::Error::NotFound(_) => Self::ObjectNameNotFound,
            _ => Self::FileCorrupt,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
