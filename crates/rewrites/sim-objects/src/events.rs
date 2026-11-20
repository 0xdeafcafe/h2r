//! The tick's simulation events (docs/engine/sim-rewrite-objects.md
//! section 8): appended only by serial bodies on the game thread, at the
//! retail moment of the event, and read at tick end.
//!
//! The buffer belongs to the thread that runs the walks. The pre-physics
//! walk clears it when a tick starts, so at tick end it holds that tick's
//! events. Events are not persistent state: not in keyframes, snapshots or
//! the hash, and nothing in the simulation reads them.

use guest_layout::Datum;
use std::cell::RefCell;

/// One event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SimEvent {
    /// Finalize's delete pass is about to delete the object
    /// (`b7150` pass B, before `b83b0`): its handle, its type byte (header
    /// `+3`) and its definition tag (body `+0`, none when the body could
    /// not be read).
    ObjectDeleted { id: Datum, kind: u8, definition: Datum },
}

thread_local! {
    static EVENTS: RefCell<Vec<SimEvent>> = const { RefCell::new(Vec::new()) };
}

/// Append an event.
pub fn push(event: SimEvent) {
    EVENTS.with_borrow_mut(|e| e.push(event));
}

/// Drop every event: a new tick starts.
pub fn clear() {
    EVENTS.with_borrow_mut(Vec::clear);
}

/// The events so far this tick, in the order they happened.
pub fn with<R>(f: impl FnOnce(&[SimEvent]) -> R) -> R {
    EVENTS.with_borrow(|e| f(e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_append_in_order_and_clear() {
        clear();
        let a = SimEvent::ObjectDeleted { id: Datum(0x8001_0001), kind: 2, definition: Datum::NONE };
        let b = SimEvent::ObjectDeleted { id: Datum(0x8002_0002), kind: 5, definition: Datum(0xe000_0010) };
        push(a);
        push(b);
        assert_eq!(with(<[SimEvent]>::to_vec), [a, b]);
        clear();
        assert!(with(<[SimEvent]>::is_empty));
    }
}
