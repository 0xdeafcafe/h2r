//! The player's pickup scan `1508e0` (`PLAYER_PICKUP_SCAN`, `ret 4`, the
//! argument a player index), a seat body over the retail frame
//! (docs/engine/sim-rewrite.md, "Seats and rules (phase 7)").
//! `players_after` calls it for each player once the BSP switch volumes
//! are done.
//!
//! | Step | Guest | Body |
//! |---|---|---|
//! | the player has no unit (`+0x2c` is `-1`) | `add esp, 40h; ret 4` | the same |
//! | the unit's object has a parent (`+0x14`) | out | the same |
//! | the object query | `call bb050` (all lists, types `0x89c`, the unit's location and centre, radius `[unit + 0x3c] + [0x44ae90]`, the frame's 16-handle list) | the call, from [`QUERIED`]: `bb050` is the object walk's Rust body |
//! | each handle, type `2` or `3` | `call 150e30 (player, handle)` | the call, from [`CANDIDATE_DONE`]: the Halo 1 prompt feature's `before` hook sees every one |
//! | each handle, type `7` or `11` | the link walk from `+0x10` along `+0xc`, appended while the list holds fewer than 16 | the same, into the frame's list |
//!
//! At every call and the return the registers, flags and frame are
//! retail's, so the callees see the frame the original builds. The body
//! declines only on the float environment ([`crate::sim_native::plan`]);
//! the seat installer panics on it by name.

use crate::sim_native::{Memory, plan::segment, sub_flags};
use crate::sim_objects::sse::{self, Ss};
use crate::sim_physics::{arithmetic, logic, pop, push};
use runtime::{CpuAbi, Guest, GuestFault, TransferAbi};

pub const ENTRY: u32 = 0x0015_08e0;
const END: u32 = 0x0015_09e0;
/// After the object query `bb050`.
pub const QUERIED: u32 = 0x0015_0959;
/// After the pickup candidate `150e30`.
pub const CANDIDATE_DONE: u32 = 0x0015_0994;
pub const SITES: &[u32] = &[QUERIED, CANDIDATE_DONE];

const QUERY: u32 = 0x000b_b050;
const CANDIDATE: u32 = 0x0015_0e30;
const PLAYERS: u32 = 0x004e_8c24;
const OBJECTS: u32 = 0x004e_0300;
/// The float the scan adds to the unit's radius.
const REACH: u32 = 0x0044_ae90;
const STRIDE: u32 = 0x21c;
/// The object types the scan asks the query for (2, 3, 4, 7 and 11).
const TYPES: u32 = 0x89c;
/// The frame's list: sixteen handles.
const LIST: u32 = 16;
const FRAME: u32 = 0x40;
const NONE: u32 = u32::MAX;

type Result<T> = std::result::Result<T, GuestFault>;

#[inline]
fn at(p: u32, n: u32) -> u32 {
    p.wrapping_add(n)
}

/// `[[OBJECTS + 44] + (handle & ffff) * 12 + 8]`, with `ebx` the pool.
fn object(g: &impl Memory, pool: u32, handle: u32) -> Result<u32> {
    let table = g.u32(at(pool, 0x44))?;
    g.u32(at(table, (handle & 0xffff).wrapping_mul(12).wrapping_add(8)))
}

/// `add esp, 40h; ret 4` at `1509da`.
fn leave(c: &mut CpuAbi, g: &impl Memory) -> Result<TransferAbi> {
    c.esp = arithmetic(c, c.esp, FRAME, false);
    g.ret(c, 4)
}

/// `pop ebx` at `1509d9`, then [`leave`].
fn leave_ebx(c: &mut CpuAbi, g: &impl Memory) -> Result<TransferAbi> {
    c.ebx = pop(c, g)?;
    leave(c, g)
}

/// `pop esi; pop ebp` at `1509d7`, then [`leave_ebx`].
fn leave_list(c: &mut CpuAbi, g: &impl Memory) -> Result<TransferAbi> {
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    leave_ebx(c, g)
}

/// `[esp + i * 4 + 10h]` with `edi` pushed: the frame's list.
fn slot(c: &CpuAbi, i: u32) -> u32 {
    at(c.esp, i.wrapping_mul(4).wrapping_add(0x10))
}

fn entry(c: &mut CpuAbi, g: &mut impl Memory) -> Result<TransferAbi> {
    c.eax = g.u32(at(c.esp, 4))?;
    c.ecx = g.u32(PLAYERS)?;
    c.edx = g.u32(at(c.ecx, 0x44))?;
    c.eax &= 0xffff;
    c.eax = at(c.eax.wrapping_mul(STRIDE), c.edx);
    c.eax = g.u32(at(c.eax, 0x2c))?;
    c.esp = arithmetic(c, c.esp, FRAME, true);
    arithmetic(c, c.eax, NONE, true);
    if c.eax == NONE {
        return leave(c, g);
    }
    c.eax &= 0xffff;
    push(c, g, c.ebx)?;
    c.ebx = g.u32(OBJECTS)?;
    c.edx = c.eax.wrapping_mul(3);
    c.eax = g.u32(at(c.ebx, 0x44))?;
    c.eax = g.u32(at(c.eax, c.edx.wrapping_mul(4).wrapping_add(8)))?;
    let parent = g.u32(at(c.eax, 0x14))?;
    arithmetic(c, parent, NONE, true);
    if parent != NONE {
        return leave_ebx(c, g);
    }
    sse::load(c, 0, g.u32(at(c.eax, 0x3c))?);
    let radius = sse::ss(c, Ss::Add, 0, g.u32(REACH)?);
    push(c, g, c.ebp)?;
    push(c, g, c.esi)?;
    push(c, g, LIST)?;
    c.ecx = at(c.esp, 0x10);
    push(c, g, c.ecx)?;
    // `push ecx; movss [esp], xmm0`.
    push(c, g, radius)?;
    c.edx = at(c.eax, 0x30);
    push(c, g, c.edx)?;
    c.eax = arithmetic(c, c.eax, 0x28, false);
    push(c, g, c.eax)?;
    push(c, g, TYPES)?;
    push(c, g, 0)?;
    g.call(c, QUERY, QUERIED)
}

/// From `150963` (`ebp` the handle's index): the handle's type test, the
/// candidate call, the link walk, the loop; until a call or the return.
fn walk(c: &mut CpuAbi, g: &mut impl Memory) -> Result<TransferAbi> {
    loop {
        c.ecx = g.u32(slot(c, c.ebp))?;
        c.edx = g.u32(at(c.ebx, 0x44))?;
        c.eax = c.ecx & 0xffff;
        c.eax = c.eax.wrapping_mul(3);
        c.edi = g.u32(at(c.edx, c.eax.wrapping_mul(4).wrapping_add(8)))?;
        c.eax = g.u8(at(c.edi, 0xaa))? as i8 as i32 as u32;
        arithmetic(c, c.eax, 2, true);
        if c.eax as i32 >= 2 {
            arithmetic(c, c.eax, 3, true);
            if c.eax as i32 <= 3 {
                c.eax = g.u32(at(c.esp, 0x54))?;
                push(c, g, c.ecx)?;
                push(c, g, c.eax)?;
                return g.call(c, CANDIDATE, CANDIDATE_DONE);
            }
        }
        links(c, g)?;
        if let Some(t) = next(c, g)? {
            return Ok(t);
        }
    }
}

/// `15099a` to `1509d0`: a machine's or crate's children, appended to the
/// list while it holds fewer than 16.
fn links(c: &mut CpuAbi, g: &mut impl Memory) -> Result<()> {
    let kind = g.u8(at(c.edi, 0xaa))?;
    c.eax = (c.eax & !0xff) | u32::from(kind);
    sub_flags(c, u32::from(kind), 0xb, 8);
    if kind != 0xb {
        sub_flags(c, u32::from(kind), 7, 8);
        if kind != 7 {
            return Ok(());
        }
    }
    c.eax = g.u32(at(c.edi, 0x10))?;
    arithmetic(c, c.eax, NONE, true);
    if c.eax == NONE {
        return Ok(());
    }
    loop {
        arithmetic(c, c.esi, LIST, true);
        if c.esi >= LIST {
            return Ok(());
        }
        c.edx = g.u32(at(c.ebx, 0x44))?;
        g.set_u32(slot(c, c.esi), c.eax)?;
        c.eax &= 0xffff;
        c.ecx = c.eax.wrapping_mul(3);
        c.eax = g.u32(at(c.edx, c.ecx.wrapping_mul(4).wrapping_add(8)))?;
        c.eax = g.u32(at(c.eax, 0xc))?;
        c.esi = c.esi.wrapping_add(1);
        arithmetic(c, c.eax, NONE, true);
        if c.eax == NONE {
            return Ok(());
        }
    }
}

/// `1509d1`: `inc ebp; cmp ebp, esi; jl`, else `pop edi` and out.
fn next(c: &mut CpuAbi, g: &impl Memory) -> Result<Option<TransferAbi>> {
    c.ebp = c.ebp.wrapping_add(1);
    arithmetic(c, c.ebp, c.esi, true);
    if (c.ebp as i32) < c.esi as i32 {
        return Ok(None);
    }
    c.edi = pop(c, g)?;
    leave_list(c, g).map(Some)
}

fn select(c: &mut CpuAbi, g: &mut impl Memory, pc: u32) -> Result<TransferAbi> {
    match pc {
        ENTRY => entry(c, g),
        QUERIED => {
            // `ret 1c` popped the query's arguments.
            c.esi = c.eax as u16 as i16 as i32 as u32;
            c.ebp = 0;
            logic(c, c.esi, 1 << 31);
            if c.esi as i32 <= 0 {
                return leave_list(c, g);
            }
            push(c, g, c.edi)?;
            walk(c, g)
        }
        CANDIDATE_DONE => {
            c.ebx = g.u32(OBJECTS)?;
            links(c, g)?;
            match next(c, g)? {
                Some(t) => Ok(t),
                None => walk(c, g),
            }
        }
        _ => unreachable!("undeclared pickup scan continuation {pc:#x}"),
    }
}

/// One scan segment.
pub fn run(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> std::result::Result<Option<TransferAbi>, GuestFault> {
    segment::<64, _>(c, g, pc, ENTRY..END, SITES, |c, p, pc| select(c, p, pc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::abi;
    use runtime::guest::Permissions;

    const HEADER: u32 = 0x0060_0000;
    const POOL: u32 = 0x0060_1000;
    const BODIES: u32 = 0x0060_2000;
    const STACK: u32 = 0x007f_0000;
    const RETURN: u32 = 0x0012_3456;
    const SAVED: [u32; 4] = [0xb0b0_0001, 0xb0b0_0002, 0xb0b0_0003, 0xb0b0_0004];

    fn body(i: u32) -> u32 {
        BODIES + i * 0x100
    }

    /// Player 1's unit is object 0; objects 1 to 4 are a machine with two
    /// children (3 and 4) and a weapon (2).
    fn world() -> (CpuAbi, Guest) {
        let mut g = Guest::new();
        g.map_zeroed(0x0010_0000, 0x0010_0000, Permissions::RW).unwrap();
        g.map_zeroed(0x0040_0000, 0x0010_0000, Permissions::RW).unwrap();
        g.map_zeroed(HEADER, 0x0001_0000, Permissions::RW).unwrap();
        g.map_zeroed(STACK - 0x1000, 0x2000, Permissions::RW).unwrap();
        g.set_u32(PLAYERS, HEADER).unwrap();
        g.set_u32(HEADER + 0x44, HEADER + 0x100).unwrap();
        g.set_u32(HEADER + 0x100 + STRIDE + 0x2c, 0xe000_0000).unwrap();
        g.set_u32(OBJECTS, POOL).unwrap();
        g.set_u32(POOL + 0x44, POOL + 0x100).unwrap();
        for i in 0..5 {
            g.set_u32(POOL + 0x100 + i * 12 + 8, body(i)).unwrap();
            g.set_u32(body(i) + 0x14, NONE).unwrap();
            g.set_u32(body(i) + 0x10, NONE).unwrap();
            g.set_u32(body(i) + 0xc, NONE).unwrap();
        }
        g.set_u32(body(0) + 0x3c, 1.0f32.to_bits()).unwrap();
        g.set_u32(REACH, 0.5f32.to_bits()).unwrap();
        g.set_u8(body(1) + 0xaa, 7).unwrap();
        g.set_u32(body(1) + 0x10, 0xe000_0003).unwrap();
        g.set_u32(body(3) + 0xc, 0xe000_0004).unwrap();
        g.set_u8(body(2) + 0xaa, 2).unwrap();
        g.set_u32(STACK, RETURN).unwrap();
        g.set_u32(STACK + 4, 1).unwrap();
        let mut cpu = CpuAbi {
            esp: STACK,
            ebx: SAVED[0],
            ebp: SAVED[1],
            esi: SAVED[2],
            edi: SAVED[3],
            mxcsr: 0x1f80,
            ..Default::default()
        };
        cpu.x87.control = 0x027f;
        (cpu, g)
    }

    /// The callee's `ret pop`, with `eax` its answer.
    fn ret(cpu: &mut CpuAbi, pop: u32, eax: u32) {
        cpu.esp += 4 + pop;
        cpu.eax = eax;
        cpu.eflags_defined = 0;
    }

    #[test]
    fn the_scan_queries_tests_a_candidate_walks_links_and_returns() {
        let (mut cpu, mut g) = world();
        let t = run(&mut cpu, &mut g, ENTRY).unwrap().unwrap();
        assert_eq!((t.kind, t.target_pc, t.resume_pc), (abi::CALL, QUERY, QUERIED));
        let sp = cpu.esp;
        assert_eq!(sp, STACK - 0x6c);
        let args: Vec<u32> = (0..8).map(|k| g.u32(sp + 4 * k).unwrap()).collect();
        assert_eq!(args, [QUERIED, 0, TYPES, body(0) + 0x28, body(0) + 0x30, 1.5f32.to_bits(), STACK - 0x40, LIST]);

        // The query found the machine and the weapon.
        g.set_u32(STACK - 0x40, 0xe000_0001).unwrap();
        g.set_u32(STACK - 0x3c, 0xe000_0002).unwrap();
        ret(&mut cpu, 0x1c, 0xffff_0002);
        let t = run(&mut cpu, &mut g, QUERIED).unwrap().unwrap();
        assert_eq!((t.kind, t.target_pc, t.resume_pc), (abi::CALL, CANDIDATE, CANDIDATE_DONE));
        let sp = cpu.esp;
        assert_eq!([g.u32(sp + 4).unwrap(), g.u32(sp + 8).unwrap()], [1, 0xe000_0002]);
        // The machine's children were appended.
        assert_eq!([g.u32(STACK - 0x38).unwrap(), g.u32(STACK - 0x34).unwrap()], [0xe000_0003, 0xe000_0004]);
        assert_eq!(cpu.esi, 4);

        ret(&mut cpu, 8, 0);
        let t = run(&mut cpu, &mut g, CANDIDATE_DONE).unwrap().unwrap();
        assert_eq!((t.kind, t.target_pc), (abi::RETURN, RETURN));
        assert_eq!(cpu.esp, STACK + 8);
        assert_eq!([cpu.ebx, cpu.ebp, cpu.esi, cpu.edi], SAVED);
    }

    #[test]
    fn a_player_without_a_unit_or_a_unit_with_a_parent_returns_at_once() {
        let (mut cpu, mut g) = world();
        g.set_u32(HEADER + 0x100 + STRIDE + 0x2c, NONE).unwrap();
        let t = run(&mut cpu, &mut g, ENTRY).unwrap().unwrap();
        assert_eq!((t.kind, t.target_pc, cpu.esp, cpu.ebx), (abi::RETURN, RETURN, STACK + 8, SAVED[0]));

        let (mut cpu, mut g) = world();
        g.set_u32(body(0) + 0x14, 0xe000_0001).unwrap();
        let t = run(&mut cpu, &mut g, ENTRY).unwrap().unwrap();
        assert_eq!((t.kind, t.target_pc, cpu.esp, cpu.ebx), (abi::RETURN, RETURN, STACK + 8, SAVED[0]));
    }

    #[test]
    fn an_empty_query_returns_and_the_float_environment_declines() {
        let (mut cpu, mut g) = world();
        run(&mut cpu, &mut g, ENTRY).unwrap().unwrap();
        ret(&mut cpu, 0x1c, 0);
        let t = run(&mut cpu, &mut g, QUERIED).unwrap().unwrap();
        assert_eq!((t.kind, t.target_pc, cpu.esp), (abi::RETURN, RETURN, STACK + 8));
        assert_eq!([cpu.ebx, cpu.ebp, cpu.esi], [SAVED[0], SAVED[1], SAVED[2]]);

        let (mut cpu, mut g) = world();
        cpu.mxcsr = 0x1f00;
        assert!(run(&mut cpu, &mut g, ENTRY).unwrap().is_none());
    }
}
