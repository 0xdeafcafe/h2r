//! Keep the guest's async search pending while Live gathers QoS echoes
//! and, for a dedicated assignment, prepares the lease's join route.

use crate::{
    client::{MatchRequest, Pending, Reply, Service, Session},
    status::{CANNOT_ACCESS_SERVICE, INVALID_ARGUMENT, SERVERS_BUSY},
};
use live_proto::{
    Xuid,
    hosts::{HostEndpoint, HostLease, HostState, ProviderKind, Transport},
    matchmaking::{Assignment, Candidate, Connectivity, Nat, Queue, RegionProbe, Sample, Ticket},
    optimatch::Query,
    session,
};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    net::{Ipv4Addr, SocketAddrV4},
    sync::OnceLock,
    time::{Duration, Instant},
};
use url::Url;

pub(crate) fn config() -> Option<Connectivity> {
    static CONFIG: OnceLock<Option<Connectivity>> = OnceLock::new();
    *CONFIG.get_or_init(|| {
        if std::env::var("H2_LIVE_PING_MATCHMAKING").as_deref() != Ok("1") {
            return None;
        }
        let nat = match std::env::var("H2_LIVE_NAT").as_deref() {
            Ok("open") => Some(Nat::Open),
            Ok("moderate") => Some(Nat::Moderate),
            Ok("strict") => Some(Nat::Strict),
            _ => None,
        };
        let upload_kbps = std::env::var("H2_LIVE_UPLOAD_KBPS").ok().and_then(|v| v.parse().ok());
        Some(Connectivity { nat, upload_kbps })
    })
}

#[derive(Clone)]
pub(crate) enum Outcome {
    Host(Assignment),
    Join {
        rows: Vec<session::Session>,
        frames_from_host: bool,
        dedicated: Option<DedicatedMatch>,
        tunnel: Option<(SocketAddrV4, crate::TunnelHandle)>,
    },
}

#[derive(Clone)]
pub(crate) enum Step {
    RefreshQueue,
    Retry(Vec<Sample>),
    WaitingForHost(Assignment),
    RequeueDedicated,
    CheckDedicated(Box<DedicatedJoin>),
    Complete(Outcome),
}

pub(crate) enum QueueAction {
    Probe(Vec<Candidate>, Vec<RegionProbe>),
    Wait,
    WaitForHost(Assignment),
    Host(Assignment),
    Join {
        session_id: String,
        frames_from_host: bool,
    },
}

pub(crate) fn queue_action(queue: Queue, hopper: u32, me: Xuid) -> Option<QueueAction> {
    match queue {
        Queue::Probing { candidates, region_echoes } if valid_probe_targets(&candidates, &region_echoes, me) => {
            Some(QueueAction::Probe(candidates, region_echoes))
        }
        Queue::Host { assignment } | Queue::Waiting { assignment }
            if assignment.host_lease.is_some()
                && assignment.hopper == hopper
                && (assignment.members.contains(&me) || assignment.host == me) =>
        {
            // A lease owns hosting even if the queue still names this client.
            Some(QueueAction::WaitForHost(assignment))
        }
        Queue::Waiting { assignment }
            if assignment.hopper == hopper && assignment.members.contains(&me) =>
        {
            if assignment.dedicated && assignment.host == Xuid(0) {
                Some(QueueAction::WaitForHost(assignment))
            } else {
                Some(QueueAction::Wait)
            }
        }
        Queue::Host { assignment }
            if assignment.host_lease.is_none()
                && assignment.hopper == hopper
                && assignment.host == me
                && session::valid_id(&assignment.id) =>
        {
            Some(QueueAction::Host(assignment))
        }
        Queue::Join {
            session_id,
            frames_from_host,
        } if session::valid_id(&session_id) => Some(QueueAction::Join {
            session_id,
            frames_from_host,
        }),
        _ => None,
    }
}

pub(crate) fn join_assignment<'a>(
    row: &'a session::Session,
    waiting: Option<&'a Assignment>,
    hopper: u32,
    me: Xuid,
    frames_from_host: bool,
) -> Option<&'a Assignment> {
    let assignment = match &row.matchmaking {
        Some(assignment) if assignment.host == row.host_xuid => assignment,
        // Waiting can name XUID zero until the host publishes. The lease
        // endpoint must then confirm the exact published session ID.
        None if frames_from_host => waiting.filter(|a| {
            a.host_lease.is_some() && (a.host == Xuid(0) || a.host == row.host_xuid)
        })?,
        _ => return None,
    };
    (assignment.hopper == hopper && assignment.members.contains(&me)).then_some(assignment)
}

#[derive(Deserialize)]
pub(crate) struct HostDetail {
    pub lease: HostLease,
    pub state: HostState,
    pub endpoint: Option<HostEndpoint>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Route {
    pub address: SocketAddrV4,
    xnaddr: [u8; 36],
    pub tunnel_url: Option<Url>,
    lease: String,
}

impl Route {
    pub(crate) fn from_endpoint(endpoint: &HostEndpoint, api: &str, lease: &str) -> Option<Self> {
        let hex = &endpoint.xnaddr;
        let mut xnaddr = [0; 36];
        if hex.len() != xnaddr.len() * 2 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        for (i, byte) in xnaddr.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
        }
        let (address, tunnel_url) = match endpoint.transport {
            Transport::Udp => (endpoint.udp.as_ref()?.parse::<SocketAddrV4>().ok()?, None),
            Transport::Tunnel => {
                let ip = Ipv4Addr::new(xnaddr[0], xnaddr[1], xnaddr[2], xnaddr[3]);
                if !ip.octets().starts_with(&[10, 77]) {
                    return None;
                }
                let port = u16::from_be_bytes([xnaddr[8], xnaddr[9]]);
                let url = Url::parse(endpoint.tunnel_url.as_ref()?).ok()?;
                // The bearer is for Live's lease route on the signed-in API.
                let mut expected = Url::parse(&format!("{api}/hosts/{lease}/tunnel")).ok()?;
                let scheme = match expected.scheme() {
                    "https" => "wss",
                    "http" => "ws",
                    _ => return None,
                };
                expected.set_scheme(scheme).ok()?;
                if url != expected {
                    return None;
                }
                (SocketAddrV4::new(ip, port), Some(url))
            }
        };
        if !valid_address(*address.ip(), address.port()) {
            return None;
        }
        Some(Self {
            address,
            xnaddr,
            tunnel_url,
            lease: lease.to_owned(),
        })
    }

    pub(crate) fn apply(&self, row: &mut session::Session) {
        row.advertisement.host_address = self.xnaddr.to_vec();
        row.endpoint.addr = *self.address.ip();
        row.endpoint.port = self.address.port();
        row.endpoint.source = "dedicated".into();
    }

    pub(crate) fn still_assigned(&self, row: &session::Session) -> bool {
        row.matchmaking.as_ref().is_none_or(|assignment| {
            assignment.host_lease.as_deref() == Some(self.lease.as_str())
        })
    }
}

pub(crate) fn valid_address(ip: Ipv4Addr, port: u16) -> bool {
    (1..=65529).contains(&port) && !ip.is_unspecified() && !ip.is_multicast() && !ip.is_broadcast()
}

#[derive(Clone)]
pub(crate) struct DedicatedJoin {
    pub row: session::Session,
    pub hopper: u32,
    pub route: Route,
    pub frames_from_host: bool,
    pub region: String,
}

#[derive(Clone)]
pub(crate) struct DedicatedMatch {
    pub hopper: u32,
    pub route: Route,
}

/// An unusable lease sends the search back to the queue for a current
/// assignment. Only the queue can elect a fallback peer host.
pub(crate) enum HostAction {
    Wait,
    Requeue,
    Join { route: Route, region: String },
}

pub(crate) fn host_action(
    host: HostDetail,
    assignment: &Assignment,
    session_id: &str,
    api: &str,
) -> HostAction {
    let lease = &host.lease;
    if assignment.host_lease.as_deref() != Some(lease.id.as_str())
        || lease.assignment_id != assignment.id
        || lease.hopper != Some(assignment.hopper)
    {
        return HostAction::Requeue;
    }
    match host.state {
        HostState::Requested | HostState::Booting => HostAction::Wait,
        HostState::Joinable | HostState::Playing => {
            let Some(endpoint) = host.endpoint else {
                return HostAction::Wait;
            };
            if endpoint.session_id.as_deref() != Some(session_id)
                || endpoint.transport != lease.transport
            {
                return HostAction::Requeue;
            }
            let Some(route) = Route::from_endpoint(&endpoint, api, &lease.id) else {
                return HostAction::Requeue;
            };
            let region = lease
                .region
                .as_deref()
                .filter(|region| {
                    !region.is_empty()
                        && region.len() <= 24
                        && region.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                })
                .unwrap_or(if lease.provider == ProviderKind::Local {
                    "local"
                } else {
                    "unknown region"
                })
                .to_owned();
            HostAction::Join { route, region }
        }
        HostState::Ended | HostState::Failed => HostAction::Requeue,
    }
}

pub(crate) fn valid_host_lease(id: &str) -> bool {
    id.strip_prefix("hl-").is_some_and(|suffix| {
        suffix.len() == 22
            && suffix.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    })
}

pub(crate) struct Search {
    ticket: Ticket,
    legacy: MatchRequest,
    service: Service,
    connection: Session,
    pending: Option<Pending>,
    dedicated: Option<Box<DedicatedJoin>>,
    waiting: Option<Assignment>,
    status: Option<String>,
    // Once a lease has been seen, a missing queue route must not induce
    // the title to host by falling back to a legacy empty search.
    saw_lease: bool,
    after: Instant,
    deadline: Instant,
}

impl Search {
    pub fn new(
        service: &Service,
        connection: &Session,
        ticket: Ticket,
        query: Query,
        limit: u32,
    ) -> Result<Self, u32> {
        let pending = service.match_request(
            connection,
            MatchRequest::Queue {
                ticket: ticket.clone(),
                waiting: None,
            },
        )?;
        let now = Instant::now();
        Ok(Self {
            ticket,
            legacy: MatchRequest::Search {
                limit,
                attributes: BTreeMap::new(),
                optimatch: Some(query),
            },
            service: service.clone(),
            connection: connection.clone(),
            pending: Some(pending),
            dedicated: None,
            waiting: None,
            status: None,
            saw_lease: false,
            after: now,
            deadline: now + Duration::from_secs(100),
        })
    }

    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    pub fn retry_dedicated(&mut self) {
        self.saw_lease = true;
        self.retry(Vec::new(), None);
    }

    pub fn poll(&mut self) -> Result<Option<Outcome>, u32> {
        if Instant::now() >= self.deadline {
            return Err(CANNOT_ACCESS_SERVICE);
        }
        if let Some(pending) = &self.pending {
            let Some(reply) = pending.take() else {
                return Ok(None);
            };
            self.pending = None;
            let reply = match reply {
                Err(CANNOT_ACCESS_SERVICE | SERVERS_BUSY) if self.saw_lease => {
                    let status = self.status.take();
                    self.retry(Vec::new(), status);
                    return Ok(None);
                }
                other => other?,
            };
            match reply {
                Reply::MatchmakingUnsupported if !self.saw_lease => {
                    self.pending = Some(
                        self.service.match_request(&self.connection, self.legacy.clone())?,
                    );
                }
                Reply::MatchmakingUnsupported => self.retry(Vec::new(), None),
                Reply::Matches(rows) => {
                    return Ok(Some(Outcome::Join {
                        rows,
                        frames_from_host: false,
                        dedicated: None,
                        tunnel: None,
                    }));
                }
                Reply::Matchmaking(step) => match *step {
                    Step::RefreshQueue => {
                        self.after = Instant::now() + Duration::from_secs(1);
                    }
                    Step::Retry(samples) => {
                        self.waiting = None;
                        self.retry(samples, None);
                    }
                    Step::WaitingForHost(assignment) => {
                        self.saw_lease = true;
                        if self.waiting.as_ref().is_none_or(|waiting| {
                            waiting.id != assignment.id
                                || (waiting.host_lease.is_none() && assignment.host_lease.is_some())
                        }) {
                            self.deadline = dedicated_deadline(self.deadline, Instant::now());
                        }
                        self.waiting = Some(assignment);
                        self.retry(Vec::new(), Some("Starting a dedicated host…".into()));
                    }
                    Step::RequeueDedicated => self.retry_dedicated(),
                    Step::CheckDedicated(join) => {
                        self.saw_lease = true;
                        self.status = Some(format!("Joining dedicated host ({})", join.region));
                        self.dedicated = Some(join);
                        self.after = Instant::now();
                    }
                    Step::Complete(outcome) => return Ok(Some(outcome)),
                },
                _ => return Err(INVALID_ARGUMENT),
            }
        } else if Instant::now() >= self.after {
            let request = match self.dedicated.take() {
                Some(join) => MatchRequest::JoinDedicated(join),
                None => MatchRequest::Queue {
                    ticket: self.ticket.clone(),
                    waiting: self.waiting.clone(),
                },
            };
            self.pending = Some(self.service.match_request(&self.connection, request)?);
        }
        Ok(None)
    }

    fn retry(&mut self, samples: Vec<Sample>, status: Option<String>) {
        self.ticket.samples = samples;
        self.status = status;
        self.after = Instant::now() + Duration::from_secs(1);
    }
}

fn dedicated_deadline(deadline: Instant, now: Instant) -> Instant {
    // Live gives the host 60 seconds from reservation. Leave time for the
    // queue to observe the boot timeout and return its fallback decision.
    deadline.max(now + Duration::from_secs(65))
}

pub(crate) fn solo_hopper(query: Query, users: &[u64; 4]) -> Option<u32> {
    let hopper = u32::try_from(query.hopper).ok()?;
    (query.party == Some(1)
        && (hopper as usize) < live_proto::hopper::HOPPERS
        && users.iter().filter(|&&user| user != 0).count() == 1)
        .then_some(hopper)
}

fn valid_probe_targets(candidates: &[Candidate], echoes: &[RegionProbe], me: Xuid) -> bool {
    use std::collections::BTreeSet;
    let mut seen = BTreeSet::new();
    echoes.len() <= live_proto::matchmaking::MAX_REGION_ECHOES
        && candidates.len() + echoes.len() <= live_proto::matchmaking::MAX_CANDIDATES
        && echoes.iter().all(|v| v.xuid != me && seen.insert(v.xuid)
            && session::valid_id(&v.generation) && live_proto::matchmaking::valid_probe_region(&v.region))
        && candidates.iter().all(|v| {
            v.xuid != me
                && seen.insert(v.xuid)
                && session::valid_id(&v.generation)
                && valid_address(*v.endpoint.ip(), v.endpoint.port())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use live_proto::hosts::HostClass;
    use serde_json::json;

    const LEASE: &str = "hl-AAAAAAAAAAAAAAAAAAAAAA";
    const ASSIGNMENT: &str = "8000000000000001";
    const SESSION: &str = "8000000000000002";
    const API: &str = "https://live.example/v1";
    const HOPPER: u32 = 3;
    const ME: Xuid = Xuid(10);
    const HOST: Xuid = Xuid(20);

    #[test]
    fn regional_probes_are_same_origin_labels_with_unique_targets() {
        let mut probe = RegionProbe { xuid: HOST, region: "weur".into(), generation: ASSIGNMENT.into() };
        assert!(valid_probe_targets(&[], &[probe.clone()], ME));
        assert!(!valid_probe_targets(&[], &[probe.clone(), probe.clone()], ME));
        for invalid in ["/other", "../escape", "x?next=z", "a%2fb", "example.com"] {
            probe.region = invalid.into();
            assert!(!valid_probe_targets(&[], &[probe.clone()], ME));
        }
    }

    fn assignment() -> Assignment {
        Assignment {
            id: ASSIGNMENT.into(),
            hopper: HOPPER,
            host: HOST,
            dedicated: true,
            members: vec![ME],
            teams: BTreeMap::new(),
            rtt_ms: BTreeMap::new(),
            host_lease: Some(LEASE.into()),
        }
    }

    fn host(state: HostState, transport: Transport) -> HostDetail {
        let advertised = crate::network::xnaddr("10.77.0.1:32000".parse().unwrap());
        let endpoint = json!({
            "udp": (transport == Transport::Udp).then_some("192.0.2.10:31000"),
            "tunnel_url": (transport == Transport::Tunnel)
                .then(|| format!("wss://live.example/v1/hosts/{LEASE}/tunnel")),
            "xnaddr": live_proto::sign::hex(&advertised),
            "session_id": SESSION,
            "transport": &transport,
        });
        let lease = HostLease {
            id: LEASE.into(),
            assignment_id: ASSIGNMENT.into(),
            provider: ProviderKind::Local,
            region: Some("lhr".into()),
            class: HostClass::Small,
            map: "lockout".into(),
            map_id: None,
            variant: None,
            hopper: Some(HOPPER),
            max_players: 8,
            roster: vec![ME],
            bots: vec![],
            transport,
            idle_timeout_s: 120,
            max_match_s: 1800,
            created_ms: 0,
        };
        serde_json::from_value(json!({ "lease": lease, "state": state, "endpoint": endpoint }))
            .unwrap()
    }

    fn row(matchmaking: Option<Assignment>) -> session::Session {
        serde_json::from_value(json!({
            "session_id": SESSION,
            "host": "host-account",
            "host_xuid": HOST,
            "host_gamertag": "Dedicated",
            "xnkid": SESSION,
            "endpoint": { "addr": "127.0.0.1", "port": 3074, "source": "observed" },
            "advertisement": {
                "port": 3074,
                "host_address": vec![0u8; 36],
                "slots": { "public_open": 8, "public_filled": 0, "private_open": 0, "private_filled": 0 },
                "attributes": {}
            },
            "matchmaking": matchmaking,
            "since": "2026-11-19T00:00:00Z",
            "last_activity": 0,
            "state": "open"
        })).unwrap()
    }

    #[test]
    fn leased_cohort_waits_even_if_it_names_this_client_as_host() {
        let mut assignment = assignment();
        assignment.host = ME;
        for queue in [
            Queue::Waiting { assignment: assignment.clone() },
            Queue::Host { assignment: assignment.clone() },
        ] {
            assert!(matches!(queue_action(queue, HOPPER, ME), Some(QueueAction::WaitForHost(_))));
        }

        assignment.host_lease = None;
        assert!(matches!(
            queue_action(Queue::Host { assignment: assignment.clone() }, HOPPER, ME),
            Some(QueueAction::Host(_)),
        ));
        assert!(matches!(
            queue_action(Queue::Waiting { assignment }, HOPPER, ME),
            Some(QueueAction::Wait),
        ));
    }

    #[test]
    fn dedicated_reservation_waits_before_a_lease_exists_and_extends_a_late_search() {
        let mut assignment = assignment();
        assignment.host = Xuid(0);
        assignment.host_lease = None;
        assert!(matches!(
            queue_action(Queue::Waiting { assignment }, HOPPER, ME),
            Some(QueueAction::WaitForHost(_)),
        ));

        let now = Instant::now();
        assert_eq!(dedicated_deadline(now + Duration::from_secs(2), now), now + Duration::from_secs(65));
        assert_eq!(dedicated_deadline(now + Duration::from_secs(100), now), now + Duration::from_secs(100));
    }

    #[test]
    fn the_queue_join_carries_the_published_id_and_frame_policy() {
        let reply = serde_json::from_value(json!({
            "state": "join", "session_id": SESSION, "frames_from_host": true
        })).unwrap();
        assert!(matches!(
            queue_action(reply, HOPPER, ME),
            Some(QueueAction::Join { session_id, frames_from_host: true }) if session_id == SESSION,
        ));
        let old_reply = serde_json::from_value(json!({ "state": "join", "session_id": SESSION })).unwrap();
        assert!(matches!(
            queue_action(old_reply, HOPPER, ME),
            Some(QueueAction::Join { frames_from_host: false, .. }),
        ));
    }

    #[test]
    fn a_cached_reservation_applies_only_to_a_dedicated_join_missing_metadata() {
        let mut waiting = assignment();
        let bare = row(None);
        assert_eq!(join_assignment(&bare, Some(&waiting), HOPPER, ME, true), Some(&waiting));
        assert!(join_assignment(&bare, Some(&waiting), HOPPER, ME, false).is_none());
        assert!(join_assignment(&bare, Some(&waiting), HOPPER, Xuid(99), true).is_none());
        waiting.host = Xuid(0);
        assert_eq!(join_assignment(&bare, Some(&waiting), HOPPER, ME, true), Some(&waiting));
        assert!(join_assignment(&row(Some(waiting.clone())), None, HOPPER, ME, true).is_none());

        let mut peer = assignment();
        peer.host_lease = None;
        let published = row(Some(peer));
        let selected = join_assignment(&published, Some(&waiting), HOPPER, ME, true).unwrap();
        assert!(selected.host_lease.is_none());
        assert!(join_assignment(&published, Some(&waiting), HOPPER + 1, ME, true).is_none());
    }

    #[test]
    fn a_booting_host_waits_and_a_failed_or_replaced_lease_requeues() {
        let assignment = assignment();
        assert!(matches!(
            host_action(host(HostState::Booting, Transport::Udp), &assignment, SESSION, API),
            HostAction::Wait,
        ));
        for state in [HostState::Failed, HostState::Ended] {
            assert!(matches!(
                host_action(host(state, Transport::Udp), &assignment, SESSION, API),
                HostAction::Requeue,
            ));
        }
        assert!(matches!(
            host_action(host(HostState::Joinable, Transport::Udp), &assignment, ASSIGNMENT, API),
            HostAction::Requeue,
        ));
    }

    #[test]
    fn udp_preserves_the_published_xnaddr_identity_and_uses_the_lease_base() {
        let assignment = assignment();
        let host = host(HostState::Joinable, Transport::Udp);
        let published_xnaddr = host.endpoint.as_ref().unwrap().xnaddr.clone();
        let HostAction::Join { route, region } = host_action(host, &assignment, SESSION, API) else {
            panic!("expected a joinable UDP endpoint");
        };
        let mut row = row(Some(assignment));
        assert_eq!(region, "lhr");
        assert!(route.still_assigned(&row));
        route.apply(&mut row);
        assert_eq!(row.session_id, SESSION);
        assert_eq!(row.endpoint.addr, Ipv4Addr::new(192, 0, 2, 10));
        assert_eq!(row.endpoint.port, 31000);
        assert_eq!(live_proto::sign::hex(&row.advertisement.host_address), published_xnaddr);

        row.matchmaking.as_mut().unwrap().host_lease = None;
        assert!(!route.still_assigned(&row));
    }

    #[test]
    fn tunnel_relay_bearer_is_only_sent_to_the_live_lease_route() {
        let assignment = assignment();
        let host = host(HostState::Joinable, Transport::Tunnel);
        let mut wrong_route = host.endpoint.clone().unwrap();
        wrong_route.tunnel_url = Some(format!("wss://elsewhere.example/v1/hosts/{LEASE}/tunnel"));
        assert!(Route::from_endpoint(&wrong_route, API, LEASE).is_none());
        wrong_route.tunnel_url = Some("wss://live.example/v1/tunnel".into());
        assert!(Route::from_endpoint(&wrong_route, API, LEASE).is_none());

        let HostAction::Join { route, .. } = host_action(host, &assignment, SESSION, API) else {
            panic!("expected a joinable tunnel endpoint");
        };
        assert_eq!(route.address, "10.77.0.1:32000".parse::<SocketAddrV4>().unwrap());
        assert!(route.tunnel_url.is_some());
    }
}
