//! A single host thread drives separate guest CPUs and execution contexts.
//! Blocking HLE calls save an explicit return transfer, never a host stack.
use crate::layout::{ethread, kpcr, kthread};
use crate::ord;
use crate::{
    Action, Core, Error, Kernel,
    abi::{Args, Value},
    memory::round_up,
    status,
    sync::{self, Object, Wait},
    timers,
};
use runtime::{
    CpuAbi, Event, ExecCtx, Guest, GuestFault, LoadedImage, Runner, Runtime, TransferAbi,
    guest::{Access, PAGE_SIZE},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

pub const FX_SAVE_SIZE: u32 = 0x210;
/// The title process's base priority (NORMAL_BASE_PRIORITY); every thread
/// starts at it. The scheduler runs the highest-priority ready thread.
pub const PROCESS_BASE_PRIORITY: i8 = 8;
/// LOW_REALTIME_PRIORITY and HIGH_PRIORITY bound the two priority classes.
const LOW_REALTIME_PRIORITY: i32 = 16;
const HIGH_PRIORITY: i32 = 31;
/// ExecCtx thread ID used while a DPC routine runs. No KTHREAD has it, so
/// blocking calls and thread-only services stop inside a DPC.
pub const DPC_THREAD_ID: u32 = 0;
/// Kernel-owned DPC stack.
const DPC_STACK_SIZE: u32 = 0x4000;

struct ThreadLayout {
    tid: u32,
    bottom: u32,
    top: u32,
    tls: u32,
    esp: u32,
    start: u32,
}

#[derive(Debug, Clone)]
pub(crate) enum State {
    Ready,
    Running,
    Suspended,
    Waiting(Wait),
    Sleeping(u64),
    Terminated(u32),
}
pub(crate) struct Record {
    pub object: u32,
    pub tls: u32,
    pub state: State,
    pub suspend_count: u32,
    pub initial_cpu: Option<CpuAbi>,
    pub resume: TransferAbi,
    pub wake_value: Option<Value>,
    pub tib: [u32; 7],
    pub irql: u8,
    pub polling: bool,
    /// KeSetBasePriorityThread state: base priority and saturation (-1/0/1).
    pub base_priority: i8,
    /// KTHREAD Priority, the one the scheduler orders ready threads by.
    pub priority: i8,
    pub saturation: i8,
}

pub struct Thread {
    pub id: u32,
    pub object: u32,
    pub tls: u32,
    pub cpu: CpuAbi,
    pub ctx: ExecCtx,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    Complete {
        exits: BTreeMap<u32, u32>,
    },
    Stopped {
        thread: u32,
        transfer: TransferAbi,
    },
    /// No runnable thread and no timed wake. This is a diagnostic, not success.
    Deadlock {
        threads: Vec<u32>,
    },
    SliceLimit,
}

pub struct Machine {
    pub kernel: Arc<Kernel>,
    pub runtime: Arc<Runtime>,
    threads: BTreeMap<u32, Thread>,
    schedule: Vec<u32>,
    dropped_schedule: u64,
    stopped: Option<RunOutcome>,
    /// Context of the most recent DPC routine, kept for stop inspection.
    dpc: Option<Thread>,
    dpc_stack_top: u32,
    /// KDPCs already run in the current drain (since the last thread slice).
    drained: BTreeSet<u32>,
    pub max_transfers_per_slice: u64,
}

impl Machine {
    /// Attach before executing image entry; CPU must still have its initial
    /// single return word. Startup arithmetic and the TLS index store stay guest.
    pub fn new(
        runtime: Arc<Runtime>,
        kernel: Arc<Kernel>,
        mut cpu: CpuAbi,
        stack_bottom: u32,
        stack_top: u32,
    ) -> Result<Self, Error> {
        {
            let mut core = kernel.lock();
            if !core.threads.is_empty() {
                return Err(Error::Invalid("kernel already has attached threads"));
            }
            if !cpu.valid_abi()
                || cpu.esp
                    != stack_top
                        .checked_sub(4)
                        .ok_or(Error::Invalid("entry stack"))?
            {
                return Err(Error::Invalid("attach requires pristine entry stack"));
            }
            let mut memory = runtime.memory();
            let template = core.tls_template(&memory)?;
            let tls_size = template
                .as_ref()
                .map_or(0, |v| round_up(v.len() as u32, 16).unwrap() + 4);
            let tib_top = stack_top
                .checked_sub(FX_SAVE_SIZE)
                .ok_or(Error::Invalid("entry stack size"))?;
            let new_esp = tib_top
                .checked_sub(tls_size + 4)
                .filter(|v| *v >= stack_bottom)
                .ok_or(Error::Invalid("entry stack too small"))?;
            memory.check(new_esp, (stack_top - new_esp) as usize, Access::Write, 1)?;
            let return_pc = memory.u32(cpu.esp)?;
            let object = core
                .memory
                .internal(&mut memory, ethread::SIZE)
                .map_err(Error::Resource)?;
            let tls = if tls_size == 0 { 0 } else { tib_top - tls_size };
            core.initialize_stack(&mut memory, stack_top, tls, template.as_deref())?;
            cpu.esp = new_esp;
            cpu.fs_base = core.kpcr;
            memory.set_u32(new_esp, return_pc)?;
            core.initialize_thread(
                &mut memory,
                object,
                ThreadLayout {
                    tid: 1,
                    bottom: stack_bottom,
                    top: stack_top,
                    tls,
                    esp: cpu.esp,
                    start: cpu.eip,
                },
            )?;
            let kpcr = core.kpcr;
            core.threads.insert(
                1,
                Record {
                    object,
                    tls,
                    state: State::Ready,
                    suspend_count: 0,
                    initial_cpu: Some(cpu),
                    resume: TransferAbi::jump(cpu.eip),
                    wake_value: None,
                    tib: [u32::MAX, tib_top, stack_bottom, 0, 0, 0, kpcr],
                    irql: 0,
                    polling: false,
                    base_priority: PROCESS_BASE_PRIORITY,
                    priority: PROCESS_BASE_PRIORITY,
                    saturation: 0,
                },
            );
            core.objects.insert(object, Object::Thread { tid: 1 });
            core.ready.push_back(1);
            core.next_tid = 2;
        }
        let mut machine = Self {
            kernel,
            runtime,
            threads: BTreeMap::new(),
            schedule: Vec::new(),
            dropped_schedule: 0,
            stopped: None,
            dpc: None,
            dpc_stack_top: 0,
            drained: BTreeSet::new(),
            max_transfers_per_slice: 1_000_000,
        };
        machine.collect_threads()?;
        Ok(machine)
    }
    pub fn from_image(image: &mut LoadedImage) -> Result<Self, Error> {
        let kernel = Kernel::for_image(image)?;
        Self::new(
            Arc::clone(&image.runtime),
            kernel,
            image.cpu,
            image.stack_bottom,
            image.stack_top,
        )
    }
    pub fn thread(&self, id: u32) -> Option<&Thread> {
        self.threads.get(&id)
    }
    /// The CPU/context of the last DPC routine run (`DPC_THREAD_ID`). A
    /// `RunOutcome::Stopped` naming that thread refers to this context.
    pub fn dpc_thread(&self) -> Option<&Thread> {
        self.dpc.as_ref()
    }
    pub fn threads(&self) -> impl Iterator<Item = &Thread> {
        self.threads.values()
    }
    /// Retained scheduled thread IDs, oldest first (`limits::MAX_SCHEDULE`).
    pub fn schedule(&self) -> &[u32] {
        &self.schedule
    }
    /// Scheduled entries dropped from the front of `schedule()` by its budget.
    pub fn dropped_schedule(&self) -> u64 {
        self.dropped_schedule
    }
    /// After installing a missing HLE handler, retry the stopped kernel frame.
    /// Faults, bugchecks and firmware exits retain their terminal diagnostics.
    pub fn retry_kernel_call(&mut self) -> Result<(), Error> {
        let Some(RunOutcome::Stopped { thread, transfer }) = self.stopped.as_ref() else {
            return Err(Error::Invalid("machine is not stopped at a kernel call"));
        };
        let Some(guest) = self.threads.get(thread) else {
            return Err(Error::Invalid("a stopped DPC routine cannot be retried"));
        };
        let Some(Event::KernelCall(call)) = guest.ctx.event(transfer.event_id) else {
            return Err(Error::Invalid("faults cannot be retried as kernel entries"));
        };
        if matches!(
            call.ordinal,
            ord::HAL_RETURN_TO_FIRMWARE
                | ord::KE_BUG_CHECK
                | ord::KE_BUG_CHECK_EX
                | ord::PS_TERMINATE_SYSTEM_THREAD
        ) || transfer.kind != runtime::abi::STOP
        {
            return Err(Error::Invalid("terminal kernel call cannot be retried"));
        }
        let mut core = self.kernel.lock();
        let record = core.threads.get_mut(thread).unwrap();
        self.runtime
            .memory()
            .set_u8(record.object + kthread::STATE, 1)?;
        record.resume = TransferAbi::jump(guest.cpu.eip);
        record.state = State::Ready;
        core.ready.push_front(*thread);
        self.stopped = None;
        Ok(())
    }
    fn collect_threads(&mut self) -> Result<(), Error> {
        for (&id, record) in &mut self.kernel.lock().threads {
            if let Some(mut cpu) = record.initial_cpu.take() {
                let mut ctx = ExecCtx::new(Arc::clone(&self.runtime), id);
                if id != 1 {
                    let entry = cpu.eip;
                    let kernel = Arc::clone(&self.kernel);
                    // Owned completion survives child waits/stops and runs
                    // only when the guest SystemRoutine actually returns.
                    record.resume = ctx.call_guest(
                        &mut cpu,
                        &mut self.runtime.memory(),
                        entry,
                        move |_, ctx| {
                            let mut core = kernel.lock();
                            core.terminate(
                                ctx.thread_id,
                                status::SUCCESS,
                                &mut ctx.runtime().memory(),
                            )?;
                            core.yielded.insert(ctx.thread_id, TransferAbi::jump(0));
                            Ok(TransferAbi::stop(0))
                        },
                    )?;
                    self.runtime
                        .memory()
                        .set_u32(record.object + kthread::KERNEL_STACK, cpu.esp)?;
                }
                self.threads.insert(
                    id,
                    Thread {
                        id,
                        object: record.object,
                        tls: record.tls,
                        cpu,
                        ctx,
                    },
                );
            }
        }
        Ok(())
    }
    /// Run until all threads terminate, an unsupported boundary/fault, deadlock,
    /// or this many cooperative slices. The limit never implies guest success.
    pub fn run(&mut self, max_slices: u64) -> Result<RunOutcome, Error> {
        if let Some(outcome) = &self.stopped {
            return Ok(outcome.clone());
        }
        for _ in 0..max_slices {
            self.collect_threads()?;
            let tid = {
                let mut core = self.kernel.lock();
                let mut memory = self.runtime.memory();
                // Completions queued under a Guest guard are applied here, with
                // kernel state then guest memory locked in the usual order.
                self.kernel
                    .drain_deferred_into_failures(&mut core, &mut memory);
                core.wake(&mut memory)?;
                let live = core
                    .threads
                    .values()
                    .any(|r| !matches!(r.state, State::Terminated(_)));
                if live && core.dpcs.is_empty() && core.idle() {
                    let deadline = core.next_deadline();
                    if let Some(deadline) = deadline {
                        core.clock.advance_to(deadline)?;
                        core.write_tick(&mut memory)?;
                        // The polling threads have had their idle time: each
                        // gets a slice before the clock may move again, so a
                        // periodic timer cannot keep the clock (and its DPC)
                        // ahead of a yielding thread forever.
                        let core = &mut *core;
                        for id in &core.ready {
                            core.threads.get_mut(id).unwrap().polling = false;
                        }
                        core.wake(&mut memory)?;
                    }
                }
                // DISPATCH_LEVEL work runs before any thread gets a slice, and
                // a drain runs every queued DPC, chains of distinct DPCs
                // included, in queue order. Only a DPC that comes round again
                // in the same drain (one that re-arms its own timer due now,
                // say) stays queued until a ready thread's next slice; DPCs
                // queued behind it still run now. So a DPC cycle cannot
                // starve threads, and finite DPC work still precedes them.
                let next = if core.ready.is_empty() {
                    (!core.dpcs.is_empty()).then_some(0)
                } else {
                    core.dpcs.iter().position(|d| !self.drained.contains(d))
                };
                if live && let Some(index) = next {
                    let call = core.take_dpc(&mut memory, index)?.expect("queued DPC");
                    self.drained.insert(call.arguments[0]);
                    drop(memory);
                    drop(core);
                    if let Some(outcome) = self.run_dpc(call)? {
                        return Ok(outcome);
                    }
                    continue;
                }
                self.drained.clear();
                let Some(tid) = core.pop_ready() else {
                    let pending: Vec<_> = core
                        .threads
                        .iter()
                        .filter(|(_, r)| !matches!(r.state, State::Terminated(_)))
                        .map(|(&id, _)| id)
                        .collect();
                    if pending.is_empty() {
                        let exits = core
                            .threads
                            .iter()
                            .map(|(&id, r)| {
                                (
                                    id,
                                    match r.state {
                                        State::Terminated(s) => s,
                                        _ => unreachable!(),
                                    },
                                )
                            })
                            .collect();
                        return Ok(RunOutcome::Complete { exits });
                    }
                    return Ok(RunOutcome::Deadlock { threads: pending });
                };
                let kpcr = core.kpcr;
                let record = core.threads.get_mut(&tid).unwrap();
                if !matches!(record.state, State::Ready) {
                    return Err(Error::Invalid("ready queue state mismatch"));
                }
                record.state = State::Running;
                record.polling = false;
                let thread = self.threads.get_mut(&tid).unwrap();
                if let Some(value) = record.wake_value.take() {
                    match value {
                        Value::U32(v) => thread.cpu.eax = v,
                        Value::U64(v) => {
                            thread.cpu.eax = v as u32;
                            thread.cpu.edx = (v >> 32) as u32;
                        }
                        Value::Void => {}
                    }
                }
                for (i, value) in record.tib.iter().enumerate() {
                    memory.set_u32(kpcr + i as u32 * 4, *value)?;
                }
                memory.set_u32(kpcr + kpcr::SELF_PCR, kpcr)?;
                memory.set_u32(kpcr + kpcr::PRCB, kpcr + kpcr::CURRENT_THREAD)?;
                memory.set_u8(kpcr + kpcr::IRQL, record.irql)?;
                memory.set_u32(kpcr + kpcr::CURRENT_THREAD, record.object)?;
                memory.set_u32(record.object + kthread::KERNEL_STACK, thread.cpu.esp)?;
                memory.set_u8(record.object + kthread::STATE, 2)?; // Running
                tid
            };
            crate::push_bounded(
                &mut self.schedule,
                crate::limits::MAX_SCHEDULE,
                &mut self.dropped_schedule,
                tid,
            );
            let thread = self.threads.get_mut(&tid).unwrap();
            let resume = self.kernel.lock().threads[&tid].resume;
            let transfer = Runner {
                max_transfers: self.max_transfers_per_slice,
            }
            .run_from(&mut thread.cpu, &mut thread.ctx, resume);
            {
                let mut core = self.kernel.lock();
                let mut memory = self.runtime.memory();
                let kpcr = core.kpcr;
                let record = core.threads.get_mut(&tid).unwrap();
                for i in 0..7 {
                    record.tib[i] = memory.u32(kpcr + i as u32 * 4)?;
                }
                record.irql = memory.u8(kpcr + kpcr::IRQL)?;
                memory.set_u32(record.object + kthread::KERNEL_STACK, thread.cpu.esp)?;
                let yielded = core.yielded.remove(&tid);
                if transfer.kind == runtime::abi::STOP
                    && let Some(resume) = yielded
                {
                    core.threads.get_mut(&tid).unwrap().resume = resume;
                    continue;
                }
                if matches!(
                    thread.ctx.event(transfer.event_id),
                    Some(Event::HarnessExit)
                ) {
                    core.terminate(tid, thread.cpu.eax, &mut memory)?;
                    continue;
                }
            }
            let outcome = RunOutcome::Stopped {
                thread: tid,
                transfer,
            };
            self.stopped = Some(outcome.clone());
            return Ok(outcome);
        }
        Ok(RunOutcome::SliceLimit)
    }
    /// Call DeferredRoutine(Dpc, DeferredContext, SystemArgument1,
    /// SystemArgument2) (stdcall) on the DPC stack at DISPATCH_LEVEL, then
    /// restore the interrupted IRQL. Returns a stop outcome if the routine
    /// does not return.
    fn run_dpc(&mut self, call: timers::DpcCall) -> Result<Option<RunOutcome>, Error> {
        let mut cpu = CpuAbi::xbox_thread();
        let mut ctx = ExecCtx::new(Arc::clone(&self.runtime), DPC_THREAD_ID);
        let (resume, old_irql) = {
            let mut core = self.kernel.lock();
            let mut memory = self.runtime.memory();
            if self.dpc_stack_top == 0 {
                let base = core
                    .memory
                    .internal(&mut memory, DPC_STACK_SIZE)
                    .map_err(Error::Resource)?;
                self.dpc_stack_top = base + DPC_STACK_SIZE;
            }
            cpu.fs_base = core.kpcr;
            cpu.esp = self.dpc_stack_top - 16;
            for (i, argument) in call.arguments.iter().enumerate() {
                memory.set_u32(cpu.esp + i as u32 * 4, *argument)?;
            }
            core.dpc_complete = false;
            let old_irql = core.dispatch_level(&mut memory)?;
            let kernel = Arc::clone(&self.kernel);
            let resume = ctx.call_guest(&mut cpu, &mut memory, call.routine, move |_, _| {
                kernel.lock().dpc_complete = true;
                Ok(TransferAbi::stop(0))
            })?;
            (resume, old_irql)
        };
        crate::push_bounded(
            &mut self.schedule,
            crate::limits::MAX_SCHEDULE,
            &mut self.dropped_schedule,
            DPC_THREAD_ID,
        );
        let transfer = Runner {
            max_transfers: self.max_transfers_per_slice,
        }
        .run_from(&mut cpu, &mut ctx, resume);
        let completed = {
            let mut core = self.kernel.lock();
            let completed =
                transfer.kind == runtime::abi::STOP && std::mem::take(&mut core.dpc_complete);
            if completed {
                self.runtime
                    .memory()
                    .set_u8(core.kpcr + kpcr::IRQL, old_irql)?;
            }
            completed
        };
        self.dpc = Some(Thread {
            id: DPC_THREAD_ID,
            object: 0,
            tls: 0,
            cpu,
            ctx,
        });
        if completed {
            return Ok(None);
        }
        let outcome = RunOutcome::Stopped {
            thread: DPC_THREAD_ID,
            transfer,
        };
        self.stopped = Some(outcome.clone());
        Ok(Some(outcome))
    }
}

impl Core {
    /// Take the next thread to run: the highest KTHREAD priority ready,
    /// first queued among equals (so equal priorities round-robin). A thread
    /// that has just yielded its slice is passed over while any other thread
    /// is ready, whatever that thread's priority (KiSelectReadyThread(1)).
    fn pop_ready(&mut self) -> Option<u32> {
        let index = self.next_ready()?;
        self.yielder = None;
        self.ready.remove(index)
    }
    /// Index in `ready` of the thread `pop_ready` would take.
    fn next_ready(&self) -> Option<usize> {
        let yielder = self.yielder.filter(|y| self.ready.iter().any(|id| id != y));
        self.ready
            .iter()
            .enumerate()
            .filter(|(_, id)| Some(**id) != yielder)
            .max_by_key(|(i, id)| (self.threads[*id].priority, std::cmp::Reverse(*i)))
            .map(|(i, _)| i)
    }
    /// Whether logical time may advance: no thread is ready, or the threads
    /// that would be scheduled are all polling. Those are the next pick and
    /// every ready thread of its priority or higher; lower-priority threads
    /// cannot run before them, so one that is never scheduled does not hold
    /// the clock (and the timers that would release the pollers) still.
    fn idle(&self) -> bool {
        let Some(index) = self.next_ready() else {
            return true;
        };
        let floor = self.threads[&self.ready[index]].priority;
        self.ready
            .iter()
            .map(|id| &self.threads[id])
            .filter(|r| r.priority >= floor)
            .all(|r| r.polling)
    }
    fn next_deadline(&self) -> Option<u64> {
        self.threads
            .values()
            .filter_map(|r| match &r.state {
                State::Sleeping(d) => Some(*d),
                State::Waiting(Wait::Objects { deadline, .. }) => *deadline,
                _ => None,
            })
            .chain(self.next_timer_deadline())
            .filter(|d| *d > self.clock.counter())
            .min()
    }
    fn tls_template(&self, memory: &Guest) -> Result<Option<Vec<u8>>, Error> {
        let Some(tls) = self.tls else {
            return Ok(None);
        };
        let raw_size = tls
            .raw_end
            .checked_sub(tls.raw_start)
            .ok_or(Error::Invalid("TLS raw bounds"))?;
        let total = raw_size
            .checked_add(tls.zero_fill)
            .filter(|n| *n <= 16 * 1024 * 1024)
            .ok_or(Error::Invalid("TLS size"))?;
        if tls.callbacks_addr != 0 && memory.u32(tls.callbacks_addr)? != 0 {
            return Err(Error::Invalid("TLS callbacks unsupported"));
        }
        let mut bytes = vec![0; total as usize];
        memory.read_bytes(tls.raw_start, &mut bytes[..raw_size as usize], 1)?;
        Ok(Some(bytes))
    }
    fn initialize_stack(
        &self,
        memory: &mut Guest,
        stack_top: u32,
        tls: u32,
        template: Option<&[u8]>,
    ) -> Result<(), GuestFault> {
        memory.set_u16(stack_top - FX_SAVE_SIZE, 0x027f)?;
        memory.set_u32(stack_top - FX_SAVE_SIZE + 24, 0x1f80)?;
        if tls != 0 {
            memory.set_u32(tls, tls + 4)?;
            if let Some(bytes) = template {
                memory.write_bytes(tls + 4, bytes, 1)?;
            }
        }
        Ok(())
    }
    fn initialize_thread(
        &self,
        memory: &mut Guest,
        object: u32,
        layout: ThreadLayout,
    ) -> Result<(), GuestFault> {
        let ThreadLayout {
            tid,
            bottom,
            top,
            tls,
            esp,
            start,
        } = layout;
        sync::header(memory, object, 6, (kthread::SIZE / 4) as u8, 0)?;
        memory.set_u32(object + kthread::MUTANT_LIST, object + kthread::MUTANT_LIST)?;
        memory.set_u32(
            object + kthread::MUTANT_LIST_BLINK,
            object + kthread::MUTANT_LIST,
        )?;
        memory.set_u32(object + kthread::STACK_BASE, top)?;
        memory.set_u32(object + kthread::STACK_LIMIT, bottom)?;
        memory.set_u32(object + kthread::KERNEL_STACK, esp)?;
        memory.set_u32(object + kthread::TLS_DATA, tls)?;
        memory.set_u8(object + kthread::PRIORITY, PROCESS_BASE_PRIORITY as u8)?;
        memory.set_u8(object + kthread::BASE_PRIORITY, PROCESS_BASE_PRIORITY as u8)?;
        memory.set_u8(object + kthread::STATE, 1)?; // Ready
        memory.set_u64(object + ethread::CREATE_TIME, self.clock.system_time())?;
        memory.set_u32(object + ethread::UNIQUE_THREAD, tid)?;
        memory.set_u32(object + ethread::START_ADDRESS, start)?;
        memory.set_u32(object + ethread::IRP_LIST, object + ethread::IRP_LIST)?;
        memory.set_u32(object + ethread::IRP_LIST_BLINK, object + ethread::IRP_LIST)
    }
    pub(crate) fn terminate(
        &mut self,
        tid: u32,
        status: u32,
        memory: &mut Guest,
    ) -> Result<(), GuestFault> {
        if !self.threads.contains_key(&tid) {
            return Ok(());
        }
        self.abandon_mutants(memory, tid)?;
        let record = self.threads.get_mut(&tid).unwrap();
        record.state = State::Terminated(status);
        memory.set_u32(record.object + kthread::SIGNAL_STATE, 1)?;
        memory.set_u8(record.object + kthread::STATE, 4)?;
        memory.set_u8(record.object + kthread::HAS_TERMINATED, 1)?;
        memory.set_u64(record.object + ethread::EXIT_TIME, self.clock.system_time())?;
        memory.set_u32(record.object + ethread::EXIT_STATUS, status)?;
        self.wake(memory)
    }
    pub(crate) fn thread_call(
        &mut self,
        memory: &mut Guest,
        a: &Args,
        tid: u32,
    ) -> Result<Action, GuestFault> {
        Ok(match a.export.ordinal {
            ord::KE_GET_CURRENT_THREAD => match self.threads.get(&tid) {
                Some(r) => Action::value(r.object),
                None => Action::Stop("thread lookup requires Machine"),
            },
            ord::KE_ENTER_CRITICAL_REGION | ord::KE_LEAVE_CRITICAL_REGION => {
                let Some(record) = self.threads.get(&tid) else {
                    return Ok(Action::Stop("critical region requires Machine"));
                };
                let address = record.object + kthread::KERNEL_APC_DISABLE;
                let old = memory.u32(address)? as i32;
                let new = if a.export.ordinal == ord::KE_ENTER_CRITICAL_REGION {
                    old.checked_sub(1)
                } else {
                    old.checked_add(1).filter(|v| *v <= 0)
                };
                let Some(new) = new else {
                    return Ok(Action::Stop("unbalanced critical region"));
                };
                memory.set_u32(address, new as u32)?;
                Action::Complete(Value::Void)
            }
            ord::KE_SET_BASE_PRIORITY_THREAD => {
                let thread = a.get(0);
                let Some(Object::Thread { tid: target }) = self.ensure_object(memory, thread)?
                else {
                    return Ok(Action::Stop("KeSetBasePriorityThread requires a KTHREAD"));
                };
                memory.check(thread + kthread::SATURATION, 2, Access::Write, 1)?;
                memory.check(thread + kthread::BASE_PRIORITY, 1, Access::Write, 1)?;
                let increment = a.get(1) as i32;
                let record = self.threads.get_mut(&target).unwrap();
                let process = PROCESS_BASE_PRIORITY as i32;
                // KeSetBasePriorityThread: the old increment, where a
                // saturated thread reports +/-16.
                let old = if record.saturation != 0 {
                    (HIGH_PRIORITY + 1) / 2 * record.saturation as i32
                } else {
                    record.base_priority as i32 - process
                };
                record.saturation =
                    if increment.unsigned_abs() >= (HIGH_PRIORITY as u32).div_ceil(2) {
                        increment.signum() as i8
                    } else {
                        0
                    };
                let (low, high) = if process >= LOW_REALTIME_PRIORITY {
                    (LOW_REALTIME_PRIORITY, HIGH_PRIORITY)
                } else {
                    (1, LOW_REALTIME_PRIORITY - 1)
                };
                let base = process.saturating_add(increment).clamp(low, high);
                record.base_priority = base as i8;
                // A saturated thread runs at the top or bottom of its class.
                let priority = match record.saturation {
                    1 => high,
                    -1 => low,
                    _ => base,
                };
                record.priority = priority as i8;
                memory.set_u8(thread + kthread::SATURATION, record.saturation as u8)?;
                memory.set_u8(thread + kthread::PRIORITY, priority as u8)?;
                memory.set_u8(thread + kthread::BASE_PRIORITY, base as u8)?;
                Action::value(old as u32)
            }
            ord::PS_TERMINATE_SYSTEM_THREAD => {
                if self.threads.contains_key(&tid) {
                    Action::Exit(a.get(0))
                } else {
                    Action::Stop("termination outside Machine")
                }
            }
            ord::NT_YIELD_EXECUTION => {
                if !self.threads.contains_key(&tid) {
                    return Ok(Action::Stop("yield requires Machine"));
                }
                if memory.u8(self.kpcr + crate::layout::kpcr::IRQL)? != 0 {
                    return Ok(Action::Stop("yield above PASSIVE_LEVEL"));
                }
                // Another ready thread of any priority takes the slice
                // (SUCCESS). Alone, the yield is a poll: with a deadline
                // pending the scheduler may advance idle time to it, run what
                // that delivers and then resume this thread with
                // NO_YIELD_PERFORMED; with none it returns at once.
                let has_other = self.ready.iter().any(|other| *other != tid);
                if !has_other && self.next_deadline().is_none() {
                    Action::status(status::NO_YIELD_PERFORMED)
                } else {
                    memory.set_u8(self.threads[&tid].object + kthread::STATE, 1)?;
                    let record = self.threads.get_mut(&tid).unwrap();
                    record.state = State::Ready;
                    record.polling = true;
                    self.ready.push_back(tid);
                    if has_other {
                        self.yielder = Some(tid);
                    }
                    Action::Block(Value::U32(if has_other {
                        status::SUCCESS
                    } else {
                        status::NO_YIELD_PERFORMED
                    }))
                }
            }
            ord::KE_DELAY_EXECUTION_THREAD => {
                if a.get(0) as u8 > 1 {
                    return Ok(Action::status(status::INVALID_PARAMETER));
                }
                if a.boolean(1) {
                    return Ok(Action::Stop("alertable delay requires APC/alert delivery"));
                }
                let deadline = match self.clock.deadline(memory.u64(a.get(2))? as i64) {
                    Ok(v) => v,
                    Err(_) => return Ok(Action::status(status::INVALID_PARAMETER)),
                };
                if !self.threads.contains_key(&tid) {
                    return Ok(Action::Stop("delay requires Machine"));
                }
                if memory.u8(self.kpcr + crate::layout::kpcr::IRQL)? > 1 {
                    return Ok(Action::Stop("delay above APC_LEVEL"));
                }
                memory.set_u8(self.threads[&tid].object + kthread::STATE, 5)?;
                let record = self.threads.get_mut(&tid).unwrap();
                record.state = State::Sleeping(deadline);
                record.polling = deadline <= self.clock.counter();
                Action::Block(Value::U32(status::SUCCESS))
            }
            ord::NT_RESUME_THREAD => {
                let object = if a.get(0) == 0xffff_fffe {
                    match self.threads.get(&tid) {
                        Some(r) => r.object,
                        None => return Ok(Action::status(status::INVALID_HANDLE)),
                    }
                } else {
                    match self.handles.get(a.get(0)) {
                        Ok(v) => v,
                        Err(s) => return Ok(Action::status(s)),
                    }
                };
                let target = match self.ensure_object(memory, object)? {
                    Some(Object::Thread { tid }) => tid,
                    Some(_) => return Ok(Action::status(status::OBJECT_TYPE_MISMATCH)),
                    None => return Ok(Action::Stop("invalid thread dispatcher header")),
                };
                if a.get(1) != 0 {
                    memory.check(a.get(1), 4, Access::Write, 1)?;
                }
                let record = self.threads.get_mut(&target).unwrap();
                let old = record.suspend_count;
                if old > 0 {
                    record.suspend_count -= 1;
                    memory.set_u8(
                        record.object + kthread::SUSPEND_COUNT,
                        record.suspend_count as u8,
                    )?;
                    if record.suspend_count == 0 && matches!(record.state, State::Suspended) {
                        memory.set_u8(record.object + kthread::STATE, 1)?;
                        record.state = State::Ready;
                        self.ready.push_back(target);
                    }
                }
                if a.get(1) != 0 {
                    memory.set_u32(a.get(1), old)?;
                }
                Action::status(status::SUCCESS)
            }
            ord::PS_CREATE_SYSTEM_THREAD_EX => {
                if !self.threads.contains_key(&tid) {
                    return Ok(Action::Stop("thread creation requires Machine"));
                }
                memory.check(a.get(0), 4, Access::Write, 1)?;
                if a.get(4) != 0 {
                    memory.check(a.get(4), 4, Access::Write, 1)?;
                }
                if a.boolean(8) {
                    return Ok(Action::Stop("debugger thread memory policy unsupported"));
                }
                if a.get(1) != 0 {
                    return Ok(Action::Stop("thread extension layout unverified"));
                }
                if a.get(5) == 0 {
                    return Ok(Action::status(status::INVALID_PARAMETER));
                }
                let template = match self.tls_template(memory) {
                    Ok(v) => v,
                    Err(Error::Fault(f)) => return Err(f),
                    Err(_) => return Ok(Action::Stop("invalid or unsupported TLS directory")),
                };
                let required = template
                    .as_ref()
                    .map_or(0, |v| round_up(v.len() as u32, 16).unwrap() + 4);
                let tls_size = if a.get(3) == 0 {
                    0
                } else {
                    match a
                        .get(3)
                        .checked_sub(4)
                        .and_then(|n| round_up(n, 16))
                        .and_then(|n| n.checked_add(4))
                    {
                        Some(n) => n,
                        None => return Ok(Action::status(status::INVALID_PARAMETER)),
                    }
                };
                if tls_size != 0 && tls_size < required {
                    return Ok(Action::status(status::INVALID_PARAMETER));
                }
                let Some(stack_size) = round_up(a.get(2).max(0x4000), PAGE_SIZE) else {
                    return Ok(Action::status(status::NO_MEMORY));
                };
                if stack_size < tls_size.saturating_add(FX_SAVE_SIZE + 64) {
                    return Ok(Action::status(status::INVALID_PARAMETER));
                }
                let Some(next_tid) = self.next_tid.checked_add(1) else {
                    return Ok(Action::status(status::INSUFFICIENT_RESOURCES));
                };
                // The stack, guard and object mappings are not undone on a
                // later failure, so the handle must be known to fit first.
                if let Err(s) = self.handles.check_capacity() {
                    return Ok(Action::status(s));
                }
                let (object, bottom, top) = match self.memory.guarded_thread(memory, stack_size) {
                    Ok(v) => v,
                    Err(s) => return Ok(Action::status(s)),
                };
                let id = self.next_tid;
                self.next_tid = next_tid;
                let tib_top = top - FX_SAVE_SIZE;
                let tls = if tls_size == 0 { 0 } else { tib_top - tls_size };
                self.initialize_stack(memory, top, tls, template.as_deref())?;
                let stack = if tls == 0 { tib_top } else { tls };
                let system_routine = a.get(9);
                let (entry, esp) = if system_routine == 0 {
                    memory.set_u32(stack - 4, a.get(6))?;
                    (a.get(5), stack - 4)
                } else {
                    memory.set_u32(stack - 8, a.get(5))?;
                    memory.set_u32(stack - 4, a.get(6))?;
                    (system_routine, stack - 8)
                };
                self.initialize_thread(
                    memory,
                    object,
                    ThreadLayout {
                        tid: id,
                        bottom,
                        top,
                        tls,
                        esp,
                        start: a.get(5),
                    },
                )?;
                memory.set_u8(object + kthread::SUSPEND_COUNT, a.boolean(7) as u8)?;
                let mut cpu = CpuAbi::xbox_thread();
                cpu.eip = entry;
                cpu.esp = esp;
                cpu.fs_base = self.kpcr;
                let handle = match self.handles.insert(object) {
                    Ok(v) => v,
                    Err(s) => return Ok(Action::status(s)),
                };
                let suspended = a.boolean(7);
                memory.set_u8(object + kthread::STATE, if suspended { 5 } else { 1 })?;
                self.threads.insert(
                    id,
                    Record {
                        object,
                        tls,
                        state: if suspended {
                            State::Suspended
                        } else {
                            State::Ready
                        },
                        suspend_count: suspended as u32,
                        initial_cpu: Some(cpu),
                        resume: TransferAbi::jump(entry),
                        wake_value: None,
                        tib: [u32::MAX, tib_top, bottom, 0, 0, 0, self.kpcr],
                        irql: 0,
                        polling: false,
                        base_priority: PROCESS_BASE_PRIORITY,
                        priority: PROCESS_BASE_PRIORITY,
                        saturation: 0,
                    },
                );
                self.objects.insert(object, Object::Thread { tid: id });
                if !suspended {
                    self.ready.push_back(id);
                }
                memory.set_u32(a.get(0), handle)?;
                if a.get(4) != 0 {
                    memory.set_u32(a.get(4), id)?;
                }
                Action::status(status::SUCCESS)
            }
            _ => Action::Stop("declared export has no handler"),
        })
    }
}

impl Kernel {
    /// Compute [[StackBase + signed_tls_index * 4]] + 8 for a saved guest
    /// thread, following its own TIB rather than another thread's live FS.
    pub fn last_error_cell(&self, thread_id: u32, tls_index: i32) -> Result<u32, Error> {
        let runtime = self.runtime()?;
        let core = self.lock();
        let record = core
            .threads
            .get(&thread_id)
            .ok_or(Error::Resource(status::INVALID_HANDLE))?;
        let memory = runtime.memory();
        let directory = core.tls.ok_or(Error::Invalid("thread has no XBE TLS"))?;
        let payload_size = directory
            .raw_end
            .checked_sub(directory.raw_start)
            .and_then(|v| v.checked_add(directory.zero_fill))
            .ok_or(Error::Invalid("TLS size"))?;
        let base = if matches!(record.state, State::Running) {
            memory.u32(core.kpcr + 4)?
        } else {
            record.tib[1]
        };
        let slot = (base as i64)
            .checked_add(tls_index as i64 * 4)
            .and_then(|v| u32::try_from(v).ok())
            .ok_or(Error::Invalid("TLS slot arithmetic"))?;
        if record.tls == 0 || slot != record.tls || payload_size < 12 {
            return Err(Error::Invalid(
                "TLS index does not select this thread's last-error cell",
            ));
        }
        let payload = memory.u32(slot)?;
        if payload != record.tls + 4 {
            return Err(Error::Invalid("TLS payload pointer changed"));
        }
        let cell = payload
            .checked_add(8)
            .ok_or(Error::Invalid("TLS cell arithmetic"))?;
        memory.check(cell, 4, Access::Write, 1)?;
        Ok(cell)
    }
}
