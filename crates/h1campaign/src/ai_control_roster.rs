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
}

pub(crate) fn prepare(
    source: &H1Ai,
    roster: &mut H1Ai,
    generated: &std::collections::HashSet<usize>,
    subjects: &RetainedSubjects,
    requests: &[h1ai::control::Request],
) -> Result<Retained> {
    let group_count = roster.encounters.len();
    let fits = |count| engine::squad_pool::partition(count, group_count).is_some();
    let name = |encounter: &str, squad: &str| format!("{encounter}/{squad}").to_ascii_lowercase();
    let maneuvers = h1ai::control::maneuver_targets(source, requests);
    let automatic =
        h1ai::control::automatic_migration_targets(source, subjects.follow_encounters.as_ref());

    let mut selected = BTreeMap::new();
    let mut output = 0;
    for (ei, e) in roster.encounters.iter_mut().enumerate() {
        for q in e.squads.drain(..) {
            selected.insert((ei, q.name.to_ascii_lowercase()), (q, generated.contains(&output)));
            output += 1;
        }
    }
    let mut keep = BTreeSet::new();
    let mut optional = Vec::new();
    let mut automatic_rows = BTreeMap::<_, Vec<_>>::new();
    for (ei, encounter) in source.encounters.iter().enumerate() {
        for (si, squad) in encounter.squads.iter().enumerate() {
            let key = name(&encounter.name, &squad.name);
            let existing = selected.get(&(ei, squad.name.to_ascii_lowercase()));
            if automatic.contains(&key) {
                automatic_rows.entry(ei).or_default().push(si);
            }
            if existing.is_some_and(|(q, _)| !q.starting_locations.is_empty()) {
                keep.insert((ei, si));
            } else if maneuvers.contains(&key) {
                optional.push((0, ei, si));
            } else if subjects.migration_destinations.contains(&key) {
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
    for &(_, ei, si) in &optional {
        if fits(keep.len() + 1) {
            keep.insert((ei, si));
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
            .or_else(|| keep.range((ei, 0)..=(ei, usize::MAX)).next().map(|&(_, si)| si))
        {
            carriers.insert(ei, si);
        }
    }
    let mut output = 0;
    let mut generated_starts = BTreeSet::new();
    let mut overflow_hosts = BTreeMap::new();
    for (ei, (source, output_encounter)) in source.encounters.iter().zip(&mut roster.encounters).enumerate() {
        for (si, source_squad) in source.squads.iter().enumerate() {
            let host = carriers.get(&ei) == Some(&si);
            if !host && !keep.contains(&(ei, si)) {
                continue;
            }
            let (mut squad, generated) = selected.remove(&(ei, source_squad.name.to_ascii_lowercase()))
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
            } else if generated {
                generated_starts.insert(output);
            }
            output_encounter.squads.push(squad);
            output += 1;
        }
    }
    Ok(Retained { generated_starts, overflow_hosts })
}
