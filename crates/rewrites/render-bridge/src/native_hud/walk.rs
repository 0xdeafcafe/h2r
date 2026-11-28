//! The HUD walk in Rust (docs/graphics/native-hud-overlay.md, "The walk
//! in Rust"): at `HUD_WALK` (and at each walk of `HUD_DEFINITIONS`) the
//! widgets a definition shows and their inputs are worked out from the
//! definition and the player's HUD record (`hud_kit_core::walk`), each
//! widget is laid out in Rust and the host draws it. The guest's walk
//! never runs.
//!
//! A bitmap widget is laid out as the guest's draw (`HUD_BITMAP_WIDGET`
//! would): its sprite, its anchor, its effects (`engine::hud_effects`),
//! its quads and the globals it writes. A text widget is laid out as
//! `HUD_TEXT_WIDGET` would: its string (the table's, or its number), its
//! rectangle from the measure, its glyphs through the host's fonts
//! ([`UiText`]) and each glyph quad's colour.
//!
//! A feature that changes what a widget shows answers at the override
//! seam ([`super::seam`]): the gate, the inputs, a text's string, a
//! bitmap's sprite and its basis' scale.
//!
//! What Rust does not lay out is a named panic: a definition or record
//! that does not read, an emblem, an anchor past the centre, an effect
//! input that is not read, vertices outside the capture's check, a text
//! with no host fonts, a screen-effect widget, a host that does not draw.

use std::collections::BTreeMap;
use std::sync::Mutex;

use engine::hud_effects;
use guest_layout::memory::GuestMemory;
use scene_extract::hud::{self, GuestBitmap, walk};
use scene_extract::ui::{Justify, UiText};
pub use walk::GlyphQuad;
use walk::{Bitmap as _, Kind, Refusal};

use super::seam::{self, At};
use super::{HudVertices, hud_inputs};

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

static COUNTS: Mutex<BTreeMap<&'static str, u64>> = Mutex::new(BTreeMap::new());

pub(super) fn count(what: &'static str) {
    *COUNTS.lock().unwrap_or_else(|e| e.into_inner()).entry(what).or_default() += 1;
}

/// `native_ui_overlay_hud_walk_<what>` for everything the walk counted.
pub fn counters() -> Vec<(String, u64)> {
    let counts = COUNTS.lock().unwrap_or_else(|e| e.into_inner());
    counts.iter().map(|(what, &n)| (format!("native_ui_overlay_hud_walk_{what}"), n)).collect()
}


/// One walk of a definition (`HUD_WALK`): the widgets it shows, in the
/// guest's order, with the context the seam's questions are asked in.
#[derive(Clone, Debug)]
pub struct Walk {
    pub definition: u32,
    pub player: u32,
    pub record: u32,
    /// The widgets shown: kind, address and inputs.
    pub shown: Vec<(Kind, u32, [f32; 4])>,
    /// The walk's weapon record and its state word D.
    weapon: Option<u32>,
    word_d: u16,
}

impl Walk {
    /// The seam's view of `widget` in this walk.
    pub fn at(&self, widget: u32) -> At {
        At { widget, player: self.player, weapon: self.weapon, word_d: self.word_d }
    }
}

/// `HUD_WALK`'s entry: the widgets definition `definition` shows for
/// `player` with HUD record `record` and `slot` (`hud_kit_core::walk`),
/// the seam's gate and inputs in place of the walk's own where a feature
/// answers. A definition or record that does not read panics.
pub fn enter(mem: &dyn GuestMemory, definition: u32, player: u32, record_at: u32, slot: u32) -> Walk {
    let read = || {
        let blocks = hud::blocks(mem, definition)?;
        let record = hud::record(mem, record_at)?;
        let session = hud::session(mem, player)?;
        let words = walk::state_words(&record, slot, &session)?;
        Some((blocks, record, session, words))
    };
    let Some((blocks, record, session, words)) = read() else {
        unimplemented!("hud walk: definition {definition:#x} for record {record_at:#x} slot {slot} did not read");
    };
    let weapon = words.weapon.map(|offset| record_at.wrapping_add(offset as u32));
    let mut shown = Vec::new();
    for kind in [Kind::Screen, Kind::Bitmap, Kind::Text] {
        let block = match kind {
            Kind::Screen => &blocks.screens.1,
            Kind::Bitmap => &blocks.bitmaps.1,
            Kind::Text => &blocks.texts.1,
        };
        for (index, w) in block.chunks_exact(kind.size()).enumerate() {
            let address = blocks.address(kind, index);
            let at = At { widget: address, player, weapon, word_d: words.d };
            // The seam's gate and inputs are the bitmap and text calls' (`0x0013a602`, `0x0013a659`).
            let seamed = kind != Kind::Screen;
            let gate = seamed.then(|| seam::gate(mem, &at)).flatten();
            if !gate.unwrap_or_else(|| walk::gate(&w[8..0x1c], &words, &record)) {
                continue;
            }
            let inputs = seamed.then(|| seam::inputs(mem, &at)).flatten();
            let inputs = inputs.unwrap_or_else(|| walk::inputs(&w[4..8], &record, words.weapon, &session));
            shown.push((kind, address, inputs));
        }
    }
    Walk { definition, player, record: record_at, shown, weapon, word_d: words.d }
}

/// A bitmap widget of a walk laid out as at its draw. `None`: it draws
/// nothing (no bitmap, shader or image, or its texture is not there, as
/// the guest's quad would draw none). Anything else Rust does not lay out
/// panics.
pub fn bitmap(mem: &dyn GuestMemory, fp: Fp, walk: &Walk, widget: u32, inputs: &[f32; 4]) -> Option<(WalkWidget, Vec<(u32, u32)>)> {
    match lay_out(mem, fp, walk.player, walk.record, widget, inputs, &walk.at(widget)) {
        Ok(Some(laid)) => Some(laid),
        Ok(None) => {
            count("empty");
            None
        }
        // The guest draws a quad only with its texture there (`0x0003bcb0`).
        Err("texture") => {
            count("texture");
            None
        }
        Err(why) => unimplemented!("hud walk: bitmap widget {widget:#x} is not laid out ({why})"),
    }
}

/// A text widget of a walk, as [`bitmap`]. `fonts` lays the string out;
/// without them the walk panics.
pub fn text(mem: &dyn GuestMemory, fp: Fp, fonts: Option<&dyn UiText>, walk: &Walk, widget: u32, inputs: &[f32; 4]) -> Option<WalkText> {
    let Some(fonts) = fonts else { unimplemented!("hud walk: text widget {widget:#x} has no host fonts") };
    match lay_out_text(mem, fp, fonts, walk.record, widget, inputs, &walk.at(widget)) {
        Ok(Some(laid)) => Some(laid),
        Ok(None) => {
            count("text_empty");
            None
        }
        Err(why) => unimplemented!("hud walk: text widget {widget:#x} is not laid out ({why})"),
    }
}

/// Rust drew every widget of a walk.
pub fn looped() {
    count("loop_owned");
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
    at: &At,
) -> Result<Option<(WalkWidget, Vec<(u32, u32)>)>, &'static str> {
    let w = mem.bytes(widget, walk::BITMAP_SIZE).map_err(|_| "read")?;
    let tag = u32::from_le_bytes([w[0x24], w[0x25], w[0x26], w[0x27]]);
    if tag == u32::MAX {
        return Ok(None);
    }
    let record = hud::record(mem, record).ok_or("read")?;
    let screen = hud::screen(mem).ok_or("read")?;
    let aim = hud::aim(mem).ok_or("read")?;
    // A feature's sprite replaces the bitmap's own pick (`seam::sprite`).
    let art = walk::Art { sprite: seam::sprite(mem, at), ..hud::art(mem, player).ok_or("read")? };
    let bitmap = GuestBitmap { mem, data: hud::tag_data(mem, tag).ok_or("read")? };
    // A feature's scale is on the basis before the widget's own effects.
    let scale = seam::scale(mem, at);
    let effects = |block: (u32, u32), inputs: &[f32; 4], basis: &mut walk::Basis| {
        if let Some([x, y]) = scale {
            basis.x[0] *= x;
            basis.y[1] *= y;
        }
        effects(mem, fp, block, inputs, basis)
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

/// The measure's rectangle (`0x0022abc9`): 0, 0 to 1000, 1000.
const MEASURE: [f32; 4] = [0.0, 0.0, 1000.0, 1000.0];

/// The text widget at `widget` laid out in Rust with `inputs`.
/// `Ok(None)`: it draws nothing. `Err`: why Rust does not lay it out.
fn lay_out_text(mem: &dyn GuestMemory, fp: Fp, fonts: &dyn UiText, record: u32, widget: u32, inputs: &[f32; 4], at: &At) -> Result<Option<WalkText>, &'static str> {
    let w = mem.bytes(widget, walk::TEXT_SIZE).map_err(|_| "read")?;
    let screen = hud::screen(mem).ok_or("read")?;
    let aim = hud::aim(mem).ok_or("read")?;
    let run = |block: (u32, u32), inputs: &[f32; 4], basis: &mut walk::Basis| effects(mem, fp, block, inputs, basis);
    let Some(place) = walk::text_widget(&w, inputs, &screen, aim, &run).map_err(Refusal::cause)? else { return Ok(None) };
    // A feature's string and font replace the table's (`seam::string`).
    let given = seam::string(mem, at, inputs);
    let font = given.as_ref().map(|t| t.font).or(place.font).and_then(|f| u16::try_from(f).ok()).ok_or("font")?;
    let units: Vec<u16> = if let Some(text) = given {
        text.units
    } else if place.flags & walk::TEXT_NUMBER != 0 {
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
        let t = lay_out_text(&m, FP, &Cells, RECORD, TEXT, &[7.9, 0.0, 0.0, 0.0], &at(TEXT)).unwrap().unwrap();
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
    fn a_name_that_is_not_read_and_a_number_past_an_int_are_refused() {
        let mut m = number_widget();
        assert_eq!(lay_out_text(&m, FP, &Cells, RECORD, TEXT, &[f32::INFINITY, 0.0, 0.0, 0.0], &at(TEXT)), Err("number"));
        m.put_u8(TEXT + 0x1e, walk::TEXT_NAME as u8);
        assert_eq!(lay_out_text(&m, FP, &Cells, RECORD, TEXT, &[1.0; 4], &at(TEXT)), Err("name"));
        // A colour the font renderer does not take.
        let mut m = number_widget();
        m.put_f32(0x6000, 1.5);
        assert_eq!(lay_out_text(&m, FP, &Cells, RECORD, TEXT, &[1.0; 4], &at(TEXT)), Err("colour"));
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
        let t = lay_out_text(&m, FP, &Cells, RECORD, TEXT, &[1.0; 4], &at(TEXT)).unwrap().unwrap();
        // Each glyph's shadow, then the glyph.
        assert_eq!(t.quads.iter().map(|q| q.index).collect::<Vec<u32>>(), b"AAll".map(u32::from));
        // No player named: the empty string, which draws nothing.
        m.put_u8(RECORD + 0x224, 0);
        assert_eq!(lay_out_text(&m, FP, &Cells, RECORD, TEXT, &[1.0; 4], &at(TEXT)), Ok(None));
    }


    fn at(widget: u32) -> At {
        At { widget, player: 0, weapon: None, word_d: 0 }
    }

    /// A seam that gives the text widget at `SEAMED` its own string and font.
    struct Says;

    const SEAMED: u32 = 0x3100;

    impl seam::Seam for Says {
        fn string(&self, _: &dyn GuestMemory, at: &At, inputs: &[f32; 4]) -> Result<Option<seam::Text>, guest_layout::memory::MemFault> {
            Ok((at.widget == SEAMED).then(|| seam::Text { units: vec![u16::from(b'Z'), inputs[0] as u16], font: 6 }))
        }
    }

    #[test]
    fn a_seams_string_and_font_replace_the_widgets_own() {
        let mut m = number_widget();
        let w = m.bytes(TEXT, walk::TEXT_SIZE).unwrap().to_vec();
        m.put(SEAMED, &w);
        seam::register(9, "walk-test-says", std::sync::Arc::new(Says));
        let t = lay_out_text(&m, FP, &Cells, RECORD, SEAMED, &[65.0, 0.0, 0.0, 0.0], &at(SEAMED)).unwrap().unwrap();
        // The seam's font, and "Z" then "A" (65), each glyph's shadow first.
        assert_eq!(t.font, 6);
        assert_eq!(t.quads.iter().map(|q| q.index).collect::<Vec<u32>>(), [90, 90, 65, 65]);
        // The widget at `TEXT` is still the walk's own.
        let own = lay_out_text(&m, FP, &Cells, RECORD, TEXT, &[7.9, 0.0, 0.0, 0.0], &at(TEXT)).unwrap().unwrap();
        assert_eq!(own.font, 8);
    }
}
