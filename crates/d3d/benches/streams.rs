//! Times copying draws' vertex streams out of guest memory: an indexed-draw
//! heavy frame, retail-free and synthetic.
//!
//! Each draw reads three streams (position, colour, texcoord) from large
//! vertex buffers, over an index window of 500 to 4,000 vertices, so the
//! copy dominates the device's own cost. Every few draws one is an
//! immediate (`Begin`/`End`) quad, whose stream buffers return to the
//! device's pool like the others. The backend discards the draws
//! (`NullBackend`); the frames then run again through the render thread
//! (`ThreadedBackend<NullBackend>`), whose spent stream buffers come back
//! to the device for refilling.
//!
//! Each configuration runs with the vertex cache off (every draw copies its
//! windows, `Device::set_vertex_cache(None)`) and on (`docs/graphics/d3d.md`, "Vertex cache"):
//! the vertex buffers are static unless `H2_BENCH_DYNAMIC` is set, in which
//! case the guest rewrites (and reports) one of the four sets every frame.
//!
//! `cargo bench -p d3d --bench streams` prints ms per frame and vertex
//! bytes copied per frame. Env: `H2_BENCH_FRAMES` (default 100),
//! `H2_BENCH_DRAWS` (default 1000), `H2_BENCH_IMMEDIATE` (one draw in this
//! many is immediate, default 4), `H2_BENCH_DYNAMIC` (default 0).

#[path = "../tests/common/mod.rs"]
mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

use common::*;
use d3d::backend::{DrawOutcome, ResolvedDraw};
use d3d::{Backend, Device, GuestMemory, SparseMemory, ThreadedBackend, UploadPolicy};
use gfx::texture::DecodedTexture;
use gfx::{Clear, D3DFormat};
use shaders::vsh::encode::{Inst, Operand, program};
use shaders::vsh::{MacOp, OutReg};

/// Counts allocations of a page or more (stream-sized, on any thread):
/// fresh ones are what a pool that runs dry costs, page faults included.
struct Counting;

static BIG_ALLOCS: AtomicU64 = AtomicU64::new(0);
static IMMEDIATE_EVERY: AtomicU32 = AtomicU32::new(4);
static DYNAMIC: AtomicU32 = AtomicU32::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.size() >= 4096 {
            BIG_ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if new_size >= 4096 {
            BIG_ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// Counts draws and reads every stream byte, as an upload would.
#[derive(Default)]
struct NullBackend {
    next: u32,
    draws: u64,
    sink: u64,
}

impl Backend for NullBackend {
    type Surface = u32;
    type Texture = u32;
    fn create_surface(&mut self, _: D3DFormat, _: u32, _: u32, _: bool) -> Result<u32, String> {
        self.next += 1;
        Ok(self.next)
    }
    fn destroy_surface(&mut self, _: u32) -> Result<(), String> {
        Ok(())
    }
    fn upload_texture(&mut self, _: &DecodedTexture) -> Result<u32, String> {
        self.next += 1;
        Ok(self.next)
    }
    fn bc_supported(&self) -> bool {
        false
    }
    fn clear(&mut self, _: u32, _: Option<u32>, _: Clear) -> Result<(), String> {
        Ok(())
    }
    fn draw(
        &mut self,
        _: u32,
        _: Option<u32>,
        d: &ResolvedDraw,
        _: [Option<&u32>; 4],
    ) -> Result<DrawOutcome, String> {
        self.draws += 1;
        for s in &d.streams {
            self.sink = self.sink.wrapping_add(s.data.len() as u64);
            if let (Some(a), Some(b)) = (s.data.first(), s.data.last()) {
                self.sink = self.sink.wrapping_add(*a as u64 + *b as u64);
            }
        }
        Ok(DrawOutcome::Drawn)
    }
    fn present(&mut self, _: u32, _: u32) -> Result<(), String> {
        Ok(())
    }
}

const VAF: u32 = MISC + 0x3000;
const STREAM_INPUTS: u32 = MISC + 0x3200;
const FUNCTION: u32 = MISC + 0x3400;
const PSDEF: u32 = MISC + 0x3800;
const PS_OBJECT: u32 = MISC + 0x3900;
/// Index lists, in the contiguous view (so the index cache may keep them).
const INDICES: u32 = 0x8000_0000 | 0x0300_0000;

const VB_VERTS: u32 = 16384;
/// Position (float4), colour (D3DCOLOR), texcoord (float2).
const STRIDES: [u32; 3] = [16, 4, 8];
const VBS: u32 = 4;
/// Index lists, each a window of a different width.
const LISTS: u32 = 8;
const LIST_INDICES: u32 = 3 * 400;

fn vb_header(vb: u32, stream: u32) -> u32 {
    MISC + 0x6000 + 0x40 * vb + 0x10 * stream
}
fn vb_data(vb: u32, stream: u32) -> u32 {
    0x0100_0000 + (vb * 3 + stream) * VB_VERTS * 16
}
fn list(i: u32) -> u32 {
    INDICES + i * LIST_INDICES * 2
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) as u32
    }
}

fn setup<B: Backend>(backend: B) -> (Device<B>, SparseMemory) {
    let (mut d, mut m) = create(backend, UploadPolicy::Tracked);
    m.map(0x8000_0000 | 0x0100_0000, VBS * 3 * VB_VERTS * 16);
    m.map(INDICES, LISTS * LIST_INDICES * 2);
    set_states(&mut d, &mut m);

    let code = program(&[
        Inst::mac(MacOp::Mov, Some(Operand::v(0)), None, None).to_out(false, OutReg::Pos, 0xF),
        Inst::mac(MacOp::Mov, Some(Operand::v(9)), None, None).to_out(false, OutReg::T(0), 0xF),
        Inst::mac(MacOp::Mov, Some(Operand::v(3)), None, None)
            .to_out(false, OutReg::D0, 0xF)
            .last(),
    ]);
    m.write(FUNCTION, &code).unwrap();
    d.load_vertex_shader_program(&mut m, FUNCTION, 0).unwrap();
    for reg in 0..16u32 {
        let (stream, format) = match reg {
            0 => (0, 0x42),
            3 => (1, 0x40),
            9 => (2, 0x22),
            _ => (0, 0x02),
        };
        for (j, w) in [stream, 0, format, 0].iter().enumerate() {
            m.write_u32(VAF + 16 * reg + 4 * j as u32, *w).unwrap();
        }
    }
    d.select_vertex_shader_direct(&mut m, VAF, 0).unwrap();

    let mut rng = Rng(3);
    for vb in 0..VBS {
        for s in 0..3 {
            let bytes: Vec<u8> = (0..VB_VERTS * STRIDES[s as usize])
                .map(|_| rng.next() as u8)
                .collect();
            m.write(0x8000_0000 | vb_data(vb, s), &bytes).unwrap();
            let h = vb_header(vb, s);
            m.write_u32(h, 1 | 0x0100_0000).unwrap();
            m.write_u32(h + 4, vb_data(vb, s)).unwrap();
            m.write_u32(h + 8, 0).unwrap();
        }
    }
    for l in 0..LISTS {
        let width = 500 + l * 500;
        let base = rng.next() % (VB_VERTS - width);
        let idx: Vec<u8> = (0..LIST_INDICES)
            .flat_map(|_| ((base + rng.next() % width) as u16).to_le_bytes())
            .collect();
        m.write(list(l), &idx).unwrap();
    }
    let def = shaders::psh::PixelShaderDef {
        combiner_count: 1,
        ..Default::default()
    };
    m.write(PSDEF, &def.to_bytes()).unwrap();
    for (i, w) in [1, 0, PSDEF].iter().enumerate() {
        m.write_u32(PS_OBJECT + 4 * i as u32, *w).unwrap();
    }
    d.set_pixel_shader(&mut m, PS_OBJECT).unwrap();
    (d, m)
}

fn bind_vb<B: Backend>(d: &mut Device<B>, m: &mut SparseMemory, vb: u32) {
    for s in 0..3 {
        for (i, w) in [vb_header(vb, s), STRIDES[s as usize], 0]
            .iter()
            .enumerate()
        {
            m.write_u32(STREAM_INPUTS + 12 * s + 4 * i as u32, *w)
                .unwrap();
        }
    }
    let handle = d3d::GuestLayout::HALO2.static_vertex_shader | 1;
    d.set_vertex_shader_input(m, handle, 3, STREAM_INPUTS)
        .unwrap();
}

fn frame<B: Backend>(d: &mut Device<B>, m: &mut SparseMemory, draws: u32, f: u32) {
    let mut rng = Rng(f as u64 + 11);
    let every = IMMEDIATE_EVERY.load(Ordering::Relaxed).max(1);
    if DYNAMIC.load(Ordering::Relaxed) != 0 {
        // Dynamic geometry: set 1 rewritten (and reported) every frame.
        for s in 0..3 {
            let len = VB_VERTS * STRIDES[s as usize];
            let bytes: Vec<u8> = (0..len).map(|i| (i ^ f) as u8).collect();
            let va = 0x8000_0000 | vb_data(1, s);
            m.write(va, &bytes).unwrap();
            d.note_write(va, len);
        }
    }
    d.clear(m, 0, 0, 0xF0, 0xFFFF_FFFF, 1.0, 0).unwrap();
    bind_vb(d, m, 0);
    for i in 0..draws {
        let r = rng.next();
        if i % 7 == 0 {
            bind_vb(d, m, r % VBS);
        }
        if i % every == every - 1 {
            d.begin(m, QUADLIST).unwrap();
            for (x, y) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
                d.set_vertex_data(m, 9, [x, y, 0.0, 0.0]).unwrap();
                d.set_vertex_data(m, 3, [1.0, 0.5, 0.25, 1.0]).unwrap();
                d.set_vertex_data(m, -1, [x, y, 0.5, 1.0]).unwrap();
            }
            d.end(m).unwrap();
        } else {
            d.draw_indexed_vertices(m, 5, LIST_INDICES, list(r % LISTS))
                .unwrap();
        }
    }
    d.swap(m, 0).unwrap();
}

fn run<B: Backend>(
    d: &mut Device<B>,
    m: &mut SparseMemory,
    frames: u32,
    draws: u32,
) -> (f64, Vec<f64>) {
    for f in 0..5 {
        frame(d, m, draws, f);
    }
    let before = d.stats();
    let allocs = BIG_ALLOCS.load(Ordering::Relaxed);
    let start = Instant::now();
    let mut times = Vec::new();
    for f in 0..frames {
        let t = Instant::now();
        frame(d, m, draws, f);
        times.push(t.elapsed().as_secs_f64());
    }
    let total = start.elapsed().as_secs_f64();
    let allocs = BIG_ALLOCS.load(Ordering::Relaxed) - allocs;
    let after = d.stats();
    println!(
        "  allocations of 4 KiB or more: {:.2} a draw; vertex bytes copied {:.0} KiB/frame, reused {:.0} KiB/frame, {} cache fills",
        allocs as f64 / (frames as f64 * draws as f64),
        (after.vertex_bytes_copied - before.vertex_bytes_copied) as f64 / 1024.0 / frames as f64,
        (after.vertex_bytes_reused - before.vertex_bytes_reused) as f64 / 1024.0 / frames as f64,
        after.vertex_cache_fills - before.vertex_cache_fills,
    );
    times.sort_by(f64::total_cmp);
    (total, times)
}

fn report(name: &str, frames: u32, draws: u32, total: f64, times: &[f64]) {
    let (median, p10) = (times[times.len() / 2], times[times.len() / 10]);
    println!(
        "{name}: {frames} frames x {draws} draws: {:.3} ms/frame mean, {:.3} median, {:.3} p10, {:.2} us/draw (mean)",
        total * 1e3 / frames as f64,
        median * 1e3,
        p10 * 1e3,
        total * 1e6 / (frames as f64 * draws as f64),
    );
}

fn main() {
    let env = |k, v: u32| {
        std::env::var(k)
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(v)
    };
    let frames = env("H2_BENCH_FRAMES", 100);
    let draws = env("H2_BENCH_DRAWS", 1000);
    IMMEDIATE_EVERY.store(env("H2_BENCH_IMMEDIATE", 4), Ordering::Relaxed);
    DYNAMIC.store(env("H2_BENCH_DYNAMIC", 0), Ordering::Relaxed);
    for cache in [false, true] {
        let tag = if cache { "cache on" } else { "cache off" };
        let budget = cache.then_some(d3d::vertex_cache::DEFAULT_BUDGET);
        let (mut d, mut m) = setup(NullBackend::default());
        d.set_vertex_cache(budget);
        let (total, times) = run(&mut d, &mut m, frames, draws);
        report(
            &format!("streams bench, {tag}"),
            frames,
            draws,
            total,
            &times,
        );
        println!("backend draws {} sink {}", d.backend.draws, d.backend.sink);

        let (mut d, mut m) = setup(ThreadedBackend::new(NullBackend::default()).unwrap());
        d.set_vertex_cache(budget);
        let (total, times) = run(&mut d, &mut m, frames, draws);
        report(
            &format!("streams bench, render thread (guest side), {tag}"),
            frames,
            draws,
            total,
            &times,
        );
        let (n, sink) = d.backend.with_inner(|b| (b.draws, b.sink)).unwrap();
        println!("backend draws {n} sink {sink}");
    }
}
