//! The `rules`, `variant` and `playlist` builders: game type scripts, the
//! built-in list entries and the default hopper's playlists as package
//! items (docs/editor/studio-redesign.md, "Contents";
//! docs/game/rules-variants.md, "Built-in list entries").
//!
//! None of them adds a tag. A script is read by the game when a record
//! names it, so what the builders do is the part that needs no map: the
//! script must compile and fit the tick budget. Variant items carry a list
//! script or a portable record, and playlist items carry a fragment or a
//! hopper image. [`script_registry`]
//! and [`hopper_fragments`] are what the game and the server read from a set.
use std::collections::BTreeSet;

use content::package::Kind;
use content::playlist::Fragment;
use rules::builtin::{Registry, package_sources};

use crate::build::{Cx, Failure, Loaded};
use crate::builders::Job;

/// An item's text: its source, then its `inputs`.
fn text(job: &Job<'_>) -> Result<String, Failure> {
    let mut out = String::new();
    let files = job.item.source.iter().chain(&job.item.inputs);
    for name in files {
        let bytes = if name.as_str() == job.item.source.as_deref().unwrap_or("") {
            Some(job.source)
        } else {
            job.files.get(name).map(Vec::as_slice)
        };
        let bytes = bytes.ok_or_else(|| Failure::from(format!("{name}: not in the package")))?;
        out.push_str(std::str::from_utf8(bytes).map_err(|e| format!("{name}: not text: {e}"))?);
    }
    Ok(out)
}

fn no_tag(job: &Job<'_>) -> Result<(), Failure> {
    match job.item.tag {
        Some(_) => Err("this item adds no tag: drop `tag`".into()),
        None => Ok(()),
    }
}

/// The script must compile (the line is the script's own) and fit the
/// tick budget.
fn script(job: &Job<'_>) -> Result<rules::Ir, Failure> {
    no_tag(job)?;
    let src = text(job)?;
    let ir = rules::compile(&src).map_err(|e| Failure::at(e.line as usize, e.message))?;
    let report = rules::verify(&ir).map_err(|e| Failure::from(e.to_string()))?;
    if !report.fits() {
        return Err(format!(
            "the script's worst tick is {} against the budget",
            report.worst_tick
        )
        .into());
    }
    Ok(ir)
}

pub fn check_rules(job: &Job<'_>) -> Result<(), Failure> {
    // A source ending `.firefight.json` is a wave set, not a script.
    if crate::firefight_sets::is_set(job.item) {
        return crate::firefight_sets::check(job);
    }
    script(job).map(drop)
}

pub fn check_variant(job: &Job<'_>) -> Result<(), Failure> {
    if job.item.source.as_deref().is_some_and(|name| name.ends_with(".h2variant")) {
        no_tag(job)?;
        return variants::portable::import(&text(job)?)
            .map(drop)
            .map_err(|e| e.to_string().into());
    }
    let ir = script(job)?;
    // A list entry is the record of the script, on a template the game's
    // loader can construct (`variants::portable::record_for`).
    variants::portable::record_for(&ir, &[])
        .map(drop)
        .map_err(|e| format!("the script makes no record: {e}").into())
}

pub fn check_playlist(job: &Job<'_>) -> Result<(), Failure> {
    no_tag(job)?;
    if job.item.source.as_deref().is_some_and(|name| name.ends_with(".dat")) {
        use live_proto::hopper::{HopperFile, INFLATED_SIZE};
        let file = if job.source.len() == INFLATED_SIZE {
            HopperFile::from_image(job.source)
        } else {
            HopperFile::parse_legacy(job.source)
        };
        return file.map(drop).map_err(|e| e.to_string().into());
    }
    Fragment::parse(&text(job)?)
        .map(drop)
        .map_err(|e| e.to_string().into())
}

/// Changes nothing in the map: the game and the server read the item.
/// The source is read again so a build stops on a bad one.
pub fn apply_rules(cx: &mut Cx<'_>, job: &Job<'_>) -> Result<(), Failure> {
    if crate::firefight_sets::is_set(job.item) {
        return crate::firefight_sets::apply(cx, job);
    }
    check_rules(job)
}

pub fn apply_variant(_cx: &mut Cx<'_>, job: &Job<'_>) -> Result<(), Failure> {
    check_variant(job)
}

pub fn apply_playlist(_cx: &mut Cx<'_>, job: &Job<'_>) -> Result<(), Failure> {
    check_playlist(job)
}

/// The scripts of `packages`' `rules` and `variant` items, in the set's
/// order with each item's order inside it. A `rules` item of `core` is a
/// retail type's game. An item that does not read is left out and named in
/// [`Registry::problems`] (`check_package` names it too).
pub fn script_registry(packages: &[Loaded]) -> Registry {
    let mut sources = Vec::new();
    let mut problems = Vec::new();
    for loaded in packages {
        let (mut s, mut p) = package_sources(&loaded.package.manifest, |name| {
            loaded.files.get(name).cloned()
        });
        sources.append(&mut s);
        problems.append(&mut p);
    }
    let mut registry = Registry::from_sources(sources);
    problems.append(&mut registry.problems);
    registry.problems = problems;
    registry
}

/// The hopper fragments of `packages`' `playlist` items, by package id, in
/// the set's order. A fragment that does not read, or that names a script
/// the set has no matchmaking script for, is left out (`check_package`
/// names it); [`Fragment::variants`] says which scripts a fragment needs.
pub fn hopper_fragments(packages: &[Loaded]) -> Vec<(String, Fragment)> {
    let have: BTreeSet<String> = script_registry(packages)
        .matchmaking()
        .map(|s| s.key.clone())
        .collect();
    let mut out = Vec::new();
    for loaded in packages {
        let manifest = &loaded.package.manifest;
        for item in manifest.items.iter().filter(|i| i.kind == Kind::Playlist) {
            let Some(bytes) = item.source.as_ref().and_then(|s| loaded.files.get(s)) else {
                continue;
            };
            let Ok(text) = std::str::from_utf8(bytes) else {
                continue;
            };
            let Ok(fragment) = Fragment::parse(text) else {
                continue;
            };
            if fragment.variants().iter().all(|v| have.contains(v)) {
                out.push((manifest.package.id.clone(), fragment));
            }
        }
    }
    out
}
