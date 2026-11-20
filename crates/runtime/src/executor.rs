//! A second executor: one guest call at a time, on whichever host thread
//! owns it, against a copy-on-write snapshot of the main runtime's guest
//! RAM (docs/runtime/second-executor.md, section 3).
//!
//! It is its own [`Runtime`] (own memory gate, page table, time-stamp
//! counter, interrupt queue) with its own [`ExecCtx`], over a view Guest
//! ([`Guest::cow_refresh`]). Its dispatch tables are copied once from the
//! main runtime: generated handlers and resumes as they are; every HLE
//! override the caller does not allow, and every kernel import, refused.
//! No kernel scheduler runs it: no slices, no interrupts, no waits.

use crate::dispatch::Handler;
use crate::guest::{FrozenRam, HandBack, PAGE_SIZE, RAM_SIZE, WrittenPage};
use crate::watch::{CONTIGUOUS_BASE, WatchId};
use crate::{
    CopyKind, CpuAbi, Event, ExecCtx, Guest, GuestFault, Runner, Runtime, TransferAbi, abi, kernel,
};
use std::sync::{Arc, Mutex};

/// The `ExecCtx::thread_id` of an executor's context: no kernel thread has
/// it.
pub const EXECUTOR_THREAD_ID: u32 = 0xe0e0_0001;

/// An HLE entry or kernel import the executor would not run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// The address called: an override's guest address or a kernel import
    /// token ([`kernel::function_token`]).
    pub target: u32,
    /// The override's name, or the kernel import's.
    pub name: String,
    /// The kernel import's ordinal, for a kernel call.
    pub ordinal: Option<u32>,
    /// The return address on the guest stack at the call, if readable.
    pub return_pc: Option<u32>,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum ExecutorError {
    #[error(
        "second executor refused {} ({:#010x}{}), called from {:x?}",
        .0.name,
        .0.target,
        .0.ordinal.map(|o| format!(", kernel ordinal {o}")).unwrap_or_default(),
        .0.return_pc
    )]
    Refused(Refusal),
    #[error("second executor stopped with {transfer:?}: {event:?}")]
    Fault {
        transfer: TransferAbi,
        event: Option<Event>,
    },
    #[error("second executor run cancelled")]
    Cancelled,
    #[error("second executor could not set up the call's stack: {0}")]
    Stack(GuestFault),
    #[error("second executor has no snapshot to run against")]
    NoSnapshot,
}

/// A call that returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallOutcome {
    /// Guest memory accesses the call made: what the main thread's logical
    /// clock would have been charged for it.
    pub accesses: u64,
}

/// Stops an executor's run at its next safe point, from any thread.
#[derive(Clone)]
pub struct CancelHandle(Arc<Runtime>);
impl CancelHandle {
    pub fn cancel(&self) {
        self.0.request_preempt(crate::preempt::REQUEST);
    }
}

pub struct SecondExecutor {
    runtime: Arc<Runtime>,
    ctx: ExecCtx,
    watch: WatchId,
    base: Option<FrozenRam>,
    refused: Arc<Mutex<Option<(Refusal, CpuAbi)>>>,
    /// Write-watch owners whose subscriptions on the main Guest the view
    /// takes over at each snapshot ([`share_watch`](Self::share_watch)).
    shared_watches: Vec<&'static str>,
    /// The CPU at the refused call of the last run, if it was refused.
    refused_cpu: Option<CpuAbi>,
    last_accesses: u64,
    pub max_transfers: u64,
}

/// Ids at and above this one are the executor's own write-watch
/// subscriptions; below it, ones adopted from the main Guest.
const OWN_WATCH_IDS: u32 = 0x4000_0000;

type RefusedSlot = Arc<Mutex<Option<(Refusal, CpuAbi)>>>;

fn refusal(refused: &RefusedSlot, target: u32, name: &str) -> Handler {
    let refused = refused.clone();
    let name = name.to_string();
    let import = target
        .checked_sub(kernel::KERNEL_TOKEN_BASE)
        .filter(|v| v.is_multiple_of(4))
        .and_then(|v| kernel::function(v / 4))
        .copied();
    Handler::Rust(Arc::new(
        move |cpu: &mut CpuAbi, ctx: &mut ExecCtx, _selector| {
            let return_pc = ctx.memory().u32(cpu.esp).ok();
            // The CPU as the call left it (return address pushed, `eip` the
            // target): a caller can carry the run on from here elsewhere.
            let at = CpuAbi {
                eip: target,
                ..*cpu
            };
            *refused.lock().unwrap_or_else(|e| e.into_inner()) = Some((
                Refusal {
                    target,
                    name: import.map_or_else(|| name.clone(), |i| i.name.to_string()),
                    ordinal: import.map(|i| i.ordinal),
                    return_pc,
                },
                at,
            ));
            // A kernel call keeps its usual record (arguments and all), as a
            // fault rather than a stop: nothing will service it.
            let event = match import {
                Some(i) => kernel::stop(cpu, ctx, i)?.event_id,
                None => ctx.emit(Event::UnsupportedTarget {
                    pc: return_pc.unwrap_or(cpu.eip),
                    target,
                    kind: abi::CALL,
                    route: abi::ENTRY,
                }),
            };
            Ok(TransferAbi::fault(event))
        },
    ))
}

impl SecondExecutor {
    /// An executor sharing `main`'s generated code. `allow(addr, name)`
    /// picks the HLE overrides it may run (on its own thread, against its
    /// own memory); kernel imports are refused unless allowed the same way
    /// by their token address. Nothing runs until [`snapshot`](Self::snapshot).
    pub fn new(main: &Runtime, allow: impl Fn(u32, &str) -> bool) -> Self {
        let refused = Arc::new(Mutex::new(None));
        let mut dispatch = main.fork_dispatch(&allow, |addr, name| refusal(&refused, addr, name));
        for import in kernel::FUNCTION_IMPORTS {
            let token = kernel::function_token(import.ordinal);
            if dispatch.override_name(token).is_none() || !allow(token, import.name) {
                dispatch.register_override_handler(
                    token,
                    import.name.to_string(),
                    refusal(&refused, token, import.name),
                );
            }
        }
        let mut guest = Guest::new();
        guest.watch.number_from(OWN_WATCH_IDS);
        let watch = guest.subscribe_writes("second executor");
        guest
            .watch_writes(watch, CONTIGUOUS_BASE, RAM_SIZE as u32)
            .expect("all of RAM is a valid watch range");
        let runtime = Runtime::new(guest, dispatch);
        let mut ctx = ExecCtx::new(runtime.clone(), EXECUTOR_THREAD_ID);
        // Count accesses and take host requests (cancel); never expire.
        ctx.arm_preemption(0);
        Self {
            runtime,
            ctx,
            watch,
            base: None,
            refused,
            shared_watches: Vec::new(),
            refused_cpu: None,
            last_accesses: 0,
            max_transfers: Runner::default().max_transfers,
        }
    }

    /// At every [`snapshot`](Self::snapshot), take over the main Guest's
    /// write-watch subscriptions named `owner` ([`Guest::adopt_watch`]): an
    /// allowed override that drains its subscription by id (the D3D write
    /// barrier) then sees, on the view, the main side's writes it had not
    /// drained and the view's own.
    pub fn share_watch(&mut self, owner: &'static str) {
        if !self.shared_watches.contains(&owner) {
            self.shared_watches.push(owner);
        }
    }

    /// The CPU at the refused call, after a run that ended in
    /// [`ExecutorError::Refused`]: `eip` the target, `esp` at the pushed
    /// return address, every other register as the caller had it. With the
    /// view's writes handed back (the stack included), a context on the main
    /// runtime can make the call itself and carry the run on from there.
    pub fn refused_cpu(&self) -> Option<CpuAbi> {
        self.refused_cpu
    }

    /// Guest memory accesses the last [`call`](Self::call) made, whether it
    /// returned or not.
    pub fn last_accesses(&self) -> u64 {
        self.last_accesses
    }

    /// The snapshot's bytes: what the view started from.
    pub fn base(&self) -> Option<&FrozenRam> {
        self.base.as_ref()
    }

    /// Undo the view's changes to physical `[physical, physical + len)`
    /// (back to the snapshot's bytes), so [`written`](Self::written) and
    /// [`hand_back`](Self::hand_back) leave them out. Takes this executor's
    /// memory guard.
    pub fn revert(&mut self, physical: u32, len: u32) {
        let Some(base) = &self.base else {
            return;
        };
        self.runtime.memory().restore_from(base, physical, len);
    }

    /// The executor's own runtime (its view's memory, its dispatch).
    pub fn runtime(&self) -> &Arc<Runtime> {
        &self.runtime
    }
    /// The executor's context, between calls.
    pub fn context(&mut self) -> &mut ExecCtx {
        &mut self.ctx
    }
    pub fn cancel_handle(&self) -> CancelHandle {
        CancelHandle(self.runtime.clone())
    }

    /// Freeze `main`'s guest RAM as it is now: the view the next calls run
    /// against, and the base its writes are compared with. Takes `main`'s
    /// memory guard (so no store is in flight), after reading its guest
    /// time, which takes the kernel lock: call it with no runtime lock held.
    /// The executor's time-stamp counter continues from `main`'s and its
    /// clock stands at `main`'s guest time now.
    pub fn snapshot(&mut self, main: &Runtime) -> CopyKind {
        let time = main.guest_time();
        self.runtime
            .set_guest_clock(time.map(|t| Arc::new(move || Some(t)) as crate::tsc::GuestClock));
        let mut view = self.runtime.memory();
        let mut live = main.memory();
        self.runtime.restore_tsc_last(main.tsc_last());
        let kind = view.cow_refresh(&live);
        for owner in &self.shared_watches {
            view.adopt_watch(&mut live, owner);
        }
        self.base = Some(live.freeze_ram().0);
        kind
    }

    /// Call guest `entry` with `args` (pushed right to left, then a return
    /// to [`crate::exec::HARNESS_EXIT`]) on `cpu`'s stack in the view, and
    /// run it on this thread until it returns. `cpu.esp` must be in the
    /// view's writable memory. `cpu` is left as the call left it.
    pub fn call(
        &mut self,
        cpu: &mut CpuAbi,
        entry: u32,
        args: &[u32],
    ) -> Result<CallOutcome, ExecutorError> {
        if self.base.is_none() {
            return Err(ExecutorError::NoSnapshot);
        }
        {
            let mut memory = self.ctx.memory();
            for &v in args.iter().rev().chain([crate::exec::HARNESS_EXIT].iter()) {
                let esp = cpu.esp.wrapping_sub(4);
                memory.set_u32(esp, v).map_err(ExecutorError::Stack)?;
                cpu.esp = esp;
            }
        }
        self.run_from(cpu, TransferAbi::jump(entry))
    }

    /// Enter a callee whose caller and return address are already in the
    /// COW stack. The outer frame returns to `HARNESS_EXIT` as in `call`.
    pub fn enter(&mut self, cpu: &mut CpuAbi) -> Result<CallOutcome, ExecutorError> {
        if self.base.is_none() {
            return Err(ExecutorError::NoSnapshot);
        }
        let entry = cpu.eip;
        self.run_from(cpu, TransferAbi::jump(entry))
    }

    /// Carry a run on by returning to `cpu.eip` with `cpu` as it is (nothing
    /// pushed; generated code's resume points take it):
    /// after an [`ExecutorError::Refused`] whose call someone else made, with
    /// `eip` the call's return address, `esp` past its arguments and the
    /// result in `eax`/`edx`. The run ends where the first [`call`](Self::call)
    /// would have: at its return to [`crate::exec::HARNESS_EXIT`]. The view
    /// and the context's continuations are as the refusal left them.
    pub fn resume(&mut self, cpu: &mut CpuAbi) -> Result<CallOutcome, ExecutorError> {
        if self.base.is_none() {
            return Err(ExecutorError::NoSnapshot);
        }
        self.run_from(cpu, TransferAbi::ret(cpu.eip))
    }

    fn run_from(
        &mut self,
        cpu: &mut CpuAbi,
        transfer: TransferAbi,
    ) -> Result<CallOutcome, ExecutorError> {
        *self.refused.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.refused_cpu = None;
        self.ctx.take_slice_spent();
        let t = Runner {
            max_transfers: self.max_transfers,
        }
        .run_from(cpu, &mut self.ctx, transfer);
        let accesses = self.ctx.take_slice_spent();
        self.last_accesses = accesses;
        if t.kind == abi::STOP && cpu.eip == crate::exec::HARNESS_EXIT {
            return Ok(CallOutcome { accesses });
        }
        if t.kind == abi::STOP && t.event_id == 0 && self.ctx.take_preempted().is_some() {
            return Err(ExecutorError::Cancelled);
        }
        if let Some((r, at)) = self
            .refused
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            self.refused_cpu = Some(at);
            return Err(ExecutorError::Refused(r));
        }
        Err(ExecutorError::Fault {
            transfer: t,
            event: self.ctx.event(t.event_id).cloned(),
        })
    }

    /// The physical pages the view stored to since the snapshot (or the
    /// last call of this), with the lines that differ from the snapshot,
    /// in address order.
    pub fn written(&mut self) -> Vec<WrittenPage> {
        let Some(base) = &self.base else {
            return Vec::new();
        };
        // Taking the guard publishes the inline C stores' dirty bits.
        let mut view = self.runtime.memory();
        let ranges = view
            .take_dirty(self.watch)
            .expect("the executor's own subscription");
        ranges
            .into_iter()
            .flat_map(|r| (r.addr..r.addr + r.len).step_by(PAGE_SIZE as usize))
            .map(|a| a & !CONTIGUOUS_BASE)
            .map(|physical| WrittenPage {
                physical,
                lines: view.changed_lines(base, physical),
            })
            .collect()
    }

    /// Merge what the view changed since the last flush (or the snapshot)
    /// into `main`'s live RAM now, in the middle of a run, so another
    /// guest thread there can read it: every page stored to since the last
    /// [`written`](Self::written) or flush, against `flushed`'s copy of it
    /// (the view's bytes at its last flush) or the snapshot's, except the
    /// physical ranges `skip`. `flushed` then holds the view's bytes of
    /// those pages. Before the final [`hand_back`](Self::hand_back), call
    /// [`unflush`](Self::unflush) so that bytes already merged are not
    /// merged again over what live RAM did since.
    pub fn flush(
        &mut self,
        main: &Runtime,
        flushed: &mut std::collections::BTreeMap<u32, Box<[u8]>>,
        skip: &[(u32, u32)],
    ) -> HandBack {
        let pages: Vec<u32> = self.written().iter().map(|w| w.physical).collect();
        let Some(base) = &self.base else {
            return HandBack::default();
        };
        let view = self.runtime.memory();
        let mut live = main.memory();
        let mut out = HandBack::default();
        for p in pages {
            let p = p & !(PAGE_SIZE - 1);
            let start = p as usize;
            let reference: &[u8] = match flushed.get(&p) {
                Some(r) => r,
                None => match base.bytes().get(start..start + PAGE_SIZE as usize) {
                    Some(b) => b,
                    None => continue,
                },
            };
            let h = live.merge_page(&view, reference, p, skip);
            out.bytes += h.bytes;
            out.conflicts += h.conflicts;
            out.lines += h.lines;
            if let Some(now) = view.physical_page(p) {
                flushed.insert(p, now.into());
            }
        }
        out
    }

    /// Before the final [`hand_back`](Self::hand_back) of a run that was
    /// [`flush`](Self::flush)ed: every byte of a flushed page the view has
    /// not changed since its flush goes back to the snapshot's value in the
    /// view (outside `skip`), so the hand-back leaves live RAM's byte there.
    pub fn unflush(
        &mut self,
        flushed: &std::collections::BTreeMap<u32, Box<[u8]>>,
        skip: &[(u32, u32)],
    ) {
        let Some(base) = &self.base else {
            return;
        };
        let mut view = self.runtime.memory();
        for (&p, copy) in flushed {
            let Some(now) = view.physical_page(p).map(<[u8]>::to_vec) else {
                continue;
            };
            let page: Vec<(u32, u32)> = skip
                .iter()
                .map(|&(s, n)| (s, s + n))
                .filter(|&(s, e)| s < p + PAGE_SIZE && e > p)
                .collect();
            let skipped = |at: u32| page.iter().any(|&(s, e)| (s..e).contains(&at));
            let mut i = 0usize;
            while i < now.len() {
                if now[i] != copy[i] || skipped(p + i as u32) {
                    i += 1;
                    continue;
                }
                let run = i;
                while i < now.len() && now[i] == copy[i] && !skipped(p + i as u32) {
                    i += 1;
                }
                view.restore_from(base, p + run as u32, (i - run) as u32);
            }
        }
    }

    /// Merge `written` (from [`written`](Self::written), filtered as the
    /// caller likes) into `main`'s live RAM ([`Guest::hand_back`]). Takes
    /// this executor's memory guard, then `main`'s.
    /// The bytes a [`hand_back`](Self::hand_back) of `written` would count
    /// as conflicts (both sides changed them, to different values), without
    /// merging: `(physical, live, view, base)` each, at most `limit`.
    pub fn conflicts(
        &self,
        main: &Runtime,
        written: &[WrittenPage],
        limit: usize,
    ) -> Vec<(u32, u8, u8, u8)> {
        let Some(base) = &self.base else {
            return Vec::new();
        };
        let view = self.runtime.memory();
        let live = main.memory();
        live.conflicts_with(&view, base, written, limit)
    }

    pub fn hand_back(&mut self, main: &Runtime, written: &[WrittenPage]) -> HandBack {
        self.hand_back_owned(main, written, &[])
    }

    /// [`hand_back`](Self::hand_back) with the physical ranges `owned`
    /// view-owned ([`Guest::hand_back_owned`]).
    pub fn hand_back_owned(
        &mut self,
        main: &Runtime,
        written: &[WrittenPage],
        owned: &[(u32, u32)],
    ) -> HandBack {
        let Some(base) = &self.base else {
            return HandBack::default();
        };
        let view = self.runtime.memory();
        let mut live = main.memory();
        live.hand_back_owned(&view, base, written, owned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Dispatch;
    use crate::guest::Permissions;

    const CODE: u32 = 0x10000;
    const DATA: u32 = 0x20000;
    const STACK: u32 = 0x30000;
    const HOOK: u32 = 0x11000;

    fn assert_send<T: Send>() {}

    fn main_runtime() -> Arc<Runtime> {
        let mut g = Guest::new();
        g.map_zeroed(CODE, 0x2000, Permissions::RX).unwrap();
        g.map_zeroed(DATA, 0x1000, Permissions::RW).unwrap();
        g.map_zeroed(STACK - 0x1000, 0x1000, Permissions::RW)
            .unwrap();
        let mut d = Dispatch::default();
        // A Rust "guest function": reads [DATA], writes it + arg at DATA+4,
        // returns it in eax (stdcall, one argument).
        d.register_override(CODE, "body", |cpu, ctx, _| {
            let mut m = ctx.memory();
            let arg = m.u32(cpu.esp + 4)?;
            let v = m.u32(DATA)?;
            m.set_u32(DATA + 4, v + arg)?;
            let ret = m.u32(cpu.esp)?;
            cpu.esp += 8;
            cpu.eax = v;
            Ok(TransferAbi::ret(ret))
        });
        d.register_override(HOOK, "hle hook", |cpu, ctx, _| {
            let ret = ctx.memory().u32(cpu.esp)?;
            cpu.esp += 4;
            Ok(TransferAbi::ret(ret))
        });
        d.register_override(
            kernel::function_token(128),
            "KeQuerySystemTime",
            |_, _, _| unreachable!(),
        );
        Runtime::new(g, d)
    }

    #[test]
    fn executor_is_send() {
        assert_send::<SecondExecutor>();
        assert_send::<CancelHandle>();
    }

    #[test]
    fn a_call_runs_on_the_snapshot_and_its_writes_stay_private() {
        let main = main_runtime();
        main.memory().set_u32(DATA, 5).unwrap();
        let mut exec = SecondExecutor::new(&main, |addr, _| addr == CODE);
        assert!(matches!(
            exec.call(&mut CpuAbi::default(), CODE, &[]),
            Err(ExecutorError::NoSnapshot)
        ));
        exec.snapshot(&main);
        main.memory().set_u32(DATA, 9).unwrap();
        let mut cpu = CpuAbi {
            esp: STACK,
            ..CpuAbi::default()
        };
        let out = exec.call(&mut cpu, CODE, &[3]).unwrap();
        assert_eq!(
            (cpu.eax, cpu.esp),
            (5, STACK),
            "read the frozen 5, stdcall popped its argument"
        );
        assert!(
            out.accesses == 0,
            "a Rust override is not charged; generated code is"
        );
        assert_eq!(exec.runtime().memory().u32(DATA + 4).unwrap(), 8);
        assert_eq!(
            main.memory().u32(DATA + 4).unwrap(),
            0,
            "the view's write did not leak"
        );
        let written = exec.written();
        // DATA's page and the stack page.
        assert_eq!(written.len(), 2, "{written:x?}");
        let data_page = written
            .iter()
            .find(|p| p.lines == 1)
            .expect("DATA's line 0 changed");
        let merged = exec.hand_back(&main, &[*data_page]);
        assert_eq!(merged.bytes, 1);
        assert_eq!(main.memory().u32(DATA + 4).unwrap(), 8);
        assert_eq!(
            main.memory().u32(DATA).unwrap(),
            9,
            "the live side's own write stays"
        );
    }

    #[test]
    fn kernel_calls_and_disallowed_hle_are_refused_by_name() {
        let main = main_runtime();
        let mut exec = SecondExecutor::new(&main, |addr, _| addr == CODE);
        exec.snapshot(&main);
        let mut cpu = CpuAbi {
            esp: STACK,
            ..CpuAbi::default()
        };
        match exec.call(&mut cpu, HOOK, &[]) {
            Err(ExecutorError::Refused(r)) => {
                assert_eq!(
                    r,
                    Refusal {
                        target: HOOK,
                        name: "hle hook".into(),
                        ordinal: None,
                        return_pc: Some(crate::exec::HARNESS_EXIT)
                    }
                );
            }
            other => panic!("{other:?}"),
        }
        // Registered on main (KeQuerySystemTime) and not (KeTickCount's
        // neighbour KeBugCheck, 95): both refused, never run.
        for (ordinal, name) in [(128, "KeQuerySystemTime"), (95, "KeBugCheck")] {
            let mut cpu = CpuAbi {
                esp: STACK,
                ..CpuAbi::default()
            };
            let e = exec
                .call(&mut cpu, kernel::function_token(ordinal), &[0x1234])
                .unwrap_err();
            let ExecutorError::Refused(r) = &e else {
                panic!("{e:?}")
            };
            assert_eq!((r.ordinal, r.name.as_str()), (Some(ordinal), name));
            assert!(e.to_string().contains(name), "{e}");
        }
    }

    #[test]
    fn a_refusal_keeps_the_cpu_at_the_call_and_revert_undoes_a_change() {
        let main = main_runtime();
        let mut exec = SecondExecutor::new(&main, |addr, _| addr == CODE);
        exec.snapshot(&main);
        let mut cpu = CpuAbi {
            esp: STACK,
            ebx: 0x1234,
            ..CpuAbi::default()
        };
        exec.call(&mut cpu, CODE, &[3]).unwrap();
        assert!(exec.refused_cpu().is_none());
        let mut cpu = CpuAbi {
            esp: STACK,
            ebx: 0x1234,
            ..CpuAbi::default()
        };
        let e = exec
            .call(&mut cpu, kernel::function_token(128), &[0x99])
            .unwrap_err();
        assert!(matches!(e, ExecutorError::Refused(_)));
        let at = exec.refused_cpu().expect("the CPU at the refused call");
        assert_eq!(
            (at.eip, at.esp, at.ebx),
            (kernel::function_token(128), STACK - 8, 0x1234)
        );
        assert_eq!(
            exec.runtime().memory().u32(at.esp).unwrap(),
            crate::exec::HARNESS_EXIT,
            "the return address is on the view's stack"
        );
        // The first call wrote DATA+4 (= 0 + 3); revert puts the snapshot's
        // bytes back, so nothing of that page is handed back.
        let physical = main.memory().fold_address(DATA).unwrap() & !CONTIGUOUS_BASE;
        exec.revert(physical + 4, 4);
        assert_eq!(exec.runtime().memory().u32(DATA + 4).unwrap(), 0);
        let written = exec.written();
        assert!(
            written
                .iter()
                .all(|p| p.physical != physical || p.lines == 0),
            "{written:x?}"
        );
    }

    #[test]
    fn a_shared_watch_moves_the_main_sides_dirty_pages_to_the_view() {
        let main = main_runtime();
        let id = {
            let mut g = main.memory();
            let id = g.subscribe_writes("d3d");
            g.watch_writes(id, CONTIGUOUS_BASE, RAM_SIZE as u32)
                .unwrap();
            id
        };
        let mut exec = SecondExecutor::new(&main, |addr, _| addr == CODE);
        exec.share_watch("d3d");
        main.memory().set_u32(DATA, 1).unwrap();
        exec.snapshot(&main);
        assert!(
            main.memory().take_dirty(id).unwrap().is_empty(),
            "moved to the view"
        );
        let mut view = exec.runtime().memory();
        let moved = view.take_dirty(id).expect("the same id on the view");
        assert_eq!(moved.len(), 1, "{moved:x?}");
        view.set_u32(DATA + 8, 2).unwrap();
        assert_eq!(
            view.take_dirty(id).unwrap().len(),
            1,
            "the view's own write"
        );
        drop(view);
        // The executor's own subscription is unaffected.
        assert!(!exec.written().is_empty());
    }

    #[test]
    fn an_allowed_override_runs_and_cancel_stops_a_run() {
        let main = main_runtime();
        let mut exec = SecondExecutor::new(&main, |addr, _| addr == HOOK);
        exec.snapshot(&main);
        let mut cpu = CpuAbi {
            esp: STACK,
            ..CpuAbi::default()
        };
        exec.call(&mut cpu, HOOK, &[]).unwrap();
        assert_eq!(cpu.esp, STACK);
        exec.cancel_handle().cancel();
        let mut cpu = CpuAbi {
            esp: STACK,
            ..CpuAbi::default()
        };
        assert!(matches!(
            exec.call(&mut cpu, HOOK, &[]),
            Err(ExecutorError::Cancelled)
        ));
    }
}
