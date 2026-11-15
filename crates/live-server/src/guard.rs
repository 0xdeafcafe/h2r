//! In-memory request guards: a replay set for token `jti`s, event nonces
//! and invites, and per-key rate limits (docs/live-security.md §3, §6).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

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
