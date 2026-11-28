//! A decal lives in two lists. The render bucket links neighbouring items
//! through +30/+34. The projection chain links pieces of one placement
//! through +3c; +38 identifies its owning root. Only that root owns a clock
//! and a geometry allocation. Each call materialises just the state it needs.
//! No handle, pointer or clock survives a guest call or a checkpoint restore.
use super::Port;
use guest_layout::datum::layout;
use guest_layout::points::{Loc, Point, Sig};
use crate::sim_native;
use crate::sim_objects::sse::{self, Ss};
use crate::sim_physics::{arithmetic, byte, logic, pop, push, Result};
use crate::sim_physics::spatial::{finish_fpu, fp_error, load_fpu};
use runtime::{CpuAbi, Guest, TransferAbi};

const DOC: &str = "docs/engine/sim-rewrite.md";
const DECALS: u32 = 0x004e_a950;
const GROUPS: u32 = 0x004e_a94c;
const GEOMETRY: u32 = 0x0050_9448;
const CLOCK: u32 = 0x0051_0c54;
const OPACITY_SCALE: u32 = 0x0045_dd5c;
const OPACITY_LIMIT: u32 = 0x0045_dc38;
const RELEASE_GEOMETRY: u32 = 0x0013_d830;
const FLOAT_TO_INTEGER: u32 = 0x0032_2b0c;
const DELETE_DATUM: u32 = 0x0016_ba40;
const NONE: u32 = u32::MAX;
const AFTER_CLOCK: u32 = 0x0017_cc8b;
const AFTER_RELEASE: u32 = 0x0017_cd7b;
const AFTER_CONVERT: u32 = 0x0017_cde4;
const FADE_LOAD: u32 = 0x0017_cddb;

const ROOT: Point = Point::new("DECAL_UPDATE_ROOT", 0x0017_cc60,
    Sig::Fn { args: &[("handle", Loc::Edx)], ret: None, pop: 0 }, DOC);
const CLOCK_STEP: Point = Point::new("DECAL_UPDATE_LIFETIME", 0x0017_ccd0,
    Sig::Fn { args: &[("handle", Loc::Eax)], ret: Some(Loc::Al), pop: 0 }, DOC);
const LINK: Point = Point::new("DECAL_LINK_BUCKET", 0x0017_ce00,
    Sig::Fn { args: &[("mode", Loc::Edx), ("cluster", Loc::Ebx), ("handle", Loc::Edi)], ret: None, pop: 0 }, DOC);
const UNLINK: Point = Point::new("DECAL_UNLINK_BUCKET", 0x0017_d860,
    Sig::Fn { args: &[("handle", Loc::Eax)], ret: None, pop: 0 }, DOC);

pub(super) const PORTS: &[Port] = &[
    Port { point: &LINK, sites: &[], supported: integer, run: link },
    Port { point: &UNLINK, sites: &[], supported: integer, run: unlink },
    Port { point: &ROOT, sites: &[AFTER_CLOCK], supported: supported_root, run: root },
    Port { point: &CLOCK_STEP, sites: &[AFTER_RELEASE, AFTER_CONVERT], supported: supported_clock, run: clock_step },
];

fn integer(_: &CpuAbi, _: &Guest, _: u32) -> bool { true }

#[derive(Clone, Copy)]
struct Pool { header: u32, data: u32 }

impl Pool {
    fn read(g: &Guest) -> Result<Self> {
        let header = g.u32(DECALS)?;
        Ok(Self { header, data: g.u32(header.wrapping_add(layout::DATA))? })
    }

    fn record(self, handle: u32) -> u32 {
        self.data.wrapping_add((handle & 0xffff) * 0x40)
    }

    // A bad chain must go back to the original without trapping the host
    // in a Rust loop. The original uses the index half of each salted link.
    fn chain_fits(self, g: &Guest, mut next: u32) -> Result<bool> {
        let mut left = g.u32(self.header.wrapping_add(layout::MAXIMUM_COUNT))?.min(1 << 16);
        while next != NONE {
            if left == 0 { return Ok(false); }
            next = g.u32(self.record(next).wrapping_add(0x3c))?;
            left -= 1;
        }
        Ok(true)
    }
}

fn supported_root(c: &CpuAbi, g: &Guest, pc: u32) -> bool {
    pc != AFTER_CLOCK || c.eax as u8 != 0 || c.esi == NONE ||
        Pool::read(g).and_then(|p| p.chain_fits(g, c.esi)).unwrap_or(false)
}

fn supported_clock(c: &CpuAbi, g: &Guest, pc: u32) -> bool {
    if pc != CLOCK_STEP.base { return true; }
    sim_native::supported(c) && Pool::read(g).and_then(|p| {
        let at = p.record(c.eax);
        Ok(g.u8(at.wrapping_add(2))? & 3 != 1 ||
            p.chain_fits(g, g.u32(at.wrapping_add(0x3c))?)?)
    }).unwrap_or(false)
}

fn bucket(base: u32, mode: i32, cluster: i32) -> u32 {
    base.wrapping_add((mode.wrapping_mul(512).wrapping_add(cluster) as u32).wrapping_mul(4))
}

// Insert at the front. Only the first member has a null back link.
fn link(c: &mut CpuAbi, g: &mut Guest, _: u32) -> Result<TransferAbi> {
    let groups = g.u32(GROUPS)?;
    push(c, g, c.ebp)?;
    let pool = Pool::read(g)?;
    c.ebp = pool.header;
    push(c, g, c.esi)?;
    let head = bucket(groups, c.edx as i16 as i32, c.ebx as i16 as i32);
    let old = g.u32(head)?;
    let at = pool.record(c.edi);
    c.eax = at;
    c.ecx = old;
    arithmetic(c, old, NONE, true);
    g.set_u32(at.wrapping_add(0x30), NONE)?;
    g.set_u32(at.wrapping_add(0x34), old)?;
    g.set_u16(at.wrapping_add(0x0a), c.ebx as u16)?;
    g.set_u8(at.wrapping_add(3), c.edx as u8)?;
    g.set_u16(at.wrapping_add(8), u16::MAX)?;
    if old != NONE {
        c.edx = g.u32(c.ebp.wrapping_add(layout::DATA))?;
        c.ecx = (old & 0xffff) << 6;
        // shl by 6 leaves CF=0, OF and AF undefined.
        logic(c, c.ecx, 1 << 31);
        c.eflags_defined &= !0x800;
        g.set_u32(c.edx.wrapping_add(c.ecx).wrapping_add(0x30), c.edi)?;
    }
    g.set_u32(head, c.edi)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    g.ret(c, 0)
}

// Detach the member before handing its slot back to the datum allocator.
fn unlink(c: &mut CpuAbi, g: &mut Guest, _: u32) -> Result<TransferAbi> {
    let pool = Pool::read(g)?;
    c.edx = pool.header;
    push(c, g, c.esi)?;
    push(c, g, c.edi)?;
    let at = pool.record(c.eax);
    let next = g.u32(at.wrapping_add(0x34))?;
    if next != NONE {
        push(c, g, c.ebx)?;
        g.set_u32(pool.record(next).wrapping_add(0x30), g.u32(at.wrapping_add(0x30))?)?;
        c.ebx = pop(c, g)?;
    }
    let prev = g.u32(at.wrapping_add(0x30))?;
    arithmetic(c, prev, NONE, true);
    c.ecx = g.u32(at.wrapping_add(0x34))?;
    if prev != NONE {
        let off = (prev & 0xffff) << 6;
        let data = g.u32(pool.header.wrapping_add(layout::DATA))?;
        logic(c, off, 1 << 31);
        c.eflags_defined &= !0x800;
        g.set_u32(data.wrapping_add(off).wrapping_add(0x34), c.ecx)?;
    } else {
        let cluster = g.u16(at.wrapping_add(0x0a))?;
        sim_native::sub_flags(c, u32::from(cluster), 0xffff, 16);
        let groups = g.u32(GROUPS)?;
        let head = if cluster == u16::MAX {
            groups.wrapping_add(0x3800)
        } else {
            let mode = i32::from(g.u8(at.wrapping_add(3))? as i8);
            let index = arithmetic(c, (mode as u32) << 9, i32::from(cluster as i16) as u32, false);
            groups.wrapping_add(index.wrapping_mul(4))
        };
        g.set_u32(head, c.ecx)?;
    }
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    Ok(TransferAbi::jump(DELETE_DATUM))
}

// The root propagates its *entry* opacity to children. Sampling it after
// clock_step would advance the pieces a frame relative to the original.
fn root(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Result<TransferAbi> {
    if pc == ROOT.base {
        let pool = Pool::read(g)?;
        push(c, g, c.edi)?;
        c.ecx = pool.record(c.edx);
        c.eax = g.u32(c.ecx.wrapping_add(0x38))?;
        arithmetic(c, c.eax, c.edx, true);
        if c.eax != c.edx {
            c.edi = pop(c, g)?;
            return g.ret(c, 0);
        }
        c.edi = u32::from(g.u8(c.ecx.wrapping_add(0x27))?);
        push(c, g, c.esi)?;
        c.esi = g.u32(c.ecx.wrapping_add(0x3c))?;
        return g.call(c, CLOCK_STEP.base, AFTER_CLOCK);
    }
    logic(c, u32::from(c.eax as u8), 0x80);
    if c.eax as u8 == 0 {
        arithmetic(c, c.esi, NONE, true);
        if c.esi != NONE {
            c.edx = g.u32(DECALS)?;
            c.ecx = c.edi << 24;
            while c.esi != NONE {
                let data = g.u32(c.edx.wrapping_add(layout::DATA))?;
                c.eax = data.wrapping_add((c.esi & 0xffff) << 6);
                let color = g.u32(c.eax.wrapping_add(0x24))?;
                c.esi = g.u32(c.eax.wrapping_add(0x3c))?;
                c.edi = (color & 0xff_ffff) | c.ecx;
                arithmetic(c, c.esi, NONE, true);
                g.set_u32(c.eax.wrapping_add(0x24), c.edi)?;
            }
        }
    }
    c.esi = pop(c, g)?;
    c.edi = pop(c, g)?;
    g.ret(c, 0)
}

/// Result of advancing one root's clock. The fade operand remains a raw
/// binary32 until the original x87 conversion. The three decisions have
/// different guest continuations, so no borrowed record crosses one.
enum Appearance { Opaque, Release, Fade(u32) }

fn appearance(c: &mut CpuAbi, g: &Guest) -> Result<Appearance> {
    let lifetime = g.u32(c.esi.wrapping_add(0x1c))?;
    let age = sse::lane0(c, 2);
    sse::load(c, 0, lifetime);
    sse::zero(c, 1);
    sse::ucomiss(c, lifetime, 0);
    sse::lahf(c);
    let classification = (c.eax >> 8) & 0x44;
    logic(c, classification, 0x80);
    // An ordered zero lifetime is permanent. The original takes the
    // expiration comparison for both nonzero numbers and unordered inputs.
    if sse::parity(c) {
        sse::comiss(c, lifetime, age);
        if sse::below_or_equal(c) { return Ok(Appearance::Release); }
    }
    sse::load(c, 0, lifetime);
    sse::comiss(c, lifetime, 0);
    if sse::below_or_equal(c) { return Ok(Appearance::Opaque); }
    let window = g.u32(c.esi.wrapping_add(0x20))?;
    sse::load(c, 0, window);
    sse::comiss(c, window, 0);
    if sse::below_or_equal(c) { return Ok(Appearance::Opaque); }
    sse::load(c, 0, lifetime);
    let remaining = sse::ss(c, Ss::Sub, 0, age);
    sse::load(c, 2, window);
    sse::comiss(c, window, remaining);
    if sse::below_or_equal(c) { return Ok(Appearance::Opaque); }
    sse::ss(c, Ss::Div, 0, window);
    let opacity = sse::ss(c, Ss::Mul, 0, g.u32(OPACITY_SCALE)?);
    sse::comiss(c, 0, opacity);
    if !sse::below_or_equal(c) { return Ok(Appearance::Fade(0)); }
    let limit = g.u32(OPACITY_LIMIT)?;
    sse::load(c, 1, limit);
    sse::comiss(c, opacity, limit);
    Ok(Appearance::Fade(if sse::below_or_equal(c) { opacity } else { limit }))
}

fn expire(c: &mut CpuAbi, g: &mut Guest) -> Result<TransferAbi> {
    logic(c, c.ecx & 1, 0x80);
    if c.ecx & 1 != 0 {
        c.edx = g.u32(GROUPS)?;
        let counter = c.edx.wrapping_add(0x3804);
        c.ecx &= !1;
        g.set_u8(c.esi.wrapping_add(2), c.ecx as u8)?;
        g.set_u32(counter, g.u32(counter)?.wrapping_sub(1))?;
        c.eax = g.u32(c.esi.wrapping_add(0x3c))?;
        arithmetic(c, c.eax, NONE, true);
        while c.eax != NONE {
            let data = g.u32(c.edi.wrapping_add(layout::DATA))?;
            let child = data.wrapping_add((c.eax & 0xffff) << 6);
            c.ecx = byte(data, g.u8(child.wrapping_add(2))?);
            if c.ecx & 1 != 0 {
                c.ecx &= !1;
                g.set_u8(child.wrapping_add(2), c.ecx as u8)?;
                g.set_u32(counter, g.u32(counter)?.wrapping_sub(1))?;
            }
            c.eax = g.u32(child.wrapping_add(0x3c))?;
            arithmetic(c, c.eax, NONE, true);
        }
    }
    c.ebx = g.u32(c.esi.wrapping_add(4))?;
    c.edi = g.u32(GEOMETRY)?;
    g.call(c, RELEASE_GEOMETRY, AFTER_RELEASE)
}

fn clock_step(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Result<TransferAbi> {
    let released = match pc {
        AFTER_RELEASE => true,
        AFTER_CONVERT => {
            c.edx = g.u32(c.esi.wrapping_add(0x24))? & 0xff_ffff;
            c.eax = (c.eax << 24) | c.edx;
            logic(c, c.eax, 1 << 31);
            g.set_u32(c.esi.wrapping_add(0x24), c.eax)?;
            false
        }
        _ => {
            push(c, g, c.ecx)?;
            push(c, g, c.ebx)?;
            push(c, g, c.esi)?;
            push(c, g, c.edi)?;
            let pool = Pool::read(g)?;
            c.edi = pool.header;
            c.esi = pool.record(c.eax);
            c.eax = g.u32(CLOCK)?;
            let ticks = g.u32(c.eax.wrapping_add(8))?
                .wrapping_sub(g.u32(c.esi.wrapping_add(0x18))?);
            sse::cvtsi2ss(c, 2, ticks);
            c.ecx = byte(ticks, g.u8(c.esi.wrapping_add(2))?);
            sse::ss(c, Ss::Mul, 2, g.u32(c.eax.wrapping_add(4))?);
            c.ebx &= !0xff;
            logic(c, c.ecx & 2, 0x80);
            g.set_u8(c.esi.wrapping_add(0x27), u8::MAX)?;
            if c.ecx & 2 == 0 {
                match appearance(c, g)? {
                    Appearance::Release => return expire(c, g),
                    Appearance::Opaque => (),
                    Appearance::Fade(bits) => {
                        let local = c.esp.wrapping_add(0xc);
                        g.set_u32(local, bits)?;
                        let mut x = load_fpu(c);
                        x.fld(bits).map_err(fp_error)?;
                        finish_fpu(c, g, x, FADE_LOAD, local)?;
                        return g.call(c, FLOAT_TO_INTEGER, AFTER_CONVERT);
                    }
                }
            }
            false
        }
    };
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.eax = byte(c.eax, if released { 1 } else { c.ebx as u8 });
    c.ebx = pop(c, g)?;
    c.ecx = pop(c, g)?;
    g.ret(c, 0)
}
