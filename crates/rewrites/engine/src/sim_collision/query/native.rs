//! Entry J as installed: plan the whole query at its entry, then write
//! the result, the flags argument and the mark state it leaves
//! ([`query`]). [`Prediction`] holds the same plan as expected bytes,
//! untouched mark slots included, for the differential tests' comparison
//! with the lifted entry.
use super::*;
use runtime::{GuestFault, TransferAbi};
use sim_collision::Entry;

pub struct Prediction {
    plan: Plan,
    out: u32,
    flags_at: u32,
    globals: Vec<(u32, Vec<u8>)>,
}

/// A planned query and where its writes go.
struct Ready {
    plan: Plan,
    out: u32,
    target: u32,
    outs: super::super::Outputs<2>,
}

fn ready(cpu: &CpuAbi, g: &mut impl CollisionMemory, world: &CollisionWorld) -> Result<Ready, Bail> {
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
    // remain separate from the start, delta and output.
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
    Ok(Ready {
        plan: p,
        out,
        target: outs.target,
        outs,
    })
}

/// Entry J, `0x1697c0`, natively: the result record, the flags argument
/// (J sets its BSP sides and object bits in place), the epochs, marks and
/// cleared active bytes of the floods, instance traces and object visits
/// it ran, `al` the hit, `ret 0x18`. A declined plan returns `None` with
/// its bail recorded; nothing was written.
pub fn query(
    cpu: &mut CpuAbi,
    g: &mut impl CollisionMemory,
    world: &CollisionWorld,
) -> Result<Option<TransferAbi>, GuestFault> {
    let ready = ready(cpu, g, world);
    world.record(Entry::Query, &ready);
    let Ok(r) = ready else {
        return Ok(None);
    };
    g.write_bytes(r.out, &r.plan.record.0, 1)?;
    g.set_u32(cpu.esp.wrapping_add(4), r.plan.flags)?;
    let mut fault = None;
    let _ = r.plan.marks.expected(|at, data| {
        if fault.is_none()
            && let Err(f) = g.write_bytes(at, data, 1)
        {
            fault = Some(f);
        }
        Ok(())
    });
    if let Some(f) = fault {
        return Err(f);
    }
    cpu.eax = (cpu.eax & !0xff) | u32::from(r.plan.record.word(0) != 0);
    Ok(Some(super::super::finish(cpu, r.target, 0x18)))
}

impl Prediction {
    pub fn gather(
        cpu: &CpuAbi,
        g: &mut impl CollisionMemory,
        world: &CollisionWorld,
    ) -> Result<Self, Bail> {
        let Ready { plan: p, out, outs, .. } = ready(cpu, g, world)?;
        let flags = p.flags;
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
