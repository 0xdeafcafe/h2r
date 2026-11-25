//! Extraction on compile (docs/engine/asset-store.md, "Extraction on
//! compile"): bring a store up to date with the disc and this build, the
//! one call `h2 build`, the pre-boot step of `h2 play`/`run`/`serve` and
//! Studio's workspace all make.
//!
//! [`prepare`] takes the disc's map files ([`DiscMap`]) and does, in order:
//!
//! 1. takes the store's lock, so two callers never write it at once (the
//!    second waits, then finds nothing stale);
//! 2. gives each map file its SHA-256: from `<store>/stamps.txt` while the
//!    file's size, modification time and place are as recorded, by reading
//!    it otherwise ([`Prepared::hashed`] counts those);
//! 3. runs [`extract::plan`] and, when it is not empty, [`extract::update`].
//!    With nothing stale no map is opened ([`Prepared::opened`] is 0);
//! 4. writes the asset catalogue ([`catalogue::update`]) when it is not
//!    current, and after every extraction, since a file can change while
//!    no manifest does ([`Prepared::catalogue`]).
//!
//! With no disc ([`prepare`] given `None`) the maps are read from the store
//! itself: [`StoreFile`] is the same bytes as the disc's file, so a layout
//! change or an extractor version change re-extracts from the store. A reset
//! empties `maps/`, so the skeletons and extent tables the run reads are
//! first copied to `<store>/reextract/`, which a stopped run leaves for the
//! next one and a finished run removes. A map the store has no skeleton for
//! needs the disc, and the error says so.
use super::catalogue;
use super::extract::{self, Layouts, MapWork, Plan, SharedFiles, Updated};
use super::skeleton::{Extents, Skeleton, StoreFile};
use super::store::{PutReport, is_shared_cache, write_atomic};
use super::{Error, Result, TagRepo};
use crate::schema::Schema;
use map::{Map, ResourceFile};
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// `<store>/stamps.txt`: what each disc map file looked like when it was
/// last hashed.
pub const STAMPS_FILE: &str = "stamps.txt";
/// `<store>/extract.lock`: held while a caller writes the store.
pub const LOCK_FILE: &str = "extract.lock";
/// `<store>/reextract/`: the skeletons and extent tables a re-extraction
/// from the store reads after `maps/` is emptied.
pub const STASH_DIR: &str = "reextract";

/// One map file of the disc: a whole file, or an entry's region of an
/// image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscMap {
    /// The map's name (`lockout` for `lockout.map`).
    pub stem: String,
    /// The file it is read from: the map's own, or the image.
    pub path: PathBuf,
    /// Where the map starts in that file.
    pub base: u64,
    /// The map file's length.
    pub len: u64,
}

impl DiscMap {
    /// The map, read from the disc.
    pub fn open(&self) -> Result<Map<Reader>> {
        let f = BufReader::new(File::open(&self.path)?);
        Ok(Map::from_region(Reader::Disc(f), self.base, self.len)?)
    }

    /// SHA-256 of the map file's bytes (hex): the disc's identity.
    pub fn file_hash(&self) -> Result<String> {
        let mut f = File::open(&self.path)?;
        f.seek(SeekFrom::Start(self.base))?;
        extract::file_hash(f.take(self.len))
    }

    fn stamp(&self) -> Result<Stamp> {
        let md = fs::metadata(&self.path)?;
        let mtime = md
            .modified()?
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        Ok(Stamp {
            size: md.len(),
            mtime,
            base: self.base,
            len: self.len,
        })
    }
}

/// A map's bytes, from the disc or from the store: one type, so one
/// [`SharedFiles`] holds both.
pub enum Reader {
    /// A region of a file on the disc.
    Disc(BufReader<File>),
    /// Reassembled from the store.
    Store(Box<StoreFile>),
}

impl Read for Reader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Reader::Disc(r) => r.read(buf),
            Reader::Store(s) => s.read(buf),
        }
    }
}

impl Seek for Reader {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        match self {
            Reader::Disc(r) => r.seek(to),
            Reader::Store(s) => s.seek(to),
        }
    }
}

/// The identity of a disc file when it was hashed. Equal stamps mean the
/// same bytes (the stamp rule of `workspace::unit_key`).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp {
    /// Size of the file read from (the image, for an entry).
    size: u64,
    /// Its modification time, nanoseconds since the epoch.
    mtime: u128,
    base: u64,
    len: u64,
}

type Stamps = BTreeMap<String, (Stamp, String)>;

fn read_stamps(root: &Path) -> Stamps {
    let mut out = Stamps::new();
    let Ok(text) = fs::read_to_string(root.join(STAMPS_FILE)) else {
        return out;
    };
    for line in text.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        let [stem, size, mtime, base, len, sha] = f.as_slice() else {
            continue;
        };
        let (Ok(size), Ok(mtime), Ok(base), Ok(len)) =
            (size.parse(), mtime.parse(), base.parse(), len.parse())
        else {
            continue;
        };
        out.insert(
            (*stem).to_owned(),
            (
                Stamp {
                    size,
                    mtime,
                    base,
                    len,
                },
                (*sha).to_owned(),
            ),
        );
    }
    out
}

fn write_stamps(root: &Path, stamps: &Stamps) -> Result<()> {
    let mut s = String::new();
    for (stem, (st, sha)) in stamps {
        s.push_str(&format!(
            "{stem}\t{}\t{}\t{}\t{}\t{sha}\n",
            st.size, st.mtime, st.base, st.len
        ));
    }
    write_atomic(&root.join(STAMPS_FILE), s.as_bytes())
}

/// Each map's SHA-256: a stamp that still matches gives it, else the file
/// is read. Stamps are rewritten when any changed.
fn identity(root: &Path, maps: &[&DiscMap], hashed: &mut usize) -> Result<Vec<(String, String)>> {
    let old = read_stamps(root);
    let mut new = old.clone();
    let mut out = Vec::with_capacity(maps.len());
    for m in maps {
        let st = m.stamp()?;
        let sha = match old.get(&m.stem) {
            Some((s, h)) if *s == st => h.clone(),
            _ => {
                *hashed += 1;
                m.file_hash()?
            }
        };
        new.insert(m.stem.clone(), (st, sha.clone()));
        out.push((m.stem.clone(), sha));
    }
    if new != old {
        write_stamps(root, &new)?;
    }
    Ok(out)
}

/// The store's lock, held until it is dropped.
struct Lock {
    _file: File,
}

impl Lock {
    fn acquire(root: &Path, say: &mut dyn FnMut(&str)) -> Result<Self> {
        fs::create_dir_all(root)?;
        let f = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(root.join(LOCK_FILE))?;
        match f.try_lock() {
            Ok(()) => {}
            Err(fs::TryLockError::WouldBlock) => {
                say("another process is extracting into this store; waiting for it");
                f.lock()?;
            }
            Err(fs::TryLockError::Error(e)) => return Err(e.into()),
        }
        Ok(Lock { _file: f })
    }
}

fn shared_file(stem: &str) -> Option<ResourceFile> {
    match stem {
        "mainmenu" => Some(ResourceFile::MainMenu),
        "shared" => Some(ResourceFile::Shared),
        "single_player_shared" => Some(ResourceFile::SinglePlayerShared),
        _ => None,
    }
}

fn stash_path(root: &Path, stem: &str, ext: &str) -> PathBuf {
    root.join(STASH_DIR).join(format!("{stem}.{ext}"))
}

/// A map from the store: from the stash when a stopped run left it there,
/// else from `maps/`.
fn open_store_file(root: &Path, stem: &str) -> Result<StoreFile> {
    if stash_path(root, stem, "extents").is_file() {
        let skel = Skeleton::decode(&fs::read(stash_path(root, stem, "skel"))?)?;
        let ext = Extents::parse(&fs::read_to_string(stash_path(root, stem, "extents"))?)?;
        StoreFile::from_parts(stem, skel, ext, root.join("raw"))
    } else {
        StoreFile::open(root, stem)
    }
}

/// The maps the store can give back, with each one's file hash: the
/// stash's, then `maps/`'s extent tables.
fn store_identity(root: &Path) -> Result<Vec<(String, String)>> {
    let mut stems = BTreeSet::new();
    for (dir, ext) in [
        (root.join(STASH_DIR), ".extents"),
        (root.join("maps"), ".extents"),
    ] {
        let Ok(rd) = fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            if let Some(s) = e.file_name().to_string_lossy().strip_suffix(ext) {
                stems.insert(s.to_owned());
            }
        }
    }
    let mut out = Vec::new();
    for s in stems {
        let path = if stash_path(root, &s, "extents").is_file() {
            stash_path(root, &s, "extents")
        } else {
            root.join("maps").join(format!("{s}.extents"))
        };
        let skel_ok = if path.starts_with(root.join(STASH_DIR)) {
            stash_path(root, &s, "skel").is_file()
        } else {
            StoreFile::exists(root, &s)
        };
        if !skel_ok {
            continue;
        }
        let ext = Extents::parse(&fs::read_to_string(&path)?)?;
        out.push((s, ext.file));
    }
    Ok(out)
}

/// Copy each map's skeleton and extent table aside, skeleton first, so a
/// map is in the stash only when both are. A map already there is kept.
pub(super) fn stash(root: &Path, stems: &[String]) -> Result<()> {
    fs::create_dir_all(root.join(STASH_DIR))?;
    for s in stems {
        if stash_path(root, s, "extents").is_file() {
            continue;
        }
        for ext in ["skel", "extents"] {
            let from = root.join("maps").join(format!("{s}.{ext}"));
            let to = stash_path(root, s, ext);
            let part = to.with_extension(format!("{ext}.part"));
            fs::copy(&from, &part)?;
            fs::rename(&part, &to)?;
        }
    }
    Ok(())
}

/// Where a run's maps came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    /// The disc's map files.
    Disc,
    /// The store's own skeletons and extents.
    Store,
}

/// How a run behaves.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Read each stored tag back and compare it.
    pub verify: bool,
    /// Take every map whole, though nothing moved.
    pub force: bool,
    /// Only these maps (stems); `None`: all.
    pub wanted: Option<Vec<String>>,
}

/// What a [`prepare`] did.
#[derive(Clone, Debug)]
pub struct Prepared {
    /// What was stale.
    pub plan: Plan,
    /// Where maps were read from.
    pub input: Input,
    /// Map files read through to hash them (no stamp matched).
    pub hashed: usize,
    /// Maps opened for extraction, shared caches included.
    pub opened: usize,
    /// What the extraction did; `None` when the plan was empty.
    pub updated: Option<Updated>,
    /// What building the catalogue did; `None` when it was current.
    pub catalogue: Option<catalogue::Built>,
}

/// Bring the store at `root` up to date. `disc` is the disc's map files, or
/// `None` to read from the store alone. `on_plan` is told once the plan is
/// known (before any map is opened) and `on_map` of each map done; `say`
/// gets the lines worth showing a person.
pub fn prepare(
    root: &Path,
    schema: &Schema,
    disc: Option<&[DiscMap]>,
    opts: &Options,
    say: &mut dyn FnMut(&str),
    on_plan: &mut dyn FnMut(&Plan, Input),
    on_map: &mut dyn FnMut(&MapWork, &PutReport),
) -> Result<Prepared> {
    let _lock = Lock::acquire(root, say)?;
    let mut repo = TagRepo::create(root)?;
    let layouts = Layouts::new(schema);
    let wanted = |stem: &str| {
        opts.wanted
            .as_ref()
            .is_none_or(|w| w.iter().any(|x| x == stem))
    };
    let mut hashed = 0usize;
    let mut by_stem: BTreeMap<&str, &DiscMap> = BTreeMap::new();
    let (input, ident) = match disc {
        Some(maps) => {
            let maps: Vec<&DiscMap> = maps.iter().filter(|m| wanted(&m.stem)).collect();
            if maps.is_empty() {
                return Err(Error::Invalid("no maps to extract from the disc".into()));
            }
            for &m in &maps {
                by_stem.insert(m.stem.as_str(), m);
            }
            (Input::Disc, identity(root, &maps, &mut hashed)?)
        }
        None => {
            let ident: Vec<_> = store_identity(root)?
                .into_iter()
                .filter(|(s, _)| wanted(s))
                .collect();
            let missing: Vec<String> = repo
                .extract_record()?
                .map(|r| r.maps.into_keys().collect::<Vec<_>>())
                .unwrap_or_default()
                .into_iter()
                .filter(|s| wanted(s) && !ident.iter().any(|(i, _)| i == s))
                .collect();
            if ident.is_empty() || !missing.is_empty() {
                return Err(Error::Invalid(if ident.is_empty() {
                    format!(
                        "no disc, and the store at {} holds no map it can give back",
                        root.display()
                    )
                } else {
                    format!(
                        "the store at {} has no skeleton for {}; extracting again needs the disc image",
                        root.display(),
                        missing.join(", ")
                    )
                }));
            }
            (Input::Store, ident)
        }
    };
    let plan = extract::plan(&repo, &layouts, &ident, opts.force)?;
    on_plan(&plan, input);
    if plan.is_empty() {
        let catalogue = catalogue::update(root, schema, false)?;
        return Ok(Prepared {
            plan,
            input,
            hashed,
            opened: 0,
            updated: None,
            catalogue,
        });
    }
    if input == Input::Store {
        for w in &plan.maps {
            if !StoreFile::exists(root, &w.map) && !stash_path(root, &w.map, "extents").is_file() {
                return Err(Error::Invalid(format!(
                    "{} has no skeleton in the store; extracting it again needs the disc image",
                    w.map
                )));
            }
        }
        if plan.reset {
            let stems: Vec<String> = ident.iter().map(|(s, _)| s.clone()).collect();
            stash(root, &stems)?;
        }
    }
    let opened = Cell::new(0usize);
    let mut open = |stem: &str| -> Result<Map<Reader>> {
        opened.set(opened.get() + 1);
        match by_stem.get(stem) {
            Some(m) => m.open(),
            None => {
                let f = open_store_file(root, stem)?;
                let len = f.len();
                Ok(Map::from_region(Reader::Store(Box::new(f)), 0, len)?)
            }
        }
    };
    let mut shared = SharedFiles::<Reader>::default();
    let mut caches: Vec<&str> = ident
        .iter()
        .map(|(s, _)| s.as_str())
        .filter(|s| is_shared_cache(s))
        .collect();
    caches.sort_unstable();
    for stem in caches {
        if let Some(f) = shared_file(stem) {
            shared.set(schema, f, open(stem)?)?;
        }
    }
    let updated = extract::update(
        &mut repo,
        &plan,
        &layouts,
        schema,
        &mut shared,
        &mut open,
        opts.verify,
        on_map,
    )?;
    drop(shared);
    match fs::remove_dir_all(root.join(STASH_DIR)) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let catalogue = catalogue::update(root, schema, true)?;
    Ok(Prepared {
        plan,
        input,
        hashed,
        opened: opened.get(),
        updated: Some(updated),
        catalogue,
    })
}
