//! The string draw `0x0001f070` in Rust (docs/graphics/native-hud-overlay.md,
//! "Strings"). At its entry (`STRING_DRAW`) the string is laid out from
//! the font renderer's state (`0x004e73a0`..) and the call's rectangle
//! through the host's fonts ([`UiText`]): each glyph's quad, its shadow's,
//! and their colours, as the glyph callback `0x0001f490` would push them
//! to the screen quad `0x000363a0`.
//!
//! A string is the guest's until [`PROOF_DRAWS`](super::walk::PROOF_DRAWS) draws from its call site
//! (the return into `0x0001fa50`'s caller) matched Rust's quads at the
//! screen quad: corners and packed colour bit for bit, and each quad's
//! texel extent the size of its glyph, in order and as many. After that
//! the host draws the site's strings as text runs over its glyph pages
//! ([`super::Host::rust_string`]) and the string draw is left out; one
//! draw in [`RECHECK`](super::walk::RECHECK) is still the guest's and is checked again. A draw
//! that differs keeps the site on the guest for the map.
//!
//! What the layout does not read is refused by name and counted
//! (`native_ui_overlay_string_<cause>`): the guest draws it.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::sync::{LazyLock, Mutex};

use engine::memory::GuestMemory;
use frame_scene::OverlayBlend;
use scene_extract::hud::{self, walk};
use scene_extract::ui::{Justify, UiText};

pub use walk::GlyphQuad;

use super::walk::{Arm, Proof};

/// The string draw's return into `0x0001fa50`, which turns a string's
/// UTF-16 into the units the draw takes, and into the HUD's `0x0001fb10`.
pub const FROM_PLAIN: u32 = 0x0001_faf7;
pub const FROM_HUD: u32 = 0x0001_fb9a;
/// From the draw's entry ESP to `0x0001fa50`'s own return: the call's
/// return and seven arguments, the four registers it saved and its
/// `0x2004` bytes of units.
const PLAIN_RETURN_FROM_ESP: u32 = 4 + 0x1c + 0x10 + 0x2004;
/// The draw's arguments: `ret 0x1c`.
pub const ARGUMENT_BYTES: u32 = 0x1c;

/// The draw's early returns (`0x0001f07f`..`0x0001f0c4`): text off, the
/// screen quads off (`native_render::hud::QUADS_OFF`), and the glyph
/// cache not ready or not made.
const TEXT_OFF: u32 = 0x0051_080c;
const QUADS_OFF: u32 = 0x0048_5602;
const CACHE_READY: u32 = 0x004b_62a0;
const GLYPH_BITMAP: u32 = 0x004b_62ac;
/// The word `0x0001f0a2` adds one to on every draw past the first two
/// returns: a string left out adds it too.
pub const DRAW_COUNT: u32 = 0x0046_70c0;

/// The font renderer's state the layout reads (`0x0013f0e0`, the scanner
/// set up by `0x0013f470`): the engine font (`0x0013ed9c`), the wrap bit,
/// a character limit (`-1`: none, `0x001616c6`), the justification (the
/// scanner's `+0x10`: 0 left, 1 right, 2 centre), the colour and the
/// shadow's (alpha first, `0x0013ec70`, `0x0013ed50`), the tab stops'
/// count and the two indents.
const FONT: u32 = 0x004e_73a0;
const WRAP: u32 = 0x004e_73a4;
const LIMIT: u32 = 0x004e_73a8;
const JUSTIFY: u32 = 0x004e_73ac;
const COLOUR: u32 = 0x004e_73b0;
const SHADOW_ON: u32 = 0x004e_73c0;
const SHADOW: u32 = 0x004e_73c4;
const TABS: u32 = 0x004e_73dc;
const INDENTS: [u32; 2] = [0x004e_73fe, 0x004e_7400];

/// `0x000142f0`'s tables at blend mode 0, the mode the draw's record
/// keeps (`0x0001f1dd` clears it): source, destination and equation.
const BLEND_SOURCE: u32 = 0x0045_0718;
const BLEND_DEST: u32 = 0x0045_06e8;
const BLEND_EQUATION: u32 = 0x0045_06b8;

/// `0x0001fa50`'s units: at most `0x800`, the last 0.
const MAX_UNITS: u32 = 0x800;
/// How far a string that does not wrap is laid out past its rectangle,
/// to find whether it would break (a line breaks after the glyph that
/// does not fit, wrap or not: docs/formats/ui.md, "Text layout").
const WIDE: f32 = 16384.0;

/// One call of the string draw, as its entry reads it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StringCall {
    /// Where the string is drawn from: `0x0001fa50`'s return, or the
    /// draw's own for another caller.
    pub site: u32,
    /// EAX: a clip rectangle (top, left, bottom, right), or 0 for the
    /// window's.
    pub clip: u32,
    /// ECX: the rectangle, or 0 for the frame less the window's corner.
    pub rect: u32,
    /// Where the layout writes its pen at the end, or 0.
    pub end: u32,
    /// Added to each line's height.
    pub leading: u32,
    /// The units, 32 bits each, the last 0.
    pub units: u32,
    pub scale: f32,
    /// The batch record's `+0x58` and `+0x5c`.
    pub extra: [u32; 2],
    /// The batch keeps the caller's shader (the HUD's text widget).
    pub keep: u8,
}

impl StringCall {
    /// The call at the draw's entry, `esp` its return slot.
    pub fn read(mem: &dyn GuestMemory, esp: u32, eax: u32, ecx: u32) -> Option<StringCall> {
        let arg = |n: u32| mem.u32(esp.checked_add(4 + 4 * n)?).ok();
        let ret = mem.u32(esp).ok()?;
        let site = if ret == FROM_PLAIN { mem.u32(esp.checked_add(PLAIN_RETURN_FROM_ESP)?).ok()? } else { ret };
        Some(StringCall {
            site,
            clip: eax,
            rect: ecx,
            end: arg(0)?,
            leading: arg(1)?,
            units: arg(2)?,
            scale: f32::from_bits(arg(3)?),
            extra: [arg(4)?, arg(5)?],
            keep: arg(6)? as u8,
        })
    }
}

/// A string laid out in Rust.
#[derive(Clone, Debug, PartialEq)]
pub struct StringRun {
    /// The engine font.
    pub font: u16,
    /// The glyph quads in the guest's order, in the window's pixels.
    pub quads: Vec<GlyphQuad>,
    /// The window's left and top in the frame: the quads' origin.
    pub origin: [f32; 2],
    /// The batch's blend.
    pub blend: OverlayBlend,
}

fn four(mem: &dyn GuestMemory, at: u32) -> Option<[f32; 4]> {
    let mut v = [0.0; 4];
    for (i, value) in v.iter_mut().enumerate() {
        *value = f32::from_bits(mem.u32(at.checked_add(4 * i as u32)?).ok()?);
    }
    Some(v)
}

fn shorts(mem: &dyn GuestMemory, at: u32) -> Option<[i16; 4]> {
    let mut out = [0; 4];
    for (i, v) in out.iter_mut().enumerate() {
        *v = mem.u16(at.checked_add(2 * i as u32)?).ok()? as i16;
    }
    Some(out)
}

/// The string's units, to its 0: `read` when one does not read,
/// `string` when one is past 16 bits.
fn units(mem: &dyn GuestMemory, at: u32) -> Result<Vec<u16>, &'static str> {
    let mut out = Vec::new();
    for n in 0..MAX_UNITS {
        let unit = at.checked_add(4 * n).and_then(|a| mem.u32(a).ok()).ok_or("read")?;
        if unit == 0 {
            return Ok(out);
        }
        out.push(u16::try_from(unit).map_err(|_| "string")?);
    }
    Ok(out)
}

/// The string the draw at `call` makes, laid out in Rust. `Err`: why
/// Rust does not lay it out.
pub fn lay_out(mem: &dyn GuestMemory, call: &StringCall, fonts: &dyn UiText) -> Result<StringRun, &'static str> {
    fn read<T>(v: Option<T>) -> Result<T, &'static str> {
        v.ok_or("read")
    }
    if call.keep != 0 {
        return Err("hud");
    }
    if call.end != 0 {
        return Err("end");
    }
    if call.leading != 0 {
        return Err("leading");
    }
    if call.scale.to_bits() != 1f32.to_bits() {
        return Err("scale");
    }
    if call.extra != [0; 2] {
        return Err("extra");
    }
    if call.clip != 0 {
        return Err("clip");
    }
    // What the draw itself returns from draws nothing: the guest's.
    if read(mem.u8(TEXT_OFF).ok())? != 0
        || read(mem.u16(QUADS_OFF).ok())? != 0
        || read(mem.u8(CACHE_READY).ok())? == 0
        || read(mem.u32(GLYPH_BITMAP).ok())? == 0
    {
        return Err("off");
    }
    let font = u16::try_from(read(mem.u32(FONT).ok())?).map_err(|_| "font")?;
    if read(mem.u16(TABS).ok())? != 0 {
        return Err("tabs");
    }
    for at in INDENTS {
        if read(mem.u16(at).ok())? != 0 {
            return Err("indent");
        }
    }
    if read(mem.u16(LIMIT).ok())? != u16::MAX {
        return Err("limit");
    }
    let justify = match read(mem.u32(JUSTIFY).ok())? {
        1 => Justify::Right,
        2 => Justify::Centre,
        // `0x0013f2d7`: any other starts at the left.
        _ => Justify::Left,
    };
    let wrap = read(mem.u8(WRAP).ok())? & 1 != 0;
    let units = units(mem, call.units)?;
    if units.is_empty() {
        return Err("empty");
    }
    // The scanner's own characters: a tab, the alignment tokens and the
    // private-use glyphs (buttons, whose units another feature swaps at
    // the glyph lookup).
    if units.contains(&0x09) {
        return Err("tab");
    }
    if units.iter().any(|u| (0xe000..=0xe3ff).contains(u)) {
        return Err("icon");
    }
    let screen = read(hud::screen(mem))?;
    let [wt, wl, wb, wr] = screen.window;
    // `0x0001f0da`: the frame less the window's corner.
    let [top, left, bottom, right] = if call.rect != 0 {
        read(shorts(mem, call.rect))?
    } else {
        let f = screen.frame;
        [f[0].wrapping_sub(wt), f[1].wrapping_sub(wl), f[2].wrapping_sub(wt), f[3].wrapping_sub(wl)]
    };
    let rect = [left, top, right, bottom].map(f32::from);
    let shape = |rect: [f32; 4]| fonts.shape(font, &units, rect, justify).ok_or("shape");
    let glyphs = shape(rect)?;
    if !wrap {
        // A line breaks after a glyph past the right edge whether or not
        // the string wraps: Rust takes the string only on one line.
        let [l, t, r, b] = rect;
        let wide = match justify {
            Justify::Left => [l, t, r + WIDE, b],
            Justify::Right => [l - WIDE, t, r, b],
            Justify::Centre => [l - WIDE, t, r + WIDE, b],
        };
        if shape(wide)? != glyphs {
            return Err("overflow");
        }
    }
    if glyphs.is_empty() {
        return Err("empty");
    }
    let colour = read(four(mem, COLOUR))?;
    let shadow = if read(mem.u8(SHADOW_ON).ok())? != 0 { Some(read(four(mem, SHADOW))?) } else { None };
    let (main, shadow) = walk::text_colours(&colour, shadow.as_ref()).ok_or("colour")?;
    let glyphs: Vec<(u32, [f32; 4])> = glyphs.iter().map(|g| (g.index, g.rect)).collect();
    let quads = walk::text_quads(&glyphs, &[], main, shadow);
    // `0x0013f700` cuts a glyph to the window: Rust draws none it would.
    let (width, height) = (f32::from(wr.wrapping_sub(wl)), f32::from(wb.wrapping_sub(wt)));
    let inside = |q: &GlyphQuad| q.rect[0] >= 0.0 && q.rect[1] >= 0.0 && q.rect[2] <= width && q.rect[3] <= height;
    if !quads.iter().all(inside) {
        return Err("clipped");
    }
    let blend = OverlayBlend::On {
        source: read(mem.u32(BLEND_SOURCE).ok())?,
        dest: read(mem.u32(BLEND_DEST).ok())?,
        equation: read(mem.u32(BLEND_EQUATION).ok())?,
    };
    Ok(StringRun { font, quads, origin: [f32::from(wl), f32::from(wt)], blend })
}

/// Whether the guest's screen quad at `corners` (four vertices of x, y,
/// u, v and a packed colour, `0x0001f3a0`) is `quad`: its corners and
/// colour bit for bit, and its texels as wide and as high as its glyph
/// (the glyph cache's texels are the glyph's own, one to one).
pub fn glyph_matches(mem: &dyn GuestMemory, corners: u32, quad: &GlyphQuad) -> bool {
    let [x0, y0, x1, y1] = quad.rect;
    let read = || {
        let mut uv = [[0.0f32; 2]; 4];
        for (i, [x, y]) in [[x0, y0], [x1, y0], [x1, y1], [x0, y1]].into_iter().enumerate() {
            let at = corners.checked_add(i as u32 * 0x14)?;
            let word = |o: u32| mem.u32(at.checked_add(o)?).ok();
            if [word(0)?, word(4)?, word(16)?] != [x.to_bits(), y.to_bits(), quad.colour] {
                return Some(false);
            }
            uv[i] = [f32::from_bits(word(8)?), f32::from_bits(word(12)?)];
        }
        let square = uv[1][1] == uv[0][1] && uv[3][0] == uv[0][0] && uv[2] == [uv[1][0], uv[3][1]];
        Some(square && uv[1][0] - uv[0][0] == x1 - x0 && uv[3][1] - uv[0][1] == y1 - y0)
    };
    read().unwrap_or(false)
}

#[derive(Default)]
struct State {
    generation: Option<u64>,
    /// By the call site.
    proofs: HashMap<u32, Proof>,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(Mutex::default);

fn proof<T>(site: u32, f: impl FnOnce(&mut Proof) -> T) -> T {
    let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let generation = crate::native_map_work::generation();
    if s.generation != Some(generation) {
        *s = State { generation: Some(generation), ..State::default() };
    }
    f(s.proofs.entry(site).or_default())
}

static COUNTS: Mutex<BTreeMap<&'static str, u64>> = Mutex::new(BTreeMap::new());

pub(super) fn count(what: &'static str) {
    *COUNTS.lock().unwrap_or_else(|e| e.into_inner()).entry(what).or_default() += 1;
}

/// `native_ui_overlay_string_<what>` for everything the strings counted.
pub fn counters() -> Vec<(String, u64)> {
    let counts = COUNTS.lock().unwrap_or_else(|e| e.into_inner());
    counts.iter().map(|(what, &n)| (format!("native_ui_overlay_string_{what}"), n)).collect()
}

/// A shadowed draw: the quads Rust laid out, and how far the guest's
/// matched.
struct Expect {
    site: u32,
    quads: Vec<GlyphQuad>,
    next: usize,
    matched: bool,
}

thread_local! {
    static EXPECT: RefCell<Option<Expect>> = const { RefCell::new(None) };
}

/// What the string draw's entry does.
#[derive(Clone, Debug, PartialEq)]
pub enum Plan {
    /// The guest draws it.
    Guest,
    /// Rust draws it.
    Own(StringRun),
}

/// `STRING_DRAW`'s entry (`esp` its return slot): whose the draw is.
/// `fonts` lays the string out; with none the guest does.
pub fn plan(mem: &dyn GuestMemory, esp: u32, eax: u32, ecx: u32, fonts: Option<&dyn UiText>) -> Plan {
    // A check the draw's return did not finish (an unwind) has no answer.
    if EXPECT.with(|e| e.borrow_mut().take()).is_some() {
        count("unfinished");
    }
    let refused = |why: &'static str| {
        count(why);
        Plan::Guest
    };
    let Some(call) = StringCall::read(mem, esp, eax, ecx) else { return refused("read") };
    // A site whose draw differed is not laid out again on this map.
    if proof(call.site, |p| p.failed()) {
        return refused("differed");
    }
    let Some(fonts) = fonts else { return refused("fonts") };
    let run = match lay_out(mem, &call, fonts) {
        Ok(run) => run,
        Err(why) => return refused(why),
    };
    match proof(call.site, Proof::arm) {
        Arm::Guest => refused("differed"),
        Arm::Shadow => {
            EXPECT.with(|e| *e.borrow_mut() = Some(Expect { site: call.site, quads: run.quads, next: 0, matched: true }));
            Plan::Guest
        }
        Arm::Own => Plan::Own(run),
    }
}

/// The guest's screen quad at `corners`, in a shadowed string's draw:
/// checked against the next quad Rust laid out.
pub fn observe_glyph(mem: &dyn GuestMemory, corners: u32) {
    EXPECT.with(|e| {
        let mut e = e.borrow_mut();
        let Some(e) = e.as_mut() else { return };
        let matched = e.quads.get(e.next).is_some_and(|quad| glyph_matches(mem, corners, quad));
        e.matched &= matched;
        e.next += 1;
    });
}

/// `STRING_DRAW`'s return: a shadowed draw's answer.
pub fn finish() {
    let Some(e) = EXPECT.with(|e| e.borrow_mut().take()) else { return };
    let matched = e.matched && e.next == e.quads.len();
    count(if matched { "shadow_matched" } else { "shadow_differed" });
    if proof(e.site, |p| p.checked(matched)) {
        count("proven");
    }
}

/// A proven string's draw by the host: counted, and the draw's counter
/// moved as the guest's draw moves it.
pub fn owned(mem: &mut dyn GuestMemory, drawn: bool) {
    if !drawn {
        count("host");
        return;
    }
    count("drawn");
    let moved = mem.u16(DRAW_COUNT).ok().map(|n| n.wrapping_add(1).to_le_bytes());
    if !moved.is_some_and(|word| mem.write(DRAW_COUNT, &word).is_ok()) {
        eprintln!("native strings: the draw counter at {DRAW_COUNT:#x} did not take its word");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::walk::PROOF_DRAWS;
    use engine::testing::FakeGuest;

    /// Every glyph 8 by 10, 2 under the line's top, 10 apart, lines 12
    /// apart; its index is its character. A left line ends after a glyph
    /// past the right edge; a right or centred line takes the room left
    /// over, so a wider rectangle moves it.
    struct Cells;

    impl UiText for Cells {
        fn shape(&self, _: u16, text: &[u16], rect: [f32; 4], justify: Justify) -> Option<Vec<frame_scene::OverlayGlyph>> {
            let width = 10.0 * text.len() as f32;
            let start = match justify {
                Justify::Left => rect[0],
                Justify::Right => rect[2] - width,
                Justify::Centre => (rect[0] + rect[2] - width) * 0.5,
            };
            let (mut x, mut y) = (start, rect[1]);
            let mut out = Vec::new();
            for &unit in text {
                if justify == Justify::Left && x > start && x + 8.0 > rect[2] {
                    (x, y) = (start, y + 12.0);
                }
                out.push(frame_scene::OverlayGlyph { index: u32::from(unit), rect: [x, y + 2.0, x + 8.0, y + 12.0] });
                x += 10.0;
            }
            Some(out)
        }
    }

    const ESP: u32 = 0x8000;
    const UNITS: u32 = 0x3000;
    const RECT: u32 = 0x3800;
    const SITE: u32 = 0x0016_1717;

    /// A plain string "hi" from `0x0001fa50`, in white with a black
    /// shadow, left in the rectangle 40, 20 to 300, 60 of a 640 by 480
    /// window at 16, 8.
    fn plain() -> FakeGuest {
        let mut m = FakeGuest::new();
        m.put_u32(ESP, FROM_PLAIN).put_u32(ESP + PLAIN_RETURN_FROM_ESP, SITE);
        for (n, word) in [0, 0, UNITS, 1f32.to_bits(), 0, 0, 0].into_iter().enumerate() {
            m.put_u32(ESP + 4 + 4 * n as u32, word);
        }
        for (n, unit) in [u32::from(b'h'), u32::from(b'i'), 0].into_iter().enumerate() {
            m.put_u32(UNITS + 4 * n as u32, unit);
        }
        for (i, v) in [20i16, 40, 60, 300].into_iter().enumerate() {
            m.put_u16(RECT + 2 * i as u32, v as u16);
        }
        for (i, v) in [8u16, 16, 488, 656, 32, 48, 464, 624].into_iter().enumerate() {
            m.put_u16(hud::WINDOW + 2 * i as u32, v);
        }
        m.put_u32(0x004e_6948, 0).put_u8(0x0048_5ac2, 0);
        m.put_u8(TEXT_OFF, 0).put_u16(QUADS_OFF, 0).put_u8(CACHE_READY, 1).put_u32(GLYPH_BITMAP, 0x9000);
        m.put_u32(FONT, 6).put_u32(WRAP, 0).put_u16(LIMIT, u16::MAX).put_u32(JUSTIFY, 0);
        m.put_u16(TABS, 0).put_u16(INDENTS[0], 0).put_u16(INDENTS[1], 0);
        for i in 0..4 {
            m.put_f32(COLOUR + 4 * i, 1.0).put_f32(SHADOW + 4 * i, if i == 0 { 1.0 } else { 0.0 });
        }
        m.put_u8(SHADOW_ON, 1);
        m.put_u32(BLEND_SOURCE, 0x302).put_u32(BLEND_DEST, 0x303).put_u32(BLEND_EQUATION, 0x8006);
        m
    }

    fn call(m: &FakeGuest) -> StringCall {
        StringCall::read(m, ESP, 0, RECT).unwrap()
    }

    #[test]
    fn a_plain_string_is_laid_out_with_each_glyphs_shadow_first() {
        let m = plain();
        let call = call(&m);
        assert_eq!(call.site, SITE, "0x0001fa50's caller");
        let run = lay_out(&m, &call, &Cells).unwrap();
        assert_eq!(run.font, 6);
        assert_eq!(run.origin, [16.0, 8.0]);
        assert_eq!(run.blend, OverlayBlend::On { source: 0x302, dest: 0x303, equation: 0x8006 });
        let h = GlyphQuad { index: u32::from(b'h'), rect: [40.0, 22.0, 48.0, 32.0], colour: 0xffff_ffff };
        let i = GlyphQuad { index: u32::from(b'i'), rect: [50.0, 22.0, 58.0, 32.0], colour: 0xffff_ffff };
        let shadow = |q: GlyphQuad| GlyphQuad { rect: q.rect.map(|v| v + 1.0), colour: 0xff00_0000, ..q };
        assert_eq!(run.quads, [shadow(h), h, shadow(i), i]);
        // With no rectangle, the frame less the window's corner.
        let framed = lay_out(&m, &StringCall { rect: 0, ..call }, &Cells).unwrap();
        assert_eq!(framed.quads[1].rect, [32.0, 26.0, 40.0, 36.0]);
    }

    #[test]
    fn what_the_layout_does_not_read_is_refused_by_name() {
        let base = plain();
        let call = call(&base);
        let refusal = |change: &dyn Fn(&mut FakeGuest, &mut StringCall)| {
            let mut m = plain();
            let mut c = call;
            change(&mut m, &mut c);
            lay_out(&m, &c, &Cells).unwrap_err()
        };
        assert_eq!(refusal(&|_, c| c.keep = 1), "hud");
        assert_eq!(refusal(&|_, c| c.end = 0x10), "end");
        assert_eq!(refusal(&|_, c| c.leading = 2), "leading");
        assert_eq!(refusal(&|_, c| c.scale = 0.5), "scale");
        assert_eq!(refusal(&|_, c| c.extra = [0, 4]), "extra");
        assert_eq!(refusal(&|_, c| c.clip = 0x10), "clip");
        assert_eq!(refusal(&|m, _| { m.put_u16(QUADS_OFF, 1); }), "off");
        assert_eq!(refusal(&|m, _| { m.put_u8(CACHE_READY, 0); }), "off");
        assert_eq!(refusal(&|m, _| { m.put_u32(FONT, 0x1_0000); }), "font");
        assert_eq!(refusal(&|m, _| { m.put_u16(TABS, 2); }), "tabs");
        assert_eq!(refusal(&|m, _| { m.put_u16(INDENTS[1], 4); }), "indent");
        assert_eq!(refusal(&|m, _| { m.put_u16(LIMIT, 3); }), "limit");
        assert_eq!(refusal(&|m, _| { m.put_u32(UNITS, 0); }), "empty");
        assert_eq!(refusal(&|m, _| { m.put_u32(UNITS, 0x1_0041); }), "string");
        assert_eq!(refusal(&|m, _| { m.put_u32(UNITS, 9); }), "tab");
        assert_eq!(refusal(&|m, _| { m.put_u32(UNITS, 0xe100); }), "icon");
        assert_eq!(refusal(&|m, _| { m.put_f32(COLOUR + 4, 2.0); }), "colour");
        // Past the rectangle with no wrap: the line would break.
        assert_eq!(refusal(&|m, _| { m.put_u16(RECT + 6, 45); }), "overflow");
        // Past the window: the guest cuts the glyph.
        assert_eq!(refusal(&|m, _| { m.put_u16(RECT + 2, 0xfffc); }), "clipped");
        // A wrapping string is shaped in its own rectangle.
        let mut m = plain();
        m.put_u32(WRAP, 1).put_u16(RECT + 6, 45);
        assert!(lay_out(&m, &call, &Cells).is_ok());
    }

    /// The guest's glyph quad as `0x0001f3a0` pushes it: the cache's
    /// texels at `(u, v)`, one to one.
    fn guest_glyph(m: &mut FakeGuest, at: u32, quad: &GlyphQuad, (u, v): (f32, f32)) {
        let [x0, y0, x1, y1] = quad.rect;
        let (w, h) = (x1 - x0, y1 - y0);
        for (i, [x, y, s, t]) in [[x0, y0, u, v], [x1, y0, u + w, v], [x1, y1, u + w, v + h], [x0, y1, u, v + h]].into_iter().enumerate() {
            let c = at + 0x14 * i as u32;
            m.put_f32(c, x).put_f32(c + 4, y).put_f32(c + 8, s).put_f32(c + 12, t).put_u32(c + 16, quad.colour);
        }
    }

    #[test]
    fn a_runs_quad_is_the_guests_place_texels_and_colour() {
        let quad = GlyphQuad { index: 7, rect: [40.0, 22.0, 48.0, 32.0], colour: 0xffff_ffff };
        let mut m = FakeGuest::new();
        guest_glyph(&mut m, 0x5000, &quad, (100.0, 30.0));
        assert!(glyph_matches(&m, 0x5000, &quad));
        assert!(!glyph_matches(&m, 0x5000, &GlyphQuad { colour: 0xff00_0000, ..quad }), "colour");
        assert!(!glyph_matches(&m, 0x5000, &GlyphQuad { rect: [41.0, 22.0, 49.0, 32.0], ..quad }), "place");
        // Texels of another size: the cache's glyph is not this one.
        m.put_f32(0x5000 + 0x14 + 8, 107.0).put_f32(0x5000 + 0x28 + 8, 107.0);
        assert!(!glyph_matches(&m, 0x5000, &quad), "texels");
    }

    #[test]
    fn a_site_is_the_guests_until_its_strings_matched_then_rusts() {
        let mut m = plain();
        let site = 0x0016_1a99;
        m.put_u32(ESP + PLAIN_RETURN_FROM_ESP, site);
        let laid = lay_out(&m, &call(&m), &Cells).unwrap();
        for (n, quad) in laid.quads.iter().enumerate() {
            guest_glyph(&mut m, 0x6000 + 0x50 * n as u32, quad, (8.0 * n as f32, 0.0));
        }
        for _ in 0..PROOF_DRAWS {
            assert_eq!(plan(&m, ESP, 0, RECT, Some(&Cells)), Plan::Guest);
            for n in 0..laid.quads.len() as u32 {
                observe_glyph(&m, 0x6000 + 0x50 * n);
            }
            finish();
        }
        assert_eq!(plan(&m, ESP, 0, RECT, Some(&Cells)), Plan::Own(laid.clone()));
        // No fonts: the guest's.
        assert_eq!(plan(&m, ESP, 0, RECT, None), Plan::Guest);
    }

    #[test]
    fn a_draw_that_differs_keeps_the_site_on_the_guest() {
        let mut m = plain();
        let site = 0x0016_1ae6;
        m.put_u32(ESP + PLAIN_RETURN_FROM_ESP, site);
        let laid = lay_out(&m, &call(&m), &Cells).unwrap();
        for (n, quad) in laid.quads.iter().enumerate() {
            guest_glyph(&mut m, 0x6000 + 0x50 * n as u32, quad, (0.0, 0.0));
        }
        assert_eq!(plan(&m, ESP, 0, RECT, Some(&Cells)), Plan::Guest);
        // One quad fewer than Rust's.
        for n in 0..laid.quads.len() as u32 - 1 {
            observe_glyph(&m, 0x6000 + 0x50 * n);
        }
        finish();
        for _ in 0..PROOF_DRAWS + 1 {
            assert_eq!(plan(&m, ESP, 0, RECT, Some(&Cells)), Plan::Guest);
            finish();
        }
        assert!(!proof(site, |p| p.proven()));
    }
}
