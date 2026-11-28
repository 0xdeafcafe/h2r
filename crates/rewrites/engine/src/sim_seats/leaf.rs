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
//! | [`Leaf::Submit`] | `187b30` | the control update, `0x186a94` | the input record's flags into the control globals' word, the latches and the axis clamps |
//! | [`Leaf::WeaponReady`] | `100f70` | the control update, `0x1866b3` | the weapon can be readied: none of its magazines holds 1 to 6 rounds (its callee `100880`, [`magazine`]) and it has a zoom level |
//! | [`Leaf::InitPacket`] | `c6ef0` | the action and neutral packet builders, `0x150673`, `0x1507b4` | the whole `0x7c`-byte unit-control packet, as a [`Block`] |
//!
//! At the call, [`call`] runs the kernel. Until [`PROVEN_AFTER`] of a
//! leaf's predictions have matched with no mismatch, the guest's leaf
//! runs and the prediction is compared at the return site ([`at_site`]):
//! `eax`, `ecx`, `edx`, the callee-saved registers, `xmm0`, the x87 stack
//! where the leaf uses it, the words it writes and its [`Block`]. After that the kernel
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
const CONTROL: u32 = 0x004e_d284;
const TAG_ENTRIES: u32 = guest_layout::points::TAG_ENTRIES.base;
/// `100880`'s index is `movsx cx`: a count past this would wrap it.
const MAGAZINES_MAX: i32 = 0x7fff;
/// `c6ef0` reads its three directions through this pointer.
const PACKET_VECTOR: u32 = 0x0046_87a8;
/// The packet's first word, `c6ef0`'s immediate at `0xc6f09`.
const PACKET_HEADER: u32 = 0x0600_0086;
const DIRECTION_FLAG: u32 = 1 << 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Leaf {
    Visible,
    PlayerIndex,
    Direction,
    UserTest,
    UserPlayer,
    Submit,
    WeaponReady,
    InitPacket,
}

impl Leaf {
    pub const ALL: [Leaf; 8] = [
        Leaf::Visible,
        Leaf::PlayerIndex,
        Leaf::Direction,
        Leaf::UserTest,
        Leaf::UserPlayer,
        Leaf::Submit,
        Leaf::WeaponReady,
        Leaf::InitPacket,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Leaf::Visible => "visible",
            Leaf::PlayerIndex => "player_index",
            Leaf::Direction => "direction",
            Leaf::UserTest => "user_test",
            Leaf::UserPlayer => "user_player",
            Leaf::Submit => "submit",
            Leaf::WeaponReady => "weapon_ready",
            Leaf::InitPacket => "init_packet",
        }
    }

    pub fn target(self) -> u32 {
        match self {
            Leaf::Visible => 0x000e_4050,
            Leaf::PlayerIndex => 0x000b_67a0,
            Leaf::Direction => 0x0011_df30,
            Leaf::UserTest => 0x0014_ddc0,
            Leaf::Submit => 0x0018_7b30,
            Leaf::WeaponReady => 0x0010_0f70,
            Leaf::UserPlayer => 0x0014_de70,
            Leaf::InitPacket => 0x000c_6ef0,
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// Why a call was left to the guest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decline {
    /// MXCSR, the x87 or the direction flag not in the pinned state, an
    /// operand off the exact range, or a step the x87 model refuses.
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

/// What a leaf leaves: the registers, `xmm0` and `xmm1`, the x87 state
/// with its last instruction and operand, up to six word stores and one
/// [`Block`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Effect {
    pub eax: u32,
    pub ecx: u32,
    pub edx: u32,
    pub ebx: u32,
    pub esi: u32,
    pub edi: u32,
    pub xmm0: [u8; 16],
    pub xmm1: [u8; 16],
    /// The MXCSR after the leaf: the entry's, with the flags its SSE
    /// arithmetic raised.
    pub mxcsr: u32,
    pub x87: Option<(X87State, u32, u32)>,
    pub writes: [Option<(u32, u32)>; 6],
    /// Laid down before `writes`. Held inline, so an effect with none
    /// costs one tag test in [`apply`] and [`at_site`], and no allocation.
    pub block: Option<Block>,
}

/// Bytes in a [`Block`]: the longest store run a leaf leaves, `c6ef0`'s
/// packet.
pub const BLOCK: usize = 0x7c;

/// A leaf's run of [`BLOCK`] bytes at `addr`, past what six word stores
/// hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
    pub addr: u32,
    pub bytes: [u8; BLOCK],
}

impl Effect {
    /// Nothing changed from `c`.
    pub fn of(c: &CpuAbi) -> Effect {
        Effect {
            eax: c.eax,
            ecx: c.ecx,
            edx: c.edx,
            ebx: c.ebx,
            esi: c.esi,
            edi: c.edi,
            xmm0: c.xmm[0],
            xmm1: c.xmm[1],
            mxcsr: c.mxcsr,
            x87: None,
            writes: [None; 6],
            block: None,
        }
    }
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

/// Lay `e` down: the block in word pieces (a segment's store log takes
/// four bytes a store), the word stores, then the registers.
pub fn apply(
    c: &mut CpuAbi,
    m: &mut impl Memory,
    e: &Effect,
) -> std::result::Result<(), runtime::GuestFault> {
    if let Some(b) = &e.block {
        for (i, piece) in b.bytes.chunks(4).enumerate() {
            m.write_bytes(b.addr.wrapping_add(4 * i as u32), piece, 1)?;
        }
    }
    for (a, v) in e.writes.iter().flatten() {
        m.set_u32(*a, *v)?;
    }
    c.eax = e.eax;
    c.ecx = e.ecx;
    c.edx = e.edx;
    c.ebx = e.ebx;
    c.esi = e.esi;
    c.edi = e.edi;
    c.xmm[0] = e.xmm0;
    c.xmm[1] = e.xmm1;
    c.mxcsr = e.mxcsr;
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
    let what = if (c.ebx, c.esi, c.edi) != (e.ebx, e.esi, e.edi) {
        Some("ebx esi edi")
    } else if (c.eax, c.ecx, c.edx) != (e.eax, e.ecx, e.edx) {
        Some("eax ecx edx")
    } else if c.xmm[0] != e.xmm0 || c.xmm[1] != e.xmm1 {
        Some("xmm0 xmm1")
    } else if c.mxcsr != e.mxcsr {
        Some("mxcsr")
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
    } else if e.block.as_ref().is_some_and(|b| {
        let mut got = [0u8; BLOCK];
        m.read_bytes(b.addr, &mut got, 1).is_err() || got != b.bytes
    }) {
        Some("block")
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
    let mut e = Effect::of(c);
    match leaf {
        Leaf::Submit => submit(c, m, &mut e)?,
        Leaf::WeaponReady => weapon_ready(c, m, &mut e)?,
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
        Leaf::InitPacket => init_packet(c, m, &mut e)?,
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
    // under the caller's x87 control word. The `mulss` raises PE when
    // inexact, whatever the MXCSR held.
    if !det::guest_env(c.mxcsr) || !det::host_fast_env() || c.x87.control & 0x3f != 0x3f {
        return Err(Decline::Float);
    }
    let ticks = word(TICKS)?;
    let count = i32::from(m.u16(ticks.wrapping_add(2)).map_err(|_| Decline::Fault)? as i16);
    let mut fp = Fp::tracked(c.mxcsr);
    let product = fp.mul(det::from_int(count), word(RANK_SCALE)?);
    if !fp.exact() {
        return Err(Decline::Float);
    }
    e.mxcsr = fp.mxcsr().unwrap_or(c.mxcsr);
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
    e.writes[0] = Some((c.eax, yaw));
    e.writes[1] = Some((c.eax.wrapping_add(4), pitch));
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

/// `100f70 (weapon: eax) -> al`: the weapon can be readied. Its callee
/// `100880` runs here as [`magazine`] (with `ebx` the `-1` `100f70` sets);
/// with no magazine holding 1 to 6 rounds, the answer is whether the
/// definition has a zoom level (`[def + 1fe] - 1 != -1`). `ecx` is what
/// `100880` leaves, `edx` that or the levels less one; `ebx`, `esi` and
/// `edi` come back saved. Only the dead frame below the return is not
/// laid down.
fn weapon_ready(c: &CpuAbi, m: &impl Memory, e: &mut Effect) -> std::result::Result<(), Decline> {
    let word = |a: u32| m.u32(a).map_err(|_| Decline::Fault);
    let entries = word(word(OBJECT_HEADER)?.wrapping_add(0x44))?;
    let object = word(entries.wrapping_add((c.eax & 0xffff) * 12 + 8))?;
    let tag = word(object)? & 0xffff;
    let def = word((tag << 4).wrapping_add(word(TAG_ENTRIES)?).wrapping_add(8))?;
    let held = magazine(m, object, def, u32::MAX)?;
    e.ecx = held.ecx;
    e.edx = held.edx;
    // `esi` is `-1` into the call; a weapon with no loaded magazine takes
    // `setne al; dec eax` over its zoom levels.
    let mut slot = u32::MAX;
    if !held.found {
        let levels = m.u16(def.wrapping_add(0x1fe)).map_err(|_| Decline::Fault)? as i16;
        e.edx = (i32::from(levels) - 1) as u32;
        slot = if e.edx != u32::MAX { 0 } else { u32::MAX };
    }
    e.eax = u32::from(slot as u16 != 0xffff);
    Ok(())
}

/// What `100880` leaves: `al` (in `found`), `ecx` and `edx`.
struct Magazine {
    found: bool,
    ecx: u32,
    edx: u32,
}

/// `100880 (weapon: eax, only: ebx) -> al`: a magazine (`[def + 2c0]` of
/// them, each 16 bytes at `object + 224`) holding 1 to 6 rounds; `only`
/// not `-1` tests that index alone. `ecx` is the index it stopped at,
/// `edx` the last rounds read, or the definition when there are none.
fn magazine(
    m: &impl Memory,
    object: u32,
    def: u32,
    only: u32,
) -> std::result::Result<Magazine, Decline> {
    let word = |a: u32| m.u32(a).map_err(|_| Decline::Fault);
    let count = word(def.wrapping_add(0x2c0))? as i32;
    if count > MAGAZINES_MAX {
        return Err(Decline::Fault);
    }
    let mut out = Magazine { found: false, ecx: 0, edx: def };
    let mut i = 0i32;
    while i < count {
        if only == u32::MAX || only == i as u32 {
            let at = ((i as u16 as i16 as i32) << 4) as u32;
            let rounds = m
                .u16(at.wrapping_add(object).wrapping_add(0x224))
                .map_err(|_| Decline::Fault)? as i16;
            out.edx = i32::from(rounds) as u32;
            if (1..=6).contains(&rounds) {
                out.found = true;
                out.ecx = i as u32;
                return Ok(out);
            }
        }
        i += 1;
        out.ecx = i as u32;
    }
    Ok(out)
}

/// `187b30`: the input record (`ecx`) and the port's block (`eax`) into
/// the control globals: the held-flags word `+4`, the latches `+0xc` and
/// the record's own latch bits, the yaw's sign and the axis clamps.
fn submit(c: &CpuAbi, m: &impl Memory, e: &mut Effect) -> std::result::Result<(), Decline> {
    let word = |a: u32| m.u32(a).map_err(|_| Decline::Fault);
    let byte = |a: u32| m.u8(a).map_err(|_| Decline::Fault);
    let (rec, block) = (c.ecx, c.eax);
    let ctrl = word(CONTROL)?;
    // The globals, the record and the block must not overlap: the order
    // of the stores would matter.
    let overlap = |a: u32, n: u32, b: u32, k: u32| {
        let (a, b) = (u64::from(a), u64::from(b));
        a < b + u64::from(k) && b < a + u64::from(n)
    };
    if overlap(ctrl, 0x10, rec, 0x28) || overlap(block.wrapping_add(8), 2, ctrl, 0x10) {
        return Err(Decline::Fault);
    }
    // Only `comiss` against zero: it raises nothing on the operands
    // `check_compared` admits, so PE need not be sticky.
    if !det::guest_env(c.mxcsr) || !det::host_fast_env() {
        return Err(Decline::Float);
    }
    let mut fp = Fp::new();
    let mut held = word(ctrl.wrapping_add(4))?;
    let r18 = word(rec.wrapping_add(0x18))?;
    let mut r1c = word(rec.wrapping_add(0x1c))?;
    let b20 = byte(rec.wrapping_add(0x20))?;
    for (on, bit) in [
        (b20 & 4 != 0, 1),
        (r18 & 2 != 0, 2),
        (r18 & 0x400_0000 != 0, 0x20),
        (r18 & 4 != 0, 0x10_0000),
        (r18 & 0x20 != 0, 0x40),
        (r1c & 0x10 != 0, 4),
        (r1c & 0x20 != 0, 8),
        (r1c & 4 != 0, 0x200),
        (r1c & 1 != 0, 0x80),
        (r1c & 2 != 0, 0x100),
    ] {
        if on {
            held |= bit;
        }
    }
    let sign = |fp: &mut Fp, v: u32| {
        fp.check_compared(v);
        det::compare(v, 0)
    };
    use std::cmp::Ordering::{Greater, Less};
    let v8 = word(rec.wrapping_add(8))?;
    let mut xmm1 = v8;
    if sign(&mut fp, v8) == Greater {
        held |= 0x10;
    }
    let mut edx = 0x20u32;
    for (o, bit) in [(8, 0x4_0000), (9, 0x8_0000)] {
        let b = byte(block.wrapping_add(o))?;
        edx = u32::from(b);
        if b != 0 {
            held |= bit;
        }
    }
    for (o, up, down) in [
        (0x14, 0x400, 0x800),
        (0x10, 0x1000, 0x2000),
        (0, 0x4000, 0x8000),
        (4, 0x1_0000, 0x2_0000),
        (0x24, 0x80_0000, 0x100_0000),
    ] {
        let v = word(rec.wrapping_add(o))?;
        xmm1 = v;
        match sign(&mut fp, v) {
            Greater => {
                held |= up;
                edx = held;
            }
            Less => {
                held |= down;
                edx = held;
            }
            _ => {}
        }
    }
    // 0x187c9b: the latches.
    let g8 = word(ctrl.wrapping_add(8))?;
    let mut gc = word(ctrl.wrapping_add(0xc))?;
    let mut r20 = word(rec.wrapping_add(0x20))?;
    let latch = |edx: &mut u32, gc: &mut u32, on: bool, bit: u32| {
        *gc = if on { *gc | bit } else { *gc & !bit };
        *edx = *gc;
    };
    let mut write20 = g8 & 1 != 0;
    if !write20 && gc & 1 != 0 {
        latch(&mut edx, &mut gc, (b20 >> 2) & 1 != 0, 1);
        write20 = true;
    }
    if write20 {
        edx = (r20 & 0xfff9) | 8;
        r20 = (r20 & 0xffff_0000) | edx;
    }
    for (shift, rbit, gbit, clear) in [(2u32, 4u32, 4u32, !0x10u32), (3, 5, 8, !0x20)] {
        edx = g8 >> shift;
        let mut clear_bit = edx & 1 != 0;
        if !clear_bit {
            edx = gc >> shift;
            if edx & 1 != 0 {
                latch(&mut edx, &mut gc, (r1c >> rbit) & 1 != 0, gbit);
                clear_bit = true;
            }
        }
        if clear_bit {
            r1c &= clear;
        }
    }
    // 0x187d39: the yaw's sign, on the x87.
    let mut yaw = word(rec.wrapping_add(0x14))?;
    let mut x87 = None;
    edx = held >> 0x15;
    let negate = if edx & 1 != 0 {
        Some(false)
    } else {
        edx = held >> 0x16;
        (edx & 1 != 0).then_some(true)
    };
    if let Some(negate) = negate {
        let mut x = load_fpu(c);
        let f = |r: std::result::Result<(), crate::light_constants::Error>| {
            r.map_err(|_| Decline::Float)
        };
        f(x.fld(yaw))?;
        f(x.unary(unary_op::ABS))?;
        if negate {
            f(x.unary(unary_op::CHS))?;
        }
        yaw = x.fstp_m32().map_err(|_| Decline::Float)?;
        x87 = Some((
            X87State {
                st: x.0.st,
                tag: x.0.tag,
                status: x.0.status,
            },
            0x0018_7d5e,
            rec.wrapping_add(0x14),
        ));
    }
    // 0x187d61: the clamps, each on `gc`'s bit.
    let mut pitch = word(rec.wrapping_add(0x10))?;
    let mut xmm0 = 0u32;
    for (shift, is_yaw, keep_positive) in [
        (0xc, false, false),
        (0xd, false, true),
        (0xa, true, false),
        (0xb, true, true),
    ] {
        // The last test shifts `eax`, not `edx`.
        if shift != 0xb {
            edx = gc >> shift;
        }
        if (gc >> shift) & 1 == 0 {
            continue;
        }
        let v = if is_yaw { yaw } else { pitch };
        let positive = sign(&mut fp, v) == Greater;
        let kept = if positive == keep_positive { v } else { 0 };
        if shift == 0xb {
            // `movaps xmm0, xmm1` only for a positive yaw.
            xmm1 = v;
            if positive {
                xmm0 = v;
            }
        } else {
            xmm1 = kept;
        }
        if is_yaw { yaw = kept } else { pitch = kept }
    }
    if !fp.exact() {
        return Err(Decline::Float);
    }
    e.eax = gc >> 0xb;
    e.edx = edx;
    e.xmm0 = u128::from(xmm0).to_le_bytes();
    e.xmm1 = u128::from(xmm1).to_le_bytes();
    e.x87 = x87;
    e.writes = [
        Some((ctrl.wrapping_add(4), held)),
        Some((ctrl.wrapping_add(0xc), gc)),
        Some((rec.wrapping_add(0x1c), r1c)),
        Some((rec.wrapping_add(0x20), r20)),
        Some((rec.wrapping_add(0x14), yaw)),
        Some((rec.wrapping_add(0x10), pitch)),
    ];
    Ok(())
}

/// `c6ef0 (packet: edx)`: zero the `0x7c`-byte unit-control packet, then
/// its header word, the `-1` selections at `+6..+0xe` and words
/// `+0x58..+0x64`, and the vector at `[PACKET_VECTOR]` into each of the
/// three directions at `+0x28`, `+0x34` and `+0x40`. It leaves `eax` the
/// vector's third word, `ecx` `-1` and `xmm0` zero (`xorps`); `ebx`, `ebp`
/// and `edi` come back saved. Only the dead frame below the return is not
/// laid down.
fn init_packet(c: &CpuAbi, m: &impl Memory, e: &mut Effect) -> std::result::Result<(), Decline> {
    let word = |a: u32| m.u32(a).map_err(|_| Decline::Fault);
    // `rep stosd` runs backwards under a set direction flag.
    if c.eflags & DIRECTION_FLAG != 0 {
        return Err(Decline::Float);
    }
    let packet = c.edx;
    let source = word(PACKET_VECTOR)?;
    // The guest reads the pointer and the vector after its first stores:
    // either inside the packet would see them.
    disjoint(m, (packet, BLOCK), (PACKET_VECTOR, 4))?;
    disjoint(m, (packet, BLOCK), (source, 12))?;
    let v = [word(source)?, word(source.wrapping_add(4))?, word(source.wrapping_add(8))?];
    // The stores must take: check the whole packet before predicting it.
    let mut bytes = [0u8; BLOCK];
    m.read_bytes(packet, &mut bytes, 1).map_err(|_| Decline::Fault)?;
    bytes = [0; BLOCK];
    bytes[..4].copy_from_slice(&PACKET_HEADER.to_le_bytes());
    bytes[6..0xe].fill(0xff);
    bytes[0x58..0x64].fill(0xff);
    for at in [0x28, 0x34, 0x40] {
        for (i, w) in v.iter().enumerate() {
            bytes[at + 4 * i..][..4].copy_from_slice(&w.to_le_bytes());
        }
    }
    e.eax = v[2];
    e.ecx = u32::MAX;
    e.xmm0 = [0; 16];
    e.block = Some(Block { addr: packet, bytes });
    Ok(())
}

/// Two guest ranges that share no physical byte.
fn disjoint(
    m: &impl Memory,
    a: (u32, usize),
    b: (u32, usize),
) -> std::result::Result<(), Decline> {
    // One lookup per page: either range can cross page mappings.
    let x = crate::sim_native::spans::Spans::of(m, a.0, a.1).ok_or(Decline::Fault)?;
    let y = crate::sim_native::spans::Spans::of(m, b.0, b.1).ok_or(Decline::Fault)?;
    if x.overlaps(&y) {
        return Err(Decline::Fault);
    }
    Ok(())
}
