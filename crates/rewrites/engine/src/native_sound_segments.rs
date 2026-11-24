//! Loop requests enter with a tag, owner and requested mode. The lookup
//! preserves datum order, so duplicate keys resolve to the first live slot.
//! Admission and allocation may both finish the request before a track runs.
//! The position update returns into the image with the live request frame.
use crate::datum::layout;
use crate::points::{self, Point};
use crate::sim_objects::{Body, Cx, Next, Result, frame};
use crate::sim_physics::{ARITH, arithmetic, byte, logic, status};
use crate::sound::LoopingSound;
use runtime::{CpuAbi, Guest};
use sim_sound::scheduling::next_in_word;
use sim_sound::segments::RequestMode;

const DOC: &str = "docs/engine/sim-rewrite.md";
pub const LOOP_REQUEST: Point = Point::new(
    "LOOP_REQUEST",
    0x21a250,
    crate::point_sig!(fn(tag: stack 0, flags: stack 1, owner: stack 2, origin: stack 3, source: stack 4, mode: stack 5, options: stack 6, parameter: stack 7) -> al ret 32),
    DOC,
);
pub const LOOP_LOOKUP: Point = Point::new(
    "LOOP_LOOKUP",
    0x21cf80,
    crate::point_sig!(fn(tag: stack 0, owner: stack 1) -> eax ret 8),
    DOC,
);
const FIRST_SLOT: u32 = 0x16bc00;
const AFTER_FIRST: u32 = 0x21cf90;
const AFTER_LOOKUP: u32 = 0x21a2ea;
const AFTER_ALLOCATION: u32 = 0x21a32d;
const AFTER_SOURCE: u32 = 0x21a387;
const ALLOCATE: u32 = 0x21aeb0;
const UPDATE_SOURCE: u32 = 0x21d060;
const NONE: u32 = u32::MAX;
pub const GENERATED_SITES: &[u32] = &[AFTER_SOURCE];
pub const BODIES: &[Body] = &[
    Body { point: &LOOP_REQUEST, sites: &[AFTER_LOOKUP, AFTER_ALLOCATION], run: request },
    Body { point: &LOOP_LOOKUP, sites: &[AFTER_FIRST], run: lookup },
];

fn at(addr: u32, off: u32) -> u32 {
    addr.wrapping_add(off)
}

fn handle(g: &Guest, header: u32, slot: u32) -> Result<u32> {
    if slot == NONE {
        return Ok(NONE);
    }
    let stride = g.u32(at(header, layout::ELEMENT_SIZE))?;
    let rows = g.u32(at(header, layout::DATA))?;
    let salt = g.u16(at(rows, slot.wrapping_mul(stride)))?;
    Ok(u32::from(salt) << 16 | slot)
}

fn lookup(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc == LOOP_LOOKUP.base {
        frame::push(c, g, c.ebx)?;
        c.ebx = g.u32(points::LOOPING_SOUNDS.base)?;
        c.edx = 0;
        logic(c, 0, 1 << 31);
        c.ecx = c.ebx;
        return Ok(Next::Call { target: FIRST_SLOT, site: AFTER_FIRST });
    }
    debug_assert_eq!(pc, AFTER_FIRST);
    c.ecx = c.eax;
    c.eax = handle(g, c.ebx, c.ecx)?;
    arithmetic(c, c.eax, NONE, true);
    if c.eax != NONE {
        frame::push(c, g, c.ebp)?;
        frame::push(c, g, c.esi)?;
        frame::push(c, g, c.edi)?;
        loop {
            let rows = g.u32(at(c.ebx, layout::DATA))?;
            let tag = frame::slot(c, g, 0x14)?;
            c.edx = c.eax & 0xffff;
            c.ecx = at(rows, c.edx * LoopingSound::SIZE);
            if g.u32(at(c.ecx, 4))? == tag {
                c.ecx = g.u32(at(c.ecx, 8))?;
                let owner = frame::slot(c, g, 0x18)?;
                arithmetic(c, c.ecx, owner, true);
                if c.ecx == owner {
                    break;
                }
            }
            let mut candidate = c.edx + 1;
            let limit = g.u32(at(c.ebx, layout::HIGH_WATER))?;
            c.edx = NONE;
            if (candidate as i32) < limit as i32 {
                let bitmap = g.u32(at(c.ebx, layout::BITMAP))?;
                while candidate < limit {
                    c.ecx = candidate / 32;
                    let word = g.u32(at(bitmap, c.ecx * 4))?;
                    if let Some(slot) = next_in_word(word, candidate, limit) {
                        c.edx = slot;
                        break;
                    }
                    candidate = (candidate & !31) + 32;
                }
            }
            c.eax = handle(g, c.ebx, c.edx)?;
            arithmetic(c, c.eax, NONE, true);
            if c.eax == NONE {
                break;
            }
        }
        c.edi = frame::pop(c, g)?;
        c.esi = frame::pop(c, g)?;
        c.ebp = frame::pop(c, g)?;
    }
    c.ebx = frame::pop(c, g)?;
    Ok(Next::Ret { pop: 8 })
}

fn finish_request(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.eax = byte(c.eax, g.u8(at(c.esp, 0x11))?);
    c.edi = frame::pop(c, g)?;
    c.esi = frame::pop(c, g)?;
    c.ebp = frame::pop(c, g)?;
    c.ebx = frame::pop(c, g)?;
    c.esp = arithmetic(c, c.esp, 0x3c, false);
    Ok(Next::Ret { pop: 32 })
}

fn request(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc == LOOP_REQUEST.base {
        c.esp = arithmetic(c, c.esp, 0x3c, true);
        frame::push(c, g, c.ebx)?;
        frame::push(c, g, c.ebp)?;
        c.ebp = frame::slot(c, g, 0x48)?;
        frame::push(c, g, c.esi)?;
        c.esi = frame::slot(c, g, 0x60)?;
        g.set_u8(at(c.esp, 0xd), u8::from(RequestMode(c.esi).stopping()))?;
        frame::push(c, g, c.edi)?;
        if c.ebp == NONE {
            g.set_u8(at(c.esp, 0x11), 1)?;
            return finish_request(c, g);
        }
        c.ebx = 1;
        g.set_u8(at(c.esp, 0x13), g.u8(at(c.esp, 0x68))? & 1)?;
        c.eax = g.u32(points::SOUND_GLOBALS.base)?;
        for enable in [0x78, 0x79, 0x7a] {
            c.ecx = byte(c.ecx, g.u8(at(c.eax, enable))?);
            if c.ecx as u8 == 0 {
                return finish_request(c, g);
            }
        }
        c.ecx = g.u32(points::TAG_ENTRIES.base)?;
        c.edx = frame::slot(c, g, 0x60)?;
        c.eax = g.u32(at(c.ecx, (c.ebp & 0xffff) * 16 + 8))?;
        frame::set_slot(c, g, 0x18, c.eax)?;
        let source_kind = g.u8(at(c.edx, 3))? & 0xf;
        logic(c, source_kind.into(), 0x80);
        let by_tag = if source_kind != 0 {
            let flag = g.u8(c.eax)?;
            // cmp byte,0 defines AF and clears it.
            c.eflags = (c.eflags & !ARITH) | status(u32::from(flag), 0x80);
            c.eflags_defined |= ARITH;
            (flag as i8) < 0
        } else {
            false
        };
        g.set_u8(at(c.esp, 0x12), u8::from(by_tag))?;
        if by_tag {
            // The owner's namespace is encoded by the request entry.
            c.edi = c.ebp | g.u32(0x21a2d4)?;
            logic(c, c.edi, 1 << 31);
        } else {
            c.edi = frame::slot(c, g, 0x58)?;
        }
        frame::push(c, g, c.edi)?;
        frame::push(c, g, c.ebp)?;
        return Ok(Next::Call { target: LOOP_LOOKUP.base, site: AFTER_LOOKUP });
    }

    frame::set_slot(c, g, 0x14, c.eax)?;
    if pc == AFTER_LOOKUP {
        g.set_u8(at(c.esp, 0x58), 0)?;
        g.set_u8(at(c.esp, 0x11), c.ebx as u8)?;
        let mode = RequestMode(c.esi);
        if c.eax == NONE {
            c.eax = c.ebx.wrapping_shl(c.esi);
            c.ecx = c.esi;
            if !mode.may_allocate() {
                return finish_request(c, g);
            }
            c.ecx = frame::slot(c, g, 0x5c)?;
            c.edx = frame::slot(c, g, 0x54)?;
            frame::push(c, g, c.ecx)?;
            frame::push(c, g, c.edi)?;
            frame::push(c, g, c.edx)?;
            logic(c, c.eax & 3, 0x80);
            c.eax = c.ebp;
            return Ok(Next::Call { target: ALLOCATE, site: AFTER_ALLOCATION });
        }
        let next = mode.for_existing(g.u8(at(c.esp, 0x68))?);
        if next != mode {
            frame::set_slot(c, g, 0x64, c.ebx)?;
        }
    } else {
        debug_assert_eq!(pc, AFTER_ALLOCATION);
        g.set_u8(at(c.esp, 0x58), c.ebx as u8)?;
        if c.eax == NONE {
            g.set_u8(at(c.esp, 0x11), 0)?;
            return finish_request(c, g);
        }
    }

    c.ecx = (c.ecx & !0xffff) | u32::from(g.u16(at(c.esp, 0x54))?);
    c.edx = frame::slot(c, g, 0x60)?;
    let pool = g.u32(points::LOOPING_SOUNDS.base)?;
    c.esi = g.u32(at(pool, layout::DATA))?;
    c.ebp = at(c.esi, (c.eax & 0xffff) * LoopingSound::SIZE);
    g.set_u16(at(c.ebp, 2), c.ecx as u16)?;
    let source_flags = g.u8(c.edx)? >> 1;
    logic(c, u32::from(source_flags & c.ebx as u8), 0x80);
    frame::set_slot(c, g, 0x20, c.ebp)?;
    c.ebx = at(c.ebp, 0xc);
    if source_flags & 1 == 0 {
        // Source and destination can alias. The original copies one word
        // at a time and honours the guest direction flag.
        let step = if c.eflags & 0x400 == 0 { 4 } else { 4u32.wrapping_neg() };
        c.ecx = 17;
        c.edi = c.ebx;
        c.esi = c.edx;
        while c.ecx != 0 {
            g.set_u32(c.edi, g.u32(c.esi)?)?;
            c.edi = at(c.edi, step);
            c.esi = at(c.esi, step);
            c.ecx -= 1;
        }
    }
    c.eax = frame::slot(c, g, 0x50)?;
    Ok(Next::CallOriginalResume { target: UPDATE_SOURCE, site: AFTER_SOURCE })
}
