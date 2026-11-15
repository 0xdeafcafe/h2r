//! The `h2x87_v1_*` symbols generated x87 code calls resolve by name in a
//! binary that links the runtime, and work on the runtime's own
//! `CpuAbi.x87`.

use runtime::CpuAbi;
use runtime::cpu::X87Abi;

unsafe extern "C" {
    fn h2x87_v1_ld_const(s: *mut X87Abi, which: u32) -> u32;
    fn h2x87_v1_arith(s: *mut X87Abi, op: u32, dst_i: u32, src_kind: u32, src: u64) -> u32;
    fn h2x87_v1_st_f32(s: *mut X87Abi, i: u32, value: *mut u32, sw: *mut u16) -> u32;
    fn h2x87_v1_store_commit(s: *mut X87Abi, sw: u16, pop: u32) -> u32;
    fn h2x87_v1_wait(s: *mut X87Abi) -> u32;
}

#[test]
fn the_x87_helpers_link_and_run_on_the_runtime_state() {
    let mut cpu = CpuAbi::default();
    let s = &mut cpu.x87 as *mut X87Abi;
    // fld1; fld1; faddp; fstp m32 -> 2.0, stack empty again.
    // SAFETY: `s` is a valid, exclusive X87Abi (h2abi.h v1 layout) and the
    // out-pointers are valid for writes.
    let (value, sw) = unsafe {
        assert_eq!(h2x87_v1_ld_const(s, 0), 0);
        assert_eq!(h2x87_v1_ld_const(s, 0), 0);
        assert_eq!(h2x87_v1_arith(s, 0x100, 1, 0, 0), 0); // ADD | POP, ST(1), from ST(0)
        let (mut value, mut sw) = (0u32, 0u16);
        assert_eq!(h2x87_v1_st_f32(s, 0, &mut value, &mut sw), 0);
        assert_eq!(h2x87_v1_store_commit(s, sw, 1), 0);
        assert_eq!(h2x87_v1_wait(s), 0);
        (value, sw)
    };
    assert_eq!(value, 2.0f32.to_bits());
    assert_eq!(sw & 0x3800, 0x3800, "TOP was 7 before the pop");
    assert_eq!((cpu.x87.tag, cpu.x87.status & 0x3800), (0xffff, 0));
}
