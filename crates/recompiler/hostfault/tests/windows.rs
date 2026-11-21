//! A real Windows access violation handled by an owner that makes the page
//! writable and retries the instruction.
#![cfg(all(windows, target_arch = "x86_64"))]

use std::ptr;
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READONLY, PAGE_READWRITE, VirtualAlloc, VirtualFree,
    VirtualProtect,
};

fn make_writable(fault: &mut hostfault::Fault, address: usize) -> bool {
    if fault.address() != address {
        return false;
    }
    let mut old = 0;
    // SAFETY: this test exclusively owns the page until it drops the region.
    unsafe { VirtualProtect(address as *const _, 1, PAGE_READWRITE, &mut old) != 0 }
}

#[test]
fn retries_owned_write_after_the_owner_changes_protection() {
    // SAFETY: reserve one zero-initialized, read-only page. The only writer
    // runs after the handler is registered.
    let page = unsafe { VirtualAlloc(ptr::null(), 1, MEM_RESERVE | MEM_COMMIT, PAGE_READONLY) };
    assert!(!page.is_null());
    let region =
        hostfault::register_region(page as usize, 1, make_writable, page as usize).unwrap();
    // SAFETY: this instruction first faults, then retries against the same
    // page, which the registered handler makes writable. Inline assembly
    // preserves the exact memory access across optimization levels.
    unsafe {
        std::arch::asm!("mov byte ptr [{}], 7", in(reg) page, options(nostack, preserves_flags));
        assert_eq!(page.cast::<u8>().read_volatile(), 7);
    }
    drop(region);
    // SAFETY: the test owns the whole reservation and readers have drained.
    assert_ne!(unsafe { VirtualFree(page, 0, MEM_RELEASE) }, 0);
}
