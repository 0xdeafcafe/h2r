//! `Runtime::memory_contention`: waits on the guest memory lock that found
//! it held by another thread are counted (docs/perf-budget.md); an
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
    let rt = Runtime::new(Guest::new(), Dispatch::default());
    let (held, holding) = mpsc::channel();
    let (release, released) = mpsc::channel::<()>();
    let holder = {
        let rt = rt.clone();
        std::thread::spawn(move || {
            let g = rt.memory();
            held.send(()).unwrap();
            released.recv().unwrap();
            std::thread::sleep(Duration::from_millis(20));
            drop(g);
        })
    };
    holding.recv().unwrap();
    release.send(()).unwrap();
    // The holder keeps the lock for 20 ms more: this one waits.
    drop(rt.memory());
    holder.join().unwrap();
    let c = rt.memory_contention();
    assert_eq!(c.waits, 1);
    assert!(c.waited >= Duration::from_millis(5), "{:?}", c.waited);
}
