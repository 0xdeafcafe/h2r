//! `sample`: one animation evaluated into the guest's per-node records,
//! as `0x279860` and the kind-0 evaluators under it would (docs/engine/
//! anim-sampling.md "Evaluators").
//!
//! A record is 32 bytes: rotation quaternion at `+0`, translation at
//! `+0x10`, scale at `+0x1c`. Per stream (static, then animated) and per
//! channel (rotation, translation, scale), each flagged node below the
//! node count whose mask bit is set gets the codec's sample, blended with
//! a second frame when the request has one, and combined into the record
//! by the mode.
use crate::clip::{Channel, Clip, Stream, Which};
use crate::codec::{Ctx, Frame, RotOp, VecOp, Vector};
use sim_core::det::{self, Fp};
use sim_core::lanes::Vec4;

/// One node's record as binary32 bits: rotation `[0..4]`, translation
/// `[4..7]`, scale `[7]`.
pub type Record = [u32; 8];

/// Bits of [`sample`]'s `written` output, per node.
pub const ROTATION: u8 = 1;
pub const TRANSLATION: u8 = 2;
pub const SCALE: u8 = 4;

/// How a sample combines with the record (`0x504458`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// 0: the sample replaces the record.
    Replace,
    /// 1: the record moves toward the sample by the weight.
    Blend,
    /// 2: the sample is added (rotations composed).
    Add,
    /// 3: the sample, scaled by the weight, is added.
    AddScaled,
}

/// The codec function per channel for one stream. `None` is a function
/// the kernel does not know; a call that would reach it declines.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ops {
    pub rotation: Option<RotOp>,
    pub translation: Option<VecOp>,
    pub scale: Option<VecOp>,
}

/// The second frame (`0x5044a4/a8/ac`) and its blend (`0x5044b0`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Secondary {
    pub frame: Frame,
    pub blend: u32,
}

/// Everything the evaluators read besides the clip and the records, as
/// the setup in `0x2798a0` leaves it.
#[derive(Clone, Copy, Debug)]
pub struct Request<'a> {
    pub mode: Mode,
    /// `0x504470`.
    pub weight: u32,
    /// `0x50445c`: nodes at or above it are skipped.
    pub count: i32,
    /// `0x50447c`: one bit per node, LSB first.
    pub mask: Option<&'a [u32]>,
    pub frame: Frame,
    pub secondary: Option<Secondary>,
    /// `0x45dbc0` and the first word of `0x47ffc0`, read from the image.
    pub one: u32,
    pub sign: u32,
    /// The blob's guest address, for the aligned-load codec.
    pub data_addr: u32,
    /// Per [`Which`]: the stream's codec functions, or `None` when the
    /// setup does not evaluate it.
    pub streams: [Option<Ops>; 2],
    /// The guest's MXCSR at the first evaluator; the arithmetic runs under
    /// it ([`Fp::guest`]) and [`Effects::mxcsr`] is the MXCSR after.
    pub mxcsr: u32,
}

/// What the call did besides the records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Effects {
    /// An int16 codec ran its MMX path: x87 TOP is 0 and every tag empty.
    pub mmx: bool,
    /// The guest's MXCSR after the call.
    pub mxcsr: u32,
}

/// Why [`sample`] cannot reproduce a call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The record or written buffers are shorter than the node count.
    Records,
    /// The setup evaluates a stream the clip does not hold.
    NoStream(Which),
    /// The mask has no word for a live node.
    Mask { node: usize },
    /// A live node reaches a codec function the kernel does not know.
    UnknownOp { which: Which, channel: Channel },
    /// The codec cannot read the sample: a key or value past the blob, or
    /// (`misaligned`) a `movaps` key off 16 bytes.
    Codec { which: Which, channel: Channel, node: usize, track: u32, misaligned: bool },
    /// Mode 0 with a second frame reaches a live node ("Broken variants").
    Broken { which: Which, channel: Channel, node: usize },
    /// An operation raised an SSE exception the MXCSR leaves unmasked.
    Unmasked,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Records => write!(f, "records shorter than the node count"),
            Self::NoStream(which) => write!(f, "no {which:?} stream"),
            Self::Mask { node } => write!(f, "no mask word for node {node}"),
            Self::UnknownOp { which, channel } => write!(f, "unknown {which:?} {channel:?} codec function"),
            Self::Codec { which, channel, node, track, misaligned } => write!(
                f,
                "{which:?} {channel:?} node {node} track {track}: {}",
                if misaligned { "movaps key off 16 bytes" } else { "a read past the blob" }
            ),
            Self::Broken { which, channel, node } => {
                write!(f, "mode 0 with a second frame reaches {which:?} {channel:?} node {node}")
            }
            Self::Unmasked => write!(f, "unmasked SSE exception"),
        }
    }
}

/// The kernel cannot reproduce this call exactly; the guest must run it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unsupported;

/// The evaluators' quaternion blend: dot product, flip the target into the
/// same hemisphere, lerp, renormalize with one reciprocal square root.
pub(crate) fn nlerp(fp: &mut Fp, a: Vec4, b: Vec4, w: u32, sign: u32) -> Vec4 {
    let m = fp.mul4(a, b);
    let even = fp.add(m[0], m[2]);
    let odd = fp.add(m[1], m[3]);
    let dot = fp.add(even, odd);
    let flip = dot & sign;
    let b = b.map(|x| x ^ flip);
    let d = fp.sub4(b, a);
    let d = fp.mul4(d, [w; 4]);
    let r = fp.add4(d, a);
    let sq = fp.mul4(r, r);
    let even = fp.add(sq[0], sq[2]);
    let odd = fp.add(sq[1], sq[3]);
    let n = fp.add(even, odd);
    fp.mul4(r, [det::rsqrt(n); 4])
}

/// The additive modes' quaternion product `a ⊗ b`, in the evaluators'
/// shuffle order.
pub(crate) fn product(fp: &mut Fp, a: Vec4, b: Vec4, sign: u32) -> Vec4 {
    let x = fp.mul4([a[3]; 4], b);
    let mut y = fp.mul4([b[3], b[3], b[3], b[0]], [a[0], a[1], a[2], a[0]]);
    y[3] ^= sign;
    let x = fp.add4(x, y);
    let mut z = fp.mul4([b[2], b[0], b[1], b[1]], [a[1], a[2], a[0], a[1]]);
    z[3] ^= sign;
    let x = fp.add4(x, z);
    let u = fp.mul4([a[2], a[0], a[1], a[2]], [b[1], b[2], b[0], b[2]]);
    fp.sub4(x, u)
}

/// The live `(node, track)` pairs of one channel: below the count, mask
/// bit set. A mask too short for a live node declines.
fn live<'s>(
    s: &'s Stream,
    c: Channel,
    count: usize,
    mask: Option<&'s [u32]>,
) -> impl Iterator<Item = Result<(usize, u32), Refusal>> + 's {
    s.flagged[c as usize]
        .iter()
        .take_while(move |&&(node, _)| usize::from(node) < count)
        .filter_map(move |&(node, track)| {
            let n = usize::from(node);
            let on = match mask {
                None => true,
                Some(words) => match words.get(n >> 5) {
                    Some(w) => w >> (n & 31) & 1 != 0,
                    None => return Some(Err(Refusal::Mask { node: n })),
                },
            };
            on.then_some(Ok((n, u32::from(track))))
        })
}

struct Walk<'a, 'r> {
    req: &'a Request<'a>,
    clip: &'a Clip,
    cx: Ctx,
    records: &'r mut [Record],
    written: &'r mut [u8],
    count: usize,
}

impl Walk<'_, '_> {
    fn stream(&mut self, which: Which, ops: Ops) -> Result<(), Refusal> {
        let clip = self.clip;
        let s = clip.stream(which).ok_or(Refusal::NoStream(which))?;
        let req = self.req;
        let (mode, w, one, sign) = (req.mode, req.weight, req.one, req.sign);

        for item in live(s, Channel::Rotation, self.count, req.mask) {
            let (node, track) = item?;
            let channel = Channel::Rotation;
            let op = ops.rotation.ok_or(Refusal::UnknownOp { which, channel })?;
            let codec = |cx: &Ctx| Refusal::Codec { which, channel, node, track, misaligned: cx.misaligned };
            let mut v = op.sample(&mut self.cx, clip, s, track, req.frame).ok_or_else(|| codec(&self.cx))?;
            if let Some(sec) = req.secondary {
                let q = op.sample(&mut self.cx, clip, s, track, sec.frame).ok_or_else(|| codec(&self.cx))?;
                v = nlerp(&mut self.cx.fp, v, q, sec.blend, sign);
            }
            let fp = &mut self.cx.fp;
            let rec: Vec4 = self.records[node][0..4].try_into().unwrap();
            let out = match mode {
                Mode::Replace => v,
                Mode::Blend => nlerp(fp, rec, v, w, sign),
                Mode::Add => product(fp, rec, v, sign),
                Mode::AddScaled => {
                    let c = one | (v[3] & sign);
                    let t = fp.sub(v[3], c);
                    let t = fp.mul(w, t);
                    let last = fp.add(t, c);
                    let head = fp.mul4(v, [w; 4]);
                    product(fp, rec, [head[0], head[1], head[2], last], sign)
                }
            };
            self.records[node][0..4].copy_from_slice(&out);
            self.written[node] |= ROTATION;
        }

        for (channel, v, ops_of, at, bit) in [
            (
                Channel::Translation,
                Vector::Translation,
                ops.translation,
                4usize,
                TRANSLATION,
            ),
            (Channel::Scale, Vector::Scale, ops.scale, 7, SCALE),
        ] {
            let n = if v == Vector::Translation { 3 } else { 1 };
            for item in live(s, channel, self.count, req.mask) {
                let (node, track) = item?;
                let op = ops_of.ok_or(Refusal::UnknownOp { which, channel })?;
                let codec = |cx: &Ctx| Refusal::Codec { which, channel, node, track, misaligned: cx.misaligned };
                let mut p = op.sample(&mut self.cx, clip, s, v, track, req.frame).ok_or_else(|| codec(&self.cx))?;
                if let Some(sec) = req.secondary {
                    let q = op.sample(&mut self.cx, clip, s, v, track, sec.frame).ok_or_else(|| codec(&self.cx))?;
                    let fp = &mut self.cx.fp;
                    for i in 0..n {
                        let d = fp.sub(q[i], p[i]);
                        let d = fp.mul(d, sec.blend);
                        p[i] = fp.add(d, p[i]);
                    }
                }
                let fp = &mut self.cx.fp;
                let rec = &mut self.records[node][at..at + n];
                for i in 0..n {
                    let (r, x) = (rec[i], p[i]);
                    rec[i] = match (mode, v) {
                        (Mode::Replace, _) => x,
                        (Mode::Blend, _) => {
                            let d = fp.sub(x, r);
                            let d = fp.mul(d, w);
                            fp.add(d, r)
                        }
                        (Mode::Add, Vector::Translation) => fp.add(r, x),
                        (Mode::Add, Vector::Scale) => fp.mul(r, x),
                        (Mode::AddScaled, Vector::Translation) => {
                            let d = fp.mul(w, x);
                            fp.add(d, r)
                        }
                        (Mode::AddScaled, Vector::Scale) => {
                            let d = fp.sub(x, one);
                            let d = fp.mul(d, w);
                            let d = fp.add(d, one);
                            fp.mul(d, r)
                        }
                    };
                }
                self.written[node] |= bit;
            }
        }
        Ok(())
    }
}

/// Evaluate `req` over `clip` into `records` (the guest's records for
/// nodes `0..count`, as read before the call; for [`Mode::Replace`] their
/// contents do not matter). `written` gets the channel bits each node's
/// record took. On `Err` the buffers hold partial results and the call
/// must be left to the guest; on `Ok` the written fields are exactly what
/// the guest would store and the guest's MXCSR would not have changed.
pub fn sample(
    req: &Request,
    clip: &Clip,
    records: &mut [Record],
    written: &mut [u8],
) -> Result<Effects, Refusal> {
    let count = usize::try_from(req.count).unwrap_or(0);
    if records.len() < count || written.len() < count {
        return Err(Refusal::Records);
    }
    let mut walk = Walk {
        req,
        clip,
        cx: Ctx {
            fp: Fp::guest(req.mxcsr),
            mmx: false,
            data_addr: req.data_addr,
            one: req.one,
            misaligned: false,
        },
        records,
        written,
        count,
    };
    let streams = [
        (Which::Static, req.streams[0]),
        (Which::Animated, req.streams[1]),
    ];

    // Mode 0 with a second frame is the pair of evaluators whose inline
    // assembly loses its frame registers (anim-sampling.md "Broken
    // variants"): what they do depends on stack garbage. Only a call that
    // reaches no node is reproduced.
    if req.mode == Mode::Replace && req.secondary.is_some() {
        for (which, ops) in streams {
            if ops.is_none() {
                continue;
            }
            let s = clip.stream(which).ok_or(Refusal::NoStream(which))?;
            for channel in [Channel::Rotation, Channel::Translation, Channel::Scale] {
                if let Some(item) = live(s, channel, walk.count, req.mask).next() {
                    let (node, _) = item?;
                    return Err(Refusal::Broken { which, channel, node });
                }
            }
        }
        return Ok(Effects { mmx: false, mxcsr: req.mxcsr });
    }

    for (which, ops) in streams {
        if let Some(ops) = ops {
            walk.stream(which, ops)?;
        }
    }
    if !walk.cx.fp.exact() {
        return Err(Refusal::Unmasked);
    }
    Ok(Effects { mmx: walk.cx.mmx, mxcsr: walk.cx.fp.mxcsr().unwrap_or(req.mxcsr) })
}
