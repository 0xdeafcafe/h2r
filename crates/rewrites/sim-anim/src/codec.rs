//! The channel codecs: what each of the guest's codec functions computes
//! for one track, in its operation order (docs/engine/anim-sampling.md
//! "Codec functions"). The adapter picks the operation from the function
//! pointer the guest's codec table holds, so a codec byte never has to be
//! trusted to mean what the table says.
//!
//! Stream header words (offsets from the stream start): `+0xc` translation
//! base, `+0x10` scale base, `+0x14/+0x18/+0x1c` per-track strides (frame
//! codecs) or key-time bases (keyed codecs), `+0x20/+0x24/+0x28` key-value
//! bases (keyed codecs), `+0x30` the inline rotation track words of the
//! keyed codecs.
use crate::clip::{Clip, Stream};
use sim_core::det::{self, Fp};
use sim_core::lanes::Vec4;

/// Frame indices and the fraction between them, as the guest's globals
/// `0x504464`, `0x504468`, `0x50446c` hold them for one codec call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Frame {
    pub a: u32,
    pub b: u32,
    /// binary32 bits.
    pub w: u32,
}

/// Rotation codec functions (integer-frame / interpolated table slots).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RotOp {
    /// `0x28c470`: one int16 quaternion per track (codec 1).
    Static16,
    /// `0x28c960`: int16 per frame, frame A (codec 3).
    Frame16,
    /// `0x28c790`: int16 per frame, `A * (1 - w) + B * w` (codec 3).
    Blend16,
    /// `0x28c710`, `0x2c4d60`: four floats per frame copied (codecs 8, 2).
    FrameRaw,
    /// `0x28c530`: float per frame, `(B - A) * w + A`, aligned loads (codec 8).
    BlendF32,
    /// `0x28cdb0` / `0x28d560`: keyed, integer frame (codecs 4 and 6 / 5 and 7).
    KeyInt { wide: bool },
    /// `0x28c9e0` / `0x28d180`: keyed, interpolated frame.
    KeyInterp { wide: bool },
}

/// Translation and scale codec functions; one shape serves both, with
/// [`Vector`] saying which header words and value width apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VecOp {
    /// `0x28c4e0` (translation), `0x28c510` (scale): one value per track.
    Static,
    /// `0x28c750`, `0x2c4da0` / `0x2c4de0`: one value per frame, frame A.
    FrameRaw,
    /// `0x28c880`, `0x28c5d0` / `0x28c6b0`: `(B - A) * w + A`.
    FrameLerp,
    /// `0x28cf40`, `0x28d6f0` / `0x28d090`, `0x28d840`.
    KeyInt { wide: bool },
    /// `0x28cb70`, `0x28d320` / `0x28ccc0`, `0x28d470`.
    KeyInterp { wide: bool },
}

/// Which non-rotation channel a [`VecOp`] reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vector {
    Translation,
    Scale,
}

impl Vector {
    const fn base(self) -> u32 {
        match self {
            Vector::Translation => 0x0c,
            Vector::Scale => 0x10,
        }
    }
    const fn stride(self) -> u32 {
        match self {
            Vector::Translation => 0x18,
            Vector::Scale => 0x1c,
        }
    }
    const fn values(self) -> u32 {
        match self {
            Vector::Translation => 0x24,
            Vector::Scale => 0x28,
        }
    }
    const fn width(self) -> usize {
        match self {
            Vector::Translation => 3,
            Vector::Scale => 1,
        }
    }
}

/// What a sampler needs besides the clip: the arithmetic, whether MMX
/// ran (the int16 codecs leave the x87 tags empty and TOP at 0), and the
/// guest address of the blob for the codecs that use aligned loads.
pub struct Ctx {
    pub fp: Fp,
    pub mmx: bool,
    pub data_addr: u32,
    /// `0x45dbc0`, read from the image by the adapter.
    pub one: u32,
    /// The last sample refused a `movaps` key off 16 bytes.
    pub misaligned: bool,
}

/// Unpack an int16 quaternion and convert each lane (`punpck*wd`,
/// `psrad 16`, `cvtpi2ps`): exact.
#[inline]
fn quat16(bytes: [u8; 8]) -> Vec4 {
    std::array::from_fn(|i| {
        let v = i16::from_le_bytes([bytes[2 * i], bytes[2 * i + 1]]);
        det::from_int(i32::from(v))
    })
}

#[inline]
fn words<const N: usize>(bytes: [u8; 16]) -> [u32; N] {
    std::array::from_fn(|i| u32::from_le_bytes(bytes[4 * i..4 * i + 4].try_into().unwrap()))
}

/// The codecs' normalization: squares, two shuffled pair sums, the
/// Pentium III reciprocal square root per lane, scale.
#[inline]
pub(crate) fn normalize4(fp: &mut Fp, q: Vec4) -> Vec4 {
    let squares = fp.mul4(q, q);
    let swapped = [squares[2], squares[3], squares[0], squares[1]];
    let pairs = fp.add4(squares, swapped);
    let transposed = [pairs[1], pairs[0], pairs[1], pairs[0]];
    let sum = fp.add4(pairs, transposed);
    fp.mul4(q, sum.map(det::rsqrt))
}

/// The keyed codecs' search: the last key whose time is at or before
/// `frame` (unsigned compare), by the guest's bisection.
#[inline]
fn bisect(time: impl Fn(u32) -> Option<u32>, count: u32, frame: u32) -> Option<u32> {
    let (mut lo, mut hi) = (0i32, count as i32);
    if hi > 1 {
        loop {
            let mid = (hi + lo) >> 1;
            if time(mid as u32)? <= frame {
                lo = mid;
            } else {
                hi = mid;
            }
            if hi <= lo + 1 {
                break;
            }
        }
    }
    Some(lo as u32)
}

/// A keyed track: its key count, and readers for key times and values.
struct Keys {
    count: u32,
    times: u32,
    values: u32,
    wide: bool,
}

impl Keys {
    fn new(
        clip: &Clip,
        s: &Stream,
        word_at: u32,
        times_field: u32,
        values_field: u32,
        size: u32,
        wide: bool,
    ) -> Option<Keys> {
        let word = clip.u32_at(s, word_at)?;
        let (count, first) = (word & 0xfff, word >> 12);
        let times = clip.u32_at(s, times_field)?.wrapping_add(if wide {
            first.wrapping_mul(2)
        } else {
            first
        });
        let values = clip
            .u32_at(s, values_field)?
            .wrapping_add(first.wrapping_mul(size));
        Some(Keys {
            count,
            times,
            values,
            wide,
        })
    }

    #[inline]
    fn time(&self, clip: &Clip, s: &Stream, key: u32) -> Option<u32> {
        if self.wide {
            let b = clip.slice::<2>(s, self.times.wrapping_add(key.wrapping_mul(2)))?;
            Some(u32::from(u16::from_le_bytes(b)))
        } else {
            Some(u32::from(
                clip.slice::<1>(s, self.times.wrapping_add(key))?[0],
            ))
        }
    }
}

impl RotOp {
    /// One rotation, as the codec function would store it.
    pub fn sample(
        self,
        cx: &mut Ctx,
        clip: &Clip,
        s: &Stream,
        track: u32,
        f: Frame,
    ) -> Option<Vec4> {
        let fp = &mut cx.fp;
        Some(match self {
            RotOp::Static16 => {
                cx.mmx = true;
                let q = quat16(clip.slice(s, track.wrapping_mul(8).wrapping_add(0x20))?);
                normalize4(fp, q)
            }
            RotOp::Frame16 | RotOp::Blend16 => {
                cx.mmx = true;
                let stride = clip.u32_at(s, 0x14)?;
                let base = stride.wrapping_mul(track).wrapping_add(0x20);
                let a = quat16(clip.slice(s, base.wrapping_add(f.a.wrapping_mul(8)))?);
                if self == RotOp::Frame16 {
                    normalize4(fp, a)
                } else {
                    let rest = fp.sub(cx.one, f.w);
                    let b = quat16(clip.slice(s, base.wrapping_add(f.b.wrapping_mul(8)))?);
                    let a = fp.mul4(a, [rest; 4]);
                    let b = fp.mul4(b, [f.w; 4]);
                    let q = fp.add4(a, b);
                    normalize4(fp, q)
                }
            }
            RotOp::FrameRaw => {
                let stride = clip.u32_at(s, 0x14)?;
                let at = stride
                    .wrapping_mul(track)
                    .wrapping_add(0x20)
                    .wrapping_add(f.a.wrapping_mul(16));
                words(clip.slice(s, at)?)
            }
            RotOp::BlendF32 => {
                let stride = clip.u32_at(s, 0x14)?;
                let base = stride.wrapping_mul(track).wrapping_add(0x20);
                let (at_a, at_b) = (
                    base.wrapping_add(f.a.wrapping_mul(16)),
                    base.wrapping_add(f.b.wrapping_mul(16)),
                );
                // `movaps` loads: a misaligned key faults in the guest.
                for at in [at_a, at_b] {
                    if cx.data_addr.wrapping_add(clip.blob_offset(s, at)) & 15 != 0 {
                        cx.misaligned = true;
                        return None;
                    }
                }
                let a: Vec4 = words(clip.slice(s, at_a)?);
                let b: Vec4 = words(clip.slice(s, at_b)?);
                let d = fp.sub4(b, a);
                let d = fp.mul4(d, [f.w; 4]);
                let q = fp.add4(d, a);
                normalize4(fp, q)
            }
            RotOp::KeyInt { wide } | RotOp::KeyInterp { wide } => {
                cx.mmx = true;
                let keys = Keys::new(
                    clip,
                    s,
                    track.wrapping_mul(4).wrapping_add(0x30),
                    0x14,
                    0x20,
                    8,
                    wide,
                )?;
                let lo = bisect(|k| keys.time(clip, s, k), keys.count, f.a)?;
                let value = |k: u32| {
                    clip.slice::<8>(s, keys.values.wrapping_add(k.wrapping_mul(8)))
                        .map(quat16)
                };
                let q0 = value(lo)?;
                let weights = if matches!(self, RotOp::KeyInt { .. }) {
                    let t0 = keys.time(clip, s, lo)?;
                    if f.a == t0 {
                        None
                    } else {
                        let t1 = keys.time(clip, s, lo.wrapping_add(1))?;
                        Some((
                            det::from_int(t1.wrapping_sub(f.a) as i32),
                            det::from_int(f.a.wrapping_sub(t0) as i32),
                        ))
                    }
                } else if lo + 1 < keys.count {
                    let t1 = keys.time(clip, s, lo + 1)?;
                    let t0 = keys.time(clip, s, lo)?;
                    let at = fp.add(det::from_int(f.a as i32), f.w);
                    let early = fp.sub(det::from_int(t1 as i32), at);
                    let late = fp.sub(at, det::from_int(t0 as i32));
                    Some((early, late))
                } else {
                    None
                };
                match weights {
                    None => normalize4(fp, q0),
                    Some((w0, w1)) => {
                        let q1 = value(lo.wrapping_add(1))?;
                        let a = fp.mul4(q0, [w0; 4]);
                        let b = fp.mul4(q1, [w1; 4]);
                        let q = fp.add4(a, b);
                        normalize4(fp, q)
                    }
                }
            }
        })
    }
}

impl VecOp {
    /// One translation (three words) or scale (the first word), as the
    /// codec function would store it.
    pub fn sample(
        self,
        cx: &mut Ctx,
        clip: &Clip,
        s: &Stream,
        v: Vector,
        track: u32,
        f: Frame,
    ) -> Option<[u32; 3]> {
        let n = v.width();
        let size = 4 * n as u32;
        let read = |at: u32| -> Option<[u32; 3]> {
            let mut out = [0; 3];
            for (i, w) in out.iter_mut().enumerate().take(n) {
                *w = clip.u32_at(s, at.wrapping_add(4 * i as u32))?;
            }
            Some(out)
        };
        let fp = &mut cx.fp;
        let lerp = |fp: &mut Fp, a: [u32; 3], b: [u32; 3], t: u32| {
            let mut out = a;
            for i in 0..n {
                let d = fp.sub(b[i], a[i]);
                let d = fp.mul(d, t);
                out[i] = fp.add(d, a[i]);
            }
            out
        };
        Some(match self {
            VecOp::Static => {
                let base = clip.u32_at(s, v.base())?;
                read(base.wrapping_add(track.wrapping_mul(size)))?
            }
            VecOp::FrameRaw | VecOp::FrameLerp => {
                let base = clip.u32_at(s, v.base())?;
                let stride = clip.u32_at(s, v.stride())?;
                let track = base.wrapping_add(stride.wrapping_mul(track));
                let a = read(track.wrapping_add(f.a.wrapping_mul(size)))?;
                if self == VecOp::FrameRaw {
                    a
                } else {
                    let b = read(track.wrapping_add(f.b.wrapping_mul(size)))?;
                    lerp(fp, a, b, f.w)
                }
            }
            VecOp::KeyInt { wide } | VecOp::KeyInterp { wide } => {
                let word = clip
                    .u32_at(s, v.base())?
                    .wrapping_add(track.wrapping_mul(4));
                let keys = Keys::new(clip, s, word, v.stride(), v.values(), size, wide)?;
                let lo = bisect(|k| keys.time(clip, s, k), keys.count, f.a)?;
                let value = |k: u32| read(keys.values.wrapping_add(k.wrapping_mul(size)));
                let v0 = value(lo)?;
                if lo + 1 >= keys.count {
                    return Some(v0);
                }
                let t0 = keys.time(clip, s, lo)?;
                let offset = if matches!(self, VecOp::KeyInt { .. }) {
                    if f.a == t0 {
                        return Some(v0);
                    }
                    fp.sub(det::from_int(f.a as i32), det::from_int(t0 as i32))
                } else {
                    let at = fp.add(det::from_int(f.a as i32), f.w);
                    fp.sub(at, det::from_int(t0 as i32))
                };
                let t1 = keys.time(clip, s, lo + 1)?;
                let span = det::from_int(t1.wrapping_sub(t0) as i32);
                let t = fp.mul(det::rcp(span), offset);
                let v1 = value(lo + 1)?;
                lerp(fp, v0, v1, t)
            }
        })
    }
}
