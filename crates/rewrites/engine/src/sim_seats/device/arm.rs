//! The bound controller's device read (docs/engine/sim-rewrite.md, "Seats
//! and rules (phase 7)"). At `185be0`'s bound branch (`0x185c6c`, a seat
//! whose port has a controller), the seat body copies the read's inputs
//! out of guest memory into a [`Device`], runs the kernel [`super::read_guest`]
//! with the x87 model and the image's `pow` for the x87 pieces, and lays
//! the outcome out as the state at the next transfer: the look curve's
//! call (`0x185ff4`) unblocked, the visibility walk's (`0x1865bf`)
//! blocked, or the walk skipped for a unit that is not a biped.
//!
//! Every read is the kernel's, under the guest's float state as it stands:
//! the MXCSR through the SSE model ([`super::read_guest`]), the x87
//! control word (precision, rounding, masks) through the x87 model. A read
//! it cannot take (an unmasked exception, an input that faults, spans
//! that alias) is
//! `unimplemented!("seats: device read: <cause>")`: the guest's read is
//! gone.

use super::{Constants, Device, Fpu, Last, Path, Seat, read_guest};
use crate::light_constants::X87;
use guest_layout::memory::{GuestMemory, MemFault};
use crate::sim_native::Memory;
use crate::sim_physics::{
    logic,
    spatial::{finish_fpu, load_fpu},
};
use runtime::CpuAbi;
use sim_core::det;
use x87::abi::{arith_op, unary_op};

/// The words the prediction holds of the frame: `[sp - 8, sp + 0xf0)`,
/// the curve's return and argument, the saved registers and the locals.
pub const FRAME_WORDS: usize = 62;
/// The record's words the read writes (`+0`..`+0x24`).
pub const RECORD_WORDS: usize = 10;

pub const CURVE_SITE: u32 = 0x0018_5ff9;
pub const VISIBLE_SITE: u32 = 0x0018_65c4;

const ONE: u32 = 0x0045_dbc0;
const ZERO: u32 = 0x0045_dbd8;
const LOW: u32 = 0x0045_dbcc;
const SCALE: u32 = 0x0045_dc54;
const DEAD: u32 = 0x0045_dc68;
const RATES: u32 = 0x0051_ea18;
const BLOCKS: u32 = 0x0051_ea88;
const SHARED_INPUT: u32 = 0x004e_61b9;
const INPUTS: u32 = 0x004e_61dc;
const CONTROL: u32 = 0x004e_d284;
const TICKS: u32 = 0x0051_0c54;
const OBJECT_HEADER: u32 = 0x004e_0300;
const TAG_ENTRIES: u32 = guest_layout::points::TAG_ENTRIES.base;
/// The x87 status word's error summary.
const ES: u16 = 0x80;

/// Why a read is not the kernel's: each is a named panic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decline {
    /// An unmasked SSE or x87 exception, a pending x87 exception, or a step
    /// the x87 model refuses.
    Float,
    /// A read of the inputs faulted.
    Fault,
    /// The record, the input slot or the buttons overlap the frame or
    /// each other, so the stores' order would matter.
    Alias,
}

impl Decline {
    pub fn name(self) -> &'static str {
        match self {
            Decline::Float => "float",
            Decline::Fault => "fault",
            Decline::Alias => "alias",
        }
    }
}

/// The next transfer the read leads to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Site {
    /// `call 185b30` at `0x185ff4`.
    Curve,
    /// `call e4050` at `0x1865bf`.
    Visible,
    /// Blocked with no visibility walk: the body goes on to `0x1865cc`.
    Unseen,
}

/// One read's prediction: the state at its transfer, and the state the
/// read started from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Prediction {
    pub site: Site,
    /// The frame's `esp` (after the four saved registers).
    pub sp: u32,
    /// The registers: at the curve's entry, or at `0x186593` before the
    /// visibility walk.
    pub cpu: CpuAbi,
    /// `[sp - 8, sp + 0xf0)` before the read and at the transfer.
    pub before: [u32; FRAME_WORDS],
    pub frame: [u32; FRAME_WORDS],
    /// The record (`[sp + 0x104]`) and its words before and after.
    pub record_at: u32,
    pub record_before: [u32; RECORD_WORDS],
    pub record: [u32; RECORD_WORDS],
    /// The input slot (`[sp + 0x100]`) and the input it is given.
    pub slot: u32,
    pub input: u32,
    /// The seat's wanted buttons (`ebp + 0xc`, `+0xe`) after the read.
    pub buttons: [u16; 2],
}

impl Prediction {
    /// The frame word at `sp + offset`.
    fn word(frame: &[u32; FRAME_WORDS], offset: i32) -> u32 {
        frame[((offset + 8) / 4) as usize]
    }

    /// Frame words the read changes: `(sp + offset, value)`, from
    /// `sp + from`.
    pub fn frame_stores(&self, from: i32) -> impl Iterator<Item = (u32, u32)> + '_ {
        (0..FRAME_WORDS).filter_map(move |i| {
            let offset = i as i32 * 4 - 8;
            (offset >= from && self.frame[i] != self.before[i])
                .then(|| (self.sp.wrapping_add(offset as u32), self.frame[i]))
        })
    }

    /// Record words the read changes.
    pub fn record_stores(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        (0..RECORD_WORDS)
            .filter(move |&i| self.record[i] != self.record_before[i])
            .map(move |i| (self.record_at.wrapping_add(4 * i as u32), self.record[i]))
    }
}

/// At the bound branch: the kernel's answer for the body to lay down.
pub fn enter(c: &CpuAbi, m: &impl Memory, sp: u32, port: u32) -> Prediction {
    predict(c, m, sp, port).unwrap_or_else(|why| unimplemented!("seats: device read: {}", why.name()))
}

/// `pow`'s constant reads through the segment's memory.
struct Reads<'a, M: Memory>(&'a M);

impl<M: Memory> GuestMemory for Reads<'_, M> {
    fn read(&self, addr: u32, buf: &mut [u8]) -> Result<(), MemFault> {
        let len = buf.len() as u32;
        self.0.read_bytes(addr, buf, 1).map_err(|_| MemFault {
            addr,
            len,
            write: false,
        })
    }

    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), MemFault> {
        Err(MemFault {
            addr,
            len: data.len() as u32,
            write: true,
        })
    }
}

/// The kernel's x87 pieces on the model, carrying its state.
struct Model<'a, M: Memory> {
    x: &'a mut X87,
    g: Reads<'a, M>,
}

impl<M: Memory> Fpu for Model<'_, M> {
    fn speed(&mut self, v: [u32; 3], lo: u32, span: u32, zero: u32) -> Option<(u32, bool)> {
        let x = &mut *self.x;
        // 0x24590: z, y, x; x*x + y*y + z*z; the root replaces z.
        for c in [v[2], v[1], v[0]] {
            x.fld(c).ok()?;
        }
        x.fld_st(0).ok()?;
        x.arith_reg(arith_op::MUL, 0, 1, false).ok()?;
        x.fld_st(2).ok()?;
        x.arith_reg(arith_op::MUL, 0, 3, false).ok()?;
        x.arith_reg(arith_op::ADD, 1, 0, true).ok()?;
        x.fld_st(3).ok()?;
        x.arith_reg(arith_op::MUL, 0, 4, false).ok()?;
        x.arith_reg(arith_op::ADD, 1, 0, true).ok()?;
        x.unary(unary_op::SQRT).ok()?;
        x.fstp_st(3).ok()?;
        x.fstp_st(0).ok()?;
        x.fstp_st(0).ok()?;
        x.arith_m(arith_op::SUB, lo).ok()?;
        x.arith_m(arith_op::DIV, span).ok()?;
        let ratio = x.fst_m32().ok()?;
        x.fld(zero).ok()?;
        let flags = x.fcomip(1).ok()?;
        x.fstp_st(0).ok()?;
        Some((ratio, flags & 0x41 == 0))
    }

    fn pow(&mut self, t: u32, e: u32) -> Option<u32> {
        self.x.fld(t).ok()?;
        self.x.fld(e).ok()?;
        crate::function_eval::pow(&self.g, self.x).ok()?;
        self.x.fstp_m32().ok()
    }

    fn abs(&mut self, v: u32) -> Option<u32> {
        self.x.fld(v).ok()?;
        self.x.unary(unary_op::ABS).ok()?;
        self.x.fstp_m32().ok()
    }

    fn above(&mut self, a: u32, dead: u32) -> Option<bool> {
        self.x.fld(dead).ok()?;
        self.x.fld(a).ok()?;
        let flags = self.x.fcomip(1).ok()?;
        self.x.fstp_st(0).ok()?;
        Some(flags & 0x41 == 0)
    }

    fn hypot(&mut self, q: u32, p: u32) -> Option<u32> {
        let x = &mut *self.x;
        x.fld(q).ok()?;
        x.fmul_m(q).ok()?;
        x.fld(p).ok()?;
        x.fmul_m(p).ok()?;
        x.arith_reg(arith_op::ADD, 1, 0, true).ok()?;
        x.unary(unary_op::SQRT).ok()?;
        x.fstp_m32().ok()
    }
}

fn whole(v: u32) -> [u8; 16] {
    u128::from(v).to_le_bytes()
}

/// The read's whole answer from `c` at `0x185c6c` (after the entry's
/// frame and zeroing, `port` the controller's), without counting or
/// choosing.
pub fn predict(c: &CpuAbi, m: &impl Memory, sp: u32, port: u32) -> Result<Prediction, Decline> {
    // The host's own environment backs the SSE fast path; a pending x87
    // exception is `#MF` at the read's first waiting instruction.
    if !det::host_fast_env() || c.x87.status & ES != 0 {
        return Err(Decline::Float);
    }
    let word = |a: u32| m.u32(a).map_err(|_| Decline::Fault);
    let half = |a: u32| m.u16(a).map_err(|_| Decline::Fault);
    let byte = |a: u32| m.u8(a).map_err(|_| Decline::Fault);
    let at = |o: i32| sp.wrapping_add(o as u32);

    let before: [u32; FRAME_WORDS] = {
        let mut f = [0; FRAME_WORDS];
        for (i, w) in f.iter_mut().enumerate() {
            *w = word(at(i as i32 * 4 - 8))?;
        }
        f
    };
    let record_at = word(at(0x104))?;
    let record_before: [u32; RECORD_WORDS] = {
        let mut r = [0; RECORD_WORDS];
        for (i, w) in r.iter_mut().enumerate() {
            *w = word(record_at.wrapping_add(4 * i as u32))?;
        }
        r
    };
    let slot = word(at(0x100))?;
    // The frame, the record, the slot and the buttons, as byte spans.
    let spans = [
        (at(-8), FRAME_WORDS as u32 * 4),
        (record_at, RECORD_WORDS as u32 * 4),
        (slot, 4),
        (c.ebp.wrapping_add(0xc), 4),
    ];
    let overlap = |(a, n): (u32, u32), (b, k): (u32, u32)| {
        let (a, b) = (u64::from(a), u64::from(b));
        a < b + u64::from(k) && b < a + u64::from(n)
    };
    if (0..4).any(|i| (i + 1..4).any(|j| overlap(spans[i], spans[j]))) {
        return Err(Decline::Alias);
    }
    let input = if byte(SHARED_INPUT)? != 0 {
        0x004e_630c
    } else {
        INPUTS.wrapping_add(port.wrapping_mul(0x48))
    };
    let k = Constants {
        one: word(ONE)?,
        zero: word(ZERO)?,
        scale: word(SCALE)?,
        dead: word(DEAD)?,
        low: word(LOW)?,
    };
    let rate_at = RATES.wrapping_add(c.edx.wrapping_mul(0x1c));
    let block_at = BLOCKS.wrapping_add(c.edx.wrapping_mul(0x50));
    let mut block = [0; 20];
    for (i, w) in block.iter_mut().enumerate() {
        *w = word(block_at.wrapping_add(4 * i as u32))?;
    }
    // The unit's vehicle seat: the unit's parent and its seat index, the
    // parent's definition's seat block.
    let unit = word(c.ebp)?;
    let mut seat = None;
    let mut esi = block_at.wrapping_add(0x50);
    if unit != u32::MAX {
        let entries = word(word(OBJECT_HEADER)?.wrapping_add(0x44))?;
        let object = |h: u32| word(entries.wrapping_add((h & 0xffff) * 12 + 8));
        let body = object(unit)?;
        let parent = word(body.wrapping_add(0x14))?;
        let index = half(body.wrapping_add(0x1fc))?;
        if parent != u32::MAX && index != u16::MAX {
            let vehicle = object(parent)?;
            let tag = word(vehicle)? & 0xffff;
            let def = word(word(TAG_ENTRIES)?.wrapping_add(tag * 16 + 8))?;
            let seats = word(def.wrapping_add(0x1cc))?;
            esi = seats.wrapping_add((i32::from(index as i16) as u32).wrapping_mul(0xb0));
            let f = |o: u32| word(esi.wrapping_add(o));
            // The velocity is read only when the range has a span.
            let (lo, hi) = (f(0x54)?, f(0x58)?);
            let velocity = if f32::from_bits(hi) - f32::from_bits(lo) > 0.0 {
                let v = |o: u32| word(vehicle.wrapping_add(0x88 + o));
                [v(0)?, v(4)?, v(8)?]
            } else {
                [0; 3]
            };
            seat = Some(Seat {
                velocity,
                lo,
                hi,
                exponent: f(0x5c)?,
                yaw: [f(0x44)?, f(0x48)?],
                pitch: [f(0x4c)?, f(0x50)?],
            });
        }
    }
    let control = word(CONTROL)?;
    let mut ticks = None;
    let blocked = byte(control.wrapping_add(0x10))? & 1 != 0 || {
        let t = word(TICKS)?;
        ticks = Some(t);
        byte(t)? != 0 && byte(t.wrapping_add(1))? != 0
    };
    let wanted = if blocked {
        [
            half(c.ebp.wrapping_add(0xc))?,
            half(c.ebp.wrapping_add(0xe))?,
        ]
    } else {
        [0; 2]
    };
    let device = Device {
        rates: [word(rate_at)?, word(rate_at.wrapping_add(4))?],
        block,
        seat,
        blocked,
        wanted,
        k,
    };

    let mut x = load_fpu(c);
    let (outcome, mxcsr) = read_guest(
        &device,
        c.mxcsr,
        &mut Model {
            x: &mut x,
            g: Reads(m),
        },
    )
    .ok_or(Decline::Float)?;
    // An unmasked x87 pre-computation exception leaves only the status
    // word changed (`x87::abi`): the guest takes `#MF` at its next waiting
    // instruction.
    if x.0.status & ES != 0 {
        return Err(Decline::Float);
    }

    let mut frame = before;
    let mut put = |o: i32, v: u32| frame[((o + 8) / 4) as usize] = v;
    for o in [0x20, 0x24, 0x28, 0x2c] {
        put(o, 0);
    }
    for o in (0x4c..=0x68).step_by(4) {
        put(o, 0);
    }
    put(0x30, outcome.rates[0]);
    put(0x34, outcome.rates[1]);
    for (i, w) in block.iter().enumerate() {
        put(0x6c + 4 * i as i32, *w);
    }
    let mut out = *c;
    out.mxcsr = mxcsr;
    out.xmm[0] = whole(block[15]);
    out.xmm[1] = whole(k.scale);
    out.xmm[4] = whole(k.one);
    out.xmm[6] = whole(0);
    out.xmm[7] = whole(outcome.rates[0]);
    out.esi = esi;
    let mut finish = None;
    if let Some(v) = outcome.vehicle {
        put(0x14, v.lo);
        put(0x18, v.span);
        put(0x10, v.t);
        out.xmm[1] = whole(v.t);
        finish = match v.last {
            Last::None => None,
            Last::Compare => Some((0x0018_5dce, ZERO)),
            Last::Pow => Some((0x0018_5e07, at(0x10))),
        };
    }
    let mut record = record_before;
    record[0] = block[14];
    record[1] = block[15];
    let mut buttons = [
        half(c.ebp.wrapping_add(0xc))?,
        half(c.ebp.wrapping_add(0xe))?,
    ];
    let site = match outcome.path {
        Path::Steer(s) => {
            put(0x10, s.axes[0]);
            put(0x14, s.axes[1]);
            if let Some(h) = s.hypot {
                put(0x18, h);
            }
            put(-4, s.axes[1]);
            put(-8, CURVE_SITE);
            let settings = Prediction::word(&before, 0x44);
            out.edi = settings;
            out.esi = word(settings.wrapping_add(0x78))?;
            out.ebx = u32::from(half(settings.wrapping_add(0x74))?);
            out.eax = out.ebx;
            out.edx = s.axes[1];
            out.ecx = ticks.ok_or(Decline::Fault)?;
            out.esp = sp.wrapping_sub(8);
            out.xmm[0] = whole(s.scaled[1]);
            out.xmm[1] = whole(s.scaled[0]);
            out.xmm[2] = whole(k.low);
            logic(&mut out, 0, 1 << 31);
            finish = Some(match s.hypot {
                Some(_) => (0x0018_5f78, at(0x18)),
                None => (0x0018_5eef, at(0x10)),
            });
            Site::Curve
        }
        Path::Blocked(s) => {
            for i in [4, 5, 9] {
                record[i] = 0;
            }
            if let Some(mask) = s.walked {
                put(0x10, 0x10);
                put(0x14, u32::from(mask));
                out.ebx = 0x13;
                out.esi = 0x12;
            }
            for i in 0..4 {
                let b = &s.states[4 * i..4 * i + 4];
                put(
                    0x20 + 4 * i as i32,
                    u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                );
            }
            for i in 0..8 {
                let w = u32::from(s.holds[2 * i]) | u32::from(s.holds[2 * i + 1]) << 16;
                put(0x4c + 4 * i as i32, w);
            }
            buttons = s.wanted;
            out.edi = record_at;
            out.eax = u32::from(s.wanted[0]);
            out.ecx = 0x10;
            out.edx = if s.wanted[0] & 0x8000 != 0 {
                0x8000
            } else {
                u32::from(s.holds[15])
            };
            out.esp = sp;
            let visible = unit != u32::MAX && {
                let entries = word(word(OBJECT_HEADER)?.wrapping_add(0x44))?;
                // `shl ebx, cl; test bl, 1`: the shift counts mod 32.
                byte(entries.wrapping_add((unit & 0xffff) * 12 + 3))? & 0x1f == 0
            };
            if visible { Site::Visible } else { Site::Unseen }
        }
    };
    if let Some((pc, operand)) = finish {
        finish_fpu(&mut out, m, x, pc, operand).map_err(|_| Decline::Fault)?;
    }
    Ok(Prediction {
        site,
        sp,
        cpu: out,
        before,
        frame,
        record_at,
        record_before,
        record,
        slot,
        input,
        buttons,
    })
}
