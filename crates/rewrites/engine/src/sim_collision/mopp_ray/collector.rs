//! Entry I's collector callbacks. The walk yields a terminal at its actual
//! interpreter depth. That depth selects the guest stack scratch passed to
//! the filter, collection and child; all three may run arbitrary guest code.
//! The installer makes each call [`CollectorCast::next`] returns and hands
//! back the CPU it returned with.
use super::{Program, Read, X87_FREE, prepare, vec4};
use crate::sim_collision::{
    CollisionMemory,
    layout::{self, mopp_ray as l},
};
use crate::sim_native::Memory;
use guest_layout::entry::GuestCall;
use runtime::CpuAbi;
use sim_collision::mopp::ray::{Constants, Event, Precision, Raised, RayWalk};
use sim_collision::{Bail, CollisionWorld, GuestRead};
use sim_core::det;

const VM_LEN: usize = 0x68;
const VM_INPUT: u32 = 0x20;
const VM_FILTER: u32 = 0x44;
const VM_HIT: u32 = 0x50;
const VM_OUTPUT: u32 = 0x58;
const VM_PARENT: u32 = 0x60;

pub(super) struct Setup {
    vm: u32,
    code: u32,
    bytes: [u8; VM_LEN],
    revision: u64,
    constants: Constants,
}

impl Setup {
    pub(super) fn read(
        m: &impl GuestRead,
        vm: u32,
        code: u32,
        input: u32,
        parent: u32,
        collection: u32,
        collector: u32,
        inverse_scale: u32,
        revision: u64,
        constants: Constants,
    ) -> Result<Self, Bail> {
        let fraction = m.u32(collector.wrapping_add(l::COLLECTOR_FRACTION))?;
        let mut bytes = [0; VM_LEN];
        m.read(vm, &mut bytes)?;
        m.read(input, &mut bytes[0x20..0x48])?;
        bytes[VM_HIT as usize] = 0;
        for (off, value) in [
            (l::CODE, code),
            (l::INVERSE_SCALE, inverse_scale),
            (l::FRACTION, fraction),
            (VM_OUTPUT, 0),
            (l::COLLECTOR, collector),
            (VM_PARENT, parent),
            (l::COLLECTION, collection),
        ] {
            bytes[off as usize..off as usize + 4].copy_from_slice(&value.to_le_bytes());
        }
        Ok(Self {
            vm,
            code,
            bytes,
            revision,
            constants,
        })
    }

    // The walk owns the bytecode, original ray, reciprocal and floating
    // constants. A callback is free to modify any of them. A trial whose
    // inputs changed cannot use the pre-callback walk for the remainder.
    fn check_walk_inputs(&self, g: &mut impl CollisionMemory) -> Result<(), Bail> {
        if g.mopp_revision(self.code)? != self.revision {
            return Err(Bail::Data);
        }
        let m = Read(&*g, (0, 0));
        let mut code_and_inverse = [0; 8];
        let mut endpoints = [0; 0x20];
        m.read(self.vm.wrapping_add(l::CODE), &mut code_and_inverse)?;
        m.read(self.vm.wrapping_add(VM_INPUT), &mut endpoints)?;
        if code_and_inverse[..] != self.bytes[0x10..0x18]
            || endpoints[..] != self.bytes[0x20..0x40]
            || m.u32(layout::ONE)? != self.constants.one
            || m.u32(layout::ZERO)? != self.constants.zero
            || vec4(&m, l::UNIT)? != self.constants.unit
        {
            return Err(Bail::Data);
        }
        Ok(())
    }

    fn write(&self, g: &mut impl CollisionMemory) -> Result<(), Bail> {
        let ranges = [0x10..0x18, 0x20..0x48, 0x50..0x51, 0x54..0x68];
        for r in &ranges {
            if !g.writable(self.vm.wrapping_add(r.start as u32), r.len() as u32) {
                return Err(Bail::Fault);
            }
        }
        for r in ranges {
            g.write_bytes(self.vm.wrapping_add(r.start as u32), &self.bytes[r], 1)
                .map_err(|_| Bail::Fault)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct Terminal {
    key: u32,
    /// The float interpreter's ESP below its locals and saved registers.
    sp: u32,
}

enum Resume {
    Walk,
    Filter(Terminal),
    Child(Terminal),
    Ray,
}

/// Entry I in Rust. A caller drives each returned guest call exactly once
/// and supplies the returned CPU on resumption. A decline after a callback
/// has run cannot hand the query back to the lifted entry: the installer
/// names it (`collision: mopp_ray: <cause>`).
pub struct CollectorCast {
    vm: u32,
    entry_sp: u32,
    root_sp: u32,
    setup: Setup,
    precision: Precision,
    walk: RayWalk<Program>,
    resume: Resume,
}

impl CollectorCast {
    pub fn begin(
        cpu: &CpuAbi,
        g: &mut impl CollisionMemory,
        world: &CollisionWorld,
    ) -> Result<Self, Bail> {
        let (setup, walk) = prepare(cpu, g, world)?;
        // Aligned setup locals, root interpreter arguments, and its own
        // aligned locals and saved registers. Nested calls cost 0x1c0 each.
        let aligned = cpu.esp.checked_sub(4).ok_or(Bail::Fault)? & !15;
        let root_sp = aligned.checked_sub(0x210).ok_or(Bail::Fault)?;
        setup.write(g)?;
        Ok(Self {
            vm: cpu.ecx,
            entry_sp: cpu.esp,
            root_sp,
            setup,
            precision: Precision::from_control(cpu.x87.control)?,
            walk,
            resume: Resume::Walk,
        })
    }

    /// The MXCSR and x87 exception flags the walk has left so far: the
    /// caller puts them in the CPU before each callback and at the return.
    pub fn raised(&self) -> Raised {
        self.walk.raised()
    }

    fn vm_word(&self, m: &impl Memory, off: u32) -> Result<u32, Bail> {
        m.u32(self.vm.wrapping_add(off)).map_err(|_| Bail::Fault)
    }

    fn method(
        &self,
        m: &impl Memory,
        this: u32,
        slot: u32,
        t: Terminal,
    ) -> Result<GuestCall, Bail> {
        let table = m.u32(this).map_err(|_| Bail::Fault)?;
        let target = m.u32(table.wrapping_add(slot)).map_err(|_| Bail::Fault)?;
        Ok(GuestCall::new(target)
            .ecx(this)
            .edx(table)
            .edi(t.key)
            .reserve(self.entry_sp - t.sp))
    }

    fn child(&mut self, g: &impl Memory, t: Terminal) -> Result<GuestCall, Bail> {
        let collection = self.vm_word(g, l::COLLECTION)?;
        let call = self.method(g, collection, l::COLLECTION_CHILD, t)?;
        self.resume = Resume::Child(t);
        Ok(call.eax(collection).args([t.key, t.sp + 0xa0]))
    }

    /// The first call ignores EAX. Thereafter EAX is the register returned
    /// by the guest callee. All mutable callback inputs are re-read from
    /// guest memory at the boundary where the interpreter reads them.
    pub fn next(
        &mut self,
        g: &mut impl CollisionMemory,
        cpu: &CpuAbi,
    ) -> Result<Option<GuestCall>, Bail> {
        if g.trapped() {
            return Err(Bail::Trap);
        }
        let eax = cpu.eax;
        match std::mem::replace(&mut self.resume, Resume::Walk) {
            Resume::Filter(t) => {
                if g.u8(eax).map_err(|_| Bail::Fault)? != 0 {
                    return self.child(g, t).map(Some);
                }
            }
            Resume::Child(t) => {
                // A callback that changes the VM to the sibling output-record
                // path cannot continue as a collector trial.
                if self.vm_word(g, VM_OUTPUT)? != 0 {
                    return Err(Bail::Data);
                }
                let call = self.method(g, eax, 0x24, t)?;
                let parent = self.vm_word(g, VM_PARENT)?;
                let info = g.u32(parent.wrapping_add(8)).map_err(|_| Bail::Fault)?;
                let collector = self.vm_word(g, l::COLLECTOR)?;
                let body = t.sp + 0x60;
                for (k, word) in [eax, t.key, info, parent].into_iter().enumerate() {
                    g.set_u32(body + k as u32 * 4, word)
                        .map_err(|_| Bail::Fault)?;
                }
                self.resume = Resume::Ray;
                return Ok(Some(call.eax(eax).esi(self.vm).args([
                    self.vm.wrapping_add(VM_INPUT),
                    body,
                    collector,
                ])));
            }
            Resume::Ray => {
                let collector = self.vm_word(g, l::COLLECTOR)?;
                let f = g
                    .u32(collector.wrapping_add(l::COLLECTOR_FRACTION))
                    .map_err(|_| Bail::Fault)?;
                g.set_u32(self.vm.wrapping_add(l::FRACTION), f)
                    .map_err(|_| Bail::Fault)?;
            }
            Resume::Walk => {}
        }
        if !det::guest_env(cpu.mxcsr)
            || !det::x87_env(cpu.x87.control, cpu.x87.status, cpu.x87.tag, X87_FREE)
            || Precision::from_control(cpu.x87.control)? != self.precision
        {
            return Err(Bail::Env);
        }
        self.setup.check_walk_inputs(g)?;
        // The callbacks ran under the flags the walk had raised; the walk
        // goes on from what they left.
        self.walk.absorb(cpu.mxcsr, cpu.x87.status);
        let fraction = self.vm_word(g, l::FRACTION)?;
        while let Some(step) = self.walk.next(fraction)? {
            let Event::Key(key) = step else { continue };
            // The walk has popped this terminal's frame; the levels left
            // are precisely its ancestors.
            let sp = self
                .root_sp
                .checked_sub(self.walk.depth() as u32 * 0x1c0)
                .ok_or(Bail::Depth)?;
            let terminal = Terminal { key, sp };
            let filter = self.vm_word(g, VM_FILTER)?;
            if filter == 0 {
                return self.child(g, terminal).map(Some);
            }
            let collection = self.vm_word(g, l::COLLECTION)?;
            let call = self.method(g, filter, 0, terminal)?;
            self.resume = Resume::Filter(terminal);
            return Ok(Some(call.eax(sp + 0x3b).esi(collection).args([
                sp + 0x3b,
                self.vm.wrapping_add(VM_INPUT),
                collection,
                key,
            ])));
        }
        Ok(None)
    }
}
