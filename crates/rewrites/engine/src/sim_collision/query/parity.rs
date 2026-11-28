//! Plan at J's entry, compare the guest answer after J returns. A plan
//! owns its expected bytes, including the incoming result record's padding.
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

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

/// Limit live return observations. Each can retain a snapshot of all three
/// mark tables. Dropping the runner's callback releases its slot, whether
/// the guest returns normally or unwinds the call.
struct Slot<'a>(&'a AtomicUsize);

impl<'a> Slot<'a> {
    fn reserve(count: &'a AtomicUsize) -> Option<Self> {
        count
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
                (live < 32).then_some(live + 1)
            })
            .ok()
            .map(|_| Self(count))
    }
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

pub struct PendingQuery {
    prediction: Prediction,
    _slot: Slot<'static>,
}

pub fn observe(
    cpu: &CpuAbi,
    g: &mut runtime::Guest,
    world: &CollisionWorld,
) -> Option<PendingQuery> {
    static LIVE: AtomicUsize = AtomicUsize::new(0);
    if g.has_trap() {
        world.record_query(Err(Bail::Trap));
        return None;
    }
    let Some(slot) = Slot::reserve(&LIVE) else {
        world.record_query(Err(Bail::Budget));
        return None;
    };
    match Prediction::gather(cpu, g, world) {
        Ok(prediction) => Some(PendingQuery {
            prediction,
            _slot: slot,
        }),
        Err(why) => {
            world.record_query(Err(why));
            None
        }
    }
}

impl PendingQuery {
    pub fn finish(self, cpu: &CpuAbi, g: &runtime::Guest, world: &CollisionWorld) {
        world.record_query(if g.has_trap() {
            Err(Bail::Trap)
        } else {
            self.prediction.compare(cpu.eax as u8, g)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abandoned_observations_release_their_budget() {
        let live = AtomicUsize::new(0);
        let mut held = Vec::new();
        while let Some(slot) = Slot::reserve(&live) {
            held.push(slot);
        }
        assert_eq!(held.len(), 32);
        held.pop();
        assert!(Slot::reserve(&live).is_some());
        drop(held);
        assert_eq!(live.load(Ordering::Relaxed), 0);
    }
}
