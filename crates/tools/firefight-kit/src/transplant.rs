//! Stage two of the kit: character classes copied from donor maps (the
//! player's own campaign maps) into the kit map.
//!
//! A class is a `char` tag of a donor and everything it references,
//! copied by `tags::schema::transplant` with its sounds' entries merged
//! into the base's `ugh!` gestalt (`h1_assets::sounds::transplant_from`).
//! A tag the base already has (same group and name) is reused, so a class
//! costs only what the base and the classes before it lack.
//!
//! Each class costs tag memory, and a multiplayer map has little
//! headroom. The class list is in keep order: the first class is the
//! last to go. The builder (`crate::build_with_classes`) either refuses a
//! build that does not fit and reports every class's cost, or, with
//! [`Classes::cut_to_fit`], drops classes from the end of the list until
//! it fits and reports what it cut. The palette rows follow the list, one
//! after the authored Elite.
use std::io::Cursor;

use anyhow::{Context, Result, anyhow, bail};
use h1_assets::sounds::{
    self,
    gestalt::{self, SoundGestalt},
};
use firefight::Faction;
use map::{
    FourCC, Map, TagId, TagQuery,
    build::{TagPayload, TagSet},
};
use serde::Serialize;
use tags::schema::{
    Schema,
    transplant::{Refused, Source},
};

const CHAR: FourCC = FourCC(*b"char");
const SND: FourCC = FourCC(*b"snd!");
/// A tag index entry.
const INDEX_ENTRY: usize = 16;

/// A donor map: its label and file bytes.
#[derive(Clone, Copy, Debug)]
pub struct Donor<'a> {
    pub name: &'a str,
    pub bytes: &'a [u8],
}

/// One class: the `char` tag at `path` in donor number `donor`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassSpec {
    /// The class's name in costs and errors.
    pub name: String,
    /// Index into [`Classes::donors`].
    pub donor: usize,
    /// The `char` tag's name in the donor.
    pub path: String,
    /// Who the class fights for: the kit writes it with the palette row
    /// (`ai::class_squads`) and the game places the class's squads on that
    /// faction's team.
    pub faction: Faction,
}

/// The classes a kit build brings in.
#[derive(Clone, Debug)]
pub struct Classes<'a> {
    /// Layouts of the tags to copy (`Schema::builtin()` for retail maps).
    pub schema: &'a Schema,
    pub donors: Vec<Donor<'a>>,
    /// Keep order: the last class is the first cut.
    pub specs: Vec<ClassSpec>,
    /// Drop classes from the end until the build fits, instead of
    /// refusing it.
    pub cut_to_fit: bool,
}

/// What one class added to the kit map.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ClassCost {
    pub name: String,
    /// Tags copied in (those the map already had are not counted).
    pub tags: usize,
    /// `snd!` tags whose gestalt entries came along.
    pub sounds: usize,
    /// Tag memory of the copied tags and their index entries, without
    /// alignment and without the gestalt's growth. The build's tag growth
    /// is the exact total.
    pub bytes: usize,
    /// Local raw resources copied (file bytes, not tag memory).
    pub raw: u64,
}

/// The classes added to a set.
#[derive(Default)]
pub(crate) struct Added {
    /// The `char` tags, in palette order.
    pub ids: Vec<TagId>,
    /// Each class's faction, in palette order.
    pub factions: Vec<Faction>,
    pub costs: Vec<ClassCost>,
}

/// A donor read once.
struct Read<'a> {
    map: Map<Cursor<&'a [u8]>>,
    source: Source<'a>,
    gestalt: Option<SoundGestalt>,
}

fn read<'a>(schema: &'a Schema, donor: Donor<'a>) -> Result<Read<'a>> {
    let mut map = Map::from_reader(Cursor::new(donor.bytes))
        .map_err(|e| anyhow!("donor {}: {e}", donor.name))?;
    let source = Source::new(schema, &mut map).map_err(|e| anyhow!("donor {}: {e}", donor.name))?;
    let gestalt = donor_gestalt(&mut map);
    Ok(Read {
        map,
        source,
        gestalt,
    })
}

/// The donor's own `ugh!`, when it has one.
fn donor_gestalt(map: &mut Map<Cursor<&[u8]>>) -> Option<SoundGestalt> {
    let tag = map
        .tags()
        .iter()
        .find(|t| t.group == gestalt::UGH)
        .cloned()?;
    let at = tag.location?.address;
    let bytes = map.read_tag_bytes(tag.id).ok()?;
    SoundGestalt::decode(&bytes, at).ok()
}

fn refusal(class: &str, donor: &str, refused: &[Refused]) -> anyhow::Error {
    let shown: Vec<_> = refused
        .iter()
        .take(3)
        .map(|r| format!("{} {} ({:?})", r.group, r.name, r.detail.refusal))
        .collect();
    anyhow!(
        "class {class}: donor {donor} cannot be copied: {}",
        shown.join(", ")
    )
}

/// Copy the first `keep` classes of `all` into `set`, in order. Nothing is
/// undone on an error: the caller drops the set.
pub(crate) fn add(set: &mut TagSet, all: &Classes<'_>, keep: usize) -> Result<Added> {
    let mut reads: Vec<Option<Read>> = all.donors.iter().map(|_| None).collect();
    let mut base_gestalt: Option<Option<SoundGestalt>> = None;
    let mut merged = 0;
    let mut out = Added::default();
    for spec in &all.specs[..keep.min(all.specs.len())] {
        let donor = *all
            .donors
            .get(spec.donor)
            .with_context(|| format!("class {}: no donor number {}", spec.name, spec.donor))?;
        if reads[spec.donor].is_none() {
            reads[spec.donor] = Some(read(all.schema, donor)?);
        }
        let from = reads[spec.donor].as_mut().expect("read above");
        let root = from
            .map
            .tag(TagQuery::NameInGroup(spec.path.as_str(), CHAR))
            .map_err(|e| {
                anyhow!(
                    "class {}: donor {} has no char {}: {e}",
                    spec.name,
                    donor.name,
                    spec.path
                )
            })?
            .id;
        let base = base_gestalt.get_or_insert_with(|| SoundGestalt::from_set(&*set).ok());
        let (tree, sounds, raw) = match (&from.gestalt, base.as_mut()) {
            (Some(from_gestalt), Some(into_gestalt)) => {
                let moved = sounds::transplant_from(
                    &from.source,
                    &mut from.map,
                    from_gestalt,
                    root,
                    set,
                    into_gestalt,
                )
                .map_err(|e| anyhow!("class {}: {e}", spec.name))?
                .map_err(|r| refusal(&spec.name, donor.name, &r))?;
                merged += moved.sounds;
                (moved.tree, moved.sounds, moved.raw.1)
            }
            _ => {
                let tree = from
                    .source
                    .transplant(&mut from.map, root, set)
                    .map_err(|e| anyhow!("class {}: {e}", spec.name))?
                    .map_err(|r| refusal(&spec.name, donor.name, &r))?;
                for id in &tree.added {
                    let tag = from.map.tag(*id).map_err(|e| anyhow!("{e}"))?;
                    if tag.group == SND {
                        bail!(
                            "class {}: sound {} needs the sound gestalt (ugh!) in the base and in donor {}",
                            spec.name,
                            tag.name.as_deref().unwrap_or_default(),
                            donor.name
                        );
                    }
                }
                (tree, 0, 0)
            }
        };
        let mut bytes = 0;
        for id in &tree.added {
            bytes += INDEX_ENTRY;
            if let Some(TagPayload::Built(block)) = set.tag(tree.ids[id]).map(|t| &t.payload) {
                bytes += block.tree_len() as usize;
            }
        }
        out.ids.push(tree.ids[&root]);
        out.factions.push(spec.faction);
        out.costs.push(ClassCost {
            name: spec.name.clone(),
            tags: tree.added.len(),
            sounds,
            bytes,
            raw: tree.raw.1 + raw,
        });
    }
    // The gestalt grows only when a class brought sounds.
    if merged > 0 {
        if let Some(Some(g)) = base_gestalt {
            g.install(set, gestalt::DEFAULT_NAME)
                .map_err(|e| anyhow!("sound gestalt: {e}"))?;
        }
    }
    Ok(out)
}

/// A build over the tag headroom, in the stage-one kit's words: a caller
/// tells it from any other error and cuts a class.
#[derive(Debug)]
pub(crate) struct Overflow(pub String);

impl std::fmt::Display for Overflow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Overflow {}

/// The classes' costs for an error message, the last in the cut order last.
pub(crate) fn describe(costs: &[ClassCost]) -> String {
    if costs.is_empty() {
        return String::new();
    }
    let list: Vec<_> = costs
        .iter()
        .map(|c| format!("{} {} B ({} tags)", c.name, c.bytes, c.tags))
        .collect();
    format!(
        "; classes in keep order, the last cut first: {}",
        list.join(", ")
    )
}

#[cfg(test)]
mod tests;
