//! The host of `render_bridge::native_flare_draw`: each glow's passes as a
//! stage where the guest's glow `0x00046ea0` would draw, over the bound
//! view colour (docs/graphics/d3d-visibility.md,
//! "The glow in Rust").

use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use native_render::effect_material::{RASTERIZER, texture_header};
use native_render::flare_draw_gpu::{Bitmap, FlareDrawGpu, FlareTarget, MASK_DEPTH};
use native_render::flare_gpu::{self, DepthTest};
use render_bridge::native_flare_draw::{self, GlowDraw, Host};
use render_bridge::native_flare_query::Quad;

use frame_types::Slot;

/// The glow's quads, squares and bitmap on the device last recorded with.
static GPU: Mutex<Option<(wgpu::Device, FlareDrawGpu)>> = Mutex::new(None);

/// The rasterizer data's bitmap the masked sample (stage 0) and the
/// down-sample (stage 1) bind, image 0 (`0x00047331`, `0x000462c4`).
pub const GLOW_BITMAP: u32 = 0x34;

/// The last bitmap decoded, by its key.
static BITMAP: Mutex<Option<Arc<Bitmap>>> = Mutex::new(None);

/// The glow bitmap as `0x000143c0` binds it ([`texture_header`]): its
/// texture's level 0 decoded from guest memory, RGBA8. Decoded again only
/// when the header, its identity words or its bytes change.
fn glow_bitmap(mem: &dyn GuestMemory) -> Result<Arc<Bitmap>, &'static str> {
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
    let bytes = mem.bytes(crate::native_targets_glue::data_address(data), total).map_err(|_| "bitmap_unread")?;
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
    draw: Arc<GlowDraw>,
    bitmap: Arc<Bitmap>,
}

impl native_render::Stage for GlowPass {
    fn name(&self) -> &'static str {
        "flare glow"
    }

    fn record(&mut self, t: &gfx::external::ExternalAttachments, view: &native_render::program::ViewInputs) -> native_render::StageWork {
        // The masked sample's fan takes the view's W clip.
        let test = match view.w_clip {
            Some((range, clamp)) => DepthTest { w_buffered: true, range, clamp },
            None => DepthTest { w_buffered: true, range: [0.0, f32::MAX], clamp: true },
        };
        let mask = Quad { x0: 0, y0: 0, x1: 1, y1: 1, z: MASK_DEPTH, w: MASK_DEPTH, area: 1 };
        // A fan the W clip culls passes nowhere.
        let reference = flare_gpu::reference(&mask, test, view.z_scale).unwrap_or(2.0);
        let (draw, bitmap, guest) = (Arc::clone(&self.draw), Arc::clone(&self.bitmap), view.guest_size);
        let (device, queue, uploads, color, depth) =
            (t.device.clone(), t.queue.clone(), t.uploads.clone(), t.color.clone(), t.depth.clone());
        crate::native_targets_glue::encoded(move |encoder| {
            let mut gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
            if gpu.as_ref().is_none_or(|(d, _)| *d != device) {
                *gpu = Some((device.clone(), FlareDrawGpu::new()));
            }
            let Some((_, pass)) = gpu.as_mut() else { return };
            let target = FlareTarget { device: &device, queue: &queue, uploads: &uploads, color: &color, depth: &depth, guest };
            if let Err(why) = pass.record(target, encoder, reference, &draw, &bitmap) {
                native_flare_draw::dropped(why);
            }
        })
    }
}

struct Draw;

impl Host for Draw {
    fn draw(&self, mem: &mut dyn GuestMemory, draw: GlowDraw) -> bool {
        // Without its bitmap the glow is not placed: it is dropped,
        // `scene_dropped_flare_glow_placed`.
        let Ok(bitmap) = glow_bitmap(&*mem) else { return false };
        crate::native_targets_glue::push(&*mem, Slot::Transparents, GlowPass { draw: Arc::new(draw), bitmap });
        true
    }
}

pub fn install() {
    native_flare_draw::set_host(Arc::new(Draw));
}
