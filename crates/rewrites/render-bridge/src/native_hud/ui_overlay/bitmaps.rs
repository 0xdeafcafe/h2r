//! Bitmap widgets on the overlay (docs/graphics/native-post-hud.md, "The
//! menus on the overlay", "Bitmap widgets"): what the bitmap widget's
//! draw `0x002b0540` makes, read at the sorted list as one quad with its
//! texture, tint, blend and coordinates, or why the game draws it.
//!
//! The draw (Code, `h2 disasm`):
//!
//! 1. No definition (`+0x70`): nothing. A definition whose flags have
//!    bit 2 is a meter (`0x002b0a68`, drawn by `0x002b07e2`): refused.
//! 2. `0x002b0565`: the view's offset from the draw's argument, a
//!    rectangle (top, left, bottom, right), by `0x002363d4`
//!    ([`view_offset`]). With the debug labels on (`[0x0054d59f]`) a
//!    label follows the quad: refused. A definition with no bitmap
//!    (`+0x1c` -1) draws no quad.
//! 3. The tint: the alpha at `+0x64` and the colour at `+0x28..+0x30`,
//!    each times `[0x0045dc38]` and rounded by `fistp`, packed ARGB with
//!    shifts and ors and no masks ([`tint`]).
//! 4. The image (`0x002b0b19`): the pointer at `+0x8c` when set (refused:
//!    its tag is not named), else the bitmap tag's image `+0x88` when in
//!    range, its first otherwise.
//! 5. The rectangle (`0x0022ea81`): the header's, plus the animated
//!    offset's X and Y, its two corners projected at the offset's Z
//!    about the view's centre (`0x0023618e`, `0x002360c3`, [`project`]);
//!    then the view's offset added.
//! 6. A definition that scrolls (`+0x10` or `+0x14` not 0) draws through
//!    `0x00235f31`: refused. Else `0x00235e5e`: coordinates 0 to the
//!    image's size over that size (at least 1), at most 1, in texels for
//!    a linear image ([`coordinates`]); then `0x0023675a` makes the four
//!    corners and a screen-quad record of the image (clamped, linear
//!    filtering) with blend mode 1 when the definition's `+0x08` is 1,
//!    else 0, and draws it through `0x00052040` when the image is
//!    resident (`0x00012360`).
//!
//! `0x00052040`'s combiner for a record of one image and no colour
//! pointers is the image times the vertex colour times stage 0's first
//! constant, packed from the colour `[0x00468710]` points at with alpha 1
//! ([`white_tint`]): `Combiner::Texture` when that colour is white. The
//! begin's screen transform with its half pixel puts a guest pixel
//! coordinate on the overlay's pixel edge of the same number (the screen
//! quads' viewport must be the full screen, which the list's claim
//! checks). S, as for the text (`native_render::hud::text_quad`): the
//! vertex program passes the coordinates through times the record's
//! scale, 1.

use frame_scene::{Combiner, OverlayBlend, OverlayTexture, Sampler};
use scene_extract::GuestRead;
use scene_extract::ui::{self as walk, UiBitmap, UiContent, UiWidget, Unread};
use timeline::native::Ui;

/// Why a bitmap widget's draw is the game's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// A meter (`0x002b07e2`).
    Meter,
    /// A scrolling bitmap (`0x00235f31`).
    Scroll,
    /// The draw is not the overlay's quad: the debug labels on, an image
    /// set from elsewhere (`+0x8c`), an image not resident, a tint
    /// constant not white, the quads turned off, a blend mode past the
    /// tables or an empty image.
    State,
    /// The widget, its definition, its bitmap or a constant did not read.
    Unreadable,
}

impl Refusal {
    pub fn counter(self) -> Ui {
        match self {
            Refusal::Meter => Ui::OverlayMenuRefusedBitmapMeter,
            Refusal::Scroll => Ui::OverlayMenuRefusedBitmapScroll,
            Refusal::State => Ui::OverlayMenuRefusedBitmapState,
            Refusal::Unreadable => Ui::OverlayMenuRefusedUnreadable,
        }
    }
}

/// The bitmap widget (Code, `0x002b01eb` builds it).
mod widget {
    pub const TINT: u32 = 0x28;
    pub const OFFSET: u32 = 0x58;
    pub const ALPHA: u32 = 0x64;
    pub const DEFINITION: u32 = 0x70;
    /// The `i16` image index (the word at `+0x88`).
    pub const IMAGE: u32 = 0x88;
    pub const IMAGE_POINTER: u32 = 0x8c;
}

/// The bitmap widget's definition (its tag block element).
mod definition {
    /// In the flags byte at `+0x00`.
    pub const METER: u32 = 1 << 2;
    /// The `i16` at `+0x08`: 1 draws with blend mode 1.
    pub const BLEND_WORD: u32 = 0x08;
    pub const SCROLL: u32 = 0x10;
    pub const BITMAP: u32 = 0x1c;
}

/// Addresses the draw reads (Code).
pub mod at {
    /// A byte: the widgets' debug labels.
    pub const DEBUG_LABELS: u32 = 0x0054_d59f;
    /// The tag instances (`0x10` bytes each, the data pointer at `+8`).
    pub const TAG_INSTANCES: u32 = 0x004e_3b44;
    /// The UI camera's distance (`0x002360c3`).
    pub const CAMERA_DISTANCE: u32 = 0x0054_d5b0;
    /// The constants `0x002360c3`, `0x00235e5e` and the tint read.
    pub const HALF: u32 = 0x0045_dbbc;
    pub const ONE: u32 = 0x0045_dbc0;
    pub const NEAR: u32 = 0x0045_dbdc;
    pub const Z_FLOOR: u32 = 0x0045_dbd8;
    pub const Z_BELOW: u32 = 0x0045_dbcc;
    pub const CHANNEL: u32 = 0x0045_dc38;
    /// `0x00012360`: an image whose `+0x70` is past this is resident when
    /// its `+0x50` is set.
    pub const RESIDENT_AFTER: u32 = 0x004e_6488;
    /// A pointer to the colour (three floats) `0x00052040` packs into
    /// stage 0's first constant when the record names none.
    pub const DEFAULT_TINT: u32 = 0x0046_8710;
    /// A word: when not 0, `0x00052040` draws nothing.
    pub const QUADS_OFF: u32 = 0x0048_5602;
    /// `0x000142f0`'s tables (source, destination, equation), 12 modes.
    pub const BLEND_SOURCE: u32 = 0x0045_0718;
    pub const BLEND_DEST: u32 = 0x0045_06e8;
    pub const BLEND_EQUATION: u32 = 0x0045_06b8;
}

/// The bitmap tag's images (`0x002b0b19`): the count at `+0x44`, the
/// first at `+0x48`, `0x74` bytes each.
const IMAGE_COUNT: u32 = 0x44;
const IMAGES: u32 = 0x48;
const IMAGE_BYTES: u32 = 0x74;

/// One image: width and height (`i16`) at `+4`, flags at `+0x0e` (bit 4
/// linear), the texture at `+0x50`, the residency stamp at `+0x70`.
const IMAGE_SIZE: u32 = 0x04;
const IMAGE_FLAGS_WORD: u32 = 0x0c;
const LINEAR: u16 = 0x10;
const IMAGE_TEXTURE: u32 = 0x50;
const IMAGE_STAMP: u32 = 0x70;

fn word(mem: &dyn GuestRead, addr: u32) -> Option<u32> {
    mem.read_u32(addr)
}

fn float(mem: &dyn GuestRead, addr: u32) -> Option<f32> {
    mem.read_f32(addr)
}

/// The `i16` at `addr` (even), read through its aligned word.
fn short(mem: &dyn GuestRead, addr: u32) -> Option<i16> {
    let w = mem.read_u32(addr & !3)?;
    Some((if addr & 2 == 0 { w } else { w >> 16 }) as u16 as i16)
}

fn byte(mem: &dyn GuestRead, addr: u32) -> Option<u8> {
    let w = mem.read_u32(addr & !3)?;
    Some((w >> (8 * (addr & 3))) as u8)
}

/// The view rectangle the draw's argument points at: top, left, bottom,
/// right.
pub fn view(mem: &dyn GuestRead, rect: u32) -> Option<[i16; 4]> {
    Some([
        short(mem, rect)?,
        short(mem, rect.checked_add(2)?)?,
        short(mem, rect.checked_add(4)?)?,
        short(mem, rect.checked_add(6)?)?,
    ])
}

/// `0x002363d4`: the offset a view puts its widgets at, `(x, y)`: 20 left
/// or right in a half-width view, 16 up or down in a half-height one, by
/// which side of the screen it is on.
pub fn view_offset([top, left, bottom, right]: [i16; 4]) -> [i16; 2] {
    let side = |at: i16, low: i16| if at == 0 { low } else { -low };
    if right.wrapping_sub(left) == 320 {
        return [side(left, 20), side(top, -16)];
    }
    let y = if bottom.wrapping_sub(top) == 240 {
        side(top, -16)
    } else {
        0
    };
    [0, y]
}

/// The constants `0x002360c3` reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lens {
    pub half: f32,
    pub one: f32,
    pub near: f32,
    pub z_floor: f32,
    pub z_below: f32,
    pub distance: f32,
}

impl Lens {
    pub fn read(mem: &dyn GuestRead) -> Option<Lens> {
        Some(Lens {
            half: float(mem, at::HALF)?,
            one: float(mem, at::ONE)?,
            near: float(mem, at::NEAR)?,
            z_floor: float(mem, at::Z_FLOOR)?,
            z_below: float(mem, at::Z_BELOW)?,
            distance: float(mem, at::CAMERA_DISTANCE)?,
        })
    }
}

/// `0x002360c3`: a point `(x, y, z)` about the view's centre, Y up, to
/// the view's pixels, Y down, with the guest's float operations in its
/// order. The camera's distance is 1 when its magnitude is under the
/// near constant; the depth past it is kept off the floor.
pub fn project(l: &Lens, [x, y, z]: [f32; 3], [top, left, bottom, right]: [i16; 4]) -> [f32; 3] {
    let cx = (i32::from(right) - i32::from(left)) as f32 * l.half;
    let cy = (i32::from(bottom) - i32::from(top)) as f32 * l.half;
    // `fcomip`: an unordered distance keeps itself.
    let d = if l.near > l.distance.abs() {
        l.one
    } else {
        l.distance
    };
    let mut z = z + d;
    // `comiss; jb`: unordered goes below.
    if z >= l.z_floor {
        // `comiss; ja`: kept when past one.
        z = if z > l.one { z } else { l.one };
    } else if z > l.z_below {
        z = l.z_below;
    }
    let scale = l.one / (l.one / d * z);
    [x * scale + cx, cy - y * scale, z - d]
}

/// `fistp` under the default control word: nearest, ties to even; the
/// integer indefinite out of range.
fn fistp(v: f32) -> u32 {
    let r = v.round_ties_even();
    if r.is_finite() && r >= i32::MIN as f32 && r < 2_147_483_648.0 {
        r as i32 as u32
    } else {
        0x8000_0000
    }
}

/// The draw's vertex colour (`0x002b05c8..0x002b0632`): alpha, red,
/// green and blue, each times `channel` and rounded, packed by shifting
/// left 8 and or-ing the next, unmasked.
pub fn tint(alpha: f32, [r, g, b]: [f32; 3], channel: f32) -> u32 {
    [r, g, b]
        .into_iter()
        .fold(fistp(alpha * channel), |argb, c| {
            argb.wrapping_shl(8) | fistp(c * channel)
        })
}

/// A D3D colour as the overlay's RGBA, 0 to 1.
pub fn rgba(argb: u32) -> [f32; 4] {
    let byte = |shift: u32| ((argb >> shift) & 0xff) as f32 / 255.0;
    [byte(16), byte(8), byte(0), byte(24)]
}

/// `0x00235e5e`'s coordinates over an image `size` (`[u0, v0, u1, v1]`),
/// normalised: in texels for a `linear` image as the guest draws it,
/// then over its size.
pub fn coordinates([w, h]: [i16; 2], linear: bool, one: f32) -> [f32; 4] {
    let (w, h) = (f32::from(w), f32::from(h));
    // `comiss one, size; ja`: the size, at least one.
    let at_least = |s: f32| if one > s { one } else { s };
    let mut u1 = (w - 0.0) / at_least(w);
    let mut v1 = (h - 0.0) / at_least(h);
    // The draw clamps (its third argument is 1).
    if u1 > one {
        u1 = one;
    }
    if v1 > one {
        v1 = one;
    }
    if !linear {
        return [0.0, 0.0, u1, v1];
    }
    let texels = [w * 0.0, h * 0.0, w * u1, h * v1];
    [texels[0] / w, texels[1] / h, texels[2] / w, texels[3] / h]
}

/// `0x00052040`'s stage 0 constant for a record with no colour: the
/// colour `[0x00468710]` points at, alpha 1. The combiner is the
/// texture times the vertex colour only when it is white.
pub fn white_tint(mem: &dyn GuestRead) -> Option<bool> {
    let colour = word(mem, at::DEFAULT_TINT)?;
    let one = float(mem, at::ONE)?;
    let rgb = [
        float(mem, colour)?,
        float(mem, colour.checked_add(4)?)?,
        float(mem, colour.checked_add(8)?)?,
    ];
    Some(one == 1.0 && rgb == [1.0; 3])
}

/// `0x000142f0`'s blend for mode `mode` (0 or 1 here).
fn blend(mem: &dyn GuestRead, mode: u32) -> Option<OverlayBlend> {
    let entry = 4 * mode;
    Some(OverlayBlend::On {
        source: word(mem, at::BLEND_SOURCE + entry)?,
        dest: word(mem, at::BLEND_DEST + entry)?,
        equation: word(mem, at::BLEND_EQUATION + entry)?,
    })
}

/// The bitmap widget at `widget` (`depth` the list entry's), drawn in
/// the view at `view_at` (the draw's argument): its header and the quad
/// its draw makes ([`UiContent::Bitmap`]; [`UiContent::None`] when it
/// makes none), or why the game draws it. The refusals come in the
/// draw's order.
pub fn read(
    mem: &dyn GuestRead,
    widget: u32,
    depth: f32,
    view_at: u32,
) -> Result<UiWidget, Refusal> {
    use Refusal::Unreadable;
    let mut header = walk::read_header(mem, widget, depth).map_err(|_: Unread| Unreadable)?;
    let off = |base: u32, o: u32| base.checked_add(o).ok_or(Unreadable);
    let w32 = |a: u32| word(mem, a).ok_or(Unreadable);
    let f32_at = |a: u32| float(mem, a).ok_or(Unreadable);
    let i16_at = |a: u32| short(mem, a).ok_or(Unreadable);
    let def = w32(off(widget, widget::DEFINITION)?)?;
    if def == 0 {
        return Ok(header);
    }
    if w32(def)? & definition::METER != 0 {
        return Err(Refusal::Meter);
    }
    if byte(mem, at::DEBUG_LABELS).ok_or(Unreadable)? != 0 {
        return Err(Refusal::State);
    }
    let datum = w32(off(def, definition::BITMAP)?)?;
    if datum == u32::MAX {
        return Ok(header);
    }
    let view = view(mem, view_at).ok_or(Unreadable)?;
    let argb = tint(
        f32_at(off(widget, widget::ALPHA)?)?,
        [
            f32_at(off(widget, widget::TINT)?)?,
            f32_at(off(widget, widget::TINT + 4)?)?,
            f32_at(off(widget, widget::TINT + 8)?)?,
        ],
        f32_at(at::CHANNEL)?,
    );
    // The image (`0x002b0b19`).
    if w32(off(widget, widget::IMAGE_POINTER)?)? != 0 {
        return Err(Refusal::State);
    }
    let instance = off(w32(at::TAG_INSTANCES)?, (datum & 0xffff) * 0x10 + 8)?;
    let data = w32(instance)?;
    let count = w32(off(data, IMAGE_COUNT)?)? as i32;
    if count <= 0 {
        return Err(Refusal::State);
    }
    let asked = i16_at(off(widget, widget::IMAGE)?)?;
    let index = if asked >= 0 && i32::from(asked) < count {
        asked as u16
    } else {
        0
    };
    let image = off(w32(off(data, IMAGES)?)?, u32::from(index) * IMAGE_BYTES)?;
    let size = [
        i16_at(off(image, IMAGE_SIZE)?)?,
        i16_at(off(image, IMAGE_SIZE + 2)?)?,
    ];
    let linear = (w32(off(image, IMAGE_FLAGS_WORD)?)? >> 16) as u16 & LINEAR != 0;
    // The rectangle (`0x0022ea81`, `0x0023618e`), then the view's offset.
    let lens = Lens::read(mem).ok_or(Unreadable)?;
    let [ox, oy, oz] = [
        f32_at(off(widget, widget::OFFSET)?)?,
        f32_at(off(widget, widget::OFFSET + 4)?)?,
        f32_at(off(widget, widget::OFFSET + 8)?)?,
    ];
    let [top, left, bottom, right] = header.rect.map(f32::from);
    let near = project(&lens, [left + ox, top + oy, oz], view);
    let far = project(&lens, [right + ox, bottom + oy, oz], view);
    let [px, py] = view_offset(view);
    let (px, py) = (f32::from(px), -i32::from(py) as f32);
    let r = [near[0] + px, far[0] + px, near[1] + py, far[1] + py];
    // The definition's scrolling.
    let scroll = [
        f32_at(off(def, definition::SCROLL)?)?,
        f32_at(off(def, definition::SCROLL + 4)?)?,
    ];
    if scroll != [0.0, 0.0] {
        return Err(Refusal::Scroll);
    }
    let mode = u32::from(i16_at(off(def, definition::BLEND_WORD)?)? == 1);
    // `0x00052040` and `0x00012360`.
    if i16_at(at::QUADS_OFF)? != 0 || !white_tint(mem).ok_or(Unreadable)? {
        return Err(Refusal::State);
    }
    let stamp = w32(off(image, IMAGE_STAMP)?)? as i32;
    let after = w32(at::RESIDENT_AFTER)? as i32;
    let texture = w32(off(image, IMAGE_TEXTURE)?)?;
    if stamp <= after || texture == 0 || size[0] <= 0 || size[1] <= 0 {
        return Err(Refusal::State);
    }
    let blend = blend(mem, mode).ok_or(Unreadable)?;
    // `0x0023675a`'s corners: the far ones as the near plus the extent.
    let rect = [r[0], r[2], r[0] + (r[1] - r[0]), r[2] + (r[3] - r[2])];
    header.content = UiContent::Bitmap(UiBitmap {
        texture: OverlayTexture::Bitmap {
            tag: datum,
            image: index,
        },
        rect,
        uv: coordinates(size, linear, lens.one),
        color: rgba(argb),
        combiner: Combiner::Texture,
        sampler: Sampler {
            wrap: false,
            point: false,
        },
        blend,
    });
    Ok(header)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use scene_extract::testing::Mem;
    use scene_extract::ui::draws;

    pub const WIDGET: u32 = 0x1000;
    const VTABLE: u32 = 0x5200;
    pub const DEF: u32 = 0x3000;
    pub const DATUM: u32 = 0xe123_0002;
    const INSTANCES: u32 = 0x6000;
    const DATA: u32 = 0x7000;
    const IMAGES_AT: u32 = 0x8000;
    const TINT_AT: u32 = 0xa000;
    pub const VIEW: u32 = 0xb000;

    /// Writes the `i16` at `addr` (even) into its aligned word.
    pub fn half(m: &mut Mem, addr: u32, v: i16) {
        let at = addr & !3;
        let old = m.0.get(&at).copied().unwrap_or(0);
        let shift = 8 * (addr & 2);
        m.w(
            at,
            (old & !(0xffff << shift)) | (u32::from(v as u16) << shift),
        );
    }

    /// A bitmap widget (header rectangle top 100, left -50, bottom 60,
    /// right 30; offset (5, 10, 0); alpha 0.5, tint (1, 0.5, 0)) of image
    /// 1 of a two-image bitmap, linear 64 by 32 and resident, in the full
    /// screen view, with the constants and tables an image would hold
    /// (invented, not the retail values).
    pub fn bitmap_widget(m: &mut Mem) {
        m.w(WIDGET, VTABLE);
        m.w(VTABLE + 0x10, draws::BITMAP);
        m.w(WIDGET + 4, 8);
        half(m, WIDGET + 0x20, 100);
        half(m, WIDGET + 0x22, -50);
        half(m, WIDGET + 0x24, 60);
        half(m, WIDGET + 0x26, 30);
        m.f(WIDGET + 0x28, 1.0);
        m.f(WIDGET + 0x2c, 0.5);
        m.f(WIDGET + 0x30, 0.0);
        m.f(WIDGET + 0x58, 5.0);
        m.f(WIDGET + 0x5c, 10.0);
        m.f(WIDGET + 0x60, 0.0);
        m.f(WIDGET + 0x64, 0.5);
        m.w(WIDGET + 0x68, 0);
        m.w(WIDGET + 0x70, DEF);
        m.w(WIDGET + 0x88, 1);
        m.w(WIDGET + 0x8c, 0);
        m.w(DEF, 0);
        m.w(DEF + 8, 1);
        m.f(DEF + 0x10, 0.0);
        m.f(DEF + 0x14, 0.0);
        m.w(DEF + 0x1c, DATUM);
        m.w(at::TAG_INSTANCES, INSTANCES);
        m.w(INSTANCES + 2 * 0x10 + 8, DATA);
        m.w(DATA + 0x44, 2);
        m.w(DATA + 0x48, IMAGES_AT);
        for (n, size) in [(0u32, [16i16, 16]), (1, [64, 32])] {
            let image = IMAGES_AT + n * 0x74;
            half(m, image + 4, size[0]);
            half(m, image + 6, size[1]);
            m.w(image + 0x0c, u32::from(LINEAR) << 16);
            m.w(image + 0x50, 0x9000 + n);
            m.w(image + 0x70, 5);
        }
        m.w(at::RESIDENT_AFTER, 3);
        m.f(at::HALF, 0.5);
        m.f(at::ONE, 1.0);
        m.f(at::NEAR, 0.001);
        m.f(at::Z_FLOOR, 0.0);
        m.f(at::Z_BELOW, -0.001);
        m.f(at::CHANNEL, 255.0);
        m.f(at::CAMERA_DISTANCE, 1.0);
        m.w(at::DEBUG_LABELS & !3, 0);
        m.w(at::QUADS_OFF & !3, 0);
        m.w(at::DEFAULT_TINT, TINT_AT);
        for n in 0..3 {
            m.f(TINT_AT + 4 * n, 1.0);
        }
        for mode in 0..2u32 {
            m.w(at::BLEND_SOURCE + 4 * mode, 0x302 + mode);
            m.w(at::BLEND_DEST + 4 * mode, 0x303);
            m.w(at::BLEND_EQUATION + 4 * mode, 0x8006);
        }
        half(m, VIEW, 0);
        half(m, VIEW + 2, 0);
        half(m, VIEW + 4, 480);
        half(m, VIEW + 6, 640);
    }

    fn content(m: &Mem) -> Result<UiContent, Refusal> {
        read(m, WIDGET, 0.0, VIEW).map(|w| w.content)
    }

    /// The draw's quad: the header's rectangle at depth 0 is where the
    /// header puts it; the tint is packed and rounded; image 1, linear,
    /// mode 1's blend, the texture combiner, clamped and linear.
    #[test]
    fn a_bitmap_widget_reads_as_the_quad_its_draw_makes() {
        let mut m = Mem::default();
        bitmap_widget(&mut m);
        let w = read(&m, WIDGET, 0.25, VIEW).unwrap();
        assert_eq!((w.kind, w.depth, w.alpha), (8, 0.25, 0.5));
        let UiContent::Bitmap(b) = w.content else {
            panic!("not a bitmap")
        };
        assert_eq!(
            b,
            UiBitmap {
                texture: OverlayTexture::Bitmap {
                    tag: DATUM,
                    image: 1
                },
                rect: [275.0, 130.0, 355.0, 170.0],
                uv: [0.0, 0.0, 1.0, 1.0],
                color: rgba(0x80ff_8000),
                combiner: Combiner::Texture,
                sampler: Sampler {
                    wrap: false,
                    point: false
                },
                blend: OverlayBlend::On {
                    source: 0x303,
                    dest: 0x303,
                    equation: 0x8006
                },
            }
        );
        // Further back: the rectangle shrinks about the view's centre.
        m.f(WIDGET + 0x60, 1.0);
        let Ok(UiContent::Bitmap(far)) = content(&m) else {
            panic!()
        };
        assert_eq!(far.rect, [297.5, 185.0, 337.5, 205.0]);
        // Blend mode 0 unless the definition's word is 1; an image index
        // out of range is the first image.
        m.w(DEF + 8, 2);
        half(&mut m, WIDGET + 0x88, 7);
        let Ok(UiContent::Bitmap(first)) = content(&m) else {
            panic!()
        };
        assert_eq!(
            first.texture,
            OverlayTexture::Bitmap {
                tag: DATUM,
                image: 0
            }
        );
        assert_eq!(
            first.blend,
            OverlayBlend::On {
                source: 0x302,
                dest: 0x303,
                equation: 0x8006
            }
        );
    }

    /// No definition, or no bitmap: no quad, nothing to draw.
    #[test]
    fn a_bitmap_widget_with_no_bitmap_draws_nothing() {
        let mut m = Mem::default();
        bitmap_widget(&mut m);
        m.w(DEF + 0x1c, u32::MAX);
        assert_eq!(content(&m), Ok(UiContent::None));
        m.w(WIDGET + 0x70, 0);
        assert_eq!(content(&m), Ok(UiContent::None));
    }

    /// Each draw the overlay does not make is the game's, by its reason.
    #[test]
    fn meters_scrolling_and_other_state_are_refused() {
        let cases: [(&str, fn(&mut Mem), Refusal); 9] = [
            ("meter", |m| m.w(DEF, 4), Refusal::Meter),
            ("scrolls", |m| m.f(DEF + 0x14, 0.5), Refusal::Scroll),
            (
                "debug labels",
                |m| m.w(at::DEBUG_LABELS & !3, 1 << 24),
                Refusal::State,
            ),
            (
                "image set elsewhere",
                |m| m.w(WIDGET + 0x8c, 0x8800),
                Refusal::State,
            ),
            (
                "not resident",
                |m| m.w(at::RESIDENT_AFTER, 5),
                Refusal::State,
            ),
            (
                "no texture",
                |m| m.w(IMAGES_AT + 0x74 + 0x50, 0),
                Refusal::State,
            ),
            ("tint constant", |m| m.f(TINT_AT + 4, 0.5), Refusal::State),
            (
                "quads off",
                |m| m.w(at::QUADS_OFF & !3, 1 << 16),
                Refusal::State,
            ),
            ("no images", |m| m.w(DATA + 0x44, 0), Refusal::State),
        ];
        for (what, set, why) in cases {
            let mut m = Mem::default();
            bitmap_widget(&mut m);
            set(&mut m);
            assert_eq!(content(&m), Err(why), "{what}");
        }
        let mut m = Mem::default();
        bitmap_widget(&mut m);
        m.0.remove(&at::CHANNEL);
        assert_eq!(content(&m), Err(Refusal::Unreadable));
    }

    /// `0x002363d4`: a half view's widgets move 20 in and 16 in.
    #[test]
    fn a_split_view_offsets_its_widgets() {
        assert_eq!(view_offset([0, 0, 480, 640]), [0, 0]);
        assert_eq!(view_offset([0, 0, 240, 640]), [0, -16]);
        assert_eq!(view_offset([240, 0, 480, 640]), [0, 16]);
        assert_eq!(view_offset([0, 0, 240, 320]), [20, -16]);
        assert_eq!(view_offset([240, 320, 480, 640]), [-20, 16]);
        assert_eq!(view_offset([0, 320, 480, 640]), [-20, -16]);
    }

    /// The tint's channels round to nearest even and are packed unmasked:
    /// a channel past 255 runs into the next.
    #[test]
    fn the_tint_is_rounded_and_packed_as_the_guest_does() {
        assert_eq!(tint(1.0, [1.0, 0.5, 0.0], 255.0), 0xffff_8000);
        assert_eq!(tint(0.5, [0.0; 3], 255.0), 0x8000_0000, "127.5 to 128");
        assert_eq!(tint(1.0, [2.0, 0.0, 0.0], 255.0), 0xfffe_0000, "510 in red");
        assert_eq!(fistp(f32::NAN), 0x8000_0000);
        assert_eq!(fistp(2.5), 2);
        assert_eq!(fistp(-1.5), (-2i32) as u32);
        assert_eq!(rgba(0x80ff_0000), [1.0, 0.0, 0.0, 128.0 / 255.0]);
    }

    /// Depth past the camera shrinks about the view's centre; a depth
    /// below the floor is kept under it.
    #[test]
    fn points_project_about_the_views_centre() {
        let mut m = Mem::default();
        bitmap_widget(&mut m);
        let l = Lens::read(&m).unwrap();
        let full = [0, 0, 480, 640];
        assert_eq!(project(&l, [0.0, 0.0, 0.0], full), [320.0, 240.0, 0.0]);
        assert_eq!(project(&l, [10.0, 20.0, 1.0], full), [325.0, 230.0, 1.0]);
        assert_eq!(
            project(&l, [10.0, 20.0, -0.5], full),
            [330.0, 220.0, 0.0],
            "kept at one"
        );
        assert_eq!(
            project(&l, [10.0, 20.0, -3.0], full)[2],
            -3.0,
            "well under the floor"
        );
        let below = project(&l, [10.0, 20.0, -1.0005], full);
        assert_eq!(
            below[2],
            -0.001f32 - 1.0,
            "just under the floor goes to the constant below it"
        );
        let near = Lens { distance: 0.0, ..l };
        assert_eq!(
            project(&near, [10.0, 20.0, 0.0], full),
            [330.0, 220.0, 0.0],
            "a zero distance is one"
        );
    }

    /// Coordinates cover the image, normalised for a linear one.
    #[test]
    fn coordinates_cover_the_image() {
        assert_eq!(coordinates([64, 32], false, 1.0), [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(coordinates([64, 32], true, 1.0), [0.0, 0.0, 1.0, 1.0]);
    }
}
