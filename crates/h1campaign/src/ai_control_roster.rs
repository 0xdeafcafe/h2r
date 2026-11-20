//! Preserve Halo 1 migration identities within the roster lane's native pool.
//! An encounter with unnamed overflow destinations gets one squad to carry
//! their native orders. Live and script-addressable squads retain native IDs.

use std::collections::{BTreeMap, BTreeSet};

use h1ai::h1::H1Ai;
use h1scripts::aicontrol::RetainedSubjects;

use crate::{Result, invalid};

pub(crate) struct Retained {
    pub generated_starts: BTreeSet<usize>,
    pub overflow_hosts: BTreeMap<u16, u16>,
    pub squad_indices: Vec<Vec<Option<u16>>>,
}

pub(crate) fn prepare(
    source: &H1Ai,
    roster: &mut H1Ai,
    generated: &std::collections::HashSet<usize>,
    subjects: &RetainedSubjects,
    requests: &[h1ai::control::Request],
    group_count: usize,
) -> Result<Retained> {
    let fits = |count| engine::campaign_squad_pool::holds(count, group_count);
    let name = |encounter: &str, squad: &str| format!("{encounter}/{squad}").to_ascii_lowercase();
    let maneuvers = h1ai::control::maneuver_targets(source, requests);
    let mut selected = BTreeMap::new();
    let mut output = 0;
    // The roster selector retains source order. Resolve before conversion,
    // while full source records can distinguish duplicated or long names.
    for (ei, (e, original)) in roster.encounters.iter_mut().zip(&source.encounters).enumerate() {
        let mut next = 0;
        for mut squad in e.squads.drain(..) {
            let generated = generated.contains(&output);
            let starts = generated.then(|| std::mem::take(&mut squad.starting_locations));
            let candidates = &original.squads[next..];
            let offset = candidates.iter().position(|original| *original == squad)
                .or_else(|| candidates.iter().position(|original| original.name == squad.name))
                .ok_or_else(|| invalid(format!("cannot recover source index for {}/{}", e.name, squad.name)))?;
            let source_index = next + offset;
            next = source_index + 1;
            if let Some(starts) = starts {
                squad.starting_locations = starts;
            }
            selected.insert((ei, source_index), (squad, generated));
            output += 1;
        }
    }
    let mut keep = BTreeSet::new();
    let mut optional = Vec::new();
    let mut automatic_rows = BTreeMap::<_, Vec<_>>::new();
    for (ei, encounter) in source.encounters.iter().enumerate() {
        for (si, squad) in encounter.squads.iter().enumerate() {
            let key = name(&encounter.name, &squad.name);
            let existing = selected.get(&(ei, si));
            if squad.flags & 0x20 != 0 && subjects.follow_encounters.as_ref()
                .is_none_or(|groups| groups.contains(&(ei as u16)))
            {
                automatic_rows.entry(ei).or_default().push(si);
            }
            if !squad.starting_locations.is_empty()
                || existing.is_some_and(|(q, _)| !q.starting_locations.is_empty())
            {
                keep.insert((ei, si));
            } else if maneuvers.contains(&key) {
                optional.push((0, ei, si));
            } else if subjects.migration_destinations.contains(&(ei as u16, si as u8)) {
                optional.push((1, ei, si));
            } else if existing.is_some() {
                optional.push((2, ei, si));
            }
        }
    }
    if !fits(keep.len()) {
        return Err(invalid("native AI roster cannot hold the live squads"));
    }
    optional.sort_unstable();
    for &(priority, ei, si) in &optional {
        if fits(keep.len() + 1) {
            keep.insert((ei, si));
        } else if priority < 2 {
            let encounter = &source.encounters[ei];
            return Err(invalid(format!(
                "no native slot for AI destination {}/{}",
                encounter.name, encounter.squads[si].name,
            )));
        }
    }

    let priority = |ei, si| optional.iter().find_map(|&(priority, e, s)| {
        ((e, s) == (ei, si)).then_some(priority)
    });
    let mut carriers = BTreeMap::new();
    let mut hidden = BTreeSet::new();
    for (&ei, rows) in &automatic_rows {
        let Some(&overflow) = rows.iter().find(|&&si| !keep.contains(&(ei, si))) else {
            continue;
        };
        if fits(keep.len() + hidden.len() + 1) {
            let si = rows.iter().copied().find(|&si| {
                !keep.contains(&(ei, si)) && priority(ei, si).is_none()
            }).unwrap_or(overflow);
            hidden.insert((ei, si));
            carriers.insert(ei, si);
            continue;
        }
        // At capacity, use a member of this encounter. An automatically
        // migrating member is already part of the population that moves as
        // one cohort. Its own order remains a separate candidate.
        if let Some(si) = rows.iter().copied().find(|&si| keep.contains(&(ei, si)))
            .or_else(|| {
                keep.range((ei, 0)..=(ei, usize::MAX))
                    .find_map(|&(_, si)| priority(ei, si).is_some().then_some(si))
            })
        {
            carriers.insert(ei, si);
            continue;
        }
        // An initially empty encounter can still have migration destinations.
        // Give it a carrier before retaining an unrelated, unnamed empty row.
        let replace = optional.iter().rev().find(|&&(priority, e, s)| {
            priority == 2
                && keep.contains(&(e, s))
                && carriers.get(&e) != Some(&s)
                && !automatic_rows.get(&e).is_some_and(|rows| rows.contains(&s))
        });
        if let Some(&(_, e, s)) = replace {
            keep.remove(&(e, s));
            hidden.insert((ei, overflow));
            carriers.insert(ei, overflow);
        }
        // Encounters with no retained members have no actors to migrate.
        // The logical routes are still compiled, without a physical host.
    }
    let mut output = 0;
    let mut generated_starts = BTreeSet::new();
    let mut overflow_hosts = BTreeMap::new();
    let mut squad_indices = Vec::with_capacity(source.encounters.len());
    for (ei, (source, output_encounter)) in source.encounters.iter().zip(&mut roster.encounters).enumerate() {
        let mut indices = vec![None; source.squads.len()];
        for (si, source_squad) in source.squads.iter().enumerate() {
            let host = carriers.get(&ei) == Some(&si);
            if !host && !keep.contains(&(ei, si)) {
                continue;
            }
            let (mut squad, generated) = selected.remove(&(ei, si))
                .unwrap_or_else(|| (source_squad.clone(), false));
            if host {
                overflow_hosts.insert(ei as u16, output as u16);
            }
            if hidden.contains(&(ei, si)) {
                // Hide the carrier from named script lookups and keep the
                // source row in the virtual destination table.
                squad.name = format!("h1_auto_{ei}");
                squad.platoon = -1;
                squad.flags = 0;
                squad.starting_locations.clear();
            } else {
                indices[si] = Some(output as u16);
                if generated {
                    generated_starts.insert(output);
                }
            }
            output_encounter.squads.push(squad);
            output += 1;
        }
        squad_indices.push(indices);
    }
    Ok(Retained { generated_starts, overflow_hosts, squad_indices })
}
