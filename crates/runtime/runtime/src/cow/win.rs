//! Windows physical RAM reservations and private file-backed restores.
use super::{Backing, File, NonNull, Ram};
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr;
use windows_sys::Win32::System::Memory::{
    CreateFileMappingFromApp, FILE_MAP_COPY, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE,
    MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFileFromApp, PAGE_NOACCESS, PAGE_READWRITE,
    PAGE_WRITECOPY, UnmapViewOfFile, VirtualAllocFromApp, VirtualFree,
};
use windows_sys::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};

fn page_size() -> usize {
    let mut info = SYSTEM_INFO::default();
    // SAFETY: GetSystemInfo fills the supplied SYSTEM_INFO.
    unsafe { GetSystemInfo(&mut info) };
    info.dwPageSize as usize
}

pub(super) fn reserve(len: usize) -> io::Result<Ram> {
    let page = page_size();
    let size = len
        .checked_next_multiple_of(page)
        .and_then(|n| n.checked_add(page)?.checked_add(page))
        .filter(|&n| len > 0 && n <= isize::MAX as usize)
        .ok_or_else(|| io::Error::other("invalid RAM size"))?;

    // Reserve two inaccessible guard pages around the committed RAM. Use
    // the app-family API so the Xbox app can use the same allocator.
    let raw = unsafe { VirtualAllocFromApp(ptr::null(), size, MEM_RESERVE, PAGE_NOACCESS) };
    let base = NonNull::new(raw.cast::<u8>()).ok_or_else(io::Error::last_os_error)?;
    // SAFETY: the rounded interior is wholly within this reservation.
    let data = unsafe { base.add(page) };
    let committed =
        unsafe { VirtualAllocFromApp(data.as_ptr().cast(), len, MEM_COMMIT, PAGE_READWRITE) };
    if committed.is_null() {
        let error = io::Error::last_os_error();
        unsafe { release(base) };
        return Err(error);
    }
    Ok(Ram {
        ptr: data,
        len,
        backing: Backing::WindowsReserved { base },
        cow_base: None,
    })
}

/// `base` must own the reservation returned by `VirtualAllocFromApp`.
pub(super) unsafe fn release(base: NonNull<u8>) {
    unsafe { VirtualFree(base.as_ptr().cast(), 0, MEM_RELEASE) };
}

pub(super) fn private_view(file: File, len: usize) -> io::Result<Ram> {
    // SAFETY: the caller verified the open file's length; null security
    // attributes and name create a private, unnamed mapping object.
    let handle = unsafe {
        CreateFileMappingFromApp(
            file.as_raw_handle(),
            ptr::null(),
            PAGE_WRITECOPY,
            0,
            ptr::null(),
        )
    };
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: we own the handle, and a successful view holds its own reference.
    let mapping = unsafe { OwnedHandle::from_raw_handle(handle) };
    let view = unsafe { MapViewOfFileFromApp(mapping.as_raw_handle(), FILE_MAP_COPY, 0, len) };
    let ptr = NonNull::new(view.Value.cast::<u8>()).ok_or_else(io::Error::last_os_error)?;
    Ok(Ram {
        ptr,
        len,
        backing: Backing::WindowsFileView,
        cow_base: Some(file),
    })
}

/// `ptr` is the start of a live view from `MapViewOfFileFromApp`.
pub(super) unsafe fn unmap(ptr: NonNull<u8>) {
    unsafe {
        UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
            Value: ptr.as_ptr().cast(),
        })
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, VirtualQuery};

    #[test]
    fn live_and_snapshot_guards_stay_uncommitted() {
        let page = page_size();
        let mut live = reserve(3 * page).unwrap();
        assert_eq!(live[0], 0);
        live[0] = 1;
        let (copy, _) = live.cow_copy();
        live[0] = 2;
        assert_eq!(copy[0], 1);
        for ram in [&live, &copy] {
            let start = ram.as_ptr() as usize;
            for (address, state) in [
                (start - 1, MEM_RESERVE),
                (start, MEM_COMMIT),
                (start + ram.len - 1, MEM_COMMIT),
                (start + ram.len, MEM_RESERVE),
            ] {
                let mut info = MEMORY_BASIC_INFORMATION::default();
                let size = std::mem::size_of_val(&info);
                assert_eq!(
                    unsafe { VirtualQuery(address as *const _, &mut info, size) },
                    size
                );
                assert_eq!(info.State, state);
            }
        }
    }

    #[test]
    fn file_views_keep_writes_private() {
        let path = std::env::temp_dir().join(format!("h2-ram-view-{}", std::process::id()));
        let page = page_size();
        std::fs::write(&path, vec![9; page]).unwrap();
        let mut a = reserve(page).unwrap();
        a.become_private_of(File::open(&path).unwrap()).unwrap();
        let mut b = private_view(File::open(&path).unwrap(), page).unwrap();
        assert!(a.maps_file());
        assert_eq!((a[0], b[1]), (9, 9));
        a[1] = 7;
        b[0] = 8;
        assert_eq!((a[0], b[1]), (9, 9));
        drop((a, b));
        assert_eq!(std::fs::read(&path).unwrap(), vec![9; page]);
        std::fs::remove_file(path).unwrap();
    }
}
