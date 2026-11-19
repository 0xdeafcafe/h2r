//! Seed the squad's unique voice when its live actors enter the cast pool.

use std::collections::BTreeMap;

use super::{cast::*, *};
use hs::h1_conversation::ACTOR_CAPACITY;

const SQUADS: u32 = 0x0051_e9d8;
const FIRST_ACTOR: u32 = 0x68;
const NEXT_ACTOR: u32 = 0x20;

#[derive(Default)]
struct Encounter {
    living: usize,
    unique_voices: usize,
}

struct Speaker {
    actor: u32,
    unit: u32,
    profile_cell: u32,
}

fn unique_voice<E>(
    mode: u32,
    encounter: &Encounter,
    draw: impl FnOnce() -> Result<i16, E>,
) -> Result<i16, E> {
    match mode {
        0 if (encounter.unique_voices == 0 && encounter.living >= 4)
            || (encounter.unique_voices == 1 && encounter.living >= 10) =>
        {
            draw()
        }
        2 => draw(),
        3 => Ok(100),
        4 => Ok(101),
        _ => Ok(0),
    }
}

pub(super) fn refresh(
    m: &mut dyn GuestMemory,
    first: u32,
    count: u32,
    profiles: u32,
) -> Result<(), MemFault> {
    let squads = m.u32(SQUADS)?;
    if squads == 0 {
        return Ok(());
    }
    let squads = DatumArray::at(squads);
    let mut encounters = BTreeMap::<u32, Encounter>::new();
    let mut fresh = Vec::new();
    for offset in 0..count.min(GLOBAL_CAPACITY) {
        let Some(meta) = global(m, first.saturating_add(offset), LONG)? else {
            break;
        };
        let meta = m.u32(meta)?;
        let rule = meta >> 24;
        let encounter = (meta >> 12) & 0xfff;
        let Some(squad) = squads.get_absolute(m, (meta & 0xfff) as i32)? else {
            continue;
        };
        let state = encounters.entry(encounter).or_default();
        let mut cohort = Vec::new();
        let mut handle = m.u32(squad + FIRST_ACTOR)?;
        for _ in 0..ACTOR_CAPACITY {
            let Some(actor) = datum(m, ACTORS, handle)? else {
                break;
            };
            let unit = m.u32(actor + ACTOR_UNIT)?;
            if Unit::read(m, unit)?.is_some()
                && let Some(slot) = voice_slot(m, profiles, unit)?
            {
                if let Some(profile) = profile(m.u32(slot)?, unit) {
                    state.living += 1;
                    state.unique_voices += usize::from(profile.leader);
                } else {
                    cohort.push(Speaker {
                        actor: handle,
                        unit,
                        profile_cell: slot,
                    });
                }
            }
            handle = m.u32(actor + NEXT_ACTOR)?;
        }
        if !cohort.is_empty() {
            fresh.push((encounter, rule, cohort));
        }
    }
    // The runtime squad chain is newest first. Seed the first actor the
    // squad spawned, as the Halo 1 placement loop at 0x0013e607 does.
    for (encounter, rule, cohort) in fresh {
        let state = encounters.entry(encounter).or_default();
        state.living += cohort.len();
        let oldest = cohort
            .iter()
            .min_by_key(|speaker| speaker.actor >> 16)
            .unwrap()
            .actor;
        let distinct = unique_voice(rule, state, || {
            Ok::<_, MemFault>(100 + draw_voice(m, 2)? as i16)
        })?;
        for speaker in cohort {
            let voice = if speaker.actor == oldest { distinct } else { 0 };
            store_profile(m, speaker.profile_cell, speaker.unit, voice, voice >= 100)?;
        }
        state.unique_voices += usize::from(distinct >= 100);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_speaker_rules_use_source_thresholds_and_fixed_personas() {
        let mut e = Encounter {
            living: 3,
            unique_voices: 0,
        };
        assert_eq!(unique_voice::<()>(0, &e, || panic!()), Ok(0));
        e.living = 4;
        assert_eq!(unique_voice::<()>(0, &e, || Ok(101)), Ok(101));
        e.unique_voices = 1;
        assert_eq!(unique_voice::<()>(0, &e, || panic!()), Ok(0));
        e.living = 10;
        assert_eq!(unique_voice::<()>(0, &e, || Ok(100)), Ok(100));
        assert_eq!(unique_voice::<()>(1, &e, || panic!()), Ok(0));
        assert_eq!(unique_voice::<()>(2, &e, || Ok(101)), Ok(101));
        assert_eq!(unique_voice::<()>(3, &e, || panic!()), Ok(100));
        assert_eq!(unique_voice::<()>(4, &e, || panic!()), Ok(101));
    }
}
