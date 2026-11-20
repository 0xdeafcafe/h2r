//! A synthetic, retail-free game frame for the draw path on the guest's
//! thread: shared by `tests/draw_sharing.rs` (the same backend calls with
//! `Device::set_draw_sharing` on and off) and `benches/draw_main.rs`
//! (guest-side microseconds per draw).
//!
//! Each frame has `draws` draws, mostly indexed, with what a busy game
//! frame does between them: texture rebinding, vertex constants, inline
//! state through the push buffer on some draws and `SetRenderState` on
//! others (so some draws read the same state arrays as the draw before and
//! some do not), a vertex buffer switch every few draws, and a switch
//! between two vertex programs (`A, B, A, ...`) every few draws.

#![allow(dead_code)]

use super::common::*;
use d3d::backend::{DrawOutcome, ResolvedDraw};
use d3d::{Backend, Device, GuestMemory, SparseMemory, UploadPolicy};
use gfx::texture::DecodedTexture;
use gfx::{Clear, D3DFormat};
use shaders::vsh::encode::{Inst, Operand, program};
use shaders::vsh::{MacOp, OutReg};

/// Counts draws and touches the parts of the draw a real backend reads.
/// With `judged`, the real shader judge (`Nv2aJudge`, what `GfxBackend`
/// judges with) answers each draw, so behind the render thread the guest's
/// side pays for deciding each draw's outcome as it does in the game.
#[derive(Default)]
pub struct NullBackend {
    next: u32,
    pub draws: u64,
    pub sink: u64,
    judged: bool,
    verdicts: Option<Box<dyn d3d::shader::ShaderJudge>>,
}

impl NullBackend {
    pub fn judged() -> Self {
        Self {
            judged: true,
            ..Self::default()
        }
    }
}

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

/// `GfxJudge`'s provider half: every view sampleable, every draw routed
/// to the provider.
struct Judge(Box<dyn d3d::shader::ShaderJudge>);

impl d3d::backend::DrawJudge for Judge {
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
        self.judged.then(|| Box::new(Judge(nv2a_judge())) as _)
    }
    fn present(&mut self, _: u32, _: u32) -> Result<(), String> {
        Ok(())
    }
}

const VAF: u32 = MISC + 0x3000;
const STREAM_INPUTS: u32 = MISC + 0x3200;
const FUNCTION_A: u32 = MISC + 0x3400;
const FUNCTION_B: u32 = MISC + 0x3600;
const PSDEF: u32 = MISC + 0x3800;
const PS_OBJECT: u32 = MISC + 0x3900;
const CONSTS: u32 = MISC + 0x3A00;
const INDICES: u32 = MISC + 0x4000;

/// Program slots of the two vertex programs.
const PROGRAM_A: u32 = 0;
const PROGRAM_B: u32 = 8;

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

/// A device on `backend` with `sharing` set, the scene's resources in
/// guest memory and program A selected.
pub fn setup<B: Backend>(backend: B, sharing: bool) -> (Device<B>, SparseMemory) {
    let (mut d, mut m) = create(backend, UploadPolicy::Tracked);
    d.set_draw_sharing(sharing);
    m.map(0x8000_0000 | 0x0040_0000, TEX_SIZE * TEXTURES);
    m.map(0x8000_0000 | 0x0030_0000, VB_VERTS * VB_STRIDE * VBS);
    set_states(&mut d, &mut m);

    // Two programs that differ in one constant register.
    for (function, slot, c) in [(FUNCTION_A, PROGRAM_A, 96), (FUNCTION_B, PROGRAM_B, 98)] {
        let code = program(&[
            Inst::mac(MacOp::Mul, Some(Operand::v(0)), Some(Operand::c(c)), None).to_temp(0, 0xF),
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
        m.write(function, &code).unwrap();
        d.load_vertex_shader_program(&mut m, function, slot)
            .unwrap();
    }
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
    d.select_vertex_shader_direct(&mut m, VAF, PROGRAM_A)
        .unwrap();

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

/// One frame `f` of `draws` draws, ending in `Swap`.
pub fn frame<B: Backend>(d: &mut Device<B>, m: &mut SparseMemory, draws: u32, f: u32) {
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
        // Inline state through the push buffer (zfunc, blend, zwrite) on
        // a quarter of the draws; the rest draw with the arrays as the
        // draw before left them, or after a SetRenderState.
        if i % 4 == 1 {
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
        }
        if i % 6 == 0 {
            d.set_render_state(m, RS_ZFUNC, 1 + (r >> 5 & 7)).unwrap();
        }
        if i % 9 == 0 {
            bind_vb(d, m, r % VBS);
        }
        // A, B, A, ... every few draws (material changes).
        if i % 3 == 2 {
            let slot = if (i / 3) % 2 == 0 {
                PROGRAM_B
            } else {
                PROGRAM_A
            };
            d.select_vertex_shader_direct(m, 0, slot).unwrap();
        }
        if i % 8 == 7 {
            d.draw_vertices(m, TRIANGLESTRIP, 0, 4 + r % 60).unwrap();
        } else {
            d.draw_indexed_vertices(m, 5, TRIS * 3, INDICES).unwrap();
        }
    }
    d.swap(m, 0).unwrap();
}
