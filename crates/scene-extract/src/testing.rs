//! Synthetic guest memory for tests, here and in the crates whose stages
//! still read guest memory through [`crate::GuestRead`] (they move here
//! slice by slice). No game data: every record is made up from the
//! offsets the layout modules name.

use std::collections::HashMap;

use frame_scene::NodeMatrix;

use crate::layout::object_nodes::{RECORD_IMPLEMENTATION, RECORD_KEY, Row};
use crate::layout::objects::{
    BODY_RENDER_STATE, CACHE_LIGHTING, CACHE_OWNER, CACHE_RECORD_SIZE, OBJECTS, RECORD_LIGHTING, RECORD_MODEL,
    RENDER_STATE_CACHE,
};
use crate::layout::skip::RENDER_RECORDS;
use crate::{GuestRead, GuestWrite};

/// Sparse guest memory of words.
#[derive(Default)]
pub struct Mem(pub HashMap<u32, u32>);

impl Mem {
    pub fn f(&mut self, addr: u32, v: f32) {
        self.0.insert(addr, v.to_bits());
    }
    pub fn w(&mut self, addr: u32, v: u32) {
        self.0.insert(addr, v);
    }
}

impl GuestRead for Mem {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.get(&addr).copied()
    }
}

impl GuestWrite for Mem {
    fn write_u32(&mut self, addr: u32, value: u32) -> Option<()> {
        self.0.insert(addr, value).map(|_| ())
    }
}

/// A camera record at `at`: at (1, 2, 3) looking down +x, z up, 640x480,
/// 70 degrees, near 1/16, far 1024.
pub fn camera_record(mem: &mut Mem, at: u32) {
    for (i, v) in [1.0, 2.0, 3.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0].into_iter().enumerate() {
        mem.f(at + 4 * i as u32, v);
    }
    mem.f(at + 0x28, 70f32.to_radians());
    mem.w(at + 0x30, 0); // top 0, left 0
    mem.w(at + 0x34, 480 | (640 << 16)); // bottom 480, right 640
    mem.f(at + 0x40, 0.0625);
    mem.f(at + 0x44, 1024.0);
}

/// Three rows whose entries name matrix `n`: `100·n + 4·row + column`.
pub fn rows_of(n: u32) -> Vec<Row> {
    (0..3).map(|r| std::array::from_fn(|c| (100 * n + 4 * r + c as u32) as f32)).collect()
}

pub const ARRAY: u32 = 0x0070_0000;
pub const HEADERS: u32 = 0x0071_0000;
pub const BODIES: u32 = 0x0072_0000;
pub const CACHE: u32 = 0x0073_0000;
pub const CACHE_DATA: u32 = 0x0074_0000;

/// The lighting pointer of render-state cache record `index`.
pub fn lighting(index: u32) -> u32 {
    CACHE_DATA + CACHE_RECORD_SIZE * index + CACHE_LIGHTING
}

/// Makes render-state cache record `index` (salt `0x4000 + index`)
/// `owner`'s, and points `body`'s cache handle at it.
pub fn cache(m: &mut Mem, index: u32, owner: u32, body: u32) {
    m.w(RENDER_STATE_CACHE, CACHE);
    m.w(CACHE + 0x38, 64);
    m.w(CACHE + 0x44, CACHE_DATA);
    m.w(CACHE_DATA + CACHE_RECORD_SIZE * index + CACHE_OWNER, owner);
    m.w(body + BODY_RENDER_STATE, (0x4000 + index) << 16 | index);
}

/// Render record `record`: object geometry of model `0xe100_0000 |
/// record` lit by `lighting`, or the BSP's for `None`.
pub fn record(m: &mut Mem, record: u32, lighting: Option<u32>) {
    let at = RENDER_RECORDS + 0x20 * record;
    // A model record of section 0, part 0, implementation 0.
    m.w(at + RECORD_KEY, 0x2000_0000);
    m.w(at + RECORD_IMPLEMENTATION, 0);
    match lighting {
        Some(l) => {
            m.w(at + RECORD_MODEL, 0xe100_0000 | record);
            m.w(at + RECORD_LIGHTING, l);
        }
        None => {
            m.w(at + RECORD_MODEL, u32::MAX);
            m.w(at + RECORD_LIGHTING, 0);
        }
    }
}

/// Puts object `slot` (salt `0x8000 + slot`) with `nodes` node matrices
/// (node `n` translated by `n` along x) and definition `def` in `m`, its
/// render-state cache record `slot + 7` (so the two indices differ).
pub fn object(m: &mut Mem, slot: u32, def: u32, nodes: u32) -> u32 {
    m.w(OBJECTS, ARRAY);
    m.w(ARRAY + 0x38, 64);
    m.w(ARRAY + 0x44, HEADERS);
    let salt = 0x8000 + slot;
    let header = HEADERS + 0xc * slot;
    let body = BODIES + 0x1000 * slot;
    m.w(header, salt | 0x0100_0000);
    m.w(header + 8, body);
    m.w(body, def);
    for (k, v) in [1.0f32, 2.0, 3.0, 0.5].into_iter().enumerate() {
        m.f(body + 0x30 + 4 * k as u32, v);
    }
    // Size, then offset 0x200.
    m.w(body + 0x114, (0x34 * nodes) | (0x200 << 16));
    for n in 0..nodes {
        let at = body + 0x200 + 0x34 * n;
        let w = NodeMatrix {
            position: [n as f32, 0.0, 0.0],
            ..NodeMatrix::IDENTITY
        };
        for (k, f) in bytemuck::cast_ref::<NodeMatrix, [f32; 13]>(&w).iter().enumerate() {
            m.f(at + 4 * k as u32, *f);
        }
    }
    let handle = salt << 16 | slot;
    cache(m, slot + 7, handle, body);
    handle
}

