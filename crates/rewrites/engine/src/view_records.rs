//! The view records in Rust (docs/graphics/native-visibility.md, "The view
//! records"): `render_game_views` `0x002234e0` up to its call of
//! `render_views` (`0x00223714`), with each view's `0x00223080` and its
//! callees `0x0002f600` (the grid), `0x0002f640` (the view's cell and
//! span), `0x0002f6b0` (its two rectangles), `0x0002f800` (the camera) and
//! `0x0002c490` (the split globals).
//!
//! [`build`] reads the guest and returns every byte the guest writes
//! there outside its own stack, as named fields ([`Proposal`]), and the
//! registers `render_views` is entered with. It writes nothing. A call it
//! does not port is a named [`Refusal`]: the guest's body runs.
//!
//! Every constant is the image's own word, read when the guest reads it.

use guest_layout::memory::{GuestMemory, MemFault};
use crate::shadow_arm::{Proposal, Shadowed};
use x87::{F80, Mode};

/// The two frame counters `0x002234e0` counts down while positive.
pub const COUNTDOWN: u32 = 0x004e_6470;
/// The second; `0x00223080` reads it after the count.
pub const SPLIT_COUNTDOWN: u32 = 0x004e_64a0;
/// The one-shot byte `0x00223080` clears when [`SPLIT_COUNTDOWN`] is out.
pub const SPLIT_ONE_SHOT: u32 = 0x004e_6098;
/// The display settings; byte `+5` forces one view.
pub const DISPLAY: u32 = 0x0051_0c50;
/// The player globals: `+8` the local player count (`i16`), `+0xc` the
/// four local players (`-1` none).
pub const PLAYER_GLOBALS: u32 = 0x004e_8c20;
/// The camera override: `+1` its lens byte, `+2` a `u16`, `+8` its kind,
/// `+0x1120` its active byte.
pub const CAMERA_OVERRIDE: u32 = 0x004e_6948;
pub const WIDESCREEN: u32 = 0x0048_5ac2;
pub const RECORDS: u32 = 0x0047_f210;
pub const RECORD_SIZE: u32 = 0x118;
pub const ACTIVE_BSP_INDEX: u32 = 0x0046_86c4;
pub const OBSERVERS: u32 = 0x004e_9c8c;
pub const OBSERVER_SIZE: u32 = 0x358;
/// The screen rectangle (`i16` top, left, bottom, right) and, after it,
/// the inner one.
pub const SCREEN: u32 = 0x0048_5a8a;
pub const INNER: u32 = 0x0048_5a92;
pub const NEAR: u32 = 0x0048_5ad4;
pub const FAR: u32 = 0x0048_5ad8;
/// `0x0002f800`'s field-of-view factors: by default and for a shape ratio
/// of 200 or 50.
pub const FOV_FACTOR: u32 = 0x0045_083c;
pub const FOV_FACTOR_200: u32 = 0x0045_dbbc;
pub const FOV_FACTOR_50: u32 = 0x0044_ae88;
/// The default camera's position, forward and up, each a pointer to three
/// floats.
pub const DEFAULT_POSITION: u32 = 0x0046_8788;
pub const DEFAULT_FORWARD: u32 = 0x0046_87a8;
pub const DEFAULT_UP: u32 = 0x0046_87b0;
pub const ONE: u32 = 0x0045_dbc0;
/// The default field of view and the x87 terms of its horizontal angle.
pub const DEFAULT_FOV: u32 = 0x0052_34dc;
pub const HALF: u32 = 0x0045_dbbc;
pub const FOV_ASPECT: u32 = 0x0045_dc2c;
/// A double.
pub const FOV_UNIT: u32 = 0x0045_dcc0;
/// `0x0002c490`'s outputs and inputs.
pub mod split {
    pub const SCALE: u32 = 0x0046_70e4;
    pub const MODE: u32 = 0x004b_a01c;
    pub const FLAGS: u32 = 0x004b_a020;
    pub const FACTOR: u32 = 0x004b_a028;
    pub const TERM: u32 = 0x004b_a02c;
    pub const ONE_SCALE: u32 = 0x0045_dc30;
    pub const FOUR_SCALE: u32 = 0x0045_dc20;
    pub const FOUR_FACTOR: u32 = 0x0045_dd1c;
    pub const TWO_SCALE: u32 = 0x0044_ae90;
}

/// In a record.
pub mod record {
    /// `+0` the view's kind (0 a game view, 1 the frame's last record),
    /// `+4` the view index, `+8` the local player.
    pub const HEADER: u32 = 0x0;
    /// The render camera, `0x74` bytes copied from [`SCRATCH`].
    pub const CAMERA: u32 = 0xc;
    pub const SCRATCH: u32 = 0x80;
    pub const CAMERA_SIZE: u32 = 0x74;
    /// In the camera: the viewport and the inner rectangle.
    pub const VIEWPORT: u32 = 0x30;
    pub const INNER: u32 = 0x38;
}

/// `render_views`'s entry, as the guest calls it from `0x00223714`.
pub const RENDER_VIEWS: u32 = 0x0002_b5d0;
/// The return address of that call.
pub const RENDER_VIEWS_RETURN: u32 = 0x0022_3719;

/// Why a call is left to the guest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Memory it reads or writes does not read.
    Unread,
    /// An MXCSR other than masked round-to-nearest, an x87 control word
    /// other than `0x027f`, or a used x87 stack.
    FloatState,
    /// The camera override is active: `0x00223080` builds a local camera
    /// through `0x0014a250`.
    Override,
    /// The one-shot byte is set while the split countdown runs:
    /// `0x00223080` calls `0x0003f450`.
    OneShot,
    /// A division the guest would fault on.
    Divide,
    /// The default camera's tangent is out of the x87's range.
    Tangent,
}

impl Refusal {
    pub const ALL: [Refusal; 6] = [
        Self::Unread,
        Self::FloatState,
        Self::Override,
        Self::OneShot,
        Self::Divide,
        Self::Tangent,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Unread => "unread",
            Self::FloatState => "float_state",
            Self::Override => "override",
            Self::OneShot => "one_shot",
            Self::Divide => "divide",
            Self::Tangent => "tangent",
        }
    }
}

impl From<MemFault> for Refusal {
    fn from(_: MemFault) -> Self {
        Self::Unread
    }
}

type Result<T> = std::result::Result<T, Refusal>;

/// The float state the call is made in.
#[derive(Clone, Copy, Debug)]
pub struct Float {
    pub mxcsr: u32,
    pub x87_control: u16,
    pub x87_tag: u16,
}

/// The registers and arguments `render_views` is entered with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderViewsCall {
    pub eax: u32,
    pub ecx: u32,
    pub edx: u32,
    pub ebx: u32,
    pub esi: u32,
    pub edi: u32,
    pub ebp: u32,
    /// `xmm0`'s low lane (the upper lanes are zero).
    pub xmm0: u32,
    /// The view count, the split mode and the records.
    pub args: [u32; 3],
}

/// What [`build`] makes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Built {
    pub proposal: Proposal,
    pub call: RenderViewsCall,
}

/// Field names, one set per record (four views and the last record).
const HEADER: [&str; 5] = ["record0_header", "record1_header", "record2_header", "record3_header", "record4_header"];
const RECTS: [&str; 5] = ["record0_rects", "record1_rects", "record2_rects", "record3_rects", "record4_rects"];
const SCRATCH: [&str; 5] = ["record0_scratch", "record1_scratch", "record2_scratch", "record3_scratch", "record4_scratch"];
const CAMERA: [&str; 5] = ["record0_camera", "record1_camera", "record2_camera", "record3_camera", "record4_camera"];

fn sx(v: u16) -> i32 {
    i32::from(v as i16)
}

fn div(a: i32, b: i32) -> Result<(i32, i32)> {
    Ok((a.checked_div(b).ok_or(Refusal::Divide)?, a.checked_rem(b).ok_or(Refusal::Divide)?))
}

/// `0x0002f600`: the grid's columns and rows for `count` views.
pub fn grid(split: u32, count: i32) -> (u16, u16) {
    let columns_first = split == 1;
    let (mut a, mut c) = (1i32, 1i32);
    if count > 1 {
        loop {
            let grow = if columns_first { a < c } else { a <= c };
            if grow {
                a += 1;
            } else {
                a = 1;
                c += 1;
            }
            if c.wrapping_mul(a) >= count {
                break;
            }
        }
    }
    (a as u16, c as u16)
}

/// `0x0002f640`: the cell span and the cell of view `view`.
pub fn cell(view: i32, grid: (u16, u16), count: i32, split: u32) -> Result<((u16, u16), (u16, u16))> {
    let mut span = (1u16, 1u16);
    let cells = sx(grid.0).wrapping_mul(sx(grid.1));
    let mut skip = 0;
    if count < cells {
        if split == 1 {
            if view == 0 {
                span.0 = 2;
            } else {
                skip = 1;
            }
        } else if view == 0 {
            span.1 = 2;
        } else {
            skip = div(view, sx(grid.0))?.0;
        }
    }
    let at = skip.wrapping_add(view);
    let (row, column) = div(at, sx(grid.0))?;
    Ok((span, (column as u16, row as u16)))
}

/// `0x0002f6b0`: the view's viewport and inner rectangle (`i16` top, left,
/// bottom, right) for its cell `pos` and `span` in `grid`.
pub fn rects(m: &dyn GuestMemory, pos: (u16, u16), grid: (u16, u16), span: (u16, u16)) -> Result<([u16; 4], [u16; 4])> {
    let s = |k: u32| m.u16(SCREEN + 2 * k);
    let (st, sl, sb, sr) = (s(0)?, s(1)?, s(2)?, s(3)?);
    let (it, il, ib, ir) = (s(4)?, s(5)?, s(6)?, s(7)?);
    let cw = div(sx(ir) - sx(il), sx(grid.0))?.0 as u16;
    let ch = div(sx(ib) - sx(it), sx(grid.1))?.0 as u16;
    let left = pos.0.wrapping_mul(cw).wrapping_add(il);
    let right = span.0.wrapping_add(pos.0).wrapping_mul(cw).wrapping_add(il);
    let top = pos.1.wrapping_mul(ch).wrapping_add(it);
    let bottom = span.1.wrapping_add(pos.1).wrapping_mul(ch).wrapping_add(it);
    let mut outer = [top, left, bottom, right];
    let mut inner = outer;
    if pos.0 == 0 {
        outer[1] = sl;
    } else {
        inner[1] = inner[1].wrapping_add(4);
    }
    if sx(pos.0) + sx(span.0) >= sx(grid.0) {
        outer[3] = sr;
    } else {
        inner[3] = inner[3].wrapping_sub(4);
    }
    if pos.1 == 0 {
        outer[0] = st;
    } else {
        inner[0] = inner[0].wrapping_add(4);
    }
    if sx(pos.1) + sx(span.1) >= sx(grid.1) {
        outer[2] = sb;
    } else {
        inner[2] = inner[2].wrapping_sub(4);
    }
    Ok((outer, inner))
}

/// The default camera's angle (`0x0002f800`'s and `0x002234e0`'s x87):
/// `2 · atan2(tan(fov · half) · aspect, unit)`, under the control word.
fn default_angle(m: &dyn GuestMemory, mode: Mode) -> Result<u32> {
    let single = |a: u32| -> Result<F80> { Ok(F80::from_f32_bits(m.u32(a)?)) };
    let product = x87::mul(single(DEFAULT_FOV)?, single(HALF)?, mode).0;
    let tangent = x87::tan(product);
    if tangent.out_of_range {
        return Err(Refusal::Tangent);
    }
    let y = x87::mul(tangent.value, single(FOV_ASPECT)?, mode).0;
    let unit = u64::from(m.u32(FOV_UNIT)?) | (u64::from(m.u32(FOV_UNIT + 4)?) << 32);
    let angle = x87::patan(y, F80::from_f64_bits(unit)).0;
    Ok(x87::add(angle, angle, mode).0.to_f32_bits(mode).0)
}

fn vec3_from(s: &mut Shadowed<'_>, name: &'static str, to: u32, from: u32) -> Result<()> {
    for k in 0..3 {
        let w = s.u32(from + 4 * k)?;
        s.put_u32(name, to + 4 * k, w)?;
    }
    Ok(())
}

/// `0x0002f800` into the camera at `out`: from `observer`, or the default
/// camera when it is 0.
fn camera(s: &mut Shadowed<'_>, name: &'static str, out: u32, observer: u32, span: (u16, u16), grid: (u16, u16), mode: Mode) -> Result<()> {
    if observer == 0 {
        let p = s.u32(DEFAULT_POSITION)?;
        vec3_from(s, name, out, p)?;
        let f = s.u32(DEFAULT_FORWARD)?;
        vec3_from(s, name, out + 0xc, f)?;
        let u = s.u32(DEFAULT_UP)?;
        vec3_from(s, name, out + 0x18, u)?;
        let one = s.u32(ONE)?;
        s.put_u32(name, out + 0x2c, one)?;
        let angle = default_angle(s, mode)?;
        s.put_u32(name, out + 0x28, angle)?;
    } else {
        vec3_from(s, name, out, observer)?;
        vec3_from(s, name, out + 0xc, observer + 0x20)?;
        vec3_from(s, name, out + 0x18, observer + 0x2c)?;
        let (fov, other) = (s.u32(observer + 0x4c)?, s.u32(observer + 0x50)?);
        s.put_u32(name, out + 0x28, fov)?;
        s.put_u32(name, out + 0x2c, other)?;
        let a = sx(grid.1).wrapping_mul(sx(span.0)).wrapping_mul(100);
        let b = sx(span.1).wrapping_mul(sx(grid.0));
        let factor = match div(a, b)?.0 {
            200 => FOV_FACTOR_200,
            50 => FOV_FACTOR_50,
            _ => FOV_FACTOR,
        };
        let factor = s.f32(factor)?;
        let fov = s.f32(out + 0x28)? * factor;
        s.put_u32(name, out + 0x28, fov.to_bits())?;
        let other = s.f32(out + 0x2c)? * factor;
        s.put_u32(name, out + 0x2c, other.to_bits())?;
    }
    s.put_u8(name, out + 0x24, 0)?;
    let near = s.u32(NEAR)?;
    s.put_u32(name, out + 0x40, near)?;
    let far = s.u32(FAR)?;
    s.put_u32(name, out + 0x44, far)?;
    s.put_u8(name, out + 0x58, 0)?;
    Ok(())
}

/// `0x0002c490` for `count` views.
fn split_globals(s: &mut Shadowed<'_>, count: u32) -> Result<()> {
    const N: &str = "split_globals";
    let (scale, term, mode, flag, factor) = match count {
        2 | 3 => (split::TWO_SCALE, FOV_FACTOR_200, 3, 0u8, FOV_FACTOR_50),
        4 => (split::FOUR_SCALE, split::FOUR_SCALE, 2, 0, split::FOUR_FACTOR),
        _ => (split::ONE_SCALE, ONE, 4, 1, ONE),
    };
    let v = s.u32(scale)?;
    s.put_u32(N, split::SCALE, v)?;
    let v = s.u32(term)?;
    s.put_u32(N, split::TERM, v)?;
    s.put_u32(N, split::MODE, mode)?;
    s.put_u8(N, split::FLAGS + 1, flag ^ 1)?;
    for k in [0, 2, 4, 3, 5] {
        s.put_u8(N, split::FLAGS + k, flag)?;
    }
    let v = s.u32(factor)?;
    s.put_u32(N, split::FACTOR, v)?;
    Ok(())
}

/// Copies the camera's `0x74` bytes from the scratch to the record.
fn copy_camera(s: &mut Shadowed<'_>, name: &'static str, rec: u32) -> Result<()> {
    let bytes = s.bytes(rec + record::SCRATCH, record::CAMERA_SIZE as usize)?;
    s.put(name, rec + record::CAMERA, &bytes)?;
    Ok(())
}

/// `0x00223080` for view `view` into record `index`.
#[allow(clippy::too_many_arguments)]
fn view_record(s: &mut Shadowed<'_>, index: usize, view: u32, player: u32, observer: u32, count: i32, split: u32, mode: Mode) -> Result<()> {
    let rec = RECORDS + RECORD_SIZE * index as u32;
    for (k, w) in [0, view, player].into_iter().enumerate() {
        s.put_u32(HEADER[index], rec + 4 * k as u32, w)?;
    }
    let o = s.u32(CAMERA_OVERRIDE)?;
    if o != 0 && s.u8(o + 0x1120)? != 0 && s.u32(o + 8)? == 3 {
        return Err(Refusal::Override);
    }
    let g = grid(split, count);
    let (span, pos) = cell(view as i32, g, count, split)?;
    let (outer, inner) = rects(s, pos, g, span)?;
    let words: Vec<u8> = outer.iter().chain(&inner).flat_map(|w| w.to_le_bytes()).collect();
    s.put(RECTS[index], rec + record::SCRATCH + record::VIEWPORT, &words)?;
    camera(s, SCRATCH[index], rec + record::SCRATCH, observer, span, g, mode)?;
    split_globals(s, count as u32)?;
    if s.i32(SPLIT_COUNTDOWN)? <= 0 {
        s.put_u8("one_shot", SPLIT_ONE_SHOT, 0)?;
    } else if s.u8(SPLIT_ONE_SHOT)? != 0 {
        return Err(Refusal::OneShot);
    }
    copy_camera(s, CAMERA[index], rec)
}

/// `0x002234e0` up to its `render_views` call, in `float`.
pub fn build(m: &dyn GuestMemory, float: Float) -> Result<Built> {
    if float.mxcsr & !0x3f != 0x1f80 || float.x87_control != 0x027f || float.x87_tag != u16::MAX {
        return Err(Refusal::FloatState);
    }
    let mode = Mode::from_cw(float.x87_control);
    let mut s = Shadowed::new(m);
    for counter in [COUNTDOWN, SPLIT_COUNTDOWN] {
        let v = s.i32(counter)?;
        if v > 0 {
            s.put_u32("countdowns", counter, (v - 1) as u32)?;
        }
    }
    let display = s.u32(DISPLAY)?;
    let count: i32 = if display != 0 && s.u8(display + 5)? != 0 {
        1
    } else {
        let players = s.u32(PLAYER_GLOBALS)?;
        let n = s.u16(players + 8)? as i16;
        if n < 1 { 1 } else if n > 4 { 4 } else { i32::from(n) }
    };
    let split = if count > 1 {
        let o = s.u32(CAMERA_OVERRIDE)?;
        let lens = o != 0 && s.u8(o + 1)? != 0 && s.u16(o + 2)? != u16::MAX && s.u32(o + 8)? == 3;
        if !lens && s.u8(WIDESCREEN)? != 0 { 2 } else { 1 }
    } else {
        0
    };
    let mut player: i32 = -1;
    for view in 0..count {
        player += 1;
        let mut observer = 0;
        if player >= 4 {
            player = -1;
        } else {
            let players = s.u32(PLAYER_GLOBALS)?;
            loop {
                if s.u32(players + 0xc + 4 * player as u32)? != u32::MAX {
                    break;
                }
                player += 1;
                if player >= 4 {
                    player = -1;
                    break;
                }
            }
            if player >= 0 && s.u16(ACTIVE_BSP_INDEX)? != u16::MAX {
                observer = OBSERVERS + OBSERVER_SIZE * player as u32;
            }
        }
        view_record(&mut s, view as usize, view as u32, player as u32, observer, count, split, mode)?;
    }
    // The frame's last record, after the views.
    let last = count as usize;
    let rec = RECORDS + RECORD_SIZE * last as u32;
    let name = HEADER[last];
    s.put_u32(name, rec + 4, u32::MAX)?;
    s.put_u32(name, rec + 8, u32::MAX)?;
    let screen = s.bytes(SCREEN, 16)?;
    s.put(RECTS[last], rec + record::SCRATCH + record::VIEWPORT, &screen)?;
    s.put_u32(name, rec, 1)?;
    let scratch = rec + record::SCRATCH;
    let n = SCRATCH[last];
    let p = s.u32(DEFAULT_POSITION)?;
    vec3_from(&mut s, n, scratch, p)?;
    let f = s.u32(DEFAULT_FORWARD)?;
    vec3_from(&mut s, n, scratch + 0xc, f)?;
    let up = s.u32(DEFAULT_UP)?;
    let ebx = s.u32(up + 4)?;
    vec3_from(&mut s, n, scratch + 0x18, up)?;
    let one = s.u32(ONE)?;
    s.put_u32(n, scratch + 0x2c, one)?;
    let near = s.u32(NEAR)?;
    s.put_u32(n, scratch + 0x40, near)?;
    s.put_u8(n, scratch + 0x24, 0)?;
    let far = s.u32(FAR)?;
    s.put_u32(n, scratch + 0x44, far)?;
    s.put_u8(n, scratch + 0x58, 0)?;
    let angle = default_angle(&s, mode)?;
    s.put_u32(n, scratch + 0x28, angle)?;
    copy_camera(&mut s, CAMERA[last], rec)?;
    let call = RenderViewsCall {
        eax: count as u32 + 1,
        ecx: 3,
        edx: count as u32 + 1,
        ebx,
        esi: scratch + record::CAMERA_SIZE,
        edi: rec + record::CAMERA + record::CAMERA_SIZE,
        ebp: count as u32,
        xmm0: far,
        args: [count as u32, split, RECORDS],
    };
    Ok(Built { proposal: s.proposal, call })
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    #[test]
    fn the_grid_grows_columns_or_rows_by_the_split() {
        assert_eq!(grid(0, 1), (1, 1));
        assert_eq!(grid(2, 2), (2, 1), "side by side");
        assert_eq!(grid(1, 2), (1, 2), "stacked");
        assert_eq!(grid(2, 3), (2, 2));
        assert_eq!(grid(2, 4), (2, 2));
    }

    #[test]
    fn with_three_views_the_first_spans_two_cells() {
        assert_eq!(cell(0, (2, 2), 3, 1).unwrap(), ((2, 1), (0, 0)));
        assert_eq!(cell(1, (2, 2), 3, 1).unwrap(), ((1, 1), (0, 1)));
        assert_eq!(cell(2, (2, 2), 3, 1).unwrap(), ((1, 1), (1, 1)));
        assert_eq!(cell(0, (2, 2), 3, 2).unwrap(), ((1, 2), (0, 0)));
        assert_eq!(cell(3, (0, 2), 3, 0), Err(Refusal::Divide));
    }

    /// The fixture: 640 by 480, an inner rectangle 8 in from each edge, one
    /// player whose observer looks down +x.
    pub(crate) fn image() -> FakeGuest {
        let mut m = FakeGuest::new();
        for (k, v) in [0u16, 0, 480, 640, 8, 8, 472, 632].into_iter().enumerate() {
            m.put_u16(SCREEN + 2 * k as u32, v);
        }
        m.put_u32(DISPLAY, 0);
        m.put_u32(PLAYER_GLOBALS, 0x0060_0000);
        m.put_u16(0x0060_0008, 1);
        m.put_u32(0x0060_000c, 0x0003_0002);
        for k in 1..4 {
            m.put_u32(0x0060_000c + 4 * k, u32::MAX);
        }
        m.put_u32(CAMERA_OVERRIDE, 0);
        m.put_u16(ACTIVE_BSP_INDEX, 0);
        m.put_f32(OBSERVERS + 0x20, 1.0);
        m.put_f32(OBSERVERS + 0x34, 1.0);
        m.put_f32(OBSERVERS + 0x4c, 1.2);
        m.put_f32(OBSERVERS + 0x50, 0.9);
        m.put_f32(FOV_FACTOR, 1.0);
        m.put_f32(FOV_FACTOR_200, 0.5);
        m.put_f32(FOV_FACTOR_50, 0.75);
        m.put_f32(NEAR, 0.0625);
        m.put_f32(FAR, 1024.0);
        m.put_f32(ONE, 1.0);
        m.put_f32(DEFAULT_FOV, 1.2);
        m.put_f32(FOV_ASPECT, 4.0 / 3.0);
        m.put_u32(FOV_UNIT, 0);
        m.put_u32(FOV_UNIT + 4, 0x3ff0_0000);
        for p in [DEFAULT_POSITION, DEFAULT_FORWARD, DEFAULT_UP] {
            m.put_u32(p, 0x0061_0000);
        }
        m.map(0x0061_0000, 12);
        m.map(RECORDS, RECORD_SIZE * 5);
        m.map(OBSERVERS, OBSERVER_SIZE * 4);
        m.map(split::MODE, 0x14);
        m.put_u8(WIDESCREEN, 0);
        m.put_u32(COUNTDOWN, 0);
        m.put_u32(SPLIT_COUNTDOWN, 0);
        m.put_u8(SPLIT_ONE_SHOT, 0);
        for (a, v) in [(split::SCALE, 0.0), (split::ONE_SCALE, 1.0), (split::FOUR_SCALE, 0.5), (split::FOUR_FACTOR, 0.5), (split::TWO_SCALE, 0.7)] {
            m.put_f32(a, v);
        }
        m
    }

    fn float() -> Float {
        Float { mxcsr: 0x1f80, x87_control: 0x027f, x87_tag: u16::MAX }
    }

    #[test]
    fn one_view_builds_its_record_and_the_last_one() {
        let m = image();
        let built = build(&m, float()).unwrap();
        let mut out = m.clone();
        built.proposal.apply(&mut out).unwrap();
        let rec = RECORDS;
        assert_eq!(out.u32(rec + 4).unwrap(), 0, "the view index");
        assert_eq!(out.u32(rec + 8).unwrap(), 0, "the player");
        assert_eq!(out.f32(rec + record::CAMERA + 0xc).unwrap(), 1.0, "forward from the observer");
        assert_eq!(out.f32(rec + record::CAMERA + 0x28).unwrap(), 1.2);
        let viewport: Vec<u16> = (0..8).map(|k| out.u16(rec + record::CAMERA + record::VIEWPORT + 2 * k).unwrap()).collect();
        assert_eq!(viewport, [0, 0, 480, 640, 8, 8, 472, 632], "a whole-screen view keeps the screen's edges");
        let last = RECORDS + RECORD_SIZE;
        assert_eq!(out.u32(last).unwrap(), 1);
        assert_eq!(out.u32(last + 4).unwrap(), u32::MAX);
        let angle = out.f32(last + record::CAMERA + 0x28).unwrap();
        let want = 2.0 * ((1.2f64 * 0.5).tan() * (4.0 / 3.0)).atan();
        assert!((f64::from(angle) - want).abs() < 1e-6, "{angle} {want}");
        assert_eq!(built.call.args, [1, 0, RECORDS]);
        assert_eq!(built.call.eax, 2);
        assert!(built.proposal.differing(&out).is_empty());
        assert!(!built.proposal.differing(&m).is_empty());
    }

    #[test]
    fn two_side_by_side_views_take_the_split_factor_and_borders() {
        let mut m = image();
        m.put_u16(0x0060_0008, 2);
        m.put_u32(0x0060_0010, 0x0004_0003);
        m.put_u8(WIDESCREEN, 1);
        let built = build(&m, float()).unwrap();
        let mut out = m.clone();
        built.proposal.apply(&mut out).unwrap();
        let second = RECORDS + RECORD_SIZE;
        let at = |rec: u32, k: u32| out.u16(rec + record::CAMERA + record::VIEWPORT + 2 * k).unwrap();
        assert_eq!((0..4).map(|k| at(RECORDS, k)).collect::<Vec<_>>(), [0, 0, 480, 320]);
        assert_eq!((4..8).map(|k| at(RECORDS, k)).collect::<Vec<_>>(), [8, 8, 472, 316]);
        assert_eq!((0..4).map(|k| at(second, k)).collect::<Vec<_>>(), [0, 320, 480, 640]);
        assert_eq!(out.u32(second + 8).unwrap(), 1, "the second local player");
        assert_eq!(out.f32(RECORDS + record::CAMERA + 0x28).unwrap(), 1.2 * 0.75, "ratio 50");
        assert_eq!(built.call.args[1], 2);
    }

    #[test]
    fn calls_it_does_not_port_refuse_by_name() {
        let mut m = image();
        m.put_u32(CAMERA_OVERRIDE, 0x0062_0000);
        m.put_u8(0x0062_1120, 1);
        m.put_u32(0x0062_0008, 3);
        assert_eq!(build(&m, float()), Err(Refusal::Override));
        let mut m = image();
        m.put_u32(SPLIT_COUNTDOWN, 2);
        m.put_u8(SPLIT_ONE_SHOT, 1);
        assert_eq!(build(&m, float()), Err(Refusal::OneShot));
        let m = image();
        assert_eq!(build(&m, Float { x87_control: 0x037f, ..float() }), Err(Refusal::FloatState));
        assert_eq!(Refusal::ALL.map(Refusal::name).len(), 6);
    }
}
