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
//! <repo>/maps/<map>.skel           its skeleton: the bytes not in raw files (skeleton.rs)
//! <repo>/maps/<map>.extents        its extent table: every other range by raw hash
//! <repo>/extract.toml              versions and disc identity ([`ExtractRecord`])
//! <repo>/assets.idx                the asset catalogue (catalogue.rs): each asset by hash
//! <repo>/conflicts.txt             keys with variants and the maps that differ
//! ```
//!
//! Path backslashes become directories. A group whose bytes are not all
//! letters, digits or `!_-+.` is written as 8 hex digits, as in the schema
//! file. `<h>` is the first 12 hex digits of the SHA-256 of the variant's
//! file. A manifest names, for each tag of its map, which content the map
//! has, so a repository opened for that map ([`TagRepo::for_map`]) reads
//! the map's own variants.
//!
//! The main file of a key is the content of the first map, in
//! [`extraction_order`], that lists the key. A partial re-extraction can
//! leave it otherwise; [`TagRepo::settle`] puts it back.
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
    /// Count raw resources instead of writing them (to size a repository
    /// without the disk space): hashes seen and their bytes.
    pub count_raw: Option<(std::collections::HashSet<String>, u64)>,
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
pub(super) fn esc(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
}

pub(super) fn unesc(s: &str) -> String {
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

pub(super) fn parse_key(g: &str, p: &str) -> Result<TagKey> {
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

/// File name of the extraction record.
pub const EXTRACT_FILE: &str = "extract.toml";

/// Whether `stem` is one of the shared caches.
pub fn is_shared_cache(stem: &str) -> bool {
    matches!(stem, "mainmenu" | "shared" | "single_player_shared")
}

/// Sort map stems in extraction order: the shared caches first, then the
/// other maps by name. A key's main file is the first of these to hold it.
pub fn extraction_order(stems: &mut [String]) {
    stems.sort_by(|a, b| (!is_shared_cache(a), a).cmp(&(!is_shared_cache(b), b)));
}

/// What the tree was extracted from and under which versions
/// (`<repo>/extract.toml`). Written by the extractor only.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExtractRecord {
    /// [`super::extract::EXTRACT_VERSION`] the tree was extracted with.
    pub extractor: u32,
    /// [`super::format::VERSION`] of its tag files.
    pub format: u16,
    /// Every extracted map, by stem.
    pub maps: BTreeMap<String, MapRecord>,
}

/// One extracted map.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MapRecord {
    /// SHA-256 of the map file (lowercase hex): the disc's identity.
    pub file: String,
    /// Every group the map has tags of.
    pub groups: BTreeMap<FourCC, GroupRecord>,
}

/// One group of one extracted map.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GroupRecord {
    /// The layout hash its tags were extracted under (hex).
    pub layout: String,
    /// Tags of the group in the map.
    pub tags: u32,
    /// Of those, taken with losses.
    pub lossy: u32,
    /// Of those, refused.
    pub refused: u32,
}

impl MapRecord {
    /// Lossy tags over every group.
    pub fn lossy(&self) -> u32 {
        self.groups.values().map(|g| g.lossy).sum()
    }
}

/// A string as TOML (basic string).
fn toml_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A TOML basic string at the start of `s`: (its value, the rest).
fn toml_take_str(s: &str) -> Option<(String, &str)> {
    let s = s.strip_prefix('"')?;
    let mut out = String::new();
    let mut it = s.char_indices();
    while let Some((i, c)) = it.next() {
        match c {
            '"' => return Some((out, &s[i + 1..])),
            '\\' => match it.next()?.1 {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                o => out.push(o),
            },
            c => out.push(c),
        }
    }
    None
}

impl ExtractRecord {
    /// TOML text: top-level versions, then `[maps."<stem>"]` with the file
    /// hash and `[maps."<stem>".groups]` with one inline table per group.
    pub fn to_text(&self) -> String {
        let mut s = String::from(
            "# h2 tags extract: what this tree was extracted from, under which\n\
             # versions (docs/formats/tag-repo.md). Game data: never commit it.\n",
        );
        s.push_str(&format!("extractor = {}\n", self.extractor));
        s.push_str(&format!("format = {}\n", self.format));
        for (stem, m) in &self.maps {
            s.push_str(&format!("\n[maps.{}]\n", toml_str(stem)));
            s.push_str(&format!("file = {}\n", toml_str(&m.file)));
            s.push_str(&format!("\n[maps.{}.groups]\n", toml_str(stem)));
            for (g, r) in &m.groups {
                s.push_str(&format!(
                    "{} = {{ layout = {}, tags = {}, lossy = {}, refused = {} }}\n",
                    toml_str(&group_ext(*g)),
                    toml_str(&r.layout),
                    r.tags,
                    r.lossy,
                    r.refused
                ));
            }
        }
        s
    }

    /// Parse [`ExtractRecord::to_text`] (that shape of TOML only).
    pub fn parse(text: &str) -> Result<Self> {
        enum At {
            Top,
            Map(String),
            Groups(String),
        }
        let mut rec = ExtractRecord::default();
        let mut at = At::Top;
        for (n, line) in text.lines().enumerate() {
            let bad = || Error::Invalid(format!("{EXTRACT_FILE} line {}: {line:?}", n + 1));
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(head) = line.strip_prefix("[maps.") {
                let (stem, rest) = toml_take_str(head).ok_or_else(bad)?;
                rec.maps.entry(stem.clone()).or_default();
                at = match rest {
                    "]" => At::Map(stem),
                    ".groups]" => At::Groups(stem),
                    _ => return Err(bad()),
                };
                continue;
            }
            match &at {
                At::Top => {
                    let (k, v) = line.split_once('=').ok_or_else(bad)?;
                    let v = v.trim();
                    match k.trim() {
                        "extractor" => rec.extractor = v.parse().map_err(|_| bad())?,
                        "format" => rec.format = v.parse().map_err(|_| bad())?,
                        _ => return Err(bad()),
                    }
                }
                At::Map(stem) => {
                    let (k, v) = line.split_once('=').ok_or_else(bad)?;
                    if k.trim() != "file" {
                        return Err(bad());
                    }
                    let (file, _) = toml_take_str(v.trim()).ok_or_else(bad)?;
                    rec.maps.get_mut(stem).expect("entered").file = file;
                }
                At::Groups(stem) => {
                    let (g, rest) = toml_take_str(line).ok_or_else(bad)?;
                    let group = parse_group(&g).ok_or_else(bad)?;
                    let table = rest
                        .trim()
                        .strip_prefix('=')
                        .map(str::trim)
                        .and_then(|t| t.strip_prefix('{'))
                        .and_then(|t| t.strip_suffix('}'))
                        .ok_or_else(bad)?;
                    let mut r = GroupRecord::default();
                    for kv in table.split(',') {
                        let (k, v) = kv.split_once('=').ok_or_else(bad)?;
                        let v = v.trim();
                        match k.trim() {
                            "layout" => r.layout = toml_take_str(v).ok_or_else(bad)?.0,
                            "tags" => r.tags = v.parse().map_err(|_| bad())?,
                            "lossy" => r.lossy = v.parse().map_err(|_| bad())?,
                            "refused" => r.refused = v.parse().map_err(|_| bad())?,
                            _ => return Err(bad()),
                        }
                    }
                    rec.maps
                        .get_mut(stem)
                        .expect("entered")
                        .groups
                        .insert(group, r);
                }
            }
        }
        Ok(rec)
    }
}

/// What [`TagRepo::settle`] changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settled {
    /// Keys whose main file became the first map's content.
    pub promoted: usize,
    /// Manifests rewritten.
    pub manifests: usize,
    /// Keys with variants (lines of `conflicts.txt`).
    pub conflicts: usize,
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
            count_raw: None,
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

    pub(crate) fn raw_path(&self, hash: &str) -> PathBuf {
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
        match &self.count_raw {
            Some((seen, _)) => seen.contains(&raw.hex()),
            None => self.raw_path(&raw.hex()).exists(),
        }
    }

    /// Store raw bytes by hash; returns the hash.
    pub fn put_raw(&mut self, bytes: &[u8]) -> Result<String> {
        let h = hex(&sha256(bytes));
        if let Some((seen, total)) = &mut self.count_raw {
            if seen.insert(h.clone()) {
                *total += bytes.len() as u64;
            }
            return Ok(h);
        }
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
        // The header first: a map is extracted once its manifest exists.
        write_atomic(
            &self.root.join("maps").join(format!("{}.header", m.map)),
            header,
        )?;
        self.write_manifest(m)
    }

    /// Rewrite a map's manifest (its header stays).
    pub fn write_manifest(&self, m: &Manifest) -> Result<()> {
        write_atomic(
            &self.root.join("maps").join(format!("{}.txt", m.map)),
            m.to_text().as_bytes(),
        )
    }

    /// Whether a map's manifest exists.
    pub fn has_manifest(&self, map: &str) -> bool {
        self.root.join("maps").join(format!("{map}.txt")).is_file()
    }

    /// The extraction record, if the tree has one.
    pub fn extract_record(&self) -> Result<Option<ExtractRecord>> {
        match fs::read_to_string(self.root.join(EXTRACT_FILE)) {
            Ok(t) => ExtractRecord::parse(&t).map(Some),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Write the extraction record.
    pub fn put_extract_record(&self, r: &ExtractRecord) -> Result<()> {
        write_atomic(&self.root.join(EXTRACT_FILE), r.to_text().as_bytes())
    }

    /// Record one map as extracted (after its manifest is written, so an
    /// interrupted map is never recorded).
    pub fn record_map(&self, stem: &str, m: MapRecord) -> Result<()> {
        let mut r = self.extract_record()?.unwrap_or_default();
        r.maps.insert(stem.to_owned(), m);
        self.put_extract_record(&r)
    }

    /// Empty the tree for a re-extraction under new versions: tag files,
    /// manifests, the group and string tables, `conflicts.txt` and the
    /// asset catalogue go; raw resources (by hash) stay. Then the record
    /// says `extractor` and `format` with no map, so an interrupted reset
    /// runs again only if it did not get that far.
    pub fn reset(&mut self, extractor: u32, format: u16) -> Result<()> {
        for d in ["tags", "maps"] {
            let p = self.root.join(d);
            match fs::remove_dir_all(&p) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
            fs::create_dir_all(&p)?;
        }
        for f in [
            "groups.txt",
            "strings.txt",
            "conflicts.txt",
            super::catalogue::CATALOGUE_FILE,
        ] {
            match fs::remove_file(self.root.join(f)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        self.groups.clear();
        self.variants.clear();
        self.put_extract_record(&ExtractRecord {
            extractor,
            format,
            maps: BTreeMap::new(),
        })
    }

    /// Make each key's main file the content of the first map (in
    /// [`extraction_order`]) that lists it, and rewrite `conflicts.txt`
    /// from the manifests. Every map reads the same content before and
    /// after. Safe to interrupt and run again: maps that read the old main
    /// file are moved to a variant of it first, then the main file is
    /// replaced, then the first map's entries name it.
    pub fn settle(&mut self) -> Result<Settled> {
        let mut stems = self.maps()?;
        extraction_order(&mut stems);
        let mut mans = Vec::with_capacity(stems.len());
        for s in &stems {
            mans.push(self.manifest(s)?);
        }
        // Each key's entries, (manifest, slot), in extraction order.
        let mut by_key: BTreeMap<TagKey, Vec<(usize, usize)>> = BTreeMap::new();
        for (mi, m) in mans.iter().enumerate() {
            for (ti, (k, _)) in m.tags.iter().enumerate() {
                by_key.entry(k.clone()).or_default().push((mi, ti));
            }
        }
        let mut changed = vec![false; mans.len()];
        let mut promote = Vec::new();
        // 1. Maps reading a main file that is not the first content move to
        //    a variant of it.
        for (key, entries) in &by_key {
            let (fm, ft) = entries[0];
            let Some(v) = mans[fm].tags[ft].1.clone() else {
                continue;
            };
            let readers: Vec<_> = entries
                .iter()
                .copied()
                .filter(|&(mi, ti)| mans[mi].tags[ti].1.is_none())
                .collect();
            if !readers.is_empty() {
                let old = fs::read(self.path_of(key, None))?;
                let h = hex(&sha256(&old)[..6]);
                let p = self.path_of(key, Some(h.as_str()));
                if !p.exists() {
                    write_atomic(&p, &old)?;
                }
                for (mi, ti) in readers {
                    mans[mi].tags[ti].1 = Some(h.clone());
                    changed[mi] = true;
                }
            }
            promote.push((key.clone(), v));
        }
        let mut written = 0;
        for (mi, m) in mans.iter().enumerate() {
            if changed[mi] {
                self.write_manifest(m)?;
                written += 1;
            }
        }
        // 2. The first content becomes the main file, and every entry
        //    naming it reads the main file.
        let mut changed = vec![false; mans.len()];
        for (key, v) in &promote {
            let bytes = fs::read(self.path_of(key, Some(v.as_str())))?;
            write_atomic(&self.path_of(key, None), &bytes)?;
            for &(mi, ti) in &by_key[key] {
                if mans[mi].tags[ti].1.as_deref() == Some(v.as_str()) {
                    mans[mi].tags[ti].1 = None;
                    changed[mi] = true;
                }
            }
        }
        for (mi, m) in mans.iter().enumerate() {
            if changed[mi] {
                self.write_manifest(m)?;
                written += 1;
            }
        }
        for (key, v) in &promote {
            let _ = fs::remove_file(self.path_of(key, Some(v.as_str())));
        }
        // 3. Keys with variants and the maps that have them.
        let mut lines = String::new();
        let mut conflicts = 0;
        for (key, entries) in &by_key {
            let maps: Vec<&str> = entries
                .iter()
                .filter(|&&(mi, ti)| mans[mi].tags[ti].1.is_some())
                .map(|&(mi, _)| mans[mi].map.as_str())
                .collect();
            if !maps.is_empty() {
                lines.push_str(&format!("{key}\t{}\n", maps.join(",")));
                conflicts += 1;
            }
        }
        let path = self.root.join("conflicts.txt");
        if fs::read_to_string(&path).ok().as_deref() != Some(lines.as_str()) {
            write_atomic(&path, lines.as_bytes())?;
        }
        Ok(Settled {
            promoted: promote.len(),
            manifests: written,
            conflicts,
        })
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

pub(super) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
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
    /// The map's skeleton and extent table, when the whole map was taken
    /// ([`super::extract::store_skeleton`]).
    pub skeleton: Option<super::skeleton::SkeletonReport>,
}
