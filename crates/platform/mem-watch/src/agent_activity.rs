//! The socket activity recorded alongside each footprint sample.
use std::sync::Once;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{Counter, Gauge};

pub(crate) const CONNECTIONS_PROBE: &str = "agent.connections";
pub(crate) const CALLS_PROBE: &str = "agent.calls_served";
static CONNECTIONS: Counter = Counter::new(CONNECTIONS_PROBE);
static CALLS: AtomicU64 = AtomicU64::new(0);

/// Count an open connection until its last pending work is released.
pub fn connection() -> Gauge {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        crate::register_probe(CALLS_PROBE, || CALLS.load(Ordering::Relaxed));
    });
    CONNECTIONS.track(1)
}

/// Count a parsed socket request entering a handler, including notifications.
pub fn served() {
    CALLS.fetch_add(1, Ordering::Relaxed);
}
