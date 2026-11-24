//! Admission of an impulse, its permutation binding and its initial cache
//! request. Guest callbacks see the original stack and can change the source,
//! instance and tag data before the next segment.

use crate::points::{self, Point};
use crate::sim_objects::{
    Body, Cx, Next, Result,
    frame::{pop, push, set_slot, slot},
    sse,
};
use crate::sim_physics::spatial::{finish_fpu, fp_error, load_fpu};
use crate::sim_physics::{arithmetic, byte, logic};
use crate::sound::SoundInstance as Instance;
use runtime::{CpuAbi, Guest};
use sim_sound::impulse::{Selection, delayed_start};

const GESTALT: u32 = 0x51ebd4;
const CHUNK_CACHE: u32 = 0x502104;
const DOC: &str = "docs/engine/sim-rewrite.md";
const BIND: Point = Point::new(
    "SOUND_BIND_PERMUTATION",
    0x1286b0,
    crate::point_sig!(fn(instance: eax, tag: stack 0, range: stack 1, permutation: stack 2, chunk: stack 3) ret 16),
    DOC,
);
const REQUEST: Point = Point::new(
    "SOUND_REQUEST_INITIAL_CHUNK",
    0x125e60,
    crate::point_sig!(fn(instance: stack 0) -> eax ret 4),
    DOC,
);
const PIN: Point = Point::new(
    "SOUND_PIN_CHUNK",
    0x1268e0,
    crate::point_sig!(fn(instance: eax) ret 0),
    DOC,
);
const UNPIN: Point = Point::new(
    "SOUND_UNPIN_CHUNK",
    0x126960,
    crate::point_sig!(fn(instance: ecx) ret 0),
    DOC,
);

pub const BODIES: &[Body] = &[
    Body {
        point: &points::SOUND_IMPULSE_ALLOCATE,
        sites: ADMIT_SITES,
        run: allocate,
    },
    Body {
        point: &BIND,
        sites: &[0x1286cd],
        run: bind,
    },
    Body {
        point: &REQUEST,
        sites: &[0x125ecb, 0x125ed9],
        run: request,
    },
    Body {
        point: &PIN,
        sites: &[0x126953],
        run: pin,
    },
    Body {
        point: &UNPIN,
        sites: &[],
        run: unpin,
    },
];

fn plus(a: u32, b: u32) -> u32 {
    a.wrapping_add(b)
}
fn low_word(old: u32, new: u32) -> u32 {
    (old & 0xffff0000) | (new & 0xffff)
}
fn call(target: u32, site: u32) -> Result<Next> {
    Ok(Next::Call { target, site })
}

fn selection(g: &Guest, at: u32) -> Result<Selection> {
    Ok(Selection {
        range: g.u8(plus(at, Instance::PITCH_RANGE.offset))? as i8,
        permutation: g.u8(plus(at, Instance::PERMUTATION.offset))? as i8,
        chunk: g.u16(plus(at, Instance::CHUNK.offset))? as i16,
    })
}

fn tag_definition(g: &Guest, tag: u32) -> Result<u32> {
    let entries = g.u32(points::TAG_ENTRIES.base)?;
    g.u32(plus(entries, (tag & 0xffff) * 16 + 8))
}

struct Chunk {
    address: u32,
    chunks: u32,
    permutations: u32,
    index: u32,
    first: u32,
}

/// Re-read the gestalt at each request. Loading another map or binding a new
/// permutation replaces the underlying ranges.
fn locate(g: &Guest, instance: u32) -> Result<Chunk> {
    let choice = selection(g, instance)?;
    let tag = tag_definition(g, g.u32(plus(instance, Instance::TAG.offset))?)?;
    let range = choice.range_index(g.u16(plus(tag, 8))? as i16) as u32;
    let gestalt = g.u32(GESTALT)?;
    let ranges = g.u32(plus(gestalt, 0x24))?;
    let range = plus(ranges, range.wrapping_mul(12));
    let permutation = choice.permutation_index(g.u16(plus(range, 8))? as i16) as u32;
    let permutations = g.u32(plus(gestalt, 0x2c))?;
    let permutation = plus(permutations, permutation.wrapping_mul(16));
    let first = g.u16(plus(permutation, 12))? as i16;
    let index = choice.chunk_index(first) as u32;
    let chunks = g.u32(plus(gestalt, 0x44))?;
    Ok(Chunk {
        address: plus(chunks, index.wrapping_mul(12)),
        chunks,
        permutations,
        index,
        first: first as i32 as u32,
    })
}

fn bind(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc == BIND.base {
        c.ecx = g.u32(points::SOUND_INSTANCES.base)?;
        c.edx = g.u32(plus(c.ecx, 0x44))?;
        c.eax = arithmetic(c, (c.eax & 0xffff) * Instance::SIZE, c.edx, false);
        c.ecx = c.eax;
        return call(UNPIN.base, 0x1286cd);
    }
    c.edx = slot(c, g, 4)?;
    c.eax = byte(c.eax, g.u8(plus(c.esp, 8))?);
    g.set_u32(plus(c.ecx, Instance::TAG.offset), c.edx)?;
    c.edx = byte(c.edx, g.u8(plus(c.esp, 12))?);
    g.set_u8(plus(c.ecx, Instance::PITCH_RANGE.offset), c.eax as u8)?;
    c.eax = low_word(c.eax, u32::from(g.u16(plus(c.esp, 16))?));
    g.set_u8(plus(c.ecx, Instance::PERMUTATION.offset), c.edx as u8)?;
    g.set_u16(plus(c.ecx, Instance::CHUNK.offset), c.eax as u16)?;
    Ok(Next::Ret { pop: 16 })
}

fn pin(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc == PIN.base {
        let flags_at = plus(c.eax, 4);
        let flags = g.u8(flags_at)?;
        c.ecx = byte(c.ecx, flags >> 1);
        logic(c, c.ecx & 1, 0x80);
        if flags & 2 != 0 {
            return Ok(Next::Ret { pop: 0 });
        }
        g.set_u8(flags_at, flags | 2)?;
        let chunk = locate(g, c.eax)?;
        push(c, g, c.esi)?;
        c.esi = chunk.permutations;
        c.edx = chunk.chunks;
        arithmetic(
            c,
            chunk.first,
            u32::from(g.u16(plus(c.eax, Instance::CHUNK.offset))?) as i16 as i32 as u32,
            false,
        );
        c.ecx = chunk.index.wrapping_mul(3);
        c.eax = chunk.address;
        for arg in [4, c.eax, u32::MAX] {
            push(c, g, arg)?;
        }
        return call(0x218850, 0x126953);
    }
    c.esi = pop(c, g)?;
    Ok(Next::Ret { pop: 0 })
}

fn unpin(cx: &mut Cx<'_>, _: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let flags_at = plus(c.ecx, 4);
    let flags = g.u8(flags_at)?;
    c.eax = byte(c.eax, flags >> 1);
    logic(c, c.eax & 1, 0x80);
    if flags & 2 != 0 {
        push(c, g, c.esi)?;
        let chunk = locate(g, c.ecx)?;
        let cache_index = g.u32(plus(chunk.address, 8))? & 0xffff;
        c.edx = g.u32(CHUNK_CACHE)?;
        c.esi = g.u32(plus(c.edx, 0x44))?;
        c.eax = plus(c.esi, cache_index * 16);
        let refs = plus(c.eax, 4);
        c.edx = byte(c.edx, g.u8(refs)?.wrapping_sub(1));
        g.set_u8(refs, c.edx as u8)?;
        let cleared = g.u8(flags_at)? & !2;
        g.set_u8(flags_at, cleared)?;
        logic(c, cleared.into(), 0x80);
        c.esi = pop(c, g)?;
    }
    Ok(Next::Ret { pop: 0 })
}

fn request(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    match pc {
        0x125e60 => {
            push(c, g, c.ebx)?;
            push(c, g, c.esi)?;
            c.esi = slot(c, g, 12)?;
            c.ecx = g.u32(plus(c.esi, Instance::TAG.offset))?;
            push(c, g, c.edi)?;
            let chunk = locate(g, c.esi)?;
            c.edi = g.u16(plus(c.esi, Instance::CHUNK.offset))? as i16 as i32 as u32;
            arithmetic(c, chunk.first, c.edi, false);
            c.edx = chunk.chunks;
            c.eax = chunk.address;
            for arg in [2, c.eax, c.ecx] {
                push(c, g, arg)?;
            }
            return call(0x218850, 0x125ecb);
        }
        0x125ecb => {
            c.ebx = c.eax;
            logic(c, c.ebx & 3, 0x80);
            if c.ebx & 3 != 0 {
                c.eax = c.esi;
                return call(PIN.base, 0x125ed9);
            }
        }
        _ => {}
    }
    c.eax = (c.ebx >> 1) & 1;
    logic(c, c.eax, 1 << 31);
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebx = pop(c, g)?;
    Ok(Next::Ret { pop: 4 })
}

const ADMIT_SITES: &[u32] = &[
    0x12603f, 0x126060, 0x12608f, 0x12609a, 0x1260c8, 0x1260df, 0x126116, 0x126173, 0x1261dd,
    0x1261f1, 0x1261f8, 0x12627e, 0x1262c4, 0x1262cc, 0x1262dd, 0x12632c, 0x126352,
];

/// Copy request payloads in the guest's order. Source and destination can
/// alias, and the final register values are visible to the following call.
fn copy_payload(c: &mut CpuAbi, g: &mut Guest, elements: u32, width: u32) -> Result<()> {
    let step = if c.eflags & 0x400 == 0 {
        width
    } else {
        width.wrapping_neg()
    };
    for _ in 0..elements {
        if width == 4 {
            g.set_u32(c.edi, g.u32(c.esi)?)?;
        } else {
            g.set_u8(c.edi, g.u8(c.esi)?)?;
        }
        c.edi = plus(c.edi, step);
        c.esi = plus(c.esi, step);
    }
    c.ecx = 0;
    Ok(())
}

fn finish(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.ebx = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, 12, false);
    Ok(Next::Ret { pop: 16 })
}

/// After collecting optional source parameters, bind the explicit range
/// and permutation or let the sound gestalt choose them.
fn choose_permutation(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    g.set_u8(plus(c.ebx, 3), 0xff)?;
    if g.u8(c.ebp)? & 0x20 != 0 {
        c.eax = g.u32(plus(c.ebp, 0x58))?;
        g.set_u32(plus(c.ebx, 0x14), c.eax)?;
        c.eax = low_word(c.eax, u32::from(g.u16(plus(c.ebp, 0x8c))?));
        c.edx = (c.eax & 0xffff).min(0x30);
        c.esi = plus(c.ebp, 0x5c);
        c.edi = plus(c.ebx, 0x5c);
        copy_payload(c, g, c.edx / 4, 4)?;
        copy_payload(c, g, c.edx % 4, 1)?;
        c.edi = slot(c, g, 0x10)?;
    } else {
        g.set_u32(plus(c.ebx, 0x14), 0)?;
    }
    c.esi = 0;
    c.eax = g.u32(c.ebp)?;
    logic(c, (c.eax >> 8) & 4, 0x80);
    if c.eax & (1 << 10) != 0 {
        c.edx = slot(c, g, 0x20)?;
        c.eax = u32::from(g.u16(plus(c.ebp, 0x9e))?);
        c.ecx = u32::from(g.u16(plus(c.ebp, 0x9c))?);
        logic(c, 0, 1 << 31);
        for arg in [c.esi, c.eax, c.ecx, c.edx] {
            push(c, g, arg)?;
        }
        c.eax = slot(c, g, 0x24)?;
        return call(BIND.base, 0x1262dd);
    }
    c.ecx = slot(c, g, 0x2c)?;
    arithmetic(c, c.ecx, u32::MAX, true);
    if c.ecx != u32::MAX {
        c.edx = c.edi;
        return call(0x218f10, 0x12627e);
    }
    let scale = g.u8(plus(c.edi, 11))? as i8 as i32 as u32;
    let gestalt = g.u32(GESTALT)?;
    let scales = g.u32(plus(gestalt, 12))?;
    c.eax = plus(scales, plus(8, scale.wrapping_mul(20)));
    c.ecx = g.u16(c.eax)? as i16 as i32 as u32;
    c.edx = g.u16(plus(c.eax, 2))? as i16 as i32 as u32;
    sse::cvtsi2ss(c, 0, c.ecx);
    sse::cvtsi2ss(c, 1, c.edx);
    let low = sse::lane0(c, 0);
    sse::ss(c, sse::Ss::Sub, 1, low);
    sse::ss(c, sse::Ss::Mul, 1, g.u32(plus(c.ebp, 0x10))?);
    sse::ss(c, sse::Ss::Add, 1, g.u32(plus(c.ebx, 0x90))?);
    c.eax = u32::MAX;
    logic(c, c.eax, 1 << 31);
    c.ecx = c.edi;
    sse::ss(c, sse::Ss::Add, 1, low);
    call(0x218f50, 0x1262c4)
}

fn allocate(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    match pc {
        0x126000 => {
            c.esp = c.esp.wrapping_sub(12);
            c.ecx = g.u32(points::TAG_ENTRIES.base)?;
            push(c, g, c.ebx)?;
            push(c, g, c.ebp)?;
            c.ebp = slot(c, g, 0x20)?;
            push(c, g, c.esi)?;
            push(c, g, c.edi)?;
            c.edi = slot(c, g, 0x20)?;
            c.esi = g.u32(plus(c.ecx, (c.edi & 0xffff) * 16 + 8))?;
            let flags = g.u8(c.ebp)?;
            logic(c, u32::from(flags), 0x80);
            c.eflags_defined |= 0x10; // CMP byte,0 defines AF.
            set_slot(c, g, 0x10, c.esi)?;
            c.ebx = if flags & 0x80 != 0 {
                plus(c.ebp, 0x94)
            } else {
                0x440c48
            };
            c.eax = c.esi;
            call(0x125260, 0x12603f)
        }
        0x12603f => {
            c.edx = g.u32(plus(c.ebp, 0x10))?;
            sse::load(c, 0, g.u32(c.ebx)?);
            set_slot(c, g, 0x28, c.eax)?;
            c.eax = g.u32(plus(c.ebp, 0x14))?;
            let gain = sse::ss(c, sse::Ss::Add, 0, slot(c, g, 0x28)?);
            push(c, g, c.edx)?;
            push(c, g, c.eax)?;
            set_slot(c, g, 0x30, gain)?;
            call(0x1251e0, 0x126060)
        }
        0x126060 => {
            c.ecx = slot(c, g, 0x28)?;
            set_slot(c, g, 0x14, c.eax)?;
            sse::load(c, 0, c.eax);
            set_slot(c, g, 0x18, c.ecx)?;
            let gain = sse::ss(c, sse::Ss::Add, 0, c.ecx);
            c.eax = u32::MAX;
            if sse::comiss(c, gain, g.u32(0x44a0b4)?) & 0x41 != 0 {
                return finish(c, g);
            }
            c.ebx = c.ebp;
            call(0x2226b0, 0x12608f)
        }
        0x12608f => {
            c.edx = g.u32(points::SOUND_INSTANCES.base)?;
            call(0x16b840, 0x12609a)
        }
        0x12609a => {
            arithmetic(c, c.eax, u32::MAX, true);
            set_slot(c, g, 0x14, c.eax)?;
            if c.eax == u32::MAX {
                return finish(c, g);
            }
            c.ecx = g.u32(plus(c.edx, 0x44))?;
            c.edx = slot(c, g, 0x24)?;
            c.ebx = arithmetic(c, (c.eax & 0xffff) * Instance::SIZE, c.ecx, false);
            c.esi = plus(c.ebp, 12);
            c.ecx = c.esi;
            call(0x127e20, 0x1260c8)
        }
        0x1260c8 => {
            let distance = sse::lane0(c, 0);
            set_slot(c, g, 0x18, distance)?;
            let mut x = load_fpu(c);
            x.fld(distance).map_err(fp_error)?;
            x.unary(x87::abi::unary_op::SQRT).map_err(fp_error)?;
            let scale_at = 0x547f14;
            x.fmul_m(g.u32(scale_at)?).map_err(fp_error)?;
            finish_fpu(c, g, x, 0x1260d4, scale_at)?;
            call(0x322b0c, 0x1260df)
        }
        0x1260df => {
            c.edi = g.u32(points::RANDOM_SEEDS.base)?;
            set_slot(c, g, 0x18, c.eax)?;
            c.eax = byte(c.eax, g.u8(plus(c.esp, 0x24))?);
            g.set_u8(plus(c.ebx, 2), 0)?;
            g.set_u16(Instance::CHANNEL.at(c.ebx), u16::MAX)?;
            c.edx = low_word(c.edx, u32::from(g.u16(plus(c.ebp, 4))?));
            g.set_u8(plus(c.ebx, 0xa1), c.eax as u8)?;
            c.eax = slot(c, g, 0x10)?;
            c.edi = arithmetic(c, c.edi, 4, false);
            c.ecx = c.edi;
            g.set_u16(plus(c.ebx, 6), c.edx as u16)?;
            call(0x1252f0, 0x126116)
        }
        0x126116 => {
            let pitch_at = plus(c.ebx, 0x90);
            let mut x = load_fpu(c);
            g.set_u32(pitch_at, x.fstp_m32().map_err(fp_error)?)?;
            finish_fpu(c, g, x, 0x126116, pitch_at)?;
            c.ecx = slot(c, g, 0x28)?;
            g.set_u32(plus(c.ebx, 0x94), c.ecx)?;
            c.edx = g.u32(c.edi)?;
            g.set_u32(plus(c.ebx, 0x98), c.edx)?;
            c.eax = if g.u8(c.ebp)? & 2 != 0 {
                g.u32(plus(c.ebp, 8))?
            } else {
                0
            };
            g.set_u16(plus(c.ebx, 4), c.eax as u16)?;
            c.eax = if g.u8(c.ebp)? & 8 != 0 {
                g.u32(plus(c.ebp, 0x50))?
            } else {
                u32::MAX
            };
            g.set_u32(Instance::LOOP_OWNER.at(c.ebx), c.eax)?;
            c.eax = plus(c.ebx, 0x18);
            c.edi = c.eax;
            copy_payload(c, g, 17, 4)?;
            c.edi = slot(c, g, 0x10)?;
            let forced = g.u8(c.edi)? & 8;
            logic(c, forced.into(), 0x80);
            if forced != 0 {
                let flags = g.u8(c.eax)? | 1;
                g.set_u8(c.eax, flags)?;
                logic(c, flags.into(), 0x80);
            }
            c.eax = low_word(c.eax, g.u8(plus(c.edi, 2))? as i8 as i32 as u32);
            push(c, g, c.eax)?;
            call(0x221810, 0x126173)
        }
        0x126173 => {
            c.ecx = byte(c.ecx, g.u8(plus(c.eax, 10))? >> 2);
            if c.ecx & 1 != 0 {
                let at = plus(c.ebx, 0x18);
                g.set_u8(at, g.u8(at)? | 0x80)?;
            }
            c.eax = if g.u8(c.ebp)? & 0x40 != 0 {
                g.u16(plus(c.ebp, 0x8e))? as i16 as i32 as u32
            } else {
                u32::MAX
            };
            c.edx = low_word(c.edx, c.eax as i8 as i32 as u32);
            g.set_u16(plus(c.ebx, 0xa2), c.edx as u16)?;
            c.eax = if g.u32(c.ebp)? & (1 << 9) != 0 {
                g.u8(plus(c.ebp, 0x90))? as i8 as i32 as u32
            } else {
                u32::MAX
            };
            g.set_u8(plus(c.ebx, 0xa4), c.eax as u8)?;
            c.eax = if g.u32(c.ebp)? & (1 << 8) != 0 {
                g.u32(plus(c.ebp, 0x98))?
            } else {
                u32::MAX
            };
            g.set_u32(plus(c.ebx, 8), c.eax)?;
            let attached = g.u8(c.ebp)? & 0x10;
            logic(c, attached.into(), 0x80);
            if attached != 0 {
                c.eax = g.u32(plus(c.ebp, 0x54))?;
                return call(0x219a30, 0x1261dd);
            }
            g.set_u32(plus(c.ebx, 0xa8), u32::MAX)?;
            choose_permutation(c, g)
        }
        0x1261dd => {
            arithmetic(c, c.eax, u32::MAX, true);
            g.set_u32(plus(c.ebx, 0xa8), c.eax)?;
            if c.eax == u32::MAX {
                return choose_permutation(c, g);
            }
            push(c, g, 1)?;
            c.ecx = c.eax;
            call(0x2198f0, 0x1261f1)
        }
        0x1261f1 => {
            c.ecx = c.eax;
            call(0x219f60, 0x1261f8)
        }
        0x1261f8 => choose_permutation(c, g),
        0x12627e | 0x1262c4 => {
            c.ebp = c.eax;
            push(c, g, c.ebp)?;
            call(0x219070, 0x1262cc)
        }
        0x1262cc => {
            push(c, g, c.esi)?;
            push(c, g, c.eax)?;
            c.eax = slot(c, g, 0x28)?;
            push(c, g, c.ebp)?;
            push(c, g, c.eax)?;
            c.eax = slot(c, g, 0x24)?;
            call(BIND.base, 0x1262dd)
        }
        0x1262dd => {
            c.ecx = byte(c.ecx, g.u8(plus(c.esp, 0x2c))?);
            g.set_u8(plus(c.ebx, 0xa0), c.ecx as u8)?;
            c.ecx = slot(c, g, 0x18)?;
            let minimum = g.u8(0x1262ed)? as i8 as i32;
            arithmetic(c, c.ecx, minimum as u32, true);
            g.set_u32(plus(c.ebx, 0xb8), c.esi)?;
            g.set_u32(plus(c.ebx, 0xb4), c.esi)?;
            let delayed = if (c.ecx as i32) > minimum {
                let flags = g.u16(plus(c.ebx, 0x18))?;
                c.eax = u32::from(flags);
                logic(c, (c.eax >> 8) & 1, 0x80);
                if flags & (1 << 8) == 0 {
                    c.eax >>= 11;
                    logic(c, c.eax & 1, 0x80);
                }
                delayed_start(c.ecx as i32, minimum, flags)
            } else {
                false
            };
            if delayed {
                c.edx = g.u32(points::SOUND_GLOBALS.base)?;
                c.eax = plus(g.u32(plus(c.edx, 0x84))?, c.ecx);
                g.set_u32(plus(c.ebx, 0x8c), c.eax)?;
                let at = plus(c.ebx, 4);
                let flags = g.u8(at)? | 1;
                g.set_u8(at, flags)?;
                logic(c, flags.into(), 0x80);
                push(c, g, c.ebx)?;
                call(REQUEST.base, 0x12632c)
            } else {
                c.ecx = g.u32(points::SOUND_GLOBALS.base)?;
                c.edx = g.u32(plus(c.ecx, 0x84))?;
                push(c, g, c.ebx)?;
                g.set_u32(plus(c.ebx, 0x8c), c.edx)?;
                call(REQUEST.base, 0x126352)
            }
        }
        0x12632c | 0x126352 => {
            c.eax = slot(c, g, 0x14)?;
            finish(c, g)
        }
        _ => unreachable!("only installed impulse continuations can enter"),
    }
}
