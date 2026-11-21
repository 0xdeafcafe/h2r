//! Reads the retail Xbox plugin format described in
//! `docs/editor/notes/tag-field-names.md`. The folder index reads root
//! headers only. Field names and comments are loaded on demand.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};

use quick_xml::Reader;

use crate::{Plugin, Source};

mod xml;
pub use xml::parse;

#[cfg(test)]
mod tests;

pub const GAME: &str = "Halo2Xbox";

/// Build directory names to reject before opening a plugin. The exact
/// game attribute is also checked for every indexed file, including files
/// in a renamed directory.
pub const REFUSED: &[&str] = &[
    "Halo2Alpha",
    "Halo2Beta",
    "Halo2E3_04",
    "Halo2Epsilon",
    "Halo2XboxAlpha",
    "Halo2XboxBeta",
    "Halo2XboxEpsilon",
    "Halo2Vista",
    "Halo2MCC",
    "Halo2AMP",
    "Halo3Beta",
    "Halo3",
    "Halo3ODST",
    "HaloReachBeta",
    "HaloReach",
    "Halo4",
    "HaloOnline",
];

#[derive(Clone, Debug)]
pub struct Folder {
    dir: PathBuf,
    files: BTreeMap<String, PathBuf>,
}

impl Folder {
    /// Select the retail child of a plugin tree, or index a flat folder of
    /// group XMLs. Siblings of the retail child are never enumerated.
    pub fn open(path: &Path) -> Result<Self, String> {
        refuse_build(path)?;
        let mut dir = fs::canonicalize(path).map_err(|e| at(path, e))?;
        refuse_build(&dir)?;
        let retail = dir.join(GAME);
        if retail.is_dir() {
            dir = fs::canonicalize(&retail).map_err(|e| at(&retail, e))?;
            refuse_build(&dir)?;
        }

        let mut files = BTreeMap::new();
        for entry in fs::read_dir(&dir).map_err(|e| at(&dir, e))? {
            let entry = entry.map_err(|e| at(&dir, e))?;
            let path = entry.path();
            if !entry.file_type().map_err(|e| at(&path, e))?.is_file() {
                continue;
            }
            let Some(group) = file_group(&path) else {
                continue;
            };
            let file = File::open(&path).map_err(|e| at(&path, e))?;
            let mut reader = Reader::from_reader(BufReader::new(file));
            xml::header(&mut reader).map_err(|e| at(&path, e))?;
            if let Some(other) = files.insert(group.clone(), path) {
                return Err(format!(
                    "two XML files for group {group} (including {})",
                    other.display()
                ));
            }
        }
        if files.is_empty() {
            return Err(at(&dir, "no retail Halo2Xbox group XML files"));
        }
        Ok(Self { dir, files })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

impl Source for Folder {
    fn groups(&self) -> Vec<String> {
        self.files.keys().cloned().collect()
    }

    fn load(&self, group: &str) -> Result<Option<Plugin>, String> {
        let Some(path) = self.files.get(group) else {
            return Ok(None);
        };
        let file = File::open(path).map_err(|e| at(path, e))?;
        xml::read(group, BufReader::new(file))
            .map(Some)
            .map_err(|e| at(path, e))
    }
}

fn at(path: &Path, e: impl std::fmt::Display) -> String {
    format!("{}: {e}", path.display())
}

fn refuse_build(path: &Path) -> Result<(), String> {
    for component in path.iter().filter_map(|s| s.to_str()) {
        if REFUSED.iter().any(|b| b.eq_ignore_ascii_case(component)) {
            return Err(format!(
                "{component} is a different build; only retail {GAME} plugins are supported"
            ));
        }
    }
    Ok(())
}

/// Literal punctuation is significant. A missing trailing space is filled
/// back in. Percent-escaped ASCII (e.g. %21) and the definition format's
/// eight hex digits can spell characters that are awkward in filenames.
fn file_group(path: &Path) -> Option<String> {
    if !path.extension()?.to_str()?.eq_ignore_ascii_case("xml") {
        return None;
    }
    let stem = path.file_stem()?.to_str()?;
    if !stem.is_ascii() {
        return None;
    }
    if stem.len() == 8 && stem.bytes().all(|c| c.is_ascii_hexdigit()) {
        return tags::defs::parse_fourcc(stem).map(tags::defs::fourcc_text);
    }

    let mut bytes = Vec::with_capacity(4);
    let mut pos = 0;
    while pos < stem.len() && bytes.len() < 4 {
        let byte = stem.as_bytes()[pos];
        pos += 1;
        if byte == b'%'
            && let Some(value) = stem.get(pos..pos + 2).and_then(|s| u8::from_str_radix(s, 16).ok())
        {
            bytes.push(value);
            pos += 2;
        } else {
            bytes.push(byte);
        }
    }
    if pos != stem.len() || bytes.is_empty() || !bytes.is_ascii() {
        return None;
    }
    bytes.resize(4, b' ');
    let group = std::str::from_utf8(&bytes).ok()?;
    tags::defs::parse_fourcc(group).map(tags::defs::fourcc_text)
}
