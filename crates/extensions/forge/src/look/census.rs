//! The census: which models this machine can draw as a monitor
//! (docs/game/forge-monitor-appearance.md, "Which model").
//!
//! The census walks the campaign maps of a [`Disc`], takes every biped
//! tag by its species key, picks one tag a key, and records what the model
//! offers: its variants, whether it has change colours, whether it attaches
//! a light, and its size. This module holds the rules and the cache and
//! reads nothing itself; the CLI's disc reader (`h2 forge looks`) and the
//! game's map reader implement [`Disc`]. It never offers a model from a
//! multiplayer map: only campaign maps are asked.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{KEY_MAX, MARKER, VARIANT_MAX, name_ok};

/// The census format. A cached census of another version is read as absent.
pub const VERSION: u32 = 1;

/// A campaign map: its name, and the SHA-256 of its file as lower-case hex.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapInfo {
    pub name: String,
    pub sha256: String,
}

/// What a model offers, read from its tag.
#[derive(Clone, Debug, PartialEq)]
pub struct Described {
    /// Its variants' names, in tag order.
    pub variants: Vec<String>,
    /// Any material has a change-colour map.
    pub change_colours: bool,
    /// The object attaches a light.
    pub light: bool,
    pub nodes: u32,
    pub triangles: u32,
}

/// Where the models are. Implemented over a disc, a map directory or, in
/// tests, invented data.
pub trait Disc {
    /// The campaign maps, in the order they should win a tie (disc order).
    fn campaign_maps(&self) -> Result<Vec<MapInfo>, String>;
    /// The names of the biped tags that have data in `map`.
    fn bipeds(&self, map: &MapInfo) -> Result<Vec<String>, String>;
    /// `tag` (one of [`Disc::bipeds`]) resolved through its model.
    fn describe(&self, map: &MapInfo, tag: &str) -> Result<Described, String>;
}

/// One model this machine can draw. Its identity across machines is
/// `(key, sha256)`, never a tag index.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// The species key: the tag's directory name, lower case.
    pub key: String,
    /// The tag's base name, read at run time.
    pub name: String,
    /// The campaign map the tag was read from.
    pub source: String,
    /// That map file's SHA-256, lower-case hex.
    pub sha256: String,
    pub variants: Vec<String>,
    pub change_colours: bool,
    pub light: bool,
    pub nodes: u32,
    pub triangles: u32,
}

impl Entry {
    /// What cannot be changed on this model, field by field, with the
    /// reason a row shows.
    pub fn locks(&self) -> BTreeMap<&'static str, String> {
        let mut out = BTreeMap::new();
        if !self.change_colours {
            for field in ["primary", "secondary"] {
                out.insert(field, "This model has no colour areas".to_owned());
            }
        }
        if self.variants.len() < 2 {
            out.insert("variant", "This model has one variant".to_owned());
        }
        out
    }

    pub fn to_value(&self) -> Value {
        json!({
            "key": self.key,
            "name": self.name,
            "source": self.source,
            "sha256": self.sha256,
            "variants": self.variants,
            "colours": self.change_colours,
            "light": self.light,
            "nodes": self.nodes,
            "triangles": self.triangles,
        })
    }

    fn from_value(v: &Value) -> Option<Entry> {
        let text = |k: &str| v.get(k)?.as_str().map(str::to_owned);
        Some(Entry {
            key: text("key")?,
            name: text("name")?,
            source: text("source")?,
            sha256: text("sha256")?,
            variants: v
                .get("variants")?
                .as_array()?
                .iter()
                .map(|s| s.as_str().map(str::to_owned))
                .collect::<Option<_>>()?,
            change_colours: v.get("colours")?.as_bool()?,
            light: v.get("light")?.as_bool()?,
            nodes: u32::try_from(v.get("nodes")?.as_u64()?).ok()?,
            triangles: u32::try_from(v.get("triangles")?.as_u64()?).ok()?,
        })
    }
}

/// A tag or variant the census left out, and why. Nothing is dropped
/// without one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skipped {
    pub what: String,
    pub reason: String,
}

/// The census of one disc.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Census {
    /// One entry a key, in key order.
    pub entries: Vec<Entry>,
    pub skipped: Vec<Skipped>,
}

/// The last component of a tag name: `objects\characters\grunt\grunt` is
/// `grunt`.
pub fn last_component(name: &str) -> &str {
    name.rsplit('\\').next().unwrap_or(name)
}

/// The species key of a biped tag: its directory's name, lower case
/// (`objects\characters\grunt\grunt` is `grunt`), as telemetry sends it.
pub fn species_of(tag: &str) -> Option<String> {
    let mut parts = tag.rsplit('\\');
    parts.next()?;
    let dir = parts.next()?.to_ascii_lowercase();
    name_ok(&dir, KEY_MAX).then_some(dir)
}

/// Per species, the biped tag to draw it with, from every biped tag name
/// and the number of maps it is on: the one named like its directory, else
/// the one on the most maps, else the shortest name (then the first
/// alphabetically).
pub fn choose_bipeds(tags: &BTreeMap<String, usize>) -> BTreeMap<String, String> {
    let mut by_species: BTreeMap<String, Vec<(&String, usize)>> = BTreeMap::new();
    for (name, &count) in tags {
        if let Some(sp) = species_of(name) {
            by_species.entry(sp).or_default().push((name, count));
        }
    }
    by_species
        .into_iter()
        .map(|(sp, mut names)| {
            names.sort_by(|a, b| {
                let own = |n: &str| !last_component(n).eq_ignore_ascii_case(&sp);
                own(a.0)
                    .cmp(&own(b.0))
                    .then(b.1.cmp(&a.1))
                    .then(a.0.len().cmp(&b.0.len()))
                    .then(a.0.cmp(b.0))
            });
            (sp, names[0].0.clone())
        })
        .collect()
}

/// The permutation a look takes where a variant's region lists several:
/// the highest probability, the first on a tie. Not a number counts lowest.
/// `None` for no permutations. (The game draws by `0x0016dad0`, which is
/// not reproduced; objects.md, "Resolver".)
pub fn pick_permutation(weights: &[f32]) -> Option<usize> {
    let key = |w: f32| if w.is_nan() { f32::NEG_INFINITY } else { w };
    let mut best: Option<(usize, f32)> = None;
    for (i, &w) in weights.iter().enumerate() {
        if best.is_none_or(|(_, b)| key(w) > b) {
            best = Some((i, key(w)));
        }
    }
    best.map(|(i, _)| i)
}

/// Runs the census. A map or tag that cannot be read is recorded in
/// [`Census::skipped`] and the rest is still listed.
pub fn run(disc: &dyn Disc) -> Result<Census, String> {
    let maps = disc.campaign_maps()?;
    let mut census = Census::default();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut first: BTreeMap<String, usize> = BTreeMap::new();
    for (i, map) in maps.iter().enumerate() {
        let tags = match disc.bipeds(map) {
            Ok(t) => t,
            Err(e) => {
                census.skipped.push(Skipped {
                    what: map.name.clone(),
                    reason: format!("its bipeds could not be listed: {e}"),
                });
                continue;
            }
        };
        let once: BTreeSet<String> = tags.into_iter().collect();
        for tag in once {
            if species_of(&tag).is_none() {
                census.skipped.push(Skipped {
                    what: tag,
                    reason: "its directory is not a model key".into(),
                });
                continue;
            }
            *counts.entry(tag.clone()).or_default() += 1;
            first.entry(tag).or_insert(i);
        }
    }
    for (key, tag) in choose_bipeds(&counts) {
        let map = &maps[first[&tag]];
        if key == MARKER {
            census.skipped.push(Skipped {
                what: tag,
                reason: format!("{MARKER:?} is the marker's key"),
            });
            continue;
        }
        match disc.describe(map, &tag) {
            Ok(d) => {
                let mut variants = Vec::new();
                for v in d.variants {
                    if name_ok(&v, VARIANT_MAX) {
                        variants.push(v);
                    } else {
                        census.skipped.push(Skipped {
                            what: format!("{tag} variant {v:?}"),
                            reason: "its name is not a variant key".into(),
                        });
                    }
                }
                census.entries.push(Entry {
                    key,
                    name: last_component(&tag).to_owned(),
                    source: map.name.clone(),
                    sha256: map.sha256.to_ascii_lowercase(),
                    variants,
                    change_colours: d.change_colours,
                    light: d.light,
                    nodes: d.nodes,
                    triangles: d.triangles,
                });
            }
            Err(e) => census.skipped.push(Skipped {
                what: tag,
                reason: format!("its model could not be read: {e}"),
            }),
        }
    }
    Ok(census)
}

/// The cache key of a disc: the set of its campaign maps' hashes (and the
/// census version), so a new or changed map takes a new census.
pub fn cache_key(maps: &[MapInfo]) -> String {
    let mut lines: Vec<String> = maps
        .iter()
        .map(|m| format!("{}={}", m.name, m.sha256.to_ascii_lowercase()))
        .collect();
    lines.sort();
    let text = format!("forge-looks {VERSION}\n{}\n", lines.join("\n"));
    content::Sha256::of(text.as_bytes()).to_string()
}

impl Census {
    pub fn to_value(&self) -> Value {
        json!({
            "version": VERSION,
            "entries": self.entries.iter().map(Entry::to_value).collect::<Vec<_>>(),
            "skipped": self.skipped.iter()
                .map(|s| json!({"what": s.what, "reason": s.reason}))
                .collect::<Vec<_>>(),
        })
    }

    /// A census from [`Census::to_value`], or `None` when it is of another
    /// version or malformed.
    pub fn from_value(v: &Value) -> Option<Census> {
        if v.get("version")?.as_u64()? != u64::from(VERSION) {
            return None;
        }
        let entries = v
            .get("entries")?
            .as_array()?
            .iter()
            .map(Entry::from_value)
            .collect::<Option<_>>()?;
        let skipped = v
            .get("skipped")?
            .as_array()?
            .iter()
            .map(|s| {
                Some(Skipped {
                    what: s.get("what")?.as_str()?.to_owned(),
                    reason: s.get("reason")?.as_str()?.to_owned(),
                })
            })
            .collect::<Option<_>>()?;
        Some(Census { entries, skipped })
    }

    /// The entry of `key`, if this machine has it.
    pub fn entry(&self, key: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.key == key)
    }
}

/// The census `h2 forge looks` last made, under `dir`: what the game reads
/// as this machine's catalogue.
pub const LATEST: &str = "forge-looks.json";

/// Where the census of `key` is cached under `dir`.
pub fn cache_path(dir: &Path, key: &str) -> PathBuf {
    dir.join(format!("forge-looks-{key}.json"))
}

/// The cached census of `key`, or `None` when there is none or it is not
/// readable (the caller runs the census again).
pub fn load_cached(dir: &Path, key: &str) -> Option<Census> {
    let bytes = std::fs::read(cache_path(dir, key)).ok()?;
    Census::from_value(&serde_json::from_slice(&bytes).ok()?)
}

/// The census `h2 forge looks` last made: `Ok(None)` when it has not run,
/// an error when the file is there and unreadable, so the game can say so.
pub fn load_latest(dir: &Path) -> Result<Option<Census>, String> {
    let path = dir.join(LATEST);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let value = serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    Census::from_value(&value)
        .map(Some)
        .ok_or_else(|| format!("{}: not a census of this version", path.display()))
}

/// Caches `census` under `key`, and as the latest: each a temporary file,
/// then a rename.
pub fn store_cached(dir: &Path, key: &str, census: &Census) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let bytes = serde_json::to_vec_pretty(&census.to_value()).map_err(|e| e.to_string())?;
    for path in [cache_path(dir, key), dir.join(LATEST)] {
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(())
}
