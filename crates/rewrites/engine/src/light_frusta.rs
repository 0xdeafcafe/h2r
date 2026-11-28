//! A light's frusta, `0x000c1d90`, bit for bit, as a shadow-arm
//! [`Proposal`] (docs/graphics/native-visibility.md, "The shadow arms").
//!
//! The light frame body calls it at its step 10 with four stack arguments,
//! `[position, volume, frusta, count]`, and returns with `ret 10h`; EAX,
//! ECX, EDX and ESI carry copies the body never reads. The volume's kind
//! (`u16` at `+0`) picks the shape: kind 0 is a cube, six frusta, one per
//! face; any other kind is projected, one frustum. The count (`i16`) is
//! written first, then each 0x1bc-byte frustum is zero filled and built:
//!
//! | Offset | Bytes | Cube face | Projected |
//! |---|---|---|---|
//! | `+0x00` | `u8` | 0 | `u32 volume[0] == 1` |
//! | `+0x04` | 0x34 | the inverse of `+0x38` (`0x00141590`) | the same |
//! | `+0x38` | `f32` | the scale, `one` | `one` |
//! | `+0x3c`, `+0x48`, `+0x54` | 3 × vec3 | right, up, back of the face | `volume+0x1c`, `position+0x34`, `0 − position+0x28` |
//! | `+0x60` | vec3 | `volume+0x2c`, the light's centre | the same |
//! | `+0x6c` | `u8` | 0 | apex `> 0` |
//! | `+0x70` | `f32`, vec4 | 0 | the apex; the clip plane through `forward·apex + centre` |
//! | `+0x84`, `+0x85`, `+0x8c` | `u8` | 1, 0, 1 | 1, 0, 1 |
//! | `+0x88` | `f32` | the far distance, `volume+0x44` | `volume+0x64 + apex` |
//! | `+0x90` | 0x108 | the window (`0x00163db0`) of the four points' rectangle | the same |
//! | `+0x198` | `u32`, 4 × vec2 | 4 and the face's four corners projected | 4 and the window's corners |
//!
//! The window is not written when its rectangle is empty. After it the
//! builder checks that each corner lies on its planes; a failed check with
//! the assert byte `0x0047ff86` set clears that byte, the one write outside
//! the arguments.
//!
//! The x87 parts (the up vector's and the window planes' lengths, the
//! checks) run at double precision under control word 0x027f; anything
//! else is refused, as are an MXCSR other than 0x1f80 (flags aside), a NaN
//! in what the builder writes, and frusta or a count that overlap what it
//! reads.

use self::guest::*;
use crate::memory::{GuestMemory, MemFault};
use crate::shadow_arm::Proposal;

/// The image's words the builder reads.
pub mod guest {
    /// `one`: the scale, the normalise numerator, the face axes' plus.
    pub const ONE: u32 = 0x0045_dbc0;
    /// `minus`: the face axes' minus and the projection's numerator.
    pub const MINUS: u32 = 0x0045_dbcc;
    /// `half`: the projected window's half size factor.
    pub const HALF: u32 = 0x0045_dbbc;
    /// A vector shorter than this is not normalised.
    pub const EPSILON: u32 = 0x0045_dbdc;
    /// The window check's tolerance.
    pub const TOLERANCE: u32 = 0x0045_dc7c;
    /// The pointer to the default rectangle (four floats) the cube's
    /// points extend.
    pub const DEFAULT_RECT: u32 = 0x0046_87dc;
    /// The pointers to the cube faces' up references: faces 1 and 3 take
    /// the alternate.
    pub const UP_PRIMARY: u32 = 0x0046_87a8;
    pub const UP_ALTERNATE: u32 = 0x0046_87ac;
    /// The pointer to the box (six floats) the window bounds start from.
    pub const WINDOW_BOUNDS: u32 = 0x0046_87e0;
    /// The assert byte a failed window check clears.
    pub const ASSERT_ONCE: u32 = 0x0047_ff86;
    /// One frustum's size.
    pub const FRUSTUM: u32 = 0x1bc;
    /// The volume bytes the builder reads (`+0x00..+0x68`).
    pub const VOLUME_READ: u32 = 0x68;
}

/// Why the port declines; the original runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// MXCSR or the x87 control word is not the state the port computes
    /// under.
    FloatState,
    /// A read faults, or a frustum or the count is not mapped.
    Unread,
    /// The frusta, the count or the assert byte overlap what the builder
    /// reads, or each other, or an address wraps.
    Layout,
    /// A NaN would be written: its payload is x86's, not the host's.
    NotANumber,
}

impl Refusal {
    pub fn name(self) -> &'static str {
        match self {
            Self::FloatState => "float_state",
            Self::Unread => "unread",
            Self::Layout => "layout",
            Self::NotANumber => "nan",
        }
    }
}

impl From<MemFault> for Refusal {
    fn from(_: MemFault) -> Self {
        Self::Unread
    }
}

/// The call. The body reads only its stack arguments; `bits` (ESI) is
/// kept for the caller's records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Call {
    /// The light's pose (`+0x28` forward, `+0x34` up), read when projected.
    pub position: u32,
    /// The posed volume.
    pub volume: u32,
    /// Six frusta of 0x1bc bytes.
    pub frusta: u32,
    /// The `i16` count.
    pub count: u32,
    pub bits: u32,
}

impl Call {
    /// From the registers and the four stack arguments as the caller
    /// passes them (`[position, volume, frusta, count]`).
    pub fn from_registers(_eax: u32, _ecx: u32, _edx: u32, esi: u32, args: [u32; 4]) -> Self {
        let [position, volume, frusta, count] = args;
        Self { position, volume, frusta, count, bits: esi }
    }
}

/// What the call leaves. EAX, ECX and EDX on return are dead to the one
/// caller, so they are not built.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Built {
    pub proposal: Proposal,
    /// The frusta written: 6 or 1.
    pub frusta: u16,
}

macro_rules! frustum_names {
    ($($i:literal),*) => {
        [$([
            concat!("frustum", $i, "_head"),
            concat!("frustum", $i, "_inverse"),
            concat!("frustum", $i, "_basis"),
            concat!("frustum", $i, "_lens"),
            concat!("frustum", $i, "_window"),
            concat!("frustum", $i, "_points"),
        ]),*]
    };
}

/// Each frustum's fields: the head byte, the inverse, the basis (scale,
/// axes, centre), the lens (clip byte and plane, flags, far), the window
/// record, the points.
const NAMES: [[&str; 6]; 6] = frustum_names!(0, 1, 2, 3, 4, 5);
/// The fields' starts; each ends where the next starts.
const SPLITS: [usize; 7] = [0, 0x04, 0x38, 0x6c, 0x90, 0x198, 0x1bc];

type V3 = [f32; 3];

/// One frustum's bytes while it is built.
struct Rec([u8; FRUSTUM as usize]);

impl Rec {
    fn bits(&self, at: usize) -> u32 {
        u32::from_le_bytes(self.0[at..at + 4].try_into().unwrap())
    }
    fn f(&self, at: usize) -> f32 {
        f32::from_bits(self.bits(at))
    }
    fn v3(&self, at: usize) -> V3 {
        [self.f(at), self.f(at + 4), self.f(at + 8)]
    }
    fn set_bits(&mut self, at: usize, v: u32) {
        self.0[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn set(&mut self, at: usize, v: f32) {
        self.set_bits(at, v.to_bits());
    }
    fn set_v3(&mut self, at: usize, v: V3) {
        for (k, c) in v.into_iter().enumerate() {
            self.set(at + 4 * k, c);
        }
    }
    fn copy_words(&mut self, at: usize, words: &[u32]) {
        for (k, w) in words.iter().enumerate() {
            self.set_bits(at + 4 * k, *w);
        }
    }
}

/// The image's constants.
#[derive(Clone, Copy)]
struct Consts {
    one: f32,
    minus: f32,
    half: f32,
    epsilon: f32,
    tolerance: f32,
}

fn words<const N: usize>(m: &dyn GuestMemory, at: u32) -> Result<[u32; N], Refusal> {
    let mut out = [0; N];
    for (k, w) in out.iter_mut().enumerate() {
        *w = m.u32(at.checked_add(4 * k as u32).ok_or(Refusal::Layout)?)?;
    }
    Ok(out)
}

fn vec3(m: &dyn GuestMemory, at: u32) -> Result<V3, Refusal> {
    Ok(words::<3>(m, at)?.map(f32::from_bits))
}

fn span(at: u32, len: u32) -> Result<(u32, u32), Refusal> {
    Ok((at, at.checked_add(len).ok_or(Refusal::Layout)?))
}

fn overlaps(a: (u32, u32), b: (u32, u32)) -> bool {
    a.0 < b.1 && b.0 < a.1
}

/// The inline x87 normalise of the face's up vector: the length of
/// `(x·x + z·z) + y·y` at double precision, stored to single.
fn normalize_up(u: V3, c: &Consts) -> V3 {
    let [x, y, z] = u.map(f64::from);
    let length = ((x * x + z * z) + y * y).sqrt();
    if f64::from(c.epsilon) > length.abs() {
        return u;
    }
    let s = c.one / length as f32;
    [s * u[0], u[1] * s, u[2] * s]
}

/// `0x00030bf0`: the same, summed `(x·x + y·y) + z·z`.
fn normalize(n: V3, c: &Consts) -> V3 {
    let [x, y, z] = n.map(f64::from);
    let length = ((x * x + y * y) + z * z).sqrt();
    if f64::from(c.epsilon) > length.abs() {
        return n;
    }
    let s = c.one / length as f32;
    [n[0] * s, s * n[1], s * n[2]]
}

/// `0x0011f5f0`: extend `[x0, x1, y0, y1]` by points, comparing ordered.
fn extend_rect(rect: &mut [u32; 4], points: &[[f32; 2]]) {
    for p in points {
        for (k, v) in p.iter().enumerate() {
            if f32::from_bits(rect[2 * k]) > *v {
                rect[2 * k] = v.to_bits();
            }
            if *v > f32::from_bits(rect[2 * k + 1]) {
                rect[2 * k + 1] = v.to_bits();
            }
        }
    }
}

/// `0x00141590`: the inverse of the basis at `+0x38`, written at `+0x04`.
fn inverse(f: &mut Rec, c: &Consts) {
    let basis: [u32; 13] = std::array::from_fn(|i| f.bits(0x38 + 4 * i));
    for (i, w) in inverse_words(&basis, c.one).into_iter().enumerate() {
        f.set_bits(0x04 + 4 * i, w);
    }
}

/// `0x00141590(ecx = basis, eax = out)`, bit for bit: the inverse of a
/// 13-word basis (scale, three axes, the point), `one` the guest's 1.0.
/// A zero scale gives all zeros; a scale of one is kept, any other its
/// reciprocal, which scales the point. The axes are transposed and the
/// point is taken through them in the guest's order.
pub fn inverse_words(basis: &[u32; 13], one: f32) -> [u32; 13] {
    let f = |i: usize| f32::from_bits(basis[i]);
    let s = f(0);
    let mut out = [0u32; 13];
    if s == 0.0 {
        return out;
    }
    let mut tp = [0.0 - f(10), 0.0 - f(11), 0.0 - f(12)];
    if s == one {
        out[0] = one.to_bits();
    } else {
        let k = one / s;
        tp = [k * tp[0], k * tp[1], k * tp[2]];
        out[0] = k.to_bits();
    }
    // The axes transposed: rows of x, y and z components.
    for row in 0..3 {
        for axis in 0..3 {
            out[1 + 3 * row + axis] = basis[1 + 3 * axis + row];
        }
    }
    for k in 0..3 {
        let a = [f(1 + 3 * k), f(2 + 3 * k), f(3 + 3 * k)];
        out[10 + k] = ((a[2] * tp[2] + a[0] * tp[0]) + a[1] * tp[1]).to_bits();
    }
    out
}

/// `0x00163db0` into the record at `+0x90`, from the rectangle `rect`
/// (`[x0, x1, y0, y1]`) and the frustum's basis, clip and far distance.
/// Returns whether a corner check failed; false with nothing written when
/// the rectangle is empty.
fn window(f: &mut Rec, rect: [u32; 4], bounds: [u32; 6], c: &Consts) -> bool {
    const W: usize = 0x90;
    let [x0, x1, y0, y1] = rect.map(f32::from_bits);
    if !(x1 > x0) || !(y1 > y0) {
        return false;
    }
    f.set_bits(W, 0);
    f.copy_words(W + 0x10, &rect);
    let far = f.f(0x88);
    let (x0k, x1k, y0k, y1k) = (x0 * far, x1 * far, y0 * far, y1 * far);
    let back = 0.0 - far;
    let s = f.f(0x38);
    let axes = [f.v3(0x3c), f.v3(0x48), f.v3(0x54)];
    let t = f.v3(0x60);
    // `0x001429d0`: the corners to the world, the axes scaled.
    let world = |[x, y, z]: V3| -> V3 {
        std::array::from_fn(|k| (x * (axes[0][k] * s) + y * (axes[1][k] * s)) + (z * (axes[2][k] * s) + t[k]))
    };
    let corners = [[x0k, y1k, back], [x0k, y0k, back], [x1k, y1k, back], [x1k, y0k, back]].map(world);
    for (j, p) in corners.iter().enumerate() {
        f.set_v3(W + 0x24 + 12 * j, *p);
    }
    let origin = f.bits(0x60);
    let origin = [origin, f.bits(0x64), f.bits(0x68)];
    f.copy_words(W + 0x54, &origin);
    // `0x0011f770`: the box by the corners and the centre.
    let mut b = bounds;
    for p in corners.iter().chain([&t]) {
        for k in 0..3 {
            if f32::from_bits(b[2 * k]) > p[k] {
                b[2 * k] = p[k].to_bits();
            }
            if p[k] > f32::from_bits(b[2 * k + 1]) {
                b[2 * k + 1] = p[k].to_bits();
            }
        }
    }
    f.copy_words(W + 0x60, &b);
    // A local plane to the world. The bottom and top planes add the z
    // term before the scaled distance.
    let plane = |[x, y, z]: V3, d: f32, vertical: bool| -> [f32; 4] {
        let n: V3 = std::array::from_fn(|k| (axes[2][k] * z + axes[1][k] * y) + axes[0][k] * x);
        let yz = if vertical {
            (t[2] * n[2] + t[1] * n[1]) + d * s
        } else {
            (t[1] * n[1] + d * s) + t[2] * n[2]
        };
        [n[0], n[1], n[2], yz + t[0] * n[0]]
    };
    let side = |v: V3, vertical| plane(normalize(v, c), 0.0, vertical);
    let eye = if f.0[0x6c] != 0 {
        let p = [f.f(0x74), f.f(0x78), f.f(0x7c), f.f(0x80)];
        p.map(|v| 0.0 - v)
    } else {
        plane([0.0, 0.0, c.one], 0.0, false)
    };
    let planes = [
        side([c.minus, 0.0, 0.0 - x0], false),
        side([c.one, 0.0, x1], false),
        side([0.0, c.minus, 0.0 - y0], true),
        side([0.0, c.one, y1], true),
        eye,
        plane([0.0, 0.0, c.minus], far, false),
    ];
    for (i, p) in planes.iter().enumerate() {
        for (k, v) in p.iter().enumerate() {
            f.set(W + 0x78 + 16 * i + 4 * k, *v);
        }
    }
    for (j, p) in corners.iter().enumerate() {
        f.set_v3(W + 0xd8 + 12 * j, [p[0] - t[0], p[1] - t[1], p[2] - t[2]]);
    }
    // The checks, on the x87 at double precision: each corner on its two
    // side planes and the far plane, the centre on the four sides.
    let tol = f64::from(c.tolerance);
    let on = |p: V3, i: usize| {
        let [nx, ny, nz, d] = planes[i].map(f64::from);
        let [x, y, z] = p.map(f64::from);
        tol > (((nz * z + ny * y) + nx * x) - d).abs()
    };
    let checks: [(V3, &[usize]); 5] = [
        (corners[1], &[0, 2, 5]),
        (corners[3], &[1, 2, 5]),
        (corners[0], &[0, 3, 5]),
        (corners[2], &[1, 3, 5]),
        (t, &[0, 1, 2, 3]),
    ];
    !checks.iter().all(|(p, planes)| planes.iter().all(|&i| on(*p, i)))
}

/// `0x000c1d90` for `call`, under `mxcsr` and the x87 control word
/// `x87_control`. Reads guest memory; writes nothing.
pub fn build(m: &dyn GuestMemory, call: &Call, mxcsr: u32, x87_control: u16) -> Result<Built, Refusal> {
    if mxcsr & !0x3f != 0x1f80 || x87_control != 0x027f {
        return Err(Refusal::FloatState);
    }
    let kind = m.u16(call.volume)?;
    let n: u16 = if kind == 0 { 6 } else { 1 };
    let cube = kind == 0;

    // What is read, against what is written.
    let rect_at = m.u32(DEFAULT_RECT)?;
    let bounds_at = m.u32(WINDOW_BOUNDS)?;
    let up_at = [m.u32(UP_PRIMARY)?, m.u32(UP_ALTERNATE)?];
    let mut reads = vec![
        span(call.volume, VOLUME_READ)?,
        span(ONE, 4)?,
        span(MINUS, 4)?,
        span(HALF, 4)?,
        span(EPSILON, 4)?,
        span(TOLERANCE, 4)?,
        span(DEFAULT_RECT, 4)?,
        span(WINDOW_BOUNDS, 4)?,
        span(rect_at, 16)?,
        span(bounds_at, 24)?,
    ];
    if cube {
        reads.extend([span(UP_PRIMARY, 4)?, span(UP_ALTERNATE, 4)?, span(up_at[0], 12)?, span(up_at[1], 12)?]);
    } else {
        reads.push(span(call.position.checked_add(0x28).ok_or(Refusal::Layout)?, 0x18)?);
    }
    let frusta = span(call.frusta, FRUSTUM * u32::from(n))?;
    let count = span(call.count, 2)?;
    let flag = span(ASSERT_ONCE, 1)?;
    if reads.iter().any(|&r| overlaps(r, frusta) || overlaps(r, count))
        || overlaps(frusta, count)
        || overlaps(flag, frusta)
        || overlaps(flag, count)
        || reads.iter().any(|&r| r != flag && overlaps(r, flag))
    {
        return Err(Refusal::Layout);
    }
    for (at, len) in [(call.count, 2), (call.frusta, FRUSTUM * u32::from(n))] {
        let mut probe = vec![0; len as usize];
        m.read(at, &mut probe)?;
    }

    let c = Consts {
        one: m.f32(ONE)?,
        minus: m.f32(MINUS)?,
        half: m.f32(HALF)?,
        epsilon: m.f32(EPSILON)?,
        tolerance: m.f32(TOLERANCE)?,
    };
    let default_rect: [u32; 4] = words(m, rect_at)?;
    let bounds: [u32; 6] = words(m, bounds_at)?;
    let v = call.volume;
    let mut flag_byte = m.u8(ASSERT_ONCE)?;
    let mut cleared = false;

    let mut proposal = Proposal::default();
    proposal.u16("count", call.count, n);
    for i in 0..usize::from(n) {
        let mut f = Rec([0; FRUSTUM as usize]);
        let rect = if cube {
            face(m, &mut f, i, v, up_at, default_rect, &c)?
        } else {
            projected(m, &mut f, v, call.position, &c)?
        };
        f.0[0x8c] = 1;
        f.0[0x84] = 1;
        f.0[0x85] = 0;
        inverse(&mut f, &c);
        let failed = window(&mut f, rect, bounds, &c);
        if failed && flag_byte != 0 {
            flag_byte = 0;
            cleared = true;
        }
        if f.0.chunks_exact(4).any(|w| f32::from_bits(u32::from_le_bytes(w.try_into().unwrap())).is_nan()) {
            return Err(Refusal::NotANumber);
        }
        let base = call.frusta + FRUSTUM * i as u32;
        for (k, name) in NAMES[i].iter().enumerate() {
            proposal.push(name, base + SPLITS[k] as u32, &f.0[SPLITS[k]..SPLITS[k + 1]]);
        }
    }
    if cleared {
        proposal.u8("assert_once", ASSERT_ONCE, 0);
    }
    Ok(Built { proposal, frusta: n })
}

/// The cube's corners: `±h` on each axis, in the guest's table order.
fn corner(k: usize, h: f32) -> V3 {
    let m = 0.0 - h;
    [
        [h, h, m],
        [m, h, m],
        [m, m, m],
        [h, m, m],
        [h, h, h],
        [m, h, h],
        [m, m, h],
        [h, m, h],
    ][k]
}

/// Each face's four corners.
const FACE_CORNERS: [[usize; 4]; 6] = [[0, 1, 2, 3], [0, 3, 7, 4], [0, 4, 5, 1], [1, 5, 6, 2], [2, 6, 7, 3], [4, 7, 6, 5]];

/// Cube face `i`: its basis and points. Returns the rectangle.
fn face(
    m: &dyn GuestMemory,
    f: &mut Rec,
    i: usize,
    v: u32,
    up_at: [u32; 2],
    default_rect: [u32; 4],
    c: &Consts,
) -> Result<[u32; 4], Refusal> {
    let h_bits = m.u32(v + 0x44)?;
    let h = f32::from_bits(h_bits);
    f.set_bits(0x88, h_bits);
    let (o, p, z) = (c.one, c.minus, 0.0f32);
    let dir = [[z, z, p], [o, z, z], [z, o, z], [p, z, z], [z, p, z], [z, z, o]][i];
    let b = [0.0 - dir[0], 0.0 - dir[1], 0.0 - dir[2]];
    f.set_v3(0x54, b);
    let r = vec3(m, up_at[usize::from(i == 1 || i == 3)])?;
    let u = [r[2] * b[1] - b[2] * r[1], b[2] * r[0] - b[0] * r[2], b[0] * r[1] - b[1] * r[0]];
    let u = normalize_up(u, c);
    f.set_v3(0x48, u);
    let a = [u[1] * b[2] - u[2] * b[1], u[2] * b[0] - b[2] * u[0], b[1] * u[0] - u[1] * b[0]];
    f.set_v3(0x3c, a);
    f.copy_words(0x60, &words::<3>(m, v + 0x2c)?);
    f.set(0x38, c.one);
    let s = f.f(0x38);
    let mut points = [[0f32; 2]; 4];
    for (j, &k) in FACE_CORNERS[i].iter().enumerate() {
        let mut q = corner(k, h);
        if s != c.one {
            let k = c.one / s;
            q = [k * q[0], k * q[1], k * q[2]];
        }
        let [x, y, zz] = q;
        let pa = (a[0] * x + a[1] * y) + a[2] * zz;
        let pb = (x * u[0] + u[2] * zz) + u[1] * y;
        let pc = (b[2] * zz + b[0] * x) + b[1] * y;
        let k = c.minus / pc;
        points[j] = [k * pa, k * pb];
    }
    f.set_bits(0x198, 4);
    for (j, q) in points.iter().enumerate() {
        f.set(0x19c + 8 * j, q[0]);
        f.set(0x1a0 + 8 * j, q[1]);
    }
    let mut rect = default_rect;
    extend_rect(&mut rect, &points);
    Ok(rect)
}

/// The projected light's one frustum. Returns the rectangle.
fn projected(m: &dyn GuestMemory, f: &mut Rec, v: u32, pose: u32, c: &Consts) -> Result<[u32; 4], Refusal> {
    f.copy_words(0x3c, &words::<3>(m, v + 0x1c)?);
    f.copy_words(0x48, &words::<3>(m, pose + 0x34)?);
    let fwd_bits: [u32; 3] = words(m, pose + 0x28)?;
    let fwd = fwd_bits.map(f32::from_bits);
    f.set_v3(0x54, [0.0 - fwd[0], 0.0 - fwd[1], 0.0 - fwd[2]]);
    f.copy_words(0x60, &words::<3>(m, v + 0x2c)?);
    f.set(0x38, c.one);
    f.0[0] = u8::from(m.u32(v)? == 1);
    let apex_bits = m.u32(v + 0x28)?;
    let apex = f32::from_bits(apex_bits);
    f.0[0x6c] = u8::from(apex > 0.0);
    f.set_bits(0x70, apex_bits);
    let o = f.v3(0x60);
    let q = [apex * fwd[0] + o[0], fwd[1] * apex + o[1], fwd[2] * apex + o[2]];
    f.copy_words(0x74, &fwd_bits);
    f.set(0x80, (fwd[2] * q[2] + fwd[1] * q[1]) + fwd[0] * q[0]);
    f.set(0x88, m.f32(v + 0x64)? + apex);
    let mut w = vec3(m, v + 0x3c)?;
    let s = f.f(0x38);
    if s != c.one {
        let k = c.one / s;
        w = [k * w[0], k * w[1], k * w[2]];
    }
    let (a0, a1, b) = (f.v3(0x3c), f.v3(0x48), f.v3(0x54));
    let x = (a0[2] * w[2] + a0[1] * w[1]) + a0[0] * w[0];
    let y = (a1[2] * w[2] + a1[1] * w[1]) + a1[0] * w[0];
    let z = (b[2] * w[2] + b[1] * w[1]) + b[0] * w[0];
    let k = c.minus / z;
    let (cx, cy) = (k * x, k * y);
    let inv = c.one / apex;
    let wd = (m.f32(v + 0x58)? * inv) * c.half;
    let ht = (m.f32(v + 0x5c)? * inv) * c.half;
    let (x0, y0, x1, y1) = (cx - wd, cy - ht, wd + cx, ht + cy);
    f.set_bits(0x198, 4);
    for (j, q) in [[x0, y0], [x1, y0], [x1, y1], [x0, y1]].iter().enumerate() {
        f.set(0x19c + 8 * j, q[0]);
        f.set(0x1a0 + 8 * j, q[1]);
    }
    Ok([x0, x1, y0, y1].map(f32::to_bits))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeGuest;

    const VOLUME: u32 = 0x0010_0000;
    const POSE: u32 = 0x0010_1000;
    const COUNT: u32 = 0x0010_2000;
    const FRUSTA: u32 = 0x0010_3000;
    const RECT: u32 = 0x0010_5000;
    const BOUNDS: u32 = 0x0010_5100;
    const UP: u32 = 0x0010_5200;
    const ALT: u32 = 0x0010_5300;

    fn image() -> FakeGuest {
        let mut m = FakeGuest::new();
        m.put_f32(ONE, 1.0);
        m.put_f32(MINUS, -1.0);
        m.put_f32(HALF, 0.5);
        m.put_f32(EPSILON, 1e-4);
        m.put_f32(TOLERANCE, 0.01);
        m.put_u32(DEFAULT_RECT, RECT);
        for (k, v) in [f32::MAX, -f32::MAX, f32::MAX, -f32::MAX].into_iter().enumerate() {
            m.put_f32(RECT + 4 * k as u32, v);
        }
        m.put_u32(WINDOW_BOUNDS, BOUNDS);
        for (k, v) in [f32::MAX, -f32::MAX, f32::MAX, -f32::MAX, f32::MAX, -f32::MAX].into_iter().enumerate() {
            m.put_f32(BOUNDS + 4 * k as u32, v);
        }
        m.put_u32(UP_PRIMARY, UP);
        m.put_u32(UP_ALTERNATE, ALT);
        for (k, v) in [1.0f32, 0.0, 0.0].into_iter().enumerate() {
            m.put_f32(UP + 4 * k as u32, v);
        }
        for (k, v) in [0.0f32, 1.0, 0.0].into_iter().enumerate() {
            m.put_f32(ALT + 4 * k as u32, v);
        }
        m.put_u8(ASSERT_ONCE, 1);
        m.map(VOLUME, 0x80).map(POSE, 0x40).map(COUNT, 2).map(FRUSTA, 6 * FRUSTUM);
        for (k, v) in [2.0f32, -3.0, 4.0].into_iter().enumerate() {
            m.put_f32(VOLUME + 0x2c + 4 * k as u32, v);
        }
        m
    }

    fn call() -> Call {
        Call::from_registers(FRUSTA, VOLUME, COUNT, 0x400, [POSE, VOLUME, FRUSTA, COUNT])
    }

    fn applied(m: &FakeGuest, b: &Built) -> FakeGuest {
        let mut out = m.clone();
        b.proposal.apply(&mut out).unwrap();
        out
    }

    #[test]
    fn a_cube_light_makes_six_face_frusta_out_to_its_radius() {
        let mut m = image();
        m.put_f32(VOLUME + 0x44, 9.0);
        let b = build(&m, &call(), 0x1f80, 0x027f).unwrap();
        assert_eq!(b.frusta, 6);
        let out = applied(&m, &b);
        assert_eq!(out.u16(COUNT).unwrap(), 6);
        for i in 0..6 {
            let f = FRUSTA + FRUSTUM * i;
            assert_eq!(out.f32(f + 0x88).unwrap(), 9.0, "face {i}: far");
            assert_eq!(out.f32(f + 0x38).unwrap(), 1.0);
            assert_eq!([out.u8(f + 0x84).unwrap(), out.u8(f + 0x85).unwrap(), out.u8(f + 0x8c).unwrap()], [1, 0, 1]);
            assert_eq!(out.u32(f + 0x198).unwrap(), 4);
            // Each face's corners project to the unit square.
            let rect: Vec<f32> = (0..4).map(|k| out.f32(f + 0x90 + 0x10 + 4 * k).unwrap()).collect();
            assert_eq!(rect, [-1.0, 1.0, -1.0, 1.0], "face {i}");
            // The back axis is the face direction negated.
            let back: Vec<f32> = (0..3).map(|k| out.f32(f + 0x54 + 4 * k).unwrap()).collect();
            assert_eq!(back.iter().map(|v| v.abs()).sum::<f32>(), 1.0);
        }
        assert!(b.proposal.fields.iter().any(|f| f.name == "frustum5_window"));
        assert!(!b.proposal.fields.iter().any(|f| f.name == "assert_once"), "a square face passes its checks");
    }

    #[test]
    fn a_projected_light_makes_one_frustum_with_its_apex_clip_plane() {
        let mut m = image();
        m.put_u16(VOLUME, 2);
        for (at, v) in [(VOLUME + 0x1c, [0.0f32, -1.0, 0.0]), (VOLUME + 0x3c, [1.0, 0.0, 0.0])] {
            for (k, c) in v.into_iter().enumerate() {
                m.put_f32(at + 4 * k as u32, c);
            }
        }
        for (at, v) in [(POSE + 0x28, [1.0f32, 0.0, 0.0]), (POSE + 0x34, [0.0, 0.0, 1.0])] {
            for (k, c) in v.into_iter().enumerate() {
                m.put_f32(at + 4 * k as u32, c);
            }
        }
        m.put_f32(VOLUME + 0x28, 2.0);
        m.put_f32(VOLUME + 0x58, 4.0);
        m.put_f32(VOLUME + 0x5c, 2.0);
        m.put_f32(VOLUME + 0x64, 7.0);
        let b = build(&m, &call(), 0x1f80, 0x027f).unwrap();
        assert_eq!(b.frusta, 1);
        let out = applied(&m, &b);
        assert_eq!(out.u16(COUNT).unwrap(), 1);
        assert_eq!(out.u8(FRUSTA).unwrap(), 0, "kind 2 is not 1");
        assert_eq!(out.u8(FRUSTA + 0x6c).unwrap(), 1);
        assert_eq!(out.f32(FRUSTA + 0x88).unwrap(), 9.0);
        // The clip plane: forward, through forward·2 + centre.
        assert_eq!(out.f32(FRUSTA + 0x80).unwrap(), 4.0);
        let points: Vec<f32> = (0..8).map(|k| out.f32(FRUSTA + 0x19c + 4 * k).unwrap()).collect();
        assert_eq!(points, [-1.0, -0.5, 1.0, -0.5, 1.0, 0.5, -1.0, 0.5]);
        // The eye plane is the clip plane negated.
        let eye: Vec<f32> = (0..4).map(|k| out.f32(FRUSTA + 0x90 + 0xb8 + 4 * k).unwrap()).collect();
        assert_eq!(eye.iter().map(|v| v.to_bits()).collect::<Vec<_>>(), [-1.0f32, 0.0, 0.0, -4.0].map(f32::to_bits));
        assert!(!b.proposal.fields.iter().any(|f| f.name.starts_with("frustum1")));
        // The second frustum's bytes are left alone.
        m.put_u32(VOLUME, 1);
        let b = build(&m, &call(), 0x1f80, 0x027f).unwrap();
        assert_eq!(applied(&m, &b).u8(FRUSTA).unwrap(), 1);
    }

    #[test]
    fn an_empty_window_leaves_its_record_zero_and_a_zero_scale_its_inverse() {
        let mut m = image();
        m.put_u16(VOLUME, 3);
        m.put_f32(VOLUME + 0x28, 2.0);
        m.put_f32(VOLUME + 0x3c, 1.0);
        m.put_f32(POSE + 0x28, 1.0);
        // Zero width: x0 == x1.
        let b = build(&m, &call(), 0x1f80, 0x027f).unwrap();
        let out = applied(&m, &b);
        assert!((0..0x108).all(|k| out.u8(FRUSTA + 0x90 + k).unwrap() == 0));
        // A zero scale zeroes the inverse.
        m.put_f32(ONE, 0.0);
        let b = build(&m, &call(), 0x1f80, 0x027f).unwrap();
        let out = applied(&m, &b);
        assert!((0x04..0x38).all(|k| out.u8(FRUSTA + k).unwrap() == 0));
        m.put_f32(ONE, 1.0);
        m.put_u32(VOLUME + 0x58, 0x7fc0_0000);
        assert_eq!(build(&m, &call(), 0x1f80, 0x027f), Err(Refusal::NotANumber));
    }

    #[test]
    fn a_failed_check_clears_the_assert_byte_once() {
        let mut m = image();
        m.put_f32(VOLUME + 0x44, 9.0);
        m.put_f32(TOLERANCE, 0.0);
        let b = build(&m, &call(), 0x1f80, 0x027f).unwrap();
        assert_eq!(b.proposal.fields.iter().filter(|f| f.name == "assert_once").count(), 1);
        m.put_u8(ASSERT_ONCE, 0);
        let b = build(&m, &call(), 0x1f80, 0x027f).unwrap();
        assert!(!b.proposal.fields.iter().any(|f| f.name == "assert_once"));
    }

    #[test]
    fn float_states_overlaps_and_unmapped_frusta_refuse() {
        let m = image();
        assert_eq!(build(&m, &call(), 0x1f80 | 0x6000, 0x027f).map(|_| ()), Err(Refusal::FloatState));
        assert_eq!(build(&m, &call(), 0x1f80, 0x037f).map(|_| ()), Err(Refusal::FloatState));
        let over = Call { frusta: VOLUME + 0x40, ..call() };
        assert_eq!(build(&m, &over, 0x1f80, 0x027f).map(|_| ()), Err(Refusal::Layout));
        let count_in = Call { count: FRUSTA + 0x10, ..call() };
        assert_eq!(build(&m, &count_in, 0x1f80, 0x027f).map(|_| ()), Err(Refusal::Layout));
        let unmapped = Call { frusta: 0x0900_0000, ..call() };
        assert_eq!(build(&m, &unmapped, 0x1f80, 0x027f).map(|_| ()), Err(Refusal::Unread));
        assert_eq!(Refusal::NotANumber.name(), "nan");
    }
}

/// The shadow arm over the light frame's step 10
/// (docs/graphics/native-visibility.md, "The shadow arms"): one byte arm
/// for every light's call, sampled by the render frame.
pub mod arm {
    use std::sync::Mutex;

    use crate::shadow_arm::{ByteArm, Plan};

    struct State {
        arm: ByteArm,
        on: bool,
    }

    static STATE: Mutex<State> = Mutex::new(State { arm: ByteArm::new(), on: true });

    fn with<R>(f: impl FnOnce(&mut ByteArm, bool) -> R) -> R {
        let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
        let on = s.on;
        f(&mut s.arm, on)
    }

    /// `impl.render.native.visibility`: off, the arm compares and never
    /// answers.
    pub fn set_on(on: bool) {
        STATE.lock().unwrap_or_else(|e| e.into_inner()).on = on;
    }

    /// What to do with a call in `frame`; counts an unsampled guest call.
    pub fn plan(frame: u32) -> Plan {
        with(|a, on| {
            let plan = a.shadow.plan(frame, on);
            if plan == Plan::Guest {
                a.shadow.unsampled(Plan::Guest);
            }
            plan
        })
    }

    /// Rust answered an unsampled call.
    pub fn answered() {
        with(|a, _| {
            a.shadow.unsampled(Plan::Answer);
        });
    }

    /// The guest made a sampled call: compare `proposal` with `m`.
    pub fn compare(proposal: &crate::shadow_arm::Proposal, m: &dyn crate::memory::GuestMemory, frame: u32, light: u32) {
        with(|a, on| {
            a.compare(proposal, m, on, || format!("frame {frame} light {light:#010x}"));
        });
    }

    pub fn refuse(why: &'static str) {
        with(|a, _| a.refuse(why));
    }

    pub fn new_map() {
        with(|a, _| a.new_map());
    }

    /// `render.scene`'s counters (`scene_light_frusta_*`).
    pub fn counters() -> Vec<(&'static str, u64)> {
        with(|a, _| a.counters("scene_light_frusta"))
    }
}
