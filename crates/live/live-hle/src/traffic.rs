//! What the title's datagram sockets carry, counted for the co-op latency
//! line (docs/live/live-coop.md section 15), and the knobs of our socket
//! layer that change timing.
//!
//! - `H2_XNET_LOWDELAY=1` marks the title's UDP sockets low delay (IP_TOS
//!   `0xb8`, DSCP EF). Off by default: it changes how the network queues
//!   the packets, not what is in them.
//! - Received datagrams are read into a buffer kept per thread rather than
//!   a fresh, zeroed 64 KiB one per call (no timing switch: it only removes
//!   work).

use std::cell::RefCell;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

/// Counters since the start; [`snapshot`] reads them, [`Traffic::since`]
/// makes a rate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Traffic {
    pub sent: u64,
    pub sent_bytes: u64,
    pub received: u64,
    pub received_bytes: u64,
    /// The largest datagram either way.
    pub largest: u64,
}

impl Traffic {
    /// The counts between `earlier` and `self` (`largest` is the all-time
    /// largest).
    pub fn since(&self, earlier: &Traffic) -> Traffic {
        Traffic {
            sent: self.sent.saturating_sub(earlier.sent),
            sent_bytes: self.sent_bytes.saturating_sub(earlier.sent_bytes),
            received: self.received.saturating_sub(earlier.received),
            received_bytes: self.received_bytes.saturating_sub(earlier.received_bytes),
            largest: self.largest,
        }
    }

    /// One phrase for a log line, for counts over `secs` seconds.
    pub fn describe(&self, secs: f64) -> String {
        let secs = secs.max(0.001);
        format!(
            "net {:.0} in / {:.0} out datagrams/s, {:.1} / {:.1} KiB/s, largest {} B",
            self.received as f64 / secs,
            self.sent as f64 / secs,
            self.received_bytes as f64 / 1024.0 / secs,
            self.sent_bytes as f64 / 1024.0 / secs,
            self.largest
        )
    }
}

static SENT: AtomicU64 = AtomicU64::new(0);
static SENT_BYTES: AtomicU64 = AtomicU64::new(0);
static RECEIVED: AtomicU64 = AtomicU64::new(0);
static RECEIVED_BYTES: AtomicU64 = AtomicU64::new(0);
static LARGEST: AtomicU64 = AtomicU64::new(0);

pub(crate) fn sent(bytes: usize) {
    SENT.fetch_add(1, Ordering::Relaxed);
    SENT_BYTES.fetch_add(bytes as u64, Ordering::Relaxed);
    LARGEST.fetch_max(bytes as u64, Ordering::Relaxed);
}

pub(crate) fn received(bytes: usize) {
    RECEIVED.fetch_add(1, Ordering::Relaxed);
    RECEIVED_BYTES.fetch_add(bytes as u64, Ordering::Relaxed);
    LARGEST.fetch_max(bytes as u64, Ordering::Relaxed);
}

/// The counters now.
pub fn snapshot() -> Traffic {
    Traffic {
        sent: SENT.load(Ordering::Relaxed),
        sent_bytes: SENT_BYTES.load(Ordering::Relaxed),
        received: RECEIVED.load(Ordering::Relaxed),
        received_bytes: RECEIVED_BYTES.load(Ordering::Relaxed),
        largest: LARGEST.load(Ordering::Relaxed),
    }
}

/// The largest UDP datagram.
pub(crate) const DATAGRAM: usize = 65536;

thread_local! {
    static BUFFER: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// A [`DATAGRAM`] byte buffer: this thread's kept one, or a new one.
pub(crate) fn take_buffer() -> Vec<u8> {
    let mut b = BUFFER.with(|b| std::mem::take(&mut *b.borrow_mut()));
    if b.len() != DATAGRAM {
        b = vec![0; DATAGRAM];
    }
    b
}

/// Keeps `b` for this thread's next receive.
pub(crate) fn give_buffer(b: Vec<u8>) {
    BUFFER.with(|k| *k.borrow_mut() = b);
}

/// `H2_XNET_LOWDELAY`: `1`, `on`, `true` or `yes` turn it on.
pub fn parse_low_delay(v: Option<&str>) -> bool {
    v.is_some_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "on" | "true" | "yes"
        )
    })
}

/// [`parse_low_delay`] of the environment, read once.
pub(crate) fn low_delay() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| parse_low_delay(std::env::var("H2_XNET_LOWDELAY").ok().as_deref()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low_delay_is_opt_in() {
        assert!(!parse_low_delay(None));
        assert!(!parse_low_delay(Some("0")));
        assert!(!parse_low_delay(Some("")));
        for v in ["1", "on", " TRUE ", "yes"] {
            assert!(parse_low_delay(Some(v)), "{v}");
        }
    }

    #[test]
    fn rates_come_from_two_snapshots() {
        let a = Traffic {
            sent: 10,
            sent_bytes: 1000,
            received: 20,
            received_bytes: 4096,
            largest: 900,
        };
        let b = Traffic {
            sent: 40,
            sent_bytes: 4072,
            received: 50,
            received_bytes: 12288,
            largest: 1200,
        };
        let d = b.since(&a);
        assert_eq!((d.sent, d.received, d.largest), (30, 30, 1200));
        assert_eq!(
            d.describe(1.0),
            "net 30 in / 30 out datagrams/s, 8.0 / 3.0 KiB/s, largest 1200 B"
        );
        assert_eq!(
            d.describe(2.0).split(',').next().unwrap(),
            "net 15 in / 15 out datagrams/s"
        );
    }

    #[test]
    fn the_receive_buffer_is_kept_and_whole() {
        let b = take_buffer();
        assert_eq!(b.len(), DATAGRAM);
        let p = b.as_ptr();
        give_buffer(b);
        let again = take_buffer();
        assert_eq!(again.as_ptr(), p, "the same buffer comes back");
        // A short one (never given, but guard) is replaced.
        give_buffer(vec![0; 3]);
        assert_eq!(take_buffer().len(), DATAGRAM);
        give_buffer(again);
    }

    #[test]
    fn counting_adds_up() {
        let before = snapshot();
        sent(100);
        received(70_000.min(DATAGRAM));
        let d = snapshot().since(&before);
        // Other tests may count too; at least ours.
        assert!(d.sent >= 1 && d.sent_bytes >= 100);
        assert!(d.received >= 1);
        assert!(snapshot().largest >= DATAGRAM as u64);
    }
}
