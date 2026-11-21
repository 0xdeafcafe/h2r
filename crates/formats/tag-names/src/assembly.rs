//! Reads the retail Xbox plugin format described in
//! `docs/editor/notes/tag-field-names.md`. The folder index reads root
//! headers only. Field names and comments are loaded on demand.
//!
//! Assembly opens a retail Halo 2 Xbox map with the plugins of
//! [`GAME`] and, for a group that folder has no plugin for, those of
//! [`FALLBACK`]. [`Folder`] indexes both the same way. [`Hints`] indexes
//! the plugins of other retail games, whose names are only hints: the
//! merge accepts one where the layout lines up ([`crate::hints`]).

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};

use quick_xml::Reader;

use crate::{Plugin, Source};

mod xml;
pub use xml::{parse, parse_game};

#[cfg(test)]
mod tests;

pub const GAME: &str = "Halo2Xbox";

/// The folder Assembly's engine table (`src/Blamite/Formats/Engines.xml`,
/// engine "Halo 2", build `02.09.27.09809`) names as the retail Xbox
/// build's `fallbackPlugins`: the retail Halo 2 Vista plugins, whose
/// roots say `game="Halo2"`. [`GAME`] holds only the groups whose Xbox
/// layout differs.
pub const FALLBACK: &str = "Halo2";

/// Folders of other retail games and editions. Each plugin's root names
/// the folder as its `game`. Their names are hints for our layout.
pub const HINTS: &[&str] = &[
    "Halo2MCC",
    "Halo2A",
    "Halo2AMCC",
    "Halo3",
    "Halo3MCC",
    "ODST",
    "ODSTMCC",
    "Reach",
    "ReachMCC",
    "Halo4",
    "Halo4MCC",
];

/// Folder names to reject before opening a plugin: pre-release builds and
/// projects derived from leaked builds. A refused name anywhere in a path
/// refuses it. The exact game attribute is also checked for every indexed
/// file, including files in a renamed directory.
pub const REFUSED: &[&str] = &[
    "Halo2Alpha",
    "Halo2Beta",
    "Halo2E3_04",
    "Halo2Epsilon",
    "Halo2XboxAlpha",
    "Halo2XboxBeta",
    "Halo2XboxEpsilon",
    "Halo3Alpha",
    "Halo3Beta",
    "Halo3PreAlpha",
    "ReachAlpha",
    "ReachBeta",
    "Halo4NetTest",
    "HaloOnline",
    "ElDewrito",
    "ManagedDonkey",
];

#[derive(Clone, Debug)]
pub struct Folder {
    dir: PathBuf,
    /// Group to its file and the game its root names.
    files: BTreeMap<String, (PathBuf, &'static str)>,
}

impl Folder {
    /// Select the retail child of a plugin tree, or index a flat folder of
    /// group XMLs. The tree's (or the flat folder's sibling) [`FALLBACK`]
    /// folder supplies the groups the retail folder lacks. No other sibling
    /// is enumerated.
    pub fn open(path: &Path) -> Result<Self, String> {
        refuse_build(path)?;
        let mut dir = fs::canonicalize(path).map_err(|e| at(path, e))?;
        refuse_build(&dir)?;
        let retail = dir.join(GAME);
        let tree = if retail.is_dir() {
            let tree = dir.clone();
            dir = fs::canonicalize(&retail).map_err(|e| at(&retail, e))?;
            refuse_build(&dir)?;
            Some(tree)
        } else if dir.file_name().and_then(|n| n.to_str()) == Some(GAME) {
            dir.parent().map(Path::to_path_buf)
        } else {
            None
        };

        let mut files = BTreeMap::new();
        for (group, path) in index(&dir, GAME)? {
            files.insert(group, (path, GAME));
        }
        if let Some(fallback) = tree.map(|t| t.join(FALLBACK)).filter(|d| d.is_dir()) {
            let fallback = fs::canonicalize(&fallback).map_err(|e| at(&fallback, e))?;
            refuse_build(&fallback)?;
            for (group, path) in index(&fallback, FALLBACK)? {
                files.entry(group).or_insert((path, FALLBACK));
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

    /// The game whose plugin supplies `group`: [`GAME`] or [`FALLBACK`].
    pub fn game(&self, group: &str) -> Option<&'static str> {
        self.files.get(group).map(|(_, game)| *game)
    }
}

impl Source for Folder {
    fn groups(&self) -> Vec<String> {
        self.files.keys().cloned().collect()
    }

    fn load(&self, group: &str) -> Result<Option<Plugin>, String> {
        let Some((path, game)) = self.files.get(group) else {
            return Ok(None);
        };
        load(group, path, game).map(Some)
    }
}

/// The other retail games' folders found beside the retail folder.
#[derive(Clone, Debug, Default)]
pub struct Hints {
    games: Vec<(&'static str, BTreeMap<String, PathBuf>)>,
}

impl Hints {
    /// Index every [`HINTS`] folder of a plugin tree (the folder that
    /// holds `Halo2Xbox`, or `Halo2Xbox` itself). A missing folder is
    /// skipped; a refused or unreadable one is an error.
    pub fn open(path: &Path) -> Result<Self, String> {
        refuse_build(path)?;
        let mut tree = fs::canonicalize(path).map_err(|e| at(path, e))?;
        if tree.file_name().and_then(|n| n.to_str()) == Some(GAME) {
            tree.pop();
        }
        refuse_build(&tree)?;
        let mut games = Vec::new();
        for &game in HINTS {
            let dir = tree.join(game);
            if !dir.is_dir() {
                continue;
            }
            let dir = fs::canonicalize(&dir).map_err(|e| at(&dir, e))?;
            refuse_build(&dir)?;
            games.push((game, index(&dir, game)?));
        }
        Ok(Self { games })
    }

    /// The games indexed, in [`HINTS`] order.
    pub fn games(&self) -> Vec<&'static str> {
        self.games.iter().map(|(game, _)| *game).collect()
    }

    /// Every hint plugin for `group`, in [`HINTS`] order.
    pub fn load(&self, group: &str) -> Result<Vec<Plugin>, String> {
        self.games
            .iter()
            .filter_map(|(game, files)| files.get(group).map(|path| load(group, path, game)))
            .collect()
    }
}

fn load(group: &str, path: &Path, game: &str) -> Result<Plugin, String> {
    let file = File::open(path).map_err(|e| at(path, e))?;
    xml::read(group, game, BufReader::new(file)).map_err(|e| at(path, e))
}

/// Group XMLs of one folder whose roots name `game`.
fn index(dir: &Path, game: &str) -> Result<BTreeMap<String, PathBuf>, String> {
    let mut files = BTreeMap::new();
    for entry in fs::read_dir(dir).map_err(|e| at(dir, e))? {
        let entry = entry.map_err(|e| at(dir, e))?;
        let path = entry.path();
        if !entry.file_type().map_err(|e| at(&path, e))?.is_file() {
            continue;
        }
        let Some(group) = file_group(&path) else {
            continue;
        };
        let file = File::open(&path).map_err(|e| at(&path, e))?;
        let mut reader = Reader::from_reader(BufReader::new(file));
        xml::header(&mut reader, game).map_err(|e| at(&path, e))?;
        if let Some(other) = files.insert(group.clone(), path) {
            return Err(format!(
                "two XML files for group {group} (including {})",
                other.display()
            ));
        }
    }
    Ok(files)
}

fn at(path: &Path, e: impl std::fmt::Display) -> String {
    format!("{}: {e}", path.display())
}

fn refuse_build(path: &Path) -> Result<(), String> {
    for component in path.iter().filter_map(|s| s.to_str()) {
        if REFUSED.iter().any(|b| b.eq_ignore_ascii_case(component)) {
            return Err(format!(
                "{component} is a pre-release or leak-derived folder; only retail plugins are read"
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
    let literal = tags::defs::parse_fourcc(group)?;
    windows_spelling(literal.0).or_else(|| Some(tags::defs::fourcc_text(literal)))
}

/// Characters a Windows file name cannot hold. Assembly writes each as
/// `_`, so `_fx_.xml` is the plugin for `<fx>`.
const NOT_IN_FILE_NAMES: &[u8] = b"<>:\"/\\|?*";

/// The schema group a file name spells with `_` for characters a file
/// name cannot hold, when the literal spelling is not a schema group and
/// exactly one schema group fits.
fn windows_spelling(name: [u8; 4]) -> Option<String> {
    let groups = &tags::schema::Schema::builtin().groups;
    if !name.contains(&b'_') || groups.keys().any(|g| g.0 == name) {
        return None;
    }
    let mut fits = groups.keys().filter(|g| {
        g.0.iter()
            .zip(name)
            .all(|(&c, n)| c == n || (n == b'_' && NOT_IN_FILE_NAMES.contains(&c)))
    });
    let first = fits.next()?;
    fits.next()
        .is_none()
        .then(|| tags::defs::fourcc_text(*first))
}
