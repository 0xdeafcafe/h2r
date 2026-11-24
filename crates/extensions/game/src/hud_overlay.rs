//! Individual HUD draws after the guest has installed the widget shader,
//! menu shader bitmaps' quads, and the fade's, the bars' and the loading
//! screen's draws (docs/graphics/native-post-hud.md, "The HUD on the
//! overlay", "The fade, the bars and the loading screen"), each captured
//! onto the overlay with the guest's combiner and blend at the draw; and
//! the passes of the menus' runs (`place_menu`).
//! The draw snapshot pins the sampled image, including the changing glyph cache.
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use d3d::backend::{ExternalAttachments, ExternalEncoded};
use d3d::{DrawAdmission, DrawSnapshot, ExternalPass, ExternalPassRequest, ExternalPlan};
use engine::memory::GuestMemory;
use frame_scene::{
    ClipRect, Overlay, OverlayBlend, OverlayMaterial, OverlayTexture, OverlayVertex, Program,
    Sampler,
};
use gfx::state::{
    BlendFactor, CompareFunc, CullMode, DepthBufferMode, FillMode, TextureAddress, TextureFilter,
};
use hud_edit::render::Art;
use map::TagId;
use native_render::overlay::{NoGlyphs, OverlayAssets, OverlayImage, OverlayPlan};
use native_render::ui_gpu::{OverlayGpu, OverlayResidency, OverlayTextures};
use render_bridge::native_hud::{HudTagQuad, HudVertices};
use timeline::native::Stage;

use crate::native_lights_glue::DeviceMemory;
use crate::native_map::NativeMap;

/// A HUD bitmap widget's quad, or a menu shader bitmap's: drawn from the
/// overlay with its tag shader's evaluated combiner. Whether it was taken.
pub fn draw_tag_quad<B: d3d::Backend + Send + 'static>(
    device: &d3d::abi::SharedDevice<B>,
    mem: &mut dyn GuestMemory,
    at: &HudTagQuad,
    vertices: &HudVertices,
) -> bool {
    capture(device, mem, Source::Tag(at), |d, m, a| {
        d.try_native_immediate_draw(m, gfx::vertex::PrimitiveType::TriangleFan, vertices, a)
    })
}

/// A HUD text widget's glyph quad, over the live glyph cache.
pub fn draw_text_quad<B: d3d::Backend + Send + 'static>(
    device: &d3d::abi::SharedDevice<B>,
    mem: &mut dyn GuestMemory,
    at: &HudTagQuad,
    vertices: &HudVertices,
) -> bool {
    capture(device, mem, Source::Glyph(at), |d, m, a| {
        d.try_native_pushed_draw(m, gfx::vertex::PrimitiveType::TriangleFan, vertices, a)
    })
}

/// A UI screen draw that names no tag: the fade, a bar, the loading
/// screen's bitmaps and spinner (docs/graphics/native-post-hud.md, "The
/// fade, the bars and the loading screen").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenDraw {
    /// The full-screen fade `0x00040890`, any of its six modes.
    Fade,
    /// A solid rectangle of `0x00052ce7`: a menu fade rectangle, a
    /// cinematic or progress bar, a split-screen divider.
    Bar,
    /// The loading screen's background, sweep or spinner (`0x000289e0`).
    Loading,
}

impl ScreenDraw {
    /// The counter of its draws the overlay took.
    pub fn counter(self) -> timeline::native::Ui {
        use timeline::native::Ui;
        match self {
            ScreenDraw::Fade => Ui::OverlayFadeQuads,
            ScreenDraw::Bar => Ui::OverlayBarQuads,
            ScreenDraw::Loading => Ui::OverlayLoadingQuads,
        }
    }
}

/// Screen draw `what`, issued by `issue` (the producer's immediate fan or
/// inline quad): drawn from the overlay with the guest's own combiner,
/// constants, blend and bound textures at the draw. Whether it was taken;
/// when not, the caller draws it as before.
pub fn draw_screen<B, F>(device: &d3d::abi::SharedDevice<B>, mem: &mut dyn GuestMemory, what: ScreenDraw, issue: F) -> bool
where
    B: d3d::Backend + Send + 'static,
    F: FnOnce(&mut d3d::device::Device<B>, &mut dyn d3d::GuestMemory, &dyn DrawAdmission) -> Result<bool, d3d::D3dError>,
{
    let taken = capture(device, mem, Source::Screen(what), issue);
    if taken {
        what.counter().count();
    } else {
        timeline::native::Ui::OverlayScreenFallback.count();
    }
    taken
}

fn capture<B, F>(device: &d3d::abi::SharedDevice<B>, mem: &mut dyn GuestMemory, source: Source<'_>, issue: F) -> bool
where
    B: d3d::Backend + Send + 'static,
    F: FnOnce(&mut d3d::device::Device<B>, &mut dyn d3d::GuestMemory, &dyn DrawAdmission) -> Result<bool, d3d::D3dError>,
{
    let admission = Widget {
        source,
        captured: Mutex::new(None),
    };
    let Ok(mut device) = device.lock() else {
        return false;
    };
    let mut mem = DeviceMemory(mem);
    let taken = issue(&mut *device, &mut mem, &admission);
    drop(device);
    if taken.ok() != Some(true) {
        return false;
    }
    if let Some(overlay) = admission
        .captured
        .into_inner()
        .unwrap_or_else(|e| e.into_inner())
    {
        let _ = render_bridge::scene_step::with_overlay(|o| o.append(&overlay));
    }
    true
}

/// What a captured draw is, and so what its item names.
#[derive(Clone, Copy, Debug)]
enum Source<'a> {
    /// A tag shader's quad: its bitmap's image.
    Tag(&'a HudTagQuad),
    /// A glyph: the glyph cache, which must be bound.
    Glyph(&'a HudTagQuad),
    /// A screen draw: its first bound texture, no tag.
    Screen(ScreenDraw),
}

impl Source<'_> {
    /// The item's shader datum (0 for a screen draw) and inputs.
    fn shader(&self) -> (u32, [f32; 4]) {
        match self {
            Source::Tag(at) | Source::Glyph(at) => (at.shader, at.hud_input),
            Source::Screen(_) => (0, [0.0; 4]),
        }
    }

    /// The item's texture from the stages bound at the draw (`first` the
    /// first sampled): `None` when a glyph's cache is not among them.
    fn texture(&self, stages: &[Option<OverlayTexture>; 4], first: Option<usize>) -> Option<OverlayTexture> {
        if let Source::Glyph(at) = self
            && !stages.contains(&Some(OverlayTexture::Bound { header: at.bitmap }))
        {
            return None;
        }
        Some(match (self, first) {
            (_, None) => OverlayTexture::None,
            (Source::Tag(at), Some(_)) => OverlayTexture::Bitmap { tag: at.bitmap, image: at.image },
            (Source::Glyph(_) | Source::Screen(_), Some(n)) => stages[n]?,
        })
    }
}

struct Widget<'a> {
    source: Source<'a>,
    captured: Mutex<Option<Arc<Overlay>>>,
}

fn sampler(draw: &d3d::ResolvedDraw, stage: usize) -> Option<Sampler> {
    let s = &draw.state.stages[stage];
    let extra = &draw.extras[stage];
    let info = draw.texture_info[stage]?;
    if info.kind != gfx::texture::TextureKind::D2
        || info.format.is_depth()
        || extra.color_sign != 0
        || extra.color_key_op != 0
        || [s.address_u, s.address_v].contains(&TextureAddress::Border)
        || (info.levels > 1 && s.mip_lod_bias != 0.0)
    {
        return None;
    }
    // Basic sampler fields remain available to scene inspection. At recording
    // the resolved stage supplies all three address modes, filters and mip levels.
    Some(Sampler {
        wrap: s.address_u == TextureAddress::Wrap,
        point: s.min_filter == TextureFilter::Point,
    })
}

/// The draw's blend and the constant colour its factors name (RGBA; zero
/// when they name none). The fade's modes 3 to 6 blend by the constant.
fn blend(rs: &gfx::state::RenderState) -> (OverlayBlend, [f32; 4]) {
    if !rs.blend_enable {
        return (OverlayBlend::Off, [0.0; 4]);
    }
    let constant = |v| {
        matches!(
            v,
            BlendFactor::ConstantColor
                | BlendFactor::InvConstantColor
                | BlendFactor::ConstantAlpha
                | BlendFactor::InvConstantAlpha
        )
    };
    let colour = if constant(rs.src_blend) || constant(rs.dst_blend) {
        let c = gfx::translate::blend_constant(rs);
        [c.r, c.g, c.b, c.a].map(|v| v as f32)
    } else {
        [0.0; 4]
    };
    let on = OverlayBlend::On {
        source: rs.src_blend.xbox_value(),
        dest: rs.dst_blend.xbox_value(),
        equation: rs.blend_op.xbox_value(),
    };
    (on, colour)
}

fn clip(r: d3d::backend::PixelRect) -> ClipRect {
    ClipRect {
        x: r.x,
        y: r.y,
        width: r.width,
        height: r.height,
    }
}

fn faces_visible(corners: &[OverlayVertex; 4], cull: CullMode) -> bool {
    if cull == CullMode::None {
        return true;
    }
    let [a, b, c, d] = corners.map(|v| v.position);
    let visible = |p: [f32; 2], q: [f32; 2]| {
        let cross = (p[0] - a[0]) * (q[1] - a[1]) - (p[1] - a[1]) * (q[0] - a[0]);
        match cull {
            CullMode::Cw => cross < 0.0,
            _ => cross > 0.0,
        }
    };
    visible(b, c) && visible(c, d)
}

impl DrawAdmission for Widget<'_> {
    fn admit(&self, snapshot: &DrawSnapshot<'_>) -> Option<(ExternalPlan, Box<dyn ExternalPass>)> {
        let draw = snapshot.draw;
        let rs = &draw.state.render;
        if snapshot.color_header == 0
            || !draw.clip_rects.is_empty()
            || draw.writes_depth_stencil()
            || (rs.depth_mode != DepthBufferMode::Disabled && rs.depth_func != CompareFunc::Always)
            || rs.stencil.enable
            || rs.fill_mode != FillMode::Solid
            || draw.hw.zpass_count_enable
            || rs.color_write.xbox_value() == 0
        {
            return None;
        }
        let words = d3d::nv2a::overlay_shader_words(draw)?;
        if !native_render::ui_gpu::hud_shader_supported(&words) {
            return None;
        }
        let sampled = snapshot.sampled_stages;
        let first = sampled.iter().position(|&v| v);
        let mut shader_stages = [None; 4];
        let mut samplers = [Sampler::default(); 4];
        let mut alpha_kill = 0;
        for (stage, &active) in sampled.iter().enumerate() {
            if !active {
                continue;
            }
            samplers[stage] = sampler(draw, stage)?;
            let header = snapshot.bound_textures[stage];
            if header == 0 {
                return None;
            }
            shader_stages[stage] = Some(OverlayTexture::Bound { header });
            if draw.extras[stage].alpha_kill & 4 != 0 {
                alpha_kill |= 1u8 << stage;
            }
        }
        let texture = self.source.texture(&shader_stages, first)?;
        let (tag, hud_input) = self.source.shader();
        let (blend, blend_constant) = blend(rs);
        let evaluated = d3d::nv2a::overlay_vertices(draw, sampled)?;
        let corners = evaluated.map(|(position, coords, color)| OverlayVertex {
            position,
            uv: coords[first.unwrap_or(0)],
            color,
        });
        if !faces_visible(&corners, rs.cull_mode) {
            return None;
        }
        let material = OverlayMaterial {
            program: Program::Shader { tag },
            texture,
            sampler: first.map_or_else(Sampler::default, |n| samplers[n]),
            blend,
        };
        let mut overlay = Overlay::new([draw.target.width, draw.target.height]);
        overlay
            .push_quad(material, clip(draw.clip?), corners)
            .ok()?;
        let item = &mut overlay.items[0];
        item.alpha_test = rs
            .alpha_test_enable
            .then_some((rs.alpha_func.xbox_value(), rs.alpha_ref));
        item.alpha_kill = alpha_kill;
        item.shader_stages = shader_stages;
        item.stage_uv = Some(Box::new(evaluated.map(|(_, coords, _)| coords)));
        item.glyph_cache = draw.glyph_cache;
        item.hud_input = hud_input;
        item.blend_constant = blend_constant;
        item.shader_words = Some(Box::new(words));
        item.color_write = rs.color_write.xbox_value();
        let overlay = Arc::new(overlay);
        *self.captured.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::clone(&overlay));
        let pass = WidgetPass {
            draw: Arc::new(draw.clone()),
            overlay,
            sampled,
        };
        Some((
            ExternalPlan {
                stage: Stage::Ui,
                view: 4,
                ranges: 1,
                slots: vec![],
            },
            Box::new(pass),
        ))
    }
}

struct WidgetPass {
    draw: Arc<d3d::ResolvedDraw>,
    overlay: Arc<Overlay>,
    sampled: [bool; 4],
}

struct Bound<'a> {
    views: &'a [Option<wgpu::TextureView>; 4],
    samplers: [Option<wgpu::Sampler>; 4],
    first: Option<usize>,
}
impl OverlayTextures for Bound<'_> {
    fn view(&self, _: OverlayTexture) -> Option<&wgpu::TextureView> {
        self.views[self.first?].as_ref()
    }
    fn stage(&self, stage: usize, _: OverlayTexture) -> Option<&wgpu::TextureView> {
        self.views.get(stage)?.as_ref()
    }
    fn stage_sampler(&self, stage: usize, _: OverlayTexture) -> Option<&wgpu::Sampler> {
        self.samplers.get(stage)?.as_ref()
    }
}

struct Renderer {
    device: wgpu::Device,
    gpu: OverlayGpu,
    samplers: HashMap<gfx::translate::SamplerKey, wgpu::Sampler>,
}
static GPU: Mutex<Option<Renderer>> = Mutex::new(None);

/// Same window-coordinate fixup as the vertex translator in vsh/wgsl.rs.
fn raster_position(window: f32, size: u32, shift: f32) -> f32 {
    if shift == 0.0 {
        return (window * 16.0).trunc() / 16.0;
    }
    let pos = window - shift;
    if window > size as f32 - 0.5 {
        pos.max(size as f32)
    } else {
        pos
    }
}

impl WidgetPass {
    fn record(
        &self,
        t: &ExternalAttachments,
        request: ExternalPassRequest,
    ) -> Result<ExternalEncoded, &'static str> {
        for (sampled, view) in self.sampled.iter().zip(&t.stages) {
            if *sampled && view.as_ref().is_some_and(|v| {
                v.texture() == t.color.texture() || v.texture() == t.depth.texture()
            }) {
                return Err("HUD texture aliases its render target");
            }
        }
        let mut overlay = (*self.overlay).clone();
        let (w, h) = request.surface_size;
        if overlay.size != [w, h] {
            return Err("HUD surface size");
        }
        let scale =
            gfx::raster::RenderScale::between(request.surface_size, t.size).ok_or("HUD scale")?;
        let draw = self.draw.for_copy(t.copy);
        let transform = draw.ui_fit.map_or([1., 1., 0., 0.], |f| f.transform);
        let shift = scale.shift();
        for v in &mut overlay.vertices {
            for i in 0..2 {
                let p = v.position[i] * transform[i] + transform[i + 2];
                v.position[i] = raster_position(p, overlay.size[i], shift[i]);
            }
        }
        overlay.items[0].clip = clip(draw.display_rect(draw.clip.ok_or("HUD clip")?));
        let plan = OverlayPlan::new(&overlay, [t.size.0, t.size.1], &NoGlyphs);
        let mut lock = GPU.lock().unwrap_or_else(|e| e.into_inner());
        if lock
            .as_ref()
            .is_none_or(|renderer| renderer.device != t.device)
        {
            *lock = Some(Renderer {
                device: t.device.clone(),
                gpu: OverlayGpu::new(&t.device, &t.queue),
                samplers: HashMap::new(),
            });
        }
        let renderer = lock.as_mut().ok_or("HUD device")?;
        let caps = gfx::translate::Capabilities::from_features(t.device.features());
        let samplers = std::array::from_fn(|n| {
            self.sampled[n].then(|| {
                let key = gfx::translate::SamplerKey::from_stage(&draw.state.stages[n], caps);
                renderer
                    .samplers
                    .entry(key)
                    .or_insert_with(|| t.device.create_sampler(&key.descriptor()))
                    .clone()
            })
        });
        let textures = Bound {
            views: &t.stages,
            samplers,
            first: self.sampled.iter().position(|&v| v),
        };
        let (encoded, stats) = renderer
            .gpu
            .prepare(&t.device, &t.color, &plan, &textures)?;
        if encoded.ranges != 1 || stats.batches != 1 {
            return Err("HUD material refused");
        }
        if t.copy == d3d::TargetCopy::Display {
            native_render::ui_gpu::count_overlay(&stats);
        }
        Ok(encoded)
    }
}

impl ExternalPass for WidgetPass {
    fn resolved_fallback(&self) -> Option<Arc<d3d::ResolvedDraw>> {
        Some(Arc::clone(&self.draw))
    }

    fn writes(&self) -> d3d::ExternalWrites {
        d3d::ExternalWrites {
            color: true,
            depth_stencil: false,
        }
    }

    fn colour_only(&self) -> bool {
        true
    }

    fn record_encoded(
        &mut self,
        t: &ExternalAttachments,
        request: ExternalPassRequest,
    ) -> Option<Result<ExternalEncoded, &'static str>> {
        let result = self.record(t, request);
        if result.is_err() {
            timeline::native::Ui::OverlayHudTagRecordFailed.count();
        } else if t.copy == d3d::TargetCopy::Display {
            timeline::native::Ui::OverlayHudTagRecorded.count();
        }
        Some(result)
    }
}

/// A map read for the menus' bitmap images: the map and its decoded
/// bitmaps.
struct MenuArt {
    map: NativeMap,
    art: Art,
}

/// The map whose overlay assets are held, the assets (one generation a
/// map, so the resident textures are not replaced between passes), and the
/// map read for the menus' images once it has loaded.
#[derive(Default)]
struct Slot {
    map: Option<(String, u64)>,
    art: Option<Arc<Mutex<MenuArt>>>,
    assets: Arc<Mutex<OverlayAssets>>,
}

static SLOT: LazyLock<Mutex<Slot>> = LazyLock::new(Mutex::default);

/// The map's overlay assets, shared by the menus' passes; `None` with no
/// map loaded.
pub fn assets(mem: &dyn GuestMemory) -> Option<Arc<Mutex<OverlayAssets>>> {
    slot(mem).map(|(_, assets)| assets)
}

/// The slot for the loaded map: its read for the menus' images, once
/// loaded, and its overlay assets; starts the read at the first ask on a
/// map; `None` with no map loaded.
fn slot(mem: &dyn GuestMemory) -> Option<(Option<Arc<Mutex<MenuArt>>>, Arc<Mutex<OverlayAssets>>)> {
    let name = crate::health::loaded_map_name(mem).ok().filter(|n| !n.is_empty())?;
    let generation = render_bridge::native_map_work::generation();
    let mut slot = SLOT.lock().unwrap_or_else(|e| e.into_inner());
    if slot.map.as_ref().is_none_or(|(n, g)| *n != name || *g != generation) {
        slot.map = Some((name.clone(), generation));
        slot.art = None;
        slot.assets = Arc::new(Mutex::new(OverlayAssets { generation, ..OverlayAssets::default() }));
        spawn_load(name, generation);
    }
    Some((slot.art.clone(), slot.assets.clone()))
}

/// Reads map `name` off the game thread for the menus' images.
fn spawn_load(name: String, generation: u64) {
    let spawned = std::thread::Builder::new().name("menu overlay load".into()).spawn(move || {
        let art = match crate::native_map::open(&name) {
            Ok((map, files)) => MenuArt { map, art: Art::new(files) },
            Err(e) => {
                eprintln!("menu overlay: {name}: the map did not open ({e}); the game draws its bitmaps");
                return;
            }
        };
        let mut slot = SLOT.lock().unwrap_or_else(|e| e.into_inner());
        if slot.map.as_ref().is_some_and(|(n, g)| *n == name && *g == generation) {
            slot.art = Some(Arc::new(Mutex::new(art)));
        }
    });
    if let Err(e) = spawned {
        eprintln!("menu overlay: {e}: the map did not open; the game draws its bitmaps");
    }
}

/// Decodes the images `part`'s quads name into `assets`: whether each did.
fn decode_images(a: &mut MenuArt, part: &Overlay, assets: &mut OverlayAssets) -> bool {
    for item in &part.items {
        let OverlayTexture::Bitmap { tag, image } = item.material.texture else { continue };
        if assets.images.contains_key(&(tag, image)) {
            continue;
        }
        let Ok((width, height, rgba)) = a.art.image(&mut a.map, TagId(tag), usize::from(image)) else {
            return false;
        };
        assets.images.insert((tag, image), OverlayImage { width, height, rgba: rgba.into() });
    }
    true
}

/// Places a pass drawing `overlay` from `assets` at this point in the
/// frame: whether the device took it.
pub fn place<B: d3d::Backend + Send + 'static>(
    device: &d3d::abi::SharedDevice<B>,
    mem: &mut dyn GuestMemory,
    overlay: Arc<Overlay>,
    assets: Arc<Mutex<OverlayAssets>>,
) -> bool {
    let pass = OverlayPass { overlay, assets };
    device.lock().ok().is_some_and(|mut d| {
        matches!(
            d.external_pass_now(&mut crate::native_lights_glue::DeviceMemory(mem), Box::new(pass)),
            Ok(true)
        )
    })
}

/// A run of menu widgets (`render_bridge::native_hud::ui_overlay`), drawn
/// from the overlay at this point in the frame with the map's assets:
/// whether it was placed. The bitmap images its quads name are decoded
/// first, from the map read for the menus; while that loads, or when an
/// image does not decode, the run is not placed and the game draws it.
pub fn place_menu<B: d3d::Backend + Send + 'static>(
    device: &d3d::abi::SharedDevice<B>,
    mem: &mut dyn GuestMemory,
    part: &Overlay,
) -> bool {
    let Some((art, assets)) = slot(mem) else { return false };
    let bitmaps = part.items.iter().any(|i| matches!(i.material.texture, OverlayTexture::Bitmap { .. }));
    if bitmaps {
        let Some(art) = art else { return false };
        let mut a = art.lock().unwrap_or_else(|e| e.into_inner());
        if !decode_images(&mut a, part, &mut assets.lock().unwrap_or_else(|e| e.into_inner())) {
            return false;
        }
    }
    place(device, mem, Arc::new(part.clone()), assets)
}

/// The renderer's overlay programs and resident textures, on the device
/// last drawn with.
static RENDERER: LazyLock<Mutex<Option<(wgpu::Device, OverlayGpu, OverlayResidency)>>> =
    LazyLock::new(|| Mutex::new(None));

/// An overlay's items, drawn where the pass was placed in the frame.
struct OverlayPass {
    overlay: Arc<Overlay>,
    assets: Arc<Mutex<OverlayAssets>>,
}

impl d3d::ExternalPass for OverlayPass {
    fn colour_only(&self) -> bool {
        true
    }

    fn writes(&self) -> d3d::ExternalWrites {
        d3d::ExternalWrites { color: true, depth_stencil: false }
    }

    fn encode(&mut self, t: d3d::ExternalTarget<'_>) {
        let mut r = RENDERER.lock().unwrap_or_else(|e| e.into_inner());
        if r.as_ref().is_none_or(|(d, _, _)| d != t.device) {
            *r = Some((t.device.clone(), OverlayGpu::new(t.device, t.queue), OverlayResidency::default()));
        }
        let Some((_, gpu, residency)) = r.as_mut() else { return };
        let assets = self.assets.lock().unwrap_or_else(|e| e.into_inner());
        residency.sync(t.device, t.queue, &assets);
        let size = t.color.texture().size();
        let plan = OverlayPlan::new(&self.overlay, [size.width, size.height], &assets.glyphs);
        drop(assets);
        if let Ok((encoded, stats)) = gpu.prepare(t.device, t.color, &plan, residency) {
            (encoded.commands)(&mut *t.encoder);
            if t.copy == d3d::TargetCopy::Display {
                native_render::ui_gpu::count_overlay(&stats);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scaled_raster_keeps_far_edges_and_reference_snaps_to_sixteenths() {
        assert_eq!(raster_position(3.09375, 8, 0.0), 3.0625);
        assert_eq!(raster_position(3.0, 8, 0.375), 2.625);
        assert_eq!(raster_position(7.75, 8, 0.375), 8.0);
    }
    #[test]
    fn both_triangles_of_a_widget_must_survive_culling() {
        let quad = [[1., 1.], [5., 1.], [5., 3.], [1., 3.]].map(|position| OverlayVertex {
            position,
            ..Default::default()
        });
        assert!(faces_visible(&quad, CullMode::Ccw));
        assert!(!faces_visible(&quad, CullMode::Cw));
        let mut crossed = quad;
        crossed.swap(2, 3);
        assert!(!faces_visible(&crossed, CullMode::Ccw));
    }

    /// Each blend the fade's modes set (invented colours): the factors
    /// and equation as the draw has them, and the constant colour only
    /// when a factor names it, its alpha splatted for an alpha factor.
    #[test]
    fn every_fade_blend_is_taken_with_its_constant() {
        use gfx::state::{BlendOp, RenderState};
        let state = |src, dst, op, colour| RenderState {
            blend_enable: true,
            src_blend: src,
            dst_blend: dst,
            blend_op: op,
            blend_color: colour,
            ..RenderState::default()
        };
        let on = |s: BlendFactor, d: BlendFactor, o: BlendOp| OverlayBlend::On {
            source: s.xbox_value(),
            dest: d.xbox_value(),
            equation: o.xbox_value(),
        };
        use BlendFactor::*;
        let modes = [
            (One, InvSrcAlpha, BlendOp::Add, 0x4020_1008, [0.0; 4]),
            (One, One, BlendOp::ReverseSubtract, 0x4020_1008, [0.0; 4]),
            (InvDestColor, InvConstantColor, BlendOp::Max, 0xff80_4000, [128.0 / 255.0, 64.0 / 255.0, 0.0, 1.0]),
            (InvDestColor, InvConstantColor, BlendOp::Min, 0x0000_00ff, [0.0, 0.0, 1.0, 0.0]),
            (InvDestColor, InvConstantColor, BlendOp::Add, 0x80ff_ffff, [1.0, 1.0, 1.0, 128.0 / 255.0]),
            (One, InvConstantColor, BlendOp::Add, 0x0010_2030, [16.0 / 255.0, 32.0 / 255.0, 48.0 / 255.0, 0.0]),
        ];
        // An alpha factor splats the constant's alpha.
        let (_, c) = blend(&state(ConstantAlpha, InvConstantAlpha, BlendOp::Add, 0x4000_0000));
        assert!(c.iter().all(|v| (v - 64.0 / 255.0).abs() < 1e-6), "{c:?}");
        for (n, (src, dst, op, colour, constant)) in modes.into_iter().enumerate() {
            let (b, c) = blend(&state(src, dst, op, colour));
            assert_eq!(b, on(src, dst, op), "mode {}", n + 1);
            for (got, want) in c.iter().zip(constant) {
                assert!((got - want).abs() < 1e-6, "mode {}: {c:?}", n + 1);
            }
        }
        let off = RenderState { blend_color: 0xffff_ffff, ..RenderState::default() };
        assert_eq!(blend(&off), (OverlayBlend::Off, [0.0; 4]));
    }

    /// A tag quad names its bitmap's image; a glyph needs its cache bound
    /// and names it; a screen draw names its first bound stage, or none.
    #[test]
    fn each_source_names_its_texture() {
        let at = HudTagQuad { bitmap: 0x40, image: 3, shader: 0xe000_0001, hud_input: [0.5; 4] };
        let bound = [None, Some(OverlayTexture::Bound { header: 0x40 }), None, None];
        assert_eq!(
            Source::Tag(&at).texture(&bound, Some(1)),
            Some(OverlayTexture::Bitmap { tag: 0x40, image: 3 })
        );
        assert_eq!(Source::Glyph(&at).texture(&bound, Some(1)), bound[1]);
        let other = [Some(OverlayTexture::Bound { header: 0x50 }), None, None, None];
        assert_eq!(Source::Glyph(&at).texture(&other, Some(0)), None, "the cache is not bound");
        let screen = Source::Screen(ScreenDraw::Loading);
        assert_eq!(screen.texture(&other, Some(0)), other[0]);
        assert_eq!(Source::Screen(ScreenDraw::Fade).texture(&[None; 4], None), Some(OverlayTexture::None));
        assert_eq!(screen.shader(), (0, [0.0; 4]));
        assert_eq!(Source::Tag(&at).shader(), (0xe000_0001, [0.5; 4]));
    }
}
