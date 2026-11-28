//! The float MOPP ray: the interpreter `0x003009e0` under entry I
//! (`0x00301800`), as a pull-style walk over the program's bytecode held in
//! its [`super::Tree`] (docs/engine/sim-rewrite-collision.md section 5.3,
//! "MOPP ray walk" and "Ray VM at entry I").
//!
//! The interpreter carries a frame (offsets, rescale bits, frame scale, key
//! offset, property) and a segment in frame units. Each opcode replays the
//! retail float sequence: SSE for the packed segment work, x87 for the
//! projections, thresholds and clip weights, in the precision the guest's
//! x87 control selects ([`Precision`]). A split that sends the ray to both
//! sides makes a nested call with a clipped copy of the segment; on return
//! it clips its own start for the other side, and when the early-out
//! fraction has dropped below one it recomputes its end from the
//! world-space ray and, after an axis split, skips the other side when the
//! ray now stops short of it.
//!
//! [`RayWalk`] keeps those nested calls on an explicit stack. The caller
//! pulls [`Event`]s: each [`Event::Enter`] is a call the original makes
//! (the root, and the first child of every two-sided split), each
//! [`Event::Key`] a terminal. After a key the caller runs the terminal's
//! callbacks and passes the refreshed fraction to the next
//! [`RayWalk::next`]. [`RayCheck`] compares a walk with what the guest
//! did, call by call.
//!
//! Every operand and result stays a normal binary32 (or binary64 in double
//! precision) or an exact zero; anything else is [`Bail::Float`], so no
//! flag but the precision flag can differ from retail. An opcode the
//! original asserts on, or a read past the program, is [`Bail::Data`];
//! nesting past [`super::MAX_FRAMES`] calls is [`Bail::Depth`].
mod check;

pub use check::{Outcome, RayCheck, Unverified};

use super::MAX_FRAMES;
use crate::Bail;
use ::mopp::boxvm::diagonal_plane;
use ::mopp::code::terminal_key;
use sim_core::det::{Fp, X87};
use std::cell::Cell;
use std::cmp::Ordering;
use std::fmt;

/// A packed SSE register as four binary32 bit patterns, lane 0 first.
pub type Vec4 = [u32; 4];

/// The x87 precision control the interpreter's scalar sequences run in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Precision {
    /// `PC = 00`: every x87 result rounds to 24 bits.
    Single,
    /// `PC = 10`: 53 bits, rounded to binary32 where the original stores.
    Double,
    /// `PC = 11`: 64 bits, on the `x87` crate's model.
    Extended,
}

impl Precision {
    /// The precision of a masked, round-to-nearest control word. Every
    /// other control (another rounding, an unmasked exception, the
    /// reserved precision) is [`Bail::Env`].
    pub fn from_control(control: u16) -> Result<Self, Bail> {
        if control & 0x0c3f != 0x003f {
            return Err(Bail::Env);
        }
        match (control >> 8) & 3 {
            0 => Ok(Precision::Single),
            2 => Ok(Precision::Double),
            3 => Ok(Precision::Extended),
            _ => Err(Bail::Env),
        }
    }

    /// The masked, round-to-nearest control word of this precision.
    pub fn control(self) -> u16 {
        match self {
            Precision::Single => 0x007f,
            Precision::Double => 0x027f,
            Precision::Extended => 0x037f,
        }
    }
}

/// Guest constants the interpreter and its setup read, binary32 bits.
/// The adapter reads them from guest memory; nothing here copies them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Constants {
    /// The zero the clip products compare against.
    pub zero: u32,
    /// The one of `S + 1`, the fraction test and the inverse scale.
    pub one: u32,
    /// The packed vector of `1 - w` in the clip blends.
    pub unit: Vec4,
    /// The factor the setup multiplies the code scale by for the root
    /// frame.
    pub root_scale: u32,
}

/// One query's inputs, owned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ray {
    /// The code object's first 16 bytes: the origin, and the scale in
    /// lane 3 (the original subtracts them as one packed vector).
    pub origin: Vec4,
    /// The ray input's two vectors (VM `+0x20` and `+0x30`).
    pub from: Vec4,
    pub to: Vec4,
    pub precision: Precision,
    /// The guest's MXCSR at the call, under which the packed work runs.
    pub mxcsr: u32,
    pub constants: Constants,
}

/// The interpreter's frame, in its guest layout: offsets `+0x00`, rescale
/// bits `+0x10`, frame scale `+0x14`, key offset `+0x18`, property `+0x1c`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub offset: Vec4,
    pub bits: u32,
    pub scale: u32,
    pub key: u32,
    pub property: u32,
}

impl Frame {
    pub fn words(&self) -> [u32; 8] {
        let o = self.offset;
        [
            o[0],
            o[1],
            o[2],
            o[3],
            self.bits,
            self.scale,
            self.key,
            self.property,
        ]
    }

    pub fn from_words(w: [u32; 8]) -> Self {
        Self {
            offset: [w[0], w[1], w[2], w[3]],
            bits: w[4],
            scale: w[5],
            key: w[6],
            property: w[7],
        }
    }
}

/// A segment in frame units: start `+0x00`, end `+0x10`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    pub start: Vec4,
    pub end: Vec4,
}

impl Segment {
    pub fn words(&self) -> [u32; 8] {
        let (s, e) = (self.start, self.end);
        [s[0], s[1], s[2], s[3], e[0], e[1], e[2], e[3]]
    }

    pub fn from_words(w: [u32; 8]) -> Self {
        Self {
            start: [w[0], w[1], w[2], w[3]],
            end: [w[4], w[5], w[6], w[7]],
        }
    }
}

/// What the setup (`0x00301800` or `0x00301720`) computes before its call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Root {
    /// VM `+0x14`: `one / scale` stored to binary32.
    pub inverse_scale: u32,
    pub frame: Frame,
    pub segment: Segment,
    /// The flags the setup's arithmetic raised; the walk continues from
    /// them.
    pub raised: Raised,
}

/// One step of the walk the caller sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The original calls the interpreter: `pc` is the bytecode offset
    /// (the guest passes `code + 0x30 + pc`), with the frame and the
    /// segment at the call.
    Enter {
        pc: u32,
        frame: Frame,
        segment: Segment,
    },
    /// A terminal: its key plus the frame's key offset. The frame ends.
    Key(u32),
}

impl fmt::Display for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Event::Enter { pc, frame, segment } => {
                write!(f, "enter pc {pc:#x} frame")?;
                for w in frame.words() {
                    write!(f, " {w:08x}")?;
                }
                write!(f, " segment")?;
                for w in segment.words() {
                    write!(f, " {w:08x}")?;
                }
                Ok(())
            }
            Event::Key(key) => write!(f, "key {key:#x}"),
        }
    }
}

/// An x87 stack value.
type St = sim_core::det::St;

/// The flags a walk raised: the guest's MXCSR after its SSE operations
/// and the x87 exception flags (`0x3f`) of its x87 sequences, PE included.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Raised {
    pub mxcsr: u32,
    pub x87: u16,
}

/// The interpreter's floats under the guest's x87 control word and MXCSR,
/// for any operand: the x87 sequences on [`sim_core::det::X87`], the
/// packed SSE work on [`Fp::guest`].
#[derive(Clone, Debug)]
struct Fpu {
    x87: X87,
    sse: Cell<Fp>,
}

impl Fpu {
    fn new(precision: Precision, raised: Raised) -> Self {
        Self {
            x87: X87::with_flags(precision.control(), raised.x87),
            sse: Cell::new(Fp::guest(raised.mxcsr)),
        }
    }

    fn raised(&self) -> Raised {
        Raised {
            mxcsr: self.sse.get().mxcsr().unwrap_or_default(),
            x87: self.x87.flags(),
        }
    }

    /// `fld m32`.
    fn load(&self, bits: u32) -> Result<St, Bail> {
        Ok(self.x87.load32(bits))
    }

    /// `fild m32` of an operand below `2^24`.
    fn int(&self, v: u32) -> St {
        self.x87.int(i64::from(v))
    }

    fn add(&self, a: St, b: St) -> Result<St, Bail> {
        Ok(self.x87.add(a, b))
    }
    fn sub(&self, a: St, b: St) -> Result<St, Bail> {
        Ok(self.x87.sub(a, b))
    }
    fn mul(&self, a: St, b: St) -> Result<St, Bail> {
        Ok(self.x87.mul(a, b))
    }
    fn div(&self, a: St, b: St) -> Result<St, Bail> {
        Ok(self.x87.div(a, b))
    }

    /// `fst(p) m32`.
    fn store(&self, a: St) -> Result<u32, Bail> {
        Ok(self.x87.store32(a))
    }

    /// `fcomp; fnstsw ax; test ah, 5` (or `41h`): every branch the
    /// interpreter takes on it tests for an ordered less or greater, so an
    /// unordered compare (`None`) takes neither.
    fn cmp(&self, a: St, b: St) -> Option<Ordering> {
        self.x87.compare(a, b, false)
    }

    /// One packed SSE operation, lane by lane, rounded once per lane. Only
    /// an unmasked exception refuses, which the entry's environment check
    /// rules out.
    fn packed(&self, op: impl FnOnce(&mut Fp) -> Vec4) -> Result<Vec4, Bail> {
        let mut fp = self.sse.get();
        let r = op(&mut fp);
        self.sse.set(fp);
        if fp.exact() { Ok(r) } else { Err(Bail::Env) }
    }

    fn add4(&self, a: Vec4, b: Vec4) -> Result<Vec4, Bail> {
        self.packed(|fp| fp.add4(a, b))
    }
    fn sub4(&self, a: Vec4, b: Vec4) -> Result<Vec4, Bail> {
        self.packed(|fp| fp.sub4(a, b))
    }
    fn mul4(&self, a: Vec4, b: Vec4) -> Result<Vec4, Bail> {
        self.packed(|fp| fp.mul4(a, b))
    }
}

fn splat(x: u32) -> Vec4 {
    [x; 4]
}

/// `cvtsi2ss`-free conversion the original makes with `fild`/`fstp m32`
/// of a byte or a threshold below `2^24`: exact.
fn small(v: i64) -> u32 {
    debug_assert!(v.unsigned_abs() < 1 << 24);
    (v as f32).to_bits()
}

impl Ray {
    /// The setup's VM scale word, root frame and root segment
    /// (`0x00301800..0x003018c2`): `one / scale` stored, the root frame
    /// scale `scale * root_scale` stored, and both ends as
    /// `(p - origin) * frame_scale`, packed.
    pub fn root(&self) -> Result<Root, Bail> {
        let fpu = Fpu::new(
            self.precision,
            Raised {
                mxcsr: self.mxcsr,
                x87: 0,
            },
        );
        let scale = fpu.load(self.origin[3])?;
        let inverse_scale = fpu.store(fpu.div(fpu.load(self.constants.one)?, scale)?)?;
        let s = fpu.store(fpu.mul(scale, fpu.load(self.constants.root_scale)?)?)?;
        let start = fpu.mul4(splat(s), fpu.sub4(self.from, self.origin)?)?;
        let end = fpu.mul4(splat(s), fpu.sub4(self.to, self.origin)?)?;
        Ok(Root {
            inverse_scale,
            frame: Frame {
                offset: [0; 4],
                bits: 0,
                scale: s,
                key: 0,
                property: 0,
            },
            segment: Segment { start, end },
            raised: fpu.raised(),
        })
    }

    /// `(1 - w) * a + w * b`, packed, as every clip writes it.
    fn blend(&self, fpu: &Fpu, w: u32, a: Vec4, b: Vec4) -> Result<Vec4, Bail> {
        let w = splat(w);
        fpu.add4(fpu.mul4(fpu.sub4(self.constants.unit, w)?, a)?, fpu.mul4(w, b)?)
    }

    /// The end a return site recomputes once the fraction drops below one:
    /// `(((1 - f) * from + f * to) - origin) * frame_scale - frame_offset`.
    fn end_at(&self, fpu: &Fpu, fraction: u32, frame: &Frame) -> Result<Vec4, Bail> {
        let world = self.blend(fpu, fraction, self.from, self.to)?;
        let local = fpu.mul4(splat(frame.scale), fpu.sub4(world, self.origin)?)?;
        fpu.sub4(local, frame.offset)
    }
}

/// What a two-sided split does once its first child returns.
#[derive(Clone, Copy, Debug)]
enum Resume {
    /// The ray rose through the split: the low child ran; clip the start
    /// with the high-threshold distances, then the high child unless the
    /// recomputed end is below the high threshold.
    High {
        start: u32,
        end: u32,
        threshold: u32,
        axis: Option<usize>,
        next: usize,
    },
    /// The ray fell: the high child ran; clip with the low-threshold
    /// distances, then the low child unless the end is above the low
    /// threshold.
    Low {
        start: u32,
        end: u32,
        threshold: u32,
        axis: Option<usize>,
        next: usize,
    },
}

/// One active interpreter call.
#[derive(Clone, Debug)]
struct Level {
    pc: usize,
    frame: Frame,
    segment: Segment,
    resume: Option<Resume>,
}

/// The query state every level shares.
#[derive(Clone, Debug)]
struct Ctx {
    ray: Ray,
    fpu: Fpu,
    inverse_scale: u32,
    fraction: u32,
}

/// The walk of one ray through one program. `B` holds the bytecode (from
/// `+0x30`), owned or borrowed.
#[derive(Clone, Debug)]
pub struct RayWalk<B> {
    code: B,
    ctx: Ctx,
    levels: Vec<Level>,
    started: bool,
}

impl<B: AsRef<[u8]>> RayWalk<B> {
    pub fn new(code: B, ray: Ray, root: &Root) -> Self {
        Self {
            code,
            ctx: Ctx {
                ray,
                fpu: Fpu::new(ray.precision, root.raised),
                inverse_scale: root.inverse_scale,
                fraction: ray.constants.one,
            },
            levels: vec![Level {
                pc: 0,
                frame: root.frame,
                segment: root.segment,
                resume: None,
            }],
            started: false,
        }
    }

    /// The next call or terminal, with `fraction` the early-out fraction
    /// the VM holds now (VM `+0x54`): every return site until the next
    /// event reads it. `None` once the root call has returned.
    pub fn next(&mut self, fraction: u32) -> Result<Option<Event>, Bail> {
        self.ctx.fraction = fraction;
        if !self.started {
            self.started = true;
            let root = &self.levels[0];
            return Ok(Some(Event::Enter {
                pc: 0,
                frame: root.frame,
                segment: root.segment,
            }));
        }
        let code = self.code.as_ref();
        loop {
            let Some(top) = self.levels.len().checked_sub(1) else {
                return Ok(None);
            };
            if let Some(resume) = self.levels[top].resume.take()
                && !resume_after_child(&self.ctx, &mut self.levels[top], resume)?
            {
                self.levels.pop();
                continue;
            }
            if let Some(event) = step(code, &self.ctx, &mut self.levels)? {
                return Ok(Some(event));
            }
        }
    }

    /// The flags the setup and the walk so far raised, for the entry to
    /// leave in MXCSR and the x87 status word.
    pub fn raised(&self) -> Raised {
        self.ctx.fpu.raised()
    }

    /// Continue from the guest's state after a callback: its `mxcsr` and
    /// x87 status word, plus the flags the walk has raised.
    pub fn absorb(&mut self, mxcsr: u32, x87_status: u16) {
        let raised = self.ctx.fpu.raised();
        self.ctx.fpu = Fpu::new(
            self.ctx.ray.precision,
            Raised {
                mxcsr: mxcsr | (raised.mxcsr & 0x3f),
                x87: (x87_status & 0x3f) | raised.x87,
            },
        );
    }

    /// Nested calls active now, the root included.
    pub fn depth(&self) -> usize {
        self.levels.len()
    }
}

/// The return site of a two-sided split (`0x00300e9d`, `0x00300ff2`).
/// `false` when the frame returns there.
fn resume_after_child(ctx: &Ctx, level: &mut Level, resume: Resume) -> Result<bool, Bail> {
    let fpu = &ctx.fpu;
    let (Resume::High {
        start,
        end,
        threshold,
        axis,
        next,
    }
    | Resume::Low {
        start,
        end,
        threshold,
        axis,
        next,
    }) = resume;
    let zero = fpu.load(ctx.ray.constants.zero)?;
    let (s, e) = (fpu.load(start)?, fpu.load(end)?);
    if fpu.cmp(fpu.mul(e, s)?, zero) == Some(Ordering::Less) {
        let w = fpu.store(fpu.div(s, fpu.sub(s, e)?)?)?;
        let seg = level.segment;
        level.segment.start = ctx.ray.blend(fpu, w, seg.start, seg.end)?;
    }
    if fpu.cmp(fpu.load(ctx.fraction)?, fpu.load(ctx.ray.constants.one)?) == Some(Ordering::Less) {
        level.segment.end = ctx.ray.end_at(fpu, ctx.fraction, &level.frame)?;
        if let Some(a) = axis {
            let end = fpu.load(level.segment.end[a])?;
            let t = fpu.load(threshold)?;
            let short = match resume {
                Resume::High { .. } => fpu.cmp(end, t) == Some(Ordering::Less),
                Resume::Low { .. } => fpu.cmp(end, t) == Some(Ordering::Greater),
            };
            if short {
                return Ok(false);
            }
        }
    }
    level.pc = next;
    Ok(true)
}

/// Big-endian operand bytes at `pc + at`.
fn be(code: &[u8], pc: usize, at: usize, n: usize) -> Result<u32, Bail> {
    (0..n).try_fold(0u32, |v, i| {
        let b = *code.get(pc + at + i).ok_or(Bail::Data)?;
        Ok((v << 8) | u32::from(b))
    })
}

/// The projection of one end on a diagonal plane, in the original's
/// operand order (`0x00300a1e..0x00300c77`): the first axis loaded, then
/// each `fadd`/`fsub` in turn.
fn project(fpu: &Fpu, op: u8, p: Vec4) -> Result<St, Bail> {
    let (first, rest): (usize, &[(bool, usize)]) = match op {
        0x13 => (2, &[(true, 1)]),
        0x14 => (1, &[(false, 2)]),
        0x15 => (2, &[(true, 0)]),
        0x16 => (0, &[(false, 2)]),
        0x17 => (1, &[(true, 0)]),
        0x18 => (0, &[(false, 1)]),
        0x19 => (2, &[(true, 1), (true, 0)]),
        0x1a => (1, &[(true, 0), (false, 2)]),
        0x1b => (0, &[(false, 1), (true, 2)]),
        _ => (0, &[(false, 1), (false, 2)]),
    };
    let mut v = fpu.load(p[first])?;
    for &(plus, axis) in rest {
        let x = fpu.load(p[axis])?;
        v = if plus { fpu.add(v, x)? } else { fpu.sub(v, x)? };
    }
    Ok(v)
}

/// A split's thresholds and children.
struct Split {
    /// `[esp+0x20]` and `[esp+0x1c]`: binary32 bits.
    low: u32,
    high: u32,
    first: usize,
    second: usize,
    /// The axis of an axis split, which the return site checks the
    /// recomputed end on; diagonals have none.
    axis: Option<usize>,
}

/// Run one opcode of the top level. `Some` when the walk has something to
/// report: a nested call (pushed) or a terminal (popped).
fn step(code: &[u8], ctx: &Ctx, levels: &mut Vec<Level>) -> Result<Option<Event>, Bail> {
    let fpu = &ctx.fpu;
    let top = levels.len() - 1;
    let level = &mut levels[top];
    let pc = level.pc;
    let op = *code.get(pc).ok_or(Bail::Data)?;
    let byte = |at: usize| be(code, pc, at, 1);
    let split = match op {
        0x00 => {
            levels.pop();
            return Ok(None);
        }
        0x01..=0x04 => {
            rescale(fpu, level, u32::from(op), [byte(1)?, byte(2)?, byte(3)?])?;
            level.pc = pc + 4;
            return Ok(None);
        }
        0x05..=0x07 => {
            let n = usize::from(op - 4);
            level.pc = pc + 1 + n + be(code, pc, 1, n)? as usize;
            return Ok(None);
        }
        0x09..=0x0b => {
            let n = 1usize << (op - 9);
            let v = be(code, pc, 1, n)?;
            level.frame.key = if n == 4 {
                v
            } else {
                level.frame.key.wrapping_add(v)
            };
            level.pc = pc + 1 + n;
            return Ok(None);
        }
        0x60 | 0x64 | 0x68 => {
            let n = 1usize << ((op - 0x60) / 4);
            level.frame.property = be(code, pc, 1, n)?;
            level.pc = pc + 1 + n;
            return Ok(None);
        }
        0x30..=0x53 => {
            let key = terminal_key(op, |n| be(code, pc, 1, n))?;
            let key = level.frame.key.wrapping_add(key);
            levels.pop();
            return Ok(Some(Event::Key(key)));
        }
        0x26..=0x2b => {
            if !bound(code, ctx, level, op)? {
                levels.pop();
            }
            return Ok(None);
        }
        0x10..=0x12 => {
            let a = usize::from(op - 0x10);
            Split {
                low: small(i64::from(byte(1)?)),
                high: small(i64::from(byte(2)?)),
                first: pc + 4,
                second: pc + 4 + byte(3)? as usize,
                axis: Some(a),
            }
        }
        0x13..=0x1c => {
            let (_, m, k) = diagonal_plane(op);
            Split {
                low: small(i64::from(byte(1)?) * m - k),
                high: small(i64::from(byte(2)?) * m - k),
                first: pc + 4,
                second: pc + 4 + byte(3)? as usize,
                axis: None,
            }
        }
        0x20..=0x22 => {
            let s = byte(1)?;
            Split {
                low: fpu.store(fpu.add(fpu.int(s), fpu.load(ctx.ray.constants.one)?)?)?,
                high: small(i64::from(s)),
                first: pc + 3,
                second: pc + 3 + byte(2)? as usize,
                axis: Some(usize::from(op - 0x20)),
            }
        }
        0x23..=0x25 => Split {
            low: small(i64::from(byte(1)?)),
            high: small(i64::from(byte(2)?)),
            first: pc + 7 + be(code, pc, 3, 2)? as usize,
            second: pc + 7 + be(code, pc, 5, 2)? as usize,
            axis: Some(usize::from(op - 0x23)),
        },
        _ => return Err(Bail::Data),
    };
    let child = two_sided(ctx, level, &split, op)?;
    let Some(child) = child else {
        return Ok(None);
    };
    if levels.len() >= MAX_FRAMES as usize {
        return Err(Bail::Depth);
    }
    let event = Event::Enter {
        pc: child.pc as u32,
        frame: child.frame,
        segment: child.segment,
    };
    levels.push(child);
    Ok(Some(event))
}

/// Rescale `01..04` (`0x003012da`): both ends lose the operand bytes and
/// gain `2^k`, and the level takes a new frame with the offsets added and
/// scaled the same way, `k` more bits and the frame scale times `2^k`.
fn rescale(fpu: &Fpu, level: &mut Level, k: u32, b: [u32; 3]) -> Result<(), Bail> {
    let bytes = [
        small(b[0].into()),
        small(b[1].into()),
        small(b[2].into()),
        0,
    ];
    let m = splat(small(1 << k));
    let seg = &mut level.segment;
    seg.start = fpu.sub4(seg.start, bytes)?;
    seg.end = fpu.sub4(seg.end, bytes)?;
    seg.start = fpu.mul4(m, seg.start)?;
    seg.end = fpu.mul4(m, seg.end)?;
    let f = level.frame;
    level.frame = Frame {
        offset: fpu.mul4(m, fpu.add4(f.offset, bytes)?)?,
        bits: f.bits.wrapping_add(k),
        scale: fpu.store(fpu.mul(fpu.int(1 << k), fpu.load(f.scale)?)?)?,
        key: f.key,
        property: f.property,
    };
    Ok(())
}

/// The split tests from `0x00300d50`. Takes one side by moving the level's
/// pc; for both sides returns the child the original calls first.
fn two_sided(ctx: &Ctx, level: &mut Level, s: &Split, op: u8) -> Result<Option<Level>, Bail> {
    let fpu = &ctx.fpu;
    let seg = level.segment;
    // The start's projection stays on the x87 stack; the end's is stored.
    let (start, end) = match s.axis {
        Some(a) => (fpu.load(seg.start[a])?, fpu.load(seg.end[a])?),
        None => (
            project(fpu, op, seg.start)?,
            fpu.load(fpu.store(project(fpu, op, seg.end)?)?)?,
        ),
    };
    let (low, high) = (fpu.load(s.low)?, fpu.load(s.high)?);
    if fpu.cmp(end, high) == Some(Ordering::Less) && fpu.cmp(start, high) == Some(Ordering::Less) {
        level.pc = s.first;
        return Ok(None);
    }
    if fpu.cmp(start, low) == Some(Ordering::Greater) && fpu.cmp(end, low) == Some(Ordering::Greater) {
        level.pc = s.second;
        return Ok(None);
    }
    let start_low = fpu.store(fpu.sub(start, low)?)?;
    let start_high = fpu.store(fpu.sub(start, high)?)?;
    let end_low = fpu.store(fpu.sub(end, low)?)?;
    let end_high = fpu.store(fpu.sub(end, high)?)?;
    let zero = fpu.load(ctx.ray.constants.zero)?;
    let rising = fpu.cmp(fpu.load(start_low)?, fpu.load(end_low)?) == Some(Ordering::Less);
    // The first child gets a copy with its end clipped at the threshold
    // on the far side of it.
    let (near_start, near_end) = if rising {
        (start_low, end_low)
    } else {
        (start_high, end_high)
    };
    let mut copy = seg;
    let (ns, ne) = (fpu.load(near_start)?, fpu.load(near_end)?);
    if fpu.cmp(fpu.mul(ne, ns)?, zero) == Some(Ordering::Less) {
        let w = fpu.store(fpu.div(ns, fpu.sub(ns, ne)?)?)?;
        copy.end = ctx.ray.blend(fpu, w, seg.start, seg.end)?;
    }
    let (resume, first) = if rising {
        (
            Resume::High {
                start: start_high,
                end: end_high,
                threshold: s.high,
                axis: s.axis,
                next: s.second,
            },
            s.first,
        )
    } else {
        (
            Resume::Low {
                start: start_low,
                end: end_low,
                threshold: s.low,
                axis: s.axis,
                next: s.first,
            },
            s.second,
        )
    };
    level.resume = Some(resume);
    Ok(Some(Level {
        pc: first,
        frame: level.frame,
        segment: copy,
        resume: None,
    }))
}

/// A bound `26..2b` (`0x0030114b`, `0x003010ed`): reject a segment wholly
/// outside `[lo, hi]` on the axis, else clip whichever ends lie outside.
/// `false` when the frame returns.
fn bound(code: &[u8], ctx: &Ctx, level: &mut Level, op: u8) -> Result<bool, Bail> {
    let fpu = &ctx.fpu;
    let pc = level.pc;
    let (a, lo, hi, len) = if op <= 0x28 {
        let a = usize::from(op - 0x26);
        (
            a,
            fpu.int(be(code, pc, 1, 1)?),
            fpu.int(be(code, pc, 2, 1)?),
            3,
        )
    } else {
        let a = usize::from(op - 0x29);
        let full = |v: u32| -> Result<St, Bail> {
            let v = fpu.mul(fpu.int(v), fpu.load(ctx.inverse_scale)?)?;
            let v = fpu.mul(v, fpu.load(level.frame.scale)?)?;
            fpu.sub(v, fpu.load(level.frame.offset[a])?)
        };
        (a, full(be(code, pc, 1, 3)?)?, full(be(code, pc, 4, 3)?)?, 7)
    };
    let seg = level.segment;
    let (s, e) = (fpu.load(seg.start[a])?, fpu.load(seg.end[a])?);
    let reverse = if fpu.cmp(s, e) == Some(Ordering::Less) {
        if fpu.cmp(e, lo) == Some(Ordering::Less) || fpu.cmp(s, hi) == Some(Ordering::Greater) {
            return Ok(false);
        }
        false
    } else {
        if fpu.cmp(s, lo) == Some(Ordering::Less) || fpu.cmp(e, hi) == Some(Ordering::Greater) {
            return Ok(false);
        }
        true
    };
    let zero = fpu.load(ctx.ray.constants.zero)?;
    // The start's distance is stored; the end's stays on the stack.
    // Both clips blend the endpoints as they were before either.
    for (t, far) in [(hi, true), (lo, false)] {
        let d0 = fpu.load(fpu.store(fpu.sub(s, t)?)?)?;
        let d1 = fpu.sub(e, t)?;
        if fpu.cmp(fpu.mul(d0, d1)?, zero) == Some(Ordering::Less) {
            let w = fpu.store(fpu.div(d0, fpu.sub(d0, d1)?)?)?;
            let v = ctx.ray.blend(fpu, w, seg.start, seg.end)?;
            // The high clip moves the end of a rising segment, the low
            // clip its start; a falling segment the other way round.
            if far != reverse {
                level.segment.end = v;
            } else {
                level.segment.start = v;
            }
        }
    }
    level.pc = pc + len;
    Ok(true)
}

#[cfg(test)]
mod tests;
