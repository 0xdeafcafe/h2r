//! The first-person filter step `11eed0` as one Rust call
//! (docs/engine/first-person-filters.md; phase 1c of
//! docs/engine/sim-rewrite.md).
//!
//! `11eed0` moves one first-person axis — a position and its velocity —
//! toward a target in one tick: it snaps when the target is within a
//! tick's reach, and otherwise accelerates toward it under a speed and
//! an acceleration bound, wrapping across `[lo, hi]` when asked and
//! clamping into it. The first-person filter port
//! (`sim_physics::first_person::filters`, `166d75`) calls it four times
//! a seat, and its weapon-slot step once more; at each of those calls
//! [`run`] does the call's work in Rust and the port carries on at the
//! return site, with no guest transfer.
//!
//! The call's ABI: `esi` the velocity, `edi` the position, the speed
//! bound in `xmm5`, the wrap flag in `dl`, and five stack arguments —
//! the target, the tick length, the acceleration bound, `lo` and `hi`;
//! `al` answers whether it snapped, and `ret 14h` pops the arguments.
//!
//! The body is SSE and x87 mixed, and both are run on the `simd` and
//! `x87` crates — the same arithmetic the lifted code does — so every
//! result, every MXCSR and x87 status bit, the x87 registers it pops
//! and the last-instruction pointers it leaves are the guest's in every
//! rounding mode and precision. What a call leaves is exactly the
//! guest's: the velocity and position; the return address, the three
//! locals and the two argument slots it rewrites on the stack; `eax`
//! and `cl`, `xmm0` to `xmm4`, `xmm6` and `xmm7`; and the flags of its
//! closing `add esp, 0Ch`.
//!
//! It declines before writing anything, and the caller panics with
//! `anim filter: <reason>` (no guest call runs) when a read faults, when an emulated instruction would deliver an
//! unmasked exception, or when stack or output stores alias the inputs
//! used to plan the call or its instructions.
use crate::light_constants::X87;
use crate::sim_native::Memory;
use crate::sim_native::spans::Spans;
use runtime::{CpuAbi, GuestFault};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use x87::abi::{arith_op, unary_op};

/// The filter step.
pub const FILTER: u32 = 0x0011_eed0;
/// First byte following the final return.
pub(crate) const FILTER_END: u32 = 0x0011_f0c2;
/// `0.5` and `2.0` in the image.
const HALF: u32 = 0x0045_dbbc;
const TWO: u32 = 0x0045_dc0c;
/// The last x87 instruction of each path (`fstp st(0)`), and the offset
/// from the body's `esp` of the last x87 memory operand before it.
const SNAP_LAST: u32 = 0x0011_ef8f;
const SNAP_OPERAND: u32 = 4;
const MOVE_LAST: u32 = 0x0011_f01c;
const MOVE_OPERAND: u32 = 0x18;
/// The body's stack window, in dwords from its `esp` (the call's `esp`
/// less 16): three locals, the return address, five arguments.
const WINDOW: usize = 9;
/// The window's slots.
const VELOCITY: usize = 0;
const REACH: usize = 1;
const SPEED_SQUARED: usize = 2;
const RETURN: usize = 3;
const TARGET: usize = 4;
const TICK: usize = 5;
const ACCEL: usize = 6;
const LO: usize = 7;
const HI: usize = 8;
/// Bytes the arguments take, popped by `ret 14h`.
const ARGS: u32 = 0x14;

static ENABLED: AtomicBool = AtomicBool::new(true);

/// On by default: the kernel runs wherever the first-person filter port runs. Off
/// leaves the register-level path, the tests' reference.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

#[cfg(any(test, feature = "differential"))]
thread_local! {
    static OVERRIDE: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Run `f` with the kernel forced on or off on this thread only, so a
/// differential test can hold the port with and without it at once
/// (tests and the `differential` feature only).
#[cfg(any(test, feature = "differential"))]
pub fn with<R>(on: bool, f: impl FnOnce() -> R) -> R {
    struct Reset(Option<bool>);
    impl Drop for Reset {
        fn drop(&mut self) {
            OVERRIDE.with(|o| o.set(self.0));
        }
    }
    let _reset = Reset(OVERRIDE.with(|o| o.replace(Some(on))));
    f()
}

pub fn enabled() -> bool {
    #[cfg(any(test, feature = "differential"))]
    {
        if let Some(on) = OVERRIDE.with(|o| o.get()) {
            return on;
        }
    }
    ENABLED.load(Ordering::Relaxed)
}

/// Why the kernel declined the call; the caller panics with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decline {
    /// A guest read faulted: `len` bytes at `addr`.
    Fault { addr: u32, len: u32 },
    /// An instruction would deliver an unmasked SSE or x87 exception.
    Float,
    /// Stack or output storage aliases a gathered input or the body's instructions.
    Alias,
}

impl Decline {
    /// The reason as the panic names it.
    pub fn name(self) -> &'static str {
        match self {
            Decline::Fault { .. } => "fault",
            Decline::Float => "float",
            Decline::Alias => "alias",
        }
    }
}

impl std::fmt::Display for Decline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Decline::Fault { addr, len } => write!(f, "fault: read of {len} bytes at {addr:#010x}"),
            other => f.write_str(other.name()),
        }
    }
}

/// Call counts since start, for timing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Calls the kernel ran.
    pub native: u64,
    /// Of those, calls that snapped to the target.
    pub snapped: u64,
}

static NATIVE: AtomicU64 = AtomicU64::new(0);
static SNAPPED: AtomicU64 = AtomicU64::new(0);

impl Stats {
    /// Publish the leaf calls in a committed first-person segment.
    pub(crate) fn record(&self) {
        if self.native != 0 {
            NATIVE.fetch_add(self.native, Ordering::Relaxed);
        }
        if self.snapped != 0 {
            SNAPPED.fetch_add(self.snapped, Ordering::Relaxed);
        }
    }
}

pub fn stats() -> Stats {
    Stats {
        native: NATIVE.load(Ordering::Relaxed),
        snapped: SNAPPED.load(Ordering::Relaxed),
    }
}

type Step<T> = Result<T, Decline>;

fn read<T>(r: Result<T, GuestFault>) -> Step<T> {
    r.map_err(|f| Decline::Fault { addr: f.addr, len: f.len as u32 })
}

/// The machine state `11eed0` reads, gathered.
#[derive(Clone, Debug)]
pub struct In {
    pub velocity: u32,
    pub position: u32,
    /// The stack window from the body's `esp`: the locals (garbage on
    /// entry), the return address and the five arguments.
    pub window: [u32; WINDOW],
    pub xmm: [u128; 8],
    pub mxcsr: u32,
    pub fpu: x87::abi::X87Abi,
    pub wrap: bool,
    pub half: u32,
    pub two: u32,
}

/// What the call leaves.
#[derive(Clone, Debug)]
pub struct Out {
    pub velocity: u32,
    pub position: u32,
    pub window: [u32; WINDOW],
    pub xmm: [u128; 8],
    pub mxcsr: u32,
    pub fpu: x87::abi::X87Abi,
    /// `al`.
    pub snapped: bool,
    /// `eax` above `al`, when the sign of the acceleration was taken
    /// (`None`: the caller's `eax` is kept).
    pub sign: Option<u32>,
}

/// The SSE side, instruction by instruction on the `simd` crate.
struct Sse {
    x: [u128; 8],
    mxcsr: u32,
}

type SseOp = fn(&mut u32, u128, u32) -> Result<u128, simd::Xm>;

impl Sse {
    fn low(&self, r: usize) -> u32 {
        self.x[r] as u32
    }
    /// `movss xmm, m32`: the upper lanes cleared.
    fn load(&mut self, r: usize, v: u32) {
        self.x[r] = u128::from(v);
    }
    /// `movaps xmm, xmm`.
    fn mov(&mut self, d: usize, s: usize) {
        self.x[d] = self.x[s];
    }
    fn op(&mut self, r: usize, b: u32, f: SseOp) -> Step<()> {
        self.x[r] = f(&mut self.mxcsr, self.x[r], b).map_err(|_| Decline::Float)?;
        Ok(())
    }
    fn add(&mut self, r: usize, b: u32) -> Step<()> {
        self.op(r, b, simd::addss)
    }
    fn sub(&mut self, r: usize, b: u32) -> Step<()> {
        self.op(r, b, simd::subss)
    }
    fn mul(&mut self, r: usize, b: u32) -> Step<()> {
        self.op(r, b, simd::mulss)
    }
    /// `comiss a, b` then `ja`.
    fn above(&mut self, a: u32, b: u32) -> Step<bool> {
        let f = simd::comiss(&mut self.mxcsr, a, b).map_err(|_| Decline::Float)?;
        Ok(f & 0x41 == 0)
    }
}

fn fpu_ok(r: Result<(), crate::light_constants::Error>) -> Step<()> {
    r.map_err(|_| Decline::Float)
}

fn fpu_val<T>(r: Result<T, crate::light_constants::Error>) -> Step<T> {
    r.map_err(|_| Decline::Float)
}

/// Body stores between the call's return-address push and the two axis
/// outputs. One path writes all nine: two locals, two distances, two
/// reaches, the stopping speed and its negation, then the acceleration.
#[derive(Default)]
struct StackStores {
    entries: [(usize, u32); 9],
    len: usize,
}

impl StackStores {
    fn write(&mut self, window: &mut [u32; WINDOW], slot: usize, value: u32) {
        window[slot] = value;
        self.entries[self.len] = (slot, value);
        self.len += 1;
    }
}

/// `11eed0`'s body on gathered state, in the guest's instruction order.
/// No guest memory is touched.
pub fn step(i: &In) -> Step<Out> {
    step_with_stores(i, &mut StackStores::default())
}

fn step_with_stores(i: &In, stores: &mut StackStores) -> Step<Out> {
    let mut w = i.window;
    let mut s = Sse {
        x: i.xmm,
        mxcsr: i.mxcsr,
    };
    let mut f = X87(i.fpu);
    let mut sign = None;

    // 11eed0: the velocity, the position, a zero, the squared speed
    // bound, and the distance to the target.
    s.load(0, i.velocity);
    s.load(6, i.position);
    s.x[7] = 0;
    stores.write(&mut w, VELOCITY, s.low(0));
    s.mov(0, 5);
    s.mul(0, s.low(5))?;
    stores.write(&mut w, SPEED_SQUARED, s.low(0));
    s.load(0, w[TARGET]);
    s.mov(2, 0);
    s.sub(2, s.low(6))?;
    stores.write(&mut w, TARGET, s.low(2));
    if i.wrap {
        // 11ef09: the short way round `[lo, hi]`.
        s.load(1, w[HI]);
        s.sub(1, w[LO])?;
        s.mul(1, i.half)?;
        if s.above(s.low(2), s.low(1))? {
            s.mul(1, i.two)?;
            s.sub(2, s.low(1))?;
            stores.write(&mut w, TARGET, s.low(2));
        } else {
            s.mov(3, 7);
            s.sub(3, s.low(1))?;
            if s.above(s.low(3), s.low(2))? {
                s.mul(1, i.two)?;
                s.add(1, s.low(2))?;
                s.mov(2, 1);
                stores.write(&mut w, TARGET, s.low(2));
            }
        }
    }
    // 11ef51: a tick's reach, the smaller of the two bounds' reaches.
    s.load(3, w[TICK]);
    s.mov(1, 3);
    s.mul(1, s.low(3))?;
    s.mul(1, w[ACCEL])?;
    s.mov(4, 3);
    s.mul(4, s.low(5))?;
    let longer = s.above(s.low(1), s.low(4))?;
    stores.write(&mut w, REACH, s.low(4));
    if !longer {
        stores.write(&mut w, REACH, s.low(1));
    }
    // 11ef7c: |distance - velocity * tick| against the reach.
    fpu_ok(f.fld(w[VELOCITY]))?;
    fpu_ok(f.fmul_m(w[TICK]))?;
    fpu_ok(f.arith_m(arith_op::SUBR, w[TARGET]))?;
    fpu_ok(f.unary(unary_op::ABS))?;
    fpu_ok(f.fld(w[REACH]))?;
    let within = fpu_val(f.fcomip(1))?;
    fpu_ok(f.fstp_st(0))?;
    if within & 1 == 0 {
        // 11ef93: snap to the target, clamped; the velocity stops.
        s.load(2, w[LO]);
        let low = s.above(s.low(2), s.low(0))?;
        s.mov(1, 7);
        if low {
            s.mov(0, 2);
        } else {
            s.load(2, w[HI]);
            if s.above(s.low(0), s.low(2))? {
                s.mov(0, 2);
            }
        }
        return Ok(Out {
            velocity: s.low(1),
            position: s.low(0),
            window: w,
            xmm: s.x,
            mxcsr: s.mxcsr,
            fpu: f.0,
            snapped: true,
            sign,
        });
    }

    // 11efc1: the speed that stops at the target under the acceleration
    // bound, sqrt(2 * accel * |distance|), capped at the speed bound.
    fpu_ok(f.fld(w[TARGET]))?;
    fpu_ok(f.unary(unary_op::ABS))?;
    fpu_ok(f.fmul_m(w[ACCEL]))?;
    fpu_ok(f.arith_reg(arith_op::ADD, 0, 0, false))?;
    fpu_ok(f.fld(w[SPEED_SQUARED]))?;
    fpu_ok(f.fld_st(1))?;
    let slower = fpu_val(f.fcomip(1))?;
    fpu_ok(f.fstp_st(0))?;
    if slower & 1 != 0 {
        fpu_ok(f.unary(unary_op::SQRT))?;
        stores.write(&mut w, TARGET, fpu_val(f.fstp_m32())?);
    } else {
        fpu_ok(f.fstp_st(0))?;
        stores.write(&mut w, TARGET, s.low(5));
    }
    // 11efe9: toward the target.
    if s.above(s.low(7), s.low(2))? {
        s.mov(0, 7);
        s.sub(0, w[TARGET])?;
        stores.write(&mut w, TARGET, s.low(0));
    }
    // 11effd: the acceleration that reaches that speed in one tick,
    // stored over the tick argument, and its size against the bound.
    fpu_ok(f.fld(w[TARGET]))?;
    fpu_ok(f.arith_m(arith_op::SUB, w[VELOCITY]))?;
    fpu_ok(f.arith_m(arith_op::DIV, w[TICK]))?;
    stores.write(&mut w, TICK, fpu_val(f.fst_m32())?);
    fpu_ok(f.unary(unary_op::ABS))?;
    s.load(0, w[TICK]);
    fpu_ok(f.fld(w[ACCEL]))?;
    fpu_ok(f.fxch(1))?;
    let over = fpu_val(f.fcomip(1))?;
    fpu_ok(f.fstp_st(0))?;
    if over & 0x41 == 0 {
        // 11f020: the bound, signed as the acceleration (`lahf` and
        // `test ah, 44h` separate equal from the rest).
        let (a, b) = (s.low(0), s.low(7));
        let u = simd::ucomiss(&mut s.mxcsr, a, b).map_err(|_| Decline::Float)?;
        let eax = if u & 0x44 == 0x40 {
            0
        } else if s.above(s.low(7), s.low(0))? {
            u32::MAX
        } else {
            1
        };
        sign = Some(eax);
        s.x[0] = simd::cvtsi2ss(&mut s.mxcsr, s.x[0], eax).map_err(|_| Decline::Float)?;
        s.mul(0, w[ACCEL])?;
    }
    // 11f048: the new velocity and position.
    s.load(2, w[LO]);
    s.mul(0, s.low(3))?;
    s.mov(1, 0);
    s.mul(0, i.half)?;
    s.add(1, w[VELOCITY])?;
    s.add(0, s.low(1))?;
    s.mul(0, s.low(3))?;
    s.add(0, s.low(6))?;
    if i.wrap {
        if s.above(s.low(2), s.low(0))? {
            s.load(3, w[HI]);
            s.sub(3, s.low(2))?;
            s.add(3, s.low(0))?;
            s.mov(0, 3);
        } else if s.above(s.low(0), w[HI])? {
            s.load(3, w[HI]);
            s.sub(3, s.low(2))?;
            s.sub(0, s.low(3))?;
        }
    }
    // 11f09f: clamped into `[lo, hi]`.
    if s.above(s.low(2), s.low(0))? {
        s.mov(0, 2);
    } else {
        s.load(2, w[HI]);
        if s.above(s.low(0), s.low(2))? {
            s.mov(0, 2);
        }
    }
    Ok(Out {
        velocity: s.low(1),
        position: s.low(0),
        window: w,
        xmm: s.x,
        mxcsr: s.mxcsr,
        fpu: f.0,
        snapped: false,
        sign,
    })
}

/// A guest virtual span can share backing RAM with a different virtual
/// address. Compare physical byte ranges, including partial-word aliases,
/// one lookup per page (`sim_native::spans`): the body's text crosses
/// 0x11f000, and the loader maps each image page on its own.
fn physical(g: &impl Memory, addr: u32, size: usize) -> Step<Spans> {
    Spans::of(g, addr, size).ok_or(Decline::Fault { addr, len: size as u32 })
}

fn overlaps(a: &Spans, b: &Spans) -> bool {
    a.overlaps(b)
}

fn fpu_in(c: &CpuAbi) -> x87::abi::X87Abi {
    let x = &c.x87;
    x87::abi::X87Abi {
        st: x.st,
        control: x.control,
        status: x.status,
        tag: x.tag,
        opcode: x.opcode,
        instruction_pointer: x.instruction_pointer,
        data_pointer: x.data_pointer,
        instruction_selector: x.instruction_selector,
        data_selector: x.data_selector,
    }
}

/// What the scatter writes, decided before anything is written.
struct Plan {
    base: u32,
    out: Out,
    stores: StackStores,
    esi: u32,
    edi: u32,
    /// FOP of the path's last x87 instruction, read from the image.
    opcode: u16,
}

fn plan(c: &CpuAbi, g: &impl Memory, site: u32) -> Step<Plan> {
    // The body's `esp`: the call pushes the return address, the body
    // takes 12 bytes of locals.
    let base = c.esp.wrapping_sub(16);
    let stack = physical(g, base, WINDOW * 4)?;
    let velocity = physical(g, c.esi, 4)?;
    let position = physical(g, c.edi, 4)?;
    let code = physical(g, FILTER, (FILTER_END - FILTER) as usize)?;
    let half = physical(g, HALF, 4)?;
    let two = physical(g, TWO, 4)?;

    if overlaps(&stack, &velocity) || overlaps(&stack, &position) {
        return Err(Decline::Alias);
    }
    // The constants are gathered before the call pushes its return
    // address. Stores into code can also change later instructions.
    if overlaps(&stack, &half)
        || overlaps(&stack, &two)
        || [&stack, &velocity, &position].iter().any(|r| overlaps(r, &code))
    {
        return Err(Decline::Alias);
    }
    let mut window = [0u32; WINDOW];
    for (k, v) in window.iter_mut().enumerate().skip(TARGET) {
        *v = read(g.u32(base.wrapping_add(4 * k as u32)))?;
    }
    window[RETURN] = site;
    let input = In {
        velocity: read(g.u32(c.esi))?,
        position: read(g.u32(c.edi))?,
        window,
        xmm: c.xmm.map(u128::from_le_bytes),
        mxcsr: c.mxcsr,
        fpu: fpu_in(c),
        wrap: c.edx as u8 != 0,
        half: read(g.u32(HALF))?,
        two: read(g.u32(TWO))?,
    };
    let mut stores = StackStores::default();
    let out = step_with_stores(&input, &mut stores)?;
    let last = if out.snapped { SNAP_LAST } else { MOVE_LAST };
    let [op, modrm] = read(g.load::<2>(last))?;
    Ok(Plan {
        base,
        out,
        stores,
        esi: c.esi,
        edi: c.edi,
        opcode: (u16::from(op & 7) << 8) | u16::from(modrm),
    })
}

fn scatter(c: &mut CpuAbi, g: &mut impl Memory, p: &Plan) -> Result<(), GuestFault> {
    // The guest's stores in its order: the return address, the stack
    // slots, then the velocity and the position.
    let at = |k: usize| p.base.wrapping_add(4 * k as u32);
    g.set_u32(at(RETURN), p.out.window[RETURN])?;
    for &(slot, value) in &p.stores.entries[..p.stores.len] {
        g.set_u32(at(slot), value)?;
    }
    g.set_u32(p.esi, p.out.velocity)?;
    g.set_u32(p.edi, p.out.position)?;

    let (last, operand) = if p.out.snapped {
        (SNAP_LAST, SNAP_OPERAND)
    } else {
        (MOVE_LAST, MOVE_OPERAND)
    };
    let f = &p.out.fpu;
    c.x87.st = f.st;
    c.x87.status = f.status;
    c.x87.tag = f.tag;
    c.x87.instruction_pointer = last;
    c.x87.instruction_selector = 8;
    c.x87.opcode = p.opcode;
    c.x87.data_pointer = p.base.wrapping_add(operand);
    c.x87.data_selector = 16;
    for (r, v) in c.xmm.iter_mut().zip(p.out.xmm) {
        *r = v.to_le_bytes();
    }
    c.mxcsr = p.out.mxcsr;
    if let Some(eax) = p.out.sign {
        c.eax = eax;
    }
    c.eax = (c.eax & !0xff) | u32::from(p.out.snapped);
    c.ecx = (c.ecx & !0xff) | u32::from(p.out.snapped);
    // `add esp, 0Ch`, then `ret 14h`.
    crate::sim_physics::arithmetic(c, p.base, 12, false);
    c.esp = c.esp.wrapping_add(ARGS);
    Ok(())
}

/// The call to `11eed0` that returns to `site`, with its arguments
/// already on the stack at `esp`. `Ok(true)`: the kernel made the call
/// — the stack is popped past the arguments and the caller carries on at
/// `site`. `Ok(false)`: the kernel is switched off; the caller makes the
/// guest call. A decline panics `anim filter: <reason>`.
pub fn run(c: &mut CpuAbi, g: &mut impl Memory, site: u32) -> Result<bool, GuestFault> {
    let mut counts = Stats::default();
    let result = run_counted(c, g, site, &mut counts)?;
    counts.record();
    Ok(result)
}

/// The caller publishes the counts when it commits the segment.
pub(crate) fn run_counted(
    c: &mut CpuAbi,
    g: &mut impl Memory,
    site: u32,
    counts: &mut Stats,
) -> Result<bool, GuestFault> {
    if !enabled() {
        return Ok(false);
    }
    let plan = match plan(c, g, site) {
        Ok(p) => p,
        Err(why) => unimplemented!("anim filter: {why}"),
    };
    scatter(c, g, &plan)?;
    counts.native += 1;
    counts.snapped += u64::from(plan.out.snapped);
    Ok(true)
}

#[cfg(test)]
mod tests;
