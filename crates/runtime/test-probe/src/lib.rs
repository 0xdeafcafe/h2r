//! Native declarations used only by runtime's integration tests.
use runtime::{CpuAbi, ExecCtxOpaque, GuestFn, H2GenModule, TransferAbi};
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
    /// tests/generated_module.c: a module in the emitter's shape.
    pub static h2gen_fixture_module: H2GenModule;
    pub fn h2gen_fixture_module_abi_version() -> u32;
    pub fn h2fn_00401000(cpu: *mut CpuAbi, ctx: *mut ExecCtxOpaque, start: u32) -> TransferAbi;
}
