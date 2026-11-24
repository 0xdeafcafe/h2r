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
//! [`place_menu`] fills before it places.
use std::collections::{BTreeMap, BTreeSet, HashMap};
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

/// Puts every glyph `overlay`'s text runs draw into `pages`: for each
/// engine font (the run's material's) and glyph index not in its page,
/// the glyph decoded from `fonts` once, white with its ARGB4444 alpha
/// times 17. A glyph already in a page is not decoded again, so a list
/// filled again costs nothing, and `Ok(())` says every glyph of the list
/// is in the pages: the call is a list's preflight too. A glyph missing
/// at plan time makes the plan drop its run (`refused_glyph`). `Err` is
/// the first glyph not put in. A page keeps coverage alone: a glyph with
/// colours of its own (a medal, a button icon) draws in its run's colour.
pub fn fill_glyphs(pages: &mut GlyphPages, overlay: &Overlay, fonts: &Fonts) -> Result<(), MissingGlyph> {
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
            if !pages.insert(font, index, glyph.record.width, glyph.record.height, &alpha) {
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
    // A glyph the fonts do not give leaves its run out of the plan
    // (`refused_glyph`); the rest of the list still draws.
    if let Some(fonts) = fonts() {
        let _ = fill_glyphs(&mut assets.lock().unwrap_or_else(|e| e.into_inner()).glyphs, part, &fonts);
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
        fill_glyphs(&mut pages, &overlay, &fonts).unwrap();
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
        fill_glyphs(&mut pages, &overlay, &fonts).unwrap();
        assert_eq!((pages.page(0).unwrap().2, pages.page(1).unwrap().2), (2, 1), "filled again: nothing added");
        let plan = OverlayPlan::new(&overlay, [640, 480], &pages);
        assert_eq!((plan.batches.len(), plan.stats.refused_glyph, plan.stats.glyphs), (2, 0, 5));
        assert_eq!(plan.vertices[1].uv, [3.0 / 16.0, 1.0 / 16.0], "A's top right");
        assert_eq!(plan.vertices[6].uv, [8.0 / 16.0, 2.0 / 16.0], "1's bottom right");
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
        let missing = |font: u16, index: u32, why| fill_glyphs(&mut GlyphPages::default(), &list(font, index), &fonts)
            .unwrap_err() == MissingGlyph { font, index, why };
        assert!(missing(0, 9, GlyphMissing::Decode), "no such record");
        assert!(missing(1, 1, GlyphMissing::Font), "a file that is not a font");
        assert!(missing(3, 1, GlyphMissing::Font), "no such line");
        // Six wide does not fit a four texel page.
        assert_eq!(
            fill_glyphs(&mut GlyphPages::new(4), &list(0, 1), &fonts),
            Err(MissingGlyph { font: 0, index: 1, why: GlyphMissing::Room })
        );
        let mut plain = Overlay::new([640, 480]);
        let all = frame_scene::ClipRect::full([640, 480]);
        plain.push_fill(all, [0.0, 0.0, 4.0, 4.0], [1.0; 4], frame_scene::OverlayBlend::Off).unwrap();
        assert_eq!(fill_glyphs(&mut GlyphPages::default(), &plain, &fonts), Ok(()));
    }
}
