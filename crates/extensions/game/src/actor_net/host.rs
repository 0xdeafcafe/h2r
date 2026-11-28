//! Read the distributed host's presentation state once per completed tick.
//! A transport peer owns its budget for the duration of its connection.

use ::actor_net::digest::state_hash;
use ::actor_net::wire::MAX_ACTORS;
use ::actor_net::{
    Actor, ActorChange, ActorSet, Budget, BudgetError, Control, DIGEST_PERIOD_TICKS, Packet,
    Record, SetError,
};
use guest_layout::GuestMemory;
use sim_state::ActorView;

use super::dialogue::Line;

#[derive(Default)]
pub struct Host {
    tick: Option<u32>,
    set: ActorSet,
    changes: Vec<Record>,
    lines: Vec<Record>,
    /// The show channel's cues for this tick
    /// (`crate::rules::world::show::take_cues`).
    cues: Vec<Record>,
    /// The script's show state this tick
    /// (`crate::rules::world::show::take_state`), encoded: `None` when no
    /// script runs.
    show: Option<Vec<u8>>,
    /// The show state the last packet that carried one had, so a state the
    /// budget dropped is offered again the next tick.
    last_show: std::sync::Mutex<Option<Vec<u8>>>,
    controls: Vec<(u32, [f32; 3], Control)>,
    /// The head of the host log the running mode writes, for the digest;
    /// all zeros when no mode keeps one.
    log_head: [u8; 32],
}

fn actor(view: &ActorView) -> Actor {
    Actor {
        entity: view.entity,
        character: view.character,
        squad: view.squad,
        team: view.team,
        posture: view.posture,
    }
}

impl Host {
    /// Read the whole pool before committing a diff. An unreadable slot must
    /// never turn the rest of the previous snapshot into removals.
    pub(super) fn capture(
        &mut self,
        memory: &dyn GuestMemory,
        tick: u32,
        controls: bool,
        dialogue: &[Line],
    ) -> anyhow::Result<()> {
        let mut views = Vec::new();
        sim_state::visit_actors(memory, controls, |view| views.push(view))
            .map_err(|e| anyhow::anyhow!("actor pool read at {:#x} ({} bytes)", e.addr, e.len))?;
        anyhow::ensure!(
            views.len() <= MAX_ACTORS,
            "actor pool exceeds channel capacity"
        );
        views.sort_unstable_by_key(|v| v.entity);
        anyhow::ensure!(
            !views.windows(2).any(|v| v[0].entity == v[1].entity),
            "two actors share a replication entity"
        );

        self.changes.clear();
        let previous = self.set.actors();
        // Removals must precede creations when a full set turns over.
        for old in previous {
            if views
                .binary_search_by_key(&old.entity, |v| v.entity)
                .is_err()
            {
                self.changes
                    .push(Record::Actor(ActorChange::Remove(old.entity)));
            }
        }
        self.controls.clear();
        for view in &views {
            let row = actor(view);
            let old = previous
                .binary_search_by_key(&row.entity, |a| a.entity)
                .ok()
                .map(|i| previous[i]);
            match old {
                Some(a) if a == row => {}
                Some(a)
                    if (a.character, a.squad, a.team) == (row.character, row.squad, row.team) =>
                {
                    self.changes.push(Record::Posture {
                        entity: row.entity,
                        posture: row.posture,
                    });
                }
                _ => self.changes.push(Record::Actor(ActorChange::Upsert(row))),
            }
            if let Some(c) = &view.control {
                self.controls.push((
                    row.entity,
                    view.position,
                    Control::quantize(c.forward, c.side, c.crouch, c.facing_delta_radians),
                ));
            }
        }
        for change in &self.changes {
            self.set.apply(change)?;
        }
        self.lines.clear();
        self.cues.clear();
        for line in dialogue {
            if let Some(speaker) = views.iter().find(|v| v.unit == line.unit) {
                self.lines.push(Record::Line {
                    entity: speaker.entity,
                    vocalization: line.vocalization,
                    tick: line.tick,
                });
            }
        }
        self.tick = Some(tick);
        Ok(())
    }

    /// The Firefight HUD and script role state for this tick: a `State` record
    /// in this tick's changes when it differs from the last, and whole in
    /// every key. Call after [`Host::capture`], which starts the tick's
    /// changes afresh.
    pub(super) fn set_mode(&mut self, blob: &[u8]) -> Result<(), SetError> {
        if self.set.state() == blob {
            return Ok(());
        }
        let record = Record::State(blob.to_vec());
        self.set.apply(&record)?;
        self.changes.push(record);
        Ok(())
    }

    /// The show channel's cues for this tick (the running rules script's
    /// `message` calls, `crate::rules::world::show`): offered to every
    /// peer after the tick's lines. A cue carries its audience as a mask
    /// of player slots; the peer shows it to its own players in the
    /// audience and to no one else. Call after [`Host::capture`].
    pub(super) fn set_cues(&mut self, cues: Vec<Record>) {
        self.cues = cues;
    }

    /// The script's show state for this tick
    /// (`crate::rules::world::show::take_state`, `None` when no script
    /// runs): offered to every peer when it changes, at most five times a
    /// second, and whole in every key. The state carries each audience as
    /// a mask of player slots, and the peer's own machines show a record
    /// only to their players in it. Call after [`Host::capture`].
    pub(super) fn set_show(&mut self, show: Option<Vec<u8>>) {
        self.show = show;
    }

    /// The running mode's host log head for this tick's digest
    /// ([`super::log_head`]); none when no mode keeps a log.
    pub(super) fn set_log_head(&mut self, head: Option<[u8; 32]>) {
        self.log_head = head.unwrap_or_default();
    }

    /// Whether a host tick carries a `Digest`: every
    /// [`DIGEST_PERIOD_TICKS`] ticks of the host clock, so every peer
    /// gets the same ones.
    pub fn digest_due(tick: u32) -> bool {
        tick % DIGEST_PERIOD_TICKS == 0
    }

    /// Schedule once per connected client after capture, including quiet ticks
    /// (a line may still owe copies). The link holds a resync request until the
    /// returned packet actually contains the key.
    pub fn schedule(
        &self,
        budget: &mut Budget,
        position: Option<[f32; 3]>,
        request_key: bool,
    ) -> Result<Option<Packet>, BudgetError> {
        let Some(tick) = self.tick else {
            return Ok(None);
        };
        let with_key = budget.key_due(tick) || request_key;
        let mut offers = Vec::new();
        if with_key {
            offers.push(self.set.key());
        } else {
            offers.extend_from_slice(&self.changes);
        }
        // The script's show state: whole in every key, and otherwise when
        // it changed since the peer was last sent one. A state the budget
        // dropped (the five a second) is offered again the next tick, and
        // a script that stopped retracts the last state with an empty
        // record.
        let last_show = self.last_show.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if self.show.is_some() && (with_key || self.show != last_show) {
            offers.push(Record::Show(self.show.clone().unwrap_or_default()));
        } else if self.show.is_none() && last_show.is_some() {
            offers.push(Record::Show(Vec::new()));
        }
        offers.extend_from_slice(&self.lines);
        offers.extend_from_slice(&self.cues);
        // The set after this tick's changes, which the client checks once
        // the whole packet is applied (docs/game/ai-networking.md,
        // "Determinism and digests"), with the show state's hash beside
        // the actor set's.
        if Self::digest_due(tick) {
            let show_hash = state_hash(self.show.as_deref().unwrap_or(&[]));
            offers.push(self.set.digest(self.log_head, show_hash));
        }
        if let Some(origin) = position.filter(|p| p.iter().all(|v| v.is_finite())) {
            let mut ranked: Vec<_> = self
                .controls
                .iter()
                .filter_map(|&(entity, pos, control)| {
                    let [dx, dy, dz] = std::array::from_fn(|i| pos[i] - origin[i]);
                    let d2 = dx * dx + dy * dy + dz * dz;
                    d2.is_finite().then_some((d2, entity, control))
                })
                .collect();
            ranked.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            offers.extend(
                ranked
                    .into_iter()
                    .map(|(_, entity, control)| Record::Control { entity, control }),
            );
        }
        let packet = budget.schedule(tick, &offers)?;
        // Only a state the peer was actually sent counts as the last one,
        // so a state the budget dropped is offered again.
        if packet
            .as_ref()
            .is_some_and(|packet| packet.records.iter().any(|r| matches!(r, Record::Show(_))))
        {
            *self.last_show.lock().unwrap_or_else(|e| e.into_inner()) = self.show.clone();
        }
        Ok(packet)
    }
}

#[cfg(test)]
mod tests;
