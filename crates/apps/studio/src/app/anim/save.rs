//! Save an animation document as source for the package animation builder.
//! See docs/editor/studio-redesign.md, "The animation editor".

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anim_edit::{ClipKind, Operation, Source};
use content::Sha256;
use content::package::{Item, Kind, LOCK_FILE, MANIFEST_FILE, Maps, Package};
use content::store::write_atomic;

use super::{AnimError, App};
use crate::app::assets::{CopyRefusal, read_set};
use ::workspace::{Job, Loaded, builder};

/// Action placement for a new or duplicated clip. A retail clip's own
/// document stays at its existing action.
#[derive(Clone, Copy, Default)]
pub struct AnimationPlacement<'a> {
    pub mode: Option<&'a str>,
    pub class: Option<&'a str>,
    pub weapon_type: Option<&'a str>,
    pub action: Option<&'a str>,
}

pub struct SavedAnimation {
    pub package: String,
    pub tag: String,
    pub map: String,
    pub source: String,
    pub item: usize,
}

use CopyRefusal::{Failed, Params, Refused};

fn animation_error(e: AnimError) -> CopyRefusal {
    match e {
        AnimError::Refused(why) => Refused(why),
        AnimError::Failed(why) => Failed(why),
    }
}

impl App {
    /// The socket and the Animation window save through this path. The package
    /// item is scoped to this map's variant of the graph.
    pub fn anim_save(
        &mut self,
        doc: u64,
        package: &str,
        placement: AnimationPlacement<'_>,
    ) -> Result<SavedAnimation, CopyRefusal> {
        let tag_id = self.anim_doc_tag(doc).map_err(animation_error)?;
        let map = self
            .shown_map_name()
            .ok_or_else(|| Refused("the Tags tool is not showing a map".to_owned()))?;
        let graph = self
            .shown_tags()
            .tag(tag_id)
            .filter(|tag| tag.group.0 == *b"jmad")
            .map(|tag| tag.name.as_str())
            .ok_or_else(|| Refused(format!("{tag_id} is not a jmad in {map}")))?;
        let entry = self.anim_document(doc).map_err(animation_error)?;
        let replaced = entry.retail_index.map(|_| entry.summary.name.as_str());
        let operation = if let Some(name) = replaced {
            if placement.mode.is_some()
                || placement.class.is_some()
                || placement.weapon_type.is_some()
                || placement.action.is_some()
            {
                return Err(Params(
                    "a source clip keeps its action; duplicate it to choose an action".to_owned(),
                ));
            }
            Operation::Replace {
                animation: name.to_owned(),
            }
        } else {
            let kind = match entry.summary.kind {
                "base" => ClipKind::Base,
                "overlay" => ClipKind::Overlay,
                "replacement" => ClipKind::Replacement,
                kind => return Err(Refused(format!("animation type {kind:?} cannot be saved"))),
            };
            Operation::Add {
                mode: placement.mode.unwrap_or("any").to_owned(),
                class: placement.class.unwrap_or("any").to_owned(),
                weapon_type: placement.weapon_type.unwrap_or("any").to_owned(),
                action: placement
                    .action
                    .ok_or_else(|| Params("action is required for a new clip".to_owned()))?
                    .to_owned(),
                kind,
            }
        };

        let dir = package_dir(&self.data_root(), package)?;
        let clip = entry.clip.name();
        let source = source_path(map, graph, replaced.unwrap_or(clip), replaced.is_some());
        let authored = Source::from_doc(graph.to_owned(), operation, &entry.clip);
        let item = persist(&dir, package, map, &source, &authored)?;
        Ok(SavedAnimation {
            package: package.to_owned(),
            tag: format!("tag:jmad:{graph}"),
            map: map.to_owned(),
            source,
            item,
        })
    }
}

fn package_dir(data_root: &Path, package: &str) -> Result<PathBuf, CopyRefusal> {
    let set = read_set(data_root);
    let row = set
        .listed
        .iter()
        .find(|row| row.id == package)
        .ok_or_else(|| Params(format!("no package {package:?}")))?;
    row.dir.as_ref().map(PathBuf::from).ok_or_else(|| {
        Refused(format!(
            "package {package:?} is built in; choose a local package"
        ))
    })
}

/// Derive a stable source path without interpreting map, tag or clip names
/// as path components. Changing an authored clip's action updates its item.
fn source_path(map: &str, graph: &str, clip: &str, replace: bool) -> String {
    let key = format!(
        "{}\0{}\0{clip}\0{replace}",
        map.to_ascii_lowercase(),
        graph.to_ascii_lowercase()
    );
    let id = Sha256::of(key.as_bytes()).to_string();
    let leaf: String = clip
        .chars()
        .take(36)
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("animations/{leaf}-{}.anim.yaml", &id[..16])
}

fn persist(
    dir: &Path,
    package: &str,
    map: &str,
    source: &str,
    authored: &Source,
) -> Result<usize, CopyRefusal> {
    let Loaded {
        package: loaded,
        mut files,
    } = Loaded::from_dir(dir).map_err(|e| Failed(e.to_string()))?;
    if loaded.name() != package {
        return Err(Refused(format!(
            "{} is no longer package {package:?}",
            dir.display()
        )));
    }
    let mut manifest = loaded.manifest;
    let mut lock = loaded.lock;
    let maps = Maps::Names(vec![map.to_owned()]);
    let file = dir.join(source);
    let source_dir = file.parent().expect("a path inside the package");
    if let Some(metadata) = path_metadata(source_dir)?
        && !metadata.is_dir()
    {
        return Err(Refused(format!(
            "{} must be a directory inside the package",
            source_dir.display()
        )));
    }

    let item = Item {
        kind: Kind::Animation,
        inputs: Vec::new(),
        source: Some(source.to_owned()),
        tag: Some(authored.graph.clone()),
        maps,
        replaces: None,
        was: None,
    };
    let mut found = None;
    let conflict = || Refused(format!("{source} is already used by another package item"));
    for (index, old) in manifest.items.iter().enumerate() {
        if old.inputs.iter().any(|p| p == source) {
            return Err(conflict());
        }
        if old.source.as_deref() == Some(source) {
            if old != &item || found.is_some() {
                return Err(conflict());
            }
            found = Some(index);
        }
    }
    if found.is_some() {
        let old = files.get(source).expect("loaded with the manifest");
        let old = Source::parse(old).map_err(|e| Refused(format!("{source}: {e}")))?;
        if !same_target(&old, authored) {
            return Err(conflict());
        }
    } else if path_metadata(&file)?.is_some() {
        return Err(Refused(format!("{} already exists", file.display())));
    }

    let index = found.unwrap_or(manifest.items.len());
    if found.is_none() {
        manifest.items.push(item.clone());
    }
    let yaml = authored.to_yaml().map_err(Failed)?.into_bytes();
    if yaml.len() as u64 > ::workspace::MAX_FILE_BYTES {
        return Err(Refused(format!(
            "{source} exceeds the package source size limit"
        )));
    }
    lock.files.insert(source.to_owned(), Sha256::of(&yaml));
    let old_source = files.insert(source.to_owned(), yaml);
    let yaml = files.get(source).expect("inserted");
    let check = builder(Kind::Animation)
        .ok_or_else(|| Failed("the animation builder is unavailable".to_owned()))?
        .check;
    check(&Job {
        item: &item,
        source: yaml,
        files: &files,
    })
    .map_err(|e| Refused(e.why))?;
    let package = Package::new(manifest, lock).map_err(|e| Refused(e.to_string()))?;

    // Validate and calculate the next lock first. Put a new item in its
    // manifest only after the referenced source exists.
    let lock_path = dir.join(LOCK_FILE);
    let old_lock = read_optional(&lock_path)?;
    write_atomic(&file, yaml).map_err(|e| Failed(file_error(&file, e)))?;
    if let Err(e) = write_atomic(&lock_path, package.lock.to_toml().as_bytes()) {
        return Err(restore(
            file_error(&lock_path, e),
            &[(&file, old_source.as_deref())],
        ));
    }
    let manifest_path = dir.join(MANIFEST_FILE);
    if found.is_none()
        && let Err(e) = write_atomic(&manifest_path, package.manifest.to_toml().as_bytes())
    {
        return Err(restore(
            file_error(&manifest_path, e),
            &[
                (&lock_path, old_lock.as_deref()),
                (&file, old_source.as_deref()),
            ],
        ));
    }
    Ok(index)
}

fn same_target(old: &Source, new: &Source) -> bool {
    old.graph.eq_ignore_ascii_case(&new.graph)
        && match (&old.operation, &new.operation) {
            (Operation::Replace { animation: a }, Operation::Replace { animation: b }) => a == b,
            (Operation::Add { .. }, Operation::Add { .. }) => old.name == new.name,
            _ => false,
        }
}

fn file_error(path: &Path, error: std::io::Error) -> String {
    format!("{}: {error}", path.display())
}

fn path_metadata(path: &Path) -> Result<Option<fs::Metadata>, CopyRefusal> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Failed(file_error(path, e))),
    }
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, CopyRefusal> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Failed(file_error(path, e))),
    }
}

fn restore(mut error: String, files: &[(&Path, Option<&[u8]>)]) -> CopyRefusal {
    for &(path, old) in files {
        let result = match old {
            Some(bytes) => write_atomic(path, bytes),
            None => fs::remove_file(path),
        };
        if let Err(e) = result {
            error.push_str(&format!(". Could not restore {}", file_error(path, e)));
        }
    }
    Failed(error)
}
