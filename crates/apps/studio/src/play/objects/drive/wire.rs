//! The worker may still be waiting for an unrelated call. A document edit stays
//! revocable until the worker accepts it at the socket boundary.
use super::*;
use crate::play::link::{Call, Purpose};

impl State {
    pub(in crate::play) fn cancel_unsent(&mut self) {
        self.flight.retain(|_, flight| !flight.dispatch.withdraw());
        self.requested.clear();
        self.dirty = true;
    }

    pub(in crate::play) fn skipped(&mut self, ticket: u64, keep_manual: bool) {
        self.flight.retain(|key, flight| {
            if flight.ticket != ticket {
                return true;
            }
            if keep_manual && flight.manual {
                self.requested.insert(*key);
            }
            false
        });
        self.dirty = true;
    }

    pub(in crate::play) fn transport_lost(&mut self) {
        self.placements.transport_lost();
        for (key, flight) in std::mem::take(&mut self.flight) {
            if flight.dispatch.withdraw() {
                continue;
            }
            if let Op::Set { id, .. } = flight.change.op
                && let Some(at) = flight.dispatch.dispatched_at()
            {
                self.last_move.insert(id, at.max(flight.offered_at));
            }
            let why = "the connection closed before the game acknowledged this object edit".into();
            self.failed.insert(key, Failure::unknown(flight.change, flight.manual, why));
        }
        self.requested.clear();
        self.release.clear();
        self.drag_moves.clear();
        self.dragging = false;
        self.dirty = true;
    }
}

impl Play {
    pub(in crate::play) fn revalidate_object_dispatch(&mut self, app: &App) {
        if self.objects.flight.is_empty() {
            return;
        }
        if self.object_scope(app) != self.objects.scope || self.object_gate(app).is_err() {
            self.objects.cancel_unsent();
            return;
        }
        let revision = app.session.markers_rev();
        let peers = self.edits.confirmed;
        let stale = self.objects.flight.values().find_map(|flight| {
            let method = match flight.change.op {
                Op::Set { .. } => method::OBJECT_SET,
                Op::Delete { .. } => method::OBJECT_DELETE,
                Op::Respawn { .. } => method::OBJECT_RESPAWN,
                Op::Reload(_) => return None,
            };
            let changed = flight.markers != revision
                || flight.peers != peers
                || !self.has_method(method);
            (changed && flight.dispatch.withdraw()).then_some(flight.ticket)
        });
        if let Some(ticket) = stale {
            self.objects.skipped(ticket, true);
        }
    }

    pub(super) fn send_object(
        &mut self,
        change: Change,
        manual: bool,
        app: &App,
        now: Instant,
    ) -> bool {
        let Some(scope) = self.object_scope(app) else { return false };
        if self.objects.scope != Some(scope) || !self.objects.flight.is_empty()
            || self.object_gate(app).is_err()
        {
            return false;
        }
        let (Some(link), Some(stream)) = (&self.link, &app.pairing.mirror) else {
            return false;
        };
        let peers = self.edits.confirmed;
        let mut empty_slot = None;
        let (method, params) = match change.op {
            Op::Set { id, frame } => (
                method::OBJECT_SET,
                json!({"id":id.to_string(), "pos":frame.pos, "forward":frame.forward,
                    "up":frame.up, "epoch":scope.epoch, "confirm_peers":peers}),
            ),
            Op::Delete { id } => (
                method::OBJECT_DELETE,
                json!({"id":id.to_string(), "epoch":scope.epoch, "confirm_peers":peers}),
            ),
            Op::Respawn { id: Some(id) } => (
                method::OBJECT_RESPAWN,
                json!({"id":id.to_string(), "epoch":scope.epoch, "confirm_peers":peers}),
            ),
            Op::Respawn { id: None } => {
                let kind = scenario_edit::layout::block(change.key.block).object_type;
                let (Some(kind), Ok(slot)) =
                    (kind.and_then(|k| u8::try_from(k).ok()), u16::try_from(change.index))
                else { return false };
                empty_slot = Some((kind, slot));
                (method::OBJECT_RESPAWN, json!({
                    "placement":{"block":crate::api::block_name(change.key.block),"index":slot},
                    "epoch":scope.epoch,
                    "confirm_peers":peers,
                }))
            }
            Op::Reload(_) => return false,
        };
        if !self.has_method(method)
            || (matches!(change.op, Op::Respawn { .. }) && self.edits.sent.is_some())
        {
            return false;
        }
        self.objects.next_ticket += 1;
        let ticket = self.objects.next_ticket;
        // The worker checks the subscription epoch before I/O. The host checks
        // `epoch` again when the edit runs, which covers a map change in between.
        let call = Call { method: method.into(), params, purpose: Purpose::Object(ticket) };
        let dispatch = Arc::new(ObjectDispatch::new(call, stream, scope.epoch, empty_slot));
        if !link.offer_object(ticket, &dispatch) {
            return false;
        }
        let creation = if matches!(change.op, Op::Respawn { .. }) {
            self.baseline.as_ref().map(|(_, base)| base.creation_fields(change.key.block, change.index))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        self.objects.flight.insert(change.key, Flight {
            ticket,
            change,
            manual,
            creation,
            dispatch,
            offered_at: now,
            markers: app.session.markers_rev(),
            peers,
        });
        true
    }
}
