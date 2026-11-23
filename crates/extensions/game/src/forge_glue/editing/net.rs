//! The editor in a shared session (docs/game/forge-design.md, F5): its
//! edits go to the host as proposals, and what the host stamps is applied
//! to the session and the world at the tick it names, on every machine.
use agent::forge::net::{self as agent_net, Replies};
use forge::net::{APPLY_DELAY, HOST_SEAT, Lane, Node, Stamped, State, film};
use forge::{Session, SessionAdmit};
use forge_doc::{Entry, Op, Sha256};
use serde_json::Value;
use std::sync::{Arc, mpsc};

use super::{After, Editor};
use crate::forge_glue::net::{self as glue, Offer};

type Wire = Box<dyn Lane + Send>;

/// A shared session's network half.
pub(super) struct Net {
    node: Node<SessionAdmit, Wire>,
    replies: Replies,
    /// Items the node released that the session has yet to apply.
    backlog: Vec<Stamped>,
    /// The state the welcome gave, until the session takes it as its own.
    joined: Option<Box<State>>,
    /// The node ended the session.
    dead: bool,
}

impl Editor {
    /// Make this session a seat of the shared session the link offered:
    /// the host's, or a guest's. `name` is this machine's author name.
    pub fn open_shared(&mut self, offer: Offer, tick: u32, name: &str) -> Result<(), String> {
        let node = match offer {
            Offer::Host(lanes) => {
                let session = (*self.session).clone().share(HOST_SEAT, name);
                let mut node = Node::host(
                    session.document().clone(),
                    session.limits().clone(),
                    session.admit(),
                    name,
                    tick,
                    APPLY_DELAY,
                )
                .map_err(|e| e.to_string())?;
                for (seat, lane) in lanes {
                    node.accept(seat, lane, forge::net::Role::Editor)
                        .map_err(|e| e.to_string())?;
                }
                self.session = Arc::new(session);
                node
            }
            Offer::Guest(lane) => {
                Node::guest(self.session.limits().clone(), name, lane).map_err(|e| e.to_string())?
            }
        };
        self.net = Some(Net {
            node,
            replies: Replies::default(),
            backlog: Vec::new(),
            joined: None,
            dead: false,
        });
        Ok(())
    }

    pub fn is_shared(&self) -> bool {
        self.net.is_some()
    }

    /// The agent API's status, as a shared session's when it is one.
    pub(super) fn shared_status(&self, status: Value) -> Value {
        match &self.net {
            Some(net) => agent_net::status(status, &net.node.report()),
            None => status,
        }
    }

    pub fn log_since(&self, since: u32) -> Vec<Entry> {
        match &self.net {
            Some(net) => net.node.log_since(since),
            None => self
                .session
                .log()
                .iter()
                .filter(|entry| entry.seq > since)
                .cloned()
                .collect(),
        }
    }

    /// `forge.propose` on a shared session: sent to the host, answered
    /// when the stream carries the entry or the host refuses it.
    pub(super) fn propose_shared(
        &mut self,
        op: Op,
        was: Option<Sha256>,
    ) -> Option<Result<mpsc::Receiver<Result<Entry, String>>, String>> {
        let net = self.net.as_mut()?;
        if net.dead {
            return Some(Err(
                net.node
                    .report()
                    .ended
                    .map_or_else(|| "forge_not_joined: the session ended".into(), |why| {
                        format!("forge_not_joined: {why}")
                    }),
            ));
        }
        Some(
            net.node
                .propose(op, was)
                .map_err(|e| e.named())
                .map(|nonce| net.replies.wait_for(nonce)),
        )
    }

    /// One game tick of the network half: send what the editor proposed,
    /// run the node, and apply what it released. Called before the
    /// editor's own work.
    pub(super) fn net_tick(&mut self, tick: u32) {
        let Some(net) = self.net.as_mut() else { return };
        if net.dead {
            return;
        }
        let outgoing = Arc::make_mut(&mut self.session).take_outgoing();
        if !outgoing.is_empty()
            && let Err(e) = net.node.submit(outgoing)
        {
            self.notice = Some((e.to_string(), tick));
        }
        match net.node.tick(tick) {
            Ok(step) => {
                if let Some(welcome) = net.node.take_welcome()
                    && let Ok(payload) = film::welcome_payload(&welcome)
                {
                    glue::record(payload);
                }
                if let Some(state) = step.joined {
                    net.joined = Some(state);
                }
                net.backlog.extend(step.items);
            }
            Err(e) => {
                net.dead = true;
                net.replies.end(&e.to_string());
                self.notice = Some((format!("The Forge session ended: {e}"), tick));
                return;
            }
        }
        net.replies.settle(net.node.take_settled());
        if self.busy() || (net.joined.is_none() && net.backlog.is_empty()) {
            return;
        }
        let joined = net.joined.take();
        let items = std::mem::take(&mut net.backlog);
        let seat = net.node.seat();
        let name = net.node.name().to_owned();
        let staged = items.clone();
        let result = self.stage(After::Shared(items), move |s: &mut Session, w| {
            if let Some(state) = &joined {
                s.sync_to(w, state)?;
                if let Some(seat) = seat {
                    *s = s.clone().share(seat, &name);
                }
            }
            for item in &staged {
                s.apply_item(w, item)?;
            }
            Ok(())
        });
        if let Err(reason) = result {
            self.notice = Some((reason, tick));
        }
    }

    /// The staged items applied: they are the film's records.
    pub(super) fn shared_applied(&mut self, items: &[Stamped]) {
        for item in items {
            if let Ok(Some(payload)) = film::item_payload(item) {
                glue::record(payload);
            }
        }
    }
}
