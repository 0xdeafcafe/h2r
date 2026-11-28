//! Converted vehicle settings in the guest script globals. Seat availability,
//! reservation, movement and entry are handled by Halo 2's vehicle commands.

use std::collections::BTreeSet;

use guest_layout::{
    DatumHandle,
    entry::{Cx, EntryError, GuestCall},
    memory::{GuestMemory, MemFault},
    view::objects,
};
use hs::h1_ai_slots::{Operation, VEHICLE_ROWS, VEHICLE_WIDTH};

use super::{
    NONE,
    actors::{self, Actor},
    state,
};

const ENTER: u32 = 0x0027_4a50;
const COMMANDS: u32 = 0x90;
const LAST_COMMAND: u32 = 0x190;
const COMMAND_SIZE: u32 = 0x40;
const ENTER_COMMAND: u16 = 0x4c;
const COMMAND_VEHICLE: u32 = 0x1c;
const ACTOR_TEAM: u32 = 0x24;
const ACTOR_TYPE: u32 = 4;
const REGISTRY_WORDS: u32 = VEHICLE_ROWS as u32 * VEHICLE_WIDTH as u32;
const SELECTORS: usize = 6;

type R<T> = Result<T, MemFault>;

#[derive(Clone, Copy)]
struct Rule {
    handle: u32,
    destination: u32,
    radius: f32,
    teams: u32,
    kinds: u32,
    enabled: bool,
    selectors: [u32; SELECTORS],
}

impl Rule {
    fn new(handle: u32) -> Self {
        Self {
            handle,
            destination: NONE,
            radius: 8.0,
            teams: 0,
            kinds: 0,
            enabled: false,
            selectors: [NONE; SELECTORS],
        }
    }

    fn read(m: &dyn GuestMemory, at: u32) -> R<Self> {
        let word = |i: u32| m.u32(at + i * 8);
        let mut selectors = [NONE; SELECTORS];
        for (i, value) in selectors.iter_mut().enumerate() {
            *value = word(i as u32 + 5)?;
        }
        Ok(Self {
            handle: word(0)?,
            destination: word(1)?,
            radius: f32::from_bits(word(2)?),
            teams: word(3)? & u32::from(u16::MAX),
            kinds: word(3)? >> 16,
            enabled: word(4)? != 0,
            selectors,
        })
    }

    fn write(self, m: &mut dyn GuestMemory, at: u32) -> R<()> {
        let words = [
            self.handle,
            self.destination,
            self.radius.to_bits(),
            self.teams | self.kinds << 16,
            u32::from(self.enabled),
        ];
        for (i, value) in words.into_iter().chain(self.selectors).enumerate() {
            m.set_u32(at + i as u32 * 8, value)?;
        }
        Ok(())
    }

    fn accepts(&self, m: &dyn GuestMemory, actor: Actor, members: &BTreeSet<u32>) -> R<bool> {
        if self.teams == 0 && self.kinds == 0 && self.selectors[0] == NONE {
            return Ok(true);
        }
        let bit = |mask: u32, index: u16| mask.checked_shr(u32::from(index)).unwrap_or(0) & 1 != 0;
        Ok(members.contains(&actor.handle)
            || bit(self.teams, m.u16(actor.at + ACTOR_TEAM)?)
            || bit(self.kinds, m.u16(actor.at + ACTOR_TYPE)?))
    }
}

fn vehicle(m: &dyn GuestMemory, handle: u32) -> R<Option<objects::Body>> {
    let Some((objects::VEHICLE, body)) = objects::get(m, DatumHandle(handle))? else {
        return Ok(None);
    };
    Ok((body.damage_flags(m)? & 4 == 0).then_some(body))
}

pub(super) fn configure(cx: &Cx, op: Operation, args: &[u32]) -> R<()> {
    cx.write(|m| {
        let handle = args[0];
        if vehicle(m, handle)?.is_none() {
            return Ok(());
        }
        let Some(start) = state::registry(m, args[args.len() - 1], REGISTRY_WORDS)? else {
            return Ok(());
        };
        let mut available = None;
        let mut existing = None;
        for row in 0..u32::from(VEHICLE_ROWS) {
            let at = start + row * u32::from(VEHICLE_WIDTH) * 8;
            let old = m.u32(at)?;
            if old == handle {
                existing = Some(at);
                break;
            }
            if available.is_none() && (old == NONE || vehicle(m, old)?.is_none()) {
                available = Some(at);
            }
        }
        let (at, mut rule) = if let Some(at) = existing {
            (at, Rule::read(m, at)?)
        } else if let Some(at) = available {
            (at, Rule::new(handle))
        } else {
            return Ok(());
        };
        match op {
            Operation::VehicleEncounter => rule.destination = args[1],
            Operation::VehicleDistance => {
                let radius = f32::from_bits(args[1]);
                if radius.is_finite() {
                    rule.enabled = true;
                    rule.radius = radius.max(0.0);
                }
            }
            Operation::VehicleTeam | Operation::VehicleActorType => {
                let index = args[1] as u16;
                if index < 16 {
                    rule.enabled = true;
                    let mask = if op == Operation::VehicleTeam {
                        &mut rule.teams
                    } else {
                        &mut rule.kinds
                    };
                    *mask |= 1 << index;
                }
            }
            Operation::VehicleActors => {
                if args[1] != NONE
                    && !rule.selectors.contains(&args[1])
                    && let Some(slot) = rule.selectors.iter_mut().find(|slot| **slot == NONE)
                {
                    rule.enabled = true;
                    *slot = args[1];
                }
            }
            Operation::VehicleDisable => {
                rule = Rule {
                    destination: rule.destination,
                    ..Rule::new(handle)
                };
            }
            _ => unreachable!(),
        }
        rule.write(m, at)
    })
}

fn pending(m: &dyn GuestMemory, actor: Actor) -> R<Option<u32>> {
    let last = m.u16(actor.at + LAST_COMMAND)? as i16;
    for index in 0..=last.min(3) {
        let at = actor.at + COMMANDS + index as u32 * COMMAND_SIZE;
        if m.u16(at)? == ENTER_COMMAND {
            let handle = m.u32(at + COMMAND_VEHICLE)?;
            if handle != NONE {
                return Ok(Some(handle));
            }
        }
    }
    Ok(None)
}

pub(super) fn count(m: &dyn GuestMemory, handle: u32) -> R<u32> {
    if vehicle(m, handle)?.is_none() {
        return Ok(0);
    }
    let mut count = 0;
    for actor in actors::all(m)? {
        if pending(m, actor)? == Some(handle)
            && let Some((_, unit)) = objects::get(m, DatumHandle(actor.unit))?
            && unit.parent(m)?.map(|p| p.raw()) != Some(handle)
        {
            count += 1;
        }
    }
    Ok(count)
}

pub(super) async fn assign(cx: &Cx, handle: u32, destination: u32) -> Result<(), EntryError> {
    if destination == NONE {
        return Ok(());
    }
    let moves = cx.read(|m| {
        let mut moves = Vec::new();
        if vehicle(m, handle)?.is_some() {
            for actor in actors::all(m)? {
                if let Some((_, unit)) = objects::get(m, DatumHandle(actor.unit))?
                    && unit.parent(m)?.map(|p| p.raw()) == Some(handle)
                    && let Some(squad) = actors::migrate_to(m, actor, destination)?
                {
                    moves.push((actor, squad));
                }
            }
        }
        Ok::<_, MemFault>(moves)
    })?;
    for (actor, squad) in moves {
        actors::move_actor(cx, actor, squad).await?;
    }
    Ok(())
}

/// The normal Halo 1 call skips actors already on their way to a vehicle.
pub(super) async fn enter_if_unassigned(
    cx: &Cx,
    subject: u32,
    handle: u32,
    seats: u32,
) -> Result<(), EntryError> {
    let candidates = cx.read(|m| {
        let Some(vehicle) = vehicle(m, handle)? else {
            return Ok(Vec::new());
        };
        let position = vehicle.position(m)?;
        let mut candidates = Vec::new();
        for actor in actors::selected(m, subject)? {
            if pending(m, actor)?.is_none()
                && let Some((_, unit)) = objects::get(m, DatumHandle(actor.unit))?
            {
                let distance = squared_distance(position, unit.position(m)?);
                candidates.push((distance, actor.handle));
            }
        }
        candidates.sort_by(|(left, _), (right, _)| left.total_cmp(right));
        Ok::<_, MemFault>(candidates)
    })?;
    for (_, actor) in candidates {
        request_seat(cx, actor, handle, seats).await?;
    }
    Ok(())
}

async fn request_seat(cx: &Cx, actor: u32, handle: u32, seats: u32) -> Result<(), EntryError> {
    cx.call(
        GuestCall::new(ENTER)
            .eax(handle)
            .ecx(0x8000_0000 | u32::from(actor as u16))
            .args([seats, 0]),
    ).await?;
    Ok(())
}

pub(super) async fn tick(cx: &Cx, base: u32) -> Result<(), EntryError> {
    let rules = cx.read(|m| {
        let mut rules = Vec::new();
        let Some(start) = state::registry(m, base, REGISTRY_WORDS)? else {
            return Ok(rules);
        };
        for row in 0..u32::from(VEHICLE_ROWS) {
            let at = start + row * u32::from(VEHICLE_WIDTH) * 8;
            let handle = m.u32(at)?;
            if handle != NONE && vehicle(m, handle)?.is_some() {
                rules.push(Rule::read(m, at)?);
            }
        }
        Ok::<_, MemFault>(rules)
    })?;
    for rule in rules {
        assign(cx, rule.handle, rule.destination).await?;
        if rule.enabled && rule.radius > 0.0 {
            board(cx, rule).await?;
        }
    }
    Ok(())
}

async fn board(cx: &Cx, rule: Rule) -> Result<(), EntryError> {
    let candidates = cx.read(|m| {
        let mut candidates = Vec::new();
        let Some(body) = vehicle(m, rule.handle)? else {
            return Ok(candidates);
        };
        let position = body.position(m)?;
        let mut members = BTreeSet::new();
        for selector in rule
            .selectors
            .into_iter()
            .filter(|&selector| selector != NONE)
        {
            members.extend(
                actors::selected(m, selector)?
                    .into_iter()
                    .map(|actor| actor.handle),
            );
        }
        for actor in actors::all(m)? {
            if pending(m, actor)?.is_some() || !rule.accepts(m, actor, &members)? {
                continue;
            }
            let Some((objects::BIPED, unit)) = objects::get(m, DatumHandle(actor.unit))? else {
                continue;
            };
            if unit.parent(m)?.is_some() || unit.damage_flags(m)? & 4 != 0 {
                continue;
            }
            let distance = squared_distance(position, unit.position(m)?);
            if distance <= rule.radius * rule.radius {
                candidates.push((distance, actor.handle));
            }
        }
        // Issue requests in the same order the retail group command uses.
        candidates.sort_by(|(a, _), (b, _)| a.total_cmp(b));
        Ok::<_, MemFault>(candidates)
    })?;
    for (_, actor) in candidates {
        request_seat(cx, actor, rule.handle, NONE).await?;
    }
    Ok(())
}

fn squared_distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    a.into_iter().zip(b).map(|(a, b)| (a - b).powi(2)).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    #[test]
    fn native_seat_command_keeps_the_salted_vehicle_handle() {
        const ROW: u32 = 0x800100;
        let mut guest = FakeGuest::new();
        guest
            .put_u16(ROW + LAST_COMMAND, 2)
            .put_u16(ROW + COMMANDS, 0x29)
            .put_u16(ROW + COMMANDS + COMMAND_SIZE, ENTER_COMMAND)
            .put_u32(ROW + COMMANDS + COMMAND_SIZE + COMMAND_VEHICLE, 0xcebe_000c);
        let actor = Actor {
            at: ROW,
            handle: 0x8300_0000,
            unit: NONE,
            squad: NONE,
        };
        assert_eq!(pending(&guest, actor), Ok(Some(0xcebe_000c)));
        guest.put_u16(ROW + LAST_COMMAND, 0);
        assert_eq!(pending(&guest, actor), Ok(None));
    }

    #[test]
    fn rules_match_team_actor_type_or_squad_membership() {
        const ROW: u32 = 0x800100;
        const STATE: u32 = 0x820100;
        let mut guest = FakeGuest::new();
        guest.map(STATE, 0x100);
        let actor = Actor {
            at: ROW,
            handle: 0x8300_0000,
            unit: NONE,
            squad: NONE,
        };
        let mut rule = Rule::new(0x8400_0001);
        rule.enabled = true;
        rule.teams = 1 << 3;
        rule.kinds = 1 << 2;
        rule.selectors[0] = 4;
        rule.write(&mut guest, STATE).unwrap();
        let rule = Rule::read(&guest, STATE).unwrap();
        guest
            .put_u16(ROW + ACTOR_TYPE, 7)
            .put_u16(ROW + ACTOR_TEAM, 1);
        assert!(!rule.accepts(&guest, actor, &BTreeSet::new()).unwrap());
        assert!(
            rule.accepts(&guest, actor, &BTreeSet::from([actor.handle]))
                .unwrap()
        );
        guest.put_u16(ROW + ACTOR_TYPE, 2);
        assert!(rule.accepts(&guest, actor, &BTreeSet::new()).unwrap());
        guest
            .put_u16(ROW + ACTOR_TYPE, 7)
            .put_u16(ROW + ACTOR_TEAM, 3);
        assert!(rule.accepts(&guest, actor, &BTreeSet::new()).unwrap());
    }
}
