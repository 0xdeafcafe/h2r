//! The decals' cut (`render_bridge::native_decals::cut`): each record of
//! the cut callback drawn as a stage from its mode's template with its own
//! vertices, tint (`v10`) and stage-0 texture, the texture decoded from
//! guest memory ([`decode_bitmap`]).
//!
//! The template was the first guest draw of a mode; with no guest draw it
//! is a named gap until it is built from the mode's material.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use frame_types::{Slot, SlotAt};
use gfx::external::{ExternalAttachments, ExternalDraws, TargetCopy};
use native_render::decal::{VERTEX_SIZE, Vertex, decode_packed};
use native_render::decal_gpu::{DecalCache, DecalClip, PacketTarget, Prepared};
use native_render::program::ViewInputs;
use native_render::stage::{FrameOp, Stage, StageWork, frame_builder};
use render_bridge::native_decals::cut::{self, Record};

/// The per-device caches of the decals' programs, pipelines and texture
/// groups, shared by every record.
type Caches = Arc<Mutex<HashMap<wgpu::Device, DecalCache>>>;

/// The open mode's template: the state, program, constants and clips
/// every record of that mode draws with.
fn template() -> Prepared {
    unimplemented!("decals: template from the guest draw")
}

/// Unless `native-decals.cut=off`, draw the cut callback's records.
pub fn install() {
    let Ok(switches) = hooks::Switches::from_env() else { return };
    if !render_bridge::native_decals::on(&switches) || !render_bridge::native_decals::cut_on(&switches) {
        return;
    }
    cut::set_host(Arc::new(Cut { caches: Caches::default() }));
}

/// A resource `Data` word as the CPU's virtual address: the Xbox's
/// contiguous memory mapping.
fn contiguous(data: u32) -> u32 {
    0x8000_0000 | (data & 0x0FFF_FFFF)
}

/// A stage-0 texture decoded from guest memory,
/// every mip level of face 0, RGBA8.
pub struct DecalBitmap {
    key: u64,
    /// `(width, height, texels)` per level, level 0 first.
    levels: Vec<(u32, u32, Vec<u8>)>,
    /// A linear texture is fetched in texels ([`ShaderSnapshot::tex_scale`]).
    tex_scale: Option<[f32; 4]>,
}

impl DecalBitmap {
    /// Level 0's width and height.
    pub(crate) fn size(&self) -> (u32, u32) {
        (self.levels[0].0, self.levels[0].1)
    }

    /// A linear texture, fetched in texels.
    pub(crate) fn linear(&self) -> bool {
        self.tex_scale.is_some()
    }
}

/// Decoded bitmaps by key; cleared when it grows past [`BITMAP_LIMIT`].
static BITMAPS: Mutex<Option<HashMap<u64, Arc<DecalBitmap>>>> = Mutex::new(None);
const BITMAP_LIMIT: usize = 256;

/// The texture header's levels (`header` as `0x00012310` returns it):
/// its words `+0` (a texture resource), `+4` (data), `+0xc` (format) and
/// `+0x10` (size). Decoded once per map load and header words.
pub fn decode_bitmap(mem: &dyn GuestMemory, header: u32) -> Result<Arc<DecalBitmap>, &'static str> {
    let w = |o: u32| mem.u32(header.wrapping_add(o)).map_err(|_| "texture_unread");
    let (common, data, format, size) = (w(0)?, w(4)?, w(0xc)?, w(0x10)?);
    if (common >> 16) & 7 != 4 {
        return Err("texture_resource");
    }
    let mut h = std::hash::DefaultHasher::new();
    (render_bridge::native_map_work::generation(), header, [data, format, size]).hash(&mut h);
    let key = h.finish();
    let mut cached = BITMAPS.lock().unwrap_or_else(|e| e.into_inner());
    let cache = cached.get_or_insert_with(HashMap::new);
    if let Some(b) = cache.get(&key) {
        return Ok(Arc::clone(b));
    }
    let desc = gfx::TextureDesc::from_header(format, size).map_err(|_| "texture_format")?;
    if desc.kind != gfx::texture::TextureKind::D2 || desc.format.is_depth() {
        return Err("texture_kind");
    }
    let total = desc.total_size().map_err(|_| "texture_format")?;
    if total > 1 << 24 {
        return Err("texture_size");
    }
    let bytes = mem.bytes(contiguous(data), total).map_err(|_| "texture_unread")?;
    let options = gfx::DecodeOptions { bc_supported: false, signed: [false; 4] };
    let decoded = gfx::decode_texture(&desc, &bytes, None, options).map_err(|_| "texture_decode")?;
    let swap = match decoded.host {
        gfx::HostFormat::Rgba8Unorm => false,
        gfx::HostFormat::Bgra8Unorm => true,
        _ => return Err("texture_host_format"),
    };
    let mut levels: Vec<(u32, u32, Vec<u8>)> = Vec::new();
    for l in decoded.levels.iter().filter(|l| l.face == 0) {
        // Each level halves the last, as the texture's mip chain is made;
        // the chain stops at the first that does not.
        let n = levels.len() as u32;
        let (w0, h0) = levels.first().map_or((l.width, l.height), |f| (f.0, f.1));
        let want = ((w0 >> n).max(1), (h0 >> n).max(1));
        if l.level != n || (l.width, l.height) != want || l.width == 0 || l.height == 0
            || l.data.len() != 4 * l.width as usize * l.height as usize
        {
            break;
        }
        let mut rgba = l.data.clone();
        if swap {
            rgba.chunks_exact_mut(4).for_each(|p| p.swap(0, 2));
        }
        levels.push((l.width, l.height, rgba));
    }
    if levels.is_empty() {
        return Err("texture_decode");
    }
    let tex_scale = (desc.format.layout() == gfx::format::Layout::Linear && desc.width != 0 && desc.height != 0)
        .then_some([1.0 / desc.width as f32, 1.0 / desc.height as f32, 1.0, 1.0]);
    let b = Arc::new(DecalBitmap { key, levels, tex_scale });
    if cache.len() >= BITMAP_LIMIT {
        cache.clear();
    }
    cache.insert(key, Arc::clone(&b));
    Ok(b)
}

/// The GPU copies of decoded bitmaps, by device and key; cleared when it
/// grows past [`BITMAP_LIMIT`].
type Views = HashMap<(wgpu::Device, u64), wgpu::TextureView>;
static VIEWS: Mutex<Option<Views>> = Mutex::new(None);

#[allow(clippy::mutable_key_type)] // wgpu's Device is a handle; it hashes by identity
pub(crate) fn texture_view(device: &wgpu::Device, queue: &wgpu::Queue, b: &DecalBitmap) -> wgpu::TextureView {
    let mut views = VIEWS.lock().unwrap_or_else(|e| e.into_inner());
    let views = views.get_or_insert_with(HashMap::new);
    if let Some(v) = views.get(&(device.clone(), b.key)) {
        return v.clone();
    }
    let (w, h, _) = b.levels[0];
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("native decal bitmap"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: b.levels.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (level, (lw, lh, texels)) in b.levels.iter().enumerate() {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { mip_level: level as u32, ..texture.as_image_copy() },
            texels,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * lw), rows_per_image: Some(*lh) },
            wgpu::Extent3d { width: *lw, height: *lh, depth_or_array_layers: 1 },
        );
    }
    let view = texture.create_view(&Default::default());
    if views.len() >= BITMAP_LIMIT {
        views.clear();
    }
    views.insert((device.clone(), b.key), view.clone());
    view
}

/// Draws a cut callback's records from the mode's template.
struct Cut {
    caches: Caches,
}

impl cut::Host for Cut {
    fn draw(&self, mem: &mut dyn GuestMemory, r: &Record) -> Result<(), &'static str> {
        if r.count == 0 {
            // `D3DDevice_DrawVertices` with no vertices draws nothing.
            return Ok(());
        }
        let header = r.texture?;
        let bytes = r.bytes.as_deref().ok_or("vertices_unread")?;
        if bytes.len() != r.count as usize * VERTEX_SIZE {
            return Err("vertices_unread");
        }
        let mut prepared = template();
        let texture = decode_bitmap(&*mem, header)?;
        prepared.vertices = vertices_of(bytes);
        prepared.shader.vertex.defaults[10] = cut::tint(r.tint);
        prepared.shader.tex_scale = texture.tex_scale;
        let pass = DecalPass {
            prepared,
            texture,
            caches: self.caches.clone(),
            frame: render_bridge::frame_wait::presents(),
        };
        frame_builder().push(FrameOp::Stage { slot: SlotAt { slot: Slot::AfterOpaque, view: 0 }, stage: Box::new(pass) });
        Ok(())
    }
}

/// Halo 2's decal vertices from their bytes: float3 position, then the
/// `NORMPACKED3` word ([`decode_packed`]).
fn vertices_of(bytes: &[u8]) -> Vec<Vertex> {
    bytes
        .as_chunks::<VERTEX_SIZE>()
        .0
        .iter()
        .map(|v| Vertex {
            position: [
                f32::from_bits(u32::from_le_bytes(v[0..4].try_into().expect("position"))),
                f32::from_bits(u32::from_le_bytes(v[4..8].try_into().expect("position"))),
                f32::from_bits(u32::from_le_bytes(v[8..12].try_into().expect("position"))),
            ],
            diffuse: decode_packed(v[12..16].try_into().expect("packed")),
        })
        .collect()
}

struct DecalPass {
    prepared: Prepared,
    /// The record's own stage-0 texture.
    texture: Arc<DecalBitmap>,
    caches: Caches,
    /// The frame of the record: the cache keeps a texture group only
    /// while draws of recent frames use it.
    frame: u64,
}

impl Stage for DecalPass {
    fn name(&self) -> &'static str {
        "decals"
    }

    /// The record's packets appended to the pass open on these
    /// attachments, from the cache's program and pipeline.
    fn record(&mut self, t: &ExternalAttachments, view: &ViewInputs) -> StageWork {
        let Some((range, clamp)) = view.w_clip else {
            panic!("decals: a view that is not W-buffered");
        };
        let clip = DecalClip { range, clamp, w_buffered: Some(true) };
        let own = texture_view(&t.device, &t.queue, &self.texture);
        let target = PacketTarget {
            color_format: t.color_format,
            depth_format: t.depth_format,
            guest_size: view.guest_size,
            size: t.size,
            z_scale: view.z_scale,
            stage0: Some(&own),
            copy: TargetCopy::Display,
        };
        let mut caches = self.caches.lock().unwrap_or_else(|e| e.into_inner());
        let cache = caches.entry(t.device.clone()).or_default();
        cache.begin_frame(self.frame);
        match cache.draws(&t.device, &t.queue, &t.uploads, &self.prepared, clip, &target) {
            Ok(Some((packets, discards))) => StageWork::Draws(ExternalDraws { ranges: 1, packets, discards, writes: Vec::new() }),
            Ok(None) => StageWork::Draws(ExternalDraws { ranges: 1, packets: Vec::new(), discards: false, writes: Vec::new() }),
            Err(why) => panic!("decals: {why}"),
        }
    }
}
