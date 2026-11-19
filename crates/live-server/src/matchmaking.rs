//! Opt-in OptiMatch tickets. The same router and session log serve native
//! Live and the Worker. Tickets expire on access, without a timer.
mod selection;

use crate::{
    ConnectInfo,
    app::{ApiError, Live, conflict, forbidden, invalid, not_found, now},
    http::{bearer, json},
};
use axum::{
    body::to_bytes,
    extract::{Path, Request, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use live_proto::{
    Author, Xuid, lxm,
    hopper::{HALO_2_HOPPER_FILE, HOPPERS, HopperFile, Rules as HopperRules},
    matchmaking::{ASSIGNMENT_SECONDS, Assignment, Candidate, Connectivity, MAX_CANDIDATES, Queue, Ticket},
    session::PING_MAX_MS,
};
use selection::{Entrant, Host, Samples};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::{IpAddr, SocketAddr, SocketAddrV4},
    sync::Arc,
};

const TICKET_SECONDS: i64 = 10;
const REGISTRATION_SECONDS: i64 = 3600;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Rules {
    size: usize,
    team_count: usize,
    ranked: bool,
}

struct Waiting {
    candidate: Candidate,
    dedicated: bool,
    connectivity: Connectivity,
    samples: Samples,
    level: u8,
    since: i64,
    seen: i64,
}

impl Waiting {
    fn host(&self) -> Host {
        Host {
            xuid: self.candidate.xuid,
            dedicated: self.dedicated,
            connectivity: self.connectivity,
        }
    }
    fn entrant(&self) -> Entrant<'_> {
        Entrant {
            xuid: self.candidate.xuid,
            level: self.level,
            since: self.since,
            samples: &self.samples,
        }
    }
}

#[derive(Default)]
struct Bucket {
    waiting: BTreeMap<Xuid, Waiting>,
}

pub(crate) struct Lease {
    pub assignment: Assignment,
    pub endpoint: SocketAddrV4,
    pub ranked: bool,
    pub joiners: u32,
    until: i64,
}

#[derive(Default)]
pub(crate) struct Matchmaker {
    // The title-file hash keys the cache and separates a changed lineup.
    rules: Option<(String, [Option<Rules>; HOPPERS])>,
    buckets: BTreeMap<u32, Bucket>,
    dedicated: BTreeMap<Xuid, i64>,
    leases: BTreeMap<String, Lease>,
}

impl Matchmaker {
    fn expire(&mut self, clock: i64) {
        self.dedicated.retain(|_, until| *until > clock);
        self.leases.retain(|_, lease| lease.until > clock);
        for bucket in self.buckets.values_mut() {
            bucket.expire(clock, &self.dedicated);
        }
    }

    pub fn lease(&self, id: &str, clock: i64) -> Option<&Lease> {
        self.leases.get(id).filter(|l| l.until > clock)
    }

    fn submit(
        &mut self,
        ticket: Ticket,
        endpoint: SocketAddrV4,
        xuid: Xuid,
        level: u8,
        clock: i64,
        live: &Live,
    ) -> Result<Queue, ApiError> {
        self.expire(clock);
        if ticket.dedicated && !self.dedicated.contains_key(&xuid) {
            return Err(forbidden("a dedicated host must be registered by an operator"));
        }

        if let Some(lease) = self.leases.values().find(|l| {
            l.assignment.host == xuid || l.assignment.members.contains(&xuid)
        }) {
            if lease.assignment.hopper != ticket.hopper {
                return Err(conflict("account has an assignment on another hopper"));
            }
            let assignment = &lease.assignment;
            if assignment.host == xuid {
                return Ok(Queue::Host { assignment: assignment.clone() });
            }
            let state = live.model.read();
            if state.sessions.get(&assignment.id).is_some_and(|s| {
                s.state == "open" && s.advertisement.slots.public_open > 0
            }) {
                return Ok(Queue::Join {
                    session_id: assignment.id.clone(),
                    frames_from_host: false, // dh-lease: existing registered or peer host
                });
            }
            return Ok(Queue::Waiting { assignment: assignment.clone() });
        }

        let rules = self.rules.as_ref()
            .and_then(|(_, all)| all.get(ticket.hopper as usize).copied().flatten())
            .ok_or_else(|| not_found("hopper does not support the QoS queue"))?;
        if ticket.dedicated && live.model.read().sessions.values().any(|s| {
            s.host_xuid == xuid && s.state == "open" && s.matchmaking.is_some()
        }) {
            return Err(conflict("dedicated host already has an open assigned session"));
        }

        for (&hopper, bucket) in &mut self.buckets {
            if hopper != ticket.hopper {
                bucket.remove(xuid);
            }
        }
        let bucket = self.buckets.entry(ticket.hopper).or_default();
        bucket.submit(&ticket, xuid, endpoint, level, clock)?;
        if let Some(lease) = bucket.assign(ticket.hopper, rules, clock)? {
            let assigned = lease.assignment.members.contains(&xuid)
                || lease.assignment.host == xuid;
            let assignment = assigned.then(|| lease.assignment.clone());
            self.leases.insert(lease.assignment.id.clone(), lease);
            if let Some(assignment) = assignment {
                return Ok(if assignment.host == xuid {
                    Queue::Host { assignment }
                } else {
                    Queue::Waiting { assignment }
                });
            }
        }
        Ok(Queue::Probing {
            candidates: bucket.waiting.values()
                .filter(|w| w.candidate.xuid != xuid && (!ticket.dedicated || !w.dedicated))
                .map(|w| w.candidate.clone())
                .collect(),
        })
    }
}

impl Bucket {
    fn remove(&mut self, xuid: Xuid) {
        if self.waiting.remove(&xuid).is_some() {
            for entry in self.waiting.values_mut() {
                entry.samples.remove(&xuid);
            }
        }
    }

    fn expire(&mut self, clock: i64, dedicated: &BTreeMap<Xuid, i64>) {
        let expired: Vec<_> = self.waiting.iter()
            .filter_map(|(&x, w)| {
                (w.seen + TICKET_SECONDS <= clock || (w.dedicated && !dedicated.contains_key(&x)))
                    .then_some(x)
            })
            .collect();
        for x in expired {
            self.remove(x);
        }
    }

    fn submit(
        &mut self,
        ticket: &Ticket,
        xuid: Xuid,
        endpoint: SocketAddrV4,
        level: u8,
        clock: i64,
    ) -> Result<(), ApiError> {
        if self.waiting.get(&xuid).is_some_and(|w| {
            w.candidate.endpoint != endpoint || w.dedicated != ticket.dedicated
        }) {
            self.remove(xuid);
        }
        if !self.waiting.contains_key(&xuid) {
            if self.waiting.len() == MAX_CANDIDATES {
                return Err(conflict("hopper QoS queue is full"));
            }
            self.waiting.insert(xuid, Waiting {
                candidate: Candidate { xuid, endpoint, generation: random_id()? },
                dedicated: ticket.dedicated,
                connectivity: ticket.connectivity,
                samples: BTreeMap::new(),
                level,
                since: clock,
                seen: clock,
            });
        }
        let samples: Samples = ticket.samples.iter()
            .filter(|s| s.target != xuid && self.waiting.get(&s.target)
                .is_some_and(|w| w.candidate.generation == s.generation))
            .map(|s| (s.target, s.rtt_ms))
            .collect();
        let entry = self.waiting.get_mut(&xuid).expect("ticket inserted");
        entry.connectivity = ticket.connectivity;
        entry.level = level;
        entry.samples = samples;
        entry.seen = clock;
        Ok(())
    }

    fn assign(&mut self, hopper: u32, rules: Rules, clock: i64) -> Result<Option<Lease>, ApiError> {
        let players: Vec<_> = self.waiting.values().filter(|p| !p.dedicated)
            .map(Waiting::entrant).collect();
        let hosts: Vec<_> = self.waiting.values().map(Waiting::host).collect();
        let Some(group) = selection::group(&players, &hosts, rules.size, clock) else {
            return Ok(None);
        };
        let teams = selection::teams(&group, rules.team_count);
        let Some(selected) = selection::select_host(&group, &hosts) else {
            return Ok(None);
        };
        let host = selected.host.xuid;
        let endpoint = self.waiting[&host].candidate.endpoint;
        let members: Vec<_> = group.iter().map(|p| p.xuid).collect();
        let assignment = Assignment {
            id: random_id()?,
            hopper,
            host,
            dedicated: selected.host.dedicated,
            members,
            teams,
            rtt_ms: selected.rtt_ms,
            host_lease: None, // dh-lease: existing registered or peer host
        };
        self.remove(host);
        for &member in &assignment.members {
            self.remove(member);
        }
        Ok(Some(Lease {
            joiners: assignment.members.len() as u32 - (!assignment.dedicated) as u32,
            assignment,
            endpoint,
            ranked: rules.ranked,
            until: clock + ASSIGNMENT_SECONDS,
        }))
    }
}

fn random_id() -> Result<String, ApiError> {
    let mut bytes = [0; 8];
    getrandom::fill(&mut bytes).map_err(|_| invalid("random source unavailable"))?;
    bytes[0] = (bytes[0] & 15) | 0x80;
    Ok(live_proto::sign::hex(&bytes))
}

async fn load_rules(live: &Live) -> Result<(), ApiError> {
    let file_key = (live_proto::HALO_2_TITLE_ID, HALO_2_HOPPER_FILE.to_owned());
    let hash = live.model.read().files.get(&file_key)
        .map(|f| f.meta.sha256.clone());
    let fingerprint = hash.as_deref().unwrap_or("default");
    if live.matchmaking.lock().unwrap().rules.as_ref().is_some_and(|(key, _)| key == fingerprint) {
        return Ok(());
    }
    let (key, rules) = if hash.is_some() {
        let (file, bytes) = live.file(file_key.0, HALO_2_HOPPER_FILE).await?;
        (
            file.sha256,
            HopperFile::parse_legacy(&bytes).map_err(|_| invalid("could not read the hopper file"))?,
        )
    } else {
        (fingerprint.to_owned(), HopperFile::default_set())
    };
    let clankers = crate::admin::clanker_hoppers(Some(&rules));
    let all = std::array::from_fn(|index| {
        let descriptor = rules.descriptor(index).ok()?;
        let kind = descriptor.kind()?;
        if clankers.contains(index as u32) {
            return None;
        }
        let (size, team_count) = match descriptor.rules()? {
            HopperRules::FreeForAll { players, .. } => (players.0, 0),
            HopperRules::UnrankedTeams { teams, team_size, .. }
            | HopperRules::RankedTeams { teams, team_size, .. } if teams >= 2 && team_size.0 > 0 => {
                (teams.checked_mul(team_size.0)?, teams)
            }
            _ => return None,
        };
        (2..=16).contains(&size).then_some(Rules {
            size: size as usize,
            team_count: team_count as usize,
            ranked: kind.ranked(),
        })
    });
    let mut matcher = live.matchmaking.lock().unwrap();
    if matcher.rules.as_ref().is_none_or(|(previous, _)| previous != &key) {
        matcher.buckets.clear();
        matcher.rules = Some((key, all));
    }
    Ok(())
}

pub(crate) async fn queue(State(live): State<Arc<Live>>, req: Request) -> Result<Response, ApiError> {
    let author = live.authenticate(bearer(req.headers()), lxm::SESSIONS_WRITE)?;
    live.rate_limit(&author, lxm::SESSIONS_WRITE)?;
    let Author::Account { xuid, .. } = author else {
        return Err(forbidden("a QoS ticket needs an account"));
    };
    let peer = req.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| c.0);
    let addr = match live.policy().public.client_ip(peer, req.headers()) {
        Some(IpAddr::V4(addr)) if !addr.is_unspecified() && !addr.is_multicast()
            && !addr.is_broadcast() => addr,
        _ => return Err(invalid("an observed IPv4 endpoint is required")),
    };
    let body = to_bytes(req.into_body(), crate::http::MAX_BODY)
        .await
        .map_err(|_| invalid("QoS ticket too large"))?;
    let ticket: Ticket = serde_json::from_slice(&body).map_err(|_| invalid("invalid QoS ticket"))?;
    let mut seen = BTreeSet::new();
    if ticket.hopper >= HOPPERS as u32
        || !(1..=65529).contains(&ticket.port)
        || ticket.samples.len() > MAX_CANDIDATES
        || ticket.samples.iter().any(|s| {
            !seen.insert(s.target) || !live_proto::session::valid_id(&s.generation)
                || s.rtt_ms.is_some_and(|rtt| !(1..=PING_MAX_MS).contains(&rtt))
        })
    {
        return Err(invalid("invalid endpoint, hopper or QoS samples"));
    }
    load_rules(&live).await?;
    let level = live.model.read().ranks.standing(xuid, &ticket.hopper.to_string()).level;
    let endpoint = SocketAddrV4::new(addr, ticket.port);
    let result = live.matchmaking.lock().unwrap()
        .submit(ticket, endpoint, xuid, level, now(), &live)?;
    Ok(json(StatusCode::OK, &result))
}

pub(crate) async fn register(
    State(live): State<Arc<Live>>,
    Path(xuid): Path<Xuid>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let author = live.authenticate(bearer(&headers), lxm::SESSIONS_WRITE)?;
    crate::admin::require_admin(&live, &author)?;
    live.rate_limit(&author, lxm::SESSIONS_WRITE)?;
    if !live.model.read().accounts.contains_key(&xuid) {
        return Err(not_found("no such dedicated host account"));
    }
    let clock = now();
    let mut matcher = live.matchmaking.lock().unwrap();
    matcher.expire(clock);
    matcher.dedicated.insert(xuid, clock + REGISTRATION_SECONDS);
    Ok(StatusCode::NO_CONTENT)
}
