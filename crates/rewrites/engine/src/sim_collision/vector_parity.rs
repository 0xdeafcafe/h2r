//! Result parity for the vector test and the outer BSP context walk.
use super::{
    Bail, CollisionMemory, CollisionWorld, Plan, Read,
    layout::{context, record},
    plan_context, plan_vector,
};
use runtime::{CpuAbi, Guest, GuestFault, TransferAbi};
use sim_collision::Entry;
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

fn plan(c: &CpuAbi, g: &mut Guest, world: &CollisionWorld, entry: Entry) -> Result<Plan, Bail> {
    if g.has_trap() {
        return Err(Bail::Trap);
    }
    if !crate::sim_native::supported(c) {
        return Err(Bail::Env);
    }
    if entry == Entry::VectorContext {
        plan_context(c, g, world)
    } else {
        plan_vector(c, g, world)
    }
}

/// Commit a fully replayable exit, or keep the prediction for the
/// lifted return. Native answers never feed the parity counters.
pub fn dispatch_vector(
    thread: u32,
    c: &mut CpuAbi,
    g: &mut Guest,
    world: &CollisionWorld,
) -> Result<Option<TransferAbi>, GuestFault> {
    OPEN.with_borrow_mut(|calls| calls.remove(&thread));
    let entry = Entry::VectorTest;
    let predicted = match plan(c, g, world, entry) {
        Ok(p) => {
            if let Some(walk) = p.outcome.straight {
                let ret = super::straight::commit(c, g, &p, walk)?;
                world.record(entry, &Ok(()));
                return Ok(Some(ret));
            }
            Prediction::from_plan(p, g)
        }
        Err(why) => Err(why),
    };
    match predicted {
        Ok(p) => OPEN.with_borrow_mut(|calls| {
            calls.insert(thread, p);
        }),
        Err(why) => {
            world.record::<()>(entry, &Err(why));
            world.record_vector(Err(why));
        }
    }
    Ok(None)
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
pub fn dispatch_context(
    thread: u32,
    c: &mut CpuAbi,
    g: &mut Guest,
    world: &CollisionWorld,
) -> Result<Option<TransferAbi>, GuestFault> {
    let return_sp = c.esp.wrapping_add(4 + super::layout::VECTOR_ARGS);
    let caller = if g.has_trap() {
        None
    } else {
        Read(&*g, (0, 0)).u32(c.esp).ok()
    };
    CONTEXT.with_borrow_mut(|calls| {
        // The recursive near child belongs to the lifted parent's
        // differential. Its return must leave the parent pending.
        if caller == Some(super::layout::CONTEXT_RECURSIVE_RETURN)
            || (caller.is_none() && calls.get(&thread).is_some_and(|(sp, _)| return_sp < *sp))
        {
            return Ok(None);
        }
        calls.remove(&thread);
        let entry = Entry::VectorContext;
        let predicted = match plan(c, g, world, entry) {
            Ok(p) => {
                // C's oracle must also run its D callee lifted.
                let in_vector_oracle = caller == Some(super::straight::WRAPPER_AFTER_WALK)
                    && OPEN.with_borrow(|v| v.contains_key(&thread));
                if let Some(walk) = p.outcome.straight.filter(|_| !in_vector_oracle) {
                    let ret = super::straight::commit(c, g, &p, walk)?;
                    world.record(entry, &Ok(()));
                    return Ok(Some(ret));
                }
                Prediction::from_plan(p, g)
            }
            Err(why) => Err(why),
        };
        let prediction = match predicted {
            Ok(p) => Some(p),
            Err(why) => {
                world.record::<()>(entry, &Err(why));
                world.record_context(Err(why));
                None
            }
        };
        calls.insert(thread, (return_sp, prediction));
        Ok(None)
    })
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
        assert!(
            dispatch_context(7, &mut cpu, &mut g, &world)
                .unwrap()
                .is_none()
        );
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
