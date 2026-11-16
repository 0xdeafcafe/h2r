//! `Runtime::memory_contention`: waits on the guest memory lock that found
//! it held by another thread are counted (docs/perf/perf-budget.md); an
//! uncontended lock counts nothing.
use runtime::{Dispatch, Guest, Runtime};
use std::sync::mpsc;
use std::time::Duration;

#[test]
fn uncontended_locks_count_nothing() {
    let rt = Runtime::new(Guest::new(), Dispatch::default());
    for _ in 0..1000 {
        drop(rt.memory());
    }
    assert_eq!(rt.memory_contention().waits, 0);
    assert_eq!(rt.memory_contention().waited, Duration::ZERO);
}

#[test]
fn a_wait_for_another_threads_lock_is_counted_with_its_length() {
    // The main thread must reach the lock while the holder still has it; on a
    // loaded host it can be descheduled past the hold, so a round that missed
    // (nothing counted, or a sliver) is retried on a fresh runtime. The
    // checks on the counted round do not depend on timing.
    for _ in 0..50 {
        let rt = Runtime::new(Guest::new(), Dispatch::default());
        let (held, holding) = mpsc::channel();
        let (release, released) = mpsc::channel::<()>();
        let holder = {
            let rt = rt.clone();
            std::thread::spawn(move || {
                let g = rt.memory();
                held.send(()).unwrap();
                released.recv().unwrap();
                std::thread::sleep(Duration::from_millis(50));
                drop(g);
            })
        };
        holding.recv().unwrap();
        release.send(()).unwrap();
        // The holder keeps the lock for 50 ms more: this one waits.
        drop(rt.memory());
        holder.join().unwrap();
        let c = rt.memory_contention();
        assert!(c.waits <= 1, "{c:?}");
        if c.waits == 1 && c.waited >= Duration::from_millis(1) {
            return;
        }
    }
    panic!("no round counted a wait of at least 1 ms");
}
