//! What a run can play: the disc's movies checked against the installed
//! transcodes, without hashing anything large.
//!
//! A movie is *current* when the manifest has an entry for it whose source
//! size matches the disc file and whose output passes [`crate::locate`]'s
//! checks (name safety, regular file, recorded size, and the output hash or
//! its install-time stamp). Source hashes are the installer's job
//! ([`crate::install`] re-hashes the disc on every run); at boot the size
//! is the cheap tell for a transcode of another disc or an older layout.
use std::fs;
use std::io;
use std::path::Path;

use crate::{MANIFEST_NAME, Manifest};

/// A movie file on the disc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscMovie {
    /// Path on the disc, e.g. `/bink/intro_60.bik`.
    pub path: String,
    pub size: u64,
}

impl DiscMovie {
    /// The name the engine asks for: `/bink/intro_60.bik` -> `intro_60`.
    pub fn name(&self) -> String {
        stem(&self.path)
    }
}

fn stem(path: &str) -> String {
    let file = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let file = file.to_ascii_lowercase();
    file.strip_suffix(".bik").map(str::to_owned).unwrap_or(file)
}

/// The `.bik` files on a disc: an XISO image, or a directory holding an
/// extracted disc. Sorted by path.
pub fn disc_movies(disc: &Path) -> io::Result<Vec<DiscMovie>> {
    let mut out = Vec::new();
    if fs::metadata(disc)?.is_dir() {
        walk(disc, "", &mut out)?;
    } else {
        let mut iso = xiso::Xiso::open(io::BufReader::new(fs::File::open(disc)?))
            .map_err(io::Error::other)?;
        for e in crate::install::discover(&mut iso)? {
            out.push(DiscMovie {
                path: e.path,
                size: e.size as u64,
            });
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

fn walk(dir: &Path, prefix: &str, out: &mut Vec<DiscMovie>) -> io::Result<()> {
    for item in fs::read_dir(dir)? {
        let item = item?;
        let name = item.file_name().to_string_lossy().into_owned();
        let path = format!("{prefix}/{name}");
        let kind = item.file_type()?;
        if kind.is_dir() {
            walk(&item.path(), &path, out)?;
        } else if kind.is_file() && name.to_ascii_lowercase().ends_with(".bik") {
            out.push(DiscMovie {
                path,
                size: item.metadata()?.len(),
            });
        }
    }
    Ok(())
}

/// What is installed for a disc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    /// No manifest: `h2 movies` has not been run for this directory.
    NoneInstalled,
    /// These movies (engine names, e.g. `intro_60`) can be played. Others
    /// on the disc are not current and are skipped one by one.
    Current(Vec<String>),
    /// A manifest exists but nothing in it can be played for this disc.
    Stale(String),
}

/// Check `dir`'s manifest against the disc's movies. With `disc` empty (the
/// disc could not be listed) every entry with a valid output counts.
pub fn availability(dir: &Path, disc: &[DiscMovie]) -> Availability {
    let manifest = match Manifest::load(dir) {
        Ok(Some(m)) => m,
        Ok(None) => return Availability::NoneInstalled,
        Err(e) => return Availability::Stale(format!("{MANIFEST_NAME}: {e}")),
    };
    let mut current = Vec::new();
    let mut problem = None;
    if disc.is_empty() {
        for e in &manifest.movies {
            if manifest.verified_output(dir, e).is_some() {
                current.push(stem(&e.source_name));
            } else {
                problem.get_or_insert_with(|| {
                    format!("{} output is missing or changed", stem(&e.source_name))
                });
            }
        }
    }
    for m in disc {
        let name = m.name();
        match manifest.find(&m.path) {
            None => {
                problem.get_or_insert_with(|| format!("{name} not transcoded"));
            }
            Some(e) if e.source_size != m.size => {
                problem.get_or_insert_with(|| {
                    format!("{name} was transcoded from another disc (re-run `h2 movies`)")
                });
            }
            Some(e) if manifest.verified_output(dir, e).is_none() => {
                problem.get_or_insert_with(|| format!("{name} output is missing or changed"));
            }
            Some(_) => current.push(name),
        }
    }
    current.sort();
    current.dedup();
    match (current.is_empty(), problem) {
        (false, _) => Availability::Current(current),
        (true, Some(p)) => Availability::Stale(p),
        (true, None) => Availability::Stale("the manifest lists no movies".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MovieEntry;
    use std::path::PathBuf;

    fn scratch(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("h2-movies-status-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn entry(name: &str, source_size: u64, output: &[u8]) -> MovieEntry {
        MovieEntry {
            source_name: format!("/bink/{name}.bik"),
            source_size,
            source_sha256: "ab".repeat(32),
            output_name: format!("{name}.mp4"),
            output_size: output.len() as u64,
            duration_secs: 1.0,
            ffmpeg_version: "ffmpeg version test".into(),
            settings: "test".into(),
        }
    }

    fn install(dir: &Path, movies: &[(&str, u64)]) {
        let mut m = Manifest::default();
        for (name, size) in movies {
            let bytes = format!("{name} synthetic output");
            fs::write(dir.join(format!("{name}.mp4")), &bytes).unwrap();
            m.upsert(entry(name, *size, bytes.as_bytes()));
        }
        fs::write(dir.join(MANIFEST_NAME), m.to_json()).unwrap();
    }

    fn disc(movies: &[(&str, u64)]) -> Vec<DiscMovie> {
        movies
            .iter()
            .map(|(n, s)| DiscMovie {
                path: format!("/bink/{n}.bik"),
                size: *s,
            })
            .collect()
    }

    #[test]
    fn none_current_partial_and_stale() {
        let d = scratch("avail");
        let on_disc = disc(&[("attract_60", 10), ("credits_60", 20), ("intro_60", 30)]);
        assert_eq!(availability(&d, &on_disc), Availability::NoneInstalled);
        assert_eq!(
            availability(&d.join("absent"), &on_disc),
            Availability::NoneInstalled
        );

        install(
            &d,
            &[("attract_60", 10), ("credits_60", 20), ("intro_60", 30)],
        );
        assert_eq!(
            availability(&d, &on_disc),
            Availability::Current(vec![
                "attract_60".into(),
                "credits_60".into(),
                "intro_60".into()
            ])
        );

        // One output truncated: the other two still play.
        fs::write(d.join("intro_60.mp4"), b"short").unwrap();
        assert_eq!(
            availability(&d, &on_disc),
            Availability::Current(vec!["attract_60".into(), "credits_60".into()])
        );

        // Transcoded from another disc: every size differs.
        install(
            &d,
            &[("attract_60", 11), ("credits_60", 21), ("intro_60", 31)],
        );
        match availability(&d, &on_disc) {
            Availability::Stale(why) => assert!(why.contains("another disc"), "{why}"),
            other => panic!("{other:?}"),
        }

        // An unreadable manifest is stale, not "none".
        fs::write(d.join(MANIFEST_NAME), "{not json").unwrap();
        assert!(matches!(availability(&d, &on_disc), Availability::Stale(_)));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn without_a_disc_listing_valid_outputs_count() {
        let d = scratch("nodisc");
        install(&d, &[("intro_60", 30)]);
        assert_eq!(
            availability(&d, &[]),
            Availability::Current(vec!["intro_60".into()])
        );
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn extracted_disc_directories_are_listed() {
        let d = scratch("tree");
        fs::create_dir_all(d.join("bink")).unwrap();
        fs::write(d.join("bink/Intro_60.BIK"), [0u8; 7]).unwrap();
        fs::write(d.join("bink/readme.txt"), b"x").unwrap();
        fs::write(d.join("default.xbe"), b"x").unwrap();
        let found = disc_movies(&d).unwrap();
        assert_eq!(
            found,
            vec![DiscMovie {
                path: "/bink/Intro_60.BIK".into(),
                size: 7
            }]
        );
        assert_eq!(found[0].name(), "intro_60");
        let _ = fs::remove_dir_all(&d);
    }
}
