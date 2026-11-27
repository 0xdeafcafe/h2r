//! The host of `render_bridge::native_flare_draw`: each glow's passes
//! placed on the shared device where the guest's glow `0x00046ea0` would
//! draw, over the bound view colour (docs/graphics/d3d-visibility.md,
//! "The glow in Rust").

use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use native_render::effect_material::{RASTERIZER, texture_header};
use native_render::flare_draw_gpu::{Bitmap, FlareDrawGpu, FlareTarget, MASK_DEPTH};
use native_render::flare_gpu::{self, DepthTest};
use render_bridge::native_flare_draw::{self, GlowDraw, Host};
use render_bridge::native_flare_query::Quad;

use crate::native_lights_glue::DeviceMemory;

/// The glow's quads, squares and bitmap on the device last recorded with.
static GPU: Mutex<Option<(wgpu::Device, FlareDrawGpu)>> = Mutex::new(None);

/// The rasterizer data's bitmap the masked sample (stage 0) and the
/// down-sample (stage 1) bind, image 0 (`0x00047331`, `0x000462c4`).
pub const GLOW_BITMAP: u32 = 0x34;

/// The last bitmap decoded, by its key.
static BITMAP: Mutex<Option<Arc<Bitmap>>> = Mutex::new(None);

/// The glow bitmap as `0x000143c0` binds it ([`texture_header`]): its
/// texture's level 0 decoded from guest memory (`map` the device's
/// `Data` mapping) as the D3D path decodes it, RGBA8. Decoded again only
/// when the header, its identity words or its bytes change.
fn glow_bitmap(mem: &dyn GuestMemory, map: d3d::AddressMap) -> Result<Arc<Bitmap>, &'static str> {
    let word = |a: u32| mem.u32(a).ok();
    let rasterizer = word(RASTERIZER).ok_or("bitmap_unread")?;
    let datum = word(rasterizer.wrapping_add(GLOW_BITMAP)).ok_or("bitmap_unread")?;
    let header = texture_header(&word, datum, 0)?;
    let w = |o: u32| word(header.wrapping_add(o)).ok_or("bitmap_unread");
    let (common, data, format, size) = (w(0)?, w(4)?, w(0xc)?, w(0x10)?);
    if (common >> 16) & 7 != 4 {
        return Err("bitmap_not_texture");
    }
    let desc = gfx::TextureDesc::from_header(format, size).map_err(|_| "bitmap_format")?;
    if desc.kind != gfx::texture::TextureKind::D2 {
        return Err("bitmap_kind");
    }
    let total = desc.total_size().map_err(|_| "bitmap_format")?;
    if total > 1 << 24 {
        return Err("bitmap_size");
    }
    let bytes = mem.bytes(map.to_virtual(data), total).map_err(|_| "bitmap_unread")?;
    let mut h = std::hash::DefaultHasher::new();
    (header, [data, format, size], &bytes).hash(&mut h);
    let key = h.finish();
    let mut cached = BITMAP.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(b) = cached.as_ref().filter(|b| b.key == key) {
        return Ok(Arc::clone(b));
    }
    let options = gfx::DecodeOptions { bc_supported: false, signed: [false; 4] };
    let decoded = gfx::decode_texture(&desc, &bytes, None, options).map_err(|_| "bitmap_decode")?;
    let level = decoded.levels.iter().find(|l| l.face == 0 && l.level == 0).ok_or("bitmap_decode")?;
    let mut rgba = level.data.clone();
    match decoded.host {
        gfx::HostFormat::Rgba8Unorm => {}
        gfx::HostFormat::Bgra8Unorm => rgba.chunks_exact_mut(4).for_each(|p| p.swap(0, 2)),
        _ => return Err("bitmap_decode"),
    }
    let b = Arc::new(Bitmap { key, size: (level.width, level.height), rgba });
    *cached = Some(Arc::clone(&b));
    Ok(b)
}

struct GlowPass {
    draw: GlowDraw,
    bitmap: Arc<Bitmap>,
    test: DepthTest,
    /// Counted once, however many target copies record.
    dropped: bool,
}

impl d3d::ExternalPass for GlowPass {
    fn writes(&self) -> d3d::ExternalWrites {
        d3d::ExternalWrites { color: true, depth_stencil: false }
    }

    /// The W clip the masked sample's fan takes.
    fn depth_clip(&mut self, clip: d3d::DepthClip) {
        self.test = DepthTest { w_buffered: clip.w_buffered.unwrap_or(true), range: clip.range, clamp: clip.clamp };
    }

    /// Drawn on every copy (the reference and the display).
    fn encode(&mut self, t: d3d::ExternalTarget<'_>) {
        let mut gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
        if gpu.as_ref().is_none_or(|(d, _)| d != t.device) {
            *gpu = Some((t.device.clone(), FlareDrawGpu::new()));
        }
        let Some((_, pass)) = gpu.as_mut() else { return };
        let mask = Quad { x0: 0, y0: 0, x1: 1, y1: 1, z: MASK_DEPTH, w: MASK_DEPTH, area: 1 };
        // A fan the W clip culls passes nowhere.
        let reference = flare_gpu::reference(&mask, self.test, t.z_scale).unwrap_or(2.0);
        let target = FlareTarget {
            device: t.device,
            queue: t.queue,
            uploads: t.uploads,
            color: t.color,
            depth: t.depth,
            guest: t.guest_size,
        };
        let recorded = pass.record(target, t.encoder, reference, &self.draw, &self.bitmap);
        if let (Err(why), false) = (recorded, self.dropped) {
            self.dropped = true;
            native_flare_draw::dropped(why);
        }
    }
}

struct Draw<B: d3d::Backend + Send + 'static> {
    device: d3d::abi::SharedDevice<B>,
}

impl<B: d3d::Backend + Send + 'static> Host for Draw<B> {
    fn draw(&self, mem: &mut dyn GuestMemory, draw: GlowDraw) -> bool {
        let Ok(mut device) = self.device.lock() else { return false };
        // Without its bitmap the glow is not placed: the guest body draws it.
        let Ok(bitmap) = glow_bitmap(&*mem, device.config().address_map) else { return false };
        let pass = GlowPass {
            draw,
            bitmap,
            test: DepthTest { w_buffered: true, range: [0.0, f32::MAX], clamp: true },
            dropped: false,
        };
        matches!(device.external_pass_now(&mut DeviceMemory(mem), Box::new(pass)), Ok(true))
    }
}

pub fn install<B: d3d::Backend + Send + 'static>(device: &d3d::abi::SharedDevice<B>) {
    if !hooks::Switches::from_env().is_ok_and(|s| native_flare_draw::on(&s)) {
        return;
    }
    native_flare_draw::set_host(Arc::new(Draw { device: Arc::clone(device) }));
}
