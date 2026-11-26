//! Individual HUD draws after the guest has installed the widget shader,
//! menu shader bitmaps' quads, and the fade's, the bars' and the loading
//! screen's draws (docs/graphics/native-post-hud.md, "The HUD on the
//! overlay", "The fade, the bars and the loading screen"), each captured
//! onto the overlay with the guest's combiner and blend at the draw; and
//! the passes of the menus' runs (`place_menu`).
//! The draw snapshot pins the sampled image, including the changing glyph cache.
//!
//! The overlay's glyphs are every list's to share: [`fonts`] is the disc's fonts
//! as the game serves them, [`assets`] the loaded map's overlay assets and
//! [`fill_glyphs`] puts a list's glyphs into their pages, each decoded once;
//! [`place_menu`] fills before it places. [`menu_text`] shapes a menu label from
//! the same fonts.

pub(crate) mod menu_text;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use d3d::backend::{ExternalAttachments, ExternalEncoded};
use d3d::{DrawAdmission, DrawSnapshot, ExternalPass, ExternalPassRequest, ExternalPlan};
use engine::memory::GuestMemory;
use frame_scene::{
    ClipRect, Overlay, OverlayBlend, OverlayKind, OverlayMaterial, OverlayTexture, OverlayVertex, Program,
    Sampler,
};
use gfx::state::{
    BlendFactor, CompareFunc, CullMode, DepthBufferMode, FillMode, TextureAddress, TextureFilter,
};
use hud_edit::render::{Art, Fonts};
use map::TagId;
use native_render::overlay::{GlyphPages, NoGlyphs, OverlayAssets, OverlayImage, OverlayPlan};
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
    /// A HUD or UI widget fan captured from the device's bound stages.
    Widget,
}

impl ScreenDraw {
    /// The counter of its draws the overlay took.
    pub fn counter(self) -> timeline::native::Ui {
        use timeline::native::Ui;
        match self {
            ScreenDraw::Fade => Ui::OverlayFadeQuads,
            ScreenDraw::Bar => Ui::OverlayBarQuads,
            ScreenDraw::Loading => Ui::OverlayLoadingQuads,
            ScreenDraw::Widget => Ui::OverlayWidgetQuads,
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
    let taken = capture(device, mem, Source::Screen, issue);
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
    Screen,
}

impl Source<'_> {
    /// The item's shader datum (0 for a screen draw) and inputs.
    fn shader(&self) -> (u32, [f32; 4]) {
        match self {
            Source::Tag(at) | Source::Glyph(at) => (at.shader, at.hud_input),
            Source::Screen => (0, [0.0; 4]),
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
            (Source::Glyph(_) | Source::Screen, Some(n)) => stages[n]?,
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
            members: vec![Member { draw: Arc::new(draw.clone()), overlay }],
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

/// One widget quad: the guest draw it stands in for and its one-quad overlay.
#[derive(Clone)]
struct Member {
    draw: Arc<d3d::ResolvedDraw>,
    overlay: Arc<Overlay>,
}

/// One widget quad, or a run of adjacent ones that share a target and
/// sampled textures ([`ExternalPass::run_with`]): recorded as one overlay,
/// one upload and one render pass, the quads in draw order.
struct WidgetPass {
    members: Vec<Member>,
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
        let (w, h) = request.surface_size;
        let scale =
            gfx::raster::RenderScale::between(request.surface_size, t.size).ok_or("HUD scale")?;
        let shift = scale.shift();
        let mut overlay = Overlay::new([w, h]);
        for member in &self.members {
            let mut one = (*member.overlay).clone();
            if one.size != [w, h] {
                return Err("HUD surface size");
            }
            let draw = member.draw.for_copy(t.copy);
            let transform = draw.ui_fit.map_or([1., 1., 0., 0.], |f| f.transform);
            for v in &mut one.vertices {
                for i in 0..2 {
                    let p = v.position[i] * transform[i] + transform[i + 2];
                    v.position[i] = raster_position(p, one.size[i], shift[i]);
                }
            }
            one.items[0].clip = clip(draw.display_rect(draw.clip.ok_or("HUD clip")?));
            let base = overlay.vertices.len() as u32;
            overlay.vertices.extend(one.vertices);
            for mut item in one.items {
                item.vertices = item.vertices.start + base..item.vertices.end + base;
                overlay.items.push(item);
            }
        }
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
        // The run's quads share their sampled stages' samplers (`run_with`).
        let draw = self.members[0].draw.for_copy(t.copy);
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
        // Every quad of the run drew, or none did: a refused one sends
        // the run back to its packets, in order.
        let quads = u32::try_from(self.members.len()).map_err(|_| "HUD run length")?;
        if stats.batches as usize != plan.batches.len() || stats.items != quads || stats.refused_material + stats.refused_texture != 0 {
            return Err("HUD material refused");
        }
        let mut encoded = encoded;
        encoded.ranges = self.members.len();
        if t.copy == d3d::TargetCopy::Display {
            native_render::ui_gpu::count_overlay(&stats);
        }
        Ok(encoded)
    }
}

impl ExternalPass for WidgetPass {
    // The overlay and the packet are immutable; `record` takes the
    // attachments, and every `Bound` stage's view, from the copy being
    // recorded, so a kept scene records it again from its own captured
    // textures. Its renderer state is per device, not per frame.
    fn replays_scene(&self) -> bool {
        true
    }

    /// The next quad's pass after this run's last, when both sample the
    /// same textures through the same samplers on the same target size:
    /// one overlay, one pass. A texture or sampler change splits the run;
    /// blend, scissor and shader state are per quad inside it.
    fn run_with(&self, next: &dyn ExternalPass) -> Option<Box<dyn ExternalPass>> {
        let next = next.as_any()?.downcast_ref::<WidgetPass>()?;
        let (a, b) = (self.members.first()?, next.members.first()?);
        let caps = gfx::translate::Capabilities::from_features(wgpu::Features::empty());
        let key = |draw: &d3d::ResolvedDraw, n: usize| gfx::translate::SamplerKey::from_stage(&draw.state.stages[n], caps);
        if self.sampled != next.sampled
            || a.draw.textures != b.draw.textures
            || a.overlay.size != b.overlay.size
            || (0..4).any(|n| self.sampled[n] && key(&a.draw, n) != key(&b.draw, n))
        {
            return None;
        }
        let mut members = self.members.clone();
        members.extend(next.members.iter().cloned());
        Some(Box::new(WidgetPass { members, sampled: self.sampled }))
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }

    /// The whole guest draw: a run of several has none, and its packets
    /// are the fallback.
    fn resolved_fallback(&self) -> Option<Arc<d3d::ResolvedDraw>> {
        match self.members.as_slice() {
            [one] => Some(Arc::clone(&one.draw)),
            _ => None,
        }
    }

    fn resolved_packets(&self) -> Vec<Arc<d3d::ResolvedDraw>> {
        self.members.iter().map(|m| Arc::clone(&m.draw)).collect()
    }

    fn diagnostic_draw(&self) -> Option<Arc<d3d::ResolvedDraw>> {
        self.members.first().map(|m| Arc::clone(&m.draw))
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

/// Why a font did not give a glyph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlyphMissing {
    /// No such font: a line the table lacks, or a file not read.
    Font,
    /// The glyph's record is out of range or its pixel stream is malformed.
    Decode,
    /// Its page has no room for it.
    Room,
}

/// A glyph a list's text run draws that its font did not give.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MissingGlyph {
    /// The engine font, as the run's material names it.
    pub font: u16,
    /// The font's glyph record.
    pub index: u32,
    pub why: GlyphMissing,
}

/// The largest scale the glyph pages rasterise at: past it a page's side
/// stops holding a screen's glyphs, and the remainder over the cap
/// magnifies a glyph rasterised at the cap (a fraction over 1 at 8K).
pub const MAX_ATLAS_SCALE: u32 = 8;

/// The glyph pages' rasterisation scale and page side for a window
/// `output`: the guest canvas's uniform fit scale, rounded up to whole
/// texels so a glyph edge lands on whole output texels, capped at
/// [`MAX_ATLAS_SCALE`] (1 with no window worth fitting). The page side
/// doubles past scale 1, so a page still holds a screen's glyphs; the
/// pages' RGBA texels are 16 MB at most per font.
pub fn atlas_for(output: (u32, u32)) -> (u32, u32) {
    let scale = if output.0 == 0 || output.1 == 0 {
        1
    } else {
        let uniform = render_bridge::ui_layout::uniform_scale(output);
        (uniform.ceil().max(1.0).min(MAX_ATLAS_SCALE as f32)) as u32
    };
    let side = if scale > 1 { GlyphPages::PAGE * 2 } else { GlyphPages::PAGE };
    (scale, side)
}

/// The glyph pages' rasterisation scale and page side as the last
/// `place_menu` left them (0, 0 before the first one): `render.stats`'s
/// `ui` (docs/runtime/agent-api.md).
static ATLAS_SCALE: AtomicU32 = AtomicU32::new(0);
static ATLAS_SIDE: AtomicU32 = AtomicU32::new(0);

/// The glyph pages' rasterisation scale and page side now.
pub fn atlas_state() -> (u32, u32) {
    (ATLAS_SCALE.load(Ordering::Relaxed), ATLAS_SIDE.load(Ordering::Relaxed))
}

/// Puts every glyph of `overlay`'s text runs into their pages at
/// `atlas_scale` times its size: for each engine font (the run's
/// material's) and glyph index not in its page, the glyph decoded from
/// `fonts` once, its ARGB4444 alpha times 17 resampled to the page's
/// density ([`font::coverage::upsample`], the display side of the glyph
/// filter), white with that coverage. A glyph already in a page is not
/// decoded again, so a list filled again costs nothing, and `Ok(())` says
/// every glyph of the list is in the pages: the call is a list's preflight
/// too. A glyph missing at plan time makes the plan drop its run
/// (`refused_glyph`). `Err` is the first glyph not put in. A page keeps
/// coverage alone: a glyph with colours of its own (a medal, a button
/// icon) draws in its run's colour.
pub fn fill_glyphs(
    pages: &mut GlyphPages,
    overlay: &Overlay,
    fonts: &Fonts,
    atlas_scale: u32,
) -> Result<(), MissingGlyph> {
    // What is not in a page yet, by engine font, in the order first drawn.
    let mut wanted: BTreeMap<u16, BTreeSet<u32>> = BTreeMap::new();
    for item in &overlay.items {
        if item.kind != OverlayKind::TextRun {
            continue;
        }
        let OverlayTexture::Glyphs { font } = item.material.texture else { continue };
        for g in overlay.glyphs_of(item) {
            if !pages.has(font, g.index) {
                wanted.entry(font).or_default().insert(g.index);
            }
        }
    }
    for (font, indices) in wanted {
        let parsed = i16::try_from(font).ok().and_then(|i| fonts.font(i));
        for index in indices {
            let missing = |why| MissingGlyph { font, index, why };
            let Some(f) = parsed.as_ref() else {
                return Err(missing(GlyphMissing::Font));
            };
            let Ok(glyph) = f.glyph_by_index(index) else {
                return Err(missing(GlyphMissing::Decode));
            };
            let alpha: Vec<u8> = glyph.pixels.iter().map(|&px| ((px >> 12) * 17) as u8).collect();
            let coverage = font::coverage::upsample(&alpha, glyph.record.width, glyph.record.height, atlas_scale);
            let (width, height) = (
                glyph.record.width.saturating_mul(atlas_scale as u16),
                glyph.record.height.saturating_mul(atlas_scale as u16),
            );
            if !pages.insert(font, index, width, height, &coverage) {
                return Err(missing(GlyphMissing::Room));
            }
        }
    }
    Ok(())
}

/// The disc's fonts once read ([`fonts`]).
static FONTS: Mutex<Option<Arc<Fonts>>> = Mutex::new(None);

/// The disc's fonts as the game serves them: `font_table.txt` and the
/// font files it names, by engine font index (a line of the table), read
/// through the VFS with the game's font rewrites. Read once, the first
/// ask (the HUDs' load thread asks first), and shared by every list drawn
/// from the overlay. `None` while they cannot be read: no VFS mounted or
/// no table on the disc, asked for again next time.
pub fn fonts() -> Option<Arc<Fonts>> {
    let mut cached = FONTS.lock().unwrap_or_else(|e| e.into_inner());
    if cached.is_none() {
        let read = read_fonts();
        if read.table.is_empty() {
            return None;
        }
        *cached = Some(Arc::new(read));
    }
    cached.clone()
}

/// The font table and every font it names, as the guest reads them.
fn read_fonts() -> Fonts {
    let dir = format!("{}\\maps\\fonts", vfs::CDROM_DEVICE);
    let Some(table) = crate::native_map::guest_file(&format!("{dir}\\font_table.txt")) else {
        return Fonts::served(b"", []);
    };
    // The table names some fonts more than once: each file is read once.
    let mut names = Fonts::served(&table, []).table;
    names.sort();
    names.dedup();
    let files = names
        .into_iter()
        .filter_map(|name| {
            crate::native_map::guest_file(&format!("{dir}\\{name}")).map(|bytes| (name, bytes))
        })
        .collect::<Vec<_>>();
    Fonts::served(&table, files)
}

/// Places a pass drawing `overlay` from `assets` at this point in the
/// frame: whether the device took it.
pub fn place<B: d3d::Backend + Send + 'static>(
    device: &d3d::abi::SharedDevice<B>,
    mem: &mut dyn GuestMemory,
    overlay: Arc<Overlay>,
    assets: Arc<Mutex<OverlayAssets>>,
) -> bool {
    device.lock().ok().is_some_and(|mut d| {
        // The run draws with the layout rule the placed draw sits in: the
        // same fit the guest's own draws carry at this point in the frame.
        let fit = d.ui_fit().map(|f| f.transform);
        let pass = OverlayPass { overlay, assets, fit };
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
    // The pages hold the glyphs at the display's density, so text draws
    // near its texels 1:1. A window of another size rasterises them again;
    // the residency drops the old pages with the assets' generation.
    let (scale, side) = atlas_for(crate::render_aspect::layout_output());
    let mut a = assets.lock().unwrap_or_else(|e| e.into_inner());
    if a.glyphs.side() != side || ATLAS_SCALE.load(Ordering::Relaxed) != scale {
        a.glyphs = GlyphPages::new(side);
        a.generation += 1;
    }
    ATLAS_SCALE.store(scale, Ordering::Relaxed);
    ATLAS_SIDE.store(side, Ordering::Relaxed);
    // A glyph the fonts do not give leaves its run out of the plan
    // (`refused_glyph`); the rest of the list still draws.
    if let Some(fonts) = fonts() {
        let _ = fill_glyphs(&mut a.glyphs, part, &fonts, scale);
    }
    drop(a);
    place(device, mem, Arc::new(part.clone()), assets)
}

/// The renderer's overlay programs and resident textures, on the device
/// last drawn with.
static RENDERER: LazyLock<Mutex<Option<(wgpu::Device, OverlayGpu, OverlayResidency)>>> =
    LazyLock::new(|| Mutex::new(None));

/// Places a run's positions and clips in the placed draw's UI fit
/// ([`render_bridge::ui_layout`]): what the pass draws when the guest's
/// own draws at this point in the frame are fitted the same way.
fn fit_overlay(overlay: &mut Overlay, fit: Option<[f32; 4]>) {
    let Some(fit) = fit else { return };
    for v in &mut overlay.vertices {
        v.position = render_bridge::ui_layout::map_point(fit, v.position);
    }
    // A text run's glyph quads carry their own rectangles.
    for g in &mut overlay.glyphs {
        let [x0, y0] = render_bridge::ui_layout::map_point(fit, [g.rect[0], g.rect[1]]);
        let [x1, y1] = render_bridge::ui_layout::map_point(fit, [g.rect[2], g.rect[3]]);
        g.rect = [x0, y0, x1, y1];
    }
    for item in &mut overlay.items {
        let c = &mut item.clip;
        let mapped = render_bridge::ui_layout::map_rect(fit, (c.x, c.y, c.width, c.height));
        *c = ClipRect { x: mapped.0, y: mapped.1, width: mapped.2, height: mapped.3 };
    }
}

/// An overlay's items, drawn where the pass was placed in the frame, with
/// the placed draw's UI fit ([`render_bridge::ui_layout`]): the same
/// placement the guest's own draws carry at this point in the frame.
struct OverlayPass {
    overlay: Arc<Overlay>,
    assets: Arc<Mutex<OverlayAssets>>,
    fit: Option<[f32; 4]>,
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
        let mut overlay = (*self.overlay).clone();
        fit_overlay(&mut overlay, self.fit);
        let size = t.color.texture().size();
        let plan = OverlayPlan::new(&overlay, [size.width, size.height], &assets.glyphs);
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
        let screen = Source::Screen;
        assert_eq!(screen.texture(&other, Some(0)), other[0]);
        assert_eq!(Source::Screen.texture(&[None; 4], None), Some(OverlayTexture::None));
        assert_eq!(screen.shader(), (0, [0.0; 4]));
        assert_eq!(Source::Tag(&at).shader(), (0xe000_0001, [0.5; 4]));
    }

    /// A widget draw is kept in a two-copy scene as its one packet: the
    /// pass records again on the replay's attachments with the textures
    /// captured beside that packet.
    #[test]
    fn a_widget_pass_replays_from_its_packet() {
        let draw = Arc::new(crate::native_hud_glue::tests::ui_draw());
        let pass = WidgetPass {
            members: vec![Member {
                draw: Arc::clone(&draw),
                overlay: Arc::new(Overlay::new([draw.target.width, draw.target.height])),
            }],
            sampled: [true, false, false, false],
        };
        assert!(pass.replays_scene(), "the sink's retain_admission requires it");
        let packet = pass.resolved_fallback().expect("the packet the sink compares and replays");
        assert!(Arc::ptr_eq(&packet, &draw));
        let packets = pass.resolved_packets();
        assert_eq!(packets.len(), 1);
        assert!(Arc::ptr_eq(&packets[0], &draw));
        assert!(pass.colour_only());
        assert!(!pass.writes().depth_stencil);
    }

    fn quad_pass(draw: &Arc<d3d::ResolvedDraw>) -> WidgetPass {
        WidgetPass {
            members: vec![Member {
                draw: Arc::clone(draw),
                overlay: Arc::new(Overlay::new([draw.target.width, draw.target.height])),
            }],
            sampled: [true, false, false, false],
        }
    }

    /// The kept scene joins adjacent quads of one state: 500 are one pass
    /// holding every packet in order, so one render pass records them.
    #[test]
    fn five_hundred_adjacent_quads_are_one_run() {
        let base = crate::native_hud_glue::tests::ui_draw();
        let draws: Vec<_> = (0..500)
            .map(|n| {
                let mut d = base.clone();
                d.vertex_count = base.vertex_count + n;
                Arc::new(d)
            })
            .collect();
        let mut run: Box<dyn ExternalPass> = Box::new(quad_pass(&draws[0]));
        for draw in &draws[1..] {
            run = run.run_with(&quad_pass(draw)).expect("same state joins");
        }
        let packets = run.resolved_packets();
        assert_eq!(packets.len(), 500);
        assert!(packets.iter().zip(&draws).all(|(p, d)| Arc::ptr_eq(p, d)), "draw order is kept");
        assert!(run.resolved_fallback().is_none(), "a run has no single fallback");
        assert!(run.replays_scene());
    }

    /// A texture change splits the run where it must, and the two runs
    /// keep their order.
    #[test]
    fn a_texture_change_splits_the_run() {
        let base = crate::native_hud_glue::tests::ui_draw();
        let with = |header: u32| {
            let mut d = base.clone();
            d.textures[0] = Some(header);
            Arc::new(d)
        };
        let (a, b, c) = (with(0x6000), with(0x6000), with(0x7000));
        let run = quad_pass(&a).run_with(&quad_pass(&b)).expect("same texture joins");
        assert!(run.run_with(&quad_pass(&c)).is_none(), "another texture is another pass");
        let after = quad_pass(&c).run_with(&quad_pass(&c)).expect("the second run joins");
        assert!(Arc::ptr_eq(&run.resolved_packets()[0], &a));
        assert!(Arc::ptr_eq(&after.resolved_packets()[1], &c));
    }

    #[test]
    fn widget_fans_use_the_sampled_stage_even_without_a_tag() {
        let widget = Source::Screen;
        let stages = [Some(OverlayTexture::Bound { header: 0x10 }), None,
            Some(OverlayTexture::Bound { header: 0x30 }), None];
        assert_eq!(widget.texture(&stages, Some(2)), stages[2]);
        assert_eq!(widget.texture(&stages, Some(1)), None);
        assert_eq!(widget.texture(&stages, None), Some(OverlayTexture::None));
    }

    /// An invented font: a blank record every unit maps to, with a glyph
    /// of its own for each `(unit, width, height, alpha over 15)`, in the
    /// font's ascent of 12.
    fn invented_font(glyphs: &[(u16, u16, u16, u16)]) -> Vec<u8> {
        let mut base = vec![0; font::RECORDS_OFFSET + font::RECORD_LEN];
        let mut put = |at: usize, bytes: &[u8]| base[at..at + bytes.len()].copy_from_slice(bytes);
        put(font::HEADER_OFFSET, &font::MAGIC.to_le_bytes());
        put(font::HEADER_OFFSET + 4, &12u16.to_le_bytes());
        put(font::HEADER_OFFSET + 0xc, &1u32.to_le_bytes());
        let added = glyphs
            .iter()
            .map(|&(unit, width, height, alpha)| font::patch::Replacement {
                unit,
                width,
                height,
                advance: width as i16 + 1,
                x_offset: 0,
                y_offset: None,
                pixels: vec![(alpha << 12) | 0x0fff; usize::from(width) * usize::from(height)],
            })
            .collect::<Vec<_>>();
        font::patch::apply(&base, &added).unwrap()
    }

    /// A text run of `text` in engine font `font`, laid out as the walk
    /// lays one out ([`Fonts::layout`]).
    fn run(overlay: &mut Overlay, fonts: &Fonts, font: u16, text: &str) {
        let (laid, _) = fonts.layout(font as i16, text).unwrap();
        let glyphs = laid
            .glyphs
            .iter()
            .filter(|g| g.width != 0 && g.height != 0)
            .map(|g| frame_scene::OverlayGlyph {
                index: g.index,
                rect: [
                    g.x as f32,
                    g.y as f32,
                    (g.x + i64::from(g.width)) as f32,
                    (g.y + i64::from(g.height)) as f32,
                ],
            })
            .collect::<Vec<_>>();
        let all = frame_scene::ClipRect::full([640, 480]);
        overlay.push_text_run(frame_scene::OverlayMaterial::text(font), all, [1.0; 4], &glyphs).unwrap();
    }

    /// The filler decodes each glyph of a list's text runs once, from the
    /// run's font, and the filled list's runs draw: the plan, which
    /// refuses a run over an empty atlas, refuses none of them.
    #[test]
    fn the_filler_decodes_each_glyph_once_and_the_runs_draw() {
        let (a, one, b) = (u16::from(b'A'), u16::from(b'1'), u16::from(b'B'));
        let fonts = Fonts::served(
            b"one\r\ntwo",
            [
                ("one".to_owned(), invented_font(&[(a, 2, 2, 15), (one, 3, 1, 8)])),
                ("two".to_owned(), invented_font(&[(b, 1, 1, 4)])),
            ],
        );
        let mut overlay = Overlay::new([640, 480]);
        run(&mut overlay, &fonts, 0, "A1A");
        run(&mut overlay, &fonts, 1, "BB");
        let index = |font: i16, unit: u16| fonts.font(font).unwrap().glyph_index(unit);
        let refused = OverlayPlan::new(&overlay, [640, 480], &GlyphPages::default());
        assert_eq!((refused.batches.len(), refused.stats.refused_glyph), (0, 2), "over an empty atlas");
        let mut pages = GlyphPages::new(16);
        fill_glyphs(&mut pages, &overlay, &fonts, 1).unwrap();
        assert!(pages.has(0, index(0, a)) && pages.has(0, index(0, one)));
        assert!(pages.has(1, index(1, b)));
        assert_eq!(pages.fonts().collect::<Vec<_>>(), [0, 1]);
        let (side, rgba, version) = pages.page(0).unwrap();
        assert_eq!((side, version), (16, 2), "three glyphs of two");
        let texel = |x: usize, y: usize| rgba[(y * 16 + x) * 4..(y * 16 + x) * 4 + 4].to_vec();
        assert_eq!(texel(1, 1), [255, 255, 255, 255], "A, white with its alpha");
        assert_eq!(texel(5, 1), [255, 255, 255, 136], "1 beside it: alpha 8 of 15");
        assert_eq!(texel(4, 1), [0, 0, 0, 0], "the texel between them");
        assert_eq!(pages.page(1).unwrap().2, 1, "B twice is decoded once");
        fill_glyphs(&mut pages, &overlay, &fonts, 1).unwrap();
        assert_eq!((pages.page(0).unwrap().2, pages.page(1).unwrap().2), (2, 1), "filled again: nothing added");
        let plan = OverlayPlan::new(&overlay, [640, 480], &pages);
        assert_eq!((plan.batches.len(), plan.stats.refused_glyph, plan.stats.glyphs), (2, 0, 5));
        assert_eq!(plan.vertices[1].uv, [3.0 / 16.0, 1.0 / 16.0], "A's top right");
        assert_eq!(plan.vertices[6].uv, [8.0 / 16.0, 2.0 / 16.0], "1's bottom right");
    }

    /// Filled at an atlas scale, a glyph lands resampled to that many
    /// times its size: the display's density (`hud_overlay::atlas_for`),
    /// so the run's UVs still name the whole glyph.
    #[test]
    fn the_filler_resamples_glyphs_to_the_atlas_scale() {
        let a = u16::from(b'A');
        let fonts = Fonts::served(b"one", [("one".to_owned(), invented_font(&[(a, 2, 2, 15)]))]);
        let mut overlay = Overlay::new([640, 480]);
        run(&mut overlay, &fonts, 0, "A");
        let mut pages = GlyphPages::new(32);
        fill_glyphs(&mut pages, &overlay, &fonts, 2).unwrap();
        let (side, rgba, _) = pages.page(0).unwrap();
        assert_eq!(side, 32);
        let texel = |x: usize, y: usize| rgba[(y * 32 + x) * 4..(y * 32 + x) * 4 + 4].to_vec();
        // A solid 2x2 glyph resampled to 4x4: solid through, one cleared
        // texel of space around it.
        for (x, y) in [(1, 1), (4, 1), (1, 4), (4, 4), (2, 2), (3, 3)] {
            assert_eq!(texel(x, y), [255, 255, 255, 255], "solid at {x},{y}");
        }
        assert_eq!(texel(0, 0), [0, 0, 0, 0], "the space around it");
        let plan = OverlayPlan::new(&overlay, [640, 480], &pages);
        assert_eq!((plan.stats.refused_glyph, plan.stats.glyphs), (0, 1));
        // The glyph's UV rectangle still spans the glyph in the page.
        assert_eq!(plan.vertices[1].uv, [5.0 / 32.0, 1.0 / 32.0], "A's top right at scale 2");
    }

    /// The pages' scale and side for a window: the guest canvas's uniform
    /// fit rounded up to whole texels, capped, and the side that keeps a
    /// screen's glyphs in a page.
    #[test]
    fn the_atlas_scales_with_the_window_and_caps() {
        assert_eq!(atlas_for((0, 0)), (1, GlyphPages::PAGE));
        assert_eq!(atlas_for((640, 480)), (1, GlyphPages::PAGE));
        assert_eq!(atlas_for((960, 720)), (2, GlyphPages::PAGE * 2));
        assert_eq!(atlas_for((1280, 720)), (2, GlyphPages::PAGE * 2));
        assert_eq!(atlas_for((1920, 1080)), (3, GlyphPages::PAGE * 2));
        assert_eq!(atlas_for((5120, 2880)), (6, GlyphPages::PAGE * 2));
        assert_eq!(atlas_for((7680, 4320)), (MAX_ATLAS_SCALE, GlyphPages::PAGE * 2));
    }

    /// A run drawn at the placed draw's UI fit: the guest canvas's 16:9
    /// fit maps its corners onto the centred safe area, and its clip with
    /// them; without a fit the run draws as the guest drew it.
    #[test]
    fn a_run_places_itself_in_the_placed_draws_fit() {
        let fit = [0.75, 1.0, 80.0, 0.0];
        let all = ClipRect::full([640, 480]);
        let mut overlay = Overlay::new([640, 480]);
        overlay
            .push_fill(all, [0.0, 240.0, 640.0, 480.0], [1.0; 4], OverlayBlend::Off)
            .unwrap();
        fit_overlay(&mut overlay, Some(fit));
        let xs: Vec<f32> = overlay.vertices.iter().map(|v| v.position[0]).collect();
        assert_eq!(xs, [80.0, 560.0, 560.0, 80.0], "the safe area's x, in guest window units");
        let ys: Vec<f32> = overlay.vertices.iter().map(|v| v.position[1]).collect();
        assert_eq!(ys, [240.0, 240.0, 480.0, 480.0]);
        let clip = &overlay.items[0].clip;
        // A clip that is the whole canvas stays the whole display, so a
        // run clipped to the canvas spans a wide window
        // (`render_bridge::ui_layout::map_rect`).
        assert_eq!((clip.x, clip.y, clip.width, clip.height), (0, 0, 640, 480));
        // The plan then stretches the fitted units over the window: the
        // fill's x spans 1440 of 1920, centred.
        let plan = OverlayPlan::new(&overlay, [1920, 1080], &NoGlyphs);
        let left = plan.vertices[0].position[0];
        let right = plan.vertices[1].position[0];
        let to_display = |clip_x: f32| (clip_x + 1.0) * 0.5 * 1920.0;
        assert!((to_display(left) - 240.0).abs() < 1e-2);
        assert!((to_display(right) - 1680.0).abs() < 1e-2);
        // A text run's glyph rectangles move with it.
        let fonts = Fonts::served(b"one", [("one".to_owned(), invented_font(&[(u16::from(b'A'), 2, 2, 15)]))]);
        let mut text = Overlay::new([640, 480]);
        run(&mut text, &fonts, 0, "A");
        let before = text.glyphs[0].rect;
        fit_overlay(&mut text, Some(fit));
        let after = text.glyphs[0].rect;
        assert_eq!((after[0], after[2]), (before[0] * 0.75 + 80.0, before[2] * 0.75 + 80.0));
        assert_eq!((after[1], after[3]), (before[1], before[3]));
        // No fit: nothing changes.
        let mut plain = Overlay::new([640, 480]);
        plain
            .push_fill(all, [0.0, 240.0, 640.0, 480.0], [1.0; 4], OverlayBlend::Off)
            .unwrap();
        fit_overlay(&mut plain, None);
        assert_eq!(plain.vertices[0].position, [0.0, 240.0]);
    }

    /// A glyph no font gives is named: a record out of range or a file
    /// that is not a font, a line the table lacks, and a page with no
    /// room. A list without a text run fills without the fonts.
    #[test]
    fn a_glyph_no_font_gives_is_named() {
        let fonts = Fonts::served(
            b"one\r\nunread",
            [
                ("one".to_owned(), invented_font(&[(u16::from(b'A'), 6, 1, 15)])),
                ("unread".to_owned(), b"not a font".to_vec()),
            ],
        );
        let list = |font: u16, index: u32| {
            let glyphs = [frame_scene::OverlayGlyph { index, rect: [0.0, 0.0, 6.0, 1.0] }];
            let mut overlay = Overlay::new([640, 480]);
            let all = frame_scene::ClipRect::full([640, 480]);
            overlay.push_text_run(frame_scene::OverlayMaterial::text(font), all, [1.0; 4], &glyphs).unwrap();
            overlay
        };
        let missing = |font: u16, index: u32, why| fill_glyphs(&mut GlyphPages::default(), &list(font, index), &fonts, 1)
            .unwrap_err() == MissingGlyph { font, index, why };
        assert!(missing(0, 9, GlyphMissing::Decode), "no such record");
        assert!(missing(1, 1, GlyphMissing::Font), "a file that is not a font");
        assert!(missing(3, 1, GlyphMissing::Font), "no such line");
        // Six wide does not fit a four texel page.
        assert_eq!(
            fill_glyphs(&mut GlyphPages::new(4), &list(0, 1), &fonts, 1),
            Err(MissingGlyph { font: 0, index: 1, why: GlyphMissing::Room })
        );
        let mut plain = Overlay::new([640, 480]);
        let all = frame_scene::ClipRect::full([640, 480]);
        plain.push_fill(all, [0.0, 0.0, 4.0, 4.0], [1.0; 4], frame_scene::OverlayBlend::Off).unwrap();
        assert_eq!(fill_glyphs(&mut GlyphPages::default(), &plain, &fonts, 1), Ok(()));
    }
}
