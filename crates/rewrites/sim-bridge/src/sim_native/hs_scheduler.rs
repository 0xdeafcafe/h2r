//! Compare the dispatch sequence of one hs_update invocation. Scripts can
//! wake, delete or allocate other threads. Predict only through the next
//! thread_execute, then read the pool again when that guest call returns.
use super::hs_parity::SCHEDULER;
use engine::datum::{DatumArray, layout};
use engine::{DatumHandle, GuestMemory, MemFault, hs_pool, points};
use hooks::{Builder, Obs};
use hs::vm::{Schedule, ScheduleSlot};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

const ENABLED: u32 = 0x004f_9388;
const SCHEDULER_RESUME: u32 = 0x0020_938d;
const THREAD_BYTES: i32 = 0x418;

// hs_update pushes its tick and three saved registers. These pushes and the
// return PC separate a scheduler dispatch from a nested synchronous execution.
const SAVED_BYTES: u32 = 16;

#[derive(Debug)]
struct Unavailable;
impl From<MemFault> for Unavailable {
    fn from(_: MemFault) -> Self {
        Self
    }
}

struct Pass {
    entry_sp: u32,
    schedule: Schedule,
    next: Option<DatumHandle>,
    executing: bool,
}

impl Pass {
    fn begin(mem: &dyn GuestMemory, entry_sp: u32) -> Result<Self, Unavailable> {
        let enabled = mem.u8(ENABLED)? != 0;
        let tick = if enabled {
            let time = mem.u32(points::GAME_TIME.base)?;
            mem.i32(time.checked_add(8).ok_or(Unavailable)?)?
        } else {
            0
        };
        let mut pass = Self {
            entry_sp,
            schedule: Schedule::new(tick),
            next: None,
            executing: false,
        };
        if enabled {
            pass.read_next(mem)?;
        }
        Ok(pass)
    }

    fn read_next(&mut self, mem: &dyn GuestMemory) -> Result<(), Unavailable> {
        self.executing = false;
        if mem.u8(ENABLED)? == 0 {
            self.next = None;
            return Ok(());
        }
        let pool = DatumArray::at(mem.u32(hs_pool::THREADS)?);
        let high = pool.header_field(mem, layout::HIGH_WATER)?;
        if !(0..=i32::from(u16::MAX)).contains(&high)
            || high > pool.header_field(mem, layout::MAXIMUM_COUNT)?
            || pool.header_field(mem, layout::ELEMENT_SIZE)? != THREAD_BYTES
        {
            return Err(Unavailable);
        }
        self.next = self.schedule.next_ready(|start| {
            let Some(index) = pool.next_index(mem, start as i32)? else {
                return Ok::<_, Unavailable>(None);
            };
            let addr = pool.element_addr(mem, index)?;
            let slot = ScheduleSlot {
                index: index as usize,
                external: mem.u8(addr.checked_add(2).ok_or(Unavailable)?)? == 4,
                wake: mem.i32(addr.checked_add(8).ok_or(Unavailable)?)?,
                thread: DatumHandle::new(mem.u16(addr)?, index as u32),
            };
            Ok(Some(slot))
        })?;
        Ok(())
    }
}

type Calls = Arc<Mutex<HashMap<(usize, u32), Pass>>>;

fn key(o: &Obs<'_>) -> (usize, u32) {
    (Arc::as_ptr(o.runtime) as usize, o.thread_id)
}

pub(super) fn install(b: &mut Builder) {
    let calls = Calls::default();
    let begin = calls.clone();
    b.raw()
        .observe_before(&points::SCRIPT_UPDATE, "HS schedule begin", move |o| {
            let mem = o.guest();
            let mut calls = begin.lock().unwrap_or_else(|e| e.into_inner());
            // A guest fault or a checkpoint restore can abandon an open invocation.
            if calls.remove(&key(o)).is_some() {
                SCHEDULER.declined();
            }
            if !mem.has_trap()
                && let Ok(pass) = Pass::begin(&*mem, o.cpu.esp)
            {
                calls.insert(key(o), pass);
            } else {
                SCHEDULER.declined();
            }
        });

    let enter = calls.clone();
    b.raw().observe_before(
        &points::HS_THREAD_EXECUTE,
        "HS scheduled thread",
        move |o| {
            let mem = o.guest();
            let mut calls = enter.lock().unwrap_or_else(|e| e.into_inner());
            let Some(pass) = calls.get_mut(&key(o)) else {
                return;
            };
            if pass.entry_sp.wrapping_sub(SAVED_BYTES + 4) != o.cpu.esp {
                return;
            }
            if mem.has_trap() || mem.u32(o.cpu.esp).ok() != Some(SCHEDULER_RESUME) {
                calls.remove(&key(o));
                SCHEDULER.declined();
                return;
            }
            let guest = DatumHandle(o.cpu.edi);
            let predicted = pass.next.take();
            if predicted != Some(guest) || pass.executing {
                SCHEDULER.mismatch(|| format!("dispatch expected {predicted:?}, guest {guest:?}"));
                calls.remove(&key(o));
                return;
            }
            pass.executing = true;
        },
    );

    let leave = calls.clone();
    b.raw()
        .observe_after(&points::HS_THREAD_EXECUTE, "HS schedule resume", move |o| {
            let mem = o.guest();
            let mut calls = leave.lock().unwrap_or_else(|e| e.into_inner());
            let Some(pass) = calls.get_mut(&key(o)) else {
                return;
            };
            if pass.executing
                && o.cpu.esp == pass.entry_sp.wrapping_sub(SAVED_BYTES)
                && (mem.has_trap() || pass.read_next(&*mem).is_err())
            {
                calls.remove(&key(o));
                SCHEDULER.declined();
            }
        });
    b.raw()
        .observe_after(&points::SCRIPT_UPDATE, "HS schedule end", move |o| {
            let mut calls = calls.lock().unwrap_or_else(|e| e.into_inner());
            let Some(pass) = calls.remove(&key(o)) else {
                return;
            };
            if o.cpu.esp != pass.entry_sp.wrapping_add(4) || pass.executing {
                SCHEDULER.declined();
            } else if let Some(missing) = pass.next {
                SCHEDULER.mismatch(|| format!("guest ended update before {missing:?}"));
            } else {
                SCHEDULER.matched();
            }
        });
}

#[cfg(test)]
mod tests;
