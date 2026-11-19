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

/// A colour surface over the depth buffer's memory, as `0x14600` builds
/// Halo's rasterizer target 3 (`0x509370`): the back buffer's header with
/// the depth buffer's `Data`.
const DCOLOR: u32 = MISC + 0x7880;

fn reads_of(d: &Device<RecordingBackend>, surface: u32) -> usize {
    d.backend
        .calls
        .iter()
        .filter(|c| matches!(c, Recorded::ReadSurface { surface: s, .. } if *s == surface))
        .count()
}

fn snapshots_of(d: &Device<RecordingBackend>, surface: u32) -> Vec<u32> {
    d.backend
        .calls
        .iter()
        .filter_map(|c| match c {
            Recorded::Snapshot { surface: s, id, .. } if *s == surface => Some(*id),
            _ => None,
        })
        .collect()
}

/// The level loading screen's trail (`0x29370`): its image is drawn into
/// target 3, the colour surface over the depth buffer's memory, then
/// sampled back through `0x509368`, the `LIN_A8R8G8B8` texture over the
/// same memory. On the console that texture reads the colour just written;
/// it read the depth buffer's bits (stencil as blue: a yellow screen).
/// What the memory holds is whichever of the two was drawn into last.
#[test]
fn textures_over_the_depth_buffer_read_the_colour_surface_drawn_into_last() {
    let (mut d, mut m) = create(RecordingBackend::new(), UploadPolicy::Always);
    set_states(&mut d, &mut m);
    quad_vb(&mut m, 0.0, 0.0, W as f32, H as f32, 0xFFFF_FFFF);
    bind_quad(&mut d, &mut m);
    let back = m.read_u32(L.device + 0x1A14).unwrap();
    let zbuf = m.read_u32(L.device + 0x1A08).unwrap();
    SurfaceHeader::linear(D3DFormat::LinA8R8G8B8, W, H, W * 4, DEPTH_BUFFER_DATA)
        .write(&mut m, DCOLOR)
        .unwrap();
    alias(&mut m, DEPTH_BUFFER_DATA, 0x12, W, H, W * 4);

    // Nothing drawn into the colour surface yet: the depth bits.
    d.set_texture(&mut m, 0, ALIAS).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    assert_eq!(depth_reads(&d).len(), 1);

    // Target 3, drawn into with nothing sampled.
    d.set_texture(&mut m, 0, 0).unwrap();
    d.set_render_target(&mut m, DCOLOR, 0).unwrap();
    let dcolor = d
        .backend
        .calls
        .iter()
        .rev()
        .find_map(|c| match c {
            Recorded::Surface {
                id,
                format: D3DFormat::LinA8R8G8B8,
                size: (W, H),
                depth: false,
            } => Some(*id),
            _ => None,
        })
        .expect("a host surface for the colour surface");
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    assert!(snapshots_of(&d, dcolor).is_empty());

    // Back on the back buffer, the texture over that memory is a copy of
    // the colour surface: no depth read, no upload.
    d.set_render_target(&mut m, back, 0).unwrap();
    d.set_texture(&mut m, 0, ALIAS).unwrap();
    let uploads = d.backend.uploads();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    assert_eq!(d.take_events(), []);
    let snap = snapshots_of(&d, dcolor);
    assert_eq!(snap.len(), 1);
    match d.backend.calls.last() {
        Some(Recorded::Draw { textures, draw, .. }) => {
            assert_eq!(textures[0], Some(snap[0]));
            assert_eq!(draw.texture_info[0].unwrap().header, ALIAS);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(depth_reads(&d).len(), 1);
    assert_eq!(d.backend.uploads(), uploads);

    // Any other view of those bytes (LIN_A8B8G8R8, `0x50936C`) decodes
    // the colour surface's bytes, read back once per change.
    alias(&mut m, DEPTH_BUFFER_DATA, 0x3F, W, H, W * 4);
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    assert_eq!(d.take_events(), []);
    assert_eq!(reads_of(&d, dcolor), 1);
    assert_eq!(d.backend.uploads(), uploads + 1);
    assert_eq!(depth_reads(&d).len(), 1);
    // A second draw into the colour surface makes that copy stale.
    d.set_texture(&mut m, 0, 0).unwrap();
    d.set_render_target(&mut m, DCOLOR, 0).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    d.set_render_target(&mut m, back, 0).unwrap();
    d.set_texture(&mut m, 0, ALIAS).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    assert_eq!(reads_of(&d, dcolor), 2);
    assert_eq!(depth_reads(&d).len(), 1);

    // A depth clear: the memory holds depth again.
    alias(&mut m, DEPTH_BUFFER_DATA, 0x12, W, H, W * 4);
    d.set_render_target(&mut m, back, zbuf).unwrap();
    d.clear(&mut m, 0, 0, 0x01, 0, 0.5, 0).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    assert_eq!(depth_reads(&d).len(), 2);
    assert_eq!(snapshots_of(&d, dcolor).len(), 1);
    assert_eq!(d.take_events(), []);
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

fn back_pixel(d: &mut Device<GfxBackend>, x: u32, y: u32) -> [u8; 4] {
    let back = d.back_buffer().unwrap();
    // `GfxBackend::read_color` submits the pending pass first.
    let (px, order) = d.backend.read_color(back).unwrap();
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
    assert_eq!(back_pixel(&mut d, 20, 30), [0x34, 0x56, 0x5A, 0x12]);
    // A8B8G8R8 of the same word: r = 0x5A, g = 0x56, b = 0x34.
    alias(&mut m, DEPTH_BUFFER_DATA, 0x3F, W, H, W * 4);
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    assert_eq!(back_pixel(&mut d, 5, 60), [0x5A, 0x56, 0x34, 0x12]);
    assert_eq!(d.stats().depth_reads, 1);
    finish(scope);
}

/// Target 3's case on a GPU: a colour cleared into the colour surface over
/// the depth buffer's memory is what the `LIN_A8R8G8B8` texture over that
/// memory shows, not the depth bits; after a depth clear the bits again.
#[test]
fn gpu_colour_drawn_over_the_depth_buffer_is_what_its_texture_shows() {
    let Some((mut d, mut m, scope)) = gpu() else {
        return;
    };
    set_states(&mut d, &mut m);
    let back = m.read_u32(L.device + 0x1A14).unwrap();
    let zbuf = m.read_u32(L.device + 0x1A08).unwrap();
    let z24 = 0x12_3456u32;
    d.clear(&mut m, 0, 0, 0x03, 0, z24 as f32 / 16_777_215.0, 0x5A)
        .unwrap();
    SurfaceHeader::linear(D3DFormat::LinA8R8G8B8, W, H, W * 4, DEPTH_BUFFER_DATA)
        .write(&mut m, DCOLOR)
        .unwrap();
    d.set_render_target(&mut m, DCOLOR, 0).unwrap();
    d.clear(&mut m, 0, 0, 0xF0, 0x80A0_C0E0, 1.0, 0).unwrap();
    d.set_render_target(&mut m, back, zbuf).unwrap();
    textured_quad(&mut d, &mut m);
    alias(&mut m, DEPTH_BUFFER_DATA, 0x12, W, H, W * 4);
    d.set_texture(&mut m, 0, ALIAS).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    let ev = d.take_events();
    assert!(ev.is_empty(), "{ev:?}");
    assert_eq!(back_pixel(&mut d, 20, 30), [0xA0, 0xC0, 0xE0, 0x80]);
    assert_eq!(d.stats().depth_reads, 0);
    d.clear(&mut m, 0, 0, 0x03, 0, z24 as f32 / 16_777_215.0, 0x5A)
        .unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    assert_eq!(back_pixel(&mut d, 20, 30), [0x34, 0x56, 0x5A, 0x12]);
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
        // Lazily the level stays on the host until the guest waits on it.
        d.block_on_resource(&mut m, DTEX).unwrap();
        let out = m.read_vec(0x8000_0000 | DDATA, bytes.len()).unwrap();
        for (i, (got, want)) in out.chunks(4).zip(&words).enumerate() {
            let got = u32::from_le_bytes(got.try_into().unwrap());
            assert_eq!(got, (want & !0xFF) | 0x3C, "format {format:#x} texel {i}");
        }
        assert_eq!(d.take_events(), []);
    }
    finish(scope);
}

const PSDEF: u32 = MISC + 0x7900;
const PS_OBJECT: u32 = MISC + 0x79F0;
const RS_SHADOWFUNC: u32 = 156;

/// A pixel shader whose stage 0 is in `mode` and whose output is t0
/// (final combiner `D = t0`, `G = t0.a`).
fn t0_shader(d: &mut Device<GfxBackend>, m: &mut SparseMemory, mode: u32) {
    use shaders::psh::bits::*;
    let def = shaders::psh::PixelShaderDef {
        texture_modes: texture_modes([mode, 0, 0, 0]),
        final_inputs_abcd: inputs(0, 0, 0, REG_T0),
        final_inputs_efg: final_efg(0, 0, REG_T0 | CHAN_ALPHA, 0),
        ..Default::default()
    };
    m.write(PSDEF, &def.to_bytes()).unwrap();
    for (i, w) in [1, 0, PSDEF].iter().enumerate() {
        m.write_u32(PS_OBJECT + 4 * i as u32, *w).unwrap();
    }
    d.set_pixel_shader(m, PS_OBJECT).unwrap();
}

/// A full-target quad with a four-component texture coordinate: (8, 8)
/// in s and t (a texel of a linear texture, a repeat of a swizzled one),
/// `r` running from 0 at the left edge to `r_right` at the right, all
/// times `q`.
fn projective_quad(d: &mut Device<GfxBackend>, m: &mut SparseMemory, r_right: f32, q: f32) {
    let mut vb = Vec::new();
    for (x, y) in [(0.0f32, 0.0f32), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
        for v in [W as f32 * x, H as f32 * y, 0.5, 1.0] {
            vb.extend_from_slice(&v.to_le_bytes());
        }
        vb.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        for v in [8.0 * q, 8.0 * q, r_right * x * q, q] {
            vb.extend_from_slice(&v.to_le_bytes());
        }
    }
    m.write(0x8000_0000 | VB_DATA, &vb).unwrap();
    m.write_u32(VB_HEADER, 1 | 0x0100_0000).unwrap();
    m.write_u32(VB_HEADER + 4, VB_DATA).unwrap();
    m.write_u32(VB_HEADER + 8, 0).unwrap();
    // TEX1 with D3DFVF_TEXCOORDSIZE4(0).
    d.set_vertex_shader(m, fvf::XYZRHW | fvf::DIFFUSE | 0x100 | (2 << 16))
        .unwrap();
    d.set_stream_source(m, 0, VB_HEADER, 36).unwrap();
}

/// A depth-format texture is a shadow map on the NV2A: PROJECT3D compares
/// the stored depth with `r/q` under `D3DRS_SHADOWFUNC` and gives 1 or 0
/// in every channel. Every texel holds the middle of the format's range
/// and `r` runs from 0 to twice that across the target, so `GREATER`
/// (stored > r, the first campaign level's `0x204`) is white on the left
/// and black on the right, and `LESSEQUAL` the reverse. D24S8 and D16,
/// swizzled and linear.
#[test]
fn gpu_shadow_maps_compare_stored_depth_with_r_over_q() {
    let Some((mut d, mut m, scope)) = gpu() else {
        return;
    };
    set_states(&mut d, &mut m);
    for (format, pitch) in [
        (0x2A, None),
        (0x2E, Some(64)),
        (0x2C, None),
        (0x30, Some(64)),
    ] {
        let d16 = format == 0x2C || format == 0x30;
        let (bytes, mid): (Vec<u8>, f32) = if d16 {
            (0x8000u16.to_le_bytes().repeat(512), 32768.0)
        } else {
            // Depth 0x800000 with stencil 0x5A.
            (0x8000_005Au32.to_le_bytes().repeat(256), 8_388_608.0)
        };
        m.write(0x8000_0000 | DDATA, &bytes).unwrap();
        depth_texture(&mut m, format, pitch);
        t0_shader(&mut d, &mut m, shaders::psh::bits::TEX_PROJECT3D);
        projective_quad(&mut d, &mut m, 2.0 * mid, 2.0);
        d.set_texture(&mut m, 0, DTEX).unwrap();
        for (func, left, right) in [(0x204, 0xFF, 0x00), (0x203, 0x00, 0xFF)] {
            m.write_u32(L.render_state_slot(RS_SHADOWFUNC), func)
                .unwrap();
            d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
            let ev = d.take_events();
            assert!(ev.is_empty(), "format {format:#x}: {ev:?}");
            for (x, v) in [(4, left), (W - 4, right)] {
                assert_eq!(
                    back_pixel(&mut d, x, 30),
                    [v; 4],
                    "format {format:#x} func {func:#x} x {x}"
                );
            }
        }
    }
    finish(scope);
}

/// PROJECT2D compares with 0: with `GREATER` every non-zero stored depth
/// passes, with `LESS` nothing does.
#[test]
fn gpu_shadow_map_project2d_compares_with_zero() {
    let Some((mut d, mut m, scope)) = gpu() else {
        return;
    };
    set_states(&mut d, &mut m);
    m.write(
        0x8000_0000 | DDATA,
        &0x0000_0100u32.to_le_bytes().repeat(256),
    )
    .unwrap();
    depth_texture(&mut m, 0x2E, Some(64));
    t0_shader(&mut d, &mut m, shaders::psh::bits::TEX_PROJECT2D);
    projective_quad(&mut d, &mut m, 0.0, 1.0);
    d.set_texture(&mut m, 0, DTEX).unwrap();
    for (func, want) in [(0x204, 0xFF), (0x201, 0x00), (0x207, 0xFF), (0x200, 0x00)] {
        m.write_u32(L.render_state_slot(RS_SHADOWFUNC), func)
            .unwrap();
        d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
        assert_eq!(d.take_events(), []);
        assert_eq!(back_pixel(&mut d, 30, 30), [want; 4], "func {func:#x}");
    }
    finish(scope);
}

/// A `D3DRS_SHADOWFUNC` that is not a `D3DCMP_*` value has no comparison:
/// the draw is skipped with a reason naming the stage, format and value.
#[test]
fn gpu_shadow_map_with_a_bad_shadowfunc_is_skipped() {
    let Some((mut d, mut m, scope)) = gpu() else {
        return;
    };
    set_states(&mut d, &mut m);
    depth_texture(&mut m, 0x2A, None);
    m.write(0x8000_0000 | DDATA, &[0x40; 1024]).unwrap();
    m.write_u32(L.render_state_slot(RS_SHADOWFUNC), 0x123)
        .unwrap();
    textured_quad(&mut d, &mut m);
    d.set_texture(&mut m, 0, DTEX).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    let ev = d.take_events();
    assert!(
        ev.iter()
            .any(|e| matches!(e, Event::DrawSkipped { reason, .. }
            if reason.contains("stage 0") && reason.contains("D24S8") && reason.contains("0x123"))),
        "{ev:?}"
    );
    // Uploaded all the same (its raw bits).
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
    // Both channel-sign variants belong to the same guest resource.
    m.write_u32(L.texture_state_slot(0, 10), 0xE000_0000)
        .unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    assert_eq!(d.backend.held(), 2);
    d.set_texture(&mut m, 0, 0).unwrap();
    d.destroy_resource(&mut m, ALIAS).unwrap();
    assert_eq!(d.backend.held(), 0);
}

/// Review P2: a shadow map's depth words are raw, even when the stage's
/// colour sign bits are set. PROJECT2D/GREATER compares each word with 0.
#[test]
fn gpu_shadow_raw_bits_ignore_colorsign() {
    let Some((mut d, mut m, scope)) = gpu() else {
        return;
    };
    set_states(&mut d, &mut m);
    t0_shader(&mut d, &mut m, shaders::psh::bits::TEX_PROJECT2D);
    projective_quad(&mut d, &mut m, 0.0, 1.0);
    m.write_u32(L.render_state_slot(RS_SHADOWFUNC), 0x204)
        .unwrap();
    for (format, pitch, bpp, positive) in [
        (0x2A, None, 4, 0x0000_0100u32), // D24 = 1, stencil = 0
        (0x2B, None, 4, 0x7F00_0000),    // F24 = 1.0
        (0x2C, None, 2, 1),              // D16 = 1
        (0x2D, None, 2, 0x7000),         // F16 = 1.0
        (0x2E, Some(64), 4, 0x0000_0100),
        (0x2F, Some(64), 4, 0x7F00_0000),
        (0x30, Some(64), 2, 1),
        (0x31, Some(64), 2, 0x7000),
    ] {
        for (word, want) in [(0u32, 0u8), (positive, 255)] {
            let stride = pitch.unwrap_or(S * bpp) as usize;
            let mut bytes = vec![0; stride * S as usize];
            for y in 0..S as usize {
                for x in 0..S as usize {
                    let offset = y * stride + x * bpp as usize;
                    bytes[offset..offset + bpp as usize]
                        .copy_from_slice(&word.to_le_bytes()[..bpp as usize]);
                }
            }
            m.write(0x8000_0000 | DDATA, &bytes).unwrap();
            depth_texture(&mut m, format, pitch);
            d.set_texture(&mut m, 0, DTEX).unwrap();
            for sign in [0, 0xE000_0000, 0xF000_0000] {
                m.write_u32(L.texture_state_slot(0, 10), sign).unwrap();
                d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
                assert_eq!(
                    back_pixel(&mut d, 20, 30),
                    [want; 4],
                    "format {format:#x}, word {word:#x}, COLORSIGN {sign:#x}"
                );
                assert_eq!(d.take_events(), []);
            }
        }
    }
    finish(scope);
}

/// Review P2: a colour alias over depth has the same signed-byte sampling
/// as ordinary memory, and signed/unsigned views can coexist in one draw.
#[test]
fn gpu_signed_depth_alias_matches_identical_guest_texture() {
    use shaders::psh::PixelShaderDef;
    use shaders::psh::bits::*;

    let Some((mut d, mut m, scope)) = gpu() else {
        return;
    };
    set_states(&mut d, &mut m);
    d.clear(
        &mut m,
        0,
        0,
        0x03,
        0,
        0xFF_2020u32 as f32 / 16_777_215.0,
        0x40,
    )
    .unwrap();
    textured_quad(&mut d, &mut m);
    alias(&mut m, DEPTH_BUFFER_DATA, 0x12, W, H, W * 4);
    texture(&mut m, 0);
    m.write(
        0x8000_0000 | TEX_DATA,
        &0xFF20_2040u32.to_le_bytes().repeat(64),
    )
    .unwrap();
    // Revisit the unsigned view: changing signs must neither reuse the
    // wrong host texture nor force another readback of unchanged depth.
    for (sign, want) in [
        (0, [32, 32, 64, 255]),
        (0xE000_0000, [64, 64, 128, 255]),
        (0, [32, 32, 64, 255]),
    ] {
        m.write_u32(L.texture_state_slot(0, 10), sign).unwrap();
        d.set_texture(&mut m, 0, ALIAS).unwrap();
        d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
        let alias_pixel = back_pixel(&mut d, 20, 30);
        d.set_texture(&mut m, 0, TEX_HEADER).unwrap();
        d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
        let ordinary_pixel = back_pixel(&mut d, 20, 30);
        assert_eq!(ordinary_pixel, want, "COLORSIGN {sign:#x}");
        assert_eq!(alias_pixel, ordinary_pixel, "COLORSIGN {sign:#x}");
    }
    // Both stages fetch the same header, with opposite sign masks. Even
    // the output from stage 0 must keep its own storage after stage 1 is
    // resolved. Select each stage in turn to check both bindings.
    d.set_texture(&mut m, 0, ALIAS).unwrap();
    d.set_texture(&mut m, 1, ALIAS).unwrap();
    m.write_u32(L.texture_state_slot(1, 10), 0xE000_0000)
        .unwrap();
    t0_shader(&mut d, &mut m, TEX_PROJECT2D);
    for (reg, want) in [(REG_T0, [32, 32, 64, 255]), (REG_T1, [64, 64, 128, 255])] {
        let def = PixelShaderDef {
            texture_modes: texture_modes([TEX_PROJECT2D, TEX_PROJECT2D, 0, 0]),
            final_inputs_abcd: inputs(0, 0, 0, reg),
            final_inputs_efg: final_efg(0, 0, reg | CHAN_ALPHA, 0),
            ..Default::default()
        };
        m.write(PSDEF, &def.to_bytes()).unwrap();
        d.set_pixel_shader(&mut m, PS_OBJECT).unwrap();
        d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
        assert_eq!(back_pixel(&mut d, 20, 30), want, "output register {reg}");
    }
    assert_eq!(d.stats().depth_reads, 1);
    assert_eq!(d.stats().depth_alias_uploads, 2);
    assert_eq!(d.take_events(), []);
    finish(scope);
}

/// The render thread makes depth-alias textures on its side
/// (`Backend::depth_alias_texture`): the same read-backs, uploads, draws
/// and stats as the device making them itself, and the guest never waits.
#[test]
fn the_render_thread_makes_the_same_depth_alias_textures() {
    fn run<B: d3d::Backend>(backend: B) -> (Device<B>, SparseMemory) {
        let (mut d, mut m) = create(backend, UploadPolicy::Always);
        set_states(&mut d, &mut m);
        quad_vb(&mut m, 0.0, 0.0, W as f32, H as f32, 0xFFFF_FFFF);
        bind_quad(&mut d, &mut m);
        alias(&mut m, DEPTH_BUFFER_DATA, 0x12, W, H, W * 4);
        d.set_texture(&mut m, 0, ALIAS).unwrap();
        d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
        d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
        alias(&mut m, DEPTH_BUFFER_DATA, 0x3F, W, H, W * 4);
        d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
        d.clear(&mut m, 0, 0, 0x01, 0, 0.5, 0).unwrap();
        d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
        z_writes(&mut m, true);
        d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
        d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
        // A format that does not decode skips the same way.
        alias(&mut m, DEPTH_BUFFER_DATA, 0x7F, W, H, W * 4);
        d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
        (d, m)
    }
    let recorder = || {
        let mut r = RecordingBackend::new();
        r.depth_readback = Some((0..=255).collect());
        r
    };
    let (mut a, _) = run(recorder());
    let (mut b, _) = run(d3d::ThreadedBackend::new(recorder()).unwrap());
    assert_eq!(a.take_events(), b.take_events());
    assert_eq!(a.stats(), b.stats());
    assert!(a.stats().depth_reads >= 3, "{:?}", a.stats());
    let calls = b.backend.with_inner(|r| r.calls.clone()).unwrap();
    assert_eq!(a.backend.calls, calls);
    assert_eq!(b.backend.misses(), 0);
}

/// The render thread reads target 3's colour through textures over the
/// depth buffer exactly as the device does on the caller's thread.
#[test]
fn the_render_thread_reads_the_colour_surface_over_the_depth_buffer_the_same() {
    fn run<B: d3d::Backend>(backend: B) -> Device<B> {
        let (mut d, mut m) = create(backend, UploadPolicy::Always);
        set_states(&mut d, &mut m);
        quad_vb(&mut m, 0.0, 0.0, W as f32, H as f32, 0xFFFF_FFFF);
        bind_quad(&mut d, &mut m);
        let back = m.read_u32(L.device + 0x1A14).unwrap();
        SurfaceHeader::linear(D3DFormat::LinA8R8G8B8, W, H, W * 4, DEPTH_BUFFER_DATA)
            .write(&mut m, DCOLOR)
            .unwrap();
        d.set_render_target(&mut m, DCOLOR, 0).unwrap();
        d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
        d.set_render_target(&mut m, back, 0).unwrap();
        alias(&mut m, DEPTH_BUFFER_DATA, 0x12, W, H, W * 4);
        d.set_texture(&mut m, 0, ALIAS).unwrap();
        d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
        alias(&mut m, DEPTH_BUFFER_DATA, 0x3F, W, H, W * 4);
        d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
        d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
        d
    }
    let mut a = run(RecordingBackend::new());
    let mut b = run(d3d::ThreadedBackend::new(RecordingBackend::new()).unwrap());
    assert_eq!(a.take_events(), []);
    assert_eq!(b.take_events(), []);
    assert_eq!(a.stats(), b.stats());
    assert_eq!(a.stats().depth_reads, 0);
    let calls = b.backend.with_inner(|r| r.calls.clone()).unwrap();
    assert_eq!(a.backend.calls, calls);
}

/// One run of the lazy-depth comparison: the level filled from a guest
/// pattern, half of it drawn over at another depth, unbound, sampled as a
/// shadow map (PROJECT3D, `GREATER`), then waited on. Answers the back
/// buffer, the level's guest bytes before and after the wait, and the
/// stats before the wait.
type ShadowRun = (Vec<u8>, Vec<u8>, Vec<u8>, d3d::Stats);

fn shadow_run(lazy: bool, format: u32, pitch: Option<u32>) -> Option<ShadowRun> {
    let (mut d, mut m, scope) = gpu()?;
    d.set_lazy_depth(lazy);
    set_states(&mut d, &mut m);
    let words: Vec<u32> = (0..S * S).map(|i| ((0x10_0000 + i * 0x7_1234) << 8) | (i & 0xFF)).collect();
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    m.write(0x8000_0000 | DDATA, &bytes).unwrap();
    depth_texture(&mut m, format, pitch);
    d.set_render_target(&mut m, CSURF, DLEVEL).unwrap();
    quad_vb(&mut m, 0.0, 0.0, S as f32, (S / 2) as f32, 0xFFFF_FFFF);
    bind_quad(&mut d, &mut m);
    z_writes(&mut m, true);
    m.write_u32(L.render_state_slot(RS_ZFUNC), 8).unwrap(); // ALWAYS
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    z_writes(&mut m, false);
    let back = m.read_u32(L.device + 0x1A04).unwrap();
    d.set_render_target(&mut m, back, 0).unwrap();
    t0_shader(&mut d, &mut m, shaders::psh::bits::TEX_PROJECT3D);
    projective_quad(&mut d, &mut m, 2.0 * 8_388_608.0, 2.0);
    d.set_texture(&mut m, 0, DTEX).unwrap();
    m.write_u32(L.render_state_slot(RS_SHADOWFUNC), 0x204).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    // Sampled again with nothing drawn into the level: the same texture.
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    let ev = d.take_events();
    assert!(ev.is_empty(), "{ev:?}");
    let bb = d.back_buffer().unwrap();
    let (px, _) = d.backend.read_color(bb).unwrap();
    let before = m.read_vec(0x8000_0000 | DDATA, bytes.len()).unwrap();
    let stats = d.stats();
    d.block_on_resource(&mut m, DTEX).unwrap();
    let after = m.read_vec(0x8000_0000 | DDATA, bytes.len()).unwrap();
    finish(scope);
    Some((px, before, after, stats))
}

/// Lazy depth levels against the eager write-back at the binding change:
/// the shadow-mapped pixels are the same, and so are the level's guest
/// bytes once the guest waits on it (the only point it may read them).
/// Lazily nothing was read back before the wait, and the sampled texture
/// was made on the GPU once.
#[test]
fn gpu_lazy_depth_levels_sample_and_write_back_as_the_eager_ones() {
    for (format, pitch) in [(0x2A, None), (0x2E, Some(64))] {
        let Some((epx, ebefore, eafter, estats)) = shadow_run(false, format, pitch) else {
            return;
        };
        let (lpx, lbefore, lafter, lstats) = shadow_run(true, format, pitch).unwrap();
        assert_eq!(lpx, epx, "format {format:#x}: shadow-mapped pixels");
        assert_eq!(lafter, eafter, "format {format:#x}: guest bytes after the wait");
        assert_eq!(ebefore, eafter, "format {format:#x}: eagerly written back at the switch");
        assert_ne!(lbefore, lafter, "format {format:#x}: lazily still the guest's pattern before the wait");
        assert_eq!(estats.writeback_causes.depth_binding, 1);
        assert_eq!(estats.depth_level_textures, 0);
        assert_eq!(lstats.rtt_writebacks, 0, "format {format:#x}: no read-back before the wait");
        assert_eq!(lstats.depth_level_textures, 1);
    }
}

/// With the backend unable to sample depth surfaces (the recording
/// backend by default), depth levels are written back at the binding
/// change as before, even with lazy depth on.
#[test]
fn lazy_depth_needs_a_backend_that_samples_depth_surfaces() {
    let (mut d, mut m) = mem();
    d.set_lazy_depth(true);
    depth_texture(&mut m, 0x2A, None);
    let back = m.read_u32(L.device + 0x1A04).unwrap();
    d.set_render_target(&mut m, CSURF, DLEVEL).unwrap();
    d.clear(&mut m, 0, 0, 0x01, 0, 1.0, 0).unwrap();
    d.backend.depth_readback = Some(vec![0x5A]);
    d.set_render_target(&mut m, back, 0).unwrap();
    assert_eq!(depth_reads(&d).len(), 1);
    assert_eq!(d.stats().writeback_causes.depth_binding, 1);
}

/// A backend that samples depth surfaces: the level is not read back at
/// the switch; a sample takes `depth_level_texture` once per time it is
/// drawn into; `BlockOnResource` writes it back, and the guest's stores
/// into it since it was unbound win over the drawn bytes, as for colour.
#[test]
fn lazy_depth_levels_are_read_back_only_when_waited_on() {
    let (mut d, mut m) = mem();
    d.backend.sample_surfaces = true;
    d.backend.sample_depth = true;
    depth_texture(&mut m, 0x2A, None);
    set_states(&mut d, &mut m);
    quad_vb(&mut m, 0.0, 0.0, S as f32, S as f32, 0xFFFF_FFFF);
    bind_quad(&mut d, &mut m);
    let back = m.read_u32(L.device + 0x1A04).unwrap();
    d.set_render_target(&mut m, CSURF, DLEVEL).unwrap();
    z_writes(&mut m, true);
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    z_writes(&mut m, false);
    d.set_render_target(&mut m, back, 0).unwrap();
    assert_eq!(depth_reads(&d).len(), 0, "kept on the host");
    let converted = |d: &Device<RecordingBackend>| {
        d.backend.calls.iter().filter(|c| matches!(c, Recorded::DepthLevelTexture { .. })).count()
    };
    d.set_texture(&mut m, 0, DTEX).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    assert_eq!(converted(&d), 1, "one texture while the level is unchanged");
    assert!(!d.backend.calls.iter().any(|c| matches!(c, Recorded::Upload { size: (S, S), .. })), "no upload from guest memory");
    let draw = *d.backend.draws().last().unwrap();
    assert_eq!(draw.texture_info[0].unwrap().format, D3DFormat::D24S8);
    // The guest stores into texel 1 after the switch; its byte wins.
    m.write_u32(0x8000_0000 | (DDATA + 4), 0x1122_3344).unwrap();
    d.note_write(0x8000_0000 | (DDATA + 4), 4);
    d.backend.depth_readback = Some(vec![0x5A]);
    d.block_on_resource(&mut m, DTEX).unwrap();
    assert_eq!(depth_reads(&d).len(), 1);
    assert_eq!(m.read_u32(0x8000_0000 | DDATA).unwrap(), 0x5A5A_5A5A);
    assert_eq!(m.read_u32(0x8000_0000 | (DDATA + 4)).unwrap(), 0x1122_3344);
    assert_eq!(d.stats().writeback_causes.block_on_resource, 1);
    assert_eq!(d.take_events(), []);
}


#[test]
fn a_native_depth_stencil_write_invalidates_the_cached_colour_alias() {
    struct ReplaceDepth;
    impl d3d::ExternalPass for ReplaceDepth {
        fn writes(&self) -> d3d::ExternalWrites { d3d::ExternalWrites { color: false, depth_stencil: true } }
        fn record_ranges(&mut self, t: d3d::ExternalTarget<'_>) -> usize {
            drop(t.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None, color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: t.depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.), store: wgpu::StoreOp::Store }),
                    stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0xa7), store: wgpu::StoreOp::Store }),
                }),
                timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
            }));
            1
        }
    }
    let Some((mut d, mut m, scope)) = gpu() else { return };
    set_states(&mut d, &mut m);
    d.clear(&mut m, 0, 0, 0x03, 0, 1., 0x5a).unwrap();
    textured_quad(&mut d, &mut m);
    alias(&mut m, DEPTH_BUFFER_DATA, 0x12, W, H, W * 4);
    d.set_texture(&mut m, 0, ALIAS).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    assert_eq!(back_pixel(&mut d, 20, 30), [255, 255, 0x5a, 255]);
    let plan = d3d::ExternalPlan { stage: timeline::native::Stage::World, view: 0, ranges: 1, slots: vec![] };
    d.prepare_external_pass(&mut m, plan, Box::new(ReplaceDepth)).unwrap().unwrap().commit();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    assert_eq!(back_pixel(&mut d, 20, 30), [0, 0, 0xa7, 0]);
    finish(scope);
}
