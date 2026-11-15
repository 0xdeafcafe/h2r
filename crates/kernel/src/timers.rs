//! Kernel timers (KTIMER) and deferred procedure calls (KDPC) on logical time.
//!
//! An inserted timer is held in Rust by guest address with its deadline in
//! `Clock::counter` units; the guest object keeps the documented fields
//! (header type/Inserted/SignalState, DueTime, Dpc, Period). Every scheduler
//! wake pass expires due timers one at a time in deadline order: the timer
//! is signalled, a periodic timer is re-armed, and its DPC (if any) is
//! queued; waiters are tested before each expiry (see `Core::wake`), so a
//! WaitAny sees the earliest expiry even when time jumps past several. Timer deadlines take part in the scheduler's idle/polling time
//! advance exactly like sleeps. `Machine::run` executes queued DPCs, one per
//! slice and before any thread, on a kernel-owned stack at DISPATCH_LEVEL;
//! a drain runs every queued DPC in order, except that one already run in
//! it stays queued until a ready thread's next slice.
//! A thread whose kernel call queued a DPC and returns below DISPATCH_LEVEL
//! yields first, so the DPC runs before the caller's next instruction.
use crate::layout::kpcr;
use crate::ord;
use crate::{
    Action, Core,
    abi::{Args, Value},
    sync::{self, Object},
};
use runtime::{Guest, GuestFault, guest::Access};

pub(crate) const KTIMER_SIZE: u32 = 0x28;
pub(crate) const KDPC_SIZE: u32 = 0x1c;
/// DISPATCHER_HEADER types.
pub(crate) const TIMER_NOTIFICATION: u8 = 8;
pub(crate) const TIMER_SYNCHRONIZATION: u8 = 9;
/// KOBJECTS DpcObject.
pub(crate) const DPC_OBJECT: u16 = 19;
const HEADER_INSERTED: u32 = 3;
const DUE_TIME: u32 = 0x10;
const TIMER_LIST_ENTRY: u32 = 0x18;
const TIMER_DPC: u32 = 0x20;
const PERIOD: u32 = 0x24;
const DPC_INSERTED: u32 = 2;
const DPC_ROUTINE: u32 = 0xc;
const DPC_CONTEXT: u32 = 0x10;
const DPC_ARGUMENT1: u32 = 0x14;
const DPC_ARGUMENT2: u32 = 0x18;
/// Milliseconds to clock ticks (100 ns).
const TICKS_PER_MS: u64 = 10_000;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Timer {
    pub deadline: u64,
    period_ms: u32,
    dpc: u32,
}

/// A DPC ready to run: routine and its four stdcall arguments.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DpcCall {
    pub routine: u32,
    pub arguments: [u32; 4],
}

/// One entry of the DPC queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Queued {
    /// A guest KDPC, by address.
    Kdpc(u32),
    /// A periodic host source's due periods, by ID (`crate::periodic`).
    Host(u32),
}

/// What `Core::take_dpc` hands `Machine::run`.
pub(crate) enum DpcWork {
    Guest(DpcCall),
    Host(u32),
}

impl Core {
    pub(crate) fn timer_call(
        &mut self,
        memory: &mut Guest,
        a: &Args,
    ) -> Result<Action, GuestFault> {
        match a.export.ordinal {
            ord::KE_INITIALIZE_DPC => {
                let dpc = a.get(0);
                if self.dpcs.contains(&Queued::Kdpc(dpc)) {
                    return Ok(Action::Stop("reinitializing a queued DPC"));
                }
                memory.check(dpc, KDPC_SIZE as usize, Access::Write, 1)?;
                let mut bytes = [0u8; KDPC_SIZE as usize];
                bytes[0..2].copy_from_slice(&DPC_OBJECT.to_le_bytes());
                bytes[DPC_ROUTINE as usize..][..4].copy_from_slice(&a.get(1).to_le_bytes());
                bytes[DPC_CONTEXT as usize..][..4].copy_from_slice(&a.get(2).to_le_bytes());
                memory.write_bytes(dpc, &bytes, 1)?;
                Ok(Action::Complete(Value::Void))
            }
            ord::KE_INITIALIZE_TIMER_EX => {
                let timer = a.get(0);
                if a.get(1) > 1 {
                    return Ok(Action::Stop("invalid timer type"));
                }
                if self.timers.contains_key(&timer) {
                    return Ok(Action::Stop("reinitializing an inserted timer"));
                }
                memory.check(timer, KTIMER_SIZE as usize, Access::Write, 1)?;
                sync::header(
                    memory,
                    timer,
                    TIMER_NOTIFICATION + a.get(1) as u8,
                    (KTIMER_SIZE / 4) as u8,
                    0,
                )?;
                memory.set_u64(timer + DUE_TIME, 0)?;
                memory.set_u32(timer + TIMER_LIST_ENTRY, 0)?;
                memory.set_u32(timer + TIMER_LIST_ENTRY + 4, 0)?;
                memory.set_u32(timer + TIMER_DPC, 0)?;
                memory.set_u32(timer + PERIOD, 0)?;
                Ok(Action::Complete(Value::Void))
            }
            ord::KE_SET_TIMER | ord::KE_SET_TIMER_EX => {
                let timer = a.get(0);
                let due = (a.get(1) as u64 | (a.get(2) as u64) << 32) as i64;
                let (period, dpc) = if a.export.ordinal == ord::KE_SET_TIMER {
                    (0, a.get(3))
                } else {
                    (a.get(3) as i32, a.get(4))
                };
                if period < 0 {
                    return Ok(Action::Stop("negative timer period"));
                }
                if !matches!(
                    self.ensure_object(memory, timer)?,
                    Some(Object::Timer { .. })
                ) {
                    return Ok(Action::Stop("KeSetTimer requires an initialized timer"));
                }
                if dpc != 0 && memory.u16(dpc)? != DPC_OBJECT {
                    return Ok(Action::Stop("KeSetTimer requires an initialized DPC"));
                }
                let Ok(deadline) = self.clock.deadline(due) else {
                    return Ok(Action::Stop("timer due time outside the logical clock"));
                };
                memory.check(timer, KTIMER_SIZE as usize, Access::Write, 1)?;
                let was_inserted = self.timers.remove(&timer).is_some();
                memory.set_u32(timer + 4, 0)?;
                memory.set_u32(timer + TIMER_DPC, dpc)?;
                memory.set_u32(timer + PERIOD, period as u32)?;
                self.insert_timer(
                    memory,
                    timer,
                    Timer {
                        deadline,
                        period_ms: period as u32,
                        dpc,
                    },
                )?;
                // A due time already reached expires now, as on retail.
                self.wake(memory)?;
                Ok(Action::value(was_inserted as u32))
            }
            ord::KE_CANCEL_TIMER => {
                let timer = a.get(0);
                if !matches!(
                    self.ensure_object(memory, timer)?,
                    Some(Object::Timer { .. })
                ) {
                    return Ok(Action::Stop("KeCancelTimer requires an initialized timer"));
                }
                let was_inserted = self.timers.remove(&timer).is_some();
                if was_inserted {
                    memory.set_u8(timer + HEADER_INSERTED, 0)?;
                }
                Ok(Action::value(was_inserted as u32))
            }
            _ => Ok(Action::Stop("declared export has no handler")),
        }
    }
    fn insert_timer(
        &mut self,
        memory: &mut Guest,
        address: u32,
        timer: Timer,
    ) -> Result<(), GuestFault> {
        memory.set_u8(address + HEADER_INSERTED, 1)?;
        memory.set_u64(address + DUE_TIME, timer.deadline)?;
        self.timers.insert(address, timer);
        Ok(())
    }
    /// Deadline of the earliest timer due by `now`.
    pub(crate) fn next_due_timer(&self, now: u64) -> Option<u64> {
        self.next_timer_deadline().filter(|d| *d <= now)
    }
    /// Expire the earliest timer due by `now` (ties by address). The wake
    /// pass calls this once per timer, testing waiters in between.
    pub(crate) fn expire_next_timer(
        &mut self,
        memory: &mut Guest,
        now: u64,
    ) -> Result<(), GuestFault> {
        let Some((_, address)) = self
            .timers
            .iter()
            .filter(|(_, t)| t.deadline <= now)
            .map(|(&address, t)| (t.deadline, address))
            .min()
        else {
            return Ok(());
        };
        let timer = self.timers.remove(&address).expect("listed timer");
        if !matches!(
            self.ensure_object(memory, address)?,
            Some(Object::Timer { .. })
        ) {
            // The guest reused an inserted timer's memory: a guest bug the
            // retail kernel would not survive either.
            return Err(GuestFault {
                addr: address,
                len: KTIMER_SIZE as usize,
                access: Access::Read,
                reason: runtime::guest::FaultReason::InvalidOperation,
            });
        }
        memory.set_u32(address + 4, 1)?;
        let period = timer.period_ms as u64 * TICKS_PER_MS;
        let mut next = timer.deadline.saturating_add(period);
        if next <= now {
            next = now.saturating_add(period);
        }
        // A re-armed timer is never due again in this pass (one expiry per
        // period, even after a long time jump); at the end of the clock's
        // range it is left expired rather than spinning.
        if timer.period_ms != 0 && next > now {
            self.insert_timer(
                memory,
                address,
                Timer {
                    deadline: next,
                    ..timer
                },
            )?;
        } else {
            memory.set_u8(address + HEADER_INSERTED, 0)?;
        }
        if timer.dpc != 0 {
            // KiTimerExpiration passes the expiry system time.
            let time = self.clock.system_time();
            self.queue_dpc(memory, timer.dpc, time as u32, (time >> 32) as u32)?;
        }
        Ok(())
    }
    /// KeInsertQueueDpc: a DPC already queued is not queued twice.
    fn queue_dpc(
        &mut self,
        memory: &mut Guest,
        dpc: u32,
        argument1: u32,
        argument2: u32,
    ) -> Result<bool, GuestFault> {
        memory.check(dpc, KDPC_SIZE as usize, Access::Write, 1)?;
        if memory.u8(dpc + DPC_INSERTED)? != 0 {
            return Ok(false);
        }
        memory.set_u32(dpc + DPC_ARGUMENT1, argument1)?;
        memory.set_u32(dpc + DPC_ARGUMENT2, argument2)?;
        memory.set_u8(dpc + DPC_INSERTED, 1)?;
        self.dpcs.push_back(Queued::Kdpc(dpc));
        Ok(true)
    }
    /// Remove the queued DPC at `index` (clearing Inserted, as KiRetireDpcList
    /// does before calling it) and return its call.
    pub(crate) fn take_dpc(
        &mut self,
        memory: &mut Guest,
        index: usize,
    ) -> Result<Option<DpcWork>, GuestFault> {
        let dpc = match self.dpcs.remove(index) {
            None => return Ok(None),
            Some(Queued::Host(id)) => {
                if let Some(source) = self.periodic.get_mut(&id) {
                    source.queued = false;
                }
                return Ok(Some(DpcWork::Host(id)));
            }
            Some(Queued::Kdpc(dpc)) => dpc,
        };
        let bytes = memory.read::<{ KDPC_SIZE as usize }>(dpc)?;
        let word = |at: u32| u32::from_le_bytes(bytes[at as usize..][..4].try_into().unwrap());
        if u16::from_le_bytes([bytes[0], bytes[1]]) != DPC_OBJECT {
            return Err(GuestFault {
                addr: dpc,
                len: KDPC_SIZE as usize,
                access: Access::Read,
                reason: runtime::guest::FaultReason::InvalidOperation,
            });
        }
        memory.set_u8(dpc + DPC_INSERTED, 0)?;
        Ok(Some(DpcWork::Guest(DpcCall {
            routine: word(DPC_ROUTINE),
            arguments: [
                dpc,
                word(DPC_CONTEXT),
                word(DPC_ARGUMENT1),
                word(DPC_ARGUMENT2),
            ],
        })))
    }
    /// Account for every periodic period due by `now` and queue each
    /// source with periods owed (once: a queued source collects more).
    pub(crate) fn fire_periodic(&mut self, now: u64) {
        for (&id, source) in &mut self.periodic {
            if source.fire(now) && !source.queued && source.owed > 0 {
                source.queued = true;
                self.dpcs.push_back(Queued::Host(id));
            }
        }
    }
    /// The earliest periodic deadline not yet accounted for, due or not.
    pub(crate) fn next_periodic_deadline(&self) -> Option<u64> {
        self.periodic.values().map(|p| p.deadline()).min()
    }
    pub(crate) fn next_timer_deadline(&self) -> Option<u64> {
        self.timers.values().map(|t| t.deadline).min()
    }
    /// Called as guest thread `tid` returns from a kernel call. If DPCs are
    /// queued and the thread is now below DISPATCH_LEVEL, retail would run
    /// them before the caller continues (the IRQL drop requests the DPC
    /// interrupt). Requeue the thread at the front of the ready queue and
    /// return true: `Machine::run` then drains the DPC queue, one routine per
    /// slice, before resuming it. The DPC context itself never yields here.
    pub(crate) fn dispatch_pending(
        &mut self,
        memory: &mut Guest,
        tid: u32,
    ) -> Result<bool, GuestFault> {
        if self.dpcs.is_empty() || tid == crate::threads::DPC_THREAD_ID {
            return Ok(false);
        }
        if memory.u8(self.kpcr + kpcr::IRQL)? >= 2 {
            return Ok(false);
        }
        let Some(record) = self.threads.get_mut(&tid) else {
            return Ok(false);
        };
        if !matches!(record.state, crate::threads::State::Running) {
            return Ok(false);
        }
        memory.set_u8(record.object + crate::layout::kthread::STATE, 1)?; // Ready
        record.state = crate::threads::State::Ready;
        record.polling = false;
        self.ready.push_front(tid);
        Ok(true)
    }
    pub(crate) fn dispatch_level(&self, memory: &mut Guest) -> Result<u8, GuestFault> {
        let old = memory.u8(self.kpcr + kpcr::IRQL)?;
        memory.set_u8(self.kpcr + kpcr::IRQL, 2)?;
        Ok(old)
    }
}
