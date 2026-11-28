//! The HUD walk in Rust (docs/graphics/native-hud-overlay.md, "The walk
//! in Rust"): at the guest's walk of a definition (`HUD_WALK`) the
//! widgets it shows and their inputs are worked out from the definition
//! and the player's HUD record (`hud_kit_core::walk`), and a bitmap
//! widget Rust selected with the inputs the guest gives it is laid out
//! in Rust at its draw (`HUD_BITMAP_WIDGET`): its sprite, its anchor, its
//! effects (`engine::hud_effects`), its quads and the globals it writes.
//!
//! A text widget is laid out the same way at `HUD_TEXT_WIDGET`: its
//! string (the table's, or its number), its rectangle from the measure,
//! its glyphs through the host's fonts ([`UiText`]) and each glyph quad's
//! colour, checked against the guest's glyph quads at the screen quad.
//!
//! A screen-effect widget's call of `0x0002d140` (its effect, its bitmap
//! and its grid) is worked out at `HUD_SCREEN_WIDGET` and checked against
//! the guest's call; a proven widget's body is left out and the guest
//! runs the call alone, on a frame Rust fills ([`call_frame`]).
//!
//! The loop itself is Rust's once the guest's widget calls matched
//! Rust's selection on [`PROOF_DRAWS`] walks of the definition and every
//! widget it shows is proven ([`plan_loop`]): the widgets are laid out
//! and drawn in the guest's order and the guest's walk is left out. One
//! walk in [`RECHECK`] is the guest's, with every widget checked again.
//!
//! A text widget laid out in Rust is Rust's at its draw ([`plan_text`]):
//! the host draws its glyph quads and the guest's string draw never runs.
//!
//! A bitmap widget is the guest's until [`PROOF_DRAWS`] of its draws matched
//! Rust's quad for quad (the vertices, the bitmap, image and shader and
//! the globals, bit for bit, at `HUD_QUAD_SETUP`). After that Rust draws
//! it through the host ([`super::Host::rust_hud_widget`]) and the guest's
//! draw is left out; one draw in [`RECHECK`] is still the guest's and is
//! checked again. A draw that differs keeps the widget on the guest for
//! the map.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::sync::{LazyLock, Mutex};

use engine::hud_effects;
use guest_layout::memory::GuestMemory;
use scene_extract::hud::{self, Blocks, GuestBitmap, walk};
use scene_extract::ui::{Justify, UiText};
pub use walk::{GlyphQuad, ScreenDraw};
use walk::{Bitmap as _, Kind, Refusal, Shown};

use super::{HudVertices, hud_inputs};

/// Matching draws that prove a widget.
pub const PROOF_DRAWS: u32 = 8;
/// A proven widget's draws the guest still makes: one in this many.
pub const RECHECK: u32 = 64;

/// One quad of a widget Rust draws.
#[derive(Clone, Debug, PartialEq)]
pub struct WalkQuad {
    /// The globals the guest writes before this quad, as (address, word):
    /// the host writes them before it reads the quad's material.
    pub writes: Vec<(u32, u32)>,
    pub image: u16,
    pub vertices: HudVertices,
}

/// A bitmap widget laid out in Rust.
#[derive(Clone, Debug, PartialEq)]
pub struct WalkWidget {
    /// The anchor and its point: what the group's fit is taken at.
    pub anchor: u32,
    pub point: [f32; 2],
    pub bitmap: u32,
    pub shader: u32,
    pub quads: Vec<WalkQuad>,
}

/// A text widget laid out in Rust.
#[derive(Clone, Debug, PartialEq)]
pub struct WalkText {
    /// The anchor and its point: what the group's fit is taken at.
    pub anchor: u32,
    pub point: [f32; 2],
    pub shader: u32,
    /// The engine font.
    pub font: u16,
    /// The glyph quads in the guest's order, in the window's pixels.
    pub quads: Vec<GlyphQuad>,
    /// The window's left and top in the frame: the quads' origin.
    pub origin: [f32; 2],
    /// The globals the guest's draw leaves for its shader: the host
    /// writes them before it reads the shader's material.
    pub writes: Vec<(u32, u32)>,
}

/// Whose a widget's draw is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arm {
    /// The guest draws it, unchecked: a draw of it differed on this map.
    Guest,
    /// The guest draws it and each quad is checked against Rust's.
    Shadow,
    /// Rust draws it.
    Own,
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Proof {
    clean: u32,
    failed: bool,
    /// Draws since it was proven, for the recheck.
    seen: u32,
}

impl Proof {
    pub(super) fn arm(&mut self) -> Arm {
        if self.failed {
            return Arm::Guest;
        }
        if self.clean < PROOF_DRAWS {
            return Arm::Shadow;
        }
        self.seen = self.seen.wrapping_add(1);
        if self.seen % RECHECK == 0 { Arm::Shadow } else { Arm::Own }
    }

    /// Whether a draw of it differed: the guest's for the map.
    pub(super) fn failed(&self) -> bool {
        self.failed
    }

    /// Whether Rust draws it, asked without counting a draw.
    pub(super) fn proven(&self) -> bool {
        !self.failed && self.clean >= PROOF_DRAWS
    }

    /// A shadowed draw's answer. True when this one proved the widget.
    pub(super) fn checked(&mut self, matched: bool) -> bool {
        if !matched {
            self.failed = true;
            return false;
        }
        self.clean = self.clean.saturating_add(1);
        self.clean == PROOF_DRAWS
    }
}

#[derive(Default)]
struct State {
    generation: Option<u64>,
    /// By the widget's address.
    proofs: HashMap<u32, Proof>,
    /// The loop's, by the definition's datum.
    walks: HashMap<u32, Proof>,
    /// The order of the definitions' walks (`super::definitions`), by
    /// the local user.
    definitions: HashMap<u32, Proof>,
    /// The motion sensor's draw calls (`super::sensor`), by the local user.
    sensors: HashMap<u32, Proof>,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(Mutex::default);

fn state<T>(f: impl FnOnce(&mut State) -> T) -> T {
    let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let generation = crate::native_map_work::generation();
    if s.generation != Some(generation) {
        *s = State { generation: Some(generation), ..State::default() };
    }
    f(&mut s)
}

fn proof<T>(widget: u32, f: impl FnOnce(&mut Proof) -> T) -> T {
    state(|s| f(s.proofs.entry(widget).or_default()))
}

fn walk_proof<T>(definition: u32, f: impl FnOnce(&mut Proof) -> T) -> T {
    state(|s| f(s.walks.entry(definition).or_default()))
}

/// The proof of local user `user`'s definitions' order, kept with the
/// walk's for the map.
pub(super) fn definitions_proof<T>(user: u32, f: impl FnOnce(&mut Proof) -> T) -> T {
    state(|s| f(s.definitions.entry(user).or_default()))
}

/// The proof of local user `user`'s motion sensor draws, kept with the
/// walk's for the map.
pub(super) fn sensor_proof<T>(user: u32, f: impl FnOnce(&mut Proof) -> T) -> T {
    state(|s| f(s.sensors.entry(user).or_default()))
}

/// The loop's side of the walk the guest is in.
#[derive(Default)]
struct Loop {
    /// The definition walked: its datum.
    definition: u32,
    /// The guest loops as the loop's check: each widget is checked too.
    shadow: bool,
    /// The widgets Rust drew before it gave the loop back.
    done: Vec<(Kind, u32)>,
}

thread_local! {
    static LOOP: RefCell<Loop> = const { RefCell::new(Loop { definition: 0, shadow: false, done: Vec::new() }) };
}

/// Whose a widget's draw is: a proven widget is checked again on a walk
/// the guest loops as a check.
fn arm(widget: u32) -> Arm {
    match proof(widget, Proof::arm) {
        Arm::Own if LOOP.with(|l| l.borrow().shadow) => Arm::Shadow,
        other => other,
    }
}

static COUNTS: Mutex<BTreeMap<&'static str, u64>> = Mutex::new(BTreeMap::new());

pub(super) fn count(what: &'static str) {
    *COUNTS.lock().unwrap_or_else(|e| e.into_inner()).entry(what).or_default() += 1;
}

/// `native_ui_overlay_hud_walk_<what>` for everything the walk counted.
pub fn counters() -> Vec<(String, u64)> {
    let counts = COUNTS.lock().unwrap_or_else(|e| e.into_inner());
    counts.iter().map(|(what, &n)| (format!("native_ui_overlay_hud_walk_{what}"), n)).collect()
}

/// Whether a widget (by its address) is one another feature's hooks
/// change inside the guest's draw.
pub type Foreign = fn(&dyn GuestMemory, u32) -> bool;

static FOREIGN: Mutex<Vec<Foreign>> = Mutex::new(Vec::new());

/// Names widgets a feature changes at the guest's hooks (the HUD kit's,
/// a converted Halo 1 HUD's): their draws stay the guest's.
pub fn add_foreign(f: Foreign) {
    let mut all = FOREIGN.lock().unwrap_or_else(|e| e.into_inner());
    if !all.iter().any(|&g| std::ptr::fn_addr_eq(g, f)) {
        all.push(f);
    }
}

fn foreign(mem: &dyn GuestMemory, widget: u32) -> bool {
    let all = FOREIGN.lock().unwrap_or_else(|e| e.into_inner()).clone();
    all.iter().any(|f| f(mem, widget))
}

/// One quad the guest is expected to make.
#[derive(Clone, Debug, PartialEq)]
struct ExpectQuad {
    /// Every global the widget has written by this quad, latest word.
    globals: Vec<(u32, u32)>,
    image: u16,
    /// x, y, z, w, u, v of each vertex, as bits.
    vertices: [[u32; 6]; 4],
}

/// A shadowed draw: what Rust laid out, and how far the guest's matched.
#[derive(Clone, Debug, PartialEq)]
struct Expect {
    widget: u32,
    bitmap: u32,
    shader: u32,
    quads: Vec<ExpectQuad>,
    /// Every global written by the draw's end.
    globals: Vec<(u32, u32)>,
    next: usize,
    matched: bool,
}

/// The walk the guest is in.
struct Active {
    blocks: Blocks,
    shown: Vec<Shown>,
    /// The widget draws the guest made: kind, address, the inputs' bits.
    calls: Vec<(Kind, u32, [u32; 4])>,
    /// A foreign widget is in the definition: the guest's selection is
    /// not Rust's to check.
    foreign: bool,
    expect: Option<Expect>,
    screen: Option<ExpectScreen>,
}

/// A shadowed screen-effect widget: the call Rust worked out, and the
/// guest's calls against it.
#[derive(Clone, Debug, PartialEq)]
struct ExpectScreen {
    widget: u32,
    draw: ScreenDraw,
    calls: u32,
    matched: bool,
}

thread_local! {
    static ACTIVE: RefCell<Option<Active>> = const { RefCell::new(None) };
}

fn four(mem: &dyn GuestMemory, at: u32) -> Option<[f32; 4]> {
    let mut v = [0.0; 4];
    for (i, value) in v.iter_mut().enumerate() {
        *value = f32::from_bits(mem.u32(at.checked_add(4 * i as u32)?).ok()?);
    }
    Some(v)
}

fn bits(v: &[f32; 4]) -> [u32; 4] {
    v.map(f32::to_bits)
}

/// `HUD_WALK`'s entry: Rust's own walk of the definition.
pub fn enter(mem: &dyn GuestMemory, definition: u32, player: u32, record: u32, slot: u32) {
    let read = || {
        let blocks = hud::blocks(mem, definition)?;
        let record = hud::record(mem, record)?;
        let session = hud::session(mem, player)?;
        let words = walk::state_words(&record, slot, &session)?;
        let shown = walk::shown(&blocks.definition(), &words, &record, &session);
        let any_foreign = [Kind::Screen, Kind::Bitmap, Kind::Text].into_iter().any(|kind| {
            let block = match kind {
                Kind::Screen => &blocks.screens.1,
                Kind::Bitmap => &blocks.bitmaps.1,
                Kind::Text => &blocks.texts.1,
            };
            (0..block.len() / kind.size()).any(|i| foreign(mem, blocks.address(kind, i)))
        });
        Some(Active { blocks, shown, calls: Vec::new(), foreign: any_foreign, expect: None, screen: None })
    };
    let active = read();
    if active.is_none() {
        count("read");
    }
    ACTIVE.with(|a| *a.borrow_mut() = active);
    LOOP.with(|l| *l.borrow_mut() = Loop { definition, ..Loop::default() });
}

/// What the walk's entry does.
#[derive(Clone, Debug, PartialEq)]
pub enum LoopPlan {
    /// The guest loops.
    Guest,
    /// Rust loops: the widgets shown, in the guest's order, each with
    /// its address and inputs.
    Own(Vec<(Kind, u32, [f32; 4])>),
}

/// `HUD_WALK`'s entry, after [`enter`]: whose the loop is. Rust's when
/// the definition's selection is proven, no feature changes a widget of
/// it, no screen effect is shown (its call is the guest's) and every
/// widget shown is proven.
pub fn plan_loop() -> LoopPlan {
    let refused = |why: &'static str| {
        count(why);
        LoopPlan::Guest
    };
    let read = |a: &Active| (a.foreign, a.shown.iter().map(|s| (s.kind, a.blocks.address(s.kind, s.index), s.inputs)).collect::<Vec<_>>());
    let Some((foreign, shown)) = ACTIVE.with(|a| a.borrow().as_ref().map(read)) else { return LoopPlan::Guest };
    if foreign {
        return refused("loop_feature");
    }
    let definition = LOOP.with(|l| l.borrow().definition);
    // The definitions' recheck checks every walk of it again.
    let arm = match walk_proof(definition, Proof::arm) {
        Arm::Own if super::definitions::rechecking() => Arm::Shadow,
        arm => arm,
    };
    match arm {
        Arm::Guest => return refused("loop_differed"),
        Arm::Shadow => {
            LOOP.with(|l| l.borrow_mut().shadow = true);
            return LoopPlan::Guest;
        }
        Arm::Own => {}
    }
    if shown.iter().any(|s| s.0 == Kind::Screen) {
        return refused("loop_screen");
    }
    // A text widget is Rust's at its draw ([`plan_text`]): only the
    // bitmaps' proofs hold the loop.
    if !shown.iter().all(|s| s.0 == Kind::Text || proof(s.1, |p| p.proven())) {
        return refused("loop_unproven");
    }
    LoopPlan::Own(shown)
}

/// A bitmap widget of a loop Rust runs, laid out as at its draw. `None`:
/// the loop goes back to the guest at this widget.
pub fn own_bitmap(mem: &dyn GuestMemory, fp: Fp, player: u32, record: u32, widget: u32, inputs: &[f32; 4]) -> Option<(WalkWidget, Vec<(u32, u32)>)> {
    match lay_out(mem, fp, player, record, widget, inputs) {
        Ok(Some(laid)) => Some(laid),
        Ok(None) => {
            count("empty");
            None
        }
        Err(why) => {
            count(why);
            None
        }
    }
}

/// A text widget of a loop Rust runs, as [`own_bitmap`].
pub fn own_text(mem: &dyn GuestMemory, fp: Fp, fonts: Option<&dyn UiText>, record: u32, widget: u32, inputs: &[f32; 4]) -> Option<WalkText> {
    let Some(fonts) = fonts else {
        count("text_fonts");
        return None;
    };
    match lay_out_text(mem, fp, fonts, record, widget, inputs) {
        Ok(Some(laid)) => Some(laid),
        Ok(None) => {
            count("text_empty");
            None
        }
        Err(why) => {
            count("text_refused");
            count(why);
            None
        }
    }
}

/// Rust drew `widget` in its loop.
pub fn drew(kind: Kind, widget: u32) {
    LOOP.with(|l| l.borrow_mut().done.push((kind, widget)));
}

/// Whether Rust drew `widget` in this walk before it gave the loop back:
/// the guest's draw of it is left out.
pub fn done(kind: Kind, widget: u32) -> bool {
    LOOP.with(|l| l.borrow().done.contains(&(kind, widget)))
}

/// Rust's loop drew every widget: the guest's walk is left out.
pub fn looped() {
    ACTIVE.with(|a| *a.borrow_mut() = None);
    LOOP.with(|l| *l.borrow_mut() = Loop::default());
    count("loop_owned");
}

/// Rust's loop met a widget that is not Rust's: the guest loops, and
/// the widgets in [`done`] are left out of it.
pub fn gave_back() {
    count("loop_gave_back");
}

/// The walk's selection as the guest's calls would be.
fn expected_calls(active: &Active) -> Vec<(Kind, u32, [u32; 4])> {
    active.shown.iter().map(|s| (s.kind, active.blocks.address(s.kind, s.index), bits(&s.inputs))).collect()
}

/// `HUD_WALK`'s return: the guest's widget draws against Rust's selection.
pub fn leave() {
    let walked = LOOP.with(|l| std::mem::take(&mut *l.borrow_mut()));
    let Some(active) = ACTIVE.with(|a| a.borrow_mut().take()) else { return };
    if active.foreign {
        count("selection_foreign");
    } else if expected_calls(&active) != active.calls {
        count("selection_differed");
        walk_proof(walked.definition, |p| p.checked(false));
    } else {
        count("selection_matched");
        if walked.shadow && walk_proof(walked.definition, |p| p.checked(true)) {
            count("loop_proven");
        }
    }
}

/// A widget draw's entry: the guest's selection, kept for [`leave`].
pub fn called(mem: &dyn GuestMemory, kind: Kind, widget: u32, inputs: u32) {
    let inputs = four(mem, inputs).map_or([0; 4], |v| bits(&v));
    ACTIVE.with(|a| {
        if let Some(active) = a.borrow_mut().as_mut() {
            active.calls.push((kind, widget, inputs));
        }
    });
}

/// The floating-point state the guest's draw runs under.
#[derive(Clone, Copy, Debug)]
pub struct Fp {
    pub mxcsr: u32,
    pub control: u16,
}

/// The HUD's function input `name` while a widget with `inputs` draws:
/// the widget's inputs are the shader inputs by then; the rest are the
/// callback's (`hud_inputs`).
fn input(mem: &dyn GuestMemory, inputs: &[f32; 4], name: u32) -> Option<f32> {
    match name {
        0x0a00_05cf..=0x0a00_05d2 => Some(inputs[(name - 0x0a00_05cf) as usize]),
        _ => hud_inputs::function_input(mem, name),
    }
}

/// A widget's effects (`engine::hud_effects`) on `basis`.
fn effects(mem: &dyn GuestMemory, fp: Fp, block: (u32, u32), inputs: &[f32; 4], basis: &mut walk::Basis) -> Result<(), Refusal> {
    let mut b = hud_effects::Basis {
        origin: basis.origin.map(f32::to_bits),
        x: basis.x.map(f32::to_bits),
        y: basis.y.map(f32::to_bits),
    };
    match hud_effects::apply(mem, fp.mxcsr, fp.control, block, &|name| input(mem, inputs, name), &mut b) {
        Ok(true) => {
            *basis = walk::Basis {
                origin: b.origin.map(f32::from_bits),
                x: b.x.map(f32::from_bits),
                y: b.y.map(f32::from_bits),
            };
            Ok(())
        }
        Ok(false) => Err(Refusal::Effect),
        Err(_) => Err(Refusal::Read),
    }
}

/// A value the quad's vertex check passes (`super::vertices`).
fn plain(v: f32) -> bool {
    v == 0.0 || v.is_normal()
}

/// The bitmap widget at `widget` laid out in Rust with `inputs`: the
/// widget for the host and the globals written after its last quad.
/// `Ok(None)`: it draws nothing. `Err`: why Rust does not lay it out.
fn lay_out(
    mem: &dyn GuestMemory,
    fp: Fp,
    player: u32,
    record: u32,
    widget: u32,
    inputs: &[f32; 4],
) -> Result<Option<(WalkWidget, Vec<(u32, u32)>)>, &'static str> {
    let w = mem.bytes(widget, walk::BITMAP_SIZE).map_err(|_| "read")?;
    let tag = u32::from_le_bytes([w[0x24], w[0x25], w[0x26], w[0x27]]);
    if tag == u32::MAX {
        return Ok(None);
    }
    let record = hud::record(mem, record).ok_or("read")?;
    let screen = hud::screen(mem).ok_or("read")?;
    let aim = hud::aim(mem).ok_or("read")?;
    let art = hud::art(mem, player).ok_or("read")?;
    let bitmap = GuestBitmap { mem, data: hud::tag_data(mem, tag).ok_or("read")? };
    let effects = |block: (u32, u32), inputs: &[f32; 4], basis: &mut walk::Basis| effects(mem, fp, block, inputs, basis);
    let draw = walk::bitmap_widget(&w, inputs, &record, &screen, aim, &bitmap, &art, &effects).map_err(Refusal::cause)?;
    let Some(draw) = draw else { return Ok(None) };
    // The guest draws a quad only with its texture there (`0x0003bcb0`).
    if !bitmap.resident(draw.sprite.image) {
        return Err("texture");
    }
    let image = bitmap.image(draw.sprite.image).ok_or("read")?;
    let index = u16::try_from(draw.sprite.image).map_err(|_| "read")?;
    let mut quads = Vec::with_capacity(draw.quads.len());
    for quad in draw.quads {
        let v = walk::vertices(&quad.corners, &quad.bounds, &image);
        if !v.iter().flatten().all(|&f| plain(f)) {
            return Err("vertices");
        }
        let vertices = v.map(|v| [(9, [1.0; 4]), (3, [v[4], v[5], 0.0, 1.0]), (0, [v[0], v[1], v[2], v[3]])]);
        quads.push(WalkQuad { writes: quad.writes, image: index, vertices });
    }
    let widget = WalkWidget { anchor: u32::from(draw.anchor), point: draw.point, bitmap: draw.bitmap, shader: draw.shader, quads };
    Ok(Some((widget, draw.writes)))
}

fn set(globals: &mut Vec<(u32, u32)>, writes: &[(u32, u32)]) {
    for &(at, word) in writes {
        match globals.iter_mut().find(|g| g.0 == at) {
            Some(g) => g.1 = word,
            None => globals.push((at, word)),
        }
    }
}

fn expect(widget: u32, laid: &WalkWidget, last: &[(u32, u32)]) -> Expect {
    let mut globals = Vec::new();
    let quads = laid
        .quads
        .iter()
        .map(|q| {
            set(&mut globals, &q.writes);
            let vertices = q.vertices.map(|v| {
                let (uv, p) = (v[1].1, v[2].1);
                [p[0], p[1], p[2], p[3], uv[0], uv[1]].map(f32::to_bits)
            });
            ExpectQuad { globals: globals.clone(), image: q.image, vertices }
        })
        .collect();
    set(&mut globals, last);
    Expect { widget, bitmap: laid.bitmap, shader: laid.shader, quads, globals, next: 0, matched: true }
}

fn globals_hold(mem: &dyn GuestMemory, globals: &[(u32, u32)]) -> bool {
    globals.iter().all(|&(at, word)| mem.u32(at).ok() == Some(word))
}

/// Whether the guest's quad in the shader quad's `frame` is `quad`.
fn quad_matches(mem: &dyn GuestMemory, frame: u32, e: &Expect, quad: &ExpectQuad) -> bool {
    let read = || {
        let arguments = [mem.u32(frame.checked_add(8)?).ok()?, mem.u32(frame.checked_add(12)?).ok()?, mem.u32(frame.checked_add(16)?).ok()?];
        if arguments != [e.bitmap, u32::from(quad.image), e.shader] {
            return Some(false);
        }
        let base = frame.checked_sub(super::HUD_QUAD_LOCAL_BYTES)?;
        for (i, vertex) in quad.vertices.iter().enumerate() {
            let at = base.checked_add(i as u32 * super::HUD_QUAD_VERTEX_BYTES)?;
            for (j, &word) in vertex.iter().enumerate() {
                if mem.u32(at.checked_add(4 * j as u32)?).ok()? != word {
                    return Some(false);
                }
            }
        }
        Some(globals_hold(mem, &quad.globals))
    };
    read().unwrap_or(false)
}

/// The guest's shader quad at `HUD_QUAD_SETUP`, in a shadowed widget's
/// draw: checked against the next quad Rust laid out.
pub fn observe(mem: &dyn GuestMemory, frame: u32) {
    ACTIVE.with(|a| {
        let mut a = a.borrow_mut();
        let Some(e) = a.as_mut().and_then(|a| a.expect.as_mut()) else { return };
        let caller = frame.checked_add(4).and_then(|at| mem.u32(at).ok()).unwrap_or(0);
        if !matches!(super::kept::quad_kind(caller), super::kept::OverlayKind::Widget | super::kept::OverlayKind::Crosshair) {
            return;
        }
        let matched = e.quads.get(e.next).is_some_and(|quad| quad_matches(mem, frame, e, quad));
        e.matched &= matched;
        e.next += 1;
    });
}

/// Whether Rust's own walk shows `widget` with the inputs the guest
/// gives it: Rust draws only then. `None` outside a walk.
fn selected(kind: Kind, widget: u32, given: &[f32; 4]) -> Option<bool> {
    ACTIVE.with(|a| {
        a.borrow().as_ref().map(|a| {
            a.shown.iter().any(|s| s.kind == kind && a.blocks.address(kind, s.index) == widget && bits(&s.inputs) == bits(given))
        })
    })
}

/// What the bitmap widget's entry does.
#[derive(Clone, Debug, PartialEq)]
pub enum Plan {
    /// The guest draws it.
    Guest,
    /// Rust draws it, then writes these globals.
    Own(WalkWidget, Vec<(u32, u32)>),
}

/// `HUD_BITMAP_WIDGET`'s entry: whose the draw is. A shadowed draw is
/// the guest's, with Rust's layout kept for [`observe`] and [`finish`].
pub fn plan(mem: &dyn GuestMemory, fp: Fp, player: u32, record: u32, widget: u32, inputs: u32) -> Plan {
    let refused = |why: &'static str| {
        count(why);
        Plan::Guest
    };
    let Some(given) = four(mem, inputs) else { return refused("read") };
    match selected(Kind::Bitmap, widget, &given) {
        None => return Plan::Guest,
        Some(false) => return refused("unselected"),
        Some(true) => {}
    }
    if foreign(mem, widget) {
        return refused("feature");
    }
    let (laid, last) = match lay_out(mem, fp, player, record, widget, &given) {
        Ok(Some(laid)) => laid,
        Ok(None) => return refused("empty"),
        Err(why) => return refused(why),
    };
    match arm(widget) {
        Arm::Guest => refused("differed"),
        Arm::Shadow => {
            let e = expect(widget, &laid, &last);
            ACTIVE.with(|a| {
                if let Some(a) = a.borrow_mut().as_mut() {
                    a.expect = Some(e);
                }
            });
            Plan::Guest
        }
        Arm::Own => Plan::Own(laid, last),
    }
}

/// `HUD_BITMAP_WIDGET`'s return: a shadowed draw's answer.
pub fn finish(mem: &dyn GuestMemory) {
    let Some(e) = ACTIVE.with(|a| a.borrow_mut().as_mut().and_then(|a| a.expect.take())) else { return };
    let matched = e.matched && e.next == e.quads.len() && globals_hold(mem, &e.globals);
    count(if matched { "shadow_matched" } else { "shadow_differed" });
    if proof(e.widget, |p| p.checked(matched)) {
        count("proven");
    }
}

/// The measure's rectangle (`0x0022abc9`): 0, 0 to 1000, 1000.
const MEASURE: [f32; 4] = [0.0, 0.0, 1000.0, 1000.0];

/// The text widget at `widget` laid out in Rust with `inputs`.
/// `Ok(None)`: it draws nothing. `Err`: why Rust does not lay it out.
fn lay_out_text(mem: &dyn GuestMemory, fp: Fp, fonts: &dyn UiText, record: u32, widget: u32, inputs: &[f32; 4]) -> Result<Option<WalkText>, &'static str> {
    let w = mem.bytes(widget, walk::TEXT_SIZE).map_err(|_| "read")?;
    let screen = hud::screen(mem).ok_or("read")?;
    let aim = hud::aim(mem).ok_or("read")?;
    let run = |block: (u32, u32), inputs: &[f32; 4], basis: &mut walk::Basis| effects(mem, fp, block, inputs, basis);
    let Some(place) = walk::text_widget(&w, inputs, &screen, aim, &run).map_err(Refusal::cause)? else { return Ok(None) };
    let font = place.font.and_then(|f| u16::try_from(f).ok()).ok_or("font")?;
    let units: Vec<u16> = if place.flags & walk::TEXT_NUMBER != 0 {
        // `0x0022a8b5`: the callback's value, through `_ftol`.
        let value = input(mem, inputs, place.string).ok_or("number")?;
        if !(value.abs() < 2_147_483_648.0) {
            return Err("number");
        }
        walk::number_text(place.flags, value as i32).encode_utf16().collect()
    } else if place.flags & walk::TEXT_NAME != 0 {
        // `0x0022a931`: the record's player's name, or nothing.
        hud::player_name(mem, record).ok_or("name")?
    } else {
        super::hud_overlay::localized(mem, place.string).ok_or("string")?.encode_utf16().collect()
    };
    // A button glyph is laid out as a character: the guest's glyph quads
    // are what proves it.
    let shape = |rect: [f32; 4]| fonts.shape_buttons(font, &units, rect, Justify::Left).ok_or("shape");
    let measured: Vec<[f32; 4]> = shape(MEASURE)?.iter().map(|g| g.rect).collect();
    let Some(bounds) = walk::text_bounds(&measured) else { return Ok(None) };
    let [top, left, bottom, right] = walk::text_rect(place.justification, place.basis.origin, &screen.window, &bounds).map(f32::from);
    let glyphs: Vec<(u32, [f32; 4])> = shape([left, top, right, bottom])?.iter().map(|g| (g.index, g.rect)).collect();
    let (main, shadow) = hud::text_colours(mem).ok_or("read")?;
    let (main, shadow) = walk::text_colours(&main, shadow.as_ref()).ok_or("colour")?;
    // `0x0013f9c2`: a coloured button's glyph is white, not the string's colour.
    let white: Vec<u32> = units.iter().filter(|&&u| walk::untinted(u)).map(|&u| fonts.glyph(font, u).ok_or("shape")).collect::<Result<_, _>>()?;
    let writes = walk::text_globals(inputs, hud::text_tint(mem).ok_or("read")?);
    Ok(Some(WalkText {
        anchor: u32::from(place.anchor),
        point: place.point,
        shader: place.shader,
        font,
        quads: walk::text_quads(&glyphs, &white, main, shadow),
        origin: [f32::from(screen.window[1]), f32::from(screen.window[0])],
        writes,
    }))
}

/// What the text widget's entry does.
#[derive(Clone, Debug, PartialEq)]
pub enum TextPlan {
    /// The guest draws it.
    Guest,
    /// Rust draws it.
    Own(WalkText),
}

/// `HUD_TEXT_WIDGET`'s entry: whose the draw is, as [`plan`] for a
/// bitmap widget. `fonts` lays the string out; with none the guest does.
pub fn plan_text(mem: &dyn GuestMemory, fp: Fp, fonts: Option<&dyn UiText>, record: u32, widget: u32, inputs: u32) -> TextPlan {
    let refused = |why: &'static str| {
        count(why);
        TextPlan::Guest
    };
    let Some(given) = four(mem, inputs) else { return refused("text_read") };
    match selected(Kind::Text, widget, &given) {
        None => return TextPlan::Guest,
        Some(false) => return refused("text_unselected"),
        Some(true) => {}
    }
    if foreign(mem, widget) {
        return refused("text_feature");
    }
    let Some(fonts) = fonts else { return refused("text_fonts") };
    let laid = match lay_out_text(mem, fp, fonts, record, widget, &given) {
        Ok(Some(laid)) => laid,
        Ok(None) => return refused("text_empty"),
        Err(why) => {
            count("text_refused");
            return refused(why);
        }
    };
    TextPlan::Own(laid)
}

/// The screen-effect widget at `widget` worked out in Rust with `inputs`.
/// `Ok(None)`: its mode has no effect, and nothing is called.
fn lay_out_screen(mem: &dyn GuestMemory, player: u32, widget: u32, inputs: &[f32; 4]) -> Result<Option<ScreenDraw>, &'static str> {
    let w = mem.bytes(widget, walk::SCREEN_SIZE).map_err(|_| "read")?;
    let screen = hud::screen(mem).ok_or("read")?;
    let aim = hud::aim(mem).ok_or("read")?;
    let tag = u32::from_le_bytes([w[0x24], w[0x25], w[0x26], w[0x27]]);
    let bitmap = match tag {
        u32::MAX => None,
        tag => Some(GuestBitmap { mem, data: hud::tag_data(mem, tag).ok_or("read")? }),
    };
    let user_state = hud::user_state(mem, player).ok_or("read")?;
    walk::screen_widget(&w, inputs, &screen, aim, bitmap.as_ref().map(|b| b as &dyn walk::Bitmap), user_state).map_err(Refusal::cause)
}

/// What the screen-effect widget's entry does.
#[derive(Clone, Debug, PartialEq)]
pub enum ScreenPlan {
    /// The guest runs the widget.
    Guest,
    /// The guest runs this call alone ([`call_frame`]).
    Own(ScreenDraw),
}

/// `HUD_SCREEN_WIDGET`'s entry: whose the widget's layout is, as [`plan`]
/// for a bitmap widget.
pub fn plan_screen(mem: &dyn GuestMemory, player: u32, widget: u32, inputs: u32) -> ScreenPlan {
    let refused = |why: &'static str| {
        count(why);
        ScreenPlan::Guest
    };
    let Some(given) = four(mem, inputs) else { return refused("screen_read") };
    match selected(Kind::Screen, widget, &given) {
        None => return ScreenPlan::Guest,
        Some(false) => return refused("screen_unselected"),
        Some(true) => {}
    }
    if foreign(mem, widget) {
        return refused("screen_feature");
    }
    let draw = match lay_out_screen(mem, player, widget, &given) {
        Ok(Some(draw)) => draw,
        Ok(None) => return refused("screen_empty"),
        Err(why) => {
            count("screen_refused");
            return refused(why);
        }
    };
    match arm(widget) {
        Arm::Guest => refused("screen_differed"),
        Arm::Shadow => {
            let e = ExpectScreen { widget, draw, calls: 0, matched: true };
            ACTIVE.with(|a| {
                if let Some(a) = a.borrow_mut().as_mut() {
                    a.screen = Some(e);
                }
            });
            ScreenPlan::Guest
        }
        Arm::Own => ScreenPlan::Own(draw),
    }
}

/// The screen-effect widget's call returns here (`0x0022bbbb`'s).
pub const SCREEN_CALL_RETURN: u32 = 0x0022_bbc0;
/// The widget's frame under its entry stack pointer `E` (`0x0022b8e2`:
/// `push ebp; lea ebp, [esp - 0x70]; sub esp, 0xa4; push ebx; push edi`,
/// then `push esi`): the grid at `ebp - 0x34`.
const SCREEN_GRID: u32 = 0xa8;
const SCREEN_SAVED: [u32; 3] = [0xac, 0xb0, 0xb4];
const SCREEN_CALL_STACK: u32 = 0xc8;

/// The globals the widget writes before its call (`0x0022bb20`,
/// `0x0022bb9f`, `0x0022bba4`).
fn screen_globals(draw: &ScreenDraw) -> Vec<(u32, u32)> {
    let mut writes = vec![(walk::global::BITMAP, draw.bitmap), (walk::global::IMAGE, draw.image as u32)];
    writes.extend(draw.values.iter().enumerate().map(|(i, v)| (walk::global::INPUTS + 4 * i as u32, v.to_bits())));
    writes
}

/// Whether the call of `0x0002d140` at `esp` with `eax` is `draw`'s:
/// the effect, the four values, a 3 by 3 grid of the same corners, and
/// the globals written.
fn screen_matches(mem: &dyn GuestMemory, eax: u32, esp: u32, draw: &ScreenDraw) -> bool {
    let read = || {
        let word = |n: u32| mem.u32(esp.checked_add(4 + 4 * n)?).ok();
        let (values, corners) = (word(0)?, word(3)?);
        if eax != draw.effect || [word(1)?, word(2)?] != [3, 3] {
            return Some(false);
        }
        for (i, v) in draw.values.iter().enumerate() {
            if mem.u32(values.checked_add(4 * i as u32)?).ok()? != v.to_bits() {
                return Some(false);
            }
        }
        for (i, v) in draw.grid.iter().flatten().enumerate() {
            if mem.u32(corners.checked_add(4 * i as u32)?).ok()? != v.to_bits() {
                return Some(false);
            }
        }
        Some(globals_hold(mem, &screen_globals(draw)))
    };
    read().unwrap_or(false)
}

/// The guest's call of `0x0002d140` (`eax`, `esp` at its entry), in a
/// shadowed screen-effect widget: checked against the call Rust worked out.
pub fn observe_screen(mem: &dyn GuestMemory, eax: u32, esp: u32) {
    ACTIVE.with(|a| {
        let mut a = a.borrow_mut();
        let Some(e) = a.as_mut().and_then(|a| a.screen.as_mut()) else { return };
        if mem.u32(esp).ok() != Some(SCREEN_CALL_RETURN) {
            return;
        }
        e.matched &= screen_matches(mem, eax, esp, &e.draw);
        e.calls += 1;
    });
}

/// `HUD_SCREEN_WIDGET`'s return: a shadowed widget's answer.
pub fn finish_screen() {
    let Some(e) = ACTIVE.with(|a| a.borrow_mut().as_mut().and_then(|a| a.screen.take())) else { return };
    let matched = e.matched && e.calls == 1;
    count(if matched { "screen_shadow_matched" } else { "screen_shadow_differed" });
    if proof(e.widget, |p| p.checked(matched)) {
        count("screen_proven");
    }
}

/// The registers the widget's body saves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Saved {
    pub ebp: u32,
    pub ebx: u32,
    pub edi: u32,
    pub esi: u32,
}

/// A proven screen-effect widget's frame, as its body leaves it at the
/// call of `0x0002d140`, written under the widget's entry stack pointer
/// `esp`: the saved registers, the grid, the globals, the call's four
/// words and its return (`SCREEN_CALL_RETURN`, whose code restores the
/// registers and returns from the widget). The stack and frame pointers
/// to enter `0x0002d140` with; EAX is `draw.effect`.
pub fn call_frame(mem: &mut dyn GuestMemory, esp: u32, saved: Saved, draw: &ScreenDraw) -> Option<(u32, u32)> {
    let mut put = |at: u32, word: u32| mem.write(at, &word.to_le_bytes()).ok();
    let inputs = esp.checked_add(8)?;
    let under = |n: u32| esp.checked_sub(n);
    put(under(4)?, saved.ebp)?;
    for (at, word) in SCREEN_SAVED.into_iter().zip([saved.ebx, saved.edi, saved.esi]) {
        put(under(at)?, word)?;
    }
    let grid = under(SCREEN_GRID)?;
    for (i, v) in draw.grid.iter().flatten().enumerate() {
        put(grid + 4 * i as u32, v.to_bits())?;
    }
    for (at, word) in screen_globals(draw) {
        put(at, word)?;
    }
    let top = under(SCREEN_CALL_STACK)?;
    // The values are the widget's own inputs argument (`[ebp + 0x7c]`).
    let values = mem.u32(inputs).ok()?;
    let mut put = |at: u32, word: u32| mem.write(at, &word.to_le_bytes()).ok();
    for (i, word) in [SCREEN_CALL_RETURN, values, 3, 3, grid].into_iter().enumerate() {
        put(top + 4 * i as u32, word)?;
    }
    count("screen_called");
    Some((top, under(0x74)?))
}

/// Rust drew the text widget (`drawn`), or the host did not take it.
pub fn owned_text(drawn: bool) {
    count(if drawn { "text_drawn" } else { "text_host" });
}

/// Rust drew the widget (`drawn`), or the host did not take it.
pub fn owned(drawn: bool) {
    count(if drawn { "drawn" } else { "host" });
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    #[test]
    fn a_widget_is_the_guests_until_its_draws_matched() {
        let mut p = Proof::default();
        for n in 0..PROOF_DRAWS {
            assert_eq!(p.arm(), Arm::Shadow, "draw {n}");
            assert_eq!(p.checked(true), n + 1 == PROOF_DRAWS);
        }
        assert_eq!(p.arm(), Arm::Own);
    }

    #[test]
    fn one_proven_draw_in_64_is_the_guests_and_checked_again() {
        let mut p = Proof { clean: PROOF_DRAWS, ..Proof::default() };
        let arms: Vec<Arm> = (0..2 * RECHECK).map(|_| p.arm()).collect();
        assert_eq!(arms.iter().filter(|&&a| a == Arm::Shadow).count(), 2);
        assert_eq!(arms[RECHECK as usize - 1], Arm::Shadow);
        assert!(arms.iter().all(|&a| a != Arm::Guest));
        // The recheck's clean answer proves nothing twice.
        assert!(!p.checked(true));
    }

    #[test]
    fn a_draw_that_differs_keeps_the_widget_on_the_guest() {
        let mut p = Proof { clean: PROOF_DRAWS, ..Proof::default() };
        assert!(!p.checked(false));
        assert_eq!(p.arm(), Arm::Guest);
        assert!(!p.checked(true));
        assert_eq!(p.arm(), Arm::Guest, "for the map");
    }

    const FRAME: u32 = 0x9000;

    fn laid() -> WalkWidget {
        let vertex = |x: f32, y: f32, u: f32, v: f32| [(9, [1.0; 4]), (3, [u, v, 0.0, 1.0]), (0, [x, y, walk::QUAD_DEPTH, walk::QUAD_DEPTH])];
        let quad = |x: f32, writes: Vec<(u32, u32)>| WalkQuad {
            writes,
            image: 3,
            vertices: [vertex(x, 10.0, 0.0, 0.0), vertex(x + 8.0, 10.0, 8.0, 0.0), vertex(x + 8.0, 18.0, 8.0, 8.0), vertex(x, 18.0, 0.0, 8.0)],
        };
        WalkWidget {
            anchor: 2,
            point: [48.0, 444.0],
            bitmap: 0xe100_0001,
            shader: 0xe200_0002,
            quads: vec![quad(20.0, vec![(walk::global::INPUTS, 1.0f32.to_bits())]), quad(40.0, vec![(walk::global::INPUTS, 0.5f32.to_bits())])],
        }
    }

    /// The guest's shader quad frame for quad `n` of `w`, called from the
    /// bitmap widget, with its globals written.
    fn guest_quad(m: &mut FakeGuest, w: &WalkWidget, n: usize) {
        let q = &w.quads[n];
        m.put_u32(FRAME + 4, super::super::kept::WIDGET_QUAD_RETURNS[0]);
        m.put_u32(FRAME + 8, w.bitmap).put_u32(FRAME + 12, u32::from(q.image)).put_u32(FRAME + 16, w.shader);
        for (i, v) in q.vertices.iter().enumerate() {
            let at = FRAME - super::super::HUD_QUAD_LOCAL_BYTES + i as u32 * super::super::HUD_QUAD_VERTEX_BYTES;
            let (uv, p) = (v[1].1, v[2].1);
            for (j, f) in [p[0], p[1], p[2], p[3], uv[0], uv[1]].into_iter().enumerate() {
                m.put_f32(at + 4 * j as u32, f);
            }
        }
        for &(at, word) in &q.writes {
            m.put_u32(at, word);
        }
    }

    fn shadow(e: Expect) {
        let blocks = Blocks::default();
        ACTIVE.with(|a| *a.borrow_mut() = Some(Active { blocks, shown: Vec::new(), calls: Vec::new(), foreign: false, expect: Some(e), screen: None }));
    }

    fn answer(m: &FakeGuest) -> Expect {
        let e = ACTIVE.with(|a| a.borrow().as_ref().and_then(|a| a.expect.clone())).unwrap();
        let _ = m;
        e
    }

    #[test]
    fn the_guests_quads_match_rusts_bit_for_bit() {
        let w = laid();
        let last = vec![(walk::global::COLOUR_A, 7)];
        let mut m = FakeGuest::new();
        shadow(expect(0x5000, &w, &last));
        for n in 0..2 {
            guest_quad(&mut m, &w, n);
            observe(&m, FRAME);
        }
        let e = answer(&m);
        assert!(e.matched && e.next == 2);
        // The globals written after the last quad are part of the draw.
        assert!(!globals_hold(&m, &e.globals));
        m.put_u32(walk::global::COLOUR_A, 7);
        assert!(globals_hold(&m, &e.globals));
        ACTIVE.with(|a| *a.borrow_mut() = None);
    }

    #[test]
    fn a_guest_quad_with_another_corner_image_or_input_differs() {
        let w = laid();
        for change in 0..3 {
            let mut m = FakeGuest::new();
            shadow(expect(0x5000, &w, &[]));
            guest_quad(&mut m, &w, 0);
            match change {
                0 => {
                    m.put_f32(FRAME - super::super::HUD_QUAD_LOCAL_BYTES, 20.5);
                }
                1 => {
                    m.put_u32(FRAME + 12, 4);
                }
                _ => {
                    m.put_u32(walk::global::INPUTS, 0.25f32.to_bits());
                }
            }
            observe(&m, FRAME);
            assert!(!answer(&m).matched, "change {change}");
            ACTIVE.with(|a| *a.borrow_mut() = None);
        }
    }

    #[test]
    fn a_quad_of_another_hud_item_is_not_the_widgets() {
        let w = laid();
        let mut m = FakeGuest::new();
        shadow(expect(0x5000, &w, &[]));
        guest_quad(&mut m, &w, 0);
        m.put_u32(FRAME + 4, 0x0012_3456);
        observe(&m, FRAME);
        assert_eq!(answer(&m).next, 0);
        ACTIVE.with(|a| *a.borrow_mut() = None);
    }

    #[test]
    fn a_guest_draw_with_fewer_quads_differs_at_its_end() {
        let w = laid();
        let mut m = FakeGuest::new();
        shadow(expect(0x5000, &w, &[]));
        guest_quad(&mut m, &w, 0);
        observe(&m, FRAME);
        let before = counters().into_iter().find(|(k, _)| k.ends_with("shadow_differed")).map_or(0, |(_, n)| n);
        finish(&m);
        let after = counters().into_iter().find(|(k, _)| k.ends_with("shadow_differed")).map_or(0, |(_, n)| n);
        assert_eq!(after, before + 1);
        assert!(proof(0x5000, |p| p.failed));
        ACTIVE.with(|a| *a.borrow_mut() = None);
    }

    #[test]
    fn the_guests_calls_are_checked_against_rusts_selection() {
        let mut blocks = Blocks::default();
        blocks.bitmaps = (0x3000, vec![0; 2 * walk::BITMAP_SIZE]);
        let shown = vec![Shown { kind: Kind::Bitmap, index: 1, inputs: [1.0, 0.0, 0.0, 0.5] }];
        let active = Active { blocks, shown, calls: Vec::new(), foreign: false, expect: None, screen: None };
        assert_eq!(expected_calls(&active), vec![(Kind::Bitmap, 0x3000 + walk::BITMAP_SIZE as u32, bits(&[1.0, 0.0, 0.0, 0.5]))]);
    }

    #[test]
    fn a_widget_rust_did_not_select_is_the_guests() {
        let mut m = FakeGuest::new();
        for i in 0..4 {
            m.put_f32(0x7000 + 4 * i, 1.0);
        }
        let mut blocks = Blocks::default();
        blocks.bitmaps = (0x3000, vec![0; walk::BITMAP_SIZE]);
        // Selected with other inputs than the guest gives.
        let shown = vec![Shown { kind: Kind::Bitmap, index: 0, inputs: [0.5; 4] }];
        ACTIVE.with(|a| *a.borrow_mut() = Some(Active { blocks, shown, calls: Vec::new(), foreign: false, expect: None, screen: None }));
        let fp = Fp { mxcsr: 0x1f80, control: 0x027f };
        assert_eq!(plan(&m, fp, 0, 0x6000, 0x3000, 0x7000), Plan::Guest);
        assert!(ACTIVE.with(|a| a.borrow().as_ref().unwrap().expect.is_none()));
        ACTIVE.with(|a| *a.borrow_mut() = None);
        // Outside a walk nothing is planned.
        assert_eq!(plan(&m, fp, 0, 0x6000, 0x3000, 0x7000), Plan::Guest);
    }

    /// Every glyph 8 by 10, 2 under the rectangle's top, 10 apart; its
    /// index is its character.
    struct Cells;

    impl UiText for Cells {
        fn shape(&self, _: u16, text: &[u16], rect: [f32; 4], _: Justify) -> Option<Vec<frame_scene::OverlayGlyph>> {
            let glyph = |(i, &unit): (usize, &u16)| {
                let x = rect[0] + 10.0 * i as f32;
                frame_scene::OverlayGlyph { index: u32::from(unit), rect: [x, rect[1] + 2.0, x + 8.0, rect[1] + 12.0] }
            };
            Some(text.iter().enumerate().map(glyph).collect())
        }
    }

    const TEXT: u32 = 0x3000;

    /// A two-digit number widget of `hud_input1`, ending 12 left of and 6
    /// under the frame's bottom right corner, in a full-screen view.
    fn number_widget() -> FakeGuest {
        let mut m = FakeGuest::new();
        let mut w = [0u8; walk::TEXT_SIZE];
        w[0x1c] = 3;
        w[0x1e] = (walk::TEXT_NUMBER | walk::TEXT_TWO_DIGITS) as u8;
        w[0x24..0x28].copy_from_slice(&0xe200_0009u32.to_le_bytes());
        w[0x28..0x2c].copy_from_slice(&0x0a00_05cfu32.to_le_bytes());
        w[0x2c] = 2;
        w[0x30] = 1;
        w[0x40..0x42].copy_from_slice(&(-12i16).to_le_bytes());
        w[0x42..0x44].copy_from_slice(&6i16.to_le_bytes());
        m.put(TEXT, &w);
        for (i, v) in [0u16, 0, 480, 640, 36, 48, 444, 592].into_iter().enumerate() {
            m.put_u16(hud::WINDOW + 2 * i as u32, v);
        }
        // No game engine, no quarter rule, no view user.
        m.put_u32(0x004e_6948, 0).put_u8(0x0048_5ac2, 0).put_u32(0x004b_9ed8, u32::MAX);
        // The string's colour, its shadow's and the HUD's tint.
        m.put_u32(0x0046_86cc, 0x6000).put_u32(0x0046_86d4, 0x6100).put_u32(0x0051_0c94, 0x7000).put_u32(0x70a4, 0x00ff_ffff);
        for i in 0..4 {
            m.put_f32(0x6000 + 4 * i, 1.0).put_f32(0x6100 + 4 * i, if i == 0 { 1.0 } else { 0.0 });
        }
        m
    }

    const FP: Fp = Fp { mxcsr: 0x1f80, control: 0x027f };
    /// The player's HUD record: not mapped until a test maps it.
    const RECORD: u32 = 0x000a_0000;

    #[test]
    fn a_number_widget_is_laid_out_ending_at_its_point_with_each_glyphs_shadow_first() {
        let m = number_widget();
        let t = lay_out_text(&m, FP, &Cells, RECORD, TEXT, &[7.9, 0.0, 0.0, 0.0]).unwrap().unwrap();
        assert_eq!((t.anchor, t.point, t.shader, t.font), (3, [592.0, 444.0], 0xe200_0009, 8));
        // "07": 19 wide by the measure, so it starts at 580 - 19.
        let quad = |index: char, x: f32, y: f32, colour: u32| GlyphQuad { index: index as u32, rect: [x, y, x + 8.0, y + 10.0], colour };
        assert_eq!(
            t.quads,
            vec![
                quad('0', 562.0, 453.0, 0xff00_0000),
                quad('0', 561.0, 452.0, 0xffff_ffff),
                quad('7', 572.0, 453.0, 0xff00_0000),
                quad('7', 571.0, 452.0, 0xffff_ffff),
            ]
        );
        assert_eq!((t.origin, t.writes.len()), ([0.0, 0.0], 7));
        assert_eq!(t.writes[0], (walk::global::INPUTS, 7.9f32.to_bits()));
        assert_eq!(t.writes[4], (walk::TEXT_COLOUR, 1.0f32.to_bits()));
    }

    #[test]
    fn a_name_that_is_not_read_and_a_number_past_an_int_are_the_guests() {
        let mut m = number_widget();
        assert_eq!(lay_out_text(&m, FP, &Cells, RECORD, TEXT, &[f32::INFINITY, 0.0, 0.0, 0.0]), Err("number"));
        m.put_u8(TEXT + 0x1e, walk::TEXT_NAME as u8);
        assert_eq!(lay_out_text(&m, FP, &Cells, RECORD, TEXT, &[1.0; 4]), Err("name"));
        // A colour the font renderer does not take.
        let mut m = number_widget();
        m.put_f32(0x6000, 1.5);
        assert_eq!(lay_out_text(&m, FP, &Cells, RECORD, TEXT, &[1.0; 4]), Err("colour"));
    }

    #[test]
    fn a_name_widget_draws_the_name_of_the_records_player() {
        let mut m = number_widget();
        m.put_u8(TEXT + 0x1e, walk::TEXT_NAME as u8);
        // The record names player 0x7001_0000, whose name is "Al".
        m.map(RECORD, walk::RECORD_SIZE as u32);
        m.put_u8(RECORD + 0x224, 1).put_u32(RECORD + 0x228, 0x7001_0000);
        m.put_u32(0x004e_8c24, 0x8000).put_u32(0x8000 + 0x24, 0x21c).put_u32(0x8000 + 0x38, 1).put_u32(0x8000 + 0x44, 0x9000);
        m.map(0x9000, 0x21c);
        m.put_u16(0x9000, 0x7001).put_u16(0x9000 + 0x44, u16::from(b'A')).put_u16(0x9000 + 0x46, u16::from(b'l'));
        let t = lay_out_text(&m, FP, &Cells, RECORD, TEXT, &[1.0; 4]).unwrap().unwrap();
        // Each glyph's shadow, then the glyph.
        assert_eq!(t.quads.iter().map(|q| q.index).collect::<Vec<u32>>(), b"AAll".map(u32::from));
        // No player named: the empty string, which draws nothing.
        m.put_u8(RECORD + 0x224, 0);
        assert_eq!(lay_out_text(&m, FP, &Cells, RECORD, TEXT, &[1.0; 4]), Ok(None));
    }

    #[test]
    fn a_selected_text_widget_is_rusts_and_one_not_selected_or_with_no_fonts_the_guests() {
        let mut m = number_widget();
        for i in 0..4 {
            m.put_f32(0x7800 + 4 * i, 1.0);
        }
        let mut blocks = Blocks::default();
        blocks.texts = (TEXT, vec![0; walk::TEXT_SIZE]);
        let shown = vec![Shown { kind: Kind::Text, index: 0, inputs: [1.0; 4] }];
        let active = || Active { blocks: blocks.clone(), shown: shown.clone(), calls: Vec::new(), foreign: false, expect: None, screen: None };
        ACTIVE.with(|a| *a.borrow_mut() = Some(active()));
        assert_eq!(plan_text(&m, FP, None, RECORD, TEXT, 0x7800), TextPlan::Guest);
        // Selected, with fonts: Rust's, with no proof.
        match plan_text(&m, FP, Some(&Cells), RECORD, TEXT, 0x7800) {
            TextPlan::Own(text) => assert_eq!(text.quads.len(), 4),
            TextPlan::Guest => panic!("a selected text widget is Rust's"),
        }
        // Other inputs than Rust's walk gave: not laid out.
        ACTIVE.with(|a| *a.borrow_mut() = Some(active()));
        m.put_f32(0x7800, 0.5);
        assert_eq!(plan_text(&m, FP, Some(&Cells), RECORD, TEXT, 0x7800), TextPlan::Guest);
        ACTIVE.with(|a| *a.borrow_mut() = None);
    }

    fn screen_draw() -> ScreenDraw {
        let grid = std::array::from_fn(|i| [100.0 * (i % 3) as f32, 50.0 * (i / 3) as f32, 0.25 * (i % 3) as f32, 1.0]);
        ScreenDraw { effect: 0xe300_0004, bitmap: 0xe100_0001, image: 2, values: [1.0, 0.5, 0.0, 0.0], grid }
    }

    #[test]
    fn a_proven_screen_widgets_frame_is_the_call_the_guests_body_makes() {
        let mut m = FakeGuest::new();
        let e = 0x000d_0000;
        m.put(e - 0x100, &[0u8; 0x110]);
        m.put_u32(walk::global::BITMAP, 0).put_u32(walk::global::INPUTS + 12, 0);
        // The widget's arguments: the player, then its inputs.
        m.put_u32(e, 0x0013_a5f0).put_u32(e + 4, 0).put_u32(e + 8, 0x7800);
        let draw = screen_draw();
        for (i, v) in draw.values.iter().enumerate() {
            m.put_f32(0x7800 + 4 * i as u32, *v);
        }
        let saved = Saved { ebp: 0x000d_0040, ebx: 11, edi: 22, esi: 33 };
        let (esp, ebp) = call_frame(&mut m, e, saved, &draw).unwrap();
        assert_eq!((esp, ebp), (e - 0xc8, e - 0x74));
        // The call as the guest's body pushes it, returning into its tail.
        assert_eq!(m.u32(esp).unwrap(), SCREEN_CALL_RETURN);
        assert_eq!([m.u32(esp + 4).unwrap(), m.u32(esp + 8).unwrap(), m.u32(esp + 12).unwrap()], [0x7800, 3, 3]);
        assert_eq!(m.u32(esp + 16).unwrap(), ebp - 0x34);
        assert!(screen_matches(&m, draw.effect, esp, &draw));
        // The tail pops ESI, EDI and EBX above the call's words, then
        // `add ebp, 0x70; leave` finds the caller's EBP.
        assert_eq!([m.u32(esp + 0x14).unwrap(), m.u32(esp + 0x18).unwrap(), m.u32(esp + 0x1c).unwrap()], [33, 22, 11]);
        assert_eq!(m.u32(ebp + 0x70).unwrap(), 0x000d_0040);
        assert_eq!(m.u32(walk::global::IMAGE).unwrap(), 2);
    }

    #[test]
    fn a_guest_call_with_another_effect_corner_or_bitmap_differs() {
        let draw = screen_draw();
        for change in 0..4 {
            let mut m = FakeGuest::new();
            let e = 0x000d_0000;
            m.put(e - 0x100, &[0u8; 0x110]);
        m.put_u32(walk::global::BITMAP, 0).put_u32(walk::global::INPUTS + 12, 0);
            m.put_u32(e + 8, 0x7800);
            for (i, v) in draw.values.iter().enumerate() {
                m.put_f32(0x7800 + 4 * i as u32, *v);
            }
            let (esp, ebp) = call_frame(&mut m, e, Saved { ebp: 0, ebx: 0, edi: 0, esi: 0 }, &draw).unwrap();
            let mut eax = draw.effect;
            match change {
                0 => eax = 0xe300_0005,
                1 => {
                    m.put_f32(ebp - 0x34 + 16, 101.0);
                }
                2 => {
                    m.put_u32(walk::global::BITMAP, 0xe100_0002);
                }
                _ => {
                    m.put_u32(esp + 8, 4);
                }
            }
            ACTIVE.with(|a| {
                let e = ExpectScreen { widget: 0x5200, draw: draw.clone(), calls: 0, matched: true };
                *a.borrow_mut() = Some(Active { blocks: Blocks::default(), shown: Vec::new(), calls: Vec::new(), foreign: false, expect: None, screen: Some(e) });
            });
            observe_screen(&m, eax, esp);
            let e = ACTIVE.with(|a| a.borrow().as_ref().and_then(|a| a.screen.clone())).unwrap();
            assert!(e.calls == 1 && !e.matched, "change {change}");
            ACTIVE.with(|a| *a.borrow_mut() = None);
        }
    }

    /// A walk of `definition` showing `shown`, as [`enter`] leaves it.
    fn walking(definition: u32, blocks: &Blocks, shown: &[Shown]) {
        let active = Active { blocks: blocks.clone(), shown: shown.to_vec(), calls: Vec::new(), foreign: false, expect: None, screen: None };
        ACTIVE.with(|a| *a.borrow_mut() = Some(active));
        LOOP.with(|l| *l.borrow_mut() = Loop { definition, ..Loop::default() });
    }

    /// The guest's draw of a widget, as [`called`] keeps it.
    fn guest_call(call: (Kind, u32, [u32; 4])) {
        ACTIVE.with(|a| a.borrow_mut().as_mut().unwrap().calls.push(call));
    }

    #[test]
    fn the_loop_is_rusts_once_the_selection_and_every_shown_widget_are_proven() {
        let (definition, widget) = (0xe300_0101, 0x0031_0000);
        let mut blocks = Blocks::default();
        blocks.bitmaps = (widget, vec![0; walk::BITMAP_SIZE]);
        let inputs = [1.0, 0.0, 0.0, 0.5];
        let shown = [Shown { kind: Kind::Bitmap, index: 0, inputs }];
        let call = (Kind::Bitmap, widget, bits(&inputs));
        // The guest loops while its calls are checked against Rust's.
        for n in 0..PROOF_DRAWS {
            walking(definition, &blocks, &shown);
            assert_eq!(plan_loop(), LoopPlan::Guest, "walk {n}");
            guest_call(call);
            leave();
        }
        // The selection is proven, the widget is not: the guest loops.
        walking(definition, &blocks, &shown);
        assert_eq!(plan_loop(), LoopPlan::Guest);
        guest_call(call);
        leave();
        proof(widget, |p| p.clean = PROOF_DRAWS);
        walking(definition, &blocks, &shown);
        assert_eq!(plan_loop(), LoopPlan::Own(vec![(Kind::Bitmap, widget, inputs)]));
        looped();
        assert!(ACTIVE.with(|a| a.borrow().is_none()));
    }

    #[test]
    fn a_walk_whose_calls_differ_keeps_the_loop_on_the_guest() {
        let (definition, widget) = (0xe300_0102, 0x0032_0000);
        walk_proof(definition, |p| p.clean = PROOF_DRAWS);
        proof(widget, |p| p.clean = PROOF_DRAWS);
        let mut blocks = Blocks::default();
        blocks.texts = (widget, vec![0; walk::TEXT_SIZE]);
        let shown = [Shown { kind: Kind::Text, index: 0, inputs: [1.0; 4] }];
        walking(definition, &blocks, &shown);
        assert!(matches!(plan_loop(), LoopPlan::Own(_)));
        // Rust gave the loop back and the guest drew nothing.
        gave_back();
        leave();
        walking(definition, &blocks, &shown);
        assert_eq!(plan_loop(), LoopPlan::Guest);
        leave();
    }

    #[test]
    fn one_proven_walk_in_64_is_the_guests_with_each_widget_checked_again() {
        let (definition, widget) = (0xe300_0103, 0x0033_0000);
        walk_proof(definition, |p| *p = Proof { clean: PROOF_DRAWS, failed: false, seen: RECHECK - 2 });
        proof(widget, |p| p.clean = PROOF_DRAWS);
        let blocks = Blocks::default();
        walking(definition, &blocks, &[]);
        assert_eq!(plan_loop(), LoopPlan::Own(Vec::new()));
        assert_eq!(arm(widget), Arm::Own);
        walking(definition, &blocks, &[]);
        assert_eq!(plan_loop(), LoopPlan::Guest);
        assert_eq!(arm(widget), Arm::Shadow);
        leave();
        assert_eq!(arm(widget), Arm::Own);
    }

    #[test]
    fn a_walk_showing_a_screen_effect_or_a_features_widget_is_the_guests_loop() {
        let (definition, widget) = (0xe300_0104, 0x0034_0000);
        walk_proof(definition, |p| p.clean = PROOF_DRAWS);
        proof(widget, |p| p.clean = PROOF_DRAWS);
        let mut blocks = Blocks::default();
        blocks.screens = (widget, vec![0; walk::SCREEN_SIZE]);
        walking(definition, &blocks, &[Shown { kind: Kind::Screen, index: 0, inputs: [1.0; 4] }]);
        assert_eq!(plan_loop(), LoopPlan::Guest);
        // With nothing shown the loop is Rust's, unless a feature has a widget in the definition.
        walking(definition, &blocks, &[]);
        assert_eq!(plan_loop(), LoopPlan::Own(Vec::new()));
        ACTIVE.with(|a| a.borrow_mut().as_mut().unwrap().foreign = true);
        assert_eq!(plan_loop(), LoopPlan::Guest);
        looped();
    }

    #[test]
    fn widgets_rust_drew_before_it_gave_the_loop_back_are_left_out_of_the_guests_walk() {
        LOOP.with(|l| *l.borrow_mut() = Loop::default());
        drew(Kind::Bitmap, 0x3500);
        assert!(done(Kind::Bitmap, 0x3500));
        assert!(!done(Kind::Text, 0x3500) && !done(Kind::Bitmap, 0x3564));
        // The next walk starts with none.
        leave();
        assert!(!done(Kind::Bitmap, 0x3500));
    }

    #[test]
    fn a_feature_names_its_widgets_once() {
        fn kit(_: &dyn GuestMemory, widget: u32) -> bool {
            widget == 0x00ab_0000
        }
        add_foreign(kit);
        add_foreign(kit);
        let m = FakeGuest::new();
        assert!(foreign(&m, 0x00ab_0000));
        assert!(!foreign(&m, 0x00ab_0064));
        assert_eq!(FOREIGN.lock().unwrap().iter().filter(|&&f| std::ptr::fn_addr_eq(f, kit as Foreign)).count(), 1);
    }
}
