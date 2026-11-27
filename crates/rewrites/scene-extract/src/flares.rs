//! The flare visibility array (docs/graphics/d3d-visibility.md, "The
//! query in Rust"): its entries, the query quad the callback `0x0002e540`
//! projects for each (with `0x00048b00`), and the byte the reader
//! `0x00020770` carries from a query's count. Pure reads and arithmetic;
//! the hooks and the writes are `render_bridge::native_flare_query`'s.
//!
//! | Guest | Here |
//! |---|---|
//! | entries `0x0051f40c + 32 i`, count `[0x005233ec]` | [`Entry`] |
//! | the walker's gates (`0x00020bb0`) | [`walk_frame`] |
//! | the reader's gates (`0x00020770`) | [`read_gate`] |
//! | `0x0002e540` + `0x00048b00` | [`Lens::query`] |
//! | the reader's byte and its smoothing | [`read_byte`] |
//! | the draw pass's late list (`0x0002f260`) | [`late_glows`] |
//! | the glow `0x00046ea0`: fade, rectangle, cull, rings | [`glow`], [`Glow::rings`] |
//!
//! Three query sets, two frames apart: the walker ends the test of entry
//! `i` in slot `set + 3 i` with `set = frame % 3` and marks bit `i` of the
//! set's bitmap; the reader reads set `(frame + 1) % 3`, the one written
//! two frames before ([`query_set`], [`read_set`]).

use crate::GuestRead;

/// The array's enable byte; the reader also needs the next byte.
pub const ON: u32 = 0x0051_f408;
pub const ENTRIES: u32 = 0x0051_f40c;
pub const ENTRY_SIZE: u32 = 32;
pub const COUNT: u32 = 0x0052_33ec;
/// Three bitmaps of 64 bytes: bit `i` of set `s` is entry `i`'s test
/// ended in slot `s + 3 i`.
pub const ENDED: u32 = 0x0052_33f0;
pub const ENDED_SET_BYTES: u32 = 64;
/// The frame the array was last built for.
pub const ARRAY_FRAME: u32 = 0x0052_34b0;
/// The frame each view (0..4) was last read for.
pub const READ_FRAME: u32 = 0x0052_34b4;
/// The view the array is current for (`-1`: none).
pub const CURRENT_VIEW: u32 = 0x0052_34c4;
pub const FRAME: u32 = 0x0048_5aa0;
/// The tag instance table: `[TAGS] + 16 (datum & 0xffff) + 8` holds a
/// tag's data.
pub const TAGS: u32 = 0x004e_3b44;
/// Bitmap capacity: 16 words a set.
pub const MAX_ENTRIES: u32 = ENDED_SET_BYTES * 8;

/// The camera's forward vector (`0x0002e540`'s mode 0).
pub const CAMERA_FORWARD: u32 = 0x004b_9dac;
/// `0x00048b00`'s inputs: the viewport shorts (`y0, x0, y1, x1`), the
/// world scale, the far (w divisor), the view's 4x3 and the projection.
pub const VIEWPORT: u32 = 0x0048_5648;
pub const FAR: u32 = 0x0048_565c;
pub const WORLD_SCALE: u32 = 0x0048_568c;
pub const VIEW: u32 = 0x0048_5690;
pub const PROJECTION: u32 = 0x0048_5704;

/// The `.rdata` constants the two functions read.
pub mod k {
    pub const ONE: u32 = 0x0045_dbc0;
    pub const HALF: u32 = 0x0045_dbbc;
    pub const TWO: u32 = 0x0045_dc0c;
    /// The projection's least radius and depth.
    pub const EPSILON: u32 = 0x0045_dbd8;
    /// The half size's clamp: `[TWO, HALF_MAX]`.
    pub const HALF_MAX: u32 = 0x0045_dd60;
    pub const AREA_MAX: u32 = 0x0045_e45c;
    /// The depth format's full scale.
    pub const Z_SCALE: u32 = 0x0045_dd40;
    /// A direction byte's scale (`b k - 1`).
    pub const DIRECTION: u32 = 0x0045_dd58;
    /// Mode 1's offset along the direction, times the radius.
    pub const ALONG: u32 = 0x0045_dfe4;
    /// Class 4's radius factors after `ONE` and `HALF`.
    pub const CLASS4: [u32; 5] = [0x0045_dc20, 0x0045_dcc8, 0x0045_df78, 0x0045_de50, 0x0045_de4c];
    /// The reader's byte scale and its largest byte.
    pub const BYTE: u32 = 0x0045_dd5c;
    pub const BYTE_MAX: u32 = 0x0045_dc38;
}

/// The walker's query set for `frame`.
pub fn query_set(frame: u32) -> u32 {
    (frame as i32).rem_euclid(3) as u32
}

/// The reader's set for `frame`: the one the walker wrote two frames
/// before.
pub fn read_set(frame: u32) -> u32 {
    (frame as i32).wrapping_add(1).rem_euclid(3) as u32
}

/// Entry `index`'s test slot in `set`.
pub fn slot(set: u32, index: u32) -> u32 {
    set + 3 * index
}

pub fn entry_addr(index: u32) -> u32 {
    ENTRIES + ENTRY_SIZE * index
}

/// The bitmap word and bit of entry `index` in `set`.
pub fn ended_bit(set: u32, index: u32) -> (u32, u32) {
    (ENDED + set * ENDED_SET_BYTES + 4 * (index >> 5), 1 << (index & 31))
}

fn byte(mem: &dyn GuestRead, addr: u32) -> Option<u8> {
    let word = mem.read_u32(addr & !3)?;
    Some((word >> (8 * (addr & 3))) as u8)
}

fn half(mem: &dyn GuestRead, addr: u32) -> Option<u16> {
    let word = mem.read_u32(addr & !3)?;
    Some((word >> (8 * (addr & 2))) as u16)
}

/// One 32-byte entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub index: u32,
    /// Dword 0: bits 1-2 the view, 3-5 the class, byte 3 the visibility.
    pub head: u32,
    /// Dword 4: byte 4 the flags (bit 1 active), the u16 at 6 the area.
    pub flags: u32,
    /// The payload at `+8` the callback gets; its first word is the lens
    /// tag's datum.
    pub datum: u32,
}

impl Entry {
    pub fn read(mem: &dyn GuestRead, index: u32) -> Option<Self> {
        let at = entry_addr(index);
        Some(Self { index, head: mem.read_u32(at)?, flags: mem.read_u32(at + 4)?, datum: mem.read_u32(at + 8)? })
    }

    pub fn view(&self) -> u32 {
        (self.head >> 1) & 3
    }

    pub fn class(&self) -> u32 {
        (self.head >> 3) & 7
    }

    pub fn byte(&self) -> u8 {
        (self.head >> 24) as u8
    }

    pub fn active(&self) -> bool {
        self.flags & 2 != 0
    }

    /// The expected count the walker stored (`+6`).
    pub fn area(&self) -> u16 {
        (self.flags >> 16) as u16
    }

    pub fn payload(&self) -> u32 {
        entry_addr(self.index) + 8
    }
}

/// The entry count, or `None` past the bitmaps.
pub fn count(mem: &dyn GuestRead) -> Option<u32> {
    let n = mem.read_u32(COUNT)? as i32;
    (n <= MAX_ENTRIES as i32).then_some(n.max(0) as u32)
}

/// The walker's frame when it runs for `view` (`0x00020bb0`'s gates: the
/// array on, built this frame, current for `view`); `None` when it does
/// nothing.
pub fn walk_frame(mem: &dyn GuestRead, view: u32) -> Option<u32> {
    let frame = mem.read_u32(FRAME)?;
    let current = mem.read_u32(CURRENT_VIEW)?;
    (byte(mem, ON)? != 0 && mem.read_u32(ARRAY_FRAME)? == frame && current != u32::MAX && current == view)
        .then_some(frame)
}

/// The reader's view and frame (`0x00020770`'s gates: both enable bytes,
/// a current view, a frame later than the view's last read); `None` when
/// it does nothing.
pub fn read_gate(mem: &dyn GuestRead) -> Option<(u32, u32)> {
    if byte(mem, ON)? == 0 || byte(mem, ON + 1)? == 0 {
        return None;
    }
    let view = mem.read_u32(CURRENT_VIEW)?;
    if view == u32::MAX || view >= 4 {
        return None;
    }
    let frame = mem.read_u32(FRAME)?;
    ((frame as i32) > mem.read_u32(READ_FRAME + 4 * view)? as i32).then_some((view, frame))
}

/// A query quad in the target's pixels: `[x0, x1) x [y0, y1)` at the
/// vertices' `z` and `w` (depth units, `[0, z scale]`), and its expected
/// count.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quad {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
    pub z: f32,
    pub w: f32,
    pub area: u32,
}

/// Why a quad cannot be made as the guest makes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unmade {
    /// A word did not read.
    Unread,
    /// Class 4's radius table index is outside the seven the guest builds.
    Table,
}

/// What `0x00048b00` and `0x0002e540` read: the view, the projection, the
/// viewport and the constants.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lens {
    /// `y0, x0, y1, x1`.
    pub viewport: [i16; 4],
    pub world_scale: f32,
    pub view: [f32; 12],
    pub projection: [f32; 16],
    pub far: f32,
    pub forward: [f32; 3],
    pub one: f32,
    pub half: f32,
    pub two: f32,
    pub epsilon: f32,
    pub half_max: f32,
    pub area_max: f32,
    pub z_scale: f32,
    pub direction: f32,
    pub along: f32,
    pub class4: [f32; 7],
}

impl Lens {
    pub fn read(mem: &dyn GuestRead) -> Option<Self> {
        let f = |a| mem.read_f32(a);
        let lo = mem.read_u32(VIEWPORT)?;
        let hi = mem.read_u32(VIEWPORT + 4)?;
        let mut view = [0.0; 12];
        for (i, v) in view.iter_mut().enumerate() {
            *v = f(VIEW + 4 * i as u32)?;
        }
        let mut projection = [0.0; 16];
        for (i, v) in projection.iter_mut().enumerate() {
            *v = f(PROJECTION + 4 * i as u32)?;
        }
        let (one, half) = (f(k::ONE)?, f(k::HALF)?);
        let mut class4 = [one, half, 0.0, 0.0, 0.0, 0.0, 0.0];
        for (i, a) in k::CLASS4.iter().enumerate() {
            class4[i + 2] = f(*a)?;
        }
        Some(Self {
            viewport: [lo as i16, (lo >> 16) as i16, hi as i16, (hi >> 16) as i16],
            world_scale: f(WORLD_SCALE)?,
            view,
            projection,
            far: f(FAR)?,
            forward: [f(CAMERA_FORWARD)?, f(CAMERA_FORWARD + 4)?, f(CAMERA_FORWARD + 8)?],
            one,
            half,
            two: f(k::TWO)?,
            epsilon: f(k::EPSILON)?,
            half_max: f(k::HALF_MAX)?,
            area_max: f(k::AREA_MAX)?,
            z_scale: f(k::Z_SCALE)?,
            direction: f(k::DIRECTION)?,
            along: f(k::ALONG)?,
            class4,
        })
    }

    /// `0x00048b00(eax &position, xmm7 radius)`: the point's pixel, its
    /// `z` and `w` in depth units, and the half size of a sphere of
    /// `radius` there. `None` where the guest returns `al = 0`. The sums
    /// keep the guest's order.
    pub fn project(&self, p: [f32; 3], radius: f32) -> Option<([f32; 4], [f32; 2])> {
        if radius <= self.epsilon || radius.is_nan() {
            return None;
        }
        let s = self.world_scale;
        let (x, y, z) = (p[0] * s, p[1] * s, p[2] * s);
        let m = &self.view;
        let v0 = m[6] * z + m[3] * y + m[0] * x + m[9];
        let v1 = m[7] * z + m[4] * y + m[1] * x + m[10];
        let v2 = m[8] * z + m[5] * y + m[2] * x + m[11];
        let q = &self.projection;
        let py = q[9] * v2 + q[5] * v1 + q[1] * v0 + q[13];
        let pz = q[10] * v2 + q[6] * v1 + q[2] * v0 + q[14];
        let pw = q[11] * v2 + q[7] * v1 + q[3] * v0 + q[15];
        let (hx, hy) = (q[0] * radius, q[5] * radius);
        if pz <= self.epsilon || pz.is_nan() {
            return None;
        }
        let [y0, x0, y1, x1] = self.viewport;
        let width = f32::from(x1.wrapping_sub(x0));
        let height = f32::from(y1.wrapping_sub(y0));
        let inv = self.one / pw;
        let px = q[8] * v2 + q[4] * v1 + q[0] * v0 + q[12];
        let sx = ((px * inv + self.one) * width + f32::from(x0) * self.two - self.one) * self.half;
        let sy = ((self.one - inv * py) * height + f32::from(y0) * self.two - self.one) * self.half;
        let t = inv * pz;
        let zc = if self.one > t { t } else { self.one };
        let out = [sx, sy, zc * self.z_scale, pw / self.far * self.z_scale];
        Some((out, [width * inv * hx * self.half, height * inv * hy * self.half]))
    }

    /// `0x0002e540(entry, payload)`: the entry's quad, or `None` where the
    /// callback returns 0 with nothing drawn.
    pub fn query(&self, mem: &dyn GuestRead, entry: &Entry) -> Result<Option<Quad>, Unmade> {
        let r = |a| mem.read_u32(a).ok_or(Unmade::Unread);
        let rf = |a| mem.read_f32(a).ok_or(Unmade::Unread);
        let payload = entry.payload();
        if entry.datum == u32::MAX {
            return Ok(None);
        }
        let tag = r(r(TAGS)?.wrapping_add((entry.datum & 0xffff) * 16).wrapping_add(8))?;
        let flags = byte(mem, tag.wrapping_add(0x28)).ok_or(Unmade::Unread)?;
        if flags & 2 != 0 {
            return Ok(None);
        }
        let mut pos = [rf(payload + 4)?, rf(payload + 8)?, rf(payload + 0xc)?];
        let dir_word = r(payload + 0x10)?;
        let dir = [0u32, 1, 2].map(|i| f32::from((dir_word >> (8 * i)) as u8) * self.direction - self.one);
        let scale = if flags & 0x40 != 0 { rf(payload + 0x14)? } else { self.one };
        let mut radius = rf(tag.wrapping_add(0x10))? * scale;
        let shorts = r(tag.wrapping_add(0x14))?;
        if entry.head & 0x38 == 0x20 {
            let i = (shorts >> 16) as i16;
            let factor = usize::try_from(i).ok().and_then(|i| self.class4.get(i)).ok_or(Unmade::Table)?;
            radius *= factor;
        }
        match shorts as i16 {
            0 => {
                let off = 0.0 - radius;
                for (p, f) in pos.iter_mut().zip(self.forward) {
                    *p += off * f;
                }
            }
            1 => {
                let k = radius * self.along;
                for (p, d) in pos.iter_mut().zip(dir) {
                    *p += d * k;
                }
            }
            _ => {}
        }
        let Some((out, mut h)) = self.project(pos, radius) else { return Ok(None) };
        for h in &mut h {
            if self.two > *h {
                *h = self.two;
            } else if *h > self.half_max {
                *h = self.half_max;
            }
        }
        // 0x00321099 (floor) on the x87 difference, as a double.
        let floor = |a: f32, b: f32| (f64::from(a) + f64::from(b)).floor() as f32;
        let (x0, y0) = (floor(out[0], -h[0]), floor(out[1], -h[1]));
        let (x1, y1) = (floor(out[0], h[0]), floor(out[1], h[1]));
        let area = (y1 - y0) * (x1 - x0);
        if area <= 0.0 || area.is_nan() || self.area_max < area {
            return Ok(None);
        }
        let depth = |v: f32| if 0.0 > v { 0.0 } else if v > self.z_scale { self.z_scale } else { v };
        Ok(Some(Quad {
            x0: x0 as i32,
            y0: y0 as i32,
            x1: x1 as i32,
            y1: y1 as i32,
            z: depth(out[2]),
            w: depth(out[3]),
            area: area as i32 as u32,
        }))
    }
}

/// The reader's constants.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReadConstants {
    pub one: f32,
    pub byte: f32,
    pub byte_max: f32,
}

impl ReadConstants {
    pub fn read(mem: &dyn GuestRead) -> Option<Self> {
        Some(Self { one: mem.read_f32(k::ONE)?, byte: mem.read_f32(k::BYTE)?, byte_max: mem.read_f32(k::BYTE_MAX)? })
    }
}

/// How a lens smooths its byte (its tag's `+0x28` bits 4 and 5, read for
/// an entry whose flags `& 0x7c` are at least `0x60`): rising halfway at
/// once, or falling a sixteenth a frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Smoothing {
    pub rise_half: bool,
    pub fall_slow: bool,
}

/// The entry's smoothing (`0x000208e0`..`0x0002091a`).
pub fn smoothing(mem: &dyn GuestRead, entry: &Entry) -> Option<Smoothing> {
    if entry.flags & 0x7c < 0x60 || entry.datum == u32::MAX {
        return Some(Smoothing::default());
    }
    let tag = mem.read_u32(mem.read_u32(TAGS)?.wrapping_add((entry.datum & 0xffff) * 16).wrapping_add(8))?;
    let bits = half(mem, tag.wrapping_add(0x28))? as u8;
    Some(Smoothing { rise_half: (bits >> 4) & 1 != 0, fall_slow: (bits >> 5) & 1 != 0 })
}

/// A count as a byte: `samples / area` held to `[0, 1]`, scaled, held to
/// the largest byte, truncated.
pub fn visibility(samples: u32, area: u16, k: &ReadConstants) -> u32 {
    let v = samples as i32 as f32 / f32::from(area);
    let v = if 0.0 > v { 0.0 } else if v > k.one { k.one } else { v };
    let x = v * k.byte;
    let x = if 0.0 > x { 0.0 } else if x > k.byte_max { k.byte_max } else { x };
    x as i32 as u32
}

/// The byte the reader leaves for an entry of `class` whose test counted
/// `samples` of `area`, from `old`. Classes 0 to 4 are smoothed: up by an
/// eighth (or half, `rise_half`), down by half (or a sixteenth,
/// `fall_slow`); the others take the count's byte. A count above zero
/// leaves at least 1.
pub fn read_byte(old: u8, samples: u32, area: u16, class: u32, s: Smoothing, k: &ReadConstants) -> u8 {
    if area == 0 {
        return 0;
    }
    let new = visibility(samples, area, k);
    let o = u32::from(old);
    let mut b = if (1u32 << class) & 0xf == 0 && class != 4 {
        new as u8
    } else if o > new {
        if s.fall_slow { ((new * 15 + o) >> 4) as u8 } else { ((o + new) >> 1) as u8 }
    } else if o < new {
        if s.rise_half { ((o + new) >> 1) as u8 } else { ((o * 7 + new) >> 3) as u8 }
    } else {
        old
    };
    if samples as i32 > 0 && b < 1 {
        b = 1;
    }
    b
}

/// The draw pass's late list (`0x0002f260` appends, `0x0002df00` resets):
/// up to four 16-byte entries, a lens tag datum and a world position,
/// each drawn by `0x00046ea0` after the pass's walker.
pub const LATE: u32 = 0x004c_19c4;
pub const LATE_COUNT: u32 = 0x004c_1a04;
pub const LATE_MAX: u32 = 4;
/// The camera's position and view forward `0x00046ea0` reads (the
/// render camera's, not the query's `CAMERA_FORWARD`).
pub const CAMERA_POSITION: u32 = 0x0048_5618;
pub const CAMERA_VIEW_FORWARD: u32 = 0x0048_5624;
/// What the composite's weight is dimmed by: `1 - [GLOW_DIM]`, held to
/// `[0, 1]`.
pub const GLOW_DIM: u32 = 0x0048_57b4;
/// The composite's rings: sixteen fans.
pub const RINGS: usize = 16;

/// The `.rdata` constants `0x00046ea0` reads.
pub mod glow_k {
    /// The view-angle fade: `(cos - COS) * COS_SCALE`, held to `[0, 1]`.
    pub const COS: u32 = 0x0045_e22c;
    pub const COS_SCALE: u32 = 0x0045_e4b0;
    /// The rectangle's half size, in pixels.
    pub const HALF: u32 = 0x0045_e42c;
    /// Ring `i` grows each side by `i * RING[0] * RING[1] - RING_START`.
    pub const RING: [u32; 2] = [0x0045_df78, 0x0045_e310];
    pub const RING_START: u32 = 0x0045_dc10;
    /// `0x00030bf0`'s least length.
    pub const EPSILON: u32 = 0x0045_dbdc;
    /// The down-sample's square (`0x00046230`): corners at these two.
    pub const SOURCE: [u32; 2] = [0x0040_3b74, 0x0045_e484];
}

/// One late-list entry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LateGlow {
    pub datum: u32,
    pub position: [f32; 3],
}

/// The late list, in the order `0x0002df00`'s loop draws it; `None` past
/// its four entries.
pub fn late_glows(mem: &dyn GuestRead) -> Option<Vec<LateGlow>> {
    let n = mem.read_u32(LATE_COUNT)? as i32;
    if n > LATE_MAX as i32 {
        return None;
    }
    (0..n.max(0) as u32)
        .map(|i| {
            let at = LATE + 16 * i;
            Some(LateGlow {
                datum: mem.read_u32(at)?,
                position: [mem.read_f32(at + 4)?, mem.read_f32(at + 8)?, mem.read_f32(at + 12)?],
            })
        })
        .collect()
}

/// What `0x00046ea0` reads besides the [`Lens`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlowConstants {
    pub camera: [f32; 3],
    pub forward: [f32; 3],
    pub dim: f32,
    pub cos: f32,
    pub cos_scale: f32,
    pub half: f32,
    pub ring: [f32; 2],
    pub ring_start: f32,
    pub epsilon: f32,
    pub source: [f32; 2],
}

impl GlowConstants {
    pub fn read(mem: &dyn GuestRead) -> Option<Self> {
        let f = |a| mem.read_f32(a);
        let v = |a: u32| Some([f(a)?, f(a + 4)?, f(a + 8)?]);
        Some(Self {
            camera: v(CAMERA_POSITION)?,
            forward: v(CAMERA_VIEW_FORWARD)?,
            dim: f(GLOW_DIM)?,
            cos: f(glow_k::COS)?,
            cos_scale: f(glow_k::COS_SCALE)?,
            half: f(glow_k::HALF)?,
            ring: [f(glow_k::RING[0])?, f(glow_k::RING[1])?],
            ring_start: f(glow_k::RING_START)?,
            epsilon: f(glow_k::EPSILON)?,
            source: [f(glow_k::SOURCE[0])?, f(glow_k::SOURCE[1])?],
        })
    }
}

/// One glow `0x00046ea0` draws: its rectangle in the target's pixels and
/// its weights. The mask and the down-sample cover `rect`; ring `i` of
/// the composite covers it grown by [`Glow::ring`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glow {
    /// `(x0, y0, x1, y1)`: the projected point rounded, plus and minus the
    /// half size.
    pub rect: [f32; 4],
    /// The view-angle fade (`[esp+8]` at `0x00046f50`).
    pub fade: f32,
    /// The composite's weight: the fade times the dim factor
    /// (`0x0004772c`).
    pub weight: f32,
}

/// One composite fan: its rectangle and the weight register 9 carries.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ring {
    pub rect: [f32; 4],
    pub weight: f32,
}

impl Glow {
    /// Ring `i` (`0x00047740`..`0x0004785d`): the rectangle grown each
    /// side by `i * RING[0] * RING[1] - RING_START`, weight
    /// `weight / (i + 1)`. The sums keep the guest's order.
    pub fn ring(&self, k: &GlowConstants, i: u32) -> Ring {
        let off = i as i32 as f32 * k.ring[0] * k.ring[1] - k.ring_start;
        let [x0, y0, x1, y1] = self.rect;
        Ring { rect: [x0 - off, y0 - off, off + x1, off + y1], weight: self.weight / (i + 1) as i32 as f32 }
    }

    pub fn rings(&self, k: &GlowConstants) -> [Ring; RINGS] {
        std::array::from_fn(|i| self.ring(k, i as u32))
    }
}

/// `0x00030bf0`: the vector over its length, unchanged when the length is
/// below `epsilon`. The length is the x87's, rounded once.
pub fn normalize(v: [f32; 3], epsilon: f32, one: f32) -> [f32; 3] {
    let [x, y, z] = v.map(f64::from);
    let length = (x * x + y * y + z * z).sqrt();
    if f64::from(epsilon) > length.abs() {
        return v;
    }
    let inv = one / length as f32;
    v.map(|c| c * inv)
}

/// `0x00046ea0(eax datum, ecx &position)`: the glow, or `None` where it
/// returns with nothing drawn (the projection fails or the rectangle is
/// outside the viewport).
pub fn glow(lens: &Lens, k: &GlowConstants, mem: &dyn GuestRead, late: &LateGlow) -> Result<Option<Glow>, Unmade> {
    let r = |a| mem.read_u32(a).ok_or(Unmade::Unread);
    let tag = r(r(TAGS)?.wrapping_add((late.datum & 0xffff) * 16).wrapping_add(8))?;
    let radius = mem.read_f32(tag.wrapping_add(0x10)).ok_or(Unmade::Unread)?;
    let p = late.position;
    let d = normalize([p[0] - k.camera[0], p[1] - k.camera[1], p[2] - k.camera[2]], k.epsilon, lens.one);
    let cos = k.forward[2] * d[2] + k.forward[0] * d[0] + d[1] * k.forward[1];
    let v = (cos - k.cos) * k.cos_scale;
    let fade = if 0.0 > v { 0.0 } else if v > lens.one { lens.one } else { v };
    let t = lens.far / cos;
    let point = [d[0] * t + k.camera[0], d[1] * t + k.camera[1], d[2] * t + k.camera[2]];
    let Some((out, _)) = lens.project(point, radius) else { return Ok(None) };
    // 0x00321099 (floor) on the x87 sum, as a double; the corners are the
    // x87's, stored as floats.
    let round = |a: f32| (f64::from(a) + f64::from(lens.half)).floor();
    let (x, y) = (round(out[0]) as f32, round(out[1]));
    let h = f64::from(k.half);
    let rect = [
        (f64::from(x) - h) as f32,
        (y - h) as f32,
        (f64::from(x) + h) as f32,
        (y + h) as f32,
    ];
    let [vy0, vx0, vy1, vx1] = lens.viewport.map(f32::from);
    // Each test culls unless it holds (an unordered one too).
    let inside = vx1 > rect[0] && vy1 > rect[1] && rect[2] > vx0 && rect[3] > vy0;
    if !inside {
        return Ok(None);
    }
    let dim = lens.one - k.dim;
    let dim = if 0.0 > dim { 0.0 } else if dim > lens.one { lens.one } else { dim };
    Ok(Some(Glow { rect, fade, weight: dim * fade }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Mem;

    const K: ReadConstants = ReadConstants { one: 1.0, byte: 255.0, byte_max: 255.0 };

    /// A flare's byte follows its count: a lens with no smoothing bits
    /// rises an eighth of the way a frame and falls half; the water's
    /// class takes the count's byte at once; a count of one still shows.
    #[test]
    fn a_flare_fades_from_its_query_result() {
        let s = Smoothing::default();
        // Half the quad seen: 127.
        assert_eq!(visibility(50, 100, &K), 127);
        let mut b = 0u8;
        let mut seen = Vec::new();
        for _ in 0..3 {
            b = read_byte(b, 100, 100, 0, s, &K);
            seen.push(b);
        }
        assert_eq!(seen, [31, 59, 83], "up an eighth of the way a frame");
        assert_eq!(read_byte(200, 0, 100, 0, s, &K), 100, "down by half");
        assert_eq!(read_byte(200, 0, 100, 0, Smoothing { fall_slow: true, ..s }, &K), 12, "or a sixteenth of the old");
        assert_eq!(read_byte(0, 100, 100, 1, Smoothing { rise_half: true, ..s }, &K), 127);
        assert_eq!(read_byte(9, 100, 100, 5, s, &K), 255, "class 5 is not smoothed");
        assert_eq!(read_byte(0, 1, 60_000, 0, s, &K), 1, "a count above zero shows");
        assert_eq!(read_byte(77, 5, 0, 0, s, &K), 0, "no area: nothing");
        assert_eq!(read_byte(42, 165, 1000, 4, s, &K), 42, "equal stays");
    }

    #[test]
    fn the_sets_are_two_frames_apart() {
        for frame in 0..9 {
            assert_eq!(read_set(frame + 2), query_set(frame));
            assert_ne!(read_set(frame), query_set(frame));
            assert_ne!(read_set(frame + 1), query_set(frame));
        }
        assert_eq!(slot(2, 5), 17);
        assert_eq!(ended_bit(1, 33), (ENDED + 64 + 4, 2));
    }

    fn lens() -> Lens {
        // A 640x480 viewport, identity view, a projection with x and y
        // scales 1 and w = view z.
        let mut projection = [0.0; 16];
        projection[0] = 1.0;
        projection[5] = 1.0;
        projection[10] = 1.0;
        projection[11] = 1.0;
        let mut view = [0.0; 12];
        view[0] = 1.0;
        view[4] = 1.0;
        view[8] = 1.0;
        Lens {
            viewport: [0, 0, 480, 640],
            world_scale: 1.0,
            view,
            projection,
            far: 1000.0,
            forward: [0.0, 0.0, 1.0],
            one: 1.0,
            half: 0.5,
            two: 2.0,
            epsilon: 1e-4,
            half_max: 128.0,
            area_max: 65_536.0,
            z_scale: 16_777_215.0,
            direction: 2.0 / 255.0,
            along: 1.0,
            class4: [1.0, 0.5, 0.25, 0.125, 2.0, 4.0, 8.0],
        }
    }

    #[test]
    fn a_quad_is_the_projected_point_with_its_half_size_held_to_2_and_128() {
        let mut m = Mem::default();
        let tags = 0x0070_0000;
        let tag = 0x0071_0000;
        m.w(TAGS, tags);
        m.w(tags + 16 * 3 + 8, tag);
        m.f(tag + 0x10, 0.01);
        m.w(tag + 0x14, 2); // mode 2: no offset
        m.w(tag + 0x28, 0);
        let entry = Entry { index: 0, head: 0, flags: 2, datum: 3 };
        let p = entry.payload();
        m.f(p + 4, 0.0);
        m.f(p + 8, 0.0);
        m.f(p + 0xc, 10.0);
        m.w(p + 0x10, 0);
        let q = lens().query(&m, &entry).unwrap().expect("drawn");
        // Centre (319.5, 239.5); the half size 640 * 0.1 * 0.01 * 0.5 is
        // held to 2.
        assert_eq!((q.x0, q.y0, q.x1, q.y1), (317, 237, 321, 241));
        assert_eq!(q.area, 16);
        m.w(tag + 0x28, 2);
        assert_eq!(lens().query(&m, &entry), Ok(None), "flag 2: no quad");
    }

    fn glow_constants() -> GlowConstants {
        GlowConstants {
            camera: [0.0; 3],
            forward: [0.0, 0.0, 1.0],
            dim: 0.0,
            cos: 0.5,
            cos_scale: 2.0,
            half: 8.0,
            ring: [1.0, 2.0],
            ring_start: 1.0,
            epsilon: 1e-4,
            source: [0.0, 64.0],
        }
    }

    /// A glow straight ahead is at full weight; 45 degrees off the view it
    /// is at `(cos 45 - 0.5) * 2`; past the fade's edge it is 0 and off
    /// the viewport it is not drawn. Its rectangle is the projected point
    /// rounded, 8 pixels each side.
    #[test]
    fn a_flare_glow_fades_with_view_angle() {
        let mut m = Mem::default();
        let (tags, tag) = (0x0070_0000, 0x0071_0000);
        m.w(TAGS, tags);
        m.w(tags + 16 * 3 + 8, tag);
        m.f(tag + 0x10, 0.01);
        let k = glow_constants();
        let at = |x: f32| LateGlow { datum: 3, position: [x, 0.0, 10.0] };
        let ahead = glow(&lens(), &k, &m, &at(0.0)).unwrap().expect("drawn");
        assert_eq!(ahead.fade, 1.0);
        assert_eq!(ahead.rect, [312.0, 232.0, 328.0, 248.0]);
        let side = glow(&lens(), &k, &m, &at(10.0)).unwrap().expect("drawn at the edge");
        assert!((side.fade - 0.414_21).abs() < 1e-4, "{}", side.fade);
        assert_eq!(side.rect[..2], [632.0, 232.0], "half past the right edge, still drawn");
        let mut edge = k;
        edge.cos = 0.75;
        assert_eq!(glow(&lens(), &edge, &m, &at(10.0)).unwrap().map(|g| g.fade), Some(0.0), "past the fade's edge");
        assert_eq!(glow(&lens(), &k, &m, &at(100.0)), Ok(None), "off the viewport");
        let dimmed = GlowConstants { dim: 0.75, ..k };
        assert_eq!(glow(&lens(), &dimmed, &m, &at(0.0)).unwrap().map(|g| g.weight), Some(0.25));
    }

    /// Ring `i` grows by `2 i - 1` a side at `1 / (i + 1)` of the weight.
    #[test]
    fn the_composite_rings_grow_and_dim() {
        let g = Glow { rect: [10.0, 20.0, 30.0, 40.0], fade: 1.0, weight: 0.5 };
        let rings = g.rings(&glow_constants());
        assert_eq!(rings[0], Ring { rect: [11.0, 21.0, 29.0, 39.0], weight: 0.5 });
        assert_eq!(rings[3], Ring { rect: [5.0, 15.0, 35.0, 45.0], weight: 0.125 });
        assert_eq!(rings.len(), 16);
    }

    #[test]
    fn the_late_list_holds_at_most_four() {
        let mut m = Mem::default();
        m.w(LATE_COUNT, 1);
        m.w(LATE, 7);
        m.f(LATE + 4, 0.0);
        m.f(LATE + 8, 0.0);
        m.f(LATE + 12, 3.0);
        assert_eq!(late_glows(&m), Some(vec![LateGlow { datum: 7, position: [0.0, 0.0, 3.0] }]));
        m.w(LATE_COUNT, 5);
        assert_eq!(late_glows(&m), None);
    }
}
