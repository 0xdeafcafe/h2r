//! The `map-variant` builder: a Forge map variant as a package item
//! (docs/game/forge-map-variants.md, "A map variant in a package";
//! docs/editor/studio-redesign.md, "Contents").
//!
//! The item's source is a `.h2mapvariant` file, the document the variant
//! store holds (`forge_doc::ForgeDoc` in its play form). It adds no tag
//! and changes no map at build: the game lists it with the variants a
//! player saved and builds its map when a lobby picks it. [`map_variants`]
//! is what the game reads from a set.
use content::package::Kind;
use forge_doc::{ForgeDoc, Limits};

use crate::build::{Cx, Failure, Loaded};
use crate::builders::Job;

/// The extension of a map variant file.
pub const VARIANT_EXT: &str = "h2mapvariant";

/// A `map-variant` item of a set: the package it is in, its source's
/// path and stem, and the file's bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapVariant {
    pub package: String,
    /// The item's number in its package's manifest.
    pub item: usize,
    /// The source's path in the package.
    pub source: String,
    /// The source's file name without its extension.
    pub stem: String,
    pub bytes: Vec<u8>,
}

/// The stem of a `map-variant` source: its file name without
/// `.h2mapvariant`, a plain name (letters, digits, `_` and `-`).
pub fn stem_of_source(source: &str) -> Option<&str> {
    let file = source.rsplit('/').next()?;
    let stem = file.strip_suffix(VARIANT_EXT)?.strip_suffix('.')?;
    content::manifest::is_map_name(stem).then_some(stem)
}

/// The document a `map-variant` source holds, or why it is not one: it
/// reads as a Forge document, is of the variant tier and has a variant's
/// name.
pub fn read(bytes: &[u8]) -> Result<ForgeDoc, String> {
    let doc = ForgeDoc::from_bytes(bytes, &Limits::default()).map_err(|e| e.to_string())?;
    doc.check_variant().map_err(|e| e.to_string())?;
    Ok(doc)
}

pub fn check(job: &Job<'_>) -> Result<(), Failure> {
    if job.item.tag.is_some() {
        return Err("this item adds no tag: drop `tag`".into());
    }
    let source = job.item.source.as_deref().unwrap_or("");
    if stem_of_source(source).is_none() {
        return Err(format!(
            "{source}: a map variant's source is <name>.{VARIANT_EXT}, the name of letters, digits, `_` and `-`"
        )
        .into());
    }
    read(job.source).map(drop).map_err(Failure::from)
}

/// Changes nothing in the map: the game reads the item. The source is
/// read again so a build stops on a bad one.
pub fn apply(_cx: &mut Cx<'_>, job: &Job<'_>) -> Result<(), Failure> {
    check(job)
}

/// The `map-variant` items of `packages`, in the set's order with each
/// item's order inside it. An item whose source is missing or has no
/// plain stem is left out (`check_package` names it); a second item of a
/// package with a stem the package already has is left out too, so a
/// package and a stem name one variant.
pub fn map_variants(packages: &[Loaded]) -> Vec<MapVariant> {
    let mut out: Vec<MapVariant> = Vec::new();
    for loaded in packages {
        let manifest = &loaded.package.manifest;
        let package = &manifest.package.id;
        for (n, item) in manifest.items.iter().enumerate() {
            if item.kind != Kind::MapVariant {
                continue;
            }
            let Some(source) = item.source.as_deref() else {
                continue;
            };
            let (Some(stem), Some(bytes)) = (stem_of_source(source), loaded.files.get(source))
            else {
                continue;
            };
            if out.iter().any(|v| v.package == *package && v.stem == stem) {
                continue;
            }
            out.push(MapVariant {
                package: package.clone(),
                item: n,
                source: source.to_owned(),
                stem: stem.to_owned(),
                bytes: bytes.clone(),
            });
        }
    }
    out
}
