use crate::layout::kthread;
use crate::ord;
use crate::{
    Action, Core,
    abi::{Args, Value},
    status,
    threads::State,
};
use runtime::{Guest, GuestFault, guest::Access};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Object {
    Event { manual: bool },
    Semaphore { limit: i32 },
    Thread { tid: u32 },
    Timer { synchronization: bool },
}
#[derive(Debug, Clone, Default)]
pub(crate) struct Critical {
    owner: Option<u32>,
    recursion: u32,
}
#[derive(Debug, Clone)]
pub(crate) enum Wait {
    Objects {
        objects: Vec<u32>,
        all: bool,
        deadline: Option<u64>,
    },
    Critical(u32),
}

struct WaitOptions {
    timeout: u32,
    alertable: bool,
    mode: u32,
}

pub(crate) fn header(
    memory: &mut Guest,
    address: u32,
    kind: u8,
    size: u8,
    signal: i32,
) -> Result<(), GuestFault> {
    memory.check(address, 16, Access::Write, 1)?;
    memory.write(address, [kind, 0, size, 0])?;
    memory.set_u32(address + 4, signal as u32)?;
    memory.set_u32(address + 8, address + 8)?;
    memory.set_u32(address + 12, address + 8)
}

fn unnamed_attributes(memory: &Guest, address: u32) -> Result<bool, GuestFault> {
    if address == 0 {
        return Ok(true);
    }
    let fields = memory.read::<12>(address)?;
    Ok(fields[..8] == [0; 8] && u32::from_le_bytes(fields[8..12].try_into().unwrap()) & !2 == 0)
}

impl Core {
    pub(crate) fn ensure_object(
        &mut self,
        memory: &Guest,
        address: u32,
    ) -> Result<Option<Object>, GuestFault> {
        if self.files.contains_key(&address) || self.links.contains_key(&address) {
            return Ok(None);
        }
        let expected = self.objects.get(&address).copied();
        let inspected = (|| {
            let header = memory.read::<16>(address)?;
            Ok(match (header[0], header[2]) {
                (0, 4) => Some(Object::Event { manual: true }),
                (1, 4) => Some(Object::Event { manual: false }),
                (5, 5) => {
                    // The 16-byte header was readable; its 20-byte semaphore
                    // body may still cross the end of the address space.
                    let limit_at = address
                        .checked_add(16)
                        .ok_or_else(|| crate::strings::overflow(address))?;
                    let limit = memory.u32(limit_at)? as i32;
                    let count = i32::from_le_bytes(header[4..8].try_into().unwrap());
                    (limit > 0 && count >= 0 && count <= limit)
                        .then_some(Object::Semaphore { limit })
                }
                (crate::timers::TIMER_NOTIFICATION, 10) => Some(Object::Timer {
                    synchronization: false,
                }),
                (crate::timers::TIMER_SYNCHRONIZATION, 10) => Some(Object::Timer {
                    synchronization: true,
                }),
                (6, 0x44) => match expected {
                    Some(Object::Thread { tid }) => Some(Object::Thread { tid }),
                    _ => None,
                },
                _ => None,
            })
        })();
        let current = match inspected {
            Ok(v) => v,
            Err(fault) => {
                self.objects.remove(&address);
                return Err(fault);
            }
        };
        let matches = match (expected, current) {
            (Some(Object::Semaphore { .. }), Some(Object::Semaphore { .. })) => true,
            (Some(old), Some(new)) => old == new,
            (None, Some(_)) => !self.owned_objects.contains(&address),
            _ => false,
        };
        if !matches {
            self.objects.remove(&address);
            return Ok(None);
        }
        // Pointer/stack dispatcher objects have no kernel-owned lifetime. Decode
        // them on every access rather than caching addresses after their scope ends.
        Ok(current)
    }
    fn object_from_handle(&self, handle: u32, tid: u32) -> Result<u32, u32> {
        if handle == 0xffff_fffe {
            self.threads
                .get(&tid)
                .map(|r| r.object)
                .ok_or(status::INVALID_HANDLE)
        } else {
            self.handles.get(handle)
        }
    }
    /// Test every object before consuming any. WaitAll must not lose semaphore
    /// units or synchronization events when one other object is unsignaled.
    fn acquire(
        &mut self,
        memory: &mut Guest,
        objects: &[u32],
        all: bool,
    ) -> Result<Option<u32>, GuestFault> {
        let mut kinds = Vec::new();
        let mut available = Vec::new();
        for &address in objects {
            let kind = self.ensure_object(memory, address)?.ok_or(GuestFault {
                addr: address,
                len: 16,
                access: Access::Read,
                reason: runtime::guest::FaultReason::InvalidOperation,
            })?;
            kinds.push(kind);
            available.push(memory.u32(address + 4)? as i32 > 0);
        }
        let selected = if all {
            if !available.iter().all(|s| *s) {
                return Ok(None);
            }
            (0..objects.len()).collect::<Vec<_>>()
        } else {
            let Some(i) = available.iter().position(|s| *s) else {
                return Ok(None);
            };
            vec![i]
        };
        for &i in &selected {
            if matches!(
                kinds[i],
                Object::Event { manual: false }
                    | Object::Semaphore { .. }
                    | Object::Timer {
                        synchronization: true
                    }
            ) {
                memory.check(objects[i] + 4, 4, Access::Write, 1)?;
            }
        }
        for &i in &selected {
            let address = objects[i];
            match kinds[i] {
                Object::Event { manual: false }
                | Object::Timer {
                    synchronization: true,
                } => memory.set_u32(address + 4, 0)?,
                Object::Semaphore { .. } => {
                    let count = memory.u32(address + 4)?;
                    memory.set_u32(address + 4, count - 1)?;
                }
                _ => {}
            }
        }
        Ok(Some(if all {
            status::WAIT_0
        } else {
            status::WAIT_0 + selected[0] as u32
        }))
    }
    fn wait(
        &mut self,
        memory: &mut Guest,
        objects: Vec<u32>,
        all: bool,
        options: WaitOptions,
        tid: u32,
    ) -> Result<Action, GuestFault> {
        let WaitOptions {
            timeout,
            alertable,
            mode,
        } = options;
        if objects.is_empty() || objects.len() > 64 || mode as u8 > 1 {
            return Ok(Action::status(status::INVALID_PARAMETER));
        }
        if alertable {
            return Ok(Action::Stop("alertable waits require APC/alert delivery"));
        }
        let unique: std::collections::BTreeSet<_> = objects.iter().copied().collect();
        if all && unique.len() != objects.len() {
            return Ok(Action::status(status::INVALID_PARAMETER_MIX));
        }
        for &object in &objects {
            if self.ensure_object(memory, object)?.is_none() {
                return Ok(Action::Stop("unsupported dispatcher object"));
            }
        }
        let deadline = if timeout == 0 {
            None
        } else {
            match self.clock.deadline(memory.u64(timeout)? as i64) {
                Ok(v) => Some(v),
                Err(_) => return Ok(Action::status(status::INVALID_PARAMETER)),
            }
        };
        if let Some(status) = self.acquire(memory, &objects, all)? {
            return Ok(Action::status(status));
        }
        if deadline.is_some_and(|v| v <= self.clock.counter()) {
            return Ok(Action::status(status::TIMEOUT));
        }
        if !self.threads.contains_key(&tid) {
            return Ok(Action::Stop("blocking call requires Machine"));
        }
        if memory.u8(self.kpcr + crate::layout::kpcr::IRQL)? > 1 {
            return Ok(Action::Stop("blocking wait above APC_LEVEL"));
        }
        memory.set_u8(self.threads[&tid].object + kthread::STATE, 5)?;
        self.threads.get_mut(&tid).unwrap().state = State::Waiting(Wait::Objects {
            objects,
            all,
            deadline,
        });
        self.wait_order.push_back(tid);
        Ok(Action::Block(Value::U32(status::SUCCESS)))
    }
    fn mark_ready(&mut self, memory: &mut Guest, tid: u32, value: Value) -> Result<(), GuestFault> {
        let record = self.threads.get_mut(&tid).unwrap();
        memory.set_u8(record.object + kthread::STATE, 1)?;
        record.state = State::Ready;
        record.wake_value = Some(value);
        self.ready.push_back(tid);
        Ok(())
    }
    /// One scheduler wake pass. Overdue timers expire one at a time in
    /// deadline order, and before each expiry the waiters see every earlier
    /// event: an object signalled so far, or a sleep or wait timeout due
    /// strictly before that timer. So when time jumps over several deadlines,
    /// a WaitAny is satisfied by the timer that expired first, as if the
    /// clock had stopped at each one. A timeout due at the same tick as a
    /// timer loses to the timer. A final pass applies everything due by now.
    pub(crate) fn wake(&mut self, memory: &mut Guest) -> Result<(), GuestFault> {
        let now = self.clock.counter();
        while let Some(deadline) = self.next_due_timer(now) {
            self.wake_waiters(memory, |d| d < deadline)?;
            self.expire_next_timer(memory, now)?;
        }
        self.wake_waiters(memory, |d| d <= now)?;
        self.collect_objects();
        Ok(())
    }
    /// Ready every sleeper and waiter that can proceed, treating a timeout
    /// deadline as reached when `due(deadline)` holds.
    fn wake_waiters(
        &mut self,
        memory: &mut Guest,
        due: impl Fn(u64) -> bool,
    ) -> Result<(), GuestFault> {
        let sleeping: Vec<_> = self
            .threads
            .iter()
            .filter_map(|(&id, r)| {
                matches!(r.state, State::Sleeping(deadline) if due(deadline)).then_some(id)
            })
            .collect();
        for tid in sleeping {
            self.mark_ready(memory, tid, Value::U32(status::SUCCESS))?;
        }
        let queue = std::mem::take(&mut self.wait_order);
        for tid in queue {
            let state = self.threads[&tid].state.clone();
            let result = match state {
                State::Waiting(Wait::Objects {
                    objects,
                    all,
                    deadline,
                }) => {
                    if let Some(status) = self.acquire(memory, &objects, all)? {
                        Some(Value::U32(status))
                    } else if deadline.is_some_and(&due) {
                        Some(Value::U32(status::TIMEOUT))
                    } else {
                        None
                    }
                }
                State::Waiting(Wait::Critical(address)) => {
                    if self.critical[&address].owner.is_none() {
                        let c = self.critical.get_mut(&address).unwrap();
                        c.owner = Some(tid);
                        c.recursion = 1;
                        self.write_critical(memory, address)?;
                        Some(Value::Void)
                    } else {
                        None
                    }
                }
                _ => continue,
            };
            if let Some(value) = result {
                self.mark_ready(memory, tid, value)?;
            } else {
                self.wait_order.push_back(tid);
            }
        }
        Ok(())
    }
    pub(crate) fn collect_objects(&mut self) {
        // Follow-up: maintain reverse counts instead of scanning handles and
        // every thread's wait objects for every owned object on each wake.
        let unused: Vec<_> = self
            .owned_objects
            .iter()
            .copied()
            .filter(|object| {
                let handled = self.handles.references(*object) != 0;
                let pinned = self.references.get(object).copied().unwrap_or(0) != 0;
                let waited = self.threads.values().any(|record| {
                    matches!(&record.state, State::Waiting(Wait::Objects { objects, .. })
                    if objects.contains(object))
                });
                !handled && !pinned && !waited
            })
            .collect();
        for object in unused {
            self.owned_objects.remove(&object);
            self.objects.remove(&object);
            self.memory.free_pool(object);
        }
    }
    pub(crate) fn event_set(
        &mut self,
        memory: &mut Guest,
        address: u32,
        signal: bool,
    ) -> Result<Action, GuestFault> {
        if !matches!(
            self.ensure_object(memory, address)?,
            Some(Object::Event { .. })
        ) {
            return Ok(Action::Stop(
                "event operation on an invalid/non-event header",
            ));
        }
        let old = memory.u32(address + 4)?;
        memory.set_u32(address + 4, signal as u32)?;
        self.wake(memory)?;
        Ok(Action::value(old))
    }
    fn release_semaphore(
        &mut self,
        memory: &mut Guest,
        address: u32,
        adjustment: i32,
    ) -> Result<Action, GuestFault> {
        let Some(Object::Semaphore { limit }) = self.ensure_object(memory, address)? else {
            return Ok(Action::Stop(
                "KeReleaseSemaphore requires a valid semaphore",
            ));
        };
        let old = memory.u32(address + 4)? as i32;
        if adjustment <= 0 || old.checked_add(adjustment).is_none_or(|n| n > limit) {
            return Ok(Action::Stop(
                "KeReleaseSemaphore would raise semaphore-limit exception",
            ));
        }
        memory.set_u32(address + 4, (old + adjustment) as u32)?;
        self.wake(memory)?;
        Ok(Action::value(old as u32))
    }
    fn write_critical(&self, memory: &mut Guest, address: u32) -> Result<(), GuestFault> {
        let c = &self.critical[&address];
        let waiting = self
            .threads
            .values()
            .filter(|r| matches!(r.state, State::Waiting(Wait::Critical(a)) if a == address))
            .count() as u32;
        memory.check(address + 16, 12, Access::Write, 1)?;
        memory.set_u32(
            address + 16,
            c.recursion.wrapping_add(waiting).wrapping_sub(1),
        )?;
        memory.set_u32(address + 20, c.recursion)?;
        memory.set_u32(
            address + 24,
            c.owner
                .and_then(|tid| self.threads.get(&tid).map(|r| r.object))
                .unwrap_or(0),
        )
    }
    fn critical_call(
        &mut self,
        memory: &mut Guest,
        a: &Args,
        tid: u32,
    ) -> Result<Action, GuestFault> {
        let address = a.get(0);
        memory.check(address, 28, Access::Write, 1)?;
        if a.export.ordinal == ord::RTL_INITIALIZE_CRITICAL_SECTION {
            if self
                .critical
                .get(&address)
                .is_some_and(|c| c.owner.is_some())
            {
                return Ok(Action::Stop("reinitializing an owned critical section"));
            }
            if !self.critical.contains_key(&address)
                && self.critical.len() >= crate::limits::MAX_CRITICAL_SECTIONS
            {
                return Ok(Action::Stop("critical-section tracking budget exhausted"));
            }
            header(memory, address, 1, 4, 0)?;
            self.critical.insert(address, Critical::default());
            self.write_critical(memory, address)?;
            return Ok(Action::Complete(Value::Void));
        }
        if !self.critical.contains_key(&address)
            && self.critical.len() >= crate::limits::MAX_CRITICAL_SECTIONS
        {
            return Ok(Action::Stop("critical-section tracking budget exhausted"));
        }
        if let std::collections::btree_map::Entry::Vacant(entry) = self.critical.entry(address) {
            if memory.u32(address + 16)? == u32::MAX
                && memory.u32(address + 20)? == 0
                && memory.u32(address + 24)? == 0
            {
                entry.insert(Critical::default());
            } else {
                return Ok(Action::Stop("unknown critical section state"));
            }
        }
        let c = self.critical.get_mut(&address).unwrap();
        if a.export.ordinal == ord::RTL_LEAVE_CRITICAL_SECTION {
            if c.owner != Some(tid) {
                return Ok(Action::Stop("critical section released by non-owner"));
            }
            c.recursion -= 1;
            if c.recursion == 0 {
                c.owner = None;
            }
            self.write_critical(memory, address)?;
            self.wake(memory)?;
            // wake changes ownership and removes one waiter.
            self.write_critical(memory, address)?;
            return Ok(Action::Complete(Value::Void));
        }
        if c.owner.is_none() || c.owner == Some(tid) {
            if !self.threads.contains_key(&tid) {
                return Ok(Action::Stop("critical ownership requires a KTHREAD"));
            }
            let Some(recursion) = c.recursion.checked_add(1) else {
                return Ok(Action::Stop("critical recursion overflow"));
            };
            c.owner = Some(tid);
            c.recursion = recursion;
            self.write_critical(memory, address)?;
            Ok(Action::Complete(
                if a.export.ordinal == ord::RTL_TRY_ENTER_CRITICAL_SECTION {
                    Value::U32(1)
                } else {
                    Value::Void
                },
            ))
        } else if a.export.ordinal == ord::RTL_TRY_ENTER_CRITICAL_SECTION {
            Ok(Action::value(0))
        } else {
            if !self.threads.contains_key(&tid) {
                return Ok(Action::Stop("contended critical section requires Machine"));
            }
            memory.set_u8(self.threads[&tid].object + kthread::STATE, 5)?;
            self.threads.get_mut(&tid).unwrap().state = State::Waiting(Wait::Critical(address));
            self.wait_order.push_back(tid);
            self.write_critical(memory, address)?;
            Ok(Action::Block(Value::Void))
        }
    }
    pub(crate) fn sync_call(
        &mut self,
        memory: &mut Guest,
        a: &Args,
        tid: u32,
    ) -> Result<Action, GuestFault> {
        Ok(match a.export.ordinal {
            ord::KE_INITIALIZE_EVENT | ord::KE_INITIALIZE_SEMAPHORE => {
                let address = a.get(0);
                if a.export.ordinal == ord::KE_INITIALIZE_EVENT {
                    if a.get(1) > 1 {
                        return Ok(Action::Stop("invalid event type"));
                    }
                    header(memory, address, a.get(1) as u8, 4, a.boolean(2) as i32)?;
                } else {
                    let count = a.get(1) as i32;
                    let limit = a.get(2) as i32;
                    if count < 0 || limit <= 0 || count > limit {
                        return Ok(Action::Stop("invalid semaphore counts"));
                    }
                    memory.check(address, 20, Access::Write, 1)?;
                    header(memory, address, 5, 5, count)?;
                    memory.set_u32(address + 16, limit as u32)?;
                }
                Action::Complete(Value::Void)
            }
            ord::KE_RESET_EVENT => return self.event_set(memory, a.get(0), false),
            ord::KE_SET_EVENT => {
                if a.boolean(2) {
                    return Ok(Action::Stop("KeSetEvent WaitNext/IRQL handoff unsupported"));
                }
                return self.event_set(memory, a.get(0), true);
            }
            ord::KE_RELEASE_SEMAPHORE => {
                if a.boolean(3) {
                    return Ok(Action::Stop(
                        "KeReleaseSemaphore WaitNext/IRQL handoff unsupported",
                    ));
                }
                return self.release_semaphore(memory, a.get(0), a.get(2) as i32);
            }
            ord::NT_CLOSE if a.get(0) == 0xffff_fffe => {
                if !self.threads.contains_key(&tid) {
                    Action::Stop("current-thread pseudo-handle requires Machine")
                } else {
                    Action::status(status::SUCCESS)
                }
            }
            ord::NT_CLOSE => match self.handles.close(a.get(0)) {
                Ok(object) => {
                    if self.links.contains_key(&object) && self.handles.references(object) == 0 {
                        self.links.remove(&object);
                        self.memory.free_pool(object);
                    }
                    self.collect_objects();
                    Action::status(status::SUCCESS)
                }
                Err(s) => Action::status(s),
            },
            ord::NT_DUPLICATE_OBJECT => {
                memory.check(a.get(1), 4, Access::Write, 1)?;
                if a.get(2) & !3 != 0 {
                    return Ok(Action::status(status::INVALID_PARAMETER));
                }
                let pseudo = a.get(0) == 0xffff_fffe;
                let result = if pseudo {
                    let Some(record) = self.threads.get(&tid) else {
                        return Ok(Action::Stop(
                            "current-thread pseudo-handle requires Machine",
                        ));
                    };
                    self.handles.insert(record.object)
                } else {
                    self.handles.duplicate(a.get(0))
                };
                match result {
                    Ok(handle) => {
                        memory.set_u32(a.get(1), handle)?;
                        if a.get(2) & 1 != 0 && !pseudo {
                            self.handles.close(a.get(0)).unwrap();
                        }
                        Action::status(status::SUCCESS)
                    }
                    Err(s) => Action::status(s),
                }
            }
            ord::NT_CREATE_EVENT | ord::NT_CREATE_SEMAPHORE => {
                memory.check(a.get(0), 4, Access::Write, 1)?;
                if !unnamed_attributes(memory, a.get(1))? {
                    return Ok(Action::Stop(
                        "named dispatcher objects need an object namespace",
                    ));
                }
                let event = a.export.ordinal == ord::NT_CREATE_EVENT;
                if event && a.get(2) > 1
                    || !event
                        && ((a.get(2) as i32) < 0 || a.get(3) as i32 <= 0 || a.get(2) > a.get(3))
                {
                    return Ok(Action::status(status::INVALID_PARAMETER));
                }
                let address = self.memory.allocate_pool(memory, 20, 0x746e7645);
                if address == 0 {
                    return Ok(Action::status(status::NO_MEMORY));
                }
                if event {
                    header(memory, address, a.get(2) as u8, 4, a.boolean(3) as i32)?;
                    self.objects.insert(
                        address,
                        Object::Event {
                            manual: a.get(2) == 0,
                        },
                    );
                } else {
                    header(memory, address, 5, 5, a.get(2) as i32)?;
                    memory.set_u32(address + 16, a.get(3))?;
                    self.objects.insert(
                        address,
                        Object::Semaphore {
                            limit: a.get(3) as i32,
                        },
                    );
                }
                self.owned_objects.insert(address);
                match self.handles.insert(address) {
                    Ok(handle) => {
                        memory.set_u32(a.get(0), handle)?;
                        Action::status(status::SUCCESS)
                    }
                    Err(s) => {
                        self.collect_objects();
                        Action::status(s)
                    }
                }
            }
            ord::NT_RELEASE_SEMAPHORE | ord::NT_SET_EVENT => {
                let address = match self.handles.get(a.get(0)) {
                    Ok(v) => v,
                    Err(s) => return Ok(Action::status(s)),
                };
                let out = if a.export.ordinal == ord::NT_RELEASE_SEMAPHORE {
                    a.get(2)
                } else {
                    a.get(1)
                };
                if out != 0 {
                    memory.check(out, 4, Access::Write, 1)?;
                }
                if self.files.contains_key(&address) || self.links.contains_key(&address) {
                    return Ok(Action::status(status::OBJECT_TYPE_MISMATCH));
                }
                let Some(kind) = self.ensure_object(memory, address)? else {
                    return Ok(Action::Stop("invalid dispatcher header for handle"));
                };
                if a.export.ordinal == ord::NT_SET_EVENT && !matches!(kind, Object::Event { .. }) {
                    return Ok(Action::status(status::OBJECT_TYPE_MISMATCH));
                }
                if a.export.ordinal == ord::NT_RELEASE_SEMAPHORE {
                    let Object::Semaphore { limit } = kind else {
                        return Ok(Action::status(status::OBJECT_TYPE_MISMATCH));
                    };
                    let old = memory.u32(address + 4)? as i32;
                    let adjustment = a.get(1) as i32;
                    if adjustment <= 0 {
                        return Ok(Action::status(status::INVALID_PARAMETER));
                    }
                    if old.checked_add(adjustment).is_none_or(|n| n > limit) {
                        return Ok(Action::status(status::SEMAPHORE_LIMIT_EXCEEDED));
                    }
                }
                let result = if a.export.ordinal == ord::NT_RELEASE_SEMAPHORE {
                    self.release_semaphore(memory, address, a.get(1) as i32)?
                } else {
                    self.event_set(memory, address, true)?
                };
                match result {
                    Action::Complete(Value::U32(old)) => {
                        if out != 0 {
                            memory.set_u32(out, old)?;
                        }
                        Action::status(status::SUCCESS)
                    }
                    other => other,
                }
            }
            ord::KE_WAIT_FOR_SINGLE_OBJECT => {
                return self.wait(
                    memory,
                    vec![a.get(0)],
                    false,
                    WaitOptions {
                        timeout: a.get(4),
                        alertable: a.boolean(3),
                        mode: a.get(2),
                    },
                    tid,
                );
            }
            ord::KE_WAIT_FOR_MULTIPLE_OBJECTS | ord::NT_WAIT_FOR_MULTIPLE_OBJECTS_EX => {
                if a.get(0) == 0 || a.get(0) > 64 || a.get(2) > 1 {
                    return Ok(Action::status(status::INVALID_PARAMETER));
                }
                let mut objects = Vec::new();
                for i in 0..a.get(0) {
                    let address = a
                        .get(1)
                        .checked_add(i * 4)
                        .ok_or_else(|| crate::strings::overflow(a.get(1)))?;
                    let object = memory.u32(address)?;
                    objects.push(
                        if a.export.ordinal == ord::NT_WAIT_FOR_MULTIPLE_OBJECTS_EX {
                            match self.object_from_handle(object, tid) {
                                Ok(v) => v,
                                Err(s) => return Ok(Action::status(s)),
                            }
                        } else {
                            object
                        },
                    );
                }
                let (mode, alert, timeout) =
                    if a.export.ordinal == ord::KE_WAIT_FOR_MULTIPLE_OBJECTS {
                        (a.get(4), a.boolean(5), a.get(6))
                    } else {
                        (a.get(3), a.boolean(4), a.get(5))
                    };
                return self.wait(
                    memory,
                    objects,
                    a.get(2) == 0,
                    WaitOptions {
                        timeout,
                        alertable: alert,
                        mode,
                    },
                    tid,
                );
            }
            ord::NT_WAIT_FOR_SINGLE_OBJECT | ord::NT_WAIT_FOR_SINGLE_OBJECT_EX => {
                let object = match self.object_from_handle(a.get(0), tid) {
                    Ok(v) => v,
                    Err(s) => return Ok(Action::status(s)),
                };
                let (mode, alert, timeout) = if a.export.ordinal == ord::NT_WAIT_FOR_SINGLE_OBJECT {
                    (0, a.boolean(1), a.get(2))
                } else {
                    (a.get(1), a.boolean(2), a.get(3))
                };
                return self.wait(
                    memory,
                    vec![object],
                    false,
                    WaitOptions {
                        timeout,
                        alertable: alert,
                        mode,
                    },
                    tid,
                );
            }
            ord::RTL_ENTER_CRITICAL_SECTION
            | ord::RTL_INITIALIZE_CRITICAL_SECTION
            | ord::RTL_LEAVE_CRITICAL_SECTION
            | ord::RTL_TRY_ENTER_CRITICAL_SECTION => return self.critical_call(memory, a, tid),
            _ => Action::Stop("declared export has no handler"),
        })
    }
}

#[cfg(test)]
mod review_tests {
    use super::*;
    use runtime::{Dispatch, Runtime, guest::Permissions};
    #[test]
    fn borrowed_dispatcher_addresses_are_not_cached_after_their_lifetime() {
        let mut memory = Guest::new();
        memory
            .map_zeroed(0x10000, 0x10000, Permissions::RW)
            .unwrap();
        let runtime = Runtime::new(memory, Dispatch::default());
        let kernel = crate::Kernel::install(&runtime, crate::Config::default()).unwrap();
        let mut core = kernel.lock();
        let baseline = core.objects.len();
        for i in 0..100 {
            let address = 0x10000 + i * 32;
            header(&mut runtime.memory(), address, 0, 4, 1).unwrap();
            let _ = core.ensure_object(&runtime.memory(), address).unwrap();
        }
        assert_eq!(core.objects.len(), baseline);
    }
    #[test]
    fn owned_cached_kind_is_evicted_when_the_guest_header_disagrees() {
        let mut memory = Guest::new();
        memory.map_zeroed(0x10000, 4096, Permissions::RW).unwrap();
        let runtime = Runtime::new(memory, Dispatch::default());
        let kernel = crate::Kernel::install(&runtime, crate::Config::default()).unwrap();
        let mut core = kernel.lock();
        let address = 0x10000;
        header(&mut runtime.memory(), address, 5, 5, 1).unwrap();
        runtime.memory().set_u32(address + 16, 3).unwrap();
        core.objects.insert(address, Object::Semaphore { limit: 3 });
        core.owned_objects.insert(address);
        header(&mut runtime.memory(), address, 0, 4, 1).unwrap();
        assert!(
            core.ensure_object(&runtime.memory(), address)
                .unwrap()
                .is_none()
        );
        assert!(!core.objects.contains_key(&address));
        assert!(
            core.ensure_object(&runtime.memory(), address)
                .unwrap()
                .is_none()
        );
        assert_eq!(runtime.memory().u32(address + 4).unwrap(), 1);
    }
}
