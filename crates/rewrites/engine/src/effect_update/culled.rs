//! `179730 (effect: ebx) -> al`: whether the effect walk skips an effect
//! this tick (docs/engine/effects-parts.md, "The per-effect bodies").
//!
//! The datum is `[[4ea93c]+44] + (handle & 0xffff) * 0x190`, its
//! definition `[[4e3b44] + (datum+4 & 0xffff) * 16 + 8]`. `visible` starts
//! true and is worked out only for an effect with a cluster word (`+20`
//! not -1):
//!
//! 1. `3eb20` (`edx` the cluster word); a true answer with an object
//!    (`+48` not -1) makes it false;
//! 2. a split-screen record `[510c50]` with byte `+5` set makes it false;
//! 3. otherwise the definition's bit 2 picks `11c080` or `11c050` (`eax`
//!    the datum's `+1c`) and their answer, inverted, is it.
//!
//! When `visible` differs from the datum's bit 3 of `+2` the bit follows
//! it. An effect that turns invisible with neither bit 1 nor bit 9 of the
//! flags word set asks `177610` (`eax` the handle); a true answer makes
//! the result true. The result is `esi == 0` after `test esi, esi`.
//!
//! Flags: the calls carry the `cmp cx, 0xffff` and the two `test al, 1`
//! before them; every other flag is overwritten by the final `test`.

use super::{at, unowned};
use crate::sim_objects::alu::{cmp16, rmw8, shr};
use crate::sim_objects::{Cx, Next, Result};
use crate::sim_physics::{byte, logic, pop, push};

pub(crate) const ENTRY: u32 = 0x0017_9730;
const ARRAY: u32 = 0x004e_a93c;
const TAGS: u32 = 0x004e_3b44;
const SPLIT: u32 = 0x0051_0c50;

const CLUSTER: u32 = 0x0003_eb20;
const SEEN_B: u32 = 0x0011_c080;
const SEEN_A: u32 = 0x0011_c050;
const RETIRE: u32 = 0x0017_7610;

const CLUSTER_SITE: u32 = 0x0017_9774;
const SEEN_B_SITE: u32 = 0x0017_979f;
const SEEN_A_SITE: u32 = 0x0017_97ac;
const RETIRE_SITE: u32 = 0x0017_97e8;

/// The return addresses of its calls.
pub(crate) const SITES: [u32; 4] = [CLUSTER_SITE, SEEN_B_SITE, SEEN_A_SITE, RETIRE_SITE];

fn set_al(c: &mut runtime::CpuAbi, v: u8) {
    c.eax = byte(c.eax, v);
}

/// `xor eax, eax; test esi, esi; pop edi; sete al; pop esi; ret`.
fn leave(cx: &mut Cx<'_>) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    c.eax = u32::from(c.esi == 0);
    logic(c, c.esi, 1 << 31);
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    Ok(Next::Ret { pop: 0 })
}

/// From `1797b6`, with `al` the visibility.
fn settle(cx: &mut Cx<'_>) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let visible = c.eax as u8;
    let flags = at(c.esi, 2);
    let bit = (g.u8(flags)? >> 3) & 1;
    c.ecx = byte(c.ecx, bit);
    if visible == bit {
        return leave(cx);
    }
    if visible == 0 {
        rmw8(c, g, flags, |b| b & 0xf7)?;
        return leave(cx);
    }
    rmw8(c, g, flags, |b| b | 8)?;
    let word = u32::from(g.u16(flags)?);
    c.eax = word;
    c.edx = byte(c.edx, (word as u8) >> 1);
    if c.edx & 1 != 0 {
        return leave(cx);
    }
    c.eax = shr(c, word, 9, 32);
    logic(c, c.eax & 1, 0x80);
    if c.eax & 1 != 0 {
        return leave(cx);
    }
    c.eax = c.ebx;
    Ok(Next::Call { target: RETIRE, site: RETIRE_SITE })
}

pub(crate) fn culled(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    match pc {
        ENTRY => {
            let (c, g) = (&mut *cx.cpu, &mut *cx.g);
            c.eax = g.u32(ARRAY)?;
            c.ecx = g.u32(at(c.eax, 0x44))?;
            c.edx = g.u32(TAGS)?;
            let v = c.esi;
            push(c, g, v)?;
            c.esi = (c.ebx & 0xffff).wrapping_mul(0x190).wrapping_add(c.ecx);
            c.ecx = (g.u32(at(c.esi, 4))? & 0xffff) << 4;
            let v = c.edi;
            push(c, g, v)?;
            c.edi = g.u32(c.ecx.wrapping_add(c.edx).wrapping_add(8))?;
            let cluster = u32::from(g.u16(at(c.esi, 0x20))?);
            c.ecx = (c.ecx & 0xffff_0000) | cluster;
            cmp16(c, cluster, 0xffff);
            set_al(c, 1);
            if cluster == 0xffff {
                return settle(cx);
            }
            c.edx = cluster as u16 as i16 as i32 as u32;
            Ok(Next::Call { target: CLUSTER, site: CLUSTER_SITE })
        }
        CLUSTER_SITE => {
            let (c, g) = (&mut *cx.cpu, &mut *cx.g);
            if c.eax as u8 != 0 && g.u32(at(c.esi, 0x48))? != u32::MAX {
                set_al(c, 0);
                return settle(cx);
            }
            c.eax = g.u32(SPLIT)?;
            if c.eax != 0 {
                let b = g.u8(at(c.eax, 5))?;
                c.ecx = byte(c.ecx, b);
                if b != 0 {
                    set_al(c, 0);
                    return settle(cx);
                }
            }
            c.eax = g.u32(c.edi)? >> 2;
            logic(c, c.eax & 1, 0x80);
            let second = c.eax & 1 != 0;
            c.eax = at(c.esi, 0x1c);
            Ok(if second {
                Next::Call { target: SEEN_B, site: SEEN_B_SITE }
            } else {
                Next::Call { target: SEEN_A, site: SEEN_A_SITE }
            })
        }
        SEEN_B_SITE | SEEN_A_SITE => {
            let c = &mut *cx.cpu;
            set_al(c, u8::from(c.eax as u8 == 0));
            settle(cx)
        }
        RETIRE_SITE => {
            if cx.cpu.eax as u8 != 0 {
                cx.cpu.esi = 0;
            }
            leave(cx)
        }
        _ => Err(unowned(pc)),
    }
}
