//! Replays a synthetic, retail-free frame through the device and times it.
//!
//! The shape follows what the menu does per frame (counts observed, no
//! bytes): about 1,200 draws, each preceded by texture rebinding, constant
//! uploads, render-state pokes through the push buffer, and a vertex input
//! switch every few draws; most draws are indexed. The backend discards the
//! draw (`NullBackend`), so this measures the device's own CPU cost: push
//! decoding and dispatch, state reads, shader inputs, vertex/index fetch and
//! texture change detection. Guest memory is `SparseMemory`, which is part
//! of what is measured.
//!
//! The same frames then run through the render thread
//! (`ThreadedBackend<NullBackend>`): the guest's side of that is what the
//! game thread pays per draw with `H2_RENDER_THREAD` (queueing each backend
//! call), timed without the render thread's own work.
//!
//! A third run puts the real shader judge (`Nv2aJudge`, what `GfxBackend`
//! judges with) behind the render thread, so the guest's side pays for
//! deciding each draw's outcome as it does in the game.
//!
//! `cargo bench -p d3d --bench pushbuf` prints ms per frame. Env:
//! `H2_BENCH_FRAMES` (default 200), `H2_BENCH_DRAWS` (default 1200).

#[path = "../tests/common/mod.rs"]
mod common;

use std::time::Instant;

use common::*;
use d3d::backend::{DrawOutcome, ResolvedDraw};
use d3d::{Backend, Device, GuestMemory, SparseMemory, ThreadedBackend, UploadPolicy};
use gfx::texture::DecodedTexture;
use gfx::{Clear, D3DFormat};
use shaders::vsh::encode::{Inst, Operand, program};
use shaders::vsh::{MacOp, OutReg};

/// Counts draws and touches the parts of the draw a real backend reads.
#[derive(Default)]
struct NullBackend {
    next: u32,
    draws: u64,
    sink: u64,
    /// With `judged`: the real shader judge (`Nv2aJudge`) answers each
    /// draw, as `GfxBackend` does, so the render thread runs it on the
    /// guest's side too.
    judged: bool,
    verdicts: Option<Box<dyn d3d::shader::ShaderJudge>>,
}

impl NullBackend {
    fn judged() -> Self {
        Self {
            judged: true,
            ..Self::default()
        }
    }
}

/// `GfxJudge`'s provider half: every view sampleable, every draw routed
/// to the provider.
struct BenchJudge(Box<dyn d3d::shader::ShaderJudge>);

fn nv2a_judge() -> Box<dyn d3d::shader::ShaderJudge> {
    use d3d::shader::ShaderProvider;
    d3d::nv2a::Nv2aShaders::new()
        .judge(gfx::translate::Capabilities::ALL)
        .expect("nv2a judges")
}

fn verdict(
    j: &mut dyn d3d::shader::ShaderJudge,
    d: &ResolvedDraw,
    views: [Option<bool>; 4],
) -> DrawOutcome {
    let present = std::array::from_fn(|i| d.texture_info[i].is_some() && views[i] == Some(true));
    match j.check(d, present) {
        Ok(()) => DrawOutcome::Drawn,
        Err(e) => DrawOutcome::Skipped(e),
    }
}

impl d3d::backend::DrawJudge for BenchJudge {
    fn upload_view(&self, _: &DecodedTexture) -> bool {
        true
    }
    fn surface_view(&self, _: D3DFormat) -> bool {
        true
    }
    fn draw(&mut self, d: &ResolvedDraw, views: [Option<bool>; 4]) -> DrawOutcome {
        verdict(&mut *self.0, d, views)
    }
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
        self.sink += d.vertex_count as u64 + d.indices.as_ref().map_or(0, |i| i.len() as u64);
        if self.judged {
            let j = self.verdicts.get_or_insert_with(nv2a_judge);
            let views = std::array::from_fn(|i| d.texture_info[i].map(|_| true));
            return Ok(verdict(&mut **j, d, views));
        }
        Ok(DrawOutcome::Drawn)
    }
    fn judge(&self) -> Option<Box<dyn d3d::backend::DrawJudge>> {
        self.judged.then(|| Box::new(BenchJudge(nv2a_judge())) as _)
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
const CONSTS: u32 = MISC + 0x3A00;
const INDICES: u32 = MISC + 0x4000;

const TEXTURES: u32 = 48;
const TEX_SIZE: u32 = 64 * 64 * 4;
const VBS: u32 = 8;
const VB_VERTS: u32 = 256;
const VB_STRIDE: u32 = 32;
const TRIS: u32 = 120;

fn tex_header(i: u32) -> u32 {
    MISC + 0x5000 + 0x20 * i
}
fn vb_header(i: u32) -> u32 {
    MISC + 0x6000 + 0x10 * i
}
fn tex_data(i: u32) -> u32 {
    0x0040_0000 + TEX_SIZE * i
}
fn vb_data(i: u32) -> u32 {
    0x0030_0000 + VB_VERTS * VB_STRIDE * i
}

/// Tiny deterministic generator.
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
    m.map(0x8000_0000 | 0x0040_0000, TEX_SIZE * TEXTURES);
    m.map(0x8000_0000 | 0x0030_0000, VB_VERTS * VB_STRIDE * VBS);
    set_states(&mut d, &mut m);

    let code = program(&[
        Inst::mac(MacOp::Mul, Some(Operand::v(0)), Some(Operand::c(96)), None).to_temp(0, 0xF),
        Inst::mac(MacOp::Add, Some(Operand::r(0)), None, Some(Operand::c(97))).to_out(
            false,
            OutReg::Pos,
            0xF,
        ),
        Inst::mac(MacOp::Mov, Some(Operand::v(9)), None, None).to_out(false, OutReg::T(0), 0xF),
        Inst::mac(MacOp::Mov, Some(Operand::v(3)), None, None)
            .to_out(false, OutReg::D0, 0xF)
            .last(),
    ]);
    m.write(FUNCTION, &code).unwrap();
    d.load_vertex_shader_program(&mut m, FUNCTION, 0).unwrap();
    for reg in 0..16u32 {
        let (offset, format) = match reg {
            0 => (0, 0x42),
            3 => (16, 0x40),
            9 => (20, 0x22),
            _ => (0, 0x02),
        };
        for (j, w) in [0, offset, format, 0].iter().enumerate() {
            m.write_u32(VAF + 16 * reg + 4 * j as u32, *w).unwrap();
        }
    }
    d.select_vertex_shader_direct(&mut m, VAF, 0).unwrap();

    let mut rng = Rng(1);
    for i in 0..VBS {
        let bytes: Vec<u8> = (0..VB_VERTS * VB_STRIDE)
            .map(|_| rng.next() as u8)
            .collect();
        m.write(0x8000_0000 | vb_data(i), &bytes).unwrap();
        m.write_u32(vb_header(i), 1 | 0x0100_0000).unwrap();
        m.write_u32(vb_header(i) + 4, vb_data(i)).unwrap();
        m.write_u32(vb_header(i) + 8, 0).unwrap();
    }
    for i in 0..TEXTURES {
        let h = tex_header(i);
        m.write_u32(h, 1 | (4 << 16) | 0x0100_0000).unwrap();
        m.write_u32(h + 4, tex_data(i)).unwrap();
        m.write_u32(h + 8, 0).unwrap();
        m.write_u32(h + 12, (2 << 4) | (0x12 << 8) | (1 << 16))
            .unwrap();
        m.write_u32(h + 16, 63 | (63 << 12) | (3 << 24)).unwrap();
        let bytes: Vec<u8> = (0..TEX_SIZE).map(|_| rng.next() as u8).collect();
        m.write(0x8000_0000 | tex_data(i), &bytes).unwrap();
    }
    let mut idx = Vec::new();
    for t in 0..TRIS * 3 {
        let v = (t * 7 + t / 3) % VB_VERTS;
        idx.extend_from_slice(&(v as u16).to_le_bytes());
    }
    m.write(INDICES, &idx).unwrap();

    let def = shaders::psh::PixelShaderDef {
        combiner_count: 1,
        ..Default::default()
    };
    m.write(PSDEF, &def.to_bytes()).unwrap();
    for (i, w) in [1, 0, PSDEF].iter().enumerate() {
        m.write_u32(PS_OBJECT + 4 * i as u32, *w).unwrap();
    }
    d.set_pixel_shader(&mut m, PS_OBJECT).unwrap();
    for i in 0..8 {
        m.write_u32(CONSTS + 4 * i, (i as f32).to_bits()).unwrap();
    }
    (d, m)
}

fn bind_vb<B: Backend>(d: &mut Device<B>, m: &mut SparseMemory, vb: u32) {
    for (i, w) in [vb_header(vb), VB_STRIDE, 0].iter().enumerate() {
        m.write_u32(STREAM_INPUTS + 4 * i as u32, *w).unwrap();
    }
    let handle = d3d::GuestLayout::HALO2.static_vertex_shader | 1;
    d.set_vertex_shader_input(m, handle, 1, STREAM_INPUTS)
        .unwrap();
}

/// Inline state pokes, the way game code stores them.
fn push_state<B: Backend>(d: &mut Device<B>, m: &mut SparseMemory, words: &[u32]) {
    let dev = d3d::GuestLayout::HALO2.device;
    let mut put = m.read_u32(dev).unwrap();
    let threshold = m.read_u32(dev + 4).unwrap();
    if put + 4 * words.len() as u32 >= threshold {
        put = d.make_requested_space(m, 0x100, 0x100).unwrap();
    }
    for w in words {
        m.write_u32(put, *w).unwrap();
        put += 4;
    }
    m.write_u32(dev, put).unwrap();
}

fn frame<B: Backend>(d: &mut Device<B>, m: &mut SparseMemory, draws: u32, f: u32) {
    let hdr = |method: u32, count: u32| (count << 18) | method;
    let mut rng = Rng(f as u64 + 7);
    d.clear(m, 0, 0, 0xF0, 0xFFFF_FFFF, 1.0, 0).unwrap();
    bind_vb(d, m, 0);
    for i in 0..draws {
        let r = rng.next();
        // Texture rebinding: stage 0 every draw, stage 1 on a third.
        d.set_texture(m, 0, tex_header((i * 5 + r % 3) % TEXTURES))
            .unwrap();
        if i % 3 == 0 {
            d.set_texture(m, 1, tex_header((i * 11) % TEXTURES))
                .unwrap();
        }
        if i % 2 == 0 {
            d.set_vertex_shader_constant(m, 96, CONSTS, 2, true)
                .unwrap();
        }
        if i % 5 == 0 {
            d.set_vertex_shader_constant(m, 100 + i % 40, CONSTS, 1, true)
                .unwrap();
        }
        // Inline state through the push buffer (zfunc, blend, zwrite).
        push_state(
            d,
            m,
            &[
                hdr(0x0354, 1),
                1 + (r & 7),
                hdr(0x0304, 1),
                r >> 3 & 1,
                hdr(0x035C, 1),
                r >> 4 & 1,
            ],
        );
        if i % 6 == 0 {
            d.set_render_state(m, RS_ZFUNC, 1 + (r >> 5 & 7)).unwrap();
        }
        if i % 9 == 0 {
            bind_vb(d, m, r % VBS);
        }
        if i % 8 == 7 {
            d.draw_vertices(m, TRIANGLESTRIP, 0, 4 + r % 60).unwrap();
        } else {
            d.draw_indexed_vertices(m, 5, TRIS * 3, INDICES).unwrap();
        }
    }
    d.swap(m, 0).unwrap();
}

/// Runs `frames` timed frames after a warm-up; returns the total and the
/// sorted per-frame times (seconds).
fn run<B: Backend>(
    d: &mut Device<B>,
    m: &mut SparseMemory,
    frames: u32,
    draws: u32,
) -> (f64, Vec<f64>) {
    for f in 0..5 {
        frame(d, m, draws, f);
    }
    let start = Instant::now();
    let mut times = Vec::new();
    for f in 0..frames {
        let t = Instant::now();
        frame(d, m, draws, f);
        times.push(t.elapsed().as_secs_f64());
        if std::env::var_os("H2_BENCH_TRACE").is_some() {
            eprintln!("frame {f}: {:.2} ms", times[times.len() - 1] * 1e3);
        }
    }
    let total = start.elapsed().as_secs_f64();
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

/// Guest memory as the game's D3D entries reach it: `GuestAdapter` over the
/// runtime's `Guest`, with many mappings (so the mapping hints miss as they
/// do in the game) and the d3d write watch over all of RAM. Times the
/// small reads and writes the device makes (words, resource headers, the
/// state arrays) and a 64 KiB data read, without and with the fast page
/// table the game's generated code builds (`Guest::prepare_fast_pages`).
fn guest_memory_bench(fast: bool) {
    use d3d::runtime_glue::GuestAdapter;
    use runtime::guest::{PAGE_SIZE, Permissions, RAM_SIZE};
    const MAPS: u32 = 256;
    const SPAN: u32 = 16 * PAGE_SIZE;
    let mut g = runtime::Guest::new();
    // Every other 64 KiB slot mapped, each to its own physical pages.
    for i in 0..MAPS {
        g.map(0x0100_0000 + 2 * SPAN * i, SPAN, SPAN * i, Permissions::RW)
            .unwrap();
    }
    let id = g.subscribe_writes("bench");
    g.watch_writes(id, runtime::watch::CONTIGUOUS_BASE, RAM_SIZE as u32)
        .unwrap();
    if fast {
        g.prepare_fast_pages();
    }
    let mut seed = 0x2545_f491u32;
    let addrs: Vec<u32> = (0..4096)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (0x0100_0000 + 2 * SPAN * (seed % MAPS) + (seed >> 8) % (SPAN - 0x800)) & !3
        })
        .collect();
    let mut layout = d3d::GuestLayout::HALO2;
    layout.render_state = addrs[0] & !0xFFF;
    layout.texture_state = (addrs[1] & !0xFFF) + 0x400;
    let mem = GuestAdapter::new(&mut g);
    let time = |name: &str, n: usize, f: &mut dyn FnMut()| {
        let mut best = f64::MAX;
        for _ in 0..7 {
            let t = Instant::now();
            f();
            best = best.min(t.elapsed().as_secs_f64());
        }
        println!(
            "guest memory ({}): {name}: {:.1} ns/op",
            if fast { "fast pages" } else { "mapping walk" },
            best * 1e9 / n as f64
        );
    };
    let mut sink = 0u64;
    let mut mem = mem;
    time("read_u32", 100 * addrs.len(), &mut || {
        for _ in 0..100 {
            for &a in &addrs {
                sink = sink.wrapping_add(mem.read_u32(a).unwrap() as u64);
            }
        }
    });
    time("write_u32", 100 * addrs.len(), &mut || {
        for i in 0..100u32 {
            for &a in &addrs {
                mem.write_u32(a, i).unwrap();
            }
        }
    });
    time("20-byte header", 100 * addrs.len(), &mut || {
        for _ in 0..100 {
            for &a in &addrs {
                let w = d3d::guest::read_array::<5>(&mem, a).unwrap();
                sink = sink.wrapping_add(w[4] as u64);
            }
        }
    });
    time("state arrays", 10_000, &mut || {
        for _ in 0..10_000 {
            let s = d3d::state::StateWords::read(&mem, &layout).unwrap();
            sink = sink.wrapping_add(s.render[3] as u64);
        }
    });
    time("64 KiB read_vec", 2_000, &mut || {
        for i in 0..2_000u32 {
            let v = mem
                .read_vec(0x0100_0000 + 2 * SPAN * (i % MAPS), SPAN as usize)
                .unwrap();
            sink = sink.wrapping_add(v[17] as u64);
        }
    });
    std::hint::black_box(sink);
}

fn main() {
    if std::env::var_os("H2_BENCH_GUEST_ONLY").is_some() {
        guest_memory_bench(false);
        guest_memory_bench(true);
        return;
    }
    let env = |k, v: u32| {
        std::env::var(k)
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(v)
    };
    let frames = env("H2_BENCH_FRAMES", 200);
    let draws = env("H2_BENCH_DRAWS", 1200);
    let (mut d, mut m) = setup(NullBackend::default());
    let (total, times) = run(&mut d, &mut m, frames, draws);
    report("pushbuf bench", frames, draws, total, &times);
    let stats = d.stats();
    println!(
        "backend draws {} sink {}  uploads {} hits {}",
        d.backend.draws, d.backend.sink, stats.texture_uploads, stats.texture_hits
    );

    // The guest's side of the render thread: the frame-in-flight limit
    // waits for the render thread only when it falls behind.
    let (mut d, mut m) = setup(ThreadedBackend::new(NullBackend::default()).unwrap());
    let (total, times) = run(&mut d, &mut m, frames, draws);
    report(
        "pushbuf bench, render thread (guest side)",
        frames,
        draws,
        total,
        &times,
    );
    let w = d.backend.waits();
    let (n, sink) = d.backend.with_inner(|b| (b.draws, b.sink)).unwrap();
    println!(
        "backend draws {n} sink {sink}  throttles {} ({:.1} ms)  render thread busy {:.1} ms",
        w.throttles,
        w.throttle_wait.as_secs_f64() * 1e3,
        w.busy.as_secs_f64() * 1e3,
    );

    // The same with the real shader judge, as the game runs it: the
    // guest's side decides each draw's outcome before queueing it.
    let (mut d, mut m) = setup(ThreadedBackend::new(NullBackend::judged()).unwrap());
    let (total, times) = run(&mut d, &mut m, frames, draws);
    report(
        "pushbuf bench, render thread with the shader judge (guest side)",
        frames,
        draws,
        total,
        &times,
    );
    let skips = d
        .take_events()
        .iter()
        .filter(|e| matches!(e, d3d::Event::DrawSkipped { .. }))
        .count();
    println!("skipped draws {skips}  misses {}", d.backend.misses());

    guest_memory_bench(false);
    guest_memory_bench(true);
}
