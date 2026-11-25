//! Converted levels' markers in the tag store (docs/engine/asset-store.md,
//! slice AS10): the read side, shared by the game's map lists and
//! `workspace::converted`, which writes them.

use std::path::{Path, PathBuf};

use content::Sha256;

use super::StoreFile;

/// Extension of a converted level's marker, in `<store>/maps/`.
pub const MARKER_EXT: &str = "converted";

/// A converted level in the store, as [`levels`] lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marked {
    pub stem: String,
    pub converter: String,
    /// The SHA-256 of the map the converter wrote (hex).
    pub file: String,
}

/// `<store>/maps/<stem>.converted`.
pub fn marker_path(store: &Path, stem: &str) -> PathBuf {
    store.join("maps").join(format!("{stem}.{MARKER_EXT}"))
}

pub fn marker_text(converter: &str, file: &Sha256) -> String {
    format!("converter {converter}\nfile {file}\n")
}

pub fn parse_marker(stem: &str, text: &str) -> Option<Marked> {
    let (mut converter, mut file) = (None, None);
    for line in text.lines() {
        match line.split_once(' ') {
            Some(("converter", v)) => converter = Some(v.to_owned()),
            Some(("file", v)) => file = Some(v.to_owned()),
            _ => {}
        }
    }
    Some(Marked {
        stem: stem.to_owned(),
        converter: converter?,
        file: file?,
    })
}

/// The marker of `stem`, when it has one that reads.
pub fn marked(store: &Path, stem: &str) -> Option<Marked> {
    parse_marker(
        stem,
        &std::fs::read_to_string(marker_path(store, stem)).ok()?,
    )
}

/// Every converted level of the store, by stem.
pub fn levels(store: &Path) -> Vec<Marked> {
    let Ok(dir) = std::fs::read_dir(store.join("maps")) else {
        return Vec::new();
    };
    let mut out: Vec<Marked> = dir
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let stem = name.strip_suffix(&format!(".{MARKER_EXT}"))?;
            marked(store, stem)
        })
        .collect();
    out.sort_by(|a, b| a.stem.cmp(&b.stem));
    out
}

/// Whether the store holds `stem` as the converted level whose map file is
/// `file`: the marker names it and the level's extent table says the same
/// file. A converter's unit whose level is not stored is converted again.
pub fn is_stored(store: &Path, stem: &str, file: &Sha256) -> bool {
    let want = file.to_string();
    marked(store, stem).is_some_and(|m| m.file == want)
        && StoreFile::exists(store, stem)
        && StoreFile::extents(store, stem).is_ok_and(|e| e.file == want)
}

/// A converter's marker whose hash matches the stored level.
pub fn verified(store: &Path, mark: &Marked, converter: &str) -> Option<Sha256> {
    let hash = mark.file.parse().ok()?;
    (mark.converter == converter && is_stored(store, &mark.stem, &hash)).then_some(hash)
}

/// Metadata for the Halo 1 multiplayer map list, indexed by the guest map name.
pub fn halo1_metadata(store: &Path) -> PathBuf {
    store.join("halo1-mp")
}

pub fn halo1_mp_stem(map: &str, sandbox: &str) -> String {
    format!("{map}_{sandbox}")
}

pub fn halo1_campaign_stem(map: &str, sandbox: &str) -> String {
    format!("halo1_campaign_{map}_{sandbox}")
}
