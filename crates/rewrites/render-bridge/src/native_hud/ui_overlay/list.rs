//! Scroll arrows and button art. Row children already have their laid-out
//! rectangles, bindings and animated highlight in the guest's sorted walk.
use frame_scene::{Combiner, OverlayBlend, OverlayTexture, Sampler};
use scene_extract::{
    GuestRead,
    ui::{UiBitmap, Unread},
};

use super::Gap;

type Read<T> = Result<T, Gap>;

/// How far a parent or sibling chain is followed. The guest's walks have
/// no bound; a chain this long is a cycle, which would hang the guest.
const CHAIN: usize = 0x1_0000;

/// Where `0x001480ff` writes a screen id it found no screen for.
pub(super) const MISSED_SCREEN: u32 = 0x0047_ff54;

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

    fn byte(&self, a: u32) -> Read<u8> {
        Ok((self.word(a & !3, 0)? >> ((a & 3) * 8)) as u8)
    }

    /// A `movsx word` at any address, odd ones included (a row stride
    /// may be odd).
    fn short(&self, p: u32, off: u32) -> Read<i16> {
        let a = p.checked_add(off).ok_or(Unread::Unreadable)?;
        if a & 3 != 3 {
            return Ok((self.word(a & !3, 0)? >> ((a & 3) * 8)) as i16);
        }
        Ok(i16::from_le_bytes([self.byte(a)?, self.byte(a.wrapping_add(1))?]))
    }

    fn float(&self, p: u32, off: u32) -> Read<f32> {
        let f = f32::from_bits(self.word(p, off)?);
        if f.is_finite() {
            Ok(f)
        } else {
            Err(Gap::class("list_number"))
        }
    }

    /// A datum's tag data from the tag table (`[0x004e3b44]`, 16 bytes a
    /// tag, the data at `+8`); `None` for the none datum, which the guest
    /// tests before each lookup.
    fn tag(&self, datum: u32) -> Read<Option<u32>> {
        if datum == u32::MAX {
            return Ok(None);
        }
        Ok(Some(self.word(self.word(0x004e_3b44, 0)?, (datum & 0xffff) * 16 + 8)?))
    }

    /// `0x001482e8`: the scenario type's screen collection (`wgtz`), or
    /// `None` (0) for no scenario, no globals, no interface tags, a type
    /// past 2 or no collection.
    fn collection(&self) -> Read<Option<u32>> {
        let scenario = self.word(0x004e_0350, 0)?;
        let globals = self.word(0x004e_034c, 0)?;
        if scenario == 0 || globals == 0 || self.word(globals, 0x110)? == 0 {
            return Ok(None);
        }
        let interface = self.word(globals, 0x114)?;
        if interface == 0 {
            return Ok(None);
        }
        let off = match self.short(scenario, 0x10)? {
            0 => 0x8c,
            1 => 0x94,
            2 => 0x84,
            _ => return Ok(None),
        };
        self.tag(self.word(interface, off)?)
    }

    /// `0x00148350`: the collection's shared UI globals (`wigl`, its
    /// `+4`), or `None`.
    fn ui_globals(&self) -> Read<Option<u32>> {
        match self.collection()? {
            Some(c) => self.tag(self.word(c, 4)?),
            None => Ok(None),
        }
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

    /// `0x0022f871`: the screen widget's `wgit` by its id (`+0x70`),
    /// through `0x001480ff` (the collection's screens searched by
    /// `0x00148098`). `Err(id)`: no screen has the id, and the lookup
    /// writes it to [`MISSED_SCREEN`] ([`missed_screen`]).
    fn screen_of(&self, root: u32) -> Read<Result<u32, u32>> {
        self.retail_table(root)?;
        let id = self.word(root, 0x70)?;
        let Some(group) = self.collection()? else { return Ok(Err(id)) };
        let count = self.word(group, 8)? as i32;
        let base = self.word(group, 12)?;
        for i in 0..count.max(0) as u32 {
            let datum = self.word(base, i.wrapping_mul(8).wrapping_add(4))?;
            if let Some(screen) = self.tag(datum)?
                && i32::from(self.short(screen, 4)?) == id as i32
            {
                return Ok(Ok(screen));
            }
        }
        Ok(Err(id))
    }

    fn screen(&self, root: u32) -> Read<Option<u32>> {
        Ok(self.screen_of(root)?.ok())
    }

    /// `0x0022eeee`: the top of `widget`'s parent chain, or, with no
    /// parent, the widget itself when it is a screen (`+4` 0), else 0.
    fn root(&self, widget: u32) -> Read<u32> {
        let mut at = self.word(widget, 0x10)?;
        if at == 0 {
            return Ok(if self.word(widget, 4)? == 0 { widget } else { 0 });
        }
        for _ in 0..CHAIN {
            let parent = self.word(at, 0x10)?;
            if parent == 0 {
                return Ok(at);
            }
            at = parent;
        }
        Err(Gap::class("list_tree"))
    }

    /// The last of `first`'s siblings (`+0x18`), as `0x0024c676` walks.
    fn last_sibling(&self, first: u32) -> Read<u32> {
        let mut last = first;
        for _ in 0..CHAIN {
            let next = self.word(last, 0x18)?;
            if next == 0 {
                return Ok(last);
            }
            last = next;
        }
        Err(Gap::class("list_tree"))
    }

    /// `0x0016bcc0`: the handle of the shown row before row `key`
    /// (rows from `+0x44`, `+0x24` apart, `+0x38` of them; a row shows
    /// when its leading `i16` is not 0). The search runs down to -1, as
    /// the guest's does, whose handle is always -1.
    fn row_before(&self, array: u32, key: u32) -> Read<u32> {
        let limit = self.word(array, 0x38)? as i32;
        let start = if key == u32::MAX { limit } else { (key & 0xffff) as i32 };
        let mut i = start.wrapping_sub(1);
        if i < 0 || i >= limit {
            return Ok(u32::MAX);
        }
        let rows = self.word(array, 0x44)?;
        let stride = self.word(array, 0x24)?;
        loop {
            let row = self.short(rows.wrapping_add(stride.wrapping_mul(i as u32)), 0)?;
            if row != 0 {
                return Ok((i32::from(row) << 16) as u32 | i as u32);
            }
            if i < 0 {
                return Ok(u32::MAX);
            }
            i -= 1;
        }
    }

    /// `0x0016bbc0`: the handle of the shown row after row `key`, by the
    /// shown mask (`+0x48`, one bit a row, `0x0016bc00`).
    fn row_after(&self, array: u32, key: u32) -> Read<u32> {
        let begin = if key == u32::MAX { 0 } else { ((key & 0xffff) + 1) as i32 };
        let limit = self.word(array, 0x38)? as i32;
        let mask = self.word(array, 0x48)?;
        for i in begin..limit.max(begin) {
            let bits = self.word(mask.wrapping_add(((i >> 5) as u32).wrapping_mul(4)), 0)?;
            if bits & (1 << (i & 31)) != 0 {
                let stride = self.word(array, 0x24)?;
                let rows = self.word(array, 0x44)?;
                let row = self.short(rows.wrapping_add(stride.wrapping_mul(i as u32)), 0)?;
                return Ok((i32::from(row) << 16) as u32 | i as u32);
            }
        }
        Ok(u32::MAX)
    }

    /// `0x0024c63e` and `0x0024c676`: whether a shown row is above the
    /// first child (`+0x14`) and below the last, by their row keys
    /// (`+0x70`) in the list's row array (`+0x70`).
    fn arrows_visible(&self, list: u32) -> Read<[bool; 2]> {
        let first = self.word(list, 0x14)?;
        let array = self.word(list, 0x70)?;
        if first == 0 || array == 0 {
            return Ok([false; 2]);
        }
        let first_key = self.word(first, 0x70)?;
        let up = first_key != u32::MAX && self.row_before(array, first_key)? != u32::MAX;
        let last_key = self.word(self.last_sibling(first)?, 0x70)?;
        let down = last_key != u32::MAX && self.row_after(array, last_key)? != u32::MAX;
        Ok([up, down])
    }

    /// A getter slot of a widget's vtable the overlay knows: `0x0023012c`
    /// (`+0x74`, the screen's channel), `0x00230130` (`+0x78`, its user).
    fn getter(&self, widget: u32, slot: u32) -> Read<u32> {
        match self.word(self.word(widget, 0)?, slot)? {
            0x0023_012c => self.word(widget, 0x74),
            0x0023_0130 => self.word(widget, 0x78),
            _ => Err(Gap::class("button_focus")),
        }
    }

    /// `0x0022ed7a`: whether the button holds its channel's focus. The
    /// button's slots `+0x30` (`0x0022ec9e`: its screen's user, 4 with no
    /// screen) and `+0x2c` (`0x0022ec90`: its screen's channel) name the
    /// channel's record (`0x00148d91`), whose focus widget (`+0x34`) is
    /// the button or one of its children.
    fn focused(&self, button: u32) -> Read<bool> {
        let root = self.root(button)?;
        if root == 0 {
            return Ok(false);
        }
        let table = self.word(button, 0)?;
        if self.word(table, 0x30)? != 0x0022_ec9e || self.word(table, 0x2c)? != 0x0022_ec90 {
            return Err(Gap::class("button_focus"));
        }
        let user = self.getter(root, 0x54)?;
        let channel = self.getter(root, 0x50)?;
        // A channel only the fourth user has is record 0 for any other,
        // read at `0x34` as the guest does.
        let only_four = |at: u32| if user == 4 { at } else { 0 };
        let record = match channel {
            0 => only_four(0x0054_d99c),
            1 => 0x0054_d884u32.wrapping_add(user.wrapping_mul(0x38)),
            2 => only_four(0x0054_dba8),
            3 => 0x0054_d76cu32.wrapping_add(user.wrapping_mul(0x38)),
            4 => only_four(0x0054_d9e0),
            5 => 0x0054_d62cu32.wrapping_add(user.wrapping_mul(0x40)),
            _ => 0x0054_d5c4,
        };
        let mut at = self
            .0
            .read_u32(record.wrapping_add(0x34))
            .ok_or(Unread::Unreadable)?;
        for _ in 0..CHAIN {
            if at == 0 {
                return Ok(false);
            }
            if at == button {
                return Ok(true);
            }
            at = self.word(at, 0x10)?;
        }
        Err(Gap::class("list_tree"))
    }

    /// `0x00253cc8`: the button's definition in its screen's first pane
    /// (`0x0022f8c7`), by its index (`+0xf8`), or 0.
    fn button_def(&self, button: u32) -> Read<u32> {
        let root = self.root(button)?;
        if root == 0 {
            return Ok(0);
        }
        let Some(screen) = self.screen(root)? else { return Ok(0) };
        if self.word(screen, 0x20)? as i32 <= 0 {
            return Ok(0);
        }
        let pane = self.word(screen, 0x24)?;
        if pane == 0 {
            return Ok(0);
        }
        let index = self.short(button, 0xf8)?;
        if index < 0 || i32::from(index) >= self.word(pane, 4)? as i32 {
            return Ok(0);
        }
        Ok(self.word(pane, 8)?.wrapping_add(index as u32 * 0x3c))
    }

    /// The button's image (its vtable's `+0x44`): `0x00254173`, image 1
    /// when focused and the bitmap has more than one; the keyboard key's
    /// `0x002377f6`, image 1 for keys `0x29` to `0x2c` whose bit of the
    /// keyboard's page flags (`+0x826`) is set, else the same.
    fn button_image(&self, button: u32, images: u32) -> Read<u16> {
        let base = |r: &Self| -> Read<u16> {
            Ok(u16::from(images as i32 > 1 && r.focused(button)?))
        };
        match self.word(self.word(button, 0)?, 0x44)? {
            0x0025_4173 => base(self),
            0x0023_77f6 => {
                let key = self.short(button, 0xf8)?;
                if !(0x29..=0x2c).contains(&key) {
                    return base(self);
                }
                let flags = self.byte(self.root(button)?.wrapping_add(0x826))?;
                if flags >> (key - 0x29) & 1 != 0 { Ok(1) } else { base(self) }
            }
            _ => Err(Gap::class("button_class")),
        }
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
        // `0x002360c3` centres the art in the view's extent (`i16` top,
        // left, bottom, right): half its width and height. Its corner
        // does not move it; the device's viewport is the screen's.
        let half = self.float(0x0045_dbbc, 0)?;
        let [top, left, bottom, right] =
            [0, 2, 4, 6].map(|o| self.short(viewport, o).map(i32::from));
        let centre = [
            (right? - left?) as f32 * half,
            (bottom? - top?) as f32 * half,
        ];
        let bitm = self.tag(tag)?.ok_or(Gap::new(Unread::Bitmap, "art_image"))?;
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
            centre[0] + x * scale,
            centre[1] - y * scale,
            centre[0] + (x + w) * scale,
            centre[1] - (y - h) * scale,
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

/// `0x0022e9c6`'s `cvttss2si`: a float offset as the integer the guest
/// adds, the integer indefinite (low half 0) when out of range or NaN.
fn truncate(v: f32) -> i32 {
    if (-2_147_483_648.0..2_147_483_648.0).contains(&v) { v as i32 } else { i32::MIN }
}

/// The list draw's scroll arrows (`0x0024bda2`): each from the list's
/// animated left and bottom, moved by the skin's offsets, or, for a class
/// the host registered at run time, by the offsets `placed` gives for its
/// vtable and height (the game's `list-scroll-arrows` hook moves the
/// game's quads the same way). A runtime class `placed` does not know is
/// the game's (`draw_runtime_class`).
pub(super) fn arrows(mem: &dyn GuestRead, widget: u32, viewport: u32, placed: super::ListArrows) -> Read<Vec<UiBitmap>> {
    let r = Reader(mem);
    // `0x0022f0ff`: a hidden or disabled list draws nothing.
    if r.short(widget, 0x42)? & 3 != 0 {
        return Ok(Vec::new());
    }
    // `0x0024bbf3`: the skin of the parent screen's pane (`0x0022f899`:
    // the pane at the screen's `+0x5f0`, -1 for none, any other index
    // under the screen tag's count), its list block's `+4`.
    let parent = r.word(widget, 0x10)?;
    let mut skin_index = 0;
    if parent != 0 {
        let index = r.short(parent, 0x5f0)?;
        if index != -1
            && let Some(screen) = r.screen(parent)?
            && i32::from(index) < r.word(screen, 0x20)? as i32
        {
            let pane = r.word(screen, 0x24)?.wrapping_add((index as i32 as u32).wrapping_mul(0x4c));
            if pane != 0 && r.word(pane, 12)? as i32 > 0 {
                skin_index = r.short(r.word(pane, 0x10)?, 4)?;
            }
        }
    }
    // `0x0014837a`: the skin from the UI globals' skins (`+0x138`).
    let Some(globals) = r.ui_globals()? else { return Ok(Vec::new()) };
    if skin_index < 0 || i32::from(skin_index) >= r.word(globals, 0x138)? as i32 {
        return Ok(Vec::new());
    }
    let skin_tag = r.word(r.word(globals, 0x13c)?, skin_index as u32 * 8 + 4)?;
    let Some(skin) = r.tag(skin_tag)? else { return Ok(Vec::new()) };
    let tag = r.word(skin, 8)?;
    if tag == u32::MAX {
        return Ok(Vec::new());
    }
    let visible = r.arrows_visible(widget)?;
    if visible == [false; 2] {
        return Ok(Vec::new());
    }
    let class = match r.retail_table(widget) {
        Ok(_) => None,
        Err(gap) => {
            let height = i32::from(r.short(widget, 0x24)?) - i32::from(r.short(widget, 0x20)?);
            Some(placed(r.word(widget, 0)?, height).ok_or(gap)?)
        }
    };
    let animated_edge = |edge, offset| -> Read<i16> {
        let delta = f32::from_bits(r.word(widget, offset)?);
        Ok(r.short(widget, edge)?.wrapping_add(truncate(delta) as i16))
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

/// The button's art (`0x00253ea8` to `0x00254022`): its definition's
/// bitmap (`+0x28`) at its animated rectangle moved by the definition's
/// offset (`+0x2c`, `+0x2e`), the image its class picks. With the widget
/// index display on (`[0x0054d59f]`) the art is drawn the same; only the
/// label changes (`text::read_widget`).
pub(super) fn button_art(
    mem: &dyn GuestRead,
    widget: u32,
    viewport: u32,
) -> Read<Option<UiBitmap>> {
    let r = Reader(mem);
    let def = r.button_def(widget)?;
    if def == 0 {
        return Ok(None);
    }
    let tag = r.word(def, 0x28)?;
    let Some(bitm) = r.tag(tag)? else { return Ok(None) };
    let image = r.button_image(widget, r.word(bitm, 0x44)?)?;
    let x =
        f32::from(r.short(widget, 0x22)?) + r.float(widget, 0x58)? + f32::from(r.short(def, 0x2c)?);
    let y =
        f32::from(r.short(widget, 0x20)?) + r.float(widget, 0x5c)? - f32::from(r.short(def, 0x2e)?);
    Ok(Some(r.image(widget, viewport, tag, image, [x, y], r.float(widget, 0x64)?)?))
}

/// The screen id a left-out draw of `widget` (a button or a list) would
/// have written to [`MISSED_SCREEN`]: its screen's (a button's top
/// screen; a shown list's parent, when it names a pane) when no screen
/// tag has it and it differs from the word there.
pub(super) fn missed_screen(mem: &dyn GuestRead, widget: u32, button: bool) -> Option<u32> {
    let r = Reader(mem);
    let screen = if button {
        r.root(widget).ok()?
    } else {
        let parent = r.word(widget, 0x10).ok()?;
        if r.short(widget, 0x42).ok()? & 3 != 0 || parent == 0 || r.short(parent, 0x5f0).ok()? == -1 {
            return None;
        }
        parent
    };
    if screen == 0 {
        return None;
    }
    let id = r.screen_of(screen).ok()?.err()?;
    (mem.read_u32(MISSED_SCREEN) != Some(id)).then_some(id)
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
        // The search back probes row -1 too; the search on reads row 6.
        m.w(rows - 8, 0);
        m.w(rows + 6 * 8, 0);
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
        m.w(button + 0x10, root);
        m.w(root, rt);
        m.w(root + 0x10, 0);
        m.w(bt + 0x30, 0x0022_ec9e);
        m.w(bt + 0x2c, 0x0022_ec90);
        m.w(rt + 0x50, 0x0023_012c);
        m.w(rt + 0x54, 0x0023_0130);
        m.w(root + 0x74, 1);
        m.w(root + 0x78, 2);
        m.w(0x0054_d884 + 2 * 0x38 + 0x34, leaf);
        m.w(leaf + 0x10, button);
        assert_eq!(Reader(&m).focused(button), Ok(true));
        m.w(leaf + 0x10, root);
        assert_eq!(Reader(&m).focused(button), Ok(false));
        // A channel past 5 is the shared record; a user past 4 indexes on.
        m.w(root + 0x74, 9);
        m.w(0x0054_d5c4 + 0x34, button);
        assert_eq!(Reader(&m).focused(button), Ok(true));
        m.w(root + 0x74, 5);
        m.w(root + 0x78, 7);
        m.w(0x0054_d62c + 7 * 0x40 + 0x34, 0);
        assert_eq!(Reader(&m).focused(button), Ok(false));
        // Channel 0 for a user other than 4 is record 0, read at 0x34.
        m.w(root + 0x74, 0);
        m.w(0x34, leaf);
        m.w(leaf + 0x10, button);
        assert_eq!(Reader(&m).focused(button), Ok(true));
        // A screen class whose user getter is not the retail one.
        m.w(rt + 0x54, 0x0099_0000);
        assert_eq!(Reader(&m).focused(button), Err(Gap::class("button_focus")));
    }

    /// The keyboard key class lights keys 0x29 to 0x2c by the page flags;
    /// other keys, and the retail button, light by focus.
    #[test]
    fn the_keyboard_key_class_lights_by_its_page_flags() {
        let mut m = Mem::default();
        let (key, screen, kt, st) = (0x1000, 0x2000, 0x3000, 0x4000);
        m.w(key, kt);
        m.w(key + 0x10, screen);
        m.w(screen, st);
        m.w(screen + 0x10, 0);
        m.w(kt + 0x44, 0x0023_77f6);
        m.w(kt + 0x30, 0x0022_ec9e);
        m.w(kt + 0x2c, 0x0022_ec90);
        m.w(st + 0x50, 0x0023_012c);
        m.w(st + 0x54, 0x0023_0130);
        m.w(screen + 0x74, 2);
        m.w(screen + 0x78, 4);
        m.w(0x0054_dba8 + 0x34, 0);
        m.w(screen + 0x824, 0b0100 << 16);
        m.w(key + 0xf8, 0x2b);
        assert_eq!(Reader(&m).button_image(key, 2), Ok(1), "page bit 2");
        m.w(key + 0xf8, 0x2a);
        assert_eq!(Reader(&m).button_image(key, 2), Ok(0), "bit 1 clear, not focused");
        m.w(0x0054_dba8 + 0x34, key);
        assert_eq!(Reader(&m).button_image(key, 2), Ok(1), "focused");
        assert_eq!(Reader(&m).button_image(key, 1), Ok(0), "one image");
        m.w(kt + 0x44, 0x0012_3456);
        assert_eq!(Reader(&m).button_image(key, 2), Err(Gap::class("button_class")));
    }

    /// `cvttss2si`: out of range and NaN are the integer indefinite.
    #[test]
    fn an_offset_truncates_as_the_guest_does() {
        assert_eq!(truncate(-2.75), -2);
        assert_eq!(truncate(f32::NAN), i32::MIN);
        assert_eq!(truncate(3e9) as i16, 0);
    }

    /// A screen id no screen tag has: no art, and the id the left-out
    /// draw would have noted.
    #[test]
    fn a_button_on_an_unknown_screen_has_no_art_and_notes_the_id() {
        let mut m = Mem::default();
        let (button, screen, tags) = (0x1000, 0x2000, 0x7000);
        m.w(button, 0x0045_a628);
        m.w(button + 0x10, screen);
        m.w(screen, 0x0045_8840);
        m.w(screen + 0x10, 0);
        m.w(screen + 0x70, 0xa7);
        m.w(0x004e_3b44, tags);
        m.w(tags + 5 * 16 + 8, 0x7100);
        m.w(0x004e_0350, 0x6000);
        m.w(0x004e_034c, 0x6100);
        m.w(0x6010, 0);
        m.w(0x6100 + 0x110, 1);
        m.w(0x6100 + 0x114, 0x6200);
        m.w(0x6200 + 0x8c, 5);
        m.w(0x7100 + 8, 0);
        m.w(0x7100 + 12, 0x7800);
        m.w(MISSED_SCREEN, 0);
        assert_eq!(button_art(&m, button, 0x8000), Ok(None));
        assert_eq!(missed_screen(&m, button, true), Some(0xa7));
        m.w(MISSED_SCREEN, 0xa7);
        assert_eq!(missed_screen(&m, button, true), None, "already noted");
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
        m.f(0x0045_dbbc, 0.5);
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
        // An inset view centres the art in its own extent, not its corner.
        m.w(image + 0x70, 8);
        m.w(viewport, 40 | 20 << 16);
        m.w(viewport + 4, 240 | 340 << 16);
        assert_eq!(read(&m).unwrap().rect, [171.0, 80.0, 208.0, 93.0]);
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
        m.w(0x4030, 0);
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
