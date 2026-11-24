//! Plan at J's entry, compare the guest answer after J returns. A plan
//! owns its expected bytes, including the incoming result record's padding.
use super::*;
use std::{cell::RefCell, collections::HashMap};

pub struct Prediction {
    plan: Plan,
    out: u32,
    flags_at: u32,
    globals: Vec<(u32, Vec<u8>)>,
}

impl Prediction {
    pub fn gather(
        cpu: &CpuAbi,
        g: &mut impl CollisionMemory,
        world: &CollisionWorld,
    ) -> Result<Self, Bail> {
        if g.trapped() {
            return Err(Bail::Trap);
        }
        let m = Read(&*g, (0, 0));
        let arg = |i: u32| m.u32(cpu.esp.wrapping_add(4 * i));
        let flags = arg(1)?;
        let (start, delta) = (arg(2)?, arg(3)?);
        let ignore = [arg(4)?, arg(5)?];
        let out = arg(6)?;
        // The caller can pass a live output on its own stack. The frame J
        // reserves below its entry, and the consumed arguments above, must
        // remain separate from the start, delta and output for a prediction.
        let outs = super::super::Outputs::new(
            g,
            cpu.esp,
            0x454,
            [(cpu.esp.wrapping_add(4), 0x18), (out, RECORD_SIZE as u32)],
        )?;
        outs.separate(g, start, 12)?;
        outs.separate(g, delta, 12)?;
        let ray = Ray {
            start: m.vec3(start)?,
            delta: m.vec3(delta)?,
            flags,
            ignore,
        };
        let mut initial = Record([0; RECORD_SIZE]);
        m.read(out, &mut initial.0)?;
        let mut guard = guards::Guard::new(g, cpu.esp, out, flags)?;
        guard.separate(start, 12)?;
        guard.separate(delta, 12)?;
        let p = plan(cpu, &mut guard, world, ray, initial);
        if guard.alias() {
            return Err(Bail::Alias);
        }
        let p = p?;
        p.marks
            .expected(|at, v| outs.separate(g, at, v.len() as u32))?;
        // Compare untouched slots too. The cluster table ends at the
        // instance active byte. The instance table ends at PLAYERS_GLOBALS.
        // The visit kernel bounds the object table at 0x800 slots.
        let mut globals = Vec::with_capacity(3);
        for (mask, base, end) in [
            (
                14,
                layout::FLOOD_ACTIVE,
                layout::CLUSTER_MARKS + 4 * sim_collision::flood::MAX_CLUSTERS,
            ),
            (
                4,
                clusters::INSTANCE_ACTIVE,
                clusters::INSTANCE_MARKS + 4 * super::super::instance::MARK_SLOTS,
            ),
            (
                8,
                layout::VISIT_ACTIVE,
                layout::OBJECT_MARKS + 4 * layout::OBJECT_MARK_SLOTS,
            ),
        ] {
            if flags & mask == 0 {
                continue;
            }
            outs.separate(g, base, end - base)?;
            globals.push((base, Read(&*g, (0, 0)).bytes(base, (end - base) as usize)?));
        }
        p.marks.expected(|at, data| {
            for (base, bytes) in &mut globals {
                if let Some(offset) = at.checked_sub(*base)
                    && let Some(dst) = bytes
                        .get_mut(offset as usize..)
                        .and_then(|tail| tail.get_mut(..data.len()))
                {
                    dst.copy_from_slice(data);
                    return Ok(());
                }
            }
            Err(Bail::Data)
        })?;
        Ok(Self {
            globals,
            plan: p,
            out,
            flags_at: cpu.esp.wrapping_add(4),
        })
    }

    pub fn compare(&self, al: u8, g: &impl CollisionMemory) -> Result<Option<String>, Bail> {
        let m = Read(g, (0, 0));
        let expected_al = u8::from(self.plan.record.word(0) != 0);
        if al != expected_al {
            return Ok(Some(format!("AL: expected {expected_al}, guest {al}")));
        }
        let flags = m.u32(self.flags_at)?;
        if flags != self.plan.flags {
            return Ok(Some(format!(
                "flags: expected {:#010x}, guest {flags:#010x}",
                self.plan.flags
            )));
        }
        let mut actual = [0; RECORD_SIZE];
        m.read(self.out, &mut actual)?;
        if let Some(i) = actual
            .iter()
            .zip(&self.plan.record.0)
            .position(|(a, b)| a != b)
        {
            return Ok(Some(format!(
                "result+{i:#x}: expected {:#04x}, guest {:#04x}",
                self.plan.record.0[i], actual[i]
            )));
        }
        for (base, expected) in &self.globals {
            let actual = m.bytes(*base, expected.len())?;
            if let Some(i) = actual.iter().zip(expected).position(|(a, b)| a != b) {
                return Ok(Some(format!(
                    "mark state at {:#x}: expected {:#04x}, guest {:#04x}",
                    base + i as u32,
                    expected[i],
                    actual[i]
                )));
            }
        }
        Ok(None)
    }
}

/// A reverted call may never reach its after hook. The map epoch and
/// backing RAM generation exclude abandoned predictions, including across
/// an image restart that reuses a thread id and stack address.
#[derive(PartialEq, Eq)]
struct Generation(u64, (u64, u64));

fn generation(g: &runtime::Guest) -> Generation {
    Generation(super::super::EPOCH.get(), g.write_watch_epoch())
}

struct Stack {
    generation: Generation,
    calls: Vec<(u32, Prediction)>,
}

thread_local! {
    static OPEN: RefCell<HashMap<u32,Stack>> = RefCell::default();
}

pub fn begin(thread: u32, cpu: &CpuAbi, g: &mut runtime::Guest, world: &CollisionWorld) -> bool {
    let ret_sp = cpu.esp.wrapping_add(4 + 0x18);
    let current_generation = generation(g);
    OPEN.with_borrow_mut(|threads| {
        threads.retain(|_, s| s.generation == current_generation);
        if let Some(s) = threads.get_mut(&thread) {
            s.calls.retain(|(outer, _)| *outer > ret_sp);
            if s.calls.len() >= 8 {
                world.record_query(Err(Bail::Depth));
                return false;
            }
        } else if threads.len() >= 32 {
            world.record_query(Err(Bail::Budget));
            return false;
        }
        let predicted = if g.has_trap() {
            Err(Bail::Trap)
        } else {
            Prediction::gather(cpu, g, world)
        };
        match predicted {
            Ok(p) => {
                threads
                    .entry(thread)
                    .or_insert_with(|| Stack {
                        generation: current_generation,
                        calls: Vec::new(),
                    })
                    .calls
                    .push((ret_sp, p));
                true
            }
            Err(why) => {
                world.record_query(Err(why));
                if threads.get(&thread).is_some_and(|s| s.calls.is_empty()) {
                    threads.remove(&thread);
                }
                false
            }
        }
    })
}

pub fn end(thread: u32, cpu: &CpuAbi, g: &runtime::Guest, world: &CollisionWorld) {
    OPEN.with_borrow_mut(|threads| {
        let Some(s) = threads.get_mut(&thread) else {
            return;
        };
        if s.generation != generation(g) {
            threads.remove(&thread);
            return;
        }
        let Some(&(sp, _)) = s.calls.last() else {
            return;
        };
        if sp != cpu.esp {
            return;
        }
        let (_, p) = s.calls.pop().unwrap();
        if s.calls.is_empty() {
            threads.remove(&thread);
        }
        world.record_query(if g.has_trap() {
            Err(Bail::Trap)
        } else {
            p.compare(cpu.eax as u8, g)
        });
    });
}
