//! The arena against the runtime's real loader and its stack mapping.
use bink_hle::{BinkHle, MovieSource, MovieStream, StreamInfo, addr::BINK_OPEN, register};
use runtime::{Runner, abi, exec::HARNESS_EXIT, guest::Permissions, load_xbe};
use std::sync::Arc;

/// Minimal valid XBE: one RX section at 0x11000, one data section, a 0x40000
/// byte stack commit (so the stack is [0x6ffc0000, 0x70000000)).
fn tiny_xbe() -> Vec<u8> {
    let base = 0x10000u32;
    let mut d = vec![0u8; 0x1100 + 0x90];
    d[..4].copy_from_slice(b"XBEH");
    let mut put = |o: usize, v: u32| d[o..o + 4].copy_from_slice(&v.to_le_bytes());
    for (o, v) in [
        (0x104, base),
        (0x108, 0x1000),
        (0x10c, 0x3000),
        (0x118, base + 0x400),
        (0x11c, 2),
        (0x120, base + 0x200),
        (0x128, 0x11000 ^ 0xa8fc57ab),
        (0x12c, 0x12040),
        (0x130, 0x40000),
        (0x158, 0x12000 ^ 0x5b6d40b6),
        (0x200, 0x06),
        (0x204, 0x11000),
        (0x208, 0x1000),
        (0x20c, 0x1000),
        (0x210, 0x100),
        (0x214, base + 0x280),
        (0x238, 0x03),
        (0x23c, 0x12000),
        (0x240, 0x1000),
        (0x244, 0x1100),
        (0x248, 0x90),
        (0x24c, base + 0x290),
        (0x1100, 0x800000ff),
        (0x1104, 0x8000009c),
        (0x1108, 0x800000a4),
        (0x1140, 0x12000 + 0x88),
        (0x1144, 0x12000 + 0x8c),
        (0x1148, 0x12080),
        (0x1150, 0x90),
        (0x1188, 0x12345678),
    ] {
        put(o, v);
    }
    d[0x280..0x286].copy_from_slice(b".text\0");
    d[0x290..0x296].copy_from_slice(b".data\0");
    d[0x1000..0x1100].fill(0xcc);
    d
}

struct One;
struct S;
impl MovieStream for S {
    fn info(&self) -> StreamInfo {
        StreamInfo {
            width: 4,
            height: 2,
            frames: 3,
            fps_num: 30,
            fps_den: 1,
        }
    }
    fn decode(&mut self, _: u32, b: &mut [u8]) -> bool {
        b.fill(7);
        true
    }
}
impl MovieSource for One {
    fn open(&self, _: &str) -> Option<Box<dyn MovieStream>> {
        Some(Box::new(S))
    }
}

#[test]
fn default_arena_stays_out_of_the_loaders_stack_and_image() {
    let image = load_xbe(&tiny_xbe()).unwrap();
    assert_eq!(
        (image.stack_bottom, image.stack_top),
        (0x6ffc_0000, 0x7000_0000)
    );
    let before: Vec<_> = image.runtime.memory().mappings().to_vec();
    // Sentinels across the top of the stack, including the old default page.
    for a in [0x6ffc_0000u32, 0x6fff_0000, 0x6fff_fff0] {
        image.runtime.memory().set_u32(a, 0xdeca_fbad).unwrap();
    }
    // The override must sit on executable memory.
    image
        .runtime
        .memory()
        .map_zeroed(0x003e_0000, 0x1_0000, Permissions::RWX)
        .unwrap();

    let hle = BinkHle::new(Some(Arc::new(One)));
    image.runtime.edit_dispatch(|d| register(d, &hle));
    let mut ctx = image.context();
    let mut cpu = image.cpu;
    // A path string in the stack, then BinkOpen(path, 0).
    let path = image.stack_top - 0x200;
    {
        let mut m = ctx.memory();
        m.write_bytes(path, b"intro_60.bik\0", 1).unwrap();
        cpu.esp = image.stack_top - 0x100;
        m.set_u32(cpu.esp, HARNESS_EXIT).unwrap();
        m.set_u32(cpu.esp + 4, path).unwrap();
        m.set_u32(cpu.esp + 8, 0).unwrap();
    }
    let t = Runner::default().run(&mut cpu, &mut ctx, BINK_OPEN);
    assert_eq!(t.kind, abi::STOP);
    let h = cpu.eax;
    assert_ne!(h, 0);
    assert!(
        !before
            .iter()
            .any(|m| h >= m.virtual_start && (h as u64) < m.virtual_start as u64 + m.len as u64),
        "handle {h:#x} lies in a pre-existing mapping"
    );
    let mem = ctx.memory();
    for a in [0x6ffc_0000u32, 0x6fff_0000, 0x6fff_fff0] {
        assert_eq!(
            mem.u32(a).unwrap(),
            0xdeca_fbad,
            "stack word at {a:#x} clobbered"
        );
    }
    assert_eq!(mem.u32(h).unwrap(), 4);
}
