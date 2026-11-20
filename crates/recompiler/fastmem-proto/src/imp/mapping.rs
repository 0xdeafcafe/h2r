//! Anonymous page-aligned mappings standing in for the fixed guest base.

use core::ffi::c_void;
use core::ptr;

use super::sys::*;
use crate::Error;

/// A private anonymous read-write mapping, unmapped on drop.
pub struct Mapping {
    base: *mut u8,
    len: usize,
}

// SAFETY: the mapping is plain memory; callers synchronise their accesses.
unsafe impl Send for Mapping {}
// SAFETY: as above.
unsafe impl Sync for Mapping {}

impl Mapping {
    /// Maps `pages` host pages read-write.
    pub fn new(pages: usize) -> Result<Self, Error> {
        let len = pages.checked_mul(page_size()).ok_or(Error::BadRange)?;
        // SAFETY: anonymous mapping with no address hint.
        let base = unsafe {
            mmap(
                ptr::null_mut(),
                len,
                PROT_READ | PROT_WRITE,
                MAP_PRIVATE | MAP_ANON,
                -1,
                0,
            )
        };
        if base == MAP_FAILED {
            return Err(Error::Errno(
                "mmap",
                std::io::Error::last_os_error().raw_os_error().unwrap_or(0),
            ));
        }
        Ok(Self {
            base: base.cast(),
            len,
        })
    }

    pub fn base(&self) -> *mut u8 {
        self.base
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Address of byte `offset`.
    pub fn at(&self, offset: usize) -> *mut u8 {
        assert!(offset < self.len, "offset {offset:#x} outside mapping");
        self.base.wrapping_add(offset)
    }

    /// Address of page `page`.
    pub fn page(&self, page: usize) -> *mut u8 {
        self.at(page * page_size())
    }

    /// Sets the protection of whole pages `[first, first + count)`.
    pub fn protect(&self, first: usize, count: usize, prot: core::ffi::c_int) -> Result<(), Error> {
        let page = page_size();
        assert!((first + count) * page <= self.len);
        // SAFETY: a page-aligned subrange of this mapping.
        if unsafe { mprotect(self.page(first).cast::<c_void>(), count * page, prot) } == 0 {
            Ok(())
        } else {
            Err(Error::Errno(
                "mprotect",
                std::io::Error::last_os_error().raw_os_error().unwrap_or(0),
            ))
        }
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: unmaps exactly what new() mapped.
        unsafe { munmap(self.base.cast(), self.len) };
    }
}
