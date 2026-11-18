//! Transcoded intro/attract/credits movies: manifest and lookup.
//!
//! The disc's movies are Bink 1. We carry no Bink decoder; `h2 movies` runs
//! the user's own ffmpeg as an external process to convert them (see
//! docs/audio/movies.md). This crate is the other half: it describes what the
//! conversion wrote (`movies.json` next to the files), provides a resumable
//! app-data installer in [`install`], and lets a movie player
//! find the outputs and notice when they are stale. It does no decoding.
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub mod install;
mod sha256;
pub mod status;
pub use sha256::{Sha256, hex};

/// File name of the manifest inside the movie directory.
pub const MANIFEST_NAME: &str = "movies.json";
/// Bumped when the manifest layout or the encode settings change meaning.
pub const MANIFEST_VERSION: u32 = 1;

/// One converted movie.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MovieEntry {
    /// Path of the source on the disc, e.g. `/bink/intro_60.bik`.
    pub source_name: String,
    pub source_size: u64,
    /// Lowercase hex SHA-256 of the source bytes.
    pub source_sha256: String,
    /// File name of the output, relative to the movie directory.
    pub output_name: String,
    pub output_size: u64,
    pub duration_secs: f64,
    /// First line of `ffmpeg -version`, up to the copyright notice.
    pub ffmpeg_version: String,
    /// Encoder summary, e.g. `libx264 crf18 + aac 192k`.
    pub settings: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub movies: Vec<MovieEntry>,
    /// Output SHA-256 by file name. Absent in legacy CLI manifests.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub output_sha256: BTreeMap<String, String>,
    /// Size and mtime of each output when the installer last verified its
    /// hash. A matching stamp lets [`locate`] skip re-hashing the movie.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub output_stamp: BTreeMap<String, OutputStamp>,
}

/// File identity recorded alongside a verified output hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputStamp {
    pub size: u64,
    pub mtime_ns: u64,
}
impl OutputStamp {
    pub fn of(path: &Path) -> io::Result<Self> {
        let meta = fs::metadata(path)?;
        let mtime_ns = meta
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos() as u64;
        Ok(Self {
            size: meta.len(),
            mtime_ns,
        })
    }
}

impl Default for Manifest {
    fn default() -> Self {
        Self {
            version: MANIFEST_VERSION,
            movies: Vec::new(),
            output_sha256: BTreeMap::new(),
            output_stamp: BTreeMap::new(),
        }
    }
}

/// How a manifest entry relates to the disc it is checked against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    Fresh,
    /// The source size or hash differs from what was converted.
    SourceChanged,
}

impl Manifest {
    pub fn parse(json: &str) -> io::Result<Self> {
        let m: Manifest = serde_json::from_str(json)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        if m.version != MANIFEST_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "movie manifest version {} (expected {MANIFEST_VERSION}); re-run `h2 movies --force`",
                    m.version
                ),
            ));
        }
        Ok(m)
    }

    /// Validate an installer's output hash; legacy entries have no hash.
    pub fn output_matches(&self, dir: &Path, entry: &MovieEntry) -> io::Result<bool> {
        let Some(want) = self.output_sha256.get(&entry.output_name) else {
            return Ok(false);
        };
        if locate_entry(dir, entry).is_none() {
            return Ok(false);
        }
        Ok(install::hash_file(&dir.join(&entry.output_name))?.eq_ignore_ascii_case(want))
    }

    /// Path of a playable output. Manifests written by the installer carry
    /// output hashes and every entry must have one (a missing key is not a
    /// free pass); a size/mtime stamp from install time avoids re-hashing,
    /// and any other change falls back to a full hash. Legacy CLI manifests
    /// have no hashes and are checked by size alone.
    pub(crate) fn verified_output(&self, dir: &Path, entry: &MovieEntry) -> Option<PathBuf> {
        let path = locate_entry(dir, entry)?;
        if self.output_sha256.is_empty() {
            return Some(path);
        }
        let want = self.output_sha256.get(&entry.output_name)?;
        if self.output_stamp.get(&entry.output_name) == OutputStamp::of(&path).ok().as_ref() {
            return Some(path);
        }
        install::hash_file(&path)
            .ok()?
            .eq_ignore_ascii_case(want)
            .then_some(path)
    }

    pub fn to_json(&self) -> String {
        let mut s = serde_json::to_string_pretty(self).expect("manifest serialises");
        s.push('\n');
        s
    }

    /// Load `dir/movies.json`. `Ok(None)` when it does not exist.
    pub fn load(dir: &Path) -> io::Result<Option<Self>> {
        match fs::read_to_string(dir.join(MANIFEST_NAME)) {
            Ok(s) => Self::parse(&s).map(Some),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Replace the entry with the same `source_name`, or add it.
    pub fn upsert(&mut self, e: MovieEntry) {
        match self
            .movies
            .iter_mut()
            .find(|m| m.source_name.eq_ignore_ascii_case(&e.source_name))
        {
            Some(slot) => *slot = e,
            None => self.movies.push(e),
        }
        self.movies
            .sort_by(|a, b| a.source_name.cmp(&b.source_name));
    }

    /// Entry for a source by disc path (`/bink/intro_60.bik`), by bare file
    /// name (`intro_60.bik`) or by stem (`intro_60`); case-insensitive.
    pub fn find(&self, name: &str) -> Option<&MovieEntry> {
        let want = name.trim_start_matches('/').to_ascii_lowercase();
        self.movies.iter().find(|m| {
            let src = m.source_name.trim_start_matches('/').to_ascii_lowercase();
            let file = src.rsplit('/').next().unwrap_or(&src);
            let stem = file.rsplit_once('.').map_or(file, |(s, _)| s);
            want == src || want == file || want == stem
        })
    }
}

impl MovieEntry {
    pub fn freshness(&self, source_size: u64, source_sha256: &str) -> Freshness {
        if self.source_size == source_size && self.source_sha256.eq_ignore_ascii_case(source_sha256)
        {
            Freshness::Fresh
        } else {
            Freshness::SourceChanged
        }
    }
}

/// Find the playable file for a movie, e.g. `locate(dir, "intro_60")`.
///
/// Returns `None` when there is no manifest, no entry for `name`, or the
/// output file is missing or does not have the size the manifest recorded
/// (truncated or replaced). That is the normal "movies not installed" case:
/// the caller skips the movie. Installer outputs were hash-verified at install
/// time; a recorded size and mtime make this cheap, and a changed stamp falls
/// back to re-hashing the movie. This does not hash the disc; use
/// [`MovieEntry::freshness`] with the disc file's size and hash when you want
/// the staleness check too.
pub fn locate(dir: &Path, name: &str) -> Option<PathBuf> {
    let manifest = Manifest::load(dir).ok().flatten()?;
    let entry = manifest.find(name)?;
    manifest.verified_output(dir, entry)
}

/// Like [`locate`], but also requires the entry to match the disc file's
/// size and SHA-256 (hex), so a conversion of a different disc is not used.
pub fn locate_fresh(
    dir: &Path,
    name: &str,
    source_size: u64,
    source_sha256: &str,
) -> Option<PathBuf> {
    let manifest = Manifest::load(dir).ok().flatten()?;
    let entry = manifest.find(name)?;
    if entry.freshness(source_size, source_sha256) != Freshness::Fresh {
        return None;
    }
    manifest.verified_output(dir, entry)
}

/// A bare file name: no separators, no `..`, nothing absolute. The manifest
/// is a file on disk and must not be able to point the player elsewhere.
pub fn is_safe_output_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', ':', '\0'])
        && !Path::new(name).is_absolute()
}

fn locate_entry(dir: &Path, entry: &MovieEntry) -> Option<PathBuf> {
    if !is_safe_output_name(&entry.output_name) {
        return None;
    }
    let path = dir.join(&entry.output_name);
    let meta = fs::symlink_metadata(&path).ok()?;
    (meta.is_file() && meta.len() == entry.output_size).then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, size: u64) -> MovieEntry {
        MovieEntry {
            source_name: format!("/bink/{name}.bik"),
            source_size: 100,
            source_sha256: "ab".repeat(32),
            output_name: format!("{name}.mp4"),
            output_size: size,
            duration_secs: 12.5,
            ffmpeg_version: "ffmpeg version 7.1".into(),
            settings: "libx264 crf18 + aac 192k".into(),
        }
    }

    fn scratch(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("h2-movies-lib-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn json_round_trip_and_upsert() {
        let mut m = Manifest::default();
        m.upsert(entry("b", 1));
        m.upsert(entry("a", 2));
        m.upsert(entry("b", 3));
        assert_eq!(m.movies.len(), 2);
        assert_eq!(m.movies[0].output_name, "a.mp4");
        assert_eq!(m.find("b").unwrap().output_size, 3);
        assert_eq!(Manifest::parse(&m.to_json()).unwrap(), m);
    }

    #[test]
    fn rejects_other_versions_and_garbage() {
        assert!(Manifest::parse(r#"{"version":99,"movies":[]}"#).is_err());
        assert!(Manifest::parse("nope").is_err());
    }

    #[test]
    fn find_by_path_file_or_stem() {
        let mut m = Manifest::default();
        m.upsert(entry("intro_60", 1));
        for n in [
            "/bink/intro_60.bik",
            "bink/intro_60.bik",
            "INTRO_60.bik",
            "intro_60",
        ] {
            assert!(m.find(n).is_some(), "{n}");
        }
        assert!(m.find("credits_60").is_none());
    }

    #[test]
    fn freshness_tracks_size_and_hash() {
        let e = entry("a", 1);
        assert_eq!(e.freshness(100, &"AB".repeat(32)), Freshness::Fresh);
        assert_eq!(e.freshness(101, &"ab".repeat(32)), Freshness::SourceChanged);
        assert_eq!(e.freshness(100, &"cd".repeat(32)), Freshness::SourceChanged);
    }

    #[test]
    fn locate_needs_manifest_file_and_matching_size() {
        let d = scratch("locate");
        assert!(locate(&d, "a").is_none()); // no manifest
        let mut m = Manifest::default();
        m.upsert(entry("a", 4));
        fs::write(d.join(MANIFEST_NAME), m.to_json()).unwrap();
        assert!(locate(&d, "a").is_none()); // output missing
        fs::write(d.join("a.mp4"), b"123").unwrap();
        assert!(locate(&d, "a").is_none()); // wrong size
        fs::write(d.join("a.mp4"), b"1234").unwrap();
        assert_eq!(locate(&d, "a"), Some(d.join("a.mp4")));
        assert!(locate(&d, "zzz").is_none());
    }

    #[test]
    fn locate_trusts_a_matching_stamp_and_rehashes_otherwise() {
        let d = scratch("stamp");
        let path = d.join("a.mp4");
        fs::write(&path, b"1234").unwrap();
        let mut m = Manifest::default();
        m.upsert(entry("a", 4));
        // Wrong hash, matching stamp: trusted without hashing.
        m.output_sha256.insert("a.mp4".into(), "00".repeat(32));
        m.output_stamp
            .insert("a.mp4".into(), OutputStamp::of(&path).unwrap());
        fs::write(d.join(MANIFEST_NAME), m.to_json()).unwrap();
        assert!(locate(&d, "a").is_some());
        // Same size, new mtime: the stamp is stale, so the wrong hash shows.
        let file = fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(60))
            .unwrap();
        assert!(locate(&d, "a").is_none());
        // A correct hash survives a stale stamp.
        let right = hex(&{
            let mut h = Sha256::default();
            h.update(b"1234");
            h.finish()
        });
        m.output_sha256.insert("a.mp4".into(), right);
        fs::write(d.join(MANIFEST_NAME), m.to_json()).unwrap();
        assert!(locate(&d, "a").is_some());
    }

    #[test]
    fn missing_hash_key_is_not_a_free_pass_in_a_hashed_manifest() {
        let d = scratch("nokey");
        fs::write(d.join("a.mp4"), b"1234").unwrap();
        fs::write(d.join("b.mp4"), b"1234").unwrap();
        let mut m = Manifest::default();
        m.upsert(entry("a", 4));
        m.upsert(entry("b", 4));
        m.output_sha256.insert("a.mp4".into(), "00".repeat(32));
        fs::write(d.join(MANIFEST_NAME), m.to_json()).unwrap();
        assert!(locate(&d, "b").is_none());
    }

    #[test]
    fn locate_rejects_escaping_output_names() {
        let d = scratch("escape");
        fs::write(d.join("secret"), b"1234").unwrap();
        for bad in [
            "../secret",
            "sub/secret",
            "/etc/passwd",
            "..",
            ".",
            "",
            "a\\b",
            "c:x",
        ] {
            let mut e = entry("a", 4);
            e.output_name = bad.into();
            let mut m = Manifest::default();
            m.upsert(e);
            fs::write(d.join(MANIFEST_NAME), m.to_json()).unwrap();
            assert!(locate(&d, "a").is_none(), "{bad:?}");
        }
        assert!(is_safe_output_name("intro_60.mp4"));
    }

    #[test]
    fn locate_fresh_checks_source() {
        let d = scratch("fresh");
        let mut m = Manifest::default();
        m.upsert(entry("a", 4));
        fs::write(d.join(MANIFEST_NAME), m.to_json()).unwrap();
        fs::write(d.join("a.mp4"), b"1234").unwrap();
        assert!(locate_fresh(&d, "a", 100, &"ab".repeat(32)).is_some());
        assert!(locate_fresh(&d, "a", 100, &"cd".repeat(32)).is_none());
        assert!(locate_fresh(&d, "a", 7, &"ab".repeat(32)).is_none());
    }
}
