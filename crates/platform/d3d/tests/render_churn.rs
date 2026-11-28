//! Render-thread churn of a level-like frame through the device and
//! `GfxBackend`: the frame switches between the back buffer and a render
//! target texture many times (every switch submits the pending pass),
//! every draw changes its pixel uniforms, every frame draws vertex windows
//! the vertex cache has not seen before (each becomes a snapshot with a GPU
//! buffer of its own, as geometry streaming in does), one vertex buffer is
//! rewritten every frame, and `Swap` asks for `D3DSWAP_FINISH`. Every resource
//! is synthetic.
//!
//! ```sh
//! cargo bench -p d3d --bench render_churn
//! ```
//!
//! prints the render-thread time per frame and the counters
//! (`H2_BENCH_FRAMES`, default 120).

mod common;

use std::time::{Duration, Instant};

use common::*;
use d3d::layout::device_field as f;
use d3d::{Device, GfxBackend, GuestMemory, SparseMemory, UploadPolicy};
use gfx::GfxContext;
use gfx::vertex::fvf;

const DEV: u32 = 0x0040_4FE0;
const RS_TEXTUREFACTOR: u32 = 148;
/// Target switches per frame (each to the texture and back).
const SWITCHES: u32 = 16;
/// Draws per visit of a target.
const DRAWS: u32 = 3;
/// The rewritten vertex buffer (physical; seen at `0x8000_0000 | pa`).
const DYN_DATA: u32 = 0x0040_0000;
const DYN_HEADER: u32 = MISC + 0x1180;
const STRIDE: u32 = 20;
/// `D3DSWAP_FINISH`.
const SWAP_FINISH: u32 = 4;
/// The static buffer: [`COPIES`] copies of the tiles (frame `f` draws copy
/// `f % COPIES`, so every frame reads vertex windows the cache has not
/// seen: geometry streaming in), then the texture's halves.
const STATIC_DATA: u32 = 0x0050_0000;
const COPIES: u32 = 256;

/// [`COPIES`] times the 16 tiles of the 64x64 back buffer, then the two
/// halves of the 16x4 texture.
fn static_rects() -> Vec<(f32, f32, f32, f32)> {
    let mut r: Vec<_> = (0..16 * COPIES)
        .map(|t| {
            let t = t % 16;
            let (x, y) = ((t % 4) as f32 * 16.0, (t / 4) as f32 * 16.0);
            (x, y, x + 16.0, y + 16.0)
        })
        .collect();
    r.push((0.0, 0.0, 8.0, 4.0));
    r.push((8.0, 0.0, 16.0, 4.0));
    r.push((0.0, 0.0, BIG.0 as f32, BIG.1 as f32));
    r
}

/// The bench's game-sized back buffer (Halo's 640x480 at 4x).
const BIG: (u32, u32) = (2560, 1920);
/// The quad covering all of a [`BIG`] back buffer.
const FULL: u32 = 16 * COPIES + 2;

/// Frame `f`'s rewritten buffer: an inset of each tile, moved every frame.
fn dyn_rects(f: u32) -> Vec<(f32, f32, f32, f32)> {
    let s = (f % 4) as f32;
    (0..16)
        .map(|t| {
            let (x, y) = ((t % 4) as f32 * 16.0, (t / 4) as f32 * 16.0);
            (x + 2.0 + s, y + 2.0 + s, x + 10.0 + s, y + 10.0 + s)
        })
        .collect()
}

fn strip_bytes(rects: &[(f32, f32, f32, f32)]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for &(x0, y0, x1, y1) in rects {
        for (x, y) in [(x0, y0), (x1, y0), (x0, y1), (x1, y1)] {
            bytes.extend(vertex(x, y, 0xFFFF_FFFF));
        }
    }
    bytes
}

/// The texture factor of draw `j` of visit `k` in frame `f` (`j == DRAWS`
/// is the rewritten buffer's quad); `rtt` for the texture's draws.
fn color(f: u32, k: u32, j: u32, rtt: bool) -> u32 {
    let h = (f * 7919 + k * 131 + j * 17 + u32::from(rtt) * 3).wrapping_mul(0x9E37_79B1);
    0xFF00_0000 | (h >> 8)
}

/// The 16x4 `LIN_A8R8G8B8` level of the test texture, as the target.
fn texture_target(m: &mut SparseMemory) -> u32 {
    texture(m, 0);
    let surf = MISC + 0x7200;
    let mut h =
        d3d::resource::SurfaceHeader::linear(gfx::D3DFormat::LinA8R8G8B8, 16, 4, 64, TEX_DATA);
    h.parent = TEX_HEADER;
    h.write(m, surf).unwrap();
    surf
}

struct Scene {
    d: Device<GfxBackend>,
    m: SparseMemory,
    back: u32,
    rtt: u32,
}

fn scene(configure: impl FnOnce(&mut GfxBackend)) -> Option<Scene> {
    scene_sized((W, H), configure)
}

/// [`scene`] with a `size` back buffer.
fn scene_sized(size: (u32, u32), configure: impl FnOnce(&mut GfxBackend)) -> Option<Scene> {
    let ctx = match GfxContext::headless() {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("skipping GPU test: {e}");
            return None;
        }
    };
    let mut b = GfxBackend::new(ctx);
    configure(&mut b);
    let (mut d, mut m) = create_with(b, UploadPolicy::Tracked, |p| {
        p.width = size.0;
        p.height = size.1;
    });
    d.set_vertex_cache(Some(1 << 20));
    set_states(&mut d, &mut m);
    // Stage 0 outputs the texture factor: every draw goes through the
    // shader provider, with its colour in the pixel uniforms.
    let l = d3d::GuestLayout::HALO2;
    m.write_u32(l.texture_state_slot(0, 14), 3).unwrap(); // COLORARG1 = TFACTOR
    m.write_u32(l.texture_state_slot(0, 12), 2).unwrap(); // COLOROP = SELECTARG1
    m.write_u32(l.texture_state_slot(0, 18), 3).unwrap(); // ALPHAARG1 = TFACTOR
    m.write_u32(l.texture_state_slot(0, 13), 2).unwrap(); // ALPHAOP = SELECTARG1
    let bytes = strip_bytes(&static_rects());
    m.map(
        0x8000_0000 | STATIC_DATA,
        (bytes.len() as u32).next_multiple_of(0x1000),
    );
    m.write(0x8000_0000 | STATIC_DATA, &bytes).unwrap();
    m.write_u32(VB_HEADER, 1 | 0x0100_0000).unwrap();
    m.write_u32(VB_HEADER + 4, STATIC_DATA).unwrap();
    m.write_u32(VB_HEADER + 8, 0).unwrap();
    m.map(0x8000_0000 | DYN_DATA, 0x1000);
    m.write_u32(DYN_HEADER, 1 | 0x0100_0000).unwrap();
    m.write_u32(DYN_HEADER + 4, DYN_DATA).unwrap();
    m.write_u32(DYN_HEADER + 8, 0).unwrap();
    d.set_vertex_shader(&mut m, fvf::XYZRHW | fvf::DIFFUSE)
        .unwrap();
    let back = m.read_u32(DEV + f::RENDER_TARGET).unwrap();
    let rtt = texture_target(&mut m);
    d.set_render_target(&mut m, rtt, 0).unwrap();
    d.set_render_target(&mut m, back, 0).unwrap();
    Some(Scene { d, m, back, rtt })
}

impl Scene {
    fn tfactor(&mut self, c: u32) {
        self.d
            .set_render_state(&mut self.m, RS_TEXTUREFACTOR, c)
            .unwrap();
    }

    /// Frame `f`, ending in `Swap(flags)`.
    fn frame(&mut self, f: u32, flags: u32) {
        let bytes = strip_bytes(&dyn_rects(f));
        self.m.write(0x8000_0000 | DYN_DATA, &bytes).unwrap();
        self.d
            .note_write(0x8000_0000 | DYN_DATA, bytes.len() as u32);
        self.d
            .clear(&mut self.m, 0, 0, 0xF0, 0xFF00_0000, 1.0, 0)
            .unwrap();
        for k in 0..SWITCHES {
            self.d.set_render_target(&mut self.m, self.rtt, 0).unwrap();
            self.d
                .set_stream_source(&mut self.m, 0, VB_HEADER, STRIDE)
                .unwrap();
            for j in 0..DRAWS {
                self.tfactor(color(f, k, j, true));
                self.d
                    .draw_vertices(
                        &mut self.m,
                        TRIANGLESTRIP,
                        4 * (16 * COPIES + (k + j) % 2),
                        4,
                    )
                    .unwrap();
            }
            self.d.set_render_target(&mut self.m, self.back, 0).unwrap();
            for j in 0..DRAWS {
                self.tfactor(color(f, k, j, false));
                self.d
                    .draw_vertices(&mut self.m, TRIANGLESTRIP, 4 * (16 * (f % COPIES) + k), 4)
                    .unwrap();
            }
            self.d
                .set_stream_source(&mut self.m, 0, DYN_HEADER, STRIDE)
                .unwrap();
            self.tfactor(color(f, k, DRAWS, false));
            self.d
                .draw_vertices(&mut self.m, TRIANGLESTRIP, 4 * k, 4)
                .unwrap();
        }
        self.d.swap(&mut self.m, flags).unwrap();
        let events = self.d.take_events();
        assert!(
            events
                .iter()
                .all(|e| !matches!(e, d3d::Event::DrawSkipped { .. } | d3d::Event::BadState(_))),
            "{events:?}"
        );
    }

    /// A game-like frame `f` on a [`BIG`] back buffer: each of the
    /// [`SWITCHES`] visits draws into the texture, reads it back on every
    /// other visit (a render-target writeback: the render thread waits for
    /// the GPU), then fills the back buffer `fills` times. The GPU has real
    /// work, so when it starts matters.
    fn game_frame(&mut self, f: u32, fills: u32, small: u32) {
        self.d
            .clear(&mut self.m, 0, 0, 0xF0, 0xFF00_0000, 1.0, 0)
            .unwrap();
        self.d
            .set_stream_source(&mut self.m, 0, VB_HEADER, STRIDE)
            .unwrap();
        for k in 0..SWITCHES {
            self.d.set_render_target(&mut self.m, self.rtt, 0).unwrap();
            for j in 0..DRAWS {
                self.tfactor(color(f, k, j, true));
                self.d
                    .draw_vertices(
                        &mut self.m,
                        TRIANGLESTRIP,
                        4 * (16 * COPIES + (k + j) % 2),
                        4,
                    )
                    .unwrap();
            }
            self.d.set_render_target(&mut self.m, self.back, 0).unwrap();
            if k % 2 == 1 {
                self.d.block_on_resource(&mut self.m, TEX_HEADER).unwrap();
            }
            for j in 0..fills {
                self.tfactor(color(f, k, j, false));
                self.d
                    .draw_vertices(&mut self.m, TRIANGLESTRIP, 4 * FULL, 4)
                    .unwrap();
            }
            for j in 0..small {
                self.tfactor(color(f, k, j, false));
                self.d
                    .draw_vertices(
                        &mut self.m,
                        TRIANGLESTRIP,
                        4 * (16 * (f % COPIES) + j % 16),
                        4,
                    )
                    .unwrap();
            }
        }
        self.d.swap(&mut self.m, SWAP_FINISH).unwrap();
        self.d.take_events();
    }
}

/// CPU time this thread has used.
fn thread_cpu() -> Duration {
    #[repr(C)]
    struct Timespec {
        sec: i64,
        nsec: i64,
    }
    unsafe extern "C" {
        fn clock_gettime(clock: i32, tp: *mut Timespec) -> i32;
    }
    #[cfg(target_os = "macos")]
    const CLOCK_THREAD_CPUTIME_ID: i32 = 16;
    #[cfg(not(target_os = "macos"))]
    const CLOCK_THREAD_CPUTIME_ID: i32 = 3;
    let mut t = Timespec { sec: 0, nsec: 0 };
    // SAFETY: `t` is a valid out-pointer for the call.
    if unsafe { clock_gettime(CLOCK_THREAD_CPUTIME_ID, &mut t) } != 0 {
        return Duration::ZERO;
    }
    Duration::new(t.sec as u64, t.nsec as u32)
}

/// Per-frame counters over `frames` frames.
#[derive(Debug, Default, Clone, Copy)]
struct PerFrame {
    buffers_created: f64,
    queue_writes: f64,
    staging_reused: f64,
    buffers_reused: f64,
    submits: f64,
    render_passes: f64,
    swap_waits: f64,
    /// Wall time.
    time: Duration,
    /// This thread's CPU time (the render thread's work: the GPU waits
    /// are not in it).
    cpu: Duration,
}

/// Runs frames `from..from + frames` with `D3DSWAP_FINISH` and returns
/// the counters per frame.
fn measure(s: &mut Scene, from: u32, frames: u32) -> PerFrame {
    let churn = gfx::churn::snapshot();
    let batch = s.d.backend.batch_stats();
    let t = Instant::now();
    let cpu = thread_cpu();
    for f in from..from + frames {
        s.frame(f, SWAP_FINISH);
    }
    let time = t.elapsed() / frames;
    let cpu = (thread_cpu() - cpu) / frames;
    let c = gfx::churn::snapshot() - churn;
    let b = s.d.backend.batch_stats();
    let per = |v: u64| v as f64 / f64::from(frames);
    PerFrame {
        buffers_created: per(c.buffers_created),
        queue_writes: per(c.queue_writes),
        staging_reused: per(c.staging_reused),
        buffers_reused: per(c.buffers_reused),
        submits: per(b.submits - batch.submits),
        render_passes: per(b.render_passes - batch.render_passes),
        swap_waits: per(b.swap_waits - batch.swap_waits),
        time,
        cpu,
    }
}

#[test]
#[ignore = "benchmark: run with --release -- --ignored --nocapture"]
fn render_churn() {
    bench();
}

pub fn bench() {
    let frames = std::env::var("H2_BENCH_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(120);
    for (name, finish) in [
        ("full wait", d3d::SwapFinish::Full),
        ("pipelined (default)", d3d::SwapFinish::Pipelined),
    ] {
        let Some(mut s) = scene(|b| b.set_swap_finish(finish)) else {
            return;
        };
        measure(&mut s, 0, 8);
        let p = measure(&mut s, 8, frames);
        println!(
            "{name:<30} {:>7.3} ms/frame (CPU {:>6.3} ms)  buffers created {:>6.1}  queue writes {:>6.1}  staging reused {:>6.1}  buffers reused {:>5.1}  submits {:>5.1}  render passes {:>5.1}  swap waits {:.1}",
            p.time.as_secs_f64() * 1e3,
            p.cpu.as_secs_f64() * 1e3,
            p.buffers_created,
            p.queue_writes,
            p.staging_reused,
            p.buffers_reused,
            p.submits,
            p.render_passes,
            p.swap_waits,
        );
    }
    game_bench(frames);
}

/// [`Scene::game_frame`]: wall time per frame is what counts here, the GPU
/// and the render thread overlapping (or not).
fn game_bench(frames: u32) {
    let fills = std::env::var("H2_BENCH_FILLS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4);
    let small = std::env::var("H2_BENCH_SMALL")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let rounds = std::env::var("H2_BENCH_ROUNDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    println!(
        "game-like frame, {}x{}, {fills} fills and {small} small draws per visit:",
        BIG.0, BIG.1
    );
    let mut times = Vec::new();
    // Several rounds, the median: the machine is shared.
    for round in 0..rounds {
        let Some(mut s) = scene_sized(BIG, |_| {}) else {
            return;
        };
        for f in 0..8 {
            s.game_frame(f, fills, small);
        }
        let submits = s.d.backend.batch_stats().submits;
        let t = Instant::now();
        let cpu = thread_cpu();
        for f in 8..8 + frames {
            s.game_frame(f, fills, small);
        }
        let time = t.elapsed() / frames;
        let cpu = (thread_cpu() - cpu) / frames;
        let submits = (s.d.backend.batch_stats().submits - submits) as f64 / f64::from(frames);
        times.push(time);
        eprintln!(
            "  round {round} {:>7.3} ms/frame (CPU {:>6.3} ms)  submits {submits:>5.1}",
            time.as_secs_f64() * 1e3,
            cpu.as_secs_f64() * 1e3,
        );
    }
    times.sort();
    println!(
        "  median {:>7.3} ms/frame, best {:>7.3}",
        times[times.len() / 2].as_secs_f64() * 1e3,
        times[0].as_secs_f64() * 1e3
    );
}
