//! Result parity for the vector test and the outer BSP context walk.
use super::{
    Bail, CollisionMemory, CollisionWorld, Plan, Read,
    layout::{context, record},
    plan_context, plan_vector,
};
use runtime::CpuAbi;
use sim_collision::GuestRead;
use std::{cell::RefCell, collections::HashMap};

pub struct Prediction {
    addr: u32,
    al: u8,
    record: Vec<u8>,
    context: Option<(u32, Vec<u8>)>,
}

impl Prediction {
    pub fn gather(
        c: &CpuAbi,
        g: &mut impl CollisionMemory,
        world: &CollisionWorld,
    ) -> Result<Self, Bail> {
        let plan = plan_vector(c, g, world)?;
        Self::from_plan(plan, g)
    }

    pub fn gather_context(
        c: &CpuAbi,
        g: &mut impl CollisionMemory,
        world: &CollisionWorld,
    ) -> Result<Self, Bail> {
        let plan = plan_context(c, g, world)?;
        Self::from_plan(plan, g)
    }

    fn from_plan(plan: Plan, g: &impl CollisionMemory) -> Result<Self, Bail> {
        let context = if let Some(addr) = plan.context {
            let mut bytes = Read(g, (0, 0)).bytes(addr, context::SIZE as usize)?;
            plan.outcome.write_context(|offset, value| {
                let start = offset as usize;
                bytes[start..start + value.len()].copy_from_slice(value);
                Ok::<_, Bail>(())
            })?;
            Some((addr, bytes))
        } else {
            None
        };
        let mut record = Read(g, (0, 0)).bytes(plan.record, record::SIZE as usize)?;
        plan.outcome
            .write_record(plan.init, plan.listed, |offset, bytes| {
                let start = offset as usize;
                record
                    .get_mut(start..start + bytes.len())
                    .ok_or(Bail::Data)?
                    .copy_from_slice(bytes);
                Ok::<_, Bail>(())
            })?;
        Ok(Self {
            addr: plan.record,
            al: u8::from(plan.outcome.hit.is_some()),
            record,
            context,
        })
    }

    pub fn compare(&self, al: u8, g: &impl CollisionMemory) -> Result<Option<String>, Bail> {
        if al != self.al {
            return Ok(Some(format!("AL: expected {}, guest {al}", self.al)));
        }
        let actual = Read(g, (0, 0)).bytes(self.addr, self.record.len())?;
        let mismatch =
            self.record
                .iter()
                .zip(actual)
                .enumerate()
                .find_map(|(i, (&expected, got))| {
                    (expected != got)
                        .then(|| format!("R+{i:#x}: expected {expected:#04x}, guest {got:#04x}"))
                });
        if mismatch.is_some() {
            return Ok(mismatch);
        }
        if let Some((addr, expected)) = &self.context {
            let actual = Read(g, (0, 0)).bytes(*addr, expected.len())?;
            for (i, (&expected, got)) in expected.iter().zip(actual).enumerate() {
                if expected != got {
                    return Ok(Some(format!(
                        "context+{i:#x}: expected {expected:#04x}, guest {got:#04x}"
                    )));
                }
            }
        }
        Ok(None)
    }
}

thread_local! {
    static CONTEXT: RefCell<HashMap<u32, (u32, Option<Prediction>)>> = RefCell::default();
    static OPEN: RefCell<HashMap<u32, Prediction>> = RefCell::default();
}

pub fn begin(thread: u32, c: &CpuAbi, g: &mut runtime::Guest, world: &CollisionWorld) {
    let predicted = if g.has_trap() {
        Err(Bail::Trap)
    } else {
        Prediction::gather(c, g, world)
    };
    OPEN.with_borrow_mut(|calls| {
        calls.remove(&thread);
        match predicted {
            Ok(p) => {
                calls.insert(thread, p);
            }
            Err(why) => world.record_vector(Err(why)),
        }
    });
}

pub fn end(thread: u32, c: &CpuAbi, g: &runtime::Guest, world: &CollisionWorld) {
    if let Some(p) = OPEN.with_borrow_mut(|calls| calls.remove(&thread)) {
        world.record_vector(if g.has_trap() {
            Err(Bail::Trap)
        } else {
            p.compare(c.eax as u8, g)
        });
    }
}

/// The recursive call is already included in the outer kernel walk.
pub fn begin_context(thread: u32, c: &CpuAbi, g: &mut runtime::Guest, world: &CollisionWorld) {
    let return_sp = c.esp.wrapping_add(4 + super::layout::VECTOR_ARGS);
    let trapped = g.has_trap();
    CONTEXT.with_borrow_mut(|calls| {
        let recursive = if trapped {
            // Keep a pending parent when the trap precludes reading the call site.
            calls.get(&thread).is_some_and(|(sp, _)| return_sp < *sp)
        } else {
            Read(&*g, (0, 0)).u32(c.esp) == Ok(super::layout::CONTEXT_RECURSIVE_RETURN)
        };
        if recursive {
            return;
        }
        let result = if trapped {
            Err(Bail::Trap)
        } else {
            Prediction::gather_context(c, g, world)
        };
        let prediction = match result {
            Ok(p) => Some(p),
            Err(why) => {
                world.record_context(Err(why));
                None
            }
        };
        calls.insert(thread, (return_sp, prediction));
    });
}

pub fn end_context(thread: u32, c: &CpuAbi, g: &runtime::Guest, world: &CollisionWorld) {
    CONTEXT.with_borrow_mut(|calls| {
        if calls.get(&thread).is_some_and(|(sp, _)| *sp == c.esp) {
            let Some((_, Some(p))) = calls.remove(&thread) else {
                return;
            };
            world.record_context(if g.has_trap() {
                Err(Bail::Trap)
            } else {
                p.compare(c.eax as u8, g)
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::{Guest, guest::Permissions};
    use sim_core::Epoch;

    #[test]
    fn recursive_entry_and_return_leave_outer_prediction_pending() {
        static EPOCH: Epoch = Epoch::new();
        let world = CollisionWorld::new(&EPOCH);
        let mut g = Guest::new();
        let addr = 0x18000;
        g.map(addr, 0x1000, 0, Permissions::RW).unwrap();
        let mut cpu = CpuAbi {
            esp: addr + 0x800,
            ..CpuAbi::default()
        };
        g.set_u32(cpu.esp, super::super::layout::CONTEXT_RECURSIVE_RETURN)
            .unwrap();
        let outer_return = cpu.esp + 0x80;
        let p = Prediction {
            addr,
            al: 0,
            record: vec![0; record::SIZE as usize],
            context: None,
        };
        CONTEXT.with_borrow_mut(|calls| {
            calls.insert(7, (outer_return, Some(p)));
        });
        begin_context(7, &cpu, &mut g, &world);
        cpu.esp += 4 + super::super::layout::VECTOR_ARGS;
        end_context(7, &cpu, &g, &world);
        assert_eq!(world.stats().context, Default::default());
        cpu.esp = outer_return;
        end_context(7, &cpu, &g, &world);
        assert_eq!(world.stats().context.matched, 1);
    }

    #[test]
    fn reports_the_first_changed_byte_even_in_the_unused_list_tail() {
        static EPOCH: Epoch = Epoch::new();
        let world = CollisionWorld::new(&EPOCH);
        let addr = 0x18000;
        let mut g = Guest::new();
        g.map(addr, 0x1000, 0, Permissions::RW).unwrap();
        let p = Prediction {
            addr,
            al: 0,
            record: vec![0; record::SIZE as usize],
            context: Some((addr + 0x800, vec![0; context::SIZE as usize])),
        };
        world.record_vector(p.compare(0, &g));
        g.set_u8(addr + 0x800 + context::PREV_KIND + 1, 1).unwrap();
        assert_eq!(
            p.compare(0, &g).unwrap().as_deref(),
            Some("context+0x21: expected 0x00, guest 0x01")
        );
        g.set_u8(addr + record::OVERFLOW, 13).unwrap();
        world.record_vector(p.compare(0, &g));
        world.record_vector(p.compare(1, &g));
        let stats = world.stats().vector;
        assert_eq!((stats.matched, stats.mismatched), (1, 2));
        assert_eq!(
            stats.first_mismatch.as_deref(),
            Some("R+0x418: expected 0x00, guest 0x0d")
        );
        assert_eq!(g.u8(addr + record::OVERFLOW).unwrap(), 13);
    }
}
