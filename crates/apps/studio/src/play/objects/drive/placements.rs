//! Added and removed placements as one `world.placement.edit` per block
//! (docs/editor/realtime-edits.md, "Placements"). The game's block is the
//! baseline's (`sync::Baseline::transaction`): a block whose layout revision
//! differs from it goes to the game as one transaction, and the receipt
//! advances the baseline to the document's layout, so add, remove, undo and
//! redo are the same call. One transaction is in flight at a time, never
//! beside a field batch or an object call, which compute against the same
//! baseline. A refusal holds that exact transaction under Reload with the
//! game's reason until the document or the peer confirmation changes it. A
//! lost reply holds the block until the map loads again: the game may have
//! applied it, so it is never sent twice.

use agent_proto::{RpcError, code};
use scenario_edit::layout::BLOCKS;
use scenario_edit::{BlockKind, Scenario};

use super::*;
use crate::play::link::Purpose;
use crate::play::objects::key_index;
use crate::play::sync::Transaction;
use crate::play::tag_sync::Effect;

/// Marks the rows this module owns in Pending, so a publish replaces them.
const MARK: &str = "placement_edit";

/// Whether the game takes a block's added and removed entries live, and
/// whether its records make objects. The game makes no live sound scenery
/// (`placement::BLOCKS` lacks it), so that block waits for a reload.
pub(in crate::play) fn live(kind: BlockKind) -> Option<bool> {
    use BlockKind::*;
    match kind {
        Scenery | Vehicles | Equipment | Weapons | Machines | Crates => Some(true),
        StartingLocations | NetgameFlags | NetgameEquipment | StartingEquipment => Some(false),
        SoundScenery => None,
    }
}

struct Flight {
    ticket: u64,
    transaction: Transaction,
    params: Value,
}

struct Held {
    params: Value,
    why: String,
    /// The reply was lost: the game may hold the edit.
    unknown: bool,
}

#[derive(Default)]
pub(in crate::play) struct Placements {
    /// The map activation and document the state belongs to: `(epoch,
    /// checksum, world revision)`.
    scope: Option<(u64, u32, u64)>,
    flight: Option<Flight>,
    held: BTreeMap<BlockKind, Held>,
    /// Entries the game holds without an object, with its reason.
    not_created: BTreeMap<Key, String>,
    /// Blocks whose add and remove go through this module rather than the
    /// object calls and the Reload row of the field diff.
    owned: BTreeSet<BlockKind>,
    rows: Vec<Value>,
}

impl Placements {
    pub(in crate::play) fn owns(&self, kind: BlockKind) -> bool {
        self.owned.contains(&kind)
    }

    pub(in crate::play) fn in_flight(&self) -> bool {
        self.flight.is_some()
    }

    pub(super) fn answers(&self, ticket: u64) -> bool {
        self.flight.as_ref().is_some_and(|f| f.ticket == ticket)
    }

    /// The connection closed, or the pairing reset, with a transaction in
    /// flight: its answer can no longer arrive.
    pub(super) fn transport_lost(&mut self) {
        if let Some(f) = self.flight.take() {
            self.held.insert(f.transaction.block, Held {
                params: f.params,
                why: "the game's answer to the placement change was lost".into(),
                unknown: true,
            });
        }
    }

    /// A new map activation drops everything; an unchanged one, or a link
    /// with no verified identity yet, keeps a lost reply's hold.
    fn rescope(&mut self, scope: Option<(u64, u32, u64)>) {
        if let Some(scope) = scope
            && self.scope != Some(scope)
        {
            *self = Self { scope: Some(scope), ..Self::default() };
        }
    }

    /// Replace the Reload row the field diff gives an owned block, and this
    /// module's rows, in `pending`.
    pub(super) fn publish(&self, pending: &mut Vec<Value>) {
        pending.retain(|row| {
            row[MARK] != true
                && !(row["doc"] == "scenario"
                    && tag_sync::Effect::is_reload(row)
                    && self.owned.iter().any(|kind| row["path"] == format!("{kind:?}")))
        });
        pending.extend(self.rows.iter().cloned());
    }

    fn rows(&self, tag: &Value, scenario: &Scenario) -> Vec<Value> {
        let block_row = |kind: BlockKind, effect, why: &str| {
            let mut row = tag_sync::row("scenario", tag.clone(), &format!("{kind:?}"), effect, why);
            row["target"] = json!({"panel": "pending"});
            row[MARK] = json!(true);
            row
        };
        let mut rows = Vec::new();
        for &kind in &self.owned {
            let row = match self.held.get(&kind) {
                Some(h) => {
                    let why = if h.unknown {
                        format!("{}; reload the map to verify the placements", h.why)
                    } else {
                        h.why.clone()
                    };
                    let mut row = block_row(kind, Effect::Reload, &why);
                    row["refused"] = json!(true);
                    row["needs_reload"] = json!(true);
                    if h.unknown {
                        row["unknown"] = json!(true);
                    }
                    row
                }
                None => block_row(kind, Effect::Now, "sending the added and removed entries to the game"),
            };
            rows.push(row);
        }
        for (key, why) in &self.not_created {
            let Some(index) = key_index(scenario, *key) else { continue };
            let name = crate::api::block_name(key.block);
            let mut row = tag_sync::row("scenario", tag.clone(), &format!("{name}[{index}]"), Effect::Reload, why);
            row["target"] = json!({"block": name, "index": index});
            row["needs_reload"] = json!(true);
            row[MARK] = json!(true);
            rows.push(row);
        }
        rows
    }
}

impl Play {
    /// Send the next placement transaction, and keep the rows of the blocks
    /// it owns. Runs every frame, before the object diff that it takes
    /// these blocks' structural changes from.
    pub(in crate::play) fn update_placements(&mut self, app: &mut App) {
        let world = app.session.world_rev();
        let scope = self.edits.epoch.zip(self.edits.checksum).map(|(e, c)| (e, c, world));
        self.objects.placements.rescope(scope);
        let base = self.baseline.as_ref()
            .filter(|_| self.edits.world == Some(world))
            .map(|(_, base)| base);
        let (Some(base), Some(scenario)) = (base, app.session.scenario()) else {
            return;
        };
        let active = scope.is_some()
            && self.objects.scope.is_some()
            && self.objects.scope == self.object_scope(app)
            && self.live_edits
            && self.reload_copy.is_none()
            && self.link.is_some()
            && self.edit_refusal(app).is_none()
            && self.has_method(method::PLACEMENT_EDIT);

        let state = &mut self.objects.placements;
        // A refusal is moot once the document is back at the game's layout.
        // A lost reply is not: the game may hold the edit.
        state.held.retain(|&kind, h| h.unknown || !base.layout_matches(scenario, kind));
        state.not_created.retain(|key, _| {
            base.object_rows().contains_key(key)
                && key_index(scenario, *key).is_some()
        });
        let structural = BLOCKS.iter()
            .map(|b| b.kind)
            .filter(|&kind| live(kind).is_some() && !base.layout_matches(scenario, kind));
        state.owned = if active { structural.collect() } else { BTreeSet::new() };
        state.owned.extend(state.held.keys().copied());
        if let Some(f) = &state.flight {
            state.owned.insert(f.transaction.block);
        }

        let mut send = None;
        if active && state.flight.is_none() && self.edits.sent.is_none() && self.objects.flight.is_empty() {
            for def in BLOCKS {
                let Some(objects) = live(def.kind) else { continue };
                let Some(t) = base.transaction(scenario, def.kind, objects) else { continue };
                let params = serde_json::to_value(t.params(self.edits.epoch, self.edits.confirmed))
                    .expect("placement edit params serialize");
                if state.held.get(&def.kind).is_some_and(|h| h.unknown || h.params == params) {
                    continue;
                }
                state.held.remove(&def.kind);
                send = Some((t, params));
                break;
            }
        }
        let tag = json!(self.scenario_tag);
        if let Some((transaction, params)) = send {
            self.objects.next_ticket += 1;
            let ticket = self.objects.next_ticket;
            self.call(method::PLACEMENT_EDIT, params.clone(), Purpose::Object(ticket));
            self.objects.placements.flight = Some(Flight { ticket, transaction, params });
        }
        let Some(scenario) = app.session.scenario() else { return };
        let rows = self.objects.placements.rows(&tag, scenario);
        if rows != self.objects.placements.rows {
            self.objects.placements.rows = rows;
            self.objects.publish(&mut app.pairing.pending);
        }
    }

    /// The game's answer to the transaction in flight.
    pub(super) fn placement_answer(&mut self, app: &mut App, result: Result<Value, RpcError>) {
        let Some(flight) = self.objects.placements.flight.take() else { return };
        let t = flight.transaction;
        match result {
            Ok(reply) => {
                let world = app.session.world_rev();
                if let Some((_, base)) = &mut self.baseline
                    && self.edits.world == Some(world)
                {
                    base.placed(&t);
                    self.objects.layout = LayoutIndex::new(base.object_rows());
                    self.edits.rediff();
                }
                for created in reply["created"].as_array().into_iter().flatten() {
                    if created["created"] != false {
                        continue;
                    }
                    let index = created["index"].as_u64().and_then(|i| usize::try_from(i).ok());
                    let Some(id) = index.and_then(|i| t.id(i)) else { continue };
                    let why = created["why"].as_str()
                        .unwrap_or("the game wrote the record without making its object");
                    self.objects.placements.not_created.insert(Key { block: t.block, id }, why.to_owned());
                }
                self.objects.placed(app, &t);
            }
            Err(e) => {
                let why = e.data.as_ref().and_then(|d| d["why"].as_str())
                    .unwrap_or(&e.message).to_owned();
                // The method answers at a tick boundary. A timeout leaves
                // open whether that boundary ran it.
                let unknown = e.code == code::TIMEOUT;
                self.feedback.problem(app, format!("{}: {why}", method::PLACEMENT_EDIT));
                self.objects.placements.held.insert(t.block, Held { params: flight.params, why, unknown });
            }
        }
        // The next frame's pass rebuilds the rows; nothing stale meanwhile.
        if let Some(scenario) = app.session.scenario() {
            let tag = json!(self.scenario_tag);
            let state = &mut self.objects.placements;
            state.owned.remove(&t.block);
            state.owned.extend(state.held.keys().copied());
            state.rows = state.rows(&tag, scenario);
        }
        self.objects.publish(&mut app.pairing.pending);
        // Object calls wait for a transaction in flight; let them go again.
        self.objects.dirty = true;
    }
}

#[cfg(all(test, unix))]
mod tests;
