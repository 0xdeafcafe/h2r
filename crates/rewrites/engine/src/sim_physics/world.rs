//! Havok step, island callbacks and the Halo overflow bookkeeping. Every
//! transfer parks the cursor in the caller's real frame. A callback can
//! replace an island's arrays or the current motion's vtable. The collide
//! phase is a frame replay here; the integrate phase is its arm
//! (`integrate_phase`). Both call the island entries below through the
//! dispatcher.
use super::*;

const MONITOR_HEAD: u32 = 0x0048_0154;
const MONITOR_END: u32 = 0x0048_0158;
const WORLD: u32 = 0x20;
const GRAVITY: u32 = 0xe0;

/// The step's entry condition: the monitor stream must be off, because
/// step functions append a timer record while `[MONITOR_HEAD] <
/// [MONITOR_END]` (sim-rewrite-havok.md §3.3). A read that faults declines.
pub(super) fn monitor_off(g: &Guest) -> bool {
    g.u32(MONITOR_HEAD)
        .is_ok_and(|head| g.u32(MONITOR_END).is_ok_and(|end| head >= end))
}

pub(super) const WORLD_SITES: &[u32] = &[0x31110e, 0x311116];
pub(super) const ISLAND_SITES: &[u32] = &[0x317d3a, 0x317d4e, 0x317d63, 0x317d69, 0x317d76, 0x317d7c];
pub(super) const INTEGRATE_SITES: &[u32] = &[0x316a48, 0x316aac, 0x316ab4, 0x316b16];

fn advance_stack(c: &mut CpuAbi, n: u32) {
    c.esp = arithmetic(c, c.esp, n, false);
}

fn decrement(c: &mut CpuAbi, x: u32) -> u32 {
    let carry = (c.eflags & 1, c.eflags_defined & 1);
    let out = arithmetic(c, x, 1, true);
    c.eflags = (c.eflags & !1) | carry.0;
    c.eflags_defined = (c.eflags_defined & !1) | carry.1;
    out
}

fn restore_island(c: &mut CpuAbi, g: &Guest, cleanup: u32) -> Result<TransferAbi> {
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    g.ret(c, cleanup)
}

pub(super) fn step(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Result<TransferAbi> {
    match pc {
        0x311100 => {
            push(c, g, c.esi)?;
            push(c, g, c.edi)?;
            c.edi = g.u32(c.esp.wrapping_add(12))?;
            c.esi = c.ecx;
            call_arg(c, g, c.edi, 0x30fcb0, WORLD_SITES[0])
        }
        0x31110e => {
            c.ecx = c.esi;
            call_arg(c, g, c.edi, 0x30fef0, WORLD_SITES[1])
        }
        0x311116 => restore_island(c, g, 4),
        _ => unreachable!(),
    }
}

pub(super) fn collide_island(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Result<TransferAbi> {
    match pc {
        0x317d30 => {
            push(c, g, c.esi)?;
            c.esi = c.ecx;
            push(c, g, c.edi)?;
            call_arg(c, g, c.esi, 0x278e60, ISLAND_SITES[0])
        }
        0x317d3a => {
            c.edi = g.u32(c.esp.wrapping_add(20))?;
            c.eax = g.u32(c.esp.wrapping_add(16))?;
            advance_stack(c, 4);
            push(c, g, c.edi)?;
            c.ecx = c.esi;
            call_arg(c, g, c.eax, 0x3175e0, ISLAND_SITES[1])
        }
        0x317d4e => {
            let world = g.u32(c.esi.wrapping_add(WORLD))?;
            let adaptive = g.u8(world.wrapping_add(0x23e))?;
            c.eax = byte(c.eax, adaptive);
            logic(c, u32::from(adaptive), 0x80);
            c.ecx = c.esi;
            let (target, resume) = if adaptive != 0 {
                (0x317950, ISLAND_SITES[2])
            } else {
                (0x317c00, ISLAND_SITES[4])
            };
            call_arg(c, g, c.edi, target, resume)
        }
        0x317d63 | 0x317d76 => {
            let resume = if pc == ISLAND_SITES[2] { ISLAND_SITES[3] } else { ISLAND_SITES[5] };
            call_arg(c, g, c.esi, 0x278e80, resume)
        }
        0x317d69 | 0x317d7c => {
            advance_stack(c, 4);
            restore_island(c, g, 8)
        }
        _ => unreachable!(),
    }
}

/// The collide phase's resumes: after the first pending pass, the island
/// collide, the island listeners, the second pending pass, the merges,
/// the queue, the third pending pass, and the step listeners.
pub(super) const COLLIDE_SITES: &[u32] = &[0x30fd8b, 0x30fdb3, 0x30fdfc, 0x30fe59, 0x30fe6a, 0x30fe7b, 0x30fe89, 0x30feba];

/// The world words the collide phase reads and writes
/// (docs/engine/havok-collide.md).
const W_ISLANDS: u32 = 0x08;
const W_ISLAND_COUNT: u32 = 0x0c;
const W_PENDING: u32 = 0x24;
const W_ISLAND_LISTENERS: u32 = 0xac;
const W_BROADPHASE: u32 = 0xc4;
const W_INPUT_SOURCE: u32 = 0xcc;
const W_MERGES: u32 = 0xf4;
const W_QUEUE: u32 = 0x100;
const W_LOCK: u32 = 0x12c;
const W_COLLIDING: u32 = 0x12d;
const W_INPUT: u32 = 0x140;
const W_STEP: u32 = 0x150;
const W_SCALED_DT: u32 = 0x170;
const W_SCALED_RATE: u32 = 0x174;
const W_ITERATIONS: u32 = 0x17c;
const W_SCALE: u32 = 0x180;
const W_SCALED_DT_COPY: u32 = 0x198;
const W_SCALED_RATE_COPY: u32 = 0x19c;
const W_ITERATION_DT: u32 = 0x1a0;
const W_RATE_TERM: u32 = 0x1a4;

const PENDING: u32 = 0x30f6b0;
const ISLAND_COLLIDE: u32 = 0x317d30;
const ISLAND_LISTENERS: u32 = 0x3188c0;
const MERGES: u32 = 0x30fc30;
const QUEUE: u32 = 0x30cda0;
const STEP_LISTENERS: u32 = 0x3187a0;

/// `cmp [MONITOR_HEAD], [MONITOR_END]` and its `jae`. The port's entry
/// condition holds the stream off (`supported`); a callee turning it on
/// would need the timer records retail writes with `rdtsc`.
fn monitor_still_off(c: &mut CpuAbi, g: &Guest) -> Result<()> {
    c.ecx = g.u32(MONITOR_HEAD)?;
    let end = g.u32(MONITOR_END)?;
    arithmetic(c, c.ecx, end, true);
    if c.ecx < end {
        unimplemented!("havok: the monitor stream turned on inside the collide phase");
    }
    Ok(())
}

/// `inc`: the carry flag is kept.
fn increment(c: &mut CpuAbi, x: u32) -> u32 {
    let carry = (c.eflags & 1, c.eflags_defined & 1);
    let out = arithmetic(c, x, 1, false);
    c.eflags = (c.eflags & !1) | carry.0;
    c.eflags_defined = (c.eflags_defined & !1) | carry.1;
    out
}

/// `mov eax, [esi+off]; test eax, eax`.
fn test_world(c: &mut CpuAbi, g: &Guest, off: u32) -> Result<u32> {
    c.eax = g.u32(c.esi.wrapping_add(off))?;
    logic(c, c.eax, 1 << 31);
    Ok(c.eax)
}

fn call_world(c: &mut CpuAbi, g: &mut Guest, target: u32, resume: u32) -> Result<TransferAbi> {
    c.ecx = c.esi;
    g.call(c, target, resume)
}

/// The step copy, the input copy and the four step terms
/// (`0x0030fce2..0x0030fd76`), in retail's order of loads and stores.
/// The x87 runs under the caller's control word, as retail's.
fn collide_terms(c: &mut CpuAbi, g: &mut Guest) -> Result<()> {
    use super::spatial::{fp_error, finish_fpu, load_fpu};
    let world = c.esi;
    let w = move |off: u32| world.wrapping_add(off);
    c.eax = g.u32(w(W_INPUT_SOURCE))?;
    c.ebp = g.u32(c.esp.wrapping_add(0x18))?;
    c.ecx = g.u32(c.ebp)?;
    g.set_u32(w(W_STEP), c.ecx)?;
    c.edx = g.u32(c.ebp.wrapping_add(4))?;
    g.set_u32(w(W_STEP + 4), c.edx)?;
    c.ebx = w(W_INPUT);
    c.ecx = c.ebx;
    for i in 0..3 {
        c.edx = g.u32(c.eax.wrapping_add(4 * i))?;
        g.set_u32(c.ecx.wrapping_add(4 * i), c.edx)?;
    }
    c.eax = g.u32(c.eax.wrapping_add(0xc))?;
    g.set_u32(c.ecx.wrapping_add(0xc), c.eax)?;
    let mut x = load_fpu(c);
    x.fld(g.u32(c.ebp)?).map_err(fp_error)?;
    x.fmul_m(g.u32(w(W_SCALE))?).map_err(fp_error)?;
    g.set_u32(w(W_SCALED_DT), x.fstp_m32().map_err(fp_error)?)?;
    c.ecx = g.u32(w(W_SCALED_DT))?;
    x.fild(g.u32(w(W_ITERATIONS))? as i32).map_err(fp_error)?;
    x.fmul_m(g.u32(c.ebp.wrapping_add(4))?).map_err(fp_error)?;
    g.set_u32(w(W_SCALED_RATE), x.fstp_m32().map_err(fp_error)?)?;
    g.set_u32(w(W_SCALED_DT_COPY), c.ecx)?;
    c.edx = g.u32(w(W_SCALED_RATE))?;
    g.set_u32(w(W_SCALED_RATE_COPY), c.edx)?;
    x.fild(g.u32(w(W_ITERATIONS))? as i32).map_err(fp_error)?;
    x.fmul_m(g.u32(w(W_SCALED_DT))?).map_err(fp_error)?;
    g.set_u32(w(W_ITERATION_DT), x.fstp_m32().map_err(fp_error)?)?;
    x.fld(g.u32(w(W_SCALE))?).map_err(fp_error)?;
    x.fmul_m(g.u32(w(W_SCALED_RATE))?).map_err(fp_error)?;
    g.set_u32(w(W_RATE_TERM), x.fstp_m32().map_err(fp_error)?)?;
    finish_fpu(c, &*g, x, 0x30fd70, w(W_RATE_TERM))
}

/// `0x0030fda0`: the island collide of island `edi`, `(broadphase, input)`.
fn collide_next_island(c: &mut CpuAbi, g: &mut Guest) -> Result<TransferAbi> {
    c.eax = g.u32(c.esi.wrapping_add(W_BROADPHASE))?;
    c.ecx = g.u32(c.esi.wrapping_add(W_ISLANDS))?;
    c.ecx = g.u32(c.ecx.wrapping_add(c.edi.wrapping_mul(4)))?;
    push(c, g, c.ebx)?;
    push(c, g, c.eax)?;
    g.call(c, ISLAND_COLLIDE, COLLIDE_SITES[1])
}

/// `0x0030fe31`: the island count is read again after every island.
fn collide_loop_tail(c: &mut CpuAbi, g: &mut Guest) -> Result<TransferAbi> {
    c.eax = g.u32(c.esi.wrapping_add(W_ISLAND_COUNT))?;
    c.edi = increment(c, c.edi);
    arithmetic(c, c.edi, c.eax, true);
    if (c.edi as i32) < (c.eax as i32) {
        return collide_next_island(c, g);
    }
    collide_after_islands(c, g)
}

/// `0x0030fe3d`: release the lock bytes, then the second pending pass.
fn collide_after_islands(c: &mut CpuAbi, g: &mut Guest) -> Result<TransferAbi> {
    let pending = test_world(c, g, W_PENDING)?;
    g.set_u8(c.esi.wrapping_add(W_COLLIDING), 0)?;
    g.set_u8(c.esi.wrapping_add(W_LOCK), 0)?;
    if pending as i32 > 0 {
        return call_world(c, g, PENDING, COLLIDE_SITES[3]);
    }
    collide_merges(c, g)
}

/// `0x0030fe59`.
fn collide_merges(c: &mut CpuAbi, g: &mut Guest) -> Result<TransferAbi> {
    if test_world(c, g, W_MERGES)? != 0 {
        return call_world(c, g, MERGES, COLLIDE_SITES[4]);
    }
    collide_queue(c, g)
}

/// `0x0030fe6a`.
fn collide_queue(c: &mut CpuAbi, g: &mut Guest) -> Result<TransferAbi> {
    if test_world(c, g, W_QUEUE)? != 0 {
        return call_world(c, g, QUEUE, COLLIDE_SITES[5]);
    }
    collide_last_pending(c, g)
}

/// `0x0030fe7b`.
fn collide_last_pending(c: &mut CpuAbi, g: &mut Guest) -> Result<TransferAbi> {
    if test_world(c, g, W_PENDING)? as i32 > 0 {
        return call_world(c, g, PENDING, COLLIDE_SITES[6]);
    }
    collide_step_listeners(c, g)
}

/// `0x0030fe89`: the step listeners, cdecl `(world, step)`.
fn collide_step_listeners(c: &mut CpuAbi, g: &mut Guest) -> Result<TransferAbi> {
    monitor_still_off(c, g)?;
    push(c, g, c.ebp)?;
    push(c, g, c.esi)?;
    g.call(c, STEP_LISTENERS, COLLIDE_SITES[7])
}

/// The collide phase `0x0030fcb0` (thiscall world, `&{dt, 1/dt}`, `ret 4`):
/// the step terms, the first pending pass, the island collide of every
/// island with its listeners, the lock release, the second pending pass,
/// merges, the queue, the third pending pass and the step listeners.
/// Every call is a guest call; the frame lives in the guest's registers
/// and stack as retail's does.
pub(super) fn collide(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Result<TransferAbi> {
    match pc {
        0x30fcb0 => {
            push(c, g, c.ecx)?;
            c.eax = g.u32(MONITOR_END)?;
            push(c, g, c.ebx)?;
            push(c, g, c.ebp)?;
            push(c, g, c.esi)?;
            c.esi = c.ecx;
            c.ecx = g.u32(MONITOR_HEAD)?;
            arithmetic(c, c.ecx, c.eax, true);
            push(c, g, c.edi)?;
            if c.ecx < c.eax {
                unimplemented!("havok: the collide phase entered with the monitor stream on");
            }
            collide_terms(c, g)?;
            let pending = test_world(c, g, W_PENDING)?;
            g.set_u8(c.esi.wrapping_add(W_LOCK), 1)?;
            if pending as i32 > 0 {
                return call_world(c, g, PENDING, COLLIDE_SITES[0]);
            }
            collide(c, g, COLLIDE_SITES[0])
        }
        0x30fd8b => {
            let count = g.u32(c.esi.wrapping_add(W_ISLAND_COUNT))?;
            c.eax = count;
            c.edi = 0;
            logic(c, count, 1 << 31);
            g.set_u8(c.esi.wrapping_add(W_COLLIDING), 1)?;
            if count as i32 <= 0 {
                return collide_after_islands(c, g);
            }
            collide_next_island(c, g)
        }
        0x30fdb3 => {
            if test_world(c, g, W_ISLAND_LISTENERS)? == 0 {
                return collide_loop_tail(c, g);
            }
            monitor_still_off(c, g)?;
            c.eax = g.u32(c.esi.wrapping_add(W_ISLANDS))?;
            g.set_u8(c.esi.wrapping_add(W_COLLIDING), 0)?;
            c.ecx = g.u32(c.eax.wrapping_add(c.edi.wrapping_mul(4)))?;
            push(c, g, c.ebp)?;
            push(c, g, c.ecx)?;
            push(c, g, c.esi)?;
            g.call(c, ISLAND_LISTENERS, COLLIDE_SITES[2])
        }
        0x30fdfc => {
            g.set_u8(c.esi.wrapping_add(W_COLLIDING), 1)?;
            c.ecx = g.u32(MONITOR_HEAD)?;
            c.eax = g.u32(MONITOR_END)?;
            advance_stack(c, 12);
            arithmetic(c, c.ecx, c.eax, true);
            if c.ecx < c.eax {
                unimplemented!("havok: the monitor stream turned on inside the collide phase");
            }
            collide_loop_tail(c, g)
        }
        0x30fe59 => collide_merges(c, g),
        0x30fe6a => collide_queue(c, g),
        0x30fe7b => collide_last_pending(c, g),
        0x30fe89 => collide_step_listeners(c, g),
        0x30feba => {
            c.ecx = g.u32(MONITOR_HEAD)?;
            c.eax = g.u32(MONITOR_END)?;
            advance_stack(c, 8);
            arithmetic(c, c.ecx, c.eax, true);
            c.edi = pop(c, g)?;
            c.esi = pop(c, g)?;
            c.ebp = pop(c, g)?;
            c.ebx = pop(c, g)?;
            if c.ecx < c.eax {
                unimplemented!("havok: the monitor stream turned on inside the collide phase");
            }
            c.ecx = pop(c, g)?;
            g.ret(c, 4)
        }
        _ => unreachable!(),
    }
}

fn finish_motion_pass(c: &mut CpuAbi, g: &Guest) -> Result<TransferAbi> {
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.ebx = pop(c, g)?;
    c.ecx = pop(c, g)?;
    g.ret(c, 4)
}

fn apply_motion(c: &mut CpuAbi, g: &mut Guest) -> Result<TransferAbi> {
    // Entity membership and gravity are live across both virtuals.
    let entities = g.u32(c.edi.wrapping_add(0x3c))?;
    c.edx = g.u32(entities.wrapping_add(c.ebx.wrapping_mul(4)))?;
    let world = g.u32(c.edi.wrapping_add(WORLD))?;
    c.esi = g.u32(c.edx.wrapping_add(0x3c))?;
    c.eax = g.u32(c.esi)?;
    let gravity = arithmetic(c, world, GRAVITY, false);
    push(c, g, gravity)?;
    push(c, g, c.ebp)?;
    c.ecx = c.esi;
    let target = g.u32(c.eax.wrapping_add(8))?;
    g.call(c, target, INTEGRATE_SITES[1])
}

pub(super) fn integrate_island(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Result<TransferAbi> {
    match pc {
        0x316a20 => {
            for word in [c.ecx, c.ebx, c.ebp, c.esi, c.edi] { push(c, g, word)?; }
            c.edi = c.ecx;
            c.esi = g.u32(c.edi.wrapping_add(0x4c))?;
            c.eax = g.u32(c.edi.wrapping_add(0x50))?;
            c.ecx = c.esi;
            c.ebx = c.esi.wrapping_add(c.eax.wrapping_mul(4));
            arithmetic(c, c.esi, c.ebx, true);
            if c.esi != c.ebx {
                let input = g.u32(c.esp.wrapping_add(24))?;
                c.ebp = arithmetic(c, input, 16, false);
            }
        }
        0x316a48 => {
            c.esi = c.esi.wrapping_add(4);
            arithmetic(c, c.esi, c.ebx, true);
        }
        0x316aac => {
            c.edx = g.u32(c.esi)?;
            push(c, g, c.ebp)?;
            c.ecx = c.esi;
            let target = g.u32(c.edx.wrapping_add(4))?;
            return g.call(c, target, INTEGRATE_SITES[2]);
        }
        0x316ab4 => {
            c.ebx = decrement(c, c.ebx);
            if c.ebx as i32 >= 0 { return apply_motion(c, g); }
            c.ecx = g.u32(MONITOR_HEAD)?;
            arithmetic(c, c.ecx, g.u32(MONITOR_END)?, true);
            return finish_motion_pass(c, g);
        }
        0x316b16 => {
            advance_stack(c, 36);
            return finish_motion_pass(c, g);
        }
        _ => unreachable!(),
    }
    // The actions' end pointer is captured once, but each action is read
    // when its turn arrives. Only afterwards choose the constraint path.
    if c.esi != c.ebx {
        c.ecx = g.u32(c.esi)?;
        c.edx = g.u32(c.ecx)?;
        push(c, g, c.ebp)?;
        let target = g.u32(c.edx.wrapping_add(0x14))?;
        return g.call(c, target, INTEGRATE_SITES[0]);
    }
    c.eax = g.u32(c.edi.wrapping_add(0x5c))?;
    logic(c, c.eax, 1 << 31);
    if c.eax != 0 {
        return solve_island(c, g);
    }
    c.ecx = g.u32(MONITOR_HEAD)?;
    arithmetic(c, c.ecx, g.u32(MONITOR_END)?, true);
    let count = g.u32(c.edi.wrapping_add(0x40))?;
    c.ebx = decrement(c, count);
    if (c.ebx as i32) < 0 {
        arithmetic(c, c.ecx, g.u32(MONITOR_END)?, true);
        return finish_motion_pass(c, g);
    }
    let input = g.u32(c.esp.wrapping_add(24))?;
    c.ebp = arithmetic(c, input, 16, false);
    apply_motion(c, g)
}

fn solve_island(c: &mut CpuAbi, g: &mut Guest) -> Result<TransferAbi> {
    c.ecx = g.u32(c.edi.wrapping_add(0x58))?;
    c.edx = g.u32(c.edi.wrapping_add(0x40))?;
    push(c, g, c.eax)?;
    c.eax = g.u32(c.edi.wrapping_add(0x3c))?;
    push(c, g, c.ecx)?;
    c.ecx = g.u32(c.edi.wrapping_add(WORLD))?;
    push(c, g, c.edx)?;
    push(c, g, c.eax)?;
    let input = g.u32(c.esp.wrapping_add(40))?;
    push(c, g, c.edi)?;
    c.ecx = arithmetic(c, c.ecx, GRAVITY, false);
    push(c, g, c.ecx)?;
    c.edx = input.wrapping_add(0x50);
    push(c, g, c.edx)?;
    c.ecx = input.wrapping_add(0x20);
    push(c, g, c.ecx)?;
    c.eax = arithmetic(c, input, 16, false);
    call_arg(c, g, c.eax, 0x31c100, INTEGRATE_SITES[3])
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::guest::Permissions;

    const STACK: u32 = 0x0020_0800;
    const ISLAND: u32 = 0x0021_0000;
    const WORLD_AT: u32 = 0x0022_0000;
    const INPUT: u32 = 0x0023_0000;
    const STEP: u32 = INPUT + 0x10;

    fn guest() -> Guest {
        let mut g = Guest::new();
        for at in [0x0020_0000, ISLAND & !0xfff, WORLD_AT & !0xfff, INPUT & !0xfff, 0x0048_0000] {
            g.map_zeroed(at, 0x2000, Permissions::RW).unwrap();
        }
        // The monitor stream is off: the head sits at the end.
        g.set_u32(MONITOR_HEAD, MONITOR_END).unwrap();
        g.set_u32(MONITOR_END, MONITOR_END).unwrap();
        g
    }

    fn cpu(g: &mut Guest, esp: u32, ret: u32) -> CpuAbi {
        let c = CpuAbi {
            ecx: ISLAND,
            ebx: 0x1111_2222,
            ebp: 0x3333_4444,
            esi: 0x5555_6666,
            edi: 0x7777_8888,
            esp,
            ..CpuAbi::default()
        };
        g.set_u32(esp, ret).unwrap();
        c
    }

    fn ret<const N: u32>(c: &mut CpuAbi, g: &mut Guest, pc: u32) {
        // Serve one guest call: pop the resume PC, then `ret N` arguments.
        let _ = g.u32(c.esp).unwrap();
        c.esp += 4 + N;
        c.eip = pc;
    }

    #[test]
    fn the_step_wrapper_calls_both_phases_and_returns_to_the_caller() {
        let mut g = guest();
        let mut c = cpu(&mut g, STACK, 0x1234_5678);
        g.set_u32(STACK + 4, STEP).unwrap();
        assert_eq!(
            step(&mut c, &mut g, 0x311100).unwrap(),
            TransferAbi::call(0x30fcb0, WORLD_SITES[0])
        );
        assert_eq!(g.u32(c.esp).unwrap(), WORLD_SITES[0]);
        assert_eq!(g.u32(c.esp + 4).unwrap(), STEP);
        ret::<4>(&mut c, &mut g, WORLD_SITES[0]);
        assert_eq!(
            step(&mut c, &mut g, WORLD_SITES[0]).unwrap(),
            TransferAbi::call(0x30fef0, WORLD_SITES[1])
        );
        assert_eq!(g.u32(c.esp).unwrap(), WORLD_SITES[1]);
        assert_eq!(g.u32(c.esp + 4).unwrap(), STEP);
        ret::<4>(&mut c, &mut g, WORLD_SITES[1]);
        assert_eq!(step(&mut c, &mut g, WORLD_SITES[1]).unwrap(), TransferAbi::ret(0x1234_5678));
        assert_eq!(c.esp, STACK + 8);
        assert_eq!((c.esi, c.edi), (0x5555_6666, 0x7777_8888));
    }

    #[test]
    fn collide_island_brackets_the_guest_updates_with_the_halo_hooks() {
        let mut g = guest();
        g.set_u32(ISLAND + WORLD, WORLD_AT).unwrap();
        let broadphase = 0x0024_0000;
        let mut c = cpu(&mut g, STACK, 0x1234_5678);
        g.set_u32(STACK + 4, broadphase).unwrap();
        g.set_u32(STACK + 8, INPUT).unwrap();
        assert_eq!(
            collide_island(&mut c, &mut g, 0x317d30).unwrap(),
            TransferAbi::call(0x278e60, ISLAND_SITES[0])
        );
        assert_eq!(g.u32(c.esp + 4).unwrap(), ISLAND);
        ret::<0>(&mut c, &mut g, ISLAND_SITES[0]);
        // The island broadphase update takes (broadphase, input).
        assert_eq!(
            collide_island(&mut c, &mut g, ISLAND_SITES[0]).unwrap(),
            TransferAbi::call(0x3175e0, ISLAND_SITES[1])
        );
        assert_eq!((g.u32(c.esp + 4).unwrap(), g.u32(c.esp + 8).unwrap()), (broadphase, INPUT));
        ret::<8>(&mut c, &mut g, ISLAND_SITES[1]);
        // Adaptive processing is off: the ordinary pair pass runs.
        g.set_u8(WORLD_AT + 0x23e, 0).unwrap();
        assert_eq!(
            collide_island(&mut c, &mut g, ISLAND_SITES[1]).unwrap(),
            TransferAbi::call(0x317c00, ISLAND_SITES[4])
        );
        assert_eq!(g.u32(c.esp + 4).unwrap(), INPUT);
        ret::<4>(&mut c, &mut g, ISLAND_SITES[4]);
        assert_eq!(
            collide_island(&mut c, &mut g, ISLAND_SITES[4]).unwrap(),
            TransferAbi::call(0x278e80, ISLAND_SITES[5])
        );
        assert_eq!(g.u32(c.esp + 4).unwrap(), ISLAND);
        ret::<0>(&mut c, &mut g, ISLAND_SITES[5]);
        assert_eq!(collide_island(&mut c, &mut g, ISLAND_SITES[5]).unwrap(), TransferAbi::ret(0x1234_5678));
        assert_eq!(c.esp, STACK + 12);
        // The adaptive pass takes the other branch and its own sites.
        let mut c = cpu(&mut g, STACK, 0x1234_5678);
        g.set_u32(STACK + 4, broadphase).unwrap();
        g.set_u32(STACK + 8, INPUT).unwrap();
        let _ = collide_island(&mut c, &mut g, 0x317d30).unwrap();
        ret::<0>(&mut c, &mut g, ISLAND_SITES[0]);
        let _ = collide_island(&mut c, &mut g, ISLAND_SITES[0]).unwrap();
        ret::<8>(&mut c, &mut g, ISLAND_SITES[1]);
        g.set_u8(WORLD_AT + 0x23e, 1).unwrap();
        assert_eq!(
            collide_island(&mut c, &mut g, ISLAND_SITES[1]).unwrap(),
            TransferAbi::call(0x317950, ISLAND_SITES[2])
        );
    }

    #[test]
    fn integrate_island_runs_each_action_then_each_motion_in_reverse() {
        let mut g = guest();
        let (action, action_table, entities, entity, motion, motion_table) =
            (0x0024_0000, 0x0025_0000, 0x0026_0000, 0x0027_0000, 0x0028_0000, 0x0029_0000);
        for at in [action, action_table, entities, entity, motion, motion_table, 0x002a_0000, 0x002b_0000] {
            g.map_zeroed(at, 0x1000, Permissions::RW).unwrap();
        }
        g.set_u32(ISLAND + 0x4c, action).unwrap();
        g.set_u32(ISLAND + 0x50, 1).unwrap();
        g.set_u32(action, action_table).unwrap();
        // The action object's first word is its vtable; its +0x14 is the step.
        g.set_u32(action_table, action_table + 0x100).unwrap();
        g.set_u32(action_table + 0x100 + 0x14, 0x002a_0000).unwrap();
        g.set_u32(ISLAND + 0x3c, entities).unwrap();
        g.set_u32(ISLAND + 0x40, 1).unwrap();
        g.set_u32(entities, entity).unwrap();
        g.set_u32(entity + 0x3c, motion).unwrap();
        g.set_u32(motion, motion_table).unwrap();
        g.set_u32(motion_table + 8, 0x002a_0000).unwrap();
        g.set_u32(motion_table + 4, 0x002b_0000).unwrap();
        g.set_u32(ISLAND + WORLD, WORLD_AT).unwrap();
        let mut c = cpu(&mut g, STACK, 0x1234_5678);
        g.set_u32(STACK + 4, INPUT).unwrap();
        // The action's virtual +0x14 receives the step at W+0x150.
        assert_eq!(
            integrate_island(&mut c, &mut g, 0x316a20).unwrap(),
            TransferAbi::call(0x002a_0000, INTEGRATE_SITES[0])
        );
        assert_eq!(g.u32(c.esp + 4).unwrap(), STEP);
        ret::<4>(&mut c, &mut g, INTEGRATE_SITES[0]);
        // No constraints: each entity's motion virtuals, pose then forces.
        assert_eq!(
            integrate_island(&mut c, &mut g, INTEGRATE_SITES[0]).unwrap(),
            TransferAbi::call(0x002a_0000, INTEGRATE_SITES[1])
        );
        assert_eq!(g.u32(c.esp + 4).unwrap(), STEP);
        assert_eq!(g.u32(c.esp + 8).unwrap(), WORLD_AT + GRAVITY);
        ret::<4>(&mut c, &mut g, INTEGRATE_SITES[1]);
        assert_eq!(
            integrate_island(&mut c, &mut g, INTEGRATE_SITES[1]).unwrap(),
            TransferAbi::call(0x002b_0000, INTEGRATE_SITES[2])
        );
        assert_eq!(g.u32(c.esp + 4).unwrap(), STEP);
        assert_eq!(g.u32(c.esp + 8).unwrap(), WORLD_AT + GRAVITY);
        ret::<8>(&mut c, &mut g, INTEGRATE_SITES[2]);
        // The last entity consumed: the pass returns to the caller.
        assert_eq!(
            integrate_island(&mut c, &mut g, INTEGRATE_SITES[2]).unwrap(),
            TransferAbi::ret(0x1234_5678)
        );
        assert_eq!(c.esp, STACK + 8);
        assert_eq!(
            (c.ecx, c.ebx, c.ebp, c.esi, c.edi),
            (ISLAND, 0x1111_2222, 0x3333_4444, 0x5555_6666, 0x7777_8888)
        );
    }

    #[test]
    fn a_constrained_island_passes_nine_solver_arguments_in_guest_order() {
        let mut g = guest();
        g.set_u32(ISLAND + 0x50, 0).unwrap();
        g.set_u32(ISLAND + 0x5c, 3).unwrap();
        g.set_u32(ISLAND + 0x58, 0x0024_0000).unwrap();
        g.set_u32(ISLAND + 0x3c, 0x0025_0000).unwrap();
        g.set_u32(ISLAND + 0x40, 7).unwrap();
        g.set_u32(ISLAND + WORLD, WORLD_AT).unwrap();
        g.map_zeroed(0x0024_0000, 0x2000, Permissions::RW).unwrap();
        let mut c = cpu(&mut g, STACK, 0x1234_5678);
        g.set_u32(STACK + 4, INPUT).unwrap();
        assert_eq!(
            integrate_island(&mut c, &mut g, 0x316a20).unwrap(),
            TransferAbi::call(0x31c100, INTEGRATE_SITES[3])
        );
        for (at, word) in [
            (c.esp + 4, STEP),
            (c.esp + 8, INPUT + 0x20),
            (c.esp + 12, INPUT + 0x50),
            (c.esp + 16, WORLD_AT + GRAVITY),
            (c.esp + 20, ISLAND),
            (c.esp + 24, 0x0025_0000),
            (c.esp + 28, 7),
            (c.esp + 32, 0x0024_0000),
            (c.esp + 36, 3),
        ] {
            assert_eq!(g.u32(at).unwrap(), word, "{at:#x}");
        }
        // The solve is cdecl: it pops only its return; the caller drops
        // the 0x24 argument bytes at the resume.
        ret::<0>(&mut c, &mut g, INTEGRATE_SITES[3]);
        assert_eq!(
            integrate_island(&mut c, &mut g, INTEGRATE_SITES[3]).unwrap(),
            TransferAbi::ret(0x1234_5678)
        );
        assert_eq!(c.esp, STACK + 8);
    }

    #[test]
    fn the_step_declines_while_the_monitor_stream_is_on() {
        let mut g = Guest::new();
        g.map_zeroed(0x0048_0000, 0x1000, Permissions::RW).unwrap();
        assert!(monitor_off(&g));
        g.set_u32(MONITOR_END, 8).unwrap();
        g.set_u32(MONITOR_HEAD, 0).unwrap();
        assert!(!monitor_off(&g));
        g.set_u32(MONITOR_HEAD, 8).unwrap();
        assert!(monitor_off(&g));
        let unmapped = Guest::new();
        assert!(!monitor_off(&unmapped));
    }

    #[test]
    fn the_collide_phase_walks_the_islands_and_world_passes_in_retail_order() {
        let mut g = guest();
        // finish_fpu reads the last x87 instruction, `fstp [esi+1a4]`.
        g.map_zeroed(0x0030_f000, 0x1000, Permissions::RW).unwrap();
        g.set_u8(0x30fd70, 0xd9).unwrap();
        g.set_u8(0x30fd71, 0x9e).unwrap();
        let islands = INPUT + 0x100;
        let source = INPUT + 0x200;
        let w = |off: u32| WORLD_AT + off;
        for (at, v) in [
            (STEP, 0.25f32.to_bits()), (STEP + 4, 4.0f32.to_bits()),
            (w(W_SCALE), 0.75f32.to_bits()), (w(W_ITERATIONS), 3),
            (w(W_INPUT_SOURCE), source), (w(W_ISLANDS), islands), (w(W_ISLAND_COUNT), 2),
            (w(W_BROADPHASE), 0xb0b0), (w(W_PENDING), 1), (w(W_ISLAND_LISTENERS), 1),
            (w(W_MERGES), 1), (w(W_QUEUE), 0), (islands, 0x11), (islands + 4, 0x22),
        ] {
            g.set_u32(at, v).unwrap();
        }
        for i in 0..4 {
            g.set_u32(source + 4 * i, 0xa0 + i).unwrap();
        }
        let mut c = cpu(&mut g, STACK, 0x1234_5678);
        c.ecx = WORLD_AT;
        g.set_u32(STACK + 4, STEP).unwrap();
        let frame = STACK - 20;
        // Entry: the terms, then the first pending pass (thiscall, no args).
        assert_eq!(collide(&mut c, &mut g, 0x30fcb0).unwrap(), TransferAbi::call(PENDING, COLLIDE_SITES[0]));
        assert_eq!((c.ecx, c.esp), (WORLD_AT, frame - 4));
        assert_eq!(g.u8(w(W_LOCK)).unwrap(), 1);
        assert_eq!(g.u32(w(W_STEP)).unwrap(), 0.25f32.to_bits());
        assert_eq!(g.u32(w(W_INPUT + 12)).unwrap(), 0xa3);
        let dt = 0.25f32 * 0.75;
        let rate = 3.0f32 * 4.0;
        for (off, v) in [
            (W_SCALED_DT, dt), (W_SCALED_RATE, rate), (W_SCALED_DT_COPY, dt),
            (W_SCALED_RATE_COPY, rate), (W_ITERATION_DT, 3.0 * dt), (W_RATE_TERM, 0.75 * rate),
        ] {
            assert_eq!(g.u32(w(off)).unwrap(), v.to_bits(), "{off:#x}");
        }
        assert_eq!(c.x87.data_pointer, w(W_RATE_TERM));
        ret::<0>(&mut c, &mut g, COLLIDE_SITES[0]);
        // Island 0: its collide `(broadphase, input)`, then its listeners.
        for island in [0x11, 0x22] {
            let pc = if island == 0x11 { COLLIDE_SITES[0] } else { COLLIDE_SITES[2] };
            assert_eq!(collide(&mut c, &mut g, pc).unwrap(), TransferAbi::call(ISLAND_COLLIDE, COLLIDE_SITES[1]));
            assert_eq!(c.ecx, island);
            assert_eq!((g.u32(c.esp + 4).unwrap(), g.u32(c.esp + 8).unwrap()), (0xb0b0, w(W_INPUT)));
            assert_eq!(g.u8(w(W_COLLIDING)).unwrap(), 1);
            ret::<8>(&mut c, &mut g, COLLIDE_SITES[1]);
            assert_eq!(collide(&mut c, &mut g, COLLIDE_SITES[1]).unwrap(), TransferAbi::call(ISLAND_LISTENERS, COLLIDE_SITES[2]));
            assert_eq!(g.u8(w(W_COLLIDING)).unwrap(), 0);
            assert_eq!(
                [4, 8, 12].map(|o| g.u32(c.esp + o).unwrap()),
                [WORLD_AT, island, STEP]
            );
            // cdecl: the phase pops the three arguments itself.
            ret::<0>(&mut c, &mut g, COLLIDE_SITES[2]);
        }
        // A listener left nothing pending: the second pass is skipped.
        g.set_u32(w(W_PENDING), 0).unwrap();
        assert_eq!(collide(&mut c, &mut g, COLLIDE_SITES[2]).unwrap(), TransferAbi::call(MERGES, COLLIDE_SITES[4]));
        assert_eq!((g.u8(w(W_COLLIDING)).unwrap(), g.u8(w(W_LOCK)).unwrap()), (0, 0));
        assert_eq!((c.ecx, c.edi, c.esp), (WORLD_AT, 2, frame - 4));
        ret::<0>(&mut c, &mut g, COLLIDE_SITES[4]);
        // No queue; a merge queued an operation, so the third pass runs.
        g.set_u32(w(W_PENDING), 1).unwrap();
        assert_eq!(collide(&mut c, &mut g, COLLIDE_SITES[4]).unwrap(), TransferAbi::call(PENDING, COLLIDE_SITES[6]));
        ret::<0>(&mut c, &mut g, COLLIDE_SITES[6]);
        assert_eq!(collide(&mut c, &mut g, COLLIDE_SITES[6]).unwrap(), TransferAbi::call(STEP_LISTENERS, COLLIDE_SITES[7]));
        assert_eq!([4, 8].map(|o| g.u32(c.esp + o).unwrap()), [WORLD_AT, STEP]);
        ret::<0>(&mut c, &mut g, COLLIDE_SITES[7]);
        assert_eq!(collide(&mut c, &mut g, COLLIDE_SITES[7]).unwrap(), TransferAbi::ret(0x1234_5678));
        assert_eq!(c.esp, STACK + 8);
        assert_eq!((c.ebx, c.ebp, c.esi, c.edi), (0x1111_2222, 0x3333_4444, 0x5555_6666, 0x7777_8888));
        // `pop ecx` takes back the slot the entry pushed: the world.
        assert_eq!((c.ecx, c.eax), (WORLD_AT, MONITOR_END));
    }
}
