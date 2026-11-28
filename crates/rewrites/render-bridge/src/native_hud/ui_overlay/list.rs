//! Scroll arrows and button art. Row children already have their laid-out
//! rectangles, bindings and animated highlight in the guest's sorted walk.
use frame_scene::{Combiner, OverlayBlend, OverlayTexture, Sampler};
use scene_extract::{
    GuestRead,
    ui::{UiBitmap, Unread},
};

use super::Gap;

type Read<T> = Result<T, Gap>;

struct Reader<'a>(&'a dyn GuestRead);

impl Reader<'_> {
    fn word(&self, p: u32, off: u32) -> Read<u32> {
        if p == 0 {
            return Err(Unread::Unreadable.into());
        }
        Ok(self
            .0
            .read_u32(p.checked_add(off).ok_or(Unread::Unreadable)?)
            .ok_or(Unread::Unreadable)?)
    }

    fn short(&self, p: u32, off: u32) -> Read<i16> {
        let a = p.checked_add(off).ok_or(Unread::Unreadable)?;
        Ok((self.word(a & !3, 0)? >> ((a & 2) * 8)) as i16)
    }

    fn float(&self, p: u32, off: u32) -> Read<f32> {
        let f = f32::from_bits(self.word(p, off)?);
        if f.is_finite() {
            Ok(f)
        } else {
            Err(Gap::class("list_number"))
        }
    }

    fn tag(&self, datum: u32) -> Read<u32> {
        if datum == u32::MAX {
            return Err(Gap::class("list_tag"));
        }
        self.word(self.word(0x004e_3b44, 0)?, (datum & 0xffff) * 16 + 8)
    }

    fn collection(&self) -> Read<u32> {
        let scenario = self.word(0x004e_0350, 0)?;
        let globals = self.word(0x004e_034c, 0)?;
        if self.word(globals, 0x110)? == 0 {
            return Err(Gap::class("list_globals"));
        }
        let interface = self.word(globals, 0x114)?;
        let off = match self.short(scenario, 0x10)? {
            0 => 0x8c,
            1 => 0x94,
            2 => 0x84,
            _ => return Err(Gap::class("list_globals")),
        };
        self.tag(self.word(interface, off)?)
    }

    /// Runtime classes can override screen-tag lookup and arrow placement.
    /// Those classes use vtables allocated outside the image's UI tables.
    fn retail_table(&self, widget: u32) -> Read<u32> {
        let table = self.word(widget, 0)?;
        if !(0x0045_8700..0x0047_0000).contains(&table) {
            return Err(Gap::class("draw_runtime_class"));
        }
        Ok(table)
    }

    fn screen(&self, root: u32) -> Read<u32> {
        self.retail_table(root)?;
        let id = self.word(root, 0x70)? as i32;
        let group = self.collection()?;
        let count = self.word(group, 8)?;
        if count > 1024 {
            return Err(Gap::class("list_screen"));
        }
        let base = self.word(group, 12)?;
        for i in 0..count {
            let tag = self.word(base, i * 8 + 4)?;
            if tag != u32::MAX {
                let screen = self.tag(tag)?;
                if i32::from(self.short(screen, 4)?) == id {
                    return Ok(screen);
                }
            }
        }
        // Failed guest lookup can write its missing id at 0x00148112.
        Err(Gap::class("list_screen"))
    }

    fn ancestor(&self, mut at: u32, needle: Option<u32>) -> Read<Option<u32>> {
        for _ in 0..256 {
            if at == 0 {
                return Ok(None);
            }
            if Some(at) == needle {
                return Ok(Some(at));
            }
            let parent = self.word(at, 0x10)?;
            if parent == 0 && needle.is_none() {
                return Ok(Some(at));
            }
            at = parent;
        }
        Err(Gap::class("list_tree"))
    }

    fn arrows_visible(&self, list: u32) -> Read<[bool; 2]> {
        let first = self.word(list, 0x14)?;
        let array = self.word(list, 0x70)?;
        if first == 0 || array == 0 {
            return Ok([false; 2]);
        }
        let mut last = first;
        for _ in 0..256 {
            let next = self.word(last, 0x18)?;
            if next == 0 {
                let limit = self.word(array, 0x38)?;
                if limit > u32::from(u16::MAX) {
                    return Err(Gap::class("list_rows"));
                }
                let first_key = self.word(first, 0x70)?;
                let last_key = self.word(last, 0x70)?;
                let mut up = false;
                let mut down = false;
                if first_key != u32::MAX {
                    let end = first_key & 0xffff;
                    if end > limit {
                        return Err(Gap::class("list_rows"));
                    }
                    let stride = self.word(array, 0x24)?;
                    let rows = self.word(array, 0x44)?;
                    if stride < 2 || stride & 1 != 0 {
                        return Err(Gap::class("list_rows"));
                    }
                    for i in (0..end).rev() {
                        if self.short(rows, i.checked_mul(stride).ok_or(Unread::Unreadable)?)? != 0
                        {
                            up = true;
                            break;
                        }
                    }
                    // The reverse helper probes one more row at index -1
                    // on exhaustion; the handle it forms with -1 is -1.
                }
                if last_key != u32::MAX {
                    let begin = (last_key & 0xffff) + 1;
                    let mask = self.word(array, 0x48)?;
                    for i in begin..limit {
                        if self.word(mask, (i / 32) * 4)? & (1 << (i % 32)) != 0 {
                            down = true;
                            break;
                        }
                    }
                }
                return Ok([up, down]);
            }
            last = next;
        }
        Err(Gap::class("list_tree"))
    }

    fn focused(&self, button: u32, root: u32) -> Read<bool> {
        let table = self.word(root, 0)?;
        let bt = self.word(button, 0)?;
        if self.word(bt, 0x30)? != 0x0022_ec9e
            || self.word(bt, 0x2c)? != 0x0022_ec90
            || self.word(table, 0x50)? != 0x0023_012c
            || self.word(table, 0x54)? != 0x0023_0130
        {
            return Err(Gap::class("button_focus"));
        }
        let user = self.word(root, 0x78)?;
        if user > 4 {
            return Err(Gap::class("button_focus"));
        }
        let channel = match self.word(root, 0x74)? {
            0 if user == 4 => 0x0054_d99c,
            1 => 0x0054_d884 + 0x38 * user,
            2 if user == 4 => 0x0054_dba8,
            3 => 0x0054_d76c + 0x38 * user,
            4 if user == 4 => 0x0054_d9e0,
            5 => 0x0054_d62c + 0x40 * user,
            _ => return Err(Gap::class("button_focus")),
        };
        Ok(self
            .ancestor(self.word(channel, 0x34)?, Some(button))?
            .is_some())
    }

    fn image(
        &self,
        widget: u32,
        viewport: u32,
        tag: u32,
        image: u16,
        anchor: [f32; 2],
        alpha: f32,
    ) -> Read<UiBitmap> {
        if self.word(viewport, 0)? != 0 || self.word(viewport, 4)? != (640 << 16 | 480) {
            return Err(Gap::class("art_viewport"));
        }
        let bitm = self.tag(tag)?;
        if u32::from(image) >= self.word(bitm, 0x44)? {
            return Err(Gap::new(Unread::Bitmap, "art_image"));
        }
        let record = self
            .word(bitm, 0x48)?
            .checked_add(u32::from(image) * 0x74)
            .ok_or(Unread::Unreadable)?;
        // 0x12360 can request a texture on a miss. Only replace the
        // resident fast path, which returns the existing pointer directly.
        if self.word(record, 0x70)? as i32 <= self.word(0x004e_6488, 0)? as i32
            || self.word(record, 0x50)? == 0
            || self.short(0x0048_5602, 0)? != 0
        {
            return Err(Gap::new(Unread::Bitmap, "art_texture"));
        }
        let width = self.short(record, 4)?;
        let height = self.short(record, 6)?;
        if width <= 0 || height <= 0 || !(0.0..=1.0).contains(&alpha) {
            return Err(Gap::new(Unread::Bitmap, "art_image"));
        }
        let depth = self.float(widget, 0x60)?;
        let mut camera = self.float(0x0054_d5b0, 0)?;
        let one = self.float(0x0045_dbc0, 0)?;
        if camera.abs() < self.float(0x0045_dbdc, 0)? {
            camera = one;
        }
        let mut z = depth + camera;
        if z >= self.float(0x0045_dbd8, 0)? {
            if z <= one {
                z = one;
            }
        } else {
            z = z.min(self.float(0x0045_dbcc, 0)?);
        }
        let scale = one / ((one / camera) * z);
        if !scale.is_finite() || scale <= 0.0 {
            return Err(Gap::class("art_scale"));
        }
        let [x, y] = anchor;
        let w = f32::from(width);
        let h = f32::from(height);
        let rect = [
            320.0 + x * scale,
            240.0 - y * scale,
            320.0 + (x + w) * scale,
            240.0 - (y - h) * scale,
        ];
        if !rect.iter().all(|v| v.is_finite()) {
            return Err(Gap::class("art_scale"));
        }
        let byte = (alpha * self.float(0x0045_dc38, 0)?).round_ties_even();
        Ok(UiBitmap {
            rect,
            texture: OverlayTexture::Bitmap { tag, image },
            uv: [0.0, 0.0, 1.0, 1.0],
            color: [1.0, 1.0, 1.0, byte / 255.0],
            combiner: Combiner::Texture,
            sampler: Sampler::default(),
            blend: OverlayBlend::On {
                source: self.word(0x0045_0718, 0)?,
                dest: self.word(0x0045_06e8, 0)?,
                equation: self.word(0x0045_06b8, 0)?,
            },
        })
    }
}

/// The list draw's scroll arrows (`0x0024bda2`): each from the list's
/// animated left and bottom, moved by the skin's offsets, or, for a class
/// the host registered at run time, by the offsets `placed` gives for its
/// vtable and height (the game's `list-scroll-arrows` hook moves the
/// game's quads the same way). A runtime class `placed` does not know is
/// the game's (`draw_runtime_class`).
pub(super) fn arrows(mem: &dyn GuestRead, widget: u32, viewport: u32, placed: super::ListArrows) -> Read<Vec<UiBitmap>> {
    let r = Reader(mem);
    if r.short(widget, 0x42)? & 3 != 0 {
        return Ok(Vec::new());
    }
    let root = r.word(widget, 0x10)?;
    let mut skin_index = 0;
    if root != 0 {
        let screen = r.screen(root)?;
        let index = r.short(root, 0x5f0)?;
        if index < -1 {
            return Err(Gap::class("list_pane"));
        }
        if index >= 0 && i32::from(index) < r.word(screen, 0x20)? as i32 {
            let pane = r
                .word(screen, 0x24)?
                .checked_add(index as u32 * 0x4c)
                .ok_or(Unread::Unreadable)?;
            if r.word(pane, 12)? as i32 > 0 {
                skin_index = r.short(r.word(pane, 0x10)?, 4)?;
            }
        }
    }
    let visible = r.arrows_visible(widget)?;
    if visible == [false; 2] || skin_index < 0 {
        return Ok(Vec::new());
    }
    let class = match r.retail_table(widget) {
        Ok(_) => None,
        Err(gap) => {
            let height = i32::from(r.short(widget, 0x24)?) - i32::from(r.short(widget, 0x20)?);
            Some(placed(r.word(widget, 0)?, height).ok_or(gap)?)
        }
    };
    let globals = r.tag(r.word(r.collection()?, 4)?)?;
    if i32::from(skin_index) >= r.word(globals, 0x138)? as i32 {
        return Ok(Vec::new());
    }
    let skin_tag = r.word(r.word(globals, 0x13c)?, skin_index as u32 * 8 + 4)?;
    if skin_tag == u32::MAX {
        return Ok(Vec::new());
    }
    let skin = r.tag(skin_tag)?;
    let tag = r.word(skin, 8)?;
    if tag == u32::MAX {
        return Ok(Vec::new());
    }
    let animated_edge = |edge, offset| -> Read<i16> {
        let delta = r.float(widget, offset)?;
        if !(-2147483648.0..2147483648.0).contains(&delta) {
            return Err(Gap::class("art_offset"));
        }
        Ok(r.short(widget, edge)?.wrapping_add(delta as i32 as i16))
    };
    let x = animated_edge(0x22, 0x58)?;
    let y = animated_edge(0x24, 0x5c)?;
    let mut result = Vec::new();
    for (i, show) in visible.into_iter().enumerate() {
        if show {
            let off = 12 + 4 * i as u32;
            let [dx, dy] = match class {
                Some(offsets) => offsets[i],
                None => [r.short(skin, off)?, r.short(skin, off + 2)?],
            };
            let anchor = [f32::from(x) + f32::from(dx), f32::from(y) + f32::from(dy)];
            let alpha = r.float(
                if r.short(widget, 0x74 + 2 * i as u32)? == 0 {
                    0x0045_dbbc
                } else {
                    0x0045_dbc0
                },
                0,
            )?;
            result.push(r.image(widget, viewport, tag, i as u16, anchor, alpha)?);
        }
    }
    Ok(result)
}

pub(super) fn button_art(
    mem: &dyn GuestRead,
    widget: u32,
    viewport: u32,
) -> Read<Option<UiBitmap>> {
    let r = Reader(mem);
    if (r.word(0x0054_d59c, 0)? >> 24) != 0 {
        // The widget index display is on.
        return Err(Gap::class("button_debug"));
    }
    let root = r.ancestor(widget, None)?.ok_or(Gap::class("list_tree"))?;
    if root == widget && r.word(widget, 4)? != 0 {
        return Ok(None);
    }
    let screen = r.screen(root)?;
    if r.word(screen, 0x20)? as i32 <= 0 {
        return Ok(None);
    }
    let pane = r.word(screen, 0x24)?;
    let index = r.short(widget, 0xf8)?;
    if index < 0 || i32::from(index) >= r.word(pane, 4)? as i32 {
        return Ok(None);
    }
    let def = r
        .word(pane, 8)?
        .checked_add(index as u32 * 0x3c)
        .ok_or(Unread::Unreadable)?;
    let tag = r.word(def, 0x28)?;
    if tag == u32::MAX {
        return Ok(None);
    }
    if r.word(r.word(widget, 0)?, 0x44)? != 0x0025_4173 {
        return Err(Gap::class("button_class"));
    }
    let count = r.word(r.tag(tag)?, 0x44)?;
    let selected = count > 1 && r.focused(widget, root)?;
    let x =
        f32::from(r.short(widget, 0x22)?) + r.float(widget, 0x58)? + f32::from(r.short(def, 0x2c)?);
    let y =
        f32::from(r.short(widget, 0x20)?) + r.float(widget, 0x5c)? - f32::from(r.short(def, 0x2e)?);
    Ok(Some(r.image(
        widget,
        viewport,
        tag,
        u16::from(selected),
        [x, y],
        r.float(widget, 0x64)?,
    )?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use scene_extract::testing::Mem;

    #[test]
    fn sparse_rows_control_the_two_arrows_independently() {
        let mut m = Mem::default();
        let (list, first, last, array, rows, mask) =
            (0x1000, 0x2000, 0x2100, 0x3000, 0x4000, 0x5000);
        m.w(list + 0x14, first);
        m.w(list + 0x70, array);
        m.w(first + 0x18, last);
        m.w(last + 0x18, 0);
        m.w(first + 0x70, 0x1234_0002);
        m.w(last + 0x70, 0x2345_0004);
        m.w(array + 0x38, 7);
        m.w(array + 0x24, 8);
        m.w(array + 0x44, rows);
        m.w(array + 0x48, mask);
        m.w(rows, 0x39);
        m.w(rows + 8, 0);
        m.w(mask, 1 << 6);
        assert_eq!(Reader(&m).arrows_visible(list), Ok([true, true]));
        m.w(rows, 0);
        assert_eq!(Reader(&m).arrows_visible(list), Ok([false, true]));
        m.w(last + 0x70, u32::MAX);
        assert_eq!(Reader(&m).arrows_visible(list), Ok([false, false]));
        m.w(last + 0x18, first);
        assert_eq!(Reader(&m).arrows_visible(list), Err(Gap::class("list_tree")));
    }

    #[test]
    fn selected_image_follows_the_channel_focus_up_its_parent_chain() {
        let mut m = Mem::default();
        let (button, root, bt, rt, leaf) = (0x1000, 0x2000, 0x3000, 0x4000, 0x5000);
        m.w(button, bt);
        m.w(root, rt);
        m.w(bt + 0x30, 0x0022_ec9e);
        m.w(bt + 0x2c, 0x0022_ec90);
        m.w(rt + 0x50, 0x0023_012c);
        m.w(rt + 0x54, 0x0023_0130);
        m.w(root + 0x74, 1);
        m.w(root + 0x78, 2);
        m.w(0x0054_d884 + 2 * 0x38 + 0x34, leaf);
        m.w(leaf + 0x10, button);
        assert_eq!(Reader(&m).focused(button, root), Ok(true));
        m.w(leaf + 0x10, root);
        m.w(root + 0x10, 0);
        assert_eq!(Reader(&m).focused(button, root), Ok(false));
    }

    #[test]
    fn art_uses_the_resident_image_dimensions_and_packed_alpha() {
        let mut m = Mem::default();
        let (widget, viewport, table, tag, base) = (0x1000, 0x2000, 0x3000, 0x4000, 0x5000);
        let image = base + 0x74;
        m.w(viewport, 0);
        m.w(viewport + 4, 640 << 16 | 480);
        m.w(0x004e_3b44, table);
        m.w(table + 3 * 16 + 8, tag);
        m.w(tag + 0x44, 2);
        m.w(tag + 0x48, base);
        m.w(image + 4, 37 | 13 << 16);
        m.w(image + 0x70, 8);
        m.w(image + 0x50, 0x6000);
        m.w(0x004e_6488, 7);
        m.w(0x0048_5600, 0);
        m.f(widget + 0x60, 0.0);
        m.f(0x0054_d5b0, 100.0);
        m.f(0x0045_dbc0, 1.0);
        m.f(0x0045_dbdc, 0.25);
        m.f(0x0045_dbd8, 0.0);
        m.f(0x0045_dc38, 255.0);
        for addr in [0x0045_0718, 0x0045_06e8, 0x0045_06b8] {
            m.w(addr, 1);
        }
        let read = |m: &Mem| Reader(m).image(widget, viewport, 0xcafe_0003, 1, [11.0, 20.0], 0.5);
        let art = read(&m).unwrap();
        assert_eq!(
            art.texture,
            OverlayTexture::Bitmap {
                tag: 0xcafe_0003,
                image: 1
            }
        );
        assert_eq!(art.rect, [331.0, 220.0, 368.0, 233.0]);
        assert_eq!(art.color, [1.0, 1.0, 1.0, 128.0 / 255.0]);
        m.w(image + 0x70, 7);
        assert_eq!(read(&m), Err(Gap::new(Unread::Bitmap, "art_texture")));
    }

    /// A list whose class the host registered at run time: its arrows
    /// come from the skin's images, placed by the host's offsets.
    #[test]
    fn a_runtime_list_class_places_its_arrows_by_the_hosts_offsets() {
        let mut m = Mem::default();
        let (widget, viewport, tags, base) = (0x1000, 0x8000, 0x7000, 0x5800);
        m.w(widget, 0x0009_0000);
        m.w(widget + 0x40, 0);
        m.w(widget + 0x10, 0);
        // Rows above the first and below the last: both arrows.
        let (first, array) = (0x2000, 0x3000);
        m.w(widget + 0x14, first);
        m.w(widget + 0x70, array);
        m.w(first + 0x18, 0);
        m.w(first + 0x70, 2);
        m.w(array + 0x38, 7);
        m.w(array + 0x24, 8);
        m.w(array + 0x44, 0x4000);
        m.w(array + 0x48, 0x5000);
        m.w(0x4000, 0x39);
        m.w(0x4008, 0);
        m.w(0x5000, 1 << 6);
        // The rectangle: top -50, left 10, bottom 30.
        m.w(widget + 0x20, 0x000a_ffce);
        m.w(widget + 0x24, 0x003c_001e);
        m.f(widget + 0x58, 0.0);
        m.f(widget + 0x5c, 0.0);
        m.f(widget + 0x60, 0.0);
        m.w(widget + 0x74, 0);
        // The interface's skins: tags 5 (the collection), 6 (its globals),
        // 7 (the skin) and 3 (its bitmap).
        m.w(0x004e_3b44, tags);
        for (datum, data) in [(3, 0x7500), (5, 0x7100), (6, 0x7200), (7, 0x7400)] {
            m.w(tags + datum * 16 + 8, data);
        }
        m.w(0x004e_0350, 0x6000);
        m.w(0x004e_034c, 0x6100);
        m.w(0x6010, 0);
        m.w(0x6100 + 0x110, 1);
        m.w(0x6100 + 0x114, 0x6200);
        m.w(0x6200 + 0x8c, 5);
        m.w(0x7104, 6);
        m.w(0x7200 + 0x138, 1);
        m.w(0x7200 + 0x13c, 0x7300);
        m.w(0x7304, 7);
        m.w(0x7408, 0xcafe_0003);
        m.w(0x7500 + 0x44, 2);
        m.w(0x7500 + 0x48, base);
        for image in [base, base + 0x74] {
            m.w(image + 4, 8 | 6 << 16);
            m.w(image + 0x70, 8);
            m.w(image + 0x50, 0x6800);
        }
        m.w(0x004e_6488, 7);
        m.w(0x0048_5600, 0);
        m.w(viewport, 0);
        m.w(viewport + 4, 640 << 16 | 480);
        m.f(0x0054_d5b0, 100.0);
        m.f(0x0045_dbbc, 0.5);
        m.f(0x0045_dbc0, 1.0);
        m.f(0x0045_dbdc, 0.25);
        m.f(0x0045_dbd8, 0.0);
        m.f(0x0045_dc38, 255.0);
        for addr in [0x0045_0718, 0x0045_06e8, 0x0045_06b8] {
            m.w(addr, 1);
        }
        let placed = |vtable, height| (vtable == 0x0009_0000 && height == 80).then_some([[1, 2], [3, -4]]);
        let art = arrows(&m, widget, viewport, &placed).unwrap();
        // Up at the left plus 1 and the bottom plus 2; down plus 3 and
        // less 4, about the canvas's centre with Y down.
        assert_eq!(art.iter().map(|a| [a.rect[0], a.rect[1]]).collect::<Vec<_>>(), [[331.0, 208.0], [333.0, 214.0]]);
        assert_eq!(arrows(&m, widget, viewport, &|_, _| None), Err(Gap::class("draw_runtime_class")));
    }
}
