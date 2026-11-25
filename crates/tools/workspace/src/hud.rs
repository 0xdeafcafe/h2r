//! The `hud` builder: a HUD of the HUD kit as YAML (docs/editor/studio-redesign.md,
//! "Contents"; docs/game/hud-kit.md, "HUDs as data").
//!
//! A `hud` item adds no tag. Its records are compiled by the game against
//! the map it loads (`hud_kit::install`), so the builder's work is the part
//! that needs no map: the source must read as a [`Hud`] (every key known,
//! every element complete, ids unique). [`huds`] is what the game reads
//! from a set.
use content::package::Kind;
use hud_kit_core::data::parse;
use hud_kit_core::model::Hud;

use crate::build::{Cx, Failure, Loaded};
use crate::builders::Job;

fn read(job: &Job<'_>) -> Result<Hud, Failure> {
    if job.item.tag.is_some() {
        return Err("a hud item adds no tag: drop `tag`".into());
    }
    let text = std::str::from_utf8(job.source).map_err(|e| format!("not text: {e}"))?;
    parse(text).map_err(|e| Failure {
        line: e.line,
        why: e.why,
    })
}

pub fn check(job: &Job<'_>) -> Result<(), Failure> {
    read(job).map(drop)
}

/// Changes nothing in the map: the HUD's records are made at the game's
/// install. The source is read again so a build stops on a bad one.
pub fn apply(_cx: &mut Cx<'_>, job: &Job<'_>) -> Result<(), Failure> {
    read(job).map(drop)
}

/// The HUDs of `packages`' `hud` items, in the set's order with each
/// item's order inside it, by package id. A later HUD of the same name
/// replaces an earlier one at install; an item that does not read is left
/// out (`check_package` names it).
pub fn huds(packages: &[Loaded]) -> Vec<(String, Hud)> {
    let mut out = Vec::new();
    for loaded in packages {
        let manifest = &loaded.package.manifest;
        for item in manifest.items.iter().filter(|i| i.kind == Kind::Hud) {
            let Some(bytes) = item.source.as_ref().and_then(|s| loaded.files.get(s)) else {
                continue;
            };
            let job = Job {
                item,
                source: bytes,
                files: &loaded.files,
            };
            if let Ok(hud) = read(&job) {
                out.push((manifest.package.id.clone(), hud));
            }
        }
    }
    out
}
