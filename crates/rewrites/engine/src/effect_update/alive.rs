//! `1794e0 (effect: stack 0) -> al`, `ret 4`: an effect's object
//! bookkeeping before its update; true when the effect is gone
//! (docs/engine/effects-parts.md, "The per-effect bodies").
//!
//! Frame: `push ecx` (the local, later the definition), then `ebx`, `ebp`
//! (the datum), `esi` and `edi` (the handle). `[esp+0x10]` is the local,
//! `[esp+0x18]` the handle.
//!
//! With an object (`+48` not -1):
//!
//! 1. `badc0(-1)` (`eax` the object) false: the effect is deleted
//!    (`1774f0`, a body of this table) and the result is true;
//! 2. `baf80` (`ecx` the object) and the object's definition: with its bit
//!    8, the offset `+30..+3c` and the marker pair `+1c`, `+20` from it;
//!    otherwise -1 and the default marker word `[4686c4]`;
//! 3. with bit 1 or bit 9 of the flags word, `1794a0(object)` (`eax` the
//!    object index): true reads the two function values `bab40` into
//!    `+6c` and `+70` and the change colour `bad50` into `+24`; false
//!    zeroes them and copies the default colour `[468710]`;
//! 4. then, by the definition's bit 0 and the effect's bits 2 and 4: the
//!    effect is ended (`177610`, true makes the result true), stopped
//!    (`177310(handle)`) or restarted (`177260(0)`);
//! 5. with the definition's bit 3 or 4, the object's light record (`badc0`,
//!    `baf80`, `3ddd0`) gives `+104` and `+108`.
//!
//! The result is `ebp == 0` after `test ebp, ebp`, with `ebp` zeroed on
//! the two ways that return true.
//!
//! Flags: each call carries its last flag-setting instruction's, per
//! path; the call to `3ddd0` carries `baf80`'s own.

use super::{at, unowned};
use crate::sim_objects::alu::{shl, shr, test16, test8};
use crate::sim_objects::{Cx, Next, Result};
use crate::sim_physics::{arithmetic, byte, logic, pop, push};
use crate::sim_objects::sse;
use runtime::CpuAbi;

pub(crate) const ENTRY: u32 = 0x0017_94e0;
const ARRAY: u32 = 0x004e_a93c;
const TAGS: u32 = 0x004e_3b44;
const OBJECTS: u32 = 0x004e_0300;
const LIGHTS: u32 = 0x0050_9434;
const MARKER: u32 = 0x0046_86c4;
const COLOUR: u32 = 0x0046_8710;
const NONE: u32 = u32::MAX;

const OBJECT_TEST: u32 = 0x000b_adc0;
const OBJECT_DATUM: u32 = 0x000b_af80;
const ACTIVE: u32 = 0x0017_94a0;
const FUNCTION_VALUE: u32 = 0x000b_ab40;
const CHANGE_COLOUR: u32 = 0x000b_ad50;
const RESTART: u32 = 0x0017_7260;
const RETIRE: u32 = 0x0017_7610;
const STOP: u32 = 0x0017_7310;
const LIGHT: u32 = 0x0003_ddd0;

const TEST_SITE: u32 = 0x0017_952c;
const DELETE_SITE: u32 = 0x0017_9537;
const DATUM_SITE: u32 = 0x0017_9558;
const ACTIVE_SITE: u32 = 0x0017_95d8;
const VALUE_A_SITE: u32 = 0x0017_95f1;
const VALUE_B_SITE: u32 = 0x0017_960c;
const COLOUR_SITE: u32 = 0x0017_9623;
const RESTART_SITE: u32 = 0x0017_9646;
const RETIRE_SITE: u32 = 0x0017_9693;
const STOP_SITE: u32 = 0x0017_96ae;
const LIGHT_TEST_SITE: u32 = 0x0017_96d1;
const LIGHT_DATUM_SITE: u32 = 0x0017_96dc;
const LIGHT_SITE: u32 = 0x0017_96e2;

/// The return addresses of its calls.
pub(crate) const SITES: [u32; 13] = [
    TEST_SITE,
    DELETE_SITE,
    DATUM_SITE,
    ACTIVE_SITE,
    VALUE_A_SITE,
    VALUE_B_SITE,
    COLOUR_SITE,
    RESTART_SITE,
    RETIRE_SITE,
    STOP_SITE,
    LIGHT_TEST_SITE,
    LIGHT_DATUM_SITE,
    LIGHT_SITE,
];

/// The labels a segment runs through between transfers.
#[derive(Clone, Copy)]
enum Label {
    /// `179548`: the object's datum.
    Datum,
    /// `1795fb`: the second function value, `bl` the first's answer.
    ValueB,
    /// `179627`: by the function value's answer.
    Values,
    /// `17962b`: the definition's bit 0.
    Definition,
    /// `179648`: no function values.
    Defaults,
    /// `179677`: the effect's bits 2 and 4.
    Flags,
    /// `17968c`: the end.
    Retire,
    /// `1796ae`: the light record.
    Light,
    /// `179714`: leave, the result `ebp == 0`.
    Leave,
}

/// `pop edi; pop esi; test ebp, ebp; pop ebp; sete al; pop ebx; pop ecx;
/// ret 4`, with `eax` cleared first.
fn leave(cx: &mut Cx<'_>) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    c.edi = pop(c, g)?;
    c.eax = u32::from(c.ebp == 0);
    c.esi = pop(c, g)?;
    logic(c, c.ebp, 1 << 31);
    c.ebp = pop(c, g)?;
    c.ebx = pop(c, g)?;
    c.ecx = pop(c, g)?;
    Ok(Next::Ret { pop: 4 })
}

/// The exit that returns true: `ebp` zeroed first.
fn gone(cx: &mut Cx<'_>) -> Result<Next> {
    cx.cpu.ebp = 0;
    leave(cx)
}

fn set_bl(c: &mut CpuAbi, v: u8) {
    c.ebx = byte(c.ebx, v);
}

pub(crate) fn alive(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let mut label = match pc {
        ENTRY => {
            let (c, g) = (&mut *cx.cpu, &mut *cx.g);
            let v = c.ecx;
            push(c, g, v)?;
            c.eax = g.u32(ARRAY)?;
            let v = c.ebx;
            push(c, g, v)?;
            c.ebx = g.u32(at(c.eax, 0x44))?;
            let v = c.ebp;
            push(c, g, v)?;
            c.edx = g.u32(TAGS)?;
            for v in [c.esi, c.edi] {
                push(c, g, v)?;
            }
            c.edi = g.u32(at(c.esp, 0x18))?;
            c.ebp = (c.edi & 0xffff).wrapping_mul(0x190);
            c.ecx = g.u32(c.ebp.wrapping_add(c.ebx).wrapping_add(4))?;
            c.ebp = c.ebp.wrapping_add(c.ebx);
            c.ecx = (c.ecx & 0xffff) << 4;
            c.eax = g.u32(c.ecx.wrapping_add(c.edx).wrapping_add(8))?;
            g.set_u32(at(c.esp, 0x10), c.eax)?;
            c.eax = g.u32(at(c.ebp, 0x48))?;
            c.esi = NONE;
            arithmetic(c, c.eax, c.esi, true);
            if c.eax == c.esi {
                Label::Datum
            } else {
                let v = c.esi;
                push(c, g, v)?;
                return Ok(Next::Call { target: OBJECT_TEST, site: TEST_SITE });
            }
        }
        TEST_SITE => {
            let c = &mut *cx.cpu;
            logic(c, c.eax, 1 << 31);
            if c.eax != 0 {
                Label::Datum
            } else {
                c.eax = c.edi;
                return Ok(Next::Call { target: super::delete::ENTRY, site: DELETE_SITE });
            }
        }
        DELETE_SITE => {
            let (c, g) = (&mut *cx.cpu, &mut *cx.g);
            c.edi = pop(c, g)?;
            c.ebp = 0;
            c.eax = 1;
            c.esi = pop(c, g)?;
            logic(c, 0, 1 << 31);
            c.ebp = pop(c, g)?;
            c.ebx = pop(c, g)?;
            c.ecx = pop(c, g)?;
            return Ok(Next::Ret { pop: 4 });
        }
        DATUM_SITE => {
            let (c, g) = (&mut *cx.cpu, &mut *cx.g);
            c.edx = g.u32(at(g.u32(OBJECTS)?, 0x44))?;
            c.ecx = (c.eax & 0xffff).wrapping_mul(3);
            c.ecx = g.u32(c.edx.wrapping_add(c.ecx.wrapping_mul(4)).wrapping_add(8))?;
            c.edx = g.u32(at(c.ecx, 4))? >> 8;
            if c.edx & 1 != 0 {
                c.edx = at(c.ecx, 0x88);
                c.esi = at(c.ebp, 0x30);
                c.ebx = g.u32(c.edx)?;
                g.set_u32(c.esi, c.ebx)?;
                c.ebx = g.u32(at(c.edx, 4))?;
                g.set_u32(at(c.esi, 4), c.ebx)?;
                c.edx = g.u32(at(c.edx, 8))?;
                g.set_u32(at(c.esi, 8), c.edx)?;
                c.edx = g.u32(at(c.ecx, 0x28))?;
                g.set_u32(at(c.ebp, 0x1c), c.edx)?;
                c.ecx = g.u32(at(c.ecx, 0x2c))?;
                g.set_u32(at(c.ebp, 0x20), c.ecx)?;
            } else {
                let marker = g.u16(MARKER)?;
                c.edx = (c.edx & 0xffff_0000) | u32::from(marker);
                g.set_u32(at(c.ebp, 0x1c), c.esi)?;
                g.set_u16(at(c.ebp, 0x20), c.esi as u16)?;
                g.set_u16(at(c.ebp, 0x22), marker)?;
            }
            // `1795b4`: bit 1 or bit 9 of the flags word.
            let word = u32::from(g.u16(at(c.ebp, 2))?);
            c.ecx = word;
            let dl = (word as u8) >> 1;
            c.edx = byte(c.edx, dl);
            if dl & 1 != 0 {
                logic(c, 1, 0x80);
            } else {
                c.ecx = shr(c, word, 9, 32);
                logic(c, c.ecx & 1, 0x80);
                if c.ecx & 1 == 0 {
                    return light(cx);
                }
            }
            c.ecx = g.u32(at(c.ebp, 0x48))?;
            let v = c.ecx;
            push(c, g, v)?;
            return Ok(Next::Call { target: ACTIVE, site: ACTIVE_SITE });
        }
        ACTIVE_SITE => {
            let (c, g) = (&mut *cx.cpu, &mut *cx.g);
            if c.eax as u8 == 0 {
                Label::Defaults
            } else {
                c.eax = g.u32(at(c.ebp, 0x10))?;
                logic(c, c.eax, 1 << 31);
                if c.eax == 0 {
                    set_bl(c, 1);
                    Label::ValueB
                } else {
                    c.edx = at(c.ebp, 0x6c);
                    for v in [c.edx, c.eax] {
                        push(c, g, v)?;
                    }
                    c.eax = g.u32(at(c.ebp, 0x48))?;
                    let v = c.eax;
                    push(c, g, v)?;
                    return Ok(Next::Call { target: FUNCTION_VALUE, site: VALUE_A_SITE });
                }
            }
        }
        VALUE_A_SITE => {
            let c = &mut *cx.cpu;
            logic(c, c.eax & 0xff, 0x80);
            if c.eax as u8 != 0 {
                set_bl(c, 1);
            } else {
                set_bl(c, 0);
                logic(c, 0, 0x80);
            }
            Label::ValueB
        }
        VALUE_B_SITE => {
            let (c, g) = (&mut *cx.cpu, &mut *cx.g);
            let ax = u32::from(g.u16(at(c.ebp, 0x18))?);
            c.eax = (c.eax & 0xffff_0000) | ax;
            test16(c, ax);
            if ax == 0 {
                Label::Values
            } else {
                c.edi = ax as u16 as i16 as i32 as u32;
                c.eax = g.u32(at(c.ebp, 0x48))?;
                c.esi = at(c.ebp, 0x24);
                return Ok(Next::Call { target: CHANGE_COLOUR, site: COLOUR_SITE });
            }
        }
        COLOUR_SITE => {
            cx.cpu.edi = cx.g.u32(at(cx.cpu.esp, 0x18))?;
            Label::Values
        }
        RESTART_SITE | STOP_SITE => Label::Light,
        RETIRE_SITE => {
            if cx.cpu.eax as u8 != 0 {
                return gone(cx);
            }
            Label::Light
        }
        LIGHT_TEST_SITE => {
            let c = &mut *cx.cpu;
            logic(c, c.eax, 1 << 31);
            if c.eax == 0 {
                Label::Leave
            } else {
                c.ecx = c.esi;
                return Ok(Next::Call { target: OBJECT_DATUM, site: LIGHT_DATUM_SITE });
            }
        }
        LIGHT_DATUM_SITE => {
            let (c, g) = (&mut *cx.cpu, &mut *cx.g);
            let v = c.eax;
            push(c, g, v)?;
            return Ok(Next::Call { target: LIGHT, site: LIGHT_SITE });
        }
        LIGHT_SITE => {
            let (c, g) = (&mut *cx.cpu, &mut *cx.g);
            if c.eax != NONE {
                c.edx = g.u32(LIGHTS)?;
                c.esi = g.u32(at(c.edx, 0x44))?;
                c.eax = shl(c, c.eax & 0xffff, 8, 32);
                let cl = g.u8(c.eax.wrapping_add(c.esi).wrapping_add(2))?;
                c.ecx = byte(c.ecx, cl);
                c.eax = c.eax.wrapping_add(c.esi);
                if cl != 0 {
                    c.ecx = g.u32(at(c.eax, 0x2c))?;
                    g.set_u32(at(c.ebp, 0x104), c.ecx)?;
                    c.edx = g.u32(at(c.eax, 0x28))?;
                    g.set_u32(at(c.ebp, 0x108), c.edx)?;
                }
            }
            Label::Leave
        }
        _ => return Err(unowned(pc)),
    };
    loop {
        let (c, g) = (&mut *cx.cpu, &mut *cx.g);
        label = match label {
            Label::Datum => {
                c.ecx = g.u32(at(c.ebp, 0x48))?;
                arithmetic(c, c.ecx, c.esi, true);
                if c.ecx == c.esi {
                    Label::Leave
                } else {
                    return Ok(Next::Call { target: OBJECT_DATUM, site: DATUM_SITE });
                }
            }
            Label::ValueB => {
                c.edx = g.u32(at(c.ebp, 0x14))?;
                c.eax = g.u32(at(c.ebp, 0x48))?;
                c.ecx = at(c.ebp, 0x70);
                for v in [c.ecx, c.edx, c.eax] {
                    push(c, g, v)?;
                }
                return Ok(Next::Call { target: FUNCTION_VALUE, site: VALUE_B_SITE });
            }
            Label::Values => {
                logic(c, c.ebx & 0xff, 0x80);
                if c.ebx as u8 != 0 { Label::Flags } else { Label::Definition }
            }
            Label::Definition => {
                c.eax = g.u32(at(c.esp, 0x10))?;
                if test8(c, g, c.eax, 1)? {
                    Label::Retire
                } else {
                    let cl = g.u8(at(c.ebp, 2))? >> 2;
                    c.ecx = byte(c.ecx, cl);
                    logic(c, u32::from(cl & 1), 0x80);
                    if cl & 1 != 0 {
                        Label::Light
                    } else {
                        push(c, g, 0)?;
                        return Ok(Next::Call { target: RESTART, site: RESTART_SITE });
                    }
                }
            }
            Label::Defaults => {
                sse::zero(c, 0);
                g.set_u32(at(c.ebp, 0x6c), 0)?;
                g.set_u32(at(c.ebp, 0x70), 0)?;
                if g.u16(at(c.ebp, 0x18))? != 0 {
                    c.edx = g.u32(COLOUR)?;
                    c.eax = g.u32(c.edx)?;
                    c.ecx = at(c.ebp, 0x24);
                    g.set_u32(c.ecx, c.eax)?;
                    c.eax = g.u32(at(c.edx, 4))?;
                    g.set_u32(at(c.ecx, 4), c.eax)?;
                    c.edx = g.u32(at(c.edx, 8))?;
                    g.set_u32(at(c.ecx, 8), c.edx)?;
                }
                Label::Definition
            }
            Label::Flags => {
                let word = u32::from(g.u16(at(c.ebp, 2))?);
                c.eax = (c.eax & 0xffff_0000) | word;
                let dl = (word as u8) >> 2;
                c.edx = byte(c.edx, dl);
                if dl & 1 == 0 {
                    Label::Light
                } else {
                    let al = (word as u8) >> 4;
                    c.eax = byte(c.eax, al);
                    logic(c, u32::from(al & 1), 0x80);
                    if al & 1 != 0 {
                        Label::Retire
                    } else {
                        let v = c.edi;
                        push(c, g, v)?;
                        return Ok(Next::Call { target: STOP, site: STOP_SITE });
                    }
                }
            }
            Label::Retire => {
                c.eax = c.edi;
                return Ok(Next::Call { target: RETIRE, site: RETIRE_SITE });
            }
            Label::Light => return light(cx),
            Label::Leave => return leave(cx),
        };
    }
}

/// `1796ae`: with the definition's bit 3 or 4, `badc0(-1)` on the object
/// (`eax` and `esi` the object).
fn light(cx: &mut Cx<'_>) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let def = g.u32(g.u32(at(c.esp, 0x10))?)?;
    c.eax = def;
    c.ecx = def >> 3;
    if c.ecx & 1 != 0 {
        logic(c, 1, 0x80);
    } else {
        c.eax = def >> 4;
        logic(c, c.eax & 1, 0x80);
        if c.eax & 1 == 0 {
            return leave(cx);
        }
    }
    c.esi = g.u32(at(c.ebp, 0x48))?;
    push(c, g, NONE)?;
    c.eax = c.esi;
    Ok(Next::Call { target: OBJECT_TEST, site: LIGHT_TEST_SITE })
}
