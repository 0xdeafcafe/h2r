//! Sound listeners, portal propagation and the gain they send to the mixer.
//! The list walk crosses collision and sound-record callbacks with its guest
//! frame intact. Each callback can change the records the next one sees.
use crate::points::{Flag, Loc, Point, Sig};
use crate::sim_objects::{Body, Cx, Next, Result, frame, sse};
use crate::sim_physics::{ARITH, arithmetic, logic, spatial, status};
use runtime::CpuAbi;
use sim_sound::environment::{ClusterPair, GainCurve};

mod portals;

const DOC: &str = "docs/engine/sim-rewrite.md";
const GLOBALS: u32 = 0x004e_6380;
const BSP: u32 = 0x004e_0348;
const OBSTRUCTION_LIMIT: u32 = 0x0045_dd5c;
pub const LINEAR_GAIN: Point = Point::new(
    "SOUND_LINEAR_GAIN",
    0x2195f0,
    Sig::Fn {
        args: &[("decibels", Loc::Stack(0))],
        ret: None,
        pop: 4,
    },
    DOC,
)
.with_flags(&[Flag::Optional, Flag::Hot]);
pub const OBSTRUCTED: Point = Point::new(
    "SOUND_OBSTRUCTED",
    0x221da0,
    Sig::Fn {
        args: &[
            ("listener", Loc::Ecx),
            ("source", Loc::Edx),
            ("scale", Loc::Stack(0)),
        ],
        ret: Some(Loc::Al),
        pop: 4,
    },
    DOC,
)
.with_flags(&[Flag::Optional]);
pub const ENVIRONMENT: Point = Point::new(
    "SOUND_LISTENER_ENVIRONMENT",
    0x221a70,
    Sig::Fn {
        args: &[("listener", Loc::Stack(0)), ("sources", Loc::Stack(1))],
        ret: None,
        pop: 8,
    },
    DOC,
)
.with_flags(&[Flag::Optional, Flag::Hot]);

pub const CLUSTER_DISTANCE: Point = Point::new(
    "SOUND_CLUSTER_DISTANCE",
    0x249d60,
    Sig::Fn {
        args: &[("a", Loc::Eax), ("b", Loc::Ecx), ("bsp", Loc::Edx)],
        ret: Some(Loc::Xmm0),
        pop: 0,
    },
    DOC,
)
.with_flags(&[Flag::Optional]);
pub const BODIES: &[Body] = &[
    Body {
        point: &CLUSTER_DISTANCE,
        sites: &[AFTER_FALLBACK],
        run: distance,
    },
    Body {
        point: &LINEAR_GAIN,
        sites: &[],
        run: gain,
    },
    Body {
        point: &OBSTRUCTED,
        sites: &[AFTER_DISTANCE],
        run: obstructed,
    },
    Body {
        point: &ENVIRONMENT,
        sites: portals::SITES,
        run: portals::update,
    },
];

fn gain(cx: &mut Cx<'_>, _: u32) -> Result<Next> {
    // The lower clamp loads its comparison threshold and its replacement
    // separately. Keep both reads from the mapped image.
    const FLOOR: u32 = 0x0044_a0b4;
    const FLOOR_RESULT: u32 = 0x0021_960c;
    const CEILING: u32 = 0x0045_dbd8;
    const SCALE: u32 = 0x0045_dc1c;
    const EXP_SCALE: u32 = 0x0044_a0bc;
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let arg = c.esp.wrapping_add(4);
    c.eax = g.u32(arg)?;
    sse::load(c, 1, g.u32(FLOOR)?);
    g.set_u32(arg, c.eax)?;
    sse::load(c, 0, c.eax);
    sse::comiss(c, sse::lane0(c, 1), c.eax);
    if !sse::below_or_equal(c) {
        c.eax = g.u32(FLOOR_RESULT)?;
    } else {
        sse::comiss(c, c.eax, g.u32(CEILING)?);
        if !sse::below_or_equal(c) {
            c.eax = 0;
            logic(c, 0, 1 << 31);
        }
    }
    g.set_u32(arg, c.eax)?;
    let mut fpu = spatial::load_fpu(c);
    GainCurve {
        decibel_scale: g.u32(SCALE)?,
        exponent_scale: g.u32(EXP_SCALE)?,
    }
    .linear(c.eax, &mut fpu.0);
    spatial::finish_fpu(c, g, fpu, 0x219645, EXP_SCALE)?;
    Ok(Next::Ret { pop: 4 })
}

fn low8(word: u32, b: u8) -> u32 {
    (word & !0xff) | u32::from(b)
}
fn low16(word: u32, w: u16) -> u32 {
    (word & !0xffff) | u32::from(w)
}
fn signed16(word: u32) -> u32 {
    (word as i16 as i32) as u32
}
fn listener(globals: u32, index: u32) -> u32 {
    globals
        .wrapping_add(0x88)
        .wrapping_add(index.wrapping_mul(0x48))
}

fn compare_small(c: &mut CpuAbi, a: u32, b: u32, bits: u32) {
    let sign = 1 << (bits - 1);
    let mask = (1 << bits) - 1;
    let (a, b) = (a & mask, b & mask);
    let diff = a.wrapping_sub(b) & mask;
    let of = u32::from((a ^ b) & (a ^ diff) & sign != 0) << 11;
    c.eflags =
        (c.eflags & !ARITH) | status(diff, sign) | ((a ^ b ^ diff) & 0x10) | u32::from(a < b) | of;
    c.eflags_defined |= ARITH;
}

const AFTER_DISTANCE: u32 = 0x221dfd;

fn obstructed(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let ret = Next::Ret { pop: 4 };
    if pc != AFTER_DISTANCE {
        c.eax = low8(c.eax, u8::from(c.ecx == u32::MAX));
        logic(c, c.eax & 0xff, 0x80);
        if c.ecx == u32::MAX {
            return Ok(ret);
        }
        frame::push(c, g, c.ebx)?;
        let flags = g.u8(c.edx)? & 1;
        logic(c, u32::from(flags), 0x80);
        if flags != 0 {
            c.ebx = frame::pop(c, g)?;
            return Ok(ret);
        }
        let channel = g.u8(c.edx.wrapping_add(3))? & 15;
        c.ebx = low8(c.ebx, channel);
        compare_small(c, u32::from(channel), 1, 8);
        if channel != 1 {
            c.ebx = frame::pop(c, g)?;
            return Ok(ret);
        }
        let index = c.ecx;
        c.eax = index.wrapping_mul(9);
        c.ecx = listener(g.u32(GLOBALS)?, index);
        let source_cluster = g.u16(c.edx.wrapping_add(0x2c))?;
        c.eax = low16(c.eax, source_cluster);
        compare_small(c, c.eax, u32::from(u16::MAX), 16);
        if source_cluster == u16::MAX {
            c.eax = low8(c.eax, 1);
            c.ebx = frame::pop(c, g)?;
            return Ok(ret);
        }
        frame::push(c, g, c.esi)?;
        let listener_cluster = g.u16(c.ecx.wrapping_add(4))?;
        c.esi = low16(c.esi, listener_cluster);
        compare_small(c, c.esi, u32::from(u16::MAX), 16);
        if listener_cluster == u16::MAX {
            c.eax = low8(c.eax, 1);
        } else {
            c.ecx = g.u32(c.ecx)?;
            let different_object = c.ecx != g.u32(c.edx.wrapping_add(0x28))?;
            if different_object && listener_cluster != source_cluster {
                compare_small(c, c.esi, c.eax, 16);
                c.edx = g.u32(BSP)?;
                c.ecx = signed16(c.eax);
                c.eax = signed16(c.esi);
                return Ok(Next::Call {
                    target: CLUSTER_DISTANCE.base,
                    site: AFTER_DISTANCE,
                });
            }
            c.eax = low8(c.eax, 0);
            logic(c, 0, 0x80);
        }
    } else {
        let scaled = sse::ss(c, sse::Ss::Mul, 0, frame::slot(c, g, 12)?);
        let limit = g.u32(OBSTRUCTION_LIMIT)?;
        sse::load(c, 1, limit);
        sse::comiss(c, limit, scaled);
        if sse::below_or_equal(c) {
            c.eax = low8(c.eax, c.ebx as u8);
        } else {
            c.eax = low8(c.eax, 0);
            logic(c, 0, 0x80);
        }
    }
    c.esi = frame::pop(c, g)?;
    c.ebx = frame::pop(c, g)?;
    Ok(ret)
}

const AFTER_FALLBACK: u32 = 0x249dff;

fn distance(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc == AFTER_FALLBACK {
        c.ecx = c.eax & 0x7f;
        logic(c, c.ecx, 1 << 31);
        sse::cvtsi2ss(c, 0, c.ecx);
        sse::ss(c, sse::Ss::Mul, 0, g.u32(0x0045_ddc4)?);
    } else {
        arithmetic(c, c.eax, c.ecx, true);
        sse::zero(c, 1);
        sse::load(c, 0, g.u32(0x0045_dc38)?);
        frame::push(c, g, c.edi)?;
        c.edi = c.edx;
        if c.eax == c.ecx {
            sse::zero(c, 0);
        } else {
            c.edx = g.u32(c.edi.wrapping_add(0x224))?;
            logic(c, c.edx, 1 << 31);
            if c.edx as i32 <= 0 {
                return Ok(Next::Call {
                    target: 0x249ed0,
                    site: AFTER_FALLBACK,
                });
            }
            frame::push(c, g, c.ebx)?;
            frame::push(c, g, c.esi)?;
            c.ebx = g.u32(c.edi.wrapping_add(0x228))?;
            let count = g.u32(c.edi.wrapping_add(0x9c))? as i32;
            let pair = ClusterPair::new(c.eax as i32, c.ecx as i32, count);
            c.eax = pair.skipped as u32;
            c.ecx = pair.upper as u32;
            c.esi = g.u32(c.ebx.wrapping_add(0x28))?;
            c.edx = c.esi.wrapping_add(pair.row_bias as u32);
            let level = g.u8(c.edx.wrapping_add(c.ecx).wrapping_sub(1))?;
            c.eax = low8(c.eax, level);
            match level {
                0 => {
                    logic(c, 0, 0x80);
                    sse::zero(c, 0);
                }
                u8::MAX => {
                    compare_small(c, 255, 255, 8);
                    sse::load(c, 0, g.u32(OBSTRUCTION_LIMIT)?);
                }
                _ => {
                    // DEC preserves the carry from the comparison to 255.
                    let value = u32::from(level);
                    c.eax = arithmetic(c, value, 1, true);
                    c.eflags |= 1;
                    sse::load(c, 1, g.u32(c.ebx.wrapping_add(4))?);
                    sse::load(c, 2, g.u32(c.ebx.wrapping_add(8))?);
                    sse::cvtsi2ss(c, 0, c.eax);
                    sse::ss(c, sse::Ss::Mul, 0, g.u32(0x0045_ddc8)?);
                    sse::ss_reg(c, sse::Ss::Sub, 2, 1);
                    sse::ss_reg(c, sse::Ss::Mul, 0, 2);
                    sse::ss_reg(c, sse::Ss::Add, 0, 1);
                }
            }
            c.esi = frame::pop(c, g)?;
            c.ebx = frame::pop(c, g)?;
        }
    }
    c.edi = frame::pop(c, g)?;
    Ok(Next::Ret { pop: 0 })
}
