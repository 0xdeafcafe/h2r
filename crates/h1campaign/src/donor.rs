//! Retail Halo 2 tags the base lacks, copied whole from other retail maps
//! (`h1sounds::transplant`: the tag tree, its raw, and its sounds' gestalt
//! entries).
//!
//! The base (05a) has no hunter, flood or monitor. Halo 1's levels use
//! them, so the characters `h1ai::convert::character_for` names come from
//! the first donor map that has them and whose tree `tags::schema` copies
//! (`donor_survey` in the survey tests lists who has what):
//!
//! | Tag | Donor |
//! |---|---|
//! | hunter | `03a_oldmombasa` |
//! | flood combat elite, infection, carrier; monitor | `04b_floodlab` |
//! | flood combat human | `06a_sentinelwalls` |
//!
//! The Halo 1 sentinel (`sentinel_aggressor_halo1`, on 04a and 04b) does
//! not copy: the schema refuses its animation graph (an unknown pointer).

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::{Arc, Mutex, OnceLock};

use h1map::H1Map;
use map::build::TagSet;
use map::{FourCC, Map};

use crate::{Report, Result, invalid};

/// The donor maps, in the order they are searched.
pub const MAPS: [&str; 3] = ["03a_oldmombasa", "04b_floodlab", "06a_sentinelwalls"];

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
    gestalt: h1sounds::gestalt::SoundGestalt,
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
    /// `snd!` tags merged into the gestalt, and of those the promotions
    /// and extra infos cut.
    pub sounds: (usize, usize, usize),
    /// Local raw resources copied (tree and sound chunks), and bytes.
    pub raw: (usize, u64),
}

impl std::fmt::Display for Brought {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} from {}: {} tags, {} sounds ({} promotions, {} extra infos cut), {} raw ({:#x} bytes)",
            self.root,
            self.donor,
            self.tags,
            self.sounds.0,
            self.sounds.1,
            self.sounds.2,
            self.raw.0,
            self.raw.1
        )
    }
}

/// What a level wants that the base may lack: its characters (by
/// `h1ai`'s table).
pub fn wanted(h1: &H1Map) -> Result<Vec<(FourCC, String)>> {
    let ai = h1ai::h1::H1Ai::read(h1).map_err(|e| invalid(format!("ai: {e}")))?;
    let plan = h1ai::convert::convert(&ai, &Default::default());
    let mut out: Vec<(FourCC, String)> = Vec::new();
    for c in &plan.characters {
        let w = (FourCC(*b"char"), c.h2_character.to_owned());
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
                    h1sounds::gestalt::SoundGestalt::from_set(set)
                        .map_err(|e| invalid(format!("gestalt: {e}")))?,
                );
            }
            let g = gestalt.as_mut().expect("set above");
            let moved = match &read {
                Some(r) => {
                    h1sounds::transplant_from(&r.source, &mut src, src_gestalt, root, set, g)
                }
                // Reading it failed: the uncached path says why.
                None => h1sounds::transplant(schema, &mut src, src_gestalt, root, set, g),
            };
            match moved.map_err(|e| invalid(format!("{group} {name} from {donor}: {e}")))? {
                Ok(m) => {
                    report.brought.push(Brought {
                        root: format!("{group} {name}"),
                        donor: donor.clone(),
                        tags: m.tree.added.len(),
                        sounds: (m.sounds, m.promotions_cut, m.extra_infos_cut),
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
        g.install(set, h1sounds::gestalt::DEFAULT_NAME)
            .map_err(|e| invalid(format!("gestalt: {e}")))?;
    }
    Ok(())
}

/// The donor's own gestalt.
fn donor_gestalt(src: &mut Map<Cursor<&[u8]>>) -> Result<h1sounds::gestalt::SoundGestalt> {
    let t = src
        .tags()
        .iter()
        .find(|t| t.group == h1sounds::gestalt::UGH)
        .cloned()
        .ok_or_else(|| invalid("the donor has no ugh!"))?;
    let at = t
        .location
        .ok_or_else(|| invalid("the donor's ugh! has no data"))?
        .address;
    let bytes = src.read_tag_bytes(t.id)?;
    h1sounds::gestalt::SoundGestalt::decode(&bytes, at)
        .map_err(|e| invalid(format!("gestalt: {e}")))
}
