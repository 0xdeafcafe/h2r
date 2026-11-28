//! World-space collision rays from the whole-query entry. The argument
//! names are in SIM_COLLISION_QUERY. The result's world position is in
//! docs/engine/sim-rewrite-collision.md, "5.4 The collision query".

use guest_layout::{GuestMemory, points::SIM_COLLISION_QUERY};
use guest_layout::points::Loc;
use runtime::{Runtime, dispatch::ReturnObservation};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, MutexGuard, OnceLock, Weak},
};

const CAPACITY: usize = 512;
const NO_POINT: &str = "needs SIM_COLLISION_QUERY in this image";
const NO_RUNTIME: &str = "this host has no collision trace source";
const LOCKSTEP: &str = "collision trace observation changes scheduling during lockstep, film recording or film playback";

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct CollisionTrace {
    pub tick: u64,
    pub start: [f32; 3],
    pub end: [f32; 3],
    pub hit: Option<[f32; 3]>,
}

#[derive(Default)]
struct History {
    generation: u64,
    rays: VecDeque<CollisionTrace>,
}

#[derive(Default)]
pub struct CollisionTraces {
    source: OnceLock<Result<Weak<Runtime>, &'static str>>,
    hook: Mutex<Option<ReturnObservation>>,
    history: Mutex<History>,
}

struct Pending {
    start: [f32; 3],
    end: [f32; 3],
    result: u32,
}

fn finite(point: [f32; 3]) -> Option<[f32; 3]> {
    point.into_iter().all(f32::is_finite).then_some(point)
}

fn point(mem: &dyn GuestMemory, address: u32) -> Option<[f32; 3]> {
    if address == 0 || address.checked_add(11).is_none() {
        return None;
    }
    finite(mem.vec3(address).ok()?)
}

impl Pending {
    fn read(mem: &dyn GuestMemory, esp: u32) -> Option<Self> {
        let arg = |name| {
            let Loc::Stack(word) = SIM_COLLISION_QUERY.sig.arg(name)? else {
                return None;
            };
            mem.u32(esp.checked_add(4 + 4 * u32::from(word))?).ok()
        };
        let start = point(mem, arg("start")?)?;
        let delta = point(mem, arg("delta")?)?;
        let end = finite(std::array::from_fn(|i| start[i] + delta[i]))?;
        let result = arg("out")?;
        (result != 0).then_some(Self { start, end, result })
    }

    fn finish(self, tick: u64, al: u8, mem: &dyn GuestMemory) -> Option<CollisionTrace> {
        // A miss leaves no hit point. Never interpret an invalid hit as a miss.
        let hit = if al != 0 {
            Some(point(mem, self.result.checked_add(8)?)?)
        } else {
            None
        };
        Some(CollisionTrace {
            tick,
            start: self.start,
            end: self.end,
            hit,
        })
    }
}

impl CollisionTraces {
    /// Record where to install the observer when requested. Startup leaves
    /// the generated entry and any native collision implementation alone.
    pub fn bind(&self, rt: &Arc<Runtime>, deterministic: bool) {
        let _ = self.source.set(if deterministic {
            Err(LOCKSTEP)
        } else {
            Ok(Arc::downgrade(rt))
        });
    }

    fn runtime(&self) -> Result<Arc<Runtime>, &'static str> {
        self.source
            .get()
            .ok_or(NO_RUNTIME)?
            .as_ref()
            .map_err(|&e| e)?
            .upgrade()
            .ok_or(NO_RUNTIME)
    }

    pub fn unavailable(&self) -> Option<&'static str> {
        let rt = match self.runtime() {
            Ok(rt) => rt,
            Err(reason) => return Some(reason),
        };
        (!rt.read_dispatch(|d| d.has_generated(SIM_COLLISION_QUERY.base))).then_some(NO_POINT)
    }

    fn history(&self) -> MutexGuard<'_, History> {
        self.history.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Drop completed and in-flight samples at a map or timeline boundary.
    pub fn clear(&self) {
        let mut history = self.history();
        history.generation = history.generation.wrapping_add(1);
        history.rays.clear();
    }

    pub fn set_enabled(self: &Arc<Self>, enabled: bool) -> Result<(), &'static str> {
        let mut active = self.hook.lock().unwrap_or_else(|e| e.into_inner());
        if active.is_some() == enabled {
            return Ok(());
        }
        let rt = self.runtime()?;
        if !enabled {
            if let Some(hook) = active.take() {
                rt.edit_dispatch(|d| hook.remove(d));
            }
            self.clear();
            return Ok(());
        }
        if let Some(reason) = self.unavailable() {
            return Err(reason);
        }
        let sample = Arc::downgrade(self);
        let store = Arc::downgrade(self);
        *active = Some(
            rt.edit_dispatch(|d| {
                d.observe_returns(
                    SIM_COLLISION_QUERY.base,
                    "debug views: world collision query",
                    move |cpu, mem| {
                        let trace = sample.upgrade()?;
                        let generation = trace.history().generation;
                        let pending = Pending::read(mem, cpu.esp)?;
                        let clock = guest_layout::game::GameTime::current(mem as &dyn GuestMemory)
                            .ok()??;
                        let tick = u64::from(clock.tick(mem as &dyn GuestMemory).ok()?);
                        Some((generation, tick, pending))
                    },
                    move |(generation, tick, pending), cpu, mem| {
                        if let Some(store) = store.upgrade()
                            && let Some(ray) = pending.finish(tick, cpu.eax as u8, mem)
                        {
                            store.push(generation, ray);
                        }
                    },
                )
            })
            .ok_or(NO_POINT)?,
        );
        Ok(())
    }

    fn push(&self, generation: u64, ray: CollisionTrace) {
        let mut history = self.history();
        if history.generation != generation {
            return;
        }
        if history.rays.len() == CAPACITY {
            history.rays.pop_front();
        }
        history.rays.push_back(ray);
    }

    /// Newest completed calls first. A tick window of one includes only
    /// the specified tick. Future ticks after a clock rewind stay hidden.
    pub fn recent(&self, tick: u64, window: u16) -> Vec<CollisionTrace> {
        self.history()
            .rays
            .iter()
            .rev()
            .filter(|ray| {
                tick.checked_sub(ray.tick)
                    .is_some_and(|age| age < u64::from(window))
            })
            .copied()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::{CpuAbi, Dispatch, ExecCtx, Guest, Runner, TransferAbi, abi, guest::Permissions};

    #[test]
    fn history_is_bounded_and_a_cut_drops_unfinished_queries() {
        let store = CollisionTraces::default();
        let mut ray = CollisionTrace {
            tick: 0,
            start: [0.; 3],
            end: [1.; 3],
            hit: None,
        };
        for tick in 0..=CAPACITY as u64 {
            ray.tick = tick;
            store.push(0, ray);
        }
        let history = store.recent(ray.tick, u16::MAX);
        assert_eq!(history.len(), CAPACITY);
        assert_eq!(history.last().unwrap().tick, 1);
        assert_eq!(history.first().unwrap().tick, ray.tick);
        assert_eq!(
            store
                .recent(3, 2)
                .iter()
                .map(|r| r.tick)
                .collect::<Vec<_>>(),
            [3, 2]
        );
        store.clear();
        store.push(0, ray);
        assert!(store.recent(ray.tick, 2).is_empty());
    }

    #[test]
    fn capture_wraps_the_selected_body_only_while_enabled_and_reads_its_hit() {
        use live_telemetry::layout::{GAME_TIME_PTR, TIME_TICK};
        const SP: u32 = 0x20300;
        const START: u32 = 0x20100;
        const DELTA: u32 = START + 12;
        const RESULT: u32 = DELTA + 12;
        const CALLER: u32 = 0x31000;
        const TIME: u32 = 0x20000;
        let addr = SIM_COLLISION_QUERY.base;
        let mut guest = Guest::new();
        for (page, perms) in [
            (GAME_TIME_PTR & !0xfff, Permissions::RW),
            (TIME, Permissions::RW),
            (CALLER, Permissions::RX),
            (addr & !0xfff, Permissions::RX),
        ] {
            guest.map_zeroed(page, 4096, perms).unwrap();
        }
        guest.set_u32(GAME_TIME_PTR, TIME).unwrap();
        guest.set_u32(TIME + TIME_TICK, 20).unwrap();
        for (i, value) in [1f32, 2., 3., 8., 0., -2.].into_iter().enumerate() {
            guest
                .set_u32(START + 4 * i as u32, value.to_bits())
                .unwrap();
        }
        for (i, word) in [CALLER, 1, START, DELTA, u32::MAX, u32::MAX, RESULT]
            .into_iter()
            .enumerate()
        {
            guest.set_u32(SP + 4 * i as u32, word).unwrap();
        }
        let mut dispatch = Dispatch::default();
        dispatch
            .register_generated_rust(addr, |_, _, _| {
                panic!("the selected native body owns this entry")
            })
            .unwrap();
        dispatch.register_override(addr, "native query", |cpu, ctx, _| {
            let mut mem = ctx.memory();
            for (i, value) in [3f32, 2., 2.5].into_iter().enumerate() {
                mem.set_u32(RESULT + 8 + 4 * i as u32, value.to_bits())?;
            }
            mem.ret(cpu, 24)
        });
        dispatch.register_override(CALLER, "fixture stop", |_, _, _| Ok(TransferAbi::stop(0)));
        let rt = Runtime::new(guest, dispatch);
        let samples = Arc::new(CollisionTraces::default());
        samples.bind(&rt, false);
        let name = || rt.read_dispatch(|d| d.override_name(addr).map(str::to_owned));
        assert_eq!(name().as_deref(), Some("native query"));
        samples.set_enabled(true).unwrap();
        let mut ctx = ExecCtx::new(rt.clone(), 1);
        for eax in [1, 0x100] {
            rt.memory().set_u32(SP, CALLER).unwrap();
            let mut cpu = CpuAbi {
                esp: SP,
                eax,
                ..Default::default()
            };
            assert_eq!(
                Runner::default().run(&mut cpu, &mut ctx, addr).kind,
                abi::STOP
            );
            assert_eq!((cpu.esp, cpu.eax), (SP + 28, eax));
            assert_eq!(rt.memory().u32(SP).unwrap(), CALLER);
        }
        let rays = samples.recent(20, 1);
        assert_eq!(rays.len(), 2);
        assert_eq!(rays[0].hit, None);
        assert_eq!(
            rays[1],
            CollisionTrace {
                tick: 20,
                start: [1., 2., 3.],
                end: [9., 2., 1.],
                hit: Some([3., 2., 2.5]),
            }
        );
        samples.set_enabled(false).unwrap();
        assert_eq!(name().as_deref(), Some("native query"));
        assert!(samples.recent(20, 1).is_empty());
    }
}
