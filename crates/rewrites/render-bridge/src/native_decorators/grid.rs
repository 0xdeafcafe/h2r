//! Compare the Rust octree traversal at the grid producer's return. For
//! an identical input the producer publishes the verified Rust cells and
//! restores the scalar/SIMD state left by the compared call.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use guest_layout::memory::GuestMemory;
use guest_layout::points;
use hooks::{Builder, Flow};
use runtime::CpuAbi;
use runtime::guest::Access;
use scene_extract::GuestRead;
use scene_extract::layout::camera::RENDER_CAMERA;
use scene_extract::layout::decorators::{VISIBLE_CLUSTERS, cells};

const RETURN: u32 = 0x0002_ba0b;
const FLAGS: u32 = 0x8d5; // arithmetic flags set by the final ADD ESP
const MAX_INPUT_WORDS: usize = 65536;

fn overlaps_stack(start: u32, end: u32, stack: u32) -> bool {
    start < stack.saturating_add(12) && end > stack.saturating_sub(0x10000)
}

static COMPARED: AtomicU64 = AtomicU64::new(0);
static DIFFER: AtomicU64 = AtomicU64::new(0);
static NATIVE: AtomicU64 = AtomicU64::new(0);

pub(super) fn counters() -> [(&'static str, u64); 3] {
    [
        (
            "scene_decorator_grid_compared",
            COMPARED.load(Ordering::Relaxed),
        ),
        (
            "scene_decorator_grid_differ",
            DIFFER.load(Ordering::Relaxed),
        ),
        (
            "scene_decorator_grid_native",
            NATIVE.load(Ordering::Relaxed),
        ),
    ]
}

struct Read<'a>(&'a dyn GuestMemory);

impl GuestRead for Read<'_> {
    fn read_u32(&self, address: u32) -> Option<u32> {
        self.0.u32(address).ok()
    }
}

struct Trace<'a> {
    mem: &'a dyn GuestRead,
    stack: u32,
    words: RefCell<BTreeMap<u32, u32>>,
}

impl GuestRead for Trace<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        // The original's local stack and published tables are writable.
        // An aliased tag block would make this an impure traversal.
        let end = addr.checked_add(4)?;
        if (addr < VISIBLE_CLUSTERS + 0x1000 && end > VISIBLE_CLUSTERS)
            || (addr < cells::VISIBLE_COUNT + 4 && end > cells::VISIBLE_COUNT)
            || (overlaps_stack(addr, end, self.stack))
        {
            return None;
        }
        let mut words = self.words.borrow_mut();
        if let Some(&value) = words.get(&addr) {
            return Some(value);
        }
        if words.len() >= MAX_INPUT_WORDS {
            return None;
        }
        let value = self.mem.read_u32(addr)?;
        words.insert(addr, value);
        Some(value)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Call {
    stack: u32,
    block: u32,
    mxcsr: u32,
}

struct Plan {
    call: Call,
    inputs: BTreeMap<u32, u32>,
    cells: Vec<cells::Cell>,
}

impl Plan {
    fn build(mem: &dyn GuestRead, call: Call) -> Option<Self> {
        let trace = Trace {
            mem,
            stack: call.stack,
            words: RefCell::default(),
        };
        let [x, y, z] = [0, 4, 8].map(|offset| trace.read_f32(RENDER_CAMERA + offset));
        let cells = cells::build(&trace, call.block, [x?, y?, z?]).ok()?;
        // The guest can continue visiting children after the table fills.
        // They cannot add a slot but can still affect the return registers.
        if cells.len() == 256 {
            return None;
        }
        Some(Self {
            call,
            inputs: trace.words.into_inner(),
            cells,
        })
    }

    fn reusable(&self, mem: &dyn GuestRead, call: Call) -> bool {
        self.call == call
            && self
                .inputs
                .iter()
                .all(|(&addr, &word)| mem.read_u32(addr) == Some(word))
    }

    fn bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(16 * self.cells.len());
        for cell in &self.cells {
            bytes.extend_from_slice(&cell.index.to_le_bytes());
            for axis in cell.origin {
                bytes.extend_from_slice(&axis.to_le_bytes());
            }
        }
        bytes
    }
}

// The producer overwrites EAX, ECX, EDX and XMM0..2 from its inputs even
// for an empty tree. The remaining registers and non-arithmetic flags
// belong to the caller. ESP is identical to the compared call's at entry.
struct Returns {
    integer: [u32; 3],
    xmm: [[u8; 16]; 3],
    mxcsr: u32,
    flags: u32,
    defined: u32,
}

impl Returns {
    fn capture(cpu: &CpuAbi) -> Self {
        Self {
            integer: [cpu.eax, cpu.ecx, cpu.edx],
            xmm: [cpu.xmm[0], cpu.xmm[1], cpu.xmm[2]],
            mxcsr: cpu.mxcsr,
            flags: cpu.eflags & FLAGS,
            defined: cpu.eflags_defined & FLAGS,
        }
    }

    fn apply(&self, cpu: &mut CpuAbi) {
        [cpu.eax, cpu.ecx, cpu.edx] = self.integer;
        cpu.xmm[..3].copy_from_slice(&self.xmm);
        cpu.mxcsr = self.mxcsr;
        cpu.eflags = (cpu.eflags & !FLAGS) | self.flags;
        cpu.eflags_defined = (cpu.eflags_defined & !FLAGS) | self.defined;
    }
}

#[derive(Default)]
struct State {
    epoch: u64,
    pending: HashMap<(u32, u32), Plan>,
    verified: Option<(Plan, Returns)>,
}

impl State {
    fn scope(&mut self) {
        let epoch = crate::native_map_work::bsp_generation();
        if epoch != self.epoch {
            *self = Self {
                epoch,
                ..Self::default()
            };
        }
    }
}

pub(super) fn install(b: &mut Builder) {
    let state = Arc::new(Mutex::new(State::default()));
    let before = state.clone();
    b.raw()
        .before(&points::DECORATOR_GRID, "build decorator grid", move |c| {
            let runtime = Arc::clone(c.ctx.runtime());
            let mut mem = runtime.memory();
            let stack = c.cpu.esp;
            if c.addr != points::DECORATOR_GRID.base
                || mem.u32(stack).ok() != Some(RETURN)
                || mem.u32(stack.wrapping_add(8)).ok() != Some(RENDER_CAMERA)
                || overlaps_stack(VISIBLE_CLUSTERS, VISIBLE_CLUSTERS + 0x1000, stack)
                || overlaps_stack(cells::VISIBLE_COUNT, cells::VISIBLE_COUNT + 4, stack)
                || c.cpu.mxcsr & !0x3f != 0x1f80
            {
                return Ok(Flow::Continue);
            }
            let Ok(block) = mem.u32(stack.wrapping_add(4)) else {
                return Ok(Flow::Continue);
            };
            let mut state = before.lock().unwrap_or_else(|e| e.into_inner());
            state.scope();
            let key = (c.ctx.thread_id, stack.wrapping_add(12));
            state.pending.remove(&key);
            let call = Call {
                stack,
                block,
                mxcsr: c.cpu.mxcsr,
            };
            if let Some((plan, returns)) = &state.verified
                && plan.reusable(&Read(&*mem), call)
            {
                let bytes = plan.bytes();
                if (!bytes.is_empty()
                    && mem
                        .check(VISIBLE_CLUSTERS, bytes.len(), Access::Write, 1)
                        .is_err())
                    || mem
                        .check(cells::VISIBLE_COUNT, 4, Access::Write, 1)
                        .is_err()
                {
                    return Ok(Flow::Continue);
                }
                let ret = mem.ret(c.cpu, 8)?;
                if !bytes.is_empty() {
                    mem.write_bytes_from(c.addr, VISIBLE_CLUSTERS, &bytes, 1)?;
                }
                mem.write_bytes_from(
                    c.addr,
                    cells::VISIBLE_COUNT,
                    &(plan.cells.len() as u32).to_le_bytes(),
                    1,
                )?;
                returns.apply(c.cpu);
                NATIVE.fetch_add(1, Ordering::Relaxed);
                return Ok(Flow::Return(ret));
            }
            if let Some(plan) = Plan::build(&Read(&*mem), call) {
                state.pending.insert(key, plan);
            }
            Ok(Flow::Continue)
        })
        .optional()
        .context_only();
    b.raw()
        .observe_after(
            &points::DECORATOR_GRID,
            "compare decorator grid",
            move |o| {
                let mem = o.guest();
                let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
                state.scope();
                if let Some(plan) = state.pending.remove(&(o.thread_id, o.cpu.esp)) {
                    COMPARED.fetch_add(1, Ordering::Relaxed);
                    if cells::matches(&Read(&*mem), &plan.cells) == Ok(true) {
                        state.verified = Some((plan, Returns::capture(o.cpu)));
                    } else {
                        state.verified = None;
                        DIFFER.fetch_add(1, Ordering::Relaxed);
                    }
                }
            },
        )
        .optional()
        .context_only();
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::render_loop::occlusion::guest::MAIN_VIS;
    use scene_extract::testing::Mem;

    #[test]
    fn replacing_a_grid_requires_the_same_tree_view_and_float_controls() {
        let mut m = Mem::default();
        let block = 0x006a_0000;
        let frustum = 0x006b_0000;
        let call = Call {
            stack: 0x0080_0000,
            block,
            mxcsr: 0x1f80,
        };
        m.w(MAIN_VIS, frustum);
        m.w(block + 0x20, 1);
        m.w(block + 0x24, 0x006c_0000);
        m.w(block + 0xc, 2);
        for (i, axis) in [1.0, 2.0, 3.0].into_iter().enumerate() {
            m.f(block + 4 * i as u32, axis);
            m.f(RENDER_CAMERA + 4 * i as u32, axis);
        }
        for (i, b) in [-8.0, 8.0, -8.0, 8.0, -8.0, 8.0].into_iter().enumerate() {
            m.f(frustum + 0xf4 + 4 * i as u32, b);
        }
        for i in 0..24 {
            m.f(frustum + 0x10c + 4 * i, 0.0);
        }
        for (addr, v) in [
            (0x0052_5930, 8.0),
            (0x0052_5924, 1.0),
            (0x0045_dbdc, 0.001),
            (0x0045_ddfc, -0.001),
            (0x0043_f9d0, 2.0),
            (0x0045_dc18, 1.0),
            (0x0045_dbbc, 0.5),
        ] {
            m.f(addr, v);
        }
        let plan = Plan::build(&m, call).unwrap();
        let expected: Vec<_> = [0, 1f32.to_bits(), 2f32.to_bits(), 3f32.to_bits()]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect();
        assert_eq!(plan.bytes(), expected);
        assert!(plan.reusable(&m, call));
        assert!(!plan.reusable(
            &m,
            Call {
                mxcsr: 0x1fa0,
                ..call
            }
        ));
        assert!(!plan.reusable(
            &m,
            Call {
                stack: call.stack + 16,
                ..call
            }
        ));
        m.w(0x0050_9438, 6); // A new view overwrites the last output.
        assert!(plan.reusable(&m, call));
        m.f(frustum + 0x110, -0.0);
        assert!(!plan.reusable(&m, call));
        m.f(frustum + 0x110, 0.0);
        m.w(block + 0xc, 4);
        assert!(!plan.reusable(&m, call));
    }

    #[test]
    fn grid_return_preserves_the_callers_unrelated_state() {
        let cpu = CpuAbi {
            eax: 15,
            ecx: 20,
            edx: 25,
            eflags: 0xd6,
            mxcsr: 0x1fa0,
            ..CpuAbi::default()
        };
        let returns = Returns::capture(&cpu);
        let mut next = CpuAbi {
            ebx: 31,
            eflags: 0x202,
            esp: 0x0080_0000,
            ..CpuAbi::default()
        };
        next.xmm[6] = [37; 16];
        returns.apply(&mut next);
        assert_eq!([next.eax, next.ecx, next.edx, next.ebx], [15, 20, 25, 31]);
        assert_eq!(next.eflags, 0x2d6);
        assert_eq!(next.mxcsr, 0x1fa0);
        assert_eq!(next.esp, 0x0080_0000);
        assert_eq!(next.xmm[6], [37; 16]);
    }
}
