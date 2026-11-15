mod common;
use common::Harness;
use runtime::perf::Scope;

#[test]
fn counts_completed_and_failed_kernel_entries() {
    let mut h = Harness::new();
    h.ctx.enable_profiling(true);
    h.success(126, &[]); // KeQueryPerformanceCounter, no args
    assert_eq!(h.ctx.profile().unwrap().timing(Scope::Kernel).calls, 1);
    // Query with an invalid output pointer: the service still counts once.
    h.call(128, &[0]); // KeQuerySystemTime
    assert_eq!(h.ctx.profile().unwrap().timing(Scope::Kernel).calls, 2);
}
