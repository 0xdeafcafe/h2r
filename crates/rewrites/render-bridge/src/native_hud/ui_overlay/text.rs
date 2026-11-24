//! Screen text and button labels. Retail entry points and field evidence are
//! recorded in docs/graphics/notes/2026-11-24-menu-text.md.

use scene_extract::GuestRead;
use scene_extract::ui::{self, Justify, UiContent, UiWidget, Unread};

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

fn interface(mem: &dyn GuestRead, widget: u32, button: bool) -> Result<(u32, u32), Unread> {
    let getter = word(mem, word(mem, widget, 0)?, 0x3c)?;
    let offset = match (button, getter) {
        (true, 0x0023_7607) => 0x70,
        (false, 0x0022_f57f) => 0x74,
        _ => return Err(Unread::Text),
    };
    let iface = widget.checked_add(offset).ok_or(Unread::Unreadable)?;
    let vt = word(mem, iface, 0)?;
    if word(mem, vt, 8)? != 0x0022_f52e {
        return Err(Unread::Text);
    }
    let capacity = match word(mem, vt, 4)? {
        0x0022_f50d => 32,
        0x0022_f545 => 256,
        _ => return Err(Unread::Text),
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

// A matched named replacement or private-use unit needs the guest's
// substitution routine. Compare the guest's names without copying its table.
fn requires_substitution(mem: &dyn GuestRead, text: &[u16]) -> Result<bool, Unread> {
    if text.iter().any(|c| (0xe000..=0xf8ff).contains(c)) {
        return Ok(true);
    }
    for i in 0..18 {
        let ptr = word(mem, 0x0044_a3f0, i * 8)?;
        let first = half(mem, ptr, 0)?;
        for (start, _) in text.iter().enumerate().filter(|(_, c)| **c == first) {
            let mut n = 1;
            loop {
                let next = half(mem, ptr, n * 2)?;
                if next == 0 {
                    return Ok(true);
                }
                if text.get(start + n as usize) != Some(&next) {
                    break;
                }
                n += 1;
            }
        }
    }
    Ok(false)
}

/// The caller checks the full-canvas viewport and, for a button, the optional bitmap.
pub(super) fn read_widget(
    mem: &dyn GuestRead,
    widget: u32,
    depth: f32,
    button: bool,
) -> Result<UiWidget, Unread> {
    let mut w = ui::read_widget_header(mem, widget, depth, UiContent::None)?;
    if word(mem, 0x0054_d59c, 0)? >> 24 != 0
        || word(mem, 0x0051_ec04, 0)? as u8 != 0
        || half(mem, 0x004e_73dc, 0)? != 0
    {
        return Err(Unread::Text);
    }
    let (iface, capacity) = interface(mem, widget, button)?;
    let mut text = string(mem, iface, capacity)?;
    if text.is_empty() {
        return Ok(w);
    }
    if word(mem, 0x0051_080c, 0)? as u8 != 0
        || half(mem, 0x0048_5602, 0)? != 0
        || word(mem, 0x004b_62a0, 0)? as u8 == 0
        || word(mem, 0x004b_62ac, 0)? == 0
    {
        return Err(Unread::Text);
    }
    let flags = half(mem, iface, 0x14)?;
    if flags & !7 != 0
        || half(mem, iface, 0x16)? != 1
        || word(mem, iface, 0x18)? != u32::MAX
        || number(mem, iface, 0x20)? != 1.0
        || word(mem, iface, 0x38)? != 0
        || (half(mem, iface, 0x3c)? as i16) >= 0
        || requires_substitution(mem, &text)?
    {
        return Err(Unread::Text);
    }
    let camera = number(mem, 0x0054_d5b0, 0)?;
    let epsilon = number(mem, 0x0045_dbdc, 0)?;
    let camera = if camera.abs() < epsilon { 1.0 } else { camera };
    let z = camera + number(mem, widget, 0x60)?;
    let z = if z >= 0.0 { z.max(1.0) } else { z.min(-1.0) };
    let scale = 1.0 / ((1.0 / camera) * z);
    if scale != 1.0 {
        return Err(Unread::Text);
    }
    for (i, delta) in w.offset.into_iter().enumerate() {
        if !delta.is_finite() || delta.abs() >= i32::MAX as f32 {
            return Err(Unread::Text);
        }
        let first = 1 - i;
        for edge in [first, first + 2] {
            w.rect[edge] = w.rect[edge].wrapping_add(delta as i32 as i16);
        }
    }
    w.offset = [0.0; 2];
    let justify = match word(mem, iface, 0x1c)? {
        0 => Justify::Left,
        1 => Justify::Right,
        2 => Justify::Centre,
        _ => return Err(Unread::Text),
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
            let ascii = u8::try_from(*unit)
                .ok()
                .filter(|c| c.is_ascii())
                .ok_or(Unread::Text)?;
            *unit = u16::from(ascii.to_ascii_uppercase());
        }
    }
    // The glyph scanner packs each channel via the truncating _ftol
    // helper after multiplying by 255 in the x87 register.
    let channel = |x: f32| -> Result<f32, Unread> {
        if !(0.0..=1.0).contains(&x) {
            return Err(Unread::Text);
        }
        Ok(f32::from((f64::from(x) * 255.0) as u8) / 255.0)
    };
    for c in &mut color[..3] {
        *c = channel(*c)?;
    }
    w.alpha = channel(w.alpha)?;
    let font = u16::try_from(word(mem, iface, 4)?).map_err(|_| Unread::Text)?;
    w.content = UiContent::Text {
        font,
        text,
        color,
        justify,
        shadow: flags & 4 == 0,
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
            (0x004e_73dc, 0),
            (0x0054_d5b0, 1f32.to_bits()),
            (0x0045_dbdc, 0.001f32.to_bits()),
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
        m.0.insert(iface + 0x3c, 0);
        assert_eq!(read_widget(&m, w, 2.0, false), Err(Unread::Text));
    }
}
