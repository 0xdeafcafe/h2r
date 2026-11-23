//! A shared Forge session (a synchronous network game) over the agent API
//! (docs/runtime/agent-api.md, "Shared Forge session").
//!
//! The methods are the local session's: `forge.status`, `forge.log`,
//! `forge.doc`, `forge.things` and `forge.propose`. The game's
//! [`LocalApi`](super::local::LocalApi) answers them from its node when the
//! session is shared; this module has what that takes that is not the
//! game's: the status a node adds, and the proposals waiting for the host.
use std::collections::BTreeMap;
use std::sync::mpsc;

use ::forge::net::{Report, Settled};
use agent_proto::forge::SHARED_SESSION;
use forge_doc::Entry;
use serde_json::{Value, json};

use super::local::ProposalReply;

/// How many game ticks `forge.propose` waits on a shared session: the
/// proposal goes to the host and back, and the entry applies at the tick
/// the host stamped it for.
pub const WAIT_TICKS: u64 = 240;

/// `status` (a local session's reply) as a shared session's: `session` is
/// `"shared"`, `role` is the seat's, and `net` holds the node's report
/// (`forge::net::Report`): `host`, `joined`, `seat`, `name`, `members`,
/// `holds`, `head`, `n`, `through`, `in_flight` and `ended`.
pub fn status(mut status: Value, report: &Report) -> Value {
    status["session"] = json!(SHARED_SESSION);
    if let Some(role) = report.role {
        status["role"] = json!(role.name());
    }
    status["net"] = serde_json::to_value(report).expect("a report serialises");
    status
}

/// `forge.status` before the session has anything but a node: no tool, no
/// screens.
pub fn bare_status(report: &Report) -> Value {
    status(json!({}), report)
}

/// The proposals this machine has sent and the host has not settled. Each
/// `forge.propose` waits on the receiver [`Replies::wait_for`] gives it.
#[derive(Default)]
pub struct Replies {
    waiting: BTreeMap<u32, mpsc::Sender<Result<Entry, String>>>,
}

impl Replies {
    pub fn wait_for(&mut self, nonce: u32) -> ProposalReply {
        let (send, recv) = mpsc::channel();
        self.waiting.insert(nonce, send);
        recv
    }

    /// Answer the proposals the node settled: the entry the stream
    /// carried, or the host's refusal, led by its name.
    pub fn settle(&mut self, settled: Vec<Settled>) {
        for Settled { nonce, result } in settled {
            let Some(send) = self.waiting.remove(&nonce) else {
                continue;
            };
            let _ = send.send(match result {
                Ok(Some(entry)) => Ok(entry),
                Ok(None) => Err("forge_refused: a hold is not an edit".into()),
                Err(sentence) => Err(sentence),
            });
        }
    }

    /// The session ended: every proposal waiting is refused with why.
    pub fn end(&mut self, why: &str) {
        for send in std::mem::take(&mut self.waiting).into_values() {
            let _ = send.send(Err(format!("forge_not_joined: {why}")));
        }
    }

    pub fn len(&self) -> usize {
        self.waiting.len()
    }

    pub fn is_empty(&self) -> bool {
        self.waiting.is_empty()
    }
}
