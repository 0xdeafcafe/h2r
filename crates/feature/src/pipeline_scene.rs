//! The game thread publishes scene ownership before releasing the tick.
//! A generated scene traversal runs on the render worker. A get must
//! return a payload before its caller can discover the next use, so the
//! worker batches settled use stamps in visit order. A miss or pending
//! payload suspends traversal at the real getter. One pending request
//! suffices because the scene needs its result to continue.

use super::{Footprint, Shared, Work, snapshot};
use runtime::exec::OriginalHandler;
use runtime::executor::{ExecutorError, SecondExecutor};
use runtime::{CpuAbi, Dispatch, Event, ExecCtx, GuestFault, Runtime, TransferAbi, abi};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{SyncSender, sync_channel};

#[derive(Clone, Copy, Debug)]
pub struct SceneTraversal {
    pub entries: &'static [u32],
    pub animation_get: u32,
    pub resident: Option<ResidentCache>,
    /// Functions with game-thread ownership even when generated.
    pub owners: &'static [u32],
}

/// Globals used by the settled-hit branch of animation_resource_get.
#[derive(Clone, Copy, Debug)]
pub struct ResidentCache {
    pub jobs: u32,
    pub blocks: u32,
}

struct Use {
    stamp: u32,
    value: u32,
}

impl ResidentCache {
    /// On a resident handle the prediction search returns immediately.
    /// A cleared I/O handle selects the wait helper's no-work branch.
    /// The remaining non-stack store is the block's current use stamp.
    fn settled(&self, m: &runtime::Guest, record: u32) -> Option<Use> {
        let handle = m.u32(record.checked_add(0x0c)?).ok()?;
        if handle == u32::MAX {
            return None;
        }
        let index = handle & 0xffff;
        let array = m.u32(self.jobs).ok()?;
        let jobs = m.u32(array.checked_add(0x44)?).ok()?;
        let job = jobs.checked_add(16 * index)?;
        // A completed job with a handle still needs a live cleanup.
        if m.u32(job.checked_add(8)?).ok()? != u32::MAX {
            return None;
        }
        let cache = m.u32(self.blocks).ok()?;
        let value = m.u32(cache.checked_add(0x38)?).ok()?;
        let slots = m.u32(cache.checked_add(0x64)?).ok()?;
        let data = m.u32(slots.checked_add(0x44)?).ok()?;
        let stamp = data.checked_add(24 * index)?.checked_add(0x14)?;
        Some(Use { stamp, value })
    }
}

impl SceneTraversal {
    pub const HALO2: Self = Self {
        entries: &[
            engine::points::RENDER_GAME_VIEW.base,
            engine::points::RENDER_SCENE_PASSES.base,
            engine::points::RENDER_ALTERNATE_VIEW.base,
            // Resolve animation data for a graph animation. This can be
            // reached through an intervening native HUD or UI draw.
            0x001d_d7c0,
        ],
        animation_get: 0x0012_37e0,
        resident: Some(ResidentCache {
            jobs: 0x004e_3b48,
            blocks: 0x004e_3b54,
        }),
        owners: &[
            0x0012_36f0, // animation resource prediction
            0x0012_34a0, // eviction
            0x0012_35b0, // resource I/O submission
            0x0017_4420, // particle visibility and location stamps
        ],
    };
}

pub(super) fn executor(rt: &Runtime, layout: SceneTraversal) -> Box<SecondExecutor> {
    let mut exec = SecondExecutor::generated_only(rt);
    exec.stop_before(layout.animation_get, "scene animation get");
    for &entry in layout.owners {
        exec.stop_before(entry, "scene owner");
    }
    Box::new(exec)
}

#[derive(Clone, Copy)]
struct Scope {
    esp: u32,
    ret: u32,
    get: u32,
    resident: Option<ResidentCache>,
    owner_hook: bool,
}

pub(super) struct Job {
    exec: Box<SecondExecutor>,
    cpu: CpuAbi,
    transfer: TransferAbi,
    scope: Scope,
    reply: SyncSender<Publication>,
}

struct Publication {
    exec: Box<SecondExecutor>,
    stop: Stop,
    footprint: Footprint,
    uses: Vec<Use>,
    accesses: u64,
}

enum Stop {
    Return(CpuAbi),
    Lookup { at: CpuAbi, caller: u32 },
    Owner(CpuAbi),
    Retry,
}

pub(super) fn run(mut job: Job) {
    snapshot::arm(&mut job.exec.runtime().memory());
    let mut uses = Vec::new();
    let mut accesses = 0;
    let stop = loop {
        let result =
            job.exec
                .run_to_return(&mut job.cpu, job.transfer, job.scope.esp, job.scope.ret);
        accesses += job.exec.last_accesses();
        let refused = match result {
            Ok(_) => break Stop::Return(job.cpu),
            Err(ExecutorError::Refused(r)) => r,
            Err(_) => break Stop::Retry,
        };
        let Some(at) = job.exec.refused_cpu() else {
            break Stop::Retry;
        };
        if refused.target != job.scope.get || job.scope.owner_hook {
            break Stop::Owner(at);
        }
        let Some(caller) = refused.return_pc else {
            break Stop::Retry;
        };
        // Bound the use log even for a guest loop containing only hits.
        if uses.len() == 256 {
            break Stop::Lookup { at, caller };
        }
        let settled = job
            .scope
            .resident
            .and_then(|layout| layout.settled(&job.exec.runtime().memory(), at.eax));
        let Some(used) = settled else {
            break Stop::Lookup { at, caller };
        };

        // The settled branch has one visible store, the use stamp. Run
        // the original on private RAM to preserve its entire register and
        // flags ABI, including the returned address in the frozen pool.
        // A nested host call invalidates this segment before publication.
        job.exec
            .runtime()
            .edit_dispatch(|d| d.set_enabled(at.eip, false));
        job.cpu = at;
        let result =
            job.exec
                .run_to_return(&mut job.cpu, TransferAbi::jump(at.eip), at.esp, caller);
        job.exec
            .runtime()
            .edit_dispatch(|d| d.set_enabled(at.eip, true));
        accesses += job.exec.last_accesses();
        if result.is_err() {
            break Stop::Retry;
        }
        uses.push(used);
        job.transfer = TransferAbi::ret(caller);
    };
    let footprint = Footprint::take(&mut job.exec.runtime().memory());
    let _ = job.reply.send(Publication {
        exec: job.exec,
        stop,
        footprint,
        uses,
        accesses,
    });
}

pub(super) fn install(
    d: &mut Dispatch,
    me: &Arc<Shared>,
    entries: Vec<(u32, OriginalHandler)>,
    layout: SceneTraversal,
) {
    let owner_hook = d.override_name(layout.animation_get).is_some();
    for (entry, original) in entries {
        // A native implementation or observer at the entry retains its
        // normal execution and can still call a nested generated traversal.
        if d.override_name(entry).is_some() {
            continue;
        }
        let me = Arc::clone(me);
        let original = Arc::new(original);
        d.register_override(entry, "pipeline: prepare scene", move |cpu, ctx, sel| {
            let preparing = ctx.thread_id == me.game_thread.load(Ordering::Relaxed)
                && me
                    .lock()
                    .preparing
                    .as_ref()
                    .is_some_and(|p| p.tail.is_none() && !p.scene_retry);
            if !preparing {
                return Ok(original.invoke(cpu, ctx, sel));
            }
            let scope = Scope {
                esp: cpu.esp,
                ret: ctx.memory().u32(cpu.esp)?,
                get: layout.animation_get,
                resident: layout.resident,
                owner_hook,
            };
            segment(
                &me,
                cpu,
                ctx,
                scope,
                TransferAbi::jump(sel),
                Some(&original),
            )
        });
    }
}

fn segment(
    me: &Arc<Shared>,
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    scope: Scope,
    transfer: TransferAbi,
    original: Option<&OriginalHandler>,
) -> Result<TransferAbi, GuestFault> {
    let retry = |cpu: &mut CpuAbi, ctx: &mut ExecCtx| {
        Ok(match original {
            Some(handler) => handler.invoke(cpu, ctx, transfer.target_pc),
            None => transfer,
        })
    };
    let rt = Arc::clone(ctx.runtime());
    let busy = super::pool_reason(me, &rt.memory()).is_some();
    let generation = rt.dispatch_generation();
    let (jobs, mut exec) = {
        let mut inner = me.lock();
        let setup = inner.setup.as_mut().expect("preparing a frame");
        if busy || generation != setup.scene_dispatch_generation {
            drop(inner);
            return retry(cpu, ctx);
        }
        (
            setup.jobs.clone(),
            setup.scene.take().expect("idle scene executor"),
        )
    };
    exec.snapshot(&rt);
    let (tx, rx) = sync_channel(1);
    let job = Job {
        exec,
        cpu: *cpu,
        scope,
        transfer,
        reply: tx,
    };
    let result = jobs
        .send(Work::Scene(job))
        .ok()
        .and_then(|()| rx.recv().ok());
    let Some(Publication {
        mut exec,
        mut stop,
        footprint,
        uses,
        accesses,
    }) = result
    else {
        return Ok(TransferAbi::fault(ctx.emit(Event::Unsupported {
            pc: cpu.eip,
            mnemonic: "pipeline".into(),
            detail: "scene worker stopped".into(),
            backtrace: Vec::new(),
        })));
    };
    let nuses = uses.len() as u64;
    let committed = if matches!(stop, Stop::Retry) || rt.dispatch_generation() != generation {
        Ok(false)
    } else {
        publish(&mut exec, &rt, footprint, &uses)
    };
    if matches!(committed, Ok(true)) {
        ctx.charge_accesses(accesses);
    } else {
        stop = Stop::Retry;
    }
    {
        let mut inner = me.lock();
        inner.setup.as_mut().expect("preparing").scene = Some(exec);
        if matches!(stop, Stop::Retry) {
            inner.stats.scene_retries += 1;
            if let Some(preparing) = &mut inner.preparing {
                preparing.scene_retry = true;
            }
        } else {
            inner.stats.animation_uses += nuses;
            inner.stats.animation_owner_calls += u64::from(matches!(stop, Stop::Lookup { .. }));
        }
    }
    committed?;
    match stop {
        Stop::Retry => retry(cpu, ctx),
        Stop::Return(done) => {
            *cpu = done;
            Ok(TransferAbi::ret(scope.ret))
        }
        Stop::Owner(at) => {
            *cpu = at;
            Ok(TransferAbi::jump(at.eip))
        }
        Stop::Lookup { at, caller } => {
            // Replay the actual get with its resource record in EAX and
            // its original stack argument. The callee owns every stamp,
            // eviction, submission and wait. When it returns, freeze the
            // resulting bytes before the worker consumes its payload.
            *cpu = at;
            cpu.esp = at.esp.wrapping_add(4);
            let me = Arc::clone(me);
            let call = ctx.call_guest_named(
                cpu,
                &mut rt.memory(),
                at.eip,
                "pipeline: owner animation get",
                move |cpu, ctx| {
                    ctx.memory().set_u32(at.esp, caller)?;
                    let next = TransferAbi::ret(caller);
                    if caller == scope.ret && cpu.esp > scope.esp {
                        return Ok(next);
                    }
                    segment(&me, cpu, ctx, scope, next, None)
                },
            );
            if !matches!(&call, Ok(t) if t.kind == abi::CALL) {
                *cpu = at;
            }
            call
        }
    }
}

/// Validate and publish with the same live-memory lock. No device call or
/// cache miss ran in the private segment, so a failed check can retry it.
fn publish(
    exec: &mut SecondExecutor,
    rt: &Runtime,
    footprint: Footprint,
    uses: &[Use],
) -> Result<bool, GuestFault> {
    let mut live = rt.memory();
    if !footprint.still_current(&live, exec.base().expect("scene snapshot")) {
        return Ok(false);
    }
    // A later unmodelled store to a stamp requires the original ordering.
    // Validate all destinations before writing any of them.
    let latest: BTreeMap<_, _> = uses.iter().map(|u| (u.stamp, u.value)).collect();
    for (va, value) in latest {
        let Some(phys) = super::physical(&live, va) else {
            return Ok(false);
        };
        if live.check(va, 4, runtime::guest::Access::Write, 1).is_err()
            || exec.runtime().memory().u32(va).ok() != Some(value)
        {
            return Ok(false);
        }
        exec.revert(phys, 4);
    }
    let writes = exec.written();
    live.hand_back(
        &exec.runtime().memory(),
        exec.base().expect("scene snapshot"),
        &writes,
    );
    for used in uses {
        live.set_u32(used.stamp, used.value)?;
    }
    Ok(true)
}
