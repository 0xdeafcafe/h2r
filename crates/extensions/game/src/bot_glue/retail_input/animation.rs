//! Animation cache misses in the private action builder. Only a read of an
//! existing live graph can be repeated on the scheduled game thread.
use guest_layout::{
    DatumHandle,
    entry::GuestCall,
    memory::GuestMemory,
    tags::{GroupTag, tag_get_checked},
};
use runtime::{Guest, GuestFault, Runtime, TransferAbi};
use std::sync::{Arc, Mutex};

const RESOURCE_GET: u32 = 0x1237e0;
const ANIMATION_DATA_RETURN: u32 = 0x1dd830;
const CACHE_ENTRIES: u32 = 0x4e3b48;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct AnimationRead {
    pub record: u32,
    animation: u32,
    owner: DatumHandle,
}

pub(super) type ColdRead = Arc<Mutex<Option<AnimationRead>>>;

// The resident branch of 0x1237e0 uses the low index of the record's
// handle to find a 16-byte entry. At 0x1237a0 only an outstanding job
// without its completion byte needs the worker.
fn needs_worker(m: &Guest, record: u32) -> Result<bool, GuestFault> {
    let handle = m.u32(record.wrapping_add(12))?;
    if handle == u32::MAX {
        return Ok(true);
    }
    let array = m.u32(CACHE_ENTRIES)?;
    let entries = m.u32(array.wrapping_add(0x44))?;
    let entry = entries.wrapping_add((handle & 0xffff) * 16);
    Ok(m.u32(entry.wrapping_add(8))? != u32::MAX && m.u8(entry.wrapping_add(4))? == 0)
}

pub(super) fn intercept_cold_reads(rt: &Runtime) -> ColdRead {
    let cold = ColdRead::default();
    if let Some(original) = rt.resolve_original(RESOURCE_GET) {
        let signal = Arc::clone(&cold);
        rt.edit_dispatch(|d| {
            d.register_override(
                RESOURCE_GET,
                "phantom: resident animation data",
                move |cpu, ctx, selector| {
                    let read = {
                        let m = ctx.memory();
                        if m.u32(cpu.esp)? == ANIMATION_DATA_RETURN && needs_worker(&m, cpu.eax)? {
                            Some(AnimationRead {
                                record: cpu.eax,
                                animation: cpu.esi,
                                owner: DatumHandle(m.u32(cpu.eax)?),
                            })
                        } else {
                            None
                        }
                    };
                    if let Some(read) = read {
                        *signal.lock().unwrap_or_else(|e| e.into_inner()) = Some(read);
                        return Ok(TransferAbi::stop(0));
                    }
                    Ok(original.invoke(cpu, ctx, selector))
                },
            );
        });
    }
    cold
}

impl AnimationRead {
    /// Reconstruct both arguments from the live jmad. A stale owner, an
    /// animation outside the graph, or a different resource gets no live call.
    pub(super) fn live_call(self, m: &dyn GuestMemory) -> Option<GuestCall> {
        let graph = tag_get_checked(m, self.owner, GroupTag::from_chars(*b"jmad")).ok()??;
        let animations = m.u32(graph.checked_add(0x30)?).ok()?;
        let offset = self.animation.checked_sub(animations)?;
        if animations == 0
            || !offset.is_multiple_of(0x6c)
            || offset / 0x6c >= m.u32(graph.checked_add(0x2c)?).ok()?
            || m.u16(self.animation.checked_add(0x16)?).ok()? & 0x10 == 0
        {
            return None;
        }
        let index = m.u32(self.animation.checked_add(0x24)?).ok()?;
        let resources = m.u32(graph.checked_add(0xb0)?).ok()?;
        if resources == 0 || index >= m.u32(graph.checked_add(0xac)?).ok()? {
            return None;
        }
        let record = resources.checked_add(index.checked_mul(0x14)?)?;
        if record != self.record || m.u32(record).ok()? != self.owner.raw() {
            return None;
        }
        Some(
            GuestCall::new(RESOURCE_GET)
                .eax(record)
                .args([m.u32(self.animation).ok()?]),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot_glue::retail_input::{ACTION_SIZE, Path, Stage, gather_with};
    use bots::phantom::SeatInput;
    use guest_layout::{entry::{self, Regs, Ret}, tags, testing::FakeGuest};
    use runtime::{CpuAbi, Dispatch, guest::Permissions};

    const GRAPH: u32 = 0x552000;
    const ANIM: u32 = GRAPH + 0x100;
    const RECORD: u32 = GRAPH + 0x200;
    const OWNER: DatumHandle = DatumHandle::new(7, 0);
    const NAME: u32 = 29;
    const ARRAY: u32 = GRAPH + 0x400;
    const ENTRY: u32 = GRAPH + 0x500;
    const WORK: u32 = 0x560000;

    fn graph() -> FakeGuest {
        let mut m = FakeGuest::new();
        let index = GRAPH + 0x1000;
        let group = GroupTag::from_chars(*b"jmad").0;
        m.put_u32(tags::TAG_INDEX_GLOBAL, index)
            .put_u32(tags::TAG_ENTRIES_GLOBAL, index + 0x100)
            .put_u32(index, index + 0x200)
            .put_u32(index + 4, 1)
            .put_u32(index + tags::index_layout::TAG_COUNT, 1)
            .put_u32(index + 0x200, group)
            .put_u32(index + 0x100, group)
            .put_u32(index + 0x104, OWNER.raw())
            .put_u32(index + 0x108, GRAPH)
            .put_u32(GRAPH + 0x2c, 1)
            .put_u32(GRAPH + 0x30, ANIM)
            .put_u32(GRAPH + 0xac, 1)
            .put_u32(GRAPH + 0xb0, RECORD)
            .put_u32(ANIM, NAME)
            .put_u16(ANIM + 0x16, 0x10)
            .put_u32(ANIM + 0x24, 0)
            .put_u32(RECORD, OWNER.raw());
        m
    }

    #[test]
    fn live_animation_read_requires_the_matching_graph_animation_and_resource() {
        let m = graph();
        let read = AnimationRead {
            record: RECORD,
            animation: ANIM,
            owner: OWNER,
        };
        assert_eq!(
            read.live_call(&m),
            Some(GuestCall::new(RESOURCE_GET).eax(RECORD).args([NAME]))
        );
        for stale in [
            AnimationRead {
                record: RECORD + 0x14,
                ..read
            },
            AnimationRead {
                animation: ANIM + 1,
                ..read
            },
            AnimationRead {
                owner: DatumHandle::new(8, 0),
                ..read
            },
        ] {
            assert!(stale.live_call(&m).is_none());
        }
        let mut m = m;
        m.put_u32(ANIM + 0x24, 1);
        assert!(read.live_call(&m).is_none());
    }

    #[test]
    fn cold_animation_read_runs_on_the_host_and_retries_from_fresh_ram() {
        let mut g = Guest::new();
        g.map_zeroed(0x120000, 0x100000, Permissions::RX).unwrap();
        g.map_zeroed(0x460000, 0x110000, Permissions::RW).unwrap();
        g.set_u32(0x4e8c20, 0x550000).unwrap();
        g.set_u32(0x4ed284, 0x550100).unwrap();
        g.set_u32(RECORD, OWNER.raw()).unwrap();
        g.set_u32(CACHE_ENTRIES, ARRAY).unwrap();
        g.set_u32(ARRAY + 0x44, ENTRY).unwrap();
        let mut d = Dispatch::default();
        for address in [0x1872d0, 0x190c34, 0x1557c0, 0x124c00, 0x217c40] {
            d.register_generated_rust(address, |cpu, ctx, _| ctx.memory().ret(cpu, 0))
                .unwrap();
        }
        d.register_generated_rust(0x186ab0, |cpu, ctx, selector| {
            let mut m = ctx.memory();
            if selector == ANIMATION_DATA_RETURN {
                let out = m.u32(cpu.esp + 16)?;
                m.write(out, [cpu.eax as u8; ACTION_SIZE])?;
                return m.ret(cpu, 0);
            }
            cpu.esi = ANIM;
            cpu.eax = RECORD;
            cpu.esp -= 4;
            m.set_u32(cpu.esp, NAME)?;
            m.call(cpu, RESOURCE_GET, ANIMATION_DATA_RETURN)
        })
        .unwrap();
        d.register_resume(ANIMATION_DATA_RETURN, 0x186ab0, ANIMATION_DATA_RETURN)
            .unwrap();
        d.register_generated_rust(RESOURCE_GET, |cpu, ctx, _| {
            let m = ctx.memory();
            assert!(!needs_worker(&m, cpu.eax)?);
            cpu.eax = 42;
            m.ret(cpu, 4)
        })
        .unwrap();
        let rt = Runtime::new(g, d);
        for (handle, job, completed, fill, calls) in [
            (u32::MAX, u32::MAX, 0, true, 1),  // cold
            (0, 17, 0, true, 1),               // submitted, still pending
            (0, 17, 1, true, 0),               // worker completed
            (0, u32::MAX, 0, true, 0),         // resident, no outstanding job
            (u32::MAX, u32::MAX, 0, false, 1), // still cold on retry
        ] {
            {
                let mut m = rt.memory();
                m.set_u32(RECORD + 12, handle).unwrap();
                m.set_u32(ENTRY + 8, job).unwrap();
                m.set_u8(ENTRY + 4, completed).unwrap();
            }
            let main = Arc::clone(&rt);
            let run = entry::testing::run(
                &move |cx| {
                    let main = Arc::clone(&main);
                    Box::pin(async move {
                        let input = Mutex::new(None);
                        let seat = SeatInput {
                            player: DatumHandle::new(1, 1),
                            player_addr: 0x550800,
                            unit: DatumHandle::new(2, 2),
                            tick: 1,
                            dt: 1. / 30.,
                            pad: Default::default(),
                            reset: true,
                        };
                        let cpu = CpuAbi {
                            esp: WORK + 0xfff0,
                            ..Default::default()
                        };
                        assert_eq!(
                            gather_with(&input, &main, WORK, Path::Alias, Stage::Guest, false, cpu, seat, &cx).await,
                            fill.then_some([42; ACTION_SIZE])
                        );
                        Ok(Ret::new(1, 8))
                    })
                },
                graph(),
                Regs::default(),
                |call, _| {
                    assert_eq!(call, &GuestCall::new(RESOURCE_GET).eax(RECORD).args([NAME]));
                    if fill {
                        let mut m = rt.memory();
                        m.set_u32(RECORD + 12, 0).unwrap();
                        m.set_u32(ENTRY + 8, u32::MAX).unwrap();
                    }
                    Regs::default()
                },
            );
            assert_eq!(run.result, Ok(Ret::new(1, 8)));
            assert_eq!(run.calls.len(), calls);
        }
    }
}
