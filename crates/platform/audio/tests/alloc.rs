//! The mixer must not allocate on a warmed render with a reverb configured
//! and nothing playing (a render on an audio callback).

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use audio::{I3dl2Listener, Mixer};

thread_local! {
    static COUNT: Cell<u64> = const { Cell::new(0) };
}

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let _ = COUNT.try_with(|c| c.set(c.get() + 1));
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        let _ = COUNT.try_with(|c| c.set(c.get() + 1));
        unsafe { System.realloc(p, l, n) }
    }
}

#[global_allocator]
static A: Counting = Counting;

fn allocs() -> u64 {
    COUNT.with(Cell::get)
}

#[test]
fn an_idle_render_with_reverb_allocates_nothing_after_reserve() {
    let mut m = Mixer::new(48_000);
    m.set_reverb(Some(I3dl2Listener {
        room: 0,
        room_hf: 0,
        room_rolloff: 0.0,
        decay_time: 1.5,
        decay_hf_ratio: 0.8,
        reflections: -1000,
        reflections_delay: 0.01,
        reverb: -500,
        reverb_delay: 0.02,
        diffusion: 100.0,
        density: 100.0,
        hf_reference: 5000.0,
    }));
    m.reserve_block(1024);
    let mut out = vec![0.0f32; 2 * 1024];
    for block in [1024usize, 480, 64, 1024] {
        let out = &mut out[..2 * block];
        m.render(out);
        let before = allocs();
        for _ in 0..8 {
            m.render(out);
        }
        assert_eq!(allocs(), before, "block {block}");
    }
}
