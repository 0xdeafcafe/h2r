//! The report the agent API's `actors.net` reads (`agent::actors`),
//! published once a tick: by the distributed host after it schedules its
//! peers, by a distributed client after it takes the tick's packets.

use std::sync::Mutex;

use ::actor_net::Stats;
use agent::actors::{Peer, Report, Role};

use super::client::Client;

static REPORT: Mutex<Option<Report>> = Mutex::new(None);

fn lock() -> std::sync::MutexGuard<'static, Option<Report>> {
    REPORT.lock().unwrap_or_else(|e| e.into_inner())
}

/// The last tick's report; role `off` when none was published.
pub fn get() -> Report {
    lock().clone().unwrap_or_default()
}

fn set(report: Report) {
    *lock() = Some(report);
}

/// Forget the report if `role` published it: the machine left that role.
pub fn clear(role: Role) {
    let mut report = lock();
    if report.as_ref().is_some_and(|r| r.role == role) {
        *report = None;
    }
}

/// The host's report: each connected peer's budget counters and its
/// bytes in the last 30 ticks, in peer order.
pub fn host(tick: u32, peers: impl IntoIterator<Item = (u32, Stats, usize)>) -> Report {
    let mut peers: Vec<Peer> = peers
        .into_iter()
        .map(|(peer, s, window)| Peer {
            peer,
            bytes_per_second: window as u64,
            packets: s.packets,
            bytes: s.bytes,
            keys: s.keys,
            digests: s.digests,
            records: s.records,
            dropped: s.dropped,
        })
        .collect();
    peers.sort_unstable_by_key(|p| p.peer);
    Report {
        role: Role::Host,
        tick: Some(tick),
        peers,
        client: None,
    }
}

/// Publish the host's report for `tick`.
pub fn publish_host(tick: u32, peers: impl IntoIterator<Item = (u32, Stats, usize)>) {
    set(host(tick, peers));
}

/// The client's report.
pub fn client(tick: u32, client: &Client) -> Report {
    Report {
        role: Role::Client,
        tick: Some(tick),
        peers: Vec::new(),
        client: Some(client.report()),
    }
}

/// Publish the client's report for `tick`.
pub fn publish_client(tick: u32, client: &Client) {
    set(self::client(tick, client));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_peers_are_reported_in_peer_order_with_their_window_as_the_rate() {
        let stats = Stats {
            packets: 4,
            bytes: 300,
            keys: 1,
            digests: 1,
            records: 9,
            dropped: 2,
        };
        let r = host(60, [(7, stats, 120), (3, Stats::default(), 0)]);
        assert_eq!(r.role, Role::Host);
        assert_eq!(r.tick, Some(60));
        assert_eq!(r.peers.iter().map(|p| p.peer).collect::<Vec<_>>(), [3, 7]);
        assert_eq!(
            r.peers[1],
            Peer {
                peer: 7,
                bytes_per_second: 120,
                packets: 4,
                bytes: 300,
                keys: 1,
                digests: 1,
                records: 9,
                dropped: 2,
            }
        );
        assert!(r.client.is_none());
    }
}
