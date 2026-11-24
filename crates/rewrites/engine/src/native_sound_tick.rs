//! Sound frame service (125b40). A mapped listener's obstruction query,
//! stream completion or pause change can reenter the guest. The scratch
//! arrays, callee-saved registers and real return sites stay live across
//! those calls. Their owners can therefore inspect or change the frame.

use crate::points;
use crate::sim_objects::frame::{pop, push};
use crate::sim_objects::{Body, Cx, Next, Result, sse};
use crate::sim_physics::{ARITH, arithmetic, byte, logic, status};
use runtime::{CpuAbi, Guest, GuestFault};
use sim_sound::tick::{USER_COUNT, next_listener};

pub const BODIES: &[Body] = &[Body {
    point: &points::SOUND_UPDATE,
    sites: &[
        0x125b4a, 0x125b80, 0x125bc7, 0x125bde, 0x125be3, 0x125be8, 0x125bfe, 0x125c0a,
        0x125c1d, 0x125c23, 0x125c28, 0x125c39, 0x125c3e, 0x125c43, 0x125c48, 0x125c87,
        0x125c8c, 0x125ca1, 0x125ce9, 0x125cee, 0x125d02, 0x125d09, 0x125d30,
    ],
    run: frame,
}];

const FRAME: u32 = 0x1744;
const CACHE_GLOBAL: u32 = 0x0050_210c;
const STACK_PROBE: u32 = 0x0032_0560;
const SERVICE_BEGIN: u32 = 0x0021_ec00;
const RESUME_CHANNEL: u32 = 0x0012_a0b0;
const COMMIT_SOUND: u32 = 0x0021_9d90;
const UNPAUSE: u32 = 0x0021_f5a0;
const READ_CLOCK: u32 = 0x0033_14b0;
const SERVICE_CLOCK: u32 = 0x0021_f360;
const UPDATE_CLOCK: u32 = 0x0012_69f0;
const UPDATE_ENVIRONMENT: u32 = 0x0022_1900;
const BEFORE_LOOPS: u32 = 0x0021_9cc0;
const UPDATE_LOOPS: u32 = 0x0021_b0e0;
const UPDATE_INSTANCES: u32 = 0x0012_90d0;
const UPDATE_SOURCES: u32 = 0x0012_8020;
const UPDATE_CHANNELS: u32 = 0x0012_82d0;
const COLLECT_EFFECTS: u32 = 0x0012_9100;
const FIRST_USER: u32 = 0x0014_dde0;
const USER_OBSTRUCTION: u32 = 0x0022_1a70;
const APPLY_EFFECTS: u32 = 0x0012_9380;
const UPDATE_VOICES: u32 = 0x0012_93f0;
const SERVICE_STREAMS: u32 = 0x0021_ee80;
const REBASE_CACHE_AGES: u32 = 0x0013_d230;

fn call(target: u32, site: u32) -> Result<Next> {
    Ok(Next::Call { target, site })
}

fn frame(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let c = &mut *cx.cpu;
    let g = &mut *cx.g;
    match pc {
        0x125b40 => {
            c.eax = FRAME;
            call(STACK_PROBE, 0x125b4a)
        }
        0x125b4a => {
            c.eax = g.u32(points::SOUND_GLOBALS.base)?;
            for offset in [0x78, 0x79, 0x7a] {
                let flag = g.u8(c.eax.wrapping_add(offset))?;
                c.ecx = byte(c.ecx, flag);
                logic(c, flag.into(), 0x80);
                if offset == 0x78 {
                    push(c, g, c.esi)?;
                    push(c, g, c.edi)?;
                }
                if flag == 0 {
                    return advance_cache(c, g);
                }
            }
            push(c, g, c.ebx)?;
            c.ebx = 1;
            g.set_u8(c.eax.wrapping_add(0x7d), c.ebx as u8)?;
            call(SERVICE_BEGIN, 0x125b80)
        }
        0x125b80 => {
            c.esi = g.u32(points::SOUND_GLOBALS.base)?;
            if !manager_flag(c, g, 0x7a)? {
                return call(SERVICE_STREAMS, 0x125d02);
            }
            if manager_flag(c, g, 0x7b)? {
                c.eax = g.u32(points::GAME_TIME.base)?;
                let initialized = g.u8(c.eax)?;
                arithmetic(c, initialized.into(), 0, true);
                let paused = initialized != 0 && {
                    let paused = g.u8(c.eax.wrapping_add(1))?;
                    c.ecx = byte(c.ecx, paused);
                    logic(c, paused.into(), 0x80);
                    paused != 0
                };
                if !paused {
                    c.edi = 0;
                    g.set_u8(c.esi.wrapping_add(0x7d), c.ebx as u8)?;
                    g.set_u8(c.esi.wrapping_add(0x7b), 0)?;
                    let count = g.u16(c.esi.wrapping_add(0x208))?;
                    c.eflags = (c.eflags & !ARITH) | status(count.into(), 0x8000);
                    c.eflags_defined |= ARITH;
                    if (count as i16) > 0 {
                        c.eax = c.edi;
                        return call(RESUME_CHANNEL, 0x125bc7);
                    }
                    return finish_resume(c, g);
                }
            }
            service_clock(c, g)
        }
        0x125bc7 => {
            c.eax = g.u32(points::SOUND_GLOBALS.base)?;
            c.ecx = g.u16(c.eax.wrapping_add(0x208))? as i16 as i32 as u32;
            c.edi = c.edi.wrapping_add(1);
            arithmetic(c, c.edi, c.ecx, true);
            if (c.edi as i32) < (c.ecx as i32) {
                c.eax = c.edi;
                call(RESUME_CHANNEL, 0x125bc7)
            } else {
                finish_resume(c, g)
            }
        }
        0x125bde => call(UNPAUSE, 0x125be3),
        0x125be3 => call(READ_CLOCK, 0x125be8),
        0x125be8 => {
            c.esi = g.u32(points::SOUND_GLOBALS.base)?;
            g.set_u32(c.esi.wrapping_add(0x80), c.eax)?;
            g.set_u8(c.esi.wrapping_add(0x7d), 0)?;
            service_clock(c, g)
        }
        0x125bfe => {
            if manager_flag(c, g, 0x7b)? {
                environment(c, g)
            } else {
                call(UPDATE_CLOCK, 0x125c0a)
            }
        }
        0x125c0a => {
            c.esi = g.u32(points::SOUND_GLOBALS.base)?;
            environment(c, g)
        }
        0x125c1d => {
            push(c, g, c.ebx)?;
            call(points::SOUND_LISTENERS_UPDATE.base, 0x125c23)
        }
        0x125c23 => call(BEFORE_LOOPS, 0x125c28),
        0x125c28 => {
            c.edx = g.u32(points::SOUND_GLOBALS.base)?;
            logic(c, 0, 1 << 31);
            c.eax = g.u8(c.edx.wrapping_add(0x7e))?.into();
            push(c, g, c.eax)?;
            call(UPDATE_LOOPS, 0x125c39)
        }
        0x125c39 => call(UPDATE_INSTANCES, 0x125c3e),
        0x125c3e => call(UPDATE_SOURCES, 0x125c43),
        0x125c43 => call(UPDATE_CHANNELS, 0x125c48),
        0x125c48 => collect_effects(c, g),
        0x125c87 => call(FIRST_USER, 0x125c8c),
        0x125c8c => {
            c.esi = c.eax;
            obstruction_or_apply(c, g)
        }
        0x125ca1 => {
            c.edx = u32::MAX;
            let first = c.esi.wrapping_add(1);
            c.eax = first;
            if (first as i32) < USER_COUNT {
                let mappings = g.u32(points::PLAYER_GLOBALS.base)?.wrapping_add(0xc);
                let next = next_listener(c.esi as i32, |slot| {
                    c.eax = slot as u32;
                    c.ecx = mappings.wrapping_add(c.eax.wrapping_mul(4));
                    Ok::<_, GuestFault>(g.u32(c.ecx)? != u32::MAX)
                })?;
                if let Some(slot) = next {
                    c.edx = slot as u32;
                } else {
                    c.ecx = c.ecx.wrapping_add(4);
                    c.eax = USER_COUNT as u32;
                }
            }
            c.esi = c.edx;
            obstruction_or_apply(c, g)
        }
        0x125ce9 => call(UPDATE_VOICES, 0x125cee),
        0x125cee => {
            c.eax = g.u32(points::SOUND_GLOBALS.base)?;
            let advance = g.u8(c.eax.wrapping_add(0x7e))?;
            c.ecx = byte(c.ecx, advance);
            logic(c, advance.into(), 0x80);
            if advance != 0 {
                let at = c.eax.wrapping_add(0x7f);
                let old = g.u8(at)?;
                let new = old.wrapping_add(1);
                g.set_u8(at, new)?;
                let flags = status(new.into(), 0x80) | (u32::from(old & 15 == 15) << 4)
                    | (u32::from(old == i8::MAX as u8) << 11);
                c.eflags = (c.eflags & !(ARITH & !1)) | flags;
                c.eflags_defined |= ARITH & !1;
            }
            call(SERVICE_STREAMS, 0x125d02)
        }
        0x125d02 => {
            push(c, g, 0)?;
            call(COMMIT_SOUND, 0x125d09)
        }
        0x125d09 => {
            c.eax = g.u32(points::SOUND_GLOBALS.base)?;
            g.set_u8(c.eax.wrapping_add(0x7e), 0)?;
            g.set_u8(c.eax.wrapping_add(0x7d), 0)?;
            c.ebx = pop(c, g)?;
            advance_cache(c, g)
        }
        0x125d30 => finish(c, g),
        _ => Err(GuestFault {
            addr: pc,
            len: 1,
            access: runtime::guest::Access::Execute,
            reason: runtime::guest::FaultReason::Unmapped,
        }),
    }
}

fn manager_flag(c: &mut CpuAbi, g: &Guest, offset: u32) -> Result<bool> {
    let flag = g.u8(c.esi.wrapping_add(offset))?;
    c.eax = byte(c.eax, flag);
    logic(c, flag.into(), 0x80);
    Ok(flag != 0)
}

fn finish_resume(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    push(c, g, c.ebx)?;
    call(COMMIT_SOUND, 0x125bde)
}

fn service_clock(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    push(c, g, c.esi)?;
    call(SERVICE_CLOCK, 0x125bfe)
}

fn environment(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    sse::load(c, 1, g.u32(c.esi.wrapping_add(0x1e0))?);
    call(UPDATE_ENVIRONMENT, 0x125c1d)
}

fn collect_effects(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let s = c.esp;
    c.eax = 0;
    logic(c, 0, 1 << 31);
    let stride = if c.eflags & 0x400 == 0 { 4 } else { 4u32.wrapping_neg() };
    for (off, words) in [(0xc, 64), (0x10c, 128)] {
        c.ecx = words;
        c.edi = s.wrapping_add(off);
        // The two forward REP stores cover contiguous stack arrays. One
        // checked span per array takes the usual path. A faulting span or
        // a backwards walk needs the per-word stores and register progress.
        let bytes = (c.ecx * 4) as usize;
        if stride == 4 && g.write_bytes(c.edi, &[0; 512][..bytes], 1).is_ok() {
            c.edi = c.edi.wrapping_add(bytes as u32);
            c.ecx = 0;
            continue;
        }
        while c.ecx != 0 {
            g.set_u32(c.edi, 0)?;
            c.edi = c.edi.wrapping_add(stride);
            c.ecx -= 1;
        }
    }
    c.ecx = s.wrapping_add(0x10c);
    push(c, g, c.ecx)?;
    c.edx = s.wrapping_add(0xc);
    push(c, g, c.edx)?;
    c.edi = s.wrapping_add(0x30c);
    g.set_u32(c.edi, 0)?;
    call(COLLECT_EFFECTS, 0x125c87)
}

fn obstruction_or_apply(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    arithmetic(c, c.esi, u32::MAX, true);
    if c.esi != u32::MAX {
        c.eax = c.esp.wrapping_add(0x30c);
        push(c, g, c.eax)?;
        push(c, g, c.esi)?;
        call(USER_OBSTRUCTION, 0x125ca1)
    } else {
        c.edi = c.esp.wrapping_add(0x10c);
        c.ebx = c.esp.wrapping_add(0xc);
        call(APPLY_EFFECTS, 0x125ce9)
    }
}

fn advance_cache(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.esi = g.u32(CACHE_GLOBAL)?;
    c.eax = g.u32(c.esi.wrapping_add(0x38))?;
    c.edi = i32::MAX as u32;
    arithmetic(c, c.eax, c.edi, true);
    if c.eax == c.edi {
        c.eax = c.esi;
        return call(REBASE_CACHE_AGES, 0x125d30);
    }
    c.eax = c.eax.wrapping_add(1);
    g.set_u32(c.esi.wrapping_add(0x38), c.eax)?;
    finish(c, g)
}

fn finish(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    for offset in (0x44..=0x60).step_by(4) {
        g.set_u32(c.esi.wrapping_add(offset), c.edi)?;
    }
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, FRAME, false);
    Ok(Next::Ret { pop: 0 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::guest::Permissions;

    #[test]
    fn obstruction_returns_to_the_next_mapped_user_before_applying_effects() {
        let mut g = Guest::new();
        g.map_zeroed(points::PLAYER_GLOBALS.base & !4095, 4096, Permissions::RW).unwrap();
        g.map_zeroed(0x1000, 0x3000, Permissions::RW).unwrap();
        let player_base = 0x1500;
        g.set_u32(points::PLAYER_GLOBALS.base, player_base).unwrap();
        for (index, datum) in [5, u32::MAX, 7, u32::MAX].into_iter().enumerate() {
            g.set_u32(player_base + 0xc + index as u32 * 4, datum).unwrap();
        }
        let mut cpu = CpuAbi { esp: 0x2000, esi: 0, ..Default::default() };
        let mut cx = Cx { cpu: &mut cpu, g: &mut g };
        assert_eq!(frame(&mut cx, 0x125ca1).unwrap(), Next::Call { target: USER_OBSTRUCTION, site: 0x125ca1 });
        assert_eq!(cx.g.u32(cx.cpu.esp).unwrap(), 2);
        assert_eq!(cx.g.u32(cx.cpu.esp + 4).unwrap(), 0x230c);
        cx.cpu.esp += 8;
        assert_eq!(frame(&mut cx, 0x125ca1).unwrap(), Next::Call { target: APPLY_EFFECTS, site: 0x125ce9 });
        assert_eq!((cx.cpu.edi, cx.cpu.ebx), (0x210c, 0x200c));
    }

    #[test]
    fn partially_mapped_scratch_retains_store_progress() {
        let mut g = Guest::new();
        g.map_zeroed(0x1000, 0x1000, Permissions::RW).unwrap();
        g.write(0x1ff8, [0xff; 8]).unwrap();
        let mut c = CpuAbi { esp: 0x1fec, ..Default::default() };
        let fault = collect_effects(&mut c, &mut g).unwrap_err();
        assert_eq!(fault.addr, 0x2000);
        assert_eq!(g.read::<8>(0x1ff8).unwrap(), [0; 8]);
        assert_eq!((c.edi, c.ecx), (0x2000, 62));
    }
}
