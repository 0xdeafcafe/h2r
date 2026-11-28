//! Retail Halo 2 tags the base lacks, copied whole from other retail maps
//! (`h1_assets::sounds::transplant`: the tag tree, its raw, and its sounds' gestalt
//! entries).
//!
//! The base (05a) has no hunter, flood or monitor. Halo 1's levels use
//! them, so the characters `crate::ai::convert::character_for` names come from
//! the first donor map that has them and whose tree `tags::schema` copies
//! (`donor_survey` in the survey tests lists who has what):
//!
//! | Tag | Donor |
//! |---|---|
//! | hunter | `03a_oldmombasa` |
//! | flood combat elite, infection, carrier; monitor | `04b_floodlab` |
//! | flood combat human | `06a_sentinelwalls` |
//! | fuel rod gun (`flak_cannon`) | `04b_floodlab`, else `08a_deltacliffs` |
//!
//! Crew and Sentinel actor variants use their own rebuilt units and graphs in `crate::roster`.

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::{Arc, Mutex, OnceLock};

use h1map::H1Map;
use map::build::TagSet;
use map::{FourCC, Map};

use crate::{Report, Result, invalid};

/// The donor maps, in the order they are searched.
///
/// `08a_deltacliffs` is last: it holds the fuel rod gun (`flak_cannon`,
/// the swap for Halo 1's plasma cannon, `crate::h2assets`) with its own
/// tree, so a level whose weapon `04b_floodlab`'s copy doesn't bring still
/// gets a retail one. V (disc scan, 2026-11-28): `weap`, `hlmt`, `mode` and
/// `coll` of `flak_cannon` carry data in `04a_gasgiant`, `04b_floodlab`,
/// `05b_deltatowers`, `07a_highcharity`, `07b_forerunnership` and
/// `08a_deltacliffs`; `03a` and `06a` have none.
pub const MAPS: [&str; 4] = [
    "03a_oldmombasa",
    "04b_floodlab",
    "06a_sentinelwalls",
    "08a_deltacliffs",
];

/// Retail maps to copy from: (name, file bytes).
#[derive(Debug, Default, Clone)]
pub struct Donors {
    pub maps: Vec<(String, Vec<u8>)>,
    /// Each donor read for transplants, once (shared by every level
    /// converted from these donors).
    read: Cache,
}

/// A donor read once: its gestalt and its transplant source.
struct ReadDonor {
    gestalt: h1_assets::sounds::gestalt::SoundGestalt,
    source: tags::schema::transplant::Source<'static>,
}

/// Donors read so far, by name: `None` when reading failed (the slow path
/// reports why).
#[derive(Default)]
struct Cache(Mutex<HashMap<String, Arc<OnceLock<Option<Arc<ReadDonor>>>>>>);

impl Clone for Cache {
    fn clone(&self) -> Self {
        Cache::default()
    }
}

impl std::fmt::Debug for Cache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Cache")
    }
}

impl Donors {
    /// `donor` (its file `bytes`) read once.
    fn read(&self, donor: &str, bytes: &[u8]) -> Option<Arc<ReadDonor>> {
        let cell = {
            let mut m = self.read.0.lock().unwrap_or_else(|e| e.into_inner());
            Arc::clone(m.entry(donor.to_owned()).or_default())
        };
        cell.get_or_init(|| {
            let mut src = Map::from_reader(Cursor::new(bytes)).ok()?;
            let gestalt = donor_gestalt(&mut src).ok()?;
            let schema = tags::schema::Schema::builtin();
            let source = tags::schema::transplant::Source::new(schema, &mut src).ok()?;
            Some(Arc::new(ReadDonor { gestalt, source }))
        })
        .clone()
    }
}

/// One tag tree copied in.
#[derive(Debug, Clone)]
pub struct Brought {
    /// Group and name of the root.
    pub root: String,
    /// The donor map.
    pub donor: String,
    /// Tags copied (the rest of the tree the base had).
    pub tags: usize,
    /// `snd!` tags merged into the gestalt.
    pub sounds: usize,
    /// Moved sounds whose native promotion rules were carried over.
    pub promotions: usize,
    /// Local raw resources copied (tree and sound chunks), and bytes.
    pub raw: (usize, u64),
}

impl std::fmt::Display for Brought {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} from {}: {} tags, {} sounds ({} with promotions), {} raw ({:#x} bytes)",
            self.root,
            self.donor,
            self.tags,
            self.sounds,
            self.promotions,
            self.raw.0,
            self.raw.1
        )
    }
}

/// What a level wants that the base may lack: its characters (by
/// `h1ai`'s table).
pub fn wanted(h1: &H1Map) -> Result<Vec<(FourCC, String)>> {
    let ai = crate::ai::h1::H1Ai::read(h1).map_err(|e| invalid(format!("ai: {e}")))?;
    let plan = crate::ai::convert::convert(
        &ai,
        &crate::ai::convert::Options {
            halo2_assets: true,
            ..Default::default()
        },
    )
    .map_err(|e| invalid(format!("{} AI: {e}", h1.header().name)))?;
    let mut out: Vec<(FourCC, String)> = Vec::new();
    for c in &plan.characters {
        let w = match c.h1_type.0 {
            8 => continue,
            13 => (FourCC(*b"styl"), crate::roster::SENTINEL_STYLE.to_owned()),
            _ => (FourCC(*b"char"), c.h2_character.to_owned()),
        };
        if !out.contains(&w) {
            out.push(w);
        }
    }
    Ok(out)
}

/// Copies each `wanted` tag `set` lacks from the first donor that has it
/// and whose tree copies, sounds included. The gestalt is installed once,
/// after the last.
pub fn bring(
    donors: &Donors,
    set: &mut TagSet,
    wanted: &[(FourCC, String)],
    report: &mut Report,
) -> Result<()> {
    let schema = tags::schema::Schema::builtin();
    let mut gestalt = None;
    for (group, name) in wanted {
        if set.find(*group, name).is_some() {
            continue;
        }
        let mut why = Vec::new();
        for (donor, bytes) in &donors.maps {
            let mut src = Map::from_reader(Cursor::new(bytes.as_slice()))?;
            let Some(root) = src
                .tags()
                .iter()
                .find(|t| t.group == *group && t.name.as_deref() == Some(name.as_str()))
                .map(|t| t.id)
            else {
                continue;
            };
            let read = donors.read(donor, bytes);
            let own;
            let src_gestalt = match &read {
                Some(r) => &r.gestalt,
                None => {
                    own = donor_gestalt(&mut src)?;
                    &own
                }
            };
            if gestalt.is_none() {
                gestalt = Some(
                    h1_assets::sounds::gestalt::SoundGestalt::from_set(set)
                        .map_err(|e| invalid(format!("gestalt: {e}")))?,
                );
            }
            let g = gestalt.as_mut().expect("set above");
            let moved = match &read {
                Some(r) => {
                    h1_assets::sounds::transplant_from(&r.source, &mut src, src_gestalt, root, set, g)
                }
                // Reading it failed: the uncached path says why.
                None => h1_assets::sounds::transplant(schema, &mut src, src_gestalt, root, set, g),
            };
            match moved.map_err(|e| invalid(format!("{group} {name} from {donor}: {e}")))? {
                Ok(m) => {
                    report.brought.push(Brought {
                        root: format!("{group} {name}"),
                        donor: donor.clone(),
                        tags: m.tree.added.len(),
                        sounds: m.sounds,
                        promotions: m.promotions,
                        raw: (m.tree.raw.0 + m.raw.0, m.tree.raw.1 + m.raw.1),
                    });
                    why.clear();
                    break;
                }
                Err(refused) => why.push(format!(
                    "{donor} refuses {}",
                    refused
                        .iter()
                        .take(2)
                        .map(|r| format!("{} {} ({:?})", r.group, r.name, r.detail.refusal))
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            }
        }
        if set.find(*group, name).is_none() {
            if why.is_empty() {
                why.push("no donor has it".into());
            }
            report
                .dropped
                .push(format!("{group} {name}: not brought: {}", why.join("; ")));
        }
    }
    if let Some(g) = gestalt {
        g.install(set, h1_assets::sounds::gestalt::DEFAULT_NAME)
            .map_err(|e| invalid(format!("gestalt: {e}")))?;
    }
    Ok(())
}

/// The donor's own gestalt.
fn donor_gestalt(src: &mut Map<Cursor<&[u8]>>) -> Result<h1_assets::sounds::gestalt::SoundGestalt> {
    let t = src
        .tags()
        .iter()
        .find(|t| t.group == h1_assets::sounds::gestalt::UGH)
        .cloned()
        .ok_or_else(|| invalid("the donor has no ugh!"))?;
    let at = t
        .location
        .ok_or_else(|| invalid("the donor's ugh! has no data"))?
        .address;
    let bytes = src.read_tag_bytes(t.id)?;
    h1_assets::sounds::gestalt::SoundGestalt::decode(&bytes, at)
        .map_err(|e| invalid(format!("gestalt: {e}")))
}
