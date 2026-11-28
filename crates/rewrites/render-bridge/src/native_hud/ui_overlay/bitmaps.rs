//! Bitmap widgets on the overlay (docs/graphics/native-post-hud.md, "The
//! menus on the overlay", "Bitmap widgets"): what the bitmap widget's
//! draw `0x002b0540` makes, read at the sorted list as one quad with its
//! texture, tint, blend and coordinates, or why the game draws it.
//!
//! The draw (Code, `h2 disasm`):
//!
//! 1. No definition (`+0x70`): nothing. A definition whose flags have
//!    bit 2 is a meter (`0x002b0a68`), drawn by `0x002b07e2` ([`meter`]).
//! 2. `0x002b0565`: the view's offset from the draw's argument, a
//!    rectangle (top, left, bottom, right), by `0x002363d4`
//!    ([`view_offset`]). With the debug labels on (`[0x0054d59f]`) a
//!    label follows the quad: refused. A definition with no bitmap
//!    (`+0x1c` -1) draws no quad.
//! 3. The tint: the alpha at `+0x64` and the colour at `+0x28..+0x30`,
//!    each times `[0x0045dc38]` and rounded by `fistp`, packed ARGB with
//!    shifts and ors and no masks ([`tint`]).
//! 4. The image (`0x002b0b19`): the pointer at `+0x8c` when set (its
//!    setters, through `0x002b0a7b`, take it from a bitmap tag's images
//!    by `0x00137550`; [`named_image`] finds that tag), else the bitmap
//!    tag's image `+0x88` when in range, its first otherwise.
//! 5. The rectangle (`0x0022ea81`): the header's, plus the animated
//!    offset's X and Y, its two corners projected at the offset's Z
//!    about the view's centre (`0x0023618e`, `0x002360c3`, [`project`]);
//!    then the view's offset added.
//! 6. A definition that scrolls (`+0x10` or `+0x14` not 0) draws through
//!    `0x00235f31` ([`scrolled`]): up to four quads. Else `0x00235e5e`:
//!    coordinates 0 to the image's size over that size (at least 1), at
//!    most 1, in texels for a linear image ([`coordinates`]). Either way
//!    `0x0023675a` makes each quad's four corners and a screen-quad
//!    record of the image (clamped, linear filtering) with blend mode 1
//!    when the definition's `+0x08` is 1, else 0, and draws it through
//!    `0x00052040` when the image is resident (`0x00012360`).
//!
//! `0x00235f31` cuts the rectangle at the widget's scroll state (`+0x7c`,
//! two fractions `su`, `sv`, read as they are): `xm = (x1 - x0) * su +
//! x0`, `ym` likewise, and draws four pieces in this order, wrapping the
//! image by the fractions (`c = 1 - s`):
//!
//! | Piece | X | Y | U | V |
//! |---|---|---|---|---|
//! | 0 | `x0..xm` | `y0..ym` | `cu..1` | `cv..1` |
//! | 1 | `xm..x1` | `y0..ym` | `0..cu` | `cv..1` |
//! | 2 | `x0..xm` | `ym..y1` | `cu..1` | `0..cv` |
//! | 3 | `xm..x1` | `ym..y1` | `0..cu` | `0..cv` |
//!
//! A piece under one pixel wide or high (`comiss; jb`, unordered too) is
//! not drawn; a linear image's coordinates are times its size. There is
//! no clamp and no size floor (`0x00235e5e`'s).
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
//!
//! The meter's draw `0x002b07e2` is one filled rectangle:
//!
//! 1. The colour: the shared UI globals' meter colour (`wigl` `+0x50`,
//!    alpha, red, green, blue; `0x00148350` through the interface's
//!    `wgtz` by the scenario's type, `0x001482e8`), each times
//!    `[0x0045dc38]`, truncated by `_ftol2` `0x00322b0c` (the bitmap's
//!    tint rounds) and packed as the tint is ([`meter_tint`]); with no
//!    globals each channel is `[0x0045dc38]` alone. The widget's alpha
//!    does not enter it.
//! 2. The view's offset (`0x002363d4`), the debug labels (refused, as
//!    above), and the rectangle (`0x0022ea81`, as step 5) truncated
//!    (`cvttss2si`) and moved by the view's offset in 16 bits, X by X
//!    and Y by Y (the bitmap's draw subtracts its Y).
//! 3. `0x00147cbc` hands the rectangle to the rectangle draw
//!    `0x0013c050`: nothing with no scenario; else the four corners
//!    (left, top) clockwise at texture coordinate 0, the colour on each,
//!    and the same screen-quad record as `0x0023675a`'s with blend mode
//!    0 and image 1 of the bitmap the rasterizer globals name (`matg`
//!    `+0x108`'s first element, its reference at `+0x60`), drawn by
//!    `0x00052040` without the residency check.

use frame_scene::{Combiner, OverlayBlend, OverlayTexture, Sampler};
use scene_extract::GuestRead;
use scene_extract::ui::{self as walk, UiBitmap, UiContent, UiWidget, Unread};
use timeline::native::Ui;

/// Why a bitmap widget's draw is the game's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The draw is not the overlay's quad: the debug labels on, an image
    /// set at `+0x8c` that no loaded bitmap tag holds, an image not resident, a tint
    /// constant not white, the quads turned off, a blend mode past the
    /// tables or an empty image; a meter whose rasterizer bitmap has no
    /// second image.
    State,
    /// The widget, its definition, its bitmap or a constant did not read.
    Unreadable,
}

impl Refusal {
    pub fn counter(self) -> Ui {
        match self {
            Refusal::State => Ui::OverlayMenuRefusedBitmapState,
            Refusal::Unreadable => Ui::OverlayMenuRefusedUnreadable,
        }
    }
}

/// What a bitmap widget's draw makes.
#[derive(Clone, Debug, PartialEq)]
pub enum Drawn {
    /// The widget, its content the one quad ([`UiContent::Bitmap`]) or
    /// nothing ([`UiContent::None`]).
    Widget(UiWidget),
    /// A scrolling bitmap's quads, in the draw's order (at least one).
    Quads(Vec<UiBitmap>),
}

/// The bitmap widget (Code, `0x002b01eb` builds it).
mod widget {
    pub const TINT: u32 = 0x28;
    pub const OFFSET: u32 = 0x58;
    pub const ALPHA: u32 = 0x64;
    pub const DEFINITION: u32 = 0x70;
    /// The scroll state `0x00235f31` cuts at: two fractions, U then V.
    pub const SCROLL_STATE: u32 = 0x7c;
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
    /// The tag instances (`0x10` bytes each: the group at `+0`, the
    /// full datum at `+4`, the data pointer at `+8`).
    pub const TAG_INSTANCES: u32 = 0x004e_3b44;
    /// The tag index header: the scenario's datum at `+0x0c`, the slot
    /// count at `+0x18` (docs/engine/tag_get_checked.md).
    pub const TAG_INDEX: u32 = 0x0054_7f00;
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
    /// The scenario (`0x0013c050` draws nothing without one; its `i16`
    /// type at `+0x10` picks the interface's `wgtz`).
    pub const SCENARIO: u32 = 0x004e_0350;
    /// The globals tag (`matg`).
    pub const GLOBALS: u32 = 0x004e_034c;
}

/// The globals tag's blocks the meter's draw reads: each a count, then
/// the elements' address.
mod globals {
    /// The rasterizer globals; the bitmap's reference at `+0x60`, its
    /// datum at `+0x64`.
    pub const RASTERIZER: u32 = 0x108;
    pub const RASTERIZER_BITMAP: u32 = 0x64;
    /// The interface; `wgtz` datums at `+0x8c`, `+0x94`, `+0x84` for
    /// scenario types 0, 1 and 2.
    pub const INTERFACE: u32 = 0x110;
    pub const WGTZ: [u32; 3] = [0x8c, 0x94, 0x84];
    /// `wgtz` `+4`: the `wigl` datum; `wigl` `+0x50`: the meter colour.
    pub const WIGL: u32 = 0x04;
    pub const METER_COLOUR: u32 = 0x50;
    pub const SCENARIO_TYPE: u32 = 0x10;
}

/// The image of the rasterizer bitmap the meter draws (`0x0013c050`).
pub const METER_IMAGE: u16 = 1;

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

/// `0x00235f31`: a scrolling bitmap's rectangle `[x0, x1, y0, y1]` cut
/// at the scroll state `[su, sv]` into the pieces it draws, each its
/// rectangle and its coordinates `[u0, v0, u1, v1]` over an image `size`
/// (normalised, as [`coordinates`]'s), in the draw's order. A piece under
/// `one` wide or high is not drawn.
pub fn scrolled(
    [x0, x1, y0, y1]: [f32; 4],
    [su, sv]: [f32; 2],
    [w, h]: [i16; 2],
    linear: bool,
    one: f32,
) -> Vec<([f32; 4], [f32; 4])> {
    let xm = (x1 - x0) * su + x0;
    let ym = (y1 - y0) * sv + y0;
    let (cu, cv) = (one - su, one - sv);
    let (w, h) = (f32::from(w), f32::from(h));
    [
        ([x0, xm, y0, ym], [cu, cv, one, one]),
        ([xm, x1, y0, ym], [0.0, cv, cu, one]),
        ([x0, xm, ym, y1], [cu, 0.0, one, cv]),
        ([xm, x1, ym, y1], [0.0, 0.0, cu, cv]),
    ]
    .into_iter()
    // `comiss; jb`: under one, or unordered, is not drawn.
    .filter(|&([x0, x1, y0, y1], _)| x1 - x0 >= one && y1 - y0 >= one)
    .map(|(rect, [u0, v0, u1, v1])| {
        if !linear {
            return (rect, [u0, v0, u1, v1]);
        }
        let texels = [w * u0, h * v0, w * u1, h * v1];
        (rect, [texels[0] / w, texels[1] / h, texels[2] / w, texels[3] / h])
    })
    .collect()
}

/// `0x0023675a`'s corners of a rectangle `[x0, x1, y0, y1]`, as the
/// overlay's `[x0, y0, x1, y1]`: the far ones as the near plus the extent.
fn corners([x0, x1, y0, y1]: [f32; 4]) -> [f32; 4] {
    [x0, y0, x0 + (x1 - x0), y0 + (y1 - y0)]
}

/// `0x00052040`'s stage 0 constant for a record with no colour: the
/// colour `[0x00468710]` points at, alpha 1. The combiner is the
/// texture times the vertex colour only when it is white.
/// The tint constant `0x00012360` sets, `[r, g, b]` 0 to 1: multiplied
/// into the quad's colour.
pub fn default_tint(mem: &dyn GuestRead) -> Option<[f32; 3]> {
    let colour = word(mem, at::DEFAULT_TINT)?;
    Some([
        float(mem, colour)?,
        float(mem, colour.checked_add(4)?)?,
        float(mem, colour.checked_add(8)?)?,
    ])
}

/// `argb` as RGBA with the tint constant multiplied in.
fn tinted(argb: u32, [r, g, b]: [f32; 3]) -> [f32; 4] {
    let [cr, cg, cb, ca] = rgba(argb);
    [cr * r, cg * g, cb * b, ca]
}

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

/// The tag data a datum names (the instance's `+8`), unchecked as the
/// guest's lookups are; `None` for -1.
fn tag_data(mem: &dyn GuestRead, datum: u32) -> Option<Option<u32>> {
    if datum == u32::MAX {
        return Some(None);
    }
    let instance = word(mem, at::TAG_INSTANCES)?.checked_add((datum & 0xffff) * 0x10 + 8)?;
    word(mem, instance).map(Some)
}

/// The bitmap group, as a tag instance holds it.
const BITM: u32 = u32::from_be_bytes(*b"bitm");

/// Image `index` of the bitmap tag `datum` when that is `image`: the
/// tag's images from `+0x48`, its count at `+0x44`, its instance still
/// naming `datum`.
fn holds(mem: &dyn GuestRead, datum: u32, image: u32) -> Option<Option<u16>> {
    let entry = word(mem, at::TAG_INSTANCES)?.checked_add((datum & 0xffff) * 0x10)?;
    if datum == u32::MAX || word(mem, entry.checked_add(4)?)? != datum {
        return Some(None);
    }
    let data = word(mem, entry.checked_add(8)?)?;
    if data == 0 {
        return Some(None);
    }
    let count = word(mem, data.checked_add(IMAGE_COUNT)?)? as i32;
    let first = word(mem, data.checked_add(IMAGES)?)?;
    let Some(past) = image.checked_sub(first) else {
        return Some(None);
    };
    let index = past / IMAGE_BYTES;
    let held = past % IMAGE_BYTES == 0 && i64::from(index) < i64::from(count) && index <= 0x7fff;
    Some(held.then_some(index as u16))
}

/// [`named_image`]'s answers for one map (the index's entries and
/// scenario): an image pointer's tag and index, or none.
type Named = ((u32, u32), std::collections::HashMap<u32, Option<(u32, u16)>>);

thread_local! {
    static NAMED: std::cell::RefCell<Named> = Default::default();
}

/// The tag and index of the bitmap image the widget's `+0x8c` points
/// at (`0x002b0a7b`'s callers set it to `0x00137550`'s image of a tag's
/// bitmap; the widget keeps only the image). The definition's bitmap
/// `own` is tried first, then every loaded bitmap tag; an answer is kept
/// for the map and checked again on each use. `None`: no loaded bitmap
/// tag holds it (the game draws it).
pub fn named_image(
    mem: &dyn GuestRead,
    own: u32,
    image: u32,
) -> Result<Option<(u32, u16)>, Refusal> {
    use Refusal::Unreadable;
    if let Some(index) = holds(mem, own, image).ok_or(Unreadable)? {
        return Ok(Some((own, index)));
    }
    let entries = word(mem, at::TAG_INSTANCES).ok_or(Unreadable)?;
    let header = word(mem, at::TAG_INDEX).ok_or(Unreadable)?;
    if header == 0 || entries == 0 {
        return Ok(None);
    }
    let scenario = word(mem, header.checked_add(0x0c).ok_or(Unreadable)?).ok_or(Unreadable)?;
    let map = (entries, scenario);
    let kept = NAMED.with(|n| {
        let mut n = n.borrow_mut();
        if n.0 != map {
            *n = (map, Default::default());
        }
        n.1.get(&image).copied()
    });
    match kept {
        Some(None) => return Ok(None),
        Some(Some((tag, _))) => {
            if let Some(index) = holds(mem, tag, image).ok_or(Unreadable)? {
                return Ok(Some((tag, index)));
            }
        }
        None => {}
    }
    let count = word(mem, header.checked_add(0x18).ok_or(Unreadable)?).ok_or(Unreadable)?;
    let mut found = None;
    for slot in 0..count.min(0x1_0000) {
        let entry = entries.checked_add(slot * 0x10).ok_or(Unreadable)?;
        if word(mem, entry).ok_or(Unreadable)? != BITM {
            continue;
        }
        let datum = word(mem, entry.checked_add(4).ok_or(Unreadable)?).ok_or(Unreadable)?;
        if let Some(index) = holds(mem, datum, image).ok_or(Unreadable)? {
            found = Some((datum, index));
            break;
        }
    }
    NAMED.with(|n| n.borrow_mut().1.insert(image, found));
    Ok(found)
}

/// A block's first element: its address when the count is not 0.
fn first(mem: &dyn GuestRead, block: u32) -> Option<Option<u32>> {
    if word(mem, block)? == 0 {
        return Some(None);
    }
    word(mem, block.checked_add(4)?).map(|at| (at != 0).then_some(at))
}

/// `0x001482e8` then `0x00148350`: the shared UI globals (`wigl`) the
/// scenario's interface names, when there are.
pub fn shared_globals(mem: &dyn GuestRead) -> Option<Option<u32>> {
    let scenario = word(mem, at::SCENARIO)?;
    let matg = word(mem, at::GLOBALS)?;
    if scenario == 0 || matg == 0 {
        return Some(None);
    }
    let Some(interface) = first(mem, matg.checked_add(globals::INTERFACE)?)? else {
        return Some(None);
    };
    let kind = short(mem, scenario.checked_add(globals::SCENARIO_TYPE)?)?;
    let Some(&slot) = usize::try_from(kind)
        .ok()
        .and_then(|k| globals::WGTZ.get(k))
    else {
        return Some(None);
    };
    let Some(wgtz) = tag_data(mem, word(mem, interface.checked_add(slot)?)?)? else {
        return Some(None);
    };
    tag_data(mem, word(mem, wgtz.checked_add(globals::WIGL)?)?)
}

/// `_ftol2` (`0x00322b0c`): toward zero through 64 bits, the low word.
/// `None` for a value it has no integer for.
fn ftol(v: f32) -> Option<u32> {
    (v.is_finite() && v.abs() < 9.223_372e18).then(|| v.trunc() as i64 as u32)
}

/// The meter's colour (`0x002b07e2`'s first steps): each of the globals'
/// meter colour's channels (alpha, red, green, blue) times `channel` in
/// single precision, truncated by `_ftol2` (not rounded, as the bitmap's
/// tint is), packed by shifting left 8 and or-ing the next, unmasked;
/// each channel `channel` alone with no globals. `None` when a channel
/// has no integer or does not read.
pub fn meter_tint(mem: &dyn GuestRead, channel: f32) -> Option<u32> {
    let scaled = match shared_globals(mem)? {
        Some(wigl) => {
            let c = |n: u32| {
                float(mem, wigl.checked_add(globals::METER_COLOUR + 4 * n)?).map(|v| v * channel)
            };
            [c(0)?, c(1)?, c(2)?, c(3)?]
        }
        None => [channel; 4],
    };
    scaled
        .into_iter()
        .try_fold(0u32, |argb, c| Some(argb.wrapping_shl(8) | ftol(c)?))
}

/// `cvttss2si` into a 16-bit add: toward zero, the integer indefinite
/// (low half 0) out of range.
pub fn truncated(v: f32) -> i16 {
    if v.is_finite() && (-2_147_483_648.0..2_147_483_648.0).contains(&v) {
        v as i32 as i16
    } else {
        0
    }
}

/// `0x0022ea81`: the widget's rectangle, `[x0, x1, y0, y1]` in the view's
/// pixels: the header's moved by the animated offset, its two corners
/// projected at the offset's Z about the view's centre.
fn placed(
    mem: &dyn GuestRead,
    lens: &Lens,
    header: &UiWidget,
    widget: u32,
    view: [i16; 4],
) -> Result<[f32; 4], Refusal> {
    let f = |o: u32| {
        widget
            .checked_add(o)
            .and_then(|a| float(mem, a))
            .ok_or(Refusal::Unreadable)
    };
    let [ox, oy, oz] = [
        f(widget::OFFSET)?,
        f(widget::OFFSET + 4)?,
        f(widget::OFFSET + 8)?,
    ];
    let [top, left, bottom, right] = header.rect.map(f32::from);
    let near = project(lens, [left + ox, top + oy, oz], view);
    let far = project(lens, [right + ox, bottom + oy, oz], view);
    Ok([near[0], far[0], near[1], far[1]])
}

/// `0x0013c050`'s rectangle `[x0, y0, x1, y1]` in colour `argb`: one quad
/// over texel 0 of the rasterizer bitmap's second image, drawn by
/// `0x00052040`; `None` when it draws nothing (no scenario, quads off), or
/// why the game draws it.
pub fn rect_bitmap(mem: &dyn GuestRead, rect: [f32; 4], argb: u32) -> Result<Option<UiBitmap>, Refusal> {
    use Refusal::{State, Unreadable};
    let w32 = |a: Option<u32>| a.and_then(|a| word(mem, a)).ok_or(Unreadable);
    if w32(Some(at::SCENARIO))? == 0 {
        return Ok(None);
    }
    let matg = w32(Some(at::GLOBALS))?;
    let Some(rasterizer) = first(
        mem,
        matg.checked_add(globals::RASTERIZER).ok_or(Unreadable)?,
    )
    .ok_or(Unreadable)?
    else {
        // The guest reads through a null element.
        return Err(Unreadable);
    };
    let datum = w32(rasterizer.checked_add(globals::RASTERIZER_BITMAP))?;
    let data = tag_data(mem, datum).ok_or(Unreadable)?.ok_or(Unreadable)?;
    // The guest takes the second image with no count check.
    if (w32(data.checked_add(IMAGE_COUNT))? as i32) <= i32::from(METER_IMAGE) {
        return Err(State);
    }
    // The guest reads the image's record before it draws.
    w32(data.checked_add(IMAGES))?
        .checked_add(u32::from(METER_IMAGE) * IMAGE_BYTES)
        .ok_or(Unreadable)?;
    // `0x00052040`.
    // Quads off: the guest draws none. The native overlay loads the
    // bitmap by tag, so the guest's residency does not gate it.
    if short(mem, at::QUADS_OFF).ok_or(Unreadable)? != 0 {
        return Ok(None);
    }
    let shade = default_tint(mem).ok_or(Unreadable)?;
    Ok(Some(UiBitmap {
        texture: OverlayTexture::Bitmap {
            tag: datum,
            image: METER_IMAGE,
        },
        rect,
        uv: [0.0; 4],
        color: tinted(argb, shade),
        combiner: Combiner::Texture,
        sampler: Sampler {
            wrap: false,
            point: false,
        },
        blend: blend(mem, 0).ok_or(Unreadable)?,
    }))
}

/// The meter widget's draw `0x002b07e2`: one rectangle of the meter
/// colour over texel 0 of the rasterizer bitmap's second image, or why
/// the game draws it. No scenario draws nothing.
pub fn meter(
    mem: &dyn GuestRead,
    mut header: UiWidget,
    widget: u32,
    view_at: u32,
) -> Result<UiWidget, Refusal> {
    use Refusal::{State, Unreadable};
    let channel = float(mem, at::CHANNEL).ok_or(Unreadable)?;
    let argb = meter_tint(mem, channel).ok_or(Unreadable)?;
    let view = view(mem, view_at).ok_or(Unreadable)?;
    if byte(mem, at::DEBUG_LABELS).ok_or(Unreadable)? != 0 {
        return Err(State);
    }
    let lens = Lens::read(mem).ok_or(Unreadable)?;
    let [x0, x1, y0, y1] = placed(mem, &lens, &header, widget, view)?;
    let [px, py] = view_offset(view);
    let x = |v: f32| truncated(v).wrapping_add(px);
    let y = |v: f32| truncated(v).wrapping_add(py);
    let rect = [x(x0), y(y0), x(x1), y(y1)].map(f32::from);
    // `0x0013c050`.
    if let Some(quad) = rect_bitmap(mem, rect, argb)? {
        header.content = UiContent::Bitmap(quad);
    }
    Ok(header)
}

/// The bitmap widget at `widget` (`depth` the list entry's), drawn in
/// the view at `view_at` (the draw's argument): its header and the quad
/// its draw makes ([`UiContent::Bitmap`]; [`UiContent::None`] when it
/// makes none), a scrolling bitmap's quads, or why the game draws it.
/// The refusals come in the draw's order.
pub fn read(
    mem: &dyn GuestRead,
    widget: u32,
    depth: f32,
    view_at: u32,
) -> Result<Drawn, Refusal> {
    use Refusal::Unreadable;
    let mut header = walk::read_header(mem, widget, depth).map_err(|_: Unread| Unreadable)?;
    let off = |base: u32, o: u32| base.checked_add(o).ok_or(Unreadable);
    let w32 = |a: u32| word(mem, a).ok_or(Unreadable);
    let f32_at = |a: u32| float(mem, a).ok_or(Unreadable);
    let i16_at = |a: u32| short(mem, a).ok_or(Unreadable);
    let def = w32(off(widget, widget::DEFINITION)?)?;
    if def == 0 {
        return Ok(Drawn::Widget(header));
    }
    if w32(def)? & definition::METER != 0 {
        return meter(mem, header, widget, view_at).map(Drawn::Widget);
    }
    if byte(mem, at::DEBUG_LABELS).ok_or(Unreadable)? != 0 {
        return Err(Refusal::State);
    }
    let datum = w32(off(def, definition::BITMAP)?)?;
    if datum == u32::MAX {
        return Ok(Drawn::Widget(header));
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
    // The image (`0x002b0b19`): the pointer when set, named by the
    // bitmap tag that holds it ([`named_image`]).
    let pointer = w32(off(widget, widget::IMAGE_POINTER)?)?;
    let (tag, index, image) = if pointer != 0 {
        let (tag, index) = named_image(mem, datum, pointer)?.ok_or(Refusal::State)?;
        (tag, index, pointer)
    } else {
        let instance = off(w32(at::TAG_INSTANCES)?, (datum & 0xffff) * 0x10 + 8)?;
        let data = w32(instance)?;
        let count = w32(off(data, IMAGE_COUNT)?)? as i32;
        if count <= 0 {
            // No image: the guest draws nothing.
            return Ok(Drawn::Widget(header));
        }
        let asked = i16_at(off(widget, widget::IMAGE)?)?;
        let index = if asked >= 0 && i32::from(asked) < count {
            asked as u16
        } else {
            0
        };
        let image = off(w32(off(data, IMAGES)?)?, u32::from(index) * IMAGE_BYTES)?;
        (datum, index, image)
    };
    let size = [
        i16_at(off(image, IMAGE_SIZE)?)?,
        i16_at(off(image, IMAGE_SIZE + 2)?)?,
    ];
    let linear = (w32(off(image, IMAGE_FLAGS_WORD)?)? >> 16) as u16 & LINEAR != 0;
    // The rectangle (`0x0022ea81`, `0x0023618e`), then the view's offset.
    let lens = Lens::read(mem).ok_or(Unreadable)?;
    let [x0, x1, y0, y1] = placed(mem, &lens, &header, widget, view)?;
    let [px, py] = view_offset(view);
    let (px, py) = (f32::from(px), -i32::from(py) as f32);
    let r = [x0 + px, x1 + px, y0 + py, y1 + py];
    let mode = u32::from(i16_at(off(def, definition::BLEND_WORD)?)? == 1);
    // The definition's scrolling (`ucomiss`: unordered scrolls).
    let scroll = [
        f32_at(off(def, definition::SCROLL)?)?,
        f32_at(off(def, definition::SCROLL + 4)?)?,
    ];
    let scrolls = scroll != [0.0, 0.0];
    let pieces = if scrolls {
        let state = [
            f32_at(off(widget, widget::SCROLL_STATE)?)?,
            f32_at(off(widget, widget::SCROLL_STATE + 4)?)?,
        ];
        let pieces = scrolled(r, state, size, linear, lens.one);
        if pieces.is_empty() {
            return Ok(Drawn::Widget(header));
        }
        pieces
    } else {
        vec![(r, coordinates(size, linear, lens.one))]
    };
    // `0x00052040` and `0x00012360`, the same for each piece.
    // Quads off or an empty image: the guest draws nothing. The native
    // overlay loads the bitmap by tag, so residency does not gate it.
    if i16_at(at::QUADS_OFF)? != 0 || size[0] <= 0 || size[1] <= 0 {
        return Ok(Drawn::Widget(header));
    }
    let shade = default_tint(mem).ok_or(Unreadable)?;
    let blend = blend(mem, mode).ok_or(Unreadable)?;
    let mut quads = pieces.into_iter().map(|(r, uv)| UiBitmap {
        texture: OverlayTexture::Bitmap { tag, image: index },
        rect: corners(r),
        uv,
        color: tinted(argb, shade),
        combiner: Combiner::Texture,
        sampler: Sampler {
            wrap: false,
            point: false,
        },
        blend,
    });
    if scrolls {
        return Ok(Drawn::Quads(quads.collect()));
    }
    header.content = quads.next().map_or(UiContent::None, UiContent::Bitmap);
    Ok(Drawn::Widget(header))
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
    const INDEX_AT: u32 = 0xf000;

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
    /// 1 of a two-image bitmap, linear 64 by 32 and resident, scroll state
    /// (0, 0), in the full
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
        m.f(WIDGET + 0x7c, 0.0);
        m.f(WIDGET + 0x80, 0.0);
        m.w(WIDGET + 0x88, 1);
        m.w(WIDGET + 0x8c, 0);
        m.w(DEF, 0);
        m.w(DEF + 8, 1);
        m.f(DEF + 0x10, 0.0);
        m.f(DEF + 0x14, 0.0);
        m.w(DEF + 0x1c, DATUM);
        m.w(at::TAG_INSTANCES, INSTANCES);
        m.w(at::TAG_INDEX, INDEX_AT);
        m.w(INDEX_AT + 0x0c, 0xe100_0000);
        m.w(INDEX_AT + 0x18, 3);
        for slot in 0..2 {
            m.w(INSTANCES + slot * 0x10, u32::MAX);
        }
        m.w(INSTANCES + 2 * 0x10, BITM);
        m.w(INSTANCES + 2 * 0x10 + 4, DATUM);
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

    const SCENARIO_AT: u32 = 0xc000;
    const MATG: u32 = 0xd000;
    const INTERFACE_AT: u32 = 0xd800;
    const RASTERIZER_AT: u32 = 0xd900;
    const WGTZ_DATA: u32 = 0xe000;
    const WIGL_DATA: u32 = 0xe400;

    /// The bitmap widget's definition marked a meter, with a scenario of
    /// type 1, its interface's `wgtz` naming a `wigl` whose meter colour
    /// is (alpha 0.5, red 1, green 0.25, blue 0), and the rasterizer
    /// globals naming the widget's two-image bitmap.
    pub fn meter_widget(m: &mut Mem) {
        bitmap_widget(m);
        m.w(DEF, definition::METER);
        m.w(at::SCENARIO, SCENARIO_AT);
        half(m, SCENARIO_AT + 0x10, 1);
        m.w(at::GLOBALS, MATG);
        m.w(MATG + 0x108, 1);
        m.w(MATG + 0x10c, RASTERIZER_AT);
        m.w(RASTERIZER_AT + 0x64, DATUM);
        m.w(MATG + 0x110, 1);
        m.w(MATG + 0x114, INTERFACE_AT);
        m.w(INTERFACE_AT + 0x94, 0xe124_0003);
        m.w(INSTANCES + 3 * 0x10 + 8, WGTZ_DATA);
        m.w(WGTZ_DATA + 4, 0xe125_0004);
        m.w(INSTANCES + 4 * 0x10 + 8, WIGL_DATA);
        for (n, v) in [0.5, 1.0, 0.25, 0.0].into_iter().enumerate() {
            m.f(WIGL_DATA + 0x50 + 4 * n as u32, v);
        }
    }

    /// `0x002b07e2`: one rectangle, the header's placed as the bitmap's
    /// is but truncated and moved by the view's offset with its Y as it
    /// is; texel 0 of the rasterizer bitmap's second image; the meter
    /// colour truncated, not the widget's tint or alpha; blend mode 0.
    #[test]
    fn a_meter_reads_as_one_rectangle_of_the_meter_colour() {
        let mut m = Mem::default();
        meter_widget(&mut m);
        m.f(WIDGET + 0x58, 5.75);
        // A top-half view: centre (320, 120), offset (0, -16).
        half(&mut m, VIEW + 4, 240);
        let w = widget_of(&m, 0.0);
        let UiContent::Bitmap(b) = w.content else {
            panic!("not a bitmap: {:?}", w.content)
        };
        assert_eq!(
            b,
            UiBitmap {
                texture: OverlayTexture::Bitmap {
                    tag: DATUM,
                    image: METER_IMAGE
                },
                // x 275.75 and 355.75; y 10 and 50, then 16 up.
                rect: [275.0, -6.0, 355.0, 34.0],
                uv: [0.0; 4],
                color: rgba(0x7fff_3f00),
                combiner: Combiner::Texture,
                sampler: Sampler {
                    wrap: false,
                    point: false
                },
                blend: OverlayBlend::On {
                    source: 0x302,
                    dest: 0x303,
                    equation: 0x8006
                },
            }
        );
    }

    /// No shared globals for the scenario's type: each channel is the
    /// channel constant, white. No scenario: the rectangle draw makes
    /// nothing.
    #[test]
    fn a_meter_without_globals_is_white_and_without_a_scenario_draws_nothing() {
        let mut m = Mem::default();
        meter_widget(&mut m);
        half(&mut m, SCENARIO_AT + 0x10, 3);
        let Ok(UiContent::Bitmap(b)) = content(&m) else {
            panic!()
        };
        assert_eq!(b.color, [1.0; 4]);
        assert_eq!(meter_tint(&m, 255.0), Some(0xffff_ffff));
        m.w(at::SCENARIO, 0);
        assert_eq!(content(&m), Ok(UiContent::None));
    }

    /// A meter the overlay does not draw as the game does is the game's.
    #[test]
    fn a_meter_the_overlay_cannot_draw_is_refused() {
        let cases: [(&str, fn(&mut Mem), Refusal); 6] = [
            (
                "debug labels",
                |m| m.w(at::DEBUG_LABELS & !3, 1 << 24),
                Refusal::State,
            ),
            ("one image", |m| m.w(DATA + 0x44, 1), Refusal::State),
            (
                "not resident",
                |m| m.w(at::RESIDENT_AFTER, 5),
                Refusal::State,
            ),
            ("tint constant", |m| m.f(TINT_AT + 4, 0.5), Refusal::State),
            (
                "no rasterizer globals",
                |m| m.w(MATG + 0x108, 0),
                Refusal::Unreadable,
            ),
            (
                "a colour with no integer",
                |m| m.f(WIGL_DATA + 0x54, f32::INFINITY),
                Refusal::Unreadable,
            ),
        ];
        for (what, set, why) in cases {
            let mut m = Mem::default();
            meter_widget(&mut m);
            set(&mut m);
            assert_eq!(content(&m), Err(why), "{what}");
        }
    }

    /// `_ftol2` truncates toward zero and keeps the low word.
    #[test]
    fn the_meter_colour_truncates() {
        assert_eq!(ftol(127.9), Some(127));
        assert_eq!(ftol(-1.5), Some((-1i32) as u32));
        assert_eq!(ftol(4_294_967_296.0), Some(0));
        assert_eq!(ftol(f32::NAN), None);
        assert_eq!(truncated(-5.9), -5);
        assert_eq!(truncated(65_537.0), 1);
        assert_eq!(truncated(f32::NAN), 0);
    }

    fn content(m: &Mem) -> Result<UiContent, Refusal> {
        read(m, WIDGET, 0.0, VIEW).map(|d| match d {
            Drawn::Widget(w) => w.content,
            Drawn::Quads(q) => panic!("scrolled: {q:?}"),
        })
    }

    fn widget_of(m: &Mem, depth: f32) -> UiWidget {
        match read(m, WIDGET, depth, VIEW) {
            Ok(Drawn::Widget(w)) => w,
            other => panic!("not a widget: {other:?}"),
        }
    }

    fn quads(m: &Mem) -> Result<Vec<UiBitmap>, Refusal> {
        read(m, WIDGET, 0.0, VIEW).map(|d| match d {
            Drawn::Quads(q) => q,
            Drawn::Widget(w) => panic!("not scrolled: {w:?}"),
        })
    }

    /// A widget whose definition scrolls in U and V, its scroll state
    /// (0.25, 0.75): the rectangle 275 to 355 by 130 to 170 cuts at x 295
    /// and y 160 into four quads in `0x00235f31`'s order, the image
    /// wrapped by the fractions; the plain quad's tint, image and blend.
    #[test]
    fn a_scrolling_bitmap_reads_as_its_four_wrapped_quads() {
        let mut m = Mem::default();
        bitmap_widget(&mut m);
        m.f(DEF + 0x10, 0.1);
        m.f(DEF + 0x14, 0.2);
        m.f(WIDGET + 0x7c, 0.25);
        m.f(WIDGET + 0x80, 0.75);
        let q = quads(&m).unwrap();
        let got: Vec<_> = q.iter().map(|b| (b.rect, b.uv)).collect();
        assert_eq!(
            got,
            [
                ([275.0, 130.0, 295.0, 160.0], [0.75, 0.25, 1.0, 1.0]),
                ([295.0, 130.0, 355.0, 160.0], [0.0, 0.25, 0.75, 1.0]),
                ([275.0, 160.0, 295.0, 170.0], [0.75, 0.0, 1.0, 0.25]),
                ([295.0, 160.0, 355.0, 170.0], [0.0, 0.0, 0.75, 0.25]),
            ]
        );
        let plain = {
            let mut m = Mem::default();
            bitmap_widget(&mut m);
            let UiContent::Bitmap(b) = widget_of(&m, 0.0).content else {
                panic!()
            };
            b
        };
        for b in &q {
            assert_eq!(
                (b.texture, b.color, b.blend, b.sampler, b.combiner),
                (plain.texture, plain.color, plain.blend, plain.sampler, plain.combiner)
            );
        }
    }

    /// A piece under a pixel wide or high is not drawn: a state of 0 is
    /// the plain quad alone; a scroll with every piece cut away draws
    /// nothing, and its state is not checked.
    #[test]
    fn a_scrolling_bitmap_drops_its_pieces_under_a_pixel() {
        let mut m = Mem::default();
        bitmap_widget(&mut m);
        m.f(DEF + 0x10, 0.5);
        let q = quads(&m).unwrap();
        assert_eq!(q.len(), 1);
        assert_eq!((q[0].rect, q[0].uv), ([275.0, 130.0, 355.0, 170.0], [0.0, 0.0, 1.0, 1.0]));
        // 80 wide: 0.99 leaves 0.8 of a pixel on the right.
        m.f(WIDGET + 0x7c, 0.99);
        assert_eq!(quads(&m).unwrap().len(), 1);
        m.f(WIDGET + 0x7c, f32::NAN);
        m.w(at::RESIDENT_AFTER, 5);
        assert_eq!(content(&m), Ok(UiContent::None));
    }

    /// `0x00235f31`'s cuts, in its float order, and a linear image's
    /// coordinates through its texels.
    #[test]
    fn scrolled_cuts_in_the_guests_order() {
        // Cut at x 20 and y 0: the top pieces are empty.
        let r = [10.0, 30.0, 0.0, 4.0];
        assert_eq!(
            scrolled(r, [0.5, 0.0], [64, 32], false, 1.0),
            [
                ([10.0, 20.0, 0.0, 4.0], [0.5, 0.0, 1.0, 1.0]),
                ([20.0, 30.0, 0.0, 4.0], [0.0, 0.0, 0.5, 1.0]),
            ]
        );
        let cu = 1.0f32 - 0.3;
        let lin = scrolled(r, [0.3, 0.0], [3, 7], true, 1.0);
        assert_eq!(lin[0].1, [cu * 3.0 / 3.0, 0.0, 1.0, 1.0]);
        assert_eq!(lin[1].1, [0.0, 0.0, cu * 3.0 / 3.0, 1.0]);
        // An unordered fraction cuts nothing the draw keeps.
        assert!(scrolled(r, [f32::NAN, 0.0], [64, 32], false, 1.0).is_empty());
    }

    /// The draw's quad: the header's rectangle at depth 0 is where the
    /// header puts it; the tint is packed and rounded; image 1, linear,
    /// mode 1's blend, the texture combiner, clamped and linear.
    #[test]
    fn a_bitmap_widget_reads_as_the_quad_its_draw_makes() {
        let mut m = Mem::default();
        bitmap_widget(&mut m);
        let w = widget_of(&m, 0.25);
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

    /// An image set at `+0x8c` draws as the image it points at, named by
    /// the bitmap tag that holds it: the definition's own (the image
    /// index at `+0x88` not read), or another loaded bitmap's, found in
    /// the tag index and kept for the map.
    #[test]
    fn an_image_set_from_elsewhere_is_named_by_its_bitmap_tag() {
        let mut m = Mem::default();
        bitmap_widget(&mut m);
        half(&mut m, WIDGET + 0x88, 1);
        m.w(WIDGET + 0x8c, IMAGES_AT);
        let Ok(UiContent::Bitmap(own)) = content(&m) else {
            panic!("not drawn: {:?}", content(&m))
        };
        assert_eq!(own.texture, OverlayTexture::Bitmap { tag: DATUM, image: 0 });
        // Another bitmap tag in slot 1 with one 8 by 8 image, resident.
        const OTHER: u32 = 0xe200_0001;
        const OTHER_DATA: u32 = 0x7400;
        const OTHER_IMAGE: u32 = 0x8400;
        m.w(INSTANCES + 0x10, BITM);
        m.w(INSTANCES + 0x10 + 4, OTHER);
        m.w(INSTANCES + 0x10 + 8, OTHER_DATA);
        m.w(OTHER_DATA + 0x44, 1);
        m.w(OTHER_DATA + 0x48, OTHER_IMAGE);
        half(&mut m, OTHER_IMAGE + 4, 8);
        half(&mut m, OTHER_IMAGE + 6, 8);
        m.w(OTHER_IMAGE + 0x0c, 0);
        m.w(OTHER_IMAGE + 0x50, 0x9100);
        m.w(OTHER_IMAGE + 0x70, 5);
        m.w(WIDGET + 0x8c, OTHER_IMAGE);
        let Ok(UiContent::Bitmap(other)) = content(&m) else {
            panic!("not drawn: {:?}", content(&m))
        };
        assert_eq!(other.texture, OverlayTexture::Bitmap { tag: OTHER, image: 0 });
        assert_eq!(other.uv, [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(named_image(&m, DATUM, OTHER_IMAGE), Ok(Some((OTHER, 0))));
        // Its residency is the pointed image's.
        m.w(OTHER_IMAGE + 0x70, 2);
        assert_eq!(content(&m), Err(Refusal::State));
        // The kept answer is checked again: a tag whose images moved no
        // longer holds the image, which goes back to the game.
        m.w(OTHER_IMAGE + 0x70, 5);
        m.w(OTHER_DATA + 0x48, OTHER_IMAGE + 0x100);
        assert_eq!(content(&m), Err(Refusal::State));
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
    fn a_bitmap_the_overlay_cannot_draw_is_refused() {
        let cases: [(&str, fn(&mut Mem), Refusal); 9] = [
            (
                "scrolls, not resident",
                |m| {
                    m.f(DEF + 0x14, 0.5);
                    m.w(at::RESIDENT_AFTER, 5);
                },
                Refusal::State,
            ),
            (
                "debug labels",
                |m| m.w(at::DEBUG_LABELS & !3, 1 << 24),
                Refusal::State,
            ),
            (
                "image set from no loaded bitmap",
                |m| m.w(WIDGET + 0x8c, 0x8800),
                Refusal::State,
            ),
            (
                "image set between two of its tag's",
                |m| m.w(WIDGET + 0x8c, IMAGES_AT + 4),
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
