//! Start one game sound. An optional auxiliary record is reserved before
//! admission. Both routes go through the same promotion and voice allocation
//! steps. Rejected starts release a reservation before returning.

use super::{at, call, invalid};
use crate::points::{self, Loc, Point, Sig};
use crate::sim_objects::{Body, Cx, Next, Result, frame::slot};
use crate::sim_physics::{arithmetic, byte, logic, pop, push};
use runtime::{CpuAbi, Guest};

const START: Point = Point::new(
    "SOUND_GAME_START",
    0x21d110,
    Sig::Fn {
        args: &[("request", Loc::Eax), ("tag", Loc::Stack(0))],
        ret: Some(Loc::Eax),
        pop: 4,
    },
    super::DOC,
);
const RECORDS: u32 = 0x0051_ebe0;
const NONE: u32 = u32::MAX;
const RESOLVE: u32 = 0x21dd30;
const DATUM_NEW: u32 = 0x16b840;
const RESOLVE_EFFECT: u32 = 0x18d090;
const BIND: u32 = 0x21dc60;
const CHECK_START: u32 = 0x126c30;
const PROMOTION: u32 = 0x126ec0;
const FINISH_RECORD: u32 = 0x21d630;
const CANCEL_RECORD: u32 = 0x21dcf0;

pub(super) const BODY: Body = Body {
    point: &START,
    sites: &[
        0x21d142, 0x21d164, 0x21d198, 0x21d1af, 0x21d1df, 0x21d1f3, 0x21d20d, 0x21d236, 0x21d247,
        0x21d27d, 0x21d28d, 0x21d2af,
    ],
    run: start,
};

/// The guest's saved call sites encode whether a source record was reserved.
/// That makes the transaction resumable from the guest stack, including when
/// a save is taken inside the mixer or a promotion callback.
#[derive(Clone, Copy)]
enum Route {
    Direct,
    Reserved,
}

impl Route {
    fn check(self, c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
        c.eax = g.u32(points::SOUND_GLOBALS.base)?;
        c.esi = NONE;
        for off in [0x78, 0x79, 0x7a] {
            let enabled = g.u8(at(c.eax, off))?;
            c.ecx = byte(c.ecx, enabled);
            logic(c, enabled.into(), 0x80);
            if enabled == 0 {
                return self.finish(c, g);
            }
        }
        let result_slot = at(c.esp, 0x10);
        match self {
            Self::Reserved => {
                c.ecx = slot(c, g, 0x18)?;
                push(c, g, 0)?;
                c.eax = result_slot;
                push(c, g, c.eax)?;
                push(c, g, c.ecx)?;
            }
            Self::Direct => {
                push(c, g, 0)?;
                c.edx = result_slot;
                push(c, g, c.edx)?;
                push(c, g, c.ebp)?;
            }
        }
        c.eax = c.ebx;
        call(
            CHECK_START,
            match self {
                Self::Reserved => 0x21d1df,
                Self::Direct => 0x21d27d,
            },
        )
    }

    fn promote(self, c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
        logic(c, c.eax & 0xff, 0x80);
        if c.eax as u8 == 0 {
            return self.finish(c, g);
        }
        let selected_rule = at(c.esp, 0x18);
        match self {
            Self::Reserved => {
                c.esi = slot(c, g, 0x18)?;
                c.edx = selected_rule;
                push(c, g, c.edx)?;
                c.ecx = c.esi;
                call(PROMOTION, 0x21d1f3)
            }
            Self::Direct => {
                c.eax = selected_rule;
                push(c, g, c.eax)?;
                c.ecx = c.ebp;
                call(PROMOTION, 0x21d28d)
            }
        }
    }

    fn allocate(self, c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
        logic(c, c.eax, 1 << 31);
        if c.eax != 0 {
            c.esi = NONE;
            logic(c, NONE, 1 << 31);
            return self.finish(c, g);
        }
        let (tag, permutation, rule, site) = match self {
            Self::Reserved => {
                c.eax = slot(c, g, 0x18)?;
                c.ecx = slot(c, g, 0x10)?;
                (c.esi, c.ecx, c.eax, 0x21d20d)
            }
            Self::Direct => {
                c.ecx = slot(c, g, 0x18)?;
                c.edx = slot(c, g, 0x10)?;
                (c.ebp, c.edx, c.ecx, 0x21d2af)
            }
        };
        for arg in [rule, c.ebx, permutation, tag] {
            push(c, g, arg)?;
        }
        call(points::SOUND_IMPULSE_ALLOCATE.base, site)
    }

    fn finish(self, c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
        if let Self::Reserved = self {
            arithmetic(c, c.esi, NONE, true);
            if c.esi == NONE {
                c.ebx = c.edi;
                return call(CANCEL_RECORD, 0x21d247);
            }
            c.edx = g.u32(RECORDS)?;
            c.eax = arithmetic(c, g.u32(at(c.edx, 0x44))?, c.ebp, false);
            g.set_u8(at(c.eax, 3), 1)?;
            g.set_u32(at(c.eax, 12), c.esi)?;
            c.ecx = (c.ecx & !0xffff) | u32::from(g.u16(at(c.ebx, 4))?);
            push(c, g, 0)?;
            push(c, g, c.edi)?;
            g.set_u16(at(c.eax, 6), c.ecx as u16)?;
            return call(FINISH_RECORD, 0x21d236);
        }
        leave(c, g)
    }
}

fn leave(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.edi = pop(c, g)?;
    c.eax = c.esi;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.ebx = pop(c, g)?;
    c.ecx = pop(c, g)?;
    Ok(Next::Ret { pop: 4 })
}

fn start(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    match pc {
        0x21d110 => {
            for saved in [c.ecx, c.ebx, c.ebp] {
                push(c, g, saved)?;
            }
            c.ebx = c.eax;
            c.eax = g.u32(c.ebx)?;
            let explicit = (c.eax >> 8) & 1;
            logic(c, explicit, 0x80);
            push(c, g, c.esi)?;
            push(c, g, c.edi)?;
            c.esi = if explicit != 0 {
                g.u32(at(c.ebx, 0x98))?
            } else {
                logic(c, NONE, 1 << 31);
                NONE
            };
            c.ebp = slot(c, g, 0x18)?;
            c.eax = at(c.esp, 0x10);
            push(c, g, c.eax)?;
            push(c, g, c.esi)?;
            c.edi = c.ebp;
            g.set_u32(at(c.esp, 0x18), 0)?;
            call(RESOLVE, 0x21d142)
        }
        0x21d142 => {
            logic(c, c.eax & 255, 0x80);
            if c.eax as u8 != 0 {
                c.ecx = slot(c, g, 0x10)?;
                c.eax = g.u32(at(c.ecx, 8))?;
                logic(c, c.eax, 1 << 31);
                if c.eax != 0 {
                    c.edx = g.u32(RECORDS)?;
                    return call(DATUM_NEW, 0x21d164);
                }
            }
            Route::Direct.check(c, g)
        }
        0x21d164 => {
            c.edi = c.eax;
            arithmetic(c, c.edi, NONE, true);
            if c.edi == NONE {
                return Route::Direct.check(c, g);
            }
            c.ecx = slot(c, g, 0x18)?;
            c.ebp = 20 * (c.eax & 0xffff);
            c.eax = g.u32(at(c.edx, 0x44))?;
            c.edx = slot(c, g, 0x10)?;
            c.eax = arithmetic(c, c.eax, c.ebp, false);
            g.set_u32(at(c.eax, 16), c.edx)?;
            c.edx = c.esi;
            g.set_u32(at(c.eax, 8), NONE)?;
            call(RESOLVE_EFFECT, 0x21d198)
        }
        0x21d198 => {
            logic(c, c.eax, 1 << 31);
            if c.eax != 0 {
                let flagged = g.u8(at(c.eax, 8))? & 1;
                logic(c, flagged.into(), 0x80);
                if flagged != 0 {
                    let byte = g.u8(at(c.ebx, 13))? | 1;
                    g.set_u8(at(c.ebx, 13), byte)?;
                    logic(c, byte.into(), 0x80);
                }
            }
            c.esi = c.ebx;
            c.ecx = c.edi;
            call(BIND, 0x21d1af)
        }
        0x21d1af => Route::Reserved.check(c, g),
        0x21d1df => Route::Reserved.promote(c, g),
        0x21d27d => Route::Direct.promote(c, g),
        0x21d1f3 => Route::Reserved.allocate(c, g),
        0x21d28d => Route::Direct.allocate(c, g),
        0x21d20d => {
            c.esi = c.eax;
            Route::Reserved.finish(c, g)
        }
        0x21d2af => {
            c.esi = c.eax;
            leave(c, g)
        }
        0x21d236 | 0x21d247 => leave(c, g),
        _ => Err(invalid(pc)),
    }
}
