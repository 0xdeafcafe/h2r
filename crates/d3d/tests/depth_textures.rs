//! Depth as a texture: depth/stencil surfaces that are levels of a texture
//! (render-to-texture shadow maps, the first campaign level's
//! SetRenderTarget), those textures sampled afterwards, and colour textures
//! over the device's own depth buffer (the main menu's depth-based pass,
//! Halo's `0x509368`/`0x50936C` headers). Synthetic headers and draws on
//! the recording backend, and pixels on a real GPU (skipped without an
//! adapter).

mod common;

use common::*;
use d3d::device::DEPTH_BUFFER_DATA;
use d3d::resource::SurfaceHeader;
use d3d::{
    D3dError, Device, Event, GfxBackend, GuestLayout, GuestMemory, Recorded, RecordingBackend,
    SparseMemory, UploadPolicy,
};
use gfx::surface::HostPixelOrder;
use gfx::vertex::fvf;
use gfx::{D3DFormat, GfxContext};

const L: GuestLayout = GuestLayout::HALO2;
/// The depth texture (the level's `Parent`).
const DTEX: u32 = MISC + 0x7800;
/// Its level 0, bound as `pNewZStencil`.
const DLEVEL: u32 = MISC + 0x7820;
/// A free-standing 16x16 colour target to pair it with.
const CSURF: u32 = MISC + 0x7840;
/// A colour texture over the depth buffer.
const ALIAS: u32 = MISC + 0x7860;
/// The depth texture's texels.
const DDATA: u32 = 0x0021_0000;
const S: u32 = 16;

const RS_ZWRITEENABLE: u32 = 64;
const RS_ZENABLE: u32 = 143;

/// A 2D one-level depth texture at `DTEX` and its level at `DLEVEL`:
/// swizzled (`D24S8` 0x2A, `D16` 0x2C) or linear with rows `pitch` apart.
fn depth_texture(m: &mut SparseMemory, format: u32, pitch: Option<u32>) {
    let (fmt_word, size) = match pitch {
        None => (
            (2 << 4) | (format << 8) | (1 << 16) | (4 << 20) | (4 << 24),
            0,
        ),
        Some(p) => (
            (2 << 4) | (format << 8) | (1 << 16),
            (S - 1) | ((S - 1) << 12) | ((p / 64 - 1) << 24),
        ),
    };
    for (i, w) in [0x0004_0001, DDATA, 0, fmt_word, size].iter().enumerate() {
        m.write_u32(DTEX + 4 * i as u32, *w).unwrap();
    }
    let mut h = SurfaceHeader::linear(D3DFormat::LinD24S8, S, S, 64, DDATA);
    h.pixels.format = fmt_word;
    h.pixels.size = size;
    h.parent = DTEX;
    h.write(m, DLEVEL).unwrap();
}

fn color_target(m: &mut SparseMemory) {
    SurfaceHeader::linear(D3DFormat::LinA8R8G8B8, S, S, S * 4, 0x0030_0000)
        .write(m, CSURF)
        .unwrap();
}

fn mem() -> (Device<RecordingBackend>, SparseMemory) {
    let (d, mut m) = create(RecordingBackend::new(), UploadPolicy::Tracked);
    m.map(0x8000_0000 | DDATA, 0x1000);
    color_target(&mut m);
    (d, m)
}

fn depth_reads(d: &Device<RecordingBackend>) -> Vec<(u32, D3DFormat, usize)> {
    d.backend
        .calls
        .iter()
        .filter_map(|c| match c {
            Recorded::ReadDepth {
                surface,
                format,
                len,
            } => Some((*surface, *format, *len)),
            _ => None,
        })
        .collect()
}

fn z_writes(m: &mut SparseMemory, on: bool) {
    m.write_u32(L.render_state_slot(RS_ZENABLE), on as u32)
        .unwrap();
    m.write_u32(L.render_state_slot(RS_ZWRITEENABLE), on as u32)
        .unwrap();
}

#[test]
fn depth_levels_render_and_write_back_in_their_format() {
    for (format, pitch, len, d3d_format) in [
        (0x2A, None, 1024, D3DFormat::D24S8),
        (0x2E, Some(64), 1024, D3DFormat::LinD24S8),
        (0x2C, None, 512, D3DFormat::D16),
        // Linear D16 rows padded to 64 bytes.
        (0x30, Some(64), 1024, D3DFormat::LinD16),
    ] {
        let (mut d, mut m) = mem();
        let pattern: Vec<u8> = (0..len).map(|i| i as u8).collect();
        m.write(0x8000_0000 | DDATA, &pattern).unwrap();
        depth_texture(&mut m, format, pitch);
        let back = m.read_u32(L.device + 0x1A04).unwrap();
        d.set_render_target(&mut m, CSURF, DLEVEL).unwrap();
        // A depth surface of the level's format and size, filled with the
        // level's texels.
        let id = d
            .backend
            .calls
            .iter()
            .find_map(|c| match c {
                Recorded::Surface {
                    id,
                    format: f,
                    size: (S, S),
                    depth: true,
                } if *f == d3d_format => Some(*id),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{d3d_format:?}: no depth surface"));
        assert!(
            d.backend.calls.iter().any(
                |c| matches!(c, Recorded::WriteSurface { surface, bytes } if *surface == id && *bytes == pattern)
            ),
            "{d3d_format:?}: filled from guest memory"
        );
        assert_eq!(m.read_u32(L.device + 0x1A08).unwrap(), DLEVEL);
        // Untouched: leaving it writes nothing back.
        d.set_render_target(&mut m, CSURF, 0).unwrap();
        assert!(depth_reads(&d).is_empty());
        // A depth clear dirties it; switching depth away writes it back.
        d.set_render_target(&mut m, CSURF, DLEVEL).unwrap();
        d.clear(&mut m, 0, 0, 0x01, 0, 1.0, 0).unwrap();
        d.backend.depth_readback = Some(vec![0xA1, 0xB2, 0xC3, 0xD4]);
        d.set_render_target(&mut m, back, 0).unwrap();
        assert_eq!(depth_reads(&d), [(id, d3d_format, len)], "{d3d_format:?}");
        assert_eq!(
            m.read_u32(0x8000_0000 | DDATA).unwrap(),
            0xD4C3_B2A1,
            "{d3d_format:?}"
        );
        assert_eq!(d.stats().rtt_writebacks, 1);
        assert_eq!(d.take_events(), []);
    }
}

#[test]
fn draws_dirty_a_depth_level_only_when_they_write_depth_or_stencil() {
    let (mut d, mut m) = mem();
    depth_texture(&mut m, 0x2A, None);
    set_states(&mut d, &mut m);
    quad_vb(&mut m, 0.0, 0.0, S as f32, S as f32, 0xFFFF_FFFF);
    bind_quad(&mut d, &mut m);
    d.set_render_target(&mut m, CSURF, DLEVEL).unwrap();
    z_writes(&mut m, false);
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    d.swap(&mut m, 0).unwrap();
    assert!(
        depth_reads(&d).is_empty(),
        "depth test only: nothing to write back"
    );
    z_writes(&mut m, true);
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    d.swap(&mut m, 0).unwrap();
    assert_eq!(depth_reads(&d).len(), 1, "Swap writes the drawn level back");
}

#[test]
fn a_depth_texture_sampled_after_rendering_reads_the_written_back_level() {
    let (mut d, mut m) = mem();
    depth_texture(&mut m, 0x2A, None);
    set_states(&mut d, &mut m);
    quad_vb(&mut m, 0.0, 0.0, S as f32, S as f32, 0xFFFF_FFFF);
    bind_quad(&mut d, &mut m);
    d.set_render_target(&mut m, CSURF, DLEVEL).unwrap();
    z_writes(&mut m, true);
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    d.backend.depth_readback = Some(vec![0x5A, 0xEF, 0xCD, 0xAB]);
    // Still bound as depth, now also sampled: written back first, then
    // uploaded from guest memory as its raw bits.
    z_writes(&mut m, false);
    d.set_texture(&mut m, 0, DTEX).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    assert_eq!(depth_reads(&d).len(), 1);
    assert_eq!(m.read_u32(0x8000_0000 | DDATA).unwrap(), 0xABCD_EF5A);
    let upload = d
        .backend
        .calls
        .iter()
        .rposition(|c| matches!(c, Recorded::Upload { size: (S, S), .. }));
    let read = d
        .backend
        .calls
        .iter()
        .position(|c| matches!(c, Recorded::ReadDepth { .. }));
    assert!(read < upload, "written back before it is uploaded");
    let draw = *d.backend.draws().last().unwrap();
    let info = draw.texture_info[0].unwrap();
    assert_eq!(
        (info.format, info.width, info.height),
        (D3DFormat::D24S8, S, S)
    );
    assert_eq!(d.take_events(), []);
}

#[test]
fn depth_levels_that_cannot_be_modelled_stop() {
    let (mut d, mut m) = mem();
    // Float depth: its bits are not what the host depth surface holds.
    depth_texture(&mut m, 0x2B, None);
    let e = d.set_render_target(&mut m, CSURF, DLEVEL);
    assert!(
        matches!(&e, Err(D3dError::Unimplemented { detail, .. }) if detail.contains("float depth")),
        "{e:?}"
    );
    // A colour level bound as depth, and a depth level bound as colour.
    let mut h = SurfaceHeader::linear(D3DFormat::LinA8R8G8B8, S, S, 64, DDATA);
    h.parent = DTEX;
    h.write(&mut m, DLEVEL).unwrap();
    assert!(matches!(
        d.set_render_target(&mut m, CSURF, DLEVEL),
        Err(D3dError::Invalid { .. })
    ));
    depth_texture(&mut m, 0x2A, None);
    assert!(matches!(
        d.set_render_target(&mut m, DLEVEL, 0),
        Err(D3dError::Invalid { .. })
    ));
}

/// A colour texture header over the device's depth buffer, as `0x14600`
/// builds them (linear 2D, one level, `w x h`, rows `pitch` apart).
fn alias(m: &mut SparseMemory, data: u32, format: u32, w: u32, h: u32, pitch: u32) {
    // `Common` (with the internal references SetTexture took) is kept.
    let common = match m.read_u32(ALIAS).unwrap() {
        0 => 0x0004_0001,
        c => c,
    };
    for (i, word) in [
        common,
        data,
        0,
        1 | (2 << 4) | (format << 8) | (1 << 16),
        (w - 1) | ((h - 1) << 12) | ((pitch / 64 - 1) << 24),
    ]
    .iter()
    .enumerate()
    {
        m.write_u32(ALIAS + 4 * i as u32, *word).unwrap();
    }
}

fn skipped(d: &mut Device<RecordingBackend>, what: &str) -> bool {
    d.take_events()
        .iter()
        .any(|e| matches!(e, Event::DrawSkipped { reason, .. } if reason.contains(what)))
}

#[test]
fn colour_textures_over_the_depth_buffer_decode_a_readback() {
    let (mut d, mut m) = create(RecordingBackend::new(), UploadPolicy::Always);
    set_states(&mut d, &mut m);
    quad_vb(&mut m, 0.0, 0.0, W as f32, H as f32, 0xFFFF_FFFF);
    bind_quad(&mut d, &mut m);
    // LIN_A8R8G8B8 over the whole buffer, as 0x509368 is.
    alias(&mut m, DEPTH_BUFFER_DATA, 0x12, W, H, W * 4);
    d.set_texture(&mut m, 0, ALIAS).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    assert_eq!(d.take_events(), []);
    let depth = d.backend.calls.iter().find_map(|c| match c {
        Recorded::Surface {
            id, depth: true, ..
        } => Some(*id),
        _ => None,
    });
    assert_eq!(
        depth_reads(&d),
        [(depth.unwrap(), D3DFormat::LinD24S8, (W * H * 4) as usize)]
    );
    let upload = d.backend.calls.iter().find_map(|c| match c {
        Recorded::Upload { id, size } => Some((*id, *size)),
        _ => None,
    });
    assert_eq!(upload.map(|u| u.1), Some((W, H)));
    match d.backend.calls.last() {
        Some(Recorded::Draw { textures, draw, .. }) => {
            assert_eq!(textures[0], upload.map(|u| u.0));
            assert_eq!(draw.texture_info[0].unwrap().format, D3DFormat::LinA8R8G8B8);
        }
        other => panic!("{other:?}"),
    }
    // The depth buffer is unchanged (no depth writes): the same texture,
    // no second readback or upload. The guest memory is never read.
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    assert_eq!(depth_reads(&d).len(), 1);
    assert_eq!(d.backend.uploads(), 1);
    // LIN_A8B8G8R8 over it (0x50936C): a second texture from the same
    // readback.
    alias(&mut m, DEPTH_BUFFER_DATA, 0x3F, W, H, W * 4);
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    assert_eq!((depth_reads(&d).len(), d.backend.uploads()), (1, 2));
    // A depth clear, then a draw that writes depth: read again each time.
    d.clear(&mut m, 0, 0, 0x01, 0, 0.5, 0).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    assert_eq!(depth_reads(&d).len(), 2);
    z_writes(&mut m, true);
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    assert_eq!(
        depth_reads(&d).len(),
        3,
        "the first draw's own write is not read by it"
    );
    z_writes(&mut m, false);
    assert_eq!(d.stats().depth_reads, 3);
    assert_eq!(d.take_events(), []);

    // An offset view inside the buffer is fine; one running past its end
    // skips with a reason.
    alias(&mut m, DEPTH_BUFFER_DATA + W * 4 * 8, 0x12, W, 8, W * 4);
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    assert_eq!(d.take_events(), []);
    alias(&mut m, DEPTH_BUFFER_DATA + W * 4 * 8, 0x12, W, H, W * 4);
    let draws = d.backend.draws().len();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    assert!(skipped(&mut d, "runs past"));
    assert_eq!(d.backend.draws().len(), draws);
    // On a stage the draw does not read it is left unbound.
    alias(&mut m, DEPTH_BUFFER_DATA, 0x12, W, H, W * 4);
    d.set_texture(&mut m, 0, 0).unwrap();
    d.set_texture(&mut m, 3, ALIAS).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    assert_eq!(d.take_events(), []);
    assert_eq!(d.backend.draws().last().unwrap().textures[3], None);
}

/// A backend without depth readback skips the draw instead of stopping.
#[test]
fn without_depth_readback_the_draw_is_skipped() {
    struct NoDepth(RecordingBackend);
    use d3d::Backend;
    impl Backend for NoDepth {
        type Surface = u32;
        type Texture = u32;
        fn create_surface(
            &mut self,
            f: D3DFormat,
            w: u32,
            h: u32,
            depth: bool,
        ) -> Result<u32, String> {
            self.0.create_surface(f, w, h, depth)
        }
        fn destroy_surface(&mut self, s: u32) -> Result<(), String> {
            self.0.destroy_surface(s)
        }
        fn upload_texture(&mut self, t: &gfx::texture::DecodedTexture) -> Result<u32, String> {
            self.0.upload_texture(t)
        }
        fn bc_supported(&self) -> bool {
            false
        }
        fn clear(&mut self, c: u32, d: Option<u32>, clear: gfx::Clear) -> Result<(), String> {
            self.0.clear(c, d, clear)
        }
        fn draw(
            &mut self,
            c: u32,
            d: Option<u32>,
            draw: &d3d::ResolvedDraw,
            t: [Option<&u32>; 4],
        ) -> Result<d3d::DrawOutcome, String> {
            self.0.draw(c, d, draw, t)
        }
        fn present(&mut self, s: u32, flags: u32) -> Result<(), String> {
            self.0.present(s, flags)
        }
    }
    let (mut d, mut m) = create(NoDepth(RecordingBackend::new()), UploadPolicy::Always);
    set_states(&mut d, &mut m);
    quad_vb(&mut m, 0.0, 0.0, W as f32, H as f32, 0xFFFF_FFFF);
    bind_quad(&mut d, &mut m);
    alias(&mut m, DEPTH_BUFFER_DATA, 0x12, W, H, W * 4);
    d.set_texture(&mut m, 0, ALIAS).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    assert!(d.take_events().iter().any(
        |e| matches!(e, Event::DrawSkipped { reason, .. } if reason.contains("cannot read depth"))
    ));
    assert!(d.backend.0.draws().is_empty());
}

// GPU ---------------------------------------------------------------------

fn gpu() -> Option<(Device<GfxBackend>, SparseMemory, wgpu::ErrorScopeGuard)> {
    let ctx = match GfxContext::headless() {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("skipping GPU test: {e}");
            return None;
        }
    };
    if !ctx.caps.depth32_stencil8 {
        eprintln!("skipping GPU test: no DEPTH32FLOAT_STENCIL8, depth cannot be read back");
        return None;
    }
    let scope = ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let (d, mut m) = create(GfxBackend::new(ctx), UploadPolicy::Always);
    m.map(0x8000_0000 | DDATA, 0x1000);
    color_target(&mut m);
    Some((d, m, scope))
}

fn finish(scope: wgpu::ErrorScopeGuard) {
    let err = pollster::block_on(scope.pop());
    assert!(err.is_none(), "wgpu validation error: {err:?}");
}

/// A full-target quad whose texture coordinates are the pixel's own
/// (linear textures are addressed in texels).
fn textured_quad(d: &mut Device<GfxBackend>, m: &mut SparseMemory) {
    let mut vb = Vec::new();
    for (x, y) in [(0.0f32, 0.0f32), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
        for v in [W as f32 * x, H as f32 * y, 0.5, 1.0] {
            vb.extend_from_slice(&v.to_le_bytes());
        }
        vb.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        for v in [W as f32 * x, H as f32 * y] {
            vb.extend_from_slice(&v.to_le_bytes());
        }
    }
    m.write(0x8000_0000 | VB_DATA, &vb).unwrap();
    m.write_u32(VB_HEADER, 1 | 0x0100_0000).unwrap();
    m.write_u32(VB_HEADER + 4, VB_DATA).unwrap();
    m.write_u32(VB_HEADER + 8, 0).unwrap();
    d.set_vertex_shader(m, fvf::XYZRHW | fvf::DIFFUSE | 0x100)
        .unwrap();
    d.set_stream_source(m, 0, VB_HEADER, 28).unwrap();
}

fn back_pixel(d: &Device<GfxBackend>, x: u32, y: u32) -> [u8; 4] {
    let (px, order) = d
        .backend
        .store
        .read_color(&d.backend.ctx, d.back_buffer().unwrap())
        .unwrap();
    let p = pixel(&px, x, y);
    match order {
        HostPixelOrder::Bgra8 => [p[2], p[1], p[0], p[3]],
        _ => p,
    }
}

/// The main menu's case: the depth buffer read through `LIN_A8R8G8B8` and
/// `LIN_A8B8G8R8` headers shows its bits, `z << 8 | stencil`, as colour.
#[test]
fn gpu_depth_buffer_sampled_as_colour_shows_its_bits() {
    let Some((mut d, mut m, scope)) = gpu() else {
        return;
    };
    set_states(&mut d, &mut m);
    let z24 = 0x12_3456u32;
    d.clear(&mut m, 0, 0, 0x03, 0, z24 as f32 / 16_777_215.0, 0x5A)
        .unwrap();
    textured_quad(&mut d, &mut m);
    alias(&mut m, DEPTH_BUFFER_DATA, 0x12, W, H, W * 4);
    d.set_texture(&mut m, 0, ALIAS).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    let ev = d.take_events();
    assert!(ev.is_empty(), "{ev:?}");
    // A8R8G8B8 of 0x1234_565A: a = 0x12, r = 0x34, g = 0x56, b = 0x5A.
    assert_eq!(back_pixel(&d, 20, 30), [0x34, 0x56, 0x5A, 0x12]);
    // A8B8G8R8 of the same word: r = 0x5A, g = 0x56, b = 0x34.
    alias(&mut m, DEPTH_BUFFER_DATA, 0x3F, W, H, W * 4);
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    assert_eq!(back_pixel(&d, 5, 60), [0x5A, 0x56, 0x34, 0x12]);
    assert_eq!(d.stats().depth_reads, 1);
    finish(scope);
}

/// A depth level starts with its guest texels and writes back what was
/// drawn: a stencil-only clear keeps the filled depth and replaces the
/// stencil byte of every word.
#[test]
fn gpu_depth_level_fills_from_and_writes_back_to_guest_memory() {
    let Some((mut d, mut m, scope)) = gpu() else {
        return;
    };
    for (format, pitch) in [(0x2A, None), (0x2E, Some(64))] {
        let words: Vec<u32> = (0..S * S)
            .map(|i| ((0x10_0000 + i * 0x101) << 8) | 0x77)
            .collect();
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        m.write(0x8000_0000 | DDATA, &bytes).unwrap();
        depth_texture(&mut m, format, pitch);
        d.set_render_target(&mut m, CSURF, DLEVEL).unwrap();
        d.clear(&mut m, 0, 0, 0x02, 0, 0.0, 0x3C).unwrap();
        let back = m.read_u32(L.device + 0x1A04).unwrap();
        d.set_render_target(&mut m, back, 0).unwrap();
        let out = m.read_vec(0x8000_0000 | DDATA, bytes.len()).unwrap();
        for (i, (got, want)) in out.chunks(4).zip(&words).enumerate() {
            let got = u32::from_le_bytes(got.try_into().unwrap());
            assert_eq!(got, (want & !0xFF) | 0x3C, "format {format:#x} texel {i}");
        }
        assert_eq!(d.take_events(), []);
    }
    finish(scope);
}

/// A depth-format texture is a shadow map on the NV2A (a comparison, not
/// colour): the shader provider skips the draw with the reason until the
/// translated programs compare.
#[test]
fn gpu_sampling_a_depth_format_texture_is_skipped_as_a_shadow_map() {
    let Some((mut d, mut m, scope)) = gpu() else {
        return;
    };
    set_states(&mut d, &mut m);
    depth_texture(&mut m, 0x2A, None);
    m.write(0x8000_0000 | DDATA, &[0x40; 1024]).unwrap();
    m.write_u32(L.render_state_slot(156), 0x203).unwrap(); // SHADOWFUNC = LESSEQUAL
    textured_quad(&mut d, &mut m);
    d.set_texture(&mut m, 0, DTEX).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    let ev = d.take_events();
    assert!(
        ev.iter()
            .any(|e| matches!(e, Event::DrawSkipped { reason, .. }
            if reason.contains("shadow map") && reason.contains("0x203"))),
        "{ev:?}"
    );
    // Uploaded all the same (its raw bits), so a provider that compares
    // has what it needs.
    assert_eq!(d.stats().texture_uploads, 1);
    finish(scope);
}

/// Review P2: changing only the colour target is a write-back point for
/// the depth level that stays bound too.
#[test]
fn a_colour_only_switch_writes_the_kept_depth_level_back() {
    let (mut d, mut m) = mem();
    depth_texture(&mut m, 0x2A, None);
    let back = m.read_u32(L.device + 0x1A04).unwrap();
    d.set_render_target(&mut m, CSURF, DLEVEL).unwrap();
    d.clear(&mut m, 0, 0, 0x01, 0, 1.0, 0).unwrap();
    d.backend.depth_readback = Some(vec![0x5A]);
    // Back buffer is 64x64, the level 16x16: the recording backend does
    // not check sizes, which is all this needs.
    d.set_render_target(&mut m, back, DLEVEL).unwrap();
    assert_eq!(depth_reads(&d).len(), 1);
    assert_eq!(m.read_u32(0x8000_0000 | DDATA).unwrap(), 0x5A5A_5A5A);
    // Clean now: switching again reads nothing.
    d.set_render_target(&mut m, CSURF, DLEVEL).unwrap();
    assert_eq!(depth_reads(&d).len(), 1);
}

/// A recording backend whose draws are skipped, and whose textures count
/// as live while the device holds them.
struct Skipping {
    inner: RecordingBackend,
    skip: bool,
    live: Vec<std::sync::Arc<()>>,
}

impl Skipping {
    fn new(skip: bool) -> Self {
        Self {
            inner: RecordingBackend::new(),
            skip,
            live: Vec::new(),
        }
    }

    /// Host textures the device still holds.
    fn held(&self) -> usize {
        self.live
            .iter()
            .filter(|a| std::sync::Arc::strong_count(a) > 1)
            .count()
    }
}

impl d3d::Backend for Skipping {
    type Surface = u32;
    type Texture = (u32, std::sync::Arc<()>);
    fn create_surface(&mut self, f: D3DFormat, w: u32, h: u32, depth: bool) -> Result<u32, String> {
        self.inner.create_surface(f, w, h, depth)
    }
    fn destroy_surface(&mut self, s: u32) -> Result<(), String> {
        self.inner.destroy_surface(s)
    }
    fn upload_texture(
        &mut self,
        t: &gfx::texture::DecodedTexture,
    ) -> Result<Self::Texture, String> {
        let a = std::sync::Arc::new(());
        self.live.push(a.clone());
        Ok((self.inner.upload_texture(t)?, a))
    }
    fn bc_supported(&self) -> bool {
        false
    }
    fn clear(&mut self, c: u32, d: Option<u32>, clear: gfx::Clear) -> Result<(), String> {
        self.inner.clear(c, d, clear)
    }
    fn draw(
        &mut self,
        c: u32,
        d: Option<u32>,
        draw: &d3d::ResolvedDraw,
        t: [Option<&Self::Texture>; 4],
    ) -> Result<d3d::DrawOutcome, String> {
        if self.skip {
            return Ok(d3d::DrawOutcome::Skipped("test skip".into()));
        }
        self.inner.draw(c, d, draw, t.map(|t| t.map(|t| &t.0)))
    }
    fn present(&mut self, s: u32, flags: u32) -> Result<(), String> {
        self.inner.present(s, flags)
    }
    fn write_surface(&mut self, s: u32, desc: &gfx::TextureDesc, b: &[u8]) -> Result<bool, String> {
        self.inner.write_surface(s, desc, b)
    }
    fn read_depth(
        &mut self,
        s: u32,
        f: D3DFormat,
        w: u32,
        h: u32,
        l: d3d::GuestLayoutKind,
    ) -> Result<Option<Vec<u8>>, String> {
        self.inner.read_depth(s, f, w, h, l)
    }
}

fn skipping_reads(d: &Device<Skipping>) -> usize {
    d.backend
        .inner
        .calls
        .iter()
        .filter(|c| matches!(c, Recorded::ReadDepth { .. }))
        .count()
}

/// Review P2: a skipped draw dirties nothing, so skipped shadow-map draws
/// cause no write-backs of the depth level and no readbacks of the depth
/// buffer.
#[test]
fn skipped_draws_dirty_neither_depth_levels_nor_the_depth_buffer() {
    let (mut d, mut m) = create(Skipping::new(true), UploadPolicy::Tracked);
    let depth = m.read_u32(L.device + 0x1A08).unwrap();
    let back = m.read_u32(L.device + 0x1A04).unwrap();
    m.map(0x8000_0000 | DDATA, 0x1000);
    color_target(&mut m);
    depth_texture(&mut m, 0x2A, None);
    set_states(&mut d, &mut m);
    quad_vb(&mut m, 0.0, 0.0, S as f32, S as f32, 0xFFFF_FFFF);
    bind_quad(&mut d, &mut m);
    z_writes(&mut m, true);
    // The level bound as depth and sampled, as the first level's shadow
    // draws are.
    d.set_render_target(&mut m, CSURF, DLEVEL).unwrap();
    d.set_texture(&mut m, 0, DTEX).unwrap();
    for _ in 0..3 {
        d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    }
    d.swap(&mut m, 0).unwrap();
    assert_eq!(
        skipping_reads(&d),
        0,
        "nothing was drawn, nothing written back"
    );
    // The device's depth buffer through a colour alias: read once.
    d.set_render_target(&mut m, back, depth).unwrap();
    d.set_texture(&mut m, 0, 0).unwrap();
    quad_vb(&mut m, 0.0, 0.0, W as f32, H as f32, 0xFFFF_FFFF);
    alias(&mut m, DEPTH_BUFFER_DATA, 0x12, W, H, W * 4);
    d.set_texture(&mut m, 0, ALIAS).unwrap();
    for _ in 0..3 {
        d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    }
    assert_eq!(d.stats().depth_reads, 1);
    // The same draws drawn do change the buffer each time.
    d.backend.skip = false;
    for _ in 0..3 {
        d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    }
    assert_eq!(d.stats().depth_reads, 3);
}

/// Review P2: destroying a texture over the depth buffer frees its host
/// copy.
#[test]
fn destroying_a_depth_alias_frees_its_host_texture() {
    let (mut d, mut m) = create(Skipping::new(false), UploadPolicy::Tracked);
    set_states(&mut d, &mut m);
    quad_vb(&mut m, 0.0, 0.0, W as f32, H as f32, 0xFFFF_FFFF);
    bind_quad(&mut d, &mut m);
    alias(&mut m, DEPTH_BUFFER_DATA, 0x12, W, H, W * 4);
    d.set_texture(&mut m, 0, ALIAS).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    assert_eq!(d.backend.held(), 1);
    d.set_texture(&mut m, 0, 0).unwrap();
    d.destroy_resource(&mut m, ALIAS).unwrap();
    assert_eq!(d.backend.held(), 0);
}
