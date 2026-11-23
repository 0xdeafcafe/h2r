//! Proven spots and ghost runs from sim batch runs (docs/runtime/sim.md,
//! "Bounce practice"). A superbounce batch run plays on its own copy of
//! each map's verified and ghosts files, so no run sees another's proofs
//! and the runs of a sweep start alike. The batch keeps what a run began
//! with and what it ended with (`<run>/harvest/start`, `<run>/harvest/end`);
//! [`harvest_run`] merges what the run added into the shared bots
//! directory (`h2 sim harvest`).
//!
//! The files are the game's: `<map>.verified` (`superbounce::verified`,
//! written by `spots::store_verified`), `<map>.ghosts`
//! (`superbounce::ghost`, written by the game's `superbounce_ghost`) and
//! `<map>.learned` (`superbounce::learned`, written by a training run's
//! `spots::store_learned`; docs/game/bounce-search.md).

use std::path::Path;

use superbounce::ghost::{self, Ghost};
use superbounce::learned;
use superbounce::verified::{self, Entry};

/// `<map>.ghosts`, beside the spots and verified files (the game's
/// `superbounce_ghost::file_name`).
pub fn ghosts_file_name(map: &str) -> String {
    crate::cache::file_name(map, 0).replace(".0.bots", ".ghosts")
}

/// `<map>.learned`, beside the spots and verified files.
pub fn learned_file_name(map: &str) -> String {
    crate::cache::file_name(map, 0).replace(".0.bots", ".learned")
}

/// The files a run writes and keeps of its own: verified, ghosts and
/// learned.
pub fn run_owned(file_name: &str) -> bool {
    file_name.ends_with(".verified")
        || file_name.ends_with(".ghosts")
        || file_name.ends_with(".learned")
}

/// The entries of `end` that are not in `start`: what a run found. An
/// entry the run did not touch reads back the same, so it is not merged
/// again over what another run may have written since.
pub fn found(start: &[Entry], end: &[Entry]) -> Vec<Entry> {
    end.iter().filter(|e| !start.contains(e)).copied().collect()
}

/// What one map's harvest added.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Harvested {
    pub map: String,
    /// Verified entries merged in (proven or failed).
    pub verified: usize,
    /// Of those, proven.
    pub proven: usize,
    /// Ghost runs kept (new spots, or better than the run kept before).
    pub ghosts: usize,
    /// Learned lines kept (new sites, or better than the shared line,
    /// `learned::merge`).
    pub learned: usize,
}

fn read_text(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// A verified file's entries. One of an older version
/// (`verified::older`) is none: not converted, and replaced when the
/// shared file is written. Any other that does not read is an error.
fn read_verified(path: &Path) -> Result<Vec<Entry>, String> {
    match read_text(path)? {
        Some(t) if verified::older(&t).is_some() => Ok(Vec::new()),
        Some(t) => verified::parse(&t).map_err(|e| format!("{}: {e}", path.display())),
        None => Ok(Vec::new()),
    }
}

/// A ghosts file's runs, as [`read_verified`] reads a verified file: an
/// older version's is none, and replaced when the shared file is written.
fn read_ghosts(path: &Path) -> Result<Vec<Ghost>, String> {
    match read_text(path)? {
        Some(t) if ghost::older(&t).is_some() => Ok(Vec::new()),
        Some(t) => ghost::parse(&t).map_err(|e| format!("{}: {e}", path.display())),
        None => Ok(Vec::new()),
    }
}

fn read_learned(path: &Path) -> Result<Vec<learned::Entry>, String> {
    match read_text(path)? {
        Some(t) => learned::parse(&t).map_err(|e| format!("{}: {e}", path.display())),
        None => Ok(Vec::new()),
    }
}

/// The learned lines of `end` that are not in `start`: the sites a run
/// flew. A line the run did not touch reads back the same.
pub fn learned_found(start: &[learned::Entry], end: &[learned::Entry]) -> Vec<learned::Entry> {
    end.iter().filter(|e| !start.contains(e)).cloned().collect()
}

/// Which of a run's files a name is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Owned {
    Verified,
    Ghosts,
    Learned,
}

/// Merges one run's files into `into` (the shared bots directory): for
/// each `<map>.verified` in `end`, the entries it added over `start`'s
/// copy; for each `<map>.ghosts` in `end`, every run the shared file does
/// not beat (`ghost::offer`); for each `<map>.learned` in `end`, the lines
/// it changed over `start`'s copy, each kept where it beats the shared
/// line about its site (`learned::merge`). With `write` false nothing is
/// written: what it would add. A verified or ghosts file of an older
/// version reads as empty, so a shared one is replaced by what the run
/// adds. A damaged file is an error naming it, and nothing of that file
/// is written.
pub fn harvest_run(
    start: &Path,
    end: &Path,
    into: &Path,
    write: bool,
) -> Result<Vec<Harvested>, String> {
    let mut names: Vec<String> = match std::fs::read_dir(end) {
        Ok(d) => d
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| run_owned(n))
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(format!("{}: {e}", end.display())),
    };
    names.sort();
    let mut out: Vec<Harvested> = Vec::new();
    for name in names {
        let (map, kind) = if let Some(m) = name.strip_suffix(".verified") {
            (m.to_string(), Owned::Verified)
        } else if let Some(m) = name.strip_suffix(".ghosts") {
            (m.to_string(), Owned::Ghosts)
        } else if let Some(m) = name.strip_suffix(".learned") {
            (m.to_string(), Owned::Learned)
        } else {
            continue;
        };
        let i = match out.iter().position(|h| h.map == map) {
            Some(i) => i,
            None => {
                out.push(Harvested {
                    map: map.clone(),
                    ..Harvested::default()
                });
                out.len() - 1
            }
        };
        let target = into.join(&name);
        if kind == Owned::Learned {
            let shared = read_learned(&target)?;
            let new = learned_found(
                &read_learned(&start.join(&name))?,
                &read_learned(&end.join(&name))?,
            );
            let merged = learned::merge(&shared, &new);
            let kept = merged.iter().filter(|e| !shared.contains(e)).count();
            if kept == 0 {
                continue;
            }
            out[i].learned += kept;
            if write {
                std::fs::create_dir_all(into).map_err(|e| format!("{}: {e}", into.display()))?;
                crate::autonav::write_atomic(&target, learned::to_text(&merged).as_bytes())
                    .map_err(|e| format!("{}: {e}", target.display()))?;
            }
        } else if kind == Owned::Verified {
            let shared = read_verified(&target)?;
            // What the run found, less what the shared file already holds
            // (a run harvested twice adds nothing the second time).
            let new: Vec<Entry> = found(
                &read_verified(&start.join(&name))?,
                &read_verified(&end.join(&name))?,
            )
            .into_iter()
            .filter(|e| !shared.contains(e))
            .collect();
            if new.is_empty() {
                continue;
            }
            out[i].verified += new.len();
            out[i].proven += new
                .iter()
                .filter(|e| e.status == superbounce::Status::Proven)
                .count();
            if write {
                let merged = verified::merge(&shared, &new);
                std::fs::create_dir_all(into).map_err(|e| format!("{}: {e}", into.display()))?;
                crate::autonav::write_atomic(&target, verified::to_text(&merged).as_bytes())
                    .map_err(|e| format!("{}: {e}", target.display()))?;
            }
        } else {
            let mut shared = read_ghosts(&target)?;
            let mut kept = 0;
            for g in read_ghosts(&end.join(&name))? {
                if ghost::offer(&mut shared, g) {
                    kept += 1;
                }
            }
            if kept == 0 {
                continue;
            }
            out[i].ghosts += kept;
            if write {
                std::fs::create_dir_all(into).map_err(|e| format!("{}: {e}", into.display()))?;
                crate::autonav::write_atomic(&target, ghost::to_text(&shared).as_bytes())
                    .map_err(|e| format!("{}: {e}", target.display()))?;
            }
        }
    }
    out.retain(|h| h.verified > 0 || h.ghosts > 0 || h.learned > 0);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use superbounce::{Kind, Status};

    fn entry(at: [f32; 3], status: Status) -> Entry {
        Entry {
            kind: Kind::Bounce,
            status,
            at,
            trials: 3,
            ok: u32::from(status == Status::Proven),
            ..Entry::default()
        }
    }

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("bots-harvest-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn only_what_the_run_added_is_merged_over_the_shared_file() {
        let root = dir("verified");
        let (start, end, into) = (root.join("start"), root.join("end"), root.join("bots"));
        for d in [&start, &end, &into] {
            std::fs::create_dir_all(d).unwrap();
        }
        let old = entry([1.0, 0.0, 0.0], Status::Failed);
        let mine = entry([5.0, 0.0, 0.0], Status::Proven);
        // Since the run began, another run proved the old spot.
        let theirs = entry([1.0, 0.0, 0.0], Status::Proven);
        let file = "zanzibar.verified";
        std::fs::write(start.join(file), verified::to_text(&[old])).unwrap();
        std::fs::write(end.join(file), verified::to_text(&[old, mine])).unwrap();
        std::fs::write(into.join(file), verified::to_text(&[theirs])).unwrap();
        let dry = harvest_run(&start, &end, &into, false).unwrap();
        assert_eq!(
            dry,
            vec![Harvested {
                map: "zanzibar".into(),
                verified: 1,
                proven: 1,
                ghosts: 0,
                learned: 0
            }]
        );
        assert_eq!(
            read_verified(&into.join(file)).unwrap().len(),
            1,
            "a dry run writes nothing"
        );
        harvest_run(&start, &end, &into, true).unwrap();
        let shared = read_verified(&into.join(file)).unwrap();
        assert_eq!(shared.len(), 2);
        assert!(
            shared.iter().all(|e| e.status == Status::Proven),
            "{shared:?}"
        );
        // Again: nothing new.
        assert!(harvest_run(&start, &end, &into, false).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A shared verified file of an older version (v1: positions read at
    /// the bounding sphere's centre) is replaced by what a run adds; a
    /// damaged shared file of this version stops the harvest of that file
    /// and is not written over.
    #[test]
    fn an_older_shared_file_is_replaced_and_a_damaged_one_is_not() {
        let root = dir("older");
        let (start, end, into) = (root.join("start"), root.join("end"), root.join("bots"));
        for d in [&start, &end, &into] {
            std::fs::create_dir_all(d).unwrap();
        }
        let file = "zanzibar.verified";
        let v1 = "# superbounce verified v1\nbounce proven 1.000 0.000 0.400 trials=3 ok=1\n";
        // The run's start copy is the old file too: it reads as empty.
        std::fs::write(start.join(file), v1).unwrap();
        std::fs::write(into.join(file), v1).unwrap();
        let mine = entry([5.0, 0.0, 0.0], Status::Proven);
        std::fs::write(end.join(file), verified::to_text(&[mine])).unwrap();
        let got = harvest_run(&start, &end, &into, true).unwrap();
        assert_eq!((got.len(), got[0].verified), (1, 1));
        let text = std::fs::read_to_string(into.join(file)).unwrap();
        assert!(text.starts_with(verified::HEADER), "{text}");
        assert_eq!(read_verified(&into.join(file)).unwrap(), vec![mine]);

        let damaged = format!("{}\nbounce maybe 1 2 3\n", verified::HEADER);
        std::fs::write(into.join(file), &damaged).unwrap();
        let e = harvest_run(&start, &end, &into, true).unwrap_err();
        assert!(e.contains("line 2"), "{e}");
        assert_eq!(std::fs::read_to_string(into.join(file)).unwrap(), damaged);
        let _ = std::fs::remove_dir_all(&root);
    }

    fn line(at: [f32; 3], state: learned::State, tries: u32, hits: u32) -> learned::Entry {
        learned::Entry {
            state,
            at,
            tries,
            hits,
            ..learned::Entry::default()
        }
    }

    #[test]
    fn learned_lines_are_harvested_as_verified_ones_are() {
        use learned::State;
        let root = dir("learned");
        let (start, end, into) = (root.join("start"), root.join("end"), root.join("bots"));
        for d in [&start, &end, &into] {
            std::fs::create_dir_all(d).unwrap();
        }
        let file = "ascension.learned";
        // The run began with two sites; it flew one further and a new one,
        // and left the third as it found it.
        let a0 = line([1.0, 0.0, 0.0], State::Open, 12, 0);
        let a1 = line([1.0, 0.0, 0.0], State::Open, 30, 2);
        let b0 = line([2.0, 0.0, 0.0], State::Open, 6, 0);
        let c1 = line([3.0, 0.0, 0.0], State::Open, 12, 0);
        // Meanwhile another run learned site b, and flew site c further.
        let b_theirs = line([2.0, 0.0, 0.0], State::Learned, 40, 9);
        let c_theirs = line([3.0, 0.0, 0.0], State::Open, 50, 0);
        std::fs::write(
            start.join(file),
            learned::to_text(&[a0.clone(), b0.clone()]),
        )
        .unwrap();
        std::fs::write(
            end.join(file),
            learned::to_text(&[a1.clone(), b0.clone(), c1.clone()]),
        )
        .unwrap();
        std::fs::write(
            into.join(file),
            learned::to_text(&[a0.clone(), b_theirs.clone(), c_theirs.clone()]),
        )
        .unwrap();
        let dry = harvest_run(&start, &end, &into, false).unwrap();
        assert_eq!(dry.len(), 1);
        assert_eq!((dry[0].map.as_str(), dry[0].learned), ("ascension", 1));
        assert_eq!(
            read_learned(&into.join(file)).unwrap(),
            vec![a0, b_theirs.clone(), c_theirs.clone()],
            "a dry run writes nothing"
        );
        harvest_run(&start, &end, &into, true).unwrap();
        // Site a: the run's, further on. Site b: untouched by the run, the
        // shared learned line stays. Site c: the shared line has more tries.
        assert_eq!(
            read_learned(&into.join(file)).unwrap(),
            vec![a1, b_theirs, c_theirs]
        );
        // Twice adds nothing.
        assert!(harvest_run(&start, &end, &into, false).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_run_with_nothing_kept_harvests_nothing() {
        let root = dir("empty");
        assert!(
            harvest_run(
                &root.join("start"),
                &root.join("end"),
                &root.join("bots"),
                true
            )
            .unwrap()
            .is_empty()
        );
        assert!(run_owned("lockout.ghosts") && run_owned("lockout.verified"));
        assert!(run_owned("lockout.learned"));
        assert_eq!(learned_file_name("lockout"), "lockout.learned");
        assert!(!run_owned("lockout.0.bots") && !run_owned("lockout.spots"));
        assert_eq!(ghosts_file_name("lockout"), "lockout.ghosts");
        let _ = std::fs::remove_dir_all(&root);
    }
}
