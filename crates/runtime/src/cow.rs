//! Guest RAM storage, and copy-on-write copies of it for a second executor
//! (docs/runtime/second-executor.md, section 2).
//!
//! The live RAM is the heap block it always was. A copy is, on macOS, a
//! second mapping of the same pages made with `mach_vm_remap(copy = TRUE)`:
//! the kernel copies a host page on the first write to it from either side,
//! so each side keeps its own writes. Elsewhere, or when the block is not
//! page-aligned, a copy is a plain heap copy.

use std::ops::{Deref, DerefMut};
use std::ptr::NonNull;

/// How [`Ram::cow_copy`] made its copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyKind {
    /// A copy-on-write mapping of the same physical pages.
    Remapped,
    /// A full copy of the bytes.
    Copied,
}

/// One block of guest physical RAM: a heap allocation, or a mapping this
/// type made and owns.
pub(crate) struct Ram {
    ptr: NonNull<u8>,
    len: usize,
    mapped: bool,
}

// SAFETY: `Ram` owns its block outright (like a `Box<[u8]>`); access goes
// through `&self`/`&mut self` like any owned buffer.
unsafe impl Send for Ram {}
// SAFETY: as above; `&Ram` only reads.
unsafe impl Sync for Ram {}

impl Ram {
    /// `len` zeroed bytes on the heap, as `vec![0; len]` (lazily zeroed by
    /// the allocator for large blocks).
    pub(crate) fn zeroed(len: usize) -> Self {
        Self::from_box(vec![0u8; len].into_boxed_slice())
    }
    fn from_box(b: Box<[u8]>) -> Self {
        let len = b.len();
        let ptr = NonNull::new(Box::into_raw(b).cast::<u8>()).expect("Box is non-null");
        Self {
            ptr,
            len,
            mapped: false,
        }
    }
    /// A copy of these bytes as they are now; later writes to either side
    /// stay on that side. The caller makes sure nothing writes `self`
    /// meanwhile (it holds `&self`, and C stores hold the gate).
    pub(crate) fn cow_copy(&self) -> (Self, CopyKind) {
        if let Some(copy) = self.remap() {
            return (copy, CopyKind::Remapped);
        }
        (Self::from_box(self[..].into()), CopyKind::Copied)
    }
    /// Whether this block is a copy-on-write mapping.
    pub(crate) fn is_mapped(&self) -> bool {
        self.mapped
    }

    #[cfg(target_os = "macos")]
    fn remap(&self) -> Option<Self> {
        let page = mach::page_size();
        let addr = self.ptr.as_ptr() as usize;
        if page == 0 || !addr.is_multiple_of(page) || !self.len.is_multiple_of(page) {
            return None;
        }
        // SAFETY: the source range is this block, owned and readable; the
        // kernel picks the destination.
        let target = unsafe { mach::remap_copy(addr, self.len)? };
        Some(Self {
            ptr: NonNull::new(target as *mut u8)?,
            len: self.len,
            mapped: true,
        })
    }
    #[cfg(not(target_os = "macos"))]
    fn remap(&self) -> Option<Self> {
        None
    }
}

impl Deref for Ram {
    type Target = [u8];
    #[inline(always)]
    fn deref(&self) -> &[u8] {
        // SAFETY: `ptr` is valid for `len` bytes for as long as `self` is.
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }
}
impl DerefMut for Ram {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut [u8] {
        // SAFETY: as above, and `&mut self` is exclusive.
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }
}

impl Drop for Ram {
    fn drop(&mut self) {
        if self.mapped {
            #[cfg(target_os = "macos")]
            // SAFETY: this mapping was made by `remap_copy` with this length
            // and is used by nothing else once its owner drops.
            unsafe {
                mach::deallocate(self.ptr.as_ptr() as usize, self.len);
            }
        } else {
            // SAFETY: made by `Box::into_raw` of a `[u8]` of `len` bytes.
            unsafe {
                drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(
                    self.ptr.as_ptr(),
                    self.len,
                )));
            }
        }
    }
}

#[cfg(target_os = "macos")]
mod mach {
    //! The few Mach VM calls a copy needs, declared here rather than
    //! adding a dependency.
    type KernReturn = i32;
    type MachPort = u32;
    type VmProt = i32;
    type VmInherit = u32;
    const KERN_SUCCESS: KernReturn = 0;
    const VM_FLAGS_ANYWHERE: i32 = 1;
    const VM_INHERIT_COPY: VmInherit = 1;

    unsafe extern "C" {
        static mach_task_self_: MachPort;
        static vm_page_size: usize;
        fn mach_vm_remap(
            target_task: MachPort,
            target_address: *mut u64,
            size: u64,
            mask: u64,
            flags: i32,
            src_task: MachPort,
            src_address: u64,
            copy: i32,
            cur_protection: *mut VmProt,
            max_protection: *mut VmProt,
            inheritance: VmInherit,
        ) -> KernReturn;
        fn mach_vm_deallocate(target: MachPort, address: u64, size: u64) -> KernReturn;
    }

    pub(super) fn page_size() -> usize {
        // SAFETY: set once by libSystem before main; read-only after.
        unsafe { vm_page_size }
    }

    /// A copy-semantics mapping of `[addr, addr + len)` at an address the
    /// kernel picks, or `None` if the kernel refuses.
    ///
    /// # Safety
    /// The range is mapped and readable in this task.
    pub(super) unsafe fn remap_copy(addr: usize, len: usize) -> Option<usize> {
        let mut target: u64 = 0;
        let mut cur: VmProt = 0;
        let mut max: VmProt = 0;
        // SAFETY: the out-pointers are live locals; the caller vouches for
        // the source range.
        let kr = unsafe {
            mach_vm_remap(
                mach_task_self_,
                &mut target,
                len as u64,
                0,
                VM_FLAGS_ANYWHERE,
                mach_task_self_,
                addr as u64,
                1,
                &mut cur,
                &mut max,
                VM_INHERIT_COPY,
            )
        };
        // Read and write (VM_PROT_READ | VM_PROT_WRITE) are what RAM needs.
        if kr != KERN_SUCCESS || cur & 3 != 3 {
            if kr == KERN_SUCCESS {
                // SAFETY: just mapped, by us, with this length.
                unsafe { deallocate(target as usize, len) };
            }
            return None;
        }
        Some(target as usize)
    }

    /// # Safety
    /// `[addr, addr + len)` is a mapping this module made and nothing uses
    /// it any more.
    pub(super) unsafe fn deallocate(addr: usize, len: usize) {
        // SAFETY: the caller's contract.
        let kr = unsafe { mach_vm_deallocate(mach_task_self_, addr as u64, len as u64) };
        debug_assert_eq!(kr, KERN_SUCCESS, "mach_vm_deallocate");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEN: usize = 64 * 1024 * 1024;

    #[test]
    fn a_copy_keeps_the_bytes_of_its_moment_and_its_own_writes() {
        let mut live = Ram::zeroed(LEN);
        live[0] = 1;
        live[LEN / 2] = 2;
        live[LEN - 1] = 3;
        let (mut copy, kind) = live.cow_copy();
        if cfg!(target_os = "macos") {
            assert_eq!(kind, CopyKind::Remapped, "the live block is page-aligned");
            assert!(copy.is_mapped());
        }
        live[0] = 10;
        live[100_000] = 11;
        copy[LEN / 2] = 20;
        copy[LEN - 2] = 21;
        assert_eq!(
            (
                copy[0],
                copy[100_000],
                copy[LEN / 2],
                copy[LEN - 2],
                copy[LEN - 1]
            ),
            (1, 0, 20, 21, 3)
        );
        assert_eq!(
            (
                live[0],
                live[100_000],
                live[LEN / 2],
                live[LEN - 2],
                live[LEN - 1]
            ),
            (10, 11, 2, 0, 3)
        );
        // A copy of a copy is frozen the same way.
        let (again, _) = copy.cow_copy();
        copy[0] = 30;
        assert_eq!((again[0], again[LEN / 2]), (1, 20));
        drop(copy);
        assert_eq!(again[LEN - 2], 21);
    }

    #[test]
    fn copies_made_and_dropped_repeatedly_stay_independent() {
        let mut live = Ram::zeroed(LEN);
        for round in 0..64u8 {
            live[round as usize * 4096] = round;
            let (mut copy, _) = live.cow_copy();
            live[round as usize * 4096] = round.wrapping_add(100);
            copy[round as usize * 4096 + 1] = 7;
            assert_eq!(copy[round as usize * 4096], round);
            assert_eq!(live[round as usize * 4096 + 1], 0);
        }
    }

    /// What a snapshot and the first writes after it cost: the copy of a
    /// 64 MB block with 16 MB touched, then one write per 4 KB page of the
    /// touched part from the live side (each first write to a 16 KB host
    /// page is a copy-on-write fault).
    /// `cargo test -p runtime --release --lib cow_copy_cost -- --ignored --nocapture`
    #[test]
    #[ignore = "benchmark"]
    fn cow_copy_cost() {
        let mut live = Ram::zeroed(LEN);
        let touched = 16 * 1024 * 1024;
        live[..touched].fill(1);
        for _ in 0..5 {
            let t = std::time::Instant::now();
            let (copy, kind) = live.cow_copy();
            let copy_us = t.elapsed().as_secs_f64() * 1e6;
            let t = std::time::Instant::now();
            for p in (0..touched).step_by(4096) {
                live[p] = 2;
            }
            let write_us = t.elapsed().as_secs_f64() * 1e6;
            let t = std::time::Instant::now();
            drop(copy);
            let drop_us = t.elapsed().as_secs_f64() * 1e6;
            println!(
                "{kind:?}: copy {copy_us:.0} us, {} first writes {write_us:.0} us ({:.2} us each), drop {drop_us:.0} us",
                touched / 4096,
                write_us / (touched / 4096) as f64
            );
        }
    }
}
