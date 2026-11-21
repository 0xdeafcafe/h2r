//! Report the sum of a cache or queue size across all of its live owners.

use std::sync::Once;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug)]
pub struct Counter {
    name: &'static str,
    registered: Once,
    total: AtomicU64,
}

impl Counter {
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            registered: Once::new(),
            total: AtomicU64::new(0),
        }
    }

    /// Register the probe on first use and account for this owner's initial size.
    pub fn track(&'static self, size: u64) -> Gauge {
        self.registered.call_once(|| {
            crate::register_probe(self.name, move || self.total.load(Ordering::Relaxed));
        });
        let mut gauge = Gauge {
            counter: self,
            size: 0,
        };
        gauge.set(size);
        gauge
    }
}

#[derive(Debug)]
pub struct Gauge {
    counter: &'static Counter,
    size: u64,
}

impl Gauge {
    /// Publish a replacement size. Unchanged capacity costs no atomic write.
    pub fn set(&mut self, size: u64) {
        if size > self.size {
            self.counter
                .total
                .fetch_add(size - self.size, Ordering::Relaxed);
        } else if size < self.size {
            self.counter
                .total
                .fetch_sub(self.size - size, Ordering::Relaxed);
        }
        self.size = size;
    }

    pub fn get(&self) -> u64 {
        self.size
    }
}

impl Drop for Gauge {
    fn drop(&mut self) {
        self.set(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gauges_add_and_retire_independently() {
        static QUEUE: Counter = Counter::new("test.gauge_queue");
        let mut first = QUEUE.track(123);
        let second = QUEUE.track(20);
        first.set(2);
        assert_eq!(QUEUE.total.load(Ordering::Relaxed), 22);
        drop(second);
        assert_eq!(QUEUE.total.load(Ordering::Relaxed), 2);
        drop(first);
        assert_eq!(QUEUE.total.load(Ordering::Relaxed), 0);
    }
}
