//! Guest RAM storage, and copy-on-write copies of it for a second executor
//! (docs/runtime/second-executor.md, section 2).
//!
//! Linux x86-64 reserves live RAM between two guard pages. Other hosts use
//! a heap block by default. Flat RAM uses a shared memfd on Linux and a
//! dedicated anonymous mapping on macOS. A macOS snapshot can share pages through
//! `mach_vm_remap(copy = TRUE)`. With that mapping,
//! the kernel copies a host page on the first write to it from either side,
//! so each side keeps its own writes. Other snapshots copy all RAM bytes.

use std::fs::File;
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
    backing: Backing,
    /// The inode used by the last file restore, even after a save replaces its name.
    pub(crate) cow_base: Option<File>,
}

enum Backing {
    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
    Heap,
    #[cfg(target_os = "macos")]
    MachLive,
    #[cfg(target_os = "macos")]
    MachCopy,
    #[cfg(all(unix, not(all(target_os = "linux", target_arch = "x86_64"))))]
    /// A whole-file private mapping ([`Ram::become_private_of`]).
    FilePrivate,
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    Guarded {
        base: NonNull<u8>,
        size: usize,
        file: Option<std::os::fd::OwnedFd>,
    },
}

// SAFETY: `Ram` owns its block outright (like a `Box<[u8]>`); access goes
// through `&self`/`&mut self` like any owned buffer.
unsafe impl Send for Ram {}
// SAFETY: as above; `&Ram` only reads.
unsafe impl Sync for Ram {}

impl Ram {
    /// The live, initially zero physical store.
    pub(crate) fn zeroed(len: usize) -> Self {
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        {
            linux::reserve(len, linux::PREFERRED_BASE, crate::flat::enabled())
                .expect("reserve guarded guest physical RAM")
        }
        #[cfg(target_os = "macos")]
        if crate::flat::enabled() {
            // A dedicated mapping guarantees host-page alignment for shared
            // aliases, independent of the selected global allocator.
            let p = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    len,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                    -1,
                    0,
                )
            };
            if p != libc::MAP_FAILED {
                if let Some(ptr) = NonNull::new(p.cast()) {
                    return Self {
                        ptr,
                        len,
                        backing: Backing::MachLive,
                    };
                }
                unsafe { libc::munmap(p, len) };
            }
        }
        #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
        Self::from_box(vec![0u8; len].into_boxed_slice())
    }
    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
    fn from_box(b: Box<[u8]>) -> Self {
        let len = b.len();
        let ptr = NonNull::new(Box::into_raw(b).cast::<u8>()).expect("Box is non-null");
        Self {
            ptr,
            len,
            backing: Backing::Heap,
            cow_base: None,
        }
    }
    /// A copy of these bytes as they are now; later writes to either side
    /// stay on that side. The caller makes sure nothing writes `self`
    /// meanwhile (it holds `&self`, and C stores hold the gate).
    pub(crate) fn cow_copy(&self) -> (Self, CopyKind) {
        if let Some(copy) = self.remap() {
            return (copy, CopyKind::Remapped);
        }
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        let copy = {
            let mut copy = Self::zeroed(self.len);
            copy.copy_from_slice(self);
            copy
        };
        #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
        let copy = Self::from_box(self[..].into());
        (copy, CopyKind::Copied)
    }
    /// Install a shared alias over a host-page-aligned subrange of a
    /// private address-space reservation.
    ///
    /// # Safety
    /// The caller owns the aligned destination for the entire length.
    /// It synchronizes alias accesses with the canonical RAM owner.
    #[cfg(any(target_os = "macos", all(target_os = "linux", target_arch = "x86_64")))]
    pub(crate) unsafe fn alias(&self, dst: *mut u8, offset: usize, len: usize) -> bool {
        if offset.checked_add(len).is_none_or(|end| end > self.len) {
            return false;
        }
        #[cfg(target_os = "macos")]
        {
            // SAFETY: the caller owns dst and we own the source bytes.
            unsafe { mach::alias(dst as usize, self.ptr.as_ptr().add(offset) as usize, len) }
        }
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        {
            use std::os::fd::AsRawFd;
            let Backing::Guarded { file: Some(fd), .. } = &self.backing else {
                return false;
            };
            // SAFETY: MAP_FIXED only replaces the caller's reservation.
            unsafe {
                libc::mmap(
                    dst.cast(),
                    len,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED | libc::MAP_FIXED,
                    fd.as_raw_fd(),
                    offset as libc::off_t,
                ) != libc::MAP_FAILED
            }
        }
    }


    /// Make this block a private copy-on-write view of `file`, which must
    /// hold exactly this block's bytes: every read comes from the file's
    /// shared pages until this block writes it, and a write gives this
    /// block its own page. Several Guests over the same file share the
    /// pages none of them writes. On failure the block is unchanged.
    pub(crate) fn become_private_of(&mut self, file: File) -> std::io::Result<()> {
        let len = self.len;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(std::io::Error::other("expected a regular RAM image"));
        }
        let bytes = metadata.len();
        if bytes != len as u64 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("the RAM image is {bytes} bytes, guest RAM is {len}"),
            ));
        }
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            // Linux fast memory keeps its address and both guards. Elsewhere
            // let the kernel choose an address before releasing the old block.
            #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
            let (addr, placement) = (self.ptr.as_ptr().cast(), libc::MAP_FIXED);
            #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
            let (addr, placement) = (std::ptr::null_mut(), 0);

            // SAFETY: the image is published by rename and stays immutable.
            // A fixed placement covers only our exclusively owned data pages.
            // MAP_PRIVATE shares the file cache until a guest writes a page.
            let raw = unsafe {
                libc::mmap(
                    addr,
                    len,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_PRIVATE | placement,
                    file.as_raw_fd(),
                    0,
                )
            };
            if raw == libc::MAP_FAILED {
                return Err(std::io::Error::last_os_error());
            }
            #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
            {
                *self = Self {
                    ptr: NonNull::new(raw.cast()).expect("mmap returned a pointer"),
                    len,
                    backing: Backing::FilePrivate,
                    cow_base: None,
                };
            }
            self.cow_base = Some(file);
            Ok(())
        }
        #[cfg(not(unix))]
        {
            let _ = (file, len);
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "a copy-on-write RAM base needs mmap",
            ))
        }
    }
    /// Whether this block is a copy-on-write mapping.
    pub(crate) fn is_mapped(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            matches!(self.backing, Backing::MachCopy)
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
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
            backing: Backing::MachCopy,
            cow_base: None,
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
        match self.backing {
            #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
            Backing::Heap => {
                // SAFETY: from_box took this slice out of its Box.
                unsafe {
                    drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(
                        self.ptr.as_ptr(),
                        self.len,
                    )));
                }
            }
            #[cfg(target_os = "macos")]
            Backing::MachLive => {
                // SAFETY: this is the original live anonymous mapping.
                unsafe { libc::munmap(self.ptr.as_ptr().cast(), self.len) };
            }
            #[cfg(target_os = "macos")]
            Backing::MachCopy => {
                // SAFETY: remap_copy transferred ownership of this mapping.
                unsafe { mach::deallocate(self.ptr.as_ptr() as usize, self.len) };
            }
            #[cfg(all(unix, not(all(target_os = "linux", target_arch = "x86_64"))))]
            Backing::FilePrivate => {
                // SAFETY: become_private_of made this whole range one
                // private mapping this block owns.
                unsafe { libc::munmap(self.ptr.as_ptr().cast(), self.len) };
            }
            #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
            Backing::Guarded { base, size, .. } => {
                // SAFETY: includes both guard pages, exclusively owned by Ram.
                unsafe { libc::munmap(base.as_ptr().cast(), size) };
            }
        }
    }
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod linux {
    use super::{Backing, NonNull, Ram};
    use std::{io, ptr};

    pub(super) const PREFERRED_BASE: usize = 0x4000_0000;

    pub(super) fn reserve(len: usize, preferred: usize, shared: bool) -> io::Result<Ram> {
        // SAFETY: sysconf takes no pointers.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        let page = usize::try_from(page)
            .ok()
            .filter(|n| n.is_power_of_two())
            .ok_or_else(|| io::Error::other("invalid host page size"))?;
        let data = len.checked_next_multiple_of(page);
        let size = data
            .and_then(|n| n.checked_add(page)?.checked_add(page))
            .filter(|&n| n <= isize::MAX as usize && len > 0)
            .ok_or_else(|| io::Error::other("invalid RAM size"))?;

        let flags = libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE;
        // Prefer a reproducible low address. Other Guests (or an existing
        // process mapping) may occupy it. The ABI carries the actual pointer,
        // so a host with a full low arena can still use the normal address space.
        let choices = [
            (preferred as *mut _, libc::MAP_FIXED_NOREPLACE),
            (ptr::null_mut(), libc::MAP_32BIT),
            (ptr::null_mut(), 0),
        ];
        for (addr, placement) in choices {
            // SAFETY: anonymous reservation. No choice can replace another mapping.
            let raw = unsafe { libc::mmap(addr, size, libc::PROT_NONE, flags | placement, -1, 0) };
            if raw == libc::MAP_FAILED {
                if placement == 0 {
                    return Err(io::Error::last_os_error());
                }
                continue;
            }
            // Old kernels can treat NOREPLACE as a hint; retry with MAP_32BIT
            // if such a kernel placed the reservation elsewhere.
            if placement == libc::MAP_FIXED_NOREPLACE && raw != addr {
                unsafe { libc::munmap(raw, size) };
                continue;
            }
            let Some(base) = NonNull::new(raw.cast::<u8>()) else {
                // SAFETY: this call just reserved the range at zero.
                unsafe { libc::munmap(raw, size) };
                continue;
            };
            // SAFETY: the first and last host pages stay inaccessible.
            let ptr = unsafe { base.add(page) };
            // Only an opted-in Guest needs a file descriptor. Keep the
            // private anonymous backing if the kernel refuses memfd.
            let file = shared.then(|| shared_backing(ptr, len)).flatten();
            let result = unsafe {
                libc::mprotect(
                    ptr.as_ptr().cast(),
                    size - page * 2,
                    libc::PROT_READ | libc::PROT_WRITE,
                )
            };
            if result != 0 {
                let err = io::Error::last_os_error();
                // SAFETY: the entire reservation is still ours.
                unsafe { libc::munmap(raw, size) };
                return Err(err);
            }
            return Ok(Ram {
                ptr,
                len,
                backing: Backing::Guarded { base, size, file },
                cow_base: None,
            });
        }
        Err(io::Error::other("could not map non-null guest RAM"))
    }

    fn shared_backing(ptr: NonNull<u8>, len: usize) -> Option<std::os::fd::OwnedFd> {
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        // SAFETY: the fd is new and the fixed destination is our reservation.
        unsafe {
            let raw = libc::memfd_create(c"h2-ram".as_ptr(), libc::MFD_CLOEXEC);
            if raw < 0 {
                return None;
            }
            let fd = OwnedFd::from_raw_fd(raw);
            if libc::ftruncate(fd.as_raw_fd(), len as libc::off_t) != 0 {
                return None;
            }
            let map = libc::mmap(
                ptr.as_ptr().cast(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_FIXED | libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            );
            (map != libc::MAP_FAILED).then_some(fd)
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

    pub(super) unsafe fn alias(dst: usize, src: usize, len: usize) -> bool {
        let mut target = dst as u64;
        let (mut cur, mut max) = (0, 0);
        // VM_FLAGS_FIXED | VM_FLAGS_OVERWRITE, shared (copy=false).
        // SAFETY: src is RAM, dst is reserved, both are host-page aligned.
        unsafe {
            mach_vm_remap(
                mach_task_self_,
                &mut target,
                len as u64,
                0,
                0x4000,
                mach_task_self_,
                src as u64,
                0,
                &mut cur,
                &mut max,
                VM_INHERIT_COPY,
            ) == KERN_SUCCESS
                && target == dst as u64
                && cur & 3 == 3
        }
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

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    fn linux_live_ram_has_two_guards_and_collision_preserves_the_first_guest() {
        let mut first = Ram::zeroed(LEN);
        first[0] = 0xa5;
        let Backing::Guarded { base, .. } = first.backing;
        let mut other = linux::reserve(LEN, base.as_ptr() as usize, false).unwrap();
        assert_ne!(first.as_ptr(), other.as_ptr());
        assert_eq!(other[0], 0);
        other[LEN - 1] = 0x5a;
        assert_eq!((first[0], first[LEN - 1]), (0xa5, 0));

        let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
        let permissions = |addr: usize| {
            maps.lines().find_map(|line| {
                let mut fields = line.split_whitespace();
                let (lo, hi) = fields.next()?.split_once('-')?;
                let range =
                    usize::from_str_radix(lo, 16).ok()?..usize::from_str_radix(hi, 16).ok()?;
                range.contains(&addr).then(|| fields.next().unwrap())
            })
        };
        for ram in [&first, &other] {
            let start = ram.as_ptr() as usize;
            assert_eq!(permissions(start - 1), Some("---p"));
            let access = if matches!(ram.backing, Backing::Guarded { file: Some(_), .. }) {
                "rw-s"
            } else {
                "rw-p"
            };
            assert_eq!(permissions(start), Some(access));
            assert_eq!(permissions(start + LEN - 1), Some(access));
            assert_eq!(permissions(start + LEN), Some("---p"));
        }
    }

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
