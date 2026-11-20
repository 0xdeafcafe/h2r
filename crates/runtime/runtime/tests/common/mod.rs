#![allow(dead_code)]
pub const ENTRY: u32 = 0x11000;
pub const WRAPPER: u32 = 0x11020;
pub const THUNK: u32 = 0x12000;
pub const TLS: u32 = 0x12040;
pub const TLS_INDEX: u32 = 0x12080;

pub fn synthetic_xbe() -> Vec<u8> {
    let base = 0x10000;
    let mut d = vec![0; 0x1100 + 0x90];
    d[..4].copy_from_slice(b"XBEH");
    let mut put = |offset, value: u32| d[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    for (offset, value) in [
        (0x104, base),
        (0x108, 0x1000),
        (0x10c, 0x3000),
        (0x118, base + 0x400),
        (0x11c, 2),
        (0x120, base + 0x200),
        (0x128, ENTRY ^ 0xa8fc57ab),
        (0x12c, TLS),
        (0x130, 0x40000),
        (0x158, THUNK ^ 0x5b6d40b6),
        (0x200, 0x06),
        (0x204, ENTRY),
        (0x208, 0x1000),
        (0x20c, 0x1000),
        (0x210, 0x100),
        (0x214, base + 0x280),
        (0x238, 0x03),
        (0x23c, THUNK),
        (0x240, 0x1000),
        (0x244, 0x1100),
        (0x248, 0x90),
        (0x24c, base + 0x290),
        (0x1100, 0x800000ff),
        (0x1104, 0x8000009c), // function 255, DATA KeTickCount
        (0x1108, 0x800000a4), // DATA LaunchDataPage: a pointer cell
        (0x1140, THUNK + 0x88),
        (0x1144, THUNK + 0x8c), // four raw TLS bytes
        (0x1148, TLS_INDEX),
        (0x1150, 0x90),
        (0x1188, 0x12345678),
    ] {
        put(offset, value);
    }
    d[0x280..0x286].copy_from_slice(b".text\0");
    d[0x290..0x296].copy_from_slice(b".data\0");
    d[0x1000..0x1100].fill(0xcc);
    d
}

/// Only fixtures use this bridge; production exposes no Rust-reference adapter.
///
/// # Safety
/// The runner must have supplied this exclusive invocation-scoped pair.
pub unsafe fn with_context<R>(
    cpu: *mut runtime::CpuAbi,
    ctx: *mut runtime::ExecCtxOpaque,
    f: impl FnOnce(&mut runtime::CpuAbi, &mut runtime::ExecCtx) -> R,
) -> R {
    // SAFETY: fixture callers forward the runner's exclusive pair.
    unsafe { f(&mut *cpu, &mut *ctx.cast::<runtime::ExecCtx>()) }
}
