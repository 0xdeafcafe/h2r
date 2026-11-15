//! Guest NTSTATUS bit patterns; never host errno values.
pub type NtStatus = u32;
pub const SUCCESS: NtStatus = 0;
pub const WAIT_0: NtStatus = 0;
pub const TIMEOUT: NtStatus = 0x102;
pub const INVALID_HANDLE: NtStatus = 0xc000_0008;
pub const INVALID_PARAMETER: NtStatus = 0xc000_000d;
pub const NO_MEMORY: NtStatus = 0xc000_0017;
pub const CONFLICTING_ADDRESSES: NtStatus = 0xc000_0018;
pub const UNABLE_TO_FREE_VM: NtStatus = 0xc000_001a;
pub const BUFFER_TOO_SMALL: NtStatus = 0xc000_0023;
pub const OBJECT_TYPE_MISMATCH: NtStatus = 0xc000_0024;
pub const SEMAPHORE_LIMIT_EXCEEDED: NtStatus = 0xc000_0047;
pub const INSUFFICIENT_RESOURCES: NtStatus = 0xc000_009a;
pub const MEMORY_NOT_ALLOCATED: NtStatus = 0xc000_00a0;
pub const INVALID_PAGE_PROTECTION: NtStatus = 0xc000_0045;
pub const INVALID_PARAMETER_MIX: NtStatus = 0xc000_0030;
pub const NO_YIELD_PERFORMED: NtStatus = 0x4000_0024;
pub const fn nt_success(status: NtStatus) -> bool {
    (status as i32) >= 0
}
