use std::{
    fs,
    io::{ErrorKind, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use forge_doc::{ForgeDoc, Limits, MAX_DOC_BYTES};

fn filename(name: &str) -> Result<String, String> {
    if !content::manifest::is_map_name(name) {
        return Err("Use 1 to 64 letters, digits, dashes or underscores for the file name.".into());
    }
    Ok(format!("{name}.h2forge"))
}

// Check the real destination before creating the first missing directory.
// This covers a .git file in a linked worktree as well as a .git directory.
fn outside_checkout(path: &Path) -> Result<(), String> {
    if path
        .ancestors()
        .any(|dir| dir.join(".git").symlink_metadata().is_ok())
    {
        return Err("Forge documents belong outside Git checkouts.".into());
    }
    Ok(())
}

fn check_dir(path: &Path) -> Result<PathBuf, String> {
    let real = path.canonicalize().map_err(|e| e.to_string())?;
    outside_checkout(&real)?;
    if !real.is_dir() {
        return Err(format!("{} is not a directory.", real.display()));
    }
    Ok(real)
}

fn ensure_dir(path: &Path) -> Result<PathBuf, String> {
    match path.symlink_metadata() {
        Ok(_) => check_dir(path),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            let parent = path.parent().ok_or_else(|| e.to_string())?;
            let dir = ensure_dir(parent)?.join(path.file_name().ok_or("Invalid Forge directory.")?);
            match fs::create_dir(&dir) {
                Ok(()) => (),
                Err(e) if e.kind() == ErrorKind::AlreadyExists => (),
                Err(e) => return Err(e.to_string()),
            }
            check_dir(&dir)
        }
        Err(e) => Err(e.to_string()),
    }
}

pub(crate) fn save(data: &Path, name: &str, doc: &ForgeDoc) -> Result<(), String> {
    let name = filename(name)?;
    let bytes = doc.canonical_json().map_err(|e| e.to_string())?;
    let path = std::path::absolute(data.join("forge")).map_err(|e| e.to_string())?;
    let dir = ensure_dir(&path)?;
    content::store::write_atomic(&dir.join(name), bytes.as_bytes()).map_err(|e| e.to_string())
}

/// Write and sync one hidden sibling. Hard-link it into the first available
/// name: creating the link cannot replace an existing file and readers see
/// only the complete document. Concurrent writers choose different names.
pub(crate) fn save_new(data: &Path, base: &str, doc: &ForgeDoc) -> Result<String, String> {
    filename(base)?;
    let bytes = doc.canonical_json().map_err(|e| e.to_string())?;
    let path = std::path::absolute(data.join("forge")).map_err(|e| e.to_string())?;
    let dir = ensure_dir(&path)?;
    create_unique(&dir, base, "h2forge", bytes.as_bytes())
}

/// Create `<base>.<ext>` in `dir`, or `<base>-2.<ext>` and on, holding
/// `bytes`, and return the stem it took.
fn create_unique(dir: &Path, base: &str, ext: &str, bytes: &[u8]) -> Result<String, String> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let (tmp, mut file) = loop {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let tmp = dir.join(format!(".forge-{}-{n}.tmp", std::process::id()));
        match fs::File::create_new(&tmp) {
            Ok(file) => break (tmp, file),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        }
    };
    let result = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);
    let result = result.and_then(|()| {
        for n in 1..=u64::MAX {
            let name = if n == 1 {
                base.to_string()
            } else {
                let suffix = format!("-{n}");
                format!("{}{suffix}", &base[..base.len().min(64 - suffix.len())])
            };
            match fs::hard_link(&tmp, dir.join(format!("{name}.{ext}"))) {
                Ok(()) => return Ok(name),
                Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(std::io::Error::other("No Forge file name available."))
    });
    let _ = fs::remove_file(tmp);
    result.map_err(|e| e.to_string())
}

pub(crate) fn load(data: &Path, name: &str, limits: &Limits) -> Result<ForgeDoc, String> {
    let path = data.join("forge").join(filename(name)?);
    if !path
        .symlink_metadata()
        .map_err(|e| e.to_string())?
        .is_file()
    {
        return Err("A Forge document must be a regular file.".into());
    }
    outside_checkout(&path.canonicalize().map_err(|e| e.to_string())?)?;
    let mut bytes = vec![];
    fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(MAX_DOC_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    ForgeDoc::from_bytes(&bytes, limits).map_err(|e| e.to_string())
}

/// The saved Forge map `name` under `data`, checked against the default
/// limits: the document a lobby's start gate applies to the map file's
/// census (docs/game/rules-start-gate.md, "Where it runs").
pub fn load_saved(data: &Path, name: &str) -> Result<ForgeDoc, String> {
    load(data, name, &Limits::default())
}

/// File stems suitable for the controller's Load picker.
pub fn saved_names(data: &Path) -> Result<Vec<String>, String> {
    let entries = match fs::read_dir(data.join("forge")) {
        Ok(entries) => entries,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e.to_string()),
    };
    let mut names = vec![];
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.file_type().map_err(|e| e.to_string())?.is_file() {
            continue;
        }
        let filename = entry.file_name();
        if let Some(name) = filename.to_str().and_then(|s| s.strip_suffix(".h2forge"))
            && content::manifest::is_map_name(name)
        {
            names.push(name.to_owned());
        }
    }
    names.sort_unstable();
    Ok(names)
}

/// The map variant store (docs/game/forge-map-variants.md, "The map variant
/// file"): `<data>/map-variants/<base map>/<stem>.h2mapvariant`, with the ones
/// fetched from Live under `live/`.
pub mod variants {
    use super::*;
    use forge_doc::{BuilderVersions, Sha256, Tier};

    /// The extension of a map variant file.
    pub const VARIANT_EXT: &str = "h2mapvariant";

    const VARIANTS_DIR: &str = "map-variants";
    const LIVE_DIR: &str = "live";

    /// Why a variant on this machine cannot be used, as the rules table names
    /// it ("Rules and limits"). Such a variant is still listed, and can be
    /// renamed, duplicated and deleted, but not loaded.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Problem {
        /// This machine has no map of the base's name.
        BaseMissing,
        /// The installed map's SHA-256 is not the base's.
        BaseHash,
        /// `builder` differs from this build's.
        Builder,
        /// `format` is one this build does not read.
        Format,
        /// The document holds more than a map variant may.
        Tier,
        /// The file does not parse, or is in another map's directory.
        Unreadable,
    }

    impl Problem {
        /// The refusal's name.
        pub const fn code(self) -> &'static str {
            match self {
                Self::BaseMissing => "variant_base_missing",
                Self::BaseHash => "variant_base_hash",
                Self::Builder => "variant_builder",
                Self::Format => "variant_format",
                Self::Tier => "variant_tier",
                Self::Unreadable => "variant_unreadable",
            }
        }

        /// The sentence shown where the player is.
        pub fn sentence(self, name: &str, map: &str) -> String {
            match self {
                Self::BaseMissing => format!("{name} needs {map}, which is not installed."),
                Self::BaseHash => format!("{name} was made for another version of {map}."),
                Self::Builder | Self::Format => {
                    format!("{name} was made with another version of Forge.")
                }
                Self::Tier => format!("{name} is a Forge map, not a map variant."),
                Self::Unreadable => format!("{name} cannot be read."),
            }
        }
    }

    /// A store refusal, named.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum Error {
        /// The file cannot be used as a variant.
        Problem(Problem, String),
        /// The name breaks the variant name rule, or the map's name is no
        /// directory name.
        Name(String),
        /// Another variant of this map has the name, compared without case.
        NameTaken(String),
        /// No such file in this map's directory.
        Missing(String),
        /// The file system refused.
        Io(String),
    }

    impl Error {
        /// The refusal's name.
        pub fn code(&self) -> &'static str {
            match self {
                Self::Problem(p, _) => p.code(),
                Self::Name(_) => "variant_name",
                Self::NameTaken(_) => "variant_name_taken",
                Self::Missing(_) => "variant_file_missing",
                Self::Io(_) => "variant_io",
            }
        }
    }

    impl std::fmt::Display for Error {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::Problem(_, s) | Self::Name(s) | Self::Missing(s) | Self::Io(s) => {
                    f.write_str(s)
                }
                Self::NameTaken(name) => write!(f, "A map variant named {name} already exists."),
            }
        }
    }

    impl std::error::Error for Error {}

    fn io(e: impl ToString) -> Error {
        Error::Io(e.to_string())
    }

    /// Where a listed variant lives.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Source {
        /// Saved by a player on this machine.
        Saved,
        /// Fetched from Xbox Live.
        Live,
    }

    /// One file of a map's directory.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Listed {
        pub source: Source,
        /// The file's stem, the handle `load`, `rename`, `duplicate` and
        /// `delete` take.
        pub file: String,
        /// The document's title; the stem when the file does not parse.
        pub name: String,
        pub author: String,
        /// The document's id; absent when the file does not parse.
        pub id: Option<Sha256>,
        /// Why it cannot be used, if it cannot.
        pub problem: Option<Problem>,
        /// `problem`'s sentence.
        pub sentence: Option<String>,
    }

    fn map_dir(data: &Path, map: &str) -> Result<PathBuf, Error> {
        if !content::manifest::is_map_name(map) {
            return Err(Error::Name("That is not a map's name.".into()));
        }
        Ok(data.join(VARIANTS_DIR).join(map))
    }

    fn read_doc(path: &Path, limits: &Limits) -> Result<ForgeDoc, Problem> {
        let meta = path.symlink_metadata().map_err(|_| Problem::Unreadable)?;
        if !meta.is_file() {
            return Err(Problem::Unreadable);
        }
        let mut bytes = vec![];
        fs::File::open(path)
            .and_then(|f| f.take(MAX_DOC_BYTES as u64 + 1).read_to_end(&mut bytes))
            .map_err(|_| Problem::Unreadable)?;
        ForgeDoc::from_bytes(&bytes, limits).map_err(|e| match e {
            forge_doc::Refusal::Format(_) => Problem::Format,
            _ => Problem::Unreadable,
        })
    }

    /// The check every use of a variant passes, in the table's order. `installed`
    /// is the SHA-256 of this machine's map of the base's name, or `None` when
    /// it has none.
    fn problem_of(doc: &ForgeDoc, map: &str, installed: Option<&Sha256>) -> Option<Problem> {
        if doc.base.map != map {
            return Some(Problem::Unreadable);
        }
        if doc.tier() != Tier::Variant {
            return Some(Problem::Tier);
        }
        if doc.builder != BuilderVersions::CURRENT {
            return Some(Problem::Builder);
        }
        match installed {
            None => Some(Problem::BaseMissing),
            Some(hash) if *hash != doc.base.sha256 => Some(Problem::BaseHash),
            Some(_) => None,
        }
    }

    fn stems(dir: &Path) -> Result<Vec<String>, Error> {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(io(e)),
        };
        let mut out = vec![];
        for entry in entries {
            let entry = entry.map_err(io)?;
            if !entry.file_type().map_err(io)?.is_file() {
                continue;
            }
            if let Some(stem) = entry
                .file_name()
                .to_str()
                .and_then(|f| f.strip_suffix(&format!(".{VARIANT_EXT}")))
                && content::manifest::is_map_name(stem)
            {
                out.push(stem.to_owned());
            }
        }
        out.sort_unstable();
        Ok(out)
    }

    fn path_of(dir: &Path, file: &str) -> PathBuf {
        dir.join(format!("{file}.{VARIANT_EXT}"))
    }

    /// Every map variant of `map`, saved then fetched, by name without case.
    /// `installed` is the hash of this machine's `map`, or `None`.
    pub fn list(data: &Path, map: &str, installed: Option<&Sha256>) -> Result<Vec<Listed>, Error> {
        let dir = map_dir(data, map)?;
        let limits = Limits::default();
        let mut out = vec![];
        for (source, dir) in [
            (Source::Saved, dir.clone()),
            (Source::Live, dir.join(LIVE_DIR)),
        ] {
            let mut part = vec![];
            for file in stems(&dir)? {
                let read = read_doc(&path_of(&dir, &file), &limits);
                let (name, author, id, problem) = match &read {
                    Ok(doc) => (
                        doc.title.clone(),
                        doc.author.clone(),
                        doc.id().ok(),
                        problem_of(doc, map, installed),
                    ),
                    Err(p) => (file.clone(), String::new(), None, Some(*p)),
                };
                let sentence = problem.map(|p| p.sentence(&name, map));
                part.push(Listed {
                    source,
                    file,
                    name,
                    author,
                    id,
                    problem,
                    sentence,
                });
            }
            part.sort_by_cached_key(|l| (l.name.to_lowercase(), l.file.clone()));
            out.extend(part);
        }
        Ok(out)
    }

    /// The file stem a name is saved under: its letters and digits, dashes and
    /// underscores, anything else an underscore.
    fn stem_for(name: &str) -> String {
        let stem: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        if stem.is_empty() {
            "variant".into()
        } else {
            stem
        }
    }

    fn named(dir: &Path, name: &str) -> Result<Option<String>, Error> {
        let want = name.to_lowercase();
        let limits = Limits::default();
        for file in stems(dir)? {
            if let Ok(doc) = read_doc(&path_of(dir, &file), &limits)
                && doc.title.to_lowercase() == want
            {
                return Ok(Some(file));
            }
        }
        Ok(None)
    }

    fn check_name(doc: &ForgeDoc) -> Result<(), Error> {
        doc.check_variant().map_err(|e| match e {
            forge_doc::Refusal::Tier => Error::Problem(
                Problem::Tier,
                Problem::Tier.sentence(&doc.title, &doc.base.map),
            ),
            e => Error::Name(e.to_string()),
        })
    }

    /// Save `doc`'s play form as a variant of its base map and return its file
    /// stem. A name another variant of the map has, compared without case, is
    /// `NameTaken` unless `replace` is set, which overwrites that file.
    pub fn save(data: &Path, doc: &ForgeDoc, replace: bool) -> Result<String, Error> {
        let doc = forge_doc::play_form(doc);
        check_name(&doc)?;
        let bytes = doc
            .canonical_json()
            .map_err(|e| Error::Name(e.to_string()))?;
        let dir = ensure_dir(&std::path::absolute(map_dir(data, &doc.base.map)?).map_err(io)?)
            .map_err(Error::Io)?;
        match named(&dir, &doc.title)? {
            Some(file) if replace => {
                content::store::write_atomic(&path_of(&dir, &file), bytes.as_bytes())
                    .map_err(io)?;
                Ok(file)
            }
            Some(_) => Err(Error::NameTaken(doc.title)),
            None => create_unique(&dir, &stem_for(&doc.title), VARIANT_EXT, bytes.as_bytes())
                .map_err(Error::Io),
        }
    }

    /// Read a saved variant of `map` and run the table's checks.
    pub fn load(
        data: &Path,
        map: &str,
        file: &str,
        installed: Option<&Sha256>,
    ) -> Result<ForgeDoc, Error> {
        let doc = open(data, map, file)?;
        match problem_of(&doc, map, installed) {
            Some(p) => Err(Error::Problem(p, p.sentence(&doc.title, map))),
            None => Ok(doc),
        }
    }

    fn open(data: &Path, map: &str, file: &str) -> Result<ForgeDoc, Error> {
        let dir = map_dir(data, map)?;
        if !content::manifest::is_map_name(file) {
            return Err(Error::Missing(format!("{file} is not a map variant.")));
        }
        let path = path_of(&dir, file);
        if path.symlink_metadata().is_err() {
            return Err(Error::Missing(format!("{file} is not a map variant.")));
        }
        read_doc(&path, &Limits::default()).map_err(|p| Error::Problem(p, p.sentence(file, map)))
    }

    /// Give a saved variant a new name. The name is part of the document, so the
    /// variant has a new id. A name another variant has, compared without case,
    /// is refused; nothing is replaced by a rename.
    pub fn rename(data: &Path, map: &str, file: &str, name: &str) -> Result<(), Error> {
        let mut doc = open(data, map, file)?;
        doc.title = name.to_owned();
        doc.expect = None;
        check_name(&doc)?;
        let dir = map_dir(data, map)?;
        if named(&dir, name)?.is_some_and(|other| other != file) {
            return Err(Error::NameTaken(name.to_owned()));
        }
        let bytes = doc
            .canonical_json()
            .map_err(|e| Error::Name(e.to_string()))?;
        content::store::write_atomic(&path_of(&dir, file), bytes.as_bytes()).map_err(io)
    }

    /// Copy a saved variant under a new name and return the copy's file stem.
    /// The copy has the same edits and a new id.
    pub fn duplicate(data: &Path, map: &str, file: &str, name: &str) -> Result<String, Error> {
        let mut doc = open(data, map, file)?;
        doc.title = name.to_owned();
        doc.expect = None;
        save(data, &doc, false)
    }

    /// Remove a saved variant, readable or not.
    pub fn delete(data: &Path, map: &str, file: &str) -> Result<(), Error> {
        let path = path_of(&map_dir(data, map)?, file);
        if !content::manifest::is_map_name(file) || path.symlink_metadata().is_err() {
            return Err(Error::Missing(format!("{file} is not a map variant.")));
        }
        fs::remove_file(path).map_err(io)
    }
}
