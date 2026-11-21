//! Keep the outcome of an object RPC with the placement and salted handle it
//! addressed. GONE can mean a weapon was picked up while the call was queued.
use super::*;
use agent_proto::{RpcError, code};
use crate::play::sync::Baseline;

#[derive(Clone, Copy)]
enum Reason {
    Refused,
    Permission,
    Gone { id: ObjectId, since: u64, was_attached: bool },
    Partial { deleted: ObjectId, since: u64, empty: bool },
    Unknown,
}

pub(super) struct Failure {
    pub(super) change: Change,
    pub(super) manual: bool,
    pub(super) why: String,
    reason: Reason,
    creation: Option<Vec<u8>>,
}

impl Failure {
    pub(super) fn unknown(change: Change, manual: bool, why: String) -> Self {
        Self { change, manual, why, reason: Reason::Unknown, creation: None }
    }

    pub(super) fn matches_creation(&self, base: &Baseline) -> bool {
        self.creation.as_ref().is_none_or(|bytes| {
            base.creation_matches(self.change.key.block, self.change.index, bytes)
        })
    }

    pub(super) fn is_unknown(&self) -> bool {
        matches!(self.reason, Reason::Unknown)
    }

    pub(super) fn is_partial(&self) -> bool {
        matches!(self.reason, Reason::Partial { .. })
    }

    pub(super) fn is_permission(&self) -> bool {
        matches!(self.reason, Reason::Permission)
    }

    pub(super) fn awaiting_sample(&self) -> Option<u64> {
        match self.reason {
            Reason::Gone { since, .. } | Reason::Partial { since, empty: false, .. } => Some(since),
            _ => None,
        }
    }

    /// Reassert undo of a move whose reply was lost, even back to the file's
    /// frame. A changed salted occupant can also need that absolute frame.
    pub(super) fn force_move(&self, frame: Option<Frame>) -> bool {
        self.is_unknown() && frame.is_some() && matches!(self.change.op, Op::Set { .. })
    }

    pub(super) fn keep_without_diff(&self, dirty_creation: bool, deleted_in_doc: bool) -> bool {
        match self.reason {
            Reason::Unknown | Reason::Gone { .. } => true,
            Reason::Partial { empty, .. } => !empty || !deleted_in_doc,
            Reason::Refused | Reason::Permission => self.manual && dirty_creation,
        }
    }

    pub(super) fn blocks(&self, change: Change) -> bool {
        match self.reason {
            // Acknowledging an absolute move is per handle. A new salt or
            // another intended frame can be safely addressed on its own.
            Reason::Unknown => match (self.change.op, change.op) {
                (Op::Set { id: a, frame: f }, Op::Set { id: b, frame: g }) => a == b && f == g,
                (Op::Set { .. }, Op::Delete { .. }) => false,
                _ => true,
            },
            Reason::Gone { id, .. } => match change.op {
                Op::Set { id: target, .. } | Op::Delete { id: target }
                | Op::Respawn { id: Some(target) } => target == id,
                Op::Respawn { id: None } => true,
                Op::Reload(_) => false,
            },
            Reason::Partial { .. } | Reason::Permission => true,
            Reason::Refused => self.change.op == change.op,
        }
    }

    pub(super) fn allows_manual(&self, change: Change, sample: &SampleIndex) -> bool {
        match self.reason {
            Reason::Partial { deleted, since, .. } => {
                change.op == (Op::Respawn { id: None })
                    && sample.tick > since
                    && !sample.contains(deleted)
                    && sample.occupant(change.key.block, change.index).is_none()
            }
            Reason::Permission | Reason::Unknown => false,
            Reason::Gone { .. } => !self.blocks(change),
            Reason::Refused => true,
        }
    }
}

impl State {
    pub(super) fn sync_preview(&self, app: &mut App, key: Key) {
        if let Some(scope) = self.scope {
            let preview = self.ack.get(&key).and_then(Ack::preview)
                .map(|(id, index)| (id.key, index));
            app.pairing.previews.set(scope.document, scope.epoch, scope.checksum, key, preview);
        }
    }

    pub(in crate::play) fn confirm_objects(&mut self) {
        self.failed.retain(|key, failure| {
            if failure.is_permission() && failure.manual {
                self.requested.insert(*key);
            }
            !failure.is_permission()
        });
        self.dirty = true;
    }

    /// Resolve GONE using a newer live sample. An attachment can temporarily
    /// make the exact same salted handle uneditable without deleting it.
    pub(super) fn reconcile_failures(&mut self, app: &mut App) {
        let mut deleted = Vec::new();
        self.failed.retain(|&key, failure| {
            match &mut failure.reason {
                Reason::Gone { id, since, was_attached } if self.sample.tick > *since => {
                    if self.sample.contains(*id) {
                        if self.sample.attached(*id) {
                            *was_attached = true;
                        } else if *was_attached {
                            return false;
                        }
                        // Undo while the object is still present requires no respawn.
                        return !(matches!(failure.change.op, Op::Delete { .. }) && self.doc.contains_key(&key));
                    }
                    if matches!(failure.change.op, Op::Delete { .. })
                        || self.ack.get(&key).and_then(Ack::preview)
                            .is_some_and(|(preview, _)| preview == *id)
                    {
                        deleted.push((key, *id));
                    }
                    return false;
                }
                Reason::Partial { deleted, since, empty } if self.sample.tick > *since => {
                    *empty = !self.sample.contains(*deleted)
                        && self.sample.occupant(key.block, failure.change.index).is_none();
                }
                _ => {}
            }
            true
        });
        for (key, id) in deleted {
            self.ack.insert(key, Ack::Delete { id });
            self.sync_preview(app, key);
        }
    }
}

fn acknowledged(
    change: Change,
    ack: Option<&Ack>,
    loaded_slot: Option<usize>,
    reply: &Value,
    tick: u64,
) -> Option<Ack> {
    let object = |field: &str| reply[field].as_str().and_then(ObjectId::parse);
    match change.op {
        Op::Set { id, frame } if object("object") == Some(id) => {
            let binding = ack.and_then(Ack::binding).filter(|(saved, _, _)| *saved == id);
            Some(Ack::Set {
                id,
                frame,
                tick,
                linked: binding.is_none_or(|(_, linked, _)| linked),
                index: binding.map_or_else(|| loaded_slot.unwrap_or(change.index), |(_, _, slot)| slot),
            })
        }
        Op::Delete { id } if object("deleted") == Some(id) => Some(Ack::Delete { id }),
        Op::Respawn { .. } => Some(Ack::Spawn {
            id: object("object")?,
            linked: reply["linked"].as_bool()?,
            tick,
            index: change.index,
        }),
        _ => None,
    }
}

impl Play {
    pub(in crate::play) fn object_answer(
        &mut self,
        app: &mut App,
        ticket: u64,
        result: Result<Value, RpcError>,
    ) {
        let Some(scope) = self.objects.scope else { return };
        if self.object_scope(app) != Some(scope) {
            return;
        }
        let Some(key) = self.objects.flight.iter()
            .find_map(|(&key, f)| (f.ticket == ticket).then_some(key))
        else {
            return;
        };
        let flight = self.objects.flight.remove(&key).unwrap();
        self.objects.dirty = true;

        let latest = app.pairing.mirror.as_ref().and_then(|stream| stream.snapshot())
            .and_then(|captures| captures.latest_header()).map(|(_, header)| header);
        if latest.is_some_and(|header| header.epoch != scope.epoch) {
            return;
        }
        let change = flight.change;
        if let Op::Set { id, .. } = change.op
            && let Some(at) = flight.dispatch.dispatched_at()
        {
            self.objects.last_move.insert(id, at.max(flight.offered_at));
        }
        // The displayed sample may lag the newest capture. Reconciliation
        // requires a sample newer than any already received at this reply.
        let sample_tick = latest.map(|header| header.tick)
            .or_else(|| app.pairing.shown.as_ref().map(|sample| sample.tick))
            .unwrap_or(0);
        let (reason, why) = match result {
            Ok(v) => {
                let tick = v["at"]["tick"].as_u64().unwrap_or(sample_tick);
                let loaded_slot = self.baseline.as_ref()
                    .and_then(|(_, base)| base.object_rows().get(&key))
                    .map(|row| row.index);
                if let Some(ack) = acknowledged(change, self.objects.ack.get(&key), loaded_slot, &v, tick) {
                    self.objects.ack.insert(key, ack);
                    // Moving the old body does not apply its refused creation fields.
                    let keep_creation_failure = matches!(change.op, Op::Set { .. })
                        && self.objects.failed.get(&key).is_some_and(|f| f.manual);
                    if !keep_creation_failure {
                        self.objects.failed.remove(&key);
                    }
                    self.objects.sync_preview(app, key);
                    if matches!(change.op, Op::Respawn { .. })
                        && let Some((_, base)) = &mut self.baseline
                    {
                        base.object_created(key.block, change.index, &flight.creation);
                    }
                    return;
                }
                (Reason::Unknown, "the game returned an unreadable object edit acknowledgement".into())
            }
            Err(e) => {
                let why = e.data.as_ref().and_then(|d| d["why"].as_str())
                    .unwrap_or(&e.message).to_owned();
                let target = match change.op {
                    Op::Set { id, .. } | Op::Delete { id } => Some(id),
                    Op::Respawn { id } => id,
                    Op::Reload(_) => None,
                };
                let reason = if let Op::Respawn { id } = change.op
                    && let Some(deleted) = e.data.as_ref()
                        .and_then(|d| d["deleted"].as_str()).and_then(ObjectId::parse)
                    && id.is_none_or(|expected| expected == deleted)
                {
                    // Replacement can fail after the host successfully deleted
                    // the old object. Require an observed empty slot for retry.
                    self.objects.ack.insert(key, Ack::Delete { id: deleted });
                    self.objects.sync_preview(app, key);
                    Reason::Partial { deleted, since: sample_tick, empty: false }
                } else if e.code == code::GONE && let Some(id) = target {
                    Reason::Gone { id, since: sample_tick, was_attached: self.objects.sample.attached(id) }
                } else if matches!(e.code, code::INVALID_REQUEST | agent_proto::sim::REFUSED)
                    && why.contains("peer count")
                {
                    if self.edits.confirmed == flight.peers {
                        self.edits.confirmed = None;
                    }
                    Reason::Permission
                } else if e.code == code::TIMEOUT {
                    Reason::Unknown
                } else {
                    // An RPC refusal with no deleted receipt did not replace
                    // the object. INVALID_REQUEST includes the pre-queue gate.
                    Reason::Refused
                };
                (reason, why)
            }
        };
        // A stale or held handle on delete needs a new observation. An absent
        // object already satisfies the document, so avoid an error toast.
        if !(matches!(reason, Reason::Gone { .. }) && matches!(change.op, Op::Delete { .. })) {
            self.feedback.problem(app, format!("live object edit: {why}"));
        }
        let creation = (flight.manual && matches!(reason, Reason::Refused | Reason::Permission))
            .then_some(flight.creation);
        self.objects.failed.insert(key, Failure {
            change, manual: flight.manual, why, reason, creation,
        });
    }
}
