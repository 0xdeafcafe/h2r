//! Tick phase 8, the HaloScript update `0x00209310` (`SCRIPT_UPDATE`), in
//! Rust (docs/engine/hs_vm.md, "Threads"; docs/engine/sim-rewrite.md,
//! "The tick driver"). The scheduler and the tail call's object-list sweep
//! are Rust's; each thread it picks still runs in the lifted
//! `HS_THREAD_EXECUTE` `0x209850`, one guest call per thread, counted in
//! [`Stats::executed`].
//!
//! The guest body, whole:
//!
//! | Step | Guest | Here |
//! |---|---|---|
//! | 1 | scripts off (`[0x4f9388]` 0): return | the same, and no tail call |
//! | 2 | the tick `[[0x510c54] + 8]`, kept for the pass | [`run`]'s `tick` |
//! | 3 | the first live slot from 0 (`0x16bc00` over `[0x4f9384]`), its salted handle | [`DatumArray::next_index`], [`DatumArray::handle_of`] |
//! | 4 | a thread whose byte `+2` is not 4 (not external) and whose wake `+8` is in `0..=tick` runs: `edi` the handle, `call 0x209850` | a guest call |
//! | 5 | the pool pointer read again, the next live slot after this one, while scripts are still on | the same reads |
//! | 6 | `jmp 0x1deed0` (a tail call, nothing on the stack) | [`crate::hs_object_lists::sweep`] |
//!
//! The thread record is `data + index * 0x418`, as the guest addresses it,
//! not by the pool's element size. A read that faults is the guest's own:
//! the lifted body faults on the same read.

use guest_layout::datum::{DatumArray, layout};
use guest_layout::entry::{Cx, EntryError, GuestCall};
use crate::hs_pool::THREADS;
use guest_layout::memory::{GuestMemory, MemFault};
use std::sync::Mutex;

/// The phase's guest entry.
pub const ENTRY: u32 = 0x0020_9310;
/// The thread executor each runnable thread is handed to (`edi` = handle).
pub const EXECUTE: u32 = 0x0020_9850;
/// The update's tail call, the object-list sweep ([`crate::hs_object_lists`]).
pub const AFTER: u32 = crate::hs_object_lists::ENTRY;
/// Scripts on.
pub const ENABLED: u32 = 0x004f_9388;
/// A thread record's stride in the guest's addressing.
const THREAD: u32 = 0x418;
/// Byte `+2`: the thread's kind; 4 is external (the guest does not run it).
const KIND: u32 = 2;
const EXTERNAL: u8 = 4;
/// Word `+8`: the tick the thread wakes on; negative is never.
const WAKE: u32 = 8;

/// The phase's counters since start.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Updates with scripts on.
    pub passes: u64,
    /// Threads handed to the lifted `HS_THREAD_EXECUTE`: the VM itself is
    /// not ported yet, so each is a counted guest call.
    pub executed: u64,
}

static STATS: Mutex<Stats> = Mutex::new(Stats { passes: 0, executed: 0 });

pub fn stats() -> Stats {
    *STATS.lock().unwrap_or_else(|e| e.into_inner())
}

fn count(f: impl FnOnce(&mut Stats)) {
    f(&mut STATS.lock().unwrap_or_else(|e| e.into_inner()))
}

/// The handle of the first runnable thread at or after slot `start`, with
/// the slot to look from next. `tick` is the pass's. As in the guest, the
/// scripts-on byte is read after each look but the first, and before the
/// end of the pool is acted on.
pub fn next_runnable(m: &dyn GuestMemory, tick: i32, mut start: i32) -> Result<Option<(u32, i32)>, MemFault> {
    let mut first = start == 0;
    loop {
        let pool = DatumArray::at(m.u32(THREADS)?);
        let found = match pool.next_index(m, start)? {
            Some(index) => pool.handle_of(m, index)?.map(|h| (h.raw(), index)),
            None => None,
        };
        if !first && m.u8(ENABLED)? == 0 {
            return Ok(None);
        }
        first = false;
        let Some((handle, index)) = found else { return Ok(None) };
        let data = pool.header_field(m, layout::DATA)? as u32;
        let thread = data.wrapping_add((index as u32 & 0xffff).wrapping_mul(THREAD));
        start = (index & 0xffff) + 1;
        if m.u8(thread.wrapping_add(KIND))? != EXTERNAL {
            let wake = m.i32(thread.wrapping_add(WAKE))?;
            if (0..=tick).contains(&wake) {
                return Ok(Some((handle, start)));
            }
        }
    }
}

/// The phase. `reserve` is the stack the driver keeps below its entry.
pub async fn run(cx: &Cx, reserve: u32) -> Result<(), EntryError> {
    if cx.read(|m| m.u8(ENABLED))? == 0 {
        return Ok(());
    }
    count(|s| s.passes += 1);
    let tick = cx.read(|m| -> Result<i32, MemFault> { m.i32(m.u32(guest_layout::points::GAME_TIME.base)?.wrapping_add(8)) })?;
    let mut start = 0;
    while let Some((handle, next)) = cx.read(|m| next_runnable(m, tick, start))? {
        count(|s| s.executed += 1);
        cx.call(GuestCall::new(EXECUTE).reserve(reserve).edi(handle)).await?;
        start = next;
    }
    cx.write(|m| crate::hs_object_lists::sweep(m))?;
    Ok(())
}

#[cfg(test)]
mod tests;
