//! The CRT copy routine through the fallback interpreter: every size,
//! alignment and overlap direction must copy as memmove does, through the
//! masked and negative-index jump tables analyze now reads (bceed11), with
//! the boot's remaining unread-table stopgap installed.
//!
//! `H2_XBE=/path/default.xbe cargo test -p boot --test crt_copy -- --ignored`

use runtime::guest::Permissions;
use runtime::{CpuAbi, Event, ExecCtx, Runner};

/// The CRT copy routine (memcpy and memmove share one body).
const CRT_COPY: u32 = 0x0032_0890;
const BUF: u32 = 0x3000_0000;
const STACK: u32 = 0x3010_0000;

#[test]
#[ignore = "needs the retail default.xbe in $H2_XBE"]
fn memmove_sweep_through_the_fallback() {
    let path = std::env::var_os("H2_XBE").expect("set H2_XBE to a retail default.xbe");
    let bytes = std::fs::read(path).unwrap();
    let loaded = runtime::load_xbe_with(&bytes, &runtime::loader::LoadOptions::halo2()).unwrap();
    let rt = loaded.runtime;
    boot::fallback::init(&bytes, &[], &[]).unwrap();
    boot::fallback::set_recovery(true);
    boot::fallback::install_unread_targets(&rt);
    rt.edit_dispatch(|d| {
        d.register_override(CRT_COPY, "test: CRT copy in the fallback", |cpu, ctx, _| {
            cpu.eip = CRT_COPY;
            Ok(boot::fallback::interpret(cpu, ctx))
        });
    });
    {
        let mut m = rt.memory();
        m.map_zeroed(BUF, 0x4000, Permissions::RW).unwrap();
        m.map_zeroed(STACK - 0x1000, 0x1000, Permissions::RW).unwrap();
    }
    let mut ctx = ExecCtx::new(rt.clone(), 0);
    let mut sizes: Vec<u32> = (0..=40).collect();
    sizes.extend([63, 64, 65, 100, 255, 256, 257, 1000, 1027]);
    let mut cases = Vec::new();
    for &n in &sizes {
        for sa in 0..4u32 {
            for da in 0..4u32 {
                cases.push((n, BUF + 0x100 + sa, BUF + 0x1800 + da)); // disjoint
            }
            for delta in [-9i32, -5, -4, -1, 1, 3, 4, 5, 8, 13] {
                let src = BUF + 0x1000 + sa;
                cases.push((n, src, src.wrapping_add_signed(delta)));
            }
        }
    }
    let mut failures = Vec::new();
    for (i, &(n, src, dst)) in cases.iter().enumerate() {
        let pattern: Vec<u8> = (0..0x4000u32).map(|k| (k.wrapping_mul(31) ^ i as u32) as u8).collect();
        let mut expect = pattern.clone();
        let (s, d) = ((src - BUF) as usize, (dst - BUF) as usize);
        expect.copy_within(s..s + n as usize, d);
        let mut cpu = CpuAbi { esp: STACK - 0x100, eflags: 0x202, ebx: 0x1111, esi: 0x2222, edi: 0x3333, ebp: 0x4444, ..Default::default() };
        {
            let mut m = rt.memory();
            m.write_bytes(BUF, &pattern, 1).unwrap();
            for (k, w) in [runtime::runner::HARNESS_EXIT, dst, src, n].into_iter().enumerate() {
                m.set_u32(cpu.esp + 4 * k as u32, w).unwrap();
            }
        }
        let t = Runner::default().run(&mut cpu, &mut ctx, CRT_COPY);
        let mut got = vec![0u8; 0x4000];
        rt.memory().read_bytes(BUF, &mut got, 1).unwrap();
        let ok = ctx.event(t.event_id) == Some(&Event::HarnessExit)
            && got == expect
            && cpu.eax == dst
            && (cpu.ebx, cpu.esi, cpu.edi, cpu.ebp) == (0x1111, 0x2222, 0x3333, 0x4444)
            && cpu.eflags & 0x400 == 0;
        if !ok {
            failures.push(format!("n={n} src={src:#x} dst={dst:#x}: {:?}", ctx.event(t.event_id)));
        }
    }
    assert!(failures.is_empty(), "{} of {} cases failed, first: {:?}", failures.len(), cases.len(), &failures[..failures.len().min(5)]);
}
