//! The merged portal node's window at 0x00172f6c. A guest call is checked
//! on its whole slot, scratch argument and return registers. After the
//! arm's proof Rust materializes every window but one in 64 (`Audit`),
//! which the guest writes and is compared; a difference sends the window
//! records to the guest for the session. A new BSP needs a new proof.
use super::*;
use scene_extract::visibility::{
    Decline, guest,
    window_record::{SIZE, WindowInputs},
};
use std::sync::Mutex;

use guest_layout::points::scene_effects::{PORTAL_WINDOW_CALL, PORTAL_WINDOW_READY, PORTAL_WINDOW_RESUME};
const VALIDATE_PLANES: u32 = 0x0047_ff86;
const WINDOWS: u32 = 0x1974;
const COUNT: u32 = 0x1970;
const LIMIT: u16 = 512;

struct Read<'a>(&'a dyn GuestMemory);

impl scene_extract::GuestRead for Read<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

fn refusal(why: Decline) -> Refusal {
    match why {
        Decline::Unread => Refusal::Memory,
        _ => Refusal::Layout,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Registers {
    eax: u32,
    ecx: u32,
    edx: u32,
}

impl Registers {
    fn read(cpu: &CpuAbi) -> Self {
        Self {
            eax: cpu.eax,
            ecx: cpu.ecx,
            edx: cpu.edx,
        }
    }
    fn apply(self, cpu: &mut CpuAbi) {
        cpu.eax = self.eax;
        cpu.ecx = self.ecx;
        cpu.edx = self.edx;
    }
}

struct Window {
    image: [u8; SIZE],
    dest: u32,
    registers: Registers,
    // The writer uses its second argument word as scratch.
    argument_addr: u32,
    argument_after: u32,
    accepted: bool,
}

impl Window {
    fn read(mem: &dyn GuestMemory, cpu: &CpuAbi) -> Result<Self> {
        fp_supported(cpu)?;
        if cpu.x87.control != 0x027f || cpu.x87.tag != u16::MAX {
            return Err(Refusal::Unsupported);
        }
        // The original can clear this flag if a corner fails one of its
        // diagnostic plane tests. Preserve the diagnostic while it is armed.
        if mem.u8(VALIDATE_PLANES)? != 0 {
            return Err(Refusal::Unsupported);
        }
        let vis = ptr(mem, guest::ACTIVE_VIS)?;
        if !matches!(vis, objects::MAIN_VIS | objects::LIGHT_VIS) {
            return Err(Refusal::Unsupported);
        }
        let base = ptr(mem, vis)?;
        let count = mem.u16(at(base, COUNT)?)?;
        if !(1..=LIMIT).contains(&count) {
            return Err(Refusal::Layout);
        }
        let dest = at(base, WINDOWS + u32::from(count - 1) * SIZE as u32)?;
        if cpu.edi != dest {
            return Err(Refusal::Layout);
        }
        let frustum = stack(mem, cpu, 0)?;
        let argument_addr = at(cpu.esp, 4)?;
        let ordinal = mem.u32(argument_addr)?;
        // The original writes interleaved with its reads. Overlapping the
        // input frustum, rectangle or call frame requires those writes.
        let end = at(dest, SIZE as u32)?;
        for (addr, size) in [(frustum, 0x1bc), (cpu.ecx, 16), (cpu.esp, 8)] {
            if dest < at(addr, size)? && end > addr {
                return Err(Refusal::Layout);
            }
        }
        let inputs = WindowInputs::read(&Read(mem), frustum, cpu.ecx, ordinal).map_err(refusal)?;
        let mut image = [0; SIZE];
        mem.read(dest, &mut image)?;
        let accepted = inputs.build(&mut image).map_err(refusal)?;
        let (registers, argument_after) = if accepted {
            (
                Registers {
                    eax: ((dest + 0xa8) & !0xff) | 1,
                    ecx: dest + 0x54,
                    edx: dest + 0x98,
                },
                dest + 0x98,
            )
        } else {
            (
                Registers {
                    eax: cpu.eax & !0xff,
                    ..Registers::read(cpu)
                },
                ordinal,
            )
        };
        Ok(Self {
            image,
            dest,
            registers,
            argument_addr,
            argument_after,
            accepted,
        })
    }

    fn matches(&self, mem: &dyn GuestMemory, cpu: &CpuAbi) -> Result<bool> {
        let mut actual = [0; SIZE];
        mem.read(self.dest, &mut actual)?;
        Ok(actual == self.image
            && self.registers == Registers::read(cpu)
            && cpu.edi == self.dest
            && mem.u32(self.argument_addr)? == self.argument_after)
    }
}

#[derive(Default)]
struct State {
    generation: Option<u64>,
    pending: HashMap<(u32, u32), Window>,
    audit: Audit,
}

impl State {
    fn scope(&mut self) {
        let generation = crate::native_map_work::bsp_generation();
        if self.generation != Some(generation) {
            self.pending.clear();
            self.audit.rescope();
            self.generation = Some(generation);
        }
    }

    /// A guest call's slot, scratch word and registers against the plan
    /// made at its call. `None`: no plan for this frame.
    fn finish(&mut self, mem: &dyn GuestMemory, cpu: &CpuAbi, thread: u32) -> Option<bool> {
        let plan = self.pending.remove(&(thread, cpu.esp))?;
        let same = plan.matches(mem, cpu) == Ok(true);
        if self.audit.compared(same) {
            eprintln!(
                "native-scene-lists: a Rust portal window differs from 0x00163db0's; the guest writes the windows from here"
            );
        }
        Some(same)
    }
}

pub(super) fn install(b: &mut Builder) {
    let state = Arc::new(Mutex::new(State::default()));
    let before = Arc::clone(&state);
    let stage = SceneList::WindowRecord;
    b.raw()
        .before(&PORTAL_WINDOW_CALL, "materialize portal window", move |c| {
            if c.addr != PORTAL_WINDOW_CALL.base {
                return Ok(Flow::Continue);
            }
            let runtime = Arc::clone(c.ctx.runtime());
            if !runtime.read_dispatch(|d| {
                d.has_generated(PORTAL_WINDOW_RESUME)
                    && d.override_name(points::VIEW_WINDOW_BUILD.base).is_none()
            }) {
                stage.count(SceneListResult::Fallback);
                stage.count(SceneListResult::Unsupported);
                return Ok(Flow::Continue);
            }
            let mut mem = runtime.memory();
            let mut state = before.lock().unwrap_or_else(|e| e.into_inner());
            state.scope();
            let key = (c.ctx.thread_id, c.cpu.esp.wrapping_add(8));
            state.pending.remove(&key);
            if !state.audit.compares() {
                stage.count(SceneListResult::Fallback);
                return Ok(Flow::Continue);
            }
            let plan = match Window::read(&*mem, c.cpu) {
                Ok(window) => window,
                Err(why) => {
                    stage.count(SceneListResult::Fallback);
                    stage.count(match why {
                        Refusal::Memory => SceneListResult::Memory,
                        Refusal::Unsupported => SceneListResult::Unsupported,
                        _ => SceneListResult::Layout,
                    });
                    return Ok(Flow::Continue);
                }
            };
            if !state.audit.answers() {
                state.pending.insert(key, plan);
                stage.count(SceneListResult::Fallback);
                return Ok(Flow::Continue);
            }
            // Preflight before changing ESP, memory, or the return registers.
            if plan.accepted
                && (mem.check(plan.dest, SIZE, Access::Write, 1).is_err()
                    || mem.check(plan.argument_addr, 4, Access::Write, 1).is_err())
            {
                stage.count(SceneListResult::Memory);
                stage.count(SceneListResult::Fallback);
                return Ok(Flow::Continue);
            }
            if plan.accepted {
                for (offset, bytes) in [
                    (0u32, &plan.image[..4]),
                    (0x10, &plan.image[0x10..0x20]),
                    (0x24, &plan.image[0x24..]),
                ] {
                    mem.write_bytes_from(c.addr, plan.dest + offset, bytes, 1)?;
                }
                mem.write_bytes_from(
                    c.addr,
                    plan.argument_addr,
                    &plan.argument_after.to_le_bytes(),
                    1,
                )?;
            }
            plan.registers.apply(c.cpu);
            stage.count(SceneListResult::Native);
            c.cpu.esp = c.cpu.esp.wrapping_add(8);
            Ok(Flow::Return(TransferAbi::jump(PORTAL_WINDOW_RESUME)))
        })
        .optional()
        .context_only();
    b.raw()
        .observe_before(&PORTAL_WINDOW_READY, "compare portal window", move |o| {
            let mem = o.guest();
            let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
            state.scope();
            match state.finish(&*mem, o.cpu, o.thread_id) {
                Some(true) => stage.count(SceneListResult::Compared),
                Some(false) => {
                    stage.count(SceneListResult::Compared);
                    stage.count(SceneListResult::Mismatch);
                }
                None => (),
            }
        })
        .optional()
        .context_only();
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    fn scene() -> (FakeGuest, CpuAbi) {
        let mut m = FakeGuest::new();
        let (regions, camera, stack, rect, bounds) = (0x10000, 0x3000, 0x4000, 0x5000, 0x6000);
        m.put_u8(VALIDATE_PLANES, 0);
        m.put_u32(guest::ACTIVE_VIS, objects::MAIN_VIS);
        m.put_u32(objects::MAIN_VIS, regions);
        m.put_u16(regions + COUNT, 1);
        m.put(regions + WINDOWS, &[0x93; SIZE]);
        m.put_u32(stack, camera);
        m.put_u32(stack + 4, 3);
        m.put_u32(guest::FRUSTUM_BOUNDS, bounds);
        m.map(bounds, 24);
        m.put_f32(guest::FRUSTUM_UNIT, 1.);
        m.put_f32(guest::FRUSTUM_NEGATIVE_UNIT, -1.);
        m.put_f32(guest::NORMALIZE_EPSILON, 0.001);
        for off in [0x38, 0x3c, 0x4c, 0x5c] {
            m.put_f32(camera + off, 1.);
        }
        m.put_f32(camera + 0x88, 5.);
        for (i, value) in [-1., 1., -1., 1.].into_iter().enumerate() {
            m.put_f32(rect + 4 * i as u32, value);
        }
        (
            m,
            CpuAbi {
                esp: stack,
                ecx: rect,
                edi: regions + WINDOWS,
                ..CpuAbi::xbox_thread()
            },
        )
    }

    #[test]
    fn compares_record_holes_scratch_and_registers_before_admission() {
        let (mut mem, mut cpu) = scene();
        let p = Window::read(&mem, &cpu).unwrap();
        assert!(p.accepted);
        assert_eq!(p.matches(&mem, &cpu), Ok(false));
        mem.put(p.dest, &p.image);
        mem.put_u32(p.argument_addr, p.dest + 0x98);
        cpu.eax = ((p.dest + 0xa8) & !0xff) | 1;
        cpu.ecx = p.dest + 0x54;
        cpu.edx = p.dest + 0x98;
        assert_eq!(p.matches(&mem, &cpu), Ok(true));
        cpu.edx -= 4;
        assert_eq!(p.matches(&mem, &cpu), Ok(false));
        cpu.edx += 4;
        mem.put_u8(p.dest + 9, 42);
        assert_eq!(p.matches(&mem, &cpu), Ok(false));
    }

    #[test]
    fn changed_float_bits_change_the_record_and_aliasing_declines_before_publication() {
        let (mut m, mut cpu) = scene();
        m.put_f32(cpu.ecx, 0.0);
        let positive = Window::read(&m, &cpu).unwrap();
        m.put_f32(cpu.ecx, -0.0);
        let negative = Window::read(&m, &cpu).unwrap();
        assert_ne!(positive.image, negative.image);
        cpu.ecx = cpu.edi + 16;
        assert!(matches!(Window::read(&m, &cpu), Err(Refusal::Layout)));
    }

    #[test]
    fn empty_window_keeps_callers_registers_and_diagnostic_uses_guest() {
        let (mut m, mut cpu) = scene();
        m.put_f32(cpu.ecx + 4, -1.);
        cpu.eax = 0x785e;
        let p = Window::read(&m, &cpu).unwrap();
        assert!(!p.accepted);
        cpu.eax = 0x7800;
        assert_eq!(p.matches(&m, &cpu), Ok(true));
        m.put_u8(VALIDATE_PLANES, 1);
        assert!(matches!(Window::read(&m, &cpu), Err(Refusal::Unsupported)));
    }

    #[test]
    fn guest_windows_prove_the_arm_and_a_different_slot_falls_it_back() {
        let (mut mem, mut cpu) = scene();
        let p = Window::read(&mem, &cpu).unwrap();
        mem.put(p.dest, &p.image);
        mem.put_u32(p.argument_addr, p.dest + 0x98);
        cpu.eax = ((p.dest + 0xa8) & !0xff) | 1;
        cpu.ecx = p.dest + 0x54;
        cpu.edx = p.dest + 0x98;
        let mut state = State::default();
        let plan = || {
            let (at_call, regs) = scene();
            Window::read(&at_call, &regs).unwrap()
        };
        // Another thread's frame compares nothing.
        state.pending.insert((2, cpu.esp), plan());
        assert_eq!(state.finish(&mem, &cpu, 1), None);
        for _ in 0..Audit::PROOF {
            assert!(!state.audit.answers());
            state.pending.insert((1, cpu.esp), plan());
            assert_eq!(state.finish(&mem, &cpu, 1), Some(true));
        }
        assert!(state.audit.answers());
        mem.put_u8(p.dest + 0x30, 0x55);
        state.pending.insert((1, cpu.esp), plan());
        assert_eq!(state.finish(&mem, &cpu, 1), Some(false));
        assert!(state.audit.fallen_back() && !state.audit.answers());
    }
}
