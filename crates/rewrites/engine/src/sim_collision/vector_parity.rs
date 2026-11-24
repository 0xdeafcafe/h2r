//! Result parity for entry C. The recursive callee D never re-enters C.
use super::{Bail, CollisionMemory, CollisionWorld, Read, layout::record, plan_vector};
use runtime::CpuAbi;
use sim_collision::GuestRead;
use std::{cell::RefCell, collections::HashMap};

pub struct Prediction {
    addr: u32,
    al: u8,
    record: Vec<u8>,
}

impl Prediction {
    pub fn gather(
        c: &CpuAbi,
        g: &mut impl CollisionMemory,
        world: &CollisionWorld,
    ) -> Result<Self, Bail> {
        let plan = plan_vector(c, g, world)?;
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
        })
    }

    pub fn compare(&self, al: u8, g: &impl CollisionMemory) -> Result<Option<String>, Bail> {
        if al != self.al {
            return Ok(Some(format!("AL: expected {}, guest {al}", self.al)));
        }
        let actual = Read(g, (0, 0)).bytes(self.addr, self.record.len())?;
        Ok(self
            .record
            .iter()
            .zip(actual)
            .enumerate()
            .find_map(|(i, (&expected, got))| {
                (expected != got)
                    .then(|| format!("R+{i:#x}: expected {expected:#04x}, guest {got:#04x}"))
            }))
    }
}

thread_local! {
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

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::{Guest, guest::Permissions};
    use sim_core::Epoch;

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
        };
        world.record_vector(p.compare(0, &g));
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
