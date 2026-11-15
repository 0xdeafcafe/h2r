//! Guest offsets, independent of the host Rust layout (Xbox x86 kernel ABI).
pub mod kpcr {
    pub const SELF_PCR: u32 = 0x1c;
    pub const PRCB: u32 = 0x20;
    pub const IRQL: u32 = 0x24;
    pub const CURRENT_THREAD: u32 = 0x28;
}
pub mod kthread {
    pub const SIZE: u32 = 0x110;
    pub const SIGNAL_STATE: u32 = 4;
    pub const MUTANT_LIST: u32 = 0x10;
    pub const MUTANT_LIST_BLINK: u32 = 0x14;
    pub const STACK_BASE: u32 = 0x1c;
    pub const STACK_LIMIT: u32 = 0x20;
    pub const KERNEL_STACK: u32 = 0x24;
    pub const TLS_DATA: u32 = 0x28;
    pub const STATE: u32 = 0x2c;
    pub const KERNEL_APC_DISABLE: u32 = 0x68;
    pub const SUSPEND_COUNT: u32 = 0x74;
    pub const HAS_TERMINATED: u32 = 0x77;
}
pub mod ethread {
    pub const SIZE: u32 = 0x140;
    pub const CREATE_TIME: u32 = 0x110;
    pub const EXIT_TIME: u32 = 0x118;
    pub const EXIT_STATUS: u32 = 0x120;
    pub const UNIQUE_THREAD: u32 = 0x12c;
    pub const START_ADDRESS: u32 = 0x130;
    pub const IRP_LIST: u32 = 0x134;
    pub const IRP_LIST_BLINK: u32 = 0x138;
}
