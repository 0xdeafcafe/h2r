//! `import.sound`: a WAV file as a `sound` item of a local package
//! (docs/editor/studio-redesign.md, "Sounds"). The file is copied into the
//! package unchanged; the package's build resamples it, encodes it as Xbox
//! ADPCM and writes the `snd!` and its gestalt rows
//! (`workspace::import`), so what is checked here is what the build will
//! do with it.
use std::path::Path;

use ::workspace::import::{self, class_of, encode, read_wav, sidecar_path};
use ::workspace::{Loaded, added_tag};
use content::package::{Item, Kind as ItemKind, LOCK_FILE, MANIFEST_FILE, Manifest, Maps};
use map::FourCC;
use serde_json::{Value, json};
use tags::repo::TagKey;

use super::assets::{CopyRefusal, read_set};

/// Characters of a tag's last segment a file name keeps.
fn file_stem(tag: &str) -> String {
    let leaf = tag.rsplit('\\').next().unwrap_or(tag);
    let s: String = leaf
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() { "sound".into() } else { s }
}

/// Add the WAV at `file` to the local package `package` as a `sound` item
/// that writes the `snd!` `tag`, in `class` (a name or number; the
/// default class when `None`).
pub fn import_sound(
    data_root: &Path,
    file: &Path,
    package: &str,
    tag: &str,
    class: Option<&str>,
) -> Result<Value, CopyRefusal> {
    use CopyRefusal::{Failed, Params, Refused};
    let tag = tag.trim().replace('/', "\\");
    if tag.is_empty() || tag.contains("..") || tag.starts_with('\\') || tag.contains('\0') {
        return Err(Params(format!("{tag:?} is not a tag path")));
    }
    let class = class
        .map(|c| class_of(c).map(|n| (c.trim().to_owned(), n)))
        .transpose()
        .map_err(|e| Params(format!("class: {e}")))?;
    let set = read_set(data_root);
    let dir = set
        .listed
        .iter()
        .find(|l| l.id == package)
        .ok_or_else(|| Params(format!("no package {package:?}")))
        .and_then(|l| match &l.dir {
            Some(d) => Ok(std::path::PathBuf::from(d)),
            None => Err(Refused(format!(
                "package {package:?} is built in: import into a package under <data>/packages/"
            ))),
        })?;
    let bytes = std::fs::read(file).map_err(|e| Failed(format!("{}: {e}", file.display())))?;
    let clip = read_wav(&bytes).map_err(|e| Refused(format!("{}: {e}", file.display())))?;
    let encoded = encode(&clip).map_err(|e| Refused(format!("{}: {e}", file.display())))?;

    let manifest_path = dir.join(MANIFEST_FILE);
    let text = std::fs::read_to_string(&manifest_path)
        .map_err(|e| Failed(format!("{}: {e}", manifest_path.display())))?;
    let mut manifest = Manifest::from_toml(&text)
        .map_err(|e| Failed(format!("{}: {e}", manifest_path.display())))?;
    let key = TagKey::new(FourCC(*b"snd!"), tag.clone());
    if manifest
        .items
        .iter()
        .any(|i| added_tag(i).as_ref() == Some(&key))
    {
        return Err(Refused(format!(
            "package {package:?} adds snd!:{tag} already"
        )));
    }
    let source = format!("sounds/{}.wav", file_stem(&tag));
    let side = sidecar_path(&source);
    let target = dir.join(&source);
    if target.exists() {
        return Err(Refused(format!("{} exists", target.display())));
    }
    let mut written = Vec::new();
    let undo = |written: &[std::path::PathBuf], e: String| {
        for f in written {
            let _ = std::fs::remove_file(f);
        }
        Failed(e)
    };
    if let Some(d) = target.parent() {
        std::fs::create_dir_all(d).map_err(|e| Failed(format!("{}: {e}", d.display())))?;
    }
    std::fs::write(&target, &bytes).map_err(|e| Failed(format!("{}: {e}", target.display())))?;
    written.push(target);
    manifest.items.push(Item {
        kind: ItemKind::Sound,
        source: Some(source.clone()),
        tag: Some(tag.clone()),
        maps: Maps::default(),
    });
    if let Some((name, _)) = &class {
        let path = dir.join(&side);
        std::fs::write(&path, format!("class: {name}\n"))
            .map_err(|e| undo(&written, format!("{}: {e}", path.display())))?;
        written.push(path);
        manifest.items.push(Item {
            kind: ItemKind::Data,
            source: Some(side.clone()),
            tag: None,
            maps: Maps::default(),
        });
    }
    manifest
        .validate()
        .map_err(|e| undo(&written, e.to_string()))?;
    std::fs::write(&manifest_path, manifest.to_toml())
        .map_err(|e| undo(&written, format!("{}: {e}", manifest_path.display())))?;
    // The lock follows the files, as `package.check` expects.
    let loaded = Loaded::from_dir(&dir).map_err(|e| Failed(e.to_string()))?;
    std::fs::write(dir.join(LOCK_FILE), loaded.package.lock.to_toml())
        .map_err(|e| Failed(format!("{LOCK_FILE}: {e}")))?;
    let item = manifest.items.len() - 1 - usize::from(class.is_some());
    Ok(json!({
        "package": package,
        "tag": format!("tag:snd!:{tag}"),
        "source": source,
        "sidecar": class.as_ref().map(|_| side),
        "item": item,
        "dir": dir.display().to_string(),
        "class": class.map(|(_, n)| n).unwrap_or(import::DEFAULT_CLASS),
        "from": {"rate": clip.rate, "channels": clip.channels, "frames": clip.frames()},
        "rate": encoded.rate,
        "frames": encoded.frames,
        "duration_ms": encoded.frames as u64 * 1000 / u64::from(encoded.rate),
        "bytes": encoded.adpcm.len(),
    }))
}
