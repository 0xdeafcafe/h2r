//! The overlay's stages: the HUD's and the menus' runs, movies, the
//! present's blit and the frame tail, each placed at its point in the
//! frame (docs/graphics/native-post-hud.md, "The HUD on the overlay") with
//! the UI fit the layout rule sets there ([`set_ui_fit`]).
//!
//! The overlay's glyphs are every list's to share: [`fonts`] is the disc's fonts
//! as the game serves them, [`assets`] the loaded map's overlay assets and
//! [`fill_glyphs`] puts a list's glyphs into their pages, each decoded once;
//! [`place_menu`] fills before it places. [`menu_text`] shapes a menu label from
//! the same fonts.

pub(crate) mod menu_text;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use engine::memory::GuestMemory;
use frame_scene::{ClipRect, Overlay, OverlayKind, OverlayTexture, ViewSlot};
use frame_types::Slot as FrameSlot;
use gfx::external::{ExternalAttachments, UiFit};
use hud_edit::render::{Art, Fonts};
use map::TagId;
use native_render::overlay::{GlyphPages, NoGlyphs, OverlayAssets, OverlayImage, OverlayPlan};
use native_render::program::ViewInputs;
use native_render::ui_gpu::{OverlayGpu, OverlayResidency};
use native_render::{Stage, StageWork};

use crate::native_map::NativeMap;

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

/// Runs `f` on the map read for the overlay's images (the loaded map's):
/// `None` while it loads or with no map loaded.
pub(crate) fn with_art_map<T>(mem: &dyn GuestMemory, f: impl FnOnce(&mut NativeMap) -> T) -> Option<T> {
    let (art, _) = slot(mem)?;
    let art = art?;
    let mut a = art.lock().unwrap_or_else(|e| e.into_inner());
    Some(f(&mut a.map))
}

/// Places `part` (HUD quads whose stages name bitmap images), tagged
/// [`ViewSlot::Hud`], at this
/// point in the frame with the map's assets, its images decoded first:
/// whether it was placed. While the map loads, or when an image does
/// not decode, nothing is placed.
pub(crate) fn place_hud(
    mem: &mut dyn GuestMemory,
    mut part: Overlay,
) -> bool {
    part.set_slot(ViewSlot::Hud);
    let Some((Some(art), assets)) = slot(mem) else { return false };
    {
        let mut a = art.lock().unwrap_or_else(|e| e.into_inner());
        if !decode_images(&mut a, &part, &mut assets.lock().unwrap_or_else(|e| e.into_inner())) {
            return false;
        }
    }
    let _ = render_bridge::scene_step::with_overlay(|o| o.append(&part));
    place(mem, Arc::new(part), assets)
}

/// Places `part` (a string's runs over the glyph pages), tagged `slot`
/// (the HUD's or the UI's), at this point in the frame, every glyph of
/// it in the pages first: whether it was placed. A glyph the fonts do
/// not give places nothing.
pub(crate) fn place_text(
    mem: &mut dyn GuestMemory,
    mut part: Overlay,
    view: ViewSlot,
) -> bool {
    part.set_slot(view);
    let Some((_, assets)) = slot(mem) else { return false };
    let Some(fonts) = fonts() else { return false };
    {
        // The pages at the display's density, as `place_menu` keeps them.
        let (scale, side) = atlas_for(crate::render_aspect::layout_output());
        let mut a = assets.lock().unwrap_or_else(|e| e.into_inner());
        if a.glyphs.side() != side || ATLAS_SCALE.load(Ordering::Relaxed) != scale {
            a.glyphs = GlyphPages::new(side);
            a.generation += 1;
        }
        ATLAS_SCALE.store(scale, Ordering::Relaxed);
        ATLAS_SIDE.store(side, Ordering::Relaxed);
        if fill_glyphs(&mut a.glyphs, &part, &fonts, scale).is_err() {
            return false;
        }
    }
    let _ = render_bridge::scene_step::with_overlay(|o| o.append(&part));
    place(mem, Arc::new(part), assets)
}

/// Decodes the images `part`'s quads name into `assets`, their material's
/// and their shader stages': whether each did.
/// [`decode_images`], naming the image that did not decode.
fn decode_named(a: &mut MenuArt, part: &Overlay, assets: &mut OverlayAssets) -> Result<(), String> {
    let named = part.items.iter().flat_map(|item| {
        std::iter::once(item.material.texture).chain(item.shader_stages.iter().flatten().copied())
    });
    for texture in named {
        let OverlayTexture::Bitmap { tag, image } = texture else { continue };
        if assets.images.contains_key(&(tag, image)) {
            continue;
        }
        let (width, height, rgba) = a
            .art
            .image(&mut a.map, TagId(tag), usize::from(image))
            .map_err(|e| format!("bitmap {tag:#010x} image {image} did not decode: {e:?}"))?;
        assets.images.insert((tag, image), OverlayImage { width, height, rgba: rgba.into() });
    }
    Ok(())
}

fn decode_images(a: &mut MenuArt, part: &Overlay, assets: &mut OverlayAssets) -> bool {
    let named = part.items.iter().flat_map(|item| {
        std::iter::once(item.material.texture).chain(item.shader_stages.iter().flatten().copied())
    });
    for texture in named {
        let OverlayTexture::Bitmap { tag, image } = texture else { continue };
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

/// A coloured glyph's page texels: each texel of the `wide` page copy
/// takes the colour of the ARGB4444 pixel it lies in (`pixels`, `width`
/// to a row, `scale` texels to a pixel, each nibble times 17) and its
/// own byte of `coverage`.
fn coloured(pixels: &[u16], width: u16, coverage: &[u8], wide: u16, scale: u32) -> Vec<u8> {
    let (width, wide, scale) = (usize::from(width), usize::from(wide), scale.max(1) as usize);
    let mut rgba = Vec::with_capacity(coverage.len() * 4);
    for (at, &alpha) in coverage.iter().enumerate() {
        let (row, col) = (at / wide.max(1) / scale, at % wide.max(1) / scale);
        let px = pixels.get(row * width + col).copied().unwrap_or(0xfff);
        let channel = |shift: u16| ((px >> shift & 0xf) * 17) as u8;
        rgba.extend([channel(8), channel(4), channel(0), alpha]);
    }
    rgba
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
            // The decoder's colour is white until a glyph sets its own.
            let placed = if glyph.pixels.iter().all(|&px| px & 0xfff == 0xfff) {
                pages.insert(font, index, width, height, &coverage)
            } else {
                pages.insert_rgba(font, index, width, height, &coloured(&glyph.pixels, glyph.record.width, &coverage, width, atlas_scale))
            };
            if !placed {
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

/// The UI fit the layout rule sets for the draws at this point in the
/// frame (`render_bridge::ui_layout`); `None` draws unfitted.
static UI_FIT: Mutex<Option<UiFit>> = Mutex::new(None);

/// Sets the UI fit the overlay stages placed from here on draw with.
pub fn set_ui_fit(fit: Option<UiFit>) {
    *UI_FIT.lock().unwrap_or_else(|e| e.into_inner()) = fit;
}

/// The UI fit set now ([`set_ui_fit`]).
pub fn ui_fit() -> Option<UiFit> {
    *UI_FIT.lock().unwrap_or_else(|e| e.into_inner())
}

/// Places a stage drawing `overlay` from `assets` at this point in the
/// frame, in the UI fit set here: whether it was placed.
pub fn place(mem: &mut dyn GuestMemory, overlay: Arc<Overlay>, assets: Arc<Mutex<OverlayAssets>>) -> bool {
    let fit = ui_fit().map(|f| f.transform);
    let objects = model_view_objects(&*mem, &overlay);
    let pass = OverlayPass { overlay, assets: Some(assets), movie: None, objects, fit, tail: false };
    crate::native_targets_glue::push(&*mem, FrameSlot::Overlay, pass);
    true
}

/// The objects of `overlay`'s model views, planned here, at the hook
/// (`native_objects_glue::model_view`); `None` for a list with no model
/// view.
fn model_view_objects(mem: &dyn GuestMemory, overlay: &Overlay) -> Option<crate::native_objects_glue::model_view::Planned> {
    if overlay.model_views.is_empty() {
        return None;
    }
    let handles: Vec<u32> = overlay.model_views.iter().flat_map(|v| v.objects.iter().copied()).collect();
    Some(crate::native_objects_glue::model_view::plan(mem, &handles))
}

/// A movie's frame (`render_bridge::native_movie`): its overlay placed at
/// this point in the frame in the UI fit set here, `movie` uploaded as
/// the stage records. Whether it was placed.
pub fn place_movie(mem: &mut dyn GuestMemory, overlay: Overlay, movie: native_render::overlay::MovieImage) -> bool {
    let fit = ui_fit().map(|f| f.transform);
    let pass = OverlayPass { overlay: Arc::new(overlay), assets: None, movie: Some(movie), objects: None, fit, tail: false };
    crate::native_targets_glue::push(&*mem, FrameSlot::Overlay, pass);
    true
}

/// The present's back-buffer blit (`render_bridge::render_frame::blit`):
/// its picture uploaded and drawn over the bound target, unfitted (the
/// guest writes the back buffer's own pixels).
pub fn place_blit(mem: &mut dyn GuestMemory, blit: &render_bridge::render_frame::blit::Blit) -> Result<(), &'static str> {
    use render_bridge::render_frame::blit;
    let picture = native_render::overlay::MovieImage {
        movie: blit::PICTURE,
        frame: blit.frame(),
        width: blit.size[0],
        height: blit.size[1],
        bgra: Arc::from(blit.bgra.as_slice()),
    };
    let pass = OverlayPass { overlay: Arc::new(blit::overlay(blit)), assets: None, movie: Some(picture), objects: None, fit: None, tail: false };
    crate::native_targets_glue::push(&*mem, FrameSlot::Outside, pass);
    Ok(())
}

/// The frame tail's freeze frame (`render_bridge::frame_tail`): its hold
/// and composite placed at this point in the frame over the bound target,
/// unfitted (the guest's quads are the window's, in the target's pixels).
/// Whether it was placed.
pub fn place_tail(mem: &mut dyn GuestMemory, overlay: Overlay) -> bool {
    let pass = OverlayPass { overlay: Arc::new(overlay), assets: None, movie: None, objects: None, fit: None, tail: true };
    crate::native_targets_glue::push(&*mem, FrameSlot::Outside, pass);
    true
}

/// A run of menu widgets (`render_bridge::native_hud::ui_overlay`), drawn
/// from the overlay at this point in the frame with the map's assets:
/// whether it was placed. The bitmap images its quads name are decoded
/// first, from the map read for the menus; while that loads, or when an
/// image does not decode, the run is not placed and the game draws it.
pub fn place_menu(
    mem: &mut dyn GuestMemory,
    part: &Arc<Overlay>,
) -> bool {
    let Some((art, assets)) = slot(mem) else { unimplemented!("menus: pass: no overlay assets for this map") };
    let bitmaps = part.items.iter().any(|i| matches!(i.material.texture, OverlayTexture::Bitmap { .. }));
    if bitmaps {
        let Some(art) = art else { unimplemented!("menus: pass: no map art to decode bitmaps from") };
        let mut a = art.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(why) = decode_named(&mut a, part, &mut assets.lock().unwrap_or_else(|e| e.into_inner())) {
            unimplemented!("menus: pass: {why}");
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
    place(mem, Arc::clone(part), assets)
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
    /// The map's images and glyph pages; `None` for a list that names
    /// neither (a movie's frame).
    assets: Option<Arc<Mutex<OverlayAssets>>>,
    /// The movie frame the list's movie item shows, uploaded first.
    movie: Option<native_render::overlay::MovieImage>,
    /// The model views' objects, planned at the hook; `None` for a list
    /// with no model view, which draws with no object stage.
    objects: Option<crate::native_objects_glue::model_view::Planned>,
    fit: Option<[f32; 4]>,
    /// The frame tail's: its refusals are counted as the tail's.
    tail: bool,
}

impl Stage for OverlayPass {
    fn name(&self) -> &'static str {
        if self.tail { "frame tail overlay" } else { "overlay" }
    }

    fn record(&mut self, t: &ExternalAttachments, _: &ViewInputs) -> StageWork {
        let mut r = RENDERER.lock().unwrap_or_else(|e| e.into_inner());
        if r.as_ref().is_none_or(|(d, _, _)| *d != t.device) {
            *r = Some((t.device.clone(), OverlayGpu::new(&t.device, &t.queue), OverlayResidency::default()));
        }
        let Some((_, gpu, residency)) = r.as_mut() else { unreachable!() };
        if let Some(movie) = &self.movie {
            // A frame that does not upload leaves its item refused
            // (`refused_movie`).
            residency.sync_movie(&t.device, &t.queue, movie);
        }
        // A hold copies into a texture of the target's size and format,
        // made the first time; what it holds stays for later frames.
        let target = t.color.texture();
        for item in &self.overlay.items {
            if let Some(hold) = self.overlay.hold_of(item) {
                residency.hold(&t.device, hold, target.size(), target.format());
            }
        }
        // Only a fitted pass moves its items: an unfitted one plans the
        // shared items as they are, with no copy a frame.
        let fitted = self.fit.map(|fit| {
            let mut overlay = (*self.overlay).clone();
            fit_overlay(&mut overlay, Some(fit));
            overlay
        });
        let overlay: &Overlay = fitted.as_ref().unwrap_or(&*self.overlay);
        let size = target.size();
        let plan = match &self.assets {
            Some(assets) => {
                let assets = assets.lock().unwrap_or_else(|e| e.into_inner());
                residency.sync(&t.device, &t.queue, &assets);
                OverlayPlan::new(overlay, [size.width, size.height], &assets.glyphs)
            }
            None => OverlayPlan::new(overlay, [size.width, size.height], &NoGlyphs),
        };
        let prepared = match &self.objects {
            Some(planned) => {
                let mut objects = crate::native_objects_glue::model_view::Objects { planned, t };
                gpu.prepare_with_objects(&t.device, &t.color, &plan, residency, &mut objects)
            }
            None => gpu.prepare(&t.device, &t.color, &plan, residency),
        };
        let (encoded, stats) = prepared.unwrap_or_else(|why| unimplemented!("overlay: {why}"));
        native_render::ui_gpu::count_overlay(&stats);
        if self.tail && stats.refused_hold != 0 {
            render_bridge::frame_tail::refused("hold");
        }
        if self.tail && stats.refused_texture + stats.refused_material != 0 {
            render_bridge::frame_tail::refused("held");
        }
        StageWork::Encoded(encoded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frame_scene::OverlayBlend;
    #[test]
    fn a_coloured_glyphs_texels_take_their_pixels_colour_and_their_own_coverage() {
        // A green pixel beside a red one, two texels to a pixel.
        let coverage = [255, 200, 100, 0, 255, 200, 100, 0];
        let rgba = coloured(&[0xf0f0, 0x7f00], 2, &coverage, 4, 2);
        assert_eq!(rgba.len(), 32);
        assert_eq!(rgba[..8], [0, 255, 0, 255, 0, 255, 0, 200]);
        assert_eq!(rgba[8..16], [255, 0, 0, 100, 255, 0, 0, 0]);
        assert_eq!(rgba[16..24], rgba[..8], "the second row of the same pixels");
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
