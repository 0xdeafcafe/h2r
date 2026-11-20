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
    http::{HeaderMap, StatusCode, header},
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
const CAPABILITY_SECONDS: i64 = 30 * 60;
const CAPABILITY_SWEEP_SECONDS: i64 = 60;
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
    has_map_pack: bool,
    dedicated: bool,
    connectivity: Connectivity,
    samples: Samples,
    level: u8,
    since: i64,
    seen: i64,
}

impl Waiting {
    fn host(&self, dedicated_uses_pack: bool) -> Option<Host> {
        let requires_map_pack = self.dedicated && dedicated_uses_pack;
        (!requires_map_pack || self.has_map_pack).then_some(Host {
            xuid: self.candidate.xuid,
            dedicated: self.dedicated,
            connectivity: self.connectivity,
            requires_map_pack,
        })
    }
    fn entrant(&self) -> Entrant<'_> {
        Entrant {
            xuid: self.candidate.xuid,
            level: self.level,
            has_map_pack: self.has_map_pack,
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
    lineup: Option<Lineup>,
    buckets: BTreeMap<u32, Bucket>,
    dedicated: BTreeMap<Xuid, i64>,
    leases: BTreeMap<String, Lease>,
    map_pack_reports: BTreeMap<Xuid, (bool, i64)>,
    next_capability_sweep: i64,
}

struct Lineup {
    fingerprint: String,
    rules: [Option<Rules>; HOPPERS],
    map_pack_only: [bool; HOPPERS],
}

pub(crate) use content::map_pack::is_map_name as map_pack_map;

fn pack_only(file: &HopperFile, index: usize) -> bool {
    file.entries(index).is_ok_and(|entries| {
        !entries.is_empty()
            && entries.iter().all(|entry| !live_proto::hopper::is_base_map(entry.map_id))
    })
}

/// The typed search and ticket treat omission as false. Keep its distinction
/// from an explicit report for the signed-in player's availability view.
pub(crate) fn reported_map_pack(json: &[u8]) -> Option<bool> {
    #[derive(serde::Deserialize)]
    struct Report {
        has_map_pack: Option<bool>,
    }
    serde_json::from_slice::<Report>(json).ok()?.has_map_pack
}

enum Submission {
    Reply(Queue, Option<HostLease>),
    MapPackLost(Option<String>),
}

impl Matchmaker {
    pub(crate) fn report_map_pack(&mut self, xuid: Xuid, owns_pack: Option<bool>, clock: i64) {
        if clock >= self.next_capability_sweep {
            self.map_pack_reports.retain(|_, (_, until)| clock < *until);
            self.next_capability_sweep = clock + CAPABILITY_SWEEP_SECONDS;
        }
        if let Some(owns_pack) = owns_pack {
            self.map_pack_reports.insert(xuid, (owns_pack, clock + CAPABILITY_SECONDS));
        } else {
            self.map_pack_reports.remove(&xuid);
        }
    }

    fn map_pack_for(&self, xuid: Xuid, clock: i64) -> Option<bool> {
        self.map_pack_reports
            .get(&xuid)
            .and_then(|&(value, until)| (clock < until).then_some(value))
    }

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
            || (map_pack_map(&host.map) && !lease.assignment.has_map_pack)
            || lease.assignment.host_lease.as_ref().is_some_and(|id| id != &host.id)
        {
            return false;
        }
        lease.assignment.host_lease = Some(host.id.clone());
        true
    }

    fn booting_host(&self, xuid: Xuid, ticket: &Ticket) -> Option<(String, String)> {
        self.leases.values().find_map(|lease| {
            let assignment = &lease.assignment;
            if matches!(lease.placement, Placement::Booting { .. })
                && assignment.hopper == ticket.hopper
                // Keep the bound host ID for submit to withdraw on capability loss.
                && (!assignment.has_map_pack || ticket.has_map_pack)
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
    ) -> Result<Submission, ApiError> {
        self.expire(clock);
        if ticket.dedicated && !self.dedicated.contains_key(&xuid) {
            return Err(forbidden("a dedicated host must be registered by an operator"));
        }

        if let Some((id, lease)) = self.leases.iter().find(|(_, l)| {
            l.assignment.host == xuid || l.assignment.members.contains(&xuid)
        }) {
            if lease.assignment.hopper != ticket.hopper {
                return Err(conflict("account has an assignment on another hopper"));
            }
            if lease.assignment.has_map_pack && !ticket.has_map_pack {
                let id = id.clone();
                let host = lease.assignment.host_lease.clone();
                self.leases.remove(&id);
                return Ok(Submission::MapPackLost(host));
            }
            return Ok(Submission::Reply(lease.queue(xuid, &live.model), None));
        }

        let index = ticket.hopper as usize;
        let lineup = self.lineup.as_ref()
            .ok_or_else(|| not_found("hopper does not support the QoS queue"))?;
        let rules = lineup.rules.get(index).copied().flatten()
            .ok_or_else(|| not_found("hopper does not support the QoS queue"))?;
        let pack_only = lineup.map_pack_only[index];
        if pack_only && !ticket.has_map_pack {
            if let Some(bucket) = self.buckets.get_mut(&ticket.hopper) {
                bucket.remove(xuid);
            }
            return Err(forbidden("this hopper requires the Multiplayer Map Pack"));
        }
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
            rules.size <= MAX_DEDICATED_SOLO_PLAYERS
                && self.config.playlist(ticket.hopper).is_some_and(|playlist| {
                    !pack_only || map_pack_map(&playlist.map)
                })
        });
        let regions = (provider.is_some() && !ticket.dedicated).then_some(&self.config.regions);
        let level = live.model.read().ranks.standing(xuid, &ticket.hopper.to_string()).level;
        let bucket = self.buckets.entry(ticket.hopper).or_default();
        bucket.submit(&ticket, xuid, endpoint, level, clock, regions)?;
        let mut request = None;
        if let Some((lease, host_request)) =
            bucket.assign(ticket.hopper, rules, clock, &self.config, provider)?
        {
            let assigned = lease.assignment.members.contains(&xuid)
                || lease.assignment.host == xuid;
            let reply = assigned.then(|| lease.queue(xuid, &live.model));
            self.leases.insert(lease.assignment.id.clone(), lease);
            request = host_request;
            if let Some(reply) = reply {
                return Ok(Submission::Reply(reply, request));
            }
        }
        let reply = Queue::Probing {
            candidates: bucket.waiting.values()
                .filter(|w| w.candidate.xuid != xuid && (!ticket.dedicated || !w.dedicated))
                .map(|w| &w.candidate)
                .chain(regions.into_iter().flat_map(RegionTable::candidates))
                .take(MAX_CANDIDATES)
                .cloned()
                .collect(),
        };
        Ok(Submission::Reply(reply, request))
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
                has_map_pack: ticket.has_map_pack,
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
        entry.has_map_pack = ticket.has_map_pack;
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
        let playlist = config.playlist(hopper);
        let dedicated_uses_pack = playlist.is_some_and(|playlist| map_pack_map(&playlist.map));
        let hosts: Vec<_> = self.waiting.values()
            .filter_map(|w| w.host(dedicated_uses_pack))
            .collect();
        // Reserve a fallback that satisfies the normal peer QoS window. A
        // cohort that can only reach a registered host keeps the legacy path.
        let peers: Vec<_> = hosts.iter().copied().filter(|h| !h.dedicated).collect();
        let eligible = provider.zip(playlist);
        let peer_group = eligible.as_ref().and_then(|_| {
            let group = selection::group(&players, &peers, rules.size, clock)?;
            (!dedicated_uses_pack || selection::owns_map_pack(&group)).then_some(group)
        });
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
            has_map_pack: self.waiting[&host].has_map_pack && selection::owns_map_pack(&group),
        };
        let mut placement = Placement::PeerOrRegistered;
        let mut until = clock + ASSIGNMENT_SECONDS;
        let request = ad_hoc.map(|(provider, playlist)| {
            let choice = config.regions.pick(&group);
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
            host_request(&assignment, playlist, provider, region)
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
    if live.matchmaking.lock().unwrap().lineup.as_ref()
        .is_some_and(|lineup| lineup.fingerprint == fingerprint)
    {
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
    let map_pack_only = std::array::from_fn(|index| pack_only(&rules, index));
    let mut matcher = live.matchmaking.lock().unwrap();
    if matcher.lineup.as_ref().is_none_or(|previous| previous.fingerprint != key) {
        matcher.buckets.clear();
        matcher.lineup = Some(Lineup {
            fingerprint: key,
            rules: all,
            map_pack_only,
        });
    }
    Ok(())
}

pub(crate) async fn hopper_requires_map_pack(live: &Live, hopper: u32) -> Result<bool, ApiError> {
    load_rules(live).await?;
    Ok(live.matchmaking.lock().unwrap().lineup.as_ref().is_some_and(|lineup| {
        lineup.map_pack_only.get(hopper as usize).copied().unwrap_or(false)
    }))
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
    let map_pack_report = (!ticket.dedicated).then(|| reported_map_pack(&body));
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
    let submission = {
        let mut matcher = live.matchmaking.lock().unwrap();
        if let Some(report) = map_pack_report {
            matcher.report_map_pack(xuid, report, clock);
        }
        if let Some((assignment, lease_id)) = matcher.booting_host(xuid, &ticket) {
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
    let (mut result, request) = match submission {
        Submission::Reply(queue, request) => (queue, request),
        Submission::MapPackLost(host) => {
            if let Some(id) = host
                && let Err(error) = live.withdraw_pending_match_host(&id).await
            {
                eprintln!("live: withdrawing Map Pack host {id}: {error}");
            }
            return Err(forbidden("the assignment requires the Multiplayer Map Pack"));
        }
    };
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
                None => Queue::Probing { candidates: Vec::new() },
            };
        }
    }
    Ok(json(StatusCode::OK, &result))
}

pub(crate) async fn availability(
    State(live): State<Arc<Live>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let Author::Account { xuid, .. } = live.authenticate(bearer(&headers), lxm::READ)? else {
        return Err(forbidden("matchmaking availability needs an account"));
    };
    let has_map_pack = live.matchmaking.lock().unwrap().map_pack_for(xuid, now());
    let mut response = json(StatusCode::OK, &serde_json::json!({ "has_map_pack": has_map_pack }));
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("private, no-store"),
    );
    Ok(response)
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
