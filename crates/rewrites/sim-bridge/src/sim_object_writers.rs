//! The object table's writers, installed over the guest functions they
//! replace (`sim_objects::writers`, docs/engine/object-update.md,
//! "Table writers"): the parent detach `b9890`, the body free with its
//! datum delete `bc300`, the body allocation `bc280` and the pool's
//! compaction `18e470`. Each reads the guest's objects array into the
//! table, runs the writer and scatters the table back, so the arena is
//! authoritative between calls and the guest's own writers never see a
//! stale one. The walks that reach them (`free_tree`, `b8460`) call them
//! as guest calls; the hook answers in place of the original.

use guest_layout::points;
use guest_layout::{Fault, Mem, MemMut};
use hooks::Builder;
use runtime::{CpuAbi, Guest, TransferAbi};
use sim_objects::{pool, writers};
use sim_world::Handle;

/// The label of the writers' implementations.
pub const LABEL: &str = "sim.native.object-writers";

/// A guest's memory as `Mem`/`MemMut`, so the writers run on the arena.
pub struct GuestMem<'a>(pub &'a mut Guest);

impl Mem for GuestMem<'_> {
    fn read_bytes(&self, addr: u32, buf: &mut [u8]) -> Result<(), Fault> {
        let len = buf.len() as u32;
        self.0.read_bytes(addr, buf, 1).map_err(|_| Fault { addr, len, write: false })
    }
}

impl MemMut for GuestMem<'_> {
    fn write_bytes(&mut self, addr: u32, data: &[u8]) -> Result<(), Fault> {
        let len = data.len() as u32;
        self.0.write_bytes(addr, data, 1).map_err(|_| Fault { addr, len, write: true })
    }
}

/// `b9890 (eax = object)`; plain `ret`. Leaves `eax` none, as the original.
fn detach(cpu: &mut CpuAbi, g: &mut Guest) -> TransferAbi {
    let object = Handle::from_raw(cpu.eax);
    let target = g.u32(cpu.esp).unwrap_or_else(|f| unimplemented!("objects: detach: the return address: {f:?}"));
    writers::detach_in_guest(&mut GuestMem(g), object)
        .unwrap_or_else(|e| unimplemented!("objects: detach {object:?}: {e:?}"));
    cpu.eax = u32::MAX;
    cpu.esp = cpu.esp.wrapping_add(4);
    TransferAbi::ret(target)
}

/// `bc300 (object: stack 0)`; `ret 4`.
fn free(cpu: &mut CpuAbi, g: &mut Guest) -> TransferAbi {
    let word = |g: &Guest, at: u32| g.u32(at).unwrap_or_else(|f| unimplemented!("objects: free: a read at {at:#x}: {f:?}"));
    let (target, object) = (word(g, cpu.esp), Handle::from_raw(word(g, cpu.esp.wrapping_add(4))));
    writers::free_in_guest(&mut GuestMem(g), object).unwrap_or_else(|e| unimplemented!("objects: free {object:?}: {e:?}"));
    cpu.esp = cpu.esp.wrapping_add(8);
    TransferAbi::ret(target)
}

/// `bc280 (size: stack 0)`; `ret 4`. The handle, or none, in `eax`.
fn alloc(cpu: &mut CpuAbi, g: &mut Guest) -> TransferAbi {
    let word = |g: &Guest, at: u32| g.u32(at).unwrap_or_else(|f| unimplemented!("objects: alloc: a read at {at:#x}: {f:?}"));
    let (target, size) = (word(g, cpu.esp), word(g, cpu.esp.wrapping_add(4)) as u16 as i16);
    let h = writers::alloc_in_guest(&mut GuestMem(g), size).unwrap_or_else(|e| unimplemented!("objects: alloc {size}: {e:?}"));
    cpu.eax = h.map_or(u32::MAX, Handle::raw);
    cpu.esp = cpu.esp.wrapping_add(8);
    TransferAbi::ret(target)
}

/// `18e470 (edi = pool)`; plain `ret`. `eax` is what the original leaves:
/// the fixed flag in `al` for a fixed pool, else 0.
fn compact(cpu: &mut CpuAbi, g: &mut Guest) -> TransferAbi {
    let target = g.u32(cpu.esp).unwrap_or_else(|f| unimplemented!("objects: compact: the return address: {f:?}"));
    let fixed = g.u8(cpu.edi.wrapping_add(pool::field::FIXED)).unwrap_or_else(|f| unimplemented!("objects: compact: the pool: {f:?}"));
    pool::compact(&mut GuestMem(g), cpu.edi).unwrap_or_else(|f| unimplemented!("objects: compact {:#x}: {f:?}", cpu.edi));
    cpu.eax = if fixed != 0 { (cpu.eax & !0xff) | u32::from(fixed) } else { 0 };
    cpu.esp = cpu.esp.wrapping_add(4);
    TransferAbi::ret(target)
}

pub(crate) fn install(b: &mut Builder) {
    for (point, run) in [
        (&points::OBJECT_DETACH, detach as fn(&mut CpuAbi, &mut Guest) -> TransferAbi),
        (&points::OBJECT_BODY_FREE, free),
        (&points::OBJECT_BODY_ALLOC, alloc),
        (&points::OBJECT_POOL_COMPACT, compact),
    ] {
        b.raw()
            .implement(point, LABEL, move |cpu, ctx, pc| {
                if pc != point.base {
                    unimplemented!("objects: {}: entered at {pc:#x}, not the entry", point.name);
                }
                let mut mem = ctx.memory();
                Ok(run(cpu, &mut mem))
            })
            .context_only()
            .replaces_guest();
    }
}
