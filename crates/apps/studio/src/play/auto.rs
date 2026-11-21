//! Discovery policy and the clock for recovering a dropped control connection.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use agent_proto::Endpoint;
use agent_proto::discover::{Found, MapMatch};

pub fn choose(
    games: &[&Found],
    open: Option<&str>,
    child: Option<&Endpoint>,
    skipped: &HashSet<PathBuf>,
) -> Option<PathBuf> {
    let game = if let Some(own) = games.iter().find(|g| Some(&g.endpoint()) == child) {
        *own
    } else if let [one] = games {
        *one
    } else {
        let mut same = games.iter().filter(|g| MapMatch::of(open, g.map()) == MapMatch::Same);
        let one = *same.next()?;
        if same.next().is_some() {
            return None;
        }
        one
    };
    (!skipped.contains(&game.path)).then(|| game.path.clone())
}

#[derive(Clone, Debug, PartialEq)]
pub struct Backoff {
    pub since: Instant,
    next: Instant,
    delay: Duration,
}

impl Backoff {
    pub fn new(now: Instant) -> Self {
        let delay = Duration::from_millis(250);
        Self { since: now, next: now + delay, delay }
    }

    pub fn expired(&self, now: Instant) -> bool {
        now.duration_since(self.since) >= Duration::from_secs(30)
    }

    pub fn due(&mut self, now: Instant) -> bool {
        if now < self.next || self.expired(now) {
            return false;
        }
        self.delay = (self.delay * 2).min(Duration::from_secs(2));
        self.next = now + self.delay;
        true
    }
}
