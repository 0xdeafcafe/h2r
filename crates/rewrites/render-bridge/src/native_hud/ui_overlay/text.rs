//! Screen text and button labels. Retail entry points and field evidence are
//! recorded in docs/graphics/notes/2026-11-24-menu-text.md.

use engine::memory::GuestMemory;
use scene_extract::GuestRead;
use scene_extract::ui::{self, Justify, TextLayout, UiContent, UiWidget, Unread};

use super::Gap;

mod tokens;

fn word(mem: &dyn GuestRead, base: u32, off: u32) -> Result<u32, Unread> {
    base.checked_add(off)
        .and_then(|a| mem.read_u32(a))
        .ok_or(Unread::Unreadable)
}

fn half(mem: &dyn GuestRead, base: u32, off: u32) -> Result<u16, Unread> {
    let at = base.checked_add(off).ok_or(Unread::Unreadable)?;
    Ok((word(mem, at & !3, 0)? >> ((at & 2) * 8)) as u16)
}

fn number(mem: &dyn GuestRead, base: u32, off: u32) -> Result<f32, Unread> {
    let n = f32::from_bits(word(mem, base, off)?);
    n.is_finite().then_some(n).ok_or(Unread::Unreadable)
}

fn interface(mem: &dyn GuestRead, widget: u32, button: bool) -> Result<(u32, u32), Gap> {
    let getter = word(mem, word(mem, widget, 0)?, 0x3c)?;
    let offset = match (button, getter) {
        (true, 0x0023_7607) => 0x70,
        (false, 0x0022_f57f) => 0x74,
        _ => return Err(Gap::text("text_interface")),
    };
    let iface = widget.checked_add(offset).ok_or(Unread::Unreadable)?;
    let vt = word(mem, iface, 0)?;
    if word(mem, vt, 8)? != 0x0022_f52e {
        return Err(Gap::text("text_interface"));
    }
    let capacity = match word(mem, vt, 4)? {
        0x0022_f50d => 32,
        0x0022_f545 => 256,
        _ => return Err(Gap::text("text_interface")),
    };
    Ok((iface, capacity))
}

fn string(mem: &dyn GuestRead, iface: u32, capacity: u32) -> Result<Vec<u16>, Unread> {
    let mut units = Vec::new();
    for i in 0..capacity {
        let unit = half(mem, iface, 0x44 + i * 2)?;
        if unit == 0 {
            return Ok(units);
        }
        units.push(unit);
    }
    Err(Unread::Unreadable)
}

/// The string draw's private-use table (`0x00470204`): 131 entries of a
/// unit and its handler, 12 bytes each.
const TOKENS: u32 = 0x0047_0204;
const TOKEN_COUNT: u32 = 131;
/// The named sequences (`0x0044a3f0`): 18 entries of a string and the
/// unit that replaces it, 8 bytes each.
const ALIASES: u32 = 0x0044_a3f0;
const ALIAS_COUNT: u32 = 18;
/// The draw's copy of the string (`0x0022cd48`: 0x400 units at
/// `ebp-0x830`), the substitution's capacity.
const COPY_UNITS: usize = 0x400;

fn wide(mem: &dyn GuestRead, at: u32) -> Result<Vec<u16>, Unread> {
    let mut units = Vec::new();
    for i in 0..COPY_UNITS as u32 {
        let unit = half(mem, at, i * 2)?;
        if unit == 0 {
            return Ok(units);
        }
        units.push(unit);
    }
    Err(Unread::Unreadable)
}

/// The substitution pass `0x0022d2ee` over the draw's copy of the string.
/// At each unit in turn: a private-use unit whose table entry has a
/// handler is a token, replaced by what its handler writes
/// ([`tokens::write`]; a handler not ported panics, named); otherwise
/// each named sequence
/// that starts there is tried, the last that matches replacing it with
/// its unit, unless the copy would not hold the result. The scan moves
/// one unit on, past the replacement. Private-use units with no handler
/// stay: they are the fonts' button glyphs.
fn substitute(mem: &dyn GuestRead, text: &[u16]) -> Result<Vec<u16>, Gap> {
    let mut aliases = Vec::with_capacity(ALIAS_COUNT as usize);
    for i in 0..ALIAS_COUNT {
        let name = wide(mem, word(mem, ALIASES, i * 8)?)?;
        let unit = half(mem, ALIASES, i * 8 + 4)?;
        aliases.push((name, unit));
    }
    let mut buf = text.to_vec();
    let mut pos = 0;
    while pos < buf.len() {
        let unit = buf[pos];
        let mut handler = 0;
        if (0xe000..=0xf8ff).contains(&unit) {
            for i in 0..TOKEN_COUNT {
                if word(mem, TOKENS, i * 12)? == u32::from(unit) {
                    handler = word(mem, TOKENS, i * 12 + 4)?;
                    break;
                }
            }
        }
        if handler != 0 {
            // `0x0022d43d`: the handler's text replaces the one unit, the
            // named sequences not tried.
            let with = tokens::write(mem, unit, handler)?;
            if buf.len() - 1 + with.len() < COPY_UNITS {
                drop(buf.splice(pos..pos + 1, with));
            }
            pos += 1;
            continue;
        }
        let mut found: Option<(usize, u16)> = None;
        for (name, unit) in &aliases {
            if buf[pos..].starts_with(name) {
                found = Some((name.len(), *unit));
            }
        }
        if let Some((matched, unit)) = found {
            // `wcsncpy` of the unit and its terminator: a zero unit
            // replaces with nothing.
            let with: &[u16] = if unit == 0 { &[] } else { std::slice::from_ref(&unit) };
            if buf.len() - matched + with.len() < COPY_UNITS {
                drop(buf.splice(pos..pos + matched, with.iter().copied()));
            }
        }
        pos += 1;
    }
    Ok(buf)
}

/// The UI clock (`[0x0054d5b8]`): the reveal cursor's and the flash's.
const UI_CLOCK: u32 = 0x0054_d5b8;
/// The last unit the reveal cursor writes at: the copy's capacity less
/// the cursor and its terminator.
const REVEAL_LAST: i16 = 0x3fe;
/// With no shared UI globals: the cursor glyph and the ticks per step.
const REVEAL_GLYPH: u16 = 0xdb;
const REVEAL_TICKS: u32 = 0x32;
/// The shared UI globals' (`wigl`) ticks per step (`i16`) and cursor glyph.
const WIGL_REVEAL_TICKS: u32 = 0x48;
const WIGL_REVEAL_GLYPH: u32 = 0x4a;

/// The interface's reveal cursor (`0x0022cdbf` to `0x0022ce2d`): the unit
/// it is at (`+0x3c`, off when negative), the unit it stops at (`+0x3e`)
/// and the clock of its last step (`+0x40`, 0 before the first).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Reveal {
    cursor: i16,
    end: i16,
    last: u32,
}

impl Reveal {
    /// The cursor; its end and clock only when it is on (the draw reads
    /// them only then).
    fn read(mem: &dyn GuestRead, iface: u32) -> Result<Reveal, Unread> {
        let cursor = half(mem, iface, 0x3c)? as i16;
        if cursor < 0 {
            return Ok(Reveal { cursor, end: 0, last: 0 });
        }
        Ok(Reveal { cursor, end: half(mem, iface, 0x3e)? as i16, last: word(mem, iface, 0x40)? })
    }

    /// The unit of the draw's copy the cursor glyph is written at, then a
    /// terminator: `None` when the cursor is off.
    fn at(self) -> Option<i16> {
        (self.cursor >= 0).then(|| self.cursor.min(REVEAL_LAST))
    }

    /// The string as the draw shows it: cut at the cursor with its glyph
    /// written there. A cursor past the end leaves the string, whose
    /// terminator comes first.
    fn show(self, text: &mut Vec<u16>, glyph: u16) {
        if let Some(at) = self.at().map(|at| at as usize)
            && at <= text.len()
        {
            text.truncate(at);
            text.push(glyph);
        }
    }

    /// The state the draw leaves at `clock`: the clamped cursor written
    /// back; the first draw stamps the clock; once `ticks` have passed
    /// (unsigned) the cursor moves on one and the clock is stamped, and a
    /// cursor that reaches its end is turned off.
    fn step(self, clock: u32, ticks: u32) -> Reveal {
        let Some(at) = self.at() else { return self };
        if self.last == 0 {
            return Reveal { cursor: at, last: clock, ..self };
        }
        if clock.wrapping_sub(self.last) < ticks {
            return Reveal { cursor: at, ..self };
        }
        let next = at + 1;
        Reveal { cursor: if next == self.end { -1 } else { next }, last: clock, ..self }
    }
}

/// The cursor glyph and the ticks per step: the shared UI globals' (the
/// ticks an `i16`, compared unsigned), else `0xdb` and 50.
fn reveal_style(mem: &dyn GuestRead) -> Result<(u16, u32), Unread> {
    match super::bitmaps::shared_globals(mem).ok_or(Unread::Unreadable)? {
        Some(wigl) => Ok((
            half(mem, wigl, WIGL_REVEAL_GLYPH)?,
            half(mem, wigl, WIGL_REVEAL_TICKS)? as i16 as i32 as u32,
        )),
        None => Ok((REVEAL_GLYPH, REVEAL_TICKS)),
    }
}

/// The interface of a text or button widget whose draw reaches the
/// reveal cursor with it on: a string not empty in a rectangle with width
/// and height (`0x0022cd5e` to `0x0022cd8a`).
fn revealing(mem: &dyn GuestRead, widget: u32, button: bool) -> Result<Option<u32>, Gap> {
    let (iface, capacity) = interface(mem, widget, button)?;
    if string(mem, iface, capacity)?.is_empty() || Reveal::read(mem, iface)?.at().is_none() {
        return Ok(None);
    }
    let mut w = ui::read_widget_header(mem, widget, 0.0, UiContent::None)?;
    offset_rect(&mut w)?;
    let [top, left, bottom, right] = w.rect;
    Ok((top != bottom && left != right).then_some(iface))
}

/// A text or button widget's draw the overlay left out: the reveal
/// cursor's step its draw makes (`0x0022cdc8`), written as the draw
/// writes it. A widget with the cursor off writes nothing; one that does
/// not read is counted and writes nothing.
pub(super) fn left_out(mem: &mut dyn GuestMemory, widget: u32, button: bool) {
    let step = {
        let ro = super::Ro(&*mem);
        let read = || -> Result<Option<(u32, Reveal)>, Gap> {
            let Some(iface) = revealing(&ro, widget, button)? else { return Ok(None) };
            let (_, ticks) = reveal_style(&ro)?;
            let clock = word(&ro, UI_CLOCK, 0)?;
            Ok(Some((iface, Reveal::read(&ro, iface)?.step(clock, ticks))))
        };
        read()
    };
    match step {
        Ok(None) => {}
        Ok(Some((iface, next))) => {
            let cursor = iface.checked_add(0x3c).map(|at| mem.write(at, &(next.cursor as u16).to_le_bytes()));
            let last = iface.checked_add(0x40).map(|at| mem.write(at, &next.last.to_le_bytes()));
            if !matches!((cursor, last), (Some(Ok(())), Some(Ok(())))) {
                timeline::native::Ui::OverlayMenuRevealUnwritten.count();
            }
        }
        Err(_) => timeline::native::Ui::OverlayMenuRevealUnwritten.count(),
    }
}

/// The draw's upper case (`0x0013fae0`): basic Latin `a` to `z`, and of
/// the rest only `é` and `ó`.
fn upper(unit: u16) -> u16 {
    match unit {
        0x61..=0x7a => unit - 0x20,
        0xe9 => 0xc9,
        0xf3 => 0xd3,
        _ => unit,
    }
}

/// The rectangle helper `0x0022e9c6`: the header's, plus the animated
/// offset's X and Y, truncated to `i32` and added as `i16`.
fn offset_rect(w: &mut UiWidget) -> Result<(), Gap> {
    for (i, delta) in w.offset.into_iter().enumerate() {
        if !delta.is_finite() || delta.abs() >= i32::MAX as f32 {
            return Err(Gap::text("text_offset"));
        }
        let first = 1 - i;
        for edge in [first, first + 2] {
            w.rect[edge] = w.rect[edge].wrapping_add(delta as i32 as i16);
        }
    }
    w.offset = [0.0; 2];
    Ok(())
}

/// The line renderer's two indents (`0x0013f1d9`): the first row
/// group's and the rest's, as the renderer holds them; the interface
/// draw does not set them.
const INDENTS: [u32; 2] = [0x004e_73fe, 0x004e_7400];
/// The interface's tab stops (`+0x28`, eight `i16`) and their count
/// (`+0x38`).
const STOPS: u32 = 0x28;
const STOP_COUNT: u32 = 0x38;
const MAX_STOPS: i32 = 8;

/// The interface draw's rectangle and advance scale (`0x0022cef5` to
/// `0x0022d067`): the rectangle's corners projected at the widget's Z
/// about the full canvas's centre (`0x002360c3`) and truncated to `i16`,
/// `[top, left, bottom, right]` Y down; the scale is the projected width
/// over the rectangle's, times the interface's `+0x20` (1, `text_style`).
fn projected(mem: &dyn GuestRead, widget: u32, [top, left, bottom, right]: [i16; 4]) -> Result<([i16; 4], f32), Gap> {
    use super::bitmaps::{project, truncated, Lens};
    let lens = Lens::read(mem).ok_or(Unread::Unreadable)?;
    let z = number(mem, widget, 0x60)?;
    let near = project(&lens, [f32::from(left), f32::from(top), z], super::FULL_SCREEN);
    let far = project(&lens, [f32::from(right), f32::from(bottom), z], super::FULL_SCREEN);
    let scale = (far[0] - near[0]) / (i32::from(right) - i32::from(left)) as f32;
    let rect = [truncated(near[1]), truncated(near[0]), truncated(far[1]), truncated(far[0])];
    Ok((rect, scale))
}

/// A projected rectangle (Y down from the canvas's corner) as a header's
/// (Y up about its centre), which `ui::widget_rect` turns back.
fn rect_about_centre([top, left, bottom, right]: [i16; 4]) -> [i16; 4] {
    let [cx, cy] = ui::CANVAS.map(|v| (v / 2) as i16);
    [cy.wrapping_sub(top), left.wrapping_sub(cx), cy.wrapping_sub(bottom), right.wrapping_sub(cx)]
}

/// The tab stops the draw hands the line renderer (`0x0022d007`,
/// `0x0013eb20`): a count at `+0x38` not above 0 is none; otherwise each
/// stop is the interface's plus the projected rectangle's left, as `i16`.
/// The interface holds eight: a larger count runs the copy past the
/// draw's stack buffer (`text_tab_overrun`).
fn stops(mem: &dyn GuestRead, iface: u32, left: i16) -> Result<Vec<i16>, Gap> {
    let count = word(mem, iface, STOP_COUNT)? as i32;
    if count <= 0 {
        return Ok(Vec::new());
    }
    if count > MAX_STOPS {
        return Err(Gap::text("text_tab_overrun"));
    }
    (0..count as u32)
        .map(|i| -> Result<i16, Gap> { Ok((half(mem, iface, STOPS + i * 2)? as i16).wrapping_add(left)) })
        .collect()
}

/// The caller checks the full-canvas viewport and, for a button, the optional bitmap.
pub(super) fn read_widget(
    mem: &dyn GuestRead,
    widget: u32,
    depth: f32,
    button: bool,
) -> Result<UiWidget, Gap> {
    let mut w = ui::read_widget_header(mem, widget, depth, UiContent::None)?;
    if word(mem, 0x0054_d59c, 0)? >> 24 != 0 || word(mem, 0x0051_ec04, 0)? as u8 != 0 {
        // The widget index display or the rectangle outlines are on.
        return Err(Gap::text("text_debug"));
    }
    let (iface, capacity) = interface(mem, widget, button)?;
    let text = string(mem, iface, capacity)?;
    if text.is_empty() {
        return Ok(w);
    }
    if word(mem, 0x0051_080c, 0)? as u8 != 0
        || half(mem, 0x0048_5602, 0)? != 0
        || word(mem, 0x004b_62a0, 0)? as u8 == 0
        || word(mem, 0x004b_62ac, 0)? == 0
    {
        // The renderer's glyph output is off, or no glyph ring is active.
        return Err(Gap::text("text_glyphs"));
    }
    let flags = half(mem, iface, 0x14)?;
    if flags & !7 != 0
        || half(mem, iface, 0x16)? != 1
        || word(mem, iface, 0x18)? != u32::MAX
        || number(mem, iface, 0x20)? != 1.0
    {
        return Err(Gap::text("text_style"));
    }
    // The reveal cursor: shown here, its step written when the draw is
    // left out (`left_out`).
    let mut text = text;
    let reveal = Reveal::read(mem, iface)?;
    if reveal.at().is_some() {
        reveal.show(&mut text, reveal_style(mem)?.0);
    }
    let mut text = substitute(mem, &text)?;
    if text.is_empty() {
        return Ok(w);
    }
    offset_rect(&mut w)?;
    let [top, left, bottom, right] = w.rect;
    if top == bottom || left == right {
        // `0x0022cd71`: no width or no height, nothing drawn.
        return Ok(w);
    }
    let (rect, scale) = projected(mem, widget, w.rect)?;
    let layout = TextLayout {
        scale,
        stops: stops(mem, iface, rect[1])?,
        indents: [half(mem, INDENTS[0], 0)? as i16, half(mem, INDENTS[1], 0)? as i16],
    };
    w.rect = rect_about_centre(rect);
    let justify = match word(mem, iface, 0x1c)? {
        0 => Justify::Left,
        1 => Justify::Right,
        2 => Justify::Centre,
        _ => return Err(Gap::text("text_justify")),
    };
    let mut color = [1.0; 4];
    for (i, value) in color[..3].iter_mut().enumerate() {
        let off = i as u32 * 4;
        *value = number(mem, iface, 8 + off)?;
        if !button {
            *value *= number(mem, widget, 0x28 + off)?;
        }
    }
    // The font colour setter compresses highlights when *all* three
    // components exceed the guest's white level.
    let white = number(mem, 0x0045_df00, 0)?;
    let minimum = color[0].min(color[1]).min(color[2]);
    if minimum > white {
        let factor = white / minimum.min(1.0);
        for c in &mut color[..3] {
            *c *= factor;
        }
    }
    if flags & 1 != 0 {
        let tick = f64::from(word(mem, 0x0054_d5b8, 0)?);
        let rate = f64::from(number(mem, 0x0045_e508, 0)?);
        w.alpha = (f64::from(w.alpha) * ((tick * rate).cos() + 1.0) * 0.5) as f32;
    }
    if flags & 2 != 0 {
        for unit in &mut text {
            *unit = upper(*unit);
        }
    }
    // The glyph scanner packs each channel via the truncating _ftol
    // helper after multiplying by 255 in the x87 register.
    let channel = |x: f32| -> Result<f32, Gap> {
        if !(0.0..=1.0).contains(&x) {
            return Err(Gap::text("text_colour"));
        }
        Ok(f32::from((f64::from(x) * 255.0) as u8) / 255.0)
    };
    for c in &mut color[..3] {
        *c = channel(*c)?;
    }
    w.alpha = channel(w.alpha)?;
    let font = u16::try_from(word(mem, iface, 4)?).map_err(|_| Gap::text("text_font"))?;
    w.content = UiContent::Text {
        font,
        text,
        color,
        justify,
        shadow: flags & 4 == 0,
        layout,
    };
    Ok(w)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    struct Mem(BTreeMap<u32, u32>);
    impl GuestRead for Mem {
        fn read_u32(&self, addr: u32) -> Option<u32> {
            self.0.get(&addr).copied()
        }
    }
    #[test]
    fn string_requires_terminator_inside_the_selected_buffer() {
        let m = Mem([(0x144, 0x00620061), (0x148, 0)].into());
        assert_eq!(string(&m, 0x100, 3), Ok(vec![97, 98]));
        assert_eq!(string(&m, 0x100, 2), Err(Unread::Unreadable));
        assert_eq!(string(&m, u32::MAX, 3), Err(Unread::Unreadable));
    }

    #[test]
    fn label_uses_interface_font_widget_tint_and_truncated_offsets() {
        let w = 0x1000;
        let iface = w + 0x74;
        let mut m = Mem(BTreeMap::new());
        for (addr, value) in [
            (w, 0x2000),
            (0x203c, 0x0022_f57f),
            (iface, 0x3000),
            (0x3004, 0x0022_f50d),
            (0x3008, 0x0022_f52e),
            (0x0054_d59c, 0),
            (0x0051_080c, 0),
            (0x0048_5600, 0),
            (0x004b_62a0, 1),
            (0x004b_62ac, 0x5000),
            (0x0045_df00, 0.5f32.to_bits()),
            (0x0051_ec04, 0),
            (0x004e_73fc, 0),
            (0x004e_7400, 0),
            (0x0054_d5b0, 1f32.to_bits()),
            (0x0045_dbbc, 0.5f32.to_bits()),
            (0x0045_dbc0, 1f32.to_bits()),
            (0x0045_dbdc, 0.001f32.to_bits()),
            (0x0045_dbd8, 0f32.to_bits()),
            (0x0045_dbcc, (-0.001f32).to_bits()),
            (w + 4, 6),
            (w + 0x20, 0),
            (w + 0x24, 0x0010_0010),
            (w + 0x58, 1.75f32.to_bits()),
            (w + 0x5c, (-3.5f32).to_bits()),
            (w + 0x60, 0),
            (w + 0x64, 0.75f32.to_bits()),
            (w + 0x68, 0),
            (iface + 4, 3),
            (iface + 0x14, 0x0001_0004),
            (iface + 0x18, u32::MAX),
            (iface + 0x1c, 1),
            (iface + 0x20, 1f32.to_bits()),
            (iface + 0x38, 0),
            (iface + 0x3c, 0xffff),
            (iface + 0x44, 98),
        ] {
            m.0.insert(addr, value);
        }
        for i in 0..3 {
            m.0.insert(w + 0x28 + 4 * i, 0.25f32.to_bits());
            m.0.insert(iface + 8 + 4 * i, 0.5f32.to_bits());
        }
        for i in 0..18 {
            m.0.insert(0x0044_a3f0 + i * 8, 0x4000);
            m.0.insert(0x0044_a3f4 + i * 8, 0xe100);
        }
        m.0.insert(0x4000, 97);
        let widget = read_widget(&m, w, 2.0, false).unwrap();
        assert_eq!(widget.rect, [-3, 1, 13, 17]);
        assert_eq!(widget.alpha, 191.0 / 255.0);
        assert_eq!(
            widget.content,
            UiContent::Text {
                font: 3,
                text: vec![98],
                color: [31.0 / 255.0, 31.0 / 255.0, 31.0 / 255.0, 1.0],
                justify: Justify::Right,
                shadow: false,
                layout: TextLayout::default(),
            }
        );
        m.0.insert(iface + 0x14, 0x0001_0001);
        m.0.insert(0x0054_d5b8, 1);
        m.0.insert(0x0045_e508, std::f32::consts::FRAC_PI_2.to_bits());
        let flash = read_widget(&m, w, 2.0, false).unwrap();
        assert_eq!(flash.alpha, 95.0 / 255.0);
        assert!(matches!(
            flash.content,
            UiContent::Text { shadow: true, .. }
        ));
        // The reveal cursor at 0 shows only its glyph; no shared UI
        // globals: `0xdb`.
        m.0.insert(0x004e_0350, 0);
        m.0.insert(0x004e_034c, 0);
        m.0.insert(iface + 0x3c, 0);
        m.0.insert(iface + 0x40, 0);
        let revealing = read_widget(&m, w, 2.0, false).unwrap();
        assert!(matches!(&revealing.content, UiContent::Text { text, .. } if text == &[0xdb]));
        m.0.insert(iface + 0x3c, 0xffff);
        // Two stops, each plus the projected rectangle's left (1 + 320).
        m.0.insert(iface + 0x38, 2);
        m.0.insert(iface + 0x28, 0xfffe_0010);
        m.0.insert(0x004e_73fc, 5 << 16);
        m.0.insert(0x004e_7400, 0xfffd);
        let tabbed = read_widget(&m, w, 2.0, false).unwrap();
        assert!(matches!(
            &tabbed.content,
            UiContent::Text { layout, .. } if *layout == TextLayout { scale: 1.0, stops: vec![337, 319], indents: [5, -3] }
        ));
        m.0.insert(iface + 0x38, 9);
        assert_eq!(read_widget(&m, w, 2.0, false), Err(Gap::text("text_tab_overrun")));
        m.0.insert(iface + 0x38, -1i32 as u32);
        assert!(matches!(read_widget(&m, w, 2.0, false).unwrap().content, UiContent::Text { layout, .. } if layout.stops.is_empty()));
        // Behind the camera's plane by 1: half the size about the centre,
        // the rectangle truncated and the advance scale its width's.
        m.0.insert(iface + 0x38, 0);
        m.0.insert(w + 0x60, 1f32.to_bits());
        let far = read_widget(&m, w, 2.0, false).unwrap();
        assert_eq!(far.rect, [-1, 0, 7, 8]);
        assert!(matches!(far.content, UiContent::Text { layout, .. } if layout.scale == 0.5));
        // No width: the draw returns before the line renderer.
        m.0.insert(w + 0x60, 0);
        m.0.insert(w + 0x24, 0x0000_0010);
        assert_eq!(read_widget(&m, w, 2.0, false).unwrap().content, UiContent::None);
    }

    #[test]
    fn the_reveal_cursor_cuts_the_string_at_its_glyph() {
        let units = |s: &str| s.encode_utf16().collect::<Vec<_>>();
        let at = |cursor| Reveal { cursor, end: 9, last: 0 };
        let shown = |cursor| {
            let mut text = units("abc");
            at(cursor).show(&mut text, 0x5f);
            text
        };
        assert_eq!(shown(1), units("a_"));
        assert_eq!(shown(3), units("abc_"), "at the terminator: the glyph follows");
        assert_eq!(shown(4), units("abc"), "past the end: the terminator comes first");
        assert_eq!(shown(-1), units("abc"), "off");
        assert_eq!(at(0x7fff).at(), Some(REVEAL_LAST), "clamped to the copy");
    }

    #[test]
    fn the_reveal_cursor_steps_once_its_ticks_pass_and_stops_at_its_end() {
        let r = |cursor, last| Reveal { cursor, end: 3, last };
        assert_eq!(r(-1, 5).step(100, 50), r(-1, 5), "off: nothing written");
        assert_eq!(r(1, 0).step(100, 50), r(1, 100), "the first draw stamps the clock");
        assert_eq!(r(1, 60).step(100, 50), r(1, 60), "too soon");
        assert_eq!(r(1, 50).step(100, 50), r(2, 100));
        assert_eq!(r(2, 50).step(100, 50), r(-1, 100), "its end turns it off");
        assert_eq!(r(1, 200).step(100, 50), r(2, 100), "the clock's distance is unsigned");
        assert_eq!(r(0x7fff, 0).step(7, 50), r(REVEAL_LAST, 7), "the clamped cursor is written back");
        // Ticks read as an `i16` and compared unsigned: a negative never passes.
        assert_eq!(r(1, 1).step(100, -1i16 as i32 as u32), r(1, 1));
    }

    #[test]
    fn a_left_out_draw_writes_its_reveal_step() {
        use engine::testing::FakeGuest;
        let (w, iface) = (0x1000, 0x1074);
        let mut g = FakeGuest::new();
        g.put_u32(w, 0x2000)
            .put_u32(0x203c, 0x0022_f57f)
            .put_u32(iface, 0x3000)
            .put_u32(0x3004, 0x0022_f50d)
            .put_u32(0x3008, 0x0022_f52e)
            .put_u32(w + 0x24, 0x0010_0010)
            .put_u16(iface + 0x44, 0x61)
            .put_u16(iface + 0x3c, 0)
            .put_u16(iface + 0x3e, 2)
            .put_u32(iface + 0x40, 10)
            .put_u32(0x004e_0350, 0)
            .put_u32(0x004e_034c, 0)
            .put_u32(UI_CLOCK, 100);
        left_out(&mut g, w, false);
        assert_eq!((g.u16(iface + 0x3c).unwrap(), g.u32(iface + 0x40).unwrap()), (1, 100));
        left_out(&mut g, w, false);
        assert_eq!(g.u16(iface + 0x3c).unwrap(), 1, "fewer than 50 ticks since");
        g.put_u32(UI_CLOCK, 150);
        left_out(&mut g, w, false);
        assert_eq!(g.u16(iface + 0x3c).unwrap(), 0xffff, "its end turns it off");
        // A zero-width rectangle: the draw returns before the cursor.
        g.put_u16(iface + 0x3c, 0).put_u32(w + 0x24, 0x0000_0010);
        left_out(&mut g, w, false);
        assert_eq!(g.u32(iface + 0x40).unwrap(), 150, "nothing written");
    }

    #[test]
    fn upper_case_is_basic_latin_and_two_accented_vowels() {
        let units = |s: &str| s.encode_utf16().collect::<Vec<_>>();
        let up = |s: &str| units(s).into_iter().map(upper).collect::<Vec<_>>();
        assert_eq!(up("az{AZ09"), units("AZ{AZ09"));
        assert_eq!(up("éó"), units("ÉÓ"));
        assert_eq!(up("àüñ"), units("àüñ"), "the rest of Latin-1 is left as it is");
        assert_eq!(upper(0xe105), 0xe105, "a button glyph");
    }

    /// Aliases at 0x5000 ("ab" to 0xe101), 0x5100 ("a" to 0xe102) and the
    /// rest empty-free ("zz" to 0xe103); one token, 0xe120, with a handler,
    /// and one button glyph, 0xe105, with none.
    fn aliases() -> Mem {
        let mut m = Mem(BTreeMap::new());
        m.0.insert(0x5000, 0x0062_0061);
        m.0.insert(0x5004, 0);
        m.0.insert(0x5100, 0x61);
        m.0.insert(0x5200, 0x007a_007a);
        m.0.insert(0x5204, 0);
        for i in 0..18 {
            let (name, unit) = match i {
                0 => (0x5000, 0xe101),
                1 => (0x5100, 0xe102),
                _ => (0x5200, 0xe103),
            };
            m.0.insert(0x0044_a3f0 + i * 8, name);
            m.0.insert(0x0044_a3f4 + i * 8, unit);
        }
        for i in 0..TOKEN_COUNT {
            m.0.insert(TOKENS + i * 12, 0xe105 + i);
            m.0.insert(TOKENS + i * 12 + 4, 0);
        }
        m.0.insert(TOKENS + 0x1b * 12 + 4, 0x0022_e0df);
        m.0.insert(TOKENS + 0x1c * 12, 0xe459);
        m.0.insert(TOKENS + 0x1c * 12 + 4, 0x0022_e1ed);
        m.0.insert(UI_CLOCK, 170);
        m
    }

    #[test]
    fn a_named_sequence_becomes_its_unit_and_the_last_match_wins() {
        let m = aliases();
        let units = |s: &str| s.encode_utf16().collect::<Vec<_>>();
        // At "ab" both "ab" and "a" match; "a" is later in the table.
        assert_eq!(substitute(&m, &units("xaby")), Ok(vec![0x78, 0xe102, 0x62, 0x79]));
        assert_eq!(substitute(&m, &units("zzz")), Ok(vec![0xe103, 0x7a]), "the scan moves past the replacement");
        assert_eq!(substitute(&m, &units("q")), Ok(units("q")));
    }

    #[test]
    fn a_button_glyph_stays_and_a_token_is_written() {
        let m = aliases();
        assert_eq!(substitute(&m, &[0xe105, 0x20]), Ok(vec![0xe105, 0x20]));
        // The handler's text replaces the unit and the scan moves on one.
        assert_eq!(substitute(&m, &[0xe459, 0x61, 0x62]), Ok(vec![0xe12d, 0xe102, 0x62]));
        assert_eq!(substitute(&m, &[0xf000]), Ok(vec![0xf000]), "not in the table: no handler");
    }
}
