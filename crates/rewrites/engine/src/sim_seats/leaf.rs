//! The seat bodies' leaf callees in Rust (docs/engine/sim-rewrite.md,
//! "Seats and rules (phase 7)"), each behind a shadow arm of its own (the
//! pattern of [`crate::sim_seats::aim::arm`]).
//!
//! A [`Leaf`] is a guest function a seat body calls that calls nothing
//! itself and reads only what its registers name:
//!
//! | Leaf | Guest | Returns to | Answer |
//! |---|---|---|---|
//! | [`Leaf::Visible`] | `e4050` | the control update, `0x1865c4` | whether a biped can take assisted aim: not dead, its look enabled, its rank under the tick's threshold, standing or seated with no parent |
//! | [`Leaf::PlayerIndex`] | `b67a0` | the control update, `0x1863a6` | the player's slot when the record's count holds it, else `-1` |
//! | [`Leaf::Direction`] | `11df30` | the control update, `0x1863f7`, `0x186403` | a vector's yaw and pitch, on the x87 model |
//! | [`Leaf::UserTest`] | `14ddc0` | the HUD seat update (`USER_TESTED`) | whether the user slot has a player |
//! | [`Leaf::UserPlayer`] | `14de70` | the HUD seat update (`PLAYER_FOUND`) | the user slot's player |
//!
//! At the call, [`call`] runs the kernel. Until [`PROVEN_AFTER`] of a
//! leaf's predictions have matched with no mismatch, the guest's leaf
//! runs and the prediction is compared at the return site ([`at_site`]):
//! `eax`, `ecx`, `edx`, the callee-saved registers, `xmm0`, the x87 stack
//! where the leaf uses it, and the words it writes. After that the kernel
//! answers in place and the body goes on at the site in the same segment;
//! one answerable call in [`AUDIT_EVERY`] still goes to the guest. A
//! mismatch hands that leaf back to the guest until the next map.

use crate::light_constants::X87;
use crate::sim_native::{Memory, Result};
use crate::sim_physics::{
    ARITH,
    spatial::{finish_fpu, load_fpu},
};
use runtime::CpuAbi;
use sim_core::det::{self, Fp};
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use x87::abi::{arith_op, unary_op};

/// Matched calls of a leaf, with no mismatch, before its kernel answers.
pub const PROVEN_AFTER: u64 = 4096;
/// After that, one answerable call in this many is compared.
pub const AUDIT_EVERY: u64 = 64;

const OBJECT_HEADER: u32 = 0x004e_0300;
const TICKS: u32 = 0x0051_0c54;
const RANK_SCALE: u32 = 0x0045_df20;
const USERS: u32 = 0x004e_8c20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Leaf {
    Visible,
    PlayerIndex,
    Direction,
    UserTest,
    UserPlayer,
}

impl Leaf {
    pub const ALL: [Leaf; 5] = [
        Leaf::Visible,
        Leaf::PlayerIndex,
        Leaf::Direction,
        Leaf::UserTest,
        Leaf::UserPlayer,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Leaf::Visible => "visible",
            Leaf::PlayerIndex => "player_index",
            Leaf::Direction => "direction",
            Leaf::UserTest => "user_test",
            Leaf::UserPlayer => "user_player",
        }
    }

    pub fn target(self) -> u32 {
        match self {
            Leaf::Visible => 0x000e_4050,
            Leaf::PlayerIndex => 0x000b_67a0,
            Leaf::Direction => 0x0011_df30,
            Leaf::UserTest => 0x0014_ddc0,
            Leaf::UserPlayer => 0x0014_de70,
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// Why a call was left to the guest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decline {
    /// MXCSR or the x87 not in the pinned state, an operand off the exact
    /// range, or a step the x87 model refuses.
    Float,
    /// A read of the inputs faulted, or a write would.
    Fault,
}

impl Decline {
    pub const ALL: [Decline; 2] = [Decline::Float, Decline::Fault];

    pub fn name(self) -> &'static str {
        match self {
            Decline::Float => "float",
            Decline::Fault => "fault",
        }
    }
}

/// One leaf's counters since the map loaded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub checked: u64,
    pub matched: u64,
    pub mismatched: u64,
    pub answered: u64,
    /// By [`Decline::ALL`] order.
    pub declined: [u64; 2],
    pub dropped: u64,
}

impl Status {
    pub fn declined_total(&self) -> u64 {
        self.declined.iter().sum()
    }
}

const N: usize = Leaf::ALL.len();
static CHECKED: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];
static MATCHED: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];
static MISMATCHED: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];
static ANSWERED: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];
static ANSWERABLE: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];
static DROPPED: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];
static DECLINED: [[AtomicU64; 2]; N] = [const { [const { AtomicU64::new(0) }; 2] }; N];

thread_local! {
    static PENDING: Cell<Option<Pending>> = const { Cell::new(None) };
    static FORCE: Cell<bool> = const { Cell::new(false) };
}

#[derive(Clone, Copy, Debug)]
struct Pending {
    leaf: Leaf,
    site: u32,
    esp: u32,
    effect: Effect,
}

pub fn status(leaf: Leaf) -> Status {
    let i = leaf.index();
    let read = |c: &AtomicU64| c.load(Ordering::Relaxed);
    Status {
        checked: read(&CHECKED[i]),
        matched: read(&MATCHED[i]),
        mismatched: read(&MISMATCHED[i]),
        answered: read(&ANSWERED[i]),
        declined: std::array::from_fn(|d| read(&DECLINED[i][d])),
        dropped: read(&DROPPED[i]),
    }
}

/// Forget every leaf's proof and counters. Called at map load.
pub fn reset() {
    for i in 0..N {
        for c in [
            &CHECKED[i],
            &MATCHED[i],
            &MISMATCHED[i],
            &ANSWERED[i],
            &ANSWERABLE[i],
            &DROPPED[i],
        ] {
            c.store(0, Ordering::Relaxed);
        }
        for c in &DECLINED[i] {
            c.store(0, Ordering::Relaxed);
        }
    }
    PENDING.set(None);
}

/// Answer every leaf on this thread, proof or not: the differential
/// harness's way to put the kernels against the originals.
pub fn force_answer(on: bool) {
    FORCE.set(on);
}

fn proven(i: usize) -> bool {
    MISMATCHED[i].load(Ordering::Relaxed) == 0 && MATCHED[i].load(Ordering::Relaxed) >= PROVEN_AFTER
}

/// What a leaf leaves: the volatile registers, `xmm0`, the x87 state
/// with its last instruction and operand, and up to two word stores.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Effect {
    pub eax: u32,
    pub ecx: u32,
    pub edx: u32,
    pub xmm0: [u8; 16],
    pub x87: Option<(X87State, u32, u32)>,
    pub writes: [Option<(u32, u32)>; 2],
}

/// The x87 registers, tag and status the model leaves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct X87State {
    pub st: [[u8; 10]; 8],
    pub tag: u16,
    pub status: u16,
}

/// `call leaf` returning to `site`, through the leaf's arm: the kernel's
/// answer laid down and `resume` run at the site, or the guest's call.
pub fn call<M: Memory>(
    c: &mut CpuAbi,
    m: &mut M,
    leaf: Leaf,
    site: u32,
    resume: impl FnOnce(&mut CpuAbi, &mut M) -> Result,
) -> Result {
    if let Some(p) = PENDING.take() {
        DROPPED[p.leaf.index()].fetch_add(1, Ordering::Relaxed);
    }
    let i = leaf.index();
    match kernel(leaf, c, m) {
        Err(why) => {
            DECLINED[i][why as usize].fetch_add(1, Ordering::Relaxed);
        }
        Ok(effect) => {
            let slot = if FORCE.get() {
                1
            } else {
                ANSWERABLE[i].fetch_add(1, Ordering::Relaxed)
            };
            if FORCE.get() || (proven(i) && slot % AUDIT_EVERY != 0) {
                apply(c, m, &effect)?;
                ANSWERED[i].fetch_add(1, Ordering::Relaxed);
                return resume(c, m);
            }
            PENDING.set(Some(Pending {
                leaf,
                site,
                esp: c.esp,
                effect,
            }));
        }
    }
    m.call(c, leaf.target(), site)
}

/// Lay `e` down: the stores, then the registers.
pub fn apply(
    c: &mut CpuAbi,
    m: &mut impl Memory,
    e: &Effect,
) -> std::result::Result<(), runtime::GuestFault> {
    for (a, v) in e.writes.iter().flatten() {
        m.set_u32(*a, *v)?;
    }
    c.eax = e.eax;
    c.ecx = e.ecx;
    c.edx = e.edx;
    c.xmm[0] = e.xmm0;
    if let Some((x, pc, operand)) = e.x87 {
        let mut model = load_fpu(c);
        model.0.st = x.st;
        model.0.tag = x.tag;
        model.0.status = x.status;
        finish_fpu(c, m, model, pc, operand)?;
    }
    c.eflags_defined &= !ARITH;
    Ok(())
}

/// At a body's resume `pc`: compare a pending prediction for it.
pub fn at_site(c: &CpuAbi, m: &impl Memory, pc: u32) {
    let Some(p) = PENDING.take() else { return };
    if p.site != pc {
        PENDING.set(Some(p));
        return;
    }
    let i = p.leaf.index();
    if p.esp != c.esp {
        DROPPED[i].fetch_add(1, Ordering::Relaxed);
        return;
    }
    CHECKED[i].fetch_add(1, Ordering::Relaxed);
    let e = &p.effect;
    let what = if (c.eax, c.ecx, c.edx) != (e.eax, e.ecx, e.edx) {
        Some("eax ecx edx")
    } else if c.xmm[0] != e.xmm0 {
        Some("xmm0")
    } else if e
        .x87
        .is_some_and(|(x, ..)| (x.st, x.tag, x.status) != (c.x87.st, c.x87.tag, c.x87.status))
    {
        Some("x87")
    } else if e
        .writes
        .iter()
        .flatten()
        .any(|(a, v)| m.u32(*a).ok() != Some(*v))
    {
        Some("stores")
    } else {
        None
    };
    match what {
        None => {
            MATCHED[i].fetch_add(1, Ordering::Relaxed);
        }
        Some(what) => {
            if MISMATCHED[i].fetch_add(1, Ordering::Relaxed) == 0 {
                eprintln!(
                    "sim: seats leaf {}: the guest's {what} differs from the prediction; \
                     the guest answers it until the next map",
                    p.leaf.name()
                );
            }
        }
    }
}

/// The leaf's whole effect from the registers at its call.
pub fn kernel(leaf: Leaf, c: &CpuAbi, m: &impl Memory) -> std::result::Result<Effect, Decline> {
    let word = |a: u32| m.u32(a).map_err(|_| Decline::Fault);
    let byte = |a: u32| m.u8(a).map_err(|_| Decline::Fault);
    let mut e = Effect {
        eax: c.eax,
        ecx: c.ecx,
        edx: c.edx,
        xmm0: c.xmm[0],
        x87: None,
        writes: [None; 2],
    };
    match leaf {
        Leaf::UserTest => {
            // `xor al, al`, then `setne al` over the table's word.
            e.eax = c.eax & !0xff;
            if c.ecx != u32::MAX {
                let table = word(USERS)?;
                let player = word(table.wrapping_add(c.ecx.wrapping_mul(4)).wrapping_add(0xc))?;
                e.eax = (table & !0xff) | u32::from(player != u32::MAX);
            }
        }
        Leaf::UserPlayer => {
            e.eax = u32::MAX;
            if c.ecx != u32::MAX {
                let table = word(USERS)?;
                e.eax = word(table.wrapping_add(c.ecx.wrapping_mul(4)).wrapping_add(0xc))?;
            }
        }
        Leaf::PlayerIndex => {
            let slot = byte(c.ecx.wrapping_add(0x18))? as i8;
            let low = (c.eax & !0xff) | u32::from(slot as u8);
            let held = slot >= 0 && {
                let count = word(c.ecx.wrapping_add(0x74))? as i32;
                e.edx = i32::from(slot) as u32;
                i32::from(slot) < count
            };
            let index = if held {
                i32::from(slot) as u16
            } else {
                u16::MAX
            };
            e.eax = (low & 0xffff_0000) | u32::from(index);
        }
        Leaf::Visible => visible(c, m, &mut e)?,
        Leaf::Direction => direction(c, m, &mut e)?,
    }
    Ok(e)
}

/// `e4050`: a biped that takes assisted aim.
fn visible(c: &CpuAbi, m: &impl Memory, e: &mut Effect) -> std::result::Result<(), Decline> {
    let word = |a: u32| m.u32(a).map_err(|_| Decline::Fault);
    let byte = |a: u32| m.u8(a).map_err(|_| Decline::Fault);
    let header = word(OBJECT_HEADER)?;
    let entries = word(header.wrapping_add(0x44))?;
    let body = word(entries.wrapping_add((c.eax & 0xffff) * 12 + 8))?;
    e.edx = 1;
    e.eax = 0;
    let dead = byte(body.wrapping_add(0xc1))?;
    e.ecx = (header & !0xff) | u32::from(dead);
    if dead & 1 != 0 || byte(body.wrapping_add(0x348))? & 1 == 0 {
        return Ok(());
    }
    // The tick's rank threshold: `cvtsi2ss`, `mulss`, then `fld; fistp`
    // under the caller's x87 control word.
    if !det::fast_env(c.mxcsr) || !det::host_fast_env() || c.x87.control & 0x3f != 0x3f {
        return Err(Decline::Float);
    }
    let ticks = word(TICKS)?;
    let count = i32::from(m.u16(ticks.wrapping_add(2)).map_err(|_| Decline::Fault)? as i16);
    let mut fp = Fp::new();
    let product = fp.mul(det::from_int(count), word(RANK_SCALE)?);
    if !fp.exact() {
        return Err(Decline::Float);
    }
    e.xmm0[..4].copy_from_slice(&product.to_le_bytes());
    let mut x = load_fpu(c);
    x.fld(product).map_err(|_| Decline::Float)?;
    let (code, store) = x87::abi::st_i32(&x.0, 0);
    X87::ok(code).map_err(|_| Decline::Float)?;
    x87::abi::store_commit(&mut x.0, store.status, true);
    let threshold = store.value;
    // `fistp [esp + 4]` in the leaf's eight-byte frame below the return.
    let operand = c.esp.wrapping_sub(8);
    e.x87 = Some((
        X87State {
            st: x.0.st,
            tag: x.0.tag,
            status: x.0.status,
        },
        0x000e_409d,
        operand,
    ));
    let rank = i32::from(byte(body.wrapping_add(0x399))? as i8);
    e.ecx = rank as u32;
    if rank < threshold {
        return Ok(());
    }
    let stance = byte(body.wrapping_add(0x3dc))?;
    e.ecx = (e.ecx & !0xff) | u32::from(stance);
    if (stance == 1 || stance == 3) && word(body.wrapping_add(0x14))? == u32::MAX {
        e.eax = 1;
    }
    Ok(())
}

/// `11df30`: `[eax] = atan2(y, x)`, `[eax + 4] = atan2(z, hypot(x, y))`.
fn direction(c: &CpuAbi, m: &impl Memory, e: &mut Effect) -> std::result::Result<(), Decline> {
    let word = |a: u32| m.u32(a).map_err(|_| Decline::Fault);
    if c.x87.control & 0x3f != 0x3f {
        return Err(Decline::Float);
    }
    let [x_, y_, z_] = [
        word(c.ecx)?,
        word(c.ecx.wrapping_add(4))?,
        word(c.ecx.wrapping_add(8))?,
    ];
    let mut x = load_fpu(c);
    let f =
        |r: std::result::Result<(), crate::light_constants::Error>| r.map_err(|_| Decline::Float);
    f(x.fld(y_))?;
    f(x.fld(x_))?;
    f(x.unary(unary_op::PATAN))?;
    let yaw = x.fstp_m32().map_err(|_| Decline::Float)?;
    f(x.fld(y_))?;
    f(x.fld(x_))?;
    f(x.fld(z_))?;
    f(x.fld_st(2))?;
    f(x.arith_reg(arith_op::MUL, 3, 0, true))?;
    f(x.fld_st(1))?;
    f(x.arith_reg(arith_op::MUL, 0, 2, false))?;
    f(x.arith_reg(arith_op::ADD, 3, 0, true))?;
    f(x.fxch(2))?;
    f(x.unary(unary_op::SQRT))?;
    f(x.fxch(2))?;
    f(x.fxch(1))?;
    f(x.fxch(2))?;
    f(x.unary(unary_op::PATAN))?;
    f(x.fstp_st(1))?;
    let pitch = x.fstp_m32().map_err(|_| Decline::Float)?;
    // Both stores must take: check before predicting them.
    word(c.eax)?;
    word(c.eax.wrapping_add(4))?;
    e.writes = [Some((c.eax, yaw)), Some((c.eax.wrapping_add(4), pitch))];
    e.x87 = Some((
        X87State {
            st: x.0.st,
            tag: x.0.tag,
            status: x.0.status,
        },
        0x0011_df59,
        c.eax.wrapping_add(4),
    ));
    Ok(())
}
