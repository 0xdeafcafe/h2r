use std::{
    fs,
    io::{ErrorKind, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use forge_doc::{ForgeDoc, Limits, MAX_DOC_BYTES, MAX_PREFAB_BYTES, Prefab};

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

/// The map variant store (docs/game/forge-map-variants.md, "The map variant
/// file"): `<data>/map-variants/<base map>/<stem>.h2mapvariant`, with the ones
/// fetched from Live under `live/`, and the `map-variant` items of the
/// packages that are on ("A map variant in a package"), which the boot
/// gives the store ([`variants::install_packaged`]). A packaged variant is
/// listed and loaded as a saved one is; its handle is `<package>.<stem>`,
/// which no file stem is.
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
        /// The variant is a package's: the store does not rename or
        /// remove it.
        Packaged(String),
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
                Self::Packaged(_) => "variant_packaged",
                Self::Io(_) => "variant_io",
            }
        }
    }

    impl std::fmt::Display for Error {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::Problem(_, s)
                | Self::Name(s)
                | Self::Missing(s)
                | Self::Packaged(s)
                | Self::Io(s) => {
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
        /// A `map-variant` item of a package that is on.
        Package,
    }

    impl Source {
        /// Whether `load` reads a variant of this source by its handle:
        /// one saved here or a package's. A fetched one is listed only.
        pub const fn loads(self) -> bool {
            matches!(self, Self::Saved | Self::Package)
        }
    }

    /// A `map-variant` item as the store holds it: read once, when the
    /// set is given.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Packaged {
        /// The package's id.
        pub package: String,
        /// The handle `load` and `duplicate` take: `<package>.<stem>`.
        pub file: String,
        pub doc: ForgeDoc,
    }

    impl Packaged {
        /// The item of `package` whose source has the stem `stem` and the
        /// bytes `bytes`; `None` when the bytes are not a Forge document
        /// (`h2 package check` names why).
        pub fn read(package: &str, stem: &str, bytes: &[u8]) -> Option<Self> {
            let doc = ForgeDoc::from_bytes(bytes, &Limits::default()).ok()?;
            Some(Self {
                package: package.to_owned(),
                file: handle(package, stem),
                doc,
            })
        }
    }

    /// A packaged variant's handle.
    pub fn handle(package: &str, stem: &str) -> String {
        format!("{package}.{stem}")
    }

    /// Whether `file` is a packaged variant's handle, not a file stem.
    fn is_handle(file: &str) -> bool {
        file.contains('.')
    }

    static PACKAGED: std::sync::RwLock<Vec<Packaged>> = std::sync::RwLock::new(Vec::new());

    /// Give the store the set's `map-variant` items, replacing the ones
    /// it had. The boot calls it once the set is read
    /// (`boot::packages::at_boot`).
    pub fn install_packaged(packaged: Vec<Packaged>) {
        *PACKAGED.write().unwrap_or_else(|e| e.into_inner()) = packaged;
    }

    /// The installed set's `map-variant` items.
    pub fn packaged() -> Vec<Packaged> {
        PACKAGED.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn packaged_refusal(file: &str) -> Error {
        let package = file.split('.').next().unwrap_or(file);
        Error::Packaged(format!(
            "That map variant belongs to the package {package}. Duplicate it to change it."
        ))
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
        /// The document has a well-formed AI ground recipe, so the lobby
        /// takes it as a Firefight map (docs/game/forge-firefight.md,
        /// "Playing it").
        pub ground: bool,
        /// The document holds a wave set of its own, which Firefight's
        /// Map Waves entry plays.
        pub waves: bool,
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

    /// Every map variant of `map`: saved, then fetched, then the installed
    /// set's packaged ones, each part by name without case. `installed` is
    /// the hash of this machine's `map`, or `None`.
    pub fn list(data: &Path, map: &str, installed: Option<&Sha256>) -> Result<Vec<Listed>, Error> {
        list_with(data, map, installed, &packaged())
    }

    /// [`list`] with the packaged variants given, not the installed ones.
    pub fn list_with(
        data: &Path,
        map: &str,
        installed: Option<&Sha256>,
        packaged: &[Packaged],
    ) -> Result<Vec<Listed>, Error> {
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
                let ground = read
                    .as_ref()
                    .is_ok_and(|doc| matches!(doc.ground_recipe(), Ok(Some(_))));
                let waves = read.as_ref().is_ok_and(|doc| doc.waves.is_some());
                let sentence = problem.map(|p| p.sentence(&name, map));
                part.push(Listed {
                    source,
                    file,
                    name,
                    author,
                    id,
                    problem,
                    sentence,
                    ground,
                    waves,
                });
            }
            part.sort_by_cached_key(|l| (l.name.to_lowercase(), l.file.clone()));
            out.extend(part);
        }
        let mut part: Vec<Listed> = packaged
            .iter()
            .filter(|p| p.doc.base.map == map)
            .map(|p| {
                let problem = problem_of(&p.doc, map, installed);
                Listed {
                    source: Source::Package,
                    file: p.file.clone(),
                    name: p.doc.title.clone(),
                    author: p.doc.author.clone(),
                    id: p.doc.id().ok(),
                    problem,
                    sentence: problem.map(|q| q.sentence(&p.doc.title, map)),
                    ground: matches!(p.doc.ground_recipe(), Ok(Some(_))),
                    waves: p.doc.waves.is_some(),
                }
            })
            .collect();
        part.sort_by_cached_key(|l| (l.name.to_lowercase(), l.file.clone()));
        out.extend(part);
        Ok(out)
    }

    /// One map variant on this machine, as a game variant's suggestion names
    /// it (docs/game/forge-map-variants.md, "How a pick names a map variant").
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Suggestable {
        pub source: Source,
        /// The base map's name.
        pub map: String,
        /// The file's stem in the map's directory.
        pub file: String,
        /// The document's title.
        pub name: String,
        /// The base map's SHA-256.
        pub base: Sha256,
        /// The document's id.
        pub id: Sha256,
    }

    /// Every map variant on this machine a game variant may suggest: of any
    /// map, saved then fetched by map and name without case, then the
    /// installed set's packaged ones. A file that
    /// does not parse, is of the map tier, has another builder's form or
    /// sits in another map's directory is left out: a suggestion names a
    /// variant the lobby could pick. Whether this machine has the base map
    /// is not asked; the lobby checks that when it picks.
    pub fn suggestable(data: &Path) -> Result<Vec<Suggestable>, Error> {
        suggestable_with(data, &packaged())
    }

    /// [`suggestable`] with the packaged variants given, not the installed
    /// ones.
    pub fn suggestable_with(data: &Path, packaged: &[Packaged]) -> Result<Vec<Suggestable>, Error> {
        let mut out = saved_suggestable(data)?;
        let mut part: Vec<Suggestable> = packaged
            .iter()
            .filter(|p| p.doc.tier() == Tier::Variant && p.doc.builder == BuilderVersions::CURRENT)
            .filter_map(|p| {
                Some(Suggestable {
                    source: Source::Package,
                    map: p.doc.base.map.clone(),
                    file: p.file.clone(),
                    name: p.doc.title.clone(),
                    base: p.doc.base.sha256,
                    id: p.doc.id().ok()?,
                })
            })
            .collect();
        part.sort_by_cached_key(|s| (s.map.clone(), s.name.to_lowercase(), s.file.clone()));
        out.extend(part);
        Ok(out)
    }

    fn saved_suggestable(data: &Path) -> Result<Vec<Suggestable>, Error> {
        let root = data.join(VARIANTS_DIR);
        let entries = match fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(io(e)),
        };
        let mut maps = vec![];
        for entry in entries {
            let entry = entry.map_err(io)?;
            if entry.file_type().map_err(io)?.is_dir()
                && let Some(map) = entry.file_name().to_str()
                && content::manifest::is_map_name(map)
            {
                maps.push(map.to_owned());
            }
        }
        maps.sort_unstable();
        let limits = Limits::default();
        let mut out = vec![];
        for map in maps {
            let dir = root.join(&map);
            for (source, dir) in [(Source::Saved, dir.clone()), (Source::Live, dir.join(LIVE_DIR))] {
                let mut part = vec![];
                for file in stems(&dir)? {
                    let Ok(doc) = read_doc(&path_of(&dir, &file), &limits) else {
                        continue;
                    };
                    // The checks of `problem_of` that do not need the installed map.
                    let usable = doc.base.map == map
                        && doc.tier() == Tier::Variant
                        && doc.builder == BuilderVersions::CURRENT;
                    let Some(id) = doc.id().ok().filter(|_| usable) else {
                        continue;
                    };
                    part.push(Suggestable {
                        source,
                        map: map.clone(),
                        file,
                        name: doc.title.clone(),
                        base: doc.base.sha256,
                        id,
                    });
                }
                part.sort_by_cached_key(|l| (l.name.to_lowercase(), l.file.clone()));
                out.extend(part);
            }
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

    /// Read a saved or packaged variant of `map` and run the table's
    /// checks.
    pub fn load(
        data: &Path,
        map: &str,
        file: &str,
        installed: Option<&Sha256>,
    ) -> Result<ForgeDoc, Error> {
        load_with(data, map, file, installed, &packaged())
    }

    /// [`load`] with the packaged variants given, not the installed ones.
    pub fn load_with(
        data: &Path,
        map: &str,
        file: &str,
        installed: Option<&Sha256>,
        packaged: &[Packaged],
    ) -> Result<ForgeDoc, Error> {
        let doc = open(data, map, file, packaged)?;
        match problem_of(&doc, map, installed) {
            Some(p) => Err(Error::Problem(p, p.sentence(&doc.title, map))),
            None => Ok(doc),
        }
    }

    fn open(data: &Path, map: &str, file: &str, packaged: &[Packaged]) -> Result<ForgeDoc, Error> {
        let dir = map_dir(data, map)?;
        if is_handle(file) {
            return packaged
                .iter()
                .find(|p| p.file == file && p.doc.base.map == map)
                .map(|p| p.doc.clone())
                .ok_or_else(|| Error::Missing(format!("{file} is not a map variant.")));
        }
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
        if is_handle(file) {
            return Err(packaged_refusal(file));
        }
        let mut doc = open(data, map, file, &[])?;
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

    /// Copy a saved or packaged variant under a new name and return the
    /// copy's file stem. The copy is a saved one, with the same edits and a
    /// new id.
    pub fn duplicate(data: &Path, map: &str, file: &str, name: &str) -> Result<String, Error> {
        let mut doc = open(data, map, file, &packaged())?;
        doc.title = name.to_owned();
        doc.expect = None;
        save(data, &doc, false)
    }

    /// Remove a saved variant, readable or not.
    pub fn delete(data: &Path, map: &str, file: &str) -> Result<(), Error> {
        if is_handle(file) {
            return Err(packaged_refusal(file));
        }
        let path = path_of(&map_dir(data, map)?, file);
        if !content::manifest::is_map_name(file) || path.symlink_metadata().is_err() {
            return Err(Error::Missing(format!("{file} is not a map variant.")));
        }
        fs::remove_file(path).map_err(io)
    }
}

/// The suffix of a prefab file.
const PREFAB_SUFFIX: &str = ".h2prefab";

/// The most prefab files one listing reads.
const MAX_PREFABS: usize = 256;

/// Write `prefab` as `<data>/forge/prefabs/<name>.h2prefab`, canonical
/// JSON, replacing a prefab of that name.
pub(crate) fn save_prefab(data: &Path, prefab: &Prefab) -> Result<(), String> {
    let bytes = prefab.canonical_json().map_err(|e| e.to_string())?;
    let path =
        std::path::absolute(data.join("forge").join("prefabs")).map_err(|e| e.to_string())?;
    let dir = ensure_dir(&path)?;
    content::store::write_atomic(
        &dir.join(format!("{}{PREFAB_SUFFIX}", prefab.name)),
        bytes.as_bytes(),
    )
    .map_err(|e| e.to_string())
}

/// The prefabs under `<data>/forge/prefabs`, by name, and a line for each
/// file that is not one: a file that is not a regular file, is over the
/// bound, does not read, or holds a name other than its own is named and
/// left out.
pub fn load_prefabs(data: &Path) -> Result<(Vec<Prefab>, Vec<String>), String> {
    let dir = data.join("forge").join("prefabs");
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok((vec![], vec![])),
        Err(e) => return Err(e.to_string()),
    };
    outside_checkout(&dir.canonicalize().map_err(|e| e.to_string())?)?;
    let mut files = vec![];
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let filename = entry.file_name();
        if let Some(stem) = filename
            .to_str()
            .and_then(|s| s.strip_suffix(PREFAB_SUFFIX))
        {
            files.push(stem.to_owned());
        }
    }
    files.sort_unstable();
    let mut prefabs = vec![];
    let mut skipped = vec![];
    for stem in files.into_iter().take(MAX_PREFABS) {
        match read_prefab(&dir.join(format!("{stem}{PREFAB_SUFFIX}")), &stem) {
            Ok(prefab) => prefabs.push(prefab),
            Err(reason) => skipped.push(format!("{stem}: {reason}")),
        }
    }
    Ok((prefabs, skipped))
}

fn read_prefab(path: &Path, stem: &str) -> Result<Prefab, String> {
    if !path
        .symlink_metadata()
        .map_err(|e| e.to_string())?
        .is_file()
    {
        return Err("not a regular file".into());
    }
    let mut bytes = vec![];
    fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(MAX_PREFAB_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let prefab = Prefab::from_bytes(&bytes).map_err(|e| e.to_string())?;
    if prefab.name != stem {
        return Err(format!("holds the name {}", prefab.name));
    }
    Ok(prefab)
}
