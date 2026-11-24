//! Drive object edits on the control connection. Replies acknowledge the exact
//! frame or slot requested, even if the document changes while the call runs.
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Instant;

use agent_proto::{ObjectId, sim::method};
use serde_json::{Value, json};

use super::{Ack, Change, Frame, Key, LayoutIndex, Op, Rows, SampleIndex, diff, document, respawn};
use crate::app::App;
use crate::play::{Play, PUSH_EVERY, link::{ObjectDispatch, Purpose}, tag_sync};

mod outcome;
mod placements;
mod wire;
use outcome::Failure;

#[derive(Clone, Copy, PartialEq)]
struct Scope {
    pairing: u64,
    document: u64,
    epoch: u64,
    checksum: u32,
}

/// At most one object call can reach the worker at a time. Other edits are
/// represented by the document or a pending user request and get diffed again.
struct Flight {
    ticket: u64,
    change: Change,
    manual: bool,
    offered_at: Instant,
    creation: Vec<u8>,
    dispatch: Arc<ObjectDispatch>,
    markers: u64,
    peers: Option<u32>,
}

#[derive(Default)]
pub(in crate::play) struct State {
    scope: Option<Scope>,
    ack: BTreeMap<Key, Ack>,
    flight: BTreeMap<Key, Flight>,
    failed: BTreeMap<Key, Failure>,
    requested: BTreeSet<Key>,
    eligible: BTreeMap<Key, Change>,
    doc_revision: Option<u64>,
    doc: Rows,
    layout: LayoutIndex,
    changed: BTreeSet<Key>,
    sample: SampleIndex,
    binding_revision: Option<(Option<u64>, u64)>,
    creation_revision: Option<u64>,
    methods: Option<[bool; 3]>,
    writable: bool,
    next_ticket: u64,
    retry_at: Option<Instant>,
    // A newer observed tick can confirm an empty slot even when the
    // stream's object bindings have stayed identical since the last reply.
    after_tick: Option<u64>,
    dirty: bool,
    dragging: bool,
    drag_moves: BTreeSet<Key>,
    last_move: HashMap<ObjectId, Instant>,
    release: BTreeSet<Key>,
    rows: Vec<Value>,
    placements: placements::Placements,
    pub(in crate::play) waiting_for_sample: Option<&'static str>,
}

impl State {
    pub(in crate::play) fn waiting_for_permission(&self) -> bool {
        self.failed.values().any(Failure::is_permission)
    }

    pub(in crate::play) fn field_answer(&mut self) {
        self.dirty = true;
    }

    pub(in crate::play) fn can_respawn(&self) -> bool {
        self.writable && !self.eligible.is_empty()
    }

    /// The game applied placement transaction `t`: object receipts of its
    /// block follow the new numbering, and those of removed placements end.
    /// The baseline's rows and the slot index were rebuilt by the caller.
    fn placed(&mut self, app: &mut App, t: &crate::play::sync::Transaction) {
        let keys: Vec<Key> = self.ack.keys().chain(self.failed.keys())
            .filter(|key| key.block == t.block)
            .copied()
            .collect();
        for key in keys {
            match t.slot_of(key.id) {
                Some(slot) => {
                    if let Some(Ack::Set { index, .. } | Ack::Spawn { index, .. }) = self.ack.get_mut(&key) {
                        *index = slot;
                    }
                }
                None => {
                    self.ack.remove(&key);
                    self.failed.remove(&key);
                }
            }
            self.sync_preview(app, key);
        }
        self.doc_revision = None;
        self.dirty = true;
    }

    fn set_rows(&mut self, app: &mut App, rows: Vec<Value>) {
        if self.rows != rows {
            self.rows = rows;
            self.publish(&mut app.pairing.pending);
        }
    }

    pub(in crate::play) fn publish(&self, pending: &mut Vec<Value>) {
        self.placements.publish(pending);
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
                    p["doc"] == "scenario" && tag_sync::Effect::is_reload(p)
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

fn row(change: Change, doc: &Rows, why: &str, refused: bool) -> Value {
    let block = change.key.block;
    let name = crate::api::block_name(block);
    let current = doc.get(&change.key);
    let index = current.map_or(change.index, |row| row.index);
    let target = current.map_or_else(
        || json!({"panel":"pending"}),
        |_| json!({"block":name, "index":index}),
    );
    let structural = matches!(change.op, Op::Reload(_) | Op::Respawn { .. } | Op::Delete { .. });
    let effect = if structural {
        tag_sync::Effect::Reload
    } else {
        tag_sync::Effect::NextSpawn
    };
    json!({
        "doc": "object",
        "block": format!("{block:?}"),
        "path": format!("{name}[{index}]"),
        "target": target,
        "effect": effect.id(),
        "text": effect.sentence(),
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

    pub(in crate::play) fn reset_objects(&mut self, app: &mut App) {
        self.objects.cancel_unsent();
        let next_ticket = self.objects.next_ticket;
        // Placement transactions keep their own map scope: a lost reply's
        // hold outlives a new pairing on the same map activation.
        let mut placements = std::mem::take(&mut self.objects.placements);
        placements.transport_lost();
        self.objects = State { next_ticket, placements, ..Default::default() };
        app.pairing.previews.clear();
        self.objects.publish(&mut app.pairing.pending);
    }

    pub(in crate::play) fn update_objects(&mut self, app: &mut App, now: Instant) {
        self.revalidate_object_dispatch(app);
        let scope = self.object_scope(app);
        if self.is_paired() && scope.is_some() && self.objects.scope != scope
            && self.edits.world == Some(app.session.world_rev())
        {
            self.reset_objects(app);
            self.objects.scope = scope;
            if let Some((_, base)) = &self.baseline {
                self.objects.layout = LayoutIndex::new(base.object_rows());
            }
        }
        self.update_placements(app);

        let writable = self.object_gate(app).is_ok() && scope == self.objects.scope;
        if self.objects.writable != writable {
            self.objects.writable = writable;
            self.objects.dirty = true;
        }
        if !writable {
            self.objects.requested.clear();
            self.objects.drag_moves.clear();
            self.objects.release.clear();
            self.objects.dragging = false;
        }
        // Without verified identity keep the last published rows, including
        // a request whose receipt was lost. Don't compare against a new file.
        if scope.is_none() || self.objects.scope != scope || !self.is_paired()
            || self.reload_copy.is_some()
        {
            if self.objects.dirty {
                let mut rows = self.objects.rows.clone();
                for failure in self.objects.failed.values().filter(|f| f.is_unknown()) {
                    let issue = failure_row(failure, &self.objects.doc);
                    if !rows.contains(&issue) {
                        rows.push(issue);
                    }
                }
                self.objects.set_rows(app, rows);
                self.objects.dirty = false;
            }
            return;
        }
        if !self.live_edits && self.objects.rows.is_empty() && self.objects.ack.is_empty()
            && self.objects.failed.is_empty()
        {
            return;
        }

        let dragging = writable && (app.session.in_gesture() || app.session.dragging());
        let released = self.objects.dragging && !dragging;
        self.objects.dragging = dragging;
        if released {
            self.objects.release.append(&mut self.objects.drag_moves);
            self.push_edits(app);
        }
        let Some((_, base)) = &self.baseline else { return };
        let Some(scenario) = app.session.scenario() else { return };
        let revision = app.session.markers_rev();
        if self.objects.doc_revision != Some(revision) {
            let next = document(scenario);
            if dragging || released {
                // A release flushes frames changed by this gesture. Other
                // placements may already have staged or outstanding writes.
                let moved = next.iter().filter_map(|(&key, row)| {
                    let previous = self.objects.doc.get(&key)?;
                    (previous.frame != row.frame).then_some(key)
                });
                if released {
                    self.objects.release.extend(moved);
                } else {
                    self.objects.drag_moves.extend(moved);
                }
            }
            self.objects.doc = next;
            self.objects.changed = self.objects.layout.changed(base.object_rows(), &self.objects.doc);
            self.objects.doc_revision = Some(revision);
            self.objects.dirty = true;
        }

        let subscribed = self.has_method(agent_proto::live::SUBSCRIBE);
        self.objects.waiting_for_sample = subscribed.then(|| sample_refusal(app)).flatten();
        let sample = app.pairing.shown.as_ref()
            .filter(|_| subscribed && self.objects.waiting_for_sample.is_none());
        let binding = sample.map(|s| {
            let segment = app.pairing.display.segment();
            (segment, segment.map_or(s.tick, |_| app.pairing.display.binding_revision()))
        });
        if self.objects.binding_revision != binding {
            self.objects.sample = sample.map(SampleIndex::new).unwrap_or_default();
            self.objects.binding_revision = binding;
            self.objects.dirty = true;
        }
        let tick = sample.map(|s| s.tick);
        if let Some(tick) = tick {
            self.objects.sample.tick = tick;
            if self.objects.after_tick.is_some_and(|after| tick > after) {
                self.objects.dirty = true;
                self.objects.after_tick = None;
            }
        }

        let methods = [method::OBJECT_SET, method::OBJECT_DELETE, method::OBJECT_RESPAWN]
            .map(|m| self.has_method(m));
        let creation_revision = base.creation_revision();
        if self.objects.creation_revision != Some(creation_revision) || self.objects.methods != Some(methods) {
            self.objects.dirty = true;
            self.objects.methods = Some(methods);
            self.objects.creation_revision = Some(creation_revision);
        }
        if !self.objects.dirty && !released
            && self.objects.retry_at.is_none_or(|due| now < due)
        {
            return;
        }
        self.objects.dirty = false;
        self.objects.retry_at = None;
        if tick.is_some() {
            self.objects.reconcile_failures(app);
        }
        let Some(scenario) = app.session.scenario() else { return };
        let loaded = base.object_rows();
        let doc = &self.objects.doc;
        self.objects.failed.retain(|_, failure| failure.matches_creation(base));
        let mut force = self.objects.release.clone();
        for (&key, failure) in &self.objects.failed {
            if failure.force_move(doc.get(&key).and_then(|row| row.frame)) {
                force.insert(key);
            }
        }
        let mut changes = diff(
            loaded, doc, &self.objects.layout, &self.objects.changed,
            &self.objects.sample, &self.objects.ack, &force,
        );
        // A placement transaction deletes, creates and relinks the objects of
        // a block it owns. Only moves of existing objects stay here.
        let placements = &self.objects.placements;
        changes.retain(|c| !placements.owns(c.key.block) || matches!(c.op, Op::Set { .. }));
        let mut rows = Vec::new();
        // The first eligible call wins this frame. Every other candidate is
        // resolved from the latest document, session and bindings on reply.
        let mut call = None;
        self.objects.eligible.clear();
        let mut candidates = BTreeSet::new();
        for (block, index) in base.creation_slots() {
            if let Some(key) = self.objects.layout.key_at(block, index) {
                candidates.insert(key);
            }
        }
        candidates.extend(self.objects.failed.iter().filter_map(|(&key, f)| f.is_partial().then_some(key)));
        for key in candidates {
            if tick.is_none() || !writable || !methods[2] || self.objects.flight.contains_key(&key)
                || self.objects.placements.owns(key.block)
            {
                continue;
            }
            if let Some(change) = respawn(loaded, doc, key, &self.objects.sample, &self.objects.ack)
                && self.objects.failed.get(&key).is_none_or(|f| f.allows_manual(change, &self.objects.sample))
            {
                self.objects.eligible.insert(key, change);
            }
        }
        self.objects.requested.retain(|key| self.objects.eligible.contains_key(key));
        if self.edits.sent.is_none()
            && let Some(key) = self.objects.requested.first()
        {
            call = self.objects.eligible.get(key).copied().map(|change| (change, true));
        }

        let mut active_failures = BTreeSet::new();
        let unobserved = tick.is_none();
        let mut release = BTreeSet::new();
        for mut change in changes {
            let key = change.key;
            let moving = matches!(change.op, Op::Set { .. });
            let final_move = moving && self.objects.release.contains(&key);
            if final_move {
                release.insert(key);
            }
            if let Some(failure) = self.objects.failed.get(&key)
                && failure.blocks(change)
            {
                active_failures.insert(key);
                continue;
            }
            let (available, method) = match change.op {
                Op::Set { .. } => (methods[0], method::OBJECT_SET),
                Op::Delete { .. } => (methods[1], method::OBJECT_DELETE),
                Op::Respawn { .. } => (methods[2], method::OBJECT_RESPAWN),
                Op::Reload(why) => {
                    let mut issue = row(change, doc, why, false);
                    issue["needs_reload"] = json!(true);
                    rows.push(issue);
                    continue;
                }
            };
            if !available {
                let why = format!("the game does not offer {method}. The object changes at its next creation or reload.");
                let mut issue = row(change, doc, &why, false);
                issue["needs_reload"] = json!(true);
                rows.push(issue);
                continue;
            }
            if let Op::Respawn { id: None } = change.op
                && u16::try_from(change.index).is_err()
            {
                let mut issue = row(change, doc, "the slot exceeds the game's placement limit", false);
                issue["needs_reload"] = json!(true);
                rows.push(issue);
                continue;
            }
            if !writable || call.is_some() || !self.objects.flight.is_empty() {
                continue;
            }
            if matches!(change.op, Op::Respawn { .. }) && self.edits.sent.is_some() {
                continue;
            }
            if let Op::Set { id, frame } = change.op {
                if let Some(original) = loaded.get(&key)
                    && base.layout_matches(scenario, key.block)
                    && base.object_frame(key.block, original.index) != Some(frame)
                {
                    if final_move {
                        continue;
                    }
                    let last = self.objects.ack.get(&key).and_then(|ack| ack.frame(id)).or(original.frame);
                    let Some(applied) = base.object_frame(key.block, original.index) else { continue };
                    if last == Some(applied) {
                        continue;
                    }
                    change.op = Op::Set { id, frame: applied };
                    // The frame to follow may be the one whose reply was lost.
                    if let Some(failure) = self.objects.failed.get(&key)
                        && failure.blocks(change)
                    {
                        active_failures.insert(key);
                        continue;
                    }
                }
                if !final_move && let Some(last) = self.objects.last_move.get(&id) {
                    let due = *last + PUSH_EVERY;
                    if now < due {
                        self.objects.retry_at = Some(self.objects.retry_at.map_or(due, |old| due.min(old)));
                        continue;
                    }
                }
            }
            call = Some((change, false));
        }
        self.objects.failed.retain(|key, f| {
            let dirty = loaded.get(key).is_some_and(|r| base.needs_creation(key.block, r.index));
            let keep = f.keep_without_diff(dirty, !doc.contains_key(key)) || unobserved || active_failures.contains(key);
            if keep && (active_failures.contains(key) || f.is_unknown() || f.is_partial() || f.manual && dirty || unobserved) {
                rows.push(failure_row(f, doc));
            }
            keep
        });
        for (&key, ack) in &self.objects.ack {
            let (index, op, why) = if let Some((id, linked, slot)) = ack.binding()
                && (!linked || !loaded.contains_key(&key)) && doc.contains_key(&key)
            {
                (slot, Op::Respawn { id: Some(id) }, "a preview until the map reloads")
            } else if let Ack::Delete { id } = ack
                && let Some(original) = loaded.get(&key)
                && !doc.contains_key(&key)
            {
                (original.index, Op::Delete { id: *id }, "still in the game's scenario until the map reloads")
            } else {
                continue;
            };
            rows.push(row(Change { key, index, op }, doc, why, false));
        }
        if let Some(tick) = tick {
            let until_ack = self.objects.ack.values().filter_map(|a| match *a {
                Ack::Set { tick: at, .. } | Ack::Spawn { tick: at, .. } if at >= tick => Some(at),
                _ => None,
            }).min();
            let until_observed = self.objects.failed.values()
                .filter_map(Failure::awaiting_sample)
                .filter(|&after| after >= tick)
                .min();
            self.objects.after_tick = until_ack.into_iter().chain(until_observed).min();
        }
        self.objects.release = release;
        self.objects.set_rows(app, rows);
        if let Some((change, manual)) = call
            && self.objects.flight.is_empty()
            && !self.objects.placements.in_flight()
            && self.send_object(change, manual, app, now)
        {
            if let Op::Set { .. } = change.op {
                self.objects.release.remove(&change.key);
            }
            self.objects.requested.remove(&change.key);
        }
    }

    /// The object definitions the Pending tab's `next_spawn` rows change.
    pub(in crate::play) fn can_respawn_definitions(&self, app: &App) -> bool {
        self.live_edits
            && self.has_method(method::OBJECT_RESPAWN)
            && !super::definitions::definitions(&app.tags, &app.pairing.pending).is_empty()
    }

    /// One `world.object.respawn {definition}` per object definition the
    /// pending `next_spawn` rows change; the count sent.
    pub(in crate::play) fn respawn_definitions(&mut self, app: &App) -> Result<usize, String> {
        let defs = super::definitions::definitions(&app.tags, &app.pairing.pending);
        if defs.is_empty() {
            return Ok(0);
        }
        if !self.live_edits {
            return Err("View mode".into());
        }
        if self.reload_copy.is_some() {
            return Err("waiting for the play copy to load".into());
        }
        if let Some(why) = self.map_refusal(app).or_else(|| self.session_refusal(app)) {
            return Err(why);
        }
        self.require_method(method::OBJECT_RESPAWN)?;
        for tag in &defs {
            self.call(
                method::OBJECT_RESPAWN,
                json!({ "definition": format!("{:08x}", tag.0) }),
                Purpose::Control("respawn.definition"),
            );
        }
        Ok(defs.len())
    }

    pub(in crate::play) fn respawn_objects(&mut self, app: &App) -> Result<(), String> {
        self.object_gate(app)?;
        self.require_method(method::OBJECT_RESPAWN)?;
        if self.object_scope(app) != self.objects.scope || self.objects.eligible.is_empty() {
            return Err("no live object has an acknowledged creation-field change to respawn".into());
        }
        self.objects.requested.extend(self.objects.eligible.keys().copied());
        self.objects.dirty = true;
        Ok(())
    }
}

fn failure_row(f: &Failure, doc: &Rows) -> Value {
    let mut issue = row(f.change, doc, &f.why, true);
    if f.is_unknown() {
        issue["why"] = json!(format!("{}; reload the map to verify the object", f.why));
        issue["unknown"] = json!(true);
        tag_sync::set_effect(&mut issue, tag_sync::Effect::Reload);
    }
    issue
}
