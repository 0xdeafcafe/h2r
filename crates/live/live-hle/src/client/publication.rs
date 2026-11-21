//! A machine logon has one session writer and no account identity.
use super::{Error, Identity, MAX_FILE, MatchRequest, Reply, Session, Worker, now};
use crate::status::{INVALID_ARGUMENT, MATCH_INVALID_SESSION, NO_SESSION, NO_USER};
use live_proto::{
    Author, Discovery,
    hosts::HostLease,
    lxm, session,
    sign::{Jws, KeyPair, PublicKey, TYP_TOKEN},
};
use serde_json::json;
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone)]
pub struct HostPublisher {
    lease: HostLease,
    subject: String,
    audience: String,
    issued_at: i64,
    expires: i64,
    token: String,
    key: Arc<KeyPair>,
}

impl std::fmt::Debug for HostPublisher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostPublisher")
            .field("lease", &self.lease.id)
            .finish_non_exhaustive()
    }
}

impl HostPublisher {
    /// Read the lease's event signing key. Discovery checks the signature and
    /// audience before the worker sends the bearer to the configured origin.
    pub fn new(lease: HostLease, token: String) -> Result<Self, crate::ConfigError> {
        #[derive(serde::Deserialize)]
        struct Claims {
            sub: String,
            aud: String,
            iss: String,
            iat: i64,
            exp: i64,
            assignment_id: String,
            event_key: String,
            lxm: Vec<String>,
        }
        let bad = |_| crate::ConfigError::HostCredential;
        let jwt = Jws::parse(&token).map_err(bad)?;
        let payload = jwt.json_payload(TYP_TOKEN).map_err(bad)?;
        let claims: Claims =
            serde_json::from_value(payload).map_err(|_| crate::ConfigError::HostCredential)?;
        if !crate::matchmaking::valid_host_lease(&lease.id)
            || claims.sub != format!("host:{}", lease.id)
            || claims.iss != Author::Server(claims.aud.clone()).to_string()
            || claims.assignment_id != lease.assignment_id
            || !claims.lxm.iter().any(|s| s == "sessions.publish")
        {
            return Err(crate::ConfigError::HostCredential);
        }
        let key = KeyPair::from_secret_b64(&claims.event_key).map_err(bad)?;
        Ok(Self {
            lease,
            subject: claims.sub,
            audience: claims.aud,
            issued_at: claims.iat,
            expires: claims.exp,
            token,
            key: Arc::new(key),
        })
    }

    pub(super) fn verify(&self, discovery: &Discovery) -> Result<(), Error> {
        let jwt = Jws::parse(&self.token).map_err(|_| Error::Protocol)?;
        let clock = now();
        if self.issued_at <= clock
            && clock < self.expires
            && self.audience == discovery.id
            && discovery
                .keys
                .iter()
                .any(|jwk| PublicKey::from_jwk(jwk).is_ok_and(|key| jwt.verify(&key).is_ok()))
        {
            Ok(())
        } else {
            Err(Error::Status(401))
        }
    }

    fn set_hopper(&self, attrs: &mut BTreeMap<u32, session::AttributeValue>) {
        // The playerless retail lobby opens in custom mode. Its playlist
        // attribute can be the custom-game sentinel on create and update.
        if let Some(hopper) = self.lease.hopper {
            attrs.insert(
                live_proto::optimatch::attr::HOPPER,
                session::AttributeValue::Integer(hopper.into()),
            );
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum Writer<'a> {
    Account(&'a Identity),
    Host(&'a HostPublisher),
}

impl<'a> Writer<'a> {
    pub(super) fn signer(self, worker: &Worker) -> (String, &'a KeyPair) {
        match self {
            Self::Account(user) => (worker.author(user), &user.key),
            Self::Host(host) => (host.subject.clone(), &host.key),
        }
    }

    pub(super) fn post(self, worker: &Worker, path: &str) -> ureq::Request {
        match self {
            Self::Account(user) => worker.request("POST", path, user, lxm::SESSIONS_WRITE),
            Self::Host(host) => {
                let api = &worker.connection.as_ref().expect("logged on").api;
                worker
                    .agent
                    .post(&format!("{api}{path}"))
                    .set("authorization", &format!("Bearer {}", host.token))
            }
        }
    }
}

impl Worker {
    pub(super) fn create_match(
        &self,
        writer: Writer<'_>,
        ad: &session::NewSession,
        assignment: Option<&str>,
    ) -> Result<session::Session, Error> {
        #[derive(serde::Deserialize)]
        struct Created {
            session: session::Session,
        }

        let build = || {
            let req = writer
                .post(self, "/sessions")
                .set("content-type", "application/json");
            match assignment {
                Some(id) => req.set(live_proto::matchmaking::MATCH_HEADER, id),
                None => req,
            }
        };
        let create = |ad: &session::NewSession| {
            let body = serde_json::to_string(ad).map_err(|_| Error::Protocol)?;
            // Only a lease has idempotent creation on the server.
            match writer {
                Writer::Host(_) => self.retried(build, Some(&body), MAX_FILE),
                Writer::Account(_) => self.response(build(), Some(&body), MAX_FILE),
            }
        };
        let bytes = match create(ad) {
            // The old strict payload decoder rejects unknown fields.
            Err(Error::Status(400)) if ad.browser.is_some() => {
                create(&session::NewSession { browser: None, ..ad.clone() })
            }
            result => result,
        }?;
        Ok(serde_json::from_slice::<Created>(&bytes)
            .map_err(|_| Error::Protocol)?
            .session)
    }

    pub(super) fn publish_host_match(
        &mut self,
        logon: &Session,
        request: &MatchRequest,
    ) -> Result<Reply, u32> {
        let host = self.publisher.as_ref().expect("host logon");
        let active = self
            .active
            .as_ref()
            .filter(|a| a.session.id == logon.id)
            .ok_or(NO_SESSION)?;
        let writer = Writer::Host(host);
        let published = |id: &str| {
            active
                .published
                .as_ref()
                .filter(|row| row.session_id == id)
                .ok_or(MATCH_INVALID_SESSION)
        };
        let row = match request {
            MatchRequest::Create { session: ad, .. } => {
                let mut ad = ad.clone();
                ad.map = Some(host.lease.map.clone());
                // Keep the title's probed package hash for Live's lease check.
                host.set_hopper(&mut ad.attributes);
                let assignment = host.lease.hopper.map(|_| host.lease.assignment_id.as_str());
                let row = self
                    .create_match(writer, &ad, assignment)
                    .map_err(Error::result)?;
                let assigned = match (&row.matchmaking, assignment) {
                    (None, None) => true,
                    (Some(a), Some(id)) => {
                        a.id == id && a.host_lease.as_ref() == Some(&host.lease.id)
                    }
                    _ => false,
                };
                if !assigned
                    || row.host != host.subject
                    || row.state != "open"
                    || row.xnkid != row.session_id
                    || !session::valid_id(&row.session_id)
                {
                    return Err(MATCH_INVALID_SESSION);
                }
                Some(row)
            }
            MatchRequest::Update { id, update } => {
                let mut row = published(id)?.clone();
                let mut update = update.clone();
                if let Some(attrs) = &mut update.attributes {
                    host.set_hopper(attrs);
                }
                row.update(&update).map_err(|_| INVALID_ARGUMENT)?;
                self.write_match_update(writer, id, &update)
                    .map_err(Error::result)?;
                // Player GETs require an account. The acknowledged update
                // uses the same projection as the server on its create reply.
                Some(row)
            }
            MatchRequest::Delete(id) => {
                published(id)?;
                self.write_match_event(writer, id, session::DELETED, json!({"reason":"closed"}))
                    .map_err(Error::result)?;
                None
            }
            MatchRequest::Players { id, joined, left } => {
                published(id)?;
                self.match_players(writer, id, joined, left)
                    .map_err(Error::result)?;
                return Ok(Reply::Matches(Vec::new()));
            }
            _ => return Err(NO_USER),
        };
        self.active.as_mut().expect("logged on").published = row.clone();
        Ok(Reply::Matches(row.into_iter().collect()))
    }
}
