//! Canonical, host-independent CPU storage shared with disposable C11 code.
//!
//! No native floating point type appears here: ext80 and SIMD values retain
//! their guest bits on both Macs. TOP lives in status bits 11..13; `st` stores
//! physical x87 registers (and therefore also the MMX aliases).

/// 2: the ExecCtx preempt-word prefix generated loops read (preempt.rs).
pub const ABI_VERSION: u32 = 2;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct X87Abi {
    /// Little-endian 64-bit significand followed by sign/exponent.
    pub st: [[u8; 10]; 8],
    pub control: u16,
    pub status: u16,
    pub tag: u16,
    pub opcode: u16,
    pub instruction_pointer: u32,
    pub data_pointer: u32,
    pub instruction_selector: u16,
    pub data_selector: u16,
}

impl Default for X87Abi {
    fn default() -> Self {
        Self {
            st: [[0; 10]; 8],
            control: 0x037f,
            status: 0,
            tag: 0xffff,
            opcode: 0,
            instruction_pointer: 0,
            data_pointer: 0,
            instruction_selector: 0,
            data_selector: 0,
        }
    }
}

/// ABI v1: 288 bytes, alignment 4, with no implicit padding or host pointers.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuAbi {
    pub abi_version: u32,
    pub struct_size: u32,
    pub eax: u32,
    pub ecx: u32,
    pub edx: u32,
    pub ebx: u32,
    pub esp: u32,
    pub ebp: u32,
    pub esi: u32,
    pub edi: u32,
    pub eip: u32,
    pub fs_base: u32,
    pub eflags: u32,
    /// Undefined flags retain deterministic values but have clear validity bits.
    pub eflags_defined: u32,
    pub x87: X87Abi,
    pub xmm: [[u8; 16]; 8],
    pub mxcsr: u32,
}

impl Default for CpuAbi {
    fn default() -> Self {
        Self {
            abi_version: ABI_VERSION,
            struct_size: size_of::<Self>() as u32,
            eax: 0,
            ecx: 0,
            edx: 0,
            ebx: 0,
            esp: 0,
            ebp: 0,
            esi: 0,
            edi: 0,
            eip: 0,
            fs_base: 0,
            eflags: 2,
            eflags_defined: u32::MAX,
            x87: X87Abi::default(),
            xmm: [[0; 16]; 8],
            mxcsr: 0x1f80,
        }
    }
}

impl CpuAbi {
    /// Provisional Xbox thread policy from LIFTER.md, distinct from FINIT.
    pub fn xbox_thread() -> Self {
        let mut cpu = Self::default();
        cpu.x87.control = 0x027f;
        cpu
    }

    pub fn valid_abi(&self) -> bool {
        self.abi_version == ABI_VERSION && self.struct_size == size_of::<Self>() as u32
    }
}
