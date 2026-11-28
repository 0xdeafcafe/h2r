//! The rest of the unit update (slice 6c of
//! docs/engine/sim-rewrite-objects.md; contracts in
//! docs/engine/object-update.md, "Slice 6c bodies"): `c6b30`'s children
//! `c6740` (the timers), `114c60` (the dialogue update), `c6990` (the
//! team) with its seat iterator `d0620` and `d0690`, `c6810` (active
//! camouflage) and `c50a0` (the integrated light). The animation state
//! `111650` is [`super::unit_anim`].
//!
//! Every body is a segment over the retail frame, the retail code block by
//! block, keyed by retail address: at each call and return the eight
//! integer registers, the `xmm` registers, MXCSR and the x87 state are
//! retail's, and every store retail makes is made, stack slots included.
//! Arithmetic flags are left undefined at every transfer: no caller and no
//! callee reads them (`scripts/sim-liveness.py`, docs/engine/object-update.md,
//! "Register liveness").
//!
//! **`c6740 (unit: eax) -> al`; `ret`.** `esi`, `edi` pushed; `edi` the
//! unit, `esi` the body, `dl` the result. Counts down the words at body
//! `+2e8` (zeroing `+2e4` when it reaches 0) and `+2ca` (when it reaches 0,
//! the deferred injury notice `1c9c80(unit: eax, +2c8, +2cc, 0, 1)`, then
//! `+2c8` = 0, `+2d0` = none, `+2cc` = 0); either running sets the result.
//! Then the byte at `+1f4`: when it reaches 0, `cffc0` (`edi` the unit).
//! `al` is 1 when that byte was running, else the result.
//!
//! **`c6810 (unit: eax)`; `ret`.** `ebx`, `ebp`, `esi`, `edi` pushed;
//! `ebp` the object array global, `edi` the header offset, `esi` the body.
//! In the campaign with the Assassins skull on, an AI unit (body `+12c`
//! not none) whose team (`+138`) is none, out of `0..10`, or not friendly
//! to the player's (`bfe60`) gets body `+134` bit 3; outside the campaign
//! only where the gate is answered ([`crate::skull_gate`], a Firefight
//! host). Then the camouflage
//! level `+2b0` moves by `+2b8` times the tick (game time `+4`) toward 1
//! (bit 3) or 0, and `b58c0(+d4, 800000)` when it hits 1 and the body
//! has a `+d4` effect; and `+2b4` moves by the tick times a constant
//! toward 1 (bit 4) or 0.
//!
//! **`c6990 (unit: eax)`; `ret`.** Frame `E` after `sub esp, 1c`; `esi`,
//! `edi`, then `ebp`, then `ebx` pushed. Unless body `+10a` bit 2: the
//! team `+138` is the player's (`+13c`), else the actor's (`+12c`), else,
//! for a vehicle (`+aa` 1), the team of the units in its seats: `[E]` and
//! `ebp` the two candidates (none), `[E+4..1c]` the iterator `d0620`
//! fills from `ebx` (`E+4`) and `d0690` advances (`edx` `E+4`); a unit in
//! a seat whose definition flag (bit 11 of the seat record at vehicle
//! definition `+1cc`, stride `b0`) is set gives `[E]`, otherwise `bp`.
//! The team is `bp` when the two agree or `[E]` is none, else none.
//!
//! **`d0620 (unit: eax, iterator: ebx)`; `ret`.** `ecx`, `ebp`, `esi`,
//! `edi` pushed. Climbs from the unit through parents while body `+4`
//! bit 26 is set and the parent is a biped or vehicle (`1 << +aa` & 3;
//! the 1 shifted is stored over the saved `ecx` slot, so `ecx` returns
//! 1 after a step); stores the top twice and its first child (`+10`).
//!
//! **`d0690 (iterator: edx) -> al`; `ret`.** `ecx`, `ebx`, `ebp`, `esi`,
//! `edi` pushed; the found byte is the saved `ecx` slot's top byte
//! (`[esp+13]`), so `ecx` returns with it. Walks the hierarchy under the
//! iterator's root (siblings `+c`, children `+10`; a biped or vehicle
//! child with body `+4` bit 26 is entered) and stops at the next biped or
//! vehicle, storing its handle at `+10`, its seat `+1fc` at `+14` and its
//! parent at `+c`. `al` is whether it found one.
//!
//! **`c50a0 (unit: eax)`; `ret`.** Frame `E` after `sub esp, 8`; `esi`
//! (the unit), `edi` (the body) pushed. Only for a player's unit in the
//! campaign. Without the player's flag at `+88` and without the Envy
//! skull: the light toggle (body `+148` bit 2) switches `+134` bit 29 with
//! `c5890(4)` or `c5890(5)` (which also sets the timer `+268`), counts the
//! timer down, clears bit 29 when the unit is gone or the timer ran out,
//! and moves the light level `+264` toward 1 or 0 by one tick over a
//! tick-rate-scaled time (`fld`/`fistp` at `c5214` or `c5281`). With
//! either: the camouflage toggle through `c5740` and `+134` bit 15, and
//! the camouflage timer `+2be` ending in `c5890(3)` or `c5740(0)` by
//! `+2bc`.
//!
//! **`114c60 (unit: eax)`; `ret`.** Frame `E` after `sub esp, 1c`; `ebx`,
//! `esi` (the unit), `edi` pushed; `[E]` the body; `edi` the dialogue
//! block (body `+ [body+342]`), `bl` 1 from `114cfb`. Counts down the
//! block's timers `+3c` (refilled from `+3e` through `fld`/`fistp` at
//! `114cce`), `+42`, `+40`, `+4e`, `+4c` (clearing `+54`), `+50`. While a
//! line plays (`+c` positive): starts it once (`+48`; `20b140` when it
//! has no sound yet, `1891d0` with the record at `E+c..18` for its
//! handle at `+54`, `20d220`), then `20d570` at most once before and
//! after (`+49`), `20fd50` with a sound, and `+c` = 0. Last, `+208` moves
//! toward zero by a clamped step.

use super::functions::{call, ret};
use super::sse::{self, Ss};
use super::{Cx, Next, Result, unowned};
use guest_layout::points;
use crate::sim_physics::{byte, pop, push};
use runtime::{CpuAbi, Guest};
use x87::abi as fp;

const NONE: u32 = u32::MAX;
const OBJECTS: u32 = points::OBJECTS.base;
const TIME: u32 = points::GAME_TIME.base;
const TAGS: u32 = points::TAG_ENTRIES.base;
/// The game globals (`+8` the options mode; 1 is the campaign).
const GAME: u32 = points::GAME_GLOBALS.base;
const PLAYERS: u32 = points::PLAYERS.base;
const ACTORS: u32 = points::ACTORS.base;
/// The skull bytes: Envy at `+0`, Assassins at `+2`.
const SKULLS: u32 = points::SKULLS.base;
/// Assassins' index in [`SKULLS`].
const ASSASSINS: u8 = points::skull_gates::ASSASSINS;
/// Holds the player's record (`+c4` the team record `bfe60` compares).
const LOCAL_PLAYER: u32 = 0x004f_55ec;
/// Holds a pointer to a byte `c50a0` tests (L: the game is paused).
const PAUSED: u32 = 0x0051_07e8;
/// `.rdata` constants: 1.0; the camouflage rate; the light timer; the
/// tick scales of the light and dialogue times; the dialogue clamp.
const K_ONE: u32 = 0x0045_dbc0;
const K_CAMO_RATE: u32 = 0x0045_dc64;
const K_LIGHT_TIMER: u32 = 0x0045_dc10;
const K_LIGHT_ON: u32 = 0x0045_dc5c;
const K_LIGHT_OFF: u32 = 0x0044_ae88;
const K_DIALOGUE: u32 = 0x0043_ff08;
const K_STEP_LOW: u32 = 0x0044_5630;
const K_STEP_HIGH: u32 = 0x0045_dc68;
/// The `imm32` of `mov eax, imm` at `114d43`.
const DIALOGUE_IMM: u32 = 0x0011_4d44;
/// The `imm32` of `push imm` at `c6918`.
const CAMO_IMM: u32 = 0x000c_6919;

pub(super) mod callee {
    /// `(victim: eax, ...)`, `ret 10`: the deferred injury notice.
    pub const INJURY: u32 = 0x1c9c80;
    /// `(unit: edi)`.
    pub const CFFC0: u32 = 0xcffc0;
    /// `(team: edx, record: stack 0) -> al`, `ret 4`.
    pub const BFE60: u32 = 0xbfe60;
    /// `(effect: stack 0, flags: stack 1)`, `ret 8`.
    pub const B58C0: u32 = 0xb58c0;
    /// `(state: ax)`, `edi` the body.
    pub const C5890: u32 = 0xc5890;
    /// `(unit: eax, on: stack 0)`, `ret 4`.
    pub const C5740: u32 = 0xc5740;
    /// `ret 8`.
    pub const S20B140: u32 = 0x20b140;
    /// `(eax, unit: esi, record: stack 0) -> eax`, `ret 4`.
    pub const S1891D0: u32 = 0x1891d0;
    /// `ret c`.
    pub const S20D220: u32 = 0x20d220;
    /// `ret 8`.
    pub const S20D570: u32 = 0x20d570;
    /// `(unit: eax, sound: ecx)`.
    pub const S20FD50: u32 = 0x20fd50;
}

fn at(a: u32, o: u32) -> u32 {
    a.wrapping_add(o)
}

/// Whether `pc` is `entry` or one of `sites`.
fn owned(pc: u32, entry: u32, sites: &[u32]) -> Result<()> {
    if pc == entry || sites.contains(&pc) { Ok(()) } else { Err(unowned(pc)) }
}

fn sx16(v: u16) -> u32 {
    v as i16 as i32 as u32
}

fn word16(r: u32, v: u32) -> u32 {
    (r & !0xffff) | (v & 0xffff)
}

/// `movss [addr], xmm[r]`.
fn st(c: &CpuAbi, g: &mut Guest, addr: u32, r: usize) -> Result<()> {
    g.set_u32(addr, sse::lane0(c, r))
}

/// `fld dword [src]; fistp dword [dst]` with the `fistp` at `fistp_pc`,
/// under the guest's x87 control word. Returns the stored integer.
fn fld_fistp(c: &mut CpuAbi, g: &mut Guest, src: u32, dst: u32, fistp_pc: u32) -> Result<u32> {
    let mut x = fp::X87Abi {
        st: c.x87.st,
        control: c.x87.control,
        status: c.x87.status,
        tag: c.x87.tag,
        opcode: c.x87.opcode,
        instruction_pointer: c.x87.instruction_pointer,
        data_pointer: c.x87.data_pointer,
        instruction_selector: c.x87.instruction_selector,
        data_selector: c.x87.data_selector,
    };
    // `supported` masks every x87 exception with none pending, so neither
    // step can ask for delivery.
    let code = fp::ld_f32(&mut x, g.u32(src)?);
    if code & fp::status::MASK != fp::status::OK {
        return Err(unowned(fistp_pc));
    }
    let (code, store) = fp::st_i32(&x, 0);
    if code & fp::status::MASK != fp::status::OK {
        return Err(unowned(fistp_pc));
    }
    g.set_u32(dst, store.value as u32)?;
    fp::store_commit(&mut x, store.status, true);
    c.x87.st = x.st;
    c.x87.status = x.status;
    c.x87.tag = x.tag;
    c.x87.instruction_pointer = fistp_pc;
    c.x87.opcode = g.u16(fistp_pc)?.swap_bytes() & 0x7ff;
    c.x87.data_pointer = dst;
    c.x87.instruction_selector = 8;
    c.x87.data_selector = 16;
    Ok(store.value as u32)
}

/// `ucomiss a, b; lahf; test ah, 44h`: whether `jp` is taken (not
/// ordered and equal). `lahf` writes `ah`.
fn not_equal(c: &mut CpuAbi, a: u32, b: u32) -> bool {
    sse::ucomiss(c, a, b);
    sse::lahf(c);
    let ah = (c.eax >> 8) & 0x44;
    ah != 0x40
}

/// `comiss a, b; jbe`.
fn below_or_equal(c: &mut CpuAbi, a: u32, b: u32) -> bool {
    sse::comiss(c, a, b);
    sse::below_or_equal(c)
}

// ---------------------------------------------------------------- c6740

/// `c6740`'s return sites.
pub(super) const TIMERS_SITES: [u32; 2] = [0xc67c4, 0xc67fd];

/// `c6740 (unit: eax) -> al`.
pub(super) fn timers(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    owned(pc, points::UNIT_TIMERS.base, &TIMERS_SITES)?;
    let mut pc = pc;
    loop {
        pc = match pc {
            0xc6740 => {
                c.ecx = g.u32(OBJECTS)?;
                c.edx = g.u32(at(c.ecx, 0x44))?;
                push(c, g, c.esi)?;
                push(c, g, c.edi)?;
                c.edi = c.eax;
                c.eax = (c.eax & 0xffff).wrapping_mul(3);
                c.esi = g.u32(at(c.edx, c.eax.wrapping_mul(4).wrapping_add(8)))?;
                c.ecx = u32::from(g.u16(at(c.esi, 0x2e8))?);
                c.edx = byte(c.edx, 0);
                if (c.ecx as u16 as i16) <= 0 {
                    0xc6785
                } else {
                    c.eax = c.ecx.wrapping_sub(1);
                    g.set_u16(at(c.esi, 0x2e8), c.eax as u16)?;
                    if c.eax as u16 == 0 {
                        sse::zero(c, 0);
                        st(c, g, at(c.esi, 0x2e4), 0)?;
                    }
                    c.edx = byte(c.edx, 1);
                    0xc6785
                }
            }
            0xc6785 => {
                c.ecx = u32::from(g.u16(at(c.esi, 0x2ca))?);
                if (c.ecx as u16 as i16) <= 0 {
                    0xc67e4
                } else {
                    c.eax = c.ecx.wrapping_sub(1);
                    g.set_u16(at(c.esi, 0x2ca), c.eax as u16)?;
                    if c.eax as u16 != 0 {
                        c.edx = byte(c.edx, 1);
                        0xc67e4
                    } else {
                        c.eax = g.u32(at(c.esi, 0x2cc))?;
                        c.edx = g.u32(at(c.esi, 0x2d0))?;
                        c.ecx = u32::from(g.u16(at(c.esi, 0x2c8))?);
                        push(c, g, 1)?;
                        push(c, g, 0)?;
                        push(c, g, c.eax)?;
                        c.eax = c.edi;
                        push(c, g, c.ecx)?;
                        return call(c, callee::INJURY, TIMERS_SITES[0]);
                    }
                }
            }
            0xc67c4 => {
                sse::zero(c, 0);
                g.set_u16(at(c.esi, 0x2c8), 0)?;
                g.set_u32(at(c.esi, 0x2d0), NONE)?;
                st(c, g, at(c.esi, 0x2cc), 0)?;
                c.edx = byte(c.edx, 1);
                0xc67e4
            }
            0xc67e4 => {
                let al = g.u8(at(c.esi, 0x1f4))?;
                c.eax = byte(c.eax, al);
                if al == 0 {
                    c.edi = pop(c, g)?;
                    c.eax = byte(c.eax, c.edx as u8);
                    c.esi = pop(c, g)?;
                    return ret(c, 0);
                }
                let al = al.wrapping_sub(1);
                c.eax = byte(c.eax, al);
                g.set_u8(at(c.esi, 0x1f4), al)?;
                if al == 0 {
                    return call(c, callee::CFFC0, TIMERS_SITES[1]);
                }
                0xc67fd
            }
            0xc67fd => {
                c.edi = pop(c, g)?;
                c.eax = byte(c.eax, 1);
                c.esi = pop(c, g)?;
                return ret(c, 0);
            }
            _ => return Err(unowned(pc)),
        };
    }
}

// ---------------------------------------------------------------- c6810

/// `c6810`'s return sites.
pub(super) const CAMO_SITES: [u32; 2] = [0xc6878, 0xc6923];

/// `c6810 (unit: eax)`.
pub(super) fn camo(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    owned(pc, points::UNIT_CAMO_UPDATE.base, &CAMO_SITES)?;
    let mut pc = pc;
    loop {
        pc = match pc {
            0xc6810 => {
                push(c, g, c.ebx)?;
                push(c, g, c.ebp)?;
                c.ebp = g.u32(OBJECTS)?;
                c.eax &= 0xffff;
                push(c, g, c.esi)?;
                push(c, g, c.edi)?;
                c.edi = c.eax.wrapping_mul(3);
                c.eax = g.u32(at(c.ebp, 0x44))?;
                c.edi <<= 2;
                c.esi = g.u32(at(c.eax, c.edi).wrapping_add(8))?;
                if g.u32(at(c.esi, 0x12c))? == NONE {
                    0xc6887
                } else {
                    c.ecx = g.u32(GAME)?;
                    // The campaign gate; a Firefight host answers it for
                    // Assassins (`points::skull_gates`, the lifted
                    // reader's `jne` at `c683f`).
                    if !crate::skull_gate::passes(g.u32(at(c.ecx, 8))?, ASSASSINS) {
                        0xc6887
                    } else {
                        let al = g.u8(at(SKULLS, 2))?;
                        c.eax = byte(c.eax, al);
                        if al == 0 {
                            0xc6887
                        } else {
                            let ax = u32::from(g.u16(at(c.esi, 0x138))?);
                            c.eax = word16(c.eax, ax);
                            let team = ax as u16 as i16;
                            if ax == 0xffff || !(0..0x10).contains(&team) {
                                0xc6880
                            } else {
                                c.edx = sx16(ax as u16);
                                c.eax = g.u32(LOCAL_PLAYER)?;
                                c.eax = c.eax.wrapping_add(0xc4);
                                c.edx = c.edx.wrapping_add(0x10);
                                push(c, g, c.eax)?;
                                return call(c, callee::BFE60, CAMO_SITES[0]);
                            }
                        }
                    }
                }
            }
            0xc6878 => {
                // `neg al; sbb al, al; inc al`: 1 when `al` was 0.
                let al = u8::from(c.eax as u8 == 0);
                c.eax = byte(c.eax, al);
                if al == 0 { 0xc6887 } else { 0xc6880 }
            }
            0xc6880 => {
                let a = at(c.esi, 0x134);
                g.set_u32(a, g.u32(a)? | 8)?;
                0xc6887
            }
            0xc6887 => {
                c.ecx = g.u32(at(c.esi, 0x134))?;
                sse::zero(c, 1);
                sse::load(c, 2, g.u32(K_ONE)?);
                c.ebx = g.u32(TIME)?;
                c.ecx >>= 3;
                if c.ecx & 1 == 0 {
                    // c68d5
                    sse::load(c, 3, g.u32(at(c.esi, 0x2b8))?);
                    sse::ss(c, Ss::Mul, 3, g.u32(at(c.ebx, 4))?);
                    sse::load(c, 0, g.u32(at(c.esi, 0x2b0))?);
                    c.ebx = c.ebx.wrapping_add(4);
                    sse::ss_reg(c, Ss::Sub, 0, 3);
                    let low = below_or_equal(c, sse::lane0(c, 1), sse::lane0(c, 0));
                    st(c, g, at(c.esi, 0x2b0), 0)?;
                    if low {
                        0xc6923
                    } else {
                        st(c, g, at(c.esi, 0x2b0), 1)?;
                        0xc6906
                    }
                } else {
                    sse::load(c, 0, g.u32(at(c.esi, 0x2b8))?);
                    sse::ss(c, Ss::Mul, 0, g.u32(at(c.ebx, 4))?);
                    sse::ss(c, Ss::Add, 0, g.u32(at(c.esi, 0x2b0))?);
                    c.ebx = c.ebx.wrapping_add(4);
                    let low = below_or_equal(c, sse::lane0(c, 0), sse::lane0(c, 2));
                    st(c, g, at(c.esi, 0x2b0), 0)?;
                    if low {
                        0xc6923
                    } else {
                        st(c, g, at(c.esi, 0x2b0), 2)?;
                        0xc6906
                    }
                }
            }
            0xc6906 => {
                c.edx = g.u32(at(c.ebp, 0x44))?;
                c.edi = g.u32(at(c.edx, c.edi).wrapping_add(8))?;
                c.eax = g.u32(at(c.edi, 0xd4))?;
                if c.eax == NONE {
                    0xc6923
                } else {
                    push(c, g, g.u32(CAMO_IMM)?)?;
                    push(c, g, c.eax)?;
                    return call(c, callee::B58C0, CAMO_SITES[1]);
                }
            }
            0xc6923 => {
                c.eax = g.u32(at(c.esi, 0x134))? >> 4;
                if c.eax & 1 == 0 {
                    // c695e
                    sse::load(c, 2, g.u32(c.ebx)?);
                    sse::load(c, 0, g.u32(at(c.esi, 0x2b4))?);
                    sse::ss(c, Ss::Mul, 2, g.u32(K_CAMO_RATE)?);
                    sse::ss_reg(c, Ss::Sub, 0, 2);
                    let low = below_or_equal(c, sse::lane0(c, 1), sse::lane0(c, 0));
                    st(c, g, at(c.esi, 0x2b4), 0)?;
                    if !low {
                        st(c, g, at(c.esi, 0x2b4), 1)?;
                    }
                } else {
                    sse::load(c, 0, g.u32(c.ebx)?);
                    sse::ss(c, Ss::Mul, 0, g.u32(K_CAMO_RATE)?);
                    sse::ss(c, Ss::Add, 0, g.u32(at(c.esi, 0x2b4))?);
                    let low = below_or_equal(c, sse::lane0(c, 0), sse::lane0(c, 2));
                    st(c, g, at(c.esi, 0x2b4), 0)?;
                    if !low {
                        c.edi = pop(c, g)?;
                        st(c, g, at(c.esi, 0x2b4), 2)?;
                        c.esi = pop(c, g)?;
                        c.ebp = pop(c, g)?;
                        c.ebx = pop(c, g)?;
                        return ret(c, 0);
                    }
                }
                c.edi = pop(c, g)?;
                c.esi = pop(c, g)?;
                c.ebp = pop(c, g)?;
                c.ebx = pop(c, g)?;
                return ret(c, 0);
            }
            _ => return Err(unowned(pc)),
        };
    }
}

// ---------------------------------------------------------------- c6990

/// `c6990`'s return sites.
pub(super) const TEAM_SITES: [u32; 3] = [0xc6a46, 0xc6a4d, 0xc6ae9];

/// `c6990 (unit: eax)`.
pub(super) fn team(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    owned(pc, points::UNIT_TEAM_UPDATE.base, &TEAM_SITES)?;
    // `[esp + n]` with all four registers pushed.
    let s = |c: &CpuAbi, n: u32| c.esp.wrapping_add(n);
    let mut pc = pc;
    loop {
        pc = match pc {
            0xc6990 => {
                c.ecx = g.u32(OBJECTS)?;
                c.esp = c.esp.wrapping_sub(0x1c);
                push(c, g, c.esi)?;
                c.esi = g.u32(at(c.ecx, 0x44))?;
                c.ecx = c.eax & 0xffff;
                c.edx = c.ecx.wrapping_mul(3);
                push(c, g, c.edi)?;
                c.edi = g.u32(at(c.esi, c.edx.wrapping_mul(4)).wrapping_add(8))?;
                let cl = g.u8(at(c.edi, 0x10a))? >> 2;
                c.ecx = byte(c.ecx, cl);
                if cl & 1 != 0 {
                    0xc6b20
                } else {
                    c.ecx = g.u32(at(c.edi, 0x13c))?;
                    push(c, g, c.ebp)?;
                    c.ebp = NONE;
                    if c.ecx != c.ebp {
                        c.edx = g.u32(PLAYERS)?;
                        c.eax = g.u32(at(c.edx, 0x44))?;
                        c.ecx = (c.ecx & 0xffff).wrapping_mul(0x21c);
                        let cx = g.u8(at(at(c.ecx, c.eax), 0xc0))? as i8 as i16 as u16;
                        c.ecx = word16(c.ecx, u32::from(cx));
                        c.ebp = pop(c, g)?;
                        g.set_u16(at(c.edi, 0x138), cx)?;
                        c.edi = pop(c, g)?;
                        c.esi = pop(c, g)?;
                        c.esp = c.esp.wrapping_add(0x1c);
                        return ret(c, 0);
                    }
                    c.ecx = g.u32(at(c.edi, 0x12c))?;
                    if c.ecx != c.ebp {
                        c.edx = g.u32(ACTORS)?;
                        c.eax = g.u32(at(c.edx, 0x44))?;
                        c.ecx = (c.ecx & 0xffff).wrapping_mul(0x888);
                        let cx = g.u16(at(at(c.ecx, c.eax), 0x24))?;
                        c.ecx = word16(c.ecx, u32::from(cx));
                        c.ebp = pop(c, g)?;
                        g.set_u16(at(c.edi, 0x138), cx)?;
                        c.edi = pop(c, g)?;
                        c.esi = pop(c, g)?;
                        c.esp = c.esp.wrapping_add(0x1c);
                        return ret(c, 0);
                    }
                    if g.u8(at(c.edi, 0xaa))? != 1 {
                        0xc6b1f
                    } else {
                        push(c, g, c.ebx)?;
                        c.ebx = s(c, 0x14);
                        g.set_u32(s(c, 0x10), c.ebp)?;
                        return call(c, points::UNIT_SEATS_BEGIN.base, TEAM_SITES[0]);
                    }
                }
            }
            0xc6a46 => {
                c.edx = c.ebx;
                return call(c, points::UNIT_SEATS_NEXT.base, TEAM_SITES[1]);
            }
            0xc6a4d => {
                if c.eax as u8 == 0 {
                    0xc6afb
                } else {
                    c.ebx = g.u32(TAGS)?;
                    0xc6a60
                }
            }
            0xc6a60 => {
                if c.ebp as u16 == 0xffff {
                    0xc6a74
                } else {
                    c.eax = g.u32(s(c, 0x10))?;
                    if c.eax as u16 != 0xffff { 0xc6af5 } else { 0xc6a74 }
                }
            }
            0xc6a74 => {
                c.eax = g.u32(s(c, 0x24))?;
                c.ecx = g.u32(s(c, 0x28))?;
                c.eax &= 0xffff;
                c.edx = c.eax.wrapping_mul(3);
                c.edx = g.u32(at(c.esi, c.edx.wrapping_mul(4)).wrapping_add(8))?;
                if c.ecx as u16 == 0xffff {
                    0xc6ae0
                } else {
                    c.eax = g.u32(s(c, 0x20))?;
                    if c.eax == NONE {
                        0xc6ae0
                    } else {
                        c.eax = (c.eax & 0xffff).wrapping_mul(3);
                        c.eax = g.u32(at(c.esi, c.eax.wrapping_mul(4)).wrapping_add(8))?;
                        c.eax = g.u32(c.eax)?;
                        c.ecx = sx16(c.ecx as u16).wrapping_mul(0xb0);
                        c.eax = (c.eax & 0xffff) << 4;
                        c.eax = g.u32(at(at(c.eax, c.ebx), 8))?;
                        c.eax = g.u32(at(c.eax, 0x1cc))?;
                        c.ecx = g.u32(at(c.ecx, c.eax))? >> 0xb;
                        if c.ecx & 1 != 0 {
                            let dx = g.u16(at(c.edx, 0x138))?;
                            c.edx = word16(c.edx, u32::from(dx));
                            g.set_u16(s(c, 0x10), dx)?;
                        } else {
                            let bp = g.u16(at(c.edx, 0x138))?;
                            c.ebp = word16(c.ebp, u32::from(bp));
                        }
                        0xc6ae0
                    }
                }
            }
            0xc6ae0 => {
                c.edx = s(c, 0x14);
                return call(c, points::UNIT_SEATS_NEXT.base, TEAM_SITES[2]);
            }
            0xc6ae9 => {
                if c.eax as u8 != 0 {
                    0xc6a60
                } else {
                    c.eax = g.u32(s(c, 0x10))?;
                    0xc6af5
                }
            }
            0xc6af5 => {
                if c.ebp as u16 != 0xffff {
                    // c6b0c
                    if c.ebp as u16 == c.eax as u16 || c.eax as u16 == 0xffff {
                        g.set_u16(at(c.edi, 0x138), c.ebp as u16)?;
                        c.ebx = pop(c, g)?;
                        0xc6b1f
                    } else {
                        0xc6afb
                    }
                } else {
                    0xc6afb
                }
            }
            0xc6afb => {
                c.ebx = pop(c, g)?;
                c.ebp = pop(c, g)?;
                g.set_u16(at(c.edi, 0x138), 0xffff)?;
                c.edi = pop(c, g)?;
                c.esi = pop(c, g)?;
                c.esp = c.esp.wrapping_add(0x1c);
                return ret(c, 0);
            }
            0xc6b1f => {
                c.ebp = pop(c, g)?;
                0xc6b20
            }
            0xc6b20 => {
                c.edi = pop(c, g)?;
                c.esi = pop(c, g)?;
                c.esp = c.esp.wrapping_add(0x1c);
                return ret(c, 0);
            }
            _ => return Err(unowned(pc)),
        };
    }
}

// ---------------------------------------------------------------- d0620

/// `1 << cl` against the biped and vehicle types (`test al, 3`).
fn unit_type(c: &mut CpuAbi, g: &Guest, body: u32) -> Result<bool> {
    c.ecx = byte(c.ecx, g.u8(at(body, 0xaa))?);
    c.eax = 1u32 << (c.ecx & 31);
    Ok(c.eax & 3 != 0)
}

/// `d0620 (unit: eax, iterator: ebx)`.
pub(super) fn seats_begin(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    owned(pc, points::UNIT_SEATS_BEGIN.base, &[])?;
    push(c, g, c.ecx)?;
    push(c, g, c.ebp)?;
    push(c, g, c.esi)?;
    push(c, g, c.edi)?;
    c.edi = c.eax;
    c.eax = g.u32(OBJECTS)?;
    c.esi = g.u32(at(c.eax, 0x44))?;
    loop {
        c.eax = c.edi & 0xffff;
        c.ecx = c.eax.wrapping_mul(3);
        c.ebp = g.u32(at(c.esi, c.ecx.wrapping_mul(4)).wrapping_add(8))?;
        c.edx = g.u32(at(c.ebp, 0x14))?;
        if c.edx == NONE {
            break;
        }
        c.eax = g.u32(at(c.ebp, 4))? >> 0x1a;
        if c.eax & 1 == 0 {
            break;
        }
        c.eax = c.edx & 0xffff;
        c.ecx = c.eax.wrapping_mul(3);
        c.eax = g.u32(at(c.esi, c.ecx.wrapping_mul(4)).wrapping_add(8))?;
        c.ecx = 1;
        g.set_u32(at(c.esp, 0xc), c.ecx)?;
        c.ecx = byte(c.ecx, g.u8(at(c.eax, 0xaa))?);
        c.eax = g.u32(at(c.esp, 0xc))? << (c.ecx & 31);
        if c.eax & 3 == 0 {
            break;
        }
        c.edi = c.edx;
    }
    g.set_u32(c.ebx, c.edi)?;
    g.set_u32(at(c.ebx, 4), c.edi)?;
    c.edx = g.u32(at(c.ebp, 0x10))?;
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    g.set_u32(at(c.ebx, 8), c.edx)?;
    c.ebp = pop(c, g)?;
    c.ecx = pop(c, g)?;
    ret(c, 0)
}

// ---------------------------------------------------------------- d0690

/// `d0690 (iterator: edx) -> al`.
pub(super) fn seats_next(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    owned(pc, points::UNIT_SEATS_NEXT.base, &[])?;
    let it = |c: &CpuAbi, o: u32| c.edx.wrapping_add(o);
    push(c, g, c.ecx)?;
    c.eax = g.u32(c.edx)?;
    let moved = c.eax != g.u32(it(c, 4))?;
    push(c, g, c.ebx)?;
    c.ebx = g.u32(OBJECTS)?;
    push(c, g, c.ebp)?;
    push(c, g, c.esi)?;
    push(c, g, c.edi)?;
    g.set_u8(at(c.esp, 0x13), 0)?;
    let mut pc = if moved { 0xd0752 } else { 0xd06ab };
    loop {
        pc = match pc {
            0xd06ab => {
                if g.u32(it(c, 8))? == NONE {
                    0xd06e7
                } else {
                    0xd06b1
                }
            }
            // The root's own children first.
            0xd06b1 => {
                c.edi = g.u32(it(c, 8))?;
                c.ecx = c.edi & 0xffff;
                c.eax = c.ecx.wrapping_mul(3);
                c.ecx = g.u32(at(c.ebx, 0x44))?;
                c.esi = g.u32(at(c.ecx, c.eax.wrapping_mul(4)).wrapping_add(8))?;
                c.eax = g.u32(at(c.esi, 0xc))?;
                g.set_u32(it(c, 8), c.eax)?;
                if unit_type(c, g, c.esi)? {
                    0xd0836
                } else if g.u32(it(c, 8))? != NONE {
                    0xd06b1
                } else {
                    0xd06e7
                }
            }
            0xd06e7 => {
                c.ecx = g.u32(c.edx)?;
                c.ebp = g.u32(at(c.ebx, 0x44))?;
                c.ecx = (c.ecx & 0xffff).wrapping_mul(3);
                c.eax = g.u32(at(c.ebp, c.ecx.wrapping_mul(4)).wrapping_add(8))?;
                c.esi = g.u32(at(c.eax, 0x10))?;
                if c.esi == NONE { 0xd0734 } else { 0xd0701 }
            }
            0xd0701 => {
                c.ecx = (c.esi & 0xffff).wrapping_mul(3);
                c.edi = g.u32(at(c.ebp, c.ecx.wrapping_mul(4)).wrapping_add(8))?;
                if unit_type(c, g, c.edi)? {
                    c.ecx = g.u32(at(c.edi, 4))? >> 0x1a;
                    if c.ecx & 1 != 0 { 0xd0734 } else { 0xd072c }
                } else {
                    0xd072c
                }
            }
            0xd072c => {
                c.esi = g.u32(at(c.edi, 0xc))?;
                if c.esi != NONE { 0xd0701 } else { 0xd0734 }
            }
            0xd0734 => {
                g.set_u32(it(c, 4), c.esi)?;
                if c.esi != NONE {
                    c.ecx = g.u32(at(c.ebx, 0x44))?;
                    c.esi &= 0xffff;
                    c.eax = c.esi.wrapping_mul(3);
                    c.eax = g.u32(at(c.ecx, c.eax.wrapping_mul(4)).wrapping_add(8))?;
                    c.ecx = g.u32(at(c.eax, 0x10))?;
                    g.set_u32(it(c, 8), c.ecx)?;
                }
                0xd0752
            }
            0xd0752 => {
                c.eax = g.u32(it(c, 4))?;
                if g.u32(c.edx)? == c.eax {
                    0xd082c
                } else {
                    c.edi = NONE;
                    if c.eax == c.edi { 0xd082c } else { 0xd0768 }
                }
            }
            0xd0768 => {
                if g.u32(it(c, 8))? == c.edi {
                    0xd07aa
                } else {
                    0xd0770
                }
            }
            0xd0770 => {
                c.edi = g.u32(it(c, 8))?;
                c.ecx = c.edi & 0xffff;
                c.eax = c.ecx.wrapping_mul(3);
                c.ecx = g.u32(at(c.ebx, 0x44))?;
                c.esi = g.u32(at(c.ecx, c.eax.wrapping_mul(4)).wrapping_add(8))?;
                c.eax = g.u32(at(c.esi, 0xc))?;
                g.set_u32(it(c, 8), c.eax)?;
                if unit_type(c, g, c.esi)? {
                    // d0854: found; return at once.
                    g.set_u32(it(c, 0x10), c.edi)?;
                    let cx = g.u16(at(c.esi, 0x1fc))?;
                    c.ecx = word16(c.ecx, u32::from(cx));
                    c.edi = pop(c, g)?;
                    g.set_u16(it(c, 0x14), cx)?;
                    c.ecx = g.u32(at(c.esi, 0x14))?;
                    c.esi = pop(c, g)?;
                    c.ebp = pop(c, g)?;
                    c.eax = byte(c.eax, 1);
                    g.set_u32(it(c, 0xc), c.ecx)?;
                    c.ebx = pop(c, g)?;
                    c.ecx = pop(c, g)?;
                    return ret(c, 0);
                }
                c.eax = g.u32(it(c, 8))?;
                c.edi = NONE;
                if c.eax != c.edi { 0xd0770 } else { 0xd07aa }
            }
            0xd07aa => {
                let al = g.u8(at(c.esp, 0x13))?;
                c.eax = byte(c.eax, al);
                if al != 0 {
                    0xd0830
                } else {
                    c.ecx = g.u32(it(c, 4))?;
                    c.ebp = g.u32(at(c.ebx, 0x44))?;
                    c.ecx = (c.ecx & 0xffff).wrapping_mul(3);
                    c.eax = g.u32(at(c.ebp, c.ecx.wrapping_mul(4)).wrapping_add(8))?;
                    c.esi = g.u32(at(c.eax, 0xc))?;
                    if c.esi == c.edi { 0xd0803 } else { 0xd07d0 }
                }
            }
            0xd07d0 => {
                c.ecx = (c.esi & 0xffff).wrapping_mul(3);
                c.edi = g.u32(at(c.ebp, c.ecx.wrapping_mul(4)).wrapping_add(8))?;
                if unit_type(c, g, c.edi)? {
                    c.ecx = g.u32(at(c.edi, 4))? >> 0x1a;
                    if c.ecx & 1 != 0 { 0xd0803 } else { 0xd07fb }
                } else {
                    0xd07fb
                }
            }
            0xd07fb => {
                c.esi = g.u32(at(c.edi, 0xc))?;
                if c.esi != NONE { 0xd07d0 } else { 0xd0803 }
            }
            0xd0803 => {
                c.edi = NONE;
                g.set_u32(it(c, 4), c.esi)?;
                if c.esi != c.edi {
                    c.ecx = g.u32(at(c.ebx, 0x44))?;
                    c.esi &= 0xffff;
                    c.eax = c.esi.wrapping_mul(3);
                    c.eax = g.u32(at(c.ecx, c.eax.wrapping_mul(4)).wrapping_add(8))?;
                    c.ecx = g.u32(at(c.eax, 0x10))?;
                    g.set_u32(it(c, 8), c.ecx)?;
                }
                if g.u32(it(c, 4))? != c.edi { 0xd0768 } else { 0xd082c }
            }
            0xd082c => {
                let al = g.u8(at(c.esp, 0x13))?;
                c.eax = byte(c.eax, al);
                0xd0830
            }
            0xd0830 => {
                c.edi = pop(c, g)?;
                c.esi = pop(c, g)?;
                c.ebp = pop(c, g)?;
                c.ebx = pop(c, g)?;
                c.ecx = pop(c, g)?;
                return ret(c, 0);
            }
            // Found among the root's own children: note it and go on.
            0xd0836 => {
                g.set_u32(it(c, 0x10), c.edi)?;
                let cx = g.u16(at(c.esi, 0x1fc))?;
                c.ecx = word16(c.ecx, u32::from(cx));
                g.set_u16(it(c, 0x14), cx)?;
                c.eax = g.u32(at(c.esi, 0x14))?;
                g.set_u8(at(c.esp, 0x13), 1)?;
                g.set_u32(it(c, 0xc), c.eax)?;
                0xd0752
            }
            _ => return Err(unowned(pc)),
        };
    }
}

// ---------------------------------------------------------------- c50a0

/// `c50a0`'s return sites.
pub(super) const LIGHT_SITES: [u32; 5] = [0xc512c, 0xc5142, 0xc52d9, 0xc5315, 0xc532b];

/// `c50a0 (unit: eax)`.
pub(super) fn light(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    owned(pc, points::UNIT_LIGHT_UPDATE.base, &LIGHT_SITES)?;
    // `E` from `esp` with `esi` and `edi` pushed.
    let e = |c: &CpuAbi| c.esp.wrapping_add(8);
    let leave = |c: &mut CpuAbi, g: &Guest| -> Result<Next> {
        c.edi = pop(c, g)?;
        c.esi = pop(c, g)?;
        c.esp = c.esp.wrapping_add(8);
        ret(c, 0)
    };
    let mut pc = pc;
    loop {
        pc = match pc {
            0xc50a0 => {
                c.ecx = g.u32(OBJECTS)?;
                c.edx = g.u32(at(c.ecx, 0x44))?;
                c.esp = c.esp.wrapping_sub(8);
                push(c, g, c.esi)?;
                c.esi = c.eax;
                c.eax = (c.eax & 0xffff).wrapping_mul(3);
                push(c, g, c.edi)?;
                c.edi = g.u32(at(c.edx, c.eax.wrapping_mul(4)).wrapping_add(8))?;
                c.eax = g.u32(at(c.edi, 0x13c))?;
                if c.eax == NONE {
                    0xc532b
                } else {
                    c.ecx = g.u32(GAME)?;
                    if g.u32(at(c.ecx, 8))? != 1 {
                        0xc532b
                    } else {
                        c.edx = g.u32(PLAYERS)?;
                        c.ecx = g.u32(at(c.edx, 0x44))?;
                        c.eax = (c.eax & 0xffff).wrapping_mul(0x21c);
                        if g.u8(at(at(c.eax, c.ecx), 0x88))? == 1 {
                            0xc52be
                        } else {
                            let al = g.u8(SKULLS)?;
                            c.eax = byte(c.eax, al);
                            if al != 0 {
                                0xc52be
                            } else {
                                let cl = g.u8(at(c.edi, 0x148))?;
                                c.ecx = byte(c.ecx, cl);
                                c.eax = 4;
                                if cl & 4 == 0 {
                                    0xc5163
                                } else {
                                    c.edx = g.u32(at(c.edi, 0x134))? >> 0x1d;
                                    if c.edx & 1 != 0 {
                                        return call(c, callee::C5890, LIGHT_SITES[0]);
                                    }
                                    c.eax = 5;
                                    return call(c, callee::C5890, LIGHT_SITES[1]);
                                }
                            }
                        }
                    }
                }
            }
            0xc512c => {
                let a = at(c.edi, 0x134);
                g.set_u32(a, g.u32(a)? & 0xdfff_ffff)?;
                0xc5163
            }
            0xc5142 => {
                c.eax = g.u32(at(c.edi, 0x134))?;
                sse::load(c, 0, g.u32(K_LIGHT_TIMER)?);
                c.eax |= 0x2000_0000;
                g.set_u32(at(c.edi, 0x134), c.eax)?;
                st(c, g, at(c.edi, 0x268), 0)?;
                0xc5163
            }
            0xc5163 => {
                c.eax = g.u32(at(c.edi, 0x134))?;
                sse::zero(c, 1);
                c.eax >>= 0x1d;
                if c.eax & 1 == 0 {
                    0xc524c
                } else {
                    sse::load(c, 0, g.u32(at(c.edi, 0x268))?);
                    let off = below_or_equal(c, sse::lane0(c, 0), sse::lane0(c, 1));
                    c.ecx = g.u32(TIME)?;
                    if !off {
                        sse::ss(c, Ss::Sub, 0, g.u32(at(c.ecx, 4))?);
                        sse::comiss(c, sse::lane0(c, 1), sse::lane0(c, 0));
                        let positive = sse::below(c);
                        st(c, g, at(c.edi, 0x268), 0)?;
                        if !positive {
                            st(c, g, at(c.edi, 0x268), 1)?;
                        }
                    }
                    // c51a4
                    let clear = if g.u32(at(c.edi, 0x14))? != NONE {
                        true
                    } else {
                        let dl = g.u8(at(c.edi, 0x10a))? >> 2;
                        c.edx = byte(c.edx, dl);
                        if dl & 1 != 0 {
                            true
                        } else {
                            c.eax = g.u32(PAUSED)?;
                            if g.u8(c.eax)? != 0 {
                                false
                            } else {
                                sse::load(c, 0, g.u32(at(c.edi, 0x268))?);
                                !not_equal(c, sse::lane0(c, 0), sse::lane0(c, 1))
                            }
                        }
                    };
                    if clear {
                        let a = at(c.edi, 0x134);
                        g.set_u32(a, g.u32(a)? & 0xdfff_ffff)?;
                    }
                    // c51dd: toward 1.
                    sse::load(c, 0, g.u32(at(c.edi, 0x264))?);
                    sse::load(c, 1, g.u32(K_ONE)?);
                    if !not_equal(c, sse::lane0(c, 0), sse::lane0(c, 1)) {
                        0xc532b
                    } else {
                        c.ecx = sx16(g.u16(at(c.ecx, 2))?);
                        sse::cvtsi2ss(c, 0, c.ecx);
                        sse::ss(c, Ss::Mul, 0, g.u32(K_LIGHT_ON)?);
                        let base = e(c);
                        st(c, g, base, 0)?;
                        fld_fistp(c, g, base, at(base, 4), 0xc5214)?;
                        sse::cvtsi2ss(c, 2, g.u32(at(base, 4))?);
                        sse::movaps(c, 0, 1);
                        sse::ss_reg(c, Ss::Div, 0, 2);
                        sse::ss(c, Ss::Add, 0, g.u32(at(c.edi, 0x264))?);
                        let low = below_or_equal(c, sse::lane0(c, 0), sse::lane0(c, 1));
                        st(c, g, at(c.edi, 0x264), 0)?;
                        if low {
                            0xc532b
                        } else {
                            st(c, g, at(c.edi, 0x264), 1)?;
                            return leave(c, g);
                        }
                    }
                }
            }
            // Toward 0.
            0xc524c => {
                sse::load(c, 0, g.u32(at(c.edi, 0x264))?);
                if !not_equal(c, sse::lane0(c, 0), sse::lane0(c, 1)) {
                    0xc532b
                } else {
                    c.edx = g.u32(TIME)?;
                    c.eax = sx16(g.u16(at(c.edx, 2))?);
                    sse::cvtsi2ss(c, 0, c.eax);
                    sse::ss(c, Ss::Mul, 0, g.u32(K_LIGHT_OFF)?);
                    let base = e(c);
                    st(c, g, at(base, 4), 0)?;
                    fld_fistp(c, g, at(base, 4), base, 0xc5281)?;
                    sse::cvtsi2ss(c, 0, g.u32(base)?);
                    sse::load(c, 2, g.u32(K_ONE)?);
                    sse::ss_reg(c, Ss::Div, 2, 0);
                    sse::load(c, 0, g.u32(at(c.edi, 0x264))?);
                    sse::ss_reg(c, Ss::Sub, 0, 2);
                    let low = below_or_equal(c, sse::lane0(c, 1), sse::lane0(c, 0));
                    st(c, g, at(c.edi, 0x264), 0)?;
                    if low {
                        0xc532b
                    } else {
                        st(c, g, at(c.edi, 0x264), 1)?;
                        return leave(c, g);
                    }
                }
            }
            0xc52be => {
                if g.u8(at(c.edi, 0x148))? & 4 == 0 {
                    0xc52e3
                } else if g.u8(at(c.edi, 0x2bc))? == 1 {
                    0xc52d9
                } else {
                    push(c, g, 1)?;
                    c.eax = c.esi;
                    return call(c, callee::C5740, LIGHT_SITES[2]);
                }
            }
            0xc52d9 => {
                let a = at(c.edi, 0x134);
                g.set_u32(a, g.u32(a)? ^ 0x8000)?;
                0xc52e3
            }
            0xc52e3 => {
                c.eax = u32::from(g.u16(at(c.edi, 0x2be))?);
                if (c.eax as u16 as i16) <= 0 {
                    0xc532b
                } else {
                    c.eax = c.eax.wrapping_sub(1);
                    g.set_u16(at(c.edi, 0x2be), c.eax as u16)?;
                    if c.eax as u16 != 0 {
                        0xc532b
                    } else {
                        c.eax = g.u8(at(c.edi, 0x2bc))? as i8 as i32 as u32;
                        c.eax = c.eax.wrapping_sub(1);
                        if c.eax == 0 {
                            push(c, g, 0)?;
                            c.eax = c.esi;
                            return call(c, callee::C5740, LIGHT_SITES[4]);
                        }
                        c.eax = c.eax.wrapping_sub(1);
                        if c.eax != 0 {
                            0xc532b
                        } else {
                            c.eax = 3;
                            return call(c, callee::C5890, LIGHT_SITES[3]);
                        }
                    }
                }
            }
            0xc5315 => {
                g.set_u8(at(c.edi, 0x2bc), 0)?;
                return leave(c, g);
            }
            0xc532b => return leave(c, g),
            _ => return Err(unowned(pc)),
        };
    }
}

// ---------------------------------------------------------------- 114c60

/// `114c60`'s return sites.
pub(super) const DIALOGUE_SITES: [u32; 6] = [0x114d2c, 0x114d5b, 0x114d73, 0x114de8, 0x114e1c, 0x114e32];

/// `dec` a positive word timer at `[edi+o]` (`xor eax, eax; mov ax, [edi+o];
/// test ax, ax; jle; dec eax; mov [edi+o], ax`).
fn count_down(c: &mut CpuAbi, g: &mut Guest, o: u32) -> Result<()> {
    c.eax = u32::from(g.u16(at(c.edi, o))?);
    if (c.eax as u16 as i16) > 0 {
        c.eax = c.eax.wrapping_sub(1);
        g.set_u16(at(c.edi, o), c.eax as u16)?;
    }
    Ok(())
}

/// `114c60 (unit: eax)`.
pub(super) fn dialogue(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    owned(pc, points::UNIT_DIALOGUE_UPDATE.base, &DIALOGUE_SITES)?;
    // `[esp + n]` with `ebx`, `esi`, `edi` pushed (`E - c`).
    let s = |c: &CpuAbi, n: u32| c.esp.wrapping_add(n);
    let mut pc = pc;
    loop {
        pc = match pc {
            0x114c60 => {
                c.esp = c.esp.wrapping_sub(0x1c);
                c.ecx = g.u32(OBJECTS)?;
                c.edx = g.u32(at(c.ecx, 0x44))?;
                push(c, g, c.ebx)?;
                push(c, g, c.esi)?;
                c.esi = c.eax;
                c.eax = (c.eax & 0xffff).wrapping_mul(3);
                c.edx = g.u32(at(c.edx, c.eax.wrapping_mul(4)).wrapping_add(8))?;
                push(c, g, c.edi)?;
                c.edi = sx16(g.u16(at(c.edx, 0x342))?).wrapping_add(c.edx);
                c.eax = u32::from(g.u16(at(c.edi, 0x3c))?);
                g.set_u32(s(c, 0xc), c.edx)?;
                if (c.eax as u16 as i16) > 0 {
                    c.eax = c.eax.wrapping_sub(1);
                    g.set_u16(at(c.edi, 0x3c), c.eax as u16)?;
                    if c.eax as u16 == 0 {
                        c.eax = u32::from(g.u16(at(c.edi, 0x3e))?);
                        if (c.eax as u16 as i16) > 0 {
                            c.eax = c.eax.wrapping_sub(1);
                            g.set_u16(at(c.edi, 0x3e), c.eax as u16)?;
                            c.eax = g.u32(TIME)?;
                            c.ecx = sx16(g.u16(at(c.eax, 2))?);
                            sse::cvtsi2ss(c, 0, c.ecx);
                            sse::ss(c, Ss::Mul, 0, g.u32(K_DIALOGUE)?);
                            st(c, g, s(c, 0x10), 0)?;
                            fld_fistp(c, g, s(c, 0x10), s(c, 0x14), 0x114cce)?;
                            let ax = g.u16(s(c, 0x14))?;
                            c.eax = word16(c.eax, u32::from(ax));
                            g.set_u16(at(c.edi, 0x3c), ax)?;
                        }
                    }
                }
                // 114cdb
                count_down(c, g, 0x42)?;
                count_down(c, g, 0x40)?;
                c.ebx = byte(c.ebx, 1);
                if (g.u16(at(c.edi, 0xc))? as i16) <= 0 {
                    0x114e3c
                } else {
                    let al = g.u8(at(c.edi, 0x48))?;
                    c.eax = byte(c.eax, al);
                    if al != 0 {
                        0x114d7a
                    } else {
                        c.eax = g.u32(at(c.edi, 0x10))?;
                        if c.eax == NONE {
                            0x114d5f
                        } else {
                            let sound = g.u32(at(c.edi, 0x1c))?;
                            push(c, g, c.ebp)?;
                            c.ebp = g.u32(at(c.edi, 8))?;
                            if sound != NONE {
                                0x114d2c
                            } else {
                                push(c, g, 0)?;
                                c.ecx = at(c.edi, 0x18);
                                push(c, g, c.ecx)?;
                                return call(c, callee::S20B140, DIALOGUE_SITES[0]);
                            }
                        }
                    }
                }
            }
            // `ebp` pushed: `esp` is `E - 10`.
            0x114d2c => {
                c.edx = g.u32(at(c.edi, 0x10))?;
                sse::load(c, 0, g.u32(K_ONE)?);
                c.eax = at(c.edi, 0x18);
                c.ecx = s(c, 0x1c);
                g.set_u32(s(c, 0x28), c.eax)?;
                push(c, g, c.ecx)?;
                c.eax = g.u32(DIALOGUE_IMM)?;
                g.set_u32(s(c, 0x24), c.edx)?;
                st(c, g, s(c, 0x28), 0)?;
                g.set_u32(s(c, 0x20), c.ebp)?;
                return call(c, callee::S1891D0, DIALOGUE_SITES[1]);
            }
            0x114d5b => {
                g.set_u32(at(c.edi, 0x54), c.eax)?;
                c.ebp = pop(c, g)?;
                0x114d5f
            }
            0x114d5f => {
                c.eax = u32::from(g.u16(at(c.edi, 0xe))?);
                c.edx = at(c.edi, 0x20);
                push(c, g, c.edx)?;
                push(c, g, c.eax)?;
                c.eax = g.u32(at(c.edi, 0x1c))?;
                push(c, g, c.esi)?;
                return call(c, callee::S20D220, DIALOGUE_SITES[2]);
            }
            0x114d73 => {
                c.edx = g.u32(s(c, 0xc))?;
                g.set_u8(at(c.edi, 0x48), c.ebx as u8)?;
                0x114d7a
            }
            0x114d7a => {
                count_down(c, g, 0x4e)?;
                c.eax = u32::from(g.u16(at(c.edi, 0x4c))?);
                if (c.eax as u16 as i16) > 0 {
                    c.eax = c.eax.wrapping_sub(1);
                    g.set_u16(at(c.edi, 0x4c), c.eax as u16)?;
                    if c.eax as u16 == 0 {
                        g.set_u32(at(c.edi, 0x54), NONE)?;
                    }
                } else {
                    let al = g.u8(at(c.edi, 0x4a))?;
                    c.eax = byte(c.eax, al);
                    if al == 0 {
                        g.set_u8(at(c.edi, 0x4a), c.ebx as u8)?;
                    }
                    count_down(c, g, 0x50)?;
                }
                // 114dc2
                if (g.u16(at(c.edi, 0xc))? as i16) <= 0 {
                    0x114e3c
                } else if g.u16(at(c.edi, 0x4e))? != 0 {
                    0x114def
                } else {
                    let al = g.u8(at(c.edi, 0x49))?;
                    c.eax = byte(c.eax, al);
                    if al != 0 {
                        0x114def
                    } else {
                        c.eax = g.u32(at(c.edi, 0x1c))?;
                        c.edx = word16(c.edx, u32::from(g.u16(at(c.edi, 0xe))?));
                        c.ecx = at(c.edi, 0x20);
                        push(c, g, c.ecx)?;
                        push(c, g, c.esi)?;
                        return call(c, callee::S20D570, DIALOGUE_SITES[3]);
                    }
                }
            }
            0x114de8 => {
                c.edx = g.u32(s(c, 0xc))?;
                g.set_u8(at(c.edi, 0x49), c.ebx as u8)?;
                0x114def
            }
            0x114def => {
                if (g.u16(at(c.edi, 0xc))? as i16) <= 0 || g.u16(at(c.edi, 0x4c))? != 0 || g.u16(at(c.edi, 0x50))? != 0
                {
                    0x114e3c
                } else {
                    let al = g.u8(at(c.edi, 0x49))?;
                    c.eax = byte(c.eax, al);
                    if al != 0 {
                        0x114e23
                    } else {
                        c.eax = g.u32(at(c.edi, 0x1c))?;
                        c.edx = at(c.edi, 0x20);
                        push(c, g, c.edx)?;
                        c.edx = word16(c.edx, u32::from(g.u16(at(c.edi, 0xe))?));
                        push(c, g, c.esi)?;
                        return call(c, callee::S20D570, DIALOGUE_SITES[4]);
                    }
                }
            }
            0x114e1c => {
                c.edx = g.u32(s(c, 0xc))?;
                g.set_u8(at(c.edi, 0x49), c.ebx as u8)?;
                0x114e23
            }
            0x114e23 => {
                c.ecx = g.u32(at(c.edi, 0x1c))?;
                if c.ecx == NONE {
                    0x114e36
                } else {
                    c.eax = c.esi;
                    return call(c, callee::S20FD50, DIALOGUE_SITES[5]);
                }
            }
            0x114e32 => {
                c.edx = g.u32(s(c, 0xc))?;
                0x114e36
            }
            0x114e36 => {
                g.set_u16(at(c.edi, 0xc), 0)?;
                0x114e3c
            }
            0x114e3c => {
                sse::zero(c, 0);
                sse::ss(c, Ss::Sub, 0, g.u32(at(c.edx, 0x208))?);
                sse::load(c, 1, g.u32(K_STEP_LOW)?);
                // `comiss xmm1, xmm0; ja`.
                sse::comiss(c, sse::lane0(c, 1), sse::lane0(c, 0));
                let under = !sse::below_or_equal(c);
                c.edi = pop(c, g)?;
                c.esi = pop(c, g)?;
                c.ebx = pop(c, g)?;
                if under {
                    sse::movaps(c, 0, 1);
                } else {
                    sse::load(c, 1, g.u32(K_STEP_HIGH)?);
                    if !below_or_equal(c, sse::lane0(c, 0), sse::lane0(c, 1)) {
                        sse::movaps(c, 0, 1);
                    }
                }
                // 114e67
                sse::load(c, 1, g.u32(at(c.edx, 0x208))?);
                sse::ss_reg(c, Ss::Add, 1, 0);
                st(c, g, at(c.edx, 0x208), 1)?;
                c.esp = c.esp.wrapping_add(0x1c);
                return ret(c, 0);
            }
            _ => return Err(unowned(pc)),
        };
    }
}

#[cfg(test)]
mod tests;
