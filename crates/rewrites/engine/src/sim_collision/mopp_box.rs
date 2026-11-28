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
    /// A conversion rounded: the x87 status word's PE after the setup.
    inexact: bool,
    keys: Vec<u32>,
}

/// The original's lowest stack byte for `frames` self-calls of the
/// interpreter, from the entry ESP.
fn dead_depth(sp: u32, frames: u32) -> u32 {
    let aligned = sp.wrapping_sub(4) & !15;
    sp.wrapping_sub(aligned.wrapping_sub(l::ROOT_FRAME + frames * l::NESTED_FRAME))
}

fn plan(c: &CpuAbi, g: &mut impl CollisionMemory, world: &CollisionWorld) -> Result<Plan, Bail> {
    // One x87 register per load. The conversions set PE when inexact; the
    // commit sets it as they would.
    if !det::x87_env(c.x87.control, c.x87.status, c.x87.tag, 1) {
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
    let (corners, inexact) = Corners::convert_inexact(min, max, source.origin, source.scale, c.x87.control)?;
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
        inexact,
        keys,
    })
}

impl Plan {
    fn vm_words(&self) -> [(u32, u32); 8] {
        [
            (l::ARRAY, self.array),
            (l::CODE, self.code),
            (l::LO, self.corners.lo[0] as u32),
            (l::HI, self.corners.hi[0] as u32),
            (l::LO + 4, self.corners.lo[1] as u32),
            (l::HI + 4, self.corners.hi[1] as u32),
            (l::LO + 8, self.corners.lo[2] as u32),
            (l::HI + 8, self.corners.hi[2] as u32),
        ]
    }

    fn final_size(&self) -> u32 {
        self.size + self.keys.len() as u32
    }

    fn finish(&self, c: &mut CpuAbi) -> TransferAbi {
        if self.inexact {
            c.x87.status |= l::X87_PE;
        }
        // The setup's epilogue keeps the flags of the root interpreter's add to ESP.
        let aligned = self.sp.wrapping_sub(4) & !15;
        add_flags(c, aligned.wrapping_sub(l::FLAGS_ESP), l::INTERPRETER_LOCALS);
        finish(c, self.target, l::ARGS)
    }
}

fn commit(c: &mut CpuAbi, g: &mut impl Memory, p: Plan) -> Result<TransferAbi, GuestFault> {
    for (offset, value) in p.vm_words() {
        g.set_u32(p.vm.wrapping_add(offset), value)?;
    }
    for (i, &key) in p.keys.iter().enumerate() {
        g.set_u32(p.keys_at.wrapping_add(4 * i as u32), key)?;
    }
    if !p.keys.is_empty() {
        g.set_u32(p.array.wrapping_add(l::SIZE), p.final_size())?;
    }
    Ok(p.finish(c))
}

/// Results held across the lifted query. The complete VM and array headers
/// include the gaps between write sites, so an unexpected store there differs.
pub struct Prediction {
    plan: Plan,
    vm: [u8; l::VM_SIZE as usize],
    array: [u8; l::ARRAY_SIZE as usize],
    cpu: CpuAbi,
}

/// Compute the physics parity arm's answer and let the guest execute the query.
pub fn capture(c: &CpuAbi, g: &mut runtime::Guest) -> Option<Prediction> {
    if g.has_trap() {
        return None;
    }
    let p = plan(c, g, &super::WORLD).ok()?;
    let mut vm = [0; l::VM_SIZE as usize];
    g.read_bytes(p.vm, &mut vm, 1).ok()?;
    for (at, word) in p.vm_words() {
        vm[at as usize..at as usize + 4].copy_from_slice(&word.to_le_bytes());
    }
    let mut array = [0; l::ARRAY_SIZE as usize];
    g.read_bytes(p.array, &mut array, 1).ok()?;
    array[l::SIZE as usize..l::SIZE as usize + 4].copy_from_slice(&p.final_size().to_le_bytes());
    let mut cpu = *c;
    p.finish(&mut cpu);
    Some(Prediction {
        plan: p,
        vm,
        array,
        cpu,
    })
}

impl Prediction {
    /// Compare at the after-observer. EIP names its continuation. The entry
    /// clobbers EAX, ECX, EDX, x87 condition codes and empty physical slots.
    pub fn matches(&self, c: &CpuAbi, g: &runtime::Guest) -> bool {
        let e = &self.cpu;
        let mut vm = [0; l::VM_SIZE as usize];
        let mut array = [0; l::ARRAY_SIZE as usize];
        if g.has_trap()
            || g.read_bytes(self.plan.vm, &mut vm, 1).is_err()
            || vm != self.vm
            || g.read_bytes(self.plan.array, &mut array, 1).is_err()
            || array != self.array
            || (c.ebx, c.ebp, c.esi, c.edi, c.esp) != (e.ebx, e.ebp, e.esi, e.edi, e.esp)
            || (c.eflags, c.eflags_defined, c.fs_base) != (e.eflags, e.eflags_defined, e.fs_base)
            || c.xmm != e.xmm
            || c.mxcsr != e.mxcsr
            || c.x87.control != e.x87.control
            || c.x87.tag != e.x87.tag
            || (c.x87.status ^ e.x87.status) & 0xb8ff != 0
        {
            return false;
        }
        for (i, expect) in e.x87.st.iter().enumerate() {
            if (e.x87.tag >> (i * 2)) & 3 != 3 && c.x87.st[i] != *expect {
                return false;
            }
        }
        let mut actual = [0u8; 256];
        let mut at = self.plan.keys_at;
        for expected in self.plan.keys.chunks(actual.len() / 4) {
            let bytes = &mut actual[..expected.len() * 4];
            if g.read_bytes(at, bytes, 1).is_err()
                || bytes
                    .chunks_exact(4)
                    .zip(expected)
                    .any(|(word, key)| word != key.to_le_bytes().as_slice())
            {
                return false;
            }
            at = at.wrapping_add(bytes.len() as u32);
        }
        true
    }
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
    fn parity_compares_duplicate_keys_in_guest_emission_order_and_header_padding() {
        use runtime::{Guest, guest::Permissions};
        use sim_collision::mopp::{HEADER_BYTES, SIZE};

        let base = 0x10000;
        let (sp, vm, aabb, array, data, code) = (
            base + 0x800,
            base + 0x810,
            base + 0x900,
            base + 0xa00,
            base + 0xb00,
            base + 0xc00,
        );
        let mut g = Guest::new();
        g.map(base, 0x2000, 0, Permissions::RW).unwrap();
        // Both sides of two nested axis splits, with the first key repeated last.
        let instructions = [0x10, 255, 0, 6, 0x10, 255, 0, 1, 0x37, 0x32, 0x37];
        g.write_bytes(code + HEADER_BYTES, &instructions, 1)
            .unwrap();
        for (at, value) in [
            (sp, base),
            (sp + 4, code),
            (sp + 8, aabb),
            (sp + 12, array),
            (code + 12, 1f32.to_bits()),
            (code + SIZE, HEADER_BYTES + instructions.len() as u32),
            (aabb + l::AABB_MAX, 2f32.to_bits()),
            (array, data),
            (array + l::SIZE, 1),
            (array + l::CAPACITY, 4),
            (data, 0x1234),
            (data + 4, 0xbeef),
            (vm + 4, 0xabcd),
        ] {
            g.set_u32(at, value).unwrap();
        }
        let mut cpu = CpuAbi {
            esp: sp,
            ecx: vm,
            ..CpuAbi::default()
        };
        cpu.x87.control = 0x027f;
        cpu.x87.status = l::X87_PE;
        let p = capture(&cpu, &mut g).unwrap();
        assert_eq!(g.u32(array + l::SIZE).unwrap(), 1);
        assert_eq!(g.u32(data + 4).unwrap(), 0xbeef);
        assert!(
            mopp_box(&mut cpu, &mut g, &super::super::WORLD)
                .unwrap()
                .is_some()
        );
        assert!(p.matches(&cpu, &g));
        assert_eq!(g.u32(data).unwrap(), 0x1234);
        assert_eq!(
            [4, 8, 12].map(|offset| g.u32(data + offset).unwrap()),
            [7, 2, 7]
        );
        g.set_u32(data + 4, 2).unwrap();
        g.set_u32(data + 8, 7).unwrap();
        assert!(!p.matches(&cpu, &g));
        g.set_u32(data + 4, 7).unwrap();
        g.set_u32(data + 8, 2).unwrap();
        g.set_u32(vm + 4, 0).unwrap();
        assert!(!p.matches(&cpu, &g));
    }

    /// The live game enters with the x87 PE clear. The setup sets it only
    /// when one of its six conversions rounds, as `fsub`, `fmul`, `fstp`
    /// and `fistp` would.
    #[test]
    fn pe_clear_is_admitted_and_set_only_by_a_rounding_conversion() {
        use runtime::{Guest, guest::Permissions};
        use sim_collision::mopp::{HEADER_BYTES, SIZE};
        use sim_core::Epoch;

        const MEM: u32 = 0x10000;
        let (code, vm, aabb, array, data, stack) =
            (MEM, MEM + 0x100, MEM + 0x200, MEM + 0x300, MEM + 0x400, MEM + 0xf00);
        for (min_x, rounds) in [(0.0f32, false), (0.5, true), (0.3, true), (-2.0, false)] {
            let mut g = Guest::new();
            g.map(MEM, 0x1000, 0, Permissions::RW).unwrap();
            g.set_u32(code + 12, 1.0f32.to_bits()).unwrap();
            g.set_u32(code + SIZE, HEADER_BYTES + 1).unwrap();
            g.set_u8(code + HEADER_BYTES, 0x31).unwrap(); // a single key
            g.set_u32(aabb, min_x.to_bits()).unwrap();
            g.set_u32(aabb + l::AABB_MAX, 2.0f32.to_bits()).unwrap();
            g.set_u32(array + l::DATA, data).unwrap();
            g.set_u32(array + l::CAPACITY, 4).unwrap();
            for (i, value) in [0x12345678, code, aabb, array].into_iter().enumerate() {
                g.set_u32(stack + 4 * i as u32, value).unwrap();
            }
            for control in [0x007f, 0x027f] {
                let mut c = CpuAbi { ecx: vm, esp: stack, ..Default::default() };
                c.x87.control = control;
                c.x87.tag = 0xffff;
                static EPOCH: Epoch = Epoch::new();
                let world = CollisionWorld::new(&EPOCH);
                assert!(mopp_box(&mut c, &mut g, &world).unwrap().is_some(), "min {min_x}");
                assert_eq!(c.x87.status & l::X87_PE != 0, rounds, "min {min_x}, control {control:#x}");
                g.set_u32(array + l::SIZE, 0).unwrap();
            }
        }
    }

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
