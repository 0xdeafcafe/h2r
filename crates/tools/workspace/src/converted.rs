//! Converted levels in the tag store (docs/engine/asset-store.md, slice
//! AS10): a map a converter wrote (the Halo 1 import, the Map Pack
//! install) kept as a level of the store, found by its stem like a disc
//! level, instead of a whole map file under `<data>/maps/` or
//! `<data>/campaigns/`.
//!
//! [`store_converted`] takes the converter's map bytes and a stem. It
//! extracts the map's tags with the extractor the disc's maps go through
//! (`tags::repo::extract::store_map`): a tag the store already holds
//! byte for byte is not written again, a key held with other contents
//! becomes a variant that only this level reads, and a raw resource is kept
//! once by hash. Then it writes the level's skeleton and extent table
//! (`store_skeleton`) and checks that the store gives the map back byte for
//! byte. A `<stem>.converted` file in `<store>/maps/` is written last:
//!
//! ```text
//! converter <name>
//! file <SHA-256 of the map the converter wrote>
//! ```
//!
//! so a stem the disc's extraction made is never overwritten, and a
//! converted level whose file is the converter's current output is
//! recognised without reading the map ([`is_stored`]).
//!
//! Nothing falls back: a stem that is not a plain lower-case name, one that
//! a disc level holds, a map that does not read, a pointer into a shared
//! cache the caller cannot give, a tag the extractor refuses and a store
//! that does not give the map back are each an error that names the level
//! and what is missing, and no marker is written. Tags a refused map
//! already wrote stay (they are content, by hash); its manifest and header
//! are removed.
//!
//! Such a level assembles as any level of the store does
//! ([`crate::level::assemble`]): with no package over it the guest is
//! handed the stored file; with one, an image kept by its `LevelId`.
//! Extraction of the disc under a new extractor version empties the store's
//! `maps/` ([`tags::repo::TagRepo::reset`]), markers included, so
//! [`is_stored`] then says no and the converter's unit is converted again.
use std::io::Cursor;
use std::path::{Path, PathBuf};

use content::Sha256;
use map::{Map, ResourceFile};
use tags::repo::extract::{MapSource, SharedFiles, store_map, store_skeleton};
use tags::repo::skeleton::SkeletonReport;
use tags::repo::store::{PutReport, is_shared_cache};
use tags::repo::{StoreFile, TagRepo};
use tags::schema::Schema;

use crate::maps::BaseMaps;

/// Extension of a converted level's marker, in `<store>/maps/`.
pub const MARKER_EXT: &str = "converted";

/// The shared caches a map reads resources from, by file stem.
const SHARED: [(&str, ResourceFile); 3] = [
    ("mainmenu", ResourceFile::MainMenu),
    ("shared", ResourceFile::Shared),
    ("single_player_shared", ResourceFile::SinglePlayerShared),
];

/// How many refused tags an error names.
const NAMED: usize = 8;

/// What [`store_converted`] did.
#[derive(Debug, Clone)]
pub struct Converted {
    pub stem: String,
    /// The converter's map file: its length and SHA-256.
    pub len: u64,
    pub file: Sha256,
    /// The store already held this file for this stem: nothing was read or
    /// written.
    pub unchanged: bool,
    /// What storing the tags did: new, identical (already held), variants,
    /// lossy tags and the raw resources new to the store.
    pub put: PutReport,
    /// The level's skeleton and extents.
    pub skeleton: Option<SkeletonReport>,
}

/// A converted level in the store, as [`levels`] lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marked {
    pub stem: String,
    pub converter: String,
    /// The SHA-256 of the map the converter wrote (hex).
    pub file: String,
}

fn err(stem: &str, e: impl std::fmt::Display) -> String {
    format!("converted level {stem}: {e}")
}

/// `<store>/maps/<stem>.converted`.
pub fn marker_path(store: &Path, stem: &str) -> PathBuf {
    store.join("maps").join(format!("{stem}.{MARKER_EXT}"))
}

fn marker_text(converter: &str, file: &Sha256) -> String {
    format!("converter {converter}\nfile {file}\n")
}

fn parse_marker(stem: &str, text: &str) -> Option<Marked> {
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
        && StoreFile::extents(store, stem).is_ok_and(|e| e.file == want)
}

/// A stem the store can name a level by: lower case letters, digits and
/// `_`, and not a shared cache.
fn check_stem(stem: &str) -> Result<(), String> {
    if stem.is_empty()
        || !stem
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    {
        return Err(err(
            stem,
            "the stem must be lower case letters, digits and `_`",
        ));
    }
    if is_shared_cache(stem) {
        return Err(err(stem, "that stem is one of the disc's shared caches"));
    }
    Ok(())
}

/// Keep `bytes`, the map `converter` wrote, in the store at `store` as the
/// level `stem`. `bases` gives the shared caches the map's resources may
/// point into; a pointer into one it cannot give is an error naming the tag
/// (module docs).
pub fn store_converted(
    store: &Path,
    stem: &str,
    bytes: &[u8],
    bases: &dyn BaseMaps,
    converter: &str,
) -> Result<Converted, String> {
    check_stem(stem)?;
    if converter.is_empty() || converter.contains('\n') {
        return Err(err(
            stem,
            "the converter's name is empty or has a line break",
        ));
    }
    let file = Sha256::of(bytes);
    let len = bytes.len() as u64;
    if is_stored(store, stem, &file) {
        return Ok(Converted {
            stem: stem.to_owned(),
            len,
            file,
            unchanged: true,
            put: PutReport::default(),
            skeleton: None,
        });
    }
    let mut repo = TagRepo::create(store).map_err(|e| err(stem, e))?;
    let marker = marker_path(store, stem);
    if marked(store, stem).is_none() && StoreFile::exists(store, stem) {
        return Err(err(
            stem,
            format!(
                "the store at {} already holds a level of that name that no converter made",
                store.display()
            ),
        ));
    }
    // A stopped run leaves no marker: the level is converted again.
    match std::fs::remove_file(&marker) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(err(stem, format!("{}: {e}", marker.display()))),
    }
    let schema = Schema::builtin();
    let mut shared: SharedFiles<Cursor<Vec<u8>>> = SharedFiles::default();
    for (name, which) in SHARED {
        // A map that does not lean on a cache needs none of them; a tag
        // that does is refused below naming what it points into.
        let Ok(cache) = bases.read(name) else {
            continue;
        };
        let cache = Map::from_reader(Cursor::new(cache))
            .map_err(|e| err(stem, format!("the shared cache {name}: {e}")))?;
        shared
            .set(schema, which, cache)
            .map_err(|e| err(stem, format!("the shared cache {name}: {e}")))?;
    }
    let map = Map::from_reader(Cursor::new(bytes.to_vec()))
        .map_err(|e| err(stem, format!("the converter's output is not a map: {e}")))?;
    let mut src = MapSource::new(schema, map, &mut shared).map_err(|e| err(stem, e))?;
    let mut put = store_map(&mut repo, &mut src, stem, true).map_err(|e| err(stem, e))?;
    if !put.refused.is_empty() {
        let n = put.refused.len();
        let named: Vec<String> = put
            .refused
            .iter()
            .take(NAMED)
            .map(|(k, why)| format!("{k}: {why}"))
            .collect();
        let maps = store.join("maps");
        for ext in ["txt", "header"] {
            let _ = std::fs::remove_file(maps.join(format!("{stem}.{ext}")));
        }
        return Err(err(
            stem,
            format!(
                "{n} tag(s) did not come out of the map ({}{})",
                named.join("; "),
                if n > NAMED { "; ..." } else { "" }
            ),
        ));
    }
    let hex = file.to_string();
    put.skeleton = Some(store_skeleton(&mut repo, &mut src, stem, &hex).map_err(|e| err(stem, e))?);
    drop(src);
    repo.check_reassembly(stem, &hex)
        .map_err(|e| err(stem, e))?;
    repo.settle().map_err(|e| err(stem, e))?;
    content::store::write_atomic(&marker, marker_text(converter, &file).as_bytes())
        .map_err(|e| err(stem, format!("writing {}: {e}", marker.display())))?;
    let skeleton = put.skeleton.clone();
    Ok(Converted {
        stem: stem.to_owned(),
        len,
        file,
        unchanged: false,
        put,
        skeleton,
    })
}
