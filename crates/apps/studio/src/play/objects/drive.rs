//! Drive object edits on the control connection. Replies acknowledge the exact
//! frame or slot requested, even if the document changes while the call runs.
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use agent_proto::{ObjectId, RpcError, code, sim::method};
use serde_json::{Value, json};

use super::{Ack, Change, Frame, Key, Op, Rows, diff, document, respawn};
use crate::app::App;
use crate::play::{Play, PUSH_EVERY, link::Purpose, sync, tag_sync};

#[derive(Clone, Copy, PartialEq)]
struct Scope {
    pairing: u64,
    document: u64,
    epoch: u64,
    checksum: u32,
}

struct Flight {
    ticket: u64,
    change: Change,
    frame: Option<Frame>,
    creation: Option<sync::Write>,
}

struct Failure {
    change: Change,
    why: String,
    // A manual Respawn has no automatic diff to attach its refusal to.
    manual: bool,
    gone_preview: bool,
    // A timeout or a closed connection can follow a committed game edit.
    unknown: bool,
}

#[derive(Default)]
pub(in crate::play) struct State {
    scope: Option<Scope>,
    ack: BTreeMap<Key, Ack>,
    flight: BTreeMap<Key, Flight>,
    failed: BTreeMap<Key, Failure>,
    doc_revision: Option<u64>,
    doc: Rows,
    next_ticket: u64,
    next_push: Option<Instant>,
    dirty: bool,
    dragging: bool,
    drag_moves: BTreeSet<Key>,
    last_move: BTreeMap<Key, Instant>,
    release: BTreeSet<Key>,
    rows: Vec<Value>,
    can_respawn: bool,
    pub(in crate::play) waiting_for_sample: Option<&'static str>,
}

impl State {
    pub(in crate::play) fn transport_lost(&mut self) {
        for (key, flight) in std::mem::take(&mut self.flight) {
            self.failed.insert(
                key,
                Failure {
                    change: flight.change,
                    why: "the connection closed before the game acknowledged this object edit".into(),
                    manual: flight.creation.is_some(),
                    gone_preview: false,
                    unknown: true,
                },
            );
        }
        self.release.clear();
        self.drag_moves.clear();
        self.dragging = false;
        self.dirty = true;
    }

    pub(in crate::play) fn field_answer(&mut self) {
        // A released gizmo can send its final object frame as soon as the
        // conditional field batch succeeds, including inside the interval.
        self.dirty = true;
    }

    pub(in crate::play) fn can_respawn(&self) -> bool {
        self.can_respawn
    }

    fn hide_rows(&mut self, pending: &mut Vec<Value>) {
        self.can_respawn = false;
        self.waiting_for_sample = None;
        if !self.rows.is_empty() {
            self.rows.clear();
            self.publish(pending);
        }
    }

    pub(in crate::play) fn publish(&self, pending: &mut Vec<Value>) {
        pending.retain(|row| row["doc"] != "object");
        for row in &mut *pending {
            if row["object_note"] == true {
                row["why"] = json!("placement layout changed");
                row.as_object_mut().unwrap().remove("object_note");
            }
        }
        for row in &self.rows {
            // Keep one Reload row for a structural block. Attach the reason
            // a replacement cannot be previewed to that row.
            if row["layout"] == true
                && let Some(block) = pending.iter_mut().find(|p| {
                    p["doc"] == "scenario" && p["class"] == tag_sync::RELOAD
                        && p["path"] == row["block"]
                })
            {
                block["why"] = row["why"].clone();
                block["object_note"] = json!(true);
            } else {
                pending.push(row.clone());
            }
        }
    }
}

// A gone salted preview can be recreated for the next document change.
// A different occupant of the same object slot is never its replacement.
fn forget_gone_preview(ack: &mut BTreeMap<Key, Ack>, key: Key, op: Op) -> bool {
    if let Op::Set { id, .. } = op
        && ack.get(&key)
            .and_then(Ack::binding)
            .is_some_and(|(saved, linked, _)| saved == id && !linked)
    {
        ack.insert(key, Ack::Delete { id });
        return true;
    }
    false
}

fn row(change: Change, doc: &Rows, why: &str, refused: bool) -> Value {
    let block = change.key.block;
    let name = crate::api::block_name(block);
    let current = doc.get(&change.key);
    let index = current.map_or(change.index, |row| row.index);
    let target = current.map_or_else(
        || json!({"panel":"pending"}),
        |_| json!({"block":name, "index":index}),
    );
    let structural = matches!(change.op, Op::Reload(_) | Op::Respawn | Op::Delete { .. });
    json!({
        "doc": "object",
        "block": format!("{block:?}"),
        "path": format!("{name}[{index}]"),
        "target": target,
        "class": if structural { tag_sync::RELOAD } else { tag_sync::CREATION },
        "why": why,
        "refused": refused,
        "layout": matches!(change.op, Op::Reload(_)),
    })
}

fn sample_refusal(app: &App) -> Option<&'static str> {
    match &app.pairing.shown {
        None => Some("Object edits are waiting for the game's live objects."),
        Some(s) if s.stale || !app.pairing.matches_sample(app, s) => {
            Some("Object edits are waiting for a fresh frame from this map.")
        }
        _ => None,
    }
}

impl Play {
    fn object_scope(&self, app: &App) -> Option<Scope> {
        Some(Scope {
            pairing: self.pairing_id,
            document: app.session.world_rev(),
            epoch: self.edits.epoch?,
            checksum: self.edits.checksum?,
        })
    }

    fn object_gate(&self, app: &App) -> Result<(), String> {
        if !self.live_edits {
            return Err("View mode".into());
        }
        if self.reload_copy.is_some() {
            return Err("waiting for the play copy to load".into());
        }
        if let Some(why) = self.map_refusal(app).or_else(|| self.session_refusal(app)) {
            return Err(why);
        }
        self.require_method(agent_proto::live::SUBSCRIBE)?;
        if let Some(why) = sample_refusal(app) {
            return Err(why.into());
        }
        Ok(())
    }

    pub(in crate::play) fn update_objects(&mut self, app: &mut App, now: Instant) {
        let scope = self.object_scope(app);
        if scope.is_some() && self.objects.scope != scope {
            // A delayed reply from the previous epoch still carries its ticket.
            self.objects = State {
                scope,
                next_ticket: self.objects.next_ticket,
                ..Default::default()
            };
            self.objects.publish(&mut app.pairing.pending);
        }
        if !self.is_paired() || !self.live_edits || self.reload_copy.is_some() {
            self.objects.hide_rows(&mut app.pairing.pending);
            self.objects.release.clear();
            self.objects.drag_moves.clear();
            self.objects.dragging = false;
            return;
        }

        let dragging = app.session.in_gesture() || app.session.dragging();
        let released = self.objects.dragging && !dragging;
        self.objects.dragging = dragging;
        if released {
            self.objects.release.append(&mut self.objects.drag_moves);
            self.push_edits(app);
        }
        let due = self.objects.next_push.is_none_or(|next| now >= next);
        if !due && !released && !self.objects.dirty {
            return;
        }
        if due {
            self.objects.next_push = Some(now + PUSH_EVERY);
        }
        self.objects.dirty = false;
        if self.object_gate(app).is_err() {
            self.objects.hide_rows(&mut app.pairing.pending);
            if self.has_method(agent_proto::live::SUBSCRIBE) {
                self.objects.waiting_for_sample = sample_refusal(app);
            }
            return;
        }
        self.objects.waiting_for_sample = None;
        let Some((_, base)) = &self.baseline else {
            return;
        };
        let Some(scenario) = app.session.scenario() else {
            return;
        };
        let revision = app.session.markers_rev();
        if self.objects.doc_revision != Some(revision) {
            self.objects.doc = document(scenario);
            self.objects.doc_revision = Some(revision);
        }
        let doc = &self.objects.doc;
        let sample = app.pairing.shown.as_ref().unwrap();
        let loaded = base.object_rows();
        // Reassert a changed document frame against the exact salted id if a
        // previous move's acknowledgement was lost, including undo to source.
        let mut force = self.objects.release.clone();
        for (&key, failure) in &self.objects.failed {
            if failure.unknown && let Op::Set { frame, .. } = failure.change.op
                && doc.get(&key).and_then(|r| r.frame).is_some_and(|new| new != frame)
            {
                force.insert(key);
            }
        }
        let changes = diff(loaded, doc, sample, &self.objects.ack, &force);
        let dirty_slots: BTreeSet<_> = self.edits.created.iter().map(|w| (w.block, w.index)).collect();
        self.objects.can_respawn = !dirty_slots.is_empty() && loaded.iter().any(|(&key, r)| {
            dirty_slots.contains(&(key.block, r.index))
                && !self.objects.flight.contains_key(&key)
                && !self.objects.failed.get(&key).is_some_and(|f| f.unknown)
                && respawn(loaded, doc, key, sample, &self.objects.ack).is_some()
        });
        let mut rows = Vec::new();
        let mut calls = Vec::new();
        let mut keep_release = BTreeSet::new();
        // Undoing a failed edit clears its refusal; a later redo is a new edit.
        self.objects.failed.retain(|key, failure| {
            failure.manual || failure.unknown || changes.iter().any(|change| change.key == *key)
        });
        for mut change in changes {
            let key = change.key;
            if self.objects.failed.get(&key).is_some_and(|f| {
                f.unknown && !matches!((f.change.op, change.op),
                    (Op::Set { id: old, frame: a }, Op::Set { id, frame: b })
                        if old == id && a != b)
            }) {
                // Structural writes cannot be replayed from a missing receipt:
                // an unlinked preview can exist without a placement in the stream.
                continue;
            }
            let method = match change.op {
                Op::Set { .. } => method::OBJECT_SET,
                Op::Delete { .. } => method::OBJECT_DELETE,
                Op::Respawn => method::OBJECT_RESPAWN,
                Op::Reload(why) => {
                    rows.push(row(change, doc, why, false));
                    continue;
                }
            };
            if !self.has_method(method) {
                let why = format!("the game does not offer {method}. The object changes at its next creation or reload.");
                rows.push(row(change, doc, &why, false));
                continue;
            }
            if change.op == Op::Respawn && u16::try_from(change.index).is_err() {
                rows.push(row(
                    change,
                    doc,
                    "the slot exceeds the game's placement limit",
                    false,
                ));
                continue;
            }
            let moving = matches!(change.op, Op::Set { .. });
            if dragging && moving {
                self.objects.drag_moves.insert(key);
            }
            let final_move = moving && (released || self.objects.release.contains(&key));
            if self.objects.flight.contains_key(&key) {
                if final_move {
                    keep_release.insert(key);
                }
                continue;
            }
            if let Op::Set { id, frame } = change.op
                && let Some(original) = loaded.get(&key)
                && base.layout_matches(scenario, key.block)
                && base.object_frame(key.block, original.index) != Some(frame)
            {
                // The scnr poke can be refused or lose a was-byte comparison.
                // Move the body only after its slot holds the requested frame.
                if final_move {
                    keep_release.insert(key);
                    continue;
                }
                // During a continuous drag the next editor sample may already
                // be newer than the last successful batch. Follow that batch.
                let last = self.objects.ack.get(&key)
                    .and_then(|ack| ack.frame(id))
                    .or(original.frame);
                let Some(applied) = base.object_frame(key.block, original.index) else {
                    continue;
                };
                if last == Some(applied) {
                    continue;
                }
                change.op = Op::Set { id, frame: applied };
            }
            if let Some(failure) = self.objects.failed.get(&key) {
                let gone = failure.gone_preview && change.op == Op::Respawn
                    && matches!(failure.change.op, Op::Set { frame, .. }
                        if doc.get(&key).and_then(|r| r.frame) == Some(frame));
                if gone || failure.change == change {
                    rows.push(row(change, doc, &failure.why, true));
                    continue;
                }
            }
            if !due && !final_move {
                continue;
            }
            if moving {
                if !final_move && self.objects.last_move.get(&key)
                    .is_some_and(|last| now < *last + PUSH_EVERY)
                {
                    continue;
                }
                self.objects.last_move.insert(key, now);
            }
            let frame = base.object_frame(key.block, change.index)
                .or_else(|| doc.get(&key).and_then(|row| row.frame));
            calls.push((change, frame));
        }
        self.objects.release = keep_release;
        for (&key, ack) in &self.objects.ack {
            let (index, op, why) = if let Some((_, linked, slot)) = ack.binding()
                && (!linked || !loaded.contains_key(&key))
            {
                if !doc.contains_key(&key) {
                    continue;
                }
                (slot, Op::Respawn, "a preview until the map reloads")
            } else if let Ack::Delete { id } = ack
                && let Some(original) = loaded.get(&key)
                && !doc.contains_key(&key)
            {
                (
                    original.index,
                    Op::Delete { id: *id },
                    "still in the game's scenario until the map reloads",
                )
            } else {
                continue;
            };
            rows.push(row(Change { key, index, op }, doc, why, false));
        }
        for failure in self.objects.failed.values() {
            if failure.unknown {
                let why = format!("{}; reload the map to verify the object", failure.why);
                let mut issue = row(failure.change, doc, &why, true);
                issue["class"] = json!(tag_sync::RELOAD);
                issue["unknown"] = json!(true);
                rows.push(issue);
            } else if failure.manual && doc.contains_key(&failure.change.key) {
                rows.push(row(failure.change, doc, &failure.why, true));
            }
        }
        self.objects.rows = rows;
        self.objects.publish(&mut app.pairing.pending);
        for (change, frame) in calls {
            self.send_object(change, frame, None);
        }
    }

    fn send_object(&mut self, change: Change, frame: Option<Frame>, creation: Option<sync::Write>) {
        let peers = self.edits.confirmed;
        let (method, params) = match change.op {
            Op::Set { id, frame } => (
                method::OBJECT_SET,
                json!({"id":id.to_string(), "pos":frame.pos, "forward":frame.forward, "up":frame.up,
                    "confirm_peers":peers}),
            ),
            Op::Delete { id } => (
                method::OBJECT_DELETE,
                json!({"id":id.to_string(), "confirm_peers":peers}),
            ),
            Op::Respawn => (
                method::OBJECT_RESPAWN,
                json!({"placement":{"block":crate::api::block_name(change.key.block),
                    "index":change.index}, "confirm_peers":peers}),
            ),
            Op::Reload(_) => return,
        };
        self.objects.next_ticket += 1;
        let ticket = self.objects.next_ticket;
        self.objects.failed.remove(&change.key);
        self.objects.flight.insert(
            change.key,
            Flight { ticket, change, frame, creation },
        );
        self.call(method, params, Purpose::Object(ticket));
    }

    pub(in crate::play) fn respawn_objects(&mut self, app: &App) -> Result<(), String> {
        self.object_gate(app)?;
        if self.object_scope(app) != self.objects.scope {
            return Err("waiting for the current map's live objects".into());
        }
        self.require_method(method::OBJECT_RESPAWN)?;
        let base = &self.baseline.as_ref().ok_or("waiting for the loaded map")?.1;
        let doc = document(app.session.scenario().ok_or("open a map first")?);
        let sample = app.pairing.shown.as_ref().unwrap();
        let mut calls = BTreeMap::new();
        for w in &self.edits.created {
            let Some((&key, _)) = base.object_rows().iter().find(|(key, row)| {
                key.block == w.block && row.index == w.index
            }) else { continue };
            if self.objects.flight.contains_key(&key)
                || self.objects.failed.get(&key).is_some_and(|f| f.unknown)
            {
                continue;
            }
            if let Some(change) = respawn(base.object_rows(), &doc, key, sample, &self.objects.ack) {
                calls.entry(key).or_insert_with(|| {
                    (change, base.object_frame(w.block, w.index), w.clone())
                });
            }
        }
        if calls.is_empty() {
            return Err("no live object has an acknowledged creation-field change to respawn".into());
        }
        for (change, frame, write) in calls.into_values() {
            self.send_object(change, frame, Some(write));
        }
        Ok(())
    }

    pub(in crate::play) fn object_answer(
        &mut self,
        app: &mut App,
        ticket: u64,
        result: Result<Value, RpcError>,
    ) {
        if self.object_scope(app) != self.objects.scope {
            return;
        }
        let Some(key) = self.objects.flight.iter()
            .find_map(|(&key, f)| (f.ticket == ticket).then_some(key))
        else {
            return;
        };
        let flight = self.objects.flight.remove(&key).unwrap();
        let change = flight.change;
        let reply = result.and_then(|v| {
            let object = |name: &str| v[name].as_str().and_then(ObjectId::parse);
            let tick = v["at"]["tick"].as_u64()
                .or_else(|| app.pairing.shown.as_ref().map(|sample| sample.tick))
                .unwrap_or(0);
            let ack = match change.op {
                Op::Set { id, frame } if object("object") == Some(id) => {
                    let binding = self.objects.ack.get(&key).and_then(Ack::binding)
                        .filter(|(saved, _, _)| *saved == id);
                    Some(Ack::Set {
                        id, frame, tick, linked: binding.is_none_or(|(_, linked, _)| linked),
                        index: binding.map_or(change.index, |(_, _, index)| index),
                    })
                }
                Op::Delete { id } if object("deleted") == Some(id) => Some(Ack::Delete { id }),
                Op::Respawn => match (object("object"), v["linked"].as_bool(), flight.frame) {
                    (Some(id), Some(linked), Some(frame)) => Some(Ack::Spawn {
                        id, linked, frame, tick, index: change.index,
                    }),
                    _ => None,
                },
                _ => None,
            };
            ack.ok_or_else(|| {
                RpcError::new(code::INVALID_REQUEST, "unreadable object edit acknowledgement")
            })
        });
        match reply {
            Ok(ack) => {
                self.objects.ack.insert(key, ack);
                if let Some(w) = flight.creation {
                    self.edits.created.retain(|dirty| dirty != &w);
                }
            }
            Err(e) => {
                if e.code == code::GONE && let Op::Delete { id } = change.op {
                    self.objects.ack.insert(key, Ack::Delete { id });
                } else {
                    let why = e.data.as_ref().and_then(|d| d["why"].as_str()).unwrap_or(&e.message);
                    // A refused respawn may have removed the old object before
                    // the host's creation call failed. Retire that salted id.
                    if change.op == Op::Respawn
                        && let Some(deleted) = e.data.as_ref()
                            .and_then(|d| d["deleted"].as_str()).and_then(ObjectId::parse)
                    {
                        self.objects.ack.insert(key, Ack::Delete { id: deleted });
                    }
                    let gone_preview = e.code == code::GONE
                        && forget_gone_preview(&mut self.objects.ack, key, change.op);
                    self.objects.failed.insert(
                        key,
                        Failure {
                            change,
                            why: why.into(),
                            manual: flight.creation.is_some(),
                            gone_preview,
                            unknown: matches!(e.code, code::TIMEOUT | code::INVALID_REQUEST),
                        },
                    );
                }
                app.error(format!("live object edit: {e}"));
            }
        }
        self.objects.dirty = true;
    }
}
