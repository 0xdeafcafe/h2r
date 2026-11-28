//! Crossing exits for C and D: a walk that splits at one or more planes
//! and reaches every leaf without the reference matcher. The exit replays
//! the guest's walk step by step (`0x1de630`, `0x1de6d0`) to rebuild the
//! volatile registers and every frame byte the lifted body leaves. It is
//! compared at the lifted return first and answers only after a clean run
//! (docs/engine/collision-queries.md, "Crossing exits").
use super::{CollisionMemory, Plan, Read, finish, layout, xmm0};
use crate::sim_native::add_flags;
use runtime::{CpuAbi, GuestFault, TransferAbi};
use sim_collision::bsp::segment::{LEAF_LIST, Step, context as ctx};
use sim_collision::bsp::{LEAF, NONE};
use sim_collision::{Bail, GuestRead};
use sim_core::det;
use std::cmp::Ordering;

/// x87 status bits an exit keeps: C0 to C3 hold scratch compare results.
const X87_STATUS_KEPT: u16 = 0xb8ff;
/// EFLAGS bits the final `add esp` defines.
const STATUS_FLAGS: u32 = 0x8d5;
/// Stack bytes below the deepest predicted write the comparison also
/// expects unchanged, so a frame write the replay missed is reported.
const GUARD: u32 = 0x40;
/// The wrapper's context and the walker's four arguments and return
/// address below it.
const WRAPPER_CONTEXT: u32 = 0x28;
const WRAPPER_FRAME: u32 = WRAPPER_CONTEXT + 0x14;

/// The registers and frame bytes a crossing exit leaves, decided before
/// anything is written.
#[derive(Clone, Debug)]
pub struct Exit {
    cpu: CpuAbi,
    /// `(address, value, bytes)` in guest order: frames and context words.
    writes: Vec<(u32, u32, u8)>,
    /// The stack window `[window, entry esp + 0x14)` as the walk leaves it.
    window: u32,
    bytes: Vec<u8>,
}

fn low(r: u32, byte: u32) -> u32 {
    (r & !0xff) | (byte & 0xff)
}

fn lane(v: u32) -> [u8; 16] {
    u128::from(v).to_le_bytes()
}

struct Machine<'s> {
    steps: std::slice::Iter<'s, Step>,
    c: CpuAbi,
    writes: Vec<(u32, u32, u8)>,
    nodes: u32,
    planes: u32,
    leaves: u32,
    ctx: u32,
    record: u32,
    flags: u32,
    delta_at: u32,
    /// R's leaf count as the guest reads it at each leaf.
    count: u32,
}

impl Machine<'_> {
    fn w(&mut self, at: u32, value: u32, len: u8) {
        self.writes.push((at, value, len));
    }

    fn next(&mut self) -> Option<Step> {
        self.steps.next().copied()
    }

    /// One `0x1de6d0` frame entered with ESP at `sp`. Callee-saved
    /// registers come back as they went in.
    fn frame(&mut self, sp: u32, node: u32, t0: u32) -> Option<()> {
        let saved = (self.c.ebx, self.c.ebp, self.c.esi);
        self.c.eax = node;
        self.w(sp.wrapping_sub(12), self.c.ebx, 4);
        self.w(sp.wrapping_sub(16), self.c.ebp, 4);
        self.c.ebp = self.ctx;
        self.w(sp.wrapping_sub(20), self.c.esi, 4);
        let (mut child, mut t0) = (node, t0);
        loop {
            if child & LEAF != 0 {
                self.leaf(sp, child)?;
                break;
            }
            let Step::Plane { node, plane, d, front: [a, b], next } = self.next()? else {
                return None;
            };
            if node != child & 0x7f_ffff {
                return None;
            }
            let (a, b) = (u32::from(a), u32::from(b));
            let c = &mut self.c;
            c.esi = self.nodes.wrapping_add(node.wrapping_mul(8));
            c.edx = self.planes;
            c.eax = low(self.planes.wrapping_add(plane.wrapping_shl(4)), a);
            c.ecx = low(self.delta_at, b);
            c.xmm[0] = [0; 16];
            c.xmm[1] = lane(d.delta);
            c.xmm[2] = lane(d.end);
            c.xmm[3] = lane(d.start);
            self.w(sp.wrapping_add(4), d.distance, 4);
            self.w(sp.wrapping_sub(4), d.delta, 4);
            if a == b {
                self.c.ecx = self.c.esi.wrapping_add(2 * a);
                self.c.eax = next;
                child = next;
                continue;
            }
            let Step::Cross { split, increasing, near } = self.next()? else {
                return None;
            };
            let inc = u32::from(increasing);
            let c = &mut self.c;
            c.ebx = low(c.ebx, inc);
            c.ecx = t0;
            c.edx = low(c.edx, inc ^ 1);
            self.w(sp.wrapping_add(8), split, 4);
            self.w(sp.wrapping_sub(24), split, 4);
            self.c.eax = inc ^ 1;
            self.w(sp.wrapping_sub(28), t0, 4);
            self.c.ecx = self.c.esi.wrapping_add(2 * (inc ^ 1));
            self.c.edx = near;
            self.w(sp.wrapping_sub(32), near, 4);
            self.w(sp.wrapping_sub(36), self.ctx, 4);
            self.w(sp.wrapping_sub(40), layout::CONTEXT_RECURSIVE_RETURN, 4);
            self.frame(sp.wrapping_sub(40), near, t0)?;
            // A replayed walk never hits, so the near call left AL clear.
            self.c.eax = self.record;
            self.c.xmm[0] = lane(split);
            match self.next()? {
                Step::Stop => {
                    self.c.eax &= !0xff;
                    break;
                }
                Step::Far { far } => {
                    self.c.ecx = plane;
                    self.c.edx = self.c.esi.wrapping_add(2 * inc);
                    self.w(self.ctx.wrapping_add(ctx::PLANE), plane, 4);
                    self.c.eax = far;
                    self.w(sp.wrapping_add(12), split, 4);
                    (child, t0) = (far, split);
                }
                _ => return None,
            }
        }
        (self.c.ebx, self.c.ebp, self.c.esi) = saved;
        Some(())
    }

    /// The leaf tail (`0x1de827..`) on a transition that skips the
    /// matcher, through its return.
    fn leaf(&mut self, sp: u32, child: u32) -> Option<()> {
        let Step::Leaf { child: seen, kind, lookup } = self.next()? else {
            return None;
        };
        if seen != child || lookup {
            return None;
        }
        self.c.esi = NONE;
        self.c.ebx = low(self.c.ebx, 3);
        self.w(sp.wrapping_add(16), NONE, 4);
        self.w(sp.wrapping_add(8), 3, 1);
        self.w(sp.wrapping_sub(8), 0, 1);
        let leaf = if child == NONE {
            NONE
        } else {
            let leaf = child & 0x7f_ffff;
            self.c.ecx = self.leaves;
            self.w(sp.wrapping_add(16), leaf, 4);
            self.c.esi = leaf;
            self.c.eax = low(leaf, u32::from(kind));
            self.w(sp.wrapping_add(8), u32::from(kind), 1);
            self.c.ebx = low(self.c.ebx, u32::from(kind));
            leaf
        };
        self.c.ecx = self.flags;
        self.c.edx = self.flags & 1;
        self.w(sp.wrapping_sub(24), self.c.edi, 4);
        self.w(sp.wrapping_sub(4), self.flags, 4);
        if leaf == NONE {
            // EAX still holds the solid child word.
            self.c.eax = NONE & !0xff;
        } else {
            self.c.ecx = self.count;
            if (self.count as i32) < LEAF_LIST as i32 {
                self.count += 1;
            }
            self.c.eax = self.record & !0xff;
        }
        self.w(self.ctx.wrapping_add(ctx::PREV_LEAF), leaf, 4);
        self.w(self.ctx.wrapping_add(ctx::PREV_KIND), u32::from(kind), 1);
        Some(())
    }
}

impl Exit {
    /// The exit for `plan`, entered with `entry`, or `None` when the walk
    /// is straight, hits, or runs the matcher at any leaf.
    pub(super) fn model(entry: &CpuAbi, g: &impl CollisionMemory, plan: &Plan) -> Option<Self> {
        let o = &plan.outcome;
        if o.straight.is_some() || o.hit.is_some() {
            return None;
        }
        if o.steps.iter().any(|s| matches!(s, Step::Leaf { lookup: true, .. })) {
            return None;
        }
        let m = Read(g, (0, 0));
        let block = |i: u32| m.u32(plan.bsp.wrapping_add(8 * i + 4)).ok();
        let sp = entry.esp;
        let mut machine = Machine {
            steps: o.steps.iter(),
            c: *entry,
            writes: Vec::with_capacity(16 * o.steps.len() + 24),
            nodes: block(0)?,
            planes: block(1)?,
            leaves: block(2)?,
            ctx: plan.context.unwrap_or(sp.wrapping_sub(WRAPPER_CONTEXT)),
            record: plan.record,
            flags: plan.flags,
            delta_at: plan.delta_at,
            count: plan.listed,
        };
        let (dead_below, outer) = match plan.context {
            Some(_) => {
                let node = m.u32(sp.wrapping_add(8)).ok()?;
                let t0 = m.u32(sp.wrapping_add(12)).ok()?;
                machine.frame(sp, node, t0)?;
                (layout::CONTEXT_DEAD_BELOW, 8)
            }
            None => {
                let t1 = wrapper_t1(entry, &m, plan.init?)?;
                let context = machine.ctx;
                let words = [
                    (ctx::FLAGS, entry.ecx, 4),
                    (ctx::BSP, entry.edx, 4),
                    (ctx::BREAKABLE_COUNT, u32::from(m.u16(sp.wrapping_add(4)).ok()?), 2),
                    (ctx::MASK, m.u32(sp.wrapping_add(8)).ok()?, 4),
                    (ctx::START, plan.start_at, 4),
                    (ctx::DELTA, plan.delta_at, 4),
                    (ctx::RECORD, plan.record, 4),
                    (ctx::PREV_LEAF, NONE, 4),
                    (ctx::PREV_KIND, 0, 1),
                    (ctx::PLANE, NONE, 4),
                ];
                for (offset, value, len) in words {
                    machine.w(context.wrapping_add(offset), value, len);
                }
                machine.w(sp.wrapping_add(4), t1, 4);
                let frame = sp.wrapping_sub(WRAPPER_FRAME);
                for (k, value) in [super::straight::WRAPPER_AFTER_WALK, context, 0, 0, t1]
                    .into_iter()
                    .enumerate()
                {
                    machine.w(frame.wrapping_add(4 * k as u32), value, 4);
                }
                machine.frame(frame, 0, 0)?;
                (layout::VECTOR_DEAD_BELOW, WRAPPER_CONTEXT)
            }
        };
        if machine.steps.next().is_some() {
            return None;
        }
        let mut cpu = machine.c;
        if o.inexact {
            cpu.x87.status |= 0x20;
        }
        add_flags(&mut cpu, sp.wrapping_sub(outer), outer);
        let _ = finish(&mut cpu, plan.target, layout::VECTOR_ARGS);
        // The frame window: every predicted stack byte, plus a guard below.
        let top = sp.wrapping_add(4 + layout::VECTOR_ARGS);
        let in_frame = |at: u32| sp.wrapping_sub(dead_below) <= at && at < top;
        let deepest = machine
            .writes
            .iter()
            .filter(|w| in_frame(w.0))
            .map(|w| w.0)
            .min()?;
        let window = deepest.saturating_sub(GUARD).max(sp.wrapping_sub(dead_below));
        let mut bytes = m.bytes(window, (top - window) as usize).ok()?;
        for &(at, value, len) in &machine.writes {
            if in_frame(at) {
                let start = (at - window) as usize;
                bytes[start..start + usize::from(len)]
                    .copy_from_slice(&value.to_le_bytes()[..usize::from(len)]);
            }
        }
        Some(Self { cpu, writes: machine.writes, window, bytes })
    }

    /// Compare the registers and frame window at the lifted return.
    pub fn difference(&self, c: &CpuAbi, g: &impl CollisionMemory) -> Result<Option<String>, Bail> {
        let want = &self.cpu;
        macro_rules! check {
            ($name:expr, $want:expr, $got:expr) => {
                if $want != $got {
                    return Ok(Some(format!("{}: expected {:#x}, guest {:#x}", $name, $want, $got)));
                }
            };
        }
        check!("EAX", want.eax, c.eax);
        check!("ECX", want.ecx, c.ecx);
        check!("EDX", want.edx, c.edx);
        check!("EBX", want.ebx, c.ebx);
        check!("ESP", want.esp, c.esp);
        check!("EBP", want.ebp, c.ebp);
        check!("ESI", want.esi, c.esi);
        check!("EDI", want.edi, c.edi);
        let both = want.eflags_defined & c.eflags_defined;
        check!("EFLAGS", want.eflags & both, c.eflags & both);
        check!(
            "defined flags",
            want.eflags_defined & STATUS_FLAGS,
            c.eflags_defined & STATUS_FLAGS
        );
        check!("MXCSR", want.mxcsr, c.mxcsr);
        for (i, (w, got)) in want.xmm.iter().zip(&c.xmm).enumerate() {
            check!(format!("XMM{i}"), u128::from_le_bytes(*w), u128::from_le_bytes(*got));
        }
        check!("x87 control", want.x87.control, c.x87.control);
        check!(
            "x87 status",
            want.x87.status & X87_STATUS_KEPT,
            c.x87.status & X87_STATUS_KEPT
        );
        check!("x87 tag", want.x87.tag, c.x87.tag);
        for slot in 0..8 {
            if c.x87.tag >> (2 * slot) & 3 != 3 && want.x87.st[slot] != c.x87.st[slot] {
                return Ok(Some(format!("x87 register {slot}")));
            }
        }
        let actual = Read(g, (0, 0)).bytes(self.window, self.bytes.len())?;
        Ok(self
            .bytes
            .iter()
            .zip(actual)
            .enumerate()
            .find_map(|(i, (&w, got))| {
                (w != got).then(|| {
                    format!(
                        "stack {:#010x}: expected {w:#04x}, guest {got:#04x}",
                        self.window + i as u32
                    )
                })
            }))
    }
}

/// The wrapper's walk end, as `0x1de67a..0x1de6ac` stores it at `[esp+4]`.
fn wrapper_t1(entry: &CpuAbi, m: &impl GuestRead, fraction: u32) -> Option<u32> {
    if det::compare(0, xmm0(entry)) == Ordering::Greater {
        return Some(0);
    }
    let one = m.u32(layout::ONE).ok()?;
    Some(if det::compare(fraction, one) == Ordering::Greater {
        one
    } else {
        fraction
    })
}

/// Write the exit: frames and context words in guest order, R and the
/// outer context, then the registers. The plan validated every range.
pub(super) fn commit(
    c: &mut CpuAbi,
    g: &mut impl CollisionMemory,
    plan: &Plan,
    exit: &Exit,
) -> Result<TransferAbi, GuestFault> {
    for &(at, value, len) in &exit.writes {
        g.write_bytes(at, &value.to_le_bytes()[..usize::from(len)], 1)?;
    }
    plan.outcome.write_record(plan.init, plan.listed, |offset, bytes| {
        g.write_bytes(plan.record.wrapping_add(offset), bytes, 1)
    })?;
    if let Some(context) = plan.context {
        plan.outcome
            .write_context(|offset, bytes| g.write_bytes(context.wrapping_add(offset), bytes, 1))?;
    }
    *c = exit.cpu;
    Ok(TransferAbi::ret(plan.target))
}

/// The crossing path alone, for the synthetic differential and the disc
/// sweep: answers whenever an exit exists, with no gate.
#[cfg(feature = "differential")]
pub fn vector_test(
    c: &mut CpuAbi,
    g: &mut impl CollisionMemory,
    world: &super::CollisionWorld,
) -> Result<Option<TransferAbi>, GuestFault> {
    let Ok(plan) = super::plan_vector(c, g, world) else {
        return Ok(None);
    };
    let exit = Exit::model(c, g, &plan);
    exit.map(|exit| commit(c, g, &plan, &exit)).transpose()
}

#[cfg(feature = "differential")]
pub fn vector_context(
    c: &mut CpuAbi,
    g: &mut impl CollisionMemory,
    world: &super::CollisionWorld,
) -> Result<Option<TransferAbi>, GuestFault> {
    let Ok(plan) = super::plan_context(c, g, world) else {
        return Ok(None);
    };
    let exit = Exit::model(c, g, &plan);
    exit.map(|exit| commit(c, g, &plan, &exit)).transpose()
}

#[cfg(test)]
mod tests {
    use super::super::{CollisionWorld, layout, vector_parity};
    use runtime::{CpuAbi, Guest, guest::Permissions};
    use sim_collision::synth::{self, Builder};
    use sim_collision::Entry;
    use sim_core::Epoch;

    const DATA: u32 = 0x10_000;
    const BSP: u32 = DATA;
    const RECORD: u32 = DATA + 0x1000;
    const START: u32 = DATA + 0x3000;
    const DELTA: u32 = START + 16;
    const SP: u32 = 0x30_000;
    const RETURN: u32 = 0xabcd_ef00;

    /// Two planes, three empty leaves, one segment across both.
    fn fixture() -> (Guest, CpuAbi) {
        let mut g = Guest::new();
        g.map(DATA, 0x4000, 0, Permissions::RW).unwrap();
        g.map(layout::PROJECTION, 36, 0x4000, Permissions::RW).unwrap();
        g.map(layout::ONE, 0x40, 0x5000, Permissions::RW).unwrap();
        g.map(SP - 0x4000, 0x4020, 0x10_000, Permissions::RW).unwrap();
        g.set_u32(layout::ONE, 1.0f32.to_bits()).unwrap();
        let mut tree = Builder::default();
        tree.planes.extend([[1.0, 0.0, 0.0, 0.0], [1.0, 0.0, 0.0, 1.0]]);
        tree.nodes.extend([
            (0, synth::leaf(0), synth::node(1)),
            (1, synth::leaf(1), synth::leaf(2)),
        ]);
        tree.leaves.extend([(0, 0, 0), (0, 0, 0), (0, 0, 0)]);
        tree.write(BSP, |a, b| g.write_bytes(a, b, 1).unwrap());
        for (at, v) in [(START, [-0.5f32, 0.0, 0.0]), (DELTA, [2.0, 0.0, 0.0])] {
            for (k, x) in v.into_iter().enumerate() {
                g.set_u32(at + 4 * k as u32, x.to_bits()).unwrap();
            }
        }
        for (k, word) in [RETURN, 0, 0, START, DELTA].into_iter().enumerate() {
            g.set_u32(SP + 4 * k as u32, word).unwrap();
        }
        let mut c = CpuAbi {
            eax: RECORD,
            ecx: 0,
            edx: BSP,
            ebx: 0x1111_2222,
            esi: 0x3333_4444,
            edi: 0x5555_6666,
            ebp: 0x7777_8888,
            esp: SP,
            mxcsr: 0x1fa0,
            ..CpuAbi::default()
        };
        c.xmm[0][..4].copy_from_slice(&1.0f32.to_le_bytes());
        (g, c)
    }

    #[test]
    fn a_crossing_exit_is_answered_at_once_with_the_exit_the_model_predicts() {
        static EPOCH: Epoch = Epoch::new();
        let world = CollisionWorld::new(&EPOCH);
        // The oracle: the model's prediction on an untouched fixture.
        let (mut fresh, entry) = fixture();
        let prediction = vector_parity::Prediction::gather(&entry, &mut fresh, &world).unwrap();
        assert!(prediction.has_exit());
        let (mut g, _) = fixture();
        let mut c = entry;
        let (ret, none) = vector_parity::dispatch_vector(&mut c, &mut g, &world).unwrap();
        assert!(none.is_none(), "the first call is answered, not shadowed");
        assert_eq!(ret.unwrap().target_pc, RETURN);
        assert_eq!((c.esp, c.eax & 0xff), (SP + 0x14, 0));
        assert_eq!((c.ebx, c.esi, c.edi, c.ebp), (entry.ebx, entry.esi, entry.edi, entry.ebp));
        assert_eq!(g.u32(RECORD + layout::record::COUNT).unwrap(), 3);
        assert_eq!(prediction.compare(c.eax as u8, &g).unwrap(), None);
        assert_eq!(prediction.compare_exit(&c, &g).unwrap(), None);
        assert_eq!(world.stats().vector.answered, 1);
        assert_eq!(world.stats().entries[Entry::VectorTest as usize].native, 1);
        // The exit comparison sees a register left differently.
        let mut other = c;
        other.ecx ^= 1;
        assert!(prediction.compare_exit(&other, &g).unwrap().unwrap().starts_with("ECX"));
    }
}
