//! Machine snapshots (docs/runtime/snapshots.md): the file format, the
//! guest RAM page codec, the runtime's own sections and the on-disk store.
//!
//! This crate is the foundation `state.save` / `state.load` build on. It
//! knows nothing about the kernel, devices or the game: each subsystem
//! adds its own tagged section with [`SnapshotWriter::section`] and reads it
//! back with [`Snapshot::require`]. What is saved, rebuilt or refused is
//! decided per subsystem in the design doc.
pub mod codec;
pub mod container;
pub mod features;
pub mod guest;
pub mod pages;
pub mod store;

pub use container::{
    BuildKey, Codec, FORMAT_VERSION, Header, Snapshot, SnapshotWriter, Tag, read_header,
};
pub use pages::{Base, PageStats};
pub use store::Store;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not a snapshot file")]
    NotASnapshot,
    #[error("snapshot format {0} is not {FORMAT_VERSION}")]
    FormatVersion(u32),
    #[error("snapshot is from another build or switch set (saved {saved:?}, running {running:?})")]
    WrongBuild {
        saved: Box<BuildKey>,
        running: Box<BuildKey>,
    },
    #[error("{0} is truncated")]
    Truncated(&'static str),
    #[error("{0} is damaged: {1}")]
    Corrupt(&'static str, String),
    #[error("{0} fails its checksum")]
    Checksum(String),
    #[error("section {0} appears twice")]
    DuplicateSection(Tag),
    #[error("section {0} is missing")]
    MissingSection(Tag),
    #[error("section {0} has version {1}, which this build does not read")]
    SectionVersion(Tag, u32),
    #[error("RAM is a delta against snapshot {0:?}, which was not supplied")]
    NeedsBase(String),
    #[error("page {0:#x} differs from the base it should equal")]
    BaseMismatch(usize),
    #[error(transparent)]
    Restore(#[from] runtime::guest::RestoreError),
    #[error("{0:?} is not a snapshot name (a-z 0-9 . _ -, at most 64)")]
    BadName(String),
    #[error("no snapshot named {0:?}")]
    NoSuchSnapshot(String),
    #[error(transparent)]
    Io(std::io::Error),
}

/// Equality for tests; I/O errors compare by kind.
impl PartialEq for Error {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Io(a), Self::Io(b)) => a.kind() == b.kind(),
            _ => format!("{self:?}") == format!("{other:?}"),
        }
    }
}
