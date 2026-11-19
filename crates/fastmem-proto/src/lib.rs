//! Isolated prototype of the fastmem write watch on macOS arm64: page
//! protection plus a Mach exception server instead of a per-store dirty mark.
//! See docs/perf/fastmem-proto.md for the design choices, the measured costs and what the
//! fastmem design has to carry over.
//!
//! The library's Mach server is macOS arm64 only. The Linux x86-64
//! implementation runs inside the isolated `fastmem-probe` binary.

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod imp {
    pub mod bench;
    pub mod mapping;
    pub mod ops;
    pub mod server;
    pub mod sys;
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub use imp::{bench, mapping::Mapping, ops, server::*, sys};

/// Errors from setup, registration and re-arming. The handler path never
/// produces these; it counts failures in its statistics instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// A Mach call returned this `kern_return_t`.
    Kern(&'static str, i32),
    /// A libc call failed with this errno.
    Errno(&'static str, i32),
    /// The region or thread table is full.
    Full,
    /// The range is empty, unaligned or overlaps a registered region.
    BadRange,
    /// The region id is not registered.
    BadRegion,
    /// The calling thread is already attached to this server.
    AlreadyAttached,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Kern(what, kr) => write!(f, "{what} failed: kern_return {kr:#x}"),
            Error::Errno(what, e) => write!(f, "{what} failed: errno {e}"),
            Error::Full => f.write_str("table full"),
            Error::BadRange => f.write_str("bad range"),
            Error::BadRegion => f.write_str("unknown region"),
            Error::AlreadyAttached => f.write_str("thread already attached"),
        }
    }
}

impl std::error::Error for Error {}
