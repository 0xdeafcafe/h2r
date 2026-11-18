//! In-memory request guards: a replay set for token `jti`s, event nonces
//! and invites, and per-key rate limits (docs/live/live-security.md §3, §6).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use crate::rt::Instant;

/// Keys seen recently. Each entry lives until its expiry (unix seconds);
/// expired entries are pruned as new ones arrive.
#[derive(Default)]
pub struct ReplayGuard {
    seen: Mutex<Seen>,
}

#[derive(Default)]
struct Seen {
    entries: HashMap<String, i64>,
    pruned_at: i64,
}

impl ReplayGuard {
    /// Records `key` until `expires`; false if it was already live.
    pub fn insert_new(&self, key: String, expires: i64, now: i64) -> bool {
        let mut seen = self.seen.lock().unwrap();
        if now - seen.pruned_at >= 10 {
            seen.entries.retain(|_, exp| *exp >= now);
            seen.pruned_at = now;
        }
        match seen.entries.get(&key) {
            Some(exp) if *exp >= now => false,
            _ => {
                seen.entries.insert(key, expires);
                true
            }
        }
    }

    /// Forgets `key` (an invite whose account creation failed).
    pub fn remove(&self, key: &str) {
        self.seen.lock().unwrap().entries.remove(key);
    }

    pub fn contains(&self, key: &str, now: i64) -> bool {
        self.seen
            .lock()
            .unwrap()
            .entries
            .get(key)
            .is_some_and(|exp| *exp >= now)
    }
}

/// A token bucket per key: `burst` requests at once, refilled at `per_sec`.
pub struct RateLimiter {
    per_sec: f64,
    burst: f64,
    buckets: Mutex<HashMap<String, (f64, Instant)>>,
}

impl RateLimiter {
    pub fn new(per_sec: f64, burst: u32) -> Self {
        RateLimiter {
            per_sec,
            burst: burst as f64,
            buckets: Mutex::default(),
        }
    }

    /// Takes one request from `key`'s bucket; false when it is empty.
    pub fn allow(&self, key: &str) -> bool {
        self.allow_at(key, Instant::now())
    }

    fn allow_at(&self, key: &str, now: Instant) -> bool {
        let mut buckets = self.buckets.lock().unwrap();
        if buckets.len() > 10_000 {
            // Full buckets are the same as absent ones.
            let (rate, burst) = (self.per_sec, self.burst);
            buckets.retain(|_, (t, at)| *t + now.duration_since(*at).as_secs_f64() * rate < burst);
        }
        let (tokens, at) = buckets.entry(key.to_string()).or_insert((self.burst, now));
        *tokens = (*tokens + now.duration_since(*at).as_secs_f64() * self.per_sec).min(self.burst);
        *at = now;
        if *tokens >= 1.0 {
            *tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Locks a key out after `free` failures, for `base` doubling with each
/// further failure (capped at `max`): wrong recovery codes per gamertag.
/// A success clears it; a key quiet for `max` starts over.
pub struct Backoff {
    free: u32,
    base: Duration,
    max: Duration,
    fails: Mutex<HashMap<String, (u32, Instant)>>,
}

impl Backoff {
    pub fn new(free: u32, base: Duration, max: Duration) -> Self {
        Backoff {
            free,
            base,
            max,
            fails: Mutex::default(),
        }
    }

    /// How long `key` is still locked out for, if it is.
    pub fn locked(&self, key: &str) -> Option<Duration> {
        self.locked_at(key, Instant::now())
    }

    /// Records a failure; returns how many more `key` may make before it
    /// is locked out (0: locked now).
    pub fn fail(&self, key: &str) -> u32 {
        self.fail_at(key, Instant::now())
    }

    pub fn clear(&self, key: &str) {
        self.fails.lock().unwrap().remove(key);
    }

    fn lock_for(&self, n: u32) -> Duration {
        if n < self.free {
            return Duration::ZERO;
        }
        let doublings = (n - self.free).min(20);
        self.base.saturating_mul(1 << doublings).min(self.max)
    }

    fn locked_at(&self, key: &str, now: Instant) -> Option<Duration> {
        let fails = self.fails.lock().unwrap();
        let (n, last) = fails.get(key)?;
        let until = *last + self.lock_for(*n);
        (until > now).then(|| until - now)
    }

    fn fail_at(&self, key: &str, now: Instant) -> u32 {
        let mut fails = self.fails.lock().unwrap();
        let max = self.max;
        if fails.len() > 10_000 {
            fails.retain(|_, (_, last)| now.duration_since(*last) < max);
        }
        let entry = fails.entry(key.to_string()).or_insert((0, now));
        if now.duration_since(entry.1) >= max {
            entry.0 = 0;
        }
        *entry = (entry.0 + 1, now);
        self.free.saturating_sub(entry.0)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn backoff_locks_after_the_free_failures_and_doubles() {
        let b = Backoff::new(3, Duration::from_secs(60), Duration::from_secs(600));
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        assert_eq!(b.fail_at("a", at(0)), 2, "tries left");
        assert_eq!(b.fail_at("a", at(1)), 1);
        assert_eq!(b.locked_at("a", at(1)), None);
        assert_eq!(b.fail_at("a", at(2)), 0, "locked now");
        assert_eq!(b.locked_at("a", at(2)), Some(Duration::from_secs(60)));
        assert_eq!(b.locked_at("b", at(2)), None, "per key");
        assert_eq!(b.locked_at("a", at(62)), None);
        b.fail_at("a", at(62));
        assert_eq!(b.locked_at("a", at(62)), Some(Duration::from_secs(120)));
        for s in 63..70 {
            b.fail_at("a", at(s));
        }
        assert_eq!(
            b.locked_at("a", at(69)),
            Some(Duration::from_secs(600)),
            "capped"
        );
        // Quiet for the cap: starts over.
        b.fail_at("a", at(69 + 600));
        assert_eq!(b.locked_at("a", at(69 + 600)), None);
        b.clear("a");
        assert_eq!(b.locked_at("a", at(69 + 600)), None);
    }

    #[test]
    fn replay_guard_expires() {
        let g = ReplayGuard::default();
        assert!(g.insert_new("a".into(), 100, 0));
        assert!(!g.insert_new("a".into(), 100, 50));
        assert!(g.contains("a", 100));
        assert!(!g.contains("a", 101));
        // Expired: usable again (and pruned).
        assert!(g.insert_new("a".into(), 300, 200));
        assert!(g.insert_new("b".into(), 300, 200));
    }

    #[test]
    fn rate_limit_refills() {
        let r = RateLimiter::new(2.0, 3);
        let t = Instant::now();
        assert!((0..3).all(|_| r.allow_at("k", t)));
        assert!(!r.allow_at("k", t));
        assert!(r.allow_at("other", t));
        assert!(r.allow_at("k", t + Duration::from_millis(500)));
        assert!(!r.allow_at("k", t + Duration::from_millis(500)));
    }
}
