//! Native declarations used only by runtime's integration tests.
use runtime::{CpuAbi, ExecCtxOpaque, GuestFn, TransferAbi};
unsafe extern "C" {
    pub fn h2gen_abi_version() -> u32;
    pub fn h2gen_bad_abi_version() -> u32;
    pub fn h2_test_c_handler(cpu: *mut CpuAbi, ctx: *mut ExecCtxOpaque, pc: u32) -> TransferAbi;
    pub fn h2_test_c_calls_rust(
        f: GuestFn,
        cpu: *mut CpuAbi,
        ctx: *mut ExecCtxOpaque,
        pc: u32,
    ) -> TransferAbi;
    pub fn h2_test_c_memory_widths(
        cpu: *mut CpuAbi,
        ctx: *mut ExecCtxOpaque,
        pc: u32,
    ) -> TransferAbi;
    pub fn h2_test_c_atomic_aliases(
        cpu: *mut CpuAbi,
        ctx: *mut ExecCtxOpaque,
        pc: u32,
    ) -> TransferAbi;
    pub fn h2_test_c_atomics(cpu: *mut CpuAbi, ctx: *mut ExecCtxOpaque, pc: u32) -> TransferAbi;
}
