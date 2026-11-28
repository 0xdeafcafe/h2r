//! Observer resource predictions. One segment walks the list to its next
//! cache request or return. A request can change the list, so its header,
//! index and accumulated success are recovered from the guest frame.

use guest_layout::points;
use crate::sim_native::{Memory, Result as TransferResult, plan};
use crate::sim_objects::alu::cmp16;
use crate::sim_physics::{arithmetic, byte, logic, pop, push};
use guest_layout::tags::{TAG_ENTRIES_GLOBAL, entry_layout};
use runtime::{CpuAbi, Guest, GuestFault, TransferAbi};

const ENTRY: u32 = points::PREDICT_RESOURCES.base;
const DISPATCH: u32 = 0x0016_e748;
const BITMAP: u32 = 0x0016_e61c;
const MODEL: u32 = 0x0016_e680;
const CLUSTER: u32 = 0x0016_e68e;
const INSTANCE: u32 = 0x0016_e6c2;
const LIGHTMAP_A: u32 = 0x0016_e6d6;
const LIGHTMAP_B: u32 = 0x0016_e6e8;
const GEOMETRY_A: u32 = 0x0016_e6fa;
const GEOMETRY_B: u32 = 0x0016_e705;
const TAG_WITH_MODE: u32 = 0x0016_e710;

const BITMAP_PRELOAD_RETURN: u32 = 0x0016_e651;
const BITMAP_REQUEST_RETURN: u32 = 0x0016_e67b;
const MODEL_RETURN: u32 = 0x0016_e689;
const CLUSTER_RETURN: u32 = 0x0016_e6bc;
const INSTANCE_RETURN: u32 = 0x0016_e6d4;
const LIGHTMAP_A_RETURN: u32 = 0x0016_e6e6;
const LIGHTMAP_B_RETURN: u32 = 0x0016_e6f8;
const GEOMETRY_A_RETURN: u32 = 0x0016_e703;
const GEOMETRY_B_RETURN: u32 = 0x0016_e70e;
const TAG_RETURN: u32 = 0x0016_e719;

pub const SITES: &[u32] = &[
    BITMAP_PRELOAD_RETURN, BITMAP_REQUEST_RETURN, MODEL_RETURN, CLUSTER_RETURN, INSTANCE_RETURN,
    LIGHTMAP_A_RETURN, LIGHTMAP_B_RETURN, GEOMETRY_A_RETURN, GEOMETRY_B_RETURN, TAG_RETURN,
];

/// The guest switches on a signed type, with nine table entries. The
/// table belongs to the image. Decode only the fields each service needs.
struct Prediction(u32);

impl Prediction {
    fn index(&self, ram: &impl Memory) -> Result<u32, GuestFault> {
        Ok(ram.u16(self.0.wrapping_add(2))? as i16 as i32 as u32)
    }

    fn tag(&self, ram: &impl Memory) -> Result<u32, GuestFault> {
        ram.u32(self.0.wrapping_add(4))
    }

    fn tag_record(&self, ram: &impl Memory) -> Result<(u32, u32), GuestFault> {
        let slot = self.tag(ram)? & 0xffff;
        let table = ram.u32(TAG_ENTRIES_GLOBAL)?;
        let at = table.wrapping_add(slot * entry_layout::STRIDE + entry_layout::ADDRESS);
        Ok((table, ram.u32(at)?))
    }

    fn request(
        &self,
        target: u32,
        cpu: &mut CpuAbi,
        ram: &mut impl Memory,
    ) -> Result<Option<TransferAbi>, GuestFault> {
        let mode = cpu.edi & 0xffff;
        let (callee, resume) = match target {
            BITMAP => {
                if mode == 1 {
                    return Ok(None);
                }
                let (table, tag) = self.tag_record(ram)?;
                let index = self.index(ram)?.wrapping_mul(0x74);
                cpu.eax = arithmetic(cpu, index, ram.u32(tag.wrapping_add(0x48))?, false);
                if mode == 2 {
                    cpu.ecx = tag;
                    cpu.edx = table;
                    (0x0003_bcb0, BITMAP_PRELOAD_RETURN)
                } else {
                    cpu.ecx = table;
                    cpu.edx = tag;
                    (0x0012_cc10, BITMAP_REQUEST_RETURN)
                }
            }
            MODEL => {
                cpu.eax = self.tag(ram)?;
                push(cpu, ram, cpu.eax)?;
                (0x0016_ea60, MODEL_RETURN)
            }
            CLUSTER => {
                (cpu.edx, cpu.ecx) = self.tag_record(ram)?;
                let index = self.index(ram)?.wrapping_mul(0xb0);
                cpu.eax = arithmetic(cpu, index, ram.u32(cpu.ecx.wrapping_add(0xa0))?, false);
                cpu.esi = cpu.eax.wrapping_add(0x28);
                (0x0012_dcb0, CLUSTER_RETURN)
            }
            INSTANCE => {
                cmp16(cpu, mode, 3);
                if mode == 3 {
                    return Ok(None);
                }
                cpu.ecx = self.index(ram)?;
                cpu.eax = self.tag(ram)?;
                (0x0016_e770, INSTANCE_RETURN)
            }
            LIGHTMAP_A | LIGHTMAP_B => {
                cmp16(cpu, mode, 1);
                if mode == 1 {
                    return Ok(None);
                }
                let index = self.index(ram)?;
                push(cpu, ram, index)?;
                if target == LIGHTMAP_A {
                    cpu.edx = index;
                    (0x0016_e7b0, LIGHTMAP_A_RETURN)
                } else {
                    cpu.eax = index;
                    (0x0016_e820, LIGHTMAP_B_RETURN)
                }
            }
            GEOMETRY_A | GEOMETRY_B => {
                cpu.edx = self.index(ram)?;
                if target == GEOMETRY_A {
                    (0x0016_e890, GEOMETRY_A_RETURN)
                } else {
                    (0x0016_e8f0, GEOMETRY_B_RETURN)
                }
            }
            TAG_WITH_MODE => {
                cpu.eax = self.tag(ram)?;
                push(cpu, ram, cpu.edi)?;
                (0x0016_e950, TAG_RETURN)
            }
            // An edited jump table can name another block. Replay the
            // untouched entry/return selector in the original.
            _ => return Err(crate::sim_native::unported(target, "an edited jump table names another block")),
        };
        ram.call(cpu, callee, resume).map(Some)
    }
}

fn finish(cpu: &mut CpuAbi, ram: &impl Memory) -> TransferResult {
    cpu.esi = pop(cpu, ram)?;
    cpu.ebp = pop(cpu, ram)?;
    // The high byte of the saved ECX doubles as the accumulated success.
    cpu.ecx = pop(cpu, ram)?;
    ram.ret(cpu, 4)
}

fn step(cpu: &mut CpuAbi, ram: &mut impl Memory, pc: u32) -> TransferResult {
    if pc == ENTRY {
        push(cpu, ram, cpu.ecx)?;
        push(cpu, ram, cpu.ebp)?;
        push(cpu, ram, cpu.esi)?;
        cpu.esi = ram.u32(cpu.esp.wrapping_add(16))?;
        cpu.ecx = ram.u32(cpu.esi)?;
        cpu.eax = byte(cpu.eax, 1);
        cpu.ebp = 0;
        logic(cpu, cpu.ecx, 1 << 31);
        ram.set_u8(cpu.esp.wrapping_add(11), 1)?;
        if cpu.ecx as i32 <= 0 {
            return finish(cpu, ram);
        }
        push(cpu, ram, cpu.ebx)?;
    } else {
        if pc == CLUSTER_RETURN {
            cpu.esi = ram.u32(cpu.esp.wrapping_add(20))?;
        }
        if pc != BITMAP_PRELOAD_RETURN && pc != BITMAP_REQUEST_RETURN {
            cpu.ebx = byte(cpu.ebx, cpu.eax as u8);
        }
    }

    let success = cpu.esp.wrapping_add(15);
    let mut next_request = pc == ENTRY;
    // A malformed count can describe an arbitrarily long run of ignored
    // types. Exceeding the budget replays just this segment in the guest.
    for _ in 0..512 {
        if next_request {
            let data = ram.u32(cpu.esi.wrapping_add(4))?;
            let entry = Prediction(data.wrapping_add(cpu.ebp.wrapping_mul(8)));
            cpu.eax = entry.0;
            cpu.ecx = ram.u16(entry.0)? as i16 as i32 as u32;
            arithmetic(cpu, cpu.ecx, 8, true);
            cpu.ebx = byte(cpu.ebx, 1);
            if cpu.ecx <= 8 {
                let target = ram.u32(DISPATCH + cpu.ecx * 4)?;
                if let Some(call) = entry.request(target, cpu, ram)? {
                    return Ok(call);
                }
            }
        }
        let accepted = ram.u8(success)? != 0 && cpu.ebx as u8 != 0;
        ram.set_u8(success, u8::from(accepted))?;
        cpu.eax = ram.u32(cpu.esi)?;
        cpu.ebp = cpu.ebp.wrapping_add(1);
        arithmetic(cpu, cpu.ebp, cpu.eax, true);
        if cpu.ebp as i32 >= cpu.eax as i32 {
            cpu.eax = byte(cpu.eax, ram.u8(success)?);
            cpu.ebx = pop(cpu, ram)?;
            return finish(cpu, ram);
        }
        next_request = true;
    }
    Err(crate::sim_native::unported(pc, "more than 512 resource entries in one segment"))
}

pub fn run(cpu: &mut CpuAbi, ram: &mut Guest, pc: u32) -> Result<Option<TransferAbi>, GuestFault> {
    plan::segment::<32, _>(cpu, ram, pc, ENTRY..DISPATCH, SITES, |cpu, plan, pc| {
        plan.allow_large_segment();
        step(cpu, plan, pc)
    })
}

#[cfg(test)]
mod tests;
