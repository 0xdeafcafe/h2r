//! Discovery policy and the clock for recovering a dropped control connection.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use agent_proto::Endpoint;
use agent_proto::discover::Found;

pub fn choose(
    games: &[&Found],
    same_map: impl Fn(&Found) -> bool,
    child: Option<&Endpoint>,
    skipped: impl Fn(&Found) -> bool,
) -> Option<PathBuf> {
    let game = if let Some(own) = games.iter().find(|g| Some(&g.endpoint()) == child) {
        *own
    } else if let [one] = games {
        *one
    } else {
        let mut same = games.iter().filter(|g| same_map(g));
        let one = *same.next()?;
        if same.next().is_some() {
            return None;
        }
        one
    };
    (!skipped(game)).then(|| game.path.clone())
}

/// A missed socket probe does not undo a deliberate Unpair.
#[derive(Default)]
pub(super) struct Unpaired {
    processes: HashSet<u32>,
    sockets: HashSet<PathBuf>,
}

impl Unpaired {
    pub fn remember(&mut self, path: PathBuf, pid: Option<u32>) {
        if let Some(pid) = pid {
            self.processes.insert(pid);
        } else {
            self.sockets.insert(path);
        }
    }

    pub fn contains(&self, path: &Path, pid: Option<u32>) -> bool {
        self.sockets.contains(path) || pid.is_some_and(|pid| self.processes.contains(&pid))
    }

    pub fn allow(&mut self, path: &Path, pid: Option<u32>) {
        self.sockets.remove(path);
        if let Some(pid) = pid {
            self.processes.remove(&pid);
        }
    }

    pub fn retain_processes(&mut self, alive: impl Fn(u32) -> bool) {
        self.processes.retain(|pid| alive(*pid));
    }
}

#[cfg(unix)]
pub(super) fn process_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal zero checks one positive PID and sends no signal.
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

#[cfg(not(unix))]
pub(super) fn process_alive(_: u32) -> bool {
    true
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
        Self {
            since: now,
            next: now + delay,
            delay,
        }
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::App;
    use crate::play::testing::fake::{Game, until};
    use crate::play::{Conn, Event, Play};
    use agent_proto::sim::SessionKind;
    use serde_json::json;

    #[test]
    fn deliberate_unpair_tracks_the_process_across_missing_scans() {
        let mut unpaired = Unpaired::default();
        let path = Path::new("4242.sock");
        let custom = Path::new("custom.sock");
        unpaired.remember(path.to_owned(), Some(4242));
        unpaired.remember(custom.to_owned(), None);
        unpaired.retain_processes(|pid| pid == 4242);
        assert!(unpaired.contains(path, Some(4242)));
        assert!(unpaired.contains(custom, None));
        assert!(!unpaired.contains(path, Some(4243)));

        unpaired.retain_processes(|_| false);
        assert!(!unpaired.contains(path, Some(4242)));
        assert!(unpaired.contains(custom, Some(4243)));
        unpaired.allow(custom, Some(4243));
        assert!(!unpaired.contains(custom, None));
    }

    #[test]
    fn typed_sockets_reconnect_outside_discovery_and_keep_pair_again() {
        let game = Game::start("typed-retry", "example", SessionKind::Local);
        let tcp = agent::Server::listen_on(
            &Endpoint::Tcp("127.0.0.1:0".into()),
            game.agent.clone(),
        )
        .unwrap();
        for endpoint in [Endpoint::Unix(game.path.clone()), tcp.endpoint()] {
            let mut play = Play::new();
            let mut app = App::new();
            if matches!(&endpoint, Endpoint::Unix(_)) {
                play.socket_text = endpoint.to_string();
                play.pair_socket_text(&mut app);
            } else {
                play.publish(&mut app);
                crate::api::Api::new()
                    .call(&mut app, "pair.connect", json!({"socket": endpoint.to_string()}))
                    .unwrap();
            }
            until(&mut play, &mut app, "the typed socket", |p, _| {
                p.is_paired() && p.edits.epoch.is_some()
            });
            assert!(play.finder.games.is_empty());
            play.on_event(&mut app, Event::Lost("dropped".into()));
            assert!(matches!(play.conn, Conn::Reconnecting { .. }));
            play.reconnect(&mut app, Instant::now() + Duration::from_secs(1));
            until(&mut play, &mut app, "the socket to reconnect", |p, _| p.is_paired());
            play.on_event(&mut app, Event::Lost("dropped again".into()));
            play.reconnect(&mut app, Instant::now() + Duration::from_secs(30));
            assert!(matches!(play.conn, Conn::Lost(_)));
            assert_eq!(play.retry_endpoint(), Some(&endpoint));
        }
    }
}
