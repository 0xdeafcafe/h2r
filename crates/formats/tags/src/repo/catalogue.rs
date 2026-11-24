//! The asset catalogue (`<store>/assets.idx`): every asset of the store
//! by its asset hash (docs/engine/asset-store.md, "The catalogue").
//!
//! ```text
//! h2 asset catalogue 1
//! normal\t<version of the normal forms>
//! record\t<sha256 of extract.toml, or ->
//! map\t<stem>\t<sha256 of the manifest's text>
//! asset\t<group>\t<path>\t<asset hash>\t<variant or ->\t<map,map,...>
//! used\t<group>\t<path>\t<group>\t<path>\t<group>\t<path>...
//! ```
//!
//! [`build`] reads every manifest. Each tag a map reads gets its normal
//! form with the map's tables ([`normal::Tables`]) and its asset hash, so a
//! tag kept as two files is one asset when the files differ only in what
//! ties them to their maps (a sound at two indices of two gestalts), and
//! one file read by many maps is one line. An `asset` line is one key, one
//! hash and one file; a `used` line gives, for one key, the keys whose
//! tags reference it.
//!
//! The catalogue is derived, so it is deleted and rebuilt freely. It is
//! current while its maps are the store's manifests, each by the hash of
//! its text, its record is the store's `extract.toml` and its normal form
//! version is this build's ([`Catalogue::is_current`]). [`update`] writes
//! it when it is not, and after every extraction; `compile::prepare` calls
//! that.
use super::normal;
use super::store::{
    Manifest, TagRepo, esc, extraction_order, group_ext, parse_key, unesc, write_atomic,
};
use super::{Error, Result, TagKey, TagSource, hex, sha256};
use crate::schema::Schema;
use map::FourCC;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

/// File name of the catalogue.
pub const CATALOGUE_FILE: &str = "assets.idx";

const MAGIC: &str = "h2 asset catalogue 1";
const SOUND: FourCC = FourCC(*b"snd!");
const GESTALT: FourCC = FourCC(*b"ugh!");
const BITMAP: FourCC = FourCC(*b"bitm");

/// Every asset of a store.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Catalogue {
    /// [`normal::VERSION`] the hashes were made with.
    pub normal: u32,
    /// The SHA-256 of the `extract.toml` it was built with, by the hash of
    /// its text.
    pub record: Option<String>,
    /// Each map, by the SHA-256 of its manifest's text.
    pub maps: BTreeMap<String, String>,
    /// Every asset, sorted by key, then hash.
    pub assets: Vec<Asset>,
    /// Per key, the keys whose tags reference it.
    pub used_by: BTreeMap<TagKey, BTreeSet<TagKey>>,
}

/// One content of a key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub key: TagKey,
    /// The asset hash, lowercase hex.
    pub hash: String,
    /// The maps reading this content.
    pub maps: BTreeSet<String>,
    /// The files they read it from: a variant's name, `None` for the main
    /// file.
    pub files: BTreeSet<Option<String>>,
}

impl Catalogue {
    /// The contents of `key`, sorted by hash.
    pub fn of_key(&self, key: &TagKey) -> &[Asset] {
        let from = self.assets.partition_point(|a| a.key < *key);
        let to = self.assets.partition_point(|a| a.key <= *key);
        &self.assets[from..to]
    }

    /// The asset hash of `key` as `map` reads it.
    pub fn hash_in(&self, map: &str, key: &TagKey) -> Option<&str> {
        self.of_key(key)
            .iter()
            .find(|a| a.maps.contains(map))
            .map(|a| a.hash.as_str())
    }

    /// Every asset `map` reads, as (key, asset hash), sorted by key.
    pub fn of_map<'a>(&'a self, map: &str) -> Vec<(&'a TagKey, &'a str)> {
        self.assets
            .iter()
            .filter(|a| a.maps.contains(map))
            .map(|a| (&a.key, a.hash.as_str()))
            .collect()
    }

    /// The keys whose tags reference `key`.
    pub fn users(&self, key: &TagKey) -> Option<&BTreeSet<TagKey>> {
        self.used_by.get(key)
    }

    /// [`Catalogue::parse`]'s text.
    pub fn to_text(&self) -> String {
        let mut s = format!(
            "{MAGIC}\nnormal\t{}\nrecord\t{}\n",
            self.normal,
            self.record.as_deref().unwrap_or("-")
        );
        for (stem, hash) in &self.maps {
            s.push_str(&format!("map\t{}\t{hash}\n", esc(stem)));
        }
        for a in &self.assets {
            let maps = a
                .maps
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(",");
            for file in &a.files {
                s.push_str(&format!(
                    "asset\t{}\t{}\t{}\t{}\t{maps}\n",
                    group_ext(a.key.group),
                    esc(&a.key.path),
                    a.hash,
                    file.as_deref().unwrap_or("-")
                ));
            }
        }
        for (key, used) in &self.used_by {
            s.push_str(&format!(
                "used\t{}\t{}",
                group_ext(key.group),
                esc(&key.path)
            ));
            for k in used {
                s.push_str(&format!("\t{}\t{}", group_ext(k.group), esc(&k.path)));
            }
            s.push('\n');
        }
        s
    }

    /// Parse [`Catalogue::to_text`].
    pub fn parse(text: &str) -> Result<Self> {
        let bad = || Error::Invalid(format!("{CATALOGUE_FILE}: a line does not read"));
        let mut lines = text.lines();
        if lines.next() != Some(MAGIC) {
            return Err(Error::Invalid(format!(
                "{CATALOGUE_FILE} is not an asset catalogue"
            )));
        }
        let mut c = Catalogue::default();
        let mut assets: BTreeMap<(TagKey, String), Asset> = BTreeMap::new();
        for line in lines {
            let f: Vec<&str> = line.split('\t').collect();
            match f.first().copied() {
                Some("normal") => {
                    c.normal = f.get(1).and_then(|v| v.parse().ok()).ok_or_else(bad)?
                }
                Some("record") => {
                    c.record = f.get(1).filter(|v| **v != "-").map(|v| (*v).to_owned())
                }
                Some("map") if f.len() == 3 => {
                    c.maps.insert(unesc(f[1]), f[2].to_owned());
                }
                Some("asset") if f.len() == 6 => {
                    let key = parse_key(f[1], f[2])?;
                    let hash = f[3].to_owned();
                    let file = (f[4] != "-").then(|| f[4].to_owned());
                    let a = assets
                        .entry((key.clone(), hash.clone()))
                        .or_insert_with(|| Asset {
                            key,
                            hash,
                            maps: BTreeSet::new(),
                            files: BTreeSet::new(),
                        });
                    a.files.insert(file);
                    a.maps
                        .extend(f[5].split(',').filter(|m| !m.is_empty()).map(unesc));
                }
                Some("used") if f.len() >= 3 && f.len() % 2 == 1 => {
                    let key = parse_key(f[1], f[2])?;
                    let mut used = BTreeSet::new();
                    for g in f[3..].chunks_exact(2) {
                        used.insert(parse_key(g[0], g[1])?);
                    }
                    c.used_by.insert(key, used);
                }
                _ => return Err(bad()),
            }
        }
        c.assets = assets.into_values().collect();
        Ok(c)
    }

    /// Whether the catalogue was built from the store at `root` as it is
    /// now, under this build's normal forms.
    pub fn is_current(&self, root: &Path) -> Result<bool> {
        let (maps, record) = stamps(root)?;
        Ok(self.normal == normal::VERSION && maps == self.maps && record == self.record)
    }
}

impl TagRepo {
    /// The store's catalogue, `None` when it has none.
    pub fn catalogue(&self) -> Result<Option<Catalogue>> {
        read(self.root())
    }
}

/// What a catalogue is built from: each manifest under `maps/`, by the
/// SHA-256 of its text, and the SHA-256 of `extract.toml`.
fn stamps(root: &Path) -> Result<(BTreeMap<String, String>, Option<String>)> {
    let mut maps = BTreeMap::new();
    for e in std::fs::read_dir(root.join("maps"))? {
        let e = e?;
        let name = e.file_name().to_string_lossy().into_owned();
        if let Some(stem) = name.strip_suffix(".txt") {
            maps.insert(stem.to_owned(), hex(&sha256(&std::fs::read(e.path())?)));
        }
    }
    let record = match std::fs::read(root.join(super::store::EXTRACT_FILE)) {
        Ok(b) => Some(hex(&sha256(&b))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    Ok((maps, record))
}

/// The raw file of `hash` (lowercase hex) under `root/raw/`.
fn raw_file(root: &Path, hash: &str) -> Result<Vec<u8>> {
    Ok(std::fs::read(root.join("raw").join(&hash[..2]).join(hash))?)
}

/// The catalogue of the store at `root`: `None` when it has none.
pub fn read(root: &Path) -> Result<Option<Catalogue>> {
    match std::fs::read_to_string(root.join(CATALOGUE_FILE)) {
        Ok(t) => Catalogue::parse(&t).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Write the catalogue of the store at `root`.
pub fn write(root: &Path, c: &Catalogue) -> Result<()> {
    write_atomic(&root.join(CATALOGUE_FILE), c.to_text().as_bytes())
}

/// Keys and contents of one group.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Variants {
    /// Keys of the group.
    pub keys: usize,
    /// Contents: the files a key is kept as, or the asset hashes it has.
    pub contents: usize,
    /// Keys with more than one content.
    pub split: usize,
    /// Readings of a key by a map: every one before the normal forms, the
    /// hashed ones after.
    pub instances: usize,
    /// Readings whose content is not the first map's for the key (the
    /// shared caches come first, then the maps by name).
    pub others: usize,
}

/// What [`build`] found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Built {
    /// Maps read, and readings of a tag by a map.
    pub maps: usize,
    /// Per group, the keys as the store keeps their files and as the
    /// catalogue holds their assets.
    pub before: BTreeMap<FourCC, Variants>,
    pub after: BTreeMap<FourCC, Variants>,
    /// Tags that could not be hashed, each line naming the map and the tag.
    pub problems: Vec<String>,
}

impl Built {
    /// Every group together, before and after the normal forms.
    pub fn totals(&self) -> (Variants, Variants) {
        let sum = |m: &BTreeMap<FourCC, Variants>| {
            m.values()
                .copied()
                .fold(Variants::default(), |a, b| Variants {
                    keys: a.keys + b.keys,
                    contents: a.contents + b.contents,
                    split: a.split + b.split,
                    instances: a.instances + b.instances,
                    others: a.others + b.others,
                })
        };
        (sum(&self.before), sum(&self.after))
    }
}

/// Build the catalogue of the store at `root` from its manifests, with the
/// layouts in `schema` ([`Schema::builtin`] reads the store as this build
/// extracted it). A tag that cannot be read or given its form is a line
/// in [`Built::problems`] and has no asset.
pub fn build(root: &Path, schema: &Schema) -> Result<(Catalogue, Built)> {
    let store = TagRepo::open(root)?;
    let mut stems = store.maps()?;
    extraction_order(&mut stems);
    let mut c = Catalogue {
        normal: normal::VERSION,
        ..Default::default()
    };
    (c.maps, c.record) = stamps(root)?;
    let mut report = Built {
        maps: stems.len(),
        ..Default::default()
    };
    let mut assets: BTreeMap<(TagKey, String), Asset> = BTreeMap::new();
    // Per key: the files it is kept as, and the first map's file and hash.
    let mut files: BTreeMap<TagKey, BTreeSet<String>> = BTreeMap::new();
    let mut first_file: BTreeMap<TagKey, String> = BTreeMap::new();
    let mut first_hash: BTreeMap<TagKey, String> = BTreeMap::new();
    // The groups whose form depends on the map's tables are hashed for
    // each reading; every other tag's form follows from its file alone.
    let mut memo: HashMap<(FourCC, Option<String>), (String, Vec<TagKey>)> = HashMap::new();
    let mut read_raw = |hash: &[u8; 32]| -> Result<Vec<u8>> { raw_file(root, &hex(hash)) };
    for stem in &stems {
        let text = std::fs::read_to_string(root.join("maps").join(format!("{stem}.txt")))?;
        let m = Manifest::parse(&text)?;
        let mut repo = TagRepo::for_map(root, stem)?;
        let gestalt = match m.tags.iter().find(|(k, _)| k.group == GESTALT) {
            Some((k, _)) => match repo.read(k) {
                Ok(g) => g,
                Err(e) => {
                    report.problems.push(format!("{stem}: {k}: {e}"));
                    None
                }
            },
            None => None,
        };
        if gestalt.is_none() && m.tags.iter().any(|(k, _)| k.group == SOUND) {
            report.problems.push(format!(
                "{stem}: no sound gestalt is read, so its sounds are hashed as extracted"
            ));
        }
        let low_detail = match m.low_detail.as_deref().filter(|h| h.len() >= 2) {
            Some(h) => match raw_file(root, h) {
                Ok(b) => b,
                Err(e) => {
                    report.problems.push(format!(
                        "{stem}: the low-detail region {h} does not read: {e}"
                    ));
                    Vec::new()
                }
            },
            None => Vec::new(),
        };
        let mut tables = normal::Tables {
            gestalt: gestalt.as_ref(),
            low_detail: &low_detail,
            raw: Some(&mut read_raw),
            schema: Some(schema),
        };
        for (key, variant) in &m.tags {
            let per_map = key.group == SOUND || key.group == BITMAP;
            let file = variant.clone().unwrap_or_default();
            let v = report.before.entry(key.group).or_default();
            v.instances += 1;
            v.others += usize::from(match first_file.get(key) {
                Some(f) => *f != file,
                None => {
                    first_file.insert(key.clone(), file.clone());
                    false
                }
            });
            files.entry(key.clone()).or_default().insert(file.clone());
            let got = if per_map {
                None
            } else {
                memo.get(&(key.group, variant.clone()))
            };
            let (hash, references) = match got {
                Some(g) => g.clone(),
                None => {
                    let tag = match repo.read(key) {
                        Ok(Some(tag)) => tag,
                        Ok(None) => {
                            report
                                .problems
                                .push(format!("{stem}: {key}: the store has no file for it"));
                            continue;
                        }
                        Err(e) => {
                            report.problems.push(format!("{stem}: {key}: {e}"));
                            continue;
                        }
                    };
                    let form = match normal::form(&tag, &mut tables) {
                        Ok(form) => form,
                        Err(e) => {
                            report.problems.push(format!("{stem}: {e}"));
                            continue;
                        }
                    };
                    let got = (hex(&normal::hash(&form)), tag.references());
                    if !per_map {
                        memo.insert((key.group, variant.clone()), got.clone());
                    }
                    got
                }
            };
            let v = report.after.entry(key.group).or_default();
            v.instances += 1;
            v.others += usize::from(match first_hash.get(key) {
                Some(h) => *h != hash,
                None => {
                    first_hash.insert(key.clone(), hash.clone());
                    false
                }
            });
            let a = assets
                .entry((key.clone(), hash.clone()))
                .or_insert_with(|| Asset {
                    key: key.clone(),
                    hash: hash.clone(),
                    maps: BTreeSet::new(),
                    files: BTreeSet::new(),
                });
            a.maps.insert(stem.clone());
            a.files.insert(variant.clone());
            for r in &references {
                c.used_by.entry(r.clone()).or_default().insert(key.clone());
            }
        }
    }
    for (key, fs) in &files {
        let v = report.before.entry(key.group).or_default();
        v.keys += 1;
        v.contents += fs.len();
        v.split += usize::from(fs.len() > 1);
    }
    let mut hashes: BTreeMap<TagKey, usize> = BTreeMap::new();
    for ((key, _), _) in &assets {
        *hashes.entry(key.clone()).or_default() += 1;
    }
    for (key, n) in hashes {
        let v = report.after.entry(key.group).or_default();
        v.keys += 1;
        v.contents += n;
        v.split += usize::from(n > 1);
    }
    c.assets = assets.into_values().collect();
    Ok((c, report))
}

/// Bring the catalogue of the store at `root` up to date: built and
/// written when it has none, when the store changed under it or when
/// `force` is set (the store was just written: a file can change while no
/// manifest does). `None` when the catalogue is current.
pub fn update(root: &Path, schema: &Schema, force: bool) -> Result<Option<Built>> {
    if !force && read(root)?.is_some_and(|c| c.is_current(root).unwrap_or(false)) {
        return Ok(None);
    }
    let (c, report) = build(root, schema)?;
    write(root, &c)?;
    Ok(Some(report))
}
