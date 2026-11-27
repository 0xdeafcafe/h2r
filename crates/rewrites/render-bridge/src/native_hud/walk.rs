//! The HUD walk in Rust (docs/graphics/native-hud-overlay.md, "The walk
//! in Rust"): at the guest's walk of a definition (`HUD_WALK`) the
//! widgets it shows and their inputs are worked out from the definition
//! and the player's HUD record (`hud_kit_core::walk`), and a bitmap
//! widget Rust selected with the inputs the guest gives it is laid out
//! in Rust at its draw (`HUD_BITMAP_WIDGET`): its sprite, its anchor, its
//! effects (`engine::hud_effects`), its quads and the globals it writes.
//!
//! A widget is the guest's until [`PROOF_DRAWS`] of its draws matched
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
use engine::memory::GuestMemory;
use scene_extract::hud::{self, Blocks, GuestBitmap, walk};
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
struct Proof {
    clean: u32,
    failed: bool,
    /// Draws since it was proven, for the recheck.
    seen: u32,
}

impl Proof {
    fn arm(&mut self) -> Arm {
        if self.failed {
            return Arm::Guest;
        }
        if self.clean < PROOF_DRAWS {
            return Arm::Shadow;
        }
        self.seen = self.seen.wrapping_add(1);
        if self.seen % RECHECK == 0 { Arm::Shadow } else { Arm::Own }
    }

    /// A shadowed draw's answer. True when this one proved the widget.
    fn checked(&mut self, matched: bool) -> bool {
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
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(Mutex::default);

fn proof<T>(widget: u32, f: impl FnOnce(&mut Proof) -> T) -> T {
    let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let generation = crate::native_map_work::generation();
    if s.generation != Some(generation) {
        *s = State { generation: Some(generation), proofs: HashMap::new() };
    }
    f(s.proofs.entry(widget).or_default())
}

static COUNTS: Mutex<BTreeMap<&'static str, u64>> = Mutex::new(BTreeMap::new());

fn count(what: &'static str) {
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
        Some(Active { blocks, shown, calls: Vec::new(), foreign: any_foreign, expect: None })
    };
    let active = read();
    if active.is_none() {
        count("read");
    }
    ACTIVE.with(|a| *a.borrow_mut() = active);
}

/// The walk's selection as the guest's calls would be.
fn expected_calls(active: &Active) -> Vec<(Kind, u32, [u32; 4])> {
    active.shown.iter().map(|s| (s.kind, active.blocks.address(s.kind, s.index), bits(&s.inputs))).collect()
}

/// `HUD_WALK`'s return: the guest's widget draws against Rust's selection.
pub fn leave() {
    let Some(active) = ACTIVE.with(|a| a.borrow_mut().take()) else { return };
    if active.foreign {
        count("selection_foreign");
    } else if expected_calls(&active) == active.calls {
        count("selection_matched");
    } else {
        count("selection_differed");
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
    let effects = |block: (u32, u32), inputs: &[f32; 4], basis: &mut walk::Basis| {
        // The widget's inputs are the shader inputs by now; the rest are
        // the callback's (`hud_inputs`).
        let input = |name: u32| match name {
            0x0a00_05cf..=0x0a00_05d2 => Some(inputs[(name - 0x0a00_05cf) as usize]),
            _ => hud_inputs::function_input(mem, name),
        };
        let mut b = hud_effects::Basis {
            origin: basis.origin.map(f32::to_bits),
            x: basis.x.map(f32::to_bits),
            y: basis.y.map(f32::to_bits),
        };
        match hud_effects::apply(mem, fp.mxcsr, fp.control, block, &input, &mut b) {
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
    };
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
    // Rust draws only what its own walk shows, with the inputs it gave.
    let selected = ACTIVE.with(|a| {
        a.borrow().as_ref().map(|a| {
            a.shown.iter().any(|s| {
                s.kind == Kind::Bitmap && a.blocks.address(Kind::Bitmap, s.index) == widget && bits(&s.inputs) == bits(&given)
            })
        })
    });
    match selected {
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
    match proof(widget, Proof::arm) {
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

/// Rust drew the widget (`drawn`), or the host did not take it.
pub fn owned(drawn: bool) {
    count(if drawn { "drawn" } else { "host" });
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::testing::FakeGuest;

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
        ACTIVE.with(|a| *a.borrow_mut() = Some(Active { blocks, shown: Vec::new(), calls: Vec::new(), foreign: false, expect: Some(e) }));
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
        assert_eq!(proof(0x5000, |p| p.failed), true);
        ACTIVE.with(|a| *a.borrow_mut() = None);
    }

    #[test]
    fn the_guests_calls_are_checked_against_rusts_selection() {
        let mut blocks = Blocks::default();
        blocks.bitmaps = (0x3000, vec![0; 2 * walk::BITMAP_SIZE]);
        let shown = vec![Shown { kind: Kind::Bitmap, index: 1, inputs: [1.0, 0.0, 0.0, 0.5] }];
        let active = Active { blocks, shown, calls: Vec::new(), foreign: false, expect: None };
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
        ACTIVE.with(|a| *a.borrow_mut() = Some(Active { blocks, shown, calls: Vec::new(), foreign: false, expect: None }));
        let fp = Fp { mxcsr: 0x1f80, control: 0x027f };
        assert_eq!(plan(&m, fp, 0, 0x6000, 0x3000, 0x7000), Plan::Guest);
        assert!(ACTIVE.with(|a| a.borrow().as_ref().unwrap().expect.is_none()));
        ACTIVE.with(|a| *a.borrow_mut() = None);
        // Outside a walk nothing is planned.
        assert_eq!(plan(&m, fp, 0, 0x6000, 0x3000, 0x7000), Plan::Guest);
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
