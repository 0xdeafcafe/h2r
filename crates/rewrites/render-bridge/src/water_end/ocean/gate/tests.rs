use std::sync::{Arc, Mutex};

use super::*;
use crate::native_flare_query::{Counts, DeviceResult, Host, State};
use guest_layout::memory::FlatMemory;

struct Pending;

impl Counts for Pending {
    fn read(&self) -> Option<Arc<[u32]>> {
        None
    }
}

#[derive(Default)]
struct Gpu {
    passes: Mutex<Vec<Vec<Quad>>>,
}

impl Host for Gpu {
    fn query(&self, _: &mut dyn GuestMemory, quads: Vec<Quad>) -> Option<Arc<dyn Counts>> {
        self.passes.lock().unwrap().push(quads);
        Some(Arc::new(Pending))
    }
    fn device_result(&self, _: &mut dyn GuestMemory, _: u32) -> DeviceResult {
        DeviceResult::Pending
    }
}

const DEF: u32 = 0x0051_0000;
const FRAME: u32 = 10;

/// The array on in view 0 at `FRAME`, an identity camera (x and y over
/// z), a 640 x 480 viewport, and the corners `(±5, ±5, z)`.
fn mem(z: f32) -> FlatMemory {
    let mut m = FlatMemory::new(0x0040_0000, 0x0030_0000);
    let f = |m: &mut FlatMemory, a: u32, v: f32| m.set_f32(a, v).unwrap();
    for (a, v) in [
        (fl::k::ONE, 1.0),
        (fl::k::HALF, 0.5),
        (fl::k::TWO, 2.0),
        (fl::k::EPSILON, 1e-4),
        (fl::k::Z_SCALE, 16_777_215.0),
        (fl::WORLD_SCALE, 1.0),
        (fl::FAR, 1000.0),
        (BYTE_SCALE, 1.0 / 255.0),
        (SEEN, 0.0),
    ] {
        f(&mut m, a, v);
    }
    for (i, v) in [(0, 1.0), (4, 1.0), (8, 1.0)] {
        f(&mut m, fl::VIEW + 4 * i, v);
    }
    for (i, v) in [(0, 1.0), (5, 1.0), (10, 1.0), (11, 1.0)] {
        f(&mut m, fl::PROJECTION + 4 * i, v);
    }
    m.set_u32(fl::VIEWPORT + 4, 480 | 640 << 16).unwrap();
    m.set_u8(fl::ON, 1).unwrap();
    m.set_u32(fl::FRAME, FRAME).unwrap();
    m.set_u32(fl::ARRAY_FRAME, FRAME).unwrap();
    for (k, [x, y]) in [[-5.0, -5.0], [5.0, -5.0], [5.0, 5.0], [-5.0, 5.0]].into_iter().enumerate() {
        let at = CORNERS + 12 * k as u32;
        f(&mut m, at, x);
        f(&mut m, at + 4, y);
        f(&mut m, at + 8, z);
    }
    m
}

/// The first call registers the water's entry, opens (nothing read yet)
/// and places its test: the corners' rectangle in two tiles at their
/// depth, the area held to 16 bits, the query set's bit.
#[test]
fn the_first_call_opens_and_places_the_test() {
    let (gpu, mut state, mut m) = (Gpu::default(), State::default(), mem(10.0));
    assert_eq!(visible(&mut m, &gpu, &mut state, DEF), Ok(true));
    let e = fl::entry_addr(0);
    assert_eq!(m.u32(e).unwrap(), CLASS << 3);
    assert_eq!(m.u32(e + 8).unwrap(), DEF);
    assert_eq!(m.u16(e + 6).unwrap(), 0xffff);
    let (word, bit) = fl::ended_bit(fl::query_set(FRAME), 0);
    assert_ne!(m.u32(word).unwrap() & bit, 0);
    let passes = gpu.passes.lock().unwrap();
    let rects: Vec<_> = passes[0].iter().map(|q| (q.x0, q.y0, q.x1, q.y1)).collect();
    assert_eq!(rects, vec![(159, 119, 415, 360), (415, 119, 480, 360)]);
    assert_eq!(passes[0][0].w, 10.0 / 1000.0 * 16_777_215.0);
    assert_eq!(passes[0][0].area, 256 * 241);
}

/// A test ended two frames back closes the gate at a byte of 0 and opens
/// it at any other; a test not ended leaves it open.
#[test]
fn the_byte_read_two_frames_back_closes_the_gate() {
    let gate = |byte: u8, ended: bool| {
        let (gpu, mut state, mut m) = (Gpu::default(), State::default(), mem(10.0));
        visible(&mut m, &gpu, &mut state, DEF).unwrap();
        m.set_u8(fl::entry_addr(0) + 3, byte).unwrap();
        let (word, bit) = fl::ended_bit(fl::read_set(FRAME), 0);
        m.set_u32(word, if ended { bit } else { 0 }).unwrap();
        visible(&mut m, &gpu, &mut state, DEF).unwrap()
    };
    assert!(!gate(0, true));
    assert!(gate(1, true));
    assert!(gate(0, false));
}

/// A corner behind the eye is all seen with no pass; corners off the
/// viewport count 0 with none.
#[test]
fn an_unbounded_or_empty_rectangle_needs_no_pass() {
    let m = mem(-1.0);
    assert_eq!(test(&m), Ok(Test::Known(0xffff)));
    let mut m = mem(10.0);
    for k in 0..4 {
        m.set_f32(CORNERS + 12 * k, 100.0).unwrap();
    }
    assert_eq!(test(&m), Ok(Test::Known(0)));
}

/// With the array off `0x000209b0` returns -1 and the gate closes.
#[test]
fn no_entry_closes_the_gate() {
    let (gpu, mut state, mut m) = (Gpu::default(), State::default(), mem(10.0));
    m.set_u8(fl::ON, 0).unwrap();
    assert_eq!(visible(&mut m, &gpu, &mut state, DEF), Ok(false));
    assert!(gpu.passes.lock().unwrap().is_empty());
}
