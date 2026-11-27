//! `1792b0 (effect: stack 0, dt: stack 1) -> al`, `ret 8`: one effect's
//! event clock (docs/engine/effects-parts.md, "The per-effect bodies").
//!
//! Frame: `sub esp, 0xc`, then `ebp`, `esi`, `edi` and, past the bit 6
//! test, `ebx`. With `ebx` pushed, `[esp+0x10]` is the pass counter,
//! `[esp+0x14]` the time this pass uses, `[esp+0x18]` the clock before it,
//! `[esp+0x20]` the handle and `[esp+0x24]` the time left (the `dt` slot).
//!
//! Each pass (at most 8, while the time left is not negative and flag bit
//! 2 is clear) moves the clock `+60` towards the event's end `+64`. Then:
//!
//! - in an event (bit 0): `179020(handle, before, used)` unless bit 3,
//!   `179fb0` unless bit 3, and at the event's end `179190(datum)`, whose
//!   answer is the next event (`1782a0`, `edi` the event, `edx` the
//!   handle) or past the last, the end (`177610`, `eax` the handle);
//! - between events, at the delay's end: bit 0 set, the clock zeroed, `+68`
//!   the end-of-step value, and the end `+64` the event's `+10`, or a draw
//!   `259d0(lo, hi)` on the stream `177c20` returns (its `fstp` stored by
//!   Rust); then `17a380(datum)` unless bit 3, `17add0` (`edx` the
//!   datum) and `+5e` = `+5c`.
//!
//! The result is `esi == 0`: 1 only when `177610` retired the effect. Bit
//! 1 of `+3` is cleared on every other way out past the bit 6 test.
//!
//! No draw moves: `259d0` is the guest's, called where retail calls it.
//! Flags: each call carries its last flag-setting instruction's (the
//! `test`s, the `cmp eax, [ebp+0x14]`, the `test ah, 0x44` after the
//! `ucomiss`); the return carries `add esp, 0xc`'s.

use super::{at, unowned};
use crate::sim_objects::projectile_step::store_f32;
use crate::sim_objects::sse::{self, Ss};
use crate::sim_objects::alu::shr;
use crate::sim_objects::{Cx, Next, Result};
use crate::sim_physics::{arithmetic, byte, logic, pop, push};
use runtime::CpuAbi;

pub(crate) const ENTRY: u32 = 0x0017_92b0;
const ARRAY: u32 = 0x004e_a93c;
const TAGS: u32 = 0x004e_3b44;
/// The end-of-step value: a new event's `+68`, and the time left once a
/// pass stops short of the event's end.
const END_OF_STEP: u32 = 0x0045_dbcc;

/// `EFFECT_STEP_EMITTERS`, a callout (Halo 1 hooks it).
const EMITTERS: u32 = 0x0017_9020;
const LOCATIONS: u32 = 0x0017_9fb0;
const SELECT: u32 = 0x0017_9190;
const START: u32 = 0x0017_82a0;
const STREAM: u32 = 0x0017_7c20;
const DRAW: u32 = 0x0002_59d0;
const PARTS: u32 = 0x0017_a380;
const BEAMS: u32 = 0x0017_add0;
const RETIRE: u32 = 0x0017_7610;

const EMITTERS_SITE: u32 = 0x0017_93ae;
const LOCATIONS_SITE: u32 = 0x0017_93be;
const SELECT_SITE: u32 = 0x0017_93cc;
const START_SITE: u32 = 0x0017_93e0;
const STREAM_SITE: u32 = 0x0017_9430;
const DRAW_SITE: u32 = 0x0017_9437;
const PARTS_SITE: u32 = 0x0017_944b;
const BEAMS_SITE: u32 = 0x0017_9452;
const RETIRE_SITE: u32 = 0x0017_9481;

/// The return addresses of its calls.
pub(crate) const SITES: [u32; 9] = [
    EMITTERS_SITE,
    LOCATIONS_SITE,
    SELECT_SITE,
    START_SITE,
    STREAM_SITE,
    DRAW_SITE,
    PARTS_SITE,
    BEAMS_SITE,
    RETIRE_SITE,
];

/// The labels a segment runs through between transfers.
#[derive(Clone, Copy)]
enum Label {
    /// `179325`: a pass, `xmm0` the time left.
    Pass,
    /// `1793ae`: the locations, after the emitters.
    Locations,
    /// `1793be`: the next event at the event's end.
    Select,
    /// `17943a`: the parts.
    Parts,
    /// `17944b`: the beams.
    Beams,
    /// `17945a`: the pass's end.
    Next,
    /// `17930a`: clear bit 1 of `+3`, pop `ebx`, leave.
    Clear,
}

/// `17930f`: `xor eax, eax; pop edi; test esi, esi; pop esi; sete al;
/// pop ebp; add esp, 0xc; ret 8`.
fn leave(cx: &mut Cx<'_>) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    c.eax = u32::from(c.esi == 0);
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    let v = c.esp;
    c.esp = arithmetic(c, v, 0xc, false);
    Ok(Next::Ret { pop: 8 })
}

fn set_bl(c: &mut CpuAbi, v: u8) {
    c.ebx = byte(c.ebx, v);
}

pub(crate) fn update(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let mut label = match pc {
        ENTRY => {
            let (c, g) = (&mut *cx.cpu, &mut *cx.g);
            c.eax = g.u32(ARRAY)?;
            c.ecx = g.u32(at(c.eax, 0x44))?;
            c.esp = c.esp.wrapping_sub(0xc);
            c.edx = g.u32(TAGS)?;
            for v in [c.ebp, c.esi, c.edi] {
                push(c, g, v)?;
            }
            c.edi = g.u32(at(c.esp, 0x1c))?;
            c.esi = (c.edi & 0xffff).wrapping_mul(0x190);
            let flags = g.u8(c.esi.wrapping_add(c.ecx).wrapping_add(2))?;
            c.esi = c.esi.wrapping_add(c.ecx);
            c.ecx = (g.u32(at(c.esi, 4))? & 0xffff) << 4;
            c.ebp = g.u32(c.ecx.wrapping_add(c.edx).wrapping_add(8))?;
            let al = shr(c, u32::from(flags), 6, 8);
            c.eax = byte(c.eax, al as u8);
            if al & 1 != 0 {
                return leave(cx);
            }
            sse::load(c, 0, g.u32(at(c.esp, 0x20))?);
            sse::zero(c, 2);
            let f = sse::comiss(c, sse::lane0(c, 0), 0);
            let v = c.ebx;
            push(c, g, v)?;
            g.set_u32(at(c.esp, 0x10), 0)?;
            if f & 1 == 0 { Label::Pass } else { Label::Clear }
        }
        EMITTERS_SITE => Label::Locations,
        LOCATIONS_SITE => Label::Select,
        SELECT_SITE => {
            let (c, g) = (&mut *cx.cpu, &mut *cx.g);
            let count = g.u32(at(c.ebp, 0x14))?;
            arithmetic(c, c.eax, count, true);
            if (c.eax as i32) >= (count as i32) {
                c.eax = c.edi;
                return Ok(Next::Call { target: RETIRE, site: RETIRE_SITE });
            }
            c.edx = g.u32(at(c.esp, 0x20))?;
            c.edi = c.eax;
            return Ok(Next::Call { target: START, site: START_SITE });
        }
        START_SITE => {
            cx.cpu.edi = cx.g.u32(at(cx.cpu.esp, 0x20))?;
            Label::Next
        }
        STREAM_SITE => {
            cx.cpu.ecx = cx.cpu.eax;
            return Ok(Next::Call { target: DRAW, site: DRAW_SITE });
        }
        DRAW_SITE => {
            let (c, g) = (&mut *cx.cpu, &mut *cx.g);
            let to = at(c.esi, 0x64);
            store_f32(c, g, DRAW_SITE, to, true)?;
            Label::Parts
        }
        PARTS_SITE => Label::Beams,
        BEAMS_SITE => {
            let (c, g) = (&mut *cx.cpu, &mut *cx.g);
            let current = g.u16(at(c.esi, 0x5c))?;
            c.edx = (c.edx & 0xffff_0000) | u32::from(current);
            g.set_u16(at(c.esi, 0x5e), current)?;
            Label::Next
        }
        RETIRE_SITE => {
            let (c, g) = (&mut *cx.cpu, &mut *cx.g);
            if c.eax as u8 == 0 {
                Label::Clear
            } else {
                c.ebx = pop(c, g)?;
                c.esi = 0;
                return leave(cx);
            }
        }
        _ => return Err(unowned(pc)),
    };
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    loop {
        label = match label {
            Label::Pass => {
                let word = u32::from(g.u16(at(c.esi, 2))?);
                c.eax = (c.eax & 0xffff_0000) | word;
                c.ecx = byte(c.ecx, (word as u8) >> 2);
                if c.ecx & 1 != 0 || (g.u32(at(c.esp, 0x10))? as i32) >= 8 {
                    Label::Clear
                } else {
                    let dt = sse::lane0(c, 0);
                    let clock = g.u32(at(c.esi, 0x60))?;
                    sse::load(c, 1, g.u32(at(c.esi, 0x64))?);
                    let until_end = sse::ss(c, Ss::Sub, 1, clock);
                    let f = sse::comiss(c, until_end, dt);
                    sse::load(c, 3, clock);
                    g.set_u32(at(c.esp, 0x18), clock)?;
                    g.set_u32(at(c.esp, 0x14), dt)?;
                    if f & 0x41 != 0 {
                        g.set_u32(at(c.esp, 0x14), until_end)?;
                    }
                    if sse::comiss(c, dt, until_end) & 1 == 0 {
                        // The event's end: the clock is the end.
                        c.edx = g.u32(at(c.esi, 0x64))?;
                        set_bl(c, 1);
                        g.set_u32(at(c.esi, 0x60), c.edx)?;
                        sse::ss(c, Ss::Sub, 0, until_end);
                    } else {
                        sse::load(c, 1, g.u32(at(c.esi, 0x60))?);
                        let moved = sse::ss(c, Ss::Add, 1, dt);
                        sse::load(c, 0, g.u32(END_OF_STEP)?);
                        set_bl(c, 0);
                        g.set_u32(at(c.esi, 0x60), moved)?;
                    }
                    let in_event = g.u8(at(c.esi, 2))? & 1 != 0;
                    logic(c, u32::from(in_event), 0x80);
                    g.set_u32(at(c.esp, 0x24), sse::lane0(c, 0))?;
                    if in_event {
                        let al = shr(c, c.eax, 3, 8);
                        c.eax = byte(c.eax, al as u8);
                        logic(c, al & 1, 0x80);
                        if al & 1 != 0 {
                            Label::Locations
                        } else {
                            c.eax = g.u32(at(c.esp, 0x14))?;
                            c.ecx = g.u32(at(c.esp, 0x18))?;
                            for v in [c.eax, c.ecx, c.edi] {
                                push(c, g, v)?;
                            }
                            return Ok(Next::Call { target: EMITTERS, site: EMITTERS_SITE });
                        }
                    } else if c.ebx as u8 == 0 {
                        Label::Next
                    } else {
                        // A delay's end: the event starts.
                        let event = g.u16(at(c.esi, 0x5c))? as i16 as i32 as u32;
                        c.eax = g.u32(at(c.ebp, 0x18))?;
                        c.ecx = event.wrapping_mul(0x38);
                        let flags = at(c.esi, 2);
                        g.set_u8(flags, g.u8(flags)? | 1)?;
                        sse::load(c, 0, g.u32(END_OF_STEP)?);
                        c.ecx = c.ecx.wrapping_add(c.eax);
                        g.set_u32(at(c.esi, 0x60), sse::lane0(c, 2))?;
                        g.set_u32(at(c.esi, 0x68), sse::lane0(c, 0))?;
                        let lo = g.u32(at(c.ecx, 0x10))?;
                        let hi = g.u32(at(c.ecx, 0x14))?;
                        sse::load(c, 0, lo);
                        sse::ucomiss(c, lo, hi);
                        sse::lahf(c);
                        logic(c, (c.eax >> 8) & 0x44, 0x80);
                        c.eax = lo;
                        if !sse::parity(c) {
                            g.set_u32(at(c.esi, 0x64), lo)?;
                            Label::Parts
                        } else {
                            c.edx = hi;
                            push(c, g, hi)?;
                            push(c, g, lo)?;
                            c.eax = g.u32(at(c.esi, 4))?;
                            return Ok(Next::Call { target: STREAM, site: STREAM_SITE });
                        }
                    }
                }
            }
            Label::Locations => {
                let dl = g.u8(at(c.esi, 2))? >> 3;
                c.edx = byte(c.edx, dl);
                logic(c, u32::from(dl & 1), 0x80);
                if dl & 1 == 0 {
                    return Ok(Next::Call { target: LOCATIONS, site: LOCATIONS_SITE });
                }
                Label::Select
            }
            Label::Select => {
                logic(c, c.ebx & 0xff, 0x80);
                if c.ebx as u8 == 0 {
                    Label::Next
                } else {
                    let v = c.esi;
                    push(c, g, v)?;
                    return Ok(Next::Call { target: SELECT, site: SELECT_SITE });
                }
            }
            Label::Parts => {
                let cl = g.u8(at(c.esi, 2))? >> 3;
                c.ecx = byte(c.ecx, cl);
                logic(c, u32::from(cl & 1), 0x80);
                if cl & 1 == 0 {
                    let v = c.esi;
                    push(c, g, v)?;
                    return Ok(Next::Call { target: PARTS, site: PARTS_SITE });
                }
                Label::Beams
            }
            Label::Beams => {
                c.edx = c.esi;
                return Ok(Next::Call { target: BEAMS, site: BEAMS_SITE });
            }
            Label::Next => {
                let count = g.u32(at(c.esp, 0x10))?.wrapping_add(1);
                c.eax = count;
                let left = g.u32(at(c.esp, 0x24))?;
                sse::load(c, 0, left);
                sse::zero(c, 2);
                let f = sse::comiss(c, left, 0);
                g.set_u32(at(c.esp, 0x10), count)?;
                if f & 1 == 0 {
                    // `17931f`: the time left again, then a pass.
                    sse::load(c, 0, left);
                    Label::Pass
                } else {
                    Label::Clear
                }
            }
            Label::Clear => {
                let flags = at(c.esi, 3);
                g.set_u8(flags, g.u8(flags)? & 0xfd)?;
                c.ebx = pop(c, g)?;
                return leave(cx);
            }
        };
    }
}
