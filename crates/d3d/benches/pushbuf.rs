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
//! `cargo bench -p d3d --bench pushbuf` prints ms per frame. Env:
//! `H2_BENCH_FRAMES` (default 200), `H2_BENCH_DRAWS` (default 1200).

#[path = "../tests/common/mod.rs"]
mod common;

use std::time::Instant;

use common::*;
use d3d::backend::{DrawOutcome, ResolvedDraw};
use d3d::{Backend, Device, GuestMemory, SparseMemory, UploadPolicy};
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

fn setup() -> (Device<NullBackend>, SparseMemory) {
    let (mut d, mut m) = create(NullBackend::default(), UploadPolicy::Tracked);
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

fn bind_vb(d: &mut Device<NullBackend>, m: &mut SparseMemory, vb: u32) {
    for (i, w) in [vb_header(vb), VB_STRIDE, 0].iter().enumerate() {
        m.write_u32(STREAM_INPUTS + 4 * i as u32, *w).unwrap();
    }
    let handle = d3d::GuestLayout::HALO2.static_vertex_shader | 1;
    d.set_vertex_shader_input(m, handle, 1, STREAM_INPUTS)
        .unwrap();
}

/// Inline state pokes, the way game code stores them.
fn push_state(d: &mut Device<NullBackend>, m: &mut SparseMemory, words: &[u32]) {
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

fn frame(d: &mut Device<NullBackend>, m: &mut SparseMemory, draws: u32, f: u32) {
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

fn main() {
    let env = |k, v: u32| {
        std::env::var(k)
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(v)
    };
    let frames = env("H2_BENCH_FRAMES", 200);
    let draws = env("H2_BENCH_DRAWS", 1200);
    let (mut d, mut m) = setup();
    for f in 0..5 {
        frame(&mut d, &mut m, draws, f);
    }
    let start = Instant::now();
    let mut times = Vec::new();
    for f in 0..frames {
        let t = Instant::now();
        frame(&mut d, &mut m, draws, f);
        times.push(t.elapsed().as_secs_f64());
        if std::env::var_os("H2_BENCH_TRACE").is_some() {
            eprintln!("frame {f}: {:.2} ms", times[times.len() - 1] * 1e3);
        }
    }
    let total = start.elapsed().as_secs_f64();
    times.sort_by(f64::total_cmp);
    let (median, p10) = (times[times.len() / 2], times[times.len() / 10]);
    let stats = d.stats();
    println!(
        "pushbuf bench: {frames} frames x {draws} draws: {:.3} ms/frame mean, {:.3} median, {:.3} p10, {:.2} us/draw (mean)",
        total * 1e3 / frames as f64,
        median * 1e3,
        p10 * 1e3,
        total * 1e6 / (frames as f64 * draws as f64),
    );
    println!(
        "backend draws {} sink {}  uploads {} hits {}",
        d.backend.draws, d.backend.sink, stats.texture_uploads, stats.texture_hits
    );
}
