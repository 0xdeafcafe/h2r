//! The string draw `0x0001f070` in Rust (docs/graphics/native-hud-overlay.md,
//! "Strings"). At its entry (`STRING_DRAW`) the string is laid out from
//! the font renderer's state (`0x004e73a0`..) and the call's rectangle
//! through the host's fonts ([`UiText`]): each glyph's quad, its shadow's,
//! and their colours, as the glyph callback `0x0001f490` would push them
//! to the screen quad `0x000363a0`.
//!
//! The draw is Rust's: its early returns (`0x0001f07f`..`0x0001f0c4`)
//! as the guest takes them ([`enter`]), then the host draws the string as
//! text runs over its glyph pages ([`super::Host::rust_string`]). The
//! guest's body, its batch (`0x00035b90`) and its glyph quads
//! (`0x0001f490`, `0x0001f3a0`, `0x000363a0`) never run. What the layout
//! does not read panics by name.

use std::collections::BTreeMap;
use std::sync::Mutex;

use guest_layout::memory::GuestMemory;
use frame_scene::OverlayBlend;
use scene_extract::hud::{self, walk};
use scene_extract::ui::{Justify, UiText};

pub use walk::GlyphQuad;

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

/// What the draw's entry does before its layout.
#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    /// It returns before it counts the draw: text off (`0x0051080c`), or
    /// the screen quads off (`0x00485602`).
    Off,
    /// It counts the draw ([`DRAW_COUNT`]) and returns: the glyph cache
    /// not ready or not made, or the string empty.
    Nothing,
    /// It counts the draw and lays the string out: the run the host draws.
    Draw(StringRun),
}

/// `STRING_DRAW`'s entry (`esp` its return slot), as `0x0001f07f`..
/// `0x0001f0c4` takes it, then the layout. `Err`: what Rust does not lay
/// out, and the call's site.
pub fn enter(mem: &mut dyn GuestMemory, esp: u32, eax: u32, ecx: u32, fonts: Option<&dyn UiText>) -> Result<Entry, String> {
    let read = |v: Option<u32>| v.ok_or_else(|| "the draw's state does not read".to_string());
    if read(mem.u8(TEXT_OFF).ok().map(u32::from))? != 0 || read(mem.u16(QUADS_OFF).ok().map(u32::from))? != 0 {
        return Ok(Entry::Off);
    }
    let moved = read(mem.u16(DRAW_COUNT).ok().map(u32::from))?.wrapping_add(1) as u16;
    if mem.write(DRAW_COUNT, &moved.to_le_bytes()).is_err() {
        return Err(format!("the draw counter at {DRAW_COUNT:#x} did not take its word"));
    }
    let Some(call) = StringCall::read(&*mem, esp, eax, ecx) else {
        return Err("the call does not read".to_string());
    };
    if read(mem.u8(CACHE_READY).ok().map(u32::from))? == 0 || read(mem.u32(GLYPH_BITMAP).ok())? == 0 {
        return Ok(Entry::Nothing);
    }
    if read(mem.u32(call.units).ok())? == 0 {
        return Ok(Entry::Nothing);
    }
    let site = call.site;
    let Some(fonts) = fonts else { return Err(format!("no fonts (from {site:#x})")) };
    match lay_out(&*mem, &call, fonts) {
        Ok(run) => {
            count("drawn");
            Ok(Entry::Draw(run))
        }
        Err(why) => Err(format!("{why} (from {site:#x})")),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

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
        m.put_u8(TEXT_OFF, 0).put_u16(QUADS_OFF, 0).put_u8(CACHE_READY, 1).put_u32(GLYPH_BITMAP, 0x9000).put_u16(DRAW_COUNT, 0);
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

    #[test]
    fn the_entry_returns_where_the_guest_does_and_counts_as_it_does() {
        let go = |m: &mut FakeGuest| enter(m, ESP, 0, RECT, Some(&Cells));
        let draws = |m: &FakeGuest| m.u16(DRAW_COUNT).unwrap();
        // Text off or the quads off: back before the count.
        let offs: [fn(&mut FakeGuest); 2] = [|m| { m.put_u8(TEXT_OFF, 1); }, |m| { m.put_u16(QUADS_OFF, 1); }];
        for off in offs {
            let mut m = plain();
            off(&mut m);
            assert_eq!(go(&mut m), Ok(Entry::Off));
            assert_eq!(draws(&m), 0);
        }
        // The cache not ready or not made, or no units: counted, nothing drawn.
        let nothings: [fn(&mut FakeGuest); 3] = [
            |m| { m.put_u8(CACHE_READY, 0); },
            |m| { m.put_u32(GLYPH_BITMAP, 0); },
            |m| { m.put_u32(UNITS, 0); },
        ];
        for nothing in nothings {
            let mut m = plain();
            nothing(&mut m);
            assert_eq!(go(&mut m), Ok(Entry::Nothing));
            assert_eq!(draws(&m), 1);
        }
        let mut m = plain();
        let laid = lay_out(&m, &call(&m), &Cells).unwrap();
        assert_eq!(go(&mut m), Ok(Entry::Draw(laid)));
        assert_eq!(go(&mut m).map(|_| draws(&m)), Ok(2));
    }

    #[test]
    fn a_string_the_layout_does_not_read_names_its_cause_and_site() {
        let mut m = plain();
        m.put_u16(TABS, 2);
        assert_eq!(enter(&mut m, ESP, 0, RECT, Some(&Cells)), Err(format!("tabs (from {SITE:#x})")));
        let mut m = plain();
        assert_eq!(enter(&mut m, ESP, 0, RECT, None), Err(format!("no fonts (from {SITE:#x})")));
    }
}
