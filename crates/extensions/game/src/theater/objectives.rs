//! Observe post-tick retail game-engine objective state and successful power-up
//! applications. The registry has at most eight entries. There is no need to
//! walk the object table or re-read scenario geometry to find state changes.
use std::sync::Arc;

use engine::objectives::{self, Registered};
use guest_layout::{DatumHandle, GuestMemory, MemFault, points};
use live_proto::film::events::{
    Event,
    objective::{Change, Item, ItemChange},
};

use super::{Mode, Session};

#[derive(Default)]
pub(super) struct Tracker {
    state: State,
    pickups: Vec<Event>,
}

#[derive(Default)]
struct State {
    item: Option<Item>,
    registered: Vec<Registered>,
    /// Optional machine fields may be unavailable during initialisation.
    bombs: Option<[objectives::BombState; 9]>,
    hill: Option<(u8, u16)>,
    territories: Vec<objectives::TerritoryState>,
    juggernauts: Option<u16>,
}

impl State {
    fn read(m: &dyn GuestMemory) -> Result<Self, MemFault> {
        let mut state = Self::default();
        let e = m.u32(objectives::GAME_ENGINE)?;
        if e == 0 {
            return Ok(state);
        }
        objectives::check_span(e, objectives::ENGINE_INDEX + 4)?;
        let engine = m.i32(e + objectives::ENGINE_INDEX)?;
        state.item = match engine {
            1 => Some(Item::Flag),
            3 => Some(Item::Ball),
            9 => Some(Item::Bomb),
            _ => None,
        };
        if state.item.is_some() {
            state.registered = objectives::registered(m, e)?;
        }
        match engine {
            4 => {
                let k = m.u32(objectives::KOTH_GLOBALS)?;
                if k != 0 {
                    objectives::check_span(k, objectives::HILL_OCCUPANTS + 2)?;
                    let hill = m.i32(k + objectives::KOTH_HILL)?;
                    if !(-1..i32::from(objectives::kind::HILLS)).contains(&hill) {
                        return Err(MemFault {
                            addr: k + objectives::KOTH_HILL,
                            len: 4,
                            write: false,
                        });
                    }
                    if hill >= 0 {
                        state.hill = Some((hill as u8, m.u16(k + objectives::HILL_OCCUPANTS)?));
                    }
                }
            }
            7 => {
                let j = m.u32(points::JUGGERNAUT_GLOBALS.base)?;
                if j != 0 {
                    state.juggernauts = Some(m.u16(j)?);
                }
            }
            8 => {
                let t = m.u32(points::TERRITORIES_GLOBALS.base)?;
                if t != 0 {
                    state.territories = objectives::territories(m, t)?;
                }
            }
            9 => {
                let a = m.u32(points::CTF_ASSAULT_GLOBALS.base)?;
                if a != 0 {
                    let mut bombs = [objectives::BombState::default(); 9];
                    for (team, bomb) in bombs.iter_mut().enumerate() {
                        *bomb = objectives::bomb_state(m, a, team as u8)?;
                    }
                    state.bombs = Some(bombs);
                }
            }
            _ => {}
        }
        Ok(state)
    }

    fn changes(&self, next: &Self, out: &mut Vec<Event>) {
        let mut emit = |change| out.push(Event::ObjectiveState(change));
        // Unregistration can compact the array by moving its last entry.
        // Identity is the salted object datum, never the registration slot.
        if let Some(item) = self.item {
            for old in &self.registered {
                if self.item != next.item || !next.registered.iter().any(|r| r.object == old.object)
                {
                    emit(item_change(item, ItemChange::Unregistered, old));
                }
            }
        }
        if let Some(item) = next.item {
            for new in &next.registered {
                let old = self
                    .registered
                    .iter()
                    .find(|r| r.object == new.object && self.item == next.item);
                let change = match old {
                    None => ItemChange::Registered,
                    Some(old) if old != new => ItemChange::CarrierChanged,
                    _ => continue,
                };
                emit(item_change(item, change, new));
            }
        }
        if let Some(bombs) = next.bombs {
            for (team, state) in bombs.into_iter().enumerate() {
                let previous = self.bombs.map(|b| b[team]).unwrap_or_default();
                if previous != state {
                    emit(Change::BombState {
                        team: team as u8,
                        arming: state.arming,
                        armed: state.armed,
                        planted: state.planted,
                    });
                }
            }
        }
        if next.hill != self.hill
            && let Some((index, occupants)) = next.hill
        {
            emit(Change::Hill { index, occupants });
        }
        if next.juggernauts != self.juggernauts
            && let Some(players) = next.juggernauts
        {
            emit(Change::Juggernauts { players });
        }
        for site in &next.territories {
            if !self.territories.contains(site) {
                emit(Change::Territory {
                    index: site.index,
                    owner: site.owner.map(|(owner, _)| u32::from(owner.index())),
                    team: site.owner.map(|(_, team)| team),
                    occupants: site.occupants,
                });
            }
        }
    }
}

fn item_change(item: Item, change: ItemChange, record: &Registered) -> Change {
    Change::Item {
        item,
        change,
        object: record.object.raw(),
        designator: record.designator,
        unit: record.unit.map(DatumHandle::raw),
        player: record.player.map(|player| u32::from(player.index())),
    }
}

impl Tracker {
    pub(super) fn start_match(&mut self) {
        self.state = State::default();
    }

    pub(super) fn take_pickups(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.pickups)
    }

    pub(super) fn observe(&mut self, m: &dyn GuestMemory) -> Result<Vec<Event>, String> {
        let next =
            State::read(m).map_err(|e| format!("cannot read film objective at {:#x}", e.addr))?;
        let mut changes = Vec::new();
        self.state.changes(&next, &mut changes);
        self.state = next;
        Ok(changes)
    }
}

fn powerup(
    m: &dyn GuestMemory,
    sp: u32,
    unit: u32,
    definition: u32,
) -> Result<Option<Event>, MemFault> {
    use guest_layout::view::{objects, players};

    let Some(object) = bots::guest::pickups::consumed_equipment(m, sp)? else {
        return Ok(None);
    };
    let Some((live_telemetry::layout::OBJECT_TYPE_EQUIPMENT, body)) = objects::get(m, object)?
    else {
        return Ok(None);
    };
    if body.definition(m)?.map(DatumHandle::raw) != Some(definition) {
        return Ok(None);
    }
    let mut player = None;
    if let Some(handle) = DatumHandle::from_raw(unit)
        && let Some((objects::BIPED, biped)) = objects::get(m, handle)?
        && let Some(owner) =
            DatumHandle::from_raw(m.u32(biped.addr.checked_add(0x13c).ok_or(MemFault {
                addr: biped.addr,
                len: 4,
                write: false,
            })?)?)
        && let Some(players) = players::array(m)?
        && let Some(p) = players.get(m, owner)?
        && p.listed(m)?
        && p.unit(m)? == Some(handle)
    {
        player = Some(u32::from(owner.index()));
    }
    Ok(Some(Event::PowerupTaken {
        object: object.raw(),
        definition,
        unit,
        player,
    }))
}

pub(super) fn install(b: &mut hooks::Builder, session: Arc<Session>) {
    b.raw()
        .observe_before(&points::POWERUP_TAKEN, "film applied power-up", move |o| {
            if session.replaying {
                return;
            }
            // Drop the guest guard before taking the recorder lock.
            let pickup = powerup(&*o.guest(), o.cpu.esp, o.cpu.ecx, o.cpu.edx);
            let mut state = session.lock();
            if state.complete || state.failure.is_some() {
                return;
            }
            match pickup {
                Ok(Some(event)) => {
                    if let Mode::Record(r) = &mut state.mode {
                        r.unified.objectives.pickups.push(event);
                    }
                }
                Ok(None) => {}
                Err(_) => {
                    if let Mode::Record(r) = &mut state.mode {
                        r.unified.dropped("powerup");
                    }
                }
            }
        })
        .optional();
}

#[cfg(test)]
mod tests;
