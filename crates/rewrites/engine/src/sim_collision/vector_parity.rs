//! The installed vector test (C) and outer BSP context walk (D). A straight
//! walk or a crossing exit replays its frame; the matcher path commits R,
//! the context and AL ([`super::commit`]). [`Prediction`] is the same plan
//! as expected bytes, the differential tests' oracle against the lifted
//! entry.
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
        _entry: Entry,
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

type Dispatch = Result<Option<TransferAbi>, GuestFault>;

/// The plan, and whether the frame-exact exits (straight and crossing)
/// may answer: not for D under C's wrapper, whose frame is the wrapper's.
fn gather(
    c: &CpuAbi,
    g: &mut Guest,
    world: &CollisionWorld,
    entry: Entry,
) -> Result<(Plan, bool), Bail> {
    if g.has_trap() {
        return Err(Bail::Trap);
    }
    if entry == Entry::VectorContext {
        super::vector_env(c)?;
        let caller = Read(&*g, (0, 0)).u32(c.esp)?;
        let exact = caller != super::straight::WRAPPER_AFTER_WALK
            && caller != super::layout::CONTEXT_RECURSIVE_RETURN;
        Ok((plan_context(c, g, world)?, exact))
    } else {
        Ok((plan_vector(c, g, world)?, true))
    }
}

fn dispatch(c: &mut CpuAbi, g: &mut Guest, world: &CollisionWorld, entry: Entry) -> Dispatch {
    let (plan, exact) = match gather(c, g, world, entry) {
        Ok(p) => p,
        Err(why) => {
            world.record::<()>(entry, &Err(why));
            return Ok(None);
        }
    };
    world.record(entry, &Ok(()));
    if exact {
        if let Some(walk) = plan.outcome.straight {
            return super::straight::commit(c, g, &plan, walk).map(Some);
        }
        if let Some(exit) = Exit::model(c, g, &plan) {
            let ret = crossing::commit(c, g, &plan, &exit)?;
            world.answered_crossing(entry);
            return Ok(Some(ret));
        }
    }
    // The matcher path (and D under C's wrapper or its own recursion):
    // R, the context, AL, the PE flags and the epilogue's flags.
    super::commit(c, g, plan).map(Some)
}

/// Entry C as installed: every admitted call is answered in Rust. A
/// declined plan returns `None` with its bail recorded.
pub fn dispatch_vector(c: &mut CpuAbi, g: &mut Guest, world: &CollisionWorld) -> Dispatch {
    dispatch(c, g, world, Entry::VectorTest)
}

/// Entry D as installed; see [`dispatch_vector`].
pub fn dispatch_context(c: &mut CpuAbi, g: &mut Guest, world: &CollisionWorld) -> Dispatch {
    dispatch(c, g, world, Entry::VectorContext)
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::{Guest, guest::Permissions};
    use sim_core::Epoch;

    /// D's own recursion is planned like any other call, never left to
    /// the guest: here its unmapped arguments decline it by name.
    #[test]
    fn a_recursive_child_is_planned_not_run_lifted() {
        static EPOCH: Epoch = Epoch::new();
        let world = CollisionWorld::new(&EPOCH);
        let mut g = Guest::new();
        let addr = 0x18000;
        g.map(addr, 4, 0, Permissions::RW).unwrap();
        let mut cpu = CpuAbi {
            esp: addr,
            mxcsr: 0x1f80,
            ..CpuAbi::default()
        };
        cpu.x87.control = 0x027f;
        cpu.x87.tag = 0xffff;
        g.set_u32(addr, super::super::layout::CONTEXT_RECURSIVE_RETURN)
            .unwrap();
        let _ = sim_collision::take_bail();
        assert!(dispatch_context(&mut cpu, &mut g, &world).unwrap().is_none());
        assert_eq!(sim_collision::take_bail(), Some((Entry::VectorContext, Bail::Fault)));
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
        assert!(dispatch_context(&mut cpu, &mut g, &world).unwrap().is_none());
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
