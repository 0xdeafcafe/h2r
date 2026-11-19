#![cfg(feature = "live-floor-bench")]
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};
struct Counted;
thread_local! {static COUNTER:Cell<Option<(u64,u64)>>=const {Cell::new(None)};}
fn count(bytes: usize) {
    COUNTER.with(|c| {
        if let Some((n, b)) = c.get() {
            c.set(Some((n + 1, b + bytes as u64)));
        }
    });
}
unsafe impl GlobalAlloc for Counted {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        count(l.size());
        unsafe { System.alloc(l) }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        count(l.size());
        unsafe { System.alloc_zeroed(l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        count(n);
        unsafe { System.realloc(p, l, n) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
}
#[global_allocator]
static COUNT: Counted = Counted;
#[test]
#[ignore = "isolated profile; allocator hooks intentionally absent from live_floor_speed"]
fn profile_speed() {
    live_proto::pack::speed::profile(|on| {
        COUNTER.with(|c| c.replace(on.then_some((0, 0))).unwrap_or_default())
    });
}
