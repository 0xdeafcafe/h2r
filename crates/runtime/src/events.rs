//! Diagnostics owned by one execution context.
use crate::{CpuAbi, GuestFault};

/// Maximum retained diagnostics; history eviction never stops execution.
pub const MAX_EVENTS: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelCall {
    pub ordinal: u32,
    pub name: &'static str,
    pub return_pc: Result<u32, GuestFault>,
    pub stack_args: Vec<Result<u32, GuestFault>>,
    pub variadic: bool,
    pub cpu: CpuAbi,
}

/// PCs are handler entry PCs until generated code spills the current EIP.
/// Memory accessors take an explicit instruction PC instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    KernelCall(Box<KernelCall>),
    MemoryFault {
        pc: u32,
        fault: GuestFault,
    },
    UnsupportedTarget {
        pc: u32,
        target: u32,
        kind: u32,
        route: u32,
    },
    UnrecordedFault {
        pc: u32,
    },
    InvalidAbi,
    InvalidTransfer,
    TransferLimit,
    ContinuationLimit,
    /// Retained for API compatibility; bounded history no longer emits it.
    EventLimit,
    HarnessExit,
}
