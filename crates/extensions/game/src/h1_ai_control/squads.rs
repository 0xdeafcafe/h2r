//! A converted encounter is a Halo 2 squad group. Persistent links are
//! stored in HaloScript globals, alongside their native squad state.

use std::collections::BTreeSet;

use guest_layout::{
    datum::DatumArray,
    entry::{Cx, EntryError, GuestCall},
    memory::{GuestMemory, MemFault},
};
use hs::h1_ai_slots::Registry;

use super::{
    AI_GROUP, AI_KIND, GROUP_COUNT, GROUP_PARENT, GROUP_SIZE, GROUPS, NONE, SQUAD_COUNT,
    SQUAD_GROUP, actors, points, squads, state,
};

const SQUAD_ARRAY: u32 = 0x0051_e9d8;
const FLAGS: u32 = 2;
const ACTIVE: u8 = 0x80;
const DELAY: u32 = 0x28;
const ACTIVATE: u32 = 0x0020_3bf0;
const LINK_COUNT: u32 = Registry::ActivationLinks.words() as u32;

type R<T> = Result<T, MemFault>;

pub(super) fn runtime(m: &dyn GuestMemory, squad: u32) -> R<Option<u32>> {
    let pool = m.u32(SQUAD_ARRAY)?;
    if pool == 0 || squad == NONE {
        return Ok(None);
    }
    DatumArray::at(pool).get_absolute(m, i32::from(squad as u16))
}

pub(super) fn expire(cx: &Cx, reference: u32) -> R<()> {
    cx.write(|m| {
        for squad in squads(m, reference)? {
            if let Some(at) = runtime(m, squad)? {
                m.set_u16(at + DELAY, 0)?;
            }
        }
        Ok(())
    })
}

fn group_of_squad(m: &dyn GuestMemory, squad: u32) -> R<Option<u16>> {
    let Some(at) = actors::squad_record(m, squad)? else {
        return Ok(None);
    };
    let group = m.u16(at + SQUAD_GROUP)?;
    Ok((group != u16::MAX).then_some(group))
}

pub(super) fn contains_squad(m: &dyn GuestMemory, group: u16, squad: u32) -> R<bool> {
    let Some(mut parent) = group_of_squad(m, squad)? else {
        return Ok(false);
    };
    let scenario = m.u32(points::SCENARIO.base)?;
    let count = m.u32(scenario + GROUP_COUNT)?;
    let groups = m.u32(scenario + GROUPS)?;
    for _ in 0..count {
        if u32::from(parent) >= count {
            break;
        }
        if parent == group {
            return Ok(true);
        }
        parent = m.u16(groups + u32::from(parent) * GROUP_SIZE + GROUP_PARENT)?;
    }
    Ok(false)
}

/// Halo 1 setters work on the containing encounter even when a squad is
/// passed. A converted encounter is the outermost group.
pub(super) fn encounter(m: &dyn GuestMemory, ai: u32) -> R<Option<u16>> {
    if ai == NONE {
        return Ok(None);
    }
    let mut index = match ai & AI_KIND {
        AI_GROUP => ai as u16,
        0 | AI_KIND => {
            let squad = if ai & AI_KIND == AI_KIND {
                (ai >> 16) & 0x3fff
            } else {
                u32::from(ai as u16)
            };
            let Some(group) = group_of_squad(m, squad)? else {
                return Ok(None);
            };
            group
        }
        _ => {
            let Some(actor) = actors::selected(m, ai)?.into_iter().next() else {
                return Ok(None);
            };
            let Some(group) = group_of_squad(m, actor.squad)? else {
                return Ok(None);
            };
            group
        }
    };
    let scenario = m.u32(points::SCENARIO.base)?;
    let count = m.u32(scenario + GROUP_COUNT)?;
    let groups = m.u32(scenario + GROUPS)?;
    for _ in 0..count {
        if u32::from(index) >= count {
            return Ok(None);
        }
        let parent = m.u16(groups + u32::from(index) * GROUP_SIZE + GROUP_PARENT)?;
        if parent == u16::MAX {
            return Ok(Some(index));
        }
        index = parent;
    }
    Ok(None)
}

pub(super) fn link(cx: &Cx, from: u32, to: u32, base: u32) -> R<()> {
    cx.write(|m| {
        let (Some(from), Some(to), Some(start)) = (
            encounter(m, from)?,
            encounter(m, to)?,
            state::registry(m, base, LINK_COUNT)?,
        ) else {
            return Ok(());
        };
        if from == to {
            return Ok(());
        }
        let mut free = None;
        let mut existing = 0;
        let link = u32::from(from) << 16 | u32::from(to);
        for row in 0..LINK_COUNT {
            let at = start + row * state::STRIDE;
            let word = m.u32(at)?;
            if word == link {
                return Ok(());
            }
            if word == NONE {
                free.get_or_insert(at);
            } else if word >> 16 == u32::from(from) {
                existing += 1;
            }
        }
        if existing < 3
            && let Some(at) = free
        {
            m.set_u32(at, link)?;
        }
        Ok(())
    })
}

fn propagate(active: &mut BTreeSet<u16>, links: &[(u16, u16)]) {
    loop {
        let mut changed = false;
        for &(from, to) in links {
            if active.contains(&to) {
                changed |= active.insert(from);
            }
        }
        if !changed {
            return;
        }
    }
}

pub(super) async fn activation_tick(cx: &Cx, base: u32) -> Result<(), EntryError> {
    let activate = cx.read(|m| {
        let Some(start) = state::registry(m, base, LINK_COUNT)? else {
            return Ok(Vec::new());
        };
        let mut links = Vec::new();
        for row in 0..LINK_COUNT {
            let word = m.u32(start + row * state::STRIDE)?;
            if word != NONE {
                links.push(((word >> 16) as u16, word as u16));
            }
        }
        if links.is_empty() {
            return Ok(Vec::new());
        }
        let scenario = m.u32(points::SCENARIO.base)?;
        let mut active = BTreeSet::new();
        let mut inactive = Vec::new();
        for squad in 0..m.u32(scenario + SQUAD_COUNT)? {
            let Some(at) = runtime(m, squad)? else {
                continue;
            };
            let Some(group) = encounter(m, squad)? else {
                continue;
            };
            if m.u8(at + FLAGS)? & ACTIVE != 0 {
                active.insert(group);
            } else {
                inactive.push((squad, group));
            }
        }
        // Determine which source encounters are held by a linked target,
        // including chains. A cycle with no active member adds nothing.
        propagate(&mut active, &links);
        let from: BTreeSet<_> = links.iter().map(|&(s, _)| s).collect();
        Ok::<_, MemFault>(
            inactive
                .into_iter()
                .filter_map(|(squad, group)| {
                    (from.contains(&group) && active.contains(&group)).then_some(squad)
                })
                .collect(),
        )
    })?;
    for squad in activate {
        cx.call(GuestCall::new(ACTIVATE).args([squad])).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_direction_and_cycles_preserve_the_initial_activity() {
        let links = [(1, 2), (2, 1), (2, 3), (4, 5), (5, 4)];
        let mut active = BTreeSet::from([3]);
        propagate(&mut active, &links);
        assert_eq!(active, BTreeSet::from([1, 2, 3]));
        active.clear();
        propagate(&mut active, &links);
        assert!(active.is_empty());
    }
}
