//! `fe5ff..fefee`: the trigger loop. For trigger `i` (index at `[S+28]`):
//! `esi` the trigger definition (definition `+2cc` + `i` × `40`), `ebx`
//! `i` × `c`, the trigger state at body `+20c + ebx`: `+20c` the state
//! byte, `+20e` a countdown word, `+210` flag bits, `+214` an effect
//! handle. Field names L.
//!
//! Each iteration counts the word down, reads the trigger's two input
//! bytes (`[S+14]` down, `[S+15]` held: the frame bytes `[S+1c..+21]`
//! chosen by the trigger definition's `+4`, and down when state bit 7
//! is set) unless `[S+16]` (not ready), then runs the state's arm:
//!
//! | State | Arm | Calls |
//! |---|---|---|
//! | 0 idle | `fe693`, by the trigger definition's `+6` (`ff59c`) | `102540` (start barrel `+8` or `+0a`), `102f70` (barrel stop), `103a90`/`103ce0`/`103b10`/`103bd0`/`103c40` (charge and fire), `1027b0`, `106280` |
//! | 1 | `fec28` | `102d60` when the countdown is zero; else the stop of barrel `+8`, effect `+214` ended by `177260(1)` |
//! | 2 | `fecf2` | `102e10`, `102f10`; or the stop and `177260(1)` |
//! | 3, 7 | `fed94` | `106280`, `103c40`; or, for 7, the stop path of 5 |
//! | 4 | none | |
//! | 5 | `feddd` | `102540`, `106280`, `103bd0`/`103c40`, `103b70` |
//! | 6 | `feec6` | `102540`, `1061c0`, `103bd0`, `103b70` |
//!
//! Then `[S+13]` is set when the state is not zero or an input is on.
//! Barrel `n`'s state is body `+1a4 + n × 34`; marking it is `or byte
//! [+4], 1` ([`barrel`]).

use super::*;

/// The state arms (`jmp [ff57c + state × 4]`).
const STATES: [u32; 8] = [0xfe693, 0xfec28, 0xfecf2, 0xfed94, 0xfefb0, 0xfeddd, 0xfeec6, 0xfed94];
/// State 0 by the trigger definition's `+6` (`ff59c`).
const IDLE: [u32; 6] = [0xfe6a7, 0xfe73c, 0xfe92d, 0xfebcf, 0xfe7b5, 0xfe854];
/// After `1027b0`'s first out byte, by the trigger definition's `+1a`
/// (`ff5b4`), and its second, by `+18` (`ff5c4`).
const FIRST: [u32; 4] = [0xfe9d8, 0xfea63, 0xfea81, 0xfea21];
const SECOND: [u32; 4] = [0xfeab5, 0xfeb40, 0xfeb5e, 0xfeaff];

const START: u32 = 0x102540;
const STOP: u32 = 0x102f70;
const CAN_FIRE: u32 = 0x103a90;
const FIRE: u32 = 0x103ce0;
const OWNER_FIRE: u32 = 0x103b10;
const OWNER_SPARE: u32 = 0x103bd0;
const OWNER_SET: u32 = 0x103c40;
const OWNER_TEST: u32 = 0x106280;
const EFFECT_END: u32 = 0x177260;

/// Which register holds the barrel index in a [`barrel`] sequence; the
/// other two hold the header offset and the body as retail leaves them.
#[derive(Clone, Copy)]
enum Idx {
    /// `movsx eax`; `ecx` the objects global then `[esp+24]`, `edx` the
    /// datum array then the body.
    Eax,
    /// `movsx ecx`; `edx` the objects global then `[esp+24]`, `eax` the
    /// datum array then the body.
    Ecx,
    /// `movsx edx`; `eax` the objects global then `[esp+24]`, `ecx` the
    /// datum array then the body.
    Edx,
}

/// `movsx r, word [esi+field]; ...; imul r, 34h; ...; [or byte
/// [body+r+1a8h], 1;] lea eax, [body+r+1a4h]`: the barrel state the
/// trigger definition names, through the re-read body.
fn barrel(c: &mut CpuAbi, g: &mut Guest, idx: Idx, field: u32, mark: bool) -> Result<()> {
    let off = (g.u16(at(c.esi, field))? as i16 as i32 as u32).wrapping_mul(0x34);
    let header = slot(c, g, 0x24)?;
    let body = body_now(c, g)?;
    if mark {
        rmw8(c, g, at(at(body, off), 0x1a8), |f| f | 1)?;
    }
    match idx {
        Idx::Eax => {
            c.ecx = header;
            c.edx = body;
        }
        Idx::Ecx => {
            c.ecx = off;
            c.edx = header;
        }
        Idx::Edx => {
            c.edx = off;
            c.ecx = body;
        }
    }
    c.eax = at(at(body, off), 0x1a4);
    Ok(())
}

/// `xor eax, eax; mov ax, [esi+field]; cmp ax, 0FFFFh`: true when the
/// trigger names no barrel there.
fn no_barrel(c: &mut CpuAbi, g: &Guest, field: u32) -> Result<bool> {
    c.eax = u32::from(g.u16(at(c.esi, field))?);
    super::super::alu::cmp16(c, c.eax, 0xffff);
    Ok(c.eax == 0xffff)
}

/// `push 0; push eax; push object; call 102540`.
fn start(c: &mut CpuAbi, g: &mut Guest, object: u32, site: u32) -> R {
    push(c, g, 0)?;
    push(c, g, c.eax)?;
    push(c, g, object)?;
    call(START, site)
}

/// The trigger's state dword at body `+210`.
fn bits(c: &CpuAbi) -> u32 {
    at(at(c.ebx, c.ebp), 0x210)
}

/// `mov al, [esp+off]; test al, al`.
fn flag(c: &mut CpuAbi, g: &Guest, off: u32) -> Result<bool> {
    set_al(c, slot8(c, g, off)?);
    Ok(test8(c, c.eax))
}

/// `fec96`/`fed46`/`fee39`/`fef19`: the re-read body into `eax`, then
/// (from `fef2a`) the state byte and the countdown cleared.
fn reset(c: &mut CpuAbi, g: &mut Guest) -> Result<()> {
    c.eax = body_now(c, g)?;
    g.set_u8(at(at(c.ebx, c.eax), 0x20c), 0)?;
    g.set_u16(at(at(c.ebx, c.eax), 0x20e), 0)
}

pub(super) fn step(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> R {
    match pc {
        0xfe5ff => {
            c.ecx = g.u32(at(c.edi, 0x2c8))?;
            c.eax = 0;
            arithmetic(c, c.ecx, c.eax, true);
            set_slot(c, g, 0x28, c.eax)?;
            if less_or_equal(c) {
                return go(BARRELS);
            }
            go(0xfe613)
        }
        0xfe613 => {
            c.ecx = g.u32(at(c.edi, 0x2cc))?;
            c.esi = c.eax << 6;
            c.ebx = c.eax.wrapping_mul(3);
            c.esi = c.esi.wrapping_add(c.ecx);
            c.ebx <<= 2;
            c.ecx = u32::from(g.u16(at(at(c.ebx, c.ebp), 0x20e))?);
            super::super::alu::test16(c, c.ecx);
            set_slot8(c, g, 0x14, 0)?;
            set_slot8(c, g, 0x15, 0)?;
            if !less_or_equal(c) {
                c.ecx = super::super::alu::dec32(c, c.ecx);
                g.set_u16(at(at(c.ebx, c.ebp), 0x20e), c.ecx as u16)?;
                set_slot8(c, g, 0x13, 1)?;
            }
            // `fe64d`: the inputs, unless not ready.
            c.ecx = byte(c.ecx, slot8(c, g, 0x16)?);
            if !test8(c, c.ecx) {
                c.edi = g.u16(at(c.esi, 4))? as i16 as i32 as u32;
                c.ecx = byte(c.ecx, g.u8(at(at(c.esp, c.edi), 0x1c))?);
                c.edx = byte(c.edx, g.u8(at(at(c.esp, c.edi), 0x20))?);
                set_slot8(c, g, 0x14, c.ecx as u8)?;
                c.ecx = byte(c.ecx, g.u8(at(at(c.ebx, c.ebp), 0x210))?);
                test8(c, c.ecx);
                set_slot8(c, g, 0x15, c.edx as u8)?;
                if c.ecx & 0x80 != 0 {
                    set_slot8(c, g, 0x14, 1)?;
                }
            }
            // `fe679`.
            c.edx = byte(c.edx, g.u8(at(at(c.ebx, c.ebp), 0x20c))?);
            c.ecx = c.edx as u8 as i8 as i32 as u32;
            arithmetic(c, c.ecx, 7, true);
            if above(c) {
                return go(0xfefb0);
            }
            go(STATES[c.ecx as usize])
        }
        0xfe693 => {
            c.ecx = g.u16(at(c.esi, 6))? as i16 as i32 as u32;
            arithmetic(c, c.ecx, 5, true);
            if above(c) {
                return go(0xfefb0);
            }
            go(IDLE[c.ecx as usize])
        }
        0xfe6a7 => {
            if !flag(c, g, 0x14)? && !flag(c, g, 0x15)? {
                return go(0xfe70e);
            }
            if no_barrel(c, g, 8)? {
                return go(0xfe70e);
            }
            push(c, g, 0)?;
            push(c, g, c.eax)?;
            c.eax = slot(c, g, 0x54)?;
            push(c, g, c.eax)?;
            call(START, 0xfe6d0)
        }
        0xfe6d0 => {
            if !test8(c, c.eax) {
                return go(0xfe70e);
            }
            let state = bits(c);
            c.edx = g.u32(state)?;
            c.ecx = g.u32(points::OBJECTS.base)?;
            c.edx |= 0x20;
            logic(c, c.edx, 1 << 31);
            g.set_u32(state, c.edx)?;
            barrel(c, g, Idx::Eax, 8, true)?;
            go(0xfe716)
        }
        0xfe70e => {
            rmw32(c, g, bits(c), |f| f & 0xffff_ffdf)?;
            go(0xfe716)
        }
        0xfe716 => {
            if flag(c, g, 0x14)? && !flag(c, g, 0x15)? {
                return go(0xfefb0);
            }
            c.ecx = word(c.ecx, g.u16(at(c.esi, 8))?);
            c.eax = slot(c, g, 0x4c)?;
            call(STOP, 0xfe737)
        }
        0xfe737 => go(0xfefb0),
        0xfe73c => {
            if !flag(c, g, 0x14)? {
                rmw32(c, g, bits(c), |f| f | 1)?;
                c.ecx = word(c.ecx, g.u16(at(c.esi, 8))?);
                c.eax = slot(c, g, 0x4c)?;
                return call(STOP, 0xfe7b0);
            }
            if !test_m8(c, g, bits(c), 1)? {
                return go(0xfefb0);
            }
            if no_barrel(c, g, 8)? {
                return go(0xfefb0);
            }
            push(c, g, 0)?;
            push(c, g, c.eax)?;
            c.eax = slot(c, g, 0x54)?;
            push(c, g, c.eax)?;
            go(0xfe76a)
        }
        0xfe76a => call(START, 0xfe76f),
        0xfe76f => {
            if !test8(c, c.eax) {
                return go(0xfefb0);
            }
            barrel(c, g, Idx::Eax, 8, false)?;
            go(0xfe909)
        }
        0xfe7b0 => go(0xfefb0),
        0xfe7b5 => {
            if !flag(c, g, 0x14)? {
                return go(0xfe91a);
            }
            if !test_m8(c, g, bits(c), 1)? {
                return go(0xfefb0);
            }
            let alt = (g.u8(at(c.ebp, 0x16e))? >> 6) & 1;
            set_al(c, alt);
            test8(c, c.eax);
            set_slot8(c, g, 0x1b, alt)?;
            let field = if alt != 0 { 0xa } else { 8 };
            if no_barrel(c, g, field)? {
                return go(0xfefb0);
            }
            c.ecx = slot(c, g, 0x4c)?;
            start(c, g, c.ecx, if alt != 0 { 0xfe846 } else { 0xfe7fd })
        }
        0xfe7fd | 0xfe846 => {
            if !test8(c, c.eax) {
                return go(0xfefb0);
            }
            let field = if pc == 0xfe846 { 0xa } else { 8 };
            barrel(c, g, Idx::Ecx, field, false)?;
            go(0xfe909)
        }
        0xfe854 => {
            if !flag(c, g, 0x14)? {
                return go(0xfe91a);
            }
            if !test_m8(c, g, bits(c), 1)? {
                return go(0xfefb0);
            }
            // `push 0; push 0; push edi`: no barrel, `eax` untouched.
            c.edi = slot(c, g, 0x4c)?;
            push(c, g, 0)?;
            push(c, g, 0)?;
            push(c, g, c.edi)?;
            call(START, 0xfe87c)
        }
        0xfe87c => {
            if !test8(c, c.eax) {
                return go(0xfefb0);
            }
            if cmp32(c, slot(c, g, 0x30)?, NONE) {
                // `fe8ca`.
                if no_barrel(c, g, 0xa)? {
                    return go(0xfefb0);
                }
                return start(c, g, c.edi, 0xfe8e3);
            }
            c.ecx = at(c.esp, 0x1b);
            push(c, g, c.ecx)?;
            c.edx = at(c.esp, 0x48);
            push(c, g, c.edx)?;
            c.eax = c.edi;
            call(OWNER_TEST, 0xfe89c)
        }
        0xfe89c => {
            if test8(c, c.eax) {
                c.eax = slot(c, g, 0x28)?;
                push(c, g, c.eax)?;
                c.eax = c.edi;
                return call(OWNER_SET, 0xfe8ac);
            }
            // `fe8b1`.
            if no_barrel(c, g, 8)? {
                return go(0xfefb0);
            }
            push(c, g, 0)?;
            push(c, g, c.eax)?;
            push(c, g, c.edi)?;
            go(0xfe76a)
        }
        0xfe8ac => go(0xfefb0),
        0xfe8e3 => {
            if !test8(c, c.eax) {
                return go(0xfefb0);
            }
            barrel(c, g, Idx::Edx, 0xa, false)?;
            go(0xfe909)
        }
        0xfe909 => {
            rmw8(c, g, at(c.eax, 4), |f| f | 1)?;
            rmw32(c, g, bits(c), |f| f & 0xffff_fffe)?;
            go(0xfefb0)
        }
        0xfe91a => {
            c.eax = g.u32(bits(c))?;
            c.edi = slot(c, g, 0x4c)?;
            c.eax |= 1;
            logic(c, c.eax, 1 << 31);
            go(0xfeb93)
        }
        0xfe92d => {
            c.ecx = byte(c.ecx, slot8(c, g, 0x16)?);
            if test8(c, c.ecx) {
                return go(0xfebb5);
            }
            c.ecx = byte(c.ecx, g.u8(at(c.ebp, 0x170))?);
            if test8(c, c.ecx) {
                return go(0xfe996);
            }
            c.ecx = byte(c.ecx, g.u8(at(c.ebp, 0x171))?);
            if test8(c, c.ecx) {
                return go(0xfe996);
            }
            if !flag(c, g, 0x15)? {
                c.edi = slot(c, g, 0x4c)?;
                return go(0xfeb89);
            }
            if !cmp_m16(c, g, at(c.esi, 0x18), 1)? {
                return go(0xfefb0);
            }
            c.esi = slot(c, g, 0x28)?;
            c.edi = slot(c, g, 0x4c)?;
            push(c, g, c.esi)?;
            c.eax = c.edi;
            call(CAN_FIRE, 0xfe970)
        }
        0xfe970 => {
            if !test8(c, c.eax) {
                return go(0xfefb0);
            }
            push(c, g, c.esi)?;
            c.ecx = c.edi;
            call(FIRE, 0xfe980)
        }
        0xfe980 => {
            rmw32(c, g, bits(c), |f| f | 0x40)?;
            go(0xfefb0)
        }
        0xfe996 => {
            c.edi = slot(c, g, 0x4c)?;
            c.edx = at(c.esp, 0x1a);
            push(c, g, c.edx)?;
            c.ecx = at(c.esp, 0x1d);
            push(c, g, c.ecx)?;
            c.edx = at(c.esp, 0x20);
            push(c, g, c.edx)?;
            c.ecx = c.edi;
            call(0x1027b0, 0xfe9b0)
        }
        0xfe9b0 => {
            if !flag(c, g, 0x18)? {
                return go(0xfea8d);
            }
            rmw32(c, g, bits(c), |f| f | 8)?;
            c.eax = g.u16(at(c.esi, 0x1a))? as i16 as i32 as u32;
            arithmetic(c, c.eax, 3, true);
            if above(c) {
                return go(0xfea8d);
            }
            go(FIRST[c.eax as usize])
        }
        0xfe9d8 | 0xfea21 => {
            let field = if pc == 0xfea21 { 0xa } else { 8 };
            if no_barrel(c, g, field)? {
                return go(0xfea8d);
            }
            start(c, g, c.edi, if pc == 0xfea21 { 0xfea36 } else { 0xfe9f1 })
        }
        0xfe9f1 => {
            if test8(c, c.eax) {
                barrel(c, g, Idx::Edx, 8, true)?;
            }
            go(0xfea8d)
        }
        0xfea36 => {
            if test8(c, c.eax) {
                barrel(c, g, Idx::Ecx, 0xa, true)?;
            }
            go(0xfea8d)
        }
        0xfea63 => {
            c.ecx = slot(c, g, 0x28)?;
            push(c, g, c.ecx)?;
            c.eax = c.edi;
            call(CAN_FIRE, 0xfea6f)
        }
        0xfea6f => {
            if !test8(c, c.eax) {
                return go(0xfea8d);
            }
            c.edx = slot(c, g, 0x28)?;
            push(c, g, c.edx)?;
            c.ecx = c.edi;
            call(FIRE, 0xfea7f)
        }
        0xfea7f => go(0xfea8d),
        0xfea81 => {
            c.eax = slot(c, g, 0x28)?;
            push(c, g, c.eax)?;
            c.eax = c.edi;
            call(OWNER_FIRE, 0xfea8d)
        }
        0xfea8d => {
            if !flag(c, g, 0x19)? {
                return go(0xfeb7d);
            }
            rmw32(c, g, bits(c), |f| f | 0x10)?;
            c.eax = g.u16(at(c.esi, 0x18))? as i16 as i32 as u32;
            arithmetic(c, c.eax, 3, true);
            if above(c) {
                return go(0xfeb7d);
            }
            go(SECOND[c.eax as usize])
        }
        0xfeab5 | 0xfeaff => {
            let field = if pc == 0xfeab5 { 0xa } else { 8 };
            if no_barrel(c, g, field)? {
                return go(0xfeb7d);
            }
            start(c, g, c.edi, if pc == 0xfeab5 { 0xfeace } else { 0xfeb14 })
        }
        0xfeace => {
            if test8(c, c.eax) {
                barrel(c, g, Idx::Eax, 0xa, true)?;
            }
            go(0xfeb7d)
        }
        0xfeb14 => {
            if test8(c, c.eax) {
                barrel(c, g, Idx::Edx, 8, true)?;
            }
            go(0xfeb7d)
        }
        0xfeb40 => {
            c.edx = slot(c, g, 0x28)?;
            push(c, g, c.edx)?;
            c.eax = c.edi;
            call(CAN_FIRE, 0xfeb4c)
        }
        0xfeb4c => {
            if !test8(c, c.eax) {
                return go(0xfeb7d);
            }
            c.eax = slot(c, g, 0x28)?;
            push(c, g, c.eax)?;
            c.ecx = c.edi;
            call(FIRE, 0xfeb5c)
        }
        0xfeb5c | 0xfeb71 => go(0xfeb7d),
        0xfeb5e => {
            let none = cmp32(c, slot(c, g, 0x30)?, NONE);
            c.eax = c.edi;
            if none {
                c.edx = slot(c, g, 0x28)?;
                push(c, g, c.edx)?;
                return call(OWNER_SPARE, 0xfeb7d);
            }
            c.ecx = slot(c, g, 0x28)?;
            push(c, g, c.ecx)?;
            call(OWNER_FIRE, 0xfeb71)
        }
        0xfeb7d => {
            if !flag(c, g, 0x1a)? {
                return go(0xfefb0);
            }
            go(0xfeb89)
        }
        0xfeb89 => {
            c.eax = g.u32(bits(c))? & 0xffff_ffe7;
            logic(c, c.eax, 1 << 31);
            go(0xfeb93)
        }
        0xfeb93 => {
            g.set_u32(bits(c), c.eax)?;
            c.ecx = word(c.ecx, g.u16(at(c.esi, 8))?);
            c.eax = c.edi;
            call(STOP, 0xfeba5)
        }
        0xfeba5 => {
            c.ecx = word(c.ecx, g.u16(at(c.esi, 0xa))?);
            c.eax = c.edi;
            call(STOP, 0xfebb0)
        }
        0xfebb0 => go(0xfefb0),
        0xfebb5 => {
            g.set_u8(at(c.ebp, 0x171), 0)?;
            g.set_u8(at(c.ebp, 0x170), 0)?;
            g.set_u8(at(c.ebp, 0x173), 0)?;
            go(0xfefb0)
        }
        0xfebcf => {
            if !flag(c, g, 0x14)? && !flag(c, g, 0x15)? {
                c.edi = slot(c, g, 0x4c)?;
                return go(0xfec18);
            }
            c.eax = slot(c, g, 0x28)?;
            c.edi = slot(c, g, 0x4c)?;
            push(c, g, c.eax)?;
            c.eax = c.edi;
            call(CAN_FIRE, 0xfebef)
        }
        0xfebef => {
            if !test8(c, c.eax) {
                return go(0xfec18);
            }
            if flag(c, g, 0x15)? {
                rmw32(c, g, bits(c), |f| f | 0x40)?;
            }
            c.ecx = slot(c, g, 0x28)?;
            push(c, g, c.ecx)?;
            c.ecx = c.edi;
            call(FIRE, 0xfec0f)
        }
        0xfec0f | 0xfec23 => go(0xfefb0),
        0xfec18 => {
            c.ecx = word(c.ecx, g.u16(at(c.esi, 8))?);
            c.eax = c.edi;
            call(STOP, 0xfec23)
        }
        // State 1.
        0xfec28 => {
            if cmp_m16(c, g, at(at(c.ebx, c.ebp), 0x20e), 0)? {
                c.ecx = slot(c, g, 0x28)?;
                c.eax = slot(c, g, 0x4c)?;
                return call(0x102d60, 0xfeced);
            }
            if flag(c, g, 0x14)? || flag(c, g, 0x15)? {
                return go(0xfefb0);
            }
            c.eax = g.u32(bits(c))?;
            if test8(c, c.eax & 0x40) {
                c.eax &= 0xffff_ffbf;
                logic(c, c.eax, 1 << 31);
                g.set_u32(bits(c), c.eax)?;
            } else {
                // `fec66`: the barrel `+8`, by `si`.
                c.esi = word(c.esi, g.u16(at(c.esi, 8))?);
                super::super::alu::cmp16(c, c.esi, 0xffff);
                if c.esi & 0xffff != 0xffff {
                    c.edx = slot(c, g, 0x24)?;
                    c.eax = body_now(c, g)?;
                    c.ecx = (c.esi as u16 as i16 as i32 as u32).wrapping_mul(0x34);
                    rmw8(c, g, at(at(c.eax, c.ecx), 0x1a8), |f| f | 1)?;
                    c.eax = at(at(c.eax, c.ecx), 0x1a4);
                }
            }
            // `fec96`.
            c.ecx = g.u32(points::OBJECTS.base)?;
            c.edx = g.u32(at(c.ecx, 0x44))?;
            reset(c, g)?;
            c.edi = g.u32(at(at(c.ebx, c.ebp), 0x214))?;
            if cmp32(c, c.edi, NONE) {
                return go(0xfefb0);
            }
            push(c, g, 1)?;
            call(EFFECT_END, 0xfecd0)
        }
        0xfecd0 => {
            g.set_u32(at(at(c.ebx, c.ebp), 0x214), NONE)?;
            go(0xfefb0)
        }
        0xfeced => go(0xfefb0),
        // State 2.
        0xfecf2 => {
            if test_m8(c, g, bits(c), 0x40)? {
                // `fed3a`.
                if flag(c, g, 0x15)? {
                    return go(0xfefb0);
                }
                c.ecx = g.u32(points::OBJECTS.base)?;
                c.edx = g.u32(at(c.ecx, 0x44))?;
                reset(c, g)?;
                c.edi = g.u32(at(at(c.ebx, c.ebp), 0x214))?;
                if cmp32(c, c.edi, NONE) {
                    return go(0xfed87);
                }
                push(c, g, 1)?;
                return call(EFFECT_END, 0xfed7c);
            }
            if flag(c, g, 0x14)? || flag(c, g, 0x16)? {
                return go(0xfed19);
            }
            c.edx = slot(c, g, 0x28)?;
            c.eax = slot(c, g, 0x4c)?;
            call(0x102e10, 0xfed19)
        }
        0xfed19 => {
            if !cmp_m16(c, g, at(at(c.ebx, c.ebp), 0x20e), 0)? {
                return go(0xfefb0);
            }
            c.edx = slot(c, g, 0x28)?;
            c.eax = slot(c, g, 0x4c)?;
            call(0x102f10, 0xfed35)
        }
        0xfed35 => go(0xfefb0),
        0xfed7c => {
            g.set_u32(at(at(c.ebx, c.ebp), 0x214), NONE)?;
            go(0xfed87)
        }
        0xfed87 => {
            rmw32(c, g, bits(c), |f| f & 0xffff_ffbf)?;
            go(0xfefb0)
        }
        // States 3 and 7.
        0xfed94 => {
            if flag(c, g, 0x14)? || flag(c, g, 0x16)? {
                c.esi = slot(c, g, 0x4c)?;
                c.ecx = at(c.esp, 0x1b);
                push(c, g, c.ecx)?;
                c.edx = at(c.esp, 0x48);
                push(c, g, c.edx)?;
                c.eax = c.esi;
                return call(OWNER_TEST, 0xfedc4);
            }
            if cmp8(c, c.edx, 7) {
                return go(0xfee39);
            }
            go(0xfeded)
        }
        0xfedc4 => {
            if !test8(c, c.eax) {
                return go(0xfefb0);
            }
            c.eax = slot(c, g, 0x28)?;
            push(c, g, c.eax)?;
            c.eax = c.esi;
            call(OWNER_SET, 0xfedd8)
        }
        0xfedd8 => go(0xfefb0),
        // State 5.
        0xfeddd => {
            if flag(c, g, 0x14)? || flag(c, g, 0x16)? {
                c.edi = slot(c, g, 0x4c)?;
                c.ecx = at(c.esp, 0x17);
                push(c, g, c.ecx)?;
                c.edx = at(c.esp, 0x40);
                push(c, g, c.edx)?;
                c.eax = c.edi;
                return call(OWNER_TEST, 0xfee64);
            }
            go(0xfeded)
        }
        0xfeded => {
            if no_barrel(c, g, 8)? {
                return go(0xfee39);
            }
            c.ecx = slot(c, g, 0x4c)?;
            start(c, g, c.ecx, 0xfee06)
        }
        0xfee06 => {
            if test8(c, c.eax) {
                barrel(c, g, Idx::Ecx, 8, true)?;
                rmw32(c, g, bits(c), |f| f & 0xffff_fffe)?;
            }
            go(0xfee39)
        }
        0xfee39 => {
            c.ecx = g.u32(points::OBJECTS.base)?;
            c.edx = g.u32(at(c.ecx, 0x44))?;
            reset(c, g)?;
            go(0xfefb0)
        }
        0xfee64 => {
            if !test8(c, c.eax) {
                c.eax = c.edi;
                return go(0xfefa6);
            }
            c.esi = slot(c, g, 0x3c)?;
            if cmp32(c, g.u32(at(c.ebp, 0x194))?, c.esi) {
                set_al(c, slot8(c, g, 0x17)?);
                g.set_u8(at(c.ebp, 0x177), c.eax as u8)?;
                if !cmp_m16(c, g, at(at(c.ebx, c.ebp), 0x20e), 0)? {
                    return go(0xfefb0);
                }
                c.ecx = slot(c, g, 0x28)?;
                push(c, g, c.ecx)?;
                c.eax = c.edi;
                return call(OWNER_SPARE, 0xfee99);
            }
            c.edx = slot(c, g, 0x28)?;
            push(c, g, c.edx)?;
            c.eax = c.edi;
            call(OWNER_SET, 0xfeeaa)
        }
        0xfee99 => go(0xfefb0),
        0xfeeaa => {
            set_al(c, slot8(c, g, 0x17)?);
            g.set_u32(at(c.ebp, 0x194), c.esi)?;
            g.set_u8(at(c.ebp, 0x177), c.eax as u8)?;
            go(0xfefb0)
        }
        // State 6.
        0xfeec6 => {
            if flag(c, g, 0x14)? || flag(c, g, 0x16)? {
                // `fef3e`.
                c.edx = g.u32(points::OBJECTS.base)?;
                c.eax = g.u32(at(c.edx, 0x44))?;
                c.ecx = slot(c, g, 0x24)?;
                c.ecx = g.u32(at(at(c.eax, c.ecx), 8))?;
                c.edx = byte(c.edx, g.u8(at(c.ecx, 0x12c))?);
                c.eax = NONE;
                if test8(c, c.edx & 1) {
                    c.eax = g.u32(at(c.ecx, 0x154))?;
                }
                c.edx = at(c.esp, 0x1b);
                push(c, g, c.edx)?;
                c.ecx = at(c.esp, 0x44);
                push(c, g, c.ecx)?;
                return call(0x1061c0, 0xfef72);
            }
            if no_barrel(c, g, 0xa)? {
                return go(0xfef19);
            }
            c.edx = slot(c, g, 0x4c)?;
            start(c, g, c.edx, 0xfeeef)
        }
        0xfeeef => {
            if test8(c, c.eax) {
                barrel(c, g, Idx::Edx, 0xa, true)?;
            }
            go(0xfef19)
        }
        0xfef19 => {
            c.edx = g.u32(points::OBJECTS.base)?;
            c.eax = g.u32(at(c.edx, 0x44))?;
            c.ecx = slot(c, g, 0x24)?;
            reset(c, g)?;
            go(0xfefb0)
        }
        0xfef72 => {
            if test8(c, c.eax) {
                c.edx = g.u32(at(c.ebp, 0x194))?;
                if cmp32(c, c.edx, slot(c, g, 0x40)?) {
                    c.eax = slot(c, g, 0x28)?;
                    push(c, g, c.eax)?;
                    c.eax = slot(c, g, 0x50)?;
                    return call(OWNER_SPARE, 0xfef90);
                }
            }
            go(0xfef90)
        }
        0xfef90 => {
            if !cmp_m16(c, g, at(at(c.ebx, c.ebp), 0x20e), 0)? {
                return go(0xfefb0);
            }
            if cmp32(c, slot(c, g, 0x30)?, NONE) {
                return go(0xfefb0);
            }
            c.eax = slot(c, g, 0x4c)?;
            go(0xfefa6)
        }
        0xfefa6 => {
            c.ecx = slot(c, g, 0x28)?;
            push(c, g, c.ecx)?;
            call(0x103b70, 0xfefb0)
        }
        // The end of every state's arm.
        0xfefb0 => {
            set_al(c, g.u8(at(at(c.ebx, c.ebp), 0x20c))?);
            if test8(c, c.eax) || flag(c, g, 0x14)? || flag(c, g, 0x15)? {
                set_slot8(c, g, 0x13, 1)?;
            }
            c.eax = slot(c, g, 0x28)?;
            c.edi = slot(c, g, 0x2c)?;
            c.ecx = g.u32(at(c.edi, 0x2c8))?;
            c.eax = c.eax.wrapping_add(1);
            set_slot(c, g, 0x28, c.eax)?;
            c.eax = c.eax as u16 as i16 as i32 as u32;
            arithmetic(c, c.eax, c.ecx, true);
            if less(c) {
                return go(0xfe613);
            }
            go(BARRELS)
        }
        _ => Err(unowned(pc)),
    }
}
