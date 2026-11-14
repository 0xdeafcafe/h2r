//! Guest CPU state as seen by generated code. Only the state the lifter
//! needs is here; flags are stored unpacked so generated code can set them
//! cheaply and dead ones can be optimised away.

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Cpu {
    pub eax: u32,
    pub ecx: u32,
    pub edx: u32,
    pub ebx: u32,
    pub esp: u32,
    pub ebp: u32,
    pub esi: u32,
    pub edi: u32,
    pub flags: Flags,
    pub x87: X87,
    pub xmm: [[u32; 4]; 8],
    pub mxcsr: u32,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Flags {
    pub cf: bool,
    pub pf: bool,
    pub af: bool,
    pub zf: bool,
    pub sf: bool,
    pub of: bool,
    pub df: bool,
}

/// x87 register stack. Values are kept as f64; whether that is precise
/// enough for Halo 2 (it runs x87 at reduced precision for most code) is a
/// question for differential testing, and this may become 80-bit.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct X87 {
    pub st: [f64; 8],
    pub top: u8,
    pub control: u16,
    pub status: u16,
    pub tag: u16,
}
