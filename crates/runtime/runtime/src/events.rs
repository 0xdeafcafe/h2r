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
    /// A non-memory guest exception raised by generated code through
    /// `h2shim_v1_event` (codes 1-7 and 13). `saved_eip` is the EIP the exception
    /// reports to the guest: `pc` for faults, the next instruction for traps.
    GuestException {
        pc: u32,
        saved_eip: u32,
        exception: GuestException,
        backtrace: Vec<GuestFrame>,
    },
    /// A generated-code stop raised through `h2shim_v1_event` (codes 8-12).
    GeneratedStop {
        pc: u32,
        saved_eip: u32,
        reason: StopReason,
        backtrace: Vec<GuestFrame>,
    },
    /// An instruction the lifter does not implement (`h2shim_v1_unsupported`).
    /// `detail` starts with the ISA class (`integer:`, `x87:`, `sse:`, `mmx:`).
    Unsupported {
        pc: u32,
        mnemonic: String,
        detail: String,
        backtrace: Vec<GuestFrame>,
    },
    /// `h2shim_v1_event` with a code this runtime does not know. Treated as
    /// a fault so a newer generator cannot silently continue.
    UnknownShimEvent {
        pc: u32,
        saved_eip: u32,
        code: u32,
        arg: u32,
    },
    /// A delivered [`crate::interrupt::GuestInterrupt`] returned with an ESP
    /// its calling convention does not allow. CPU state was restored to the
    /// interrupted point; the interrupted transfer did not continue.
    InterruptImbalance {
        name: &'static str,
        target: u32,
        expected_esp: u32,
        actual_esp: u32,
    },
}

/// One guest frame, innermost first. Frame 0 is the function executing at
/// the event's PC. Every later frame is a caller and `pc` is the return
/// address its callee will return to. `function` is the entry (or resume
/// owner) the runner last dispatched in that frame, so it is the start of a
/// generated owner, an override address or a callback token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuestFrame {
    pub function: u32,
    pub pc: u32,
}

/// `h2shim_v1_event` codes 1-7, 13 and 14.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestException {
    /// #DE, code 1.
    DivideError,
    /// #BP from `int3`/`int 3`, code 2 (trap).
    Breakpoint,
    /// #UD, e.g. `ud2`, code 3.
    InvalidOpcode,
    /// `int n` with n != 3, code 4 (trap).
    Interrupt { vector: u32 },
    /// `into`, code 5 (trap).
    Overflow,
    /// #BR from `bound`, code 6.
    BoundRange,
    /// #GP, code 7: misaligned 16-byte SSE access, reserved MXCSR bits.
    GeneralProtection,
    /// #XM, code 13: an unmasked SSE floating-point exception.
    SimdException,
    /// #MF (vector 16), code 14: a waiting x87 instruction (or an MMX
    /// instruction, or `emms`) found an unmasked x87 exception pending
    /// (status-word ES set). `status` is the x87 status word, which still
    /// holds the exception flags; `saved_eip` is the waiting instruction,
    /// not the one that raised the exception (FIP still points there).
    FpError { status: u16 },
}

/// `h2shim_v1_event` codes 8-12.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// `hlt`, code 8.
    Halt,
    /// The handler was entered with a start selector it does not accept,
    /// code 9. This is a dispatch or registration bug, not a guest error.
    BadEntry { selector: u32 },
    /// A switch loaded a target outside its validated cases, code 10.
    UnknownTarget { target: u32 },
    /// Analysis could not follow control past the PC, code 11.
    DescentError,
    /// `popfd` would change TF/IF/IOPL/NT, code 12.
    Unmodelled,
}

/// What one `h2shim_v1_event` code means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShimEvent {
    Exception(GuestException),
    Stop(StopReason),
    Unknown { code: u32, arg: u32 },
}

impl ShimEvent {
    pub const DIVIDE_ERROR: u32 = 1;
    pub const BREAKPOINT: u32 = 2;
    pub const INVALID_OPCODE: u32 = 3;
    pub const INTERRUPT: u32 = 4;
    pub const OVERFLOW: u32 = 5;
    pub const BOUND_RANGE: u32 = 6;
    pub const GENERAL_PROTECTION: u32 = 7;
    pub const HALT: u32 = 8;
    pub const BAD_ENTRY: u32 = 9;
    pub const UNKNOWN_TARGET: u32 = 10;
    pub const DESCENT_ERROR: u32 = 11;
    pub const UNMODELLED: u32 = 12;
    pub const SIMD_EXCEPTION: u32 = 13;
    pub const FP_ERROR: u32 = 14;

    pub fn decode(code: u32, arg: u32) -> Self {
        use GuestException as E;
        use StopReason as S;
        match code {
            Self::DIVIDE_ERROR => Self::Exception(E::DivideError),
            Self::BREAKPOINT => Self::Exception(E::Breakpoint),
            Self::INVALID_OPCODE => Self::Exception(E::InvalidOpcode),
            Self::INTERRUPT => Self::Exception(E::Interrupt { vector: arg }),
            Self::OVERFLOW => Self::Exception(E::Overflow),
            Self::BOUND_RANGE => Self::Exception(E::BoundRange),
            Self::GENERAL_PROTECTION => Self::Exception(E::GeneralProtection),
            Self::HALT => Self::Stop(S::Halt),
            Self::BAD_ENTRY => Self::Stop(S::BadEntry { selector: arg }),
            Self::UNKNOWN_TARGET => Self::Stop(S::UnknownTarget { target: arg }),
            Self::DESCENT_ERROR => Self::Stop(S::DescentError),
            Self::UNMODELLED => Self::Stop(S::Unmodelled),
            Self::SIMD_EXCEPTION => Self::Exception(E::SimdException),
            Self::FP_ERROR if arg <= 0xffff => Self::Exception(E::FpError { status: arg as u16 }),
            _ => Self::Unknown { code, arg },
        }
    }
}

impl GuestException {
    /// The instruction class that raises it, for reports.
    pub fn mnemonic(self) -> &'static str {
        match self {
            Self::DivideError => "div/idiv",
            Self::Breakpoint => "int3",
            Self::InvalidOpcode => "ud2",
            Self::Interrupt { .. } => "int",
            Self::Overflow => "into",
            Self::BoundRange => "bound",
            Self::GeneralProtection => "sse/ldmxcsr",
            Self::SimdException => "sse",
            Self::FpError { .. } => "fwait/x87",
        }
    }
    /// Traps report the next instruction; faults report the instruction.
    pub fn is_trap(self) -> bool {
        matches!(
            self,
            Self::Breakpoint | Self::Interrupt { .. } | Self::Overflow
        )
    }
}

impl std::fmt::Display for GuestException {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DivideError => f.write_str("#DE divide error"),
            Self::Breakpoint => f.write_str("#BP breakpoint"),
            Self::InvalidOpcode => f.write_str("#UD invalid opcode"),
            Self::Interrupt { vector } => write!(f, "software interrupt {vector:#04x}"),
            Self::Overflow => f.write_str("#OF overflow"),
            Self::BoundRange => f.write_str("#BR bound range exceeded"),
            Self::GeneralProtection => f.write_str("#GP general protection"),
            Self::SimdException => f.write_str("#XM SIMD floating-point exception"),
            Self::FpError { status } => {
                write!(
                    f,
                    "#MF x87 floating-point error (status word {status:#06x})"
                )
            }
        }
    }
}

impl StopReason {
    pub fn mnemonic(self) -> &'static str {
        match self {
            Self::Halt => "hlt",
            Self::BadEntry { .. } => "<entry>",
            Self::UnknownTarget { .. } => "jmp [switch]",
            Self::DescentError => "<descent>",
            Self::Unmodelled => "popfd",
        }
    }
}

impl std::fmt::Display for StopReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Halt => f.write_str("hlt"),
            Self::BadEntry { selector } => {
                write!(
                    f,
                    "handler entered with unaccepted selector {selector:#010x}"
                )
            }
            Self::UnknownTarget { target } => {
                write!(
                    f,
                    "switch target {target:#010x} outside its validated cases"
                )
            }
            Self::DescentError => f.write_str("analysis could not follow control past here"),
            Self::Unmodelled => f.write_str("popfd would change TF/IF/IOPL/NT"),
        }
    }
}

/// A readable account of one retained event: what happened, where, which
/// instruction, and the guest call chain. See `ExecCtx::report`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaultReport {
    pub id: u32,
    pub pc: Option<u32>,
    pub mnemonic: Option<String>,
    pub summary: String,
    pub backtrace: Vec<GuestFrame>,
}

impl std::fmt::Display for FaultReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "event {}: {}", self.id, self.summary)?;
        if let Some(pc) = self.pc {
            write!(f, "\n  pc {pc:#010x}")?;
        }
        if let Some(m) = &self.mnemonic {
            write!(f, "\n  instruction {m}")?;
        }
        if self.backtrace.is_empty() {
            return f.write_str("\n  no guest frames recorded");
        }
        f.write_str("\n  guest backtrace:")?;
        for (i, frame) in self.backtrace.iter().enumerate() {
            let at = if i == 0 { "at" } else { "returns to" };
            write!(
                f,
                "\n    #{i:<2} {at} {:#010x} in {:#010x}",
                frame.pc, frame.function
            )?;
        }
        Ok(())
    }
}
