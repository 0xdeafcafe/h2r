//! Keep the guest's async search pending while Live gathers real QoS echoes.

use crate::{
    client::{MatchRequest, Pending, Reply, Service, Session},
    status::{CANNOT_ACCESS_SERVICE, INVALID_ARGUMENT},
};
use live_proto::{
    Xuid,
    matchmaking::{Assignment, Connectivity, Nat, Sample, Ticket},
    optimatch::Query,
    session,
};
use std::{
    collections::BTreeMap,
    sync::OnceLock,
    time::{Duration, Instant},
};

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
    Join(Vec<session::Session>),
}

#[derive(Clone)]
pub(crate) enum Step {
    Retry(Vec<Sample>),
    Complete(Outcome),
}

pub(crate) struct Search {
    ticket: Ticket,
    legacy: MatchRequest,
    service: Service,
    connection: Session,
    pending: Option<Pending>,
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
        let pending = service.match_request(connection, MatchRequest::Queue(ticket.clone()))?;
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
            after: now,
            deadline: now + Duration::from_secs(100),
        })
    }

    pub fn poll(&mut self) -> Result<Option<Outcome>, u32> {
        if let Some(pending) = &self.pending {
            let Some(reply) = pending.take() else {
                return Ok(None);
            };
            self.pending = None;
            match reply? {
                Reply::MatchmakingUnsupported => {
                    self.pending = Some(
                        self.service.match_request(&self.connection, self.legacy.clone())?,
                    );
                }
                Reply::Matches(rows) => return Ok(Some(Outcome::Join(rows))),
                Reply::Matchmaking(step) => match *step {
                    Step::Retry(samples) => {
                        self.ticket.samples = samples;
                        self.after = Instant::now() + Duration::from_secs(1);
                    }
                    Step::Complete(outcome) => return Ok(Some(outcome)),
                },
                _ => return Err(INVALID_ARGUMENT),
            }
        } else if Instant::now() >= self.after {
            if Instant::now() >= self.deadline {
                return Err(CANNOT_ACCESS_SERVICE);
            }
            self.pending = Some(self.service.match_request(
                &self.connection,
                MatchRequest::Queue(self.ticket.clone()),
            )?);
        }
        Ok(None)
    }
}

pub(crate) fn solo_hopper(query: Query, users: &[u64; 4]) -> Option<u32> {
    let hopper = u32::try_from(query.hopper).ok()?;
    (query.party == Some(1)
        && (hopper as usize) < live_proto::hopper::HOPPERS
        && users.iter().filter(|&&user| user != 0).count() == 1)
        .then_some(hopper)
}

pub(crate) fn valid_probe_targets(candidates: &[live_proto::matchmaking::Candidate], me: Xuid) -> bool {
    use std::collections::BTreeSet;
    let mut seen = BTreeSet::new();
    candidates.len() <= live_proto::matchmaking::MAX_CANDIDATES
        && candidates.iter().all(|v| {
            v.xuid != me
                && seen.insert(v.xuid)
                && session::valid_id(&v.generation)
                && (1..=65529).contains(&v.endpoint.port())
                && !v.endpoint.ip().is_unspecified()
                && !v.endpoint.ip().is_multicast()
                && !v.endpoint.ip().is_broadcast()
        })
}
