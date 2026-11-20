//! Registration of [`crate::abi::OVERRIDES`] with the runtime's dispatch
//! table.
//!
//! Built with `--features runtime-glue`. Uses `ExecCtx::memory`,
//! `Guest::read_bytes`/`write_bytes` and `Dispatch::register_override`
//! with a `(cpu, ctx, selector)` handler. [`register_all`] has no caller;
//! the `feature` crate's `engine-leaves` feature places the same adapters
//! through the plan (`impl.engine`, off by default).
//!
//! Each handler holds one memory guard for the whole call, so the Rust body
//! sees a consistent guest, and releases it before returning the transfer.
//! Nothing here calls back into guest code, so no continuation is needed.

use std::cell::RefCell;

use runtime::dispatch::{Dispatch, Override as Displaced};
use runtime::guest::{Access, FaultReason};
use runtime::{CpuAbi, Guest, GuestFault, TransferAbi};

use crate::abi::{Override, Regs};
use crate::memory::{GuestMemory, MemFault};

/// The runtime's checked memory seen through [`GuestMemory`]. The latest
/// runtime fault is kept with its full reason, since [`MemFault`] has none.
/// It must be the latest, not the first: [`GuestMemory::fill`] tries a
/// bulk store and, if that faults, retries one store at a time, so the
/// fault it returns is the last one recorded.
struct RuntimeMemory<'a> {
    guest: &'a mut Guest,
    fault: RefCell<Option<(MemFault, GuestFault)>>,
}

impl RuntimeMemory<'_> {
    fn latch(&self, f: GuestFault, addr: u32, len: usize, write: bool) -> MemFault {
        let mf = MemFault {
            addr,
            len: len as u32,
            write,
        };
        *self.fault.borrow_mut() = Some((mf, f));
        mf
    }
}

impl GuestMemory for RuntimeMemory<'_> {
    fn take_write_watch(&mut self, id: runtime::WatchId) -> Result<Vec<runtime::DirtyRange>, runtime::WatchError> {
        self.guest.take_dirty(id)
    }

    fn read(&self, addr: u32, buf: &mut [u8]) -> Result<(), MemFault> {
        self.guest
            .read_bytes(addr, buf, 1)
            .map_err(|f| self.latch(f, addr, buf.len(), false))
    }
    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), MemFault> {
        let r = self.guest.write_bytes(addr, data, 1);
        r.map_err(|f| self.latch(f, addr, data.len(), true))
    }
}

fn regs_of(cpu: &CpuAbi) -> Regs {
    Regs {
        eax: cpu.eax,
        ecx: cpu.ecx,
        edx: cpu.edx,
        ebx: cpu.ebx,
        esp: cpu.esp,
        ebp: cpu.ebp,
        esi: cpu.esi,
        edi: cpu.edi,
    }
}

fn store(cpu: &mut CpuAbi, r: &Regs) {
    (cpu.eax, cpu.ecx, cpu.edx, cpu.ebx) = (r.eax, r.ecx, r.edx, r.ebx);
    (cpu.esp, cpu.ebp, cpu.esi, cpu.edi) = (r.esp, r.ebp, r.esi, r.edi);
}

/// Run one adapter against the runtime. On a fault the CPU is left exactly
/// as it was on entry (memory writes before the fault remain, as they would
/// for the original).
pub fn invoke(
    o: &Override,
    cpu: &mut CpuAbi,
    guest: &mut Guest,
) -> Result<TransferAbi, GuestFault> {
    let mut regs = regs_of(cpu);
    let mut mem = RuntimeMemory {
        guest,
        fault: RefCell::new(None),
    };
    match (o.run)(&mut regs, &mut mem) {
        Ok(target) => {
            store(cpu, &regs);
            Ok(TransferAbi::ret(target))
        }
        // Use the runtime's own fault only if it is the one the body
        // returned; anything else is a fault the body recovered from.
        Err(f) => Err(mem
            .fault
            .into_inner()
            .filter(|(mf, _)| *mf == f)
            .map(|(_, g)| g)
            .unwrap_or(GuestFault {
                addr: f.addr,
                len: f.len as usize,
                access: if f.write { Access::Write } else { Access::Read },
                reason: FaultReason::Unmapped,
            })),
    }
}

/// Register every adapter. Returns whatever overrides they displaced, so
/// a caller can put them back with `Dispatch::restore_override`.
pub fn register_all(dispatch: &mut Dispatch) -> Vec<(u32, Displaced)> {
    let mut displaced = Vec::new();
    for o in crate::abi::OVERRIDES {
        let prev = dispatch.register_override(o.addr, o.name, move |cpu, ctx, _selector| {
            let mut guest = ctx.memory();
            invoke(o, cpu, &mut guest)
        });
        if let Some(p) = prev {
            displaced.push((o.addr, p));
        }
    }
    displaced
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::guest::Permissions;

    #[test]
    fn invoke_runs_datum_new_on_a_runtime_guest() {
        let mut g = Guest::new();
        g.map_zeroed(0x10_0000, 0x1_0000, Permissions::RW).unwrap();
        let (array, stack) = (0x10_0100u32, 0x10_f000u32);
        // A two-slot array, made valid by the same adapters.
        let mut cpu = CpuAbi::default();
        let (name, ret) = (0x10_0000u32, 0x1234_5678u32);
        g.set_u32(stack, ret).unwrap();
        g.set_u32(stack + 4, array).unwrap();
        g.set_u32(stack + 8, name).unwrap();
        g.set_u32(stack + 12, 0).unwrap();
        (cpu.esp, cpu.ebx, cpu.edx, cpu.ecx) = (stack, 2, 8, 0);
        invoke(crate::abi::find(0x16b5f0).unwrap(), &mut cpu, &mut g).unwrap();
        g.set_u32(stack, ret).unwrap();
        (cpu.esp, cpu.eax) = (stack, array);
        invoke(crate::abi::find(0x16b790).unwrap(), &mut cpu, &mut g).unwrap();

        g.set_u32(stack, ret).unwrap();
        (cpu.esp, cpu.edx) = (stack, array);
        let t = invoke(crate::abi::find(0x16b840).unwrap(), &mut cpu, &mut g).unwrap();
        assert_eq!(t, TransferAbi::ret(ret));
        assert_eq!((cpu.eax, cpu.esp), (0x8000_0000, stack + 4));

        // An unmapped array faults with the runtime's own reason, and the
        // CPU is untouched.
        let before = cpu;
        cpu.edx = 0x20_0000;
        cpu.esp = stack;
        let err = invoke(crate::abi::find(0x16b840).unwrap(), &mut cpu, &mut g).unwrap_err();
        assert_eq!(err.reason, FaultReason::Unmapped);
        assert_eq!((cpu.eax, cpu.edx), (before.eax, 0x20_0000));
    }

    /// A fill that runs off the writable mapping reports the store that
    /// faulted (the dword at 0x101000), with the runtime's own reason, not
    /// the bulk probe `fill` tried first, and keeps the stores before it.
    /// The read-only case shows the reason is the runtime's, not one
    /// rebuilt from the [`MemFault`].
    #[test]
    fn invoke_reports_the_faulting_store_of_a_partial_fill() {
        for next_page in [None, Some(Permissions::READ)] {
            let mut g = Guest::new();
            g.map_zeroed(0x10_0000, 0x1000, Permissions::RW).unwrap();
            if let Some(p) = next_page {
                g.map_zeroed(0x10_1000, 0x1000, p).unwrap();
            }
            let (header, elem, stack) = (0x10_0100u32, 0x10_0ff8u32, 0x10_0800u32);
            g.set_u32(header + 0x24, 16).unwrap(); // element size
            g.write_bytes(elem, &[0xcc; 8], 1).unwrap();
            g.set_u32(stack, 0x1234_5678).unwrap();
            let mut cpu = CpuAbi {
                esp: stack,
                edx: header,
                esi: elem,
                ..CpuAbi::default()
            };
            let before = cpu;
            let err = invoke(crate::abi::find(0x16ba00).unwrap(), &mut cpu, &mut g).unwrap_err();
            assert_eq!((err.addr, err.len), (0x10_1000, 4));
            assert_eq!(err.access, Access::Write);
            let reason = match next_page {
                None => FaultReason::Unmapped,
                Some(_) => FaultReason::Protection,
            };
            assert_eq!(err.reason, reason);
            let mut prefix = [0xffu8; 8];
            g.read_bytes(elem, &mut prefix, 1).unwrap();
            assert_eq!(prefix, [0; 8]);
            assert_eq!(cpu, before);
        }
    }
}
