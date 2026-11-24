//! The HUD drawn from the overlay stage (docs/graphics/native-post-hud.md,
//! "The HUD on the overlay"; docs/graphics/retire-d3d.md, slice R6).
//!
//! At each HUD draw's first bitmap widget (`render_bridge::native_hud::
//! hud_overlay`) the game thread reads the user's unit and held weapon into
//! a [`HudState`], walks the `nhdt` with the engine's own gate words
//! (`hud_edit::overlay::draw_list`, [`Shading::Engine`]), and, when every
//! shown widget is drawn as the engine draws it, adds the items to the
//! scene's overlay (`FrameScene::overlay`), decodes the bitmap images they
//! name, and places a pass at that point in the frame that draws them with
//! `native_render::ui_gpu::OverlayGpu`. The game's quads for those widgets
//! are then left out. Any other HUD draw is the game's, counted by its
//! first reason (`native_ui_overlay_hud_refused_*`).
//!
//! What keeps a retail HUD on the game's quads today: every bitmap widget
//! is drawn through its tag shader, and no HUD shader translates yet
//! ([`translates`]); a text widget's colour is its shader's.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};

use engine::memory::GuestMemory;
use frame_scene::{Overlay, OverlayTexture};
use guest_layout::{Unit, Weapon};
use hud_edit::layout::Mode;
use hud_edit::model::{Hud, NHDT, Widget};
use hud_edit::overlay::{self as walk, Inexact, Shading};
use hud_edit::render::{Answers, Art};
use hud_edit::state::{self, GateRecord, Grenade, HudState};
use map::TagId;
use native_render::overlay::{OverlayAssets, OverlayImage, OverlayPlan};
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

/// The loaded map's HUDs and the assets their passes draw from, starting
/// the load at the first ask on a map; `None` while it loads.
fn loaded(mem: &dyn GuestMemory) -> Option<(Arc<Mutex<Loaded>>, Arc<Mutex<OverlayAssets>>)> {
    let (loaded, assets) = slot(mem)?;
    Some((loaded?, assets))
}

/// The map's overlay assets, shared by the HUD's and the menus' passes
/// (one generation a map, so the resident textures are not replaced
/// between them); `None` with no map loaded.
pub fn assets(mem: &dyn GuestMemory) -> Option<Arc<Mutex<OverlayAssets>>> {
    slot(mem).map(|(_, assets)| assets)
}

/// The slot for the loaded map: its HUDs, once loaded, and its assets.
fn slot(mem: &dyn GuestMemory) -> Option<(Option<Arc<Mutex<Loaded>>>, Arc<Mutex<OverlayAssets>>)> {
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
    Some((slot.loaded.clone(), slot.assets.clone()))
}

fn spawn_load(name: String, generation: u64) {
    let spawned = std::thread::Builder::new().name("hud overlay load".into()).spawn(move || {
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
    decode_images(&mut l, &part, &mut assets.lock().unwrap_or_else(|e| e.into_inner()))?;
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
    let Some(assets) = assets(mem) else { return false };
    place(device, mem, Arc::new(part.clone()), assets)
}

/// The renderer's overlay programs and resident textures, on the device
/// last drawn with.
static RENDERER: LazyLock<Mutex<Option<(wgpu::Device, OverlayGpu, OverlayResidency)>>> =
    LazyLock::new(|| Mutex::new(None));

/// One HUD draw's items, drawn where the HUD draw was in the frame.
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
}
