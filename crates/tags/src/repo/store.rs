//! A repository on disk ([`TagRepo`]).
//!
//! ```text
//! <repo>/repo.txt                  "h2 tag repository", format version
//! <repo>/groups.txt                group table: class parent grandparent (hex)
//! <repo>/strings.txt               string IDs every map starts with, in order
//! <repo>/tags/<path>.<group>       a tag (the first content seen for the key)
//! <repo>/tags/<path>.<group>~<h>   another content for the same key (a variant)
//! <repo>/raw/<hh>/<hash>           raw resource bytes, by SHA-256
//! <repo>/maps/<map>.txt            what a retail map holds (manifest)
//! <repo>/maps/<map>.header         its 0x800-byte header
//! ```
//!
//! Path backslashes become directories. A group whose bytes are not all
//! letters, digits or `!_-+.` is written as 8 hex digits, as in the schema
//! file. `<h>` is the first 12 hex digits of the SHA-256 of the variant's
//! file. A manifest names, for each tag of its map, which content the map
//! has, so a repository opened for that map ([`TagRepo::for_map`]) reads
//! the map's own variants.
use super::format::{decode, encode};
use super::{Error, LooseTag, RawRes, Result, TagKey, TagSource, hex, sha256};
use map::{CacheType, FourCC, TagGroup};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

/// First line of `repo.txt`.
const REPO_MAGIC: &str = "h2 tag repository";

/// A repository directory.
pub struct TagRepo {
    root: PathBuf,
    /// Variant per key for the map this repository was opened for.
    variants: HashMap<TagKey, String>,
    groups: Vec<TagGroup>,
}

/// What a retail map held, as recorded at extraction.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Manifest {
    /// Map file stem (`lockout`).
    pub map: String,
    /// Header `+0x140`.
    pub cache_type: u32,
    /// Internal name (header `+0x198`).
    pub name: String,
    /// Scenario key.
    pub scenario: Option<TagKey>,
    /// Globals key.
    pub globals: Option<TagKey>,
    /// Low-detail bitmap region, by hash (`raw/`), if the map has one.
    pub low_detail: Option<String>,
    /// Index header word `+0x14`.
    pub index_word_14: u32,
    /// Every tag in slot order, with its variant (`None`: the main file).
    pub tags: Vec<(TagKey, Option<String>)>,
    /// Tags that could not be extracted, with why.
    pub refused: Vec<(TagKey, String)>,
}

/// File-name form of a group.
pub fn group_ext(g: FourCC) -> String {
    let plain =
        g.0.iter()
            .all(|b| b.is_ascii_alphanumeric() || b"!_-+.".contains(b));
    if plain {
        String::from_utf8_lossy(&g.0).into_owned()
    } else {
        format!("{:08x}", g.as_u32())
    }
}

fn parse_group(s: &str) -> Option<FourCC> {
    match s.len() {
        4 => Some(FourCC(s.as_bytes().try_into().ok()?)),
        8 => u32::from_str_radix(s, 16).ok().map(FourCC::from_u32),
        _ => None,
    }
}

/// Escape a string for one line of a text file.
fn esc(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
}

fn unesc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(o) => out.push(o),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn key_line(k: &TagKey) -> String {
    format!("{}\t{}", group_ext(k.group), esc(&k.path))
}

fn parse_key(g: &str, p: &str) -> Result<TagKey> {
    let group = parse_group(g).ok_or_else(|| Error::Invalid(format!("bad group {g:?}")))?;
    Ok(TagKey::new(group, unesc(p)))
}

impl Manifest {
    /// Text form.
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        let opt = |k: &Option<TagKey>| k.as_ref().map_or("-\t-".to_owned(), key_line);
        s.push_str(&format!("map\t{}\n", esc(&self.map)));
        s.push_str(&format!("cache_type\t{}\n", self.cache_type));
        s.push_str(&format!("name\t{}\n", esc(&self.name)));
        s.push_str(&format!("scenario\t{}\n", opt(&self.scenario)));
        s.push_str(&format!("globals\t{}\n", opt(&self.globals)));
        s.push_str(&format!(
            "low_detail\t{}\n",
            self.low_detail.as_deref().unwrap_or("-")
        ));
        s.push_str(&format!("index_word_14\t{:#x}\n", self.index_word_14));
        for (k, v) in &self.tags {
            s.push_str(&format!(
                "tag\t{}\t{}\n",
                key_line(k),
                v.as_deref().unwrap_or("-")
            ));
        }
        for (k, why) in &self.refused {
            s.push_str(&format!("refused\t{}\t{}\n", key_line(k), esc(why)));
        }
        s
    }

    /// Parse [`Manifest::to_text`].
    pub fn parse(text: &str) -> Result<Self> {
        let mut m = Manifest::default();
        for line in text.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            let opt = |f: &[&str]| -> Result<Option<TagKey>> {
                if f.len() < 3 || f[1] == "-" {
                    Ok(None)
                } else {
                    parse_key(f[1], f[2]).map(Some)
                }
            };
            match f[0] {
                "map" => m.map = unesc(f.get(1).copied().unwrap_or("")),
                "cache_type" => m.cache_type = f.get(1).and_then(|v| v.parse().ok()).unwrap_or(0),
                "name" => m.name = unesc(f.get(1).copied().unwrap_or("")),
                "scenario" => m.scenario = opt(&f)?,
                "globals" => m.globals = opt(&f)?,
                "low_detail" => {
                    m.low_detail = f.get(1).filter(|v| **v != "-").map(|v| (*v).to_owned())
                }
                "index_word_14" => {
                    m.index_word_14 = f
                        .get(1)
                        .and_then(|v| u32::from_str_radix(v.trim_start_matches("0x"), 16).ok())
                        .unwrap_or(0)
                }
                "tag" if f.len() >= 4 => {
                    let v = (f[3] != "-").then(|| f[3].to_owned());
                    m.tags.push((parse_key(f[1], f[2])?, v));
                }
                "refused" if f.len() >= 4 => m.refused.push((parse_key(f[1], f[2])?, unesc(f[3]))),
                _ => {}
            }
        }
        Ok(m)
    }

    /// Cache type.
    pub fn cache_type(&self) -> CacheType {
        CacheType::from(self.cache_type)
    }
}

/// How [`TagRepo::put`] stored a tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stored {
    /// New key: written as the main file.
    New,
    /// The main file already holds these bytes.
    Same,
    /// The key exists with other contents: kept as this variant (new or not).
    Variant(String),
}

impl TagRepo {
    /// Create (or reuse) a repository at `root`.
    pub fn create(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(root.join("tags"))?;
        fs::create_dir_all(root.join("raw"))?;
        fs::create_dir_all(root.join("maps"))?;
        let marker = root.join("repo.txt");
        if !marker.exists() {
            fs::write(
                &marker,
                format!("{REPO_MAGIC}\nformat {}\n", super::format::VERSION),
            )?;
        }
        Self::open(root)
    }

    /// Open an existing repository (main files only).
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        let marker = fs::read_to_string(root.join("repo.txt")).map_err(|e| {
            Error::Invalid(format!("{} is not a tag repository: {e}", root.display()))
        })?;
        if !marker.starts_with(REPO_MAGIC) {
            return Err(Error::Invalid(format!(
                "{} is not a tag repository",
                root.display()
            )));
        }
        let mut repo = TagRepo {
            root,
            variants: HashMap::new(),
            groups: Vec::new(),
        };
        repo.groups = repo.read_groups()?;
        Ok(repo)
    }

    /// Open for one retail map: tags the map had in another content read
    /// that content (its manifest's variants).
    pub fn for_map(root: impl AsRef<Path>, map: &str) -> Result<Self> {
        let mut repo = Self::open(root)?;
        let m = repo.manifest(map)?;
        repo.variants = m
            .tags
            .into_iter()
            .filter_map(|(k, v)| Some((k, v?)))
            .collect();
        Ok(repo)
    }

    /// The directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// File of a key (main content, or a variant).
    pub fn path_of(&self, key: &TagKey, variant: Option<&str>) -> PathBuf {
        let mut p = self.root.join("tags");
        for part in key.path.split('\\') {
            p.push(part);
        }
        let mut name = p.file_name().map(|n| n.to_os_string()).unwrap_or_default();
        name.push(".");
        name.push(group_ext(key.group));
        if let Some(v) = variant {
            name.push("~");
            name.push(v);
        }
        p.set_file_name(name);
        p
    }

    fn raw_path(&self, hash: &str) -> PathBuf {
        self.root.join("raw").join(&hash[..2]).join(hash)
    }

    /// Store a tag: as the main file if the key is new, nothing if the main
    /// file is the same, otherwise as a variant named by its hash.
    pub fn put(&mut self, tag: &LooseTag) -> Result<Stored> {
        let bytes = encode(tag);
        let main = self.path_of(&tag.key, None);
        match fs::read(&main) {
            Ok(have) if have == bytes => Ok(Stored::Same),
            Ok(have) => {
                // The key must really be this one (case-insensitive file
                // systems fold paths).
                let other = decode(&have)?;
                if other.key != tag.key {
                    return Err(Error::Invalid(format!(
                        "{} and {} share a file name",
                        other.key, tag.key
                    )));
                }
                let v = hex(&sha256(&bytes)[..6]);
                let path = self.path_of(&tag.key, Some(&v));
                if !path.exists() {
                    write_atomic(&path, &bytes)?;
                }
                Ok(Stored::Variant(v))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                write_atomic(&main, &bytes)?;
                Ok(Stored::New)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Whether the raw store has these bytes.
    pub fn has_raw(&self, raw: &RawRes) -> bool {
        self.raw_path(&raw.hex()).exists()
    }

    /// Store raw bytes by hash; returns the hash.
    pub fn put_raw(&mut self, bytes: &[u8]) -> Result<String> {
        let h = hex(&sha256(bytes));
        let p = self.raw_path(&h);
        if !p.exists() {
            write_atomic(&p, bytes)?;
        }
        Ok(h)
    }

    /// Raw bytes by hash.
    pub fn raw_by_hash(&self, hash: &str) -> Result<Vec<u8>> {
        if hash.len() < 2 {
            return Err(Error::Invalid(format!("bad raw hash {hash:?}")));
        }
        Ok(fs::read(self.raw_path(hash))?)
    }

    /// The tag file's bytes for a key (this map's variant if any).
    pub fn file(&self, key: &TagKey) -> Result<Option<Vec<u8>>> {
        let p = self.path_of(key, self.variants.get(key).map(String::as_str));
        match fs::read(p) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Record a map's manifest and header.
    pub fn put_manifest(&mut self, m: &Manifest, header: &[u8]) -> Result<()> {
        write_atomic(
            &self.root.join("maps").join(format!("{}.txt", m.map)),
            m.to_text().as_bytes(),
        )?;
        write_atomic(
            &self.root.join("maps").join(format!("{}.header", m.map)),
            header,
        )?;
        Ok(())
    }

    /// A map's manifest.
    pub fn manifest(&self, map: &str) -> Result<Manifest> {
        let p = self.root.join("maps").join(format!("{map}.txt"));
        let text = fs::read_to_string(&p)
            .map_err(|e| Error::Invalid(format!("no manifest {}: {e}", p.display())))?;
        Manifest::parse(&text)
    }

    /// A map's header.
    pub fn header(&self, map: &str) -> Result<Vec<u8>> {
        Ok(fs::read(
            self.root.join("maps").join(format!("{map}.header")),
        )?)
    }

    /// Every manifest's map name.
    pub fn maps(&self) -> Result<Vec<String>> {
        let mut out = Vec::new();
        for e in fs::read_dir(self.root.join("maps"))? {
            let n = e?.file_name().to_string_lossy().into_owned();
            if let Some(s) = n.strip_suffix(".txt") {
                out.push(s.to_owned());
            }
        }
        out.sort();
        Ok(out)
    }

    /// The map whose manifest names this scenario, if any.
    pub fn map_of_scenario(&self, scenario: &str) -> Result<Option<String>> {
        for m in self.maps()? {
            if self
                .manifest(&m)?
                .scenario
                .is_some_and(|k| k.path == scenario)
            {
                return Ok(Some(m));
            }
        }
        Ok(None)
    }

    fn read_groups(&self) -> Result<Vec<TagGroup>> {
        let Ok(text) = fs::read_to_string(self.root.join("groups.txt")) else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for line in text.lines() {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() != 3 {
                continue;
            }
            let g = |s: &str| (s != "-").then(|| parse_group(s)).flatten();
            if let Some(class) = g(f[0]) {
                out.push(TagGroup {
                    class,
                    parent: g(f[1]),
                    grandparent: g(f[2]),
                });
            }
        }
        Ok(out)
    }

    /// Add groups not yet in the table (in the order given).
    pub fn merge_groups(&mut self, groups: &[TagGroup]) -> Result<()> {
        let before = self.groups.len();
        for g in groups {
            if !self.groups.iter().any(|h| h.class == g.class) {
                self.groups.push(*g);
            }
        }
        if self.groups.len() != before || !self.root.join("groups.txt").exists() {
            let opt = |g: Option<FourCC>| g.map_or("-".to_owned(), group_ext);
            let text: String = self
                .groups
                .iter()
                .map(|g| {
                    format!(
                        "{} {} {}\n",
                        group_ext(g.class),
                        opt(g.parent),
                        opt(g.grandparent)
                    )
                })
                .collect();
            write_atomic(&self.root.join("groups.txt"), text.as_bytes())?;
        }
        Ok(())
    }

    /// The string IDs every map starts with.
    pub fn strings(&self) -> Result<Vec<Option<String>>> {
        let Ok(text) = fs::read_to_string(self.root.join("strings.txt")) else {
            return Ok(Vec::new());
        };
        Ok(text
            .lines()
            .map(|l| (l != "\\-").then(|| unesc(l)))
            .collect())
    }

    /// Keep the longest prefix `strings` shares with the stored list (the
    /// whole of it for the first map).
    pub fn merge_strings(&mut self, strings: &[Option<String>]) -> Result<usize> {
        let path = self.root.join("strings.txt");
        let have = if path.exists() {
            let old = self.strings()?;
            let n = old.iter().zip(strings).take_while(|(a, b)| a == b).count();
            strings[..n].to_vec()
        } else {
            strings.to_vec()
        };
        let text: String = have
            .iter()
            .map(|s| s.as_deref().map_or("\\-".to_owned(), esc) + "\n")
            .collect();
        write_atomic(&path, text.as_bytes())?;
        Ok(have.len())
    }

    /// Every main tag file's key, by walking `tags/` (slow on a big
    /// repository).
    pub fn scan(&self) -> Result<Vec<TagKey>> {
        let mut out = Vec::new();
        let base = self.root.join("tags");
        let mut todo = vec![base.clone()];
        while let Some(d) = todo.pop() {
            for e in fs::read_dir(&d)? {
                let e = e?;
                let p = e.path();
                if e.file_type()?.is_dir() {
                    todo.push(p);
                    continue;
                }
                let name = e.file_name().to_string_lossy().into_owned();
                if name.contains('~') {
                    continue;
                }
                let Some((stem, g)) = name.rsplit_once('.') else {
                    continue;
                };
                let Some(group) = parse_group(g) else {
                    continue;
                };
                let rel = p
                    .parent()
                    .expect("in tags")
                    .strip_prefix(&base)
                    .expect("under");
                let mut path: Vec<String> = rel
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect();
                path.push(stem.to_owned());
                out.push(TagKey::new(group, path.join("\\")));
            }
        }
        out.sort();
        Ok(out)
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(d) = path.parent() {
        fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

impl TagSource for TagRepo {
    fn keys(&self) -> Vec<TagKey> {
        self.scan().unwrap_or_default()
    }

    fn read(&mut self, key: &TagKey) -> Result<Option<LooseTag>> {
        let Some(bytes) = self.file(key)? else {
            return Ok(None);
        };
        let tag = decode(&bytes)?;
        if tag.key != *key {
            return Err(Error::Invalid(format!(
                "{} holds {}, not {key}",
                self.path_of(key, None).display(),
                tag.key
            )));
        }
        Ok(Some(tag))
    }

    fn raw(&mut self, raw: &RawRes) -> Result<Vec<u8>> {
        let b = self.raw_by_hash(&raw.hex())?;
        if b.len() != raw.len as usize {
            return Err(Error::Invalid(format!(
                "raw {} has the wrong length",
                raw.hex()
            )));
        }
        Ok(b)
    }

    fn groups(&self) -> Vec<TagGroup> {
        self.groups.clone()
    }
}

/// Counts from storing one map.
#[derive(Clone, Debug, Default)]
pub struct PutReport {
    /// Tags new to the repository.
    pub new: usize,
    /// Tags the repository already had, identical.
    pub same: usize,
    /// Tags kept as variants: key → variant.
    pub variants: BTreeMap<TagKey, String>,
    /// Tags with losses (fields left empty).
    pub lossy: Vec<TagKey>,
    /// Tags refused.
    pub refused: Vec<(TagKey, String)>,
    /// Raw resources new to the store, and their bytes.
    pub raw_new: (usize, u64),
}
