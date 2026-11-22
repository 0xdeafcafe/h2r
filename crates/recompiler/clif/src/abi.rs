//! The runtime ABI as the generated code sees it (`h2abi.h` v2,
//! `h2gen.h`, `h2x87.h`): field offsets and constants. The layout
//! assertions in those headers fix them.

pub const ABI_VERSION: u32 = 2;

// TransferAbi kinds and routes.
pub const CALL: u32 = 1;
pub const JUMP: u32 = 2;
pub const RETURN: u32 = 3;
pub const FAULT: u32 = 4;
pub const STOP: u32 = 5;
pub const ENTRY: u32 = 0;
pub const RESUME: u32 = 1;
pub const RETURN_TARGET: u32 = 2;

// CpuAbi.
pub const OFF_GPR: i32 = 8;
pub const OFF_EIP: i32 = 40;
pub const OFF_FS_BASE: i32 = 44;
pub const OFF_EFLAGS: i32 = 48;
pub const OFF_EFLAGS_DEFINED: i32 = 52;
pub const OFF_X87: i32 = 56;
pub const OFF_X87_ST: i32 = OFF_X87;
pub const OFF_X87_CONTROL: i32 = OFF_X87 + 80;
pub const OFF_X87_STATUS: i32 = OFF_X87 + 82;
pub const OFF_X87_TAG: i32 = OFF_X87 + 84;
pub const OFF_XMM: i32 = 156;
pub const OFF_MXCSR: i32 = 284;

// X87Abi fields, from its start.
pub const X87_FOP: i32 = 86;
pub const X87_FIP: i32 = 88;
pub const X87_FDP: i32 = 92;
pub const X87_FCS: i32 = 96;
pub const X87_FDS: i32 = 98;
pub const GUEST_CS: u32 = 0x0008;
pub const GUEST_DS: u32 = 0x0010;

// h2x87.h statuses.
pub const X87_MF: u32 = 0x0100_0000;
pub const X87_SUPPRESS: u32 = 0x0300_0000;
pub const X87_STATUS_MASK: u32 = 0xff00_0000;

// H2SHIM_* event codes.
pub const SHIM_HALT: u32 = 8;
pub const SHIM_BAD_ENTRY: u32 = 9;
pub const SHIM_UNKNOWN_TARGET: u32 = 10;
pub const SHIM_UNMODELLED: u32 = 12;

/// MXCSR bits the guest's Pentium III implements (`H2_MXCSR_MASK`).
pub const MXCSR_MASK: u32 = 0x0000_ffbf;

// The import table (`module::bridge`): every call a handler makes out of
// the module loads its target from `h2clif_imports` and calls it there.
// The host's installers fill it: the boot or test bridge's first eight
// callbacks, the 35 x87 helpers, then the fast tier's own helpers.

/// The table's symbol, hidden in the module.
pub const IMPORTS: &str = "h2clif_imports";
/// The bridge callbacks the handlers call, in both bridges' slot order
/// (`H2BootBridge` and the test host's `H2Bridge` agree on these eight).
pub const CORE: [&str; 8] = [
    "h2abi_v1_read",
    "h2abi_v1_write",
    "h2shim_v1_event",
    "h2shim_v1_unsupported",
    "h2shim_v1_rdtsc",
    "h2abi_v1_atomic_rmw",
    "h2abi_v1_xchg",
    "h2abi_v1_cmpxchg",
];
/// The first x87 slot: then `emit::host::X87_BRIDGE_SYMBOLS` in order.
pub const X87_SLOT: usize = CORE.len();
pub const X87_COUNT: usize = 35;
/// The SSE helper, `ir::interp::sse_helper`: the first of the helpers
/// `h2clif_install_v1` installs.
pub const SSE_SLOT: usize = X87_SLOT + X87_COUNT;
pub const SSE: &str = "h2clif_sse";
/// How many helpers `h2clif_install_v1` takes.
pub const CLIF_COUNT: usize = 1;
pub const SLOTS: usize = SSE_SLOT + CLIF_COUNT;

/// `sizeof(H2BootBridge)` (`h2boot_install_v3`) and `sizeof(H2Bridge)`
/// (`h2bridge_install_v3`): 17 and 16 pointers.
pub const BOOT_BRIDGE_SIZE: u64 = 17 * 8;
pub const TEST_BRIDGE_SIZE: u64 = 16 * 8;

/// `H2X87_UNMODELLED`, what an x87 slot the host left null answers.
pub const X87_UNMODELLED: u32 = 0x0200_0000;

/// The table slot of the import `name`.
pub fn slot(name: &str) -> Option<usize> {
    if name == SSE {
        return Some(SSE_SLOT);
    }
    CORE.iter()
        .position(|n| *n == name)
        .or_else(|| emit::host::X87_BRIDGE_SYMBOLS.iter().position(|n| *n == name).map(|i| X87_SLOT + i))
}
