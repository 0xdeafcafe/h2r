//! Reading a package from its folder, and `package.check`: what is wrong
//! with a package or a set of them before any map is built.
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use content::Sha256;
use content::package::{
    Kind, LOCK_FILE, Lock, MANIFEST_FILE, Manifest, Maps, Package, PackageError,
};

use crate::build::Loaded;
use crate::builders::{self, Job};

/// A source larger than this is refused: the content store's cap.
pub const MAX_FILE_BYTES: u64 = 64 << 20;

/// Why a package folder cannot be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadError {
    pub dir: PathBuf,
    pub why: String,
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.dir.display(), self.why)
    }
}
impl std::error::Error for LoadError {}

impl Loaded {
    /// The package in `dir`: `package.toml` and every file it names. The
    /// lock is made from the files as they are; compare it with the one on
    /// disc ([`read_lock`], [`lock_findings`]).
    pub fn from_dir(dir: &Path) -> Result<Self, LoadError> {
        let fail = |why: String| LoadError {
            dir: dir.to_owned(),
            why,
        };
        let text = std::fs::read_to_string(dir.join(MANIFEST_FILE))
            .map_err(|e| fail(format!("{MANIFEST_FILE}: {e}")))?;
        let manifest = Manifest::from_toml(&text).map_err(|e| fail(e.to_string()))?;
        let mut files = BTreeMap::new();
        for name in manifest.files() {
            let bytes = std::fs::read(dir.join(name)).map_err(|e| fail(format!("{name}: {e}")))?;
            files.insert(name.to_owned(), bytes);
        }
        let lock = Lock::new(
            files
                .iter()
                .map(|(n, b)| (n.clone(), Sha256::of(b)))
                .collect(),
        );
        let package =
            Package::new(manifest, lock).map_err(|e: PackageError| fail(e.to_string()))?;
        Ok(Self { package, files })
    }
}

impl Loaded {
    /// The package whose files are `files`, by path in the package:
    /// `package.toml` and every file it names (a baked package). `name`
    /// says where it came from in an error.
    pub fn from_files(name: &str, files: &BTreeMap<String, Vec<u8>>) -> Result<Self, LoadError> {
        let fail = |why: String| LoadError {
            dir: PathBuf::from(name),
            why,
        };
        let text = files
            .get(MANIFEST_FILE)
            .ok_or_else(|| fail(format!("no {MANIFEST_FILE}")))?;
        let text = std::str::from_utf8(text).map_err(|e| fail(format!("{MANIFEST_FILE}: {e}")))?;
        let manifest = Manifest::from_toml(text).map_err(|e| fail(e.to_string()))?;
        let mut named = BTreeMap::new();
        for file in manifest.files() {
            let bytes = files
                .get(file)
                .ok_or_else(|| fail(format!("{file}: not in the package")))?;
            named.insert(file.to_owned(), bytes.clone());
        }
        let lock = Lock::new(
            named
                .iter()
                .map(|(n, b)| (n.clone(), Sha256::of(b)))
                .collect(),
        );
        let package =
            Package::new(manifest, lock).map_err(|e: PackageError| fail(e.to_string()))?;
        Ok(Self {
            package,
            files: named,
        })
    }
}

/// The `package.lock` beside the manifest, if there is one.
pub fn read_lock(dir: &Path) -> Result<Option<Lock>, LoadError> {
    let path = dir.join(LOCK_FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => Lock::from_toml(&text).map(Some).map_err(|e| LoadError {
            dir: dir.to_owned(),
            why: e.to_string(),
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(LoadError {
            dir: dir.to_owned(),
            why: format!("{LOCK_FILE}: {e}"),
        }),
    }
}

/// How a lock on disc differs from the files: one line per file.
pub fn lock_findings(on_disc: &Lock, actual: &Lock) -> Vec<String> {
    let mut out = Vec::new();
    for (name, hash) in &actual.files {
        match on_disc.files.get(name) {
            None => out.push(format!("{name}: not in {LOCK_FILE}")),
            Some(h) if h != hash => {
                out.push(format!("{name}: {LOCK_FILE} has {h}, the file is {hash}"))
            }
            Some(_) => {}
        }
    }
    for name in on_disc.files.keys() {
        if !actual.files.contains_key(name) {
            out.push(format!("{name}: in {LOCK_FILE}, no longer in the manifest"));
        }
    }
    out
}

/// One thing `check` found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub package: String,
    /// The item's place in the manifest, when it is about one.
    pub item: Option<usize>,
    pub file: Option<String>,
    pub line: Option<usize>,
    pub what: String,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.package)?;
        if let Some(i) = self.item {
            write!(f, " item {i}")?;
        }
        if let Some(file) = &self.file {
            write!(f, " {file}")?;
            if let Some(l) = self.line {
                write!(f, " line {l}")?;
            }
        }
        write!(f, ": {}", self.what)
    }
}

/// Everything wrong with one package that needs no map: an item with no
/// builder, a missing or oversize source, a source its builder refuses.
pub fn check_package(loaded: &Loaded) -> Vec<Finding> {
    let manifest = &loaded.package.manifest;
    let package = manifest.package.id.clone();
    let mut out = Vec::new();
    for (index, item) in manifest.items.iter().enumerate() {
        let found = |line: Option<usize>, what: String| Finding {
            package: package.clone(),
            item: Some(index),
            file: item.source.clone(),
            line,
            what,
        };
        let Some(builder) = builders::builder(item.kind) else {
            out.push(found(
                None,
                format!("{:?} has no builder in this build", item.kind),
            ));
            continue;
        };
        let source: &[u8] = match &item.source {
            Some(path) => match loaded.files.get(path) {
                Some(b) if b.len() as u64 > MAX_FILE_BYTES => {
                    out.push(found(
                        None,
                        format!("{} bytes: over the {MAX_FILE_BYTES} a file may be", b.len()),
                    ));
                    continue;
                }
                Some(b) => b,
                None => {
                    out.push(found(None, format!("the package has no file {path:?}")));
                    continue;
                }
            },
            None => &[],
        };
        let job = Job {
            item,
            source,
            files: &loaded.files,
        };
        if let Err(f) = (builder.check)(&job) {
            out.push(found(f.line, format!("{}: {}", builder.name, f.why)));
        }
    }
    out
}

/// Files and folders in `dir` a package may not hold, by name (a package
/// is data we can ship: no cache map, tag bundle, disc file, voice dataset
/// or model weights), whether the manifest names them or not.
pub fn forbidden_files(dir: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        let Ok(read) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = read.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let path = e.path();
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let name = e.file_name().to_string_lossy().to_ascii_lowercase();
            if path.is_dir() {
                if matches!(
                    name.as_str(),
                    "dataset" | "datasets" | "voice-dataset" | "weights"
                ) {
                    out.push(format!("{rel}: a voice dataset or model weights folder"));
                } else {
                    walk(root, &path, out);
                }
                continue;
            }
            let what = match path.extension().and_then(|x| x.to_str()) {
                Some(x) => match x.to_ascii_lowercase().as_str() {
                    "map" => Some("a cache map"),
                    "h2tag" => Some("a .h2tag bundle"),
                    "xbe" | "iso" | "xiso" => Some("disc files"),
                    "safetensors" | "ckpt" | "pt" | "pth" => Some("model weights"),
                    _ => None,
                },
                None => None,
            };
            if let Some(what) = what {
                out.push(format!("{rel}: {what}"));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out
}

fn overlap(a: &Maps, b: &Maps) -> bool {
    use content::package::MapScope::All;
    match (a, b) {
        (Maps::Scope(All), _) | (_, Maps::Scope(All)) => true,
        (Maps::Scope(x), Maps::Scope(y)) => x == y,
        (Maps::Names(x), Maps::Names(y)) => x
            .iter()
            .any(|x| y.iter().any(|y| x.eq_ignore_ascii_case(y))),
        // A map's name does not say its class: assume it can be.
        _ => true,
    }
}

/// The tag a `tag`, `bitmap` or `model` item adds (a `model`'s is its
/// `hlmt`), from the manifest alone.
pub fn added_tag(item: &content::package::Item) -> Option<tags::repo::TagKey> {
    let path = item.tag.as_deref()?.replace('/', "\\");
    let group = match item.kind {
        Kind::Bitmap => map::FourCC(*b"bitm"),
        Kind::Model => map::FourCC(*b"hlmt"),
        Kind::Tag => crate::tagitem::group_of_source(item.source.as_deref()?).ok()?,
        _ => return None,
    };
    Some(tags::repo::TagKey::new(group, path))
}

fn adds(item: &content::package::Item) -> Option<(String, String)> {
    let key = added_tag(item)?;
    Some((tags::defs::fourcc_text(key.group), key.path))
}

/// Two items, in the same package or different ones, that add the same tag
/// path for maps they could share: the build stops on them, so `check`
/// names both before it.
pub fn check_set(packages: &[Loaded]) -> Vec<Finding> {
    let mut seen: BTreeMap<(String, String), Vec<(&str, usize, &Maps)>> = BTreeMap::new();
    let mut out = Vec::new();
    for p in packages {
        let id = p.package.manifest.package.id.as_str();
        for (i, item) in p.package.manifest.items.iter().enumerate() {
            let Some(key) = adds(item) else { continue };
            let list = seen.entry(key.clone()).or_default();
            if let Some((first, fi, _)) = list.iter().find(|(_, _, m)| overlap(m, &item.maps)) {
                out.push(Finding {
                    package: id.to_owned(),
                    item: Some(i),
                    file: item.source.clone(),
                    line: None,
                    what: format!(
                        "adds tag {}.{}, which package {first:?} item {fi} adds too",
                        key.1, key.0
                    ),
                });
            }
            list.push((id, i, &item.maps));
        }
    }
    out
}
