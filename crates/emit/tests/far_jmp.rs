//! A far `jmp 0008:target` through the production pipeline (analyze, lift,
//! emit, compiled C) on a synthetic XBE: it is a direct jump for discovery,
//! the CFG and return propagation, here a tail call into another function.

mod common;

use emit::host::{CpuAbi, Ctx, ENTRY, HARNESS_EXIT, JUMP, RETURN, TransferAbi};
use ir::interp::FlatMemory;

const BASE: u32 = 0x10000;
const TEXT: u32 = 0x11000;
const RDATA: u32 = 0x12000;

/// A minimal retail XBE: `.text` at 0x11000 holding `code`, and `.rdata` at
/// 0x12000 with a kernel thunk table importing KeBugCheck.
fn xbe_with(code: &[u8]) -> Vec<u8> {
    let mut d = vec![0u8; 0x1200];
    let put = |d: &mut Vec<u8>, o: usize, v: u32| d[o..o + 4].copy_from_slice(&v.to_le_bytes());
    d[..4].copy_from_slice(b"XBEH");
    put(&mut d, 0x104, BASE);
    put(&mut d, 0x108, 0x1000);
    put(&mut d, 0x10C, 0x3000);
    put(&mut d, 0x118, BASE + 0x400);
    put(&mut d, 0x11C, 2);
    put(&mut d, 0x120, BASE + 0x200);
    put(&mut d, 0x128, TEXT ^ 0xA8FC_57AB);
    put(&mut d, 0x158, RDATA ^ 0x5B6D_40B6);
    for (at, va, name, off) in [(0x200, TEXT, b".text\0".as_slice(), 0x1000), (0x238, RDATA, b".rdata\0", 0x1100)] {
        put(&mut d, at, if va == TEXT { 0x06 } else { 0x02 });
        put(&mut d, at + 4, va);
        put(&mut d, at + 8, 0x100);
        put(&mut d, at + 12, off);
        put(&mut d, at + 16, 0x100);
        let n = if va == TEXT { 0x280 } else { 0x290 };
        put(&mut d, at + 20, BASE + n as u32);
        d[n..n + name.len()].copy_from_slice(name);
    }
    d[0x1000..0x1100].fill(0xCC);
    d[0x1000..0x1000 + code.len()].copy_from_slice(code);
    put(&mut d, 0x1100, 0x8000_0000 | 95);
    d
}

#[test]
fn a_far_jump_to_the_flat_selector_is_a_direct_tail_call() {
    const NAME: &str = "a_far_jump_to_the_flat_selector_is_a_direct_tail_call";
    let (a, b) = (TEXT + 0x10, TEXT + 0x40);
    let mut c = vec![0xCCu8; 0x60];
    let mut at = |va: u32, bytes: &[u8]| c[(va - TEXT) as usize..][..bytes.len()].copy_from_slice(bytes);
    at(TEXT, &[0xE8, 0x0B, 0, 0, 0]); // call A
    at(TEXT + 5, &[0xE8, 0x36, 0, 0, 0]); // call B (so B is a function)
    at(TEXT + 0xA, &[0xC3]);
    let [b0, b1, b2, b3] = b.to_le_bytes();
    at(a, &[0xEA, b0, b1, b2, b3, 0x08, 0x00]); // jmp far 0008:B
    at(b, &[0xB8, 7, 0, 0, 0, 0xC3]); // mov eax, 7; ret

    let data = xbe_with(&c);
    let x = xbe::Xbe::parse(&data).unwrap();
    let analysis = analyze::analyze(&x).unwrap();
    assert!(analysis.functions[&a].returns, "A returns through its far-jump target");
    assert_eq!(analysis.claim(b).map(|c| c.0), Some('I'));
    let lifter = lift::Lifter::new(&analysis, &x).unwrap();
    let f = lifter.lift(a).unwrap();
    assert!(f.insns().all(|i| i.is_supported()));
    let last = f.insns().last().unwrap();
    assert!(
        matches!(last.control, ir::Control::Jump { target: ir::Target::Direct(t), tail: true, .. } if t == b),
        "{last:?}"
    );

    let Some(header) = common::abi_header() else {
        common::skip(NAME, "needs h2abi.h (H2ABI_HEADER)");
        return;
    };
    let owners = emit::plan_image(&analysis, &x).unwrap();
    let g = common::build(&owners, &header, "far-jmp");
    let stack = 0x20_0000;
    let mut mem = FlatMemory::new(stack - 0x1000, 0x1000);
    mem.load(stack - 4, &HARNESS_EXIT.to_le_bytes());
    let mut cpu = CpuAbi::default();
    cpu.gpr[4] = stack - 4;
    let mut ctx = Ctx::new(mem);
    let (t, _) = g.run(&mut cpu, &mut ctx, TransferAbi::new(JUMP, ENTRY, a), 16);
    assert_eq!((t.kind, t.target_pc), (RETURN, HARNESS_EXIT), "{t:x?}");
    assert_eq!(cpu.gpr[0], 7, "B ran");
}
