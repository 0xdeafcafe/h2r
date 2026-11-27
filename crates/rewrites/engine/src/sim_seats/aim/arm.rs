//! The aim-assist cone's shadow arm (docs/engine/sim-rewrite.md, "Seats
//! and rules (phase 7)"; the pattern of `sim_objects::functions::parity`):
//! at every call of `1a50a0` (`AIM_ASSIST_PARAMS`), the idiomatic kernel
//! `sim_objects::aim_assist` answers from a copy of every field the call
//! reads (the unit's hands, each weapon's tag fields and trigger states)
//! and the image's `pow` on the x87 model for the zoom.
//!
//! **Proof, then take-over.** Until [`PROVEN_AFTER`] calls have matched
//! the guest's with no mismatch, the guest answers every call and the
//! prediction made at the entry is compared at its return. After that the
//! kernel answers, and one answerable call in [`AUDIT_EVERY`] still goes
//! to the guest and is compared. A mismatch hands every call back to the
//! guest until the next map. A declined call is the guest's, counted by
//! reason; an audit whose return the runtime cannot watch is `dropped`.
//!
//! **What an answer leaves** is what retail leaves, but the arithmetic
//! flags (undefined), the x87 last-instruction pointers and the dead stack
//! below the return's `esp`: the record at `esi` (nothing on an unarmed
//! exit), `al`, the rest of `eax`, `ecx` (retail's `push ecx` slot holds
//! the first bend and is popped into it), `edx`, `xmm0`..`xmm3`, the x87
//! registers, tag and status after the zoom's `pow`, and `ret 4`. The
//! audit compares all of them.

use crate::function_eval::{self, constant::ONE};
use crate::light_constants::X87;
use crate::points;
use crate::sim_physics::ARITH;
use runtime::{CpuAbi, Guest};
use sim_core::det;
use sim_objects::aim_assist::{
    self, Answer, Hand, Input, Lane, Trigger, Unarmed, Weapon, WeaponAim, ZoomRange,
};
use std::sync::atomic::{AtomicU64, Ordering};
use x87::abi::arith_op;

/// The stack word `ret 4` pops, besides the return address.
pub const ARGS: u32 = 4;
/// Matched calls, with no mismatch, before the kernel answers.
pub const PROVEN_AFTER: u64 = 4096;
/// After that, one answerable call in this many is compared.
pub const AUDIT_EVERY: u64 = 64;
/// Triggers per weapon the capture copies; a definition with more is
/// declined.
pub const MAX_TRIGGERS: usize = 16;

/// Why a call was left to the guest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decline {
    /// MXCSR or the x87 not in the pinned state, or an operand off the
    /// exact range.
    Float,
    /// A weapon with more than [`MAX_TRIGGERS`] triggers.
    Triggers,
    /// A read of the call's inputs faulted.
    Fault,
}

impl Decline {
    pub const ALL: [Decline; 3] = [Decline::Float, Decline::Triggers, Decline::Fault];

    pub fn name(self) -> &'static str {
        match self {
            Decline::Float => "float",
            Decline::Triggers => "triggers",
            Decline::Fault => "fault",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// The arm's counters since the map loaded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub checked: u64,
    pub matched: u64,
    pub mismatched: u64,
    pub answered: u64,
    /// By [`Decline::ALL`] order.
    pub declined: [u64; 3],
    pub dropped: u64,
}

impl Status {
    pub fn declined_total(&self) -> u64 {
        self.declined.iter().sum()
    }
}

static CHECKED: AtomicU64 = AtomicU64::new(0);
static MATCHED: AtomicU64 = AtomicU64::new(0);
static MISMATCHED: AtomicU64 = AtomicU64::new(0);
static ANSWERED: AtomicU64 = AtomicU64::new(0);
static ANSWERABLE: AtomicU64 = AtomicU64::new(0);
static DROPPED: AtomicU64 = AtomicU64::new(0);
static DECLINED: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];

pub fn status() -> Status {
    let read = |c: &AtomicU64| c.load(Ordering::Relaxed);
    Status {
        checked: read(&CHECKED),
        matched: read(&MATCHED),
        mismatched: read(&MISMATCHED),
        answered: read(&ANSWERED),
        declined: std::array::from_fn(|i| read(&DECLINED[i])),
        dropped: read(&DROPPED),
    }
}

/// Forget the map's proof and counters. Called at map load.
pub fn reset() {
    for c in [
        &CHECKED,
        &MATCHED,
        &MISMATCHED,
        &ANSWERED,
        &ANSWERABLE,
        &DROPPED,
    ] {
        c.store(0, Ordering::Relaxed);
    }
    for c in &DECLINED {
        c.store(0, Ordering::Relaxed);
    }
}

/// An audit the runtime could not watch to its return.
pub fn dropped() {
    DROPPED.fetch_add(1, Ordering::Relaxed);
}

fn proven() -> bool {
    MISMATCHED.load(Ordering::Relaxed) == 0 && MATCHED.load(Ordering::Relaxed) >= PROVEN_AFTER
}

/// One call's prediction: the registers at the return and the record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Prediction {
    /// The CPU as the return leaves it (`eip` aside).
    pub cpu: CpuAbi,
    /// The record's address and words, on an armed call.
    pub record: Option<(u32, [u32; 9])>,
}

impl Prediction {
    /// The first thing `cpu` and `record` (the nine words now at the
    /// predicted record's address) differ from the prediction in.
    pub fn differs(&self, cpu: &CpuAbi, record: Option<[u32; 9]>) -> Option<&'static str> {
        let p = &self.cpu;
        let gprs = [
            ("eax", p.eax, cpu.eax),
            ("ecx", p.ecx, cpu.ecx),
            ("edx", p.edx, cpu.edx),
            ("ebx", p.ebx, cpu.ebx),
            ("esp", p.esp, cpu.esp),
            ("ebp", p.ebp, cpu.ebp),
            ("esi", p.esi, cpu.esi),
            ("edi", p.edi, cpu.edi),
        ];
        if let Some((name, ..)) = gprs.iter().find(|(_, a, b)| a != b) {
            return Some(name);
        }
        const XMM: [&str; 4] = ["xmm0", "xmm1", "xmm2", "xmm3"];
        if let Some(i) = (0..4).find(|&i| p.xmm[i] != cpu.xmm[i]) {
            return Some(XMM[i]);
        }
        if p.x87.st != cpu.x87.st || p.x87.tag != cpu.x87.tag || p.x87.status != cpu.x87.status {
            return Some("x87");
        }
        match (self.record, record) {
            (Some((_, want)), Some(got)) if want != got => Some("record"),
            (Some(_), None) => Some("record"),
            _ => None,
        }
    }
}

/// What [`enter`] did with a call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// The kernel answered: `cpu` and the record hold the result; the
    /// caller returns to `target`.
    Answered { target: u32 },
    /// The guest answers; compare at its return ([`compare`]).
    Audit(Prediction),
    /// The guest answers alone: declined, counted.
    Guest,
}

/// At the entry: predict the call and decide who answers.
pub fn enter(cpu: &mut CpuAbi, guest: &mut Guest) -> Step {
    let prediction = match predict(cpu, guest) {
        Ok(p) => p,
        Err(why) => {
            DECLINED[why.index()].fetch_add(1, Ordering::Relaxed);
            return Step::Guest;
        }
    };
    let slot = ANSWERABLE.fetch_add(1, Ordering::Relaxed);
    if !proven() || slot % AUDIT_EVERY == 0 {
        return Step::Audit(prediction);
    }
    match apply(&prediction, cpu, guest) {
        Ok(target) => {
            ANSWERED.fetch_add(1, Ordering::Relaxed);
            Step::Answered { target }
        }
        Err(_) => {
            DECLINED[Decline::Fault.index()].fetch_add(1, Ordering::Relaxed);
            Step::Guest
        }
    }
}

/// Put `prediction` in place of the call: the record, the registers and
/// the popped frame. Returns the return address. Writes nothing when the
/// return address or the record does not take the write.
pub fn apply(
    prediction: &Prediction,
    cpu: &mut CpuAbi,
    guest: &mut Guest,
) -> Result<u32, runtime::GuestFault> {
    let target = guest.u32(cpu.esp)?;
    if let Some((out, words)) = prediction.record {
        // Check every word first, so a fault leaves the record whole.
        for i in 0..9u32 {
            guest.u32(out.wrapping_add(4 * i))?;
        }
        for (i, w) in words.iter().enumerate() {
            guest.set_u32(out.wrapping_add(4 * i as u32), *w)?;
        }
    }
    let eip = cpu.eip;
    *cpu = prediction.cpu;
    cpu.eip = eip;
    Ok(target)
}

/// At the guest's return: compare with the prediction. The guest's answer
/// stands either way.
pub fn compare(prediction: &Prediction, cpu: &CpuAbi, guest: &Guest) {
    CHECKED.fetch_add(1, Ordering::Relaxed);
    let record = prediction.record.and_then(|(out, _)| {
        let mut w = [0u32; 9];
        for (i, slot) in w.iter_mut().enumerate() {
            *slot = guest.u32(out.wrapping_add(4 * i as u32)).ok()?;
        }
        Some(w)
    });
    match prediction.differs(cpu, record) {
        None => {
            MATCHED.fetch_add(1, Ordering::Relaxed);
        }
        Some(what) => {
            if MISMATCHED.fetch_add(1, Ordering::Relaxed) == 0 {
                eprintln!(
                    "sim: seats aim-assist arm: the guest's {what} differs from the prediction; \
                     the guest answers every call until the next map"
                );
            }
        }
    }
}

/// The arm's whole answer for the call `cpu` is entering, without
/// counting or choosing: the kernel over a copy of the call's inputs.
pub fn predict(cpu: &CpuAbi, g: &Guest) -> Result<Prediction, Decline> {
    if !det::fast_env(cpu.mxcsr)
        || !det::host_fast_env()
        || !det::x87_env(cpu.x87.control, cpu.x87.status, cpu.x87.tag, 8)
    {
        return Err(Decline::Float);
    }
    let word = |a: u32| g.u32(a).map_err(|_| Decline::Fault);
    let one = word(ONE)?;
    let zoom = word(cpu.esp.wrapping_add(4))? as u16 as i16;
    let header = word(points::OBJECTS.base)?;
    let data = word(header.wrapping_add(0x44))?;
    let tags = word(points::TAG_ENTRIES.base)?;
    let mut triggers = [Trigger::default(); MAX_TRIGGERS];
    let mut raw = [Raw::default(); 2];
    let hands = if cpu.ecx == u32::MAX {
        None
    } else {
        let unit = word(data.wrapping_add((cpu.ecx & 0xffff) * 12 + 8))?;
        raw[0] = read_hand(g, data, tags, unit, 0x212, Some(&mut triggers))?;
        // The guest reads the second hand only after the first arms the
        // cone; so does the capture.
        if raw[0].weapon.is_some() {
            raw[1] = read_hand(g, data, tags, unit, 0x213, None)?;
        }
        Some([raw[0].hand(&triggers), raw[1].hand(&[])])
    };
    let input = Input { one, zoom, hands };
    let mut x87 = X87(to_x87(&cpu.x87));
    let answer = aim_assist::cone(&input, &mut |max, min, t| magnify(g, &mut x87, max, min, t));
    let mut out = *cpu;
    out.esp = cpu.esp.wrapping_add(4 + ARGS);
    out.eflags_defined &= !ARITH;
    let low = cpu.eax & !0xff;
    let record = match answer {
        Answer::Inexact => return Err(Decline::Float),
        Answer::Unarmed(why) => {
            out.eax = low;
            match why {
                Unarmed::NoUnit => {}
                Unarmed::NoSlot => out.edx = (cpu.edx & 0xffff_0000) | 0xffff,
                Unarmed::Empty { index } => out.edx = i32::from(index) as u32,
                Unarmed::Unzoomed => out.edx = raw[0].flags >> 5,
            }
            None
        }
        Answer::Armed { cone, lanes } => {
            // `push ecx` was the first bend's slot.
            out.ecx = lanes_bend1(&lanes, &raw, cone.bend);
            let (eax, edx) = match raw[1].weapon {
                Some(_) => (raw[1].aim.cone_range, raw[1].aim.cone_angle),
                None if raw[1].slot => (u32::MAX, header),
                None => ((data & 0xffff_0000) | 0xffff, header),
            };
            out.eax = (eax & !0xff) | 1;
            out.edx = edx;
            for (i, lane) in lanes.0.iter().enumerate() {
                out.xmm[i] = match *lane {
                    Lane::Kept => cpu.xmm[i],
                    Lane::Low(v) => {
                        let mut x = cpu.xmm[i];
                        x[..4].copy_from_slice(&v.to_le_bytes());
                        x
                    }
                    Lane::Whole(v) => u128::from(v).to_le_bytes(),
                };
            }
            out.x87 = from_x87(&x87.0, &cpu.x87);
            Some((cpu.esi, cone.words()))
        }
    };
    Ok(Prediction { cpu: out, record })
}

/// The first bend: `xmm1` holds it with two hands; with one, it is the
/// record's bend.
fn lanes_bend1(lanes: &aim_assist::Lanes, raw: &[Raw; 2], bend: u32) -> u32 {
    match (raw[1].weapon, lanes.0[1]) {
        (Some(_), Lane::Whole(b)) => b,
        _ => bend,
    }
}

/// One weapon slot as the guest reads it.
#[derive(Clone, Copy, Debug, Default)]
struct Raw {
    /// The slot byte names an index.
    slot: bool,
    index: i16,
    /// The weapon handle, when it is not `-1`.
    weapon: Option<u32>,
    aim: WeaponAim,
    /// The definition's flags word `+0x12c`.
    flags: u32,
    triggers: usize,
}

impl Raw {
    fn hand<'a>(&self, triggers: &'a [Trigger]) -> Hand<'a> {
        match (self.slot, self.weapon) {
            (false, _) => Hand::NoSlot,
            (true, None) => Hand::Empty { index: self.index },
            (true, Some(_)) => Hand::Held(Weapon {
                aim: self.aim,
                triggers: &triggers[..self.triggers.min(triggers.len())],
            }),
        }
    }
}

/// The slot byte at `unit + slot`, its weapon and that weapon's
/// definition; with `triggers`, the weapon object's trigger states too.
fn read_hand(
    g: &Guest,
    data: u32,
    tags: u32,
    unit: u32,
    slot: u32,
    triggers: Option<&mut [Trigger; MAX_TRIGGERS]>,
) -> Result<Raw, Decline> {
    let word = |a: u32| g.u32(a).map_err(|_| Decline::Fault);
    let half = |a: u32| g.u16(a).map_err(|_| Decline::Fault);
    let byte = |a: u32| g.u8(a).map_err(|_| Decline::Fault);
    let index = i16::from(byte(unit.wrapping_add(slot))? as i8);
    if index == -1 {
        return Ok(Raw::default());
    }
    let handle = word(
        unit.wrapping_add(0x218)
            .wrapping_add((i32::from(index) as u32).wrapping_mul(4)),
    )?;
    if handle == u32::MAX {
        return Ok(Raw {
            slot: true,
            index,
            ..Raw::default()
        });
    }
    let object = word(data.wrapping_add((handle & 0xffff) * 12 + 8))?;
    let tag = word(object)? & 0xffff;
    let def = word(tags.wrapping_add(tag * 16 + 8))?;
    let at = |o: u32| def.wrapping_add(o);
    let flags = word(at(0x12c))?;
    let aim = WeaponAim {
        cone_angle: word(at(0x208))?,
        cone_range: word(at(0x20c))?,
        pull_angle: word(at(0x210))?,
        pull_range: word(at(0x214))?,
        bend_angle: word(at(0x218))?,
        class: half(at(0x294))?,
        needs_zoom: (flags >> 5) & 1 != 0,
        zoom: ZoomRange {
            levels: half(at(0x1fe))? as i16,
            min: word(at(0x200))?,
            max: word(at(0x204))?,
        },
    };
    let mut count = 0;
    if let Some(out) = triggers {
        let n = word(at(0x2c8))? as i32;
        if n > MAX_TRIGGERS as i32 {
            return Err(Decline::Triggers);
        }
        count = n.max(0) as usize;
        for (i, t) in out.iter_mut().enumerate().take(count) {
            let i = i as u32;
            let state = byte(object.wrapping_add(0x20c + 12 * i))?;
            let kind = if state == 0 {
                Some(half(word(at(0x2cc))?.wrapping_add(0x40 * i + 6))?)
            } else {
                None
            };
            *t = Trigger { state, kind };
        }
    }
    Ok(Raw {
        slot: true,
        index,
        weapon: Some(handle),
        aim,
        flags,
        triggers: count,
    })
}

/// `101090`'s x87 half on the model, carrying its state from call to call:
/// `fld max; fdiv min; fld t; call pow; fmul min; fstp m32`.
fn magnify(g: &Guest, x: &mut X87, max: u32, min: u32, t: u32) -> Option<u32> {
    x.fld(max).ok()?;
    x.arith_m(arith_op::DIV, min).ok()?;
    x.fld(t).ok()?;
    function_eval::pow(g, x).ok()?;
    x.fmul_m(min).ok()?;
    x.fstp_m32().ok()
}

fn to_x87(r: &runtime::cpu::X87Abi) -> x87::abi::X87Abi {
    x87::abi::X87Abi {
        st: r.st,
        control: r.control,
        status: r.status,
        tag: r.tag,
        opcode: r.opcode,
        instruction_pointer: r.instruction_pointer,
        data_pointer: r.data_pointer,
        instruction_selector: r.instruction_selector,
        data_selector: r.data_selector,
    }
}

/// The model's registers, tag, status and control over `entry`'s
/// last-instruction pointers, which the answer does not model.
fn from_x87(x: &x87::abi::X87Abi, entry: &runtime::cpu::X87Abi) -> runtime::cpu::X87Abi {
    runtime::cpu::X87Abi {
        st: x.st,
        control: x.control,
        status: x.status,
        tag: x.tag,
        ..*entry
    }
}

#[cfg(test)]
mod tests;
