//! Opt-in OptiMatch tickets. The same router and session log serve native
//! Live and the Worker. Tickets expire on access, without a timer.
mod dedicated;
mod selection;
#[cfg(test)]
mod tests;

use crate::{
    ConnectInfo,
    app::{ApiError, Live, conflict, forbidden, invalid, not_found, now},
    hosts::HostView,
    http::{bearer, json},
    model::Model,
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
    hosts::{
        DEFAULT_IDLE_TIMEOUT_S, DEFAULT_MAX_MATCH_S, HostClass, HostLease, HostState, ProviderKind,
        Transport,
    },
    matchmaking::{
        ASSIGNMENT_SECONDS, Assignment, Candidate, Connectivity, MAX_CANDIDATES, Queue, Ticket,
    },
    session::PING_MAX_MS,
};
use dedicated::{DedicatedConfig, DedicatedPlaylist, RegionTable};
use selection::{Entrant, Host, Samples};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::{IpAddr, SocketAddr, SocketAddrV4},
    sync::Arc,
};

const TICKET_SECONDS: i64 = 10;
const REGISTRATION_SECONDS: i64 = 3600;
const BOOT_SECONDS: i64 = 60;
// The playerless host still occupies one of the sixteen machine slots.
const MAX_DEDICATED_SOLO_PLAYERS: usize = 15;

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
    placement: Placement,
}

enum Placement {
    PeerOrRegistered,
    Booting {
        fallback: Assignment,
    },
    Ready {
        session_id: String,
    },
}

enum BootProgress {
    Pending,
    Ready { session_id: String, ready_ms: i64 },
    Failed,
}

impl BootProgress {
    fn from_view(view: Option<HostView>) -> Self {
        let Some(view) = view else {
            return Self::Failed;
        };
        match view.state {
            HostState::Ended | HostState::Failed => Self::Failed,
            HostState::Joinable | HostState::Playing => {
                let session_id = view.endpoint
                    .and_then(|endpoint| endpoint.session_id)
                    .filter(|id| live_proto::session::valid_id(id));
                let boot_ms = view.boot_ms.and_then(|ms| i64::try_from(ms).ok());
                match (session_id, boot_ms) {
                    (Some(session_id), Some(boot_ms)) => Self::Ready {
                        session_id,
                        ready_ms: view.lease.created_ms.saturating_add(boot_ms),
                    },
                    _ => Self::Pending,
                }
            }
            HostState::Requested | HostState::Booting => Self::Pending,
        }
    }
}

impl Lease {
    fn fallback(&mut self, clock: i64) {
        let Placement::Booting { fallback } = &mut self.placement else {
            return;
        };
        std::mem::swap(&mut self.assignment, fallback);
        self.placement = Placement::PeerOrRegistered;
        self.joiners = self.assignment.members.len() as u32 - 1;
        self.until = self.until.min(clock) + ASSIGNMENT_SECONDS;
    }

    fn retain(&mut self, clock: i64) -> bool {
        if matches!(self.placement, Placement::Booting { .. }) {
            if self.assignment.host_lease.is_some() {
                // A member's next poll checks when the bound host became ready.
                return clock < self.until + ASSIGNMENT_SECONDS;
            }
            if self.until <= clock {
                self.fallback(clock);
            }
        }
        clock < self.until
    }

    fn poll_host(&mut self, lease_id: &str, clock: i64, progress: BootProgress) {
        if !matches!(self.placement, Placement::Booting { .. })
            || self.assignment.host_lease.as_deref() != Some(lease_id)
            || clock >= self.until + ASSIGNMENT_SECONDS
        {
            return;
        }
        match progress {
            BootProgress::Ready { session_id, ready_ms } if ready_ms < self.until * 1000 => {
                self.placement = Placement::Ready { session_id };
                self.until = clock + ASSIGNMENT_SECONDS;
            }
            BootProgress::Pending if clock < self.until => {}
            _ => self.fallback(clock),
        }
    }

    fn queue(&self, xuid: Xuid, model: &Model) -> Queue {
        let assignment = &self.assignment;
        match &self.placement {
            Placement::Booting { .. } => Queue::Waiting {
                assignment: assignment.clone(),
            },
            Placement::Ready { session_id } => Queue::Join {
                session_id: session_id.clone(),
                frames_from_host: true,
            },
            Placement::PeerOrRegistered if assignment.host == xuid => Queue::Host {
                assignment: assignment.clone(),
            },
            Placement::PeerOrRegistered => {
                if model.read().sessions.get(&assignment.id).is_some_and(|s| {
                    s.host_xuid == assignment.host
                        && s.state == "open"
                        && s.advertisement.slots.public_open > 0
                }) {
                    Queue::Join { session_id: assignment.id.clone(), frames_from_host: false }
                } else {
                    Queue::Waiting { assignment: assignment.clone() }
                }
            }
        }
    }
}

#[derive(Default)]
pub(crate) struct Matchmaker {
    config: DedicatedConfig,
    // The title-file hash keys the cache and separates a changed lineup.
    rules: Option<(String, [Option<Rules>; HOPPERS])>,
    buckets: BTreeMap<u32, Bucket>,
    dedicated: BTreeMap<Xuid, i64>,
    leases: BTreeMap<String, Lease>,
}

impl Matchmaker {
    fn expire(&mut self, clock: i64) {
        self.dedicated.retain(|_, until| *until > clock);
        self.leases.retain(|_, lease| lease.retain(clock));
        for bucket in self.buckets.values_mut() {
            bucket.expire(clock, &self.dedicated);
        }
    }

    pub fn lease(&self, id: &str, clock: i64) -> Option<&Lease> {
        self.leases.get(id).filter(|l| {
            l.until > clock && matches!(l.placement, Placement::PeerOrRegistered)
        })
    }

    /// The service-identity publisher must still verify the lease ID and roster.
    pub(crate) fn dedicated_lease(&self, id: &str, clock: i64) -> Option<&Lease> {
        self.leases.get(id).filter(|l| {
            l.until > clock && l.assignment.host_lease.is_some()
        })
    }

    /// Called after HostRequested is readable and before the provider starts.
    pub(crate) fn host_requested(&mut self, host: &HostLease) -> bool {
        self.bind_host_lease(host, now())
    }

    fn bind_host_lease(&mut self, host: &HostLease, clock: i64) -> bool {
        if host.assignment_id.starts_with("sim-") || host.assignment_id.starts_with("custom-") {
            return true;
        }
        let Some(lease) = self.leases.get_mut(&host.assignment_id) else {
            return false;
        };
        if !matches!(lease.placement, Placement::Booting { .. })
            || clock >= lease.until
            || host.hopper != Some(lease.assignment.hopper)
            || host.roster != lease.assignment.members
            || lease.assignment.host_lease.as_ref().is_some_and(|id| id != &host.id)
        {
            return false;
        }
        lease.assignment.host_lease = Some(host.id.clone());
        true
    }

    fn booting_host(&self, xuid: Xuid, hopper: u32) -> Option<(String, String)> {
        self.leases.values().find_map(|lease| {
            let assignment = &lease.assignment;
            if matches!(lease.placement, Placement::Booting { .. })
                && assignment.hopper == hopper
                && assignment.members.contains(&xuid)
            {
                return Some((assignment.id.clone(), assignment.host_lease.clone()?));
            }
            None
        })
    }

    fn host_request_failed(&mut self, assignment_id: &str, clock: i64) {
        if let Some(lease) = self.leases.get_mut(assignment_id) {
            lease.fallback(clock);
        }
    }

    fn submit(
        &mut self,
        ticket: Ticket,
        endpoint: SocketAddrV4,
        xuid: Xuid,
        clock: i64,
        provider: Option<ProviderKind>,
        live: &Live,
    ) -> Result<(Queue, Option<HostLease>), ApiError> {
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
            return Ok((lease.queue(xuid, &live.model), None));
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
        let provider = provider.filter(|_| {
            rules.size <= MAX_DEDICATED_SOLO_PLAYERS && self.config.enabled(ticket.hopper)
        });
        let regions = (provider.is_some() && !ticket.dedicated).then_some(&self.config.regions);
        let level = live.model.read().ranks.standing(xuid, &ticket.hopper.to_string()).level;
        let bucket = self.buckets.entry(ticket.hopper).or_default();
        bucket.submit(&ticket, xuid, endpoint, level, clock, regions)?;
        let mut request = None;
        let probe_rooms = matches!(provider, Some(ProviderKind::Local | ProviderKind::Cloudflare));
        if let Some((lease, host_request)) =
            bucket.assign(ticket.hopper, rules, clock, &self.config, provider)?
        {
            let assigned = lease.assignment.members.contains(&xuid)
                || lease.assignment.host == xuid;
            let reply = assigned.then(|| lease.queue(xuid, &live.model));
            self.leases.insert(lease.assignment.id.clone(), lease);
            request = host_request;
            if let Some(reply) = reply {
                return Ok((reply, request));
            }
        }
        let candidates: Vec<_> = bucket.waiting.values()
            .filter(|w| w.candidate.xuid != xuid && (!ticket.dedicated || !w.dedicated))
            .map(|w| &w.candidate)
            .chain(regions.into_iter().flat_map(RegionTable::candidates))
            .take(MAX_CANDIDATES)
            .cloned()
            .collect();
        let region_echoes = regions.filter(|_| probe_rooms)
            .into_iter()
            .flat_map(RegionTable::echoes)
            .take((MAX_CANDIDATES - candidates.len()).min(live_proto::matchmaking::MAX_REGION_ECHOES))
            .cloned()
            .collect();
        let reply = Queue::Probing { candidates, region_echoes };
        Ok((reply, request))
    }
}

impl Matchmaker {
    /// Count independent searchers by their current best region, including
    /// the default location (None) when there are no configured probes.
    pub(crate) fn search_demand(&self, clock: i64) -> BTreeMap<Option<String>, usize> {
        let mut demand = BTreeMap::new();
        let Some((_, rules)) = &self.rules else { return demand };
        for (&hopper, bucket) in &self.buckets {
            if !self.config.enabled(hopper)
                || !rules.get(hopper as usize).copied().flatten()
                    .is_some_and(|r| r.size <= MAX_DEDICATED_SOLO_PLAYERS)
            {
                continue;
            }
            for player in bucket.waiting.values() {
                if !player.dedicated && player.seen + TICKET_SECONDS > clock {
                    let region = self.config.regions.pick(&[player.entrant()])
                        .map(|choice| choice.region.to_owned());
                    *demand.entry(region).or_default() += 1;
                }
            }
        }
        demand
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
        regions: Option<&RegionTable>,
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
            .filter(|s| {
                s.target != xuid && (
                    self.waiting.get(&s.target)
                        .is_some_and(|w| w.candidate.generation == s.generation)
                    || regions.is_some_and(|table| table.accepts(s))
                )
            })
            .map(|s| (s.target, s.rtt_ms))
            .collect();
        let entry = self.waiting.get_mut(&xuid).expect("ticket inserted");
        entry.connectivity = ticket.connectivity;
        entry.level = level;
        entry.samples = samples;
        entry.seen = clock;
        Ok(())
    }

    fn assign(
        &mut self,
        hopper: u32,
        rules: Rules,
        clock: i64,
        config: &DedicatedConfig,
        provider: Option<ProviderKind>,
    ) -> Result<Option<(Lease, Option<HostLease>)>, ApiError> {
        let players: Vec<_> = self.waiting.values().filter(|p| !p.dedicated)
            .map(Waiting::entrant).collect();
        let hosts: Vec<_> = self.waiting.values().map(Waiting::host).collect();
        // Reserve a fallback that satisfies the normal peer QoS window. A
        // cohort that can only reach a registered host keeps the legacy path.
        let peers: Vec<_> = hosts.iter().copied().filter(|h| !h.dedicated).collect();
        let eligible = provider.zip(config.playlist(hopper));
        let peer_group = eligible.as_ref()
            .and_then(|_| selection::group(&players, &peers, rules.size, clock));
        let (group, candidates, ad_hoc) = match peer_group {
            Some(group) => (Some(group), peers.as_slice(), eligible),
            None => (selection::group(&players, &hosts, rules.size, clock), hosts.as_slice(), None),
        };
        let Some(group) = group else {
            return Ok(None);
        };
        let teams = selection::teams(&group, rules.team_count);
        let Some(selected) = selection::select_host(&group, candidates) else {
            return Ok(None);
        };
        let host = selected.host.xuid;
        let endpoint = self.waiting[&host].candidate.endpoint;
        let members: Vec<_> = group.iter().map(|p| p.xuid).collect();
        let mut assignment = Assignment {
            id: random_id()?,
            hopper,
            host,
            dedicated: selected.host.dedicated,
            members,
            teams,
            rtt_ms: selected.rtt_ms,
            host_lease: None,
        };
        let mut placement = Placement::PeerOrRegistered;
        let mut until = clock + ASSIGNMENT_SECONDS;
        let request = ad_hoc.and_then(|(provider, playlist)| {
            let choice = config.regions.pick(&group);
            if let Some(measured) = &choice
                && measured.rtt_ms.len() == group.len()
                && let Some(worst) = measured.rtt_ms.values().max()
                && *worst > 120
                && assignment.rtt_ms.values().max().is_some_and(|peer| peer < worst)
            {
                return None;
            }
            let region = choice.as_ref().map(|c| c.region.to_owned());
            let dedicated = Assignment {
                // The ad-hoc host authenticates with its lease, without an XUID.
                host: Xuid(0),
                dedicated: true,
                rtt_ms: choice.map(|c| c.rtt_ms).unwrap_or_default(),
                ..assignment.clone()
            };
            placement = Placement::Booting {
                fallback: std::mem::replace(&mut assignment, dedicated),
            };
            until = clock + BOOT_SECONDS;
            Some(host_request(&assignment, playlist, provider, region))
        });
        self.remove(host);
        for &member in &assignment.members {
            self.remove(member);
        }
        Ok(Some((Lease {
            joiners: assignment.members.len() as u32 - (!assignment.dedicated) as u32,
            assignment,
            endpoint,
            ranked: rules.ranked,
            until,
            placement,
        }, request)))
    }
}

fn host_request(
    assignment: &Assignment,
    playlist: &DedicatedPlaylist,
    provider: ProviderKind,
    region: Option<String>,
) -> HostLease {
    let transport = match provider {
        ProviderKind::Cloudflare => Transport::Tunnel,
        ProviderKind::Fly | ProviderKind::Local => Transport::Udp,
    };
    HostLease {
        // Live mints the lease ID and creation time.
        id: String::new(),
        assignment_id: assignment.id.clone(),
        provider,
        region,
        class: HostClass::Small,
        map: playlist.map.clone(),
        map_id: None,
        variant: playlist.variant.clone(),
        hopper: Some(assignment.hopper),
        max_players: assignment.members.len() as u8,
        roster: assignment.members.clone(),
        bots: Vec::new(),
        transport,
        idle_timeout_s: DEFAULT_IDLE_TIMEOUT_S,
        max_match_s: DEFAULT_MAX_MATCH_S,
        created_ms: 0,
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
    let endpoint = SocketAddrV4::new(addr, ticket.port);
    let clock = now();
    let provider = live.host_provider_kind();
    let running = if matches!(provider, Some(ProviderKind::Local | ProviderKind::Fly)) {
        Some(live.regional_udp_probes())
    } else { None };
    let (mut result, request) = {
        let mut matcher = live.matchmaking.lock().unwrap();
        if let Some(running) = running { matcher.refresh_udp_probes(running)?; }
        if let Some((assignment, lease_id)) = matcher.booting_host(xuid, ticket.hopper) {
            // Provider callbacks also take the matcher lock.
            drop(matcher);
            let progress = BootProgress::from_view(live.host_view(&lease_id));
            matcher = live.matchmaking.lock().unwrap();
            if let Some(lease) = matcher.leases.get_mut(&assignment) {
                lease.poll_host(&lease_id, clock, progress);
            }
        }
        matcher.submit(ticket, endpoint, xuid, clock, provider, &live)?
    };
    // The first ticket gets its QoS candidates while the pack starts. On
    // the Worker the hub attaches this work to the request with waitUntil.
    #[cfg(not(target_arch = "wasm32"))]
    if request.is_none() {
        let live = live.clone();
        crate::rt::spawn(async move { live.refresh_host_pools().await });
    }
    if let Some(request) = request {
        // LiveHub schedules its next sweep after this response. Wait for the
        // host request to be recorded with the matcher lock released.
        let assignment_id = request.assignment_id.clone();
        let observed = live.request_host(request).await.ok().map(|host| {
            let progress = BootProgress::from_view(live.host_view(&host.id));
            (host.id, progress)
        });
        let clock = now();
        let mut matcher = live.matchmaking.lock().unwrap();
        if let Some((lease_id, progress)) = observed {
            if let Some(lease) = matcher.leases.get_mut(&assignment_id) {
                lease.poll_host(&lease_id, clock, progress);
            }
        } else {
            matcher.host_request_failed(&assignment_id, clock);
        }
        if matches!(result, Queue::Waiting { .. }) {
            matcher.expire(clock);
            result = match matcher.leases.get(&assignment_id) {
                Some(lease) => lease.queue(xuid, &live.model),
                None => Queue::Probing { candidates: Vec::new(), region_echoes: Vec::new() },
            };
        }
    }
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
