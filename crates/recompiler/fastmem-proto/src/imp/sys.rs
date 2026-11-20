//! Hand-written declarations for the Mach and libc entry points the prototype
//! uses. Values come from the macOS SDK headers (`mach/exception_types.h`,
//! `mach/arm/thread_status.h`, `mach/message.h`, `mach/exc.h`).

#![allow(non_camel_case_types, non_upper_case_globals, dead_code)]

use core::ffi::{c_int, c_void};

pub type kern_return_t = c_int;
pub type mach_port_t = u32;
pub type exception_mask_t = u32;
pub type exception_behavior_t = c_int;
pub type thread_state_flavor_t = c_int;
pub type mach_msg_return_t = c_int;

pub const KERN_SUCCESS: kern_return_t = 0;
pub const KERN_INVALID_ADDRESS: kern_return_t = 1;
pub const KERN_PROTECTION_FAILURE: kern_return_t = 2;
pub const KERN_FAILURE: kern_return_t = 5;
pub const MIG_BAD_ID: kern_return_t = -303;

pub const MACH_PORT_NULL: mach_port_t = 0;
pub const MACH_PORT_DEAD: mach_port_t = !0;
pub const MACH_PORT_RIGHT_SEND: u32 = 0;
pub const MACH_PORT_RIGHT_RECEIVE: u32 = 1;

pub const MACH_MSG_TYPE_MOVE_SEND_ONCE: u32 = 18;
pub const MACH_MSG_TYPE_COPY_SEND: u32 = 19;
pub const MACH_MSG_TYPE_MAKE_SEND: u32 = 20;
pub const MACH_MSG_TYPE_MAKE_SEND_ONCE: u32 = 21;
pub const MACH_MSG_PORT_DESCRIPTOR: u32 = 0;
pub const MACH_MSGH_BITS_COMPLEX: u32 = 0x8000_0000;

pub const MACH_SEND_MSG: c_int = 0x1;
pub const MACH_RCV_MSG: c_int = 0x2;
pub const MACH_SEND_TIMEOUT: c_int = 0x10;
pub const MACH_RCV_TIMEOUT: c_int = 0x100;
pub const MACH_MSG_SUCCESS: mach_msg_return_t = 0;

pub const EXC_BAD_ACCESS: c_int = 1;
pub const EXC_MASK_BAD_ACCESS: exception_mask_t = 1 << EXC_BAD_ACCESS;
pub const EXC_TYPES_COUNT: usize = 14;
pub const EXCEPTION_DEFAULT: exception_behavior_t = 1;
pub const MACH_EXCEPTION_CODES: exception_behavior_t = 0x8000_0000_u32 as c_int;

pub const THREAD_STATE_NONE: thread_state_flavor_t = 5;
pub const ARM_EXCEPTION_STATE64: thread_state_flavor_t = 7;
pub const ARM_EXCEPTION_STATE64_COUNT: u32 = 4;

/// MIG message ids from `exc.defs` / `mach_exc.defs` (subsystem 2401).
pub const EXCEPTION_RAISE_ID: i32 = 2401;
pub const MACH_EXCEPTION_RAISE_ID: i32 = 2405;
pub const MIG_REPLY_DELTA: i32 = 100;
/// The kernel's notification for a send-once right that died unused.
pub const MACH_NOTIFY_SEND_ONCE: i32 = 0o107;

/// `NDR_record` for little-endian, IEEE-float, ASCII hosts.
pub const NDR_RECORD: [u8; 8] = [0, 0, 0, 0, 1, 0, 0, 0];

pub const PROT_NONE: c_int = 0;
pub const PROT_READ: c_int = 1;
pub const PROT_WRITE: c_int = 2;
pub const MAP_PRIVATE: c_int = 0x2;
pub const MAP_ANON: c_int = 0x1000;
pub const MAP_FAILED: *mut c_void = !0usize as *mut c_void;
pub const _SC_PAGESIZE: c_int = 29;
/// `QOS_CLASS_USER_INTERACTIVE` from `sys/qos.h`.
pub const QOS_CLASS_USER_INTERACTIVE: u32 = 0x21;

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct mach_msg_header_t {
    pub bits: u32,
    pub size: u32,
    pub remote_port: mach_port_t,
    pub local_port: mach_port_t,
    pub voucher_port: mach_port_t,
    pub id: i32,
}

/// `mach_msg_port_descriptor_t`: name, pad1, then a 32-bit word holding
/// pad2:16, disposition:8 and type:8 (low to high).
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct mach_msg_port_descriptor_t {
    pub name: mach_port_t,
    pub pad1: u32,
    pub word: u32,
}

impl mach_msg_port_descriptor_t {
    pub fn new(name: mach_port_t, disposition: u32) -> Self {
        Self {
            name,
            pad1: 0,
            word: (disposition << 16) | (MACH_MSG_PORT_DESCRIPTOR << 24),
        }
    }
}

/// `__Request__mach_exception_raise_t`: MIG packs these at 4 bytes, so the
/// 64-bit codes sit at offset 68.
#[repr(C, packed(4))]
#[derive(Clone, Copy, Debug)]
pub struct ExceptionRaise64 {
    pub head: mach_msg_header_t,
    pub descriptor_count: u32,
    pub thread: mach_msg_port_descriptor_t,
    pub task: mach_msg_port_descriptor_t,
    pub ndr: [u8; 8],
    pub exception: c_int,
    pub code_count: u32,
    pub code: [i64; 2],
}

/// `__Request__exception_raise_t`: the same with 32-bit codes.
#[repr(C, packed(4))]
#[derive(Clone, Copy, Debug)]
pub struct ExceptionRaise32 {
    pub head: mach_msg_header_t,
    pub descriptor_count: u32,
    pub thread: mach_msg_port_descriptor_t,
    pub task: mach_msg_port_descriptor_t,
    pub ndr: [u8; 8],
    pub exception: c_int,
    pub code_count: u32,
    pub code: [i32; 2],
}

/// `__Reply__exception_raise_t` (identical for the 64-bit variant).
#[repr(C, packed(4))]
#[derive(Clone, Copy, Debug)]
pub struct ExceptionReply {
    pub head: mach_msg_header_t,
    pub ndr: [u8; 8],
    pub ret_code: kern_return_t,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct arm_exception_state64 {
    pub far: u64,
    pub esr: u32,
    pub exception: u32,
}

const _: () = assert!(core::mem::size_of::<ExceptionRaise64>() == 84);
const _: () = assert!(core::mem::size_of::<ExceptionRaise32>() == 76);
const _: () = assert!(core::mem::size_of::<ExceptionReply>() == 36);

unsafe extern "C" {
    pub static mach_task_self_: mach_port_t;

    pub fn mach_msg(
        msg: *mut mach_msg_header_t,
        option: c_int,
        send_size: u32,
        rcv_size: u32,
        rcv_name: mach_port_t,
        timeout: u32,
        notify: mach_port_t,
    ) -> mach_msg_return_t;

    pub fn mach_port_allocate(
        task: mach_port_t,
        right: u32,
        name: *mut mach_port_t,
    ) -> kern_return_t;
    pub fn mach_port_insert_right(
        task: mach_port_t,
        name: mach_port_t,
        poly: mach_port_t,
        poly_poly: u32,
    ) -> kern_return_t;
    pub fn mach_port_mod_refs(
        task: mach_port_t,
        name: mach_port_t,
        right: u32,
        delta: c_int,
    ) -> kern_return_t;
    pub fn mach_port_deallocate(task: mach_port_t, name: mach_port_t) -> kern_return_t;
    pub fn mach_port_names(
        task: mach_port_t,
        names: *mut *mut mach_port_t,
        names_count: *mut u32,
        types: *mut *mut u32,
        types_count: *mut u32,
    ) -> kern_return_t;
    pub fn mach_msg_destroy(msg: *mut mach_msg_header_t);
    pub fn vm_deallocate(task: mach_port_t, address: usize, size: usize) -> kern_return_t;

    pub fn thread_swap_exception_ports(
        thread: mach_port_t,
        mask: exception_mask_t,
        new_port: mach_port_t,
        behavior: exception_behavior_t,
        flavor: thread_state_flavor_t,
        masks: *mut exception_mask_t,
        masks_count: *mut u32,
        old_handlers: *mut mach_port_t,
        old_behaviors: *mut exception_behavior_t,
        old_flavors: *mut thread_state_flavor_t,
    ) -> kern_return_t;
    pub fn thread_set_exception_ports(
        thread: mach_port_t,
        mask: exception_mask_t,
        new_port: mach_port_t,
        behavior: exception_behavior_t,
        flavor: thread_state_flavor_t,
    ) -> kern_return_t;
    pub fn thread_get_exception_ports(
        thread: mach_port_t,
        mask: exception_mask_t,
        masks: *mut exception_mask_t,
        masks_count: *mut u32,
        old_handlers: *mut mach_port_t,
        old_behaviors: *mut exception_behavior_t,
        old_flavors: *mut thread_state_flavor_t,
    ) -> kern_return_t;
    pub fn task_swap_exception_ports(
        task: mach_port_t,
        mask: exception_mask_t,
        new_port: mach_port_t,
        behavior: exception_behavior_t,
        flavor: thread_state_flavor_t,
        masks: *mut exception_mask_t,
        masks_count: *mut u32,
        old_handlers: *mut mach_port_t,
        old_behaviors: *mut exception_behavior_t,
        old_flavors: *mut thread_state_flavor_t,
    ) -> kern_return_t;
    pub fn task_set_exception_ports(
        task: mach_port_t,
        mask: exception_mask_t,
        new_port: mach_port_t,
        behavior: exception_behavior_t,
        flavor: thread_state_flavor_t,
    ) -> kern_return_t;
    pub fn thread_get_state(
        thread: mach_port_t,
        flavor: thread_state_flavor_t,
        state: *mut u32,
        count: *mut u32,
    ) -> kern_return_t;

    pub fn pthread_self() -> usize;
    pub fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: c_int) -> c_int;
    pub fn pthread_mach_thread_np(thread: usize) -> mach_port_t;

    pub fn mmap(
        addr: *mut c_void,
        len: usize,
        prot: c_int,
        flags: c_int,
        fd: c_int,
        offset: i64,
    ) -> *mut c_void;
    pub fn munmap(addr: *mut c_void, len: usize) -> c_int;
    pub fn mprotect(addr: *mut c_void, len: usize, prot: c_int) -> c_int;
    pub fn sysconf(name: c_int) -> core::ffi::c_long;
}

#[inline]
pub fn task_self() -> mach_port_t {
    // SAFETY: libsystem initialises this before main and never changes it.
    unsafe { mach_task_self_ }
}

/// The calling thread's port name. `pthread_mach_thread_np` does not add a
/// send-right reference, unlike `mach_thread_self`.
#[inline]
pub fn thread_self() -> mach_port_t {
    // SAFETY: plain queries on the calling thread.
    unsafe { pthread_mach_thread_np(pthread_self()) }
}

#[inline]
pub fn mach_msgh_bits(remote: u32, local: u32) -> u32 {
    remote | (local << 8)
}

pub fn page_size() -> usize {
    // SAFETY: sysconf has no preconditions.
    unsafe { sysconf(_SC_PAGESIZE) as usize }
}
