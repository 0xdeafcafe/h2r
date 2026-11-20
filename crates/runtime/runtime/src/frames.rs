//! Best-effort guest call chain for diagnostics. The runner records every
//! CALL it routes and forgets records once ESP rises above their return
//! slot, so ordinary returns, `ret n`, tail jumps and longjmp-style unwinds
//! all drop the right frames once the next transfer is routed. It never affects control flow: the guest
//! stack, not this record, decides where a RET goes.
use crate::events::GuestFrame;
use std::collections::VecDeque;

/// Records kept per context. Deeper chains drop their outermost records.
pub(crate) const MAX_RECORDS: usize = 4096;
/// Frames copied into one event, innermost first.
pub const MAX_BACKTRACE: usize = 64;

#[derive(Debug, Clone, Copy)]
struct CallRecord {
    /// The function that made the call.
    caller: u32,
    /// Its continuation, as pushed on the guest stack.
    return_pc: u32,
    /// ESP after the push: the address of the return slot.
    slot: u32,
}

/// A position in a [`ShadowStack`] ([`ShadowStack::mark`]).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Mark {
    len: usize,
    current: u32,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct ShadowStack {
    records: VecDeque<CallRecord>,
    current: u32,
}

impl ShadowStack {
    /// A CALL transfer: `slot` is ESP with the return address already pushed.
    pub(crate) fn call(&mut self, callee: u32, return_pc: u32, slot: u32) {
        if self.records.len() == MAX_RECORDS {
            self.records.pop_front();
        }
        self.records.push_back(CallRecord {
            caller: self.current,
            return_pc,
            slot,
        });
        self.current = callee;
    }

    /// Any transfer after which ESP is `esp`. Records whose return slot lies
    /// below ESP have been popped by the guest.
    pub(crate) fn unwind(&mut self, esp: u32) {
        while let Some(top) = self.records.back() {
            if top.slot >= esp {
                break;
            }
            self.current = top.caller;
            self.records.pop_back();
        }
    }

    /// The function dispatch is about to run in the current frame.
    pub(crate) fn enter(&mut self, function: u32) {
        self.current = function;
    }

    pub(crate) fn clear(&mut self) {
        self.records.clear();
        self.current = 0;
    }

    /// Where the record stands now, for [`rewind`](Self::rewind).
    pub(crate) fn mark(&self) -> Mark {
        Mark {
            len: self.records.len(),
            current: self.current,
        }
    }

    /// Drop the records made since `mark` and make its function current
    /// again (the host-call thunk, direct.rs). A record that has since
    /// unwound below the mark stays as it is.
    pub(crate) fn rewind(&mut self, mark: Mark) {
        if self.records.len() >= mark.len {
            self.records.truncate(mark.len);
            self.current = mark.current;
        }
    }

    pub(crate) fn snapshot(&self, pc: u32) -> Vec<GuestFrame> {
        let mut frames = Vec::with_capacity(MAX_BACKTRACE.min(self.records.len() + 1));
        frames.push(GuestFrame {
            function: self.current,
            pc,
        });
        frames.extend(
            self.records
                .iter()
                .rev()
                .take(MAX_BACKTRACE - 1)
                .map(|r| GuestFrame {
                    function: r.caller,
                    pc: r.return_pc,
                }),
        );
        frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn returns_and_unwinds_drop_frames_by_stack_slot() {
        let mut s = ShadowStack::default();
        s.enter(0x1000);
        s.call(0x2000, 0x1005, 0x7ffc);
        s.call(0x3000, 0x2005, 0x7ff0);
        assert_eq!(
            s.snapshot(0x3001),
            vec![
                GuestFrame {
                    function: 0x3000,
                    pc: 0x3001
                },
                GuestFrame {
                    function: 0x2000,
                    pc: 0x2005
                },
                GuestFrame {
                    function: 0x1000,
                    pc: 0x1005
                },
            ]
        );
        // `ret 8` from 0x3000: ESP passes the slot by 12.
        s.unwind(0x7ffc);
        assert_eq!(s.snapshot(0x2005)[0].function, 0x2000);
        assert_eq!(s.snapshot(0x2005).len(), 2);
        // A longjmp-style restore past both frames.
        s.call(0x3000, 0x2009, 0x7ff0);
        s.unwind(0x8000);
        assert_eq!(
            s.snapshot(0x1005),
            vec![GuestFrame {
                function: 0x1000,
                pc: 0x1005
            }]
        );
    }
    #[test]
    fn rewind_drops_what_came_after_the_mark() {
        let mut s = ShadowStack::default();
        s.enter(0x1000);
        s.call(0x2000, 0x1005, 0x7ffc);
        let mark = s.mark();
        let before = s.snapshot(0x2001);
        s.call(0x3000, 0x2005, 0x7ff0);
        s.call(0x4000, 0x3005, 0x7fe0);
        s.enter(0x4100);
        s.rewind(mark);
        assert_eq!(s.snapshot(0x2001), before);
        // Unwound below the mark since: left as it is.
        s.unwind(0x8000);
        s.rewind(mark);
        assert_eq!(s.snapshot(0x1005), vec![GuestFrame { function: 0x1000, pc: 0x1005 }]);
    }
    #[test]
    fn deep_chains_are_bounded() {
        let mut s = ShadowStack::default();
        for i in 0..(MAX_RECORDS as u32 + 10) {
            s.call(i + 1, i, 0x10_0000 - 4 * i);
        }
        assert_eq!(s.records.len(), MAX_RECORDS);
        assert_eq!(s.snapshot(0).len(), MAX_BACKTRACE);
    }
}
