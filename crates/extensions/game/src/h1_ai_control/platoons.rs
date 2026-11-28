//! Evaluate Halo 1 platoon rules against the native Halo 2 squad summaries.

use std::collections::BTreeMap;

use guest_layout::{entry::{Cx, EntryError, GuestCall}, memory::{GuestMemory, MemFault}};
use hs::h1_ai_slots::{MAX_PLATOON_TEXT, PlatoonRule, PlatoonTrigger, decode_words};

use super::{NONE, actors, speech, squad_control, state, uncover};

const SQUAD_TOTAL: u32 = 8;
const SQUAD_LIVING: u32 = 0x0a;
const SQUAD_AVERAGE_VITALITY: u32 = 0x10;
const SQUAD_CURRENT_ORDER: u32 = 0x2a;
const SET_ORDERS: u32 = 0x0027_5cb0;

type R<T> = Result<T, MemFault>;

#[derive(Clone, Copy, Default)]
struct Counts {
    living: u32,
    total: u32,
    vitality: f32,
}

impl Counts {
    fn matches(self, condition: u16) -> bool {
        if self.total == 0 {
            return false;
        }
        let mean = (self.vitality / self.total as f32 - 0.001).max(0.0);
        match condition {
            1 => mean < 0.75,
            2 => mean < 0.5,
            3 => mean < 0.25,
            4 => self.living < self.total,
            5 => self.living * 4 / 3 <= self.total,
            6 => self.living * 2 <= self.total,
            7 => self.living * 4 <= self.total,
            8 => self.living <= 1,
            9 => self.living == 0,
            _ => false,
        }
    }
}

struct Squad {
    counts: Counts,
    order: u16,
}

fn count(squads: &BTreeMap<u16, Squad>, ids: &[u16]) -> Counts {
    let mut counts = Counts::default();
    for squad in ids.iter().filter_map(|id| squads.get(id)) {
        counts.total += squad.counts.total;
        counts.living += squad.counts.living;
        counts.vitality += squad.counts.vitality;
    }
    counts
}

fn matches(squads: &BTreeMap<u16, Squad>, trigger: &PlatoonTrigger) -> bool {
    count(squads, &trigger.squads).matches(trigger.condition)
}

pub(super) fn set(cx: &Cx, argument: u32, enabled: bool) -> R<()> {
    if matches!(argument, 0 | NONE) {
        return Ok(());
    }
    cx.write(|m| {
        let encoded = m.cstr(argument, 256 * 4)?;
        let Some(indices) = decode_words(&encoded) else {
            return Ok(());
        };
        for index in indices {
            if let Some(at) = state::registry(m, u32::from(index), 1)? {
                m.set_u32(at, u32::from(enabled))?;
            }
        }
        Ok(())
    })
}

/// `offset` is the table's place in the scenario's script strings.
fn rules(m: &dyn GuestMemory, offset: u32) -> R<Vec<PlatoonRule>> {
    let Some((at, remaining)) = state::script_strings(m, offset)? else {
        return Ok(Vec::new());
    };
    if remaining < 5 {
        return Ok(Vec::new());
    }
    let prefix = m.bytes(at, 4)?;
    let Some(len) = decode_words(&prefix).and_then(|words| words.first().copied()) else {
        return Ok(Vec::new());
    };
    let len = usize::from(len);
    if len > MAX_PLATOON_TEXT || 4 + len as u32 >= remaining || m.u8(at + 4 + len as u32)? != 0 {
        return Ok(Vec::new());
    }
    let encoded = m.bytes(at + 4, len)?;
    Ok(PlatoonRule::decode_all(&encoded).unwrap_or_default())
}

pub(super) async fn tick(cx: &Cx, argument: u32) -> Result<(), EntryError> {
    let (orders, moves) = cx.write(|m| {
        let rules = rules(m, argument)?;
        if rules.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        let mut squads = BTreeMap::new();
        for rule in &rules {
            let ids = rule
                .members
                .iter()
                .chain(rule.change.iter().flat_map(|trigger| &trigger.squads))
                .chain(rule.maneuver.iter().flat_map(|trigger| &trigger.squads));
            for &id in ids {
                if let std::collections::btree_map::Entry::Vacant(entry) = squads.entry(id)
                    && let Some(at) = squad_control::runtime(m, u32::from(id))?
                {
                    let total = u32::from(m.u16(at + SQUAD_TOTAL)?);
                    let living = u32::from(m.u16(at + SQUAD_LIVING)?);
                    let vitality = m.f32(at + SQUAD_AVERAGE_VITALITY)? * total as f32;
                    entry.insert(Squad {
                        counts: Counts {
                            total,
                            living,
                            vitality,
                        },
                        order: m.u16(at + SQUAD_CURRENT_ORDER)?,
                    });
                }
            }
        }
        let mut orders = Vec::new();
        let mut routes = BTreeMap::new();
        for rule in rules {
            let has_actors = count(&squads, &rule.members).living > 0;
            let mut armed = false;
            if let Some(index) = rule.latch
                && let Some(at) = state::registry(m, u32::from(index), 1)?
            {
                armed = m.u32(at)? != 0;
                if has_actors
                    && !armed
                    && rule.maneuver.as_ref().is_some_and(|t| matches(&squads, t))
                {
                    armed = true;
                    m.set_u32(at, 1)?;
                }
            }
            let enabled = match rule.enabled_by {
                Some(index) => state::boolean(m, index)?,
                None => true,
            };
            if armed && enabled {
                routes.extend(
                    rule.routes
                        .into_iter()
                        .map(|(from, to)| (u32::from(from), (u32::from(to), rule.maneuver_flags))),
                );
            } else if has_actors && rule.change.as_ref().is_some_and(|t| matches(&squads, t)) {
                orders.extend(rule.orders.into_iter().filter(|&(id, order)| {
                    squads.get(&id).is_some_and(|entry| entry.order != order)
                }));
            }
        }
        let mut moves = Vec::new();
        if !routes.is_empty() {
            for actor in actors::all(m)? {
                if let Some(&(to, flags)) = routes.get(&actor.squad) {
                    moves.push((actor, to, flags));
                }
            }
        }
        Ok::<_, MemFault>((orders, moves))
    })?;
    for (squad, order) in orders {
        cx.call(
            GuestCall::new(SET_ORDERS)
                .eax(u32::from(squad))
                .ecx(u32::from(order)),
        )
        .await?;
    }
    for (actor, to, flags) in moves {
        actors::move_actor(cx, actor, to).await?;
        if flags & 1 != 0 {
            uncover::maneuver(cx, actor).await?;
        }
        speech::maneuver(cx, actor.unit, flags & 2 != 0).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retail_thresholds_use_initial_count_and_weighted_vitality() {
        assert!(!Counts::default().matches(9));
        let survivors = Counts {
            living: 2,
            total: 4,
            vitality: 2.0,
        };
        assert!(survivors.matches(2));
        assert!(!survivors.matches(3));
        assert!(survivors.matches(4));
        assert!(survivors.matches(6));
        assert!(!survivors.matches(7));
        assert!(!survivors.matches(8));
        assert!(!survivors.matches(9));
        let one = Counts {
            living: 1,
            ..survivors
        };
        assert!(one.matches(7));
        assert!(one.matches(8));
        let wounded = Counts {
            living: 3,
            vitality: 1.5,
            ..survivors
        };
        assert!(wounded.matches(2));
        assert!(wounded.matches(5));
        assert!(!wounded.matches(6));
    }
}
