//! The package document (docs/editor/studio-redesign.md, "Packages" and
//! "Studio's side"): what `package.*` reads and writes. A package is a
//! folder under `<data>/packages/<id>/` with a `package.toml`, its files and
//! a `package.lock`; the user's order and what is off are
//! `<data>/packages/set.toml`; the edit target is `<data>/packages/target.txt`.
//!
//! Every write goes through one path: the new manifest is written, read back
//! with its files (`Loaded::from_dir`), put in the set (`content::package::Set`,
//! which keeps the always-on rule and refuses a cycle), and only then locked.
//! A manifest the set refuses is put back as it was.
//!
//! The built-in packages (`boot::packages::baked`) are read and listed; they
//! are never written.

use std::collections::BTreeSet;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::Instant;

use ::workspace::{
    Loaded, PACKAGES_DIR, Target, added_tag, build_map, builder, changes_map, check_package,
    check_set, forbidden_files, lock_findings, read_lock, scope_of, store_findings,
};
use content::package::{
    ALWAYS_ON, Credit, Header, Item, Kind, LOCK_FILE, MANIFEST_FILE, Manifest, MapScope, PACKAGE_FORMAT,
    SET_FILE, Set, SetFile, valid_slug,
};
use serde_json::{Value, json};

use super::assets::read_set;

/// The edit target's file under `<data>/packages/`: the package id and a
/// newline.
pub const TARGET_FILE: &str = "target.txt";

/// Why a call is refused.
#[derive(Debug)]
pub enum Refusal {
    /// A parameter is wrong.
    Params(String),
    /// The package, the set or the build cannot take it.
    Refused(String),
    /// A file did not read or write.
    Failed(String),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::Params(m) | Refusal::Refused(m) | Refusal::Failed(m) => f.write_str(m),
        }
    }
}

type Res<T = Value> = Result<T, Refusal>;

use Refusal::{Failed, Params, Refused};

fn root(data: &Path) -> PathBuf {
    data.join(PACKAGES_DIR)
}

/// Every package that reads: the built-in ones, then the folders under
/// `<data>/packages/`, with the folder of a local one.
struct Known {
    data: PathBuf,
    all: Vec<(Loaded, Option<PathBuf>)>,
}

fn known(data: &Path) -> Known {
    let (baked, _) = boot::packages::baked();
    let mut all: Vec<(Loaded, Option<PathBuf>)> = baked.into_iter().map(|l| (l, None)).collect();
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root(data))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join(MANIFEST_FILE).is_file())
        .collect();
    dirs.sort();
    for dir in dirs {
        if let Ok(l) = Loaded::from_dir(&dir)
            && !all
                .iter()
                .any(|(b, _)| b.package.name() == l.package.name())
        {
            all.push((l, Some(dir)));
        }
    }
    Known {
        data: data.to_owned(),
        all,
    }
}

impl Known {
    fn packages(&self) -> Vec<content::package::Package> {
        self.all.iter().map(|(l, _)| l.package.clone()).collect()
    }

    fn get(&self, id: &str) -> Option<&(Loaded, Option<PathBuf>)> {
        self.all.iter().find(|(l, _)| l.package.name() == id)
    }

    /// The package, or why it is not here (a folder that does not read says
    /// what is wrong with it).
    fn need(&self, id: &str) -> Res<&(Loaded, Option<PathBuf>)> {
        if let Some(found) = self.get(id) {
            return Ok(found);
        }
        let dir = root(&self.data).join(id);
        if dir.join(MANIFEST_FILE).is_file()
            && let Err(e) = Loaded::from_dir(&dir)
        {
            return Err(Failed(e.to_string()));
        }
        Err(Params(format!(
            "no package {id:?} (package.list names them)"
        )))
    }

    /// A package under `<data>/packages/`, with its folder.
    fn local(&self, id: &str) -> Res<(&Loaded, PathBuf)> {
        let (loaded, dir) = self.need(id)?;
        match dir {
            Some(d) => Ok((loaded, d.clone())),
            None => Err(Refused(format!(
                "package {id:?} is built in: only a package under <data>/packages/ is edited"
            ))),
        }
    }
}

/// `set.toml`, or the default when there is none. A file that does not read
/// (or turns an always-on package off) is refused by what it says.
fn set_file(data: &Path) -> Res<SetFile> {
    match std::fs::read_to_string(root(data).join(SET_FILE)) {
        Ok(text) => SetFile::from_toml(&text).map_err(|e| Refused(e.to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(SetFile::default()),
        Err(e) => Err(Failed(format!("{SET_FILE}: {e}"))),
    }
}

fn write_set_file(data: &Path, file: &SetFile) -> Res<()> {
    file.check().map_err(|e| Refused(e.to_string()))?;
    let dir = root(data);
    std::fs::create_dir_all(&dir).map_err(|e| Failed(format!("{}: {e}", dir.display())))?;
    let path = dir.join(SET_FILE);
    std::fs::write(&path, file.to_toml()).map_err(|e| Failed(format!("{}: {e}", path.display())))
}

/// The set the packages and `file` make, or why they cannot be ordered.
fn ordered(known: &Known, file: &SetFile) -> Res<Set> {
    Set::new(known.packages(), file).map_err(|e| Refused(e.to_string()))
}

// ---- the edit target ----

/// The edit target's id as the file holds it, whether or not it is still a
/// package.
fn target_id(data: &Path) -> Option<String> {
    let text = std::fs::read_to_string(root(data).join(TARGET_FILE)).ok()?;
    let id = text.trim();
    (!id.is_empty()).then(|| id.to_owned())
}

/// The edit target when it is a package under `<data>/packages/`.
pub fn target(data: &Path) -> Option<String> {
    let id = target_id(data)?;
    known(data)
        .get(&id)
        .is_some_and(|(_, d)| d.is_some())
        .then_some(id)
}

/// `package.target`: read the edit target, set it to the local package `id`,
/// or clear it.
pub fn edit_target(data: &Path, id: Option<&str>, clear: bool) -> Res {
    let file = root(data).join(TARGET_FILE);
    if clear && id.is_some() {
        return Err(Params("pass an id or clear, not both".into()));
    }
    if clear {
        match std::fs::remove_file(&file) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(Failed(format!("{}: {e}", file.display()))),
        }
    } else if let Some(id) = id {
        let k = known(data);
        k.local(id)?;
        std::fs::create_dir_all(root(data)).map_err(|e| Failed(e.to_string()))?;
        std::fs::write(&file, format!("{id}\n"))
            .map_err(|e| Failed(format!("{}: {e}", file.display())))?;
    }
    let k = known(data);
    let held = target_id(data);
    Ok(match held {
        Some(id) => match k.get(&id) {
            Some((l, Some(d))) => json!({
                "target": id,
                "name": l.package.manifest.package.name,
                "dir": d.display().to_string(),
            }),
            _ => json!({
                "target": null,
                "stale": id,
                "why": format!("{TARGET_FILE} names {id:?}, which is not a package under <data>/packages/"),
            }),
        },
        None => json!({ "target": null }),
    })
}

// ---- reading ----

/// `package.list`: the set in order, then the packages that are off, with
/// why, and the edit target.
pub fn list(data: &Path) -> Value {
    let set = read_set(data);
    let target = target(data);
    let rows: Vec<Value> = set
        .listed
        .iter()
        .map(|l| {
            let mut v = serde_json::to_value(l).unwrap_or(Value::Null);
            v["target"] = json!(target.as_deref() == Some(l.id.as_str()));
            v["editable"] = json!(l.dir.is_some());
            v
        })
        .collect();
    json!({
        "packages": rows,
        "problems": set.problems,
        "content_hash": set.content_hash.map(|h| h.to_string()),
        "target": target,
        "dir": root(data).display().to_string(),
    })
}

/// What an item adds to a map, by builder (docs/editor/studio-redesign.md,
/// "Contents").
fn builds(item: &Item) -> Vec<String> {
    use content::package::Kind::*;
    let words = |w: &[&str]| w.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    match item.kind {
        Tag => match added_tag(item) {
            Some(k) => vec![format!("{}:{}", tags::defs::fourcc_text(k.group), k.path)],
            None => Vec::new(),
        },
        Edits => words(&["changed fields"]),
        Model => words(&["bitm", "shad", "mode", "phmo", "hlmt"]),
        Bitmap => words(&["bitm"]),
        Animation => words(&["jmad"]),
        Sound if changes_map(item) => words(&["snd!", "ugh!"]),
        Screens => words(&["wgit", "wgtz"]),
        ScreenEdits => words(&["wgit"]),
        _ => Vec::new(),
    }
}

fn scope_word(s: content::package::ItemScope) -> &'static str {
    use content::package::ItemScope::*;
    match s {
        Sim => "sim",
        Client => "client",
        Live => "live",
        PerFeature => "per-feature",
    }
}

/// The findings `h2 package check` makes about one package, as rows. With
/// `store`, the tag store's root, a file that is a raw resource or a tag
/// file of that store is a finding too (`workspace::store_findings`).
fn findings(k: &Known, id: &str, store: Option<&Path>) -> Res<Vec<Value>> {
    let (loaded, dir) = k.need(id)?;
    let mut out: Vec<Value> = Vec::new();
    let row = |kind: &str, message: String| json!({ "kind": kind, "message": message });
    if let Some(dir) = dir {
        match read_lock(dir) {
            Ok(Some(on_disc)) => {
                for f in lock_findings(&on_disc, &loaded.package.lock) {
                    out.push(row("lock", f));
                }
            }
            Ok(None) => out.push(row(
                "lock",
                format!("no {LOCK_FILE} (`h2 package build` or any package.* edit writes it)"),
            )),
            Err(e) => out.push(row("lock", e.to_string())),
        }
        for f in forbidden_files(dir) {
            out.push(row("files", f));
        }
    }
    if let Some(store) = store {
        for f in store_findings(store, loaded) {
            let mut r = row("store", f.what.clone());
            r["item"] = json!(f.item);
            r["file"] = json!(f.file);
            out.push(r);
        }
    }
    for f in check_package(loaded) {
        let mut r = row("item", f.what.clone());
        r["item"] = json!(f.item);
        r["file"] = json!(f.file);
        r["line"] = json!(f.line);
        out.push(r);
    }
    let file = set_file(&k.data)?;
    match Set::new(k.packages(), &file) {
        Ok(set) => {
            for p in set.problems.iter().filter(|p| p.package() == id) {
                out.push(row("set", p.to_string()));
            }
            let in_order: Vec<Loaded> = set
                .loaded
                .iter()
                .filter_map(|p| k.get(p.name()).map(|(l, _)| l.clone()))
                .collect();
            for f in check_set(&in_order) {
                if f.package == id || f.what.contains(&format!("package {id:?}")) {
                    let mut r = row("conflict", f.to_string());
                    r["item"] = json!(f.item);
                    out.push(r);
                }
            }
        }
        Err(e) => out.push(row("set", e.to_string())),
    }
    Ok(out)
}

/// `package.read`: the manifest, each item with what it builds, the lock
/// and the findings.
pub fn read(data: &Path, id: &str) -> Res {
    read_with(data, id, None)
}

/// [`read`], with the findings over `store` (the tag store's root).
pub fn read_with(data: &Path, id: &str, store: Option<&Path>) -> Res {
    let k = known(data);
    let (loaded, dir) = k.need(id)?;
    let manifest = &loaded.package.manifest;
    let items: Vec<Value> = manifest
        .items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let mut v = serde_json::to_value(item).unwrap_or(Value::Null);
            v["index"] = json!(i);
            v["asset"] = json!(format!("item:{id}:{i}"));
            v["builder"] = json!(builder(item.kind).map(|b| b.name));
            v["builds"] = json!(builds(item));
            v["scope"] = json!(scope_word(item.kind.scope()));
            v["changes_map"] = json!(changes_map(item));
            v
        })
        .collect();
    let lock: Vec<Value> = loaded
        .package
        .lock
        .files
        .iter()
        .map(|(f, h)| json!({ "file": f, "sha256": h.to_string() }))
        .collect();
    let listed = read_set(data);
    let row = listed.listed.iter().find(|l| l.id == id);
    let problems = findings(&k, id, store)?;
    Ok(json!({
        "id": id,
        "package_id": loaded.package.id.to_string(),
        "origin": if dir.is_some() { "local" } else { "baked" },
        "dir": dir.as_ref().map(|d| d.display().to_string()),
        "editable": dir.is_some(),
        "on": row.is_some_and(|r| r.on),
        "why": row.and_then(|r| r.why.clone()),
        "target": target(data).as_deref() == Some(id),
        "package": manifest.package,
        "credit": manifest.credit,
        "items": items,
        "lock": lock,
        "problems": problems,
    }))
}

/// `package.check`: the refusals `h2 package check` makes for one package
/// (a stale lock, a cache map, a bundle or a dataset folder in its folder,
/// an item with no builder, a missing source, a source its builder refuses),
/// and its conflicts with the rest of the set.
pub fn check(data: &Path, id: &str) -> Res {
    check_with(data, id, None)
}

/// [`check`], with the findings over `store` (the tag store's root): a
/// file of the package that is a raw resource or a tag file of the user's
/// own store.
pub fn check_with(data: &Path, id: &str, store: Option<&Path>) -> Res {
    let k = known(data);
    let found = findings(&k, id, store)?;
    Ok(json!({
        "id": id,
        "ok": found.is_empty(),
        "count": found.len(),
        "findings": found,
    }))
}

// ---- writing the manifest ----

fn safe_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains("..")
        && !path.starts_with(['/', '\\'])
        && !path.contains(':')
        && !path.contains('\0')
}

fn check_item_paths(item: &Item) -> Res<()> {
    for f in item.source.iter().chain(item.inputs.iter()) {
        if !safe_path(f) {
            return Err(Refused(format!(
                "{f:?} is not a path inside the package: no .., no drive or leading slash"
            )));
        }
    }
    Ok(())
}

/// Write `manifest` to `dir`, read the package back, put it in the set and
/// lock it. Anything refused puts the old manifest back.
fn commit(data: &Path, dir: &Path, manifest: &Manifest) -> Res<Loaded> {
    manifest.validate().map_err(|e| Refused(e.to_string()))?;
    for item in &manifest.items {
        check_item_paths(item)?;
    }
    let path = dir.join(MANIFEST_FILE);
    let old =
        std::fs::read_to_string(&path).map_err(|e| Failed(format!("{}: {e}", path.display())))?;
    content::store::write_atomic(&path, manifest.to_toml().as_bytes())
        .map_err(|e| Failed(format!("{}: {e}", path.display())))?;
    let undo = |why: Refusal| {
        let _ = content::store::write_atomic(&path, old.as_bytes());
        why
    };
    let loaded = Loaded::from_dir(dir).map_err(|e| undo(Refused(e.to_string())))?;
    let file = set_file(data).map_err(&undo)?;
    ordered(&known(data), &file).map_err(&undo)?;
    let lock = dir.join(LOCK_FILE);
    content::store::write_atomic(&lock, loaded.package.lock.to_toml().as_bytes())
        .map_err(|e| undo(Failed(format!("{}: {e}", lock.display()))))?;
    Ok(loaded)
}

/// Save an editor's source, updating its original item when it belongs to
/// this target. A new document gets an unused path even if the title matches.
pub(super) fn put_document(
    data: &Path,
    id: &str,
    kind: Kind,
    origin: Option<&Path>,
    suggested: &str,
    bytes: &[u8],
) -> Res<PathBuf> {
    put_source(data, id, kind, origin, suggested, None, bytes)
}

/// Sounds have an identity even across sessions: their target tag or clip id.
pub(super) fn put_sound(
    data: &Path,
    id: &str,
    clip: &str,
    tag: Option<&str>,
    bytes: &[u8],
) -> Res<PathBuf> {
    let tag = tag.map(|t| t.trim().replace('/', "\\"));
    if let Some(t) = &tag
        && (t.is_empty() || t.contains("..") || t.starts_with('\\') || t.contains('\0'))
    {
        return Err(Params(format!("{t:?} is not a tag path")));
    }
    let source = match &tag {
        Some(t) => format!(
            "sounds/{}-{}.wav",
            super::sound_import::file_stem(t, "sound"),
            &content::Sha256::of(t.to_ascii_lowercase().as_bytes()).to_string()[..16],
        ),
        None => format!("sounds/{clip}.wav"),
    };
    let wav = ::workspace::import::read_wav(bytes).map_err(|e| Refused(e.to_string()))?;
    ::workspace::import::encode(&wav).map_err(|e| Refused(e.to_string()))?;
    let dir = known(data).local(id)?.1;
    put_source(data, id, Kind::Sound, Some(&dir.join(&source)), &source, tag, bytes)
}

fn put_source(
    data: &Path,
    id: &str,
    kind: Kind,
    origin: Option<&Path>,
    suggested: &str,
    tag: Option<String>,
    bytes: &[u8],
) -> Res<PathBuf> {
    let k = known(data);
    let (loaded, dir) = k.local(id)?;
    let mut manifest = loaded.package.manifest.clone();
    let at = manifest.items.iter().position(|i| {
        i.kind == kind && (i.source.as_ref().is_some_and(|p| origin.is_some_and(|path| path == dir.join(p)))
            || (kind == Kind::Sound && tag.as_ref().zip(i.tag.as_ref()).is_some_and(|(a, b)| a.eq_ignore_ascii_case(b))))
    });
    let existing = at.map(|i| &manifest.items[i]);
    let source = if let Some(item) = existing {
        if kind == Kind::Sound && item.tag != tag
            && !item.tag.as_ref().zip(tag.as_ref()).is_some_and(|(a, b)| a.eq_ignore_ascii_case(b))
        {
            return Err(Refused(format!("{suggested} belongs to another sound")));
        }
        item.source.clone().ok_or_else(|| Refused("the item has no source".into()))?
    } else {
        let used = |s: &str| loaded.files.contains_key(s) || dir.join(s).symlink_metadata().is_ok();
        if kind == Kind::Sound && used(suggested) {
            return Err(Refused(format!("{suggested} already exists")));
        }
        let mut source = suggested.to_owned();
        let (stem, extension) = suggested.rsplit_once('.').expect("editor sources have extensions");
        let mut suffix = 2;
        while used(&source) {
            source = format!("{stem}-{suffix}.{extension}");
            suffix += 1;
        }
        source
    };
    if !safe_path(&source) || bytes.len() as u64 > ::workspace::MAX_FILE_BYTES {
        return Err(Refused(format!("invalid package source {source:?}")));
    }
    if manifest.items.iter().enumerate().any(|(i, item)| {
        Some(i) != at && (item.source.as_deref() == Some(&source) || item.inputs.contains(&source))
    }) {
        return Err(Refused(format!("{source} is shared by another item")));
    }
    let path = dir.join(&source);
    let root = root(data);
    for entry in path.ancestors().take_while(|p| p.starts_with(&root)) {
        match entry.symlink_metadata() {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(Refused(format!("{} is a symlink", entry.display())));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(Failed(format!("{}: {e}", entry.display()))),
        }
    }
    if existing.is_none() {
        manifest.items.push(Item {
            kind, source: Some(source.clone()), inputs: Vec::new(), tag,
            maps: Default::default(), replaces: None, was: None,
        });
    }
    let write = |bytes: &[u8]| content::store::write_atomic(&path, bytes);
    write(bytes).map_err(|e| Failed(format!("{}: {e}", path.display())))?;
    if let Err(why) = commit(data, &dir, &manifest) {
        let restored = match loaded.files.get(&source) {
            Some(old) => write(old),
            None => std::fs::remove_file(&path),
        };
        return Err(match restored {
            Ok(()) => why,
            Err(e) => Failed(format!("{why}. Could not restore {}: {e}", path.display())),
        });
    }
    Ok(path)
}

fn changed(id: &str, loaded: &Loaded) -> Value {
    json!({
        "id": id,
        "package_id": loaded.package.id.to_string(),
        "items": loaded.package.manifest.items.len(),
    })
}

/// `package.new`: an empty package folder with its manifest and lock.
pub fn new(data: &Path, id: &str, name: &str) -> Res {
    if !valid_slug(id) {
        return Err(Params(format!(
            "{id:?} is not a package id: lowercase letters, digits and '-'"
        )));
    }
    if ALWAYS_ON.contains(&id) {
        return Err(Refused(format!(
            "{id:?} is always on and is ours: choose another id"
        )));
    }
    let dir = root(data).join(id);
    if known(data).get(id).is_some() || dir.exists() {
        return Err(Refused(format!("package {id:?} exists")));
    }
    let manifest = Manifest {
        package: Header {
            format: PACKAGE_FORMAT,
            id: id.to_owned(),
            name: name.to_owned(),
            description: None,
            image: None,
            requires: Vec::new(),
            after: Vec::new(),
            features: Vec::new(),
        },
        credit: Credit::default(),
        items: Vec::new(),
    };
    manifest.validate().map_err(|e| Params(e.to_string()))?;
    std::fs::create_dir_all(&dir).map_err(|e| Failed(format!("{}: {e}", dir.display())))?;
    let made = (|| -> Res<Loaded> {
        let path = dir.join(MANIFEST_FILE);
        std::fs::write(&path, manifest.to_toml())
            .map_err(|e| Failed(format!("{}: {e}", path.display())))?;
        let loaded = Loaded::from_dir(&dir).map_err(|e| Failed(e.to_string()))?;
        std::fs::write(dir.join(LOCK_FILE), loaded.package.lock.to_toml())
            .map_err(|e| Failed(format!("{LOCK_FILE}: {e}")))?;
        Ok(loaded)
    })();
    match made {
        Ok(loaded) => {
            let mut v = changed(id, &loaded);
            v["dir"] = json!(dir.display().to_string());
            Ok(v)
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&dir);
            Err(e)
        }
    }
}

/// The manifest with `path` set in its `[package]` table (`name`,
/// `description`, `image`, `requires`, `after`, `features`) or `[credit]`
/// (`credit.authors`, `credit.notes`, ...). A null value removes an optional
/// field. A field the table does not have, or a value of the wrong type, is
/// refused by the manifest's own reader.
fn edit_header(m: &Manifest, path: &str, value: &Value) -> Result<Manifest, String> {
    let (table, key) = match path.split_once('.') {
        Some(("credit", key)) => ("credit", key),
        Some(_) => {
            return Err(format!(
                "{path:?} is a [package] field (name, description, image, requires, after, features) or credit.<field>"
            ));
        }
        None => ("package", path),
    };
    if table == "package" && matches!(key, "id" | "format") {
        return Err(format!(
            "a package's {key} is not edited: the id is its folder's name, so make a new package"
        ));
    }
    let mut v = serde_json::to_value(m).map_err(|e| e.to_string())?;
    let obj = v
        .get_mut(table)
        .and_then(Value::as_object_mut)
        .ok_or_else(|| format!("the manifest has no {table} table"))?;
    if value.is_null() {
        obj.remove(key);
    } else {
        obj.insert(key.to_owned(), value.clone());
    }
    serde_json::from_value(v).map_err(|e| format!("{path}: {e}"))
}

/// `package.set`.
pub fn set(data: &Path, id: &str, path: &str, value: &Value) -> Res {
    let k = known(data);
    let (loaded, dir) = k.local(id)?;
    let manifest = edit_header(&loaded.package.manifest, path, value).map_err(Params)?;
    let l = commit(data, &dir, &manifest)?;
    let mut v = changed(id, &l);
    v["path"] = json!(path);
    v["value"] = value.clone();
    Ok(v)
}

fn item_index(m: &Manifest, index: usize) -> Res<()> {
    if index >= m.items.len() {
        return Err(Params(format!(
            "the package has {} items: no item {index}",
            m.items.len()
        )));
    }
    Ok(())
}

/// `package.item.add`: an item from its manifest form (`{kind, source?,
/// inputs?, tag?, maps?}`), at `at` or last. Its files must be in the
/// package already.
pub fn item_add(data: &Path, id: &str, item: &Value, at: Option<usize>) -> Res {
    let k = known(data);
    let (loaded, dir) = k.local(id)?;
    let item: Item =
        serde_json::from_value(item.clone()).map_err(|e| Params(format!("item: {e}")))?;
    let mut manifest = loaded.package.manifest.clone();
    let at = at.unwrap_or(manifest.items.len());
    if at > manifest.items.len() {
        return Err(Params(format!(
            "at: the package has {} items",
            manifest.items.len()
        )));
    }
    manifest.items.insert(at, item);
    let l = commit(data, &dir, &manifest)?;
    let mut v = changed(id, &l);
    v["item"] = json!(at);
    v["asset"] = json!(format!("item:{id}:{at}"));
    Ok(v)
}

/// `package.item.set`: one field of an item (`kind`, `source`, `inputs`,
/// `tag`, `maps`); null removes an optional one.
pub fn item_set(data: &Path, id: &str, index: usize, path: &str, value: &Value) -> Res {
    let k = known(data);
    let (loaded, dir) = k.local(id)?;
    item_index(&loaded.package.manifest, index)?;
    let mut v =
        serde_json::to_value(&loaded.package.manifest).map_err(|e| Failed(e.to_string()))?;
    let item = v
        .get_mut("item")
        .and_then(|i| i.get_mut(index))
        .and_then(Value::as_object_mut)
        .ok_or_else(|| Failed("the manifest has no item table".into()))?;
    if value.is_null() {
        item.remove(path);
    } else {
        item.insert(path.to_owned(), value.clone());
    }
    let manifest: Manifest =
        serde_json::from_value(v).map_err(|e| Params(format!("item {index} {path}: {e}")))?;
    let l = commit(data, &dir, &manifest)?;
    let mut out = changed(id, &l);
    out["item"] = json!(index);
    out["path"] = json!(path);
    Ok(out)
}

/// `package.item.remove`: the item leaves the manifest; its files stay in
/// the folder.
pub fn item_remove(data: &Path, id: &str, index: usize) -> Res {
    let k = known(data);
    let (loaded, dir) = k.local(id)?;
    item_index(&loaded.package.manifest, index)?;
    let mut manifest = loaded.package.manifest.clone();
    let gone = manifest.items.remove(index);
    let l = commit(data, &dir, &manifest)?;
    let mut v = changed(id, &l);
    v["removed"] = serde_json::to_value(&gone).unwrap_or(Value::Null);
    v["index"] = json!(index);
    Ok(v)
}

/// The edits file the window and `tag.set` write for `map`: a path in the
/// package, which is also the `edits` item's source.
pub fn edits_file(map: &str) -> String {
    format!("edits/{map}.yaml")
}

/// Put `text` in the local package `id` as `edits/<map>.yaml`, with the
/// `edits` item that names it (its `maps` just `map`), and lock the
/// package. Empty text removes the file and its item. Answers whether
/// anything changed on disc.
pub fn put_edits(data: &Path, id: &str, map: &str, text: &str) -> Res<bool> {
    if map.is_empty() || !map.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return Err(Params(format!(
            "map {map:?} cannot name a file: letters, digits, '_' and '-'"
        )));
    }
    put_edits_at(
        data,
        id,
        &edits_file(map),
        content::package::Maps::Names(vec![map.to_owned()]),
        text,
    )
}

/// [`put_edits`] for any file of the package: `rel` is the file and the
/// `edits` item's source, `maps` the item's scope when the item is made.
/// Empty text removes the file and its item.
pub fn put_edits_at(
    data: &Path,
    id: &str,
    rel: &str,
    maps: content::package::Maps,
    text: &str,
) -> Res<bool> {
    if !safe_path(rel) {
        return Err(Refused(format!(
            "{rel:?} is not a path inside the package: no .., no drive or leading slash"
        )));
    }
    let k = known(data);
    let (loaded, dir) = k.local(id)?;
    let path = dir.join(rel);
    let at = loaded
        .package
        .manifest
        .items
        .iter()
        .position(|i| i.kind == content::package::Kind::Edits && i.source.as_deref() == Some(rel));
    let relock = |dir: &Path| -> Res<()> {
        let l = Loaded::from_dir(dir).map_err(|e| Refused(e.to_string()))?;
        let lock = dir.join(LOCK_FILE);
        std::fs::write(&lock, l.package.lock.to_toml())
            .map_err(|e| Failed(format!("{}: {e}", lock.display())))
    };
    if text.is_empty() {
        let Some(at) = at else {
            return Ok(false);
        };
        let mut manifest = loaded.package.manifest.clone();
        manifest.items.remove(at);
        commit(data, &dir, &manifest)?;
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(Failed(format!("{}: {e}", path.display()))),
        }
        relock(&dir)?;
        return Ok(true);
    }
    if at.is_some() && std::fs::read_to_string(&path).is_ok_and(|t| t == text) {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| Failed(format!("{}: {e}", parent.display())))?;
    }
    std::fs::write(&path, text).map_err(|e| Failed(format!("{}: {e}", path.display())))?;
    if at.is_some() {
        relock(&dir)?;
        return Ok(true);
    }
    let mut manifest = loaded.package.manifest.clone();
    manifest.items.push(Item {
        kind: content::package::Kind::Edits,
        source: Some(rel.to_owned()),
        inputs: Vec::new(),
        tag: None,
        maps,
        replaces: None,
        was: None,
    });
    if let Err(e) = commit(data, &dir, &manifest) {
        let _ = std::fs::remove_file(&path);
        return Err(e);
    }
    Ok(true)
}

/// The files of a ground map in its package: its `map.toml` (the `map`
/// item's source) and the ground file it names.
pub fn ground_files(name: &str) -> (String, String) {
    (format!("maps/{name}/map.toml"), format!("maps/{name}/ground.yaml"))
}

/// What [`put_map_ground`] did.
pub struct GroundPut {
    /// The `map` item's place in the manifest.
    pub item: usize,
    /// The ground file's path in the package.
    pub ground: String,
    /// Whether anything changed on disc.
    pub changed: bool,
}

/// Put the AI ground of a map in the local package `id`: a `map` item
/// (`onto`, over the disc map `base`, served as `name`) with the ground file
/// `yaml`, and lock the package. The `edits` items of the map `takes` also
/// name the new map, so it is the open map as edited, with its ground built
/// over it. The item and its files are replaced when `name` is in the
/// package already.
pub fn put_map_ground(
    data: &Path,
    id: &str,
    base: &str,
    name: &str,
    takes: &str,
    yaml: &str,
) -> Res<GroundPut> {
    let (toml_file, ground) = ground_files(name);
    let toml = format!(
        "mode = \"onto\"\nbase = \"{}\"\nname = \"{name}\"\nground = \"{ground}\"\n",
        base.to_ascii_lowercase()
    );
    ::workspace::MapSpec::parse(toml.as_bytes()).map_err(Params)?;
    ::workspace::GroundFile::parse(yaml.as_bytes()).map_err(Params)?;
    let k = known(data);
    let (loaded, dir) = k.local(id)?;
    let mut manifest = loaded.package.manifest.clone();
    let existing = manifest
        .items
        .iter()
        .position(|i| i.kind == Kind::Map && i.source.as_deref() == Some(toml_file.as_str()));
    let item = existing.unwrap_or(manifest.items.len());
    if existing.is_none() {
        manifest.items.push(Item {
            kind: Kind::Map,
            source: Some(toml_file.clone()),
            inputs: vec![ground.clone()],
            tag: None,
            maps: content::package::Maps::Scope(MapScope::All),
            replaces: None,
            was: None,
        });
    }
    // The loader reads only the files items name: the ground file is the map item's input.
    let mut inputs_changed = false;
    if let Some(i) = existing
        && !manifest.items[i].inputs.contains(&ground)
    {
        manifest.items[i].inputs.push(ground.clone());
        inputs_changed = true;
    }
    // The new map takes what the open map's edits change.
    let mut renamed = false;
    for i in &mut manifest.items {
        if i.kind != Kind::Edits {
            continue;
        }
        if let content::package::Maps::Names(names) = &mut i.maps
            && names.iter().any(|n| n.eq_ignore_ascii_case(takes))
            && !names.iter().any(|n| n.eq_ignore_ascii_case(name))
        {
            names.push(name.to_owned());
            renamed = true;
        }
    }
    let mut changed = existing.is_none() || renamed || inputs_changed;
    let mut written = Vec::new();
    for (rel, text) in [(&toml_file, toml.as_str()), (&ground, yaml)] {
        let path = dir.join(rel);
        if std::fs::read_to_string(&path).is_ok_and(|t| t == text) {
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Failed(format!("{}: {e}", parent.display())))?;
        }
        std::fs::write(&path, text).map_err(|e| Failed(format!("{}: {e}", path.display())))?;
        written.push(path);
        changed = true;
    }
    if let Err(e) = commit(data, &dir, &manifest) {
        if existing.is_none() {
            for path in written {
                let _ = std::fs::remove_file(path);
            }
        }
        return Err(e);
    }
    Ok(GroundPut {
        item,
        ground,
        changed,
    })
}

// ---- the set file ----

/// `package.enable`: switch a package on or off in `set.toml`. An always-on
/// package is refused by name. The result lists the packages the change
/// leaves out because they `require` this one.
pub fn enable(data: &Path, id: &str, on: bool) -> Res {
    if !on && ALWAYS_ON.contains(&id) {
        return Err(Refused(
            content::package::SetError::AlwaysOn(id.to_owned()).to_string(),
        ));
    }
    let k = known(data);
    k.need(id)?;
    let mut file = set_file(data)?;
    file.off.retain(|n| n != id);
    if !on {
        file.off.push(id.to_owned());
    }
    let set = ordered(&k, &file)?;
    write_set_file(data, &file)?;
    let left_out: Vec<Value> = set
        .problems
        .iter()
        .map(|p| json!({ "package": p.package(), "why": p.to_string() }))
        .collect();
    Ok(json!({
        "id": id,
        "on": on,
        "off": file.off,
        "left_out": left_out,
        "order": set.loaded.iter().map(|p| p.name()).collect::<Vec<_>>(),
    }))
}

/// `package.order`: the user's load order, first to load first. It breaks
/// ties the dependencies leave. An order that puts a package before one it
/// `requires` (or follows with `after`) is refused naming both.
pub fn order(data: &Path, ids: &[String]) -> Res {
    let k = known(data);
    for (i, id) in ids.iter().enumerate() {
        k.need(id)?;
        if ids[..i].contains(id) {
            return Err(Params(format!("ids: {id:?} is listed twice")));
        }
    }
    let at = |n: &str| ids.iter().position(|i| i == n);
    for id in ids {
        let h = &k.need(id)?.0.package.manifest.package;
        for (list, word) in [(&h.requires, "requires"), (&h.after, "follows")] {
            for dep in list {
                if let (Some(me), Some(them)) = (at(id), at(dep))
                    && them > me
                {
                    return Err(Refused(format!(
                        "package {id:?} {word} {dep:?}, so {dep:?} must come before it in the order"
                    )));
                }
            }
        }
    }
    let mut file = set_file(data)?;
    file.order = ids.to_vec();
    let set = ordered(&k, &file)?;
    write_set_file(data, &file)?;
    Ok(json!({
        "order": ids,
        "loaded": set.loaded.iter().map(|p| p.name()).collect::<Vec<_>>(),
        "left_out": set.problems.iter().map(|p| json!({
            "package": p.package(), "why": p.to_string(),
        })).collect::<Vec<_>>(),
    }))
}

// ---- building ----

/// A map the build can start from: where its file is.
#[derive(Debug, Clone)]
pub struct BaseMap {
    pub name: String,
    pub path: PathBuf,
    /// `(offset, length)` inside `path` for a map in a disc image.
    pub region: Option<(u64, u64)>,
}

fn scope_at(base: &BaseMap) -> Result<MapScope, String> {
    let mut f = File::open(&base.path).map_err(|e| format!("{}: {e}", base.path.display()))?;
    let (at, len) = match base.region {
        Some(r) => r,
        None => (
            0,
            f.metadata()
                .map_err(|e| format!("{}: {e}", base.path.display()))?
                .len(),
        ),
    };
    let h = map::Header::read_region(&mut f, at, len).map_err(|e| format!("not a map: {e}"))?;
    Ok(scope_of(h.cache_type))
}

/// The class of `base`'s map, read from its header.
pub fn scope_of_base(base: &BaseMap) -> Result<MapScope, String> {
    scope_at(base)
}

/// The package `id` as loaded, built in or local: its manifest and files.
pub fn loaded(data: &Path, id: &str) -> Res<Loaded> {
    Ok(known(data).need(id)?.0.clone())
}

/// The local package `id` as loaded, with its folder; a built-in one is
/// refused.
pub fn loaded_local(data: &Path, id: &str) -> Res<(Loaded, PathBuf)> {
    let k = known(data);
    let (l, dir) = k.local(id)?;
    Ok((l.clone(), dir))
}

/// Every package the set has on, in load order: what a level is built over.
pub fn in_order(data: &Path) -> Res<Vec<Loaded>> {
    let k = known(data);
    let file = set_file(data)?;
    let set = ordered(&k, &file)?;
    Ok(set
        .loaded
        .iter()
        .filter_map(|p| k.get(p.name()).map(|(l, _)| l.clone()))
        .collect())
}

/// The packages a build of `id` applies, in load order: `id` and what it
/// `requires`, transitively. A package that is off is refused with why.
fn closure(data: &Path, k: &Known, id: &str) -> Res<Vec<Loaded>> {
    k.need(id)?;
    let file = set_file(data)?;
    let set = ordered(k, &file)?;
    if !set.loaded.iter().any(|p| p.name() == id) {
        let why = read_set(data)
            .listed
            .into_iter()
            .find(|l| l.id == id)
            .and_then(|l| l.why)
            .unwrap_or_else(|| "it is not in the set".into());
        return Err(Refused(format!(
            "package {id:?} is off ({why}): package.enable turns it on"
        )));
    }
    let mut want = BTreeSet::new();
    let mut todo = vec![id.to_owned()];
    while let Some(n) = todo.pop() {
        if want.insert(n.clone())
            && let Some((l, _)) = k.get(&n)
        {
            todo.extend(l.package.manifest.package.requires.iter().cloned());
        }
    }
    Ok(set
        .loaded
        .iter()
        .filter(|p| want.contains(p.name()))
        .filter_map(|p| k.get(p.name()).map(|(l, _)| l.clone()))
        .collect())
}

/// `package.build`: what the package (and what it requires) gives each of
/// `bases`. With `only_touched`, a map the package has no item for is left
/// out; otherwise it is answered with `touched: false` and not built. Each
/// row is `{map, scope, touched, took_ms, tags, added, changed, overrides,
/// bytes, problems}`; a build that fails has `problems`. With `out`, each
/// built map is written to `<out>/<map>.map`.
pub fn build(
    data: &Path,
    id: &str,
    bases: &[BaseMap],
    only_touched: bool,
    out: Option<&Path>,
) -> Res<Vec<Value>> {
    let k = known(data);
    let packages = closure(data, &k, id)?;
    let me = k.need(id)?.0.package.manifest.clone();
    let mut rows = Vec::new();
    for base in bases {
        let row = |touched: bool, problems: Vec<String>| {
            json!({
                "map": base.name,
                "touched": touched,
                "took_ms": 0,
                "tags": 0,
                "added": 0,
                "changed": 0,
                "overrides": [],
                "bytes": 0,
                "problems": problems,
            })
        };
        let scope = match scope_at(base) {
            Ok(s) => s,
            Err(e) => {
                if !only_touched {
                    rows.push(row(false, vec![e]));
                }
                continue;
            }
        };
        let target = Target {
            name: &base.name,
            scope,
        };
        let touched = me
            .items
            .iter()
            .any(|i| changes_map(i) && target.takes(&i.maps));
        if !touched {
            if !only_touched {
                let mut r = row(false, Vec::new());
                r["scope"] = json!(scope.word());
                rows.push(r);
            }
            continue;
        }
        let began = Instant::now();
        let built = scenario_edit::MapSource::region(&base.path, base.region)
            .read_base()
            .map_err(|e| format!("{}: {e}", base.path.display()))
            .and_then(|bytes| build_map(&bytes, &packages, &target));
        let mut r = match built {
            Ok((bytes, report)) => {
                let mut problems = Vec::new();
                if let Some(dir) = out {
                    let file = dir.join(format!("{}.map", base.name));
                    if let Err(e) =
                        std::fs::create_dir_all(dir).and_then(|()| std::fs::write(&file, &bytes))
                    {
                        problems.push(format!("{}: {e}", file.display()));
                    }
                }
                let mut r = row(true, problems);
                r["tags"] = json!(report.added.len() + report.changed.len());
                r["added"] = json!(report.added.len());
                r["changed"] = json!(report.changed.len());
                r["overrides"] = json!(
                    report
                        .overrides
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                );
                r["bytes"] = json!(bytes.len());
                r
            }
            Err(e) => row(true, vec![e]),
        };
        r["scope"] = json!(scope.word());
        r["took_ms"] = json!(began.elapsed().as_millis() as u64);
        rows.push(r);
    }
    Ok(rows)
}

/// Export the manifest and only its named files, with a lock for these bytes.
pub(crate) fn export(data: &Path, store: &Path, id: &str, out: &Path) -> Res {
    if !valid_slug(id) {
        return Err(Params(format!("invalid package id {id:?}")));
    }
    let (loaded, dir) = loaded_local(data, id)?;
    let mut refused = forbidden_files(&dir);
    refused.extend(store_findings(store, &loaded).into_iter().map(|f| f.what));
    if !refused.is_empty() {
        return Err(Refused(refused.join("\n")));
    }
    let package_dir = dir.canonicalize().map_err(|e| Failed(e.to_string()))?;
    for name in loaded.files.keys() {
        let source = dir.join(name).canonicalize().map_err(|e| Failed(format!("{name}: {e}")))?;
        if !source.starts_with(&package_dir)
            || source == package_dir.join(MANIFEST_FILE)
            || source == package_dir.join(LOCK_FILE)
        {
            return Err(Refused(format!("{name:?} is not an exportable package source")));
        }
    }
    let failed = |e: std::io::Error| Failed(format!("{}: {e}", out.display()));
    std::fs::create_dir(out).map_err(failed)?;
    let write = || -> std::io::Result<()> {
        std::fs::write(out.join(MANIFEST_FILE), loaded.package.manifest.to_toml())?;
        std::fs::write(out.join(LOCK_FILE), loaded.package.lock.to_toml())?;
        for (name, bytes) in &loaded.files {
            let file = out.join(name);
            if let Some(parent) = file.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(file, bytes)?;
        }
        Ok(())
    };
    if let Err(e) = write() {
        let _ = std::fs::remove_dir_all(out);
        return Err(failed(e));
    }
    Ok(json!({"id": id, "out": out, "files": loaded.files.len() + 2}))
}

/// The package tab and its most recent command's result.
#[derive(Default)]
pub(crate) struct Document {
    pub read: Option<Value>,
    pub result: Option<(&'static str, Result<Value, String>)>,
    pub revision: u64,
}

impl Document {
    pub fn id(&self) -> Option<&str> {
        self.read.as_ref()?["id"].as_str()
    }

    pub fn name(&self) -> Option<&str> {
        self.read.as_ref()?["package"]["name"].as_str()
    }
}

impl super::App {
    pub(super) fn open_package_document(&mut self, id: &str) {
        if !valid_slug(id) {
            return self.error(format!("invalid package id {id:?}"));
        }
        match read_with(&self.data_root(), id, Some(&self.tags_root())) {
            Ok(read) => {
                self.package_document.read = Some(read);
                self.package_document.result = None;
                self.open_doc(super::Doc::Package);
            }
            Err(e) => self.error(e.to_string()),
        }
    }

    pub(crate) fn refresh_package_document(&mut self) {
        self.package_document.revision += 1;
        if let Some(id) = self.package_document.id()
            && let Ok(read) = read_with(&self.data_root(), id, Some(&self.tags_root()))
        {
            self.package_document.read = Some(read);
        }
    }

    pub(super) fn package_command(&mut self, method: &'static str, params: Value) {
        use crate::api::{P, packages};
        use studio_proto::method as m;

        let answer = packages::call(self, method, &P(&params)).and_then(|reply| {
            if method == m::PACKAGE_NEW && let Some(id) = reply["id"].as_str() {
                packages::call(self, m::PACKAGE_TARGET, &P(&json!({"id": id})))?;
                self.open_package_document(id);
            }
            Ok(reply)
        });
        match &answer {
            Ok(reply) => self.info(format!("{method} {reply}")),
            Err(e) => self.error(e.message.clone()),
        }
        self.package_document.result = Some((method, answer.map_err(|e| e.message)));
    }
}
