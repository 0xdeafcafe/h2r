//! Result parity for the vector test and the outer BSP context walk. A
//! straight walk or a crossing exit is answered natively; the matcher path
//! runs lifted and is compared at its return.
use super::{
    Bail, CollisionMemory, CollisionWorld, Plan, Read,
    crossing::{self, Exit},
    layout::{context, record},
    plan_context, plan_vector,
};
use runtime::{CpuAbi, Guest, GuestFault, TransferAbi};
use sim_collision::Entry;
use sim_collision::GuestRead;

pub struct Prediction {
    entry: Entry,
    addr: u32,
    al: u8,
    record: Vec<u8>,
    context: Option<(u32, Vec<u8>)>,
    /// A crossing walk's whole exit, compared as well as R and the context.
    exit: Option<Exit>,
}

impl Prediction {
    pub fn gather(
        c: &CpuAbi,
        g: &mut impl CollisionMemory,
        world: &CollisionWorld,
    ) -> Result<Self, Bail> {
        let plan = plan_vector(c, g, world)?;
        let exit = Exit::model(c, g, &plan);
        Self::from_plan(plan, g, Entry::VectorTest, exit)
    }

    pub fn gather_context(
        c: &CpuAbi,
        g: &mut impl CollisionMemory,
        world: &CollisionWorld,
    ) -> Result<Self, Bail> {
        let plan = plan_context(c, g, world)?;
        let exit = Exit::model(c, g, &plan);
        Self::from_plan(plan, g, Entry::VectorContext, exit)
    }

    fn from_plan(
        plan: Plan,
        g: &impl CollisionMemory,
        entry: Entry,
        exit: Option<Exit>,
    ) -> Result<Self, Bail> {
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
            entry,
            addr: plan.record,
            al: u8::from(plan.outcome.hit.is_some()),
            record,
            context,
            exit,
        })
    }

    /// The walk crossed a plane and skipped the matcher: the comparison
    /// covers every register and frame byte.
    pub fn has_exit(&self) -> bool {
        self.exit.is_some()
    }

    /// Compare the crossing exit's registers and frame window; `None`
    /// when there is no exit or it matched.
    pub fn compare_exit(
        &self,
        c: &runtime::CpuAbi,
        g: &impl CollisionMemory,
    ) -> Result<Option<String>, Bail> {
        match &self.exit {
            Some(exit) => exit.difference(c, g),
            None => Ok(None),
        }
    }

    /// Compare at the lifted return and count the result towards the
    /// entry's gate.
    pub fn settle(self, c: &runtime::CpuAbi, g: &impl CollisionMemory, world: &CollisionWorld) {
        let result = self
            .compare(c.eax as u8, g)
            .and_then(|d| if d.is_some() { Ok(d) } else { self.compare_exit(c, g) });
        world.record_crossing(self.entry, result, self.exit.is_some());
    }

    pub fn compare(&self, al: u8, g: &impl CollisionMemory) -> Result<Option<String>, Bail> {
        if g.trapped() {
            return Err(Bail::Trap);
        }
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

type Dispatch = Result<(Option<TransferAbi>, Option<Prediction>), GuestFault>;

/// The recursive near child is included in its outer prediction. A D
/// entry called by C runs lifted too, so C's oracle remains independent.
fn gather(
    c: &CpuAbi,
    g: &mut Guest,
    world: &CollisionWorld,
    entry: Entry,
) -> Result<Option<(Plan, bool)>, Bail> {
    if g.has_trap() {
        return Err(Bail::Trap);
    }
    if entry == Entry::VectorContext {
        super::vector_env(c)?;
        let caller = Read(&*g, (0, 0)).u32(c.esp)?;
        if caller == super::layout::CONTEXT_RECURSIVE_RETURN {
            return Ok(None);
        }
        let can_answer = caller != super::straight::WRAPPER_AFTER_WALK;
        Ok(Some((plan_context(c, g, world)?, can_answer)))
    } else {
        Ok(Some((plan_vector(c, g, world)?, true)))
    }
}

fn dispatch(c: &mut CpuAbi, g: &mut Guest, world: &CollisionWorld, entry: Entry) -> Dispatch {
    let predicted = match gather(c, g, world, entry) {
        Ok(None) => return Ok((None, None)),
        Ok(Some((plan, can_answer))) => {
            if let Some(walk) = plan.outcome.straight.filter(|_| can_answer) {
                let ret = super::straight::commit(c, g, &plan, walk)?;
                world.record(entry, &Ok(()));
                return Ok((Some(ret), None));
            }
            // D under the wrapper stays lifted: C's comparison owns it.
            let exit = if can_answer { Exit::model(c, g, &plan) } else { None };
            if let Some(exit) = exit.as_ref() {
                let ret = crossing::commit(c, g, &plan, exit)?;
                world.answered_crossing(entry);
                world.record(entry, &Ok(()));
                return Ok((Some(ret), None));
            }
            Prediction::from_plan(plan, g, entry, exit)
        }
        Err(why) => Err(why),
    };
    match predicted {
        Ok(p) => Ok((None, Some(p))),
        Err(why) => {
            world.record::<()>(entry, &Err(why));
            if entry == Entry::VectorContext {
                world.record_context(Err(why));
            } else {
                world.record_vector(Err(why));
            }
            Ok((None, None))
        }
    }
}

/// Return the exact answer or hand the lifted call's prediction to its
/// caller. A declined plan returns neither.
pub fn dispatch_vector(c: &mut CpuAbi, g: &mut Guest, world: &CollisionWorld) -> Dispatch {
    dispatch(c, g, world, Entry::VectorTest)
}

pub fn dispatch_context(c: &mut CpuAbi, g: &mut Guest, world: &CollisionWorld) -> Dispatch {
    dispatch(c, g, world, Entry::VectorContext)
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::{Guest, guest::Permissions};
    use sim_core::Epoch;

    #[test]
    fn recursive_child_needs_no_prediction_or_context_read() {
        static EPOCH: Epoch = Epoch::new();
        let world = CollisionWorld::new(&EPOCH);
        let mut g = Guest::new();
        let addr = 0x18000;
        g.map(addr, 4, 0, Permissions::RW).unwrap();
        let mut cpu = CpuAbi {
            esp: addr,
            mxcsr: 0x1fa0,
            ..CpuAbi::default()
        };
        g.set_u32(addr, super::super::layout::CONTEXT_RECURSIVE_RETURN)
            .unwrap();
        let (ret, prediction) = dispatch_context(&mut cpu, &mut g, &world).unwrap();
        assert!(ret.is_none() && prediction.is_none());
        assert_eq!(world.stats().context, Default::default());
    }

    #[test]
    fn an_unsupported_environment_declines_before_reading_an_unmapped_return_slot() {
        static EPOCH: Epoch = Epoch::new();
        let world = CollisionWorld::new(&EPOCH);
        let mut g = Guest::new();
        let mut cpu = CpuAbi {
            esp: 0x18000,
            mxcsr: 0,
            ..CpuAbi::default()
        };
        let (ret, prediction) = dispatch_context(&mut cpu, &mut g, &world).unwrap();
        assert!(ret.is_none() && prediction.is_none());
        assert_eq!(
            world.stats().entries[Entry::VectorContext as usize].bails[Bail::Env as usize],
            1
        );
    }

    #[test]
    fn reports_the_first_changed_byte_even_in_the_unused_list_tail() {
        static EPOCH: Epoch = Epoch::new();
        let world = CollisionWorld::new(&EPOCH);
        let addr = 0x18000;
        let mut g = Guest::new();
        g.map(addr, 0x1000, 0, Permissions::RW).unwrap();
        let p = Prediction {
            entry: Entry::VectorContext,
            addr,
            al: 0,
            record: vec![0; record::SIZE as usize],
            context: Some((addr + 0x800, vec![0; context::SIZE as usize])),
            exit: None,
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
