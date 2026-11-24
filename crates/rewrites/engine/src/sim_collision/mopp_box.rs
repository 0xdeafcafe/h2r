//! Entry F, `0x00300150`: the integer MOPP box query and the interpreter
//! `0x002ff8b0` below it, as one kernel call (docs/engine/collision-queries.md,
//! "Entry F").
//!
//! `ecx` is the box VM, the stack holds `code`, `aabb` and `array`, and the
//! entry returns with `ret 0xc`. The adapter writes what the original
//! leaves: the VM's array and code pointers and its full-precision corners,
//! the keys at `array.data[size..]` and the new `array.size`, callee-saved
//! registers as they were, and EFLAGS from the interpreter's last
//! epilogue. `eax`, `ecx`, `edx`, the x87 condition codes and the dead
//! stack below the entry are the declared clobbers. It declines before any
//! write when the answer would need the array to grow, when an output
//! overlaps an input or the dead stack, or off the checked float path.
use super::layout::mopp_box as l;
use super::preflight::{CollisionMemory, Outputs};
use super::{Read, finish};
use crate::sim_native::{Memory, add_flags};
use runtime::{CpuAbi, GuestFault, TransferAbi};
use sim_collision::mopp::{Corners, Found};
use sim_collision::{Bail, CollisionWorld, Entry, GuestRead};
use sim_core::det;

/// What the entry writes, decided before anything is written.
struct Plan {
    target: u32,
    sp: u32,
    vm: u32,
    code: u32,
    array: u32,
    size: u32,
    keys_at: u32,
    corners: Corners,
    keys: Vec<u32>,
}

/// The original's lowest stack byte for `frames` self-calls of the
/// interpreter, from the entry ESP.
fn dead_depth(sp: u32, frames: u32) -> u32 {
    let aligned = sp.wrapping_sub(4) & !15;
    sp.wrapping_sub(aligned.wrapping_sub(l::ROOT_FRAME + frames * l::NESTED_FRAME))
}

fn plan(c: &CpuAbi, g: &mut impl CollisionMemory, world: &CollisionWorld) -> Result<Plan, Bail> {
    // One x87 register per load. The conversions set PE when inexact, and
    // nothing here tracks that, so the flag must already be sticky.
    if !det::x87_env(c.x87.control, c.x87.status, c.x87.tag, 1) || c.x87.status & l::X87_PE == 0 {
        return Err(Bail::Env);
    }
    let sp = c.esp;
    let vm = c.ecx;
    let (code, aabb, array) = {
        let m = Read(&*g, (0, 0));
        let arg = |k: u32| m.u32(sp.wrapping_add(4 * k));
        (arg(1)?, arg(2)?, arg(3)?)
    };
    let revision = g.mopp_revision(code)?;
    let m = Read(&*g, (0, 0));
    let tree = world.mopp(&m, code, revision)?;
    let min = m.vec3(aabb)?;
    let max = m.vec3(aabb.wrapping_add(l::AABB_MAX))?;
    let source = tree.source();
    let corners = Corners::convert(min, max, source.origin, source.scale, c.x87.control)?;
    let data = m.u32(array.wrapping_add(l::DATA))?;
    let size = m.u32(array.wrapping_add(l::SIZE))?;
    let capacity = m.u32(array.wrapping_add(l::CAPACITY))? & l::CAPACITY_MASK;
    let room = capacity.checked_sub(size).ok_or(Bail::Grow)?;
    let Found { keys, frames } = tree.query_box(&corners, room as usize)?;
    let keys_at = data.wrapping_add(size.wrapping_mul(4));
    let bytes = keys.len() as u32 * 4;
    let key_range = if bytes == 0 { (sp, 0) } else { (keys_at, bytes) };
    let outputs = Outputs::new(
        g,
        sp,
        dead_depth(sp, frames),
        [(vm, l::VM_SIZE), (array, l::ARRAY_SIZE), key_range],
    )?;
    // The original reads its arguments after the first VM write, and the
    // AABB and the code between the corner writes.
    let code_span = tree.span();
    outputs.separate(g, sp.wrapping_add(4), l::ARGS)?;
    outputs.separate(g, aabb, l::AABB_SIZE)?;
    outputs.separate(g, code_span.0, code_span.1)?;
    Ok(Plan {
        target: outputs.target,
        sp,
        vm,
        code,
        array,
        size,
        keys_at,
        corners,
        keys,
    })
}

fn commit(c: &mut CpuAbi, g: &mut impl Memory, p: Plan) -> Result<TransferAbi, GuestFault> {
    let vm = |offset: u32| p.vm.wrapping_add(offset);
    g.set_u32(vm(l::ARRAY), p.array)?;
    g.set_u32(vm(l::CODE), p.code)?;
    for a in 0..3u32 {
        g.set_u32(vm(l::LO + 4 * a), p.corners.lo[a as usize] as u32)?;
        g.set_u32(vm(l::HI + 4 * a), p.corners.hi[a as usize] as u32)?;
    }
    for (i, &key) in p.keys.iter().enumerate() {
        g.set_u32(p.keys_at.wrapping_add(4 * i as u32), key)?;
    }
    if !p.keys.is_empty() {
        g.set_u32(
            p.array.wrapping_add(l::SIZE),
            p.size.wrapping_add(p.keys.len() as u32),
        )?;
    }
    // The setup's own epilogue (pop, mov, pop, ret) leaves the flags of
    // the root interpreter's `add esp, 0x38`.
    let aligned = p.sp.wrapping_sub(4) & !15;
    add_flags(c, aligned.wrapping_sub(l::FLAGS_ESP), l::INTERPRETER_LOCALS);
    Ok(finish(c, p.target, l::ARGS))
}

/// Entry F, `0x00300150`, natively.
pub fn mopp_box(
    c: &mut CpuAbi,
    g: &mut impl CollisionMemory,
    world: &CollisionWorld,
) -> Result<Option<TransferAbi>, GuestFault> {
    let plan = plan(c, g, world);
    world.record(Entry::MoppBox, &plan);
    match plan {
        Ok(plan) => commit(c, g, plan).map(Some),
        Err(_) => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dead_window_covers_alignment_and_every_frame() {
        // ESP 0x1000: EBP at 0xffc, aligned to 0xff0, the root
        // interpreter's last byte at 0xff0 - 0xa4.
        assert_eq!(dead_depth(0x1000, 0), 0x1000 - (0xff0 - 0xa4));
        assert_eq!(dead_depth(0x1000, 2), 0x1000 - (0xff0 - 0xa4 - 2 * 0x54));
        // The worst alignment.
        assert_eq!(dead_depth(0x1003, 0), 0x1003 - (0xff0 - 0xa4));
        assert!(dead_depth(0x1003, sim_collision::mopp::MAX_FRAMES) <= l::DEAD_BELOW);
        assert!(dead_depth(0x1000, sim_collision::mopp::MAX_FRAMES) <= l::DEAD_BELOW);
    }

    #[test]
    fn keys_cannot_overwrite_the_callers_return_address_through_an_alias() {
        use runtime::{Guest, guest::Permissions};
        use sim_collision::mopp::{HEADER_BYTES, SIZE};
        use sim_core::Epoch;

        const MEM: u32 = 0x10000;
        const ALIAS: u32 = 0x20000;
        let (code, vm, aabb, array, stack) =
            (MEM, MEM + 0x100, MEM + 0x200, MEM + 0x300, MEM + 0xf00);
        let mut g = Guest::new();
        g.map(MEM, 0x1000, 0, Permissions::RW).unwrap();
        g.map(ALIAS, 0x1000, 0, Permissions::RW).unwrap();
        g.set_u32(code + 12, 1.0f32.to_bits()).unwrap();
        g.set_u32(code + SIZE, HEADER_BYTES + 1).unwrap();
        g.set_u8(code + HEADER_BYTES, 0x31).unwrap(); // a single key
        g.set_u32(array + l::CAPACITY, 1).unwrap();
        g.set_u32(array + l::DATA, ALIAS + stack - MEM).unwrap();
        for (i, value) in [0x12345678, code, aabb, array].into_iter().enumerate() {
            g.set_u32(stack + 4 * i as u32, value).unwrap();
        }
        let mut c = CpuAbi { ecx: vm, esp: stack, ..Default::default() };
        c.x87.control = 0x027f;
        c.x87.status = l::X87_PE;
        c.x87.tag = 0xffff;
        static EPOCH: Epoch = Epoch::new();
        let world = CollisionWorld::new(&EPOCH);
        let before = c;
        let memory = g.physical_ram()[..0x1000].to_vec();
        assert!(mopp_box(&mut c, &mut g, &world).unwrap().is_none());
        assert_eq!(c, before);
        assert_eq!(&g.physical_ram()[..0x1000], &memory[..]);
        assert_eq!(world.stats().entries[Entry::MoppBox as usize].bails[Bail::Alias as usize], 1);
    }
}
