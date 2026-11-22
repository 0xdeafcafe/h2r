//! A single document edit offered by Play. The control queue holds a weak reference.
//! Switching off Edit and entering the socket compete for the same payload.

use std::sync::{Arc, Mutex, Weak};
use std::time::Instant;

use super::{Call, Stream};

#[derive(Debug)]
enum State {
    Offered(Call),
    Withdrawn,
    Dispatched(Instant),
}

#[derive(Debug)]
pub(in crate::play) struct ObjectDispatch {
    state: Mutex<State>,
    stream: Weak<Stream>,
    epoch: u64,
    /// Only an empty-slot respawn can replace a different body without a salt check.
    empty_slot: Option<(u8, u16)>,
}

impl ObjectDispatch {
    pub(in crate::play) fn new(
        call: Call,
        stream: &Arc<Stream>,
        epoch: u64,
        empty_slot: Option<(u8, u16)>,
    ) -> Self {
        Self {
            state: Mutex::new(State::Offered(call)),
            stream: Arc::downgrade(stream),
            epoch,
            empty_slot,
        }
    }

    /// True when no request went to the socket. A dispatched edit still needs a receipt.
    pub(in crate::play) fn withdraw(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        if matches!(*state, State::Dispatched(_)) {
            return false;
        }
        *state = State::Withdrawn;
        true
    }

    pub(in crate::play) fn dispatched_at(&self) -> Option<Instant> {
        match *self.state.lock().unwrap() {
            State::Dispatched(at) => Some(at),
            _ => None,
        }
    }

    pub(super) fn current_stream(&self) -> bool {
        let Some(store) = self.stream.upgrade().and_then(|stream| stream.snapshot()) else {
            return false;
        };
        let now = Instant::now();
        let Some((_, header)) = store.latest_header() else { return false };
        if header.epoch != self.epoch || store.stale(header, now) {
            return false;
        }
        self.empty_slot.is_none_or(|(kind, index)| {
            store.sample(now, true).is_some_and(|sample| {
                !sample.stale && !sample.objects.iter().any(|object| {
                    object.kind == kind && object.placement == Some(index)
                })
            })
        })
    }

    pub(super) fn dispatch(&self, current: bool) -> Option<Call> {
        let mut state = self.state.lock().unwrap();
        if !current {
            return None;
        }
        if matches!(*state, State::Offered(_))
            && let State::Offered(call) =
                std::mem::replace(&mut *state, State::Dispatched(Instant::now()))
        {
            return Some(call);
        }
        None
    }
}
