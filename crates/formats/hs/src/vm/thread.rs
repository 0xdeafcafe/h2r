//! Script threads and their frame stacks.
//!
//! A retail thread is a `0x418`-byte datum (`hs thread`, `0x004f9384`) with
//! a 1,024-byte inline stack of frames. Here a frame keeps its evaluator's
//! locals as cells, in the order the evaluator reserves them, so the state an
//! evaluator re-reads on every entry is preserved across yields exactly as the
//! guest's frame-local cursor reservation preserves it.
use super::value::Cell;
use crate::Datum;

/// Thread category byte (`+2`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    /// Scenario script thread (startup, dormant, continuous), created by
    /// `0x002090a0`.
    Script,
    /// Global-initializer thread, deleted when initialization ends.
    GlobalInit,
    /// Transient expression thread (console evaluation, `0x002093d0`);
    /// deleted when its expression completes.
    Transient,
    /// Category 4: skipped by `hs_update`, finished with wake -1. Driven
    /// outside the scheduler (command scripts); not yet created by this VM.
    External,
}

/// A scheduler pass reads its tick once. Each lookup starts past the last
/// inspected slot, including after a script mutates the pool or wakes a peer.
#[derive(Debug, Clone, Copy)]
pub struct Schedule {
    tick: i32,
    next_slot: usize,
}

/// The fields the scheduler needs from an occupied slot. The payload is the
/// caller's identity for dispatch (a VM slot or a salted guest handle).
pub struct ScheduleSlot<T> {
    pub index: usize,
    pub external: bool,
    pub wake: i32,
    pub thread: T,
}

impl Schedule {
    pub fn new(tick: i32) -> Self {
        Self { tick, next_slot: 0 }
    }

    /// `occupied(start)` returns the first occupied slot at or above `start`.
    /// Read the pool again on each call so a later slot can run immediately.
    pub fn next_ready<T, E>(
        &mut self,
        mut occupied: impl FnMut(usize) -> Result<Option<ScheduleSlot<T>>, E>,
    ) -> Result<Option<T>, E> {
        while let Some(slot) = occupied(self.next_slot)? {
            self.next_slot = slot.index + 1;
            if !slot.external && (0..=self.tick).contains(&slot.wake) {
                return Ok(Some(slot.thread));
            }
        }
        Ok(None)
    }
}

/// Thread flag bits (`+3`).
pub(crate) const FIRST_ENTRY: u8 = 1;
pub(crate) const SAVED_SLEEP: u8 = 2;
pub(crate) const INSPECT: u8 = 4;

/// Wake-time sentinels (`+8`).
pub const FINISHED: i32 = -1;
pub const DORMANT: i32 = -2;

/// Where an evaluated value is written: the address the guest passes in EBX.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Dest {
    /// A local of the frame that is current when the evaluation starts.
    Local(usize),
    /// A global cell (`set` and initializers evaluate straight into it).
    Global(usize),
}

#[derive(Debug, Clone)]
pub(crate) struct Frame {
    /// Expression being evaluated; NULL for the root frame.
    pub expr: Datum,
    pub locals: Vec<Cell>,
    /// Bytes of locals the evaluator reserves (for the stack bound).
    pub bytes: u16,
    /// Destination of the child evaluation this frame is waiting on
    /// (stored in the frame's `+8` by `0x002099f0`).
    pub pending: Dest,
}
impl Frame {
    pub fn new(expr: Datum) -> Self {
        Self {
            expr,
            locals: Vec::new(),
            bytes: 0,
            pending: Dest::Local(0),
        }
    }
    pub fn local(&self, i: usize) -> Cell {
        self.locals.get(i).copied().unwrap_or_default()
    }
    pub fn set_local(&mut self, i: usize, c: Cell) {
        if self.locals.len() <= i {
            self.locals.resize(i + 1, Cell::ZERO);
        }
        self.locals[i] = c;
    }
}

#[derive(Debug, Clone)]
pub struct Thread {
    pub(crate) salt: u16,
    pub(crate) category: Category,
    pub(crate) flags: u8,
    pub(crate) script: Option<u16>,
    pub(crate) wake: i32,
    pub(crate) saved_wake: i32,
    pub(crate) frames: Vec<Frame>,
}

/// Observable scheduling state of a thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadState {
    /// Runs on the next update at or after this tick (0: next update).
    Waiting(i32),
    /// Wake -2: dormant, or put to sleep forever.
    Dormant,
    /// Wake -1: completed (startup/dormant) or stopped.
    Finished,
    /// Other negative wake values are skipped like these two.
    Suspended(i32),
}

impl Thread {
    /// `0x002097c0`: a root frame with no expression; dormant scripts start
    /// at wake -2, everything else at 0.
    pub(crate) fn new(salt: u16, category: Category, script: Option<u16>, dormant: bool) -> Self {
        Self {
            salt,
            category,
            flags: 0,
            script,
            wake: if dormant { DORMANT } else { 0 },
            saved_wake: 0,
            frames: vec![Frame::new(Datum::NULL)],
        }
    }
    pub fn category(&self) -> Category {
        self.category
    }
    pub fn script(&self) -> Option<u16> {
        self.script
    }
    pub fn salt(&self) -> u16 {
        self.salt
    }
    pub fn depth(&self) -> usize {
        self.frames.len() - 1
    }
    /// Live frames in root-to-top order with their expression and local byte
    /// cursor. The cursor places the next guest frame after the frame header.
    pub fn frame_layout(&self) -> impl Iterator<Item = (Datum, u16)> + '_ {
        self.frames.iter().map(|frame| (frame.expr, frame.bytes))
    }
    /// The value in the root frame once its expression has completed.
    pub fn root_result(&self) -> Option<Cell> {
        let [root] = self.frames.as_slice() else { return None };
        root.locals.first().copied()
    }
    pub fn state(&self) -> ThreadState {
        match self.wake {
            FINISHED => ThreadState::Finished,
            DORMANT => ThreadState::Dormant,
            w if w < 0 => ThreadState::Suspended(w),
            w => ThreadState::Waiting(w),
        }
    }
    pub(crate) fn top(&mut self) -> &mut Frame {
        self.frames.last_mut().expect("root frame")
    }
    /// Bytes the guest stack would use: a `0x10` header plus locals per frame.
    pub(crate) fn stack_bytes(&self) -> usize {
        self.frames.iter().map(|f| 0x10 + f.bytes as usize).sum()
    }
}

/// Thread slots with `datum_new`'s allocation order (`0x0016b840`): lowest
/// free slot from the scan start, else the high-water slot; deleting lowers
/// the scan start. Iteration order is slot order, which is what `hs_update`
/// uses, so threads created mid-update in later slots run in the same tick.
#[derive(Debug, Clone)]
pub(crate) struct Pool {
    pub slots: Vec<Option<Thread>>,
    pub(crate) free_scan_start: usize,
    pub(crate) next_salt: u16,
}
pub(crate) const THREAD_CAPACITY: usize = 0x140;

impl Pool {
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            free_scan_start: 0,
            // `data_initialize` seeds the salt from the name ("hs thread").
            next_salt: u16::from_le_bytes(*b"hs") | 0x8000,
        }
    }
    pub fn clear(&mut self) {
        self.slots.clear();
        self.free_scan_start = 0;
    }
    pub fn allocate(
        &mut self,
        category: Category,
        script: Option<u16>,
        dormant: bool,
    ) -> Option<usize> {
        let slot = (self.free_scan_start..self.slots.len())
            .find(|&i| self.slots[i].is_none())
            .or_else(|| (self.slots.len() < THREAD_CAPACITY).then_some(self.slots.len()))?;
        let salt = self.next_salt;
        self.next_salt = self.next_salt.wrapping_add(1);
        if self.next_salt == 0xffff {
            self.next_salt = 0x8000;
        }
        if slot == self.slots.len() {
            self.slots.push(None);
        }
        self.slots[slot] = Some(Thread::new(salt, category, script, dormant));
        self.free_scan_start = slot + 1;
        Some(slot)
    }
    pub fn delete(&mut self, slot: usize) {
        if let Some(s) = self.slots.get_mut(slot) {
            *s = None;
        }
        self.free_scan_start = self.free_scan_start.min(slot);
        while self.slots.last().is_some_and(Option::is_none) {
            self.slots.pop();
        }
    }
    /// `data_next_index`: first occupied slot at or after `start`, with the
    /// high-water mark re-read each call.
    pub fn next(&self, start: usize) -> Option<usize> {
        (start..self.slots.len()).find(|&i| self.slots[i].is_some())
    }
}
