//! The local action builder `0x186ab0` (docs/perf/sim-phantom-build.md,
//! "The builder body in Rust"): one local user's control update, the
//! dialog and input-block tests, the unit's weapon and grenade slots, the
//! zoom and flashlight latches, the interaction search, the facing of a
//! unit whose controller asked for it, and the 0x5c-byte action at `out`.
//!
//! A register-level body in the style of [`super::look`]: each segment
//! runs from the entry or a call's return site to the next call or the
//! return, buffering its stores in a plan, and leaves every register,
//! flag, x87 and SSE state the original leaves there. A segment that
//! cannot take its shape declines and the original runs from the same
//! point, with nothing written:
//!
//! - the direction flag set (the `rep stos` and `rep movs` copies);
//! - a float environment [`crate::sim_native::supported`] refuses, or an
//!   x87 step the x87 crate stops on (the facing's `fpatan` under a
//!   directed rounding mode);
//! - the grenade slot walk past [`SLOT_WALK`] steps (a slot count no tag
//!   has, on which the original spins);
//! - a read or write that faults.
//!
//! The callees stay the guest's, made in place by the caller's driver: the
//! control update `0x185be0` (a seat of its own, [`super::look`]), the
//! dialog test `0x148fff`, the unit queries, the interaction search
//! `0x151110` ([`super::interaction`]), `fmod` and the action's copy.
use crate::light_constants::X87;
use crate::sim_native::{Memory, Result, plan::segment, sub_flags};
use crate::sim_objects::sse::{self, lane0, load};
use crate::sim_physics::{
    ARITH, arithmetic, byte, logic, pop, push,
    spatial::{finish_fpu, fp_error, load_fpu},
};
use runtime::{CpuAbi, Guest, GuestFault, TransferAbi};
use x87::abi::{arith_op, unary_op};

pub const BUILD: u32 = 0x0018_6ab0;
/// The return sites of the body's calls, sorted.
pub const SITES: &[u32] = &[
    0x186b1b, 0x186b24, 0x186b2b, 0x186c05, 0x186c18, 0x186c9c, 0x186d2f, 0x186d4a,
    0x186d70, 0x186e03, 0x186f0f, 0x186feb, 0x187002, 0x18701d,
];
const TEXT: std::ops::Range<u32> = BUILD..0x0018_709b;

/// The callees, in the body's order.
pub const CONTROL_UPDATE: u32 = super::look::UPDATE;
pub const DIALOG_TEST: u32 = 0x0014_8fff;
pub const INPUT_BLOCKED: u32 = 0x0012_be90;
pub const WEAPON_SELECT: u32 = 0x000c_deb0;
pub const WEAPON_INDEX: u32 = 0x000c_fe40;
pub const GRENADE_TEST: u32 = 0x000c_dff0;
pub const CAMERA_MODE: u32 = 0x0015_5760;
pub const ZOOM_ALLOWED: u32 = 0x000c_85c0;
pub const ZOOM_NEXT: u32 = 0x000c_8960;
pub const SCRIPT_LOOK: u32 = 0x0018_7510;
pub const INTERACTION: u32 = super::interaction::SEARCH;
pub const FMOD: u32 = 0x0037_3b6a;
pub const ACTION_COPY: u32 = 0x0014_d040;

/// The grenade slot walk's bound: the original walks until it comes back
/// to the slot it started from, which a slot outside 0 and 1 never does
/// when neither of those holds a grenade.
pub const SLOT_WALK: u32 = 0x1_0000;

const PLAYERS: u32 = 0x004e_8c24;
const CONTROL_GLOBALS: u32 = 0x004e_d284;
const SETTINGS: u32 = 0x004e_034c;
const OBJECT_HEADER: u32 = 0x004e_0300;
const USERS: u32 = 0x004e_8c99;
const SLOWDOWN: u32 = 0x0051_0c50;
const PAUSE: u32 = 0x0051_0c54;
const SIM_GLOBALS: u32 = 0x004e_6948;
const PROFILE_FLAGS: u32 = 0x0054_e8e0;
const PROFILES: u32 = 0x0054_e9f4;
const PROFILE_STRIDE: u32 = 0xc70;
const ZOOM_THRESHOLD: u32 = 0x0045_dbbc;
const TINY: u32 = 0x0045_dbdc;
const TWO_PI: u32 = 0x0043_f17c;
const PI: u32 = 0x0045_e3a0;
const FULL_TURN: u32 = 0x0045_e088;
const HALF_TURN: u32 = 0x0045_dddc;
const DF: u32 = 1 << 10;

fn at(base: u32, offset: u32) -> u32 {
    base.wrapping_add(offset)
}

fn low16(word: u32, v: u16) -> u32 {
    (word & !0xffff) | u32::from(v)
}

fn sx8(v: u8) -> u32 {
    v as i8 as i32 as u32
}

fn sx16(v: u16) -> u32 {
    v as i16 as i32 as u32
}

fn logic8(c: &mut CpuAbi, v: u8) {
    logic(c, u32::from(v), 0x80);
}

fn logic16(c: &mut CpuAbi, v: u16) {
    logic(c, u32::from(v), 0x8000);
}

fn logic32(c: &mut CpuAbi, v: u32) {
    logic(c, v, 1 << 31);
}

// Hand the point back to the original with nothing written.
fn declined(pc: u32) -> GuestFault {
    crate::sim_native::refuse(pc, crate::sim_native::Decline::Boundary)
}

type Unit = std::result::Result<(), GuestFault>;

fn forward(c: &CpuAbi, pc: u32) -> Unit {
    if c.eflags & DF != 0 { Err(crate::sim_native::refuse(pc, crate::sim_native::Decline::Direction)) } else { Ok(()) }
}

/// `rep stosd` / `rep movsd` with the direction flag clear: `ecx` words.
fn rep_stos(c: &mut CpuAbi, m: &mut impl Memory) -> Unit {
    while c.ecx != 0 {
        m.set_u32(c.edi, c.eax)?;
        c.edi = at(c.edi, 4);
        c.ecx -= 1;
    }
    Ok(())
}

fn rep_movs(c: &mut CpuAbi, m: &mut impl Memory) -> Unit {
    while c.ecx != 0 {
        let v = m.u32(c.esi)?;
        m.set_u32(c.edi, v)?;
        c.edi = at(c.edi, 4);
        c.esi = at(c.esi, 4);
        c.ecx -= 1;
    }
    Ok(())
}

/// The object a handle names: `[[0x4e0300] + 0x44] + (handle & 0xffff) * 12 + 8`,
/// with `eax` and the index registers as the original leaves them.
fn object_index(c: &mut CpuAbi, handle: u32) -> u32 {
    let index = handle & 0xffff;
    logic32(c, index);
    index.wrapping_mul(3)
}

/// One body segment.
pub fn run(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> std::result::Result<Option<TransferAbi>, GuestFault> {
    segment::<64, _>(c, g, pc, TEXT, SITES, |c, p, pc| step(c, p, pc))
}

fn step(c: &mut CpuAbi, m: &mut impl Memory, pc: u32) -> Result {
    let sp = c.esp;
    match pc {
        BUILD => enter(c, m),
        0x186b1b => {
            c.eax = m.u32(at(c.esi, 0x24))?;
            push(c, m, c.eax)?;
            m.call(c, DIALOG_TEST, 0x186b24)
        }
        0x186b24 => {
            c.edx = byte(c.edx, c.eax as u8);
            m.call(c, INPUT_BLOCKED, 0x186b2b)
        }
        0x186b2b => blocked(c, m, sp),
        0x186c05 => {
            c.ebx = m.u32(OBJECT_HEADER)?;
            m.set_u8(at(c.ebp, 0x2a), c.eax as u8)?;
            weapon_index(c, m)
        }
        0x186c18 => after_weapon_index(c, m, sp),
        0x186c9c => {
            let ax = c.eax as u16;
            logic16(c, ax);
            if ax != 0 { pause(c, m, sp) } else { grenade_walk(c, m, sp) }
        }
        0x186d2f => after_camera(c, m, sp),
        0x186d4a => after_zoom_allowed(c, m, sp),
        0x186d70 => {
            let ax = c.eax as u16;
            sub_flags(c, u32::from(ax), c.edi, 16);
            m.set_u16(at(c.ebp, 0x2e), ax)?;
            if ax != c.edi as u16 {
                m.set_u8(at(c.ebp, 0x89), 1)?;
                m.set_u16(at(c.ebp, 0x8c), 0)?;
            }
            script_look(c, m, sp)
        }
        0x186e03 => flashlight(c, m, sp),
        0x186f0f => publish(c, m, sp),
        0x186feb => {
            let mut x = load_fpu(c);
            let yaw = x.fstp_m32().map_err(fp_error)?;
            m.set_u32(at(c.ebp, 0x10), yaw)?;
            x.fld(m.u32(at(c.ebp, 0x14))?).map_err(fp_error)?;
            x.fadd_m(m.u32(PI)?).map_err(fp_error)?;
            X87::ok(x87::abi::ld_f64(&mut x.0, m.u64(FULL_TURN)?)).map_err(fp_error)?;
            finish_fpu(c, m, x, 0x186ff7, FULL_TURN)?;
            m.call(c, FMOD, 0x187002)
        }
        0x187002 => {
            let mut x = load_fpu(c);
            x.arith_m(arith_op::SUB, m.u32(HALF_TURN)?).map_err(fp_error)?;
            let pitch = x.fstp_m32().map_err(fp_error)?;
            let slot = at(c.ebp, 0x14);
            m.set_u32(slot, pitch)?;
            finish_fpu(c, m, x, 0x187008, slot)?;
            let flags = at(c.esi, 0x348);
            let v = m.u8(flags)? & 0xdf;
            m.set_u8(flags, v)?;
            logic8(c, v);
            copy_action(c, m, sp)
        }
        0x18701d => finish(c, m, sp),
        _ => Err(declined(pc)),
    }
}

/// `0x186ab0`..`0x186b16`: the frame, the user's control block in `ebp`,
/// then the control update with the action at `esp + 0x18`.
fn enter(c: &mut CpuAbi, m: &mut impl Memory) -> Result {
    c.esp = c.esp.wrapping_sub(0x54);
    c.eax = m.u32(PLAYERS)?;
    push(c, m, c.ebx)?;
    push(c, m, c.ebp)?;
    push(c, m, c.esi)?;
    c.esi = m.u32(at(c.esp, 0x64))?;
    c.edx = m.u32(CONTROL_GLOBALS)?;
    c.esi = (c.esi & 0xffff).wrapping_mul(0x21c);
    push(c, m, c.edi)?;
    let sp = c.esp;
    c.edi = m.u32(at(c.eax, 0x44))?;
    c.eax = sx16(m.u16(at(at(c.esi, c.edi), 0x28))?);
    c.esi = c.esi.wrapping_add(c.edi);
    c.ecx = c.eax.wrapping_mul(0x94);
    c.ebp = at(at(c.ecx, c.edx), 0x14);
    c.ecx = m.u32(SETTINGS)?;
    c.edx = m.u32(at(c.ecx, 0xf4))?;
    m.set_u32(at(sp, 0x14), c.edx)?;
    c.ecx = at(sp, 0x18);
    push(c, m, c.ecx)?;
    c.ecx = m.u32(at(c.esp, 0x74))?;
    c.edx = at(c.esp, 0x14);
    push(c, m, c.edx)?;
    c.edx = m.u32(at(c.esp, 0x74))?;
    push(c, m, c.ecx)?;
    push(c, m, c.edx)?;
    c.edx = m.u32(at(c.esi, 0x24))?;
    c.ebx = 0;
    logic32(c, 0);
    push(c, m, c.eax)?;
    m.set_u32(at(c.esp, 0x24), c.ebx)?;
    m.call(c, CONTROL_UPDATE, 0x186b1b)
}

/// `0x186b2b`: input blocked (a dialog, the console, the user's director)
/// clears the action but its zoom bit; then the unit's weapon state.
fn blocked(c: &mut CpuAbi, m: &mut impl Memory, sp: u32) -> Result {
    let dl = (c.edx as u8) | (c.eax as u8);
    c.edx = byte(c.edx, dl);
    logic8(c, dl);
    c.eax = sx16(m.u16(at(c.esi, 0x28))?).wrapping_mul(5) << 6;
    let dl = dl | m.u8(at(c.eax, 0x004e_8c9a))?;
    c.edx = byte(c.edx, dl);
    logic8(c, dl);
    if dl == 0 {
        c.eax = u32::MAX;
        logic32(c, c.eax);
    } else {
        forward(c, 0x186b2b)?;
        let cl = m.u8(at(sp, 0x39))?;
        sse::zero(c, 0);
        let cl = cl & 1;
        c.ecx = byte(c.ecx, cl);
        logic8(c, cl);
        m.set_u8(at(sp, 0x70), cl)?;
        c.eax = c.ebx;
        c.ecx = 0x13;
        c.edi = at(sp, 0x18);
        rep_stos(c, m)?;
        c.eax = u32::MAX;
        let zero = lane0(c, 0);
        m.set_u32(at(sp, 0x5c), zero)?;
        m.set_u32(at(sp, 0x60), zero)?;
        for offset in [0x40, 0x44, 0x48] {
            m.set_u32(at(sp, offset), c.eax)?;
        }
        m.set_u16(at(sp, 0x58), c.ebx as u16)?;
        let bits = at(sp, 0x39);
        let v = if cl == 0 { m.u8(bits)? & 0xfe } else { m.u8(bits)? | 1 };
        m.set_u8(bits, v)?;
        logic8(c, v);
    }
    // 0x186b8f
    c.edx = m.u32(c.ebp)?;
    sub_flags(c, c.edx, c.eax, 32);
    if c.edx == c.eax {
        return interaction(c, m, sp);
    }
    c.ebx = m.u32(OBJECT_HEADER)?;
    c.ecx = m.u32(at(c.ebx, 0x44))?;
    c.eax = object_index(c, c.edx);
    c.edi = m.u32(at(at(c.ecx, c.eax.wrapping_mul(4)), 8))?;
    let ax = m.u16(at(c.ebp, 0x28))?;
    c.eax = low16(c.eax, ax);
    let weapon = m.u16(at(c.edi, 0x214))?;
    sub_flags(c, u32::from(ax), u32::from(weapon), 16);
    m.set_u32(at(sp, 0x70), c.edi)?;
    if ax != weapon {
        c.ecx = m.u32(at(c.edi, 0x214))?;
        m.set_u32(at(c.ebp, 0x28), c.ecx)?;
    }
    // 0x186bcb
    let al = m.u8(at(sp, 0x34))?;
    let cl = m.u8(at(c.ebp, 0x2a))?;
    c.ecx = byte(c.ecx, cl);
    let al = al & 1;
    c.eax = byte(c.eax, al);
    logic8(c, al);
    let held = m.u8(at(c.edi, 0x213))?;
    sub_flags(c, u32::from(held), 0xff, 8);
    if held != 0xff {
        c.eax = byte(c.eax, 0);
        logic8(c, 0);
    }
    sub_flags(c, u32::from(cl), 0xff, 8);
    if cl == 0xff {
        c.ecx = byte(c.ecx, m.u8(at(c.edi, 0x212))?);
    }
    // 0x186bea
    let al = c.eax as u8;
    let cl = c.ecx as u8;
    logic8(c, al);
    if al == 0 {
        sub_flags(c, u32::from(cl), 0xff, 8);
        if cl != 0xff {
            return weapon_index(c, m);
        }
    }
    // 0x186bf3
    c.eax = low16(c.eax, u16::from(al));
    c.ecx = low16(c.ecx, cl as i8 as i16 as u16);
    push(c, m, c.eax)?;
    push(c, m, c.ecx)?;
    push(c, m, u32::MAX)?;
    push(c, m, c.edx)?;
    m.call(c, WEAPON_SELECT, 0x186c05)
}

/// `0x186c0e`.
fn weapon_index(c: &mut CpuAbi, m: &mut impl Memory) -> Result {
    c.edi = m.u32(c.ebp)?;
    c.ecx = c.edi;
    m.call(c, WEAPON_INDEX, 0x186c18)
}

/// `0x186c18`: the unit's weapon slot against the block's, then its
/// grenade slot.
fn after_weapon_index(c: &mut CpuAbi, m: &mut impl Memory, sp: u32) -> Result {
    let ax = c.eax as u16;
    sub_flags(c, u32::from(ax), 0xffff, 16);
    if ax != 0xffff {
        c.edx = m.u32(at(sp, 0x70))?;
        let cx = m.u8(at(c.edx, 0x212))? as i8 as i16 as u16;
        c.ecx = low16(c.ecx, cx);
        sub_flags(c, u32::from(cx), u32::from(ax), 16);
        if cx != ax {
            m.set_u8(at(c.ebp, 0x2a), ax as u8)?;
            m.set_u8(at(c.ebp, 0x2b), 0xff)?;
        }
    }
    // 0x186c36
    let al = m.u8(at(c.ebp, 0x2a))?;
    c.eax = byte(c.eax, al);
    let next = m.u8(at(c.ebp, 0x2b))?;
    sub_flags(c, u32::from(al), u32::from(next), 8);
    if al == next {
        sub_flags(c, u32::from(al), 0xff, 8);
        if al != 0xff {
            m.set_u8(at(c.ebp, 0x2b), 0xff)?;
        }
    }
    // 0x186c46
    let cx = m.u16(at(c.ebp, 0x2c))?;
    c.ecx = low16(c.ecx, cx);
    sub_flags(c, u32::from(cx), 0xffff, 16);
    let mut held = false;
    if cx != 0xffff {
        c.eax = object_index(c, c.edi);
        c.edx = c.eax;
        c.eax = m.u32(at(c.ebx, 0x44))?;
        c.edx = m.u32(at(at(c.eax, c.edx.wrapping_mul(4)), 8))?;
        c.ecx = sx16(cx);
        let count = m.u8(at(at(c.edx, c.ecx), 0x23e))? as i8 as i16 as u16;
        c.eax = low16(c.eax, count);
        logic16(c, count);
        held = count != 0;
    }
    if !held {
        // 0x186c72
        c.eax = m.u32(at(sp, 0x70))?;
        let slot = m.u8(at(c.eax, 0x23d))? as i8 as i16 as u16;
        c.ecx = low16(c.ecx, slot);
        m.set_u16(at(c.ebp, 0x2c), slot)?;
    }
    // 0x186c82
    let wanted = m.u8(at(sp, 0x34))? & 2;
    logic8(c, wanted);
    if wanted == 0 {
        c.ecx = 0;
        logic32(c, 0);
        let cx = m.u16(at(c.ebp, 0x2c))?;
        c.ecx = u32::from(cx);
        sub_flags(c, c.ecx, 0xffff, 16);
        if cx != 0xffff {
            c.eax = c.edi;
            return m.call(c, GRENADE_TEST, 0x186c9c);
        }
    }
    grenade_walk(c, m, sp)
}

/// `0x186ca1`: the next grenade slot with a grenade, from the block's.
fn grenade_walk(c: &mut CpuAbi, m: &mut impl Memory, sp: u32) -> Result {
    let cx = m.u16(at(c.ebp, 0x2c))?;
    c.ecx = low16(c.ecx, cx);
    c.eax = object_index(c, c.edi);
    c.edx = c.eax;
    c.eax = m.u32(at(c.ebx, 0x44))?;
    c.edi = m.u32(at(at(c.eax, c.edx.wrapping_mul(4)), 8))?;
    c.ebx = u32::MAX;
    logic32(c, c.ebx);
    sub_flags(c, u32::from(cx), 0xffff, 16);
    if cx == 0xffff {
        c.ecx = 0;
        logic32(c, 0);
    }
    c.eax = c.ecx;
    let mut steps = 0;
    loop {
        // 0x186cc3
        c.edx = sx16(c.eax as u16);
        let count = m.u8(at(at(c.edi, c.edx), 0x23e))?;
        sub_flags(c, u32::from(count), 0, 8);
        if (count as i8) > 0 {
            sub_flags(c, c.eax, c.ecx, 16);
            c.ebx = c.eax;
            if c.eax as u16 != c.ecx as u16 {
                break;
            }
        }
        // 0x186cd7
        sub_flags(c, c.eax, 1, 16);
        if c.eax as u16 == 1 {
            c.eax = 0;
            logic32(c, 0);
        } else {
            c.eax = c.edx.wrapping_add(1);
        }
        // 0x186ce4
        sub_flags(c, c.eax, c.ecx, 16);
        if c.eax as u16 == c.ecx as u16 {
            break;
        }
        steps += 1;
        if steps >= SLOT_WALK {
            return Err(crate::sim_native::unported(0x186cc3, "the weapon slot walk passes its bound"));
        }
    }
    // 0x186ce9
    m.set_u16(at(c.ebp, 0x2c), c.ebx as u16)?;
    pause(c, m, sp)
}

/// `0x186ced`: no zoom while the game is slowed, the console is up or
/// the tick is paused; else the camera mode.
fn pause(c: &mut CpuAbi, m: &mut impl Memory, sp: u32) -> Result {
    c.eax = m.u32(SLOWDOWN)?;
    logic32(c, c.eax);
    if c.eax != 0 {
        let cl = m.u8(at(c.eax, 5))?;
        c.ecx = byte(c.ecx, cl);
        logic8(c, cl);
        if cl != 0 {
            return no_zoom(c, m, sp);
        }
    }
    // 0x186d01
    c.ecx = m.u32(CONTROL_GLOBALS)?;
    let console = m.u8(at(c.ecx, 0x10))? & 1;
    logic8(c, console);
    if console != 0 {
        return no_zoom(c, m, sp);
    }
    c.eax = m.u32(PAUSE)?;
    let paused = m.u8(c.eax)?;
    sub_flags(c, u32::from(paused), 0, 8);
    if paused != 0 {
        let cl = m.u8(at(c.eax, 1))?;
        c.ecx = byte(c.ecx, cl);
        logic8(c, cl);
        if cl != 0 {
            return no_zoom(c, m, sp);
        }
    }
    // 0x186d26
    c.eax = sx16(m.u16(at(c.esi, 0x28))?);
    m.call(c, CAMERA_MODE, 0x186d2f)
}

/// `0x186d2f`.
fn after_camera(c: &mut CpuAbi, m: &mut impl Memory, sp: u32) -> Result {
    logic32(c, c.eax);
    if c.eax != 0 {
        return no_zoom(c, m, sp);
    }
    c.eax = m.u32(c.ebp)?;
    c.edi = u32::MAX;
    logic32(c, c.edi);
    sub_flags(c, c.eax, c.edi, 32);
    if c.eax == c.edi {
        return zoom_cleared(c, m, sp);
    }
    m.call(c, ZOOM_ALLOWED, 0x186d4a)
}

/// `0x186d4a`: the zoom button steps the zoom; held, it counts towards
/// the reset.
fn after_zoom_allowed(c: &mut CpuAbi, m: &mut impl Memory, sp: u32) -> Result {
    let al = c.eax as u8;
    logic8(c, al);
    if al == 0 {
        return zoom_cleared(c, m, sp);
    }
    let bl = m.u8(at(sp, 0x34))?;
    c.ebx = byte(c.ebx, bl);
    let pressed = bl & 4;
    logic8(c, pressed);
    if pressed != 0 {
        c.eax = m.u32(c.ebp)?;
        c.ecx = 0;
        logic32(c, 0);
        let cx = m.u16(at(c.ebp, 0x2e))?;
        c.ecx = u32::from(cx);
        m.set_u16(at(c.ebp, 0x8a), cx)?;
        return m.call(c, ZOOM_NEXT, 0x186d70);
    }
    // 0x186d8b
    let held = bl & 8;
    logic8(c, held);
    let al = m.u8(at(c.ebp, 0x89))?;
    c.eax = byte(c.eax, al);
    if held != 0 {
        logic8(c, al);
        if al == 0 {
            return script_look(c, m, sp);
        }
        c.eax = 0;
        logic32(c, 0);
        let ax = m.u16(at(c.ebp, 0x8c))?;
        c.eax = u32::from(ax);
        sub_flags(c, c.eax, 0x7f, 16);
        if (ax as i16) >= 0x7f {
            return script_look(c, m, sp);
        }
        // `inc eax`: its flags are replaced at 0x186df0 before any transfer.
        c.eax = c.eax.wrapping_add(1);
        m.set_u16(at(c.ebp, 0x8c), c.eax as u16)?;
        return script_look(c, m, sp);
    }
    // 0x186db3
    logic8(c, al);
    if al != 0 {
        let ticks = m.u16(at(c.ebp, 0x8c))?;
        sub_flags(c, u32::from(ticks), 0xf, 16);
        if (ticks as i16) > 0xf {
            let dx = m.u16(at(c.ebp, 0x8a))?;
            c.edx = low16(c.edx, dx);
            m.set_u16(at(c.ebp, 0x2e), dx)?;
        }
    }
    zoom_latch_off(c, m, sp)
}

/// `0x186dce`.
fn no_zoom(c: &mut CpuAbi, m: &mut impl Memory, sp: u32) -> Result {
    c.edi = u32::MAX;
    logic32(c, c.edi);
    zoom_cleared(c, m, sp)
}

/// `0x186dd1`.
fn zoom_cleared(c: &mut CpuAbi, m: &mut impl Memory, sp: u32) -> Result {
    c.ebx = byte(c.ebx, m.u8(at(sp, 0x34))?);
    m.set_u16(at(c.ebp, 0x2e), c.edi as u16)?;
    zoom_latch_off(c, m, sp)
}

/// `0x186dd9`.
fn zoom_latch_off(c: &mut CpuAbi, m: &mut impl Memory, sp: u32) -> Result {
    m.set_u8(at(c.ebp, 0x89), 0)?;
    script_look(c, m, sp)
}

/// `0x186de0`: a user whose look a script does not hold reads it.
fn script_look(c: &mut CpuAbi, m: &mut impl Memory, sp: u32) -> Result {
    c.eax = sx16(m.u16(at(c.esi, 0x28))?);
    // `shl ecx, 6`: its flags are replaced by the `test` below.
    c.ecx = c.eax.wrapping_mul(5) << 6;
    let dl = m.u8(at(c.ecx, USERS))?;
    c.edx = byte(c.edx, dl);
    logic8(c, dl);
    if dl == 0 {
        c.edx = m.u32(at(sp, 0x2c))?;
        c.ecx = m.u32(at(sp, 0x28))?;
        push(c, m, c.edx)?;
        push(c, m, c.ecx)?;
        return m.call(c, SCRIPT_LOOK, 0x186e03);
    }
    flashlight(c, m, sp)
}

/// `0x186e03`: the flashlight's hold count, for a unit with no parent and
/// a profile that holds it.
fn flashlight(c: &mut CpuAbi, m: &mut impl Memory, sp: u32) -> Result {
    c.ecx = m.u32(at(sp, 0x70))?;
    let parent = m.u32(at(c.ecx, 0x14))?;
    sub_flags(c, parent, c.edi, 32);
    if parent != c.edi {
        m.set_u8(at(c.ebp, 0x30), 0)?;
    } else if !hold_count(c, m, sp)? {
        // 0x186e8d
        m.set_u8(at(c.ebp, 0x31), 0)?;
        m.set_u8(at(c.ebp, 0x30), 0)?;
    }
    zoom_toggle(c, m, sp)
}

/// `0x186e10`..`0x186e8b`: true when it set the hold count and its flag;
/// false at `0x186e8d`.
fn hold_count(c: &mut CpuAbi, m: &mut impl Memory, sp: u32) -> std::result::Result<bool, GuestFault> {
    c.esi = m.u32(at(c.esi, 0x24))?.wrapping_mul(PROFILE_STRIDE);
    // `shr edx, 4` and `shr eax, 3`: replaced by the tests after them.
    c.edx = m.u32(at(c.esi, PROFILE_FLAGS))? >> 4;
    let on = (c.edx as u8) & 1;
    logic8(c, on);
    if on == 0 {
        return Ok(false);
    }
    c.eax = m.u32(at(c.esi, PROFILES))? >> 3;
    let al = (c.eax as u8) & 1;
    c.eax = byte(c.eax, al);
    logic8(c, al);
    m.set_u8(at(sp, 0x70), al)?;
    if al == 0 {
        return Ok(false);
    }
    // fld [ebp+18h]; fabs; fld threshold; fxch; fcomip; fstp st(0).
    let mut x = load_fpu(c);
    x.fld(m.u32(at(c.ebp, 0x18))?).map_err(fp_error)?;
    x.unary(unary_op::ABS).map_err(fp_error)?;
    x.fld(m.u32(ZOOM_THRESHOLD)?).map_err(fp_error)?;
    x.fxch(1).map_err(fp_error)?;
    let flags = x.fcomip(1).map_err(fp_error)?;
    x.fstp_st(0).map_err(fp_error)?;
    finish_fpu(c, m, x, 0x186e47, ZOOM_THRESHOLD)?;
    c.eflags = c.eflags & !ARITH | flags;
    c.eflags_defined |= ARITH;
    if flags & 0x41 != 0 {
        return Ok(false);
    }
    load(c, 0, m.u32(TINY)?);
    let small = lane0(c, 0);
    if sse::comiss(c, small, m.u32(at(sp, 0x2c))?) & 0x41 != 0 {
        return Ok(false);
    }
    let al = m.u8(at(c.ebp, 0x68))?;
    c.eax = byte(c.eax, al);
    logic8(c, al);
    if al != 0 {
        return Ok(false);
    }
    // `inc eax`: replaced by the `xor` or the `cmp` after it.
    c.eax = sx8(m.u8(at(c.ebp, 0x31))?).wrapping_add(1);
    if (c.eax as i32) < 0 {
        c.eax = 0;
        logic32(c, 0);
    } else {
        sub_flags(c, c.eax, 0x7f, 32);
        if (c.eax as i32) > 0x7f {
            c.eax = 0x7f;
        }
    }
    // 0x186e76
    let dx = c.eax as u8 as i8 as i16 as u16;
    c.edx = low16(c.edx, dx);
    m.set_u8(at(c.ebp, 0x31), c.eax as u8)?;
    c.eax = m.u32(at(sp, 0x14))?;
    let limit = m.u16(at(c.eax, 0x6e))?;
    sub_flags(c, u32::from(dx), u32::from(limit), 16);
    let over = u8::from((dx as i16) > (limit as i16));
    c.edx = byte(c.edx, over);
    m.set_u8(at(c.ebp, 0x30), over)?;
    Ok(true)
}

/// `0x186e95`: the zoom toggle's latch, outside a film's playback.
fn zoom_toggle(c: &mut CpuAbi, m: &mut impl Memory, sp: u32) -> Result {
    c.eax = m.u32(SIM_GLOBALS)?;
    c.edx = at(c.eax, 8);
    logic32(c, c.edx);
    if c.edx == 0 {
        return interaction(c, m, sp);
    }
    let mode = m.u8(at(c.eax, 0xd))?;
    sub_flags(c, u32::from(mode), 3, 8);
    if mode == 3 {
        return interaction(c, m, sp);
    }
    let toggle = (c.ebx as u8) & 0x40;
    logic8(c, toggle);
    if toggle == 0 {
        // 0x18708f
        m.set_u8(at(c.ebp, 0x92), 0)?;
    } else {
        let al = m.u8(at(c.ebp, 0x92))?;
        c.eax = byte(c.eax, al);
        logic8(c, al);
        if al == 0 {
            let on = m.u8(at(c.ebp, 0x91))?;
            logic8(c, on);
            let flipped = u8::from(on == 0);
            c.eax = byte(c.eax, flipped);
            m.set_u8(at(c.ebp, 0x92), 1)?;
            m.set_u8(at(c.ebp, 0x91), flipped)?;
        }
    }
    // 0x186ed2
    let bits = m.u8(at(sp, 0x30))? & 0x60;
    logic8(c, bits);
    let off = if bits != 0 {
        true
    } else {
        let bits = m.u32(at(sp, 0x30))? & 0x0463_0200;
        logic32(c, bits);
        bits != 0
    };
    if off {
        m.set_u8(at(c.ebp, 0x91), 0)?;
    }
    // 0x186eea
    let al = m.u8(at(c.ebp, 0x91))?;
    c.eax = byte(c.eax, al);
    logic8(c, al);
    if al != 0 {
        let parent = m.u32(at(c.ecx, 0x14))?;
        sub_flags(c, parent, c.edi, 32);
        if parent == c.edi {
            let v = m.u32(at(sp, 0x30))? | 0x4000;
            m.set_u32(at(sp, 0x30), v)?;
            logic32(c, v);
        }
    }
    interaction(c, m, sp)
}

/// `0x186f01`: the interaction search into the block at `ebp + 0x6c`.
fn interaction(c: &mut CpuAbi, m: &mut impl Memory, sp: u32) -> Result {
    c.ecx = m.u32(at(sp, 0x68))?;
    c.ebx = at(c.ebp, 0x6c);
    push(c, m, c.ebx)?;
    push(c, m, c.ecx)?;
    m.call(c, INTERACTION, 0x186f0f)
}

/// `0x186f0f`: the action's halves into the control block, and the facing
/// of a unit whose controller asked for it (`+0x348` bit 5).
fn publish(c: &mut CpuAbi, m: &mut impl Memory, sp: u32) -> Result {
    forward(c, 0x186f0f)?;
    let ax = m.u16(at(sp, 0x38))?;
    c.eax = low16(c.eax, ax);
    c.edx = m.u32(at(sp, 0x30))?;
    load(c, 0, m.u32(at(sp, 0x20))?);
    c.edi = at(c.ebp, 0x44);
    c.ecx = 9;
    c.esi = at(sp, 0x40);
    m.set_u32(at(sp, 0x68), c.edi)?;
    m.set_u16(at(c.ebp, 8), ax)?;
    c.eax = m.u32(c.ebp)?;
    sub_flags(c, c.eax, u32::MAX, 32);
    rep_movs(c, m)?;
    c.ecx = m.u32(at(sp, 0x18))?;
    m.set_u32(at(c.ebp, 4), c.edx)?;
    c.edx = m.u32(at(sp, 0x1c))?;
    m.set_u32(at(c.ebp, 0x20), lane0(c, 0))?;
    load(c, 0, m.u32(at(sp, 0x24))?);
    m.set_u32(at(c.ebp, 0x18), c.ecx)?;
    m.set_u32(at(c.ebp, 0x1c), c.edx)?;
    m.set_u32(at(c.ebp, 0x24), lane0(c, 0))?;
    if c.eax == u32::MAX {
        return copy_action(c, m, sp);
    }
    c.ecx = m.u32(OBJECT_HEADER)?;
    c.edx = m.u32(at(c.ecx, 0x44))?;
    c.eax = object_index(c, c.eax);
    let entry = at(c.edx, c.eax.wrapping_mul(4));
    let cl = m.u8(at(entry, 3))?;
    c.ecx = byte(c.ecx, cl);
    c.esi = entry;
    // `shl eax, cl`: replaced by the `test` below.
    c.eax = 1u32 << (cl & 31);
    let unit = (c.eax as u8) & 1;
    logic8(c, unit);
    if unit == 0 {
        return copy_action(c, m, sp);
    }
    c.esi = m.u32(at(c.esi, 8))?;
    let cl = m.u8(at(c.esi, 0x348))? >> 5;
    c.ecx = byte(c.ecx, cl);
    let asked = cl & 1;
    logic8(c, asked);
    if asked == 0 {
        return copy_action(c, m, sp);
    }
    let (x, y, z) = (
        m.u32(at(c.esi, 0x168))?,
        m.u32(at(c.esi, 0x16c))?,
        m.u32(at(c.esi, 0x170))?,
    );
    let mut f = load_fpu(c);
    let e = fp_error;
    f.fld(y).map_err(e)?;
    f.fld(x).map_err(e)?;
    f.unary(unary_op::PATAN).map_err(e)?;
    let yaw = f.fstp_m32().map_err(e)?;
    m.set_u32(at(c.ebp, 0x10), yaw)?;
    f.fld(y).map_err(e)?;
    f.fld(x).map_err(e)?;
    f.fld(z).map_err(e)?;
    f.fld_st(2).map_err(e)?;
    f.arith_reg(arith_op::MUL, 3, 0, true).map_err(e)?;
    f.fld_st(1).map_err(e)?;
    f.arith_reg(arith_op::MUL, 0, 2, false).map_err(e)?;
    f.arith_reg(arith_op::ADD, 3, 0, true).map_err(e)?;
    f.fxch(2).map_err(e)?;
    f.unary(unary_op::SQRT).map_err(e)?;
    f.fxch(2).map_err(e)?;
    f.fxch(1).map_err(e)?;
    f.fxch(2).map_err(e)?;
    f.unary(unary_op::PATAN).map_err(e)?;
    f.fstp_st(1).map_err(e)?;
    let pitch = f.fstp_m32().map_err(e)?;
    m.set_u32(at(c.ebp, 0x14), pitch)?;
    f.fld(m.u32(at(c.ebp, 0x10))?).map_err(e)?;
    f.fadd_m(m.u32(TWO_PI)?).map_err(e)?;
    X87::ok(x87::abi::ld_f64(&mut f.0, m.u64(FULL_TURN)?)).map_err(e)?;
    finish_fpu(c, m, f, 0x186fe0, FULL_TURN)?;
    m.call(c, FMOD, 0x186feb)
}

/// `0x187012`.
fn copy_action(c: &mut CpuAbi, m: &mut impl Memory, sp: u32) -> Result {
    c.esi = m.u32(at(sp, 0x74))?;
    c.edx = c.esi;
    m.call(c, ACTION_COPY, 0x18701d)
}

/// `0x18701d`: the action record at `out` from the control block and the
/// interaction block, then the return.
fn finish(c: &mut CpuAbi, m: &mut impl Memory, sp: u32) -> Result {
    forward(c, 0x18701d)?;
    let (out, block) = (c.esi, c.ebp);
    c.edx = m.u32(at(block, 0x10))?;
    m.set_u32(at(out, 4), c.edx)?;
    c.eax = m.u32(at(block, 0x14))?;
    m.set_u32(at(out, 8), c.eax)?;
    let cx = m.u16(at(block, 8))?;
    c.ecx = low16(c.ecx, cx);
    m.set_u16(at(out, 0x1c), cx)?;
    c.edx = m.u32(at(block, 4))?;
    m.set_u32(out, c.edx)?;
    c.eax = m.u32(at(block, 0x28))?;
    m.set_u32(at(out, 0x1e), c.eax)?;
    let cx = m.u16(at(block, 0x2c))?;
    c.ecx = low16(c.ecx, cx);
    m.set_u16(at(out, 0x22), cx)?;
    let dx = m.u16(at(block, 0x2e))?;
    c.edx = low16(c.edx, dx);
    m.set_u16(at(out, 0x24), dx)?;
    c.eax = m.u32(at(block, 0x18))?;
    m.set_u32(at(out, 0xc), c.eax)?;
    c.ecx = m.u32(at(block, 0x1c))?;
    m.set_u32(at(out, 0x10), c.ecx)?;
    c.edx = m.u32(at(block, 0x20))?;
    m.set_u32(at(out, 0x14), c.edx)?;
    c.eax = m.u32(at(block, 0x24))?;
    m.set_u32(at(out, 0x18), c.eax)?;
    c.edx = m.u32(c.ebx)?;
    c.ecx = at(out, 0x28);
    m.set_u32(c.ecx, c.edx)?;
    c.eax = m.u32(at(c.ebx, 4))?;
    m.set_u32(at(c.ecx, 4), c.eax)?;
    c.edx = m.u32(at(c.ebx, 8))?;
    c.edi = at(out, 0x34);
    c.esi = m.u32(at(sp, 0x68))?;
    m.set_u32(at(c.ecx, 8), c.edx)?;
    c.ecx = 9;
    rep_movs(c, m)?;
    c.edi = pop(c, m)?;
    c.esi = pop(c, m)?;
    c.ebp = pop(c, m)?;
    c.ebx = pop(c, m)?;
    c.esp = arithmetic(c, c.esp, 0x54, false);
    m.ret(c, 0x10)
}
