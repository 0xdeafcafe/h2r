//! Birth of one emitter particle. Only the allocator chooses a slot. The
//! particle is linked and gets its five local RNG draws before the shape
//! initializer sees it. Guest callees run on the original birth frame.
use super::Port;
use guest_layout::points::{self, Loc, Point, Sig};
use crate::sim_native::Result;
use crate::sim_objects::sse;
use crate::sim_physics::{arithmetic, logic, pop, push, spatial::{finish_fpu, fp_error, load_fpu}};
use runtime::{CpuAbi, Guest, GuestFault};
use x87::abi::arith_op;

const BIRTH: Point = Point::new("PARTICLE_BIRTH", 0x247950, Sig::Fn {
    args: &[("properties", Loc::Eax), ("emitter", Loc::Ecx), ("system", Loc::Stack(0))],
    ret: None, pop: 24,
}, "docs/engine/sim-rewrite.md");
const AFTER_ALLOCATE: u32 = 0x2479a5;
const AFTER_SYSTEM_DEF: u32 = 0x2479c9;
const AFTER_PROPERTIES: u32 = 0x247b74;
const AFTER_SHAPE: u32 = 0x247b8d;
const PARTICLES: u32 = 0x51ec84;
const EFFECTS: u32 = 0x4ea93c;
const PROPERTY_BITS: u32 = 0xf80f;
const RNG_UNIT_SCALE: u32 = 0x45dc50;
const RNG_SHORT_SCALE: u32 = 0x45dca4;
const SITES: &[u32] = &[AFTER_ALLOCATE, AFTER_SYSTEM_DEF, AFTER_PROPERTIES];

pub(super) const PORTS: &[Port] = &[Port {
    point: &BIRTH, sites: SITES, supported: |c, _, _| crate::sim_native::supported(c), run: birth,
}];

fn leave(c: &mut CpuAbi, g: &Guest) -> Result {
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.ebx = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, 0xb0, false);
    g.ret(c, 24)
}

fn allocate(c: &mut CpuAbi, g: &mut Guest) -> Result {
    c.esp = c.esp.wrapping_sub(0xb0);
    for saved in [c.ebx, c.ebp, c.esi, c.edi] { push(c, g, saved)?; }
    c.edi = g.u32(c.esp.wrapping_add(0xc4))?;
    c.esi = c.eax;
    c.eax = g.u32(c.edi.wrapping_add(0x18))?;
    c.ebx = c.ecx;
    g.set_u32(c.esp.wrapping_add(0x14), c.eax)?;
    arithmetic(c, c.eax, u32::MAX, true);
    if c.eax != u32::MAX {
        c.ecx = g.u32(EFFECTS)?;
        c.edx = g.u32(c.ecx.wrapping_add(0x44))?;
        c.eax = c.edx.wrapping_add((c.eax & 0xffff) * 0x190);
        let suspended = (g.u8(c.eax.wrapping_add(2))? >> 3) & 1;
        c.edx = (c.edx & !0xff) | u32::from(suspended);
        logic(c, u32::from(suspended), 0x80);
        g.set_u8(c.esp.wrapping_add(0x1b), suspended)?;
        if suspended != 0 { return leave(c, g); }
    }
    c.edx = g.u32(PARTICLES)?;
    g.call(c, 0x16b840, AFTER_ALLOCATE)
}

/// Draw from the local effects stream in program order. Writing each step
/// keeps the seed visible even when another operand aliases the record.
fn draw(g: &mut Guest, seed: u32) -> std::result::Result<u32, GuestFault> {
    let word = g.u32(seed)?.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    g.set_u32(seed, word)?;
    Ok(word >> 16)
}

fn initialize(c: &mut CpuAbi, g: &mut Guest) -> Result {
    let sp = c.esp;
    let particle = c.ebp;
    let emitter = c.ebx;
    let properties = c.esi;
    let handle = g.u32(sp.wrapping_add(0x20))?;
    g.set_u32(sp.wrapping_add(0x1c), c.eax)?;
    g.set_u32(particle.wrapping_add(4), g.u32(emitter.wrapping_add(4))?)?;
    g.set_u32(emitter.wrapping_add(4), handle)?;
    let seed = g.u32(points::RANDOM_SEEDS.base)?.wrapping_add(4);

    let flags = particle.wrapping_add(2);
    g.set_u16(flags, (g.u16(flags)? & 0xe000) | 4)?;
    let variant = ((draw(g, seed)? * 5) >> 16) as u16;
    let preserved = g.u16(flags)? & 0x1fff;
    g.set_u32(particle.wrapping_add(8), 0)?;
    g.set_u16(flags, preserved | (variant << 13))?;

    let scratch = sp.wrapping_add(0x10);
    let mut x = load_fpu(c);
    for offset in [0x14, 0x16, 0x18, 0x1a] {
        let sample = draw(g, seed)?;
        g.set_u32(scratch, sample)?;
        x.fild(sample as i32).map_err(fp_error)?;
        x.arith_m(arith_op::MUL, g.u32(RNG_UNIT_SCALE)?).map_err(fp_error)?;
        x.arith_m(arith_op::MUL, g.u32(RNG_SHORT_SCALE)?).map_err(fp_error)?;
        let scaled = x.fstp_m32().map_err(fp_error)?;
        g.set_u32(scratch, scaled)?;
        let short = sse::cvttss2si(c, scaled);
        g.set_u16(particle.wrapping_add(offset), short as u16)?;
    }
    // Final x87 store's provenance is part of the checkpointed CPU state.
    finish_fpu(c, g, x, 0x247b28, scratch)?;
    let age = g.u32(c.edi.wrapping_add(4))?;
    sse::load(c, 0, age);
    g.set_u32(particle.wrapping_add(0x10), age)?;

    let current = properties.wrapping_add(0x50);
    let valid = properties.wrapping_add(0x44);
    if particle != g.u32(current)? {
        let retained = g.u32(valid)? & !PROPERTY_BITS;
        g.set_u32(current, particle)?;
        g.set_u32(valid, retained)?;
    }
    let current = g.u32(current)?;
    c.ecx = g.u32(properties.wrapping_add(0x4c))?;
    c.edx = g.u32(properties.wrapping_add(0x48))?;
    push(c, g, properties)?;
    push(c, g, current)?;
    c.eax = !g.u32(valid)? & PROPERTY_BITS;
    for arg in [c.ecx, c.edx, c.eax] { push(c, g, arg)?; }
    logic(c, c.eax, 1 << 31);
    g.call(c, 0x173ba0, AFTER_PROPERTIES)
}

fn birth(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Result {
    match pc {
        pc if pc == BIRTH.base => allocate(c, g),
        AFTER_ALLOCATE => {
            g.set_u32(c.esp.wrapping_add(0x20), c.eax)?;
            if c.eax == u32::MAX { return leave(c, g); }
            c.ebp = (c.eax & 0xffff) << 6;
            c.eax = g.u32(c.edx.wrapping_add(0x44))?;
            c.ecx = c.edi;
            c.ebp = arithmetic(c, c.ebp, c.eax, false);
            g.call(c, 0x1751d0, AFTER_SYSTEM_DEF)
        }
        AFTER_SYSTEM_DEF => initialize(c, g),
        AFTER_PROPERTIES => {
            let at = c.esi.wrapping_add(0x44);
            let ready = g.u32(at)? | PROPERTY_BITS;
            g.set_u32(at, ready)?;
            logic(c, ready, 1 << 31);
            c.ecx = g.u32(c.esp.wrapping_add(0xc8))?;
            for arg in [c.edi, c.ecx] { push(c, g, arg)?; }
            c.edi = c.ebp;
            c.eax = c.esi;
            g.call(c, 0x249170, AFTER_SHAPE)
        }
        _ => unreachable!("particle birth continuation"),
    }
}
