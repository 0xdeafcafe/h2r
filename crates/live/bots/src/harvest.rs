//! Proven spots and ghost runs from sim batch runs (docs/runtime/sim.md,
//! "Bounce practice"). A superbounce batch run plays on its own copy of
//! each map's verified and ghosts files, so no run sees another's proofs
//! and the runs of a sweep start alike. The batch keeps what a run began
//! with and what it ended with (`<run>/harvest/start`, `<run>/harvest/end`);
//! [`harvest_run`] merges what the run added into the shared bots
//! directory (`h2 sim harvest`).
//!
//! The files are the game's: `<map>.verified` (`superbounce::verified`,
//! written by `spots::store_verified`) and `<map>.ghosts`
//! (`superbounce::ghost`, written by the game's `superbounce_ghost`).

use std::path::Path;

use superbounce::ghost::{self, Ghost};
use superbounce::verified::{self, Entry};

/// `<map>.ghosts`, beside the spots and verified files (the game's
/// `superbounce_ghost::file_name`).
pub fn ghosts_file_name(map: &str) -> String {
    crate::cache::file_name(map, 0).replace(".0.bots", ".ghosts")
}

/// The files a run writes and keeps of its own: verified and ghosts.
pub fn run_owned(file_name: &str) -> bool {
    file_name.ends_with(".verified") || file_name.ends_with(".ghosts")
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
}

fn read_text(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn read_verified(path: &Path) -> Result<Vec<Entry>, String> {
    match read_text(path)? {
        Some(t) => verified::parse(&t).map_err(|e| format!("{}: {e}", path.display())),
        None => Ok(Vec::new()),
    }
}

fn read_ghosts(path: &Path) -> Result<Vec<Ghost>, String> {
    match read_text(path)? {
        Some(t) => ghost::parse(&t).map_err(|e| format!("{}: {e}", path.display())),
        None => Ok(Vec::new()),
    }
}

/// Merges one run's files into `into` (the shared bots directory): for
/// each `<map>.verified` in `end`, the entries it added over `start`'s
/// copy; for each `<map>.ghosts` in `end`, every run the shared file does
/// not beat (`ghost::offer`). With `write` false nothing is written: what
/// it would add. A damaged file is an error naming it, and nothing of that
/// file is written.
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
        let (map, is_verified) =
            match (name.strip_suffix(".verified"), name.strip_suffix(".ghosts")) {
                (Some(m), _) => (m.to_string(), true),
                (_, Some(m)) => (m.to_string(), false),
                _ => continue,
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
        if is_verified {
            let new = found(
                &read_verified(&start.join(&name))?,
                &read_verified(&end.join(&name))?,
            );
            if new.is_empty() {
                continue;
            }
            let shared = read_verified(&target)?;
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
    out.retain(|h| h.verified > 0 || h.ghosts > 0);
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
                ghosts: 0
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
        assert!(!run_owned("lockout.0.bots") && !run_owned("lockout.spots"));
        assert_eq!(ghosts_file_name("lockout"), "lockout.ghosts");
        let _ = std::fs::remove_dir_all(&root);
    }
}
