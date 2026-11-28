//! Reserve native game-state storage for the campaign squad arrays, and
//! size the script pools a converted scenario needs.

use guest_layout::{entry::{Cx, EntryError, GuestCall, Ret}, memory::MemFault};
use engine::campaign_squad_pool as pool;
use hooks::{BuildError, Builder, Flow, Manifest, SnapshotPolicy, points};

const ALLOCATOR: u32 = 0x0051_0c2c;
const INITIAL_HANDLES: [u32; 5] = [0x557c74, 0x557c78, 0x557c7c, 0x557c80, 0x557c84];

pub struct SquadStorage;

impl hooks::Feature for SquadStorage {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "campaign-squad-pools",
            "Reserve native campaign squad storage",
        )
        .doc("docs/halo1/ai-control.md")
    }

    fn build(&self, b: &mut Builder<'_>) -> Result<(), BuildError> {
        b.raw().before(
            &points::AI_SQUADS_ALLOCATE,
            "reserve AI game-state storage",
            |call| call.answer_async(|cx| Box::pin(allocate(cx))),
        );
        type Prepare = fn(&mut dyn guest_layout::GuestMemory) -> Result<(), MemFault>;
        for (point, label, prepare) in [
            (&points::AI_SQUADS_INITIALIZE, "set squad capacity before activation", pool::prepare as Prepare),
            (&points::HS_NEW_MAP, "size the campaign script pools", engine::hs_pool::prepare as Prepare),
        ] {
            b.raw().before(point, label, move |call| {
                let mut guest = call.ctx.runtime().memory();
                prepare(&mut *guest).map_err(|f| runtime::GuestFault {
                    addr: f.addr,
                    len: f.len as usize,
                    access: if f.write {
                        runtime::guest::Access::Write
                    } else {
                        runtime::guest::Access::Read
                    },
                    reason: runtime::guest::FaultReason::Unmapped,
                })?;
                Ok(Flow::Continue)
            });
        }
        // A converted scenario may hold more syntax nodes than retail's
        // bitmap covers (engine::hs_syntax). The arena is guest RAM, so
        // snapshots keep it with the array that points at it.
        let bitmap = b.guest_alloc(
            engine::hs_syntax::BITMAP_BYTES,
            runtime::guest::Permissions::RW,
        )?;
        b.raw().before(
            &points::HS_MAP_ATTACH,
            "size the script syntax bitmap",
            move |call| {
                let mut guest = call.ctx.runtime().memory();
                engine::hs_syntax::prepare(&mut *guest, bitmap).map_err(|f| {
                    runtime::GuestFault {
                        addr: f.addr,
                        len: f.len as usize,
                        access: if f.write {
                            runtime::guest::Access::Write
                        } else {
                            runtime::guest::Access::Read
                        },
                        reason: runtime::guest::FaultReason::Unmapped,
                    }
                })?;
                Ok(Flow::Continue)
            },
        );
        b.snapshot(SnapshotPolicy::Stateless);
        Ok(())
    }
}

async fn allocate(cx: Cx) -> Result<Ret, EntryError> {
    let (allocator, alloc) = cx.write(|m| {
        for address in INITIAL_HANDLES {
            m.set_u32(address, u32::MAX)?;
        }
        let allocator = m.u32(ALLOCATOR)?;
        Ok::<_, MemFault>((allocator, m.u32(m.u32(allocator)?)?))
    })?;
    let squad = cx
        .call(
            GuestCall::new(alloc)
                .ecx(allocator)
                .args([pool::SQUAD_BYTES]),
        )
        .await?
        .eax;
    let group = cx
        .call(
            GuestCall::new(alloc)
                .ecx(allocator)
                .args([pool::GROUP_ALLOCATION_BYTES]),
        )
        .await?
        .eax;
    cx.write(|m| pool::publish(m, squad, group, allocator))?;
    Ok(Ret::new(0, 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::{entry::{EntryFuture, Regs, testing}, testing::FakeGuest};

    #[test]
    fn reserve_calls_the_guest_game_state_allocator_for_both_arrays() {
        let mut guest = FakeGuest::new();
        let allocator = 0x800000;
        let vtable = 0x801000;
        let alloc = 0x810000;
        let squad = 0x820000;
        let group = squad + pool::SQUAD_BYTES;
        guest
            .put_u32(ALLOCATOR, allocator)
            .put_u32(allocator, vtable)
            .put_u32(vtable, alloc)
            .map(INITIAL_HANDLES[0], 20)
            .map(squad, pool::SQUAD_BYTES + pool::GROUP_ALLOCATION_BYTES)
            .put_u32(0x51e9d8, 0)
            .put_u32(0x51e9dc, 0)
            .put(0x4561e4, b"squad group\0squad\0");
        let run = testing::run(
            &|cx| -> EntryFuture { Box::pin(allocate(cx)) },
            guest,
            Regs::default(),
            |call, _| {
                assert_eq!(call.target, alloc);
                assert_eq!(call.ecx, Some(allocator));
                let eax = match call.args.as_slice() {
                    [pool::SQUAD_BYTES] => squad,
                    [pool::GROUP_ALLOCATION_BYTES] => group,
                    _ => panic!("unexpected game-state allocation"),
                };
                Regs {
                    eax,
                    ..Default::default()
                }
            },
        );
        assert_eq!(run.result, Ok(Ret::new(0, 0)));
        assert_eq!(run.calls.len(), 2);
    }
}
