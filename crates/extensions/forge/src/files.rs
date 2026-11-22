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
    let result = file
        .write_all(bytes.as_bytes())
        .and_then(|()| file.sync_all());
    drop(file);
    let result = result.and_then(|()| {
        for n in 1..=u64::MAX {
            let name = if n == 1 {
                base.to_string()
            } else {
                let suffix = format!("-{n}");
                format!("{}{suffix}", &base[..base.len().min(64 - suffix.len())])
            };
            match fs::hard_link(&tmp, dir.join(format!("{name}.h2forge"))) {
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
