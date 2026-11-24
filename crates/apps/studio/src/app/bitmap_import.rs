//! `import.bitmap`: a PNG as a `bitmap` item of a local package, a new
//! `bitm` the package's build adds (docs/editor/live-editors.md, LE3). The
//! PNG is copied into the package unchanged; the `bitmap` builder
//! (`workspace::bitmap`) decodes it and writes one linear `ARGB8888` image,
//! so what is checked here is what the build will write. A new tag is a
//! Shape change: it reaches a game by a build and a load, never a push.
use std::path::Path;

use ::workspace::{Loaded, added_tag, decode_png};
use content::package::{Item, Kind as ItemKind, LOCK_FILE, MANIFEST_FILE, Manifest, Maps};
use map::FourCC;
use serde_json::{Value, json};
use tags::repo::TagKey;

use super::assets::CopyRefusal;
use super::sound_import::{file_stem, package_dir};

/// Add the PNG at `file` to the local package `package` as a `bitmap` item
/// that writes the `bitm` `tag`.
pub fn import_bitmap(
    data_root: &Path,
    file: &Path,
    package: &str,
    tag: &str,
) -> Result<Value, CopyRefusal> {
    use CopyRefusal::{Failed, Params, Refused};
    let tag = tag.trim().replace('/', "\\");
    if tag.is_empty() || tag.contains("..") || tag.starts_with('\\') || tag.contains('\0') {
        return Err(Params(format!("{tag:?} is not a tag path")));
    }
    let dir = package_dir(data_root, package)?;
    let bytes = std::fs::read(file).map_err(|e| Failed(format!("{}: {e}", file.display())))?;
    let pic = decode_png(&bytes).map_err(|e| Refused(format!("{}: {e}", file.display())))?;

    let manifest_path = dir.join(MANIFEST_FILE);
    let text = std::fs::read_to_string(&manifest_path)
        .map_err(|e| Failed(format!("{}: {e}", manifest_path.display())))?;
    let mut manifest = Manifest::from_toml(&text)
        .map_err(|e| Failed(format!("{}: {e}", manifest_path.display())))?;
    let key = TagKey::new(FourCC(*b"bitm"), tag.clone());
    if manifest
        .items
        .iter()
        .any(|i| added_tag(i).as_ref() == Some(&key))
    {
        return Err(Refused(format!(
            "package {package:?} adds bitm:{tag} already"
        )));
    }
    let source = format!("bitmaps/{}.png", file_stem(&tag, "bitmap"));
    let target = dir.join(&source);
    if target.exists() {
        return Err(Refused(format!("{} exists", target.display())));
    }
    if let Some(d) = target.parent() {
        std::fs::create_dir_all(d).map_err(|e| Failed(format!("{}: {e}", d.display())))?;
    }
    std::fs::write(&target, &bytes).map_err(|e| Failed(format!("{}: {e}", target.display())))?;
    let undo = |e: String| {
        let _ = std::fs::remove_file(&target);
        Failed(e)
    };
    manifest.items.push(Item {
        kind: ItemKind::Bitmap,
        inputs: Vec::new(),
        source: Some(source.clone()),
        tag: Some(tag.clone()),
        maps: Maps::default(),
    });
    manifest.validate().map_err(|e| undo(e.to_string()))?;
    std::fs::write(&manifest_path, manifest.to_toml())
        .map_err(|e| undo(format!("{}: {e}", manifest_path.display())))?;
    // The lock follows the files, as `package.check` expects.
    let loaded = Loaded::from_dir(&dir).map_err(|e| Failed(e.to_string()))?;
    std::fs::write(dir.join(LOCK_FILE), loaded.package.lock.to_toml())
        .map_err(|e| Failed(format!("{LOCK_FILE}: {e}")))?;
    Ok(json!({
        "package": package,
        "tag": format!("tag:bitm:{tag}"),
        "source": source,
        "item": manifest.items.len() - 1,
        "dir": dir.display().to_string(),
        "width": pic.width,
        "height": pic.height,
        "format": "argb8888",
        "mips": 1,
        "bytes": u64::from(pic.width) * u64::from(pic.height) * 4,
        "class": "shape",
    }))
}
