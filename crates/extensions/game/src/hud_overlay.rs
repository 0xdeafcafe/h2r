//! The HUD drawn from the overlay stage (docs/graphics/native-post-hud.md,
//! "The HUD on the overlay"; docs/graphics/retire-d3d.md, slice R6).
//!
//! At each HUD draw's first bitmap widget (`render_bridge::native_hud::
//! hud_overlay`) the game thread reads the user's unit and held weapon into
//! a [`HudState`], walks the `nhdt` with the engine's own gate words
//! (`hud_edit::overlay::draw_list`, [`Shading::Engine`]), and, when every
//! shown widget is drawn as the engine draws it, adds the items to the
//! scene's overlay (`FrameScene::overlay`), decodes the bitmap images they
//! name and the glyphs their text runs draw, and places a pass at that
//! point in the frame that draws them with
//! `native_render::ui_gpu::OverlayGpu`. The game's quads for those widgets
//! are then left out. Any other HUD draw is the game's, counted by its
//! first reason (`native_ui_overlay_hud_refused_*`).
//!
//! The overlay's glyphs are every list's to share: [`fonts`] is the
//! disc's fonts as the game serves them, [`assets`] the loaded map's
//! overlay assets, and [`fill_glyphs`] puts a list's glyphs into their
//! pages, each decoded once (a menu's list takes the same three).
//!
//! What keeps a retail HUD on the game's quads today: every bitmap widget
//! is drawn through its tag shader, and no HUD shader translates yet
//! ([`translates`]); and a text widget, which the walk refuses under the
//! engine's shading for its colour and its text (neither read).

pub(crate) mod menu_text;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};

use engine::memory::GuestMemory;
use frame_scene::{Overlay, OverlayKind, OverlayTexture};
use guest_layout::{Unit, Weapon};
use hud_edit::layout::Mode;
use hud_edit::model::{Hud, NHDT, Widget};
use hud_edit::overlay::{self as walk, Inexact, Shading};
use hud_edit::render::{Answers, Art, Fonts};
use hud_edit::state::{self, GateRecord, Grenade, HudState};
use map::TagId;
use native_render::overlay::{GlyphPages, OverlayAssets, OverlayImage, OverlayPlan};
use native_render::ui_gpu::{OverlayGpu, OverlayResidency};
use render_bridge::native_hud::{HudBitmaps, HudGateRecord, HudRefusal};

use crate::native_map::NativeMap;

/// The rectangle the HUD places its widgets in (E `0x0013a690`): four
/// signed words, top, left, bottom, right.
pub const HUD_RECT: u32 = 0x004b_9dd0;
/// The HUD's canvas, and the only frame the overlay draws: the full
/// screen. A split-screen frame is not read (hud_edit's `Frame::for_mode`).
pub const CANVAS: [u32; 2] = [640, 480];

/// Whether the renderer draws HUD shader `shader` (its datum) as the
/// engine does. None does yet: a HUD shader is a template's pixel shader
/// with its parameters and `hud_input1`..`4` as constants, and its
/// translation is a named program per template
/// (docs/graphics/native-post-hud.md, "The HUD on the overlay").
pub fn translates(_shader: u32) -> bool {
    false
}

/// The first reason a walk's widget is not the engine's, as the HUD
/// draw's refusal.
pub fn refusal(why: Inexact) -> HudRefusal {
    match why {
        Inexact::Shader => HudRefusal::Shader,
        Inexact::Text => HudRefusal::Text,
        Inexact::Effects => HudRefusal::Effects,
        Inexact::Anchor => HudRefusal::Anchor,
        Inexact::Unlisted => HudRefusal::Unlisted,
    }
}

/// The engine's gate words and record as the walk's answer to `shown`.
struct EngineGate {
    words: [u16; 4],
    record: Option<GateRecord>,
}

impl EngineGate {
    fn new(at: &HudBitmaps) -> EngineGate {
        let record = at.record.map(|HudGateRecord { at_10, at_06, at_08 }| GateRecord { at_10, at_06, at_08 });
        EngineGate { words: at.words, record }
    }
}

impl Answers for EngineGate {
    fn shown(&self, _hud: usize, w: &Widget) -> Option<bool> {
        Some(state::shown_as_engine(w.state(), self.words, self.record))
    }

    // Nothing else is answered: a text widget's text (a string ID's
    // localized text is not read) and its colour are its shader's, so the
    // walk's engine shading refuses it (`Inexact::Text`). That also keeps
    // a text run out of a claimed draw, which leaves out the game's plain
    // widget quads alone: one the pass drew would be drawn twice.
}

/// What the HUD state is read from: the user's unit and its held weapon.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vitals {
    pub shield: f32,
    pub body: f32,
    /// The selected grenade type, -1 none.
    pub grenade: i8,
    pub grenades: [i8; 2],
    /// The held weapon's first magazine and its battery used.
    pub loaded: Option<i16>,
    pub reserve: Option<i16>,
    pub age: Option<f32>,
}

/// The HUD state at `v` in the full screen: the shield, the body, the
/// selected grenade and the counts, the held weapon's rounds and battery.
/// Not read: the magazine's size (the weapon tag's), the heat and the zoom;
/// they keep [`HudState::default`]'s. The gate is the engine's words, not
/// this state ([`EngineGate`]).
pub fn hud_state(v: &Vitals) -> HudState {
    let count = |n: i8| n.max(0) as u8;
    let rounds = |n: Option<i16>, or: u16| n.map_or(or, |n| n.max(0) as u16);
    let d = HudState::default();
    HudState {
        mode: Mode::Full,
        shield: v.shield,
        body: v.body,
        grenade: if v.grenade == 1 { Grenade::Plasma } else { Grenade::Frag },
        frags: count(v.grenades[0]),
        plasmas: count(v.grenades[1]),
        rounds_loaded: rounds(v.loaded, 0),
        rounds_reserve: rounds(v.reserve, 0),
        battery: v.age.map_or(d.battery, |a| (1.0 - a).clamp(0.0, 1.0)),
        gate: true,
        ..d
    }
}

/// User `user`'s unit and held weapon, read from the guest; `None` with no
/// local player for the user or between lives.
pub fn vitals(m: &dyn GuestMemory, user: u32) -> Option<Vitals> {
    use engine::view::{objects, players};
    let array = players::array(m).ok()??;
    let player = (0..16).find_map(|i| {
        let p = array.get_absolute(m, i).ok()??;
        (p.listed(m).ok()? && i32::from(p.local_user(m).ok()?) == user as i32).then_some(p)
    })?;
    let (_, body) = objects::get(m, player.unit(m).ok()??).ok()??;
    let unit = Unit::at(body.addr);
    let mut v = Vitals {
        shield: body.shield(m).ok()?,
        body: body.health(m).ok()?,
        grenade: unit.grenade_index(m).ok()?,
        grenades: unit.grenade_counts(m).ok()?,
        ..Vitals::default()
    };
    let held = unit
        .held_weapon(m)
        .ok()
        .flatten()
        .and_then(|d| engine::DatumHandle::from_raw(d.raw()))
        .and_then(|h| objects::get(m, h).ok().flatten());
    if let Some((_, w)) = held {
        let w = Weapon::at(w.addr);
        v.loaded = w.magazine_loaded(m).ok();
        v.reserve = w.magazine_reserve(m).ok();
        v.age = w.age(m).ok();
    }
    Some(v)
}

/// Whether the HUD rectangle (top, left, bottom, right) is the full
/// screen.
pub fn full_screen(rect: [i16; 4]) -> bool {
    rect == [0, 0, CANVAS[1] as i16, CANVAS[0] as i16]
}

fn hud_rect(m: &dyn GuestMemory) -> Option<[i16; 4]> {
    let word = |n: u32| m.u16(HUD_RECT + 2 * n).ok().map(|w| w as i16);
    Some([word(0)?, word(1)?, word(2)?, word(3)?])
}

/// A map's HUDs as the walk reads them: the map, its bitmaps and its
/// `nhdt`s by the address the engine draws them from.
struct Loaded {
    map: NativeMap,
    art: Art,
    huds: HashMap<u32, Hud>,
}

/// The map whose HUDs are loaded (or loading), and those claims stopped
/// on ([`mismatch`]).
#[derive(Default)]
struct Slot {
    map: Option<(String, u64)>,
    loaded: Option<Arc<Mutex<Loaded>>>,
    refused: HashSet<u32>,
    assets: Arc<Mutex<OverlayAssets>>,
}

static SLOT: LazyLock<Mutex<Slot>> = LazyLock::new(Mutex::default);

/// The loaded map's HUDs and the assets their passes draw from; `None`
/// while they load.
fn loaded(mem: &dyn GuestMemory) -> Option<(Arc<Mutex<Loaded>>, Arc<Mutex<OverlayAssets>>)> {
    let slot = entered(mem)?;
    Some((slot.loaded.clone()?, Arc::clone(&slot.assets)))
}

/// The loaded map's overlay assets: the glyph pages every list drawn from
/// the overlay shares ([`fill_glyphs`]) and the HUD's decoded images. New
/// at each map, with the generation the residency keys on. The first ask
/// on a map also starts the load of its HUDs. `None` between maps, when
/// no map's name is loaded.
pub fn assets(mem: &dyn GuestMemory) -> Option<Arc<Mutex<OverlayAssets>>> {
    Some(Arc::clone(&entered(mem)?.assets))
}

/// The slot of the loaded map: on another map than the one it holds,
/// emptied and that map's HUDs' load started.
fn entered(mem: &dyn GuestMemory) -> Option<std::sync::MutexGuard<'static, Slot>> {
    let name = crate::health::loaded_map_name(mem).ok().filter(|n| !n.is_empty())?;
    let generation = render_bridge::native_map_work::generation();
    let mut slot = SLOT.lock().unwrap_or_else(|e| e.into_inner());
    if slot.map.as_ref().is_none_or(|(n, g)| *n != name || *g != generation) {
        slot.map = Some((name.clone(), generation));
        slot.loaded = None;
        slot.refused.clear();
        slot.assets = Arc::new(Mutex::new(OverlayAssets { generation, ..OverlayAssets::default() }));
        spawn_load(name, generation);
    }
    Some(slot)
}

fn spawn_load(name: String, generation: u64) {
    let spawned = std::thread::Builder::new().name("hud overlay load".into()).spawn(move || {
        // The fonts are read here first, off the game thread.
        let _ = fonts();
        let loaded = match load(&name) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("hud overlay: {name}: the HUDs did not load ({e}); the game draws its HUD");
                return;
            }
        };
        let mut slot = SLOT.lock().unwrap_or_else(|e| e.into_inner());
        if slot.map.as_ref().is_some_and(|(n, g)| *n == name && *g == generation) {
            slot.loaded = Some(Arc::new(Mutex::new(loaded)));
        }
    });
    if let Err(e) = spawned {
        eprintln!("hud overlay: {e}: the HUDs did not load; the game draws its HUD");
    }
}

fn load(name: &str) -> Result<Loaded, String> {
    let (mut map, files) = crate::native_map::open(name).map_err(|e| e.to_string())?;
    let ids: Vec<(u32, TagId)> = map
        .tags()
        .iter()
        .filter(|t| t.group == NHDT && t.location.is_some())
        .map(|t| (t.index_address, t.id))
        .collect();
    let mut huds = HashMap::new();
    for (address, id) in ids {
        match hud_edit::model::read(&mut map, id) {
            Ok(hud) => {
                huds.insert(address, hud);
            }
            Err(e) => eprintln!("hud overlay: {name}: {id}: {e:#}"),
        }
    }
    // The bitmaps' sequences read here, not on the game thread.
    let mut art = Art::new(files);
    for w in huds.values().flat_map(|h| &h.widgets) {
        if let Some(bitmap) = w.bitmap() {
            let _ = art.sprite_place(&mut map, bitmap, w.sequence(Mode::Full));
        }
    }
    Ok(Loaded { map, art, huds })
}

/// A claimed HUD draw's quads left out other than its pass drew: that
/// `nhdt` is the game's for the rest of the map.
pub fn mismatch(nhdt: u32) {
    SLOT.lock().unwrap_or_else(|e| e.into_inner()).refused.insert(nhdt);
}

/// The walk of HUD draw `at` over `l`'s HUD: its items, when every shown
/// widget is the engine's, or why not.
fn walk_draw(l: &mut Loaded, mem: &dyn GuestMemory, at: &HudBitmaps) -> Result<Overlay, HudRefusal> {
    let Loaded { map, art, huds } = l;
    let hud = huds.get(&at.nhdt).ok_or(HudRefusal::Loading)?;
    let st = vitals(mem, at.user).map_or_else(|| hud_state(&Vitals::default()), |v| hud_state(&v));
    let mut part = Overlay::new(CANVAS);
    let listed = walk::draw_list(map, art, None, &[hud], &st, &EngineGate::new(at), Shading::Engine, &mut part);
    if let Some((_, why)) = walk::first_inexact(&listed, &part, &translates) {
        return Err(refusal(why));
    }
    Ok(part)
}

/// Decodes the images `part`'s quads name into `assets`.
fn decode_images(l: &mut Loaded, part: &Overlay, assets: &mut OverlayAssets) -> Result<(), HudRefusal> {
    for item in &part.items {
        let OverlayTexture::Bitmap { tag, image } = item.material.texture else { continue };
        if assets.images.contains_key(&(tag, image)) {
            continue;
        }
        let (width, height, rgba) =
            l.art.image(&mut l.map, TagId(tag), usize::from(image)).map_err(|_| HudRefusal::Unlisted)?;
        assets.images.insert((tag, image), OverlayImage { width, height, rgba: rgba.into() });
    }
    Ok(())
}

/// Puts the glyphs `part`'s text runs draw into the assets' pages
/// ([`fill_glyphs`]). A glyph the fonts do not give refuses the claim, as
/// an image that does not decode does.
fn decode_glyphs(part: &Overlay, glyphs: &mut GlyphPages) -> Result<(), HudRefusal> {
    // Without the fonts only a list whose glyphs are all in the pages
    // already fills.
    let none = Fonts::served(b"", []);
    fill_glyphs(glyphs, part, fonts().as_deref().unwrap_or(&none)).map_err(|_| HudRefusal::Unlisted)
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

/// HUD draw `at` at its first bitmap widget: draws its widgets from the
/// overlay, placed here in the frame, and answers how many game quads
/// that leaves out; or why the game draws them.
pub fn claim<B: d3d::Backend + Send + 'static>(
    device: &d3d::abi::SharedDevice<B>,
    mem: &mut dyn GuestMemory,
    at: &HudBitmaps,
) -> Result<u32, HudRefusal> {
    if !hud_rect(mem).is_some_and(full_screen) {
        return Err(HudRefusal::Frame);
    }
    let (loaded, assets) = loaded(mem).ok_or(HudRefusal::Loading)?;
    // A HUD whose claim did not match is the game's for the map.
    if SLOT.lock().unwrap_or_else(|e| e.into_inner()).refused.contains(&at.nhdt) {
        return Err(HudRefusal::Unlisted);
    }
    let mut l = loaded.lock().unwrap_or_else(|e| e.into_inner());
    let part = walk_draw(&mut l, mem, at)?;
    if part.is_empty() {
        // Nothing shown: nothing to draw, and nothing is claimed.
        return Ok(0);
    }
    {
        let mut assets = assets.lock().unwrap_or_else(|e| e.into_inner());
        decode_images(&mut l, &part, &mut assets)?;
        decode_glyphs(&part, &mut assets.glyphs)?;
    }
    drop(l);
    let quads = part.items.len() as u32;
    let overlay = Arc::new(part);
    if !place(device, mem, Arc::clone(&overlay), assets) {
        return Err(HudRefusal::Pass);
    }
    render_bridge::scene_step::with_overlay(|o| {
        if o.append(&overlay).is_err() {
            eprintln!("hud overlay: the scene's overlay refused a HUD's items");
        }
    });
    Ok(quads)
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
/// whether it was placed.
pub fn place_menu<B: d3d::Backend + Send + 'static>(
    device: &d3d::abi::SharedDevice<B>,
    mem: &mut dyn GuestMemory,
    part: &Overlay,
) -> bool {
    if part.items.iter().any(|item| native_render::ui_gpu::overlay_blend(item.material.blend).is_err()) {
        return false;
    }
    let (loaded, assets) = {
        let Some(slot) = entered(mem) else { return false };
        (slot.loaded.clone(), Arc::clone(&slot.assets))
    };
    if part.items.iter().any(|item| matches!(item.material.texture, OverlayTexture::Glyphs { .. })) {
        let Some(fonts) = fonts() else { return false };
        let mut a = assets.lock().unwrap_or_else(|e| e.into_inner());
        if fill_glyphs(&mut a.glyphs, part, &fonts).is_err() {
            return false;
        }
    }
    if part.items.iter().any(|item| matches!(item.material.texture, OverlayTexture::Bitmap { .. })) {
        let Some(loaded) = loaded else { return false };
        let mut l = loaded.lock().unwrap_or_else(|e| e.into_inner());
        let mut a = assets.lock().unwrap_or_else(|e| e.into_inner());
        // A menu can refer to UI assets outside the level's map. Only
        // populate the shared image key when both indices name this bitmap.
        for item in &part.items {
            let OverlayTexture::Bitmap { tag, .. } = item.material.texture else { continue };
            let Some(datum) = guest_layout::DatumHandle::from_raw(tag) else { return false };
            let Ok(entry) = l.map.tag(TagId(tag)) else { return false };
            let address = guest_layout::tags::tag_get_checked(mem, datum, guest_layout::tags::GroupTag::from_chars(*b"bitm"));
            if entry.location.is_none() || address.ok().flatten() != Some(entry.index_address) {
                return false;
            }
        }
        if decode_images(&mut l, part, &mut a).is_err() {
            return false;
        }
    }
    place(device, mem, Arc::new(part.clone()), assets)
}

/// The renderer's overlay programs and resident textures, on the device
/// last drawn with.
static RENDERER: LazyLock<Mutex<Option<(wgpu::Device, OverlayGpu, OverlayResidency)>>> =
    LazyLock::new(|| Mutex::new(None));

/// A list's items drawn where the pass is placed in the frame, from
/// `assets` and the shared renderer (its residency holds every glyph page
/// once per device). The HUD's claims and a menu's list both use it.
pub struct OverlayPass {
    pub overlay: Arc<Overlay>,
    pub assets: Arc<Mutex<OverlayAssets>>,
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
    fn the_hud_state_takes_the_units_and_weapons_values() {
        let v = Vitals {
            shield: 0.5,
            body: 0.25,
            grenade: 1,
            grenades: [3, -1],
            loaded: Some(12),
            reserve: Some(-4),
            age: Some(0.75),
        };
        let st = hud_state(&v);
        assert_eq!((st.shield, st.body), (0.5, 0.25));
        assert_eq!((st.grenade, st.frags, st.plasmas), (Grenade::Plasma, 3, 0));
        assert_eq!((st.rounds_loaded, st.rounds_reserve), (12, 0));
        assert_eq!(st.battery, 0.25);
        assert!(st.gate && st.mode == Mode::Full);
        assert_eq!(st.magazine, HudState::default().magazine, "the magazine's size is not read");
        let unarmed = hud_state(&Vitals { grenade: -1, ..Vitals::default() });
        assert_eq!((unarmed.grenade, unarmed.rounds_loaded, unarmed.battery), (Grenade::Frag, 0, 1.0));
    }

    #[test]
    fn only_the_full_screen_is_drawn() {
        assert!(full_screen([0, 0, 480, 640]));
        assert!(!full_screen([0, 0, 240, 640]), "the top player's half");
        assert!(!full_screen([240, 320, 480, 640]));
    }

    #[test]
    fn each_inexact_widget_names_its_refusal() {
        let reasons: Vec<_> = Inexact::ALL.iter().map(|w| refusal(*w)).collect();
        assert_eq!(
            reasons,
            [HudRefusal::Shader, HudRefusal::Text, HudRefusal::Effects, HudRefusal::Anchor, HudRefusal::Unlisted]
        );
        assert!(!translates(0xe001_0002), "no HUD shader translates yet");
    }

    /// The walk's gate is the engine's: the words as passed, word C with
    /// the widget's thresholds against the record.
    #[test]
    fn the_walk_is_gated_by_the_engines_words() {
        let at = HudBitmaps {
            frame: 0,
            nhdt: 0,
            user: 0,
            words: [0x0004, 0, 0, 0],
            record: Some(HudGateRecord { at_10: 1.0, at_06: 0, at_08: 0 }),
        };
        let gate = EngineGate::new(&at);
        let mut w = Widget {
            key: hud_edit::model::WidgetKey { kind: hud_edit::model::WidgetKind::Bitmap, index: 0 },
            address: 0,
            bytes: vec![0; hud_edit::layout::BITMAP_SIZE],
            original: Vec::new(),
            name: "frag".into(),
        };
        w.bytes[hud_edit::layout::bitmap::STATE] = 0x04;
        assert_eq!(gate.shown(0, &w), Some(true), "show word A 0x04");
        w.bytes[hud_edit::layout::bitmap::STATE] = 0x08;
        assert_eq!(gate.shown(0, &w), Some(false));
        // Shown on word C 0x08: the threshold byte 2.0 is above 1.0.
        w.bytes[hud_edit::layout::bitmap::STATE] = 0;
        w.bytes[hud_edit::layout::bitmap::STATE + 4] = 0x08;
        w.bytes[hud_edit::layout::bitmap::STATE + 0x10] = 2;
        assert_eq!(gate.shown(0, &w), Some(true));
    }

    /// An invented font: a blank record every unit maps to, with a glyph
    /// of its own for each `(unit, width, height, alpha over 15)`, in the
    /// font's ascent of 12.
    fn invented_font(glyphs: &[(u16, u16, u16, u16)]) -> Vec<u8> {
        let mut base = vec![0; font::RECORDS_OFFSET + font::RECORD_LEN];
        let put = |at: usize, bytes: &[u8]| base[at..at + bytes.len()].copy_from_slice(bytes);
        put(font::HEADER_OFFSET, &font::MAGIC.to_le_bytes());
        put(font::HEADER_OFFSET + 4, &12u16.to_le_bytes());
        put(font::HEADER_OFFSET + 0xc, &1u32.to_le_bytes());
        let added = glyphs
            .iter()
            .map(|&(unit, width, height, alpha)| font::patch::Replacement {
                unit,
                width,
                height,
                advance: i16::from(width) + 1,
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
        let (laid, _) = fonts.layout(i16::from(font), text).unwrap();
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
        assert_eq!(decode_glyphs(&plain, &mut GlyphPages::default()), Ok(()));
    }
}
