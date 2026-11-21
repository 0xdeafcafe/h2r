//! Read the distributed host's presentation state once per completed tick.
//! A transport peer owns its budget for the duration of its connection.

use ::actor_net::wire::MAX_ACTORS;
use ::actor_net::{Actor, ActorChange, ActorSet, Budget, BudgetError, Control, Packet, Record};
use engine::GuestMemory;
use sim_state::ActorView;

use super::dialogue::Line;

#[derive(Default)]
pub struct Host {
    tick: Option<u32>,
    set: ActorSet,
    changes: Vec<Record>,
    lines: Vec<Record>,
    controls: Vec<(u32, [f32; 3], Control)>,
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
        let mut offers = Vec::new();
        if budget.key_due(tick) || request_key {
            offers.push(self.set.key());
        } else {
            offers.extend_from_slice(&self.changes);
        }
        offers.extend_from_slice(&self.lines);
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
        budget.schedule(tick, &offers)
    }
}

#[cfg(test)]
mod tests;
