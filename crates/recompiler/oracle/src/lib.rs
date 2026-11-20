//! Independent synthetic x86-32 oracle. Execution is optional: enable `unicorn`.
//! No lifter/runtime code or generated instruction semantics are used here.
use serde::{Deserialize, Serialize};

#[cfg(feature = "unicorn")]
mod backend;
pub mod fixtures;
pub mod flags;

pub const CODE_ADDRESS: u32 = 0x10_0000;
pub const PAGE_SIZE: usize = 4096;
pub const MAX_STEPS: usize = 4096;
pub const BACKEND: &str = "unicorn-engine/2.1.5 (Unicorn 2.1.4), x86-32 Pentium III";

/// Register byte arrays are little endian, never host floating-point values.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpuState {
    pub eax: u32,
    pub ecx: u32,
    pub edx: u32,
    pub ebx: u32,
    pub esp: u32,
    pub ebp: u32,
    pub esi: u32,
    pub edi: u32,
    /// The instruction bytes are installed here. Zero is a valid guest address.
    pub eip: u32,
    pub eflags: u32,
    pub x87: X87State,
    pub xmm: [[u8; 16]; 8],
    pub mxcsr: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct X87State {
    /// Logical ST(0)..ST(7); TOP in `status` maps these to physical tags.
    pub st: [[u8; 10]; 8],
    pub control: u16,
    pub status: u16,
    /// Full physical two-bit tag word, not the abridged FXSAVE byte.
    pub tag: u16,
    pub opcode: u16,
    pub instruction_pointer: u32,
    pub data_pointer: u32,
    pub code_segment: u16,
    pub data_segment: u16,
}

impl Default for X87State {
    fn default() -> Self {
        Self {
            st: [[0; 10]; 8],
            control: 0x037f,
            status: 0,
            tag: 0xffff,
            opcode: 0,
            instruction_pointer: 0,
            data_pointer: 0,
            code_segment: 0,
            data_segment: 0,
        }
    }
}

impl Default for CpuState {
    fn default() -> Self {
        Self {
            eax: 0,
            ecx: 0,
            edx: 0,
            ebx: 0,
            esp: 0,
            ebp: 0,
            esi: 0,
            edi: 0,
            eip: CODE_ADDRESS,
            eflags: 2,
            x87: X87State::default(),
            xmm: [[0; 16]; 8],
            mxcsr: 0x1f80,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Access {
    Read,
    Write,
    Execute,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Fault {
    DivideError,
    InvalidOpcode,
    /// Synthetic linear page mapping fault; no guest page tables/error code.
    PageFault {
        address: u32,
        access: Access,
        protection: bool,
    },
    /// Other CPU exceptions remain visible; they are never treated as success.
    Exception {
        vector: u32,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Stop {
    Completed,
    Fault(Fault),
    StepLimit,
    EscapedCode,
    BackendUnavailable,
    InvalidInput(String),
    BackendError(String),
    /// Not an architectural #UD: the harness deliberately refuses this case.
    Unsupported(String),
    /// A sequence consumed an undefined flag or register; do not use as evidence.
    UndefinedInput,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryWrite {
    pub address: u32,
    /// Final bytes for the union of successful writes, including unchanged stores.
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryRead {
    pub address: u32,
    pub size: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstructionOutcome {
    pub eip: u32,
    pub bytes: Vec<u8>,
    pub eflags: u32,
    pub undefined_flags: u32,
    /// EFLAGS bits safe to compare after this instruction (validity propagates).
    pub defined_flags: u32,
    pub completed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    pub backend: String,
    pub state: CpuState,
    pub stop: Stop,
    /// EFLAGS bit positions, including preserved bits. Reserved bits excluded.
    pub defined_flags: u32,
    /// One mask per GPR, in EAX,ECX,EDX,EBX,ESP,EBP,ESI,EDI order.
    pub defined_gprs: [u32; 8],
    pub instructions: Vec<InstructionOutcome>,
    pub writes: Vec<MemoryWrite>,
    pub reads: Vec<MemoryRead>,
}

impl Outcome {
    fn initial(state: CpuState, stop: Stop) -> Self {
        Self {
            backend: BACKEND.into(),
            state,
            stop,
            defined_flags: flags::EFLAGS_MASK,
            defined_gprs: [u32::MAX; 8],
            instructions: vec![],
            writes: vec![],
            reads: vec![],
        }
    }
}

/// Execute bytes from `state.eip` until the byte range ends, a fault, or 4096
/// emulator steps (REP iterations count). Every call creates an isolated engine.
/// `mem` seeds zero-filled 4 KiB pages, RW and non-executable. Code pages are RX.
/// Overlapping seeds/code pages and ranges wrapping 4 GiB are invalid input.
/// This is a flat, paging-disabled protected-mode test harness, not an Xbox VM.
pub fn run(bytes: &[u8], state: CpuState, mem: &[(u32, Vec<u8>)]) -> Outcome {
    #[cfg(feature = "unicorn")]
    {
        backend::run(bytes, state, mem)
    }
    #[cfg(not(feature = "unicorn"))]
    {
        let _ = (bytes, mem);
        Outcome::initial(state, Stop::BackendUnavailable)
    }
}

